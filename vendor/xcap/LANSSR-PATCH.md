# UKUI/KWin screenshot compatibility

Vendored from xcap 0.9.8, upstream commit
`ef9e2a3892c8b972b5964aece131192d5777c660`. The Apache 2.0 license is retained
in `LICENSE`.

Linux changes:

- Detect an explicit X11 session before considering residual WAYLAND_DISPLAY.
  A genuine Wayland session never falls back to an Xwayland root screenshot.
- Probe native UKUI (`org.ukui.KWin`, compatibility alias `org.ukui.kwin`) and
  KDE (`org.kde.KWin`) legacy screenshot services before the existing GNOME,
  portal and wlroots paths. Use the noninteractive string-returning
  screenshotArea overload. Cache the selected backend and D-Bus connection
  within each receiving worker thread.
- Validate PNG dimensions, reject empty/invalid responses, clean up regular
  compositor screenshot files in the temporary directory, and return errors
  without reusing an older frame. Preserve failures from each probed backend.
- Add capture_image_region, which uses local pixels from capture_image. X11
  and legacy KWin capture the region directly; other Wayland backends retain
  full-image cropping for native pixel/scale correctness. Include the monitor
  origin when forming a global Wayland region.

Old UKUI screenshotArea produces a logical-size PNG and issues a compositor
notification. PNG compression, disk I/O, notification configuration and desktop
scaling affect practical reception. No notification settings or desktop-session
environment variables are changed, and no interactive FD overload is used.

Source references:

- https://github.com/ukui/ukui-kwin/blob/master/effects/screenshot/screenshot.h
- https://github.com/ukui/ukui-kwin/blob/master/effects/screenshot/screenshot.cpp
- https://github.com/ukui/ukui-kwin/blob/master/dbusinterface.cpp
- https://github.com/ukui/ukui-kwin/blob/master/libkwineffects/kwinglutils.cpp

Regression fixtures: scripts/validation-linux-capture and
scripts/validation-wayland. Windows and macOS implementations are unchanged.
