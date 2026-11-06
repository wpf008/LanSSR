use image::RgbaImage;

use crate::error::XCapResult;

use super::{
    impl_monitor::ImplMonitor,
    impl_window::ImplWindow,
    utils::{get_current_screen_buf, get_monitor_info_buf, wayland_detect},
    wayland_capture::{kwin_selected, wayland_capture},
    xorg_capture::xorg_capture,
};

pub fn capture_monitor(impl_monitor: &ImplMonitor) -> XCapResult<RgbaImage> {
    let monitor_info_buf = get_monitor_info_buf(impl_monitor.output)?;

    if wayland_detect() {
        wayland_capture(
            monitor_info_buf.x() as i32,
            monitor_info_buf.y() as i32,
            monitor_info_buf.width() as i32,
            monitor_info_buf.height() as i32,
        )
    } else {
        let screen_buf = get_current_screen_buf()?;

        xorg_capture(
            screen_buf.root(),
            monitor_info_buf.x() as i32,
            monitor_info_buf.y() as i32,
            monitor_info_buf.width() as u32,
            monitor_info_buf.height() as u32,
        )
    }
}

pub fn capture_region(
    impl_monitor: &ImplMonitor,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> XCapResult<RgbaImage> {
    let monitor_info_buf = get_monitor_info_buf(impl_monitor.output)?;

    if wayland_detect() {
        wayland_capture(
            monitor_info_buf.x() as i32 + x as i32,
            monitor_info_buf.y() as i32 + y as i32,
            width as i32,
            height as i32,
        )
    } else {
        let screen_buf = get_current_screen_buf()?;

        xorg_capture(
            screen_buf.root(),
            monitor_info_buf.x() as i32 + x as i32,
            monitor_info_buf.y() as i32 + y as i32,
            width,
            height,
        )
    }
}

/// Coordinates are pixels in capture_monitor's returned image. Legacy KWin
/// PNGs preserve the requested logical dimensions exactly, so their local
/// image coordinates can be passed to screenshotArea without scaling again.
/// Other Wayland backends can return a higher-resolution image: preserve the
/// existing full-image crop for those, rather than guessing a scale factor.
pub fn capture_image_region(
    impl_monitor: &ImplMonitor,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> XCapResult<RgbaImage> {
    if !wayland_detect() || kwin_selected() {
        let monitor = get_monitor_info_buf(impl_monitor.output)?;
        if width == 0
            || height == 0
            || x.checked_add(width)
                .is_none_or(|right| right > monitor.width() as u32)
            || y.checked_add(height)
                .is_none_or(|bottom| bottom > monitor.height() as u32)
        {
            return Err(crate::XCapError::InvalidCaptureRegion(
                "Image region is outside the monitor".into(),
            ));
        }
        return capture_region(impl_monitor, x, y, width, height);
    }
    let image = capture_monitor(impl_monitor)?;
    if width == 0
        || height == 0
        || x.checked_add(width)
            .is_none_or(|right| right > image.width())
        || y.checked_add(height)
            .is_none_or(|bottom| bottom > image.height())
    {
        return Err(crate::XCapError::InvalidCaptureRegion(
            "Image region is outside the captured frame".into(),
        ));
    }
    Ok(image::imageops::crop_imm(&image, x, y, width, height).to_image())
}

pub fn capture_window(impl_window: &ImplWindow) -> XCapResult<RgbaImage> {
    let width = impl_window.width()?;
    let height = impl_window.height()?;

    xorg_capture(impl_window.window, 0, 0, width, height)
}
