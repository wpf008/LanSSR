//! Shared visual styles and controls for the desktop sender and receiver.

use eframe::egui;

#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub(crate) background: egui::Color32,
    pub(crate) surface: egui::Color32,
    pub(crate) canvas: egui::Color32,
    pub(crate) border: egui::Color32,
    pub(crate) text: egui::Color32,
    pub(crate) muted: egui::Color32,
    pub(crate) accent: egui::Color32,
    pub(crate) accent_soft: egui::Color32,
    pub(crate) error: egui::Color32,
}

impl Palette {
    pub(crate) fn for_theme(theme: egui::Theme) -> Self {
        use egui::Color32 as Color;
        match theme {
            egui::Theme::Light => Self {
                background: Color::from_rgb(245, 247, 251),
                surface: Color::WHITE,
                canvas: Color::from_rgb(234, 238, 245),
                border: Color::from_rgb(218, 225, 236),
                text: Color::from_rgb(30, 42, 61),
                muted: Color::from_rgb(99, 112, 132),
                accent: Color::from_rgb(45, 105, 223),
                accent_soft: Color::from_rgb(225, 235, 253),
                error: Color::from_rgb(193, 57, 57),
            },
            egui::Theme::Dark => Self {
                background: Color::from_rgb(20, 26, 36),
                surface: Color::from_rgb(30, 38, 51),
                canvas: Color::from_rgb(15, 20, 29),
                border: Color::from_rgb(53, 65, 84),
                text: Color::from_rgb(231, 237, 246),
                muted: Color::from_rgb(149, 165, 187),
                accent: Color::from_rgb(71, 132, 244),
                accent_soft: Color::from_rgb(41, 64, 99),
                error: Color::from_rgb(245, 134, 134),
            },
        }
    }
}

pub(crate) fn install_ui_styles(ctx: &egui::Context) {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let colors = Palette::for_theme(theme);
        ctx.style_mut_of(theme, |style| {
            style.spacing.item_spacing = egui::vec2(8.0, 5.0);
            style.spacing.button_padding = egui::vec2(12.0, 7.0);
            style.spacing.interact_size = egui::vec2(40.0, 28.0);
            style
                .text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
            style
                .text_styles
                .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
            style
                .text_styles
                .insert(egui::TextStyle::Small, egui::FontId::proportional(11.5));
            let visuals = &mut style.visuals;
            visuals.panel_fill = colors.background;
            visuals.window_fill = colors.surface;
            visuals.override_text_color = Some(colors.text);
            visuals.weak_text_color = Some(colors.muted);
            visuals.extreme_bg_color = colors.canvas;
            visuals.text_edit_bg_color = Some(colors.background);
            visuals.selection.bg_fill = colors.accent_soft;
            visuals.selection.stroke = egui::Stroke::new(1.0_f32, colors.accent);
            visuals.error_fg_color = colors.error;
            visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, colors.border);
            visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, colors.text);
            for widget in [
                &mut visuals.widgets.inactive,
                &mut visuals.widgets.hovered,
                &mut visuals.widgets.active,
                &mut visuals.widgets.open,
            ] {
                widget.corner_radius = egui::CornerRadius::same(7);
                widget.bg_stroke = egui::Stroke::new(1.0_f32, colors.border);
                widget.fg_stroke = egui::Stroke::new(1.0_f32, colors.text);
            }
            visuals.widgets.inactive.bg_fill = colors.surface;
            visuals.widgets.inactive.weak_bg_fill = colors.surface;
            visuals.widgets.hovered.bg_fill = colors.accent_soft;
            visuals.widgets.hovered.weak_bg_fill = colors.accent_soft;
            visuals.widgets.active.bg_fill = colors.accent_soft;
            visuals.widgets.active.weak_bg_fill = colors.accent_soft;
            visuals.widgets.open.bg_fill = colors.accent_soft;
            visuals.widgets.open.weak_bg_fill = colors.accent_soft;
        });
    }
}

pub(crate) fn card(colors: Palette) -> egui::Frame {
    egui::Frame::NONE
        .fill(colors.surface)
        .stroke(egui::Stroke::new(1.0_f32, colors.border))
        .corner_radius(10)
        .inner_margin(10)
}

pub(crate) fn action_button(label: &str, width: f32) -> egui::Button<'_> {
    egui::Button::new(label)
        .min_size(egui::vec2(width, 32.0))
        .wrap_mode(egui::TextWrapMode::Extend)
}

pub(crate) fn primary_button(label: &str, width: f32) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(label).color(egui::Color32::WHITE))
        .min_size(egui::vec2(width, 32.0))
        .wrap_mode(egui::TextWrapMode::Extend)
        .fill(egui::Color32::from_rgb(45, 105, 223))
        .stroke(egui::Stroke::NONE)
}

pub(crate) fn watermark_switch(ui: &mut egui::Ui, enabled: &mut bool) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(84.0, 28.0), egui::Sense::click());
    if response.clicked() {
        *enabled = !*enabled;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            ui.is_enabled(),
            *enabled,
            "水印",
        )
    });
    if ui.is_rect_visible(rect) {
        let theme = if ui.visuals().dark_mode {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        };
        let colors = Palette::for_theme(theme);
        ui.painter().text(
            rect.left_center(),
            egui::Align2::LEFT_CENTER,
            "水印",
            egui::FontId::proportional(14.0),
            colors.text,
        );
        let track = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 22.0, rect.center().y),
            egui::vec2(44.0, 24.0),
        );
        let amount = ui.ctx().animate_bool(response.id, *enabled);
        ui.painter().rect(
            track,
            12.0,
            if *enabled {
                colors.accent
            } else {
                colors.canvas
            },
            egui::Stroke::new(1.0_f32, colors.border),
            egui::StrokeKind::Inside,
        );
        let center = egui::pos2(
            egui::lerp(track.left() + 12.0..=track.right() - 12.0, amount),
            track.center().y,
        );
        ui.painter()
            .circle_filled(center, 9.0, egui::Color32::WHITE);
        if response.has_focus() {
            ui.painter().rect_stroke(
                track.expand(2.0),
                14.0,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Outside,
            );
        }
    }
    response
}

pub(crate) fn theme_selector(
    ui: &mut egui::Ui,
    current_theme: &mut egui::Theme,
    ctx: &egui::Context,
) {
    let colors = Palette::for_theme(*current_theme);
    egui::Frame::NONE
        .fill(colors.surface)
        .stroke(egui::Stroke::new(1.0_f32, colors.border))
        .corner_radius(9)
        .inner_margin(4)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (theme, label) in [(egui::Theme::Dark, "深色"), (egui::Theme::Light, "浅色")]
                {
                    let selected = *current_theme == theme;
                    let button = egui::Button::new(egui::RichText::new(label).size(12.0).color(
                        if selected {
                            colors.accent
                        } else {
                            colors.muted
                        },
                    ))
                    .fill(if selected {
                        colors.accent_soft
                    } else {
                        colors.surface
                    })
                    .stroke(egui::Stroke::NONE)
                    .min_size(egui::vec2(50.0, 25.0));
                    if ui.add(button).clicked() {
                        *current_theme = theme;
                        ctx.set_theme(theme);
                        ctx.request_repaint();
                    }
                }
            });
        });
}
