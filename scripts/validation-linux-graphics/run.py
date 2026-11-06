#!/usr/bin/env python3
"""Validate Linux EGL preference and explicit GLX fallback with real Xvfb/Mesa.

The preload only injects failures at API entry points. It does not emulate the
user's graphics driver or kernel, and must never be installed with the apps.
"""

import argparse
from contextlib import contextmanager
import importlib.util
import os
from pathlib import Path
import select
import shlex
import subprocess
import sys
import tempfile
import time


DIRECTORY = Path(__file__).resolve().parent
ROOT = DIRECTORY.parents[1]


def execute(arguments, **options):
    print("+ " + shlex.join(map(str, arguments)), flush=True)
    return subprocess.run(list(map(str, arguments)), **options)


def build_probe(target):
    execute(["cargo", "build", "--locked", "--offline", "--manifest-path",
             DIRECTORY / "probe" / "Cargo.toml", "--target-dir", target], check=True)
    return target / "debug" / "lanbridge-linux-graphics-probe"


@contextmanager
def private_display():
    process = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0",
                                "1920x1080x24", "-nolisten", "tcp", "-ac", "-noreset"],
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        if not select.select([process.stdout], [], [], 10)[0]:
            raise RuntimeError("Private Xvfb did not become ready")
        number = process.stdout.readline().strip()
        if not number.isdigit():
            raise RuntimeError("Private Xvfb returned an invalid display number")
        yield ":" + number
    finally:
        process.terminate()
        _, stderr = process.communicate(timeout=5)
        if stderr:
            print("XVFB: " + stderr, end="", flush=True)


def base_environment(display, temporary, shim):
    environment = os.environ.copy()
    for name in ("WAYLAND_DISPLAY", "LANSSR_GL_BACKEND", "LANSSR_GRAPHICS_TRAP"):
        environment.pop(name, None)
    environment.update({"DISPLAY": display, "XDG_SESSION_TYPE": "x11",
                        "WINIT_UNIX_BACKEND": "x11", "LIBGL_ALWAYS_SOFTWARE": "1",
                        "LD_PRELOAD": str(shim), "XDG_RUNTIME_DIR": str(temporary)})
    return environment


def inspect_trace(path):
    return path.read_text() if path.exists() else ""


def assert_trace(trace, *, egl, glx):
    if ("CALL eglInitialize" in trace) != egl:
        raise RuntimeError("Unexpected EGL call trace: " + repr(trace))
    if ("CALL glX" in trace) != glx:
        raise RuntimeError("Unexpected GLX call trace: " + repr(trace))


def run_probe(probe, temporary, environment, log):
    cases = [
        ("default-broken-glx", None, "broken-glx", "EGL", 0, True, False),
        ("explicit-egl-broken-glx", "egl", "broken-glx", "EGL", 0, True, False),
        ("explicit-glx-positive-control", "glx", "broken-glx", None, 86, False, True),
        ("auto-egl-unavailable", "auto", "unavailable-egl", "GLX", 0, True, True),
        ("strict-egl-unavailable", "egl", "unavailable-egl", None, 1, True, False),
        ("unknown-value-uses-auto", "invalid-fixture-value", "broken-glx", "EGL", 0, True, False),
    ]
    for name, backend, trap, expected, code, egl, glx in cases:
        trace_path = temporary / (name + ".trace")
        current = dict(environment, LANSSR_GRAPHICS_TRAP=trap,
                       LANSSR_GRAPHICS_TRACE=str(trace_path))
        if backend is not None:
            current["LANSSR_GL_BACKEND"] = backend
        result = execute([probe], env=current, capture_output=True, text=True, timeout=25)
        output = result.stdout + result.stderr
        trace = inspect_trace(trace_path)
        log("CASE " + name + "\n" + output.strip() + "\nTRACE " + repr(trace))
        if result.returncode != code:
            raise RuntimeError(f"{name}: expected exit {code}, got {result.returncode}")
        if expected:
            if "DISPLAY_BACKEND " + expected not in output or "PASS context created" not in output:
                raise RuntimeError(name + ": expected actual " + expected + " context and successful draw/map")
        elif code == 1 and ("Error:" not in output or "panicked" in output):
            raise RuntimeError(name + ": expected an ordinary initialization error")
        assert_trace(trace, egl=egl, glx=glx)
        log("PASS " + name)


def load_smoke_module():
    specification = importlib.util.spec_from_file_location("lanbridge_gui_smoke", ROOT / "scripts" / "smoke-linux-gui.py")
    module = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(module)
    return module


def map_application(title, command, temporary, environment, x11, smoke, log, name, backend, trap, expected):
    trace_path = temporary / (name + "-" + title + ".trace")
    output_path = temporary / (name + "-" + title + ".output")
    current = dict(environment, LANSSR_GRAPHICS_TRAP=trap,
                   LANSSR_GRAPHICS_TRACE=str(trace_path))
    if backend is not None:
        current["LANSSR_GL_BACKEND"] = backend
    with output_path.open("w+b") as output:
        process = subprocess.Popen([str(command)], env=current, cwd=temporary,
                                   stdout=output, stderr=output, start_new_session=True)
        try:
            deadline, mapped = time.monotonic() + 35, None
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError(title + " exited before map: " + str(process.returncode))
                matches = [window for window in x11.windows()
                           if window["title"] == title and window["mapped"]
                           and window["pid"] == process.pid and min(window["size"]) > 0]
                if matches:
                    mapped = matches[0]
                    break
                time.sleep(0.1)
            if not mapped:
                raise RuntimeError(title + " did not map a viewable window")
            deadline = time.monotonic() + 1
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError(title + " failed after map: " + str(process.returncode))
                time.sleep(0.1)
            if not any(window["id"] == mapped["id"] and window["mapped"] for window in x11.windows()):
                raise RuntimeError(title + " lost its mapped window")
            output.flush()
            diagnostics = output_path.read_text(errors="replace")
            trace = inspect_trace(trace_path)
            if "LanSSR 图形后端：" + expected not in diagnostics:
                raise RuntimeError(title + " did not report its actual " + expected + " backend")
            if "panicked" in diagnostics:
                raise RuntimeError(title + " logged an unexpected panic")
            assert_trace(trace, egl=True, glx=expected == "GLX")
            log(f"PASS {name} {title}: mapped={hex(mapped['id'])} size={mapped['size']} survived=1s backend={expected}")
        finally:
            smoke.stop_child(process)
            output.flush()
            diagnostics = output_path.read_text(errors="replace").strip()
            if diagnostics:
                log(title + " diagnostics:\n" + diagnostics[-12000:])
            log("TRACE " + repr(inspect_trace(trace_path)))


def run_applications(sender, receiver, temporary, environment, display, log):
    smoke = load_smoke_module()
    x11 = smoke.X11(display)
    try:
        cases = [("default-broken-glx", None, "broken-glx", "EGL"),
                 ("auto-egl-unavailable", "auto", "unavailable-egl", "GLX")]
        for name, backend, trap, expected in cases:
            for title, command in (("LanSender", sender), ("LanReceiver", receiver)):
                map_application(title, command, temporary, environment, x11, smoke, log,
                                name, backend, trap, expected)
    finally:
        x11.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--skip-apps", action="store_true")
    parser.add_argument("--probe", type=Path)
    parser.add_argument("--sender", type=Path, default=ROOT / "target" / "linux-arm64" / "release" / "LanSender")
    parser.add_argument("--receiver", type=Path, default=ROOT / "target" / "linux-arm64" / "release" / "LanReceiver")
    parser.add_argument("--log", type=Path)
    arguments = parser.parse_args()
    if sys.platform != "linux":
        parser.error("Run this fixture inside the Linux ARM64 build image")
    target = ROOT / "target" / "linux-graphics-validation"
    probe = arguments.probe or target / "debug" / "lanbridge-linux-graphics-probe"
    if not arguments.skip_build:
        probe = build_probe(target)
    report = None
    if arguments.log:
        arguments.log.parent.mkdir(parents=True, exist_ok=True)
        report = arguments.log.open("w", encoding="utf-8")

    def log(message):
        print(message, flush=True)
        if report:
            report.write(message + "\n")
            report.flush()

    try:
        with tempfile.TemporaryDirectory(prefix="lanbridge-graphics-") as directory:
            temporary = Path(directory)
            os.chmod(temporary, 0o700)
            shim = temporary / "graphics-trap.so"
            execute(["cc", "-shared", "-fPIC", "-O2", "-Wall", "-Wextra", "-Werror",
                     DIRECTORY / "graphics-trap.c", "-ldl", "-o", shim], check=True)
            with private_display() as display:
                environment = base_environment(display, temporary, shim)
                run_probe(probe.resolve(), temporary, environment, log)
                if not arguments.skip_apps:
                    run_applications(arguments.sender.resolve(), arguments.receiver.resolve(),
                                     temporary, environment, display, log)
            log("PASS all Linux graphics backend fixtures")
            log("Scope: real Mesa/Xvfb context creation and GUI map with injected API failures; physical Kylin GPU and kernel remain untested.")
        return 0
    except Exception as error:
        log("FAIL " + str(error))
        return 1
    finally:
        if report:
            report.close()


if __name__ == "__main__":
    raise SystemExit(main())
