//! Native sender. Protocol cells always occupy an integer number of physical pixels.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use eframe::egui;
use image::RgbImage;

use crate::file_picker::{FilePicker, PickerMode, PickerResult};
use crate::transfer_texture::TransferTexture;

use crate::{
    config::{Config, FRAME_INTERVAL_OPTIONS_MS},
    grid::Grid,
    protocol::{Frame, Transfer, TransferKind, parse_resend},
    transfer_speed::{SpeedMeter, format_speed},
    ui_style::{
        Palette, action_button, card, install_ui_styles, primary_button, theme_selector,
        watermark_switch,
    },
    window_layout::{self, ScreenArea, WindowRole},
};

/// Open the native sending window. An initial file is selected, but never sent automatically.
pub fn run(config: Config, initial_file: Option<PathBuf>) -> Result<()> {
    config.validate()?;
    let grid = config.grid()?;
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("LanSender")
        .with_icon(
            eframe::icon_data::from_png_bytes(include_bytes!("../assets/icons/lansender.png"))
                .context("无法加载 LanSender 图标")?,
        )
        .with_position([20.0, 40.0])
        .with_inner_size([960.0, 640.0])
        .with_min_inner_size([640.0, 360.0])
        .with_resizable(false)
        .with_maximize_button(false)
        .with_always_on_top()
        .with_drag_and_drop(true);
    if let Some((area, scale)) = window_layout::initial_screen_area() {
        let plan = window_layout::fractional_window_plan(area, scale, WindowRole::Sender);
        viewport = viewport
            .with_inner_size(plan.inner_pixels / scale)
            .with_min_inner_size(plan.inner_pixels / scale)
            .with_max_inner_size(plan.inner_pixels / scale);
        if let Some(position) = plan.outer_position {
            viewport = viewport.with_position(position / scale);
        }
    }
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        multisampling: 0,
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native(
        "LanSender",
        options,
        Box::new(move |cc| {
            crate::platform_startup::initialize_graphics_limits(cc);
            install_cjk_fallback(&cc.egui_ctx);
            // A discarded layout pass must never consume a transmission frame.
            cc.egui_ctx
                .options_mut(|options| options.max_passes = 1.try_into().unwrap());
            Ok(Box::new(Sender::new(
                &cc.egui_ctx,
                config,
                grid,
                initial_file,
            )))
        }),
    )
    .map_err(|error| anyhow!("无法打开 LanSender 窗口：{error}"))
}

/// Chrome is sized in logical points; protocol pixels are always physical pixels.
// Include slack for system CJK font metrics and fractional DPI rounding.
const CONTROLS_HEIGHT: f32 = 236.0;
const WINDOW_CHROME_HEIGHT: f32 = CONTROLS_HEIGHT + 64.0;
const FULLSCREEN_CHROME_HEIGHT: f32 = CONTROLS_HEIGHT + 32.0;

/// Settings select the minimum data-frame dwell; the display refresh can
/// make actual presentation slower. Calibration and retries have their own dwell.
fn data_interval_ms(configured_ms: u64, _display_pixels: u64) -> u64 {
    configured_ms
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct WindowPlan {
    cell_size: u32,
    columns: u32,
    rows: u32,
    chrome_scale: f32,
    inner_pixels: egui::Vec2,
    max_inner_pixels: Option<egui::Vec2>,
    outer_position: Option<egui::Pos2>,
}

/// More screen space carries more bytes, using two physical pixels per cell.
/// Four-cell increments reduce rounding churn and identify dynamic calibration.
fn dynamic_grid_size(available: egui::Vec2) -> (u32, u32) {
    let columns = ((available.x.max(0.0) / 8.0).floor() as u32) * 4;
    let rows = ((available.y.max(0.0) / 8.0).floor() as u32) * 4;
    // An undersized desktop pauses rather than painting a clipped calibration.
    (columns.max(160), rows.max(80))
}

fn window_plan(config: &Config, native_scale: f32, area: Option<ScreenArea>) -> WindowPlan {
    if let Some(area) = area {
        let fraction =
            window_layout::fractional_window_plan(area, native_scale, WindowRole::Sender);
        let available = fraction.inner_pixels
            - egui::vec2(32.0, WINDOW_CHROME_HEIGHT) * fraction.chrome_scale
            - egui::Vec2::splat(2.0);
        let (columns, rows) = dynamic_grid_size(available);
        WindowPlan {
            cell_size: 2,
            columns,
            rows,
            chrome_scale: fraction.chrome_scale,
            inner_pixels: fraction.inner_pixels,
            max_inner_pixels: Some(fraction.inner_pixels),
            outer_position: fraction.outer_position,
        }
    } else {
        // A native monitor is normally available. Keep custom/headless grids
        // unchanged until its work area can be measured.
        WindowPlan {
            cell_size: config.cell_size,
            columns: config.width / config.cell_size,
            rows: config.height / config.cell_size,
            chrome_scale: native_scale,
            inner_pixels: egui::vec2(
                (config.width as f32 + 32.0 * native_scale).max(640.0 * native_scale) + 2.0,
                config.height as f32 + WINDOW_CHROME_HEIGHT * native_scale + 2.0,
            ),
            max_inner_pixels: None,
            outer_position: None,
        }
    }
}

fn fullscreen_window_plan(inner_pixels: egui::Vec2, native_scale: f32) -> WindowPlan {
    let chrome_scale = native_scale
        .min(inner_pixels.x / 640.0)
        .min(inner_pixels.y / 600.0);
    let chrome_scale = (chrome_scale * 1000.0).floor().max(1.0) / 1000.0;
    let available = inner_pixels
        - egui::vec2(0.0, FULLSCREEN_CHROME_HEIGHT) * chrome_scale
        - egui::vec2(0.0, 2.0);
    let (columns, rows) = dynamic_grid_size(available);
    WindowPlan {
        cell_size: 2,
        columns,
        rows,
        chrome_scale,
        inner_pixels,
        max_inner_pixels: None,
        outer_position: None,
    }
}

#[derive(Clone, Copy, PartialEq)]
struct FullscreenGeometry {
    size: egui::Vec2,
    position: Option<egui::Pos2>,
    native_scale: f32,
}

struct FullscreenRequest {
    target: bool,
    requested_at: Instant,
    last_geometry: Option<FullscreenGeometry>,
    stable_since: Instant,
}

impl FullscreenRequest {
    fn new(target: bool, now: Instant) -> Self {
        Self {
            target,
            requested_at: now,
            last_geometry: None,
            stable_since: now,
        }
    }

    fn ready(&mut self, actual: bool, geometry: FullscreenGeometry, now: Instant) -> bool {
        if actual != self.target {
            self.last_geometry = None;
            return false;
        }
        if self.last_geometry != Some(geometry) {
            self.last_geometry = Some(geometry);
            self.stable_since = now;
        }
        // AppKit reports full-screen state before the compositor finishes animating.
        let animation_delay = if cfg!(target_os = "macos") {
            Duration::from_secs(1)
        } else {
            Duration::ZERO
        };
        now.duration_since(self.stable_since) >= Duration::from_millis(250)
            && now.duration_since(self.requested_at) >= animation_delay
    }
}

struct Prepared {
    source: PathBuf,
    label: String,
    transfer: Transfer,
}

enum FramePlan {
    All { next: u32, last: u32 },
    Selected { numbers: Vec<u32>, next: usize },
}

impl FramePlan {
    fn next(&mut self) -> Option<u32> {
        match self {
            Self::All { next, last } if *next <= *last => {
                let number = *next;
                *next += 1;
                Some(number)
            }
            Self::Selected { numbers, next } => {
                let number = numbers.get(*next).copied();
                *next += usize::from(number.is_some());
                number
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionStage {
    Calibration,
    Information,
    Data(u32),
}

struct Session {
    plan: FramePlan,
    stage: SessionStage,
    displayed: usize,
    count: usize,
    interval: Duration,
    started: Instant,
    submitted_frame: Option<u64>,
    visible_since: Option<Instant>,
    current_frame_bytes: usize,
    speed: SpeedMeter,
    speed_label: String,
    speed_updated_at: Option<Instant>,
}

impl Session {
    fn hold_duration(&self) -> Duration {
        match self.stage {
            SessionStage::Calibration | SessionStage::Information => Duration::from_secs(1),
            SessionStage::Data(_) => self.interval,
        }
    }

    fn due(&self, now: Instant) -> bool {
        self.visible_since
            .is_some_and(|since| now.duration_since(since) >= self.hold_duration())
    }

    fn pause(&mut self) {
        self.submitted_frame = None;
        self.visible_since = None;
    }

    fn reset_speed(&mut self, now: Instant) {
        self.speed.reset(now);
        self.speed_label = format_speed(0.0);
        self.speed_updated_at = None;
    }

    fn speed_label(&mut self, now: Instant) -> &str {
        // Reuse native updates and refresh this small text at most four times
        // per second; speed reporting never schedules extra transmission frames.
        if self.speed_updated_at.is_none_or(|previous| {
            now.saturating_duration_since(previous) >= Duration::from_millis(250)
        }) {
            self.speed_label = format_speed(self.speed.bytes_per_second(now));
            self.speed_updated_at = Some(now);
        }
        &self.speed_label
    }

    fn record_paint(&mut self, frame_number: u64, now: Instant) {
        match self.submitted_frame {
            None => self.submitted_frame = Some(frame_number),
            Some(previous) if frame_number > previous && self.visible_since.is_none() => {
                // Start the dwell timer only in a later native frame. The preceding
                // update merely submitted the image for rendering after it returned.
                self.visible_since = Some(now);
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Main,
    Settings,
}

struct Sender {
    config: Config,
    grid: Grid,
    selected_file: Option<PathBuf>,
    transfer: Option<Prepared>,
    pending: Option<Receiver<std::result::Result<Prepared, String>>>,
    session: Option<Session>,
    texture: TransferTexture,
    resend: String,
    status: String,
    error: bool,
    progress: f32,
    progress_label: String,
    screen_area: Option<ScreenArea>,
    last_window_plan: Option<WindowPlan>,
    fullscreen: bool,
    fullscreen_request: Option<FullscreenRequest>,
    last_grid_rect: Option<egui::Rect>,
    requires_full_transfer: bool,
    display_cell_size: u32,
    theme: egui::Theme,
    page: Page,
    file_picker: Option<FilePicker>,
}

impl Sender {
    fn new(
        ctx: &egui::Context,
        config: Config,
        grid: Grid,
        selected_file: Option<PathBuf>,
    ) -> Self {
        install_ui_styles(ctx);
        let theme = ctx.system_theme().unwrap_or(egui::Theme::Light);
        ctx.set_theme(theme);
        let image = grid.checkerboard();
        Self {
            config,
            grid,
            selected_file,
            transfer: None,
            pending: None,
            session: None,
            texture: TransferTexture::new(ctx, &image),
            resend: String::new(),
            status: "等待发送 · 请先让接收端定位下方棋盘格".into(),
            error: false,
            progress: 0.0,
            progress_label: "0 / 0".into(),
            screen_area: None,
            last_window_plan: None,
            fullscreen: false,
            fullscreen_request: None,
            last_grid_rect: None,
            requires_full_transfer: false,
            display_cell_size: grid.cell_size,
            theme,
            page: Page::Main,
            file_picker: None,
        }
    }

    fn busy(&self) -> bool {
        self.pending.is_some() || self.session.is_some()
    }

    fn fullscreen_target(&self) -> bool {
        self.fullscreen_request
            .as_ref()
            .map_or(self.fullscreen, |request| request.target)
    }

    fn pause_for_window_change(&mut self) {
        if let Some(session) = self.session.as_mut() {
            session.pause();
            session.reset_speed(Instant::now());
        }
    }

    fn request_fullscreen(&mut self, ctx: &egui::Context, fullscreen: bool) {
        self.fullscreen_request = Some(FullscreenRequest::new(fullscreen, Instant::now()));
        self.last_window_plan = None;
        self.pause_for_window_change();
        if fullscreen {
            // Release the fixed windowed constraints before the native transition.
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::Vec2::ZERO));
            ctx.send_viewport_cmd(egui::ViewportCommand::MaxInnerSize(egui::Vec2::INFINITY));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(fullscreen));
        ctx.request_repaint();
    }

    fn update_fullscreen(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        if let Some(actual) = ctx.input(|input| input.viewport().fullscreen)
            && actual != self.fullscreen
        {
            self.fullscreen = actual;
            self.last_window_plan = None;
            self.pause_for_window_change();
            // Native system controls can enter or leave full screen too.
            if self.fullscreen_request.is_none() {
                self.fullscreen_request = Some(FullscreenRequest::new(actual, now));
            }
        }
        if let Some(request) = self.fullscreen_request.as_mut() {
            let scale = ctx.pixels_per_point();
            let (position, native_scale) = ctx.input(|input| {
                let viewport = input.viewport();
                (
                    viewport.inner_rect.map(|rect| (rect.min * scale).round()),
                    viewport.native_pixels_per_point.unwrap_or(scale),
                )
            });
            let geometry = FullscreenGeometry {
                size: (ctx.content_rect().size() * scale).round(),
                position,
                native_scale,
            };
            if request.ready(self.fullscreen, geometry, now) {
                self.fullscreen_request = None;
                self.last_window_plan = None;
            } else if now.duration_since(request.requested_at) > Duration::from_secs(5) {
                self.fullscreen_request = None;
                self.last_window_plan = None;
                self.set_error("系统未能切换窗口显示模式，请重试");
            }
        }
        let toggle = ctx.input_mut(|input| {
            let pressed = input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::F11,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                        ..
                    }
                )
            });
            if pressed {
                input.consume_key(egui::Modifiers::NONE, egui::Key::F11);
            }
            pressed
        });
        if toggle {
            self.request_fullscreen(ctx, !self.fullscreen_target());
        } else if self.file_picker.is_none()
            && self.fullscreen_target()
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.request_fullscreen(ctx, false);
        }
        if self.fullscreen_request.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    fn display_interval_ms(&self) -> u64 {
        let columns = u64::from(self.grid.width / self.grid.cell_size);
        let rows = u64::from(self.grid.height / self.grid.cell_size);
        let cell = u64::from(self.display_cell_size);
        data_interval_ms(self.config.interval_ms, columns * rows * cell * cell)
    }

    fn set_interval_ms(&mut self, interval_ms: u64) -> Result<()> {
        if self.busy() {
            bail!("请先停止发送，再更改发送间隔");
        }
        if !FRAME_INTERVAL_OPTIONS_MS.contains(&interval_ms) {
            bail!("发送间隔只能选择 30、50 或 100 毫秒");
        }
        self.config.interval_ms = interval_ms;
        Ok(())
    }

    fn resize_grid(&mut self, grid: Grid, ctx: &egui::Context) -> Result<()> {
        if grid == self.grid {
            return Ok(());
        }
        let preparing = self.pending.take().is_some();
        let sending = self.session.take().is_some();
        if let Some(mut prepared) = self.transfer.take() {
            prepared.transfer = prepared
                .transfer
                .with_payload_capacity(grid.payload_capacity())?;
            self.transfer = Some(prepared);
        }
        self.grid = grid;
        self.last_grid_rect = None;
        self.requires_full_transfer = self.transfer.is_some();
        self.resend.clear();
        self.set_image(grid.checkerboard());
        if preparing {
            if let Some(source) = self.selected_file.clone() {
                self.prepare(source, ctx);
            }
        } else if sending {
            self.start(false)?;
            self.status = "画面容量已调整 · 正在重新校准并按新容量从头发送".into();
        }
        Ok(())
    }

    fn set_error(&mut self, message: impl Into<String>) {
        self.status = message.into();
        self.error = true;
    }

    fn set_image(&mut self, image: RgbImage) {
        self.texture.set(&image);
    }

    fn select_file(&mut self, path: PathBuf) {
        self.selected_file = Some(path);
        self.transfer = None;
        self.resend.clear();
        self.set_image(self.grid.checkerboard());
        self.status = "文件已选择 · 接收端定位完成后，点击“发送文件”".into();
        self.error = false;
        self.progress = 0.0;
        self.progress_label = "0 / 0".into();
    }

    fn choose_file(&mut self) {
        if self.busy() {
            return;
        }
        let start = self
            .selected_file
            .clone()
            .unwrap_or_else(crate::desktop_paths::default_desktop_dir);
        self.file_picker = Some(FilePicker::new(PickerMode::File, start));
    }

    fn show_file_picker(&mut self, ctx: &egui::Context) {
        if let Some(result) = self
            .file_picker
            .as_mut()
            .and_then(|picker| picker.show(ctx))
        {
            self.apply_file_picker_result(result);
        }
    }

    fn apply_file_picker_result(&mut self, result: PickerResult) {
        self.file_picker = None;
        if let PickerResult::Selected(path) = result {
            self.select_file(path);
        }
    }

    fn stop(&mut self) {
        // Dropping the receiver also makes a completed preparation unable to restart
        // a stopped/reset transfer. Only this UI thread ever advances a session.
        self.pending = None;
        self.session = None;
        self.status = "发送已停止 · 已准备的数据仍可补发".into();
        self.error = false;
    }

    fn reset(&mut self) {
        self.stop();
        self.selected_file = None;
        self.transfer = None;
        self.resend.clear();
        self.set_image(self.grid.checkerboard());
        self.progress = 0.0;
        self.progress_label = "0 / 0".into();
        self.status = "等待发送 · 请先让接收端定位下方棋盘格".into();
    }

    fn set_water_mask(&mut self, enabled: bool) -> Result<()> {
        if self.busy() {
            bail!("请先停止发送，再更改水印设置");
        }
        if self.config.water_mask == enabled {
            return Ok(());
        }
        let mut config = self.config.clone();
        config.water_mask = enabled;
        config.validate()?;
        let grid = Grid::new(
            self.grid.width,
            self.grid.height,
            self.grid.cell_size,
            enabled,
        )?;
        self.config = config;
        self.grid = grid;
        self.transfer = None;
        self.resend.clear();
        self.set_image(self.grid.checkerboard());
        self.progress = 0.0;
        self.progress_label = "0 / 0".into();
        self.status = format!(
            "水印已{} · 请在接收端选择相同水印设置，然后重新发送文件",
            if enabled { "开启" } else { "关闭" },
        );
        self.error = false;
        Ok(())
    }

    fn prepare(&mut self, source: PathBuf, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        if !self.resend.trim().is_empty() {
            if self
                .transfer
                .as_ref()
                .is_some_and(|prepared| prepared.source == source)
            {
                if let Err(error) = self.start(true) {
                    self.set_error(format!("无法补发：{error:#}"));
                }
            } else {
                self.set_error("请先完整发送此内容，再填写缺失帧号补发；发送新内容前请清空帧号。");
            }
            return;
        }
        let payload_capacity = self.grid.payload_capacity();
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        self.pending = Some(receiver);
        self.status = "正在准备传输数据…".into();
        self.error = false;
        self.progress = 0.0;
        self.progress_label = "正在准备".into();
        thread::spawn(move || {
            let result =
                prepare_file(source, payload_capacity).map_err(|error| format!("{error:#}"));
            let _ = sender.send(result);
            ctx.request_repaint();
        });
    }

    fn poll_preparation(&mut self) {
        let result = self.pending.as_ref().map(Receiver::try_recv);
        match result {
            Some(Ok(Ok(prepared))) => {
                self.pending = None;
                self.transfer = Some(prepared);
                if let Err(error) = self.start(false) {
                    self.set_error(format!("无法发送：{error:#}"));
                }
            }
            Some(Ok(Err(error))) => {
                self.pending = None;
                self.set_error(format!("准备失败：{error}"));
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.pending = None;
                self.set_error("准备任务意外中断，请重新发送。");
            }
            _ => {}
        }
    }

    fn start(&mut self, resend: bool) -> Result<()> {
        // A moved/resized idle board also invalidates the receiver's location
        // and partial assembly. Selected-frame resends need the full boundary.
        let recalibrating = resend && self.requires_full_transfer;
        let resend = resend && !self.requires_full_transfer;
        let prepared = self.transfer.as_ref().context("尚未准备传输数据")?;
        let total = prepared.transfer.total_frames();
        let (plan, count) = if resend {
            if self.resend.trim().is_empty() {
                bail!("请填写缺失帧号，例如 1,3-5,8");
            }
            let numbers = parse_resend(&self.resend, total)?;
            let count = numbers.len();
            (FramePlan::Selected { numbers, next: 0 }, count)
        } else {
            (
                FramePlan::All {
                    next: 1,
                    last: total,
                },
                total as usize,
            )
        };
        let info = self
            .grid
            .encode(&prepared.transfer.info_frame().encode()?)?;
        let byte_len = prepared.transfer.byte_len();
        // A fresh checkerboard marks the boundary between full file transfers.
        // Resends must retain the receiver's current incomplete assembly.
        self.set_image(if resend {
            info
        } else {
            self.grid.checkerboard()
        });
        self.session = Some(Session {
            plan,
            stage: if resend {
                SessionStage::Information
            } else {
                SessionStage::Calibration
            },
            displayed: 0,
            count,
            interval: Duration::from_millis(if resend {
                self.display_interval_ms().max(200)
            } else {
                self.display_interval_ms()
            }),
            started: Instant::now(),
            submitted_frame: None,
            visible_since: None,
            current_frame_bytes: 0,
            speed: SpeedMeter::default(),
            speed_label: format_speed(0.0),
            speed_updated_at: None,
        });
        self.requires_full_transfer = false;
        self.status = if recalibrating {
            "传输画面已调整 · 正在重新校准并从头发送".into()
        } else if resend {
            format!(
                "正在发送信息帧 · {} · 共 {total} 帧",
                format_byte_size(byte_len)
            )
        } else {
            "正在校准传输画面 · 即将开始发送文件".into()
        };
        self.error = false;
        self.progress = 0.0;
        self.progress_label = format!("0 / {count}");
        Ok(())
    }

    fn advance(&mut self, now: Instant) -> Result<()> {
        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        if !session.due(now) {
            return Ok(());
        }
        // Deliberately advance exactly once: a slow display must not skip frames
        // to catch up with an absolute timer or queued worker messages.
        if session.stage == SessionStage::Calibration {
            let prepared = self.transfer.as_ref().context("传输数据已丢失")?;
            let info = self
                .grid
                .encode(&prepared.transfer.info_frame().encode()?)?;
            self.status = format!(
                "正在发送信息帧 · {} · 共 {} 帧",
                format_byte_size(prepared.transfer.byte_len()),
                prepared.transfer.total_frames()
            );
            session.stage = SessionStage::Information;
            session.pause();
            self.set_image(info);
            return Ok(());
        }
        if matches!(session.stage, SessionStage::Data(_)) {
            // Count payload only after the rendered frame has completed its
            // dwell. The current length comes from the frame already encoded,
            // avoiding another payload clone or any work on the capture path.
            session
                .speed
                .record_bytes(session.current_frame_bytes as u64, now);
        } else {
            // Metadata and calibration do not contribute bytes or elapsed time
            // to the current data throughput.
            session.reset_speed(now);
        }
        if let Some(number) = session.plan.next() {
            let prepared = self.transfer.as_ref().context("传输数据已丢失")?;
            let frame = prepared.transfer.frame(number)?;
            let Frame::Data { payload, .. } = &frame else {
                unreachable!("a prepared data frame must contain a payload");
            };
            let image = self.grid.encode(&frame.encode()?)?;
            session.current_frame_bytes = payload.len();
            session.stage = SessionStage::Data(number);
            session.displayed += 1;
            session.pause();
            self.progress = session.displayed as f32 / session.count.max(1) as f32;
            self.progress_label = format!(
                "{} / {} 帧 · 原帧 {}/{}",
                session.displayed,
                session.count,
                number,
                prepared.transfer.total_frames()
            );
            self.status = format!("正在发送 · {}", prepared.label);
            self.set_image(image);
        } else {
            let elapsed = now.duration_since(session.started).as_secs_f32();
            self.status =
                format!("发送完成 · 用时 {elapsed:.1} 秒；如有缺失，请填写接收端提示的帧号补发");
            self.progress = 1.0;
            self.session = None;
        }
        Ok(())
    }

    fn fit_window_to_grid(&mut self, ctx: &egui::Context) {
        if self.fullscreen_request.is_some() {
            return;
        }
        let scale = ctx.pixels_per_point();
        let native_scale = ctx
            .input(|input| input.viewport().native_pixels_per_point)
            .unwrap_or(scale);
        let plan = if self.fullscreen {
            fullscreen_window_plan(ctx.content_rect().size() * scale, native_scale)
        } else {
            window_plan(&self.config, native_scale, self.screen_area)
        };
        if self.last_window_plan == Some(plan) && (scale - plan.chrome_scale).abs() < 0.001 {
            return;
        }
        self.last_window_plan = Some(plan);
        self.display_cell_size = plan.cell_size;
        let new_grid = Grid::new(
            plan.columns * plan.cell_size,
            plan.rows * plan.cell_size,
            plan.cell_size,
            self.config.water_mask,
        );
        if let Err(error) = new_grid.and_then(|grid| self.resize_grid(grid, ctx)) {
            self.set_error(format!("无法调整传输画面：{error:#}"));
        }
        ctx.set_zoom_factor(plan.chrome_scale / native_scale);
        if let Some(session) = self.session.as_mut() {
            session.pause();
            session.reset_speed(Instant::now());
        }
        if self.fullscreen {
            // The OS owns full-screen geometry. Resize cells to its actual viewport.
            return;
        }
        // Viewport commands use the current frame's scale, while the new UI
        // zoom takes effect in the following frame. Keep these targets physical.
        if let Some(maximum) = plan.max_inner_pixels {
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(
                (egui::vec2(640.0, 360.0) * plan.chrome_scale + egui::Vec2::splat(2.0))
                    .min(maximum)
                    / scale,
            ));
            ctx.send_viewport_cmd(egui::ViewportCommand::MaxInnerSize(maximum / scale));
        }
        // Apply the new constraints first, so the previous monitor's maximum
        // cannot clamp a larger window after moving to a larger display.
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(plan.inner_pixels / scale));
        if let Some(position) = plan.outer_position {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(position / scale));
        }
    }

    fn controls(&mut self, ctx: &egui::Context) {
        let colors = Palette::for_theme(self.theme);
        egui::TopBottomPanel::top("controls")
            // egui otherwise clips the first frame to its one-row default height.
            // A fixed height also keeps transmission pixels still as status changes.
            .exact_height(CONTROLS_HEIGHT)
            .frame(
                egui::Frame::NONE
                    .fill(colors.background)
                    .inner_margin(egui::Margin {
                        left: 16,
                        right: 16,
                        top: 12,
                        bottom: 12,
                    }),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                row(ui, 42.0, |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2((ui.available_width() - 200.0).max(0.0), 42.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.label(egui::RichText::new("LanSender").size(23.0).strong());
                            ui.add_space(2.0);
                            ui.label(
                                egui::RichText::new("通过屏幕传输文件")
                                    .small()
                                    .color(colors.muted),
                            );
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!self.busy(), action_button("设置", 84.0))
                            .clicked()
                        {
                            self.page = Page::Settings;
                        }
                        let label = if self.fullscreen_target() {
                            "退出全屏"
                        } else {
                            "全屏"
                        };
                        let response = ui.add(action_button(label, 100.0));
                        let clicked = response.clicked();
                        if !self.busy() {
                            response.on_hover_text("F11 切换全屏，Esc 退出全屏");
                        }
                        if clicked {
                            self.request_fullscreen(ctx, !self.fullscreen_target());
                        }
                    });
                });
                ui.add_space(8.0);
                card(colors).show(ui, |ui| {
                    row(ui, 34.0, |ui| {
                        let file_width = ((ui.available_width() - 8.0) / 3.0).floor();
                        ui.allocate_ui_with_layout(
                            egui::vec2(file_width, 34.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_min_width(file_width);
                                ui.set_max_width(file_width);
                                let label = self.selected_file.as_ref().map_or_else(
                                    || "将文件拖入窗口，或点击选择文件".to_owned(),
                                    |path| path.file_name().unwrap_or(path.as_os_str())
                                        .to_string_lossy().into_owned(),
                                );
                                let hover = self.selected_file.as_ref().map_or_else(
                                    || "支持将文件拖入窗口。".to_owned(),
                                    |path| path.display().to_string(),
                                );
                                ui.allocate_ui_with_layout(
                                    egui::vec2((file_width - 92.0).max(0.0), 34.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.set_min_width(ui.available_width());
                                        ui.label(egui::RichText::new("待发送文件").small().color(colors.muted));
                                        ui.add_space(2.0);
                                        let response = ui.add(egui::Label::new(
                                            egui::RichText::new(single_line(&label)).strong()
                                        ).truncate().show_tooltip_when_elided(!self.busy()));
                                        if !self.busy() { response.on_hover_text(hover); }
                                    },
                                );
                                if ui.add_enabled(!self.busy(), action_button("选择文件", 84.0)).clicked() {
                                    self.choose_file();
                                }
                            },
                        );
                        let actions_width = ui.available_width().floor();
                        ui.allocate_ui_with_layout(
                            egui::vec2(actions_width, 34.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_min_width(actions_width);
                                ui.spacing_mut().item_spacing.x = 0.0;
                                let send_width = if actions_width < 400.0 { 88.0 } else { 120.0 };
                                let button_width = if actions_width < 400.0 { 56.0 } else { 68.0 };
                                let watermark_width = 132.0;
                                let spacer = ((actions_width - send_width - watermark_width
                                    - 2.0 * button_width - 24.0) / 2.0).max(0.0);
                                if ui.add_enabled(
                                    !self.busy() && self.selected_file.is_some(),
                                    primary_button("发送文件", send_width),
                                ).clicked() && let Some(path) = self.selected_file.clone() {
                                    self.prepare(path, ctx);
                                }
                                ui.add_space(spacer + 8.0);
                                ui.allocate_ui_with_layout(
                                    egui::vec2(watermark_width, 34.0),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.set_min_width(watermark_width);
                                        ui.spacing_mut().item_spacing.x = 8.0;
                                        let mut enabled = self.config.water_mask;
                                        let response = ui.add_enabled_ui(!self.busy(), |ui| {
                                            watermark_switch(ui, &mut enabled)
                                        }).inner;
                                        if response.changed() && let Err(error) = self.set_water_mask(enabled) {
                                            self.set_error(format!("无法更改水印：{error:#}"));
                                        }
                                        if !self.busy() {
                                            response.on_hover_text("发送端与接收端的水印设置需要一致；更改后需重新发送文件");
                                        }
                                        ui.label(egui::RichText::new(if self.config.water_mask {
                                            "已开启"
                                        } else { "已关闭" }).small().color(colors.muted));
                                    },
                                );
                                ui.add_space(spacer + 8.0);
                                if ui.add_enabled(self.busy(), action_button("停止", button_width)).clicked() {
                                    self.stop();
                                }
                                ui.add_space(8.0);
                                if ui.add(action_button("重置", button_width))
                                    .on_hover_text("清空传输并恢复定位棋盘格").clicked() {
                                    self.reset();
                                }
                            },
                        );
                    });
                });
                ui.add_space(8.0);
                card(colors).show(ui, |ui| {
                    row(ui, 28.0, |ui| {
                        ui.label(egui::RichText::new("补发帧号").small().color(colors.muted));
                        let width = (ui.available_width() - 78.0).max(30.0);
                        ui.add_enabled_ui(!self.busy(), |ui| {
                            ui.add_sized(
                                [width, 28.0],
                                egui::TextEdit::singleline(&mut self.resend)
                                    .vertical_align(egui::Align::Center)
                                    .font(egui::TextStyle::Body)
                                    .hint_text("例如 1,3-5,8")
                                    .desired_width(width),
                            )
                            .on_hover_text(
                                "填写接收端报告的缺失帧号，从 1 开始；用逗号分隔，连续帧可用 3-5。",
                            );
                        });
                        if ui
                            .add_enabled(
                                !self.busy() && self.transfer.is_some(),
                                action_button("补发", 70.0),
                            )
                            .clicked()
                            && let Err(error) = self.start(true)
                        {
                            self.set_error(format!("无法补发：{error:#}"));
                        }
                    });
                    ui.add_space(8.0);
                    row(ui, 15.0, |ui| {
                        let bar_width = (ui.available_width() - 174.0).max(20.0);
                        ui.add(
                            egui::ProgressBar::new(self.progress)
                                .desired_width(bar_width)
                                .desired_height(6.0)
                                .fill(colors.accent),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let response = ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&self.progress_label)
                                        .small()
                                        .color(colors.muted),
                                )
                                .truncate()
                                .show_tooltip_when_elided(!self.busy()),
                            );
                            if !self.busy() {
                                response.on_hover_text(&self.progress_label);
                            }
                        });
                    });
                    ui.add_space(4.0);
                    row(ui, 18.0, |ui| {
                        let speed_width = 158.0;
                        let status_width =
                            (ui.available_width() - speed_width - ui.spacing().item_spacing.x)
                                .max(0.0);
                        let response = ui
                            .allocate_ui_with_layout(
                                egui::vec2(status_width, 18.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.set_min_width(status_width);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(single_line(&self.status))
                                                .size(12.0)
                                                .color(if self.error {
                                                    colors.error
                                                } else {
                                                    colors.muted
                                                }),
                                        )
                                        .truncate()
                                        .halign(egui::Align::Min)
                                        .show_tooltip_when_elided(!self.busy()),
                                    )
                                },
                            )
                            .inner;
                        if !self.busy() {
                            response.on_hover_text(&self.status);
                        }
                        let speed = self
                            .session
                            .as_mut()
                            .map_or("0 B/s", |session| session.speed_label(Instant::now()));
                        ui.allocate_ui_with_layout(
                            egui::vec2(speed_width, 18.0),
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.set_min_width(speed_width);
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(format!("当前速度：{speed}"))
                                            .size(12.0)
                                            .color(colors.muted),
                                    )
                                    .truncate()
                                    .halign(egui::Align::Max)
                                    .show_tooltip_when_elided(false),
                                );
                            },
                        );
                    });
                });
            });
    }

    fn settings_page(&mut self, ctx: &egui::Context) {
        let colors = Palette::for_theme(self.theme);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(colors.background).inner_margin(16))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let width = ui.available_width().min(720.0);
                    let left = (ui.available_width() - width) / 2.0;
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.add_space(left);
                        ui.allocate_ui_with_layout(
                            egui::vec2(width, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                row(ui, 42.0, |ui| {
                                    ui.allocate_ui_with_layout(
                                        egui::vec2((ui.available_width() - 92.0).max(0.0), 42.0),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new("设置").size(23.0).strong(),
                                            );
                                            ui.label(
                                                egui::RichText::new("传输与界面外观")
                                                    .small()
                                                    .color(colors.muted),
                                            );
                                        },
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.add(action_button("返回", 84.0)).clicked() {
                                                self.page = Page::Main;
                                            }
                                        },
                                    );
                                });
                                ui.add_space(12.0);
                                card(colors).show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    ui.label(egui::RichText::new("发送间隔").strong());
                                    ui.add_space(6.0);
                                    ui.horizontal(|ui| {
                                        for interval in FRAME_INTERVAL_OPTIONS_MS {
                                            let label = format!("{interval} 毫秒");
                                            if ui.add_enabled(
                                                !self.busy(),
                                                egui::Button::new(label)
                                                    .selected(self.config.interval_ms == interval)
                                                    .min_size(egui::vec2(100.0, 30.0)),
                                            ).clicked()
                                                && let Err(error) = self.set_interval_ms(interval)
                                            {
                                                self.set_error(format!("无法更改间隔：{error:#}"));
                                            }
                                        }
                                    });
                                    ui.add_space(6.0);
                                    ui.label(egui::RichText::new("30 毫秒：速度优先；50 毫秒：默认，兼顾速度与稳定性。")
                                        .small().color(colors.muted));
                                    ui.label(egui::RichText::new("100 毫秒：适合性能较弱的设备，或接收时出现丢帧。")
                                        .small().color(colors.muted));
                                    ui.label(egui::RichText::new("实际速度受屏幕刷新和截屏影响；补发帧至少显示 200 毫秒。")
                                        .small().color(colors.muted));
                                });
                                ui.add_space(12.0);
                                card(colors).show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    ui.label(egui::RichText::new("界面外观").strong());
                                    ui.add_space(6.0);
                                    theme_selector(ui, &mut self.theme, ctx);
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new("启动时采用系统主题")
                                            .small()
                                            .color(colors.muted),
                                    );
                                });
                                if self.error {
                                    ui.add_space(12.0);
                                    ui.colored_label(colors.error, &self.status);
                                }
                            },
                        );
                    });
                });
            });
    }

    fn show(&mut self, ctx: &egui::Context) {
        match self.page {
            Page::Main => {
                self.controls(ctx);
                // Navigation may have changed the page during this frame. The
                // settings page starts on the next frame, so both panels keep
                // their normal geometry while the click is handled.
                self.draw_grid(ctx);
            }
            Page::Settings => self.settings_page(ctx),
        }
    }

    fn draw_grid(&mut self, ctx: &egui::Context) {
        let colors = Palette::for_theme(self.theme);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(colors.canvas)
                    .inner_margin(if self.fullscreen {
                        egui::Margin {
                            left: 0,
                            right: 0,
                            top: 4,
                            bottom: 0,
                        }
                    } else {
                        egui::Margin::same(16)
                    }),
            )
            .show(ctx, |ui| {
                if self.fullscreen_request.is_some() {
                    self.pause_for_window_change();
                    ui.label("正在切换窗口显示模式…");
                    ctx.request_repaint_after(Duration::from_millis(50));
                    return;
                }
                ui.spacing_mut().item_spacing.y = 0.0;
                row(ui, 20.0, |ui| {
                    ui.label(egui::RichText::new("传输画面").size(13.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!(
                                    "{} × {} · 每帧 {} · {} 毫秒 · {}",
                                    self.grid.width / self.grid.cell_size * self.display_cell_size,
                                    self.grid.height / self.grid.cell_size * self.display_cell_size,
                                    format_byte_size(self.grid.payload_capacity()),
                                    self.session.as_ref().map_or(
                                        u128::from(self.display_interval_ms()),
                                        |session| session.interval.as_millis()
                                    ),
                                    if self.config.water_mask {
                                        "黑白"
                                    } else {
                                        "灰度"
                                    },
                                ))
                                .small()
                                .color(colors.muted),
                            )
                            .truncate(),
                        );
                    });
                });
                ui.add_space(8.0);
                let scale = ctx.pixels_per_point();
                let available = ui.available_rect_before_wrap().intersect(ui.clip_rect());
                // Nearest filtering and integer cell sizes retain each encoded
                // quadrant. Align both edges to whole physical pixels.
                let columns = self.grid.width / self.grid.cell_size;
                let rows = self.grid.height / self.grid.cell_size;
                let size = egui::vec2(
                    (columns * self.display_cell_size) as f32 / scale,
                    (rows * self.display_cell_size) as f32 / scale,
                );
                let origin = egui::pos2(
                    ((available.min.x + (available.width() - size.x).max(0.0) / 2.0) * scale)
                        .round()
                        .max((available.min.x * scale).ceil())
                        .min(
                            ((available.max.x - size.x) * scale)
                                .floor()
                                .max((available.min.x * scale).ceil()),
                        )
                        / scale,
                    (available.min.y * scale).ceil() / scale,
                );
                let rect = egui::Rect::from_min_size(origin, size);
                let window_origin = ctx
                    .input(|input| input.viewport().inner_rect.map(|rect| rect.min))
                    .unwrap_or(egui::Pos2::ZERO);
                let physical_rect = egui::Rect::from_min_size(
                    ((window_origin + rect.min.to_vec2()) * scale).round(),
                    egui::vec2(
                        (columns * self.display_cell_size) as f32,
                        (rows * self.display_cell_size) as f32,
                    ),
                );
                let minimized = ctx.input(|input| input.viewport().minimized.unwrap_or(false));
                let fits = available.contains_rect(rect)
                    && !minimized
                    && (self.fullscreen
                        || self
                            .screen_area
                            .is_none_or(|area| area.work_rect.contains_rect(physical_rect)));
                if !fits {
                    if let Some(session) = self.session.as_mut() {
                        session.pause();
                        session.reset_speed(Instant::now());
                    }
                    ui.colored_label(
                        colors.error,
                        "传输已暂停：请调整窗口或显示缩放，确保棋盘格完整显示在屏幕工作区内。",
                    );
                    ctx.request_repaint_after(Duration::from_millis(250));
                    return;
                }
                if self
                    .last_grid_rect
                    .is_some_and(|previous| previous != physical_rect)
                {
                    if self.session.is_some() {
                        match self.start(false) {
                            Ok(()) => {
                                self.status = "传输画面已调整 · 正在重新校准并从头发送".into()
                            }
                            Err(error) => {
                                self.session = None;
                                self.set_error(format!("重新校准失败：{error:#}"));
                            }
                        }
                    } else {
                        self.set_image(self.grid.checkerboard());
                        self.requires_full_transfer = self.transfer.is_some();
                        if self.requires_full_transfer {
                            self.status = "传输画面已调整 · 下次发送将重新校准并从头发送".into();
                        }
                    }
                }
                self.last_grid_rect = Some(physical_rect);
                if let Err(error) = self.advance(Instant::now()) {
                    self.session = None;
                    self.set_error(format!("发送失败：{error:#}"));
                }
                self.texture.paint(ui.painter(), rect);
                ui.allocate_rect(rect, egui::Sense::hover());
                if let Some(session) = self.session.as_mut() {
                    let now = Instant::now();
                    session.record_paint(ctx.cumulative_frame_nr(), now);
                    if let Some(since) = session.visible_since {
                        ctx.request_repaint_after(
                            session
                                .hold_duration()
                                .saturating_sub(now.duration_since(since)),
                        );
                    } else {
                        ctx.request_repaint();
                    }
                }
            });
    }
}

impl eframe::App for Sender {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.screen_area = window_layout::screen_area(ctx, _frame);
        self.update_fullscreen(ctx);
        self.fit_window_to_grid(ctx);
        self.poll_preparation();
        let can_drop = !self.busy() && self.file_picker.is_none();
        if can_drop {
            let files = ctx.input(|input| input.raw.dropped_files.clone());
            if let Some(path) = files.into_iter().find_map(|file| file.path) {
                self.select_file(path);
            }
        }
        self.show(ctx);
        self.show_file_picker(ctx);
        if self.pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}

fn row(ui: &mut egui::Ui, height: f32, contents: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), height),
        egui::Layout::left_to_right(egui::Align::Center),
        contents,
    );
}

fn single_line(text: &str) -> String {
    text.replace(['\n', '\r'], " ")
}

fn format_byte_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} 字节")
    }
}

/// Use an installed system font for filenames without changing the default UI
/// fonts. Font collection face zero includes the CJK glyphs used here.
pub(crate) fn install_cjk_fallback(ctx: &egui::Context) -> bool {
    let Some(bytes) = cjk_font_paths()
        .into_iter()
        .find_map(|path| std::fs::read(path).ok().filter(|bytes| !bytes.is_empty()))
    else {
        return false;
    };
    let name = "system-cjk-fallback".to_owned();
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert(name.clone(), Arc::new(egui::FontData::from_owned(bytes)));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push(name.clone());
    }
    ctx.set_fonts(fonts);
    true
}

fn cjk_font_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    let paths = [
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/System/Library/Fonts/PingFang.ttc",
    ];
    #[cfg(target_os = "windows")]
    {
        let windows = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        ["msyh.ttc", "simhei.ttf", "simsun.ttc"]
            .into_iter()
            .map(|name| windows.join("Fonts").join(name))
            .collect()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let paths = [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansSC-Regular.ttf",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/local/share/fonts/NotoSansCJK-Regular.ttc",
    ];
    #[cfg(not(target_os = "windows"))]
    paths.into_iter().map(PathBuf::from).collect()
}

fn prepare_file(source: PathBuf, payload_capacity: usize) -> Result<Prepared> {
    let name = source
        .file_name()
        .context("文件路径缺少文件名")?
        .to_str()
        .context("文件名不是有效的 UTF-8 文本")?
        .to_owned();
    let bytes = std::fs::read(&source).with_context(|| format!("无法读取 {}", source.display()))?;
    Ok(Prepared {
        source,
        label: name.clone(),
        transfer: Transfer::new(TransferKind::File, name, bytes, payload_capacity)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_intervals_apply_and_100ms_handles_slow_capture_cycles() {
        use crate::receive::ReceiveSession;

        fn sample(
            transfer: &Transfer,
            dwell_ms: u64,
            phase: u64,
            capacity: usize,
        ) -> ReceiveSession {
            let mut received = ReceiveSession::new(capacity, 32 * 1024 * 1024).unwrap();
            let finish = 1000 + dwell_ms * u64::from(transfer.total_frames());
            let mut time = phase;
            let mut sample_number = 0;
            while time < finish {
                let frame = if time < 1000 {
                    transfer.info_frame()
                } else {
                    transfer
                        .frame(((time - 1000) / dwell_ms + 1) as u32)
                        .unwrap()
                };
                let decoded = crate::protocol::Frame::decode(&frame.encode().unwrap()).unwrap();
                received.accept(decoded).unwrap();
                // A deterministic timing model of capture/refresh delays; it
                // does not claim to reproduce a measured Windows frame clock.
                time += [51, 77, 80, 66, 58][sample_number % 5];
                sample_number += 1;
            }
            received
        }

        let capacity = Config::default().grid().unwrap().payload_capacity();
        for bytes in [3 * 1024 * 1024, 16 * 1024 * 1024] {
            let payload: Vec<_> = (0..bytes).map(|index| (index * 97 % 256) as u8).collect();
            let transfer = Transfer::new(
                TransferKind::File,
                "4K传输.bin".into(),
                payload.clone(),
                capacity,
            )
            .unwrap();
            assert!(!sample(&transfer, 30, 17, capacity).is_complete());
            let dwell = data_interval_ms(100, 1440 * 800);
            for phase in [0, 17, 79] {
                let received = sample(&transfer, dwell, phase, capacity);
                assert!(received.is_complete());
                assert!(received.missing_frames(100).is_empty());
                let output = tempfile::tempdir().unwrap();
                let path = received.save_file(output.path(), false).unwrap();
                assert_eq!(std::fs::read(path).unwrap(), payload);
            }
        }

        let mut sender = prepared_sender();
        for interval in FRAME_INTERVAL_OPTIONS_MS {
            sender.set_interval_ms(interval).unwrap();
            for cell in [2, 4] {
                sender.display_cell_size = cell;
                sender.start(false).unwrap();
                assert_eq!(
                    sender.session.as_ref().unwrap().interval,
                    Duration::from_millis(interval)
                );
                assert!(
                    sender.set_interval_ms(50).is_err(),
                    "active settings must be locked"
                );
                sender.stop();
            }
        }
        assert!(sender.set_interval_ms(40).is_err());
        sender.resend = "1".into();
        sender.start(true).unwrap();
        assert_eq!(
            sender.session.as_ref().unwrap().interval,
            Duration::from_millis(200)
        );
    }

    #[test]
    fn changing_capacity_repartitions_cached_file_and_restarts_calibration() {
        let ctx = egui::Context::default();
        let mut sender = prepared_sender();
        let bytes: Vec<_> = (0..160_000).map(|i| (i % 251) as u8).collect();
        sender.transfer.as_mut().unwrap().transfer = Transfer::new(
            TransferKind::File,
            "文件.bin".into(),
            bytes.clone(),
            sender.grid.payload_capacity(),
        )
        .unwrap();
        let original_frames = sender.transfer.as_ref().unwrap().transfer.total_frames();
        sender.start(false).unwrap();
        sender.session.as_mut().unwrap().stage = SessionStage::Data(2);
        let larger = Grid::new(1200, 640, 2, true).unwrap();
        sender.resize_grid(larger, &ctx).unwrap();
        let prepared = &sender.transfer.as_ref().unwrap().transfer;
        assert!(prepared.total_frames() < original_frames);
        let mut received =
            crate::receive::ReceiveSession::new(larger.payload_capacity(), bytes.len()).unwrap();
        received.accept(prepared.info_frame()).unwrap();
        for frame in 1..=prepared.total_frames() {
            received.accept(prepared.frame(frame).unwrap()).unwrap();
        }
        let output = tempfile::tempdir().unwrap();
        assert_eq!(
            std::fs::read(received.save_file(output.path(), false).unwrap()).unwrap(),
            bytes
        );
        assert_eq!(
            sender.session.as_ref().unwrap().stage,
            SessionStage::Calibration
        );
        assert!(matches!(
            sender.session.as_ref().unwrap().plan,
            FramePlan::All { next: 1, .. }
        ));
        assert!(sender.last_grid_rect.is_none());
    }

    fn session() -> Session {
        Session {
            plan: FramePlan::All { next: 1, last: 3 },
            stage: SessionStage::Information,
            displayed: 0,
            count: 3,
            interval: Duration::from_millis(30),
            started: Instant::now(),
            submitted_frame: None,
            visible_since: None,
            current_frame_bytes: 0,
            speed: SpeedMeter::default(),
            speed_label: format_speed(0.0),
            speed_updated_at: None,
        }
    }

    #[test]
    fn information_dwell_starts_after_a_subsequent_native_frame() {
        let mut session = session();
        let start = Instant::now();
        session.record_paint(10, start);
        assert!(!session.due(start + Duration::from_secs(3)));
        session.record_paint(10, start + Duration::from_secs(3));
        assert!(!session.due(start + Duration::from_secs(3)));
        session.record_paint(11, start + Duration::from_secs(3));
        assert!(!session.due(start + Duration::from_millis(3999)));
        assert!(session.due(start + Duration::from_secs(4)));
    }

    #[test]
    fn pause_requires_the_current_frame_to_be_shown_again() {
        let mut session = session();
        let start = Instant::now();
        session.stage = SessionStage::Data(1);
        session.record_paint(10, start);
        session.record_paint(11, start);
        assert!(session.due(start + Duration::from_millis(30)));
        session.pause();
        assert!(!session.due(start + Duration::from_secs(10)));
        session.record_paint(12, start + Duration::from_secs(10));
        session.record_paint(13, start + Duration::from_secs(10));
        assert!(!session.due(start + Duration::from_millis(10029)));
        assert!(session.due(start + Duration::from_millis(10030)));
    }

    fn prepared_sender() -> Sender {
        let ctx = egui::Context::default();
        let config = Config::default();
        let mut sender = Sender::new(&ctx, config.clone(), config.grid().unwrap(), None);
        sender.transfer = Some(Prepared {
            source: PathBuf::from("文件.txt"),
            label: "文件.txt".into(),
            transfer: Transfer::new(
                TransferKind::File,
                "文件.txt".into(),
                b"hello".to_vec(),
                sender.grid.payload_capacity(),
            )
            .unwrap(),
        });
        sender
    }

    #[test]
    fn every_full_transfer_holds_visible_calibration_before_metadata_and_data() {
        let mut sender = prepared_sender();
        sender.start(false).unwrap();
        let start = sender.session.as_ref().unwrap().started;
        assert_eq!(
            sender.session.as_ref().unwrap().stage,
            SessionStage::Calibration
        );
        // An unpainted image cannot advance, even after a long stall.
        sender.advance(start + Duration::from_secs(3)).unwrap();
        assert_eq!(
            sender.session.as_ref().unwrap().stage,
            SessionStage::Calibration
        );
        let active = sender.session.as_mut().unwrap();
        active.record_paint(10, start);
        active.record_paint(11, start + Duration::from_secs(3));
        sender.advance(start + Duration::from_millis(3999)).unwrap();
        assert_eq!(
            sender.session.as_ref().unwrap().stage,
            SessionStage::Calibration
        );
        sender.advance(start + Duration::from_secs(4)).unwrap();
        let active = sender.session.as_mut().unwrap();
        assert_eq!(active.stage, SessionStage::Information);
        assert_eq!(active.displayed, 0);
        assert!(active.visible_since.is_none());
        active.record_paint(12, start + Duration::from_secs(4));
        active.record_paint(13, start + Duration::from_secs(4));
        sender.advance(start + Duration::from_millis(4999)).unwrap();
        assert_eq!(
            sender.session.as_ref().unwrap().stage,
            SessionStage::Information
        );
        sender.advance(start + Duration::from_secs(5)).unwrap();
        let active = sender.session.as_mut().unwrap();
        assert_eq!(active.stage, SessionStage::Data(1));
        assert_eq!(active.displayed, 1);
        active.record_paint(14, start + Duration::from_secs(5));
        active.record_paint(15, start + Duration::from_secs(5));
        sender.advance(start + Duration::from_secs(10)).unwrap();
        assert!(sender.session.is_none());
        // Sending the same prepared file again still creates a new boundary.
        sender.start(false).unwrap();
        assert_eq!(
            sender.session.as_ref().unwrap().stage,
            SessionStage::Calibration
        );
    }

    #[test]
    fn a_resend_starts_with_metadata_without_resetting_the_receiver() {
        let mut sender = prepared_sender();
        sender.resend = "1".into();
        sender.start(true).unwrap();
        let active = sender.session.as_mut().unwrap();
        assert_eq!(active.stage, SessionStage::Information);
        let start = active.started;
        active.record_paint(10, start);
        active.record_paint(11, start);
        sender.advance(start + Duration::from_secs(1)).unwrap();
        assert_eq!(
            sender.session.as_ref().unwrap().stage,
            SessionStage::Data(1)
        );
        assert_eq!(sender.session.as_ref().unwrap().displayed, 1);
    }

    #[test]
    fn current_speed_counts_only_completed_payload_frames_and_expires_when_stalled() {
        let mut sender = prepared_sender();
        let capacity = sender.grid.payload_capacity();
        sender.transfer.as_mut().unwrap().transfer = Transfer::new(
            TransferKind::File,
            "速度测试.bin".into(),
            vec![0x5a; capacity * 2 + 13],
            capacity,
        )
        .unwrap();
        sender.start(false).unwrap();
        let start = sender.session.as_ref().unwrap().started;
        let active = sender.session.as_mut().unwrap();
        active.record_paint(1, start);
        active.record_paint(2, start);
        sender.advance(start + Duration::from_secs(1)).unwrap();
        let active = sender.session.as_mut().unwrap();
        assert_eq!(
            active
                .speed
                .bytes_per_second(start + Duration::from_secs(1)),
            0.0
        );
        active.record_paint(3, start + Duration::from_secs(1));
        active.record_paint(4, start + Duration::from_secs(1));
        let first_data = start + Duration::from_secs(2);
        sender.advance(first_data).unwrap();
        let active = sender.session.as_mut().unwrap();
        assert_eq!(active.current_frame_bytes, capacity);
        assert_eq!(active.speed.bytes_per_second(first_data), 0.0);
        active.record_paint(5, first_data);
        active.record_paint(6, first_data);
        let second_data = first_data + Duration::from_millis(50);
        sender.advance(second_data).unwrap();
        let active = sender.session.as_mut().unwrap();
        assert_eq!(active.stage, SessionStage::Data(2));
        assert!((active.speed.bytes_per_second(second_data) - capacity as f64 / 0.05).abs() < 0.1);
        active.record_paint(7, second_data);
        active.record_paint(8, second_data);
        let final_data = second_data + Duration::from_millis(50);
        sender.advance(final_data).unwrap();
        let active = sender.session.as_mut().unwrap();
        assert_eq!(active.stage, SessionStage::Data(3));
        assert_eq!(active.current_frame_bytes, 13);
        assert!((active.speed.bytes_per_second(final_data) - capacity as f64 / 0.05).abs() < 0.1);

        // An unpainted final frame contributes no bytes, even if the UI stalls.
        let stalled = final_data + Duration::from_secs(2);
        sender.advance(stalled).unwrap();
        let active = sender.session.as_mut().unwrap();
        assert_eq!(active.stage, SessionStage::Data(3));
        assert_eq!(active.speed.bytes_per_second(stalled), 0.0);
        active.reset_speed(stalled);
        assert_eq!(active.speed_label(stalled), "0 B/s");
        sender.stop();
        assert!(sender.session.is_none());
    }

    #[test]
    fn a_resend_measures_its_actual_frame_bytes_with_the_resend_dwell() {
        let mut sender = prepared_sender();
        let capacity = sender.grid.payload_capacity();
        sender.transfer.as_mut().unwrap().transfer = Transfer::new(
            TransferKind::File,
            "补发速度.bin".into(),
            vec![0x5a; capacity * 2 + 13],
            capacity,
        )
        .unwrap();
        sender.resend = "1,3".into();
        sender.start(true).unwrap();
        let start = sender.session.as_ref().unwrap().started;
        let active = sender.session.as_mut().unwrap();
        active.record_paint(1, start);
        active.record_paint(2, start);
        let first_data = start + Duration::from_secs(1);
        sender.advance(first_data).unwrap();
        let active = sender.session.as_mut().unwrap();
        assert_eq!(active.speed.bytes_per_second(first_data), 0.0);
        active.record_paint(3, first_data);
        active.record_paint(4, first_data);
        sender
            .advance(first_data + Duration::from_millis(199))
            .unwrap();
        assert_eq!(
            sender.session.as_ref().unwrap().stage,
            SessionStage::Data(1)
        );
        let next_data = first_data + Duration::from_millis(200);
        sender.advance(next_data).unwrap();
        let active = sender.session.as_mut().unwrap();
        assert_eq!(active.stage, SessionStage::Data(3));
        assert_eq!(active.current_frame_bytes, 13);
        assert!((active.speed.bytes_per_second(next_data) - capacity as f64 / 0.2).abs() < 0.1);
        sender.reset();
        assert!(sender.session.is_none());
    }

    #[test]
    fn plans_preserve_every_frame_and_selected_order() {
        let mut all = FramePlan::All { next: 1, last: 3 };
        assert_eq!(all.next(), Some(1));
        assert_eq!(all.next(), Some(2));
        assert_eq!(all.next(), Some(3));
        assert_eq!(all.next(), None);
        let mut selected = FramePlan::Selected {
            numbers: vec![1, 3, 8],
            next: 0,
        };
        assert_eq!(selected.next(), Some(1));
        assert_eq!(selected.next(), Some(3));
        assert_eq!(selected.next(), Some(8));
        assert_eq!(selected.next(), None);
    }

    fn painted_grid_rect(
        output: &egui::FullOutput,
        texture: &TransferTexture,
    ) -> Option<egui::Rect> {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Mesh(mesh) if texture.contains_texture(mesh.texture_id) => {
                    Some(mesh.calc_bounds())
                }
                _ => None,
            })
            .reduce(|bounds, tile| bounds.union(tile))
    }

    fn headless_input(scale: f32, size: egui::Vec2) -> egui::RawInput {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(scale);
        input
    }

    fn visible_text(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        fn collect(shape: &egui::Shape, clip: egui::Rect, result: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => {
                    // Galley coordinates include left/center/right alignment;
                    // right-aligned text ends at its position instead of starting there.
                    let bounds = text.galley.rect.translate(text.pos.to_vec2());
                    if bounds.intersect(clip).area() > 0.0 {
                        result.push((text.galley.job.text.clone(), bounds));
                    }
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, clip, result);
                    }
                }
                _ => {}
            }
        }
        let mut result = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, clipped.clip_rect, &mut result);
        }
        result
    }

    fn text_bounds(output: &egui::FullOutput, label: &str) -> egui::Rect {
        visible_text(output)
            .into_iter()
            .find(|(text, _)| text == label)
            .unwrap_or_else(|| panic!("missing visible text {label}"))
            .1
    }

    fn render(ctx: &egui::Context, sender: &mut Sender, scale: f32) -> egui::FullOutput {
        ctx.run(headless_input(scale, egui::vec2(640.0, 480.0)), |ctx| {
            sender.show(ctx);
            sender.show_file_picker(ctx);
        })
    }

    fn click(ctx: &egui::Context, sender: &mut Sender, point: egui::Pos2, scale: f32) {
        let mut press = headless_input(scale, egui::vec2(640.0, 480.0));
        press.events.extend([
            egui::Event::PointerMoved(point),
            egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        let _ = ctx.run(press, |ctx| {
            sender.show(ctx);
            sender.show_file_picker(ctx);
        });
        let mut release = headless_input(scale, egui::vec2(640.0, 480.0));
        release.events.push(egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(release, |ctx| {
            sender.show(ctx);
            sender.show_file_picker(ctx);
        });
    }

    #[test]
    fn file_picker_button_cancel_and_confirm_preserve_native_paths() {
        let directory = tempfile::tempdir().unwrap();
        let previous = directory.path().join("原文件.txt");
        let chosen = directory.path().join("待发送 空格文件.txt");
        std::fs::write(&previous, b"previous").unwrap();
        std::fs::write(&chosen, b"chosen").unwrap();
        let ctx = egui::Context::default();
        ctx.options_mut(|options| options.max_passes = 1.try_into().unwrap());
        install_cjk_fallback(&ctx);
        let config = Config::default();
        let mut sender = Sender::new(
            &ctx,
            config.clone(),
            config.grid().unwrap(),
            Some(previous.clone()),
        );
        sender.transfer = prepared_sender().transfer;
        sender.resend = "1".into();
        let _ = render(&ctx, &mut sender, 1.0);
        let output = render(&ctx, &mut sender, 1.0);
        click(
            &ctx,
            &mut sender,
            text_bounds(&output, "选择文件").center(),
            1.0,
        );
        assert!(sender.file_picker.is_some());
        let _ = render(&ctx, &mut sender, 1.0);
        let output = render(&ctx, &mut sender, 1.0);
        click(
            &ctx,
            &mut sender,
            text_bounds(&output, "取消").center(),
            1.0,
        );
        assert!(sender.file_picker.is_none());
        assert_eq!(sender.selected_file, Some(previous.clone()));
        assert!(sender.transfer.is_some());
        assert_eq!(sender.resend, "1");

        let output = render(&ctx, &mut sender, 1.0);
        click(
            &ctx,
            &mut sender,
            text_bounds(&output, "选择文件").center(),
            1.0,
        );
        let _ = render(&ctx, &mut sender, 1.0);
        let output = render(&ctx, &mut sender, 1.0);
        click(
            &ctx,
            &mut sender,
            text_bounds(&output, "文件  待发送 空格文件.txt").center(),
            1.0,
        );
        assert_eq!(sender.selected_file, Some(previous));
        let output = render(&ctx, &mut sender, 1.0);
        click(
            &ctx,
            &mut sender,
            text_bounds(&output, "确认选择文件").center(),
            1.0,
        );
        assert!(sender.file_picker.is_none());
        assert_eq!(sender.selected_file, Some(chosen));
        assert!(sender.transfer.is_none());
        assert!(sender.resend.is_empty());
        assert_eq!(sender.progress, 0.0);
        assert!(sender.status.starts_with("文件已选择"));
    }

    #[test]
    fn theme_settings_and_main_watermark_switch_keep_the_selected_file() {
        for scale in [1.0, 2.0] {
            let ctx = egui::Context::default();
            ctx.options_mut(|options| options.max_passes = 1.try_into().unwrap());
            install_cjk_fallback(&ctx);
            let config = Config::default();
            let selected_file = PathBuf::from("文件.txt");
            let mut sender = Sender::new(
                &ctx,
                config.clone(),
                config.grid().unwrap(),
                Some(selected_file.clone()),
            );
            sender.theme = egui::Theme::Light;
            ctx.set_theme(sender.theme);
            sender.transfer = prepared_sender().transfer;
            sender.resend = "1".into();
            assert!(sender.config.water_mask && sender.grid.water_mask);
            let _ = render(&ctx, &mut sender, scale);
            let main = render(&ctx, &mut sender, scale);
            let _ = text_bounds(&main, "已开启");
            click(
                &ctx,
                &mut sender,
                text_bounds(&main, "水印").center(),
                scale,
            );
            assert!(!sender.config.water_mask && !sender.grid.water_mask);
            assert_eq!(sender.selected_file, Some(selected_file.clone()));
            assert!(sender.transfer.is_none());
            assert!(sender.resend.is_empty());
            let main = render(&ctx, &mut sender, scale);
            let _ = text_bounds(&main, "已关闭");
            click(
                &ctx,
                &mut sender,
                text_bounds(&main, "水印").center(),
                scale,
            );
            assert!(sender.config.water_mask && sender.grid.water_mask);
            let bytes = [0x12, 0x3a, 0xcf, 0x00, 0xff];
            let image = sender.grid.encode(&bytes).unwrap();
            assert_eq!(image.get_pixel(0, 0).0, [255, 255, 255]);
            assert_eq!(sender.grid.decode(&image).unwrap(), bytes);
            let main = render(&ctx, &mut sender, scale);
            click(
                &ctx,
                &mut sender,
                text_bounds(&main, "设置").center(),
                scale,
            );
            assert_eq!(sender.page, Page::Settings);
            let settings = render(&ctx, &mut sender, scale);
            for label in ["返回", "界面外观", "深色", "浅色"] {
                let _ = text_bounds(&settings, label);
            }
            for label in ["水印", "水印模式", "关闭", "开启"] {
                assert!(
                    !visible_text(&settings)
                        .iter()
                        .any(|(text, _)| text == label)
                );
            }
            assert!(painted_grid_rect(&settings, &sender.texture).is_none());
            click(
                &ctx,
                &mut sender,
                text_bounds(&settings, "深色").center(),
                scale,
            );
            assert_eq!(sender.theme, egui::Theme::Dark);
            assert_eq!(sender.selected_file, Some(selected_file));
            let settings = render(&ctx, &mut sender, scale);
            click(
                &ctx,
                &mut sender,
                text_bounds(&settings, "返回").center(),
                scale,
            );
            assert_eq!(sender.page, Page::Main);
            let main = render(&ctx, &mut sender, scale);
            for label in ["LanSender", "水印", "已开启"] {
                let _ = text_bounds(&main, label);
            }
            for label in ["界面外观", "深色", "浅色", "水印模式", "关闭", "开启"] {
                assert!(!visible_text(&main).iter().any(|(text, _)| text == label));
            }
        }
    }

    #[test]
    fn active_transmission_cannot_open_settings_or_change_watermark() {
        let ctx = egui::Context::default();
        let config = Config::default();
        let mut sender = Sender::new(&ctx, config.clone(), config.grid().unwrap(), None);
        sender.session = Some(session());
        let _ = render(&ctx, &mut sender, 2.0);
        let main = render(&ctx, &mut sender, 2.0);
        click(&ctx, &mut sender, text_bounds(&main, "设置").center(), 2.0);
        assert_eq!(sender.page, Page::Main);
        let main = render(&ctx, &mut sender, 2.0);
        click(&ctx, &mut sender, text_bounds(&main, "水印").center(), 2.0);
        assert!(sender.set_water_mask(false).is_err());
        assert!(sender.config.water_mask && sender.grid.water_mask);
        assert!(sender.session.is_some());
    }

    #[test]
    fn interval_settings_select_all_options_without_clearing_the_file() {
        for scale in [1.0, 2.0] {
            let ctx = egui::Context::default();
            install_cjk_fallback(&ctx);
            let mut sender = prepared_sender();
            sender.selected_file = Some(PathBuf::from("文件.txt"));
            sender.page = Page::Settings;
            assert_eq!(sender.config.interval_ms, 50);
            for interval in FRAME_INTERVAL_OPTIONS_MS {
                let _ = render(&ctx, &mut sender, scale);
                let settings = render(&ctx, &mut sender, scale);
                click(
                    &ctx,
                    &mut sender,
                    text_bounds(&settings, &format!("{interval} 毫秒")).center(),
                    scale,
                );
                assert_eq!(sender.config.interval_ms, interval);
                assert_eq!(sender.selected_file, Some(PathBuf::from("文件.txt")));
                assert!(sender.transfer.is_some());
                sender.page = Page::Main;
                sender.start(false).unwrap();
                assert_eq!(
                    sender.session.as_ref().unwrap().interval,
                    Duration::from_millis(interval)
                );
                sender.stop();
                sender.page = Page::Settings;
            }
        }
    }

    #[test]
    fn displayed_texture_has_exact_physical_dimensions_at_multiple_dpi_scales() {
        for scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
            let config = Config {
                width: 64,
                height: 32,
                cell_size: 4,
                ..Config::default()
            };
            let ctx = egui::Context::default();
            let mut sender = Sender::new(&ctx, config.clone(), config.grid().unwrap(), None);
            let output = ctx.run(headless_input(scale, egui::vec2(640.0, 480.0)), |ctx| {
                sender.draw_grid(ctx)
            });
            let rect = painted_grid_rect(&output, &sender.texture).expect("grid must be drawn");
            assert!((rect.width() * scale - 64.0).abs() < 0.001);
            assert!((rect.height() * scale - 32.0).abs() < 0.001);
            assert!((rect.min.x * scale - (rect.min.x * scale).round()).abs() < 0.001);
            assert!((rect.min.y * scale - (rect.min.y * scale).round()).abs() < 0.001);
        }
    }

    #[test]
    fn an_insufficient_viewport_draws_no_partial_grid_and_pauses_dwell() {
        let config = Config {
            width: 640,
            height: 480,
            cell_size: 4,
            ..Config::default()
        };
        let ctx = egui::Context::default();
        let mut sender = Sender::new(&ctx, config.clone(), config.grid().unwrap(), None);
        let mut active = session();
        active.visible_since = Some(Instant::now());
        sender.session = Some(active);
        let output = ctx.run(headless_input(1.0, egui::vec2(320.0, 240.0)), |ctx| {
            sender.draw_grid(ctx)
        });
        assert!(painted_grid_rect(&output, &sender.texture).is_none());
        assert!(sender.session.as_ref().unwrap().visible_since.is_none());
    }

    #[test]
    fn an_available_system_fallback_renders_chinese_filenames() {
        let ctx = egui::Context::default();
        if !install_cjk_fallback(&ctx) {
            return;
        }
        let _ = ctx.run(headless_input(1.0, egui::vec2(640.0, 480.0)), |ctx| {
            ctx.fonts_mut(|fonts| {
                for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                    assert!(fonts.has_glyphs(&egui::FontId::new(14.0, family), "中文文件.txt"));
                }
            });
        });
    }

    #[test]
    fn proportional_window_uses_more_cells_as_screen_space_increases() {
        let config = Config::default();
        let mut capacities = Vec::new();
        for (size, scale) in [
            ([1366.0, 728.0], 1.0),
            ([1920.0, 1040.0], 1.0),
            ([3840.0, 2080.0], 2.0),
        ] {
            let area = ScreenArea {
                work_rect: egui::Rect::from_min_size(egui::pos2(-1920.0, 40.0), size.into()),
                decoration: egui::vec2(16.0, 39.0) * scale,
                can_position: true,
            };
            let plan = window_plan(&config, scale, Some(area));
            assert_eq!(plan.cell_size, 2);
            assert!(plan.columns >= 160 && plan.rows >= 80);
            assert!(plan.columns.is_multiple_of(4) && plan.rows.is_multiple_of(4));
            assert!(plan.chrome_scale <= scale);
            let outer = egui::Rect::from_min_size(
                plan.outer_position.unwrap(),
                plan.inner_pixels + area.decoration,
            );
            assert!(area.work_rect.contains_rect(outer));
            assert!((outer.width() / size[0] - 0.6).abs() < 0.001);
            assert!((outer.height() / size[1] - 0.8).abs() < 0.001);
            assert!(
                plan.inner_pixels.x >= (plan.columns * 2) as f32 + 32.0 * plan.chrome_scale + 2.0
            );
            assert!(
                plan.inner_pixels.y
                    >= (plan.rows * 2) as f32 + WINDOW_CHROME_HEIGHT * plan.chrome_scale + 2.0
            );
            capacities.push(
                Grid::new(plan.columns * 2, plan.rows * 2, 2, true)
                    .unwrap()
                    .payload_capacity(),
            );
        }
        assert!(capacities.windows(2).all(|pair| pair[1] > pair[0]));
        assert!(capacities[1] > Config::default().grid().unwrap().payload_capacity());
        assert_eq!(window_plan(&config, 1.0, None).cell_size, 4);
    }

    #[test]
    fn dynamic_grid_uses_both_dimensions_without_an_aspect_ratio_limit() {
        for available in [
            egui::vec2(1920.0, 808.0),
            egui::vec2(3440.0, 808.0),
            egui::vec2(1080.0, 1640.0),
            egui::vec2(7680.0, 4088.0),
        ] {
            let (columns, rows) = dynamic_grid_size(available);
            assert!(available.x - ((columns * 2) as f32) < 8.0);
            assert!(available.y - ((rows * 2) as f32) < 8.0);
            assert!(columns.is_multiple_of(4) && rows.is_multiple_of(4));
            assert!(columns >= 160 && rows >= 80);
        }
    }

    #[test]
    fn sender_initial_image_tiles_without_reducing_the_grid() {
        let ctx = egui::Context::default();
        ctx.input_mut(|input| input.max_texture_side = 1024);
        let config = Config::default();
        let grid = config.grid().unwrap();
        let sender = Sender::new(&ctx, config, grid, None);
        assert_eq!(sender.grid, grid);
        assert_eq!(
            sender.texture.size(),
            [grid.width as usize, grid.height as usize]
        );
        let output = ctx.run(egui::RawInput::default(), |_| {});
        let mut pixel_count = 0;
        for (id, delta) in &output.textures_delta.set {
            if sender.texture.contains_texture(*id) {
                let size = delta.image.size();
                assert!(size[0] <= 1024 && size[1] <= 1024);
                pixel_count += size[0] * size[1];
            }
        }
        assert_eq!(pixel_count, grid.width as usize * grid.height as usize);
    }

    /// Model the native backend's physical window and command constraints. This
    /// also exercises egui's delayed zoom and its first-frame layout cache.
    struct DesktopWindow {
        area: ScreenArea,
        native_scale: f32,
        inner_pixels: egui::Vec2,
        outer_position: egui::Pos2,
        minimum: egui::Vec2,
        maximum: egui::Vec2,
        fullscreen: bool,
    }

    impl DesktopWindow {
        fn render(&mut self, ctx: &egui::Context, sender: &mut Sender) -> egui::FullOutput {
            self.render_with_events(ctx, sender, Vec::new())
        }

        fn render_with_events(
            &mut self,
            ctx: &egui::Context,
            sender: &mut Sender,
            events: Vec<egui::Event>,
        ) -> egui::FullOutput {
            let scale = ctx.zoom_factor() * self.native_scale;
            let mut input = headless_input(self.native_scale, self.inner_pixels / scale);
            input.events = events;
            let viewport = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
            viewport.fullscreen = Some(self.fullscreen);
            let decoration = if self.fullscreen {
                egui::Vec2::ZERO
            } else {
                self.area.decoration
            };
            let border = decoration.x / 2.0;
            let client_position = self.outer_position + egui::vec2(border, decoration.y - border);
            viewport.inner_rect = Some(egui::Rect::from_min_size(
                client_position / scale,
                self.inner_pixels / scale,
            ));
            viewport.outer_rect = Some(egui::Rect::from_min_size(
                self.outer_position / scale,
                (self.inner_pixels + decoration) / scale,
            ));
            sender.screen_area = Some(self.area);
            let output = ctx.run(input, |ctx| {
                sender.update_fullscreen(ctx);
                sender.fit_window_to_grid(ctx);
                sender.show(ctx);
                sender.show_file_picker(ctx);
            });
            let command_scale = ctx.zoom_factor() * self.native_scale;
            for command in &output.viewport_output[&egui::ViewportId::ROOT].commands {
                match command {
                    egui::ViewportCommand::MinInnerSize(size) => {
                        self.minimum = *size * command_scale
                    }
                    egui::ViewportCommand::MaxInnerSize(size) => {
                        self.maximum = *size * command_scale
                    }
                    egui::ViewportCommand::InnerSize(size) => {
                        self.inner_pixels = (*size * command_scale)
                            .max(self.minimum)
                            .min(self.maximum)
                            .round();
                    }
                    egui::ViewportCommand::OuterPosition(position) => {
                        self.outer_position = (*position * command_scale).round()
                    }
                    _ => {}
                }
            }
            output
        }
    }

    fn fullscreen_test_window(size: egui::Vec2, native_scale: f32) -> DesktopWindow {
        DesktopWindow {
            area: ScreenArea {
                work_rect: egui::Rect::from_min_size(
                    egui::pos2(-size.x, 24.0 * native_scale),
                    size - egui::vec2(0.0, 80.0 * native_scale),
                ),
                decoration: (egui::vec2(16.0, 39.0) * native_scale).round(),
                can_position: true,
            },
            native_scale,
            inner_pixels: egui::vec2(960.0, 640.0) * native_scale,
            outer_position: egui::pos2(-size.x, 0.0),
            minimum: egui::Vec2::ZERO,
            maximum: egui::Vec2::INFINITY,
            fullscreen: false,
        }
    }

    fn finish_fullscreen_animation(sender: &mut Sender) {
        let request = sender.fullscreen_request.as_mut().unwrap();
        request.requested_at = Instant::now() - Duration::from_secs(2);
        request.stable_since = Instant::now() - Duration::from_millis(500);
    }

    fn key_event(key: egui::Key, pressed: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn fullscreen_waits_for_native_geometry_after_an_early_state_change() {
        let start = Instant::now();
        let windowed = FullscreenGeometry {
            size: egui::vec2(1136.0, 793.0),
            position: Some(egui::pos2(8.0, 120.0)),
            native_scale: 1.0,
        };
        let fullscreen = FullscreenGeometry {
            size: egui::vec2(1920.0, 1080.0),
            position: Some(egui::Pos2::ZERO),
            ..windowed
        };
        let mut request = FullscreenRequest::new(true, start);
        assert!(!request.ready(false, windowed, start));
        assert!(!request.ready(true, windowed, start + Duration::from_millis(100)));
        assert!(!request.ready(true, fullscreen, start + Duration::from_millis(400)));
        assert!(!request.ready(true, fullscreen, start + Duration::from_millis(600)));
        assert!(request.ready(true, fullscreen, start + Duration::from_secs(2)));
        let changed_dpi = FullscreenGeometry {
            native_scale: 2.0,
            ..fullscreen
        };
        assert!(!request.ready(true, changed_dpi, start + Duration::from_millis(2100)));
    }

    #[test]
    fn fullscreen_increases_payload_and_keeps_exact_pixels_in_both_themes() {
        for (size, native_scale) in [
            (egui::vec2(1366.0, 768.0), 1.0),
            (egui::vec2(1920.0, 1080.0), 1.0),
            (egui::vec2(1920.0, 1080.0), 1.25),
            (egui::vec2(1920.0, 1080.0), 1.5),
            (egui::vec2(1920.0, 1080.0), 2.0),
            (egui::vec2(3840.0, 2160.0), 1.0),
            (egui::vec2(3840.0, 2160.0), 1.5),
            (egui::vec2(3840.0, 2160.0), 2.0),
            (egui::vec2(1080.0, 1920.0), 1.0),
            (egui::vec2(3440.0, 1440.0), 1.0),
            (egui::vec2(920.0, 1080.0), 1.0),
        ] {
            for theme in [egui::Theme::Light, egui::Theme::Dark] {
                let ctx = egui::Context::default();
                ctx.options_mut(|options| options.max_passes = 1.try_into().unwrap());
                install_cjk_fallback(&ctx);
                ctx.input_mut(|input| input.max_texture_side = 1024);
                let config = Config::default();
                let mut sender = Sender::new(&ctx, config.clone(), config.grid().unwrap(), None);
                sender.theme = theme;
                ctx.set_theme(theme);
                let mut window = fullscreen_test_window(size, native_scale);
                for _ in 0..4 {
                    let _ = window.render(&ctx, &mut sender);
                }
                let windowed_capacity = sender.grid.payload_capacity();
                window.fullscreen = true;
                window.inner_pixels = size;
                window.outer_position = egui::pos2(-size.x, 0.0);
                let _ = window.render(&ctx, &mut sender);
                finish_fullscreen_animation(&mut sender);
                for _ in 0..4 {
                    let _ = window.render(&ctx, &mut sender);
                }
                let output = window.render(&ctx, &mut sender);
                let grid = painted_grid_rect(&output, &sender.texture).unwrap();
                let screen =
                    egui::Rect::from_min_size(egui::Pos2::ZERO, size / output.pixels_per_point);
                assert!(screen.contains_rect(grid));
                let scale = output.pixels_per_point;
                assert!(
                    (screen.width() - grid.width()) * scale < 8.01,
                    "unused horizontal pixels: {} at {size:?}/{native_scale}",
                    (screen.width() - grid.width()) * scale
                );
                assert!(
                    (screen.bottom() - grid.bottom()) * scale < 8.01,
                    "unused bottom pixels: {} at {size:?}/{native_scale}",
                    (screen.bottom() - grid.bottom()) * scale
                );
                let choose = text_bounds(&output, "选择文件");
                let send = text_bounds(&output, "发送文件");
                let watermark = text_bounds(&output, "水印");
                let stop = text_bounds(&output, "停止");
                let reset = text_bounds(&output, "重置");
                assert!(choose.right() < screen.width() / 3.0 + 16.0);
                assert!(choose.right() < send.left() && send.right() < watermark.left());
                assert!(watermark.right() < stop.left() && stop.right() < reset.left());
                assert!((send.center().y - choose.center().y).abs() < 1.0);
                assert!((stop.center().y - send.center().y).abs() < 1.0);
                assert!(screen.contains_rect(text_bounds(&output, "退出全屏")));
                assert!(sender.grid.payload_capacity() > windowed_capacity);
                assert_eq!(sender.grid.cell_size, 2);
                assert_eq!(sender.display_cell_size, 2);
                assert!(
                    (grid.width() * output.pixels_per_point - sender.grid.width as f32).abs()
                        < 0.01
                );
                assert!(
                    (grid.height() * output.pixels_per_point - sender.grid.height as f32).abs()
                        < 0.01
                );
                assert!(
                    sender.grid.width.is_multiple_of(8) && sender.grid.height.is_multiple_of(8)
                );
                assert!(
                    output.viewport_output[&egui::ViewportId::ROOT]
                        .commands
                        .is_empty()
                );
            }
        }
    }

    #[test]
    fn fullscreen_switch_pauses_recalibrates_and_restores_the_window() {
        let ctx = egui::Context::default();
        ctx.options_mut(|options| options.max_passes = 1.try_into().unwrap());
        let config = Config::default();
        let mut sender = Sender::new(&ctx, config.clone(), config.grid().unwrap(), None);
        let size = egui::vec2(1920.0, 1080.0);
        let mut window = fullscreen_test_window(size, 1.0);
        for _ in 0..4 {
            let _ = window.render(&ctx, &mut sender);
        }
        let original_size = window.inner_pixels;
        let original_position = window.outer_position;
        let original_capacity = sender.grid.payload_capacity();
        sender.transfer = Some(Prepared {
            source: PathBuf::from("large-file.bin"),
            label: "large-file.bin".into(),
            transfer: Transfer::new(
                TransferKind::File,
                "large-file.bin".into(),
                vec![19; 500_000],
                original_capacity,
            )
            .unwrap(),
        });
        let original_frame_count = sender.transfer.as_ref().unwrap().transfer.total_frames();
        sender.start(false).unwrap();
        sender.session.as_mut().unwrap().stage = SessionStage::Data(1);
        let output =
            window.render_with_events(&ctx, &mut sender, vec![key_event(egui::Key::F11, true)]);
        let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
        assert!(matches!(commands.as_slice(), [
            egui::ViewportCommand::MinInnerSize(size),
            egui::ViewportCommand::MaxInnerSize(maximum),
            egui::ViewportCommand::Fullscreen(true),
        ] if *size == egui::Vec2::ZERO && *maximum == egui::Vec2::INFINITY));
        assert!(painted_grid_rect(&output, &sender.texture).is_none());
        assert_eq!(
            sender.session.as_ref().unwrap().stage,
            SessionStage::Data(1)
        );
        let _ =
            window.render_with_events(&ctx, &mut sender, vec![key_event(egui::Key::F11, false)]);
        assert!(sender.fullscreen_request.is_some());
        assert_eq!(window.inner_pixels, original_size);
        window.fullscreen = true;
        window.inner_pixels = size;
        window.outer_position = egui::pos2(-size.x, 0.0);
        let _ = window.render(&ctx, &mut sender);
        assert!(sender.fullscreen_request.is_some());
        finish_fullscreen_animation(&mut sender);
        for _ in 0..4 {
            let _ = window.render(&ctx, &mut sender);
        }
        assert_eq!(
            sender.session.as_ref().unwrap().stage,
            SessionStage::Calibration
        );
        assert!(sender.transfer.as_ref().unwrap().transfer.total_frames() < original_frame_count);
        assert_eq!(
            sender.transfer.as_ref().unwrap().transfer.byte_len(),
            500_000
        );
        let output =
            window.render_with_events(&ctx, &mut sender, vec![key_event(egui::Key::Escape, true)]);
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Fullscreen(false)))
        );
        window.fullscreen = false;
        let _ = window.render(&ctx, &mut sender);
        finish_fullscreen_animation(&mut sender);
        for _ in 0..4 {
            let _ = window.render(&ctx, &mut sender);
        }
        assert_eq!(window.inner_pixels, original_size);
        assert_eq!(window.outer_position, original_position);
        assert_eq!(sender.grid.payload_capacity(), original_capacity);
        assert_eq!(
            sender.session.as_ref().unwrap().stage,
            SessionStage::Calibration
        );
        assert_eq!(
            sender.transfer.as_ref().unwrap().transfer.byte_len(),
            500_000
        );
    }

    #[test]
    fn fullscreen_escape_closes_the_file_picker_before_exiting_and_obeys_texture_limits() {
        let ctx = egui::Context::default();
        ctx.options_mut(|options| options.max_passes = 1.try_into().unwrap());
        ctx.input_mut(|input| input.max_texture_side = 1024);
        let config = Config::default();
        let mut sender = Sender::new(&ctx, config.clone(), config.grid().unwrap(), None);
        let mut window = fullscreen_test_window(egui::vec2(3840.0, 2160.0), 2.0);
        window.fullscreen = true;
        window.inner_pixels = egui::vec2(3840.0, 2160.0);
        let _ = window.render(&ctx, &mut sender);
        finish_fullscreen_animation(&mut sender);
        for _ in 0..4 {
            let _ = window.render(&ctx, &mut sender);
        }
        assert!(sender.grid.width > 1024 && sender.grid.height > 1024);
        let bytes: Vec<u8> = (0..=255).collect();
        assert_eq!(
            sender
                .grid
                .decode(&sender.grid.encode(&bytes).unwrap())
                .unwrap(),
            bytes
        );
        let directory = tempfile::tempdir().unwrap();
        sender.file_picker = Some(FilePicker::new(
            PickerMode::File,
            directory.path().to_path_buf(),
        ));
        let _ = window.render(&ctx, &mut sender);
        let output =
            window.render_with_events(&ctx, &mut sender, vec![key_event(egui::Key::Escape, true)]);
        assert!(sender.file_picker.is_none());
        assert!(sender.fullscreen);
        assert!(sender.fullscreen_request.is_none());
        assert!(
            !output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Fullscreen(false)))
        );
    }

    #[test]
    fn adaptive_native_window_keeps_all_controls_and_grid_inside_the_work_area() {
        for (origin, size, native_scale) in [
            ([0.0, 0.0], [1920.0, 1040.0], 1.0),
            ([0.0, 0.0], [1920.0, 1030.0], 1.25),
            ([0.0, 0.0], [1920.0, 1020.0], 1.5),
            ([0.0, 0.0], [1920.0, 1000.0], 2.0),
            ([0.0, 0.0], [1920.0, 960.0], 3.0),
            ([0.0, 40.0], [1366.0, 728.0], 1.0),
            ([60.0, 0.0], [1306.0, 768.0], 1.25),
            ([0.0, 0.0], [1366.0, 708.0], 1.5),
            ([-1080.0, -100.0], [1080.0, 1880.0], 1.0),
            ([-2560.0, 0.0], [2560.0, 1400.0], 1.0),
        ] {
            for theme in [egui::Theme::Light, egui::Theme::Dark] {
                let ctx = egui::Context::default();
                ctx.options_mut(|options| options.max_passes = 1.try_into().unwrap());
                install_cjk_fallback(&ctx);
                let config = Config::default();
                let mut sender = Sender::new(&ctx, config.clone(), config.grid().unwrap(), None);
                sender.theme = theme;
                ctx.set_theme(theme);
                let mut window = DesktopWindow {
                    area: ScreenArea {
                        work_rect: egui::Rect::from_min_size(origin.into(), size.into()),
                        decoration: (egui::vec2(16.0, 39.0) * native_scale).round(),
                        can_position: true,
                    },
                    native_scale,
                    inner_pixels: egui::vec2(960.0, 640.0) * native_scale,
                    outer_position: origin.into(),
                    minimum: egui::vec2(640.0, 360.0) * native_scale,
                    maximum: egui::Vec2::INFINITY,
                    fullscreen: false,
                };
                for _ in 0..4 {
                    let _ = window.render(&ctx, &mut sender);
                }
                let output = window.render(&ctx, &mut sender);
                let grid = painted_grid_rect(&output, &sender.texture).unwrap_or_else(|| {
                    panic!("missing grid at {size:?}, {native_scale}, {theme:?}")
                });
                let screen = egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    window.inner_pixels / output.pixels_per_point,
                );
                assert!(
                    // egui rounds panel bounds to fractions of a point. Check
                    // actual text and protocol pixels without this tolerance.
                    screen
                        .expand(0.5 / output.pixels_per_point)
                        .contains_rect(ctx.used_rect()),
                    "controls {:?} exceeded viewport {screen:?} by ({:.6}, {:.6}) at {size:?}, {native_scale}, {theme:?}, ppp {}",
                    ctx.used_rect(),
                    ctx.used_rect().max.x - screen.max.x,
                    ctx.used_rect().max.y - screen.max.y,
                    output.pixels_per_point
                );
                assert!(screen.contains_rect(grid));
                let physical_grid = sender.last_grid_rect.unwrap();
                assert!(window.area.work_rect.contains_rect(physical_grid));
                assert!(
                    window
                        .area
                        .work_rect
                        .contains_rect(egui::Rect::from_min_size(
                            window.outer_position,
                            window.inner_pixels + window.area.decoration
                        ))
                );
                assert_eq!(
                    physical_grid.size(),
                    egui::vec2(sender.grid.width as f32, sender.grid.height as f32)
                );
                assert!(
                    (grid.width() * output.pixels_per_point - physical_grid.width()).abs() < 0.01
                );
                for label in [
                    "设置",
                    "选择文件",
                    "发送文件",
                    "停止",
                    "重置",
                    "水印",
                    "补发",
                ] {
                    assert!(
                        screen.contains_rect(text_bounds(&output, label)),
                        "clipped control {label}"
                    );
                }
                assert!(
                    output.viewport_output[&egui::ViewportId::ROOT]
                        .commands
                        .is_empty(),
                    "stable layout must not continually resize the window"
                );

                // Moving to a taller screen must lift the old maximum before
                // requesting the larger four-pixel grid's window.
                let previous_capacity = sender.grid.payload_capacity();
                window.native_scale = 1.0;
                window.area.work_rect =
                    egui::Rect::from_min_size(egui::pos2(-2560.0, 0.0), egui::vec2(2560.0, 1400.0));
                window.area.decoration = egui::vec2(16.0, 39.0);
                for _ in 0..4 {
                    let _ = window.render(&ctx, &mut sender);
                }
                let output = window.render(&ctx, &mut sender);
                assert_eq!(sender.display_cell_size, 2);
                assert!(sender.grid.payload_capacity() >= previous_capacity);
                assert!(painted_grid_rect(&output, &sender.texture).is_some());
            }
        }
    }

    #[test]
    fn moving_the_board_recalibrates_and_restarts_all_frames_including_idle_resends() {
        let ctx = egui::Context::default();
        let mut sender = prepared_sender();
        let size = egui::vec2(1500.0, 900.0);
        let paint = |sender: &mut Sender, position: egui::Pos2| {
            let mut input = headless_input(1.0, size);
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .inner_rect = Some(egui::Rect::from_min_size(position, size));
            let _ = ctx.run(input, |ctx| sender.draw_grid(ctx));
        };
        paint(&mut sender, egui::Pos2::ZERO);
        sender.start(false).unwrap();
        sender.session.as_mut().unwrap().stage = SessionStage::Data(1);
        sender.session.as_mut().unwrap().displayed = 1;
        paint(&mut sender, egui::pos2(25.0, 10.0));
        assert_eq!(
            sender.session.as_ref().unwrap().stage,
            SessionStage::Calibration
        );
        assert_eq!(sender.session.as_ref().unwrap().displayed, 0);
        let start = sender.session.as_ref().unwrap().started;
        // Floating point roundtrips within the same physical pixel are stable.
        paint(&mut sender, egui::pos2(25.00001, 10.00001));
        assert_eq!(sender.session.as_ref().unwrap().started, start);
        sender.stop();
        paint(&mut sender, egui::pos2(50.0, 20.0));
        assert!(sender.requires_full_transfer);
        sender.resend = "1".into();
        sender.start(true).unwrap();
        assert_eq!(
            sender.session.as_ref().unwrap().stage,
            SessionStage::Calibration
        );
        assert!(matches!(
            sender.session.as_ref().unwrap().plan,
            FramePlan::All { next: 1, .. }
        ));
        assert!(!sender.requires_full_transfer);
    }

    #[test]
    fn full_window_keeps_an_exact_grid_visible_with_long_paths_in_both_themes() {
        let mut cases = vec![(Config::default(), 2.0, 752.0)];
        for scale in [1.0, 2.0] {
            for width in [640.0, 720.0] {
                cases.push((
                    Config {
                        width: ((width - 32.0) * scale) as u32,
                        height: (400.0 * scale) as u32,
                        ..Config::default()
                    },
                    scale,
                    width,
                ));
            }
        }
        for (config, scale, width) in cases {
            for theme in [egui::Theme::Light, egui::Theme::Dark] {
                let ctx = egui::Context::default();
                ctx.options_mut(|options| options.max_passes = 1.try_into().unwrap());
                install_cjk_fallback(&ctx);
                let mut sender = Sender::new(&ctx, config.clone(), config.grid().unwrap(), None);
                sender.theme = theme;
                ctx.set_theme(theme);
                let size = egui::vec2(width, config.height as f32 / scale + WINDOW_CHROME_HEIGHT);
                let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                let mut remaining = egui::Rect::NOTHING;
                let mut used = egui::Rect::NOTHING;
                let output = ctx.run(headless_input(scale, size), |ctx| {
                    sender.fit_window_to_grid(ctx);
                    sender.controls(ctx);
                    remaining = ctx.available_rect();
                    sender.draw_grid(ctx);
                    used = ctx.used_rect();
                });
                let baseline = painted_grid_rect(&output, &sender.texture).unwrap_or_else(|| {
                    panic!("grid did not fit viewport {width} at scale {scale} in {theme:?}; remaining area {remaining:?}")
                });
                assert!(
                    screen.contains_rect(used),
                    "used area {used:?} exceeded screen {screen:?} at {width}, {scale}, {theme:?}"
                );
                assert!(screen.contains_rect(baseline));
                assert!((baseline.width() * scale - config.width as f32).abs() < 0.001);
                assert!((baseline.height() * scale - config.height as f32).abs() < 0.001);
                assert!((baseline.min.x * scale - (baseline.min.x * scale).round()).abs() < 0.001);
                assert!((baseline.min.y * scale - (baseline.min.y * scale).round()).abs() < 0.001);
                let file = text_bounds(&output, "待发送文件");
                let choose = text_bounds(&output, "选择文件");
                let send = text_bounds(&output, "发送文件");
                let stop = text_bounds(&output, "停止");
                let reset = text_bounds(&output, "重置");
                let watermark = text_bounds(&output, "水印");
                let speed = text_bounds(&output, "当前速度：0 B/s");
                let status = text_bounds(&output, &sender.status);
                let left_edge = text_bounds(&output, "补发帧号").left();
                let right_edge = text_bounds(&output, &sender.progress_label).right();
                assert!((status.left() - left_edge).abs() < 1.0);
                assert!((speed.right() - right_edge).abs() < 1.0);
                assert!(
                    screen.contains_rect(speed),
                    "speed {speed:?} exceeded screen {screen:?} at scale {scale} in {theme:?}"
                );
                assert!(file.left() < width / 3.0 && choose.right() < width / 3.0 + 16.0);
                assert!(send.left() > choose.right());
                assert!((send.center().y - choose.center().y).abs() < 1.0);
                assert!(send.right() < watermark.left() && watermark.right() < stop.left());
                assert!(stop.right() < reset.left());
                assert!((watermark.center().y - send.center().y).abs() < 1.0);
                assert!((stop.center().y - send.center().y).abs() < 1.0);
                assert!(watermark.left() > width / 3.0);

                sender.selected_file = Some(PathBuf::from(format!(
                    "/{}/{}\n下一行.docx",
                    "很长的目录/".repeat(60),
                    "很长的文件名称".repeat(30),
                )));
                sender.status = format!("发送中\r\n{}", "文件路径及状态说明".repeat(100));
                sender.progress_label = "999999 / 999999 · 帧 999999 / 999999".into();
                sender.session = Some(session());
                let output = ctx.run(headless_input(scale, size), |ctx| {
                    sender.controls(ctx);
                    sender.draw_grid(ctx);
                    used = ctx.used_rect();
                });
                let active = painted_grid_rect(&output, &sender.texture)
                    .expect("long paths and progress must preserve the complete grid");
                assert_eq!(active, baseline, "changing status must not move the grid");
                assert!(screen.contains_rect(used));
                let active_status = text_bounds(&output, &single_line(&sender.status));
                let active_speed = text_bounds(&output, "当前速度：0 B/s");
                assert!((active_status.left() - left_edge).abs() < 1.0);
                assert!((active_speed.right() - right_edge).abs() < 1.0);
                assert!(active_status.right() < active_speed.left());
            }
        }
    }

    #[test]
    fn hovering_the_transmission_grid_never_paints_a_tooltip_over_it() {
        let config = Config::default();
        let ctx = egui::Context::default();
        ctx.options_mut(|options| options.max_passes = 1.try_into().unwrap());
        install_cjk_fallback(&ctx);
        let mut sender = Sender::new(&ctx, config.clone(), config.grid().unwrap(), None);
        sender.session = Some(session());
        let scale = 2.0;
        let size = egui::vec2(752.0, config.height as f32 / scale + WINDOW_CHROME_HEIGHT);
        let output = ctx.run(headless_input(scale, size), |ctx| {
            sender.controls(ctx);
            sender.draw_grid(ctx);
        });
        let grid = painted_grid_rect(&output, &sender.texture).expect("default grid must fit");
        for time in [0.0, 0.7, 1.4] {
            let mut input = headless_input(scale, size);
            input.time = Some(time);
            input.events.push(egui::Event::PointerMoved(grid.center()));
            // The test advances egui's tooltip clock, not the real transmission timer.
            sender.session.as_mut().unwrap().pause();
            let output = ctx.run(input, |ctx| {
                sender.controls(ctx);
                sender.draw_grid(ctx);
            });
            let grid_index = output.shapes.iter().position(|clipped| {
                matches!(&clipped.shape, egui::Shape::Mesh(mesh) if sender.texture.contains_texture(mesh.texture_id))
            }).unwrap();
            for clipped in &output.shapes[grid_index + 1..] {
                let visible = clipped
                    .shape
                    .visual_bounding_rect()
                    .intersect(clipped.clip_rect);
                assert!(
                    visible.intersect(grid).area() <= 0.0,
                    "a later shape obscured the protocol image: {visible:?}"
                );
            }
        }
    }

    #[test]
    fn the_default_grid_pauses_only_when_the_minimum_window_cannot_fit_it() {
        for (scale, should_fit) in [(1.0, false), (2.0, false), (4.0, true)] {
            let config = Config::default();
            let ctx = egui::Context::default();
            ctx.options_mut(|options| options.max_passes = 1.try_into().unwrap());
            install_cjk_fallback(&ctx);
            let mut sender = Sender::new(&ctx, config.clone(), config.grid().unwrap(), None);
            sender.transfer = prepared_sender().transfer;
            let mut active = session();
            active.visible_since = Some(Instant::now() - Duration::from_secs(3));
            sender.session = Some(active);
            let size = egui::vec2(640.0, config.height as f32 / scale + WINDOW_CHROME_HEIGHT);
            let mut used = egui::Rect::NOTHING;
            // Let the dynamic panel cache its height before checking painted controls.
            let _ = ctx.run(headless_input(scale, size), |ctx| {
                sender.controls(ctx);
                sender.draw_grid(ctx);
            });
            let output = ctx.run(headless_input(scale, size), |ctx| {
                sender.controls(ctx);
                sender.draw_grid(ctx);
                used = ctx.used_rect();
            });
            let grid = painted_grid_rect(&output, &sender.texture);
            assert_eq!(grid.is_some(), should_fit);
            assert_eq!(
                sender.session.as_ref().unwrap().visible_since.is_some(),
                should_fit
            );
            if let Some(grid) = grid {
                assert!((grid.width() * scale - config.width as f32).abs() < 0.001);
                assert!((grid.height() * scale - config.height as f32).abs() < 0.001);
                assert!(egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(grid));
            }
            assert!(
                egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(used),
                "used {used:?} exceeded screen {size:?} scale {scale}"
            );
            fn collect_text<'a>(shape: &'a egui::Shape, texts: &mut Vec<&'a str>) {
                match shape {
                    egui::Shape::Text(text) => texts.push(&text.galley.job.text),
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            collect_text(shape, texts);
                        }
                    }
                    _ => {}
                }
            }
            let mut texts = Vec::new();
            for clipped in &output.shapes {
                collect_text(&clipped.shape, &mut texts);
            }
            for label in [
                "选择文件",
                "发送文件",
                "停止",
                "重置",
                "水印",
                "补发",
                "设置",
                "当前速度：0 B/s",
            ] {
                assert!(
                    texts.contains(&label),
                    "missing Chinese control: {label}; {texts:?}"
                );
            }
            assert!(
                !texts.contains(&"发送文本"),
                "text transmission must not appear"
            );
        }
    }
}
