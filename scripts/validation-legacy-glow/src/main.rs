//! Offscreen, real-driver validation of the OpenGL 1.1 painter.
//! Run in Linux/Xvfb with MESA_GL_VERSION_OVERRIDE=1.1 and LIBGL_ALWAYS_SOFTWARE=1.

// Compile the production module without adding the full desktop runtime to
// this isolated renderer harness. Its `eframe::egui` refers to this same egui.
pub extern crate egui;
extern crate self as eframe;
pub use imgbridge::grid;

#[path = "../../../src/transfer_texture.rs"]
mod transfer_texture;

use egui::{
    ClippedPrimitive, Color32, ColorImage, Pos2, Rect, TextureId, TextureOptions,
    epaint::{ImageDelta, Mesh, Primitive},
};
use glow::HasContext as _;
use std::{
    ffi::{CString, c_char, c_int, c_ulong, c_void},
    ptr,
    sync::Arc,
};

#[link(name = "X11")]
unsafe extern "C" {
    fn XOpenDisplay(name: *const c_char) -> *mut c_void;
    fn XCloseDisplay(display: *mut c_void) -> c_int;
    fn XFree(data: *mut c_void) -> c_int;
}

#[link(name = "GL")]
unsafe extern "C" {
    fn glXChooseFBConfig(
        display: *mut c_void,
        screen: c_int,
        attrs: *const c_int,
        count: *mut c_int,
    ) -> *mut *mut c_void;
    fn glXCreatePbuffer(display: *mut c_void, config: *mut c_void, attrs: *const c_int) -> c_ulong;
    fn glXDestroyPbuffer(display: *mut c_void, drawable: c_ulong);
    fn glXCreateNewContext(
        display: *mut c_void,
        config: *mut c_void,
        render_type: c_int,
        shared: *mut c_void,
        direct: c_int,
    ) -> *mut c_void;
    fn glXDestroyContext(display: *mut c_void, context: *mut c_void);
    fn glXMakeContextCurrent(
        display: *mut c_void,
        draw: c_ulong,
        read: c_ulong,
        context: *mut c_void,
    ) -> c_int;
    fn glXGetProcAddress(name: *const u8) -> *const c_void;
    fn glGetTexLevelParameteriv(target: u32, level: i32, pname: u32, value: *mut i32);
}

struct GlSurface {
    display: *mut c_void,
    pbuffer: c_ulong,
    context: *mut c_void,
}

impl GlSurface {
    fn new(width: i32, height: i32) -> Self {
        // GLX itself is 1.3; the context's GL_VERSION is independently forced to 1.1.
        unsafe {
            let display = XOpenDisplay(ptr::null());
            assert!(!display.is_null(), "Xvfb display unavailable");
            let attrs = [0x8010, 0x0004, 0x8011, 0x0001, 8, 8, 9, 8, 10, 8, 11, 0, 0];
            let mut count = 0;
            let configs = glXChooseFBConfig(display, 0, attrs.as_ptr(), &mut count);
            assert!(
                count > 0 && !configs.is_null(),
                "No GLX RGBA pbuffer config"
            );
            let config = *configs;
            let pbuffer_attrs = [0x8041, width, 0x8040, height, 0];
            let pbuffer = glXCreatePbuffer(display, config, pbuffer_attrs.as_ptr());
            let context = glXCreateNewContext(display, config, 0x8014, ptr::null_mut(), 1);
            XFree(configs.cast());
            assert!(
                pbuffer != 0 && !context.is_null(),
                "GLX context creation failed"
            );
            assert_ne!(glXMakeContextCurrent(display, pbuffer, pbuffer, context), 0);
            Self {
                display,
                pbuffer,
                context,
            }
        }
    }
}

impl Drop for GlSurface {
    fn drop(&mut self) {
        unsafe {
            glXMakeContextCurrent(self.display, 0, 0, ptr::null_mut());
            glXDestroyContext(self.display, self.context);
            glXDestroyPbuffer(self.display, self.pbuffer);
            XCloseDisplay(self.display);
        }
    }
}

fn textured_rect(
    id: TextureId,
    rect: Rect,
    uv: Rect,
    color: Color32,
    clip: Rect,
) -> ClippedPrimitive {
    let mut mesh = Mesh::with_texture(id);
    mesh.add_rect_with_uv(rect, uv, color);
    ClippedPrimitive {
        clip_rect: clip,
        primitive: Primitive::Mesh(mesh),
    }
}

fn assert_no_gl_error(gl: &glow::Context, stage: &str) {
    let error = unsafe { gl.get_error() };
    assert_eq!(error, glow::NO_ERROR, "GL error after {stage}: 0x{error:x}");
}

fn validate_transfer_tiles(
    gl: &glow::Context,
    painter: &mut egui_glow::Painter,
    screen: [u32; 2],
    watermark: bool,
    scale: f32,
) {
    const WIDTH: u32 = 1936;
    const HEIGHT: u32 = 1088;
    const ORIGIN: [usize; 2] = [24, 12];
    let grid = grid::Grid::new(WIDTH, HEIGHT, 2, watermark).unwrap();
    let ctx = egui::Context::default();
    ctx.input_mut(|input| input.max_texture_side = 1024);
    let mut texture = transfer_texture::TransferTexture::new(&ctx, &grid.checkerboard());
    let tile_ids = ctx
        .tex_manager()
        .read()
        .allocated()
        .filter(|(_, meta)| meta.name.starts_with("transfer-grid-"))
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    assert_eq!(
        tile_ids.len(),
        4,
        "the production module must tile both axes"
    );

    let input = || {
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(screen[0] as f32, screen[1] as f32) / scale,
            )),
            max_texture_side: Some(1024),
            ..Default::default()
        };
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(scale);
        input
    };
    let rect = Rect::from_min_size(
        egui::pos2(ORIGIN[0] as f32, ORIGIN[1] as f32) / scale,
        egui::vec2(WIDTH as f32, HEIGHT as f32) / scale,
    );

    for frame in 0..2 {
        let payload = (0..grid.payload_capacity())
            .map(|index| ((index * 73 + index / 256 + frame * 31) % 256) as u8)
            .collect::<Vec<_>>();
        let source = grid.encode(&payload).unwrap();
        texture.set(&source);
        let current_ids = ctx
            .tex_manager()
            .read()
            .allocated()
            .filter(|(_, meta)| meta.name.starts_with("transfer-grid-"))
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        assert_eq!(
            current_ids, tile_ids,
            "same-shape frames must reuse texture handles"
        );
        let output = ctx.run(input(), |ctx| {
            texture.paint(&ctx.layer_painter(egui::LayerId::background()), rect);
        });
        assert_eq!(output.pixels_per_point, scale);
        let uploads = output
            .textures_delta
            .set
            .iter()
            .filter(|(id, _)| tile_ids.contains(id))
            .collect::<Vec<_>>();
        assert_eq!(uploads.len(), 4, "all tiles must update in the same frame");
        for (_, upload) in uploads {
            assert_eq!(upload.pos, None);
            assert_eq!(upload.options, TextureOptions::NEAREST);
            assert!(
                upload
                    .image
                    .size()
                    .into_iter()
                    .all(|side| { side <= 1024 && side.next_power_of_two() <= 1024 })
            );
        }
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        assert_eq!(
            primitives.iter().filter(|primitive| {
                matches!(&primitive.primitive, Primitive::Mesh(mesh) if tile_ids.contains(&mesh.texture_id))
            }).count(),
            4,
            "each production tile must have a visible GPU mesh"
        );
        painter.clear(screen, [0.2, 0.3, 0.4, 1.0]);
        painter.paint_and_update_textures(
            screen,
            output.pixels_per_point,
            &primitives,
            &output.textures_delta,
        );
        unsafe { gl.finish() };
        let capture = painter.read_screen_rgba(screen);
        let mut raw = Vec::with_capacity(WIDTH as usize * HEIGHT as usize * 3);
        for y in 0..HEIGHT as usize {
            for x in 0..WIDTH as usize {
                let pixel =
                    capture.pixels[(y + ORIGIN[1]) * screen[0] as usize + x + ORIGIN[0]].to_array();
                let expected = source.get_pixel(x as u32, y as u32).0;
                assert_eq!(
                    &pixel[..3],
                    &expected,
                    "tiled GPU pixels changed at {x},{y}, watermark={watermark}, dpi={scale}, frame={frame}"
                );
                raw.extend_from_slice(&pixel[..3]);
            }
        }
        let reconstructed = image::RgbImage::from_raw(WIDTH, HEIGHT, raw).unwrap();
        assert_eq!(grid.decode(&reconstructed).unwrap(), payload);
        assert_no_gl_error(gl, "production tiled protocol roundtrip");

        // A normal UI repaint keeps the currently displayed codec frame in
        // GPU memory instead of spending a second upload on the same pixels.
        let idle = ctx.run(input(), |ctx| {
            texture.paint(&ctx.layer_painter(egui::LayerId::background()), rect);
        });
        assert!(
            idle.textures_delta
                .set
                .iter()
                .all(|(id, _)| !tile_ids.contains(id))
        );
    }
    drop(texture);
    let cleanup = ctx.tex_manager().write().take_delta();
    assert_eq!(cleanup.free.len(), 4);
    for id in cleanup.free {
        painter.free_texture(id);
    }
    println!(
        "PASS: production TransferTexture 1936x1088, 4 tiles at limit=1024, two frames GPU pixel exact + protocol decode, reused handles, idle upload-free; watermark={watermark}, dpi={scale}"
    );
}

fn main() {
    const W: usize = 1984;
    const H: usize = 1200;
    let _surface = GlSurface::new(W as i32, H as i32);
    let gl = Arc::new(unsafe {
        glow::Context::from_loader_function(|name| {
            let name = CString::new(name).unwrap();
            glXGetProcAddress(name.as_ptr().cast())
        })
    });
    let version = unsafe { gl.get_parameter_string(glow::VERSION) };
    let renderer = unsafe { gl.get_parameter_string(glow::RENDERER) };
    let maximum_texture_size = unsafe { gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) };
    println!(
        "GL_VERSION={version}; GL_RENDERER={renderer}; GL_MAX_TEXTURE_SIZE={maximum_texture_size}"
    );
    let mut edge_clamp_extensions = gl
        .supported_extensions()
        .iter()
        .filter(|name| name.contains("edge_clamp") || name.contains("border_clamp"))
        .cloned()
        .collect::<Vec<_>>();
    edge_clamp_extensions.sort();
    println!("EDGE_CLAMP_EXTENSIONS={edge_clamp_extensions:?}");
    let expected = std::env::var("MESA_GL_VERSION_OVERRIDE")
        .expect("Set an explicit GL version for reproducible validation");
    assert!(
        version.starts_with(&expected),
        "Expected {expected}, got {version}"
    );
    let legacy = gl.version().major < 2 || cfg!(feature = "force-legacy");
    println!(
        "PAINTER_BACKEND={}",
        if legacy {
            "fixed-function OpenGL 1.1"
        } else {
            "shader OpenGL 2+"
        }
    );
    let mut painter = egui_glow::Painter::new(gl.clone(), "", None, false).unwrap();
    let screen = [W as u32, H as u32];
    let full = Rect::from_min_max(Pos2::ZERO, egui::pos2(W as f32, H as f32));

    for watermark in [false, true] {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            validate_transfer_tiles(&gl, &mut painter, screen, watermark, scale);
        }
    }

    // NPOT (360x180) and every grayscale/watermark nibble must survive GPU readback exactly.
    for watermark in [false, true] {
        let grid = imgbridge::grid::Grid::new(360, 180, 4, watermark).unwrap();
        let payload = (0..grid.payload_capacity())
            .map(|n| (n % 256) as u8)
            .collect::<Vec<_>>();
        let source = grid.encode(&payload).unwrap();
        let image = ColorImage::from_rgb([360, 180], source.as_raw());
        painter.set_texture(
            TextureId::Managed(100),
            &ImageDelta::full(image, TextureOptions::NEAREST),
        );
        let rect = Rect::from_min_size(egui::pos2(12.0, 6.0), egui::vec2(360.0, 180.0));
        painter.clear(screen, [0.2, 0.3, 0.4, 1.0]);
        painter.paint_primitives(
            screen,
            1.0,
            &[textured_rect(
                TextureId::Managed(100),
                rect,
                Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
                full,
            )],
        );
        unsafe { gl.finish() };
        let capture = painter.read_screen_rgba(screen);
        let mut raw = Vec::with_capacity(360 * 180 * 3);
        for y in 6..186 {
            for x in 12..372 {
                raw.extend_from_slice(&capture.pixels[y * W + x].to_array()[..3]);
            }
        }
        let output = imgbridge_image(raw);
        assert_eq!(
            source.as_raw(),
            output.as_raw(),
            "NPOT grid pixels changed, watermark={watermark}"
        );
        assert_eq!(
            grid.decode(&output).unwrap(),
            payload,
            "Protocol decode failed"
        );
        assert_no_gl_error(&gl, "protocol grid roundtrip");
        println!(
            "PASS: NPOT encoded grid + pixel-perfect protocol roundtrip, watermark={watermark}"
        );
    }

    // Partial uploads must retain unrelated pixels in the padded power-of-two texture.
    let mut pixels = vec![Color32::from_rgb(19, 31, 47); 13 * 7];
    painter.set_texture(
        TextureId::Managed(101),
        &ImageDelta::full(
            ColorImage::new([13, 7], pixels.clone()),
            TextureOptions::NEAREST,
        ),
    );
    painter.set_texture(
        TextureId::Managed(101),
        &ImageDelta::partial(
            [3, 2],
            ColorImage::new([5, 3], vec![Color32::from_rgb(211, 103, 59); 15]),
            TextureOptions::NEAREST,
        ),
    );
    for y in 2..5 {
        for x in 3..8 {
            pixels[y * 13 + x] = Color32::from_rgb(211, 103, 59);
        }
    }
    painter.clear(screen, [0.0, 0.0, 0.0, 1.0]);
    painter.paint_primitives(
        screen,
        1.0,
        &[textured_rect(
            TextureId::Managed(101),
            Rect::from_min_size(egui::pos2(5.0, 8.0), egui::vec2(13.0, 7.0)),
            Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
            Color32::WHITE,
            full,
        )],
    );
    unsafe { gl.finish() };
    let capture = painter.read_screen_rgba(screen);
    for y in 0..7 {
        for x in 0..13 {
            assert_eq!(
                capture.pixels[(y + 8) * W + x + 5],
                pixels[y * 13 + x],
                "Partial update mismatch at {x},{y}"
            );
        }
    }
    assert_no_gl_error(&gl, "partial texture update");
    println!("PASS: partial NPOT texture update retains existing pixels");

    // Validate premultiplied vertex colors and scissor clipping without shader functions.
    let white = ColorImage::new([1, 1], vec![Color32::WHITE]);
    painter.set_texture(
        TextureId::Managed(102),
        &ImageDelta::full(white, TextureOptions::NEAREST),
    );
    let colored = Color32::from_rgba_premultiplied(128, 0, 0, 128);
    painter.clear(screen, [0.0, 0.0, 1.0, 1.0]);
    let clip = Rect::from_min_max(egui::pos2(20.0, 20.0), egui::pos2(40.0, 40.0));
    painter.paint_primitives(
        screen,
        1.0,
        &[textured_rect(
            TextureId::Managed(102),
            Rect::from_min_max(egui::pos2(10.0, 10.0), egui::pos2(50.0, 50.0)),
            Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
            colored,
            clip,
        )],
    );
    unsafe { gl.finish() };
    let capture = painter.read_screen_rgba(screen);
    assert_eq!(&capture.pixels[25 * W + 25].to_array()[..3], &[128, 0, 127]);
    assert_eq!(&capture.pixels[15 * W + 15].to_array()[..3], &[0, 0, 255]);
    assert_no_gl_error(&gl, "vertex alpha blend and clipping");
    println!("PASS: premultiplied blending + scissor clipping");

    // A 1024-wide interior is legal even when its GL1.1 border makes storage 1026 wide.
    let pixels = (0..1024 * 8)
        .map(|n| {
            Color32::from_rgb(
                (n % 256) as u8,
                ((n / 7) % 256) as u8,
                ((n / 13) % 256) as u8,
            )
        })
        .collect::<Vec<_>>();
    let wide_id = TextureId::Managed(103);
    painter.set_texture(
        wide_id,
        &ImageDelta::full(
            ColorImage::new([1024, 8], pixels.clone()),
            TextureOptions::NEAREST,
        ),
    );
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, painter.texture(wide_id));
        let mut width = 0;
        let mut border = 0;
        glGetTexLevelParameteriv(glow::TEXTURE_2D, 0, 0x1000, &mut width); // GL_TEXTURE_WIDTH
        glGetTexLevelParameteriv(glow::TEXTURE_2D, 0, 0x1005, &mut border); // GL_TEXTURE_BORDER
        // Mesa may normalize bordered uploads to their interior storage. Pixel
        // readback below verifies the effective result rather than driver layout.
        assert!(
            (width, border) == (1024, 0) || (legacy && (width, border) == (1026, 1)),
            "Unexpected 1024-wide texture storage: width={width}, border={border}"
        );
        println!("1024-wide texture driver storage: width={width}, border={border}");
    }
    painter.clear(screen, [0.0, 0.0, 0.0, 1.0]);
    painter.paint_primitives(
        screen,
        1.0,
        &[textured_rect(
            wide_id,
            Rect::from_min_size(egui::pos2(32.0, 64.0), egui::vec2(1024.0, 8.0)),
            Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
            Color32::WHITE,
            full,
        )],
    );
    unsafe { gl.finish() };
    let capture = painter.read_screen_rgba(screen);
    for y in 0..8 {
        for x in 0..1024 {
            assert_eq!(
                capture.pixels[(y + 64) * W + x + 32],
                pixels[y * 1024 + x],
                "1024 texture mismatch at {x},{y}"
            );
        }
    }
    assert_no_gl_error(&gl, "1024-wide texture with border");
    println!(
        "PASS: 1024-wide texture, legal border dimensions, every pixel exact (driver limit={maximum_texture_size})"
    );

    // egui uses UV(0,0) for opaque panels; linear sampling must not darken that white texel.
    let mut atlas = vec![Color32::from_rgb(210, 17, 33); 3 * 5];
    atlas[0] = Color32::WHITE;
    let panel_id = TextureId::Managed(104);
    let panel_color = Color32::from_rgb(31, 47, 71);
    painter.set_texture(
        panel_id,
        &ImageDelta::full(ColorImage::new([3, 5], atlas), TextureOptions::LINEAR),
    );
    painter.clear(screen, [0.0, 0.0, 0.0, 1.0]);
    let panel_rect = Rect::from_min_max(egui::pos2(20.0, 100.0), egui::pos2(100.0, 160.0));
    painter.paint_primitives(
        screen,
        1.0,
        &[textured_rect(
            panel_id,
            panel_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::ZERO),
            panel_color,
            full,
        )],
    );
    unsafe { gl.finish() };
    let capture = painter.read_screen_rgba(screen);
    for y in 100..160 {
        for x in 20..100 {
            assert_eq!(
                capture.pixels[y * W + x],
                panel_color,
                "WHITE_UV panel mismatch at {x},{y}"
            );
        }
    }
    assert_no_gl_error(&gl, "linear WHITE_UV panel");
    println!("PASS: linear-filter WHITE_UV panel remains exact at edges and interior");

    // A top-left patch touches the white texel and its border. Drivers which
    // normalize bordered storage must receive valid nonnegative subimage offsets.
    let updated_white = Color32::from_rgb(73, 113, 151);
    painter.set_texture(
        panel_id,
        &ImageDelta::partial(
            [0, 0],
            ColorImage::new([1, 1], vec![updated_white]),
            TextureOptions::LINEAR,
        ),
    );
    assert_no_gl_error(&gl, "top-left white-texel partial upload");
    painter.clear(screen, [0.0, 0.0, 0.0, 1.0]);
    painter.paint_primitives(
        screen,
        1.0,
        &[textured_rect(
            panel_id,
            panel_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::ZERO),
            Color32::WHITE,
            full,
        )],
    );
    unsafe { gl.finish() };
    let capture = painter.read_screen_rgba(screen);
    for y in 100..160 {
        for x in 20..100 {
            assert_eq!(
                capture.pixels[y * W + x],
                updated_white,
                "Patched WHITE_UV mismatch at {x},{y}"
            );
        }
    }
    assert_no_gl_error(&gl, "patched WHITE_UV panel");
    println!("PASS: top-left WHITE_UV partial patch renders exact and reports no GL error");

    // Use actual CJK glyphs and egui's real atlas with the Windows software limit (1024).
    let ctx = egui::Context::default();
    let font_path = "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc";
    let bytes = std::fs::read(font_path).expect("Install fonts-noto-cjk in the validation image");
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "cjk-validation".into(),
        Arc::new(egui::FontData::from_owned(bytes)),
    );
    fonts
        .families
        .get_mut(&egui::FontFamily::Proportional)
        .unwrap()
        .insert(0, "cjk-validation".into());
    ctx.set_fonts(fonts);
    let font_id = egui::FontId::proportional(18.0);
    let text = "开始接收 文件传输 LanSender";
    let input = egui::RawInput {
        screen_rect: Some(full),
        max_texture_side: Some(1024),
        ..Default::default()
    };
    let text_panel = Rect::from_min_max(egui::pos2(20.0, 200.0), egui::pos2(650.0, 320.0));
    let output = ctx.run(input, |ctx| {
        assert!(
            ctx.fonts_mut(|fonts| fonts.has_glyphs(&font_id, text)),
            "CJK glyph fallback missing"
        );
        let painter = ctx.layer_painter(egui::LayerId::background());
        painter.rect_filled(text_panel, 0.0, panel_color);
        painter.text(
            egui::pos2(40.0, 230.0),
            egui::Align2::LEFT_TOP,
            text,
            font_id.clone(),
            Color32::WHITE,
        );
    });
    let font_atlas = output
        .textures_delta
        .set
        .iter()
        .find(|(id, _)| *id == TextureId::Managed(0))
        .expect("egui font atlas upload");
    assert_eq!(
        font_atlas.1.image.width(),
        1024,
        "Font atlas should use the minimum supported driver maximum"
    );
    let atlas_size = [font_atlas.1.image.width(), font_atlas.1.image.height()];
    println!("FONT_ATLAS_SIZE={atlas_size:?}");
    let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
    let mut glyph_vertex_count = 0;
    let mut touching_outer_half_texel = 0;
    let mut affected_uvs = Vec::new();
    let interior = [
        atlas_size[0].next_power_of_two(),
        atlas_size[1].next_power_of_two(),
    ];
    for primitive in &primitives {
        if let Primitive::Mesh(mesh) = &primitive.primitive {
            for vertex in &mesh.vertices {
                if vertex.uv == Pos2::ZERO {
                    continue;
                }
                glyph_vertex_count += 1;
                let texel = [
                    vertex.uv.x * atlas_size[0] as f32,
                    vertex.uv.y * atlas_size[1] as f32,
                ];
                if (0..2).any(|axis| texel[axis] < 0.5 || texel[axis] > interior[axis] as f32 - 0.5)
                {
                    touching_outer_half_texel += 1;
                    affected_uvs.push(texel);
                }
            }
        }
    }
    println!(
        "GLYPH_VERTICES={glyph_vertex_count}; OUTER_HALF_TEXEL_VERTICES={touching_outer_half_texel}; AFFECTED_TEXEL_UVS={affected_uvs:?}"
    );
    painter.clear(screen, [0.0, 0.0, 0.0, 1.0]);
    painter.paint_and_update_textures(
        screen,
        output.pixels_per_point,
        &primitives,
        &output.textures_delta,
    );
    unsafe { gl.finish() };
    let capture = painter.read_screen_rgba(screen);
    for (x, y) in [(21, 201), (649, 201), (21, 319), (649, 319), (600, 260)] {
        assert_eq!(
            capture.pixels[y * W + x],
            panel_color,
            "Real font atlas WHITE_UV panel mismatch at {x},{y}"
        );
    }
    let ink = (230..257)
        .flat_map(|y| (40..330).map(move |x| (x, y)))
        .filter(|&(x, y)| capture.pixels[y * W + x] != panel_color)
        .count();
    assert!(
        ink > 500,
        "Expected visibly rendered CJK and Latin glyphs, found {ink} pixels"
    );
    assert_no_gl_error(&gl, "CJK font atlas and solid panels");
    println!(
        "PASS: actual Chinese/Latin font glyphs ({ink} ink pixels), 1024-wide atlas, panel colors exact; font={font_path}"
    );
    if let Ok(path) = std::env::var("VALIDATION_CAPTURE_FILE") {
        let pixels = capture
            .pixels
            .iter()
            .flat_map(|pixel| pixel.to_array())
            .collect::<Vec<_>>();
        let frame = image::RgbaImage::from_raw(W as u32, H as u32, pixels).unwrap();
        image::imageops::crop_imm(&frame, 20, 200, 630, 120)
            .to_image()
            .save(&path)
            .expect("Save font rendering capture");
        println!("FONT_PANEL_CAPTURE={path}");
    }
    painter.destroy();
    assert_no_gl_error(&gl, "destroy");
}

fn imgbridge_image(raw: Vec<u8>) -> image::RgbImage {
    image::RgbImage::from_raw(360, 180, raw).unwrap()
}
