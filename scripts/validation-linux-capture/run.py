#!/usr/bin/env python3
"""Run production xcap against a private real D-Bus compositor fixture offline."""
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


DIRECTORY = Path(__file__).resolve().parent
ROOT = DIRECTORY.parents[1]


def execute(arguments, **options):
    print("+ " + shlex.join(map(str, arguments)), flush=True)
    options.setdefault("check", True)
    return subprocess.run(list(map(str, arguments)), **options)


def build_probe():
    manifest = DIRECTORY / "probe" / "Cargo.toml"
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target" / "linux-arm64"))
    execute(["cargo", "build", "--locked", "--offline", "--manifest-path", manifest,
             "--target-dir", target])
    return target / "debug" / "lanbridge-linux-capture-probe"


@contextmanager
def xcb_display(geometry="64x48x24"):
    process = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0",
                                geometry, "-nolisten", "tcp", "-ac", "-noreset"],
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


def run_case(probe, case, services, wayland_server=None):
    print("CASE " + case, flush=True)
    with tempfile.TemporaryDirectory(prefix="lanbridge-capture-runtime-") as runtime:
        os.chmod(runtime, 0o700)
        environment = dict(os.environ, XDG_RUNTIME_DIR=runtime,
                           WAYLAND_DISPLAY="wayland-fixture-unavailable",
                           XDG_SESSION_TYPE="x11" if case == "x11-stale-wayland" else "wayland")
        wayland = None
        if wayland_server:
            environment["WAYLAND_DISPLAY"] = "legacy-test"
            wayland = subprocess.Popen([str(wayland_server), "legacy-test", "2", "2", "0", "0", "none"],
                                       env=environment, stdout=subprocess.PIPE,
                                       stderr=subprocess.PIPE, text=True)
            if not select.select([wayland.stdout], [], [], 5)[0] or wayland.stdout.readline().strip() != "READY":
                wayland.terminate()
                wayland.communicate(timeout=3)
                raise RuntimeError("Missing-screencopy Wayland fixture failed to become ready")
        service = subprocess.Popen([str(probe), "service", services], env=environment,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            if not select.select([service.stdout], [], [], 8)[0] or service.stdout.readline().strip() != "READY":
                raise RuntimeError("D-Bus fixture failed to become ready: " + case)
            result = subprocess.run([str(probe), "probe", case], env=environment,
                                    text=True, capture_output=True, timeout=24)
            print(result.stdout, end="", flush=True)
            if result.returncode:
                print(result.stderr, end="", flush=True)
                raise RuntimeError(f"Capture probe exited {result.returncode}: " + case)
            if service.poll() is not None:
                raise RuntimeError("D-Bus fixture exited unexpectedly: " + case)
        finally:
            service.terminate()
            _, stderr = service.communicate(timeout=3)
            if stderr:
                print("SERVICE: " + stderr, end="", flush=True)
            if wayland:
                wayland.terminate()
                _, stderr = wayland.communicate(timeout=3)
                if stderr:
                    print("WAYLAND: " + stderr, end="", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--probe", type=Path)
    parser.add_argument("--only", action="append", help="Run only the named case; may be repeated")
    parser.add_argument("--case", help=argparse.SUPPRESS)
    parser.add_argument("--services", help=argparse.SUPPRESS)
    parser.add_argument("--wayland-server", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--log", type=Path)
    arguments = parser.parse_args()
    if arguments.case:
        run_case(arguments.probe, arguments.case, arguments.services, arguments.wayland_server)
        return
    probe = arguments.probe
    if not arguments.skip_build:
        probe = build_probe()
    elif probe is None:
        target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target" / "linux-arm64"))
        probe = target / "debug" / "lanbridge-linux-capture-probe"
    probe = probe.resolve()
    cases = [("ukui", "ukui"), ("ukui-lower", "ukui-lower"), ("kde", "kde"),
             ("both", "ukui+kde"), ("fallback-from-ukui", "ukui+kde"),
             ("missing-screencopy", "ukui"), ("missing-services", "none"),
             ("x11-stale-wayland", "ukui"), ("continuous-files", "ukui")]
    if arguments.only:
        cases = [(case, services) for case, services in cases if case in arguments.only]
        if len(cases) != len(set(arguments.only)):
            raise RuntimeError("Unknown case supplied to --only")
    log = None
    if arguments.log:
        arguments.log.parent.mkdir(parents=True, exist_ok=True)
        log = arguments.log.open("w")
    with tempfile.TemporaryDirectory(prefix="lanbridge-dbus-test-") as directory:
        build = Path(directory)
        # Do not permit activation of the container's installed interactive
        # portal. This private bus must contain only the fixture services.
        config = build / "bus.conf"
        config.write_text('''<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen>
<policy context="default"><allow own="*"/><allow send_destination="*"/>
<allow receive_sender="*"/></policy></busconfig>''')
        protocol_runner = DIRECTORY.parent / "validation-wayland" / "run.py"
        specification = importlib.util.spec_from_file_location("legacy_wayland_fixture", protocol_runner)
        module = importlib.util.module_from_spec(specification)
        specification.loader.exec_module(module)
        wayland_server = module.build_server(build)
        for case, services in cases:
            command = ["dbus-run-session", "--config-file=" + str(config), "--", "python3", str(Path(__file__).resolve()),
                       "--case", case, "--services", services, "--probe", str(probe)]
            if case == "missing-screencopy":
                command += ["--wayland-server", str(wayland_server)]
            with xcb_display("1280x720x24" if case == "continuous-files" else "64x48x24") as display:
                environment = dict(os.environ, DISPLAY=display)
                result = execute(command, env=environment, text=True, capture_output=True,
                                 timeout=30, check=False)
            print(result.stdout, end="", flush=True)
            if log:
                log.write(result.stdout)
                log.flush()
            if result.stderr:
                print(result.stderr, end="", file=sys.stderr, flush=True)
                if log:
                    log.write(result.stderr)
            if result.returncode:
                raise RuntimeError(f"Private session-bus test failed: {case}")
        message = f"PASS all {len(cases)} native compositor capture cases\n"
        print(message, end="", flush=True)
        if log:
            log.write(message)
    if log:
        log.close()


if __name__ == "__main__":
    main()
