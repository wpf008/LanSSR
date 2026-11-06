//! Repeatable receiver image-processing benchmark, without a desktop session.
//!
//! Run with `cargo run --release --no-default-features --example receive_benchmark -- 64`.
//! Input cloning and synthetic screen creation are outside the measured time.
//! Native capture, display presentation, thread scheduling and file saving are
//! deliberately excluded; this compares the processing after capture returns.

use std::{hint::black_box, time::Instant};

use image::{DynamicImage, Rgb, RgbImage, Rgba, RgbaImage};
use imgbridge::{config::Config, grid::Grid, locator::find_grid, protocol::Frame};

struct Timings {
    median_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}

fn measure<S, R>(
    samples: usize,
    mut setup: impl FnMut() -> S,
    mut operation: impl FnMut(S) -> R,
) -> Timings {
    for _ in 0..3 {
        black_box(operation(setup()));
    }
    let mut times = Vec::with_capacity(samples);
    for _ in 0..samples {
        let input = black_box(setup());
        let start = Instant::now();
        let result = black_box(operation(input));
        times.push(start.elapsed().as_secs_f64() * 1000.0);
        drop(result);
    }
    times.sort_unstable_by(f64::total_cmp);
    Timings {
        median_ms: times[samples / 2],
        p95_ms: times[(samples * 95 / 100).min(samples - 1)],
        max_ms: times[samples - 1],
    }
}

fn rgb(image: RgbaImage) -> RgbImage {
    DynamicImage::ImageRgba8(image).into_rgb8()
}

fn decode(grid: Grid, image: &RgbImage) -> Frame {
    Frame::decode(&grid.decode(image).unwrap()).unwrap()
}

fn rgba_panel(image: &RgbImage) -> RgbaImage {
    RgbaImage::from_fn(image.width(), image.height(), |x, y| {
        let [r, g, b] = image.get_pixel(x, y).0;
        Rgba([r, g, b, 255])
    })
}

fn record(screen: (u32, u32), grid: Grid, stage: &str, timings: Timings) {
    println!(
        "{}x{}\t{}x{}\t{}\t{}\t{:.3}\t{:.3}\t{:.3}",
        screen.0,
        screen.1,
        grid.width,
        grid.height,
        if grid.water_mask { "watermark" } else { "gray" },
        stage,
        timings.median_ms,
        timings.p95_ms,
        timings.max_ms,
    );
}

fn run_case(screen_size: (u32, u32), cell_size: u32, water_mask: bool, samples: usize) {
    let config = Config {
        water_mask,
        ..Config::default()
    };
    let grid = Grid::new(360 * cell_size, 200 * cell_size, cell_size, water_mask).unwrap();
    let expected = Frame::Data {
        number: 1,
        payload: (0..grid.payload_capacity())
            .map(|index| ((index * 73 + index / 11) & 255) as u8)
            .collect(),
    };
    let panel = grid.encode(&expected.encode().unwrap()).unwrap();
    let panel_rgba = rgba_panel(&panel);
    // A near-edge offset detects incorrect screen/region coordinates; place
    // the exact same panel at both resolutions so payload work stays fixed.
    let x = screen_size.0 - grid.width - 37;
    let y = screen_size.1 - grid.height - 29;
    let mut screen = RgbaImage::from_pixel(screen_size.0, screen_size.1, Rgba([78, 101, 124, 255]));
    image::imageops::replace(&mut screen, &panel_rgba, i64::from(x), i64::from(y));

    let full_screen_path = |input: RgbaImage| {
        let full_rgb = rgb(input);
        let crop = image::imageops::crop_imm(&full_rgb, x, y, grid.width, grid.height).to_image();
        decode(grid, &crop)
    };
    let crop_first_path = |input: RgbaImage| {
        let region = image::imageops::crop_imm(&input, x, y, grid.width, grid.height).to_image();
        decode(grid, &rgb(region))
    };
    let captured_region_path = |input: RgbaImage| decode(grid, &rgb(input));
    // Every timed path must preserve the complete payload, including its MD5.
    assert_eq!(full_screen_path(screen.clone()), expected);
    assert_eq!(crop_first_path(screen.clone()), expected);
    assert_eq!(captured_region_path(panel_rgba.clone()), expected);

    record(
        screen_size,
        grid,
        "old_full_convert_crop_decode",
        measure(samples, || screen.clone(), full_screen_path),
    );
    record(
        screen_size,
        grid,
        "full_capture_crop_rgba_then_decode",
        measure(samples, || screen.clone(), crop_first_path),
    );
    record(
        screen_size,
        grid,
        "native_region_convert_decode",
        measure(samples, || panel_rgba.clone(), captured_region_path),
    );
    record(
        screen_size,
        grid,
        "grid_decode_and_md5",
        measure(samples, || (), |()| decode(grid, &panel)),
    );

    let mut calibration_screen =
        RgbImage::from_pixel(screen_size.0, screen_size.1, Rgb([78, 101, 124]));
    image::imageops::replace(
        &mut calibration_screen,
        &grid.checkerboard(),
        i64::from(x),
        i64::from(y),
    );
    let found = find_grid(&calibration_screen, &config).unwrap();
    assert_eq!((found.x, found.y, found.grid), (x, y, grid));
    record(
        screen_size,
        grid,
        "initial_full_screen_find_grid",
        measure(
            samples.clamp(1, 8),
            || (),
            |()| black_box(find_grid(&calibration_screen, &config)).unwrap(),
        ),
    );
}

fn main() {
    let samples = std::env::args()
        .nth(1)
        .map(|argument| {
            argument
                .parse::<usize>()
                .expect("samples must be an integer")
        })
        .unwrap_or(32);
    assert!(
        (1..=10_000).contains(&samples),
        "samples must be between 1 and 10000"
    );
    println!(
        "# Image processing only; samples={samples}; no native screen capture; input clone excluded"
    );
    println!("screen\tpanel\tmode\tstage\tmedian_ms\tp95_ms\tmax_ms");
    for water_mask in [false, true] {
        for cell_size in [2, 4] {
            for screen_size in [(1920, 1080), (3840, 2160)] {
                run_case(screen_size, cell_size, water_mask, samples);
            }
        }
    }
}
