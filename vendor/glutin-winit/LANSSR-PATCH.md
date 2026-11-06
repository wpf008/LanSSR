# Linux X11 EGL preference

Vendored from glutin-winit 0.5.0, upstream commit
`0811b6c753334f1681c7db8a48e407596cfa9366`, subdirectory `glutin-winit`.
The complete MIT license is retained in `LICENSE`.

Only Linux builds with EGL and GLX enabled are changed, and only when the
actual display handle is Xlib or Xcb. Native Wayland, Windows and macOS keep
the upstream API preference. A Wayland session's residual DISPLAY variable
does not affect this choice.

eframe requests GLX before EGL upstream. Some Kylin ARM drivers expose GLX
but later raise GLXBadContextTag; the asynchronous error can surface while
winit maps the first window and cause a second panic during destruction.
Choose EGL first to avoid that GLX path while preserving hardware rendering.
The default eframe OpenGL/GLES context selection and vsync are unchanged.

`LANSSR_GL_BACKEND` can override the Linux X11 selection for diagnosis:

- `auto` (default): EGL, with GLX fallback if EGL **display initialization**
  fails. This is not a full retry after EGL config, context or surface failure.
- `egl`: EGL only.
- `glx`: GLX only, preserving the old path.

The selected API is printed to stderr. No desktop or graphics-driver settings
are modified, and software rendering is not forced by default.

Regression fixture: `../../scripts/validation-linux-graphics/`. Its Xvfb/Mesa
checks cannot reproduce or certify the physical Kylin GPU/driver combination.
