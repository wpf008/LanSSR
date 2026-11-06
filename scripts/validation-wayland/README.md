# Offline legacy Wayland regression

`legacy-server.c` is a small protocol fixture using the real libwayland server
and Unix socket transport. It advertises one output at 32×16 physical pixels,
16×8 logical units, scale 2, and logical origin (7, 11). A screencopy request
creates a real `wl_shm` buffer; the server writes a known RGB pattern and the
Rust probe checks every pixel returned by the patched `libwayshot-xcap`.
The probe also calls the exact xcap monitor APIs used by both apps' initial
window planning. A private Xvfb display supplies real XCB/RandR monitor data;
the old Wayland socket supplies scale 2. Missing Wayland output information
must take xcap's ordinary scale 1 fallback without a startup panic.

Run inside the existing ARM64 builder, with no network access:

```sh
python3 scripts/validation-wayland/run.py \
  --log dist/validation-2.0.21/wayland-legacy-protocol.log
```

The script builds an independent Rust probe offline, uses cached protocol XML
and the builder's installed `wayland-scanner`, C compiler, Xvfb and
libwayland-server 1.18.0. It tests:

- `wl_output` versions 2 and 4 × xdg-output versions 1, 2 and 3 × screencopy
  versions 1, 2 and 3, plus `wl_output` v1 with xdg-output v1/v2 (24 combinations).
- Output dimensions, logical position and size, legacy output names, repeated
  captures, output refresh, and every captured RGBA pixel.
- The real `xcap::Monitor::all`, `is_primary`, `scale_factor`, `x`, `y`, `width`
  and `height` startup path under each protocol combination and error case.
- Missing xdg-output, missing screencopy and no output devices return errors.
- Screencopy `failed` before buffer advertisement and after copy return errors
  within 8 seconds for all three protocol versions.
- Bottom-up (`y_invert`) SHM pixels are correctly normalized; unknown SHM
  formats return errors without hanging, for all three protocol versions.

Together these are 45 cases. `wl_output` v1 with xdg-output v3 is omitted because
v3 requires the `wl_output.done` completion event, which only exists from v2.

The builder's old core Wayland library only describes `wl_output` through v3.
The fixture uses a private interface descriptor for the two v4 events; it does
not replace system files or need a newer runtime.

This verifies wire negotiation, event ordering and SHM capture. It is not a
full desktop compositor and does not verify GUI integration, permission
portals, the user's physical Kylin machine, or DMA-BUF capture.
