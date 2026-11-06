#!/usr/bin/env python3
"""Verify installed GUI clients under Xvfb, using only Python stdlib + libX11.

Example (inside the Linux build container after package installation):
    python3 scripts/smoke-linux-gui.py --log dist/linux-arm64/gui-smoke.log

Requires Xvfb and Mesa's software OpenGL driver. This checks actual viewable
windows and process survival; it does not exercise screen file transmission.
"""

import argparse
import ctypes as C
import ctypes.util
import os
from pathlib import Path
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import time


class WindowAttributes(C.Structure):
    # XWindowAttributes from Xlib.h, including fields after map_state so Xlib
    # never writes beyond the allocated structure on 32-bit or 64-bit hosts.
    _fields_ = [
        ("x", C.c_int), ("y", C.c_int), ("width", C.c_int),
        ("height", C.c_int), ("border_width", C.c_int), ("depth", C.c_int),
        ("visual", C.c_void_p), ("root", C.c_ulong), ("window_class", C.c_int),
        ("bit_gravity", C.c_int), ("win_gravity", C.c_int),
        ("backing_store", C.c_int), ("backing_planes", C.c_ulong),
        ("backing_pixel", C.c_ulong), ("save_under", C.c_int),
        ("colormap", C.c_ulong), ("map_installed", C.c_int),
        ("map_state", C.c_int), ("all_event_masks", C.c_long),
        ("your_event_mask", C.c_long), ("do_not_propagate_mask", C.c_long),
        ("override_redirect", C.c_int), ("screen", C.c_void_p),
    ]


class X11:
    def __init__(self, display_name):
        library = ctypes.util.find_library("X11")
        if not library:
            raise RuntimeError("libX11 is unavailable")
        self.lib = C.CDLL(library)
        signatures = {
            "XOpenDisplay": ([C.c_char_p], C.c_void_p),
            "XCloseDisplay": ([C.c_void_p], C.c_int),
            "XDefaultRootWindow": ([C.c_void_p], C.c_ulong),
            "XQueryTree": ([C.c_void_p, C.c_ulong, C.POINTER(C.c_ulong),
                            C.POINTER(C.c_ulong), C.POINTER(C.POINTER(C.c_ulong)),
                            C.POINTER(C.c_uint)], C.c_int),
            "XFetchName": ([C.c_void_p, C.c_ulong, C.POINTER(C.c_void_p)], C.c_int),
            "XGetWindowAttributes": ([C.c_void_p, C.c_ulong,
                                      C.POINTER(WindowAttributes)], C.c_int),
            "XInternAtom": ([C.c_void_p, C.c_char_p, C.c_int], C.c_ulong),
            "XGetWindowProperty": ([C.c_void_p, C.c_ulong, C.c_ulong,
                                    C.c_long, C.c_long, C.c_int, C.c_ulong,
                                    C.POINTER(C.c_ulong), C.POINTER(C.c_int),
                                    C.POINTER(C.c_ulong), C.POINTER(C.c_ulong),
                                    C.POINTER(C.c_void_p)], C.c_int),
            "XFree": ([C.c_void_p], C.c_int),
        }
        for name, (arguments, result) in signatures.items():
            function = getattr(self.lib, name)
            function.argtypes = arguments
            function.restype = result
        self.display = self.lib.XOpenDisplay(display_name.encode())
        if not self.display:
            raise RuntimeError("Cannot open X11 display " + display_name)
        # A window can be destroyed between querying the tree and attributes.
        # Suppress the default fatal handler; failed queries are skipped.
        callback = C.CFUNCTYPE(C.c_int, C.c_void_p, C.c_void_p)
        self.error_handler = callback(lambda _display, _event: 0)
        self.lib.XSetErrorHandler.argtypes = [callback]
        self.lib.XSetErrorHandler.restype = C.c_void_p
        self.lib.XSetErrorHandler(self.error_handler)
        self.pid_atom = self.lib.XInternAtom(self.display, b"_NET_WM_PID", 0)

    def close(self):
        if self.display:
            self.lib.XCloseDisplay(self.display)
            self.display = None

    def children(self, window):
        root, parent = C.c_ulong(), C.c_ulong()
        children, count = C.POINTER(C.c_ulong)(), C.c_uint()
        if not self.lib.XQueryTree(self.display, window, C.byref(root),
                                  C.byref(parent), C.byref(children), C.byref(count)):
            return []
        try:
            return list(children[:count.value]) if children else []
        finally:
            if children:
                self.lib.XFree(children)

    def title(self, window):
        value = C.c_void_p()
        if not self.lib.XFetchName(self.display, window, C.byref(value)) or not value:
            return ""
        try:
            return C.string_at(value).decode("utf-8", errors="replace")
        finally:
            self.lib.XFree(value)

    def pid(self, window):
        actual_type, count, remaining = C.c_ulong(), C.c_ulong(), C.c_ulong()
        actual_format, value = C.c_int(), C.c_void_p()
        status = self.lib.XGetWindowProperty(
            self.display, window, self.pid_atom, 0, 1, 0, 6,
            C.byref(actual_type), C.byref(actual_format), C.byref(count),
            C.byref(remaining), C.byref(value))
        try:
            if status == 0 and actual_type.value == 6 and actual_format.value == 32 and count.value == 1 and value:
                return C.cast(value, C.POINTER(C.c_ulong))[0]
            return None
        finally:
            if value:
                self.lib.XFree(value)

    def windows(self):
        root = self.lib.XDefaultRootWindow(self.display)
        pending, seen, found = [(root, 0)], set(), []
        while pending and len(seen) < 4096:
            window, depth = pending.pop()
            if window in seen:
                continue
            seen.add(window)
            attributes = WindowAttributes()
            if self.lib.XGetWindowAttributes(self.display, window, C.byref(attributes)):
                found.append({"id": window, "title": self.title(window),
                              "pid": self.pid(window), "mapped": attributes.map_state == 2,
                              "size": (attributes.width, attributes.height)})
            if depth < 8:
                pending.extend((child, depth + 1) for child in self.children(window))
        return found


def stop_child(process):
    """Signal only the process group created for this script's own child."""
    if process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=5)


def start_xvfb(env, output, children):
    executable = shutil.which("Xvfb")
    if not executable:
        raise RuntimeError("Xvfb is missing; install xvfb and libgl1-mesa-dri")
    read_fd, write_fd = os.pipe()
    try:
        process = subprocess.Popen(
            [executable, "-displayfd", str(write_fd), "-screen", "0", "1920x1080x24",
             "-nolisten", "tcp", "-ac"], env=env, stdout=output, stderr=output,
            pass_fds=(write_fd,), start_new_session=True)
        children.append(process)
        os.close(write_fd)
        write_fd = None
        deadline, reply = time.monotonic() + 15, b""
        while b"\n" not in reply:
            if process.poll() is not None:
                raise RuntimeError("Xvfb exited with code " + str(process.returncode))
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeError("Xvfb did not report its allocated display")
            ready, _, _ = select.select([read_fd], [], [], min(remaining, 0.5))
            if ready:
                chunk = os.read(read_fd, 128)
                if not chunk:
                    raise RuntimeError("Xvfb display allocation pipe closed")
                reply += chunk
        number = reply.splitlines()[0].decode("ascii")
        if not number.isdigit():
            raise RuntimeError("Unexpected Xvfb display reply: " + repr(reply))
        return ":" + number
    finally:
        os.close(read_fd)
        if write_fd is not None:
            os.close(write_fd)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sender", default="LanSender", help="installed sender command/path")
    parser.add_argument("--receiver", default="LanReceiver", help="installed receiver command/path")
    parser.add_argument("--log", type=Path, default=Path("gui-smoke.log"))
    parser.add_argument("--timeout", type=float, default=30, help="window appearance timeout per app")
    parser.add_argument("--hold", type=float, default=3, help="survival time after both windows appear")
    parser.add_argument("--display", default=os.environ.get("DISPLAY"),
                        help="existing X11 display (defaults to DISPLAY); otherwise start private Xvfb")
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("Run this smoke check inside Linux")
    if args.timeout <= 0 or args.hold < 0:
        parser.error("--timeout must be positive and --hold must be nonnegative")
    args.log.parent.mkdir(parents=True, exist_ok=True)
    with args.log.open("w", encoding="utf-8") as report, tempfile.TemporaryDirectory(prefix="lanbridge-gui-smoke-") as temporary:
        def log(message):
            print(message, flush=True)
            report.write(message + "\n")
            report.flush()

        children, outputs, x11 = [], [], None
        try:
            env = os.environ.copy()
            env.pop("WAYLAND_DISPLAY", None)
            env.update({"XDG_SESSION_TYPE": "x11", "WINIT_UNIX_BACKEND": "x11",
                        "LIBGL_ALWAYS_SOFTWARE": "1"})
            xvfb_output = open(Path(temporary) / "Xvfb.log", "w+b")
            outputs.append(("Xvfb", xvfb_output))
            display = args.display or start_xvfb(env, xvfb_output, children)
            env["DISPLAY"] = display
            x11 = X11(display)
            baseline = {window["id"] for window in x11.windows()}
            log("Display: " + display + (" (existing)" if args.display else " (private Xvfb)"))
            apps, mapped = [], {}
            for title, command in (("LanSender", args.sender), ("LanReceiver", args.receiver)):
                executable = shutil.which(command)
                if not executable:
                    raise RuntimeError("Installed executable is missing: " + command)
                output = open(Path(temporary) / (title + ".log"), "w+b")
                outputs.append((title, output))
                process = subprocess.Popen([executable], env=env, cwd=temporary,
                                           stdout=output, stderr=output, start_new_session=True)
                children.append(process)
                apps.append((title, process))
                deadline = time.monotonic() + args.timeout
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        raise RuntimeError(title + " exited before mapping a window: " + str(process.returncode))
                    matches = [window for window in x11.windows() if window["title"] == title
                               and window["mapped"] and min(window["size"]) > 0
                               and (window["pid"] == process.pid or
                                    (window["pid"] is None and window["id"] not in baseline))]
                    if matches:
                        window = matches[0]
                        mapped[title] = window["id"]
                        log("PASS " + title + ": pid=" + str(process.pid) + " mapped window="
                            + hex(window["id"]) + " size=" + str(window["size"])
                            + " title=" + repr(window["title"]))
                        break
                    time.sleep(0.1)
                else:
                    raise RuntimeError(title + " did not map a viewable window within " + str(args.timeout) + " seconds")
            deadline = time.monotonic() + args.hold
            while time.monotonic() < deadline:
                for title, process in apps:
                    if process.poll() is not None:
                        raise RuntimeError(title + " exited during survival check: " + str(process.returncode))
                time.sleep(min(0.1, max(0, deadline - time.monotonic())))
            windows = {window["id"]: window for window in x11.windows()}
            for title, process in apps:
                if process.poll() is not None or not windows.get(mapped[title], {}).get("mapped"):
                    raise RuntimeError(title + " is no longer alive with a mapped window")
            log("PASS: both GUI windows remained viewable and both processes survived " + str(args.hold) + " seconds.")
            log("Scope: GUI launch only; no file-transfer or physical Kylin display verification.")
            return 0
        except Exception as error:
            log("FAIL: " + str(error))
            return 1
        finally:
            if x11 is not None:
                x11.close()
            for child in reversed(children):
                stop_child(child)
            for title, output in outputs:
                output.flush()
                output.seek(0)
                text = output.read().decode("utf-8", errors="replace").strip()
                if text:
                    log(title + " diagnostics:\n" + text[-8000:])
                output.close()
            log("Cleanup: only this script's child process groups were stopped.")


if __name__ == "__main__":
    raise SystemExit(main())
