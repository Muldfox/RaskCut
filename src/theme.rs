//! Colors for dark and light mode, fonts and egui styling.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Stroke, Vec2};
use std::path::PathBuf;
use std::sync::Arc;

// ─── Theme ──────────────────────────────────────────────────────────────────
#[derive(Clone, Copy)]
pub(crate) struct Pal {
    pub(crate) bg: Color32,      // window
    pub(crate) card: Color32,    // panels, dialogs
    pub(crate) field: Color32,   // inputs, secondary buttons, tracks
    pub(crate) hover: Color32,   // hovered secondary
    pub(crate) border: Color32,
    pub(crate) fg: Color32,
    pub(crate) dim: Color32,
    pub(crate) accent: Color32,
    pub(crate) accent_hover: Color32,
    pub(crate) accent_press: Color32,
    pub(crate) green: Color32,
    pub(crate) red: Color32,
    pub(crate) preview: Color32,
    pub(crate) handle: Color32,
    pub(crate) raised: Color32, // selected segment
}

pub(crate) const DARK: Pal = Pal {
    bg: Color32::from_rgb(0x16, 0x17, 0x19),
    card: Color32::from_rgb(0x1e, 0x20, 0x23),
    field: Color32::from_rgb(0x27, 0x29, 0x2d),
    hover: Color32::from_rgb(0x31, 0x34, 0x39),
    border: Color32::from_rgb(0x2e, 0x31, 0x36),
    fg: Color32::from_rgb(0xec, 0xed, 0xef),
    dim: Color32::from_rgb(0x8b, 0x90, 0x98),
    accent: Color32::from_rgb(0x4a, 0x86, 0xe8),
    accent_hover: Color32::from_rgb(0x5d, 0x94, 0xee),
    accent_press: Color32::from_rgb(0x3d, 0x74, 0xd0),
    green: Color32::from_rgb(0x3f, 0xb9, 0x78),
    red: Color32::from_rgb(0xf0, 0x6f, 0x6f),
    preview: Color32::from_rgb(0x0c, 0x0d, 0x0e),
    handle: Color32::from_rgb(0xf4, 0xf5, 0xf6),
    raised: Color32::from_rgb(0x3b, 0x3f, 0x46),
};

pub(crate) const LIGHT: Pal = Pal {
    bg: Color32::from_rgb(0xf4, 0xf4, 0xf2),
    card: Color32::from_rgb(0xff, 0xff, 0xff),
    field: Color32::from_rgb(0xe9, 0xe9, 0xe6),
    hover: Color32::from_rgb(0xdf, 0xdf, 0xdb),
    border: Color32::from_rgb(0xdd, 0xdd, 0xd9),
    fg: Color32::from_rgb(0x1d, 0x1e, 0x20),
    dim: Color32::from_rgb(0x74, 0x77, 0x7c),
    accent: Color32::from_rgb(0x3a, 0x76, 0xd8),
    accent_hover: Color32::from_rgb(0x4a, 0x84, 0xe2),
    accent_press: Color32::from_rgb(0x2f, 0x66, 0xc0),
    green: Color32::from_rgb(0x2f, 0x9e, 0x63),
    red: Color32::from_rgb(0xd6, 0x45, 0x45),
    preview: Color32::from_rgb(0xe4, 0xe4, 0xe1),
    handle: Color32::from_rgb(0xff, 0xff, 0xff),
    raised: Color32::from_rgb(0xff, 0xff, 0xff),
};

pub(crate) static DARK_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub(crate) fn pal() -> Pal {
    if DARK_MODE.load(std::sync::atomic::Ordering::Relaxed) { DARK } else { LIGHT }
}

// ─── Look & feel ────────────────────────────────────────────────────────────
pub(crate) fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

pub(crate) fn setup_fonts(ctx: &egui::Context) {
    // Segoe UI on Windows when available.
    let mut fonts = egui::FontDefinitions::default();
    let mut bold_family = fonts.families[&FontFamily::Proportional].clone();
    if cfg!(windows) {
        let dir = PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into())).join("Fonts");
        if let Ok(bytes) = std::fs::read(dir.join("segoeui.ttf")) {
            fonts.font_data.insert("segoe".into(), Arc::new(egui::FontData::from_owned(bytes)));
            fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "segoe".into());
            bold_family.insert(0, "segoe".into());
        }
        if let Ok(bytes) = std::fs::read(dir.join("seguisb.ttf")) {
            fonts.font_data.insert("segoe_sb".into(), Arc::new(egui::FontData::from_owned(bytes)));
            bold_family.insert(0, "segoe_sb".into());
        }
    }
    fonts.families.insert(bold(), bold_family);
    ctx.set_fonts(fonts);
}

/// Applies the current palette (dark or light) to egui's built-in widgets.
pub(crate) fn apply_visuals(ctx: &egui::Context) {
    let p = pal();
    let dark = DARK_MODE.load(std::sync::atomic::Ordering::Relaxed);
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0_f32, p.border);
    v.window_corner_radius = CornerRadius::same(14);
    v.window_shadow = egui::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(if dark { 110 } else { 40 }) };
    v.popup_shadow = v.window_shadow;
    v.extreme_bg_color = p.field; // text fields
    v.selection.bg_fill = p.accent.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.5_f32, p.accent);
    v.override_text_color = Some(p.fg);
    v.hyperlink_color = p.accent;

    let r = CornerRadius::same(10);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
    v.widgets.noninteractive.corner_radius = r;
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = r;
        w.fg_stroke.color = p.fg;
    }
    v.widgets.inactive.weak_bg_fill = p.field;
    v.widgets.inactive.bg_fill = p.field;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::TRANSPARENT);
    v.widgets.hovered.weak_bg_fill = p.hover;
    v.widgets.hovered.bg_fill = p.hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, p.border);
    v.widgets.active.weak_bg_fill = p.hover;
    v.widgets.active.bg_fill = p.accent;
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, p.accent);
    v.widgets.open.weak_bg_fill = p.field;
    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        s.spacing.interact_size.y = 30.0;
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(13.5));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(13.5));
    });
}
