use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use image::RgbImage;
#[cfg(any(all(not(target_os = "windows"), not(target_os = "linux")), test))]
use image::RgbaImage;
use xcap::Monitor;

use crate::{
    config::Config,
    grid::Grid,
    locator::{LocatedGrid, find_grid},
    protocol::{Frame, TransferKind},
    receive::{ReceiveEvent, ReceiveSession},
};

#[derive(Debug, Clone)]
pub struct ReceiverOptions {
    pub output_dir: PathBuf,
    pub overwrite: bool,
    pub monitor: Option<usize>,
    /// Capture pixel coordinates within a monitor: x,y,width,height,cell_size.
    pub region: Option<LocatedGrid>,
    pub poll_ms: u64,
    pub timeout_seconds: u64,
    pub max_bytes: usize,
}

pub fn list_monitors() -> Result<()> {
    for (index, monitor) in Monitor::all()
        .context("Cannot enumerate displays")?
        .iter()
        .enumerate()
    {
        // The captured image determines pixel coordinates on Retina displays.
        println!(
            "{index}: {} ({}x{}, desktop origin {},{})",
            monitor.name()?,
            monitor.width()?,
            monitor.height()?,
            monitor.x()?,
            monitor.y()?
        );
    }
    Ok(())
}

pub fn parse_region(value: &str, water_mask: bool) -> Result<LocatedGrid> {
    let values: Vec<u32> = value
        .split(',')
        .map(|part| {
            part.trim()
                .parse()
                .context("Region requires unsigned integers")
        })
        .collect::<Result<_>>()?;
    ensure!(
        values.len() == 5,
        "Region format: x,y,width,height,cell_size (capture pixels)"
    );
    Ok(LocatedGrid {
        x: values[0],
        y: values[1],
        grid: Grid::new(values[2], values[3], values[4], water_mask)?,
    })
}

fn capture(monitor: &Monitor) -> Result<RgbImage> {
    #[cfg(target_os = "windows")]
    let result = crate::windows_capture::capture_monitor(monitor);
    #[cfg(not(target_os = "windows"))]
    let result = monitor.capture_image();
    #[cfg(target_os = "linux")]
    let rgba = result.context("无法截取屏幕。请确认桌面截屏服务可用，并查看具体错误原因")?;
    #[cfg(not(target_os = "linux"))]
    let rgba = result
        .context("Cannot capture display. On macOS, enable Screen Recording for the launching app, then restart it")?;
    // Calibration searches use the captured image's physical pixel dimensions.
    Ok(image::DynamicImage::ImageRgba8(rgba).into_rgb8())
}

fn check_region(width: u32, height: u32, location: LocatedGrid) -> Result<()> {
    ensure!(
        location
            .x
            .checked_add(location.grid.width)
            .is_some_and(|end| end <= width)
            && location
                .y
                .checked_add(location.grid.height)
                .is_some_and(|end| end <= height),
        "Grid region is outside the captured display"
    );
    Ok(())
}

fn crop(screen: &RgbImage, location: LocatedGrid) -> Result<RgbImage> {
    check_region(screen.width(), screen.height(), location)?;
    Ok(image::imageops::crop_imm(
        screen,
        location.x,
        location.y,
        location.grid.width,
        location.grid.height,
    )
    .to_image())
}

#[cfg(any(all(not(target_os = "windows"), not(target_os = "linux")), test))]
fn crop_rgba(screen: &RgbaImage, location: LocatedGrid) -> Result<RgbImage> {
    check_region(screen.width(), screen.height(), location)?;
    let rgba = image::imageops::crop_imm(
        screen,
        location.x,
        location.y,
        location.grid.width,
        location.grid.height,
    )
    .to_image();
    // Convert only the protocol rectangle, rather than every desktop pixel.
    Ok(image::DynamicImage::ImageRgba8(rgba).into_rgb8())
}

/// A located frame no longer needs a whole-desktop screenshot. Windows keeps
/// its GDI objects and image buffer alive on this worker thread between frames.
struct RegionCapture {
    #[cfg(target_os = "windows")]
    native: crate::windows_capture::WindowsRegionCapture,
    #[cfg(not(target_os = "windows"))]
    monitor: Monitor,
    #[cfg(not(target_os = "windows"))]
    location: LocatedGrid,
    #[cfg(not(target_os = "windows"))]
    image: RgbImage,
}

impl RegionCapture {
    fn new(monitor: &Monitor, location: LocatedGrid) -> Result<Self> {
        Ok(Self {
            #[cfg(target_os = "windows")]
            native: crate::windows_capture::WindowsRegionCapture::new(monitor, location)?,
            #[cfg(not(target_os = "windows"))]
            monitor: monitor.clone(),
            #[cfg(not(target_os = "windows"))]
            location,
            #[cfg(not(target_os = "windows"))]
            image: RgbImage::new(0, 0),
        })
    }

    fn capture(&mut self) -> Result<&RgbImage> {
        #[cfg(target_os = "windows")]
        {
            self.native.capture()
        }
        #[cfg(not(target_os = "windows"))]
        {
            #[cfg(target_os = "linux")]
            {
                let grid = self.location.grid;
                let screen = self
                    .monitor
                    .capture_image_region(self.location.x, self.location.y, grid.width, grid.height)
                    .context("无法截取传输画面")?;
                self.image = image::DynamicImage::ImageRgba8(screen).into_rgb8();
            }
            #[cfg(not(target_os = "linux"))]
            {
                let screen = self.monitor.capture_image().context("无法截取传输画面")?;
                self.image = crop_rgba(&screen, self.location)?;
            }
            Ok(&self.image)
        }
    }
}

fn poll_delay(cycle_started: Instant, now: Instant, poll_ms: u64) -> Duration {
    Duration::from_millis(poll_ms).saturating_sub(now.saturating_duration_since(cycle_started))
}

const LOCATION_SEARCH_INTERVAL: Duration = Duration::from_millis(500);

/// A cropped checkerboard can be only a subset of a larger calibration panel.
/// Only a full-screen snapshot may confirm its current payload capacity.
#[derive(Default)]
struct CalibrationBoundary {
    active: bool,
    confirmed: bool,
}

impl CalibrationBoundary {
    fn checkerboard_needs_confirmation(&mut self, since_search: Duration) -> bool {
        if !self.active {
            self.active = true;
            self.confirmed = false;
        }
        !self.confirmed || since_search >= LOCATION_SEARCH_INTERVAL
    }

    fn confirm_snapshot(&mut self, found: bool) {
        self.active = true;
        self.confirmed = found;
    }

    fn accept_data_frame(&mut self) -> bool {
        if self.active && !self.confirmed {
            return false;
        }
        self.active = false;
        true
    }
}

enum CalibrationSnapshot {
    Same,
    Changed(LocatedGrid),
    Missing,
}

fn confirm_calibration_snapshot(
    snapshot: &RgbImage,
    config: &Config,
    previous: LocatedGrid,
) -> CalibrationSnapshot {
    match find_grid(snapshot, config) {
        Some(found)
            if found.x == previous.x && found.y == previous.y && found.grid == previous.grid =>
        {
            CalibrationSnapshot::Same
        }
        Some(found) => CalibrationSnapshot::Changed(found),
        None => CalibrationSnapshot::Missing,
    }
}

pub fn run(config: Config, options: ReceiverOptions) -> Result<()> {
    config.validate()?;
    // Quartz may silently return only wallpaper and this process's windows
    // without authorization. Check permission before waiting for a hidden grid.
    #[cfg(target_os = "macos")]
    ensure!(
        objc2_core_graphics::CGPreflightScreenCaptureAccess(),
        "Screen Recording permission is required. Enable it for the launching app in System Settings > Privacy & Security > Screen Recording, then restart that app"
    );
    ensure!(
        (1..=1000).contains(&options.poll_ms),
        "poll-ms must be between 1 and 1000"
    );
    ensure!(options.max_bytes > 0, "max-bytes must be positive");
    ensure!(
        options.region.is_none() || options.monitor.is_some(),
        "--region requires --monitor"
    );
    std::fs::create_dir_all(&options.output_dir).with_context(|| {
        format!(
            "Cannot create output directory {}",
            crate::desktop_paths::display_path(&options.output_dir)
        )
    })?;
    let output_dir =
        std::fs::canonicalize(&options.output_dir).context("无法确定接收文件的保存目录")?;
    println!(
        "LanReceiver · 接收文件保存目录：{}",
        crate::desktop_paths::display_path(&output_dir)
    );
    let monitors = Monitor::all().context("Cannot enumerate displays")?;
    ensure!(!monitors.is_empty(), "No displays available");
    if let Some(index) = options.monitor {
        ensure!(
            index < monitors.len(),
            "Monitor index {index} is unavailable; use --list-monitors"
        );
    }

    let start = Instant::now();
    let check_timeout = || -> Result<()> {
        if options.timeout_seconds > 0
            && start.elapsed() >= Duration::from_secs(options.timeout_seconds)
        {
            bail!(
                "Receive timed out after {} seconds",
                options.timeout_seconds
            );
        }
        Ok(())
    };
    println!("寻找 LanSender 棋盘格；请在发送端保持重置画面，定位后再发送");
    let (monitor_index, location) = if let Some(location) = options.region {
        let index = options.monitor.unwrap();
        crop(&capture(&monitors[index])?, location)?;
        (index, location)
    } else {
        'search: loop {
            check_timeout()?;
            let mut captured_any = false;
            let mut last_error = None;
            for (index, monitor) in monitors.iter().enumerate() {
                if options.monitor.is_some_and(|selected| selected != index) {
                    continue;
                }
                match capture(monitor) {
                    Ok(screen) => {
                        captured_any = true;
                        if let Some(location) = find_grid(&screen, &config) {
                            break 'search (index, location);
                        }
                    }
                    Err(error) => last_error = Some(error),
                }
            }
            if !captured_any {
                return Err(
                    last_error.unwrap_or_else(|| anyhow::anyhow!("No display can be captured"))
                );
            }
            thread::sleep(Duration::from_millis(500));
        }
    };

    println!(
        "找到发送端：显示器 {monitor_index}，区域 {},{},{}x{}，格子 {} 像素",
        location.x, location.y, location.grid.width, location.grid.height, location.grid.cell_size
    );
    println!("等待信息帧，随后接收数据；Ctrl+C 退出");
    let mut region_capture = RegionCapture::new(&monitors[monitor_index], location)?;
    let mut session = ReceiveSession::new(location.grid.payload_capacity(), options.max_bytes)?;
    let mut last_progress = Instant::now();
    let mut reported_missing = false;
    let mut last_error_message = String::new();
    loop {
        let cycle_started = Instant::now();
        check_timeout()?;
        let image = region_capture.capture()?;
        let bytes = location.grid.decode(image)?;
        // Idle checkerboard starts with alternating 0/F nibbles, not a frame.
        if let Ok(frame) = Frame::decode(&bytes) {
            match session.accept(frame) {
                Ok(event) => match event {
                    ReceiveEvent::Metadata => {
                        let metadata = session.metadata().unwrap();
                        println!(
                            "{}总帧数：{}，名称：{}",
                            if metadata.kind == TransferKind::Text {
                                "文本"
                            } else {
                                "文件"
                            },
                            metadata.total_frames,
                            metadata.name
                        );
                        last_progress = Instant::now();
                        reported_missing = false;
                        last_error_message.clear();
                    }
                    ReceiveEvent::Data {
                        number,
                        received,
                        total,
                    } => {
                        if received == 1 || received % 25 == 0 || received == total as usize {
                            println!("收到帧 {number}，进度 {received}/{total}");
                        }
                        last_progress = Instant::now();
                        reported_missing = false;
                        last_error_message.clear();
                    }
                    ReceiveEvent::Duplicate | ReceiveEvent::Ignored => {}
                },
                Err(error) => {
                    let message = error.to_string();
                    if message != last_error_message {
                        eprintln!("忽略无效帧：{message}");
                        last_error_message = message;
                    }
                }
            }
        }

        if session.is_complete() {
            let metadata = session.metadata().unwrap();
            match metadata.kind {
                TransferKind::File => {
                    let path = session.save_file(&output_dir, options.overwrite)?;
                    println!("收到文件：{}", crate::desktop_paths::display_path(&path));
                }
                TransferKind::Text => {
                    let text = session.text()?;
                    let length = text.chars().count();
                    arboard::Clipboard::new()
                        .context("Cannot open system clipboard")?
                        .set_text(text)
                        .context("Cannot write received text to clipboard")?;
                    println!("收到文本，已写入剪贴板，文本长度：{length}");
                }
            }
            println!("结束，耗时 {:.2} 秒", start.elapsed().as_secs_f64());
            return Ok(());
        }

        if !reported_missing && last_progress.elapsed() >= Duration::from_secs(3) {
            if session.metadata().is_some() {
                let missing = session.missing_frames(100);
                let specification = missing
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                println!("缺失帧（最多 100 个）：{specification}");
                println!("在 LanSender 的“补发帧号”输入上述帧号，点击“补发”");
            } else {
                println!("尚未收到信息帧；请重新点击“发送文件”或“发送文本”，或“重置”后重新定位");
            }
            reported_missing = true;
        }
        thread::sleep(poll_delay(cycle_started, Instant::now(), options.poll_ms));
    }
}

/// Updates emitted by the desktop receiver worker. The UI owns the channel and
/// stop flag; screen capture never blocks the UI event loop.
#[derive(Debug, Clone)]
pub enum ReceiverUpdate {
    Searching,
    Located {
        monitor_index: usize,
        monitor_name: String,
        location: LocatedGrid,
    },
    Waiting,
    Metadata {
        name: String,
        total_frames: u32,
        started_at: Instant,
    },
    Progress {
        number: u32,
        received: usize,
        total: u32,
        received_bytes: u64,
        sampled_at: Instant,
    },
    Missing {
        frames: Vec<u32>,
    },
    Completed {
        path: PathBuf,
        bytes: u64,
        total_frames: u32,
        elapsed_seconds: f64,
    },
    AwaitingReset,
    /// A periodic liveness check; frontends may ignore this update.
    Heartbeat,
    Error(String),
    Stopped,
}

/// Receive successive files until cancelled. A completed transfer is latched
/// until the sender displays its calibration checkerboard, preventing the last
/// visible frame from producing repeated saves. A fresh metadata frame is then
/// required, including when the next file has identical metadata.
pub fn run_continuous(
    config: Config,
    options: ReceiverOptions,
    updates: Sender<ReceiverUpdate>,
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
    let result = continuous_worker(config, options, &updates, &cancelled);
    if let Err(error) = &result {
        let _ = updates.send(ReceiverUpdate::Error(format!("{error:#}")));
    }
    let _ = updates.send(ReceiverUpdate::Stopped);
    result
}

fn continuous_worker(
    config: Config,
    options: ReceiverOptions,
    updates: &Sender<ReceiverUpdate>,
    cancelled: &AtomicBool,
) -> Result<()> {
    if cancelled.load(Ordering::Relaxed) || updates.send(ReceiverUpdate::Searching).is_err() {
        return Ok(());
    }
    config.validate()?;
    #[cfg(target_os = "macos")]
    ensure!(
        objc2_core_graphics::CGPreflightScreenCaptureAccess(),
        "需要屏幕录制权限。请在系统设置 → 隐私与安全性 → 屏幕录制中允许 LanReceiver，然后重新启动应用"
    );
    ensure!(
        (1..=1000).contains(&options.poll_ms),
        "接收轮询间隔必须为 1 至 1000 毫秒"
    );
    ensure!(options.max_bytes > 0, "接收文件大小上限必须大于零");
    ensure!(
        options.region.is_none() || options.monitor.is_some(),
        "指定接收区域时必须选择显示器"
    );
    std::fs::create_dir_all(&options.output_dir).context("无法创建接收文件夹")?;
    let output_dir =
        std::fs::canonicalize(&options.output_dir).context("无法确定接收文件夹位置")?;
    let monitors = Monitor::all().context("无法读取显示器信息")?;
    ensure!(!monitors.is_empty(), "未找到可用显示器");
    if let Some(index) = options.monitor {
        ensure!(index < monitors.len(), "所选显示器不可用");
    }
    let emit = |event| {
        updates
            .send(event)
            .map_err(|_| anyhow::anyhow!("接收界面已关闭"))
    };
    let started = Instant::now();
    let mut heartbeat = Instant::now();
    let mut last_valid_frame = Instant::now();
    let mut last_location_search = Instant::now() - LOCATION_SEARCH_INTERVAL;
    let mut calibration_boundary = CalibrationBoundary::default();
    let mut location = options
        .region
        .map(|region| (options.monitor.unwrap(), region));
    let mut announced = false;
    let mut region_capture: Option<RegionCapture> = None;
    let mut transfer: Option<ContinuousTransfer> = None;
    let mut last_progress = Instant::now();
    let mut missing_reported = false;
    let mut last_error = String::new();
    while !cancelled.load(Ordering::Relaxed) {
        let cycle_started = Instant::now();
        ensure!(
            options.timeout_seconds == 0
                || started.elapsed() < Duration::from_secs(options.timeout_seconds),
            "接收超时（{} 秒）",
            options.timeout_seconds
        );
        if heartbeat.elapsed() >= Duration::from_secs(1) {
            emit(ReceiverUpdate::Heartbeat)?;
            heartbeat = Instant::now();
        }
        let recover_location =
            options.region.is_none() && last_valid_frame.elapsed() >= LOCATION_SEARCH_INTERVAL;
        if (location.is_none() || recover_location)
            && last_location_search.elapsed() >= LOCATION_SEARCH_INTERVAL
        {
            last_location_search = Instant::now();
            for (index, monitor) in monitors.iter().enumerate() {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(());
                }
                if options.monitor.is_some_and(|selected| selected != index) {
                    continue;
                }
                let image = capture(monitor)?;
                if let Some(region) = find_grid(&image, &config) {
                    if !location.is_some_and(|(previous_index, previous)| {
                        previous_index == index
                            && previous.x == region.x
                            && previous.y == region.y
                            && previous.grid == region.grid
                    }) {
                        announced = false;
                    }
                    location = Some((index, region));
                    calibration_boundary.confirm_snapshot(true);
                    break;
                }
            }
        }
        if let Some((index, region)) = location {
            if !announced {
                region_capture = Some(RegionCapture::new(&monitors[index], region)?);
                emit(ReceiverUpdate::Located {
                    monitor_index: index,
                    monitor_name: monitors[index]
                        .name()
                        .unwrap_or_else(|_| format!("显示器 {}", index + 1)),
                    location: region,
                })?;
                emit(ReceiverUpdate::Waiting)?;
                transfer = Some(ContinuousTransfer::new(
                    region.grid.payload_capacity(),
                    options.max_bytes,
                )?);
                announced = true;
            }
            let image = match region_capture.as_mut().unwrap().capture() {
                Ok(image) => Some(image),
                Err(error) if options.region.is_some() => return Err(error),
                // A resized display can invalidate the located rectangle.
                // Capture failures must not reuse pixels from an earlier frame.
                Err(error) => {
                    location = None;
                    announced = false;
                    let message = format!("传输画面已失效，正在重新定位：{error:#}");
                    if message != last_error {
                        emit(ReceiverUpdate::Error(message.clone()))?;
                        last_error = message;
                    }
                    None
                }
            };
            let update = if image
                .as_ref()
                .is_some_and(|image| is_checkerboard(image, region.grid))
            {
                last_valid_frame = Instant::now();
                // A larger calibration board can contain the previously located
                // board as a valid checkerboard subset. Confirm the full boundary
                // synchronously while this calibration is still on screen. Keep
                // the confirmation through the following metadata transition.
                let check_boundary = options.region.is_none()
                    && calibration_boundary
                        .checkerboard_needs_confirmation(last_location_search.elapsed());
                let confirmed = if check_boundary {
                    last_location_search = Instant::now();
                    let snapshot = capture(&monitors[index])?;
                    let confirmation = confirm_calibration_snapshot(&snapshot, &config, region);
                    calibration_boundary
                        .confirm_snapshot(!matches!(confirmation, CalibrationSnapshot::Missing));
                    match confirmation {
                        CalibrationSnapshot::Same => true,
                        CalibrationSnapshot::Changed(found) => {
                            location = Some((index, found));
                            announced = false;
                            region_capture = None;
                            transfer = None;
                            false
                        }
                        CalibrationSnapshot::Missing => {
                            // An unconfirmed old capacity must never be used to
                            // parse the information/data that follows calibration.
                            location = None;
                            announced = false;
                            region_capture = None;
                            transfer = None;
                            emit(ReceiverUpdate::Searching)?;
                            false
                        }
                    }
                } else {
                    true
                };
                if confirmed && transfer.as_mut().unwrap().reset_if_active()? {
                    last_progress = Instant::now();
                    missing_reported = false;
                    last_error.clear();
                    Some(ReceiverUpdate::Waiting)
                } else {
                    None
                }
            } else if let Some(image) = image {
                // A capture taken during a frame transition can be truncated or
                // invalid. It contributes no chunks; a later resend recovers it.
                match region
                    .grid
                    .decode(image)
                    .ok()
                    .and_then(|bytes| Frame::decode(&bytes).ok())
                {
                    Some(frame) if calibration_boundary.accept_data_frame() => {
                        last_valid_frame = Instant::now();
                        match transfer.as_mut().unwrap().accept(frame, &output_dir) {
                            Ok(event) => event,
                            Err(error) => {
                                let message = format!("{error:#}");
                                if message != last_error {
                                    emit(ReceiverUpdate::Error(message.clone()))?;
                                    last_error = message;
                                }
                                None
                            }
                        }
                    }
                    _ => None,
                }
            } else {
                None
            };
            if let Some(event) = update {
                if matches!(
                    event,
                    ReceiverUpdate::Metadata { .. } | ReceiverUpdate::Progress { .. }
                ) {
                    last_progress = Instant::now();
                    missing_reported = false;
                    last_error.clear();
                }
                let complete = matches!(event, ReceiverUpdate::Completed { .. });
                emit(event)?;
                if complete {
                    emit(ReceiverUpdate::AwaitingReset)?;
                }
            }
            if let Some(state) = transfer.as_ref()
                && !state.session.is_complete()
                && !missing_reported
                && state.session.metadata().is_some()
                && last_progress.elapsed() >= Duration::from_secs(3)
            {
                emit(ReceiverUpdate::Missing {
                    frames: state.session.missing_frames(100),
                })?;
                missing_reported = true;
            }
        }
        // Keep stop latency bounded even with a deliberately slow poll setting.
        let now = Instant::now();
        let wait_until = now + poll_delay(cycle_started, now, options.poll_ms);
        while Instant::now() < wait_until && !cancelled.load(Ordering::Relaxed) {
            thread::sleep(
                wait_until
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(20)),
            );
        }
    }
    Ok(())
}

struct ContinuousTransfer {
    session: ReceiveSession,
    payload_capacity: usize,
    max_bytes: usize,
    completed: bool,
    started: Instant,
    save_retry_after: Option<Instant>,
}

impl ContinuousTransfer {
    fn new(payload_capacity: usize, max_bytes: usize) -> Result<Self> {
        Ok(Self {
            session: ReceiveSession::new(payload_capacity, max_bytes)?,
            payload_capacity,
            max_bytes,
            completed: false,
            started: Instant::now(),
            save_retry_after: None,
        })
    }

    fn reset_if_active(&mut self) -> Result<bool> {
        if !self.completed && self.session.metadata().is_none() {
            return Ok(false);
        }
        self.session = ReceiveSession::new(self.payload_capacity, self.max_bytes)?;
        self.completed = false;
        self.save_retry_after = None;
        Ok(true)
    }

    fn accept(&mut self, frame: Frame, output_dir: &Path) -> Result<Option<ReceiverUpdate>> {
        if self.completed {
            return Ok(None);
        }
        // The desktop product receives files; terminal text support remains intact.
        if matches!(&frame, Frame::Info(metadata) if metadata.kind != TransferKind::File) {
            bail!("当前客户端仅支持接收文件");
        }
        let update = match self.session.accept(frame)? {
            ReceiveEvent::Metadata => {
                self.started = Instant::now();
                let metadata = self.session.metadata().unwrap();
                Some(ReceiverUpdate::Metadata {
                    name: metadata.name.clone(),
                    total_frames: metadata.total_frames,
                    started_at: self.started,
                })
            }
            ReceiveEvent::Data {
                number,
                received,
                total,
            } => Some(ReceiverUpdate::Progress {
                number,
                received,
                total,
                received_bytes: self.session.received_bytes() as u64,
                sampled_at: Instant::now(),
            }),
            ReceiveEvent::Duplicate | ReceiveEvent::Ignored => None,
        };
        if self.session.is_complete() {
            let total_frames = self.session.metadata().unwrap().total_frames;
            if self
                .save_retry_after
                .is_some_and(|deadline| Instant::now() < deadline)
            {
                return Ok(update);
            }
            let (path, bytes) = match save_unique(&self.session, output_dir) {
                Ok(saved) => saved,
                Err(error) => {
                    self.save_retry_after = Some(Instant::now() + Duration::from_secs(1));
                    return Err(error);
                }
            };
            // Publication and byte count are already confirmed; latch before
            // notifying the UI so lingering final frames cannot save again.
            self.completed = true;
            return Ok(Some(ReceiverUpdate::Completed {
                path,
                bytes,
                total_frames,
                elapsed_seconds: self.started.elapsed().as_secs_f64(),
            }));
        }
        Ok(update)
    }
}

fn is_checkerboard(image: &RgbImage, grid: Grid) -> bool {
    if image.dimensions() != (grid.width, grid.height) {
        return false;
    }
    for row in 0..grid.height / grid.cell_size {
        for column in 0..grid.width / grid.cell_size {
            let pixel = image.get_pixel(
                column * grid.cell_size + grid.cell_size / 2,
                row * grid.cell_size + grid.cell_size / 2,
            );
            let white = (row + column) % 2 != 0;
            if !pixel
                .0
                .iter()
                .all(|value| if white { *value >= 223 } else { *value <= 32 })
            {
                return false;
            }
        }
    }
    true
}

/// Publish completed files without overwriting earlier transfers. Persist's
/// no-clobber operation resolves races with other applications saving files.
fn save_unique(session: &ReceiveSession, output_dir: &Path) -> Result<(PathBuf, u64)> {
    let staging = tempfile::tempdir_in(output_dir).context("无法创建接收临时目录")?;
    let staged_path = session.save_file(staging.path(), false)?;
    let mut temporary = tempfile::NamedTempFile::new_in(output_dir)?;
    let bytes = std::io::copy(
        &mut std::fs::File::open(&staged_path)?,
        temporary.as_file_mut(),
    )?;
    temporary.as_file().sync_all()?;
    let name = &session.metadata().unwrap().name;
    let original = Path::new(name);
    let stem = original
        .file_stem()
        .unwrap_or(original.as_os_str())
        .to_string_lossy();
    let extension = original.extension().map(|value| value.to_string_lossy());
    for index in 0..10_000_u32 {
        let candidate = if index == 0 {
            output_dir.join(name)
        } else {
            let suffix = extension
                .as_ref()
                .map(|value| format!(".{value}"))
                .unwrap_or_default();
            output_dir.join(format!("{stem} ({index}){suffix}"))
        };
        match temporary.persist_noclobber(&candidate) {
            Ok(_) => return Ok((candidate, bytes)),
            Err(error) => {
                if error.error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(error.error).context("无法保存接收文件");
                }
                temporary = error.file;
            }
        }
    }
    bail!("同名文件过多，无法生成新的文件名")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_conversion_matches_full_capture_without_moving_or_flipping_pixels() {
        let screen = RgbaImage::from_fn(96, 64, |x, y| {
            image::Rgba([x as u8, y as u8, (x ^ y) as u8, (x + y) as u8])
        });
        let location = LocatedGrid {
            x: 11,
            y: 17,
            grid: Grid::new(64, 32, 4, true).unwrap(),
        };
        let expected = crop(
            &image::DynamicImage::ImageRgba8(screen.clone()).into_rgb8(),
            location,
        )
        .unwrap();
        let actual = crop_rgba(&screen, location).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual.get_pixel(0, 0).0, [11, 17, 11 ^ 17]);
        assert_eq!(actual.get_pixel(63, 31).0, [74, 48, 74 ^ 48]);
        for invalid in [
            LocatedGrid { x: 33, ..location },
            LocatedGrid { y: 33, ..location },
            LocatedGrid {
                x: u32::MAX,
                ..location
            },
        ] {
            assert!(crop_rgba(&screen, invalid).is_err());
        }
    }

    #[test]
    fn polling_accounts_for_capture_and_processing_time() {
        let start = Instant::now();
        assert_eq!(
            poll_delay(start, start + Duration::from_millis(2), 5),
            Duration::from_millis(3)
        );
        assert_eq!(
            poll_delay(start, start + Duration::from_millis(5), 5),
            Duration::ZERO
        );
        assert_eq!(
            poll_delay(start, start + Duration::from_millis(80), 5),
            Duration::ZERO
        );
    }

    #[test]
    fn failed_full_calibration_confirmation_blocks_stale_information_and_data() {
        let previous = LocatedGrid {
            x: 20,
            y: 30,
            grid: Grid::new(320, 160, 2, true).unwrap(),
        };
        let obscured = RgbImage::from_pixel(800, 600, image::Rgb([127, 127, 127]));
        let mut boundary = CalibrationBoundary::default();
        assert!(boundary.checkerboard_needs_confirmation(Duration::ZERO));
        assert!(matches!(
            confirm_calibration_snapshot(&obscured, &Config::default(), previous),
            CalibrationSnapshot::Missing
        ));
        boundary.confirm_snapshot(false);
        // The information frame and all following data must remain blocked;
        // merely observing a valid frame cannot confirm the larger panel bounds.
        assert!(!boundary.accept_data_frame());
        assert!(!boundary.accept_data_frame());
        assert!(boundary.checkerboard_needs_confirmation(Duration::from_millis(5)));
        boundary.confirm_snapshot(true);
        assert!(boundary.accept_data_frame());
    }

    #[test]
    fn confirmed_calibration_allows_metadata_without_a_late_full_screen_search() {
        let location = LocatedGrid {
            x: 20,
            y: 30,
            grid: Grid::new(320, 160, 2, true).unwrap(),
        };
        let mut snapshot = RgbImage::from_pixel(800, 600, image::Rgb([127, 127, 127]));
        image::imageops::replace(&mut snapshot, &location.grid.checkerboard(), 20, 30);
        let mut boundary = CalibrationBoundary::default();
        assert!(boundary.checkerboard_needs_confirmation(Duration::ZERO));
        assert!(matches!(
            confirm_calibration_snapshot(&snapshot, &Config::default(), location),
            CalibrationSnapshot::Same
        ));
        boundary.confirm_snapshot(true);
        assert!(!boundary.checkerboard_needs_confirmation(Duration::from_millis(499)));
        assert!(boundary.checkerboard_needs_confirmation(LOCATION_SEARCH_INTERVAL));
        boundary.confirm_snapshot(true);
        assert!(!boundary.checkerboard_needs_confirmation(Duration::from_millis(499)));
        // Metadata can arrive after the 500 ms deadline. No full-screen search
        // should be scheduled from a normal data frame after calibration ends.
        assert!(boundary.accept_data_frame());
        assert!(boundary.accept_data_frame());
        // The next distinct calibration requires a fresh full-boundary check.
        assert!(boundary.checkerboard_needs_confirmation(Duration::ZERO));
    }

    #[test]
    fn a_larger_calibration_subset_rebuilds_capacity_before_accepting_file_frames() {
        use crate::protocol::Transfer;
        let previous = LocatedGrid {
            x: 20,
            y: 30,
            grid: Grid::new(320, 160, 2, true).unwrap(),
        };
        let larger = Grid::new(640, 320, 2, true).unwrap();
        let mut snapshot = RgbImage::from_pixel(800, 600, image::Rgb([127, 127, 127]));
        image::imageops::replace(&mut snapshot, &larger.checkerboard(), 20, 30);
        assert!(is_checkerboard(
            &crop(&snapshot, previous).unwrap(),
            previous.grid
        ));
        let CalibrationSnapshot::Changed(found) =
            confirm_calibration_snapshot(&snapshot, &Config::default(), previous)
        else {
            panic!("a valid old subregion must not hide the larger panel boundary");
        };
        assert_eq!((found.x, found.y, found.grid), (20, 30, larger));

        let bytes: Vec<_> = (0..50_000).map(|index| (index % 251) as u8).collect();
        let old_transfer = Transfer::new(
            TransferKind::File,
            "resized.bin".into(),
            bytes.clone(),
            previous.grid.payload_capacity(),
        )
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut state =
            ContinuousTransfer::new(previous.grid.payload_capacity(), bytes.len()).unwrap();
        state
            .accept(old_transfer.info_frame(), directory.path())
            .unwrap();
        state
            .accept(old_transfer.frame(1).unwrap(), directory.path())
            .unwrap();
        assert_eq!(state.session.received_count(), 1);

        // Rebuilding the state drops old-sized chunks before the repartitioned
        // file is accepted, including when its file name has not changed.
        state = ContinuousTransfer::new(found.grid.payload_capacity(), bytes.len()).unwrap();
        let transfer = old_transfer
            .with_payload_capacity(found.grid.payload_capacity())
            .unwrap();
        let mut boundary = CalibrationBoundary::default();
        boundary.confirm_snapshot(true);
        assert!(boundary.accept_data_frame());
        state
            .accept(transfer.info_frame(), directory.path())
            .unwrap();
        let mut completed = None;
        for number in 1..=transfer.total_frames() {
            let frame = Frame::decode(&transfer.frame(number).unwrap().encode().unwrap()).unwrap();
            if let Some(ReceiverUpdate::Completed { path, .. }) =
                state.accept(frame, directory.path()).unwrap()
            {
                completed = Some(path);
            }
        }
        assert_eq!(std::fs::read(completed.unwrap()).unwrap(), bytes);
    }

    #[test]
    fn progress_reports_only_unique_bytes_with_worker_timestamps() {
        use crate::protocol::Transfer;
        let directory = tempfile::tempdir().unwrap();
        let transfer =
            Transfer::new(TransferKind::File, "speed.bin".into(), (0..23).collect(), 8).unwrap();
        let mut state = ContinuousTransfer::new(8, 100).unwrap();
        let Some(ReceiverUpdate::Metadata { started_at, .. }) = state
            .accept(transfer.info_frame(), directory.path())
            .unwrap()
        else {
            panic!("metadata should start the speed measurement");
        };
        let Some(ReceiverUpdate::Progress {
            received_bytes,
            sampled_at,
            ..
        }) = state
            .accept(transfer.frame(3).unwrap(), directory.path())
            .unwrap()
        else {
            panic!("a unique validated frame should report progress");
        };
        assert_eq!(received_bytes, 7);
        assert!(sampled_at >= started_at);
        assert!(
            state
                .accept(transfer.frame(3).unwrap(), directory.path())
                .unwrap()
                .is_none()
        );
        let Some(ReceiverUpdate::Progress { received_bytes, .. }) = state
            .accept(transfer.frame(1).unwrap(), directory.path())
            .unwrap()
        else {
            panic!("the next unique frame should report cumulative bytes");
        };
        assert_eq!(received_bytes, 15);
        assert!(matches!(
            state
                .accept(transfer.frame(2).unwrap(), directory.path())
                .unwrap(),
            Some(ReceiverUpdate::Completed { bytes: 23, .. })
        ));
    }

    #[test]
    fn continuous_transfers_require_reset_and_preserve_identical_files() {
        use crate::protocol::Transfer;
        let directory = tempfile::tempdir().unwrap();
        let transfer = Transfer::new(
            TransferKind::File,
            "report.txt".into(),
            b"abcdefghij".to_vec(),
            8,
        )
        .unwrap();
        let mut state = ContinuousTransfer::new(8, 100).unwrap();
        state
            .accept(transfer.info_frame(), directory.path())
            .unwrap();
        state
            .accept(transfer.frame(1).unwrap(), directory.path())
            .unwrap();
        let update = state
            .accept(transfer.frame(2).unwrap(), directory.path())
            .unwrap()
            .unwrap();
        let ReceiverUpdate::Completed { path, bytes, .. } = update else {
            panic!("expected completion")
        };
        assert_eq!(bytes, 10);
        assert_eq!(path.file_name().unwrap(), "report.txt");
        // A retained final screen, repeated info, and even an entire repeated
        // transfer cannot save again until calibration has been observed.
        for _ in 0..3 {
            assert!(
                state
                    .accept(transfer.info_frame(), directory.path())
                    .unwrap()
                    .is_none()
            );
            assert!(
                state
                    .accept(transfer.frame(1).unwrap(), directory.path())
                    .unwrap()
                    .is_none()
            );
            assert!(
                state
                    .accept(transfer.frame(2).unwrap(), directory.path())
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(state.reset_if_active().unwrap());
        // Data arriving before new metadata belongs to no active session.
        assert!(
            state
                .accept(transfer.frame(2).unwrap(), directory.path())
                .unwrap()
                .is_none()
        );
        state
            .accept(transfer.info_frame(), directory.path())
            .unwrap();
        state
            .accept(transfer.frame(2).unwrap(), directory.path())
            .unwrap();
        let update = state
            .accept(transfer.frame(1).unwrap(), directory.path())
            .unwrap()
            .unwrap();
        let ReceiverUpdate::Completed { path: second, .. } = update else {
            panic!("expected completion")
        };
        assert_eq!(second.file_name().unwrap(), "report (1).txt");
        assert_eq!(std::fs::read(path).unwrap(), std::fs::read(second).unwrap());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn reset_discards_partial_transfer_and_empty_files_complete_once() {
        use crate::protocol::Transfer;
        let directory = tempfile::tempdir().unwrap();
        let partial = Transfer::new(TransferKind::File, "partial".into(), vec![7; 10], 8).unwrap();
        let mut state = ContinuousTransfer::new(8, 100).unwrap();
        state
            .accept(partial.info_frame(), directory.path())
            .unwrap();
        state
            .accept(partial.frame(1).unwrap(), directory.path())
            .unwrap();
        assert!(state.reset_if_active().unwrap());
        assert_eq!(state.session.received_count(), 0);
        assert!(!state.reset_if_active().unwrap());
        let empty = Transfer::new(TransferKind::File, "empty".into(), vec![], 8).unwrap();
        assert!(matches!(
            state.accept(empty.info_frame(), directory.path()).unwrap(),
            Some(ReceiverUpdate::Completed { bytes: 0, .. })
        ));
        assert!(
            state
                .accept(empty.info_frame(), directory.path())
                .unwrap()
                .is_none()
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn calibration_requires_the_entire_board_not_any_invalid_frame() {
        for water_mask in [false, true] {
            let grid = Grid::new(80, 40, 4, water_mask).unwrap();
            let mut board = grid.checkerboard();
            assert!(is_checkerboard(&board, grid));
            board.put_pixel(2, 2, image::Rgb([127, 127, 127]));
            assert!(!is_checkerboard(&board, grid));
            assert!(!is_checkerboard(
                &grid.encode(&[0, 15, 0, 15]).unwrap(),
                grid
            ));
        }
    }

    #[test]
    fn saving_failure_keeps_complete_chunks_and_retries_without_spinning() {
        use crate::protocol::Transfer;
        let directory = tempfile::tempdir().unwrap();
        let transfer =
            Transfer::new(TransferKind::File, "retry.bin".into(), vec![1; 3], 8).unwrap();
        let mut state = ContinuousTransfer::new(8, 100).unwrap();
        state
            .accept(transfer.info_frame(), directory.path())
            .unwrap();
        let unavailable = directory.path().join("missing-folder");
        assert!(
            state
                .accept(transfer.frame(1).unwrap(), &unavailable)
                .is_err()
        );
        assert!(state.session.is_complete());
        assert!(!state.completed);
        assert!(
            state
                .accept(transfer.frame(1).unwrap(), &unavailable)
                .unwrap()
                .is_none()
        );
        // Move the retry deadline rather than sleeping in a unit test.
        state.save_retry_after = Some(Instant::now());
        let event = state
            .accept(transfer.frame(1).unwrap(), directory.path())
            .unwrap();
        assert!(matches!(
            event,
            Some(ReceiverUpdate::Completed { bytes: 3, .. })
        ));
        assert!(
            state
                .accept(transfer.frame(1).unwrap(), directory.path())
                .unwrap()
                .is_none()
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn cancelled_and_detached_workers_stop_before_native_capture() {
        let options = ReceiverOptions {
            output_dir: PathBuf::new(),
            overwrite: false,
            monitor: None,
            region: None,
            poll_ms: 10,
            timeout_seconds: 0,
            max_bytes: 100,
        };
        let (sender, events) = std::sync::mpsc::channel();
        run_continuous(
            Config::default(),
            options.clone(),
            sender,
            Arc::new(AtomicBool::new(true)),
        )
        .unwrap();
        assert!(matches!(events.recv().unwrap(), ReceiverUpdate::Stopped));
        let (sender, events) = std::sync::mpsc::channel();
        drop(events);
        run_continuous(
            Config::default(),
            options,
            sender,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    }

    #[test]
    fn region_uses_capture_pixels_and_rejects_overflow() {
        let region = parse_region("10,20,80,40,4", false).unwrap();
        let screen = RgbImage::new(100, 70);
        assert_eq!(crop(&screen, region).unwrap().dimensions(), (80, 40));
        let out = LocatedGrid {
            x: u32::MAX,
            ..region
        };
        assert!(crop(&screen, out).is_err());
        assert!(parse_region("0,0,1,1,0", false).is_err());
        assert!(parse_region("1,2", false).is_err());
    }
}
