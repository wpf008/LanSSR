//! Desktop OpenGL 1.1 fixed-function rendering for the Windows GDI fallback.
//!
//! No extension, shader, VBO, FBO or external GPU runtime is required. Managed
//! textures use a power-of-two interior and a one-texel OpenGL border. Replicated
//! edges keep both linear filtering and egui's white texel at UV (0, 0) correct.

#![allow(unsafe_code)]

use std::{collections::HashMap, ffi::c_void, mem::size_of};

use egui::epaint::{ImageDelta, Mesh, Vertex};
use glow::HasContext as _;
use libloading::Library;
use memoffset::offset_of;

// Removed from the modern core GL header used by glow, but all are core 1.1.
const GL_PROJECTION: u32 = 0x1701;
const GL_MODELVIEW: u32 = 0x1700;
const GL_TEXTURE: u32 = 0x1702;
const GL_TEXTURE_ENV: u32 = 0x2300;
const GL_TEXTURE_ENV_MODE: u32 = 0x2200;
const GL_MODULATE: i32 = 0x2100;
const GL_SMOOTH: u32 = 0x1D01;
const GL_CLAMP: i32 = 0x2900;
const GL_TEXTURE_BORDER: u32 = 0x1005;
const GL_ALPHA_TEST: u32 = 0x0BC0;
const GL_LIGHTING: u32 = 0x0B50;
const GL_FOG: u32 = 0x0B60;
const GL_NORMAL_ARRAY: u32 = 0x8075;
const GL_COLOR_ARRAY: u32 = 0x8076;
const GL_INDEX_ARRAY: u32 = 0x8077;
const GL_TEXTURE_COORD_ARRAY: u32 = 0x8078;
const GL_EDGE_FLAG_ARRAY: u32 = 0x8079;

type MatrixMode = unsafe extern "system" fn(u32);
type LoadIdentity = unsafe extern "system" fn();
type Ortho = unsafe extern "system" fn(f64, f64, f64, f64, f64, f64);
type TexEnvi = unsafe extern "system" fn(u32, u32, i32);
type ShadeModel = unsafe extern "system" fn(u32);
type ClientState = unsafe extern "system" fn(u32);
type ArrayPointer = unsafe extern "system" fn(i32, u32, i32, *const c_void);
type DrawElements = unsafe extern "system" fn(u32, i32, u32, *const c_void);
type GetTexLevelParameteriv = unsafe extern "system" fn(u32, i32, u32, *mut i32);

/// The loaded library stays alive for at least as long as its function pointers.
pub(crate) struct LegacyPainter {
    _library: Library,
    matrix_mode: MatrixMode,
    load_identity: LoadIdentity,
    ortho: Ortho,
    tex_envi: TexEnvi,
    shade_model: ShadeModel,
    enable_client_state: ClientState,
    disable_client_state: ClientState,
    vertex_pointer: ArrayPointer,
    color_pointer: ArrayPointer,
    tex_coord_pointer: ArrayPointer,
    draw_elements: DrawElements,
    get_tex_level_parameter_iv: GetTexLevelParameteriv,
    max_texture_side: usize,
    supports_edge_clamp: bool,
    textures: HashMap<egui::TextureId, BorderedTexture>,
    tex_coords: Vec<[f32; 2]>,
}

impl LegacyPainter {
    pub(crate) fn new(gl: &glow::Context) -> Result<Self, String> {
        // These are OS libraries. In particular Windows' opengl32.dll exports
        // every fixed-function 1.1 entry point, even without a vendor driver.
        let library = unsafe { Library::new(system_gl_library()) }
            .map_err(|error| format!("Unable to load the system OpenGL 1.1 library: {error}"))?;

        macro_rules! load {
            ($name:literal, $ty:ty) => {
                unsafe {
                    *library
                        .get::<$ty>(concat!($name, "\0").as_bytes())
                        .map_err(|error| format!("Missing OpenGL 1.1 export {}: {error}", $name))?
                }
            };
        }

        let matrix_mode = load!("glMatrixMode", MatrixMode);
        let load_identity = load!("glLoadIdentity", LoadIdentity);
        let ortho = load!("glOrtho", Ortho);
        let tex_envi = load!("glTexEnvi", TexEnvi);
        let shade_model = load!("glShadeModel", ShadeModel);
        let enable_client_state = load!("glEnableClientState", ClientState);
        let disable_client_state = load!("glDisableClientState", ClientState);
        let vertex_pointer = load!("glVertexPointer", ArrayPointer);
        let color_pointer = load!("glColorPointer", ArrayPointer);
        let tex_coord_pointer = load!("glTexCoordPointer", ArrayPointer);
        let draw_elements = load!("glDrawElements", DrawElements);
        let get_tex_level_parameter_iv = load!("glGetTexLevelParameteriv", GetTexLevelParameteriv);
        let max_texture_side = unsafe { gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) };
        let max_texture_side = usize::try_from(max_texture_side)
            .ok()
            .filter(|&size| size > 0)
            .ok_or_else(|| "OpenGL 1.1 did not report a usable texture size".to_owned())?;
        let version = gl.version();
        let extensions = gl.supported_extensions();
        let supports_edge_clamp = version.major > 1
            || (version.major == 1 && version.minor >= 2)
            || extensions.contains("GL_EXT_texture_edge_clamp")
            || extensions.contains("GL_SGIS_texture_edge_clamp");

        Ok(Self {
            _library: library,
            matrix_mode,
            load_identity,
            ortho,
            tex_envi,
            shade_model,
            enable_client_state,
            disable_client_state,
            vertex_pointer,
            color_pointer,
            tex_coord_pointer,
            draw_elements,
            get_tex_level_parameter_iv,
            max_texture_side,
            supports_edge_clamp,
            textures: HashMap::new(),
            tex_coords: Vec::new(),
        })
    }

    pub(crate) fn prepare(&self, gl: &glow::Context, screen: [u32; 2], ppp: f32) {
        if !ppp.is_finite() || ppp <= 0.0 || screen.contains(&0) {
            return;
        }
        let width = f64::from(screen[0]) / f64::from(ppp);
        let height = f64::from(screen[1]) / f64::from(ppp);

        unsafe {
            gl.viewport(0, 0, screen[0] as i32, screen[1] as i32);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::STENCIL_TEST);
            gl.disable(glow::DITHER);
            gl.disable(GL_ALPHA_TEST);
            gl.disable(GL_LIGHTING);
            gl.disable(GL_FOG);
            gl.disable(glow::POLYGON_SMOOTH);
            gl.disable(glow::LINE_SMOOTH);
            gl.disable(glow::TEXTURE_1D);
            gl.enable(glow::TEXTURE_2D);
            gl.enable(glow::SCISSOR_TEST);
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.color_mask(true, true, true, true);

            (self.shade_model)(GL_SMOOTH);
            (self.tex_envi)(GL_TEXTURE_ENV, GL_TEXTURE_ENV_MODE, GL_MODULATE);
            (self.matrix_mode)(GL_PROJECTION);
            (self.load_identity)();
            (self.ortho)(0.0, width, height, 0.0, -1.0, 1.0);
            (self.matrix_mode)(GL_MODELVIEW);
            (self.load_identity)();
            (self.matrix_mode)(GL_TEXTURE);
            (self.load_identity)();
            (self.matrix_mode)(GL_MODELVIEW);

            // No enabled client array may retain a pointer to a prior mesh.
            for array in [
                glow::VERTEX_ARRAY,
                GL_COLOR_ARRAY,
                GL_TEXTURE_COORD_ARRAY,
                GL_NORMAL_ARRAY,
                GL_INDEX_ARRAY,
                GL_EDGE_FLAG_ARRAY,
            ] {
                (self.disable_client_state)(array);
            }
        }
    }

    pub(crate) fn paint_mesh(&mut self, gl: &glow::Context, mesh: &Mesh, texture: glow::Texture) {
        if mesh.indices.is_empty() || mesh.vertices.is_empty() {
            return;
        }
        debug_assert!(mesh.is_valid(), "Mesh is not valid");
        let Ok(count) = i32::try_from(mesh.indices.len()) else {
            log::error!("OpenGL 1.1 mesh has too many indices");
            return;
        };

        // Native textures have no padding metadata and keep their full UV range.
        let transform = self
            .textures
            .get(&mesh.texture_id)
            .map_or_else(TextureUvTransform::native, |backing| {
                backing.uv_transform(self.supports_edge_clamp)
            });
        self.tex_coords.clear();
        self.tex_coords.extend(
            mesh.vertices
                .iter()
                .map(|vertex| transform.apply([vertex.uv.x, vertex.uv.y])),
        );

        let vertices = mesh.vertices.as_ptr().cast::<u8>();
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            (self.vertex_pointer)(
                2,
                glow::FLOAT,
                size_of::<Vertex>() as i32,
                vertices.add(offset_of!(Vertex, pos)).cast(),
            );
            (self.color_pointer)(
                4,
                glow::UNSIGNED_BYTE,
                size_of::<Vertex>() as i32,
                vertices.add(offset_of!(Vertex, color)).cast(),
            );
            (self.tex_coord_pointer)(
                2,
                glow::FLOAT,
                size_of::<[f32; 2]>() as i32,
                self.tex_coords.as_ptr().cast(),
            );
            (self.enable_client_state)(glow::VERTEX_ARRAY);
            (self.enable_client_state)(GL_COLOR_ARRAY);
            (self.enable_client_state)(GL_TEXTURE_COORD_ARRAY);
            // GL consumes client arrays before this call returns. All backing
            // vectors stay alive and immobile until after draw_elements.
            (self.draw_elements)(
                glow::TRIANGLES,
                count,
                glow::UNSIGNED_INT,
                mesh.indices.as_ptr().cast(),
            );
            (self.disable_client_state)(glow::VERTEX_ARRAY);
            (self.disable_client_state)(GL_COLOR_ARRAY);
            (self.disable_client_state)(GL_TEXTURE_COORD_ARRAY);
            (self.vertex_pointer)(2, glow::FLOAT, 0, std::ptr::null());
            (self.color_pointer)(4, glow::UNSIGNED_BYTE, 0, std::ptr::null());
            (self.tex_coord_pointer)(2, glow::FLOAT, 0, std::ptr::null());
        }
    }

    pub(crate) fn set_texture(
        &mut self,
        gl: &glow::Context,
        id: egui::TextureId,
        texture: glow::Texture,
        delta: &ImageDelta,
    ) {
        let egui::ImageData::Color(image) = &delta.image;
        let bytes: &[u8] = bytemuck::cast_slice(image.pixels.as_slice());
        let mut dirty = None;
        if let Some(pos) = delta.pos {
            let Some(backing) = self.textures.get_mut(&id) else {
                log::error!("OpenGL 1.1 cannot patch an unallocated texture {id:?}");
                return;
            };
            match backing.patch(pos, image.size, bytes) {
                Ok(rect) => dirty = Some(rect),
                Err(error) => {
                    log::error!("OpenGL 1.1 texture {id:?} patch rejected: {error}");
                    return;
                }
            }
        } else {
            match BorderedTexture::new(image.size, bytes, self.max_texture_side) {
                Ok(backing) => {
                    self.textures.insert(id, backing);
                }
                Err(error) => {
                    log::error!("OpenGL 1.1 texture {id:?} allocation rejected: {error}");
                    return;
                }
            }
        }

        let backing = self
            .textures
            .get_mut(&id)
            .expect("Texture was allocated above");
        backing.linear_filter = delta.options.magnification == egui::TextureFilter::Linear
            || delta.options.minification == egui::TextureFilter::Linear;
        // Mipmaps require another algorithm on 1.1, so retain the requested
        // base min/mag filter. LanSender's nearest filter remains exact.
        let filter = |filter| match filter {
            egui::TextureFilter::Nearest => glow::NEAREST as i32,
            egui::TextureFilter::Linear => glow::LINEAR as i32,
        };
        let wrap = match delta.options.wrap_mode {
            egui::TextureWrapMode::Repeat if backing.logical == backing.interior => {
                glow::REPEAT as i32
            }
            // Neither arbitrary NPOT repetition nor MIRRORED_REPEAT exists in
            // core 1.1. All application/atlas textures use clamp-to-edge.
            _ if self.supports_edge_clamp => glow::CLAMP_TO_EDGE as i32,
            _ => GL_CLAMP,
        };
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                filter(delta.options.magnification),
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                filter(delta.options.minification),
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, wrap);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, wrap);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
            gl.pixel_store_i32(glow::UNPACK_SKIP_ROWS, 0);
            gl.pixel_store_i32(glow::UNPACK_SKIP_PIXELS, 0);

            if let Some(rect) = dirty {
                let Some(rect) = backing.upload_rect(rect) else {
                    return;
                };
                let pixels = backing.rectangle_bytes(rect);
                // True 1.1 borders use offsets -1. Drivers that strip borders
                // only receive the interior, with nonnegative GL offsets.
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    rect.min[0] as i32 - 1,
                    rect.min[1] as i32 - 1,
                    rect.width() as i32,
                    rect.height() as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&pixels)),
                );
            } else {
                let [w, h] = backing.storage_size();
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA as i32,
                    w as i32,
                    h as i32,
                    1,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&backing.pixels)),
                );
                // Some compatibility drivers accept border=1, then silently
                // discard it. Query the actual allocation before later patches.
                let mut actual_border = 1;
                (self.get_tex_level_parameter_iv)(
                    glow::TEXTURE_2D,
                    0,
                    GL_TEXTURE_BORDER,
                    &mut actual_border,
                );
                backing.gpu_border = usize::from(actual_border != 0);
            }
        }
    }

    pub(crate) fn free_texture(&mut self, id: egui::TextureId) {
        self.textures.remove(&id);
    }
}

#[cfg(target_os = "windows")]
fn system_gl_library() -> &'static str {
    "opengl32.dll"
}

#[cfg(target_os = "macos")]
fn system_gl_library() -> &'static str {
    "/System/Library/Frameworks/OpenGL.framework/OpenGL"
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn system_gl_library() -> &'static str {
    "libGL.so.1"
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DirtyRect {
    /// Indices in bordered storage; GL offsets are these values minus one.
    min: [usize; 2],
    max: [usize; 2],
}

impl DirtyRect {
    fn width(self) -> usize {
        self.max[0] - self.min[0]
    }

    fn height(self) -> usize {
        self.max[1] - self.min[1]
    }
}

struct BorderedTexture {
    logical: [usize; 2],
    interior: [usize; 2],
    /// The driver may strip the requested border from the GPU allocation.
    gpu_border: usize,
    linear_filter: bool,
    /// RGBA storage: a one-texel GL border surrounds the padded POT interior.
    pixels: Vec<u8>,
}

impl BorderedTexture {
    fn new(logical: [usize; 2], bytes: &[u8], max_side: usize) -> Result<Self, String> {
        validate_pixels(logical, bytes)?;
        let interior = [
            logical[0].checked_next_power_of_two(),
            logical[1].checked_next_power_of_two(),
        ];
        let [Some(w), Some(h)] = interior else {
            return Err("Texture dimensions overflow".to_owned());
        };
        if w > max_side || h > max_side {
            return Err(format!(
                "Texture {}x{} needs a {w}x{h} interior; OpenGL supports {max_side}",
                logical[0], logical[1]
            ));
        }
        let length = w
            .checked_add(2)
            .and_then(|width| {
                let height = h.checked_add(2)?;
                width.checked_mul(height)
            })
            .and_then(|area| area.checked_mul(4))
            .ok_or_else(|| "Texture allocation overflows".to_owned())?;
        let mut texture = Self {
            logical,
            interior: [w, h],
            gpu_border: 1,
            linear_filter: false,
            pixels: vec![0; length],
        };
        let rect = DirtyRect {
            min: [0, 0],
            max: texture.storage_size(),
        };
        texture.copy_patch(rect, [0, 0], logical, bytes);
        Ok(texture)
    }

    fn storage_size(&self) -> [usize; 2] {
        [self.interior[0] + 2, self.interior[1] + 2]
    }

    fn uv_scale(&self) -> [f32; 2] {
        [
            self.logical[0] as f32 / self.interior[0] as f32,
            self.logical[1] as f32 / self.interior[1] as f32,
        ]
    }

    fn uv_transform(&self, supports_edge_clamp: bool) -> TextureUvTransform {
        let clamp = (self.gpu_border == 0 && !supports_edge_clamp && self.linear_filter)
            .then(|| self.interior.map(|side| 0.5 / side as f32));
        TextureUvTransform {
            scale: self.uv_scale(),
            clamp,
        }
    }

    fn upload_rect(&self, rect: DirtyRect) -> Option<DirtyRect> {
        let rect = if self.gpu_border == 0 {
            DirtyRect {
                min: [rect.min[0].max(1), rect.min[1].max(1)],
                max: [
                    rect.max[0].min(self.interior[0] + 1),
                    rect.max[1].min(self.interior[1] + 1),
                ],
            }
        } else {
            rect
        };
        (rect.min[0] < rect.max[0] && rect.min[1] < rect.max[1]).then_some(rect)
    }

    fn patch(
        &mut self,
        pos: [usize; 2],
        size: [usize; 2],
        bytes: &[u8],
    ) -> Result<DirtyRect, String> {
        validate_pixels(size, bytes)?;
        let end = [pos[0].checked_add(size[0]), pos[1].checked_add(size[1])];
        let [Some(end_x), Some(end_y)] = end else {
            return Err("Texture patch coordinates overflow".to_owned());
        };
        if end_x > self.logical[0] || end_y > self.logical[1] {
            return Err("Texture patch extends outside its logical image".to_owned());
        }
        let storage = self.storage_size();
        let rect = DirtyRect {
            min: [
                if pos[0] == 0 { 0 } else { pos[0] + 1 },
                if pos[1] == 0 { 0 } else { pos[1] + 1 },
            ],
            max: [
                if end_x == self.logical[0] {
                    storage[0]
                } else {
                    end_x + 1
                },
                if end_y == self.logical[1] {
                    storage[1]
                } else {
                    end_y + 1
                },
            ],
        };
        self.copy_patch(rect, pos, size, bytes);
        Ok(rect)
    }

    fn copy_patch(&mut self, rect: DirtyRect, pos: [usize; 2], size: [usize; 2], bytes: &[u8]) {
        let stride = self.storage_size()[0];
        for y in rect.min[1]..rect.max[1] {
            let source_y = y.saturating_sub(1).min(self.logical[1] - 1) - pos[1];
            let target_row = &mut self.pixels[y * stride * 4..(y + 1) * stride * 4];
            let source_row = &bytes[source_y * size[0] * 4..(source_y + 1) * size[0] * 4];

            if rect.min[0] == 0 {
                target_row[..4].copy_from_slice(&source_row[..4]);
            }
            // Copy real pixels as a contiguous row; transmission frames should
            // not pay a per-pixel padding cost when most texels are unchanged.
            let start = rect.min[0].max(1);
            let end = rect.max[0].min(self.logical[0] + 1);
            if start < end {
                let source_start = (start - 1 - pos[0]) * 4;
                let source_end = source_start + (end - start) * 4;
                target_row[start * 4..end * 4]
                    .copy_from_slice(&source_row[source_start..source_end]);
            }
            if rect.max[0] > self.logical[0] + 1 {
                let last = (self.logical[0] - 1 - pos[0]) * 4;
                let edge = &source_row[last..last + 4];
                let (padding, _) =
                    target_row[(self.logical[0] + 1) * 4..rect.max[0] * 4].as_chunks_mut::<4>();
                for pixel in padding {
                    pixel.copy_from_slice(edge);
                }
            }
        }
    }

    fn rectangle_bytes(&self, rect: DirtyRect) -> Vec<u8> {
        let stride = self.storage_size()[0] * 4;
        let mut bytes = Vec::with_capacity(rect.width() * rect.height() * 4);
        for y in rect.min[1]..rect.max[1] {
            bytes.extend_from_slice(
                &self.pixels[y * stride + rect.min[0] * 4..y * stride + rect.max[0] * 4],
            );
        }
        bytes
    }
}

struct TextureUvTransform {
    scale: [f32; 2],
    /// Only for nonconforming drivers that strip borders without edge-clamp.
    /// Clamping vertices slightly changes a linear full-image mapping at its
    /// outer half texel, but preserves `WHITE_UV` and padded glyph coordinates.
    /// Nearest transmission textures keep their original UVs in every case.
    clamp: Option<[f32; 2]>,
}

impl TextureUvTransform {
    fn native() -> Self {
        Self {
            scale: [1.0, 1.0],
            clamp: None,
        }
    }

    fn apply(&self, uv: [f32; 2]) -> [f32; 2] {
        let mut transformed = [uv[0] * self.scale[0], uv[1] * self.scale[1]];
        if let Some(half_texel) = self.clamp {
            for axis in 0..2 {
                transformed[axis] =
                    transformed[axis].clamp(half_texel[axis], 1.0 - half_texel[axis]);
            }
        }
        transformed
    }
}

fn validate_pixels(size: [usize; 2], bytes: &[u8]) -> Result<(), String> {
    if size.contains(&0) {
        return Err("Texture dimensions must be nonzero".to_owned());
    }
    let expected = size[0]
        .checked_mul(size[1])
        .and_then(|area| area.checked_mul(4));
    if expected != Some(bytes.len()) {
        return Err("Texture dimensions do not match the RGBA byte count".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{BorderedTexture, DirtyRect};

    fn pixels(values: &[u8]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|&value| [value, value, value, 255])
            .collect()
    }

    fn values(texture: &BorderedTexture) -> Vec<u8> {
        texture
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| pixel[0])
            .collect()
    }

    #[test]
    fn non_power_of_two_texture_replicates_every_edge_and_border() {
        let texture = BorderedTexture::new([3, 2], &pixels(&[1, 2, 3, 4, 5, 6]), 4).unwrap();
        assert_eq!(texture.interior, [4, 2]);
        assert_eq!(texture.storage_size(), [6, 4]);
        assert_eq!(
            values(&texture),
            [
                1, 1, 2, 3, 3, 3, 1, 1, 2, 3, 3, 3, 4, 4, 5, 6, 6, 6, 4, 4, 5, 6, 6, 6
            ]
        );
        assert_eq!(texture.uv_scale(), [0.75, 1.0]);
    }

    #[test]
    fn full_power_of_two_texture_can_use_the_reported_maximum() {
        let texture = BorderedTexture::new([4, 4], &pixels(&[7; 16]), 4).unwrap();
        assert_eq!(texture.interior, [4, 4]);
        // Border storage is deliberately larger than GL_MAX_TEXTURE_SIZE;
        // that limit applies to the POT interior in the OpenGL 1.1 API.
        assert_eq!(texture.storage_size(), [6, 6]);
        assert_eq!(texture.uv_scale(), [1.0, 1.0]);
    }

    #[test]
    fn internal_patch_does_not_modify_unrelated_pixels_or_borders() {
        let mut texture =
            BorderedTexture::new([3, 3], &pixels(&[1, 2, 3, 4, 5, 6, 7, 8, 9]), 4).unwrap();
        let before = texture.pixels.clone();
        let dirty = texture.patch([1, 1], [1, 1], &pixels(&[50])).unwrap();
        assert_eq!(
            dirty,
            DirtyRect {
                min: [2, 2],
                max: [3, 3]
            }
        );
        assert_eq!(texture.rectangle_bytes(dirty), pixels(&[50]));
        let mut expected = before;
        expected[(2 * 6 + 2) * 4..(2 * 6 + 3) * 4].copy_from_slice(&pixels(&[50]));
        assert_eq!(texture.pixels, expected);
    }

    #[test]
    fn edge_patch_updates_padded_texels_and_right_bottom_border() {
        let mut texture =
            BorderedTexture::new([3, 3], &pixels(&[1, 2, 3, 4, 5, 6, 7, 8, 9]), 4).unwrap();
        let dirty = texture.patch([2, 2], [1, 1], &pixels(&[99])).unwrap();
        assert_eq!(
            dirty,
            DirtyRect {
                min: [3, 3],
                max: [6, 6]
            }
        );
        assert_eq!(texture.rectangle_bytes(dirty), pixels(&[99; 9]));
        assert_eq!(values(&texture)[2 * 6 + 2], 5);
    }

    #[test]
    fn top_left_patch_refreshes_borders_used_by_white_uv() {
        let mut texture = BorderedTexture::new([3, 3], &pixels(&[0; 9]), 4).unwrap();
        let dirty = texture.patch([0, 0], [1, 1], &pixels(&[255])).unwrap();
        assert_eq!(
            dirty,
            DirtyRect {
                min: [0, 0],
                max: [2, 2]
            }
        );
        assert_eq!(texture.rectangle_bytes(dirty), pixels(&[255; 4]));
        assert_eq!(values(&texture)[2], 0);
    }

    #[test]
    fn stripped_border_clips_edge_patches_to_the_gpu_interior() {
        let mut texture = BorderedTexture::new([3, 3], &pixels(&[0; 9]), 4).unwrap();
        texture.gpu_border = 0;
        let scale = texture.uv_scale();
        let top_left = texture.patch([0, 0], [1, 1], &pixels(&[255])).unwrap();
        let upload = texture.upload_rect(top_left).unwrap();
        assert_eq!(
            upload,
            DirtyRect {
                min: [1, 1],
                max: [2, 2],
            }
        );
        assert_eq!(texture.rectangle_bytes(upload), pixels(&[255]));

        let bottom_right = texture.patch([2, 2], [1, 1], &pixels(&[99])).unwrap();
        let upload = texture.upload_rect(bottom_right).unwrap();
        assert_eq!(
            upload,
            DirtyRect {
                min: [3, 3],
                max: [5, 5],
            }
        );
        assert_eq!(texture.rectangle_bytes(upload), pixels(&[99; 4]));
        assert_eq!(texture.uv_scale(), scale);
    }

    #[test]
    fn upload_rect_keeps_real_borders_and_discards_stripped_border_only_rects() {
        let mut texture = BorderedTexture::new([4, 4], &pixels(&[7; 16]), 4).unwrap();
        let full = DirtyRect {
            min: [0, 0],
            max: texture.storage_size(),
        };
        assert_eq!(texture.upload_rect(full), Some(full));
        texture.gpu_border = 0;
        assert_eq!(
            texture.upload_rect(full),
            Some(DirtyRect {
                min: [1, 1],
                max: [5, 5],
            })
        );
        assert_eq!(
            texture.upload_rect(DirtyRect {
                min: [0, 0],
                max: [1, 1],
            }),
            None
        );
        assert_eq!(texture.uv_scale(), [1.0, 1.0]);
    }

    fn sample_borderless_linear(texture: &BorderedTexture, uv: [f32; 2]) -> f32 {
        let position = [
            uv[0] * texture.interior[0] as f32 - 0.5,
            uv[1] * texture.interior[1] as f32 - 0.5,
        ];
        let start = position.map(|value| value.floor() as isize);
        let fraction = [position[0] - start[0] as f32, position[1] - start[1] as f32];
        let mut value = 0.0;
        for dy in 0..2 {
            for dx in 0..2 {
                let x = start[0] + dx;
                let y = start[1] + dy;
                if x >= 0
                    && x < texture.interior[0] as isize
                    && y >= 0
                    && y < texture.interior[1] as isize
                {
                    let weight_x = if dx == 0 {
                        1.0 - fraction[0]
                    } else {
                        fraction[0]
                    };
                    let weight_y = if dy == 0 {
                        1.0 - fraction[1]
                    } else {
                        fraction[1]
                    };
                    let index = ((y as usize + 1) * texture.storage_size()[0] + x as usize + 1) * 4;
                    value += f32::from(texture.pixels[index]) * weight_x * weight_y;
                }
            }
        }
        value
    }

    #[test]
    fn borderless_linear_fallback_keeps_white_uv_at_full_intensity() {
        let mut texture = BorderedTexture::new([4, 4], &pixels(&[255; 16]), 4).unwrap();
        texture.gpu_border = 0;
        texture.linear_filter = true;
        assert_eq!(sample_borderless_linear(&texture, [0.0, 0.0]), 255.0 / 4.0);
        let transform = texture.uv_transform(false);
        for corner in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]] {
            assert_eq!(
                sample_borderless_linear(&texture, transform.apply(corner)),
                255.0
            );
        }
        assert_eq!(transform.apply([0.0, 0.0]), [0.125, 0.125]);
        assert_eq!(transform.apply([1.0, 1.0]), [0.875, 0.875]);
    }

    #[test]
    fn real_border_and_edge_clamp_paths_preserve_original_linear_uvs() {
        let mut texture = BorderedTexture::new([7, 5], &pixels(&[7; 35]), 8).unwrap();
        texture.linear_filter = true;
        for (border, edge_clamp) in [(1, false), (1, true), (0, true)] {
            texture.gpu_border = border;
            let transform = texture.uv_transform(edge_clamp);
            assert!(transform.clamp.is_none());
            for uv in [[0.0, 0.0], [0.25, 0.5], [1.0, 1.0]] {
                let scale = texture.uv_scale();
                assert_eq!(transform.apply(uv), [uv[0] * scale[0], uv[1] * scale[1]]);
            }
        }
    }

    #[test]
    fn borderless_fallback_preserves_nearest_pixels_and_padded_glyph_uvs() {
        let mut texture = BorderedTexture::new([7, 5], &pixels(&[7; 35]), 8).unwrap();
        texture.gpu_border = 0;
        let nearest = texture.uv_transform(false);
        assert!(nearest.clamp.is_none());
        assert_eq!(nearest.apply([0.0, 0.0]), [0.0, 0.0]);
        for y in 0..5 {
            for x in 0..7 {
                let uv = [(x as f32 + 0.5) / 7.0, (y as f32 + 0.5) / 5.0];
                let transformed = nearest.apply(uv);
                assert_eq!((transformed[0] * 8.0).floor() as usize, x);
                assert_eq!((transformed[1] * 8.0).floor() as usize, y);
            }
        }
        texture.linear_filter = true;
        let linear = texture.uv_transform(false);
        // Glyph rectangles are inset in the atlas, so their mapping is exact.
        for uv in [[1.0 / 7.0, 1.0 / 5.0], [6.0 / 7.0, 4.0 / 5.0]] {
            assert_eq!(linear.apply(uv), nearest.apply(uv));
        }
    }

    #[test]
    fn padding_and_uv_scaling_preserve_nearest_texel_centers() {
        let width = 7;
        let texture = BorderedTexture::new([width, 1], &pixels(&[0; 7]), 8).unwrap();
        let scale = texture.uv_scale()[0];
        for index in 0..width {
            let uv = (index as f32 + 0.5) / width as f32;
            let padded_texel = uv * scale * texture.interior[0] as f32;
            assert_eq!(padded_texel.floor() as usize, index);
        }
    }

    #[test]
    fn multiple_rectangle_patches_match_a_fresh_full_upload() {
        let size = [7, 5];
        let mut source: Vec<u8> = (0..35).collect();
        let mut texture = BorderedTexture::new(size, &pixels(&source), 8).unwrap();
        for (pos, patch_size, value) in [
            ([0, 0], [2, 3], 101),
            ([3, 1], [3, 2], 102),
            ([6, 4], [1, 1], 103),
            ([0, 4], [7, 1], 104),
            ([4, 0], [3, 5], 105),
        ] {
            let patch = vec![value; patch_size[0] * patch_size[1]];
            texture.patch(pos, patch_size, &pixels(&patch)).unwrap();
            for y in pos[1]..pos[1] + patch_size[1] {
                source[y * size[0] + pos[0]..y * size[0] + pos[0] + patch_size[0]].fill(value);
            }
            let full = BorderedTexture::new(size, &pixels(&source), 8).unwrap();
            assert_eq!(texture.pixels, full.pixels);
        }
    }

    #[test]
    fn rejects_invalid_or_oversized_images_before_mutating_backing() {
        assert!(BorderedTexture::new([0, 2], &[], 4).is_err());
        assert!(BorderedTexture::new([3, 2], &pixels(&[1; 5]), 4).is_err());
        assert!(BorderedTexture::new([5, 1], &pixels(&[1; 5]), 4).is_err());
        let mut texture = BorderedTexture::new([3, 2], &pixels(&[1; 6]), 4).unwrap();
        let before = texture.pixels.clone();
        assert!(texture.patch([2, 1], [2, 1], &pixels(&[5, 6])).is_err());
        assert!(
            texture
                .patch([usize::MAX, 0], [1, 1], &pixels(&[2]))
                .is_err()
        );
        assert_eq!(texture.pixels, before);
    }
}
