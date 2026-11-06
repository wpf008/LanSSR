//! Physical-pixel window planning shared by both desktop applications.
use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScreenArea {
    pub(crate) work_rect: egui::Rect,
    pub(crate) decoration: egui::Vec2,
    pub(crate) can_position: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowRole {
    Sender,
    Receiver,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FractionalWindowPlan {
    pub(crate) inner_pixels: egui::Vec2,
    pub(crate) outer_position: Option<egui::Pos2>,
    pub(crate) chrome_scale: f32,
}

/// Split the work area into adjacent 60% / 40% columns. Fractions include the
/// native title bar and borders, so opening the receiver cannot cover the board.
pub(crate) fn fractional_window_plan(
    area: ScreenArea,
    native_scale: f32,
    role: WindowRole,
) -> FractionalWindowPlan {
    let native_scale = valid_scale(native_scale);
    let work = area.work_rect.size().max(egui::Vec2::splat(1.0));
    let split = (work.x * 0.6).floor();
    let width = match role {
        WindowRole::Sender => split,
        WindowRole::Receiver => work.x - split,
    };
    let outer_size = egui::vec2(width, (work.y * 0.8).floor());
    let inner_pixels =
        (outer_size - area.decoration.max(egui::Vec2::ZERO)).max(egui::Vec2::splat(1.0));
    let minimum_ui = match role {
        WindowRole::Sender => egui::vec2(640.0, 600.0),
        WindowRole::Receiver => egui::vec2(480.0, 650.0),
    };
    let chrome_scale = native_scale
        .min(inner_pixels.x / minimum_ui.x)
        .min(inner_pixels.y / minimum_ui.y);
    // Round downward: UI geometry must never gain a pixel past the target.
    let chrome_scale = (chrome_scale * 1000.0).floor().max(1.0) / 1000.0;
    let offset_x = if role == WindowRole::Receiver {
        split
    } else {
        0.0
    };
    FractionalWindowPlan {
        inner_pixels,
        outer_position: area.can_position.then_some(
            area.work_rect.min + egui::vec2(offset_x, ((work.y - outer_size.y) * 0.5).floor()),
        ),
        chrome_scale,
    }
}

fn valid_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// An estimate used before a native window exists. It is replaced by the
/// platform's actual work area on the first frame.
pub(crate) fn initial_screen_area() -> Option<(ScreenArea, f32)> {
    let monitors = xcap::Monitor::all().ok()?;
    let monitor = monitors
        .iter()
        .find(|monitor| monitor.is_primary().unwrap_or(false))
        .or_else(|| monitors.first())?;
    let scale = valid_scale(monitor.scale_factor().ok()?);
    #[cfg(target_os = "windows")]
    let units = 1.0;
    #[cfg(not(target_os = "windows"))]
    let units = scale;
    let origin = egui::pos2(monitor.x().ok()? as f32, monitor.y().ok()? as f32) * units;
    let size = egui::vec2(monitor.width().ok()? as f32, monitor.height().ok()? as f32) * units;
    if !size.is_finite() || size.min_elem() <= 0.0 {
        return None;
    }
    Some((
        ScreenArea {
            work_rect: egui::Rect::from_min_size(
                origin + egui::vec2(0.0, 24.0 * scale),
                egui::vec2(size.x, (size.y - 80.0 * scale).max(1.0)),
            ),
            decoration: egui::vec2(16.0, 40.0) * scale,
            can_position: true,
        },
        scale,
    ))
}

pub(crate) fn screen_area(ctx: &egui::Context, frame: &eframe::Frame) -> Option<ScreenArea> {
    #[cfg(target_os = "windows")]
    if let Some(area) = windows_screen_area(frame) {
        return Some(area);
    }
    #[cfg(target_os = "macos")]
    if let Some(area) = macos_screen_area(ctx) {
        return Some(area);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = frame;
    fallback_screen_area(ctx)
}

fn viewport_decoration(ctx: &egui::Context, native_scale: f32) -> egui::Vec2 {
    ctx.input(|input| input.viewport().outer_rect.zip(input.viewport().inner_rect))
        .map(|(outer, inner)| (outer.size() - inner.size()) * ctx.pixels_per_point())
        .unwrap_or(egui::vec2(16.0, 40.0) * native_scale)
        .max(egui::Vec2::ZERO)
}

fn fallback_screen_area(ctx: &egui::Context) -> Option<ScreenArea> {
    let pixels_per_point = ctx.pixels_per_point();
    let (size, native_scale) = ctx.input(|input| {
        let viewport = input.viewport();
        Some((
            viewport.monitor_size? * pixels_per_point,
            valid_scale(viewport.native_pixels_per_point.unwrap_or(pixels_per_point)),
        ))
    })?;
    Some(ScreenArea {
        work_rect: egui::Rect::from_min_size(
            egui::pos2(0.0, 24.0 * native_scale),
            egui::vec2(size.x, (size.y - 80.0 * native_scale).max(1.0)),
        ),
        decoration: viewport_decoration(ctx, native_scale),
        can_position: false,
    })
}

#[cfg(target_os = "windows")]
fn windows_screen_area(frame: &eframe::Frame) -> Option<ScreenArea> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::{
        Foundation::RECT,
        Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        },
        UI::WindowsAndMessaging::{GetClientRect, GetWindowRect},
    };
    let handle = frame.window_handle().ok()?;
    let RawWindowHandle::Win32(window) = handle.as_raw() else {
        return None;
    };
    let hwnd = window.hwnd.get() as *mut core::ffi::c_void;
    let mut info = MONITORINFO {
        cbSize: core::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let mut outer = RECT::default();
    let mut inner = RECT::default();
    // The borrowed handle remains alive. The native API reports physical
    // pixels under winit's DPI-aware process and excludes the taskbar.
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if monitor.is_null()
            || GetMonitorInfoW(monitor, &mut info) == 0
            || GetWindowRect(hwnd, &mut outer) == 0
            || GetClientRect(hwnd, &mut inner) == 0
        {
            return None;
        }
    }
    Some(ScreenArea {
        work_rect: egui::Rect::from_min_max(
            egui::pos2(info.rcWork.left as f32, info.rcWork.top as f32),
            egui::pos2(info.rcWork.right as f32, info.rcWork.bottom as f32),
        ),
        decoration: egui::vec2(
            ((outer.right - outer.left) - (inner.right - inner.left)).max(0) as f32,
            ((outer.bottom - outer.top) - (inner.bottom - inner.top)).max(0) as f32,
        ),
        can_position: true,
    })
}

#[cfg(target_os = "macos")]
fn macos_screen_area(ctx: &egui::Context) -> Option<ScreenArea> {
    use core::ffi::{c_char, c_void};
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct NativeRect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }
    type Object = *mut c_void;
    type Selector = *const c_void;
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Object;
        fn sel_registerName(name: *const c_char) -> Selector;
        fn objc_msgSend();
        #[cfg(target_arch = "x86_64")]
        fn objc_msgSend_stret();
    }
    unsafe fn object(receiver: Object, selector: Selector) -> Object {
        let send: unsafe extern "C" fn(Object, Selector) -> Object =
            unsafe { core::mem::transmute(objc_msgSend as *const ()) };
        unsafe { send(receiver, selector) }
    }
    unsafe fn rectangle(receiver: Object, selector: Selector) -> NativeRect {
        #[cfg(target_arch = "x86_64")]
        {
            let send: unsafe extern "C" fn(*mut NativeRect, Object, Selector) =
                unsafe { core::mem::transmute(objc_msgSend_stret as *const ()) };
            let mut rect = NativeRect::default();
            unsafe { send(&mut rect, receiver, selector) };
            rect
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let send: unsafe extern "C" fn(Object, Selector) -> NativeRect =
                unsafe { core::mem::transmute(objc_msgSend as *const ()) };
            unsafe { send(receiver, selector) }
        }
    }
    // Called from eframe's main-thread update, with AppKit loaded and its
    // autorelease pool active. Selectors are stable runtime-owned pointers.
    let (frame, visible) = unsafe {
        let class = objc_getClass(c"NSScreen".as_ptr());
        if class.is_null() {
            return None;
        }
        let screens = object(class, sel_registerName(c"screens".as_ptr()));
        if screens.is_null() {
            return None;
        }
        let first = object(screens, sel_registerName(c"firstObject".as_ptr()));
        if first.is_null() {
            return None;
        }
        let selected = object(class, sel_registerName(c"mainScreen".as_ptr()));
        let selected = if selected.is_null() { first } else { selected };
        (
            rectangle(first, sel_registerName(c"frame".as_ptr())),
            rectangle(selected, sel_registerName(c"visibleFrame".as_ptr())),
        )
    };
    let scale = valid_scale(
        ctx.input(|input| input.viewport().native_pixels_per_point)
            .unwrap_or(ctx.pixels_per_point()),
    );
    let size = egui::vec2(visible.width as f32, visible.height as f32) * scale;
    if !size.is_finite() || size.min_elem() <= 0.0 {
        return None;
    }
    // Cocoa uses a bottom-left origin; egui uses the primary screen's top-left.
    let origin = egui::pos2(
        visible.x as f32,
        (frame.y + frame.height - visible.y - visible.height) as f32,
    ) * scale;
    Some(ScreenArea {
        work_rect: egui::Rect::from_min_size(origin, size),
        decoration: viewport_decoration(ctx, scale),
        can_position: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proportional_windows_include_borders_stay_inside_work_area_and_never_overlap() {
        for size in [(1366.0, 728.0), (1920.0, 1040.0), (3840.0, 2080.0)] {
            for native_scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
                let area = ScreenArea {
                    work_rect: egui::Rect::from_min_size(egui::pos2(-1920.0, 40.0), size.into()),
                    decoration: egui::vec2(16.0, 39.0) * native_scale,
                    can_position: true,
                };
                let sender = fractional_window_plan(area, native_scale, WindowRole::Sender);
                let receiver = fractional_window_plan(area, native_scale, WindowRole::Receiver);
                let sender_rect = egui::Rect::from_min_size(
                    sender.outer_position.unwrap(),
                    sender.inner_pixels + area.decoration,
                );
                let receiver_rect = egui::Rect::from_min_size(
                    receiver.outer_position.unwrap(),
                    receiver.inner_pixels + area.decoration,
                );
                assert!(area.work_rect.contains_rect(sender_rect));
                assert!(area.work_rect.contains_rect(receiver_rect));
                assert_eq!(sender_rect.right(), receiver_rect.left());
                assert_eq!(receiver_rect.right(), area.work_rect.right());
                assert!((sender_rect.width() / size.0 - 0.6).abs() < 0.001);
                assert!((receiver_rect.width() / size.0 - 0.4).abs() < 0.001);
                assert!((sender_rect.height() / size.1 - 0.8).abs() < 0.002);
                let logical = receiver.inner_pixels / receiver.chrome_scale;
                assert!(logical.x >= 479.999 && logical.y >= 649.999);
            }
        }
    }

    #[test]
    fn backend_without_global_position_keeps_window_manager_positioning() {
        let area = ScreenArea {
            work_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1040.0)),
            decoration: egui::vec2(16.0, 39.0),
            can_position: false,
        };
        assert!(
            fractional_window_plan(area, 1.0, WindowRole::Receiver)
                .outer_position
                .is_none()
        );
    }
}
