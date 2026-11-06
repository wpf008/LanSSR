//! A real session-bus fixture for the legacy UKUI/KWin compositor API.
//! It produces a new on-disk PNG for each successful D-Bus method call.
use std::{
    collections::HashMap,
    io::Write,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use image::{Rgba, RgbaImage};
use imgbridge::{config::Config, protocol::{Transfer, TransferKind},
    receiver::{ReceiverOptions, ReceiverUpdate, run_continuous}};
use zbus::blocking::{Connection, Proxy, connection::Builder};

#[derive(Default)]
struct State {
    modes: HashMap<String, String>,
    calls: Vec<(String, i32, i32, i32, i32, bool)>,
    paths: Vec<String>,
    display: Option<String>,
}

type Shared = Arc<Mutex<State>>;

fn pixel(sequence: u32, x: u32, y: u32) -> Rgba<u8> {
    Rgba([
        (x * 3 + sequence * 23) as u8,
        (y * 5 + sequence * 37) as u8,
        ((x + y) * 7 + sequence * 19) as u8,
        255,
    ])
}

fn screenshot(
    state: &Shared,
    service: &str,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    cursor: bool,
) -> zbus::fdo::Result<String> {
    let mut state = state.lock().unwrap();
    state.calls.push((service.into(), x, y, width, height, cursor));
    let sequence = state.calls.len() as u32;
    let mode = state.modes.get(service).map(String::as_str).unwrap_or("success");
    if mode == "denied" {
        return Err(zbus::fdo::Error::AccessDenied("fixture denies capture".into()));
    }
    if mode == "empty" {
        return Ok(String::new());
    }
    if x < 0 || y < 0 || width <= 0 || height <= 0 || width > 2048 || height > 2048 || cursor {
        return Err(zbus::fdo::Error::InvalidArgs(format!(
            "unexpected capture rectangle ({x},{y},{width},{height}) cursor={cursor}"
        )));
    }
    let mut output = tempfile::Builder::new()
        .prefix("lanbridge-dbus-fixture-")
        .suffix(".png")
        .tempfile()
        .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
    if let Some(display) = &state.display {
        let image = image::open(display)
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?.into_rgba8();
        let image = image::imageops::crop_imm(&image, x as u32, y as u32,
                                            width as u32, height as u32).to_image();
        image.save_with_format(output.path(), image::ImageFormat::Png)
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
    } else if mode == "corrupt" {
        output.write_all(b"not an image")
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
    } else {
        let width = if mode == "wrong-size" { width - 1 } else { width } as u32;
        let image = RgbaImage::from_fn(width, height as u32, |column, row| {
            pixel(sequence, x as u32 + column, y as u32 + row)
        });
        image.save_with_format(output.path(), image::ImageFormat::Png)
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
    }
    let path = output.into_temp_path().keep()
        .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
    let path = path.to_string_lossy().into_owned();
    if mode == "missing" {
        std::fs::remove_file(&path)
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
    }
    state.paths.push(path.clone());
    Ok(path)
}

struct Ukui(Shared);
#[zbus::interface(name = "org.ukui.kwin.Screenshot")]
impl Ukui {
    #[zbus(name = "screenshotArea")]
    fn screenshot_area(&self, x: i32, y: i32, width: i32, height: i32, cursor: bool)
        -> zbus::fdo::Result<String>
    {
        screenshot(&self.0, "ukui", x, y, width, height, cursor)
    }
}

struct Kde(Shared);
#[zbus::interface(name = "org.kde.kwin.Screenshot")]
impl Kde {
    #[zbus(name = "screenshotArea")]
    fn screenshot_area(&self, x: i32, y: i32, width: i32, height: i32, cursor: bool)
        -> zbus::fdo::Result<String>
    {
        screenshot(&self.0, "kde", x, y, width, height, cursor)
    }
}

struct Control(Shared);
#[zbus::interface(name = "org.lanbridge.CaptureFixture")]
impl Control {
    fn mode(&self, service: &str, mode: &str) {
        self.0.lock().unwrap().modes.insert(service.into(), mode.into());
    }

    fn calls(&self) -> Vec<(String, i32, i32, i32, i32, bool)> {
        self.0.lock().unwrap().calls.clone()
    }

    fn paths(&self) -> Vec<String> {
        self.0.lock().unwrap().paths.clone()
    }

    fn display(&self, path: &str) {
        self.0.lock().unwrap().display = Some(path.into());
    }
}

fn serve(services: &str) -> Result<()> {
    let state = Arc::new(Mutex::new(State::default()));
    let mut builder = Builder::session()?
        .name("org.lanbridge.CaptureFixture")?
        .serve_at("/org/lanbridge/CaptureFixture", Control(state.clone()))?;
    if services.contains("ukui") {
        builder = builder.name(if services == "ukui-lower" { "org.ukui.kwin" } else { "org.ukui.KWin" })?
            .serve_at("/Screenshot", Ukui(state.clone()))?;
    }
    if services.contains("kde") {
        builder = builder.name("org.kde.KWin")?
            .serve_at("/Screenshot", Kde(state.clone()))?;
    }
    let _connection = builder.build()?;
    println!("READY");
    std::io::stdout().flush()?;
    // The runner terminates this private fixture when its probe completes.
    std::thread::park();
    Ok(())
}

fn control(connection: &Connection) -> Result<Proxy<'_>> {
    Ok(Proxy::new(
        connection,
        "org.lanbridge.CaptureFixture",
        "/org/lanbridge/CaptureFixture",
        "org.lanbridge.CaptureFixture",
    )?)
}

fn check_image(image: &RgbaImage, sequence: u32, x: u32, y: u32, width: u32, height: u32) -> Result<()> {
    ensure!(image.dimensions() == (width, height), "wrong dimensions: {:?}", image.dimensions());
    for (column, row, actual) in image.enumerate_pixels() {
        ensure!(*actual == pixel(sequence, x + column, y + row),
            "stale/incorrect pixel at ({column},{row}): {actual:?} frame={sequence}");
    }
    Ok(())
}

fn probe(case: &str) -> Result<()> {
    let connection = Connection::session()?;
    let control = control(&connection)?;
    if case == "continuous-files" {
        return probe_continuous(&control);
    }
    let monitors = xcap::Monitor::all().context("enumerate the real private Xvfb display")?;
    ensure!(monitors.len() == 1, "expected one private fixture monitor");
    let monitor = &monitors[0];
    let expected_geometry = if case == "missing-screencopy" { (32, 24) } else { (64, 48) };
    ensure!((monitor.width()?, monitor.height()?) == expected_geometry,
        "unexpected Xvfb/Wayland scale geometry");
    // Monitor::width reports logical dimensions. The XCB screenshot API used
    // by the application returns the 64x48 physical pixels of our Xvfb output.
    let (width, height) = (64, 48);

    if case == "x11-stale-wayland" {
        let image = monitor.capture_image().context("real XCB full capture in explicit X11 session")?;
        ensure!(image.dimensions() == (width, height));
        let image = monitor.capture_image_region(3, 5, 17, 11)
            .context("real XCB image region capture in explicit X11 session")?;
        ensure!(image.dimensions() == (17, 11));
        let calls: Vec<(String, i32, i32, i32, i32, bool)> = control.call("Calls", &())?;
        ensure!(calls.is_empty(), "X11 must not call a Wayland D-Bus screenshot backend");
        println!("PASS X11 ignores stale WAYLAND_DISPLAY and captures via real XCB");
        return Ok(());
    }

    if case == "missing-services" {
        ensure!(monitor.capture_image().is_err(), "missing services must return an error");
        println!("PASS missing compositor services return an error");
        return Ok(());
    }

    if case == "fallback-from-ukui" {
        control.call::<_, _, ()>("Mode", &("ukui", "denied"))?;
    }
    let first = monitor.capture_image().context("capture first native-compositor frame")?;
    let mut calls: Vec<(String, i32, i32, i32, i32, bool)> = control.call("Calls", &())?;
    ensure!(!calls.is_empty(), "native D-Bus service was not called");
    check_image(&first, calls.len() as u32, 0, 0, width, height)?;
    let mut selected = calls.last().unwrap().0.clone();
    let initially_selected = selected.clone();
    if case == "ukui" || case == "kde" {
        ensure!(selected == case, "selected {selected}, expected {case}");
    }
    if case == "fallback-from-ukui" {
        ensure!(selected == "kde", "failed UKUI must fall through to KDE");
        ensure!(calls.iter().filter(|call| call.0 == "ukui").count() == 1);
        // UKUI becomes available again, but successful cached KDE captures
        // must keep using the already selected service on later frames.
        control.call::<_, _, ()>("Mode", &("ukui", "success"))?;
    }
    ensure!(calls.last().unwrap().1 == 0 && calls.last().unwrap().2 == 0);
    ensure!(calls.last().unwrap().3 == width as i32 && calls.last().unwrap().4 == height as i32);
    ensure!(!calls.last().unwrap().5, "cursor must stay out of transferred pixels");

    for _ in 0..4 {
        let image = monitor.capture_image()?;
        calls = control.call("Calls", &())?;
        check_image(&image, calls.len() as u32, 0, 0, width, height)?;
        ensure!(calls.last().unwrap().0 == selected, "backend changed between frames");
    }

    let image = monitor.capture_image_region(3, 5, 17, 11)?;
    calls = control.call("Calls", &())?;
    check_image(&image, calls.len() as u32, 3, 5, 17, 11)?;
    let last = calls.last().unwrap();
    ensure!((last.1, last.2, last.3, last.4, last.5) == (3, 5, 17, 11, false));
    if case == "fallback-from-ukui" {
        ensure!(calls.iter().filter(|call| call.0 == "ukui").count() == 1,
            "working cached backend must not retry failed services on every frame");
    }

    for mode in ["denied", "empty", "missing", "corrupt", "wrong-size"] {
        control.call::<_, _, ()>("Mode", &(selected.as_str(), mode))?;
        ensure!(monitor.capture_image().is_err(), "{mode} must return an error, never an old frame");
        control.call::<_, _, ()>("Mode", &(selected.as_str(), "success"))?;
        let fresh = monitor.capture_image().with_context(|| format!("recover after {mode}"))?;
        calls = control.call("Calls", &())?;
        check_image(&fresh, calls.len() as u32, 0, 0, width, height)?;
        selected = calls.last().unwrap().0.clone();
    }

    let paths: Vec<String> = control.call("Paths", &())?;
    let existing: Vec<_> = paths.iter().filter(|path| std::path::Path::new(path).exists()).collect();
    ensure!(existing.is_empty(), "capture did not clean up compositor image files: {existing:?}");
    println!("PASS {case}: fresh full and image-region RGBA pixels; initially_selected={initially_selected}; no stale frames; PNG cleanup");
    Ok(())
}

fn save_display(directory: &Path, name: &str, board: &image::RgbImage) -> Result<String> {
    let mut display = image::RgbImage::from_pixel(1280, 720, image::Rgb([47, 73, 96]));
    image::imageops::replace(&mut display, board, 112, 84);
    let path = directory.join(name);
    display.save(&path)?;
    Ok(path.to_string_lossy().into_owned())
}

fn probe_continuous(control: &Proxy<'_>) -> Result<()> {
    let directory = tempfile::tempdir()?;
    let config = Config { width: 512, height: 320, cell_size: 4, water_mask: true,
                          interval_ms: 50 };
    let grid = config.grid()?;
    let checkerboard = save_display(directory.path(), "checkerboard.png", &grid.checkerboard())?;
    control.call::<_, _, ()>("Display", &(checkerboard.as_str(),))?;

    let payloads = [
        ("首个离线文件.bin", (0..27_431).map(|value| (value * 71 + 19) as u8).collect::<Vec<_>>()),
        ("second-frame-file.bin", (0..19_783).map(|value| (value * 59 + 101) as u8).collect::<Vec<_>>()),
    ];
    let mut frames = Vec::<Vec<String>>::new();
    for (index, (name, payload)) in payloads.iter().enumerate() {
        let transfer = Transfer::new(TransferKind::File, name.to_string(), payload.clone(), grid.payload_capacity())?;
        let mut images = vec![save_display(directory.path(), &format!("{index}-info.png"),
            &grid.encode(&transfer.info_frame().encode()?)?)?];
        for number in 1..=transfer.total_frames() {
            images.push(save_display(directory.path(), &format!("{index}-{number}.png"),
                &grid.encode(&transfer.frame(number)?.encode()?)?)?);
        }
        frames.push(images);
    }

    let output = directory.path().join("received");
    let options = ReceiverOptions { output_dir: output.clone(), overwrite: false,
        monitor: None, region: None, poll_ms: 5, timeout_seconds: 15,
        max_bytes: 1024 * 1024 };
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = cancelled.clone();
    let (updates, events) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || run_continuous(config, options, updates, worker_cancelled));
    let started = Instant::now();
    let mut transfer_index = 0;
    let mut location_verified = false;
    let result = (|| -> Result<()> {
        loop {
            ensure!(started.elapsed() < Duration::from_secs(15), "continuous file fixture timed out");
            let event = events.recv_timeout(Duration::from_secs(3))?;
            match event {
                ReceiverUpdate::Located { location, .. } => {
                    ensure!((location.x, location.y, location.grid.width, location.grid.height)
                            == (112, 84, 512, 320), "real locator found wrong offset or boundary");
                    location_verified = true;
                }
                ReceiverUpdate::Waiting => {
                    control.call::<_, _, ()>("Display", &(frames[transfer_index][0].as_str(),))?;
                }
                ReceiverUpdate::Metadata { name, .. } => {
                    ensure!(name == payloads[transfer_index].0);
                    control.call::<_, _, ()>("Display", &(frames[transfer_index][1].as_str(),))?;
                }
                ReceiverUpdate::Progress { number, .. } => {
                    let next = number as usize + 1;
                    if next < frames[transfer_index].len() {
                        control.call::<_, _, ()>("Display", &(frames[transfer_index][next].as_str(),))?;
                    }
                }
                ReceiverUpdate::Completed { path, bytes, .. } => {
                    ensure!(location_verified, "transfer bypassed screen locator");
                    ensure!(bytes as usize == payloads[transfer_index].1.len());
                    ensure!(std::fs::read(&path)? == payloads[transfer_index].1,
                            "received file differs from sender bytes");
                    transfer_index += 1;
                    if transfer_index == payloads.len() { return Ok(()); }
                    control.call::<_, _, ()>("Display", &(checkerboard.as_str(),))?;
                }
                ReceiverUpdate::Error(error) => bail!("production Receiver failed: {error}"),
                ReceiverUpdate::Stopped => bail!("production Receiver stopped before both files arrived"),
                _ => {}
            }
        }
    })();
    cancelled.store(true, Ordering::Relaxed);
    worker.join().map_err(|_| anyhow::anyhow!("receiver worker panicked"))??;
    result?;
    ensure!(std::fs::read_dir(&output)?.count() == 2, "continuous receiver wrote duplicate or missing files");
    let calls: Vec<(String, i32, i32, i32, i32, bool)> = control.call("Calls", &())?;
    ensure!(calls.iter().any(|call| (call.1, call.2, call.3, call.4) == (112, 84, 512, 320)),
            "real receiver did not use direct native screenshot rectangle");
    println!("PASS continuous production Receiver: automatic checkerboard location at (112,84); two files / 47,214 bytes saved exactly; native cropped capture; no duplicate saves");
    Ok(())
}

fn main() -> Result<()> {
    let arguments: Vec<_> = std::env::args().collect();
    match arguments.get(1).map(String::as_str) {
        Some("service") => serve(arguments.get(2).map(String::as_str).unwrap_or("ukui")),
        Some("probe") => probe(arguments.get(2).map(String::as_str).unwrap_or("ukui")),
        _ => bail!("Usage: lanbridge-linux-capture-probe service|probe CASE"),
    }
}
