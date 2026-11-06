//! Locate the complete calibration checkerboard in a captured display.
//!
//! Desktop senders can fit the number of cells to the available screen area.
//! Infer their complete, quantized bounds before trying legacy fixed geometry;
//! otherwise a large panel would be mistaken for a smaller panel inside it.

use std::collections::BTreeSet;

use image::RgbImage;

use crate::{config::Config, grid::Grid};

#[derive(Clone, Copy, Debug)]
pub struct LocatedGrid {
    pub x: u32,
    pub y: u32,
    pub grid: Grid,
}

#[derive(Clone, Copy)]
struct Run {
    start: u32,
    length: u32,
    color: u8,
}

const BLACK: u8 = 0;
const WHITE: u8 = 1;
const UNKNOWN: u8 = 2;

// Desktop geometry uses four-cell steps. A substantial two-dimensional panel
// distinguishes calibration from icons, ordinary tables, and UI separators.
const DYNAMIC_STEP: u32 = 4;
const MIN_DYNAMIC_COLUMNS: u32 = 160;
const MIN_DYNAMIC_ROWS: u32 = 80;

/// Find a calibration panel, including its first and last rows and columns.
///
/// Both axes need at least two cells to distinguish calibration from stripes.
/// Colors may have moderate compression noise, but cells must have a uniform
/// integer size: fractional or nonuniform scaling cannot be decoded by `Grid`.
pub fn find_grid(image: &RgbImage, config: &Config) -> Option<LocatedGrid> {
    if config.cell_size == 0 {
        return None;
    }
    let columns = config.width / config.cell_size;
    let rows = config.height / config.cell_size;
    if columns < 2 || rows < 2 || image.width() == 0 || image.height() == 0 {
        return None;
    }

    let width = image.width() as usize;
    let height = image.height() as usize;
    let pixels: Vec<u8> = image
        .pixels()
        .map(|pixel| {
            let [r, g, b] = pixel.0;
            // Saturated UI colors and mid-gray areas should not form cells.
            if r.max(g).max(b) - r.min(g).min(b) > 80 {
                return UNKNOWN;
            }
            let gray = (299 * r as u32 + 587 * g as u32 + 114 * b as u32) / 1000;
            if gray <= 100 {
                BLACK
            } else if gray >= 155 {
                WHITE
            } else {
                UNKNOWN
            }
        })
        .collect();

    // A horizontal run sequence gives its true cell size and only a few
    // possible origins. Deduplication avoids checking a candidate once per
    // scan line. The first/last cell can merge into the desktop background.
    let mut horizontal = BTreeSet::new();
    let mut dynamic_horizontal = BTreeSet::new();
    let mut runs = Vec::new();
    for line in pixels.chunks_exact(width) {
        collect_runs(line.iter().copied(), &mut runs);
        collect_origins(&runs, columns, image.width(), &mut horizontal);
        collect_dynamic_spans(
            &runs,
            MIN_DYNAMIC_COLUMNS,
            image.width(),
            &mut dynamic_horizontal,
        );
    }
    let mut dynamic_horizontal: Vec<_> = dynamic_horizontal.into_iter().collect();
    // Prefer the full-sized panel at the intended pixel scale. Quantized
    // boundaries leave only a handful of candidates per alternating sequence.
    dynamic_horizontal.sort_unstable_by_key(|&(x, cell, columns)| {
        (
            cell.abs_diff(config.cell_size),
            std::cmp::Reverse(columns),
            x,
        )
    });
    for (x, cell_size, columns) in dynamic_horizontal {
        let panel_width = columns * cell_size;
        let sample_x = (x + cell_size / 2) as usize;
        collect_runs((0..height).map(|y| pixels[y * width + sample_x]), &mut runs);
        let mut vertical = BTreeSet::new();
        collect_dynamic_spans(&runs, MIN_DYNAMIC_ROWS, image.height(), &mut vertical);
        for (y, observed_cell_size, rows) in vertical {
            if observed_cell_size != cell_size
                || !dynamic_geometry(columns, rows)
                || !validate_checkerboard(&pixels, width, height, x, y, cell_size, columns, rows)
                || checkerboard_extends(&pixels, width, height, x, y, cell_size, columns, rows)
            {
                continue;
            }
            if let Ok(grid) = Grid::new(panel_width, rows * cell_size, cell_size, config.water_mask)
            {
                return Some(LocatedGrid { x, y, grid });
            }
        }
    }
    let mut horizontal: Vec<_> = horizontal.into_iter().collect();
    horizontal.sort_unstable_by_key(|&(x, cell)| (cell.abs_diff(config.cell_size), x, cell));

    for (x, cell_size) in horizontal {
        let Some(panel_width) = columns.checked_mul(cell_size) else {
            continue;
        };
        let Some(panel_height) = rows.checked_mul(cell_size) else {
            continue;
        };
        if panel_height > image.height()
            || u64::from(x) + u64::from(panel_width) > u64::from(image.width())
        {
            continue;
        }

        // Looking down a cell center verifies vertical alternation before
        // doing the more expensive two-dimensional checkerboard validation.
        let sample_x = (x + cell_size / 2) as usize;
        collect_runs((0..height).map(|y| pixels[y * width + sample_x]), &mut runs);
        let mut vertical = BTreeSet::new();
        collect_origins(&runs, rows, image.height(), &mut vertical);
        for (y, observed_cell_size) in vertical {
            if observed_cell_size != cell_size {
                continue;
            }
            if !checkerboard_extends(&pixels, width, height, x, y, cell_size, columns, rows)
                && validate_checkerboard(&pixels, width, height, x, y, cell_size, columns, rows)
                && let Ok(grid) = Grid::new(panel_width, panel_height, cell_size, config.water_mask)
            {
                return Some(LocatedGrid { x, y, grid });
            }
        }
    }
    None
}

fn dynamic_geometry(columns: u32, rows: u32) -> bool {
    // Portrait and ultrawide desktops use the entire available rectangle.
    // Both axes still need substantial, quantized runs of equally sized cells;
    // the full interior and outside edges are validated before accepting it.
    columns >= MIN_DYNAMIC_COLUMNS
        && rows >= MIN_DYNAMIC_ROWS
        && columns.is_multiple_of(DYNAMIC_STEP)
        && rows.is_multiple_of(DYNAMIC_STEP)
}

fn collect_dynamic_spans(
    runs: &[Run],
    minimum_cells: u32,
    axis_length: u32,
    candidates: &mut BTreeSet<(u32, u32, u32)>,
) {
    let mut start = 0;
    while start < runs.len() {
        if runs[start].color == UNKNOWN {
            start += 1;
            continue;
        }
        let size = runs[start].length;
        let mut end = start + 1;
        while end < runs.len() && runs[end].length == size && runs[end].color != UNKNOWN {
            end += 1;
        }
        let repeated = (end - start) as u32;
        if repeated >= minimum_cells.saturating_sub(2) {
            // Each outer cell can merge into a same-colored desktop area, or
            // an adjacent desktop run can coincidentally be exactly one cell
            // long. Consider only those outer adjustments to the maximal
            // alternating sequence, never arbitrary inner subrectangles.
            for before in -1..=1 {
                let origin = i64::from(runs[start].start) + i64::from(before) * i64::from(size);
                if origin < 0 {
                    continue;
                }
                for after in -1..=1 {
                    let cells = (i64::from(repeated) + i64::from(after) - i64::from(before)) as u32;
                    if cells >= minimum_cells
                        && cells.is_multiple_of(DYNAMIC_STEP)
                        && origin as u64 + u64::from(cells) * u64::from(size)
                            <= u64::from(axis_length)
                    {
                        candidates.insert((origin as u32, size, cells));
                    }
                }
            }
        }
        start = end;
    }
}

#[allow(clippy::too_many_arguments)]
fn checkerboard_extends(
    pixels: &[u8],
    width: usize,
    height: usize,
    x: u32,
    y: u32,
    cell: u32,
    columns: u32,
    rows: u32,
) -> bool {
    // A board embedded in a larger checkerboard is not a complete panel.
    // Require matching alternation along an entire outside edge, allowing the
    // same small compression error budget as the interior validation. A long
    // two-layer segment also exposes a larger board with local corruption:
    // it must not be mistaken for a smaller, intact rectangular subsection.
    let edge_matches = |vertical: bool, before: bool| {
        let origin = if vertical { x } else { y };
        let count = if vertical { columns } else { rows };
        let limit = if vertical { width } else { height };
        let length = if vertical { rows } else { columns };
        // Two layers distinguish continuation from adjacent striped UI: one
        // stripe row can accidentally match a checkerboard's outer phase.
        // At a screen edge, one visible matching layer is enough evidence to
        // reject an interior crop; requiring a second would miss that crop.
        let sample_at = |depth: u32| {
            if before {
                origin.checked_sub(depth * cell - cell / 2)
            } else {
                let position = u64::from(origin)
                    + u64::from(count + depth - 1) * u64::from(cell)
                    + u64::from(cell / 2);
                (position < limit as u64).then_some(position as u32)
            }
        };
        let Some(first) = sample_at(1) else {
            return false;
        };
        let second = sample_at(2);
        let matches_at = |sample, index, depth| {
            let (px, py) = if vertical {
                (sample, y + index * cell + cell / 2)
            } else {
                (x + index * cell + cell / 2, sample)
            };
            let phase = if before { depth } else { count + depth - 1 };
            let expected = ((index + phase) % 2) as u8;
            pixels[py as usize * width + px as usize] == expected
        };
        let mut errors = [0, 0];
        let mut consecutive = 0;
        for index in 0..length {
            let first_matches = matches_at(first, index, 1);
            errors[0] += u32::from(!first_matches);
            if let Some(second) = second {
                let second_matches = matches_at(second, index, 2);
                errors[1] += u32::from(!second_matches);
                consecutive = if first_matches && second_matches {
                    consecutive + 1
                } else {
                    0
                };
                if consecutive >= 16 {
                    return true;
                }
            }
        }
        errors[0] <= length / 50 && errors[1] <= length / 50
    };
    edge_matches(true, true)
        || edge_matches(true, false)
        || edge_matches(false, true)
        || edge_matches(false, false)
}

fn collect_runs(colors: impl Iterator<Item = u8>, runs: &mut Vec<Run>) {
    runs.clear();
    for (position, color) in colors.enumerate() {
        match runs.last_mut() {
            Some(last) if last.color == color => last.length += 1,
            _ => runs.push(Run {
                start: position as u32,
                length: 1,
                color,
            }),
        }
    }
}

fn collect_origins(
    runs: &[Run],
    cells: u32,
    axis_length: u32,
    candidates: &mut BTreeSet<(u32, u32)>,
) {
    let required_runs = cells.saturating_sub(2).max(1) as usize;
    let mut start = 0;
    while start < runs.len() {
        if runs[start].color == UNKNOWN {
            start += 1;
            continue;
        }
        let size = runs[start].length;
        let mut end = start + 1;
        while end < runs.len() && runs[end].length == size && runs[end].color != UNKNOWN {
            end += 1;
        }
        let repeated = end - start;
        if repeated >= required_runs && size <= axis_length / cells {
            let panel_length = size * cells;
            // At most the outer two runs may be absent/merged. Consider the
            // sequence beginning at the first cell or the second cell.
            for run in &runs[start..=end - required_runs] {
                for origin in [Some(run.start), run.start.checked_sub(size)]
                    .into_iter()
                    .flatten()
                {
                    if origin <= axis_length - panel_length {
                        candidates.insert((origin, size));
                    }
                }
            }
        }
        start = end;
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_checkerboard(
    pixels: &[u8],
    width: usize,
    height: usize,
    x: u32,
    y: u32,
    cell: u32,
    columns: u32,
    rows: u32,
) -> bool {
    if x as u64 + cell as u64 * columns as u64 > width as u64
        || y as u64 + cell as u64 * rows as u64 > height as u64
    {
        return false;
    }
    let color_at = |column: u32, row: u32, dx: u32, dy: u32| {
        pixels[(y + row * cell + dy) as usize * width + (x + column * cell + dx) as usize]
    };
    // Quickly reject patterns with the wrong bounds or phase. Java and Rust
    // both start with black and alternate by (row + column), not cell index.
    for (column, row) in [
        (0, 0),
        (columns - 1, 0),
        (0, rows - 1),
        (columns - 1, rows - 1),
        (columns / 2, rows / 2),
    ] {
        if color_at(column, row, cell / 2, cell / 2) != ((column + row) % 2) as u8 {
            return false;
        }
    }

    let mut offsets = vec![cell / 4, cell / 2, (cell as u64 * 3 / 4) as u32];
    offsets.dedup();
    let samples = columns as u64 * rows as u64 * offsets.len() as u64 * offsets.len() as u64;
    let allowed_errors = samples / 50;
    let mut errors = 0;
    for row in 0..rows {
        for column in 0..columns {
            let expected = ((column + row) % 2) as u8;
            for &dy in &offsets {
                for &dx in &offsets {
                    if color_at(column, row, dx, dy) != expected {
                        errors += 1;
                        if errors > allowed_errors {
                            return false;
                        }
                    }
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn config() -> Config {
        Config {
            width: 80,
            height: 48,
            cell_size: 4,
            ..Config::default()
        }
    }

    fn place(screen: &mut RgbImage, panel: &RgbImage, x: u32, y: u32) {
        for (px, py, pixel) in panel.enumerate_pixels() {
            screen.put_pixel(x + px, y + py, *pixel);
        }
    }

    fn checkerboard(config: &Config, cell_size: u32) -> RgbImage {
        Grid::new(
            config.width / config.cell_size * cell_size,
            config.height / config.cell_size * cell_size,
            cell_size,
            config.water_mask,
        )
        .unwrap()
        .checkerboard()
    }

    #[test]
    fn locates_full_panel_at_offset() {
        let config = config();
        let mut screen = RgbImage::from_pixel(150, 110, Rgb([100, 120, 140]));
        place(&mut screen, &checkerboard(&config, 4), 17, 23);
        let found = find_grid(&screen, &config).unwrap();
        assert_eq!((found.x, found.y), (17, 23));
        assert_eq!(found.grid.checkerboard().dimensions(), (80, 48));
    }

    #[test]
    fn infers_scaled_cells() {
        let config = config();
        let mut screen = RgbImage::from_pixel(240, 140, Rgb([100, 120, 140]));
        place(&mut screen, &checkerboard(&config, 6), 35, 29);
        let found = find_grid(&screen, &config).unwrap();
        assert_eq!((found.x, found.y), (35, 29));
        assert_eq!(found.grid.checkerboard().dimensions(), (120, 72));
    }

    #[test]
    fn finds_boards_touching_screen_edges() {
        let config = config();
        for (x, y) in [(0, 0), (70, 62), (0, 62), (70, 0)] {
            // Black may merge with the checkerboard's outer black cells.
            let mut screen = RgbImage::from_pixel(150, 110, Rgb([0, 0, 0]));
            place(&mut screen, &checkerboard(&config, 4), x, y);
            let found = find_grid(&screen, &config).unwrap();
            assert_eq!((found.x, found.y), (x, y));
        }
        let board = checkerboard(&config, 4);
        let found = find_grid(&board, &config).unwrap();
        assert_eq!((found.x, found.y), (0, 0));
    }

    #[test]
    fn rejects_stripes_and_finds_board_beside_them() {
        let config = config();
        let mut screen = RgbImage::from_fn(260, 140, |x, _| {
            if (x / 4) % 2 == 0 {
                Rgb([0, 0, 0])
            } else {
                Rgb([255, 255, 255])
            }
        });
        assert!(find_grid(&screen, &config).is_none());
        place(&mut screen, &checkerboard(&config, 4), 147, 67);
        let found = find_grid(&screen, &config).unwrap();
        assert_eq!((found.x, found.y), (147, 67));
    }

    #[test]
    fn accepts_moderate_compression_color_changes() {
        let config = config();
        let mut board = checkerboard(&config, 4);
        for pixel in board.pixels_mut() {
            *pixel = if pixel[0] == 0 {
                Rgb([23, 18, 20])
            } else {
                Rgb([230, 237, 231])
            };
        }
        let mut screen = RgbImage::from_pixel(150, 110, Rgb([120, 120, 120]));
        place(&mut screen, &board, 17, 23);
        let found = find_grid(&screen, &config).unwrap();
        assert_eq!((found.x, found.y), (17, 23));
    }

    #[test]
    fn rejects_partial_or_corrupted_checkerboard() {
        let config = config();
        let mut board = checkerboard(&config, 4);
        for y in 12..36 {
            for x in 20..60 {
                board.put_pixel(x, y, Rgb([120, 120, 120]));
            }
        }
        assert!(find_grid(&board, &config).is_none());
        let full = checkerboard(&config, 4);
        let cropped = image::imageops::crop_imm(&full, 4, 4, 76, 44).to_image();
        assert!(find_grid(&cropped, &config).is_none());
    }

    #[test]
    fn skips_undecodable_watermark_scale_and_keeps_searching() {
        let config = Config {
            water_mask: true,
            ..config()
        };
        let mut screen = RgbImage::from_pixel(240, 160, Rgb([120, 120, 120]));
        // A 3px cell board cannot carry evenly divided watermark quadrants.
        // It is closer to the configured 4px cell than the valid 6px board.
        let distractor = RgbImage::from_fn(60, 36, |x, y| {
            let gray = if (x / 3 + y / 3).is_multiple_of(2) {
                0
            } else {
                255
            };
            Rgb([gray; 3])
        });
        place(&mut screen, &distractor, 10, 5);
        place(&mut screen, &checkerboard(&config, 6), 100, 70);
        let found = find_grid(&screen, &config).unwrap();
        assert_eq!((found.x, found.y, found.grid.cell_size), (100, 70, 6));
    }

    #[test]
    fn infers_complete_dynamic_geometry_instead_of_a_legacy_subrectangle() {
        for water_mask in [false, true] {
            let receiver = Config {
                water_mask,
                ..Config::default()
            };
            for (columns, rows, cell) in [(160, 80, 2), (604, 316, 2), (440, 244, 4)] {
                let panel = Grid::new(columns * cell, rows * cell, cell, water_mask)
                    .unwrap()
                    .checkerboard();
                let mut screen = RgbImage::from_pixel(
                    panel.width() + 91,
                    panel.height() + 77,
                    Rgb([120, 120, 120]),
                );
                place(&mut screen, &panel, 39, 27);
                let found = find_grid(&screen, &receiver).unwrap();
                assert_eq!((found.x, found.y), (39, 27));
                assert_eq!(
                    (found.grid.width, found.grid.height, found.grid.cell_size),
                    (columns * cell, rows * cell, cell),
                );
                assert_eq!(
                    found.grid.payload_capacity(),
                    (columns * rows / 2 - 6) as usize
                );
            }
        }
    }

    #[test]
    fn dynamic_geometry_preserves_complete_bounds_on_uniform_desktop_edges() {
        let config = Config::default();
        let grid = Grid::new(808, 448, 2, true).unwrap();
        let panel = grid.checkerboard();
        for gray in [0, 120, 255] {
            for (x, y) in [(0, 0), (2, 2), (43, 29), (103, 73)] {
                let mut screen =
                    RgbImage::from_pixel(grid.width + 103, grid.height + 73, Rgb([gray; 3]));
                place(&mut screen, &panel, x, y);
                let found = find_grid(&screen, &config).unwrap();
                assert_eq!((found.x, found.y, found.grid), (x, y, grid));
            }
        }
    }

    #[test]
    fn dynamic_detection_rejects_small_ui_patterns_stripes_and_invalid_geometry() {
        let config = Config::default();
        for (columns, rows) in [(80, 40), (156, 200), (400, 76), (162, 84)] {
            let grid = Grid::new(columns * 2, rows * 2, 2, true).unwrap();
            assert!(find_grid(&grid.checkerboard(), &config).is_none());
        }
        let stripes = RgbImage::from_fn(1400, 750, |x, _| {
            let gray = if (x / 2).is_multiple_of(2) { 0 } else { 255 };
            Rgb([gray; 3])
        });
        assert!(find_grid(&stripes, &config).is_none());
    }

    #[test]
    fn dynamic_detection_keeps_full_portrait_and_ultrawide_bounds() {
        let config = Config::default();
        for (width, height) in [(320, 1920), (3840, 160), (1080, 1712), (3440, 872)] {
            let grid = Grid::new(width, height, 2, true).unwrap();
            let panel = grid.checkerboard();
            for gray in [0, 120, 255] {
                // The full-screen case has no surrounding pixels. Offset
                // cases verify merged outer cells on black/white desktops.
                for (x, y, padding) in [(0, 0, 0), (0, 0, 27), (31, 23, 27)] {
                    let mut screen = RgbImage::from_pixel(
                        width + x + padding,
                        height + y + padding,
                        Rgb([gray; 3]),
                    );
                    place(&mut screen, &panel, x, y);
                    let found = find_grid(&screen, &config).unwrap();
                    assert_eq!((found.x, found.y, found.grid), (x, y, grid));
                }
            }
        }
    }

    #[test]
    fn wide_and_tall_detection_rejects_corruption_stripes_and_partial_cells() {
        let config = Config::default();
        for (width, height) in [(320, 1920), (3840, 160)] {
            let grid = Grid::new(width, height, 2, true).unwrap();
            let board = grid.checkerboard();
            let mut corrupted = board.clone();
            // A substantial damaged block must fail the interior validation,
            // even when all four corners and the center retain their phase.
            for y in height / 8..height / 4 {
                for x in width / 8..width / 4 {
                    corrupted.put_pixel(x, y, Rgb([120; 3]));
                }
            }
            // Damage two blocks to exceed the 2% compression error budget.
            for y in height / 4..height / 2 {
                for x in width / 4..width / 2 {
                    corrupted.put_pixel(x, y, Rgb([120; 3]));
                }
            }
            assert!(find_grid(&corrupted, &config).is_none());
            for (crop_width, crop_height) in [(width - 4, height), (width, height - 4)] {
                let clipped =
                    image::imageops::crop_imm(&board, 0, 0, crop_width, crop_height).to_image();
                assert!(find_grid(&clipped, &config).is_none());
            }
            for vertical in [false, true] {
                let stripes = RgbImage::from_fn(width, height, |x, y| {
                    let axis = if vertical { x } else { y };
                    Rgb([if (axis / 2).is_multiple_of(2) { 0 } else { 255 }; 3])
                });
                assert!(find_grid(&stripes, &config).is_none());
            }
        }
    }

    #[test]
    fn dynamic_candidates_stay_bounded_for_long_alternating_runs() {
        // Relaxing the aspect ratio does not enumerate inner rectangles.
        // Only the same three quantized edge adjustments are considered,
        // regardless of how long a fullscreen scan line becomes.
        for cells in [160, 384, 1024, 4096] {
            let mut runs = Vec::new();
            collect_runs(
                (0..cells * 2 + 64).map(|x| {
                    if (32..cells * 2 + 32).contains(&x) {
                        (((x - 32) / 2) % 2) as u8
                    } else {
                        UNKNOWN
                    }
                }),
                &mut runs,
            );
            let mut candidates = BTreeSet::new();
            collect_dynamic_spans(&runs, MIN_DYNAMIC_COLUMNS, cells * 2 + 64, &mut candidates);
            assert_eq!(candidates.len(), 3);
            assert!(candidates.contains(&(32, 2, cells)));
            assert!(
                candidates
                    .iter()
                    .all(|&(_, cell, columns)| cell == 2 && columns == cells)
            );
        }
    }

    #[test]
    fn dynamic_detection_accepts_compression_and_rejects_unquantized_crops() {
        let config = Config::default();
        let mut panel = Grid::new(1008, 544, 2, true).unwrap().checkerboard();
        for pixel in panel.pixels_mut() {
            *pixel = if pixel[0] == 0 {
                Rgb([20, 24, 18])
            } else {
                Rgb([230, 237, 231])
            };
        }
        let found = find_grid(&panel, &config).unwrap();
        assert_eq!(
            (found.x, found.y, found.grid.width, found.grid.height),
            (0, 0, 1008, 544)
        );
        // A crop must not silently fall back to a default-sized subrectangle.
        let cropped = image::imageops::crop_imm(&panel, 2, 2, 1004, 540).to_image();
        assert!(find_grid(&cropped, &config).is_none());
    }
}
