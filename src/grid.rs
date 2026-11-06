//! Java-compatible two-nibble cells with a red end-of-frame marker.

use anyhow::{Result, ensure};
use image::{Rgb, RgbImage};

/// Bound image allocation while allowing an 8K desktop-sized transfer area.
const MAX_PIXELS: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    pub width: u32,
    pub height: u32,
    pub cell_size: u32,
    pub water_mask: bool,
}

impl Grid {
    pub fn new(width: u32, height: u32, cell_size: u32, water_mask: bool) -> Result<Self> {
        let grid = Self {
            width,
            height,
            cell_size,
            water_mask,
        };
        grid.validate()?;
        Ok(grid)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.width > 0 && self.height > 0,
            "grid dimensions must be positive"
        );
        ensure!(self.cell_size > 0, "cell size must be positive");
        ensure!(
            self.width.is_multiple_of(self.cell_size) && self.height.is_multiple_of(self.cell_size),
            "grid dimensions must be multiples of cell size"
        );
        ensure!(
            !self.water_mask || (self.cell_size >= 2 && self.cell_size.is_multiple_of(2)),
            "watermark cells must have an even size of at least two pixels"
        );
        ensure!(
            u64::from(self.width) * u64::from(self.height) <= MAX_PIXELS,
            "grid exceeds the {MAX_PIXELS}-pixel limit"
        );
        ensure!(
            self.byte_capacity() > 6,
            "grid must contain at least fourteen cells for the frame header and red terminator"
        );
        Ok(())
    }

    /// Total whole bytes before reserving the frame header and terminator.
    pub fn byte_capacity(&self) -> usize {
        if self.cell_size == 0 {
            return 0;
        }
        let cells =
            u64::from(self.width / self.cell_size) * u64::from(self.height / self.cell_size);
        usize::try_from(cells / 2).unwrap_or(usize::MAX)
    }

    /// Java reserves six bytes: a five-byte header and a red terminator.
    pub fn payload_capacity(&self) -> usize {
        self.byte_capacity().saturating_sub(6)
    }

    pub fn encode(&self, bytes: &[u8]) -> Result<RgbImage> {
        self.validate()?;
        ensure!(
            bytes.len() < self.byte_capacity(),
            "frame needs {} bytes; grid allows {} plus a red terminator",
            bytes.len(),
            self.byte_capacity() - 1
        );
        let mut image = RgbImage::from_pixel(self.width, self.height, Rgb([255, 0, 0]));
        for (index, byte) in bytes.iter().enumerate() {
            self.draw_cell(&mut image, index * 2, byte >> 4);
            self.draw_cell(&mut image, index * 2 + 1, byte & 15);
        }
        Ok(image)
    }

    pub fn checkerboard(&self) -> RgbImage {
        // Public fields permit literal construction; avoid allocating an
        // unbounded image for an invalid literal.
        if self.validate().is_err() {
            return RgbImage::new(0, 0);
        }
        RgbImage::from_fn(self.width, self.height, |x, y| {
            let gray = if (x / self.cell_size + y / self.cell_size).is_multiple_of(2) {
                0
            } else {
                255
            };
            Rgb([gray, gray, gray])
        })
    }

    pub fn decode(&self, image: &RgbImage) -> Result<Vec<u8>> {
        self.validate()?;
        ensure!(
            image.width() == self.width && image.height() == self.height,
            "image dimensions {}x{} do not match grid {}x{}",
            image.width(),
            image.height(),
            self.width,
            self.height
        );
        let mut bytes = Vec::with_capacity(self.byte_capacity());
        for index in 0..self.byte_capacity() {
            let Some(high) = self.parse_cell(image, index * 2) else {
                break;
            };
            let Some(low) = self.parse_cell(image, index * 2 + 1) else {
                break;
            };
            bytes.push(high << 4 | low);
        }
        Ok(bytes)
    }

    fn cell_origin(&self, index: usize) -> (u32, u32) {
        let columns = (self.width / self.cell_size) as usize;
        (
            (index % columns) as u32 * self.cell_size,
            (index / columns) as u32 * self.cell_size,
        )
    }

    fn draw_cell(&self, image: &mut RgbImage, index: usize, value: u8) {
        let (x, y) = self.cell_origin(index);
        let half = self.cell_size / 2;
        for dy in 0..self.cell_size {
            for dx in 0..self.cell_size {
                let gray = if self.water_mask {
                    // bit3: upper left, bit2: upper right, bit1: lower
                    // left, bit0: lower right. A one is black.
                    let quadrant = u32::from(dy >= half) * 2 + u32::from(dx >= half);
                    if value & (1 << (3 - quadrant)) != 0 {
                        0
                    } else {
                        255
                    }
                } else {
                    value * 16 + 8
                };
                image.put_pixel(x + dx, y + dy, Rgb([gray, gray, gray]));
            }
        }
    }

    fn parse_cell(&self, image: &RgbImage, index: usize) -> Option<u8> {
        let (x, y) = self.cell_origin(index);
        if self.water_mask {
            let half = self.cell_size / 2;
            let mut value = 0;
            for quadrant in 0..4 {
                let sx = x + quadrant % 2 * half;
                let sy = y + quadrant / 2 * half;
                if average_gray(image, sx, sy, half)? < 128 {
                    value |= 1 << (3 - quadrant);
                }
            }
            Some(value)
        } else {
            Some((average_gray(image, x, y, self.cell_size)? / 16) as u8)
        }
    }
}

fn average_gray(image: &RgbImage, x: u32, y: u32, size: u32) -> Option<u32> {
    let mut sum = 0_u64;
    for dy in 0..size {
        for dx in 0..size {
            let [r, g, b] = image.get_pixel(x + dx, y + dy).0;
            if r > 200 && g < 50 && b < 50 {
                return None;
            }
            // Match Java's per-pixel truncation before averaging.
            let gray = if r == g && g == b {
                u32::from(r)
            } else {
                (0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b)) as u32
            };
            sum += u64::from(gray);
        }
    }
    Some((sum / (u64::from(size) * u64::from(size))) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Frame;

    #[test]
    fn grayscale_levels_and_red_suffix_match_java() {
        let grid = Grid::new(32, 16, 4, false).unwrap();
        let data = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
        let image = grid.encode(&data).unwrap();
        for nibble in 0..16 {
            let (x, y) = grid.cell_origin(nibble);
            assert_eq!(image.get_pixel(x, y).0, [nibble as u8 * 16 + 8; 3]);
        }
        let (x, y) = grid.cell_origin(16);
        assert_eq!(image.get_pixel(x, y).0, [255, 0, 0]);
        assert_eq!(grid.decode(&image).unwrap(), data);
    }

    #[test]
    fn watermark_quadrants_and_all_bytes_round_trip() {
        let grid = Grid::new(128, 128, 4, true).unwrap();
        let data: Vec<u8> = (0..=255).collect();
        let image = grid.encode(&data).unwrap();
        assert_eq!(grid.decode(&image).unwrap(), data);
        let pattern = grid.encode(&[0xa5]).unwrap();
        // 0xA = 1010, 0x5 = 0101.
        for (x, y, expected) in [
            (0, 0, 0),
            (2, 0, 255),
            (0, 2, 0),
            (2, 2, 255),
            (4, 0, 255),
            (6, 0, 0),
            (4, 2, 255),
            (6, 2, 0),
        ] {
            assert_eq!(pattern.get_pixel(x, y).0, [expected; 3]);
        }
    }

    #[test]
    fn reserves_terminator_with_even_and_odd_cell_counts() {
        for grid in [
            Grid::new(28, 8, 4, false).unwrap(),
            Grid::new(20, 12, 4, false).unwrap(),
        ] {
            assert_eq!(grid.byte_capacity(), 7);
            assert_eq!(grid.payload_capacity(), 1);
            let bytes = [1, 2, 3, 4, 5, 6];
            assert_eq!(grid.decode(&grid.encode(&bytes).unwrap()).unwrap(), bytes);
            assert!(grid.encode(&[0; 7]).is_err());
        }
    }

    #[test]
    fn average_gray_tolerates_noise_and_any_red_pixel_terminates() {
        let grid = Grid::new(32, 16, 4, false).unwrap();
        let mut image = grid.encode(&[0xab, 0xcd]).unwrap();
        image.put_pixel(0, 0, Rgb([160, 160, 160]));
        image.put_pixel(1, 0, Rgb([175, 175, 175]));
        assert_eq!(grid.decode(&image).unwrap(), [0xab, 0xcd]);
        image.put_pixel(8, 0, Rgb([201, 49, 49]));
        assert_eq!(grid.decode(&image).unwrap(), [0xab]);
        // The same red rule applies inside watermark quadrants.
        let grid = Grid::new(32, 16, 4, true).unwrap();
        let mut image = grid.encode(&[0xab, 0xcd]).unwrap();
        image.put_pixel(10, 2, Rgb([255, 0, 0]));
        assert_eq!(grid.decode(&image).unwrap(), [0xab]);
    }

    #[test]
    fn frame_checksum_catches_corrupt_cells() {
        for water_mask in [false, true] {
            let grid = Grid::new(64, 32, 4, water_mask).unwrap();
            let frame = Frame::Data {
                number: 7,
                payload: b"hello".to_vec(),
            };
            let mut image = grid.encode(&frame.encode().unwrap()).unwrap();
            assert_eq!(Frame::decode(&grid.decode(&image).unwrap()).unwrap(), frame);
            grid.draw_cell(&mut image, 10, 0);
            assert!(Frame::decode(&grid.decode(&image).unwrap()).is_err());
        }
    }

    #[test]
    fn validates_grid_and_image_dimensions() {
        for (width, height, cell, water) in [
            (0, 20, 4, false),
            (20, 0, 4, false),
            (20, 20, 0, false),
            (21, 20, 4, false),
            (20, 20, 3, true),
            (4, 4, 4, false),
            (u32::MAX, u32::MAX, 1, false),
        ] {
            assert!(Grid::new(width, height, cell, water).is_err());
        }
        let grid = Grid::new(32, 16, 4, false).unwrap();
        let small_watermark = Grid::new(8, 8, 2, true).unwrap();
        let data = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab];
        assert_eq!(
            small_watermark
                .decode(&small_watermark.encode(&data).unwrap())
                .unwrap(),
            data,
        );
        assert!(grid.decode(&RgbImage::new(16, 32)).is_err());
        let checker = grid.checkerboard();
        assert_eq!(checker.get_pixel(0, 0).0, [0; 3]);
        assert_eq!(checker.get_pixel(4, 0).0, [255; 3]);
        assert_eq!(checker.get_pixel(0, 4).0, [255; 3]);
        assert_eq!(checker.get_pixel(4, 4).0, [0; 3]);
    }
}
