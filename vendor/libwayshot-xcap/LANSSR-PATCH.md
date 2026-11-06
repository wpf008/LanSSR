# Kylin V10 SP1 Wayland compatibility

Vendored from `libwayshot-xcap` 0.3.3, upstream commit
`4351de93758fa02e3c01b712aabbb8fdb8d7ef19` (`libwayshot/`). The upstream BSD
license is preserved in `LICENSE` (from the same commit's repository root).

Local changes:

- Negotiate xdg-output v1–v3 instead of panicking when v3 is unavailable.
- Bind wl_output v1–v4, obtain legacy names from xdg-output, and validate sizes
  before computing scale. Use the current physical mode.
- Negotiate SHM screencopy v1–v3. v1/v2 use the single Buffer event to finish
  format enumeration; v3 still waits for BufferDone. DMABUF still requires v3.
- Return early on capture failure, destroy completed/rejected SHM frames, and
  normalize bottom-up SHM rows indicated by the protocol's y_invert flag.
- Include the missing protocol name in error messages.

Regression fixture: `scripts/validation-wayland/`. This changes neither the
macOS nor Windows capture implementation. No runtime environment overrides or
Xwayland root-window fallback are introduced.
