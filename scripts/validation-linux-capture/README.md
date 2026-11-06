# Offline native UKUI/KWin capture regression

Run in the existing Linux ARM64 builder without network access:

```sh
python3 scripts/validation-linux-capture/run.py \
  --log dist/validation-2.0.22/native-dbus-capture.log
```

The independent Rust probe links the production patched `xcap` and `imgbridge`
crates. A second process registers the actual legacy compositor service/object/
interface names on a private `dbus-daemon`. Calls to
`screenshotArea(i,i,i,i,b)->s` cross the real session bus, and the server writes
a new PNG into the temporary directory for each successful call. No mock
image reader or replacement client capture function is used.

Each case receives an independent private Xvfb, bus and process lifetime. The
bus has no system service activation directories so absent screenshot services
stay absent rather than launching the container's interactive screenshot
portal. All processes have deadlines and are removed after the case.

The nine cases cover:

- UKUI service `org.ukui.KWin`, its lowercase `org.ukui.kwin` alternative,
  legacy KDE, and both compositors being present.
- UKUI denying the first screenshot, successful fallback to KDE, and later
  captures retaining the selected KDE backend when UKUI becomes usable again.
- A real older Wayland socket advertising xdg-output v2 with no
  `zwlr_screencopy_manager_v1`, while native UKUI screenshots continue working.
- No compositor screenshot service returning an error.
- Explicit X11 with a stale `WAYLAND_DISPLAY` capturing via XCB and making
  zero calls to the available UKUI screenshot service.
- The production `receiver::run_continuous` finding a checkerboard at physical
  image position (112, 84), receiving two successive files with watermark
  enabled, and saving 47,214 bytes with exact byte comparison and no duplicates.
  Events change the visible fixture's real desktop PNG through checkerboard,
  metadata and data frames; the receiver reads these through the same native
  D-Bus backend used by the application. This also checks native capture of
  the located 512×320 region within a 1280×720 desktop.

For each working compositor case, every returned RGBA pixel in full-desktop
and `Monitor::capture_image_region` captures is checked against independently
generated row/column and frame values. Access denial, empty paths, vanished
files, corrupt PNG and incorrect image dimensions must return errors, never
reuse an earlier frame, recover on the next valid frame, and clean up temporary
compositor images.

This verifies native API transport, frame freshness, image coordinates and the
actual receiver file loop. It does not emulate a full UKUI compositor, screen
capture notifications, the user's physical Kylin machine, or real display
scaling. The chosen legacy API returns PNG paths; this suite makes no claim
about the interactive legacy FD overloads or the newer KDE Screenshot2 API.
