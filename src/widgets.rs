//! Custom widgets: buttons, icons, segmented control.

use crate::theme::*;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Pos2, Rect, Sense, Stroke, Vec2,
};

// ─── Widgets ────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Ico {
    Play,
    Stop,
    Plus,
    Close,
    Folder,
    Sun,
    Moon,
}

/// Small vector icons, drawn so they look crisp at any size and in both themes.
/// `bg` is the color behind the icon (used to cut out the moon).
pub(crate) fn paint_icon(p: &egui::Painter, ico: Ico, c: Pos2, s: f32, col: Color32, bg: Color32) {
    let line = Stroke::new((s * 0.12).max(1.4), col);
    match ico {
        Ico::Play => {
            let pts = vec![
                Pos2::new(c.x - s * 0.32, c.y - s * 0.42),
                Pos2::new(c.x - s * 0.32, c.y + s * 0.42),
                Pos2::new(c.x + s * 0.42, c.y),
            ];
            p.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
        }
        Ico::Stop => {
            p.rect_filled(Rect::from_center_size(c, Vec2::splat(s * 0.7)), CornerRadius::same(2), col);
        }
        Ico::Plus => {
            let h = s * 0.4;
            p.line_segment([Pos2::new(c.x - h, c.y), Pos2::new(c.x + h, c.y)], line);
            p.line_segment([Pos2::new(c.x, c.y - h), Pos2::new(c.x, c.y + h)], line);
        }
        Ico::Close => {
            let h = s * 0.32;
            p.line_segment([Pos2::new(c.x - h, c.y - h), Pos2::new(c.x + h, c.y + h)], line);
            p.line_segment([Pos2::new(c.x - h, c.y + h), Pos2::new(c.x + h, c.y - h)], line);
        }
        Ico::Folder => {
            let body = Rect::from_min_max(Pos2::new(c.x - s * 0.5, c.y - s * 0.26), Pos2::new(c.x + s * 0.5, c.y + s * 0.4));
            let tab = Rect::from_min_max(Pos2::new(c.x - s * 0.5, c.y - s * 0.4), Pos2::new(c.x - s * 0.05, c.y - s * 0.2));
            p.rect_filled(tab, CornerRadius::same(2), col);
            p.rect_stroke(body, CornerRadius::same(3), Stroke::new((s * 0.1).max(1.4), col), egui::StrokeKind::Inside);
        }
        Ico::Sun => {
            p.circle_filled(c, s * 0.2, col);
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::TAU / 8.0;
                let dir = Vec2::new(a.cos(), a.sin());
                p.line_segment([c + dir * s * 0.33, c + dir * s * 0.46], Stroke::new((s * 0.09).max(1.3), col));
            }
        }
        Ico::Moon => {
            p.circle_filled(c, s * 0.38, col);
            p.circle_filled(c + Vec2::new(s * 0.2, -s * 0.16), s * 0.32, bg);
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Kind {
    Primary,
    Secondary,
    Ghost,
}

pub(crate) fn button(ui: &mut egui::Ui, label: &str, icon: Option<Ico>, size: Vec2, kind: Kind, enabled: bool) -> egui::Response {
    let p = pal();
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let hovered = enabled && resp.hovered();
    let pressed = enabled && resp.is_pointer_button_down_on();
    let (fill, text, font) = match kind {
        Kind::Primary => (
            if !enabled { p.accent.gamma_multiply(0.45) } else if pressed { p.accent_press } else if hovered { p.accent_hover } else { p.accent },
            Color32::WHITE,
            FontId::new(14.0, bold()),
        ),
        Kind::Secondary => (
            if hovered { p.hover } else { p.field },
            if enabled { p.fg } else { p.dim },
            FontId::proportional(13.5),
        ),
        Kind::Ghost => (
            if hovered { p.field } else { Color32::TRANSPARENT },
            if enabled { p.dim } else { p.border },
            FontId::proportional(12.5),
        ),
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(10), fill);
    let galley = painter.layout_no_wrap(label.to_string(), font, text);
    if label.is_empty() {
        if let Some(ico) = icon {
            paint_icon(painter, ico, rect.center(), 12.0, text, fill);
        }
        if hovered {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        return resp;
    }
    let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
    let total = icon_w + galley.size().x;
    let x0 = rect.center().x - total / 2.0;
    if let Some(ico) = icon {
        paint_icon(painter, ico, Pos2::new(x0 + 6.0, rect.center().y), 12.0, text, fill);
    }
    painter.galley(Pos2::new(x0 + icon_w, rect.center().y - galley.size().y / 2.0), galley, text);
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    resp
}

pub(crate) fn icon_button(ui: &mut egui::Ui, ico: Ico, size: f32, filled: bool) -> egui::Response {
    let p = pal();
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let fill = if resp.hovered() { p.hover } else if filled { p.field } else { p.bg };
    ui.painter().rect_filled(rect, CornerRadius::same((size / 2.0) as u8), fill);
    paint_icon(ui.painter(), ico, rect.center(), size * 0.55, p.fg, fill);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    resp
}

pub(crate) fn pill(ui: &mut egui::Ui, text: &str, color: Color32, enabled: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::new(12.0, bold()), Color32::WHITE);
    let size = galley.size() + Vec2::new(22.0, 10.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let fill = if enabled && resp.hovered() { color.gamma_multiply(1.15) } else { color };
    ui.painter().rect_filled(rect, CornerRadius::same(255), fill);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    resp
}

/// Two or more options in one rounded control. Returns the clicked index.
pub(crate) fn segmented(ui: &mut egui::Ui, options: &[&str], selected: usize, width: f32) -> Option<usize> {
    let p = pal();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 32.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(10), p.field);
    let w = width / options.len() as f32;
    let mut clicked = None;
    for (i, opt) in options.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(rect.left() + w * i as f32, rect.top()), Vec2::new(w, 32.0));
        let resp = ui.interact(r, ui.id().with(("seg", i, *opt)), Sense::click());
        let inner = r.shrink(3.0);
        if i == selected {
            ui.painter().rect_filled(inner.translate(Vec2::new(0.0, 1.0)), CornerRadius::same(8), Color32::from_black_alpha(30));
            ui.painter().rect_filled(inner, CornerRadius::same(8), p.raised);
        } else if resp.hovered() {
            ui.painter().rect_filled(inner, CornerRadius::same(8), p.hover);
        }
        let (font, col) = if i == selected { (FontId::new(13.0, bold()), p.fg) } else { (FontId::proportional(13.0), p.dim) };
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *opt, font, col);
        if resp.hovered() && i != selected {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if resp.clicked() && i != selected {
            clicked = Some(i);
        }
    }
    clicked
}
