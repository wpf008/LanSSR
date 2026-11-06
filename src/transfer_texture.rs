//! Upload the screen codec without constraining it to a single GPU texture.

use eframe::egui;
use image::RgbImage;

struct Tile {
    offset: [usize; 2],
    texture: egui::TextureHandle,
}

/// Nearest-filtered tiles of one codec image. Only `set` uploads pixels; painting
/// an unchanged calibration image leaves its existing GPU textures untouched.
pub(crate) struct TransferTexture {
    ctx: egui::Context,
    size: [usize; 2],
    tile_side: usize,
    tiles: Vec<Tile>,
}

impl TransferTexture {
    pub(crate) fn new(ctx: &egui::Context, image: &RgbImage) -> Self {
        let mut texture = Self {
            ctx: ctx.clone(),
            size: [0, 0],
            tile_side: 0,
            tiles: Vec::new(),
        };
        texture.set(image);
        texture
    }

    #[cfg(test)]
    pub(crate) fn size(&self) -> [usize; 2] {
        self.size
    }

    #[cfg(test)]
    pub(crate) fn contains_texture(&self, id: egui::TextureId) -> bool {
        self.tiles.iter().any(|tile| tile.texture.id() == id)
    }

    pub(crate) fn set(&mut self, image: &RgbImage) {
        let size = [image.width() as usize, image.height() as usize];
        let limit = self.ctx.input(|input| input.max_texture_side);
        // The OpenGL 1.1 fallback rounds backing textures up to a power of two.
        // Even an unusual reported limit must leave room for that allocation.
        let tile_side = power_of_two_tile_side(limit);
        let rebuild = size != self.size || tile_side != self.tile_side;
        if rebuild {
            // Drop every previous handle: the next output will free the old
            // backing textures after painting the replacement image.
            self.tiles.clear();
            self.size = size;
            self.tile_side = tile_side;
        }

        let mut tile_index = 0;
        for y in (0..size[1]).step_by(tile_side) {
            for x in (0..size[0]).step_by(tile_side) {
                let tile_size = [tile_side.min(size[0] - x), tile_side.min(size[1] - y)];
                let pixels = tile_image(image, [x, y], tile_size);
                if rebuild {
                    self.tiles.push(Tile {
                        offset: [x, y],
                        texture: self.ctx.load_texture(
                            format!("transfer-grid-{x}-{y}"),
                            pixels,
                            egui::TextureOptions::NEAREST,
                        ),
                    });
                } else {
                    self.tiles[tile_index]
                        .texture
                        .set(pixels, egui::TextureOptions::NEAREST);
                }
                tile_index += 1;
            }
        }
    }

    /// `rect` describes the complete image in logical points at one image
    /// pixel per physical screen pixel. No tile or codec cell is rescaled.
    pub(crate) fn paint(&self, painter: &egui::Painter, rect: egui::Rect) {
        let scale = painter.pixels_per_point();
        let origin_pixels = (rect.min.to_vec2() * scale).round();
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        for tile in &self.tiles {
            let [x, y] = tile.offset;
            let [width, height] = tile.texture.size();
            // Compute both edges from absolute image offsets. Neighboring
            // tiles therefore share the identical floating-point boundary,
            // including fractional DPI and monitors with negative origins.
            let min = (origin_pixels + egui::vec2(x as f32, y as f32)) / scale;
            let max = (origin_pixels + egui::vec2((x + width) as f32, (y + height) as f32)) / scale;
            painter.image(
                tile.texture.id(),
                egui::Rect::from_min_max(min.to_pos2(), max.to_pos2()),
                uv,
                egui::Color32::WHITE,
            );
        }
    }
}

fn power_of_two_tile_side(limit: usize) -> usize {
    assert!(limit > 0, "the renderer must support a nonempty texture");
    1usize << (usize::BITS - 1 - limit.leading_zeros())
}

fn tile_image(image: &RgbImage, [x, y]: [usize; 2], size: [usize; 2]) -> egui::ColorImage {
    let stride = image.width() as usize * 3;
    let bytes = image.as_raw();
    let mut pixels = Vec::with_capacity(size[0] * size[1]);
    for row in y..y + size[1] {
        let start = row * stride + x * 3;
        pixels.extend(
            bytes[start..start + size[0] * 3]
                .as_chunks::<3>()
                .0
                .iter()
                .map(|rgb| egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2])),
        );
    }
    egui::ColorImage::new(size, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Grid;

    fn context(limit: usize) -> egui::Context {
        let ctx = egui::Context::default();
        ctx.input_mut(|input| input.max_texture_side = limit);
        // Ignore egui's initially reserved, empty font atlas in codec uploads.
        let _ = ctx.tex_manager().write().take_delta();
        ctx
    }

    fn reconstruct(texture: &TransferTexture, delta: &egui::TexturesDelta) -> RgbImage {
        let mut image = RgbImage::new(texture.size[0] as u32, texture.size[1] as u32);
        let mut coverage = vec![false; texture.size[0] * texture.size[1]];
        for tile in &texture.tiles {
            let update = delta
                .set
                .iter()
                .find(|(id, _)| *id == tile.texture.id())
                .expect("every tile must be uploaded in the same frame");
            assert_eq!(update.1.pos, None);
            assert_eq!(update.1.options, egui::TextureOptions::NEAREST);
            let egui::ImageData::Color(pixels) = &update.1.image;
            assert_eq!(pixels.size, tile.texture.size());
            for (index, pixel) in pixels.pixels.iter().enumerate() {
                let x = tile.offset[0] + index % pixels.size[0];
                let y = tile.offset[1] + index / pixels.size[0];
                let index = y * texture.size[0] + x;
                assert!(!coverage[index], "tiles must not overlap");
                coverage[index] = true;
                assert_eq!(pixel.a(), 255);
                image.put_pixel(
                    x as u32,
                    y as u32,
                    image::Rgb([pixel.r(), pixel.g(), pixel.b()]),
                );
            }
        }
        assert!(coverage.into_iter().all(|covered| covered));
        image
    }

    #[test]
    fn tiled_codec_preserves_every_pixel_and_decodes_at_legacy_limits() {
        // Six-pixel cells cross tile edges, so tiling must preserve their
        // pixel pattern even when a texture boundary cuts through a cell.
        for (limit, width, height, cell_size) in [
            (1024, 1936, 1088, 2),
            (1500, 1936, 1088, 2),
            (1000, 1936, 1088, 2),
            (1000, 1944, 1092, 6),
        ] {
            for water_mask in [false, true] {
                let ctx = context(limit);
                let grid = Grid::new(width, height, cell_size, water_mask).unwrap();
                let payload: Vec<u8> = (0..grid.payload_capacity())
                    .map(|index| ((index * 73 + index / 256) % 256) as u8)
                    .collect();
                let original = grid.encode(&payload).unwrap();
                let texture = TransferTexture::new(&ctx, &original);
                let delta = ctx.tex_manager().write().take_delta();
                assert!(texture.tiles.len() > 1);
                for tile in &texture.tiles {
                    assert!(
                        tile.texture
                            .size()
                            .into_iter()
                            .all(|side| { side <= limit && side.next_power_of_two() <= limit })
                    );
                }
                let reconstructed = reconstruct(&texture, &delta);
                assert_eq!(reconstructed.as_raw(), original.as_raw());
                assert_eq!(grid.decode(&reconstructed).unwrap(), payload);

                // Calibration is also a pixel-exact black/white frame.
                let checkerboard = grid.checkerboard();
                let _ = texture.ctx.tex_manager().write().take_delta();
                let mut texture = texture;
                texture.set(&checkerboard);
                let delta = ctx.tex_manager().write().take_delta();
                assert!(delta.free.is_empty());
                assert_eq!(
                    reconstruct(&texture, &delta).as_raw(),
                    checkerboard.as_raw()
                );
            }
        }
    }

    #[test]
    fn updates_reuse_handles_and_shape_or_limit_changes_release_them() {
        let ctx = context(1024);
        let first = RgbImage::new(1936, 1088);
        let mut texture = TransferTexture::new(&ctx, &first);
        let ids: Vec<_> = texture.tiles.iter().map(|tile| tile.texture.id()).collect();
        let _ = ctx.tex_manager().write().take_delta();
        texture.set(&RgbImage::from_pixel(
            1936,
            1088,
            image::Rgb([31, 127, 255]),
        ));
        let delta = ctx.tex_manager().write().take_delta();
        assert_eq!(delta.set.iter().map(|(id, _)| *id).collect::<Vec<_>>(), ids);
        assert!(delta.free.is_empty());

        let second = RgbImage::new(1024, 1024);
        texture.set(&second);
        let delta = ctx.tex_manager().write().take_delta();
        assert_eq!(delta.free, ids);
        assert_eq!(texture.tiles.len(), 1);
        assert_eq!(texture.size(), [1024, 1024]);
        let second_id = texture.tiles[0].texture.id();
        assert!(!ids.contains(&second_id));
        for id in &ids {
            assert!(ctx.tex_manager().read().meta(*id).is_none());
        }

        ctx.input_mut(|input| input.max_texture_side = 1000);
        texture.set(&second);
        let delta = ctx.tex_manager().write().take_delta();
        assert_eq!(delta.free, [second_id]);
        assert_eq!(texture.tiles.len(), 4);
        assert!(
            texture
                .tiles
                .iter()
                .all(|tile| tile.texture.size() == [512, 512])
        );
        let ids: Vec<_> = texture.tiles.iter().map(|tile| tile.texture.id()).collect();
        drop(texture);
        assert_eq!(ctx.tex_manager().write().take_delta().free, ids);
    }

    #[test]
    fn full_size_4k_frame_never_uploads_an_oversized_texture() {
        let ctx = context(1024);
        let image = RgbImage::new(3840, 2040);
        let texture = TransferTexture::new(&ctx, &image);
        let delta = ctx.tex_manager().write().take_delta();
        assert_eq!(texture.size(), [3840, 2040]);
        assert_eq!(texture.tiles.len(), 8);
        assert_eq!(delta.set.len(), 8);
        assert_eq!(
            delta
                .set
                .iter()
                .map(|(_, update)| update.image.size().into_iter().product::<usize>())
                .sum::<usize>(),
            3840 * 2040
        );
        assert!(
            delta.set.iter().all(|(_, update)| update
                .image
                .size()
                .into_iter()
                .all(|side| side <= 1024))
        );
    }

    #[test]
    fn painted_tiles_share_edges_and_preserve_physical_pixels_at_every_dpi() {
        for scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
            let ctx = context(1024);
            let image = RgbImage::new(2208, 1320);
            let texture = TransferTexture::new(&ctx, &image);
            let _ = ctx.tex_manager().write().take_delta();
            let physical_origin = egui::vec2(-1776.0, -40.0);
            let rect = egui::Rect::from_min_size(
                (physical_origin / scale).to_pos2(),
                egui::vec2(2208.0, 1320.0) / scale,
            );
            let mut input = egui::RawInput {
                screen_rect: Some(rect.expand(100.0)),
                ..Default::default()
            };
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .native_pixels_per_point = Some(scale);
            let output = ctx.run(input, |ctx| {
                texture.paint(&ctx.layer_painter(egui::LayerId::background()), rect);
            });
            let mut bounds = Vec::new();
            for tile in &texture.tiles {
                let mesh = output
                    .shapes
                    .iter()
                    .find_map(|clipped| match &clipped.shape {
                        egui::Shape::Mesh(mesh) if mesh.texture_id == tile.texture.id() => {
                            Some(mesh)
                        }
                        _ => None,
                    })
                    .expect("each tile must have its own image mesh");
                assert_eq!(mesh.vertices.len(), 4);
                assert!(
                    mesh.vertices
                        .iter()
                        .all(|vertex| vertex.color == egui::Color32::WHITE)
                );
                let bounds_pixels = mesh.calc_bounds() * scale;
                let expected_min =
                    physical_origin + egui::vec2(tile.offset[0] as f32, tile.offset[1] as f32);
                let [width, height] = tile.texture.size();
                let expected_max = expected_min + egui::vec2(width as f32, height as f32);
                assert!((bounds_pixels.min.to_vec2() - expected_min).length() < 0.001);
                assert!((bounds_pixels.max.to_vec2() - expected_max).length() < 0.001);
                bounds.push(mesh.calc_bounds());
            }
            assert_eq!(bounds[0].right(), bounds[1].left());
            assert_eq!(bounds[1].right(), bounds[2].left());
            for column in 0..3 {
                assert_eq!(bounds[column].bottom(), bounds[column + 3].top());
            }
            let union = bounds.into_iter().reduce(|a, b| a.union(b)).unwrap();
            assert!((union.min - rect.min).length() < 0.001);
            assert!((union.max - rect.max).length() < 0.001);
            let ids: Vec<_> = texture.tiles.iter().map(|tile| tile.texture.id()).collect();
            assert!(
                output
                    .textures_delta
                    .set
                    .iter()
                    .all(|(id, _)| !ids.contains(id)),
                "painting must not upload unchanged codec pixels"
            );
        }
    }
}
