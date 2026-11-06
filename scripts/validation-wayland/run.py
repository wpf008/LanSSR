#!/usr/bin/env python3
"""Build and run genuine Wayland wire-protocol regression fixtures offline.

Run in the Linux ARM64 builder. No compositor, network or display is required.
Both test process launch and capture have explicit deadlines.
"""
import argparse
from contextlib import contextmanager
from itertools import product
import os
from pathlib import Path
import select
import shlex
import subprocess
import tempfile


DIRECTORY = Path(__file__).resolve().parent
ROOT = DIRECTORY.parents[1]


def execute(arguments, **options):
    print("+ " + shlex.join(map(str, arguments)), flush=True)
    return subprocess.run(list(map(str, arguments)), check=True, **options)


def find_protocol(registry, package, relative):
    candidates = sorted(registry.glob("*/" + package + "/" + relative))
    if not candidates:
        raise RuntimeError("Protocol XML is absent from the offline Cargo cache: " + package)
    return candidates[-1]


def build_server(build):
    cargo_home = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")))
    registry = cargo_home / "registry" / "src"
    sources = [
        ("xdg-output", find_protocol(registry, "wayland-protocols-0.32.*",
                                   "protocols/unstable/xdg-output/xdg-output-unstable-v1.xml")),
        ("screencopy", find_protocol(registry, "wayland-protocols-wlr-0.3.*",
                                   "wlr-protocols/unstable/wlr-screencopy-unstable-v1.xml")),
    ]
    generated = []
    for name, source in sources:
        header = build / (name + "-server.h")
        implementation = build / (name + "-protocol.c")
        execute(["wayland-scanner", "server-header", source, header])
        execute(["wayland-scanner", "private-code", source, implementation])
        generated.append(implementation)
    flags = shlex.split(subprocess.check_output(
        ["pkg-config", "--cflags", "--libs", "wayland-server"], text=True))
    server = build / "legacy-server"
    execute(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-I" + str(build),
             DIRECTORY / "legacy-server.c", *generated, *flags, "-o", server])
    return server


def build_probe():
    manifest = DIRECTORY / "probe" / "Cargo.toml"
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target" / "linux-arm64"))
    execute(["cargo", "build", "--locked", "--offline", "--manifest-path", manifest,
             "--target-dir", target])
    return target / "debug" / "lanbridge-wayland-legacy-probe"


@contextmanager
def xcb_display():
    # Allocation through Xvfb's displayfd avoids colliding with the separate
    # installed-app GUI smoke test or any user's running desktop.
    process = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0",
                                "1280x720x24", "-nolisten", "tcp", "-ac"],
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        if not select.select([process.stdout], [], [], 8)[0]:
            raise RuntimeError("Private Xvfb did not become ready")
        number = process.stdout.readline().strip()
        if not number.isdigit():
            raise RuntimeError("Private Xvfb returned an invalid display number")
        yield ":" + number
    finally:
        process.terminate()
        _, stderr = process.communicate(timeout=3)
        if stderr:
            print("XVFB: " + stderr, end="", flush=True)


def run_case(server, probe, build, output, xdg, copy, failure="none", no_outputs=False):
    socket = "legacy-test"
    server_arguments = [str(server), socket, str(output), str(xdg), str(copy),
                        "1" if no_outputs else "0", failure]
    label = f"wl_output={output} xdg-output={xdg} screencopy={copy} failure={failure} no_outputs={no_outputs}"
    print("CASE " + label, flush=True)
    with tempfile.TemporaryDirectory(prefix="wayland-runtime-", dir=build) as runtime:
        os.chmod(runtime, 0o700)
        environment = dict(os.environ, XDG_RUNTIME_DIR=runtime, WAYLAND_DISPLAY=socket,
                           XDG_SESSION_TYPE="wayland")
        process = subprocess.Popen(server_arguments, env=environment, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, text=True)
        try:
            if not select.select([process.stdout], [], [], 5)[0] or process.stdout.readline().strip() != "READY":
                raise RuntimeError("Wayland fixture failed to become ready: " + label)
            expected = ("missing-xdg" if xdg == 0 else
                        "no-outputs" if no_outputs else
                        "missing-copy" if copy == 0 else
                        "capture-fails" if failure in ["early", "copy"] else
                        "unknown-format" if failure == "unknown" else "success")
            result = subprocess.run([str(probe), expected, str(output), str(xdg)],
                                    env=environment, text=True, capture_output=True, timeout=8)
            print(result.stdout, end="", flush=True)
            if result.returncode:
                print(result.stderr, end="", flush=True)
                raise RuntimeError(f"Wayland probe exited {result.returncode}: " + label)
            if process.poll() is not None:
                raise RuntimeError("Fixture server exited unexpectedly: " + label)
        finally:
            process.terminate()
            _, stderr = process.communicate(timeout=3)
            if stderr:
                print("SERVER: " + stderr, end="", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build-probe", action="store_true")
    parser.add_argument("--probe", type=Path)
    parser.add_argument("--log", type=Path)
    arguments = parser.parse_args()
    if arguments.log:
        # Shell-free tee keeps all commands, assertions and server diagnostics.
        import sys
        arguments.log.parent.mkdir(parents=True, exist_ok=True)
        log = arguments.log.open("w")

        class Tee:
            def write(self, content):
                sys.__stdout__.write(content)
                log.write(content)
            def flush(self):
                sys.__stdout__.flush()
                log.flush()

        sys.stdout = Tee()
    probe = arguments.probe
    if not arguments.skip_build_probe:
        probe = build_probe()
    elif probe is None:
        target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target" / "linux-arm64"))
        probe = target / "debug" / "lanbridge-wayland-legacy-probe"
    probe = probe.resolve()
    with tempfile.TemporaryDirectory(prefix="lanbridge-wayland-") as directory, xcb_display() as display:
        os.environ["DISPLAY"] = display
        build = Path(directory)
        server = build_server(build)
        count = 0
        for output, xdg, copy in product([1, 2, 4], [1, 2, 3], [1, 2, 3]):
            if output == 1 and xdg == 3:
                # xdg-output v3's completion event is wl_output.done (since v2).
                continue
            run_case(server, probe, build, output, xdg, copy)
            count += 1
        for output in [1, 2, 4]:
            run_case(server, probe, build, output, 0, 1)
            run_case(server, probe, build, output, 2, 1, no_outputs=True)
            run_case(server, probe, build, output, 2, 0)
            count += 3
        for copy in [1, 2, 3]:
            for failure in ["early", "copy"]:
                run_case(server, probe, build, 2, 2, copy, failure)
                count += 1
            for mode in ["invert", "unknown"]:
                run_case(server, probe, build, 2, 2, copy, mode)
                count += 1
        print(f"PASS all {count} Wayland protocol regression cases", flush=True)


if __name__ == "__main__":
    main()
