#!/usr/bin/env python3
"""Capture and interact with an installed client in a disposable Xvfb session.

Uses only Python's standard library, libX11, and Xvfb (no Pillow, xdotool,
zenity, portal, or libXtst). Screenshots are lossless PPM images of the actual
native application window. Input uses the server's XTEST extension, including
the same mouse events winit normally receives through XInput2.

First capture the main window, inspect that image, then pass observed window-
relative coordinates to --actions. For example, inside a Linux build container:

    python3 scripts/inspect-linux-picker.py --output dist/picker-visual/sender
    python3 scripts/inspect-linux-picker.py --output dist/picker-visual/sender \
        --actions '[{"click":[100,150]},{"capture":"file-picker"}]'

An actions file is also accepted via --actions-file. Supported actions: click,
double_click, scroll [x,y,signed_steps], key (X11 keysym name), type (lowercase
ASCII text), resize [w,h],
wait (seconds), and capture (filename stem). Mouse coordinates must be based
on an inspected screenshot; the script deliberately does not guess controls.
Each run uses a private desktop with representative files and destroys only
its own processes. No file-transfer operation is started automatically.
"""

import argparse
import ctypes as C
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time


spec = importlib.util.spec_from_file_location(
    "linux_gui_smoke", Path(__file__).with_name("smoke-linux-gui.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class XImage(C.Structure):
    # We only read this prefix. XGetImage itself owns the complete structure.
    _fields_ = [
        ("width", C.c_int), ("height", C.c_int), ("xoffset", C.c_int),
        ("format", C.c_int), ("data", C.c_void_p), ("byte_order", C.c_int),
        ("bitmap_unit", C.c_int), ("bitmap_bit_order", C.c_int),
        ("bitmap_pad", C.c_int), ("depth", C.c_int),
        ("bytes_per_line", C.c_int), ("bits_per_pixel", C.c_int),
        ("red_mask", C.c_ulong), ("green_mask", C.c_ulong),
        ("blue_mask", C.c_ulong),
    ]


class FakeInputRequest(C.Structure):
    # xXTestFakeInputReq from X11/extensions/xtestproto.h.
    _fields_ = [
        ("reqType", C.c_uint8), ("xtReqType", C.c_uint8),
        ("length", C.c_uint16), ("type", C.c_uint8),
        ("detail", C.c_uint8), ("pad0", C.c_uint16),
        ("time", C.c_uint32), ("root", C.c_uint32),
        ("pad1", C.c_uint32), ("pad2", C.c_uint32),
        ("rootX", C.c_int16), ("rootY", C.c_int16),
        ("pad3", C.c_uint32), ("pad4", C.c_uint16),
        ("pad5", C.c_uint8), ("deviceid", C.c_uint8),
    ]


class InspectionX11(smoke.X11):
    def __init__(self, display_name):
        super().__init__(display_name)
        signatures = {
            "XGetImage": ([C.c_void_p, C.c_ulong, C.c_int, C.c_int,
                           C.c_uint, C.c_uint, C.c_ulong, C.c_int],
                          C.POINTER(XImage)),
            "XDestroyImage": ([C.POINTER(XImage)], C.c_int),
            "XTranslateCoordinates": ([C.c_void_p, C.c_ulong, C.c_ulong,
                                       C.c_int, C.c_int, C.POINTER(C.c_int),
                                       C.POINTER(C.c_int), C.POINTER(C.c_ulong)], C.c_int),
            "XSetInputFocus": ([C.c_void_p, C.c_ulong, C.c_int, C.c_ulong], C.c_int),
            "XRaiseWindow": ([C.c_void_p, C.c_ulong], C.c_int),
            "XResizeWindow": ([C.c_void_p, C.c_ulong, C.c_uint, C.c_uint], C.c_int),
            "XQueryExtension": ([C.c_void_p, C.c_char_p, C.POINTER(C.c_int),
                                 C.POINTER(C.c_int), C.POINTER(C.c_int)], C.c_int),
            "XLockDisplay": ([C.c_void_p], None),
            "XUnlockDisplay": ([C.c_void_p], None),
            "_XGetRequest": ([C.c_void_p, C.c_uint8, C.c_size_t], C.c_void_p),
            "XFlush": ([C.c_void_p], C.c_int),
            "XSync": ([C.c_void_p, C.c_int], C.c_int),
            "XStringToKeysym": ([C.c_char_p], C.c_ulong),
            "XKeysymToKeycode": ([C.c_void_p, C.c_ulong], C.c_uint8),
        }
        for name, (arguments, result) in signatures.items():
            function = getattr(self.lib, name)
            function.argtypes = arguments
            function.restype = result
        opcode, event_base, error_base = C.c_int(), C.c_int(), C.c_int()
        if not self.lib.XQueryExtension(self.display, b"XTEST", C.byref(opcode),
                                       C.byref(event_base), C.byref(error_base)):
            raise RuntimeError("This X server does not support the XTEST extension")
        self.xtest_opcode = opcode.value
        if C.sizeof(FakeInputRequest) != 36:
            raise RuntimeError("Unexpected XTEST request structure size")

    def geometry(self, window):
        attributes = smoke.WindowAttributes()
        if not self.lib.XGetWindowAttributes(self.display, window, C.byref(attributes)):
            raise RuntimeError("Application window is no longer available")
        root_x, root_y, child = C.c_int(), C.c_int(), C.c_ulong()
        root = self.lib.XDefaultRootWindow(self.display)
        if not self.lib.XTranslateCoordinates(self.display, window, root, 0, 0,
                                             C.byref(root_x), C.byref(root_y), C.byref(child)):
            raise RuntimeError("Cannot determine application's root coordinates")
        return {"x": root_x.value, "y": root_y.value,
                "width": attributes.width, "height": attributes.height}

    def capture(self, window, path):
        self.lib.XSync(self.display, 0)
        geometry = self.geometry(window)
        width, height = geometry["width"], geometry["height"]
        ximage = self.lib.XGetImage(self.display, window, 0, 0, width, height,
                                   C.c_ulong(-1).value, 2)  # ZPixmap
        if not ximage:
            raise RuntimeError("XGetImage failed for application window")
        try:
            info = ximage.contents
            if info.width != width or info.height != height or not info.data:
                raise RuntimeError("Invalid XImage data")
            raw = C.string_at(info.data, info.bytes_per_line * height)
            masks = (info.red_mask, info.green_mask, info.blue_mask)
            if info.bits_per_pixel == 32 and masks == (0xff0000, 0xff00, 0xff):
                rows = b"".join(raw[y * info.bytes_per_line:y * info.bytes_per_line + width * 4]
                                for y in range(height))
                rgb = bytearray(width * height * 3)
                channels = (2, 1, 0) if info.byte_order == 0 else (1, 2, 3)
                for channel, offset in enumerate(channels):
                    rgb[channel::3] = rows[offset::4]
            else:
                bytes_per_pixel = (info.bits_per_pixel + 7) // 8
                byte_order = "little" if info.byte_order == 0 else "big"
                shifts = [((mask & -mask).bit_length() - 1) for mask in masks]
                maxima = [mask >> shift for mask, shift in zip(masks, shifts)]
                rgb = bytearray()
                for y in range(height):
                    for x in range(width):
                        start = y * info.bytes_per_line + x * bytes_per_pixel
                        pixel = int.from_bytes(raw[start:start + bytes_per_pixel], byte_order)
                        rgb.extend(((pixel & mask) >> shift) * 255 // maximum
                                   for mask, shift, maximum in zip(masks, shifts, maxima))
            path.write_bytes(("P6\n%d %d\n255\n" % (width, height)).encode("ascii") + rgb)
            return geometry
        finally:
            self.lib.XDestroyImage(ximage)

    def fake_input(self, event_type, detail=0, root_x=0, root_y=0):
        # libXtst is absent in the offline builder. Queue the same 36-byte
        # protocol request through Xlib's existing buffered connection.
        self.lib.XLockDisplay(self.display)
        try:
            address = self.lib._XGetRequest(self.display, self.xtest_opcode, 36)
            if not address:
                raise RuntimeError("Cannot allocate an XTEST input request")
            request = C.cast(address, C.POINTER(FakeInputRequest)).contents
            request.xtReqType = 2  # X_XTestFakeInput
            request.type, request.detail = event_type, detail
            request.root = self.lib.XDefaultRootWindow(self.display)
            request.rootX, request.rootY = root_x, root_y
            request.time = 0
            request.pad0 = request.pad1 = request.pad2 = request.pad3 = 0
            request.pad4 = request.pad5 = request.deviceid = 0
        finally:
            self.lib.XUnlockDisplay(self.display)
        self.lib.XFlush(self.display)

    def focus(self, window):
        self.lib.XRaiseWindow(self.display, window)
        self.lib.XSetInputFocus(self.display, window, 1, 0)
        self.lib.XFlush(self.display)

    def move(self, window, x, y):
        geometry = self.geometry(window)
        if not 0 <= x < geometry["width"] or not 0 <= y < geometry["height"]:
            raise ValueError("Mouse coordinates are outside the captured window")
        self.focus(window)
        self.fake_input(6, root_x=geometry["x"] + x, root_y=geometry["y"] + y)
        time.sleep(0.15)

    def click(self, window, x, y, button=1):
        self.move(window, x, y)
        self.fake_input(4, button)
        time.sleep(0.07)
        self.fake_input(5, button)

    def key(self, window, name):
        keysym = self.lib.XStringToKeysym(name.encode("ascii"))
        code = self.lib.XKeysymToKeycode(self.display, keysym) if keysym else 0
        if not code:
            raise ValueError("Unknown or unmapped X11 keysym: " + name)
        self.focus(window)
        self.fake_input(2, code)
        time.sleep(0.07)
        self.fake_input(3, code)


def sample_desktop(directory, extra_files):
    directory.mkdir(parents=True)
    for name in ("项目文档", "设计素材", "接收文件"):
        (directory / name).mkdir()
    for name, size in (("产品说明.pdf", 180 * 1024), ("测试报告 2026-10.pdf", 920 * 1024),
                       ("项目资料.zip", 8 * 1024 * 1024), ("数据表.xlsx", 65 * 1024),
                       ("会议记录.txt", 2 * 1024),
                       ("名称较长的文件用于检查列表省略显示和选择功能.txt", 12 * 1024)):
        with (directory / name).open("wb") as output:
            output.truncate(size)
    (directory / ".hidden-file").write_text("hidden fixture", encoding="utf-8")
    for index in range(extra_files):
        (directory / ("列表滚动测试文件-%03d.txt" % (index + 1))).write_text(
            "scroll fixture %d\n" % index, encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", default="LanSender", help="installed application command/path")
    parser.add_argument("--title", help="expected native window title, defaults to executable name")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--timeout", type=float, default=30)
    parser.add_argument("--extra-files", type=int, default=0,
                        help="additional desktop files to exercise scrolling (0..2000)")
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--actions", default="[]", help="JSON array of observed-coordinate actions")
    group.add_argument("--actions-file", type=Path)
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("Run this inspection inside Linux")
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    if not 0 <= args.extra_files <= 2000:
        parser.error("--extra-files must be between 0 and 2000")
    actions = json.loads(args.actions_file.read_text(encoding="utf-8")
                         if args.actions_file else args.actions)
    if not isinstance(actions, list):
        parser.error("Actions must be a JSON array")
    executable = shutil.which(args.app)
    if not executable:
        parser.error("Application executable is unavailable: " + args.app)
    title = args.title or Path(executable).name
    args.output.mkdir(parents=True, exist_ok=True)
    records = []
    children, outputs, x11 = [], [], None
    with tempfile.TemporaryDirectory(prefix="lanssr-picker-visual-") as temporary:
        temporary = Path(temporary)
        try:
            config = temporary / "config"
            config.mkdir()
            desktop = temporary / "Desktop"
            sample_desktop(desktop, args.extra_files)
            (config / "user-dirs.dirs").write_text(
                'XDG_DESKTOP_DIR="%s"\n' % desktop, encoding="utf-8")
            env = os.environ.copy()
            env.pop("WAYLAND_DISPLAY", None)
            env.update({"XDG_SESSION_TYPE": "x11", "WINIT_UNIX_BACKEND": "x11",
                        "LIBGL_ALWAYS_SOFTWARE": "1", "XDG_CONFIG_HOME": str(config)})
            xvfb_output = (args.output / "Xvfb.log").open("w+b")
            outputs.append(xvfb_output)
            display = smoke.start_xvfb(env, xvfb_output, children)
            env["DISPLAY"] = display
            x11 = InspectionX11(display)
            app_output = (args.output / (title + ".log")).open("w+b")
            outputs.append(app_output)
            app = subprocess.Popen([executable], cwd=temporary, env=env,
                                   stdout=app_output, stderr=app_output, start_new_session=True)
            children.append(app)
            deadline = time.monotonic() + args.timeout
            while time.monotonic() < deadline:
                if app.poll() is not None:
                    raise RuntimeError(title + " exited before mapping: " + str(app.returncode))
                matches = [item for item in x11.windows() if item["title"] == title
                           and item["mapped"] and min(item["size"]) > 0
                           and item["pid"] in (None, app.pid)]
                if matches:
                    window = matches[0]["id"]
                    break
                time.sleep(0.1)
            else:
                raise RuntimeError("Application did not map a window before timeout")
            time.sleep(1)

            def capture(name):
                if not re.fullmatch(r"[A-Za-z0-9_-]+", name):
                    raise ValueError("Capture names must use letters, digits, '_' or '-'")
                if app.poll() is not None:
                    raise RuntimeError(title + " exited during inspection")
                path = args.output / (name + ".ppm")
                geometry = x11.capture(window, path)
                record = {"capture": str(path), "window": hex(window),
                          "pid": app.pid, "geometry": geometry}
                records.append(record)
                print(json.dumps(record, ensure_ascii=False), flush=True)

            capture("00-main")
            for index, action in enumerate(actions, 1):
                if not isinstance(action, dict) or len(action) != 1:
                    raise ValueError("Each action must contain exactly one operation")
                operation, value = next(iter(action.items()))
                records.append({"action": action})
                if operation == "capture":
                    capture(value)
                    continue
                if operation in ("click", "double_click"):
                    x11.click(window, *value)
                    if operation == "double_click":
                        time.sleep(0.08)
                        x11.click(window, *value)
                elif operation == "scroll":
                    x, y, steps = value
                    if abs(steps) > 50:
                        raise ValueError("Scroll step count must be between -50 and 50")
                    for _ in range(abs(steps)):
                        x11.click(window, x, y, button=4 if steps > 0 else 5)
                elif operation == "key":
                    x11.key(window, value)
                elif operation == "type":
                    if not re.fullmatch(r"[a-z0-9 ./-]{0,128}", value):
                        raise ValueError("Type accepts at most 128 lowercase ASCII letters/digits/path characters")
                    key_names = {" ": "space", ".": "period", "/": "slash", "-": "minus"}
                    for character in value:
                        x11.key(window, key_names.get(character, character))
                elif operation == "resize":
                    width, height = value
                    if not 300 <= width <= 1920 or not 300 <= height <= 1080:
                        raise ValueError("Resize dimensions exceed inspection screen limits")
                    x11.lib.XResizeWindow(x11.display, window, width, height)
                    x11.lib.XFlush(x11.display)
                elif operation == "wait":
                    if not 0 <= value <= 10:
                        raise ValueError("Wait duration must be between 0 and 10 seconds")
                    time.sleep(value)
                else:
                    raise ValueError("Unsupported operation: " + operation)
                time.sleep(0.5)
                capture("%02d-%s" % (index, operation))
            records.append({"result": "PASS", "scope": "Native window screenshots and real XTEST input; no file-transfer verification."})
            return 0
        except Exception as error:
            records.append({"result": "FAIL", "error": str(error)})
            print("FAIL: " + str(error), file=sys.stderr, flush=True)
            return 1
        finally:
            if x11:
                x11.close()
            for process in reversed(children):
                smoke.stop_child(process)
            for output in outputs:
                output.close()
            (args.output / "inspection.json").write_text(
                json.dumps(records, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    raise SystemExit(main())
