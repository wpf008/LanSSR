//! Shared in-process file selection, including desktops without native dialog services.

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::ui_style::{Palette, action_button, primary_button};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickerMode {
    File,
    // Other platforms retain their native receiver directory dialog.
    #[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
    Directory,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PickerResult {
    Selected(PathBuf),
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryKind {
    Directory,
    File,
    Unavailable,
}

type EntrySelection = Option<(PathBuf, EntryKind)>;

struct Entry {
    path: PathBuf,
    name: String,
    kind: EntryKind,
    size: String,
    file_type: String,
    hidden: bool,
}

pub(crate) struct FilePicker {
    mode: PickerMode,
    current_dir: PathBuf,
    home: PathBuf,
    desktop: PathBuf,
    locations: Vec<(String, PathBuf)>,
    path_input: String,
    path_dirty: bool,
    entries: Vec<Entry>,
    selected: Option<PathBuf>,
    selected_is_file: bool,
    search: String,
    show_hidden: bool,
    visible_entries: Vec<usize>,
    error: Option<String>,
}

impl FilePicker {
    pub(crate) fn new(mode: PickerMode, start: PathBuf) -> Self {
        let home = user_home_dir();
        let desktop = crate::desktop_paths::default_desktop_dir();
        let locations = common_locations(&home, &desktop);
        let start = if start.is_absolute() {
            start
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| home.clone())
                .join(start)
        };
        let metadata = std::fs::metadata(&start);
        let is_file = metadata.as_ref().is_ok_and(|metadata| metadata.is_file());
        let show_hidden = entry_is_hidden(
            &start.file_name().unwrap_or_default().to_string_lossy(),
            metadata.as_ref().ok(),
        );
        let directory = if is_file {
            start.parent().unwrap_or(&home).to_path_buf()
        } else {
            start.clone()
        };
        let mut picker = Self {
            mode,
            current_dir: directory.clone(),
            home,
            desktop,
            locations,
            path_input: directory.to_string_lossy().into_owned(),
            path_dirty: false,
            entries: Vec::new(),
            selected: None,
            selected_is_file: false,
            search: String::new(),
            show_hidden,
            visible_entries: Vec::new(),
            error: None,
        };
        if picker.navigate(directory) {
            if mode == PickerMode::File && is_file {
                picker.selected = Some(start);
                picker.selected_is_file = true;
            }
        } else {
            let error = picker.error.take();
            // A removed selection or missing desktop should still allow browsing.
            for fallback in [picker.home.clone(), filesystem_root(&picker.home)] {
                if picker.navigate(fallback) {
                    break;
                }
            }
            picker.error = error;
        }
        picker
    }

    /// Reads the directory once on navigation; repainting only uses this snapshot.
    fn navigate(&mut self, directory: PathBuf) -> bool {
        match read_directory(&directory) {
            Ok(entries) => {
                self.path_input = directory.to_string_lossy().into_owned();
                self.path_dirty = false;
                self.current_dir = directory;
                self.entries = entries;
                self.search.clear();
                self.update_visible_entries();
                self.selected = None;
                self.selected_is_file = false;
                self.error = None;
                true
            }
            Err(error) => {
                self.error = Some(error);
                false
            }
        }
    }

    fn entered_path(&self) -> Result<PathBuf, String> {
        // Keep the original OsString when the displayed path was not edited.
        // Lossy display strings must never become the selected native path.
        if !self.path_dirty {
            return Ok(self.current_dir.clone());
        }
        if self.path_input.is_empty() {
            return Err("请输入文件或文件夹路径".to_owned());
        }
        if self.path_input == "~" {
            return Ok(self.home.clone());
        }
        if let Some(relative) = self.path_input.strip_prefix("~/") {
            return Ok(self.home.join(relative));
        }
        #[cfg(target_os = "windows")]
        if let Some(relative) = self.path_input.strip_prefix("~\\") {
            return Ok(self.home.join(relative));
        }
        resolve_entered_path(PathBuf::from(&self.path_input), &self.current_dir)
    }

    fn activate_path(&mut self, confirm: bool) -> Option<PickerResult> {
        let path = match self.entered_path() {
            Ok(path) => path,
            Err(error) => {
                self.error = Some(error);
                return None;
            }
        };
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_dir() => {
                if confirm && self.mode == PickerMode::Directory {
                    // Ensure the chosen directory is accessible before returning it.
                    match std::fs::read_dir(&path) {
                        Ok(_) => return Some(PickerResult::Selected(path)),
                        Err(error) => self.error = Some(path_error(&path, error)),
                    }
                } else {
                    self.navigate(path);
                }
            }
            Ok(metadata) if metadata.is_file() && self.mode == PickerMode::File => {
                return Some(PickerResult::Selected(path));
            }
            Ok(_) => {
                self.error = Some(if self.mode == PickerMode::Directory {
                    "请选择文件夹，不能选择文件或特殊设备".to_owned()
                } else {
                    "请选择普通文件，不能选择特殊设备".to_owned()
                });
            }
            Err(error) => self.error = Some(path_error(&path, error)),
        }
        None
    }

    fn confirm(&mut self) -> Option<PickerResult> {
        if self.path_dirty {
            return self.activate_path(true);
        }
        if self.mode == PickerMode::Directory {
            let path = self.selected.as_ref().unwrap_or(&self.current_dir).clone();
            return match std::fs::read_dir(&path) {
                Ok(_) => Some(PickerResult::Selected(path)),
                Err(error) => {
                    self.error = Some(path_error(&path, error));
                    None
                }
            };
        }
        let path = self.selected.clone()?;
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => Some(PickerResult::Selected(path)),
            Ok(_) => {
                self.error = Some("请选择普通文件；双击文件夹可以进入".to_owned());
                None
            }
            Err(error) => {
                self.error = Some(path_error(&path, error));
                None
            }
        }
    }

    fn file_selected(&self) -> bool {
        self.selected_is_file
    }

    fn update_visible_entries(&mut self) {
        let search = self.search.to_lowercase();
        self.visible_entries = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                (self.show_hidden || !entry.hidden)
                    && (search.is_empty() || entry.name.to_lowercase().contains(&search))
            })
            .map(|(index, _)| index)
            .collect();
        if self.selected.as_ref().is_some_and(|selected| {
            !self
                .visible_entries
                .iter()
                .any(|index| &self.entries[*index].path == selected)
        }) {
            self.selected = None;
            self.selected_is_file = false;
        }
    }

    fn refresh(&mut self) {
        let selection = self.selected.clone();
        let search = self.search.clone();
        if self.navigate(self.current_dir.clone()) {
            self.search = search;
            self.update_visible_entries();
            let entry = self
                .entries
                .iter()
                .find(|entry| selection.as_ref() == Some(&entry.path));
            self.selected = entry.map(|entry| entry.path.clone());
            self.selected_is_file = entry.is_some_and(|entry| entry.kind == EntryKind::File);
        }
    }

    pub(crate) fn show(&mut self, ctx: &egui::Context) -> Option<PickerResult> {
        let viewport = ctx.content_rect();
        let available = viewport.size();
        let width = (available.x - 48.0).clamp(180.0, 820.0);
        let dialog_height = (available.y - 48.0).clamp(240.0, 560.0);
        let spacious = width >= 600.0;
        let colors = Palette::for_theme(if ctx.style().visuals.dark_mode {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });
        let mut result = None;
        let modal_id = egui::Id::new("shared_file_picker");
        let frame = egui::Frame::popup(&ctx.style());
        let outer_size = egui::vec2(width, dialog_height) + frame.total_margin().sum();
        // Area's centered anchor uses its previous measured size. Position from
        // the current viewport instead, so a resized open dialog stays visible
        // immediately and cannot keep the height of a larger previous window.
        let position = viewport.center() - outer_size * 0.5;
        let modal = egui::Modal::new(modal_id)
            .area(
                egui::Area::new(modal_id)
                    .kind(egui::UiKind::Modal)
                    .sense(egui::Sense::hover())
                    .order(egui::Order::Foreground)
                    .interactable(true)
                    .fixed_pos(position)
                    .constrain(false)
                    .default_size(outer_size),
            )
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.set_height(dialog_height);
                let content_top = ui.cursor().top();
                ui.horizontal(|ui| {
                    ui.heading(match self.mode {
                        PickerMode::File => "选择发送文件",
                        PickerMode::Directory => "选择保存文件夹",
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(action_button("关闭", 60.0)).clicked() {
                            result = Some(PickerResult::Cancelled);
                        }
                    });
                });
                if spacious {
                    ui.label(
                        egui::RichText::new(match self.mode {
                            PickerMode::File => "浏览本机文件，选择后即可发送",
                            PickerMode::Directory => "选择接收文件的保存位置",
                        })
                        .small()
                        .color(colors.muted),
                    );
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("位置").color(colors.muted));
                    let response = ui.add_sized(
                        egui::vec2((ui.available_width() - 76.0).max(40.0), 32.0),
                        egui::TextEdit::singleline(&mut self.path_input)
                            .id_salt("shared_picker_path")
                            .vertical_align(egui::Align::Center)
                            .hint_text("完整路径、相对路径或 ~/"),
                    );
                    if response.changed() {
                        self.path_dirty = true;
                    }
                    let enter = response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    if ui.add(action_button("前往", 60.0)).clicked() || enter {
                        result = self.activate_path(false);
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let parent = self.current_dir.parent().map(Path::to_path_buf);
                    if ui
                        .add_enabled(parent.is_some(), action_button("上一级", 68.0))
                        .clicked()
                        && let Some(parent) = parent
                    {
                        self.navigate(parent);
                    }
                    if !spacious {
                        if ui.add(action_button("桌面", 60.0)).clicked() {
                            self.navigate(self.desktop.clone());
                        }
                        if ui.add(action_button("主目录", 68.0)).clicked() {
                            self.navigate(self.home.clone());
                        }
                    }
                    if ui.add(action_button("刷新", 60.0)).clicked() {
                        self.refresh();
                    }
                    if spacious {
                        self.search_controls(ui);
                    }
                });
                if !spacious {
                    ui.horizontal(|ui| {
                        #[cfg(target_os = "windows")]
                        self.drive_selector(ui);
                        self.search_controls(ui);
                    });
                }
                ui.add_space(6.0);
                // Reserve the footer before allocating the browser. Only its list
                // scrolls, so selection details and actions never leave the window.
                let header_height = ui.cursor().top() - content_top;
                let browser_height = (dialog_height - header_height - 90.0).max(56.0);
                let (browser_rect, _) =
                    ui.allocate_exact_size(egui::vec2(width, browser_height), egui::Sense::hover());
                let sidebar_width = if spacious { 128.0 } else { 0.0 };
                if spacious {
                    self.show_locations(
                        ui,
                        egui::Rect::from_min_size(
                            browser_rect.min,
                            egui::vec2(sidebar_width - 10.0, browser_height),
                        ),
                        colors,
                    );
                }
                let list_rect = egui::Rect::from_min_max(
                    browser_rect.min + egui::vec2(sidebar_width, 0.0),
                    browser_rect.max,
                );
                let (clicked, activated) = self.show_entries(ui, list_rect, colors);
                if let Some((path, kind)) = clicked {
                    self.selected = Some(path);
                    self.selected_is_file = kind == EntryKind::File;
                    self.path_input = self.current_dir.to_string_lossy().into_owned();
                    self.path_dirty = false;
                    self.error = None;
                }
                if let Some((path, kind)) = activated {
                    if kind == EntryKind::Directory {
                        self.navigate(path);
                    } else if kind == EntryKind::File {
                        result = self.confirm();
                    }
                }
                ui.add_space(7.0);
                let selected_entry = self
                    .selected
                    .as_ref()
                    .and_then(|path| self.entries.iter().find(|entry| &entry.path == path));
                let summary = if self.path_dirty {
                    format!("待选择路径：{}", self.path_input)
                } else {
                    match self.mode {
                        PickerMode::Directory => format!(
                            "保存位置：{}",
                            self.selected
                                .as_ref()
                                .unwrap_or(&self.current_dir)
                                .display()
                        ),
                        PickerMode::File => selected_entry.map_or_else(
                            || "尚未选择文件".to_owned(),
                            |entry| format!("已选择：{}  ·  {}", entry.name, entry.size),
                        ),
                    }
                };
                ui.add(
                    egui::Label::new(egui::RichText::new(&summary).color(colors.text)).truncate(),
                )
                .on_hover_text(
                    self.selected
                        .as_ref()
                        .unwrap_or(&self.current_dir)
                        .to_string_lossy(),
                );
                let hint = self.error.clone().unwrap_or_else(|| {
                    format!(
                        "{} 个项目  ·  {}",
                        self.visible_entries.len(),
                        match self.mode {
                            PickerMode::File => "单击选择文件，双击打开文件夹",
                            PickerMode::Directory => "单击选择文件夹，双击进入",
                        }
                    )
                });
                ui.add(
                    egui::Label::new(egui::RichText::new(&hint).small().color(
                        if self.error.is_some() {
                            colors.error
                        } else {
                            colors.muted
                        },
                    ))
                    .truncate(),
                )
                .on_hover_text(&hint);
                ui.add_space(5.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_confirm = self.mode == PickerMode::Directory
                        || self.path_dirty
                        || self.file_selected();
                    let confirm_label = match self.mode {
                        PickerMode::File => "确认选择文件",
                        PickerMode::Directory if self.selected.is_some() => "确认选择文件夹",
                        PickerMode::Directory => "选择当前文件夹",
                    };
                    if ui
                        .add_enabled(can_confirm, primary_button(confirm_label, 124.0))
                        .clicked()
                    {
                        result = self.confirm();
                    }
                    if ui.add(action_button("取消", 68.0)).clicked() {
                        result = Some(PickerResult::Cancelled);
                    }
                });
            });
        if result.is_none() && modal.should_close() {
            result = Some(PickerResult::Cancelled);
        }
        result
    }

    fn search_controls(&mut self, ui: &mut egui::Ui) {
        let search = ui.add_sized(
            egui::vec2((ui.available_width() - 120.0).max(48.0), 32.0),
            egui::TextEdit::singleline(&mut self.search)
                .id_salt("shared_picker_search")
                .vertical_align(egui::Align::Center)
                .hint_text("搜索当前文件夹"),
        );
        let hidden = ui.checkbox(&mut self.show_hidden, "隐藏文件");
        if search.changed() || hidden.changed() {
            self.update_visible_entries();
        }
    }

    #[cfg(target_os = "windows")]
    fn drive_selector(&mut self, ui: &mut egui::Ui) {
        let mut destination = None;
        egui::ComboBox::from_id_salt("picker_drives")
            .selected_text("盘符")
            .width(58.0)
            .show_ui(ui, |ui| {
                for (name, path) in self.locations.iter().skip(2) {
                    if ui
                        .selectable_label(self.current_dir.starts_with(path), name)
                        .clicked()
                    {
                        destination = Some(path.clone());
                    }
                }
            });
        if let Some(path) = destination {
            self.navigate(path);
        }
    }

    fn show_locations(&mut self, ui: &mut egui::Ui, rect: egui::Rect, colors: Palette) {
        let mut destination = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.add_space(9.0);
            ui.label(egui::RichText::new("常用位置").small().color(colors.muted));
            ui.add_space(7.0);
            egui::ScrollArea::vertical()
                .id_salt("picker_locations")
                .max_height((rect.height() - 42.0).max(35.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (name, path) in &self.locations {
                        let (row, response) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 35.0),
                            egui::Sense::click(),
                        );
                        let selected = self.current_dir == *path;
                        if selected || response.hovered() {
                            ui.painter().rect_filled(row, 7.0, colors.accent_soft);
                        }
                        paint_entry_icon(
                            ui.painter(),
                            row.left_center() + egui::vec2(18.0, 0.0),
                            EntryKind::Directory,
                            if selected {
                                colors.accent
                            } else {
                                colors.muted
                            },
                        );
                        ui.painter().text(
                            row.left_center() + egui::vec2(34.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            name,
                            egui::FontId::proportional(14.0),
                            if selected { colors.accent } else { colors.text },
                        );
                        if response.clicked() {
                            destination = Some(path.clone());
                        }
                    }
                });
        });
        if let Some(path) = destination {
            self.navigate(path);
        }
    }

    fn show_entries(
        &self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        colors: Palette,
    ) -> (EntrySelection, EntrySelection) {
        let mut clicked = None;
        let mut activated = None;
        ui.painter().rect_filled(rect, 8.0, colors.surface);
        ui.painter().rect_stroke(
            rect,
            8.0,
            egui::Stroke::new(1.0_f32, colors.border),
            egui::StrokeKind::Inside,
        );
        let inner = rect.shrink(7.0);
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            let wide = inner.width() >= 470.0;
            let show_size = inner.width() >= 260.0;
            let size_width = if show_size { 88.0 } else { 0.0 };
            let type_width = if wide { 112.0 } else { 0.0 };
            let (heading, _) =
                ui.allocate_exact_size(egui::vec2(inner.width(), 23.0), egui::Sense::hover());
            let painter = ui.painter();
            let font = egui::FontId::proportional(11.5);
            painter.text(
                heading.left_center() + egui::vec2(29.0, 0.0),
                egui::Align2::LEFT_CENTER,
                "名称",
                font.clone(),
                colors.muted,
            );
            if wide {
                painter.text(
                    egui::pos2(
                        heading.right() - size_width - type_width,
                        heading.center().y,
                    ),
                    egui::Align2::LEFT_CENTER,
                    "类型",
                    font.clone(),
                    colors.muted,
                );
            }
            if show_size {
                painter.text(
                    heading.right_center() - egui::vec2(8.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    "大小",
                    font,
                    colors.muted,
                );
            }
            painter.hline(
                heading.x_range(),
                heading.bottom(),
                egui::Stroke::new(1.0_f32, colors.border),
            );
            let list_height = (inner.height() - 28.0).max(24.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add_space(5.0);
            egui::ScrollArea::vertical()
                .id_salt(("shared_picker_entries", &self.current_dir))
                .max_height(list_height)
                .min_scrolled_height(list_height)
                .auto_shrink([false, false])
                .show_rows(ui, 34.0, self.visible_entries.len().max(1), |ui, rows| {
                    if self.visible_entries.is_empty() {
                        ui.add_space(7.0);
                        ui.weak(if self.search.is_empty() {
                            "此文件夹为空"
                        } else {
                            "没有匹配的文件或文件夹"
                        });
                        return;
                    }
                    for row in rows {
                        let entry = &self.entries[self.visible_entries[row]];
                        let enabled = entry.kind == EntryKind::Directory
                            || (self.mode == PickerMode::File && entry.kind == EntryKind::File);
                        let (rect, response) = ui
                            .push_id(&entry.path, |ui| {
                                ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), 34.0),
                                    if enabled {
                                        egui::Sense::click()
                                    } else {
                                        egui::Sense::hover()
                                    },
                                )
                            })
                            .inner;
                        let selected = self.selected.as_ref() == Some(&entry.path);
                        if selected || (enabled && response.hovered()) {
                            ui.painter().rect_filled(rect, 5.0, colors.accent_soft);
                        } else if row % 2 != 0 {
                            ui.painter().rect_filled(rect, 3.0, colors.background);
                        }
                        if selected {
                            ui.painter().rect_stroke(
                                rect,
                                5.0,
                                egui::Stroke::new(1.0_f32, colors.accent),
                                egui::StrokeKind::Inside,
                            );
                        }
                        let color = if !enabled {
                            colors.muted
                        } else if selected {
                            colors.accent
                        } else {
                            colors.text
                        };
                        paint_entry_icon(
                            ui.painter(),
                            rect.left_center() + egui::vec2(13.0, 0.0),
                            entry.kind,
                            color,
                        );
                        // Both spans share a galley while the file name stays left aligned.
                        let mut job = egui::text::LayoutJob::default();
                        job.append(
                            match entry.kind {
                                EntryKind::Directory => "目录  ",
                                EntryKind::File => "文件  ",
                                EntryKind::Unavailable => "不可用  ",
                            },
                            0.0,
                            egui::TextFormat {
                                font_id: egui::FontId::proportional(11.5),
                                color: colors.muted,
                                ..Default::default()
                            },
                        );
                        job.append(
                            &entry.name,
                            0.0,
                            egui::TextFormat {
                                font_id: egui::FontId::proportional(14.0),
                                color,
                                ..Default::default()
                            },
                        );
                        job.wrap.max_width =
                            (rect.width() - 37.0 - size_width - type_width).max(20.0);
                        job.wrap.max_rows = 1;
                        job.wrap.break_anywhere = true;
                        let galley = ui.painter().layout_job(job);
                        ui.painter().galley(
                            egui::pos2(rect.left() + 28.0, rect.center().y - galley.size().y * 0.5),
                            galley,
                            color,
                        );
                        if wide {
                            ui.painter().text(
                                egui::pos2(rect.right() - size_width - type_width, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                &entry.file_type,
                                egui::FontId::proportional(11.5),
                                colors.muted,
                            );
                        }
                        if show_size {
                            ui.painter().text(
                                rect.right_center() - egui::vec2(8.0, 0.0),
                                egui::Align2::RIGHT_CENTER,
                                &entry.size,
                                egui::FontId::proportional(11.5),
                                colors.muted,
                            );
                        }
                        let response = response.on_hover_text(entry.path.to_string_lossy());
                        if response.clicked() {
                            clicked = Some((entry.path.clone(), entry.kind));
                        }
                        if response.double_clicked() {
                            activated = Some((entry.path.clone(), entry.kind));
                        }
                    }
                });
        });
        (clicked, activated)
    }
}

fn path_error(path: &Path, error: std::io::Error) -> String {
    format!("无法访问 {}：{error}", path.display())
}

fn paint_entry_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    kind: EntryKind,
    color: egui::Color32,
) {
    let stroke = egui::Stroke::new(1.25_f32, color);
    if kind == EntryKind::Directory {
        let folder =
            egui::Rect::from_center_size(center + egui::vec2(0.0, 1.0), egui::vec2(16.0, 11.0));
        painter.rect_stroke(folder, 2.0, stroke, egui::StrokeKind::Inside);
        painter.line_segment(
            [
                folder.left_top() + egui::vec2(1.0, 0.0),
                folder.left_top() + egui::vec2(1.0, -3.0),
            ],
            stroke,
        );
        painter.line_segment(
            [
                folder.left_top() + egui::vec2(1.0, -3.0),
                folder.left_top() + egui::vec2(7.0, -3.0),
            ],
            stroke,
        );
        painter.line_segment(
            [
                folder.left_top() + egui::vec2(7.0, -3.0),
                folder.left_top() + egui::vec2(9.0, 0.0),
            ],
            stroke,
        );
    } else {
        let file = egui::Rect::from_center_size(center, egui::vec2(12.0, 16.0));
        painter.rect_stroke(file, 2.0, stroke, egui::StrokeKind::Inside);
        for y in [-2.0, 2.0, 5.0] {
            painter.line_segment(
                [center + egui::vec2(-3.0, y), center + egui::vec2(3.0, y)],
                stroke,
            );
        }
        if kind == EntryKind::Unavailable {
            painter.line_segment([file.left_top(), file.right_bottom()], stroke);
        }
    }
}

fn format_file_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn user_home_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    let candidates = ["USERPROFILE", "HOME"];
    #[cfg(not(target_os = "windows"))]
    let candidates = ["HOME", "USERPROFILE"];
    candidates
        .into_iter()
        .find_map(|name| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        })
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| platform_root()))
}

fn platform_root() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let drive = std::env::var_os("SystemDrive")
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "C:".into());
        PathBuf::from(drive).join(r"\")
    }
    #[cfg(not(target_os = "windows"))]
    PathBuf::from("/")
}

fn filesystem_root(path: &Path) -> PathBuf {
    path.ancestors()
        .last()
        .filter(|root| root.is_absolute())
        .map(Path::to_path_buf)
        .unwrap_or_else(platform_root)
}

fn common_locations(home: &Path, desktop: &Path) -> Vec<(String, PathBuf)> {
    let mut locations = vec![
        ("桌面".to_owned(), desktop.to_path_buf()),
        ("主目录".to_owned(), home.to_path_buf()),
    ];
    #[cfg(target_os = "windows")]
    {
        // Enumerating a drive bitmask avoids probing disconnected network drives.
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetLogicalDrives() -> u32;
        }
        locations.extend(drive_locations(unsafe { GetLogicalDrives() }));
    }
    #[cfg(not(target_os = "windows"))]
    locations.push(("文件系统".to_owned(), platform_root()));
    #[cfg(target_os = "macos")]
    locations.push(("磁盘".to_owned(), PathBuf::from("/Volumes")));
    locations
}

#[cfg(any(target_os = "windows", test))]
fn drive_locations(mask: u32) -> Vec<(String, PathBuf)> {
    (0..26)
        .filter(|index| mask & (1 << index) != 0)
        .map(|index| {
            let letter = char::from(b'A' + index as u8);
            (
                format!("磁盘 {letter}:"),
                PathBuf::from(format!("{letter}:\\")),
            )
        })
        .collect()
}

fn resolve_entered_path(path: PathBuf, current_dir: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path);
    }
    #[cfg(target_os = "windows")]
    {
        use std::path::Component;
        if matches!(path.components().next(), Some(Component::Prefix(_))) {
            // C:relative follows Windows' per-drive working-directory semantics.
            // Calling absolute preserves that native behavior rather than joining
            // this drive-relative prefix onto an unrelated directory.
            return std::path::absolute(&path).map_err(|error| path_error(&path, error));
        }
        if path.has_root() {
            let relative: PathBuf = path
                .components()
                .filter(|component| !matches!(component, Component::RootDir))
                .collect();
            return Ok(filesystem_root(current_dir).join(relative));
        }
    }
    Ok(current_dir.join(path))
}

fn entry_is_hidden(name: &str, _metadata: Option<&std::fs::Metadata>) -> bool {
    let hidden = name.starts_with('.');
    #[cfg(target_os = "windows")]
    let hidden = hidden
        || _metadata.is_some_and(|metadata| {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes() & 0x2 != 0
        });
    hidden
}

fn read_directory(path: &Path) -> Result<Vec<Entry>, String> {
    let directory = std::fs::read_dir(path).map_err(|error| path_error(path, error))?;
    let mut entries = Vec::new();
    for entry in directory {
        let entry = entry.map_err(|error| path_error(path, error))?;
        let entry_path = entry.path();
        // Follow valid symlinks, but leave dangling links and special devices disabled.
        let metadata = std::fs::metadata(&entry_path);
        let (kind, size) = match &metadata {
            Ok(metadata) if metadata.is_dir() => (EntryKind::Directory, "—".to_owned()),
            Ok(metadata) if metadata.is_file() => {
                (EntryKind::File, format_file_size(metadata.len()))
            }
            _ => (EntryKind::Unavailable, "—".to_owned()),
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = match kind {
            EntryKind::Directory => "文件夹".to_owned(),
            EntryKind::Unavailable => "不可用".to_owned(),
            EntryKind::File => entry_path
                .extension()
                .filter(|extension| !extension.is_empty())
                .map(|extension| {
                    extension
                        .to_string_lossy()
                        .chars()
                        .take(9)
                        .collect::<String>()
                        .to_uppercase()
                })
                .map_or_else(
                    || "文件".to_owned(),
                    |extension| format!("{extension} 文件"),
                ),
        };
        let hidden = entry_is_hidden(&name, metadata.as_ref().ok());
        entries.push(Entry {
            path: entry_path,
            hidden,
            name,
            kind,
            size,
            file_type,
        });
    }
    entries.sort_by(|left, right| {
        (left.kind != EntryKind::Directory)
            .cmp(&(right.kind != EntryKind::Directory))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entered_paths_keep_relative_absolute_and_home_navigation() {
        let directory = tempfile::tempdir().unwrap();
        let mut picker = FilePicker::new(PickerMode::File, directory.path().to_path_buf());
        picker.path_dirty = true;
        picker.path_input = "子目录/带 空格文件.txt".to_owned();
        assert_eq!(
            picker.entered_path().unwrap(),
            directory.path().join("子目录/带 空格文件.txt")
        );
        let absolute = directory.path().join("绝对路径.txt");
        picker.path_input = absolute.to_string_lossy().into_owned();
        assert_eq!(picker.entered_path().unwrap(), absolute);
        picker.path_input = "~/Desktop".to_owned();
        assert_eq!(picker.entered_path().unwrap(), picker.home.join("Desktop"));
        picker.path_input = "~".to_owned();
        assert_eq!(picker.entered_path().unwrap(), picker.home);
    }

    #[test]
    fn drive_shortcuts_include_each_available_drive_and_ignore_other_bits() {
        let locations = drive_locations((1 << 2) | (1 << 3) | (1 << 25) | (1 << 30));
        assert_eq!(
            locations,
            vec![
                ("磁盘 C:".to_owned(), PathBuf::from(r"C:\")),
                ("磁盘 D:".to_owned(), PathBuf::from(r"D:\")),
                ("磁盘 Z:".to_owned(), PathBuf::from(r"Z:\")),
            ]
        );
        assert!(drive_locations(0).is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_root_relative_absolute_unc_and_home_backslash_paths() {
        let current = Path::new(r"D:\工作\子目录");
        assert_eq!(filesystem_root(current), Path::new(r"D:\"));
        assert_eq!(
            resolve_entered_path(PathBuf::from(r"\待发送\文件.txt"), current).unwrap(),
            Path::new(r"D:\待发送\文件.txt")
        );
        for path in [r"C:\其他磁盘\文件.txt", r"\\server\share\文件.txt"] {
            assert_eq!(
                resolve_entered_path(PathBuf::from(path), current).unwrap(),
                Path::new(path)
            );
        }
        let directory = tempfile::tempdir().unwrap();
        let mut picker = FilePicker::new(PickerMode::File, directory.path().to_path_buf());
        picker.path_dirty = true;
        picker.path_input = r"~\Desktop".to_owned();
        assert_eq!(picker.entered_path().unwrap(), picker.home.join("Desktop"));
    }

    fn input() -> egui::RawInput {
        input_size(egui::vec2(760.0, 640.0))
    }

    fn input_size(size: egui::Vec2) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        }
    }

    fn render(ctx: &egui::Context, picker: &mut FilePicker) -> egui::FullOutput {
        ctx.run(input(), |ctx| {
            assert_eq!(picker.show(ctx), None);
        })
    }

    fn text_bounds(output: &egui::FullOutput, label: &str) -> egui::Rect {
        fn find(shape: &egui::Shape, label: &str) -> Option<egui::Rect> {
            match shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(text.galley.rect.translate(text.pos.to_vec2()))
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, label)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|shape| {
                find(&shape.shape, label).filter(|bounds| shape.clip_rect.contains_rect(*bounds))
            })
            .unwrap_or_else(|| panic!("missing fully visible label {label}"))
    }

    fn click(
        ctx: &egui::Context,
        picker: &mut FilePicker,
        point: egui::Pos2,
    ) -> Option<PickerResult> {
        click_at(ctx, picker, point, egui::vec2(760.0, 640.0))
    }

    fn click_at(
        ctx: &egui::Context,
        picker: &mut FilePicker,
        point: egui::Pos2,
        size: egui::Vec2,
    ) -> Option<PickerResult> {
        let mut result = None;
        for pressed in [true, false] {
            let mut raw = input_size(size);
            raw.events.extend([
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            let _ = ctx.run(raw, |ctx| {
                if let Some(value) = picker.show(ctx) {
                    result = Some(value);
                }
            });
        }
        result
    }

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.options_mut(|options| options.max_passes = 1.try_into().unwrap());
        crate::ui_style::install_ui_styles(&ctx);
        ctx
    }

    #[test]
    fn file_dialog_selects_a_file_through_its_visible_controls() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("payload.bin");
        std::fs::write(&file, b"payload").unwrap();
        let ctx = context();
        let mut picker = FilePicker::new(PickerMode::File, directory.path().to_path_buf());
        let _ = render(&ctx, &mut picker);
        let output = render(&ctx, &mut picker);
        assert_eq!(
            click(
                &ctx,
                &mut picker,
                text_bounds(&output, "文件  payload.bin").center()
            ),
            None
        );
        let output = render(&ctx, &mut picker);
        assert_eq!(
            click(
                &ctx,
                &mut picker,
                text_bounds(&output, "确认选择文件").center()
            ),
            Some(PickerResult::Selected(file))
        );
    }

    #[test]
    fn directory_confirmation_and_cancel_return_distinct_results() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().to_path_buf();
        let ctx = context();
        let mut picker = FilePicker::new(PickerMode::Directory, original.clone());
        let _ = render(&ctx, &mut picker);
        let output = render(&ctx, &mut picker);
        assert_eq!(
            click(
                &ctx,
                &mut picker,
                text_bounds(&output, "选择当前文件夹").center()
            ),
            Some(PickerResult::Selected(original.clone()))
        );
        let file = original.join("previous.bin");
        std::fs::write(&file, b"previous selection").unwrap();
        for close_label in ["取消", "关闭"] {
            let ctx = context();
            let mut picker = FilePicker::new(PickerMode::File, file.clone());
            let _ = render(&ctx, &mut picker);
            let output = render(&ctx, &mut picker);
            assert_eq!(
                click(
                    &ctx,
                    &mut picker,
                    text_bounds(&output, close_label).center()
                ),
                Some(PickerResult::Cancelled)
            );
            assert_eq!(picker.current_dir, original);
            assert_eq!(picker.selected, Some(file.clone()));
        }
    }

    #[test]
    fn single_clicked_directory_is_the_confirmed_save_location() {
        let directory = tempfile::tempdir().unwrap();
        let child = directory.path().join("receipts");
        std::fs::create_dir(&child).unwrap();
        let ctx = context();
        let mut picker = FilePicker::new(PickerMode::Directory, directory.path().to_path_buf());
        let _ = render(&ctx, &mut picker);
        let output = render(&ctx, &mut picker);
        assert_eq!(
            click(
                &ctx,
                &mut picker,
                text_bounds(&output, "目录  receipts").center()
            ),
            None
        );
        assert_eq!(picker.current_dir, directory.path());
        let output = render(&ctx, &mut picker);
        assert_eq!(
            click(
                &ctx,
                &mut picker,
                text_bounds(&output, "确认选择文件夹").center()
            ),
            Some(PickerResult::Selected(child))
        );
    }

    #[test]
    fn search_and_hidden_toggle_do_not_confirm_an_invisible_selection() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.txt");
        std::fs::write(&target, b"target").unwrap();
        std::fs::write(directory.path().join("another.txt"), vec![0; 1024]).unwrap();
        std::fs::write(directory.path().join(".private.txt"), b"hidden").unwrap();
        let ctx = context();
        let mut picker = FilePicker::new(PickerMode::File, target.clone());
        assert_eq!(picker.visible_entries.len(), 2);
        let _ = render(&ctx, &mut picker);
        let output = render(&ctx, &mut picker);
        assert_eq!(
            click(
                &ctx,
                &mut picker,
                text_bounds(&output, "搜索当前文件夹").center()
            ),
            None
        );
        let mut typed = input();
        typed.events.push(egui::Event::Text("another".to_owned()));
        let _ = ctx.run(typed, |ctx| assert_eq!(picker.show(ctx), None));
        assert_eq!(picker.visible_entries.len(), 1);
        assert_eq!(
            picker.entries[picker.visible_entries[0]].name,
            "another.txt"
        );
        assert_eq!(picker.entries[picker.visible_entries[0]].size, "1.0 KB");
        assert_eq!(picker.selected, None);
        assert!(!picker.file_selected());
        assert_eq!(picker.confirm(), None);
        picker.refresh();
        assert_eq!(picker.search, "another");
        assert_eq!(picker.visible_entries.len(), 1);

        picker.search.clear();
        picker.update_visible_entries();
        let output = render(&ctx, &mut picker);
        assert_eq!(
            click(&ctx, &mut picker, text_bounds(&output, "隐藏文件").center()),
            None
        );
        assert!(picker.show_hidden);
        assert_eq!(picker.visible_entries.len(), 3);
        picker.selected = Some(directory.path().join(".private.txt"));
        picker.selected_is_file = true;
        let output = render(&ctx, &mut picker);
        assert_eq!(
            click(&ctx, &mut picker, text_bounds(&output, "隐藏文件").center()),
            None
        );
        assert!(!picker.show_hidden);
        assert_eq!(picker.selected, None);
        assert_eq!(picker.confirm(), None);
    }

    #[test]
    fn typed_directory_has_priority_over_the_clicked_directory() {
        let directory = tempfile::tempdir().unwrap();
        let clicked = directory.path().join("clicked");
        let typed = directory.path().join("typed");
        std::fs::create_dir(&clicked).unwrap();
        std::fs::create_dir(&typed).unwrap();
        let mut picker = FilePicker::new(PickerMode::Directory, directory.path().to_path_buf());
        picker.selected = Some(clicked);
        picker.path_input = "typed".to_owned();
        picker.path_dirty = true;
        assert_eq!(picker.confirm(), Some(PickerResult::Selected(typed)));
    }

    #[test]
    fn invalid_or_removed_selections_stay_open_and_repaint_uses_cached_entries() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("payload.bin");
        std::fs::write(&file, b"payload").unwrap();
        let mut picker = FilePicker::new(PickerMode::File, file.clone());
        std::fs::remove_file(&file).unwrap();
        let ctx = context();
        let _ = render(&ctx, &mut picker);
        // Repainting does not reread the directory or lose the original selection.
        assert_eq!(picker.entries.len(), 1);
        assert_eq!(picker.selected, Some(file));
        assert_eq!(picker.confirm(), None);
        assert!(picker.error.is_some());
        picker.path_input = "missing-folder".to_owned();
        picker.path_dirty = true;
        assert_eq!(picker.activate_path(false), None);
        assert_eq!(picker.current_dir, directory.path());
        picker.path_input = "~".to_owned();
        assert_eq!(picker.entered_path().unwrap(), picker.home);
    }

    #[test]
    fn long_scrollable_lists_keep_confirmation_visible_in_small_windows_and_both_themes() {
        let directory = tempfile::tempdir().unwrap();
        for index in 0..128 {
            std::fs::write(
                directory.path().join(format!("payload-{index:03}.bin")),
                b"data",
            )
            .unwrap();
        }
        let selected = directory.path().join("payload-127.bin");
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            for size in [egui::vec2(400.0, 360.0), egui::vec2(640.0, 480.0)] {
                let ctx = context();
                ctx.set_theme(theme);
                let mut picker = FilePicker::new(PickerMode::File, selected.clone());
                picker.error = Some("测试错误不会遮挡确认按钮".to_owned());
                let mut output = ctx.run(input_size(size), |ctx| {
                    assert_eq!(picker.show(ctx), None);
                });
                for _ in 0..2 {
                    output = ctx.run(input_size(size), |ctx| {
                        assert_eq!(picker.show(ctx), None);
                    });
                }
                let confirm = text_bounds(&output, "确认选择文件");
                let cancel = text_bounds(&output, "取消");
                let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                assert!(viewport.contains_rect(confirm));
                assert!(viewport.contains_rect(cancel));
                let first_row = text_bounds(&output, "文件  payload-000.bin");
                let mut scroll = input_size(size);
                scroll.events.extend([
                    egui::Event::PointerMoved(first_row.center()),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -4000.0),
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
                let output = ctx.run(scroll, |ctx| {
                    assert_eq!(picker.show(ctx), None);
                });
                assert_eq!(text_bounds(&output, "确认选择文件"), confirm);
                assert_eq!(text_bounds(&output, "取消"), cancel);
                assert_eq!(
                    click_at(&ctx, &mut picker, confirm.center(), size),
                    Some(PickerResult::Selected(selected.clone()))
                );
            }
        }
    }

    #[test]
    fn resizing_an_open_dialog_keeps_its_title_and_actions_visible_immediately() {
        let directory = tempfile::tempdir().unwrap();
        let selected = directory.path().join("payload.bin");
        std::fs::write(&selected, b"data").unwrap();
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            for mode in [PickerMode::File, PickerMode::Directory] {
                let ctx = context();
                ctx.set_theme(theme);
                let mut picker = FilePicker::new(mode, selected.clone());
                let large = egui::vec2(1152.0, 800.0);
                for _ in 0..3 {
                    let _ = ctx.run(input_size(large), |ctx| assert_eq!(picker.show(ctx), None));
                }
                for size in [egui::vec2(640.0, 480.0), egui::vec2(400.0, 360.0), large] {
                    // Check the first frame after each resize as well as its
                    // settled state; Area's remembered size must not clip either.
                    for _ in 0..3 {
                        let output =
                            ctx.run(input_size(size), |ctx| assert_eq!(picker.show(ctx), None));
                        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                        let title = match mode {
                            PickerMode::File => "选择发送文件",
                            PickerMode::Directory => "选择保存文件夹",
                        };
                        let confirm = match mode {
                            PickerMode::File => "确认选择文件",
                            PickerMode::Directory => "选择当前文件夹",
                        };
                        for label in [title, "关闭", confirm, "取消"] {
                            assert!(
                                viewport.contains_rect(text_bounds(&output, label)),
                                "{label} clipped after resizing to {size:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    // Linux permits native non-UTF8 names; macOS/APFS rejects their creation.
    #[cfg(target_os = "linux")]
    #[test]
    fn native_paths_and_symlinks_remain_usable_without_utf8_conversion() {
        use std::ffi::OsString;
        use std::os::unix::{ffi::OsStringExt, fs::symlink};

        let directory = tempfile::tempdir().unwrap();
        let non_utf8_dir = directory
            .path()
            .join(OsString::from_vec(b"folder-\xff".to_vec()));
        std::fs::create_dir(&non_utf8_dir).unwrap();
        let file = non_utf8_dir.join(OsString::from_vec(b"file-\xfe".to_vec()));
        std::fs::write(&file, b"payload").unwrap();
        symlink(&file, non_utf8_dir.join("linked-file")).unwrap();
        symlink(non_utf8_dir.join("absent"), non_utf8_dir.join("dangling")).unwrap();
        let mut picker = FilePicker::new(PickerMode::File, file.clone());
        assert_eq!(picker.entered_path().unwrap(), non_utf8_dir);
        assert_eq!(picker.confirm(), Some(PickerResult::Selected(file)));
        assert_eq!(
            picker
                .entries
                .iter()
                .find(|entry| entry.name == "linked-file")
                .unwrap()
                .kind,
            EntryKind::File
        );
        assert_eq!(
            picker
                .entries
                .iter()
                .find(|entry| entry.name == "dangling")
                .unwrap()
                .kind,
            EntryKind::Unavailable
        );
    }
}
