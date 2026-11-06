//! Exercise the real patched glutin-winit display, context, surface and map path.

use std::{
    ffi::{CString, c_int, c_void},
    num::NonZeroU32,
    sync::Arc,
};

use glow::HasContext as _;
use glutin::{
    config::ConfigTemplateBuilder,
    context::{ContextAttributesBuilder, NotCurrentGlContext as _},
    display::GetGlDisplay as _,
    prelude::GlDisplay as _,
    surface::{GlSurface as _, SurfaceAttributesBuilder, SwapInterval, WindowSurface},
};
use raw_window_handle::{HasDisplayHandle as _, HasWindowHandle as _, RawDisplayHandle};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};

#[link(name = "X11")]
unsafe extern "C" {
    fn XSync(display: *mut c_void, discard: c_int) -> c_int;
}

#[derive(Default)]
struct Probe {
    initialized: bool,
    error: Option<String>,
}

impl Probe {
    fn initialize(
        &mut self,
        event_loop: &ActiveEventLoop,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (window, config) = glutin_winit::DisplayBuilder::new()
            // Match eframe 0.33.3: the production glutin-winit patch handles
            // Linux's platform-specific preference and optional override.
            .with_preference(glutin_winit::ApiPreference::FallbackEgl)
            .with_window_attributes(Some(
                Window::default_attributes()
                    .with_title("LanSSR graphics probe")
                    .with_inner_size(winit::dpi::PhysicalSize::new(320, 180))
                    .with_visible(false),
            ))
            .build(
                event_loop,
                ConfigTemplateBuilder::new(),
                |mut configurations| {
                    configurations
                        .next()
                        .expect("no matching graphics configuration")
                },
            )?;
        let window = Arc::new(window.ok_or("glutin-winit did not create an X11 window")?);
        let display = config.display();
        println!("DISPLAY_BACKEND {}", display.version_string());
        let handle = window.window_handle()?.as_raw();
        let context = unsafe {
            display.create_context(
                &config,
                &ContextAttributesBuilder::new().build(Some(handle)),
            )?
        };
        let surface = unsafe {
            display.create_window_surface(
                &config,
                &SurfaceAttributesBuilder::<WindowSurface>::new().build(
                    handle,
                    NonZeroU32::new(320).unwrap(),
                    NonZeroU32::new(180).unwrap(),
                ),
            )?
        };
        let context = context.make_current(&surface)?;
        // eframe configures vsync here. The reported driver's asynchronous
        // GLX error could originate in this vendor-specific swap operation.
        match surface.set_swap_interval(&context, SwapInterval::Wait(NonZeroU32::MIN)) {
            Ok(()) => println!("SWAP_INTERVAL wait=1 accepted"),
            Err(error) => println!("SWAP_INTERVAL wait=1 unavailable: {error}"),
        }
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                display.get_proc_address(&CString::new(name).unwrap())
            })
        };
        println!("GL_RENDERER {}", unsafe {
            gl.get_parameter_string(glow::RENDERER)
        });
        unsafe {
            gl.clear_color(0.2, 0.45, 0.8, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.finish();
        }
        let mut pixel = [0u8; 4];
        unsafe {
            gl.read_pixels(
                0,
                0,
                1,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixel)),
            );
            assert_eq!(
                gl.get_error(),
                glow::NO_ERROR,
                "GL failed during pixel readback"
            );
        }
        for (actual, expected) in pixel.into_iter().zip([51u8, 115, 204, 255]) {
            assert!(
                actual.abs_diff(expected) <= 1,
                "Unexpected rendered color: {pixel:?}"
            );
        }
        println!("PIXEL_READBACK {pixel:?}");
        surface.swap_buffers(&context)?;
        window.set_visible(true);
        // Synchronize the actual native display after swap and map so a
        // deferred X11/GLX server error cannot remain hidden in this probe.
        if let RawDisplayHandle::Xlib(display) = event_loop.display_handle()?.as_raw() {
            unsafe {
                XSync(display.display.ok_or("Missing Xlib display")?.as_ptr(), 0);
            }
        }
        let size = window.inner_size();
        assert!(size.width > 0 && size.height > 0);
        println!("PASS context created, pixels painted, surface swapped, window mapped");
        Ok(())
    }
}

impl ApplicationHandler for Probe {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        if let Err(error) = self.initialize(event_loop) {
            self.error = Some(error.to_string());
        }
        event_loop.exit();
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window: WindowId,
        _event: WindowEvent,
    ) {
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::new()?;
    let mut probe = Probe::default();
    event_loop.run_app(&mut probe)?;
    if let Some(error) = probe.error {
        return Err(error.into());
    }
    Ok(())
}
