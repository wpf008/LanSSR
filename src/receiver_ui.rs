//! Persistent desktop receiver. Capture and decoding live on a cancellable worker.
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    config::Config,
    desktop_paths,
    receiver::{self, ReceiverOptions, ReceiverUpdate},
    transfer_speed::{SpeedMeter, format_speed},
    ui_style::{self, Palette, action_button, primary_button},
    window_layout::{self, FractionalWindowPlan, ScreenArea, WindowRole},
};
use anyhow::{Context, Result, anyhow};
use eframe::egui;

#[cfg(target_os = "linux")]
use crate::file_picker::{FilePicker, PickerMode, PickerResult};

const WINDOW_SIZE: egui::Vec2 = egui::vec2(860.0, 730.0);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Main,
    Settings,
}

pub fn run(config: Config, options: ReceiverOptions) -> Result<()> {
    config.validate()?;
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("LanReceiver")
        .with_icon(
            eframe::icon_data::from_png_bytes(include_bytes!("../assets/icons/lanreceiver.png"))
                .context("无法加载 LanReceiver 图标")?,
        )
        .with_inner_size(WINDOW_SIZE)
        .with_resizable(false)
        .with_maximize_button(false);
    if let Some((area, scale)) = window_layout::initial_screen_area() {
        let plan = window_layout::fractional_window_plan(area, scale, WindowRole::Receiver);
        viewport = viewport.with_inner_size(plan.inner_pixels / scale);
        if let Some(position) = plan.outer_position {
            viewport = viewport.with_position(position / scale);
        }
    }
    eframe::run_native(
        "LanReceiver",
        eframe::NativeOptions {
            viewport,
            persist_window: false,
            ..Default::default()
        },
        Box::new(move |cc| {
            crate::platform_startup::initialize_graphics_limits(cc);
            crate::sender::install_cjk_fallback(&cc.egui_ctx);
            Ok(Box::new(ReceiverApp::new(&cc.egui_ctx, config, options)))
        }),
    )
    .map_err(|error| anyhow!("无法打开 LanReceiver：{error}"))
}

#[derive(Debug)]
struct HistoryEntry {
    path: PathBuf,
    bytes: u64,
    frames: u32,
    seconds: f64,
}

struct Worker {
    updates: Receiver<ReceiverUpdate>,
    cancelled: Arc<AtomicBool>,
}

struct ReceiverApp {
    config: Config,
    options: ReceiverOptions,
    worker: Option<Worker>,
    stopping: bool,
    theme: egui::Theme,
    page: Page,
    pending_reveal: Option<PathBuf>,
    connection: String,
    location: String,
    status: String,
    error: Option<String>,
    filename: Option<String>,
    received: usize,
    total: u32,
    received_bytes: u64,
    speed: SpeedMeter,
    receiving: bool,
    show_progress: bool,
    missing: String,
    history: Vec<HistoryEntry>,
    last_window_plan: Option<FractionalWindowPlan>,
    #[cfg(target_os = "linux")]
    file_picker: Option<FilePicker>,
}

impl ReceiverApp {
    fn new(ctx: &egui::Context, config: Config, options: ReceiverOptions) -> Self {
        let theme = ctx.system_theme().unwrap_or(egui::Theme::Light);
        ctx.set_theme(theme);
        ui_style::install_ui_styles(ctx);
        Self {
            config,
            options,
            worker: None,
            stopping: false,
            theme,
            page: Page::Main,
            pending_reveal: None,
            connection: "尚未开始接收".into(),
            location: "点击“开始接收”，随后在 LanSender 发送文件".into(),
            status: "准备就绪 · 文件接收完成后会继续等待下一次传输".into(),
            error: None,
            filename: None,
            received: 0,
            total: 0,
            received_bytes: 0,
            speed: SpeedMeter::default(),
            receiving: false,
            show_progress: false,
            missing: String::new(),
            history: Vec::new(),
            last_window_plan: None,
            #[cfg(target_os = "linux")]
            file_picker: None,
        }
    }

    fn choose_output_directory(&mut self) {
        if self.worker.is_some() {
            return;
        }
        #[cfg(target_os = "linux")]
        {
            self.file_picker = Some(FilePicker::new(
                PickerMode::Directory,
                self.options.output_dir.clone(),
            ));
        }
        #[cfg(not(target_os = "linux"))]
        if let Some(path) = rfd::FileDialog::new()
            .set_title("选择接收文件的保存目录")
            .set_directory(&self.options.output_dir)
            .pick_folder()
        {
            self.options.output_dir = path;
        }
    }

    #[cfg(target_os = "linux")]
    fn show_file_picker(&mut self, ctx: &egui::Context) {
        if let Some(result) = self
            .file_picker
            .as_mut()
            .and_then(|picker| picker.show(ctx))
        {
            self.apply_file_picker_result(result);
        }
    }

    #[cfg(target_os = "linux")]
    fn apply_file_picker_result(&mut self, result: PickerResult) {
        self.file_picker = None;
        if self.worker.is_none()
            && let PickerResult::Selected(path) = result
        {
            self.options.output_dir = path;
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        if self.worker.is_some() {
            return;
        }
        if let Err(error) = std::fs::create_dir_all(&self.options.output_dir) {
            self.error = Some(format!("无法创建保存目录：{error}"));
            return;
        }
        match std::fs::canonicalize(&self.options.output_dir) {
            Ok(path) => self.options.output_dir = path,
            Err(error) => {
                self.error = Some(format!("无法读取保存目录：{error}"));
                return;
            }
        }
        self.error = None;
        self.filename = None;
        self.received = 0;
        self.total = 0;
        self.clear_speed();
        self.show_progress = false;
        self.missing.clear();
        self.connection = "正在启动接收…".into();
        self.status = "请让 LanSender 的完整传输画面保持可见".into();
        let (updates, incoming) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        self.worker = Some(Worker {
            updates: incoming,
            cancelled: cancelled.clone(),
        });
        self.stopping = false;
        let config = self.config.clone();
        let options = self.options.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let _ = receiver::run_continuous(config, options, updates, cancelled);
            ctx.request_repaint();
        });
    }

    fn stop(&mut self) {
        if let Some(worker) = &self.worker {
            worker.cancelled.store(true, Ordering::Relaxed);
            self.stopping = true;
            self.connection = "正在停止…".into();
            self.clear_speed();
            self.show_progress = false;
        }
    }

    fn set_water_mask(&mut self, enabled: bool) {
        if self.worker.is_some() || self.config.water_mask == enabled {
            return;
        }
        let config = Config {
            water_mask: enabled,
            ..self.config.clone()
        };
        let region = self.options.region.map(|mut location| {
            location.grid = crate::grid::Grid::new(
                location.grid.width,
                location.grid.height,
                location.grid.cell_size,
                enabled,
            )?;
            Ok::<_, anyhow::Error>(location)
        });
        match config.validate().and_then(|_| region.transpose()) {
            Ok(region) => {
                self.config = config;
                self.options.region = region;
                self.error = None;
                self.filename = None;
                self.received = 0;
                self.total = 0;
                self.clear_speed();
                self.show_progress = false;
                self.missing.clear();
                self.connection = "尚未开始接收".into();
                self.location = "点击“开始接收”，随后在 LanSender 发送文件".into();
                self.status = "准备就绪 · 请保持两端水印模式一致".into();
            }
            Err(error) => self.error = Some(format!("无法更改水印模式：{error}")),
        }
    }

    fn apply(&mut self, event: ReceiverUpdate) {
        match event {
            ReceiverUpdate::Searching => {
                self.clear_speed();
                self.show_progress = false;
                self.connection = "正在寻找发送端".into();
                self.location = "扫描显示器中的 LanSender 棋盘格".into();
                self.status = "保持发送端画面完整可见，定位后即可发送".into();
            }
            ReceiverUpdate::Located {
                monitor_index,
                monitor_name,
                location,
            } => {
                self.connection = "已定位发送端".into();
                self.location = format!(
                    "显示器 {} · {}\n区域 ({}, {}) · {} × {} 像素 · {} 像素格",
                    monitor_index + 1,
                    monitor_name,
                    location.x,
                    location.y,
                    location.grid.width,
                    location.grid.height,
                    location.grid.cell_size
                );
            }
            ReceiverUpdate::Waiting => {
                self.connection = "已定位 · 等待文件".into();
                self.status = "准备接收下一份文件".into();
                self.filename = None;
                self.received = 0;
                self.total = 0;
                self.clear_speed();
                self.show_progress = false;
                self.missing.clear();
            }
            ReceiverUpdate::Metadata {
                name,
                total_frames,
                started_at,
            } => {
                self.connection = "正在接收".into();
                self.filename = Some(name);
                self.received = 0;
                self.total = total_frames;
                self.received_bytes = 0;
                self.speed.reset(started_at);
                self.receiving = !self.stopping;
                self.show_progress = !self.stopping;
                self.missing.clear();
                self.error = None;
                self.status = "已收到文件信息，正在接收数据".into();
            }
            ReceiverUpdate::Progress {
                number,
                received,
                total,
                received_bytes,
                sampled_at,
            } => {
                self.received = received;
                self.total = total;
                if !self.stopping {
                    self.speed.record_bytes(
                        received_bytes.saturating_sub(self.received_bytes),
                        sampled_at,
                    );
                    self.receiving = true;
                    self.show_progress = true;
                }
                self.received_bytes = received_bytes;
                self.missing.clear();
                self.status = format!("已收到第 {number} 帧 · {received} / {total} 帧");
            }
            ReceiverUpdate::Missing { frames } => {
                self.missing = frames
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                self.status =
                    "部分帧尚未收到，请将以下帧号填入 LanSender 补发；补发后继续接收".into();
            }
            ReceiverUpdate::Completed {
                path,
                bytes,
                total_frames,
                elapsed_seconds,
            } => {
                self.received = total_frames as usize;
                self.total = total_frames;
                self.clear_speed();
                self.show_progress = !self.stopping && total_frames > 0;
                self.missing.clear();
                self.error = None;
                self.status = format!(
                    "接收完成 · {} · 用时 {elapsed_seconds:.2} 秒",
                    format_bytes(bytes)
                );
                self.connection = "接收完成 · 等待下一份文件".into();
                self.history.push(HistoryEntry {
                    path,
                    bytes,
                    frames: total_frames,
                    seconds: elapsed_seconds,
                });
            }
            ReceiverUpdate::AwaitingReset => {
                self.connection = "接收完成 · 等待下一份文件".into();
            }
            ReceiverUpdate::Error(message) => self.error = Some(message),
            ReceiverUpdate::Heartbeat => {}
            ReceiverUpdate::Stopped => {
                self.worker = None;
                self.stopping = false;
                self.clear_speed();
                self.show_progress = false;
                self.connection = "接收已停止".into();
                self.location = "再次点击“开始接收”可重新定位发送端".into();
                if self.error.is_none() {
                    self.status = "已停止接收，本次文件历史保留".into();
                }
            }
        }
    }

    fn clear_speed(&mut self) {
        self.speed = SpeedMeter::default();
        self.received_bytes = 0;
        self.receiving = false;
    }

    fn current_speed(&mut self, now: Instant) -> f64 {
        if self.receiving && !self.stopping {
            self.speed.bytes_per_second(now)
        } else {
            0.0
        }
    }

    fn poll(&mut self) {
        for _ in 0..1000 {
            let event = self.worker.as_ref().map(|worker| worker.updates.try_recv());
            match event {
                Some(Ok(event)) => self.apply(event),
                Some(Err(TryRecvError::Disconnected)) => {
                    self.apply(ReceiverUpdate::Stopped);
                    break;
                }
                _ => break,
            }
        }
    }

    fn show(&mut self, ctx: &egui::Context) {
        let colors = Palette::for_theme(self.theme);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(colors.background).inner_margin(16))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                match self.page {
                    Page::Main => self.main_page(ui, ctx, colors),
                    Page::Settings => self.settings_page(ui, ctx, colors),
                }
            });
    }

    fn fit_window(&mut self, ctx: &egui::Context, area: Option<ScreenArea>) {
        let Some(area) = area else { return };
        let scale = ctx.pixels_per_point();
        let native_scale = ctx
            .input(|input| input.viewport().native_pixels_per_point)
            .unwrap_or(scale);
        let plan = window_layout::fractional_window_plan(area, native_scale, WindowRole::Receiver);
        if self.last_window_plan == Some(plan) && (scale - plan.chrome_scale).abs() < 0.001 {
            return;
        }
        self.last_window_plan = Some(plan);
        ctx.set_zoom_factor(plan.chrome_scale / native_scale);
        // Commands use this frame's DPI; the new zoom takes effect next frame.
        // Clear the previous minimum before installing an exact new size.
        let size = plan.inner_pixels / scale;
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::Vec2::splat(1.0)));
        ctx.send_viewport_cmd(egui::ViewportCommand::MaxInnerSize(size));
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(size));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        if let Some(position) = plan.outer_position {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(position / scale));
        }
    }

    fn header(&mut self, ui: &mut egui::Ui, colors: Palette) {
        fixed_region(ui, 46.0, |ui| {
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 92.0).max(0.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 46.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.label(
                            egui::RichText::new(if self.page == Page::Main {
                                "LanReceiver"
                            } else {
                                "设置"
                            })
                            .size(23.0)
                            .strong(),
                        );
                        ui.label(
                            egui::RichText::new(if self.page == Page::Main {
                                "持续接收屏幕传输的文件"
                            } else {
                                "保存目录与界面外观"
                            })
                            .small()
                            .color(colors.muted),
                        );
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if self.page == Page::Main {
                        "设置"
                    } else {
                        "返回"
                    };
                    if ui.add(action_button(label, 84.0)).clicked() {
                        self.page = if self.page == Page::Main {
                            Page::Settings
                        } else {
                            Page::Main
                        };
                    }
                });
            });
        });
        ui.add_space(12.0);
    }

    fn main_page(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, colors: Palette) {
        self.header(ui, colors);
        fixed_region(ui, 32.0, |ui| {
            ui.horizontal(|ui| {
                let active = self.worker.is_some();
                if ui
                    .add_enabled(!active, primary_button("开始接收", 140.0))
                    .clicked()
                {
                    self.start(ctx);
                }
                let mut enabled = self.config.water_mask;
                let response = ui
                    .add_enabled_ui(!active, |ui| ui_style::watermark_switch(ui, &mut enabled))
                    .inner;
                if response.changed() {
                    self.set_water_mask(enabled);
                }
                response.on_hover_text(if active {
                    "停止接收后可更改水印；请与 LanSender 的水印设置保持一致"
                } else {
                    "默认开启；请与 LanSender 的水印设置保持一致"
                });
                if ui
                    .add_enabled(active && !self.stopping, action_button("停止接收", 110.0))
                    .clicked()
                {
                    self.stop();
                }
            });
        });
        ui.add_space(12.0);
        fixed_card(ui, 110.0, colors, |ui| {
            ui.label(egui::RichText::new("连接信息").strong());
            ui.add(
                egui::Label::new(
                    egui::RichText::new(single_line(&self.connection)).color(colors.accent),
                )
                .truncate(),
            );
            truncated(ui, &self.location, colors.text);
            truncated(ui, "屏幕图像连接 · 完成后自动等待下一份文件", colors.muted);
        });
        ui.add_space(10.0);
        fixed_card(ui, 164.0, colors, |ui| {
            ui.label(egui::RichText::new("传输信息").strong());
            let speed = format!(
                "当前速度：{}",
                format_speed(self.current_speed(Instant::now()))
            );
            ui.horizontal(|ui| {
                let speed_width = ui
                    .painter()
                    .layout_no_wrap(
                        speed.clone(),
                        egui::FontId::proportional(12.0),
                        colors.muted,
                    )
                    .size()
                    .x
                    .min(ui.available_width() * 0.5);
                let filename_width =
                    (ui.available_width() - speed_width - ui.spacing().item_spacing.x).max(0.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(filename_width, 17.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_size(egui::vec2(filename_width, 17.0));
                        let filename = self.filename.as_deref().unwrap_or("等待接收文件");
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(single_line(filename))
                                    .size(12.0)
                                    .color(colors.text),
                            )
                            .halign(egui::Align::Min)
                            .truncate(),
                        )
                        .on_hover_text(filename);
                    },
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(speed_width, 17.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.set_min_size(egui::vec2(speed_width, 17.0));
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&speed).size(12.0).color(colors.muted),
                            )
                            .halign(egui::Align::Max)
                            .truncate(),
                        )
                        .on_hover_text(format!(
                            "{speed}\n按最近 1 秒收到的有效文件数据统计；重复帧不计入速度"
                        ));
                    },
                );
            });
            let fraction = if self.total == 0 {
                0.0
            } else {
                self.received as f32 / self.total as f32
            };
            ui.add(
                egui::ProgressBar::new(fraction)
                    .fill(if self.show_progress {
                        colors.accent
                    } else {
                        egui::Color32::TRANSPARENT
                    })
                    .desired_width(ui.available_width())
                    .desired_height(20.0)
                    .text(format!("{} / {} 帧", self.received, self.total)),
            );
            truncated(ui, &self.status, colors.muted);
            if !self.missing.is_empty() {
                ui.horizontal(|ui| {
                    if ui.add(action_button("复制帧号", 100.0)).clicked() {
                        match arboard::Clipboard::new()
                            .and_then(|mut clipboard| clipboard.set_text(self.missing.clone()))
                        {
                            Ok(()) => {
                                self.status = "帧号已复制，可粘贴到 LanSender 的“补发帧号”".into()
                            }
                            Err(error) => self.error = Some(format!("无法复制帧号：{error}")),
                        }
                    }
                    truncated(ui, &format!("待补发：{}", self.missing), colors.muted);
                });
            }
            if let Some(error) = &self.error {
                truncated(ui, error, colors.error);
            }
        });
        ui.add_space(12.0);
        let height = ui.available_height();
        fixed_card(ui, height, colors, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("本次接收历史").strong());
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "{} 个文件 · {}",
                            self.history.len(),
                            format_bytes(self.history.iter().map(|entry| entry.bytes).sum())
                        ))
                        .small()
                        .color(colors.muted),
                    )
                    .truncate(),
                );
            });
            truncated(ui, "点击文件名定位文件", colors.muted);
            ui.add_space(4.0);
            if self.history.is_empty() {
                ui.label(egui::RichText::new("收到的文件会显示在这里").color(colors.muted));
            } else {
                egui::ScrollArea::vertical()
                    .id_salt("received-files")
                    .auto_shrink([false, false])
                    .max_height(ui.available_height())
                    .show_rows(ui, 66.0, self.history.len(), |ui, range| {
                        for index in range {
                            let entry = &self.history[self.history.len() - 1 - index];
                            fixed_region(ui, 66.0, |ui| {
                                ui.separator();
                                let name = entry
                                    .path
                                    .file_name()
                                    .unwrap_or(entry.path.as_os_str())
                                    .to_string_lossy();
                                let response = ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new(single_line(&name))
                                                .strong()
                                                .color(colors.accent),
                                        )
                                        .frame(false)
                                        .wrap_mode(egui::TextWrapMode::Truncate)
                                        .min_size(egui::vec2(0.0, 24.0)),
                                    )
                                    .on_hover_text(format!(
                                        "{}\n点击在文件管理器中定位",
                                        desktop_paths::display_path(&entry.path)
                                    ));
                                if response.clicked() {
                                    self.pending_reveal = Some(entry.path.clone());
                                }
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(format!(
                                                "{} · {} 帧 · {:.2} 秒",
                                                format_bytes(entry.bytes),
                                                entry.frames,
                                                entry.seconds
                                            ))
                                            .small()
                                            .color(colors.muted),
                                        )
                                        .truncate(),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(single_line(
                                                &desktop_paths::display_path(&entry.path),
                                            ))
                                            .small()
                                            .color(colors.muted),
                                        )
                                        .truncate(),
                                    );
                                });
                            });
                        }
                    });
            }
        });
    }

    fn settings_page(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, colors: Palette) {
        self.header(ui, colors);
        fixed_card(ui, 148.0, colors, |ui| {
            ui.label(egui::RichText::new("保存目录").strong());
            truncated(
                ui,
                &desktop_paths::display_path(&self.options.output_dir),
                colors.text,
            );
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.worker.is_none(), action_button("选择目录", 110.0))
                    .clicked()
                {
                    self.choose_output_directory();
                }
                if ui.add(action_button("打开目录", 110.0)).clicked()
                    && let Err(error) = desktop_paths::open_folder(&self.options.output_dir)
                {
                    self.error = Some(format!("无法打开目录：{error:#}"));
                }
                if self.worker.is_some()
                    && ui
                        .add_enabled(!self.stopping, action_button("停止接收", 110.0))
                        .clicked()
                {
                    self.stop();
                }
            });
            ui.label(
                egui::RichText::new(if self.worker.is_some() {
                    "停止接收后可更换保存目录"
                } else {
                    "默认保存到系统桌面，接收的文件将写入所选目录"
                })
                .small()
                .color(colors.muted),
            );
        });
        ui.add_space(12.0);
        fixed_card(ui, 102.0, colors, |ui| {
            ui.label(egui::RichText::new("界面外观").strong());
            ui_style::theme_selector(ui, &mut self.theme, ctx);
            ui.label(
                egui::RichText::new("启动时采用系统主题")
                    .small()
                    .color(colors.muted),
            );
        });
        ui.add_space(12.0);
        if let Some(error) = &self.error {
            truncated(ui, error, colors.error);
        }
    }
}

fn single_line(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

fn truncated(ui: &mut egui::Ui, value: &str, color: egui::Color32) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(single_line(value))
                .size(12.0)
                .color(color),
        )
        .truncate(),
    )
    .on_hover_text(value);
}

/// Reserve a stable rectangle before rendering content, so long labels cannot
/// move the history list or enlarge the surrounding window.
fn fixed_region(ui: &mut egui::Ui, height: f32, contents: impl FnOnce(&mut egui::Ui)) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height.max(0.0)),
        egui::Sense::hover(),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    child.spacing_mut().item_spacing.y = 5.0;
    contents(&mut child);
}

fn fixed_card(
    ui: &mut egui::Ui,
    height: f32,
    colors: Palette,
    contents: impl FnOnce(&mut egui::Ui),
) {
    fixed_region(ui, height, |ui| {
        let inner = ui.max_rect().shrink(11.0);
        ui.painter().add(ui_style::card(colors).paint(inner));
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(inner.intersect(ui.clip_rect()));
        contents(&mut child);
    });
}

impl eframe::App for ReceiverApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.poll();
        self.fit_window(ctx, window_layout::screen_area(ctx, frame));
        self.show(ctx);
        #[cfg(target_os = "linux")]
        self.show_file_picker(ctx);
        if let Some(path) = self.pending_reveal.take()
            && let Err(error) = desktop_paths::reveal_file(&path)
        {
            self.error = Some(format!("无法定位文件：{error:#}"));
        }
        if self.worker.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

impl Drop for ReceiverApp {
    fn drop(&mut self) {
        if let Some(worker) = &self.worker {
            worker.cancelled.store(true, Ordering::Relaxed);
        }
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} 字节")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(size: egui::Vec2) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        }
    }

    fn visible_text(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        fn collect(shape: &egui::Shape, clip: egui::Rect, result: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => {
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

    fn app() -> ReceiverApp {
        ReceiverApp::new(
            &egui::Context::default(),
            Config::default(),
            ReceiverOptions {
                output_dir: PathBuf::from("/test"),
                overwrite: false,
                monitor: None,
                region: None,
                poll_ms: 5,
                timeout_seconds: 0,
                max_bytes: 512 * 1024 * 1024,
            },
        )
    }
    #[test]
    fn successive_completions_keep_history_and_next_transfer_clears_progress() {
        let mut app = app();
        app.apply(ReceiverUpdate::Metadata {
            name: "first.bin".into(),
            total_frames: 2,
            started_at: Instant::now(),
        });
        app.apply(ReceiverUpdate::Progress {
            number: 1,
            received: 1,
            total: 2,
            received_bytes: 8,
            sampled_at: Instant::now(),
        });
        app.apply(ReceiverUpdate::Missing { frames: vec![2] });
        assert_eq!(app.missing, "2");
        app.apply(ReceiverUpdate::Completed {
            path: PathBuf::from("/test/first.bin"),
            bytes: 10,
            total_frames: 2,
            elapsed_seconds: 1.0,
        });
        app.apply(ReceiverUpdate::Waiting);
        assert_eq!(app.received, 0);
        assert!(app.missing.is_empty());
        assert_eq!(app.history.len(), 1);
        app.apply(ReceiverUpdate::Metadata {
            name: "second.bin".into(),
            total_frames: 1,
            started_at: Instant::now(),
        });
        app.apply(ReceiverUpdate::Completed {
            path: PathBuf::from("/test/second.bin"),
            bytes: 8,
            total_frames: 1,
            elapsed_seconds: 0.5,
        });
        app.apply(ReceiverUpdate::Stopped);
        assert_eq!(app.history.len(), 2);
        assert_eq!(app.history[0].bytes, 10);
        assert_eq!(app.history[1].path, PathBuf::from("/test/second.bin"));
    }
    #[test]
    fn current_speed_uses_unique_bytes_worker_times_and_clears_for_idle_states() {
        let mut app = app();
        let started = Instant::now() - Duration::from_secs(3);
        let (outgoing, incoming) = mpsc::channel();
        app.worker = Some(Worker {
            updates: incoming,
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        outgoing
            .send(ReceiverUpdate::Metadata {
                name: "speed.bin".into(),
                total_frames: 10,
                started_at: started,
            })
            .unwrap();
        for received in 1..=3 {
            outgoing
                .send(ReceiverUpdate::Progress {
                    number: received as u32,
                    received,
                    total: 10,
                    received_bytes: received as u64 * 1024,
                    sampled_at: started + Duration::from_millis(received as u64 * 250),
                })
                .unwrap();
        }
        // Polling an accumulated channel batch must preserve capture timing,
        // rather than report all three frames as an instantaneous burst.
        app.poll();
        assert!((app.current_speed(started + Duration::from_millis(750)) - 4096.0).abs() < 0.1);
        assert_eq!(app.current_speed(Instant::now()), 0.0);
        assert!(app.show_progress);
        for event in [
            ReceiverUpdate::Waiting,
            ReceiverUpdate::Searching,
            ReceiverUpdate::Stopped,
        ] {
            app.apply(ReceiverUpdate::Metadata {
                name: "speed.bin".into(),
                total_frames: 10,
                started_at: started,
            });
            app.apply(ReceiverUpdate::Progress {
                number: 1,
                received: 1,
                total: 10,
                received_bytes: 1024,
                sampled_at: started + Duration::from_millis(250),
            });
            assert!(app.current_speed(started + Duration::from_millis(250)) > 0.0);
            app.apply(event);
            assert_eq!(app.current_speed(started + Duration::from_millis(250)), 0.0);
            assert!(!app.show_progress);
        }
        app.apply(ReceiverUpdate::Metadata {
            name: "empty.bin".into(),
            total_frames: 0,
            started_at: started,
        });
        app.apply(ReceiverUpdate::Completed {
            path: PathBuf::from("/test/empty.bin"),
            bytes: 0,
            total_frames: 0,
            elapsed_seconds: 0.0,
        });
        assert_eq!(app.current_speed(Instant::now()), 0.0);
        assert!(!app.show_progress);
    }

    #[test]
    fn worker_failure_remains_visible_after_worker_stops() {
        let mut app = app();
        app.apply(ReceiverUpdate::Error("需要屏幕录制权限".into()));
        app.apply(ReceiverUpdate::Stopped);
        assert_eq!(app.error.as_deref(), Some("需要屏幕录制权限"));
        assert!(app.worker.is_none());
    }

    fn themed_app(ctx: &egui::Context, theme: egui::Theme) -> ReceiverApp {
        ctx.options_mut(|options| options.max_passes = 1.try_into().unwrap());
        crate::sender::install_cjk_fallback(ctx);
        let mut app = ReceiverApp::new(ctx, Config::default(), app().options.clone());
        app.theme = theme;
        ctx.set_theme(theme);
        app
    }

    fn screen_input(pixels_per_point: f32) -> egui::RawInput {
        let mut raw = input(WINDOW_SIZE);
        raw.viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(pixels_per_point);
        raw
    }

    fn render(
        ctx: &egui::Context,
        app: &mut ReceiverApp,
        pixels_per_point: f32,
    ) -> egui::FullOutput {
        ctx.run(screen_input(pixels_per_point), |ctx| {
            app.show(ctx);
            #[cfg(target_os = "linux")]
            app.show_file_picker(ctx);
        })
    }

    fn text_bounds(output: &egui::FullOutput, label: &str) -> egui::Rect {
        visible_text(output)
            .into_iter()
            .find(|(text, _)| text == label)
            .unwrap_or_else(|| panic!("missing visible text {label}"))
            .1
    }

    fn painted_rectangles(output: &egui::FullOutput) -> Vec<(egui::Rect, egui::Color32)> {
        fn collect(shape: &egui::Shape, result: &mut Vec<(egui::Rect, egui::Color32)>) {
            match shape {
                egui::Shape::Rect(rectangle) => result.push((rectangle.rect, rectangle.fill)),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, result);
                    }
                }
                _ => {}
            }
        }
        let mut result = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut result);
        }
        result
    }

    #[test]
    fn progress_bar_has_a_thicker_empty_track_until_a_transfer_starts() {
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            for scale in [1.0, 2.0] {
                let ctx = egui::Context::default();
                let mut app = themed_app(&ctx, theme);
                let colors = Palette::for_theme(theme);
                let _ = render(&ctx, &mut app, scale);
                for (event, filled) in [
                    (ReceiverUpdate::Waiting, false),
                    (
                        ReceiverUpdate::Metadata {
                            name: "speed.bin".into(),
                            total_frames: 2,
                            started_at: Instant::now(),
                        },
                        true,
                    ),
                    (
                        ReceiverUpdate::Progress {
                            number: 1,
                            received: 1,
                            total: 2,
                            received_bytes: 1024,
                            sampled_at: Instant::now(),
                        },
                        true,
                    ),
                    (ReceiverUpdate::Searching, false),
                    (ReceiverUpdate::Stopped, false),
                ] {
                    app.apply(event);
                    let output = render(&ctx, &mut app, scale);
                    let rectangles = painted_rectangles(&output);
                    let (track, _) = rectangles
                        .iter()
                        .find(|(rect, color)| {
                            *color == colors.canvas && (rect.height() - 20.0).abs() < 0.01
                        })
                        .expect("the 20 point progress track should always be visible");
                    let has_blue_fill = rectangles.iter().any(|(rect, color)| {
                        *color == colors.accent
                            && rect.min == track.min
                            && (rect.height() - track.height()).abs() < 0.01
                    });
                    assert_eq!(has_blue_fill, filled, "wrong progress fill in {theme:?}");
                    let speed = visible_text(&output)
                        .into_iter()
                        .find(|(text, _)| text.starts_with("当前速度："))
                        .expect("current transfer speed should remain visible");
                    assert!(speed.1.width() > 0.0);
                    let filename =
                        text_bounds(&output, app.filename.as_deref().unwrap_or("等待接收文件"));
                    let tolerance = 1.0 / output.pixels_per_point;
                    assert!((filename.left() - track.left()).abs() <= tolerance);
                    assert!((speed.1.right() - track.right()).abs() <= tolerance);
                    assert!(filename.right() < speed.1.left());
                }
            }
        }
    }

    #[test]
    fn forty_percent_receiver_keeps_actions_settings_and_scrolling_history_visible_at_high_dpi() {
        for size in [(1366.0, 728.0), (1920.0, 1040.0), (3840.0, 2080.0)] {
            for native_scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
                for theme in [egui::Theme::Light, egui::Theme::Dark] {
                    let area = ScreenArea {
                        work_rect: egui::Rect::from_min_size(
                            egui::pos2(-1920.0, 40.0),
                            size.into(),
                        ),
                        decoration: egui::vec2(16.0, 39.0) * native_scale,
                        can_position: true,
                    };
                    let plan = window_layout::fractional_window_plan(
                        area,
                        native_scale,
                        WindowRole::Receiver,
                    );
                    let ctx = egui::Context::default();
                    let mut app = themed_app(&ctx, theme);
                    for index in 0..30 {
                        app.apply(ReceiverUpdate::Completed {
                            path: PathBuf::from(format!("/桌面/接收文件{index:02}.bin")),
                            bytes: 1024,
                            total_frames: 10,
                            elapsed_seconds: 1.2,
                        });
                    }
                    app.connection = "连接状态\n".repeat(100);
                    app.location = "显示器和位置详情\n".repeat(100);
                    app.filename = Some("当前文件名\n".repeat(100));
                    app.missing = (1..=200)
                        .map(|number| number.to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    app.error = Some("错误信息详情\n".repeat(100));
                    let raw = |ppp: f32| {
                        let mut raw = input(plan.inner_pixels / ppp);
                        raw.viewports
                            .get_mut(&egui::ViewportId::ROOT)
                            .unwrap()
                            .native_pixels_per_point = Some(native_scale);
                        raw
                    };
                    let mut actual_pixels = plan.inner_pixels;
                    let mut outer_position = plan.outer_position.unwrap();
                    for _ in 0..4 {
                        let ppp = ctx.pixels_per_point();
                        let mut input = raw(ppp);
                        input.screen_rect = Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            actual_pixels / ppp,
                        ));
                        let output = ctx.run(input, |ctx| {
                            app.fit_window(ctx, Some(area));
                            app.show(ctx);
                        });
                        for command in &output.viewport_output[&egui::ViewportId::ROOT].commands {
                            match command {
                                egui::ViewportCommand::InnerSize(size) => {
                                    actual_pixels = *size * output.pixels_per_point
                                }
                                egui::ViewportCommand::OuterPosition(position) => {
                                    outer_position = *position * output.pixels_per_point
                                }
                                _ => {}
                            }
                        }
                    }
                    // Even an unusually long GB/s label must leave room for
                    // the filename in the minimum-width receiver window.
                    let sampled_at = Instant::now();
                    app.speed.reset(sampled_at - Duration::from_millis(100));
                    app.speed.record_bytes(u64::MAX, sampled_at);
                    app.receiving = true;
                    let output = ctx.run(raw(ctx.pixels_per_point()), |ctx| {
                        app.fit_window(ctx, Some(area));
                        app.show(ctx);
                    });
                    let screen = egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        plan.inner_pixels / output.pixels_per_point,
                    );
                    assert!(
                        screen
                            .expand(1.0 / output.pixels_per_point)
                            .contains_rect(ctx.used_rect()),
                        "receiver overflow at {size:?}/{native_scale}/{theme:?}: {:?} outside {screen:?}",
                        ctx.used_rect()
                    );
                    assert!((actual_pixels - plan.inner_pixels).length() < 0.01);
                    assert!((outer_position - plan.outer_position.unwrap()).length() < 0.01);
                    for label in [
                        "LanReceiver",
                        "设置",
                        "开始接收",
                        "水印",
                        "停止接收",
                        "连接信息",
                        "传输信息",
                        "复制帧号",
                        "本次接收历史",
                        "接收文件29.bin",
                    ] {
                        assert!(
                            screen.contains_rect(text_bounds(&output, label)),
                            "{label} clipped at {size:?}/{native_scale}/{theme:?}"
                        );
                    }
                    let texts = visible_text(&output);
                    for prefix in ["当前速度：", "当前文件名", "错误信息详情"] {
                        let (_, bounds) = texts
                            .iter()
                            .find(|(text, _)| text.starts_with(prefix))
                            .unwrap_or_else(|| panic!("missing visible {prefix}"));
                        assert!(
                            screen.contains_rect(*bounds),
                            "{prefix} clipped: {bounds:?}"
                        );
                    }
                    let filename = texts
                        .iter()
                        .find(|(text, _)| text.starts_with("当前文件名"))
                        .unwrap()
                        .1;
                    let speed = texts
                        .iter()
                        .find(|(text, _)| text.starts_with("当前速度："))
                        .unwrap()
                        .1;
                    let rectangles = painted_rectangles(&output);
                    let (track, _) = rectangles
                        .iter()
                        .find(|(rect, color)| {
                            *color == Palette::for_theme(theme).canvas
                                && (rect.height() - 20.0).abs() < 0.01
                        })
                        .unwrap();
                    let tolerance = 1.0 / output.pixels_per_point;
                    assert!((filename.left() - track.left()).abs() <= tolerance);
                    assert!((speed.right() - track.right()).abs() <= tolerance);
                    assert!(filename.right() < speed.left());
                    assert!(
                        output.viewport_output[&egui::ViewportId::ROOT]
                            .commands
                            .is_empty(),
                        "stable receiver must not continually force its position"
                    );
                    let mut wheel = raw(ctx.pixels_per_point());
                    wheel.events.push(egui::Event::PointerMoved(
                        text_bounds(&output, "接收文件29.bin").center(),
                    ));
                    wheel.events.push(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -100_000.0),
                        modifiers: egui::Modifiers::NONE,
                    });
                    let _ = ctx.run(wheel, |ctx| app.show(ctx));
                    let mut settled = raw(ctx.pixels_per_point());
                    settled.time = Some(5.0);
                    let scrolled = ctx.run(settled, |ctx| app.show(ctx));
                    let _ = text_bounds(&scrolled, "接收文件00.bin");
                    assert_eq!(
                        text_bounds(&output, "开始接收"),
                        text_bounds(&scrolled, "开始接收")
                    );
                    app.page = Page::Settings;
                    app.options.output_dir = PathBuf::from("很长的目录/".repeat(100));
                    // Include the third directory-card action while receiving.
                    let (_outgoing, incoming) = mpsc::channel();
                    app.worker = Some(Worker {
                        updates: incoming,
                        cancelled: Arc::new(AtomicBool::new(false)),
                    });
                    let settings = ctx.run(raw(ctx.pixels_per_point()), |ctx| app.show(ctx));
                    for label in [
                        "设置",
                        "返回",
                        "保存目录",
                        "选择目录",
                        "打开目录",
                        "停止接收",
                        "深色",
                        "浅色",
                    ] {
                        assert!(
                            screen.contains_rect(text_bounds(&settings, label)),
                            "settings {label} clipped at {size:?}/{native_scale}/{theme:?}"
                        );
                    }
                }
            }
        }
    }

    fn click(ctx: &egui::Context, app: &mut ReceiverApp, point: egui::Pos2, pixels_per_point: f32) {
        let mut press = screen_input(pixels_per_point);
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
            app.show(ctx);
            #[cfg(target_os = "linux")]
            app.show_file_picker(ctx);
        });
        let mut release = screen_input(pixels_per_point);
        release.events.push(egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(release, |ctx| {
            app.show(ctx);
            #[cfg(target_os = "linux")]
            app.show_file_picker(ctx);
        });
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_directory_picker_button_confirm_cancel_and_worker_guard() {
        let directory = tempfile::tempdir().unwrap();
        let chosen = directory.path().join("接收 空格目录");
        std::fs::create_dir(&chosen).unwrap();
        let ctx = egui::Context::default();
        let mut app = themed_app(&ctx, egui::Theme::Light);
        app.options.output_dir = directory.path().to_path_buf();
        app.page = Page::Settings;
        let _ = render(&ctx, &mut app, 1.0);
        let output = render(&ctx, &mut app, 1.0);
        click(
            &ctx,
            &mut app,
            text_bounds(&output, "选择目录").center(),
            1.0,
        );
        assert!(app.file_picker.is_some());
        let _ = render(&ctx, &mut app, 1.0);
        let output = render(&ctx, &mut app, 1.0);
        let child_directory = text_bounds(&output, "目录  接收 空格目录").center();
        click(&ctx, &mut app, child_directory, 1.0);
        click(&ctx, &mut app, child_directory, 1.0);
        assert_eq!(app.options.output_dir, directory.path());
        let output = render(&ctx, &mut app, 1.0);
        click(
            &ctx,
            &mut app,
            text_bounds(&output, "选择当前文件夹").center(),
            1.0,
        );
        assert!(app.file_picker.is_none());
        assert_eq!(app.options.output_dir, chosen);

        let output = render(&ctx, &mut app, 1.0);
        click(
            &ctx,
            &mut app,
            text_bounds(&output, "选择目录").center(),
            1.0,
        );
        let _ = render(&ctx, &mut app, 1.0);
        let output = render(&ctx, &mut app, 1.0);
        click(&ctx, &mut app, text_bounds(&output, "取消").center(), 1.0);
        assert!(app.file_picker.is_none());
        assert_eq!(app.options.output_dir, chosen);

        let (_outgoing, incoming) = mpsc::channel();
        app.worker = Some(Worker {
            updates: incoming,
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        let output = render(&ctx, &mut app, 1.0);
        click(
            &ctx,
            &mut app,
            text_bounds(&output, "选择目录").center(),
            1.0,
        );
        assert!(app.file_picker.is_none());
        assert_eq!(app.options.output_dir, chosen);
    }

    #[test]
    fn fixed_main_layout_keeps_history_visible_with_long_text_in_both_themes_and_scales() {
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            for pixels_per_point in [1.0, 2.0] {
                let ctx = egui::Context::default();
                let mut app = themed_app(&ctx, theme);
                for index in 0..30 {
                    app.apply(ReceiverUpdate::Completed {
                        path: PathBuf::from(format!("/桌面/接收文件{index:02}.bin")),
                        bytes: 1024,
                        total_frames: 10,
                        elapsed_seconds: 1.2,
                    });
                }
                let _ = render(&ctx, &mut app, pixels_per_point);
                let baseline = render(&ctx, &mut app, pixels_per_point);
                assert_eq!(ctx.pixels_per_point(), pixels_per_point);
                let labels = [
                    "LanReceiver",
                    "设置",
                    "开始接收",
                    "水印",
                    "停止接收",
                    "连接信息",
                    "传输信息",
                    "本次接收历史",
                ];
                let baseline_bounds: Vec<_> = labels
                    .iter()
                    .map(|label| text_bounds(&baseline, label))
                    .collect();
                let first_file_bounds = text_bounds(&baseline, "接收文件29.bin");
                app.options.output_dir =
                    PathBuf::from(format!("/桌面/{}", "很长的目录/".repeat(30)));
                app.connection = "连接状态\n".repeat(100);
                app.location = "显示器和位置详情\n".repeat(100);
                app.apply(ReceiverUpdate::Metadata {
                    name: "当前文件名\n".repeat(100),
                    total_frames: 200,
                    started_at: Instant::now(),
                });
                app.apply(ReceiverUpdate::Missing {
                    frames: (1..=200).collect(),
                });
                app.apply(ReceiverUpdate::Error("错误信息详情\n".repeat(100)));
                let mut used = egui::Rect::NOTHING;
                let output = ctx.run(screen_input(pixels_per_point), |ctx| {
                    app.show(ctx);
                    used = ctx.used_rect();
                });
                let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, WINDOW_SIZE);
                assert!(screen.contains_rect(used), "viewport overflow: {used:?}");
                for (label, bounds) in labels.iter().zip(&baseline_bounds) {
                    assert_eq!(
                        text_bounds(&output, label),
                        *bounds,
                        "{label} moved in {theme:?}/{pixels_per_point}"
                    );
                }
                assert_eq!(text_bounds(&output, "接收文件29.bin"), first_file_bounds);
                let texts = visible_text(&output);
                for forbidden in ["保存目录", "选择目录", "打开目录", "深色", "浅色"]
                {
                    assert!(
                        !texts.iter().any(|(text, _)| text == forbidden),
                        "main shows settings control {forbidden}"
                    );
                }
                for (_, bounds) in &texts {
                    assert!(
                        bounds.min.x >= 0.0 && bounds.max.x <= WINDOW_SIZE.x,
                        "horizontal text overflow in {theme:?}/{pixels_per_point}: {bounds:?}"
                    );
                }
                let mut wheel = screen_input(pixels_per_point);
                wheel
                    .events
                    .push(egui::Event::PointerMoved(first_file_bounds.center()));
                wheel.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -100_000.0),
                    modifiers: egui::Modifiers::NONE,
                });
                let _ = ctx.run(wheel, |ctx| app.show(ctx));
                let mut settled = screen_input(pixels_per_point);
                settled.time = Some(5.0);
                settled
                    .events
                    .push(egui::Event::PointerMoved(egui::pos2(0.0, 0.0)));
                let scrolled = ctx.run(settled, |ctx| app.show(ctx));
                for (label, bounds) in labels.iter().zip(&baseline_bounds) {
                    assert_eq!(
                        text_bounds(&scrolled, label),
                        *bounds,
                        "history scroll moved {label}"
                    );
                }
                let scrolled_text = visible_text(&scrolled);
                assert!(
                    scrolled_text
                        .iter()
                        .any(|(text, _)| text == "接收文件00.bin"),
                    "oldest file unavailable after history scroll"
                );
                assert!(
                    !scrolled_text
                        .iter()
                        .any(|(text, _)| text == "接收文件29.bin"),
                    "history did not scroll independently"
                );
            }
        }
    }

    #[test]
    fn settings_navigation_keeps_watermark_between_main_receive_actions() {
        for pixels_per_point in [1.0, 2.0] {
            let ctx = egui::Context::default();
            let mut app = themed_app(&ctx, egui::Theme::Light);
            assert!(app.config.water_mask);
            let _ = render(&ctx, &mut app, pixels_per_point);
            let main = render(&ctx, &mut app, pixels_per_point);
            let watermark = text_bounds(&main, "水印");
            assert!(watermark.min.x > text_bounds(&main, "开始接收").max.x);
            assert!(watermark.max.x < text_bounds(&main, "停止接收").min.x);
            click(&ctx, &mut app, watermark.center(), pixels_per_point);
            assert!(!app.config.water_mask);
            let main = render(&ctx, &mut app, pixels_per_point);
            click(
                &ctx,
                &mut app,
                text_bounds(&main, "设置").center(),
                pixels_per_point,
            );
            assert!(app.page == Page::Settings);
            let settings = render(&ctx, &mut app, pixels_per_point);
            for label in [
                "设置",
                "返回",
                "保存目录",
                "选择目录",
                "打开目录",
                "界面外观",
                "深色",
                "浅色",
            ] {
                let _ = text_bounds(&settings, label);
            }
            for label in ["连接信息", "水印", "水印模式", "开启", "关闭"] {
                assert!(
                    !visible_text(&settings)
                        .iter()
                        .any(|(text, _)| text == label)
                );
            }
            click(
                &ctx,
                &mut app,
                text_bounds(&settings, "深色").center(),
                pixels_per_point,
            );
            assert_eq!(app.theme, egui::Theme::Dark);
            let settings = render(&ctx, &mut app, pixels_per_point);
            assert!(!app.config.water_mask);
            click(
                &ctx,
                &mut app,
                text_bounds(&settings, "返回").center(),
                pixels_per_point,
            );
            assert!(app.page == Page::Main);
            let main = render(&ctx, &mut app, pixels_per_point);
            let _ = text_bounds(&main, "LanReceiver");
            let watermark = text_bounds(&main, "水印");
            click(&ctx, &mut app, watermark.center(), pixels_per_point);
            assert!(app.config.water_mask);
            let (_outgoing, incoming) = mpsc::channel();
            app.worker = Some(Worker {
                updates: incoming,
                cancelled: Arc::new(AtomicBool::new(false)),
            });
            let receiving = render(&ctx, &mut app, pixels_per_point);
            click(
                &ctx,
                &mut app,
                text_bounds(&receiving, "水印").center(),
                pixels_per_point,
            );
            assert!(app.config.water_mask, "receiving must lock the switch");
            app.stop();
            let stopping = render(&ctx, &mut app, pixels_per_point);
            click(
                &ctx,
                &mut app,
                text_bounds(&stopping, "水印").center(),
                pixels_per_point,
            );
            assert!(
                app.config.water_mask,
                "stopping must keep the switch locked"
            );
            app.apply(ReceiverUpdate::Stopped);
            let stopped = render(&ctx, &mut app, pixels_per_point);
            click(
                &ctx,
                &mut app,
                text_bounds(&stopped, "水印").center(),
                pixels_per_point,
            );
            assert!(
                !app.config.water_mask,
                "stopped receiver must unlock the switch"
            );
            for label in [
                "保存目录",
                "选择目录",
                "打开目录",
                "深色",
                "浅色",
                "水印模式",
                "开启",
                "关闭",
            ] {
                assert!(!visible_text(&main).iter().any(|(text, _)| text == label));
            }
        }
    }

    #[test]
    fn watermark_updates_manual_region_atomically_and_is_locked_during_receive() {
        let mut app = app();
        assert!(app.config.water_mask);
        app.set_water_mask(false);
        app.options.region = Some(receiver::parse_region("10,20,80,40,4", false).unwrap());
        app.set_water_mask(true);
        assert!(app.config.water_mask);
        let region = app.options.region.unwrap();
        assert!(region.grid.water_mask);
        assert_eq!((region.x, region.y), (10, 20));

        let (_sender, incoming) = mpsc::channel();
        app.worker = Some(Worker {
            updates: incoming,
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        app.set_water_mask(false);
        assert!(app.config.water_mask);
        assert!(app.options.region.unwrap().grid.water_mask);

        app.worker = None;
        app.set_water_mask(false);
        app.options.region = Some(receiver::parse_region("10,20,60,30,3", false).unwrap());
        app.set_water_mask(true);
        assert!(!app.config.water_mask);
        assert!(!app.options.region.unwrap().grid.water_mask);
        assert!(app.error.is_some());
    }

    #[test]
    fn clicking_history_filename_queues_exact_file_to_reveal_without_os_dispatch() {
        let ctx = egui::Context::default();
        let mut app = themed_app(&ctx, egui::Theme::Light);
        let path = PathBuf::from("/桌面/测试目录/report (1).txt");
        app.apply(ReceiverUpdate::Completed {
            path: path.clone(),
            bytes: 2,
            total_frames: 1,
            elapsed_seconds: 0.2,
        });
        let _ = render(&ctx, &mut app, 1.0);
        let output = render(&ctx, &mut app, 1.0);
        click(
            &ctx,
            &mut app,
            text_bounds(&output, "report (1).txt").center(),
            1.0,
        );
        assert_eq!(app.pending_reveal, Some(path));
        assert!(app.error.is_none());
    }

    #[test]
    fn worker_event_sequence_keeps_history_and_errors_until_a_new_transfer() {
        let mut app = app();
        app.apply(ReceiverUpdate::Completed {
            path: PathBuf::from("/test/first.bin"),
            bytes: 20,
            total_frames: 2,
            elapsed_seconds: 0.5,
        });
        let (outgoing, incoming) = mpsc::channel();
        app.worker = Some(Worker {
            updates: incoming,
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        outgoing
            .send(ReceiverUpdate::Error("屏幕读取失败".into()))
            .unwrap();
        // A worker can disconnect immediately after its final error.
        drop(outgoing);
        app.poll();
        assert!(app.worker.is_none());
        assert_eq!(app.error.as_deref(), Some("屏幕读取失败"));
        assert_eq!(app.history.len(), 1);
        assert_eq!(app.history[0].path, PathBuf::from("/test/first.bin"));
        app.apply(ReceiverUpdate::Searching);
        app.apply(ReceiverUpdate::Waiting);
        assert_eq!(app.error.as_deref(), Some("屏幕读取失败"));
        app.apply(ReceiverUpdate::Metadata {
            name: "second.bin".into(),
            total_frames: 3,
            started_at: Instant::now(),
        });
        assert!(app.error.is_none());
        assert_eq!(app.received, 0);
        assert_eq!(app.total, 3);
        assert_eq!(app.filename.as_deref(), Some("second.bin"));
        assert_eq!(app.history.len(), 1);
    }
}
