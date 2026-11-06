//! Reusable Windows capture of the located grid, in physical screen pixels.
//!
//! Full-monitor capture is only needed to find calibration. During transfer,
//! the GDI bitmap and RGB output stay the size of the grid, irrespective of
//! the desktop resolution. The capture object stays on its creating thread.

use anyhow::{Result, ensure};

use crate::locator::LocatedGrid;

#[cfg(any(target_os = "windows", test))]
type SetThreadDpiAwareness =
    unsafe extern "system" fn(*mut std::ffi::c_void) -> *mut std::ffi::c_void;

/// Optional thread DPI support: Windows 7 has only process DPI awareness.
/// When the thread API is absent, startup must have already enabled process
/// DPI awareness before creating windows or capturing the screen.
#[cfg(any(target_os = "windows", test))]
struct ThreadDpiScope(Option<(SetThreadDpiAwareness, *mut std::ffi::c_void)>);

#[cfg(any(target_os = "windows", test))]
impl ThreadDpiScope {
    fn enter(api: Option<SetThreadDpiAwareness>, desired: *mut std::ffi::c_void) -> Result<Self> {
        let Some(api) = api else {
            // The process DPI policy applies to capture threads on Windows 7.
            // Do not change a process-wide policy from a worker thread.
            return Ok(Self(None));
        };
        // SAFETY: The caller supplies the resolved Windows thread-DPI function
        // and a valid DPI context. This scope restores its result on this thread.
        let previous = unsafe { api(desired) };
        ensure!(
            !previous.is_null(),
            "Cannot enable physical-pixel capture: {}",
            std::io::Error::last_os_error()
        );
        Ok(Self(Some((api, previous))))
    }
}

#[cfg(any(target_os = "windows", test))]
impl Drop for ThreadDpiScope {
    fn drop(&mut self) {
        if let Some((api, previous)) = self.0 {
            // SAFETY: The handle was returned by the same API on this thread.
            unsafe { api(previous) };
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PhysicalRegion {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    bgra_bytes: usize,
}

impl PhysicalRegion {
    fn new(
        monitor_x: i32,
        monitor_y: i32,
        monitor_width: u32,
        monitor_height: u32,
        region: LocatedGrid,
    ) -> Result<Self> {
        // Grid has public fields; validate literal instances before allocating.
        crate::grid::Grid::new(
            region.grid.width,
            region.grid.height,
            region.grid.cell_size,
            region.grid.water_mask,
        )?;
        ensure!(
            region
                .x
                .checked_add(region.grid.width)
                .is_some_and(|end| end <= monitor_width)
                && region
                    .y
                    .checked_add(region.grid.height)
                    .is_some_and(|end| end <= monitor_height),
            "Grid region is outside the captured display"
        );
        let x = i32::try_from(i64::from(monitor_x) + i64::from(region.x))?;
        let y = i32::try_from(i64::from(monitor_y) + i64::from(region.y))?;
        // BitBlt uses signed coordinates and dimensions, including both edges.
        i32::try_from(i64::from(x) + i64::from(region.grid.width))?;
        i32::try_from(i64::from(y) + i64::from(region.grid.height))?;
        let width = i32::try_from(region.grid.width)?;
        let height = i32::try_from(region.grid.height)?;
        let bgra_bytes =
            usize::try_from(u64::from(region.grid.width) * u64::from(region.grid.height) * 4)?;
        ensure!(
            u32::try_from(bgra_bytes).is_ok(),
            "Capture bitmap exceeds the Windows DIB size limit"
        );
        Ok(Self {
            x,
            y,
            width,
            height,
            bgra_bytes,
        })
    }
}

/// Convert a top-down, tightly packed 32-bit BI_RGB DIB directly to RGB.
/// BI_RGB stores B, G, R, unused bytes; its fourth byte need not be opaque.
fn copy_bgra_to_rgb(bgra: &[u8], rgb: &mut [u8]) {
    assert_eq!(bgra.len() % 4, 0);
    assert_eq!(rgb.len(), bgra.len() / 4 * 3);
    for (source, destination) in bgra
        .as_chunks::<4>()
        .0
        .iter()
        .zip(rgb.as_chunks_mut::<3>().0)
    {
        destination[0] = source[2];
        destination[1] = source[1];
        destination[2] = source[0];
    }
}

#[cfg(target_os = "windows")]
mod native {
    use std::{
        ffi::c_void,
        marker::PhantomData,
        mem::size_of,
        ptr::{null_mut, slice_from_raw_parts},
        rc::Rc,
        sync::OnceLock,
    };

    use anyhow::{Context, Result, ensure};
    use image::{RgbImage, RgbaImage};
    use windows_sys::Win32::{
        Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
            DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, GetDC, HBITMAP, HDC, HGDIOBJ,
            ReleaseDC, SRCCOPY, SelectObject,
        },
        System::LibraryLoader::{GetModuleHandleW, GetProcAddress},
        UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
    };
    use xcap::Monitor;

    use super::{PhysicalRegion, SetThreadDpiAwareness, ThreadDpiScope, copy_bgra_to_rgb};
    use crate::locator::LocatedGrid;

    fn thread_dpi_api() -> Option<SetThreadDpiAwareness> {
        static API: OnceLock<Option<SetThreadDpiAwareness>> = OnceLock::new();
        *API.get_or_init(|| {
            const USER32: [u16; 11] = [117, 115, 101, 114, 51, 50, 46, 100, 108, 108, 0];
            // SAFETY: user32 is a loaded process dependency and is not unloaded
            // during capture. Resolve the Win10 API by name so it never appears
            // as a required PE import that would prevent Windows 7 from loading.
            unsafe {
                let module = GetModuleHandleW(USER32.as_ptr());
                if module.is_null() {
                    return None;
                }
                GetProcAddress(module, c"SetThreadDpiAwarenessContext".as_ptr().cast()).map(
                    |address| {
                        // The symbol has the documented system calling
                        // convention and context argument/return signature.
                        std::mem::transmute::<
                            unsafe extern "system" fn() -> isize,
                            SetThreadDpiAwareness,
                        >(address)
                    },
                )
            }
        })
    }

    /// Temporarily use physical desktop coordinates on modern Windows. On
    /// Windows 7, inherit the DPI-aware process policy established at startup.
    fn physical_dpi_scope() -> Result<ThreadDpiScope> {
        ThreadDpiScope::enter(thread_dpi_api(), DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE)
    }

    /// Calibration and region capture must use the same physical coordinate
    /// space even in CLI mode, where no winit window has initialized DPI.
    pub(crate) fn capture_monitor(monitor: &Monitor) -> Result<RgbaImage> {
        let _dpi = physical_dpi_scope()?;
        Ok(monitor.capture_image()?)
    }

    pub(crate) struct WindowsRegionCapture {
        screen_dc: HDC,
        memory_dc: HDC,
        bitmap: HBITMAP,
        previous_bitmap: HGDIOBJ,
        bits: *mut c_void,
        region: PhysicalRegion,
        rgb: RgbImage,
        // GetDC/ReleaseDC and the DIB must remain on their creating thread.
        _same_thread: PhantomData<Rc<()>>,
    }

    impl WindowsRegionCapture {
        pub(crate) fn new(monitor: &Monitor, location: LocatedGrid) -> Result<Self> {
            let _dpi = physical_dpi_scope()?;
            // xcap's Windows monitor bounds come from EnumDisplaySettingsW:
            // dmPosition and dmPelsWidth/Height already describe physical pixels.
            // Do not multiply by the monitor's scale_factor; negative origins
            // are valid coordinates in the Windows virtual desktop.
            let region = PhysicalRegion::new(
                monitor.x().context("Cannot read monitor x position")?,
                monitor.y().context("Cannot read monitor y position")?,
                monitor.width().context("Cannot read monitor width")?,
                monitor.height().context("Cannot read monitor height")?,
                location,
            )?;
            let mut capture = Self {
                screen_dc: null_mut(),
                memory_dc: null_mut(),
                bitmap: null_mut(),
                previous_bitmap: null_mut(),
                bits: null_mut(),
                region,
                rgb: RgbImage::new(location.grid.width, location.grid.height),
                _same_thread: PhantomData,
            };
            // SAFETY: A null HWND gets a screen DC for the virtual desktop.
            // All successful allocations are immediately owned by capture's
            // Drop implementation, including during subsequent setup failures.
            unsafe {
                capture.screen_dc = GetDC(null_mut());
                ensure!(
                    !capture.screen_dc.is_null(),
                    "Cannot acquire screen DC: {}",
                    std::io::Error::last_os_error()
                );
                capture.memory_dc = CreateCompatibleDC(capture.screen_dc);
                ensure!(
                    !capture.memory_dc.is_null(),
                    "Cannot create capture DC: {}",
                    std::io::Error::last_os_error()
                );
                let bitmap_info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: region.width,
                        // A negative height makes the DIB top-down. Each 32bpp
                        // row is already DWORD aligned, so there is no padding.
                        biHeight: -region.height,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB,
                        biSizeImage: region.bgra_bytes as u32,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                capture.bitmap = CreateDIBSection(
                    capture.screen_dc,
                    &bitmap_info,
                    DIB_RGB_COLORS,
                    &mut capture.bits,
                    null_mut(),
                    0,
                );
                ensure!(
                    !capture.bitmap.is_null() && !capture.bits.is_null(),
                    "Cannot create capture bitmap: {}",
                    std::io::Error::last_os_error()
                );
                let previous = SelectObject(capture.memory_dc, capture.bitmap);
                ensure!(
                    !previous.is_null() && previous as isize != -1,
                    "Cannot select capture bitmap: {}",
                    std::io::Error::last_os_error()
                );
                capture.previous_bitmap = previous;
            }
            Ok(capture)
        }

        /// Capture a fresh grid, reusing both native and RGB pixel storage.
        /// Failure returns an error rather than exposing pixels from a prior
        /// frame. The borrow lasts only until the next capture or object drop.
        pub(crate) fn capture(&mut self) -> Result<&RgbImage> {
            let _dpi = physical_dpi_scope()?;
            // SAFETY: Both DCs and the selected DIB are live, thread-bound
            // resources owned by self; dimensions were checked in new.
            unsafe {
                ensure!(
                    BitBlt(
                        self.memory_dc,
                        0,
                        0,
                        self.region.width,
                        self.region.height,
                        self.screen_dc,
                        self.region.x,
                        self.region.y,
                        SRCCOPY,
                    ) != 0,
                    "Cannot capture grid: {}",
                    std::io::Error::last_os_error()
                );
                // CreateDIBSection requires synchronization before CPU access
                // to memory written by GDI. Do not read before this succeeds.
                ensure!(
                    GdiFlush() != 0,
                    "Cannot synchronize grid capture: {}",
                    std::io::Error::last_os_error()
                );
                let bgra = &*slice_from_raw_parts(self.bits.cast::<u8>(), self.region.bgra_bytes);
                copy_bgra_to_rgb(bgra, &mut self.rgb);
            }
            Ok(&self.rgb)
        }
    }

    impl Drop for WindowsRegionCapture {
        fn drop(&mut self) {
            // SAFETY: Drop runs on the creating thread. Restore the original
            // bitmap before deleting the DIB, then dispose of both owned DCs.
            // Null checks cover constructor failures with partial allocation.
            unsafe {
                if !self.previous_bitmap.is_null() {
                    SelectObject(self.memory_dc, self.previous_bitmap);
                }
                if !self.bitmap.is_null() {
                    DeleteObject(self.bitmap);
                }
                if !self.memory_dc.is_null() {
                    DeleteDC(self.memory_dc);
                }
                if !self.screen_dc.is_null() {
                    ReleaseDC(null_mut(), self.screen_dc);
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) use native::{WindowsRegionCapture, capture_monitor};

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, ffi::c_void};

    use image::{Rgb, RgbImage};

    use super::*;
    use crate::{config::Config, grid::Grid, protocol::Frame};

    thread_local! {
        static DPI_CALLS: RefCell<Vec<isize>> = const { RefCell::new(Vec::new()) };
    }

    unsafe extern "system" fn fake_thread_dpi(context: *mut c_void) -> *mut c_void {
        DPI_CALLS.with(|calls| calls.borrow_mut().push(context as isize));
        (-1isize) as *mut c_void
    }

    unsafe extern "system" fn failing_thread_dpi(context: *mut c_void) -> *mut c_void {
        DPI_CALLS.with(|calls| calls.borrow_mut().push(context as isize));
        std::ptr::null_mut()
    }

    #[test]
    fn missing_thread_dpi_api_inherits_process_policy_without_error() {
        DPI_CALLS.with(|calls| calls.borrow_mut().clear());
        let scope = ThreadDpiScope::enter(None, (-3isize) as *mut c_void).unwrap();
        assert!(scope.0.is_none());
        drop(scope);
        DPI_CALLS.with(|calls| assert!(calls.borrow().is_empty()));
    }

    #[test]
    fn thread_dpi_scope_restores_prior_context_even_if_capture_fails() {
        fn failed_capture() -> Result<()> {
            let _scope = ThreadDpiScope::enter(Some(fake_thread_dpi), (-3isize) as *mut c_void)?;
            anyhow::bail!("Simulated GDI capture failure")
        }
        DPI_CALLS.with(|calls| calls.borrow_mut().clear());
        assert!(failed_capture().is_err());
        DPI_CALLS.with(|calls| assert_eq!(*calls.borrow(), [-3, -1]));
    }

    #[test]
    fn failed_thread_dpi_switch_does_not_restore_an_invalid_context() {
        DPI_CALLS.with(|calls| calls.borrow_mut().clear());
        assert!(ThreadDpiScope::enter(Some(failing_thread_dpi), (-3isize) as *mut c_void).is_err());
        DPI_CALLS.with(|calls| assert_eq!(*calls.borrow(), [-3]));
    }

    fn located(x: u32, y: u32) -> LocatedGrid {
        LocatedGrid {
            x,
            y,
            grid: Grid::new(1440, 800, 4, true).unwrap(),
        }
    }

    #[test]
    fn physical_region_supports_negative_monitor_origins_without_dpi_scaling() {
        let local = located(120, 60);
        let region = PhysicalRegion::new(-3840, -2160, 3840, 2160, local).unwrap();
        assert_eq!((region.x, region.y), (-3720, -2100));
        assert_eq!((region.width, region.height), (1440, 800));
        assert_eq!(region.bgra_bytes, 1440 * 800 * 4);
        // Increasing the desktop pixel count must not increase steady-state
        // capture storage or alter the region's physical dimensions.
        let larger = PhysicalRegion::new(-3840, -2160, 7680, 4320, local).unwrap();
        assert_eq!(region, larger);
    }

    #[test]
    fn physical_region_rejects_out_of_bounds_or_overflowing_coordinates() {
        assert!(PhysicalRegion::new(0, 0, 1920, 1080, located(481, 0)).is_err());
        assert!(PhysicalRegion::new(0, 0, 1920, 1080, located(0, 281)).is_err());
        assert!(PhysicalRegion::new(i32::MAX, 0, 3840, 2160, located(0, 0)).is_err());
        assert!(PhysicalRegion::new(0, 0, u32::MAX, 2160, located(u32::MAX, 0)).is_err());
        let edge = PhysicalRegion::new(0, 0, 1920, 1080, located(480, 280)).unwrap();
        assert_eq!((edge.x, edge.y), (480, 280));
    }

    #[test]
    fn top_down_dib_preserves_channels_rows_and_reuses_rgb_storage() {
        let mut rgb = vec![0; 12];
        let buffer = rgb.as_ptr();
        // Two 2-pixel rows, with differing unused/alpha bytes.
        copy_bgra_to_rgb(
            &[3, 2, 1, 0, 6, 5, 4, 255, 9, 8, 7, 0, 12, 11, 10, 37],
            &mut rgb,
        );
        assert_eq!(rgb, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        copy_bgra_to_rgb(
            &[
                30, 20, 10, 0, 60, 50, 40, 0, 90, 80, 70, 0, 120, 110, 100, 0,
            ],
            &mut rgb,
        );
        assert_eq!(rgb.as_ptr(), buffer);
        assert_eq!(rgb, [10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120]);
    }

    #[test]
    fn captured_dib_preserves_watermark_and_plain_frame_payloads() {
        let config = Config::default();
        for water_mask in [false, true] {
            for cell_size in [2, 4] {
                let grid = Grid::new(
                    config.width / config.cell_size * cell_size,
                    config.height / config.cell_size * cell_size,
                    cell_size,
                    water_mask,
                )
                .unwrap();
                let frame = Frame::Data {
                    number: 7,
                    payload: vec![0, 1, 15, 16, 127, 128, 254, 255],
                };
                let original = grid.encode(&frame.encode().unwrap()).unwrap();
                let bgra: Vec<u8> = original
                    .pixels()
                    .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], 0])
                    .collect();
                let mut capture = RgbImage::from_pixel(grid.width, grid.height, Rgb([0, 0, 0]));
                copy_bgra_to_rgb(&bgra, &mut capture);
                assert_eq!(capture, original);
                assert_eq!(
                    Frame::decode(&grid.decode(&capture).unwrap()).unwrap(),
                    frame
                );
            }
        }
    }
}
