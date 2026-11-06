#!/usr/bin/env python3
"""Inspect a portable Windows package without executing its programs.

Examples:
    python3 scripts/verify-windows.py --version 2.0.14
    python3 scripts/verify-windows.py dist/LanBridge-2.0.14-windows-x64.zip
    python3 scripts/verify-windows.py --bundle /path/to/bundle --zip /path/to/bundle.zip

The Windows 7 import policy excludes known newer DLLs and entry points. It is
not a complete authoritative list of all exports available on Windows 7.
"""

from __future__ import annotations

import argparse
import hashlib
import re
import struct
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from zipfile import BadZipFile, ZipFile


BINARY_FILES = (
    "LanSender.exe",
    "LanReceiver.exe",
)

PACKAGE_FILES = BINARY_FILES + (
    "LICENSE",
    "rfd-LICENSE",
    "egui_glow-LICENSE",
    "使用说明.txt",
)

# System libraries shipped with Windows 7. New libraries require a deliberate
# policy review even if another supported Windows version ships them.
WINDOWS7_DLLS = frozenset(
    {
        "advapi32.dll",
        "bcrypt.dll",
        "comctl32.dll",
        "comdlg32.dll",
        "crypt32.dll",
        "dwmapi.dll",
        "gdi32.dll",
        "imm32.dll",
        "kernel32.dll",
        "netapi32.dll",
        "ntdll.dll",
        "ole32.dll",
        "oleaut32.dll",
        "opengl32.dll",
        "psapi.dll",
        "rpcrt4.dll",
        "secur32.dll",
        "setupapi.dll",
        "shell32.dll",
        "shlwapi.dll",
        "user32.dll",
        "userenv.dll",
        "uxtheme.dll",
        "version.dll",
        "winmm.dll",
        "winspool.drv",
        "ws2_32.dll",
    }
)

# These entry points require Windows 8/8.1/10/11 or an optional Windows 7
# update. Reject them in regular and delay-import tables. Runtime discovery
# through GetProcAddress is intentionally outside static import inspection.
NEWER_WINDOWS_APIS = frozenset(
    {
        "AddDllDirectory",
        "AdjustWindowRectExForDpi",
        "AreDpiAwarenessContextsEqual",
        "CreateFile2",
        "DiscardVirtualMemory",
        "EnableMouseInPointer",
        "EnableNonClientDpiScaling",
        "GetAutoRotationState",
        "GetAwarenessFromDpiAwarenessContext",
        "GetCurrentPackageFamilyName",
        "GetCurrentPackageFullName",
        "GetCurrentThreadStackLimits",
        "GetDpiForMonitor",
        "GetDpiForSystem",
        "GetDpiForWindow",
        "GetHostNameW",
        "GetOverlappedResultEx",
        "GetPackageFamilyName",
        "GetPackageFullName",
        "GetPointerDevice",
        "GetPointerDeviceCursors",
        "GetPointerDeviceProperties",
        "GetPointerDeviceRects",
        "GetPointerDevices",
        "GetPointerFrameInfo",
        "GetPointerFrameInfoHistory",
        "GetPointerFramePenInfo",
        "GetPointerFramePenInfoHistory",
        "GetPointerFrameTouchInfo",
        "GetPointerFrameTouchInfoHistory",
        "GetPointerInfo",
        "GetPointerInfoHistory",
        "GetPointerPenInfo",
        "GetPointerPenInfoHistory",
        "GetPointerTouchInfo",
        "GetPointerTouchInfoHistory",
        "GetProcessDpiAwareness",
        "GetSystemMetricsForDpi",
        "GetSystemTimePreciseAsFileTime",
        "GetTempPath2A",
        "GetTempPath2W",
        "GetThreadDescription",
        "GetThreadDpiAwarenessContext",
        "GetThreadDpiHostingBehavior",
        "GetWindowDpiAwarenessContext",
        "IsValidDpiAwarenessContext",
        "OfferVirtualMemory",
        "PrefetchVirtualMemory",
        "ProcessPrng",
        "QueryInterruptTime",
        "QueryInterruptTimePrecise",
        "QueryUnbiasedInterruptTimePrecise",
        "ReclaimVirtualMemory",
        "RemoveDllDirectory",
        "SetCoalescableTimer",
        "SetDefaultDllDirectories",
        "SetProcessDpiAwareness",
        "SetProcessDpiAwarenessContext",
        "SetThreadDescription",
        "SetThreadDpiAwarenessContext",
        "SetThreadDpiHostingBehavior",
        "SystemParametersInfoForDpi",
        "VirtualAllocFromApp",
        "WaitOnAddress",
        "WakeByAddressAll",
        "WakeByAddressSingle",
    }
)


class VerificationError(Exception):
    """A package or PE check failed."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise VerificationError(message)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


@dataclass(frozen=True)
class ImportLibrary:
    name: str
    symbols: tuple[str, ...]


class PortableExecutable:
    """Small bounds-checked PE32/PE32+ reader for normal and delay imports."""

    def __init__(self, data: bytes):
        self.data = data
        require(data[:2] == b"MZ", "Missing DOS MZ header")
        pe_offset = self.unpack("<I", 0x3C)[0]
        require(self.read(pe_offset, 4) == b"PE\0\0", "Missing PE signature")
        self.machine, section_count = self.unpack("<HH", pe_offset + 4)
        optional_size = self.unpack("<H", pe_offset + 20)[0]
        optional = pe_offset + 24
        self.read(optional, optional_size)
        self.magic = self.unpack("<H", optional)[0]
        require(self.magic in (0x10B, 0x20B), "Unsupported PE optional header")
        self.is_64_bit = self.magic == 0x20B
        minimum_size = 112 if self.is_64_bit else 96
        require(optional_size >= minimum_size, "Truncated PE optional header")
        self.image_base = self.unpack(
            "<Q" if self.is_64_bit else "<I",
            optional + (24 if self.is_64_bit else 28),
        )[0]
        self.os_version = self.unpack("<HH", optional + 40)
        self.subsystem_version = self.unpack("<HH", optional + 48)
        self.headers_size = self.unpack("<I", optional + 60)[0]
        self.subsystem = self.unpack("<H", optional + 68)[0]
        self.sections: list[tuple[int, int, int, int]] = []
        section_table = optional + optional_size
        self.read(section_table, 40 * section_count)
        for index in range(section_count):
            virtual_size, virtual_address, raw_size, raw_offset = self.unpack(
                "<IIII", section_table + 40 * index + 8
            )
            self.read(raw_offset, raw_size)
            self.sections.append(
                (virtual_address, max(virtual_size, raw_size), raw_offset, raw_size)
            )
        directory_count = self.unpack("<I", optional + minimum_size - 4)[0]
        require(
            directory_count <= (optional_size - minimum_size) // 8,
            "PE data directories exceed the optional header",
        )
        directories = [
            self.unpack("<II", optional + minimum_size + 8 * index)
            for index in range(directory_count)
        ]
        self.regular_imports = self.imports(directories, 1, delay=False)
        self.delay_imports = self.imports(directories, 13, delay=True)

    def read(self, offset: int, size: int) -> bytes:
        require(
            offset >= 0 and size >= 0 and offset + size <= len(self.data),
            f"PE data outside the file: offset={offset}, size={size}",
        )
        return self.data[offset : offset + size]

    def unpack(self, format_string: str, offset: int) -> tuple[int, ...]:
        return struct.unpack(format_string, self.read(offset, struct.calcsize(format_string)))

    def location(self, rva: int, size: int = 1) -> int:
        require(rva >= 0, "Negative PE RVA")
        if rva < self.headers_size:
            require(rva + size <= self.headers_size, "RVA exceeds PE headers")
            self.read(rva, size)
            return rva
        for address, virtual_size, raw_offset, raw_size in self.sections:
            if address <= rva < address + virtual_size:
                delta = rva - address
                require(delta + size <= raw_size, "RVA has no data on disk")
                self.read(raw_offset + delta, size)
                return raw_offset + delta
        raise VerificationError(f"Unmapped PE RVA: 0x{rva:x}")

    def ascii_string(self, rva: int) -> str:
        offset = self.location(rva)
        limit = min(len(self.data), offset + 4096)
        end = self.data.find(b"\0", offset, limit)
        require(end >= 0, "Unterminated PE import string")
        self.location(rva, end - offset + 1)
        try:
            name = self.data[offset:end].decode("ascii")
        except UnicodeDecodeError as error:
            raise VerificationError("Non-ASCII PE import name") from error
        require(bool(name), "Empty PE import name")
        return name

    def thunks(self, table_rva: int, names_are_rva: bool = True) -> tuple[str, ...]:
        require(table_rva != 0, "Missing import name table")
        width = 8 if self.is_64_bit else 4
        format_string = "<Q" if self.is_64_bit else "<I"
        ordinal_mask = 1 << (width * 8 - 1)
        symbols = []
        for index in range(len(self.data) // width):
            value = self.unpack(
                format_string, self.location(table_rva + width * index, width)
            )[0]
            if value == 0:
                return tuple(sorted(symbols))
            if value & ordinal_mask:
                symbols.append(f"#{value & 0xFFFF}")
            else:
                name_rva = value if names_are_rva else value - self.image_base
                # The first two bytes are the import-name hint.
                self.location(name_rva, 2)
                symbols.append(self.ascii_string(name_rva + 2))
        raise VerificationError("Unterminated PE import name table")

    def imports(
        self,
        directories: list[tuple[int, int]],
        directory_index: int,
        *,
        delay: bool,
    ) -> tuple[ImportLibrary, ...]:
        if len(directories) <= directory_index:
            return ()
        table_rva, table_size = directories[directory_index]
        if table_rva == 0:
            require(table_size == 0, "Import directory has a size without an RVA")
            return ()
        descriptor_size = 32 if delay else 20
        require(table_size >= descriptor_size, "Truncated PE import directory")
        found = []
        for index in range(table_size // descriptor_size):
            descriptor = self.unpack(
                "<8I" if delay else "<5I",
                self.location(table_rva + descriptor_size * index, descriptor_size),
            )
            if not any(descriptor):
                return tuple(found)
            if delay:
                flags, name_address, _, iat_address, names_address, _, _, _ = descriptor
                require(flags in (0, 1), "Unsupported delay-import descriptor flags")
                names_are_rva = bool(flags & 1)
                convert = lambda address: address if names_are_rva else address - self.image_base
                name_rva = convert(name_address)
                names_rva = convert(names_address or iat_address)
            else:
                names_address, _, _, name_rva, iat_address = descriptor
                names_rva = names_address or iat_address
                names_are_rva = True
            found.append(
                ImportLibrary(
                    self.ascii_string(name_rva),
                    self.thunks(names_rva, names_are_rva),
                )
            )
        raise VerificationError("PE import directory has no terminating descriptor")


def import_failures(imports: tuple[ImportLibrary, ...], kind: str) -> list[str]:
    failures = []
    for library in imports:
        lower_name = library.name.lower()
        if re.match(r"(?:msvcp|msvcr|vcruntime|ucrtbase|api-ms-win-crt)", lower_name):
            failures.append(f"{kind}: dynamic Visual C++/UCRT dependency {library.name}")
        if lower_name not in WINDOWS7_DLLS:
            failures.append(f"{kind}: DLL outside the Windows 7 policy: {library.name}")
        for symbol in library.symbols:
            if symbol in NEWER_WINDOWS_APIS:
                failures.append(f"{kind}: newer required API {library.name}!{symbol}")
            if symbol.startswith("#"):
                failures.append(f"{kind}: ordinal import needs manual review: {library.name}!{symbol}")
    return failures


def check_pe(data: bytes) -> tuple[PortableExecutable, list[str]]:
    pe = PortableExecutable(data)
    failures = []
    if pe.machine != 0x8664 or not pe.is_64_bit:
        failures.append(f"Expected PE32+ AMD64; machine=0x{pe.machine:x}, magic=0x{pe.magic:x}")
    if pe.subsystem != 2:
        failures.append(f"Expected Windows GUI subsystem 2; got {pe.subsystem}")
    if pe.os_version > (6, 1):
        failures.append(f"PE OS version exceeds Windows 7: {pe.os_version}")
    if pe.subsystem_version > (6, 1):
        failures.append(f"PE subsystem version exceeds Windows 7: {pe.subsystem_version}")
    if not pe.regular_imports:
        failures.append("PE has no regular imports")
    failures.extend(import_failures(pe.regular_imports, "Regular import"))
    failures.extend(import_failures(pe.delay_imports, "Delay import"))
    return pe, failures


def describe_pe(name: str, data: bytes) -> tuple[list[str], list[str]]:
    pe, issues = check_pe(data)
    lines = [
        name,
        f"Size: {len(data)}",
        f"SHA256: {sha256(data)}",
        f"PE machine: 0x{pe.machine:x}; optional header: 0x{pe.magic:x}; subsystem: {pe.subsystem}",
        f"PE OS version: {pe.os_version[0]}.{pe.os_version[1]}",
        f"PE subsystem version: {pe.subsystem_version[0]}.{pe.subsystem_version[1]}",
    ]
    for kind, libraries in (("Regular", pe.regular_imports), ("Delay", pe.delay_imports)):
        lines.append(f"{kind} imports: {len(libraries)} DLLs, {sum(len(item.symbols) for item in libraries)} symbols")
        for library in libraries:
            lines += [f"  {library.name}", "    " + ", ".join(library.symbols)]
    lines.append("")
    return lines, [f"{name}: {issue}" for issue in issues]


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifact", nargs="?", type=Path, help="Portable ZIP or extracted bundle")
    parser.add_argument("--version", help="Package version; default: Cargo.toml")
    parser.add_argument("--target", default="x86_64-win7-windows-msvc", help="Release target directory")
    parser.add_argument("--project-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--bundle", type=Path, help="Override extracted bundle path")
    parser.add_argument("--zip", dest="archive", type=Path, help="Override portable ZIP path")
    parser.add_argument("--release-dir", type=Path, help="Override target/<target>/release directory")
    parser.add_argument("--report", type=Path, help="Output report path")
    parser.add_argument("--pe-only", action="store_true", help="Audit PE files only; artifact may be an exe, ZIP or directory")
    return parser.parse_args()


def verify(args: argparse.Namespace) -> tuple[list[str], list[str]]:
    root = args.project_root.resolve()
    version = args.version
    if version is None and args.artifact:
        match = re.fullmatch(r"LanBridge-(.+)-windows(?:7)?-x64(?:\.zip)?", args.artifact.name)
        if match:
            version = match.group(1)
    if version is None:
        version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
    bundle = args.bundle
    archive = args.archive
    if args.artifact:
        if args.artifact.suffix.lower() == ".zip":
            archive = archive or args.artifact
            bundle = bundle or args.artifact.with_suffix("")
        else:
            bundle = bundle or args.artifact
            archive = archive or Path(str(args.artifact) + ".zip")
    bundle = (bundle or root / "dist" / f"LanBridge-{version}-windows-x64").resolve()
    archive = (archive or Path(str(bundle) + ".zip")).resolve()
    release = (args.release_dir or root / "target" / args.target / "release").resolve()
    args.report = args.report or root / "dist" / f"validation-{version}" / "windows-build-report.txt"
    lines = [
        f"Windows package static verification {version}",
        f"Target: {args.target}",
        f"Bundle: {bundle}",
        f"Release directory: {release}",
        "Supported systems: Windows 7 SP1 / Windows 10 / Windows 11 x64",
        "Policy: Windows 7 system DLL allowlist and known newer required API denylist",
        "Policy limit: no complete authoritative Windows 7 API export allowlist was used.",
        "A pass excludes known static blockers; it does not prove all imported APIs or runtime behavior compatible.",
        "Regular and delay imports are listed in full below; GetProcAddress-loaded APIs are not statically audited.",
        "Execution on Windows 7 / 10 / 11 hardware: not performed by this verifier",
        "GPU/OpenGL driver capability and file transfer behavior: not tested by this verifier",
        "",
    ]
    if args.pe_only:
        source = args.artifact or args.bundle or release
        binaries = {}
        if source.suffix.lower() == ".exe":
            binaries[source.name] = source.read_bytes()
        elif source.suffix.lower() == ".zip":
            with ZipFile(source) as packaged:
                require(packaged.testzip() is None, "ZIP CRC check failed")
                for name in BINARY_FILES:
                    matches = [path for path in packaged.namelist() if Path(path).name == name]
                    require(len(matches) == 1, f"Expected exactly one {name} in ZIP")
                    binaries[name] = packaged.read(matches[0])
        else:
            binaries = {name: (source / name).read_bytes() for name in BINARY_FILES}
        failures = []
        lines.append("PE-only mode: package allowlist, licenses and release equality were not checked.")
        for name, data in binaries.items():
            details, issues = describe_pe(name, data)
            lines.extend(details)
            failures.extend(issues)
        return lines, failures
    require(bundle.is_dir(), f"Bundle directory does not exist: {bundle}")
    require(archive.is_file(), f"Portable ZIP does not exist: {archive}")
    require(
        {path.name for path in bundle.iterdir()} == set(PACKAGE_FILES),
        f"Bundle must contain exactly {', '.join(PACKAGE_FILES)}",
    )
    for name in PACKAGE_FILES:
        require((bundle / name).is_file(), f"Bundle entry is not a file: {name}")
        require(not (bundle / name).is_symlink(), f"Bundle entry is a symlink: {name}")
    with ZipFile(archive) as packaged:
        expected = {f"{bundle.name}/{name}" for name in PACKAGE_FILES}
        require(len(packaged.namelist()) == len(expected), "ZIP contains duplicate or extra entries")
        require(set(packaged.namelist()) == expected, "ZIP file allowlist mismatch")
        require(packaged.testzip() is None, "ZIP CRC check failed")
        for name in PACKAGE_FILES:
            require(
                packaged.read(f"{bundle.name}/{name}") == (bundle / name).read_bytes(),
                f"ZIP file differs from extracted bundle: {name}",
            )
    require((bundle / "LICENSE").read_bytes() == (root / "LICENSE").read_bytes(), "Project license differs")
    require(
        (bundle / "rfd-LICENSE").read_bytes() == (root / "vendor" / "rfd" / "LICENSE").read_bytes(),
        "Vendored rfd license differs",
    )
    require(
        (bundle / "egui_glow-LICENSE").read_bytes()
        == (root / "vendor" / "egui_glow" / "LICENSE-MIT").read_bytes(),
        "Vendored egui_glow license differs",
    )
    failures = []
    for name in BINARY_FILES:
        data = (bundle / name).read_bytes()
        require(data == (release / name).read_bytes(), f"Packaged executable differs from release: {name}")
        details, issues = describe_pe(name, data)
        lines.extend(details)
        failures.extend(issues)
    lines += [
        f"Portable ZIP: {archive}",
        f"Size: {archive.stat().st_size}",
        f"SHA256: {sha256(archive.read_bytes())}",
        "ZIP allowlist, CRC, extracted file equality, release binary equality and license equality: passed",
        "",
    ]
    return lines, failures


def main() -> int:
    args = arguments()
    try:
        lines, failures = verify(args)
    except (VerificationError, OSError, ValueError, struct.error, BadZipFile) as error:
        lines = ["Windows package static verification failed", f"Error: {error}"]
        failures = [str(error)]
    lines.append("Result: " + ("FAILED" if failures else "PASSED static package checks"))
    if failures:
        lines += ["Failures:", *(f"  {failure}" for failure in failures)]
    report = "\n".join(lines) + "\n"
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(report, encoding="utf-8")
        print(f"Report: {args.report}")
    if failures:
        for failure in failures:
            print(failure, file=sys.stderr)
        return 1
    print("Passed PE and known Windows 7 static import checks." if args.pe_only else "Passed ZIP, release equality, PE and known Windows 7 static import checks.")
    print("Windows 7 / 10 / 11 execution and complete API compatibility remain unverified.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
