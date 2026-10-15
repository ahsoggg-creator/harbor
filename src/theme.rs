//! Catppuccin Mocha look, one accent colour.

use eframe::egui::{self, Color32, CornerRadius, Margin, Response, Stroke, Theme, Ui, vec2};

pub const BASE: Color32 = Color32::from_rgb(30, 30, 46);
pub const MANTLE: Color32 = Color32::from_rgb(24, 24, 37);
pub const SURFACE0: Color32 = Color32::from_rgb(49, 50, 68);
pub const SURFACE1: Color32 = Color32::from_rgb(69, 71, 90);
pub const TEXT: Color32 = Color32::from_rgb(205, 214, 244);
pub const SUBTEXT: Color32 = Color32::from_rgb(166, 173, 200);
pub const ACCENT: Color32 = Color32::from_rgb(203, 166, 247);
pub const BLUE: Color32 = Color32::from_rgb(137, 180, 250);
pub const GOOD: Color32 = Color32::from_rgb(166, 227, 161);
pub const WARN: Color32 = Color32::from_rgb(249, 226, 175);
pub const BAD: Color32 = Color32::from_rgb(243, 139, 168);

pub fn install(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.style_mut_of(Theme::Dark, |s| {
        s.spacing.item_spacing = vec2(8.0, 6.0);
        s.spacing.button_padding = vec2(10.0, 4.0);
        s.spacing.interact_size.y = 26.0;
        let v = &mut s.visuals;
        let r = CornerRadius::same(7);
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(8);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = r;
        }
        v.override_text_color = Some(TEXT);
        v.selection.bg_fill = Color32::from_rgb(88, 70, 120);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.hyperlink_color = BLUE;
        v.panel_fill = BASE;
        v.window_fill = BASE;
        v.extreme_bg_color = MANTLE;
        v.faint_bg_color = MANTLE;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, SURFACE0);
        v.widgets.inactive.weak_bg_fill = SURFACE0;
        v.widgets.inactive.bg_fill = SURFACE0;
        v.widgets.hovered.weak_bg_fill = SURFACE1;
        v.widgets.hovered.bg_fill = SURFACE1;
    });
}

pub fn card(selected: bool) -> egui::Frame {
    egui::Frame::new()
        .fill(MANTLE)
        .stroke(Stroke::new(1.0, if selected { ACCENT } else { SURFACE0 }))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(12))
}

pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).strong().color(MANTLE))
            .fill(ACCENT)
            .min_size(vec2(0.0, 28.0)),
    )
}

pub fn danger_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(egui::Button::new(egui::RichText::new(text).color(BAD)).min_size(vec2(0.0, 26.0)))
}

pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    ui.label(
        egui::RichText::new(format!(" {text} "))
            .small()
            .strong()
            .color(color)
            .background_color(color.gamma_multiply(0.16)),
    );
}

pub fn lang_color(lang: &str) -> Color32 {
    match lang {
        "Rust" => Color32::from_rgb(222, 165, 132),
        "TypeScript" => Color32::from_rgb(49, 120, 198),
        "JavaScript" => Color32::from_rgb(241, 224, 90),
        "Python" => Color32::from_rgb(75, 139, 190),
        "PowerShell" => Color32::from_rgb(90, 140, 210),
        "Luau" | "Lua" => Color32::from_rgb(0, 80, 160),
        "C#" => Color32::from_rgb(23, 134, 0),
        "C++" | "C" => Color32::from_rgb(243, 75, 125),
        "Go" => Color32::from_rgb(0, 173, 216),
        _ => SUBTEXT,
    }
}
