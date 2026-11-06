# Linux graphics initialization regression fixture

This fixture exercises the production `vendor/glutin-winit` patch on an actual
Mesa/Xvfb X11 display. It is a developer validation tool and is not included in
the installed applications. No packages, desktop settings, driver settings or
user files are changed by the fixture.

Run in the existing offline ARM64 builder, from the repository root:

```sh
docker run --rm --platform linux/arm64 --network none \
  --mount type=bind,source="$PWD",target=/workspace -w /workspace \
  -e CARGO_HOME=/workspace/target/linux-cargo \
  lanbridge-kylin-arm64-builder \
  python3 scripts/validation-linux-graphics/run.py \
    --log dist/validation-2.0.23/linux-graphics.log
```

Build the two production release binaries first, or supply `--sender` and
`--receiver` paths. The independent probe builds in
`target/linux-graphics-validation`; `--skip-apps` runs the six probe cases
without requiring production binaries. The checked-in probe lockfile is
compatible with the builder's Rust 1.88. All Cargo operations are locked and
offline.

The Rust probe uses the same `ApiPreference::FallbackEgl` call as eframe, then
creates the real selected display, context and X11 window surface. It requests
the same vsync interval as eframe (reporting unsupported swap interval as a
diagnostic), draws a solid color, verifies an RGBA pixel with a one-unit
quantization tolerance, swaps buffers and requests window mapping. An XSync on
the actual Xlib display flushes deferred server errors. The production GUI
cases additionally query X11 to verify viewable windows owned by the launched
processes and check that each stays alive for one second.

`graphics-trap.c` is loaded only into child processes through `LD_PRELOAD`.
glutin obtains its function pointers through `dlsym(library_handle)`, so the
fixture intercepts those lookups and wraps only the relevant GLX query calls
and EGL initialization/error calls. Its per-case trace distinguishes API calls
from symbol lookup. The fixture covers:

- Default selection and explicit `LANSSR_GL_BACKEND=egl` use EGL with a GLX
  trap armed. The trap exits immediately if a real GLX query executes.
- Explicit `glx` hits that trap and exits with code 86, proving the fault
  injection is active.
- `auto` falls back to a real GLX context when `eglInitialize` reports
  `EGL_NOT_INITIALIZED`.
- Strict `egl` returns an ordinary initialization error without calling GLX
  under that same injected EGL failure.
- An unknown override value reports a warning and follows automatic selection.
- Both production applications map actual windows with the GLX trap armed,
  and again through the automatic GLX fallback with EGL unavailable.

This verifies backend selection and real rendering/window initialization under
injected API failures. The trace does not reproduce the user's delayed
`GLXBadContextTag`, the physical graphics driver, or kernel
`5.4.18-53-generic`. Mesa llvmpipe is forced only in the fixture to make the
container reproducible; normal application launches keep hardware rendering.
It does not benchmark transfer speed or replace physical Kylin verification.
