//! Drawing the main window.

use crate::{app::*, config::*, theme::*, update::*, util::*, widgets::*};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Key,
    Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2,
};
use std::path::PathBuf;
use std::time::Instant;

impl RaskCut {
    // ── UI pieces ───────────────────────────────────────────────────────────
    pub(crate) fn ui_top_bar(&mut self, ui: &mut egui::Ui) {
        let p = pal();
        ui.horizontal(|ui| {
            ui.set_height(30.0);
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::new((logo.id(), Vec2::splat(24.0))).corner_radius(CornerRadius::same(6)));
            }
            ui.add_space(2.0);
            ui.label(RichText::new("RaskCut").font(FontId::new(17.0, bold())).color(p.fg));
            ui.label(RichText::new(format!("v{APP_VERSION}")).size(11.5).color(p.dim));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (ico, tip) = if self.settings.dark { (Ico::Sun, "Light mode") } else { (Ico::Moon, "Dark mode") };
                if icon_button(ui, ico, 30.0, false).on_hover_text(tip).clicked() {
                    let ctx = ui.ctx().clone();
                    self.toggle_theme(&ctx);
                }
                if let Some(u) = self.update.clone() {
                    let verb = if is_installed() && u.installer_url.is_some() { "Update to" } else { "Download" };
                    let text = if self.updating { "Updating...".to_string() } else { format!("{verb} {}", u.version) };
                    if pill(ui, &text, p.green, !self.updating).clicked() {
                        self.start_update();
                    }
                }
            });
        });
    }

    /// The loaded clips as a tidy grid of tiles (3 per row, more rows as
    /// needed, never a scrollbar). Click to show, × to remove, + to add more.
    pub(crate) fn ui_clips(&mut self, ui: &mut egui::Ui) {
        let p = pal();
        let h = 34.0;
        let gap = 6.0;
        let mut switch_to = None;
        let mut remove = None;
        let mut add = false;

        if self.clips.is_empty() {
            ui.horizontal(|ui| {
                let add_w = 112.0;
                let list_w = ui.available_width() - add_w - ui.spacing().item_spacing.x;
                let (r, resp) = ui.allocate_exact_size(Vec2::new(list_w, h), Sense::click());
                ui.painter().rect_filled(r, CornerRadius::same(10), p.field);
                ui.painter().text(
                    Pos2::new(r.left() + 12.0, r.center().y),
                    Align2::LEFT_CENTER,
                    "Drop one or more clips, or add them",
                    FontId::proportional(13.0),
                    p.dim,
                );
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }
                if resp.clicked() {
                    add = true;
                }
                if button(ui, "Add clips", Some(Ico::Plus), Vec2::new(add_w, h), Kind::Secondary, true).clicked() {
                    add = true;
                }
            });
        } else {
            const COLS: usize = 3;
            let tile_w = (ui.available_width() - gap * (COLS as f32 - 1.0)) / COLS as f32;
            let items = self.clips.len() + 1; // + the "Add" tile
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(gap, gap);
                for row in 0..items.div_ceil(COLS) {
                    ui.horizontal(|ui| {
                        for i in row * COLS..((row + 1) * COLS).min(items) {
                            if i == self.clips.len() {
                                if button(ui, "Add clips", Some(Ico::Plus), Vec2::new(tile_w, h), Kind::Ghost, true)
                                    .on_hover_text("Add more clips. Cuts from all clips become one video.")
                                    .clicked()
                                {
                                    add = true;
                                }
                                continue;
                            }
                            let c = &self.clips[i];
                            let current = self.current == Some(i);
                            let has_cuts = self.queue.iter().any(|q| q.clip == c.id);
                            let (r, resp) = ui.allocate_exact_size(Vec2::new(tile_w, h), Sense::click());
                            let x_r = Rect::from_center_size(Pos2::new(r.right() - 15.0, r.center().y), Vec2::splat(22.0));
                            let x_resp = ui.interact(x_r, ui.id().with(("clip_x", c.id)), Sense::click());
                            let painter = ui.painter();
                            let fill = if current { p.raised } else if resp.hovered() { p.hover } else { p.field };
                            painter.rect_filled(r, CornerRadius::same(10), fill);
                            if current {
                                painter.rect_stroke(r, CornerRadius::same(10), Stroke::new(1.5_f32, p.accent), egui::StrokeKind::Inside);
                            }
                            // Number (blue when the clip has cuts)
                            let nc = Pos2::new(r.left() + 20.0, r.center().y);
                            painter.circle_filled(nc, 9.0, if has_cuts { p.accent } else { p.border });
                            let tc = if has_cuts { Color32::WHITE } else { p.fg };
                            painter.text(nc, Align2::CENTER_CENTER, format!("{}", i + 1), FontId::new(11.0, bold()), tc);
                            // Length, right-aligned before the ×
                            let len = match &c.info {
                                Some(info) => {
                                    let t = info.duration.round() as u64;
                                    format!("{}:{:02}", t / 60, t % 60)
                                }
                                None => "...".into(),
                            };
                            let lr = painter.text(Pos2::new(x_r.left() - 2.0, r.center().y), Align2::RIGHT_CENTER, len, FontId::proportional(11.5), p.dim);
                            // Name, shortened to fit what's left
                            let name_x = nc.x + 15.0;
                            let max_w = (lr.left() - 8.0 - name_x).max(10.0);
                            let font = FontId::new(12.5, if current { bold() } else { egui::FontFamily::Proportional });
                            let col = if current { p.fg } else { p.dim };
                            let galley = fit_text(painter, &c.path, font, col, max_w);
                            painter.galley(Pos2::new(name_x, r.center().y - galley.size().y / 2.0), galley, col);
                            if x_resp.hovered() {
                                painter.circle_filled(x_r.center(), 10.0, p.hover);
                            }
                            paint_icon(painter, Ico::Close, x_r.center(), 9.0, if x_resp.hovered() { p.red } else { p.dim }, fill);
                            if resp.hovered() || x_resp.hovered() {
                                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                            }
                            if x_resp.clicked() {
                                remove = Some(i);
                            } else if resp.clicked() && !current {
                                switch_to = Some(i);
                            }
                            x_resp.on_hover_text("Remove clip");
                            resp.on_hover_text(file_name(&c.path));
                        }
                    });
                }
            });
        }
        if add {
            self.browse_files();
        } else if let Some(i) = remove {
            self.remove_clip(i);
        } else if let Some(i) = switch_to {
            self.switch_clip(i);
        }
    }

    pub(crate) fn ui_preview(&mut self, ui: &mut egui::Ui) {
        let p = pal();
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(PW, PH), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let radius = CornerRadius::same(12);
        painter.rect_filled(rect, radius, p.preview);

        let loading = self.current.is_some() && self.cur().is_none();
        if let (Some(tex), true) = (&self.texture, self.cur().is_some()) {
            egui::Image::new((tex.id(), rect.size())).corner_radius(radius).paint_at(ui, rect);
        } else if loading {
            painter.text(rect.center(), Align2::CENTER_CENTER, "Loading...", FontId::proportional(14.0), p.dim);
            return;
        } else {
            let c = rect.center();
            painter.circle_filled(c - Vec2::new(0.0, 22.0), 26.0, p.field);
            paint_icon(&painter, Ico::Plus, c - Vec2::new(0.0, 22.0), 20.0, p.dim, p.field);
            painter.text(c + Vec2::new(0.0, 22.0), Align2::CENTER_CENTER, "Drop your clips here", FontId::new(15.0, bold()), p.fg);
            painter.text(c + Vec2::new(0.0, 44.0), Align2::CENTER_CENTER, "or click to browse", FontId::proportional(12.5), p.dim);
            if resp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            if resp.clicked() {
                self.browse_files();
            }
            return;
        }

        if self.duration() <= 0.0 {
            return;
        }

        // Left press / drag = scrub, left release = set start, right click = set end.
        let pointer = ui.input(|i| i.pointer.latest_pos());
        let primary_down = ui.input(|i| i.pointer.primary_down());
        if resp.is_pointer_button_down_on() && primary_down {
            self.preview_pressed = true;
        }
        if self.preview_pressed {
            if let Some(pos) = pointer {
                let x = (pos.x - rect.left()).clamp(0.0, PW);
                if self.last_scrub_x != Some(x) {
                    self.last_scrub_x = Some(x);
                    self.scrub_pending = Some((x, Instant::now() + SCRUB_DEBOUNCE));
                }
                let label = seconds_to_time(self.x_to_time(x));
                let pill_pos = Pos2::new(
                    (rect.left() + x).clamp(rect.left() + 36.0, rect.right() - 36.0),
                    rect.top() + 20.0,
                );
                let galley = painter.layout_no_wrap(label, FontId::proportional(12.0), Color32::WHITE);
                let r = Rect::from_center_size(pill_pos, galley.size() + Vec2::new(16.0, 8.0));
                painter.rect_filled(r, CornerRadius::same(10), Color32::from_black_alpha(170));
                painter.galley(r.center() - galley.size() / 2.0, galley, Color32::WHITE);
                painter.line_segment(
                    [Pos2::new(rect.left() + x, r.bottom()), Pos2::new(rect.left() + x, rect.bottom())],
                    Stroke::new(1.0_f32, Color32::from_white_alpha(110)),
                );
            }
            if ui.input(|i| i.pointer.primary_released()) || !primary_down {
                self.preview_pressed = false;
                self.last_scrub_x = None;
                if let Some(pos) = pointer {
                    let t = self.x_to_time(pos.x - rect.left());
                    self.trim_set_start(t);
                }
            }
        }
        if resp.secondary_clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let t = self.x_to_time(pos.x - rect.left());
                self.trim_set_end(t);
            }
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        }
    }

    pub(crate) fn ui_trim_bar(&mut self, ui: &mut egui::Ui) {
        let p = pal();
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(PW, 24.0), Sense::click_and_drag());
        let painter = ui.painter_at(rect.expand(8.0));
        let mid = rect.center().y;
        let track = Rect::from_min_max(Pos2::new(rect.left(), mid - 3.0), Pos2::new(rect.right(), mid + 3.0));
        painter.rect_filled(track, CornerRadius::same(3), p.field);

        let d = self.duration();
        if d <= 0.0 {
            return;
        }
        let sx = (self.trim_start / d) as f32 * PW;
        let ex = (self.trim_end / d) as f32 * PW;

        let pointer_x = ui.input(|i| i.pointer.latest_pos()).map(|q| (q.x - rect.left()).clamp(0.0, PW));
        let primary_down = ui.input(|i| i.pointer.primary_down());
        if self.trim_drag.is_none() && resp.is_pointer_button_down_on() && primary_down {
            if let Some(x) = pointer_x {
                let t = self.x_to_time(x);
                if (x - sx).abs() <= (x - ex).abs() {
                    self.trim_drag = Some(Handle::Start);
                    self.trim_set_start(t);
                } else {
                    self.trim_drag = Some(Handle::End);
                    self.trim_set_end(t);
                }
                self.last_trim_x = Some(x);
            }
        } else if let Some(h) = self.trim_drag {
            if !primary_down {
                self.trim_drag = None;
                self.last_trim_x = None;
            } else if let Some(x) = pointer_x {
                if self.last_trim_x != Some(x) {
                    self.last_trim_x = Some(x);
                    let t = self.x_to_time(x);
                    match h {
                        Handle::Start => self.trim_set_start(t),
                        Handle::End => self.trim_set_end(t),
                    }
                }
            }
        }
        let near = |hx: f32| resp.hovered() && pointer_x.is_some_and(|x| (x - hx).abs() <= 9.0);
        if self.trim_drag.is_some() || near(sx) || near(ex) {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }

        let to_x = |t: f64| rect.left() + (t / d) as f32 * PW;
        let cur_id = self.cur_id();
        for (i, c) in self.queue.iter().enumerate() {
            if self.editing == Some(i) || Some(c.clip) != cur_id {
                continue;
            }
            let r = Rect::from_min_max(Pos2::new(to_x(c.start), mid - 3.0), Pos2::new(to_x(c.end), mid + 3.0));
            painter.rect_filled(r, CornerRadius::same(3), p.accent.gamma_multiply(0.35));
        }
        let (sx, ex) = (to_x(self.trim_start), to_x(self.trim_end));
        let sel = Rect::from_min_max(Pos2::new(sx, mid - 3.0), Pos2::new(ex, mid + 3.0));
        painter.rect_filled(sel, CornerRadius::same(3), p.accent);
        for (hx, which) in [(sx, Handle::Start), (ex, Handle::End)] {
            let active = self.active == which;
            let r = Rect::from_center_size(Pos2::new(hx, mid), Vec2::new(10.0, 22.0));
            painter.rect_filled(r.translate(Vec2::new(0.0, 1.0)), CornerRadius::same(5), Color32::from_black_alpha(50));
            painter.rect_filled(r, CornerRadius::same(5), p.handle);
            if active {
                painter.rect_stroke(r, CornerRadius::same(5), Stroke::new(2.0_f32, p.accent), egui::StrokeKind::Inside);
            } else {
                painter.rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0_f32, p.border), egui::StrokeKind::Inside);
            }
        }
    }

    pub(crate) fn ui_controls(&mut self, ui: &mut egui::Ui) {
        let p = pal();
        ui.horizontal(|ui| {
            ui.set_height(32.0);
            let entry = |ui: &mut egui::Ui, text: &mut String| {
                ui.add_sized(
                    [92.0, 32.0],
                    egui::TextEdit::singleline(text)
                        .horizontal_align(Align::Center)
                        .vertical_align(Align::Center)
                        .margin(Vec2::new(6.0, 6.0)),
                )
            };
            ui.label(RichText::new("Start").size(12.5).color(p.dim));
            let r = entry(ui, &mut self.start_text);
            if r.gained_focus() {
                self.active = Handle::Start;
            }
            if r.lost_focus() {
                self.on_start_entry_change();
            }
            ui.add_space(4.0);
            ui.label(RichText::new("End").size(12.5).color(p.dim));
            let r = entry(ui, &mut self.end_text);
            if r.gained_focus() {
                self.active = Handle::End;
            }
            if r.lost_focus() {
                self.on_end_entry_change();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let cur = if self.settings.quality == Quality::Discord { 0 } else { 1 };
                if let Some(i) = segmented(ui, &["Discord", "Lossless"], cur, 200.0) {
                    self.settings.quality = Quality::ALL[i];
                    save_settings(&self.settings);
                }
            });
        });
    }

    pub(crate) fn ui_actions(&mut self, ui: &mut egui::Ui) {
        let p = pal();
        let loaded = self.duration() > 0.0;
        ui.horizontal(|ui| {
            ui.set_height(30.0);
            let text = if loaded {
                format!(
                    "Selection {}   \u{b7}   Arrows: frame, Shift: 1 s, Space: play selection",
                    seconds_to_time((self.trim_end - self.trim_start).max(0.0))
                )
            } else {
                "Add clips, then pick the moments with the handles".to_string()
            };
            match self.editing {
                Some(i) => ui.label(
                    RichText::new(format!("Editing cut {}   \u{b7}   Esc or Add cut when done", i + 1))
                        .size(11.5)
                        .color(p.accent),
                ),
                None => ui.label(RichText::new(text).size(11.5).color(p.dim)),
            };
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if button(ui, "Add cut", Some(Ico::Plus), Vec2::new(104.0, 30.0), Kind::Secondary, loaded)
                    .on_hover_text("Add the selection as a cut. All cuts are played and exported as one video.")
                    .clicked()
                {
                    self.add_cut();
                }
                let (ico, tip) = if self.playing_sel { (Ico::Stop, "Stop") } else { (Ico::Play, "Play the selection (Space)") };
                if button(ui, "", Some(ico), Vec2::new(36.0, 30.0), Kind::Secondary, loaded).on_hover_text(tip).clicked() {
                    self.toggle_play_selection();
                }
            });
        });
    }

    pub(crate) fn ui_cuts(&mut self, ui: &mut egui::Ui) {
        if self.queue.is_empty() {
            return;
        }
        let p = pal();
        let mut action: Option<(char, usize)> = None;
        let mut clear = false;
        let total: f64 = self.queue.iter().map(|c| c.end - c.start).sum();
        let multi = self.clips.len() > 1;
        egui::Frame::NONE
            .fill(p.card)
            .stroke(Stroke::new(1.0_f32, p.border))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new("Cuts").font(FontId::new(13.0, bold())).color(p.fg));
                    let n = self.queue.len();
                    ui.label(RichText::new(format!("{n}  \u{b7}  {}", seconds_to_time(total))).size(12.0).color(p.dim));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if button(ui, "Clear", None, Vec2::new(56.0, 24.0), Kind::Ghost, true).clicked() {
                            clear = true;
                        }
                    });
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical().max_height(132.0).show(ui, |ui| {
                    let w = ui.available_width();
                    for (i, cut) in self.queue.iter().enumerate() {
                        let (s, e) = (cut.start, cut.end);
                        let (row, row_resp) = ui.allocate_exact_size(Vec2::new(w, 30.0), Sense::click());
                        let cy = row.center().y;
                        let play_r = Rect::from_center_size(Pos2::new(row.left() + 16.0, cy), Vec2::splat(26.0));
                        let del_r = Rect::from_center_size(Pos2::new(row.right() - 16.0, cy), Vec2::splat(26.0));
                        let play_resp = ui.interact(play_r, ui.id().with(("play", i)), Sense::click());
                        let del_resp = ui.interact(del_r, ui.id().with(("del", i)), Sense::click());
                        let painter = ui.painter();
                        let editing = self.editing == Some(i);
                        if editing {
                            painter.rect_filled(row, CornerRadius::same(8), p.accent.gamma_multiply(0.18));
                        } else if row_resp.hovered() || play_resp.hovered() || del_resp.hovered() {
                            painter.rect_filled(row, CornerRadius::same(8), p.field);
                        }
                        if play_resp.hovered() {
                            painter.circle_filled(play_r.center(), 12.0, p.hover);
                        }
                        paint_icon(painter, Ico::Play, play_r.center(), 11.0, if play_resp.hovered() { p.accent } else { p.fg }, p.card);
                        if del_resp.hovered() {
                            painter.circle_filled(del_r.center(), 12.0, p.hover);
                        }
                        paint_icon(painter, Ico::Close, del_r.center(), 10.0, if del_resp.hovered() { p.red } else { p.dim }, p.card);
                        painter.text(Pos2::new(row.left() + 38.0, cy), Align2::LEFT_CENTER, format!("{}", i + 1), FontId::proportional(12.5), p.dim);
                        let tcol = if editing { p.accent } else { p.fg };
                        let r1 = painter.text(Pos2::new(row.left() + 58.0, cy), Align2::LEFT_CENTER, seconds_to_time(s), FontId::proportional(13.0), tcol);
                        let ax = r1.right() + 10.0;
                        let astroke = Stroke::new(1.3_f32, p.dim);
                        painter.line_segment([Pos2::new(ax, cy), Pos2::new(ax + 12.0, cy)], astroke);
                        painter.line_segment([Pos2::new(ax + 8.0, cy - 3.5), Pos2::new(ax + 12.0, cy)], astroke);
                        painter.line_segment([Pos2::new(ax + 8.0, cy + 3.5), Pos2::new(ax + 12.0, cy)], astroke);
                        painter.text(Pos2::new(ax + 22.0, cy), Align2::LEFT_CENTER, seconds_to_time(e), FontId::proportional(13.0), tcol);
                        if multi {
                            if let Some(ci) = self.clip_index(cut.clip) {
                                let br = Rect::from_center_size(Pos2::new(row.right() - 112.0, cy), Vec2::new(54.0, 20.0));
                                painter.rect_stroke(br, CornerRadius::same(10), Stroke::new(1.0_f32, p.border), egui::StrokeKind::Inside);
                                painter.text(br.center(), Align2::CENTER_CENTER, format!("Clip {}", ci + 1), FontId::proportional(11.0), p.dim);
                            }
                        }
                        painter.text(Pos2::new(row.right() - 40.0, cy), Align2::RIGHT_CENTER, format!("{:.1} s", e - s), FontId::proportional(12.5), p.dim);
                        if play_resp.hovered() || del_resp.hovered() || row_resp.hovered() {
                            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                        }
                        if play_resp.clicked() {
                            action = Some(('p', i));
                        } else if del_resp.clicked() {
                            action = Some(('x', i));
                        } else if row_resp.clicked() {
                            action = Some(('e', i));
                        }
                        play_resp.on_hover_text("Play this cut");
                        row_resp.on_hover_text("Click to edit this cut with the handles. Click again or press Esc when done.");
                    }
                });
            });
        if clear {
            self.editing = None;
            self.queue.clear();
        }
        match action {
            Some(('p', i)) => {
                if let Some(seg) = self.queue.get(i).and_then(|c| self.seg_of(c)) {
                    self.play_segs(vec![seg]);
                }
            }
            Some(('x', i)) => self.remove_cut(i),
            Some(('e', i)) => {
                if self.editing == Some(i) { self.finish_editing() } else { self.select_cut(i) }
            }
            _ => {}
        }
    }

    pub(crate) fn ui_progress(&mut self, ui: &mut egui::Ui) {
        let p = pal();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(PW, 4.0), Sense::hover());
        let painter = ui.painter_at(rect);
        let r = CornerRadius::same(2);
        painter.rect_filled(rect, r, p.field);
        if self.indeterminate {
            let t = ui.input(|i| i.time) as f32;
            let seg = PW * 0.25;
            let x = (t * 0.7).fract() * (PW + seg) - seg;
            let bar = Rect::from_min_max(
                Pos2::new(rect.left() + x.max(0.0), rect.top()),
                Pos2::new((rect.left() + x + seg).min(rect.right()), rect.bottom()),
            );
            painter.rect_filled(bar, r, p.accent);
            ui.ctx().request_repaint();
        } else if self.progress > 0.0 {
            let mut bar = rect;
            bar.set_width(PW * self.progress / 100.0);
            painter.rect_filled(bar, r, p.accent);
        }
    }

    pub(crate) fn ui_buttons(&mut self, ui: &mut egui::Ui) {
        let p = pal();
        let loaded = self.duration() > 0.0;
        let n = self.queue.len();
        ui.horizontal(|ui| {
            let gap = ui.spacing().item_spacing.x;
            let folder_w = 44.0;
            let play_w = 150.0;
            let export_w = ui.available_width() - folder_w - play_w - gap * 2.0;
            let h = 42.0;

            let playing = self.player.is_some();
            let (plabel, pico) = if playing { ("Stop", Ico::Stop) } else { ("Play", Ico::Play) };
            if button(ui, plabel, Some(pico), Vec2::new(play_w, h), Kind::Secondary, loaded)
                .on_hover_text(if n > 0 { "Play the final video: all cuts back to back" } else { "Play the selection" })
                .clicked()
            {
                self.toggle_play();
            }

            let elabel = if self.exporting { "Exporting..." } else { "Export" };
            if button(ui, elabel, None, Vec2::new(export_w, h), Kind::Primary, loaded && !self.exporting)
                .on_hover_text(if n > 0 { "Export all cuts as one video" } else { "Export the selection" })
                .clicked()
            {
                self.export_main();
            }

            let (r, resp) = ui.allocate_exact_size(Vec2::new(folder_w, h), Sense::click());
            let fill = if resp.hovered() { p.hover } else { p.field };
            ui.painter().rect_filled(r, CornerRadius::same(10), fill);
            paint_icon(ui.painter(), Ico::Folder, r.center(), 18.0, p.fg, fill);
            if resp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            if resp.on_hover_text("Open the clips folder").clicked() {
                self.open_folder();
            }
        });
        ui.horizontal(|ui| {
            if ui
                .checkbox(&mut self.settings.clipboard, RichText::new("Copy to clipboard after export").size(12.0).color(p.dim))
                .changed()
            {
                save_settings(&self.settings);
            }
        });
    }

    pub(crate) fn ui_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = self.dialogs.front() else { return };
        let p = pal();
        let (title, body, kind) = (d.title.clone(), d.body.clone(), d.kind);
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("raskcut_dialog")).show(ctx, |ui| {
            ui.set_width(380.0);
            let color = match kind {
                DialogKind::Info => p.fg,
                DialogKind::Error => p.red,
            };
            ui.label(RichText::new(title).font(FontId::new(16.0, bold())).color(color));
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                ui.label(RichText::new(body).color(p.fg));
            });
            ui.add_space(12.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if button(ui, "OK", None, Vec2::new(88.0, 34.0), Kind::Primary, true).clicked()
                    || ui.input(|i| i.key_pressed(Key::Enter))
                {
                    close = true;
                }
            });
        });
        if close || modal.should_close() {
            self.dialogs.pop_front();
        }
    }

    pub(crate) fn ui_drop_overlay(&self, ctx: &egui::Context) {
        if !ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            return;
        }
        let p = pal();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
        let screen = ctx.content_rect();
        painter.rect_filled(screen, CornerRadius::ZERO, p.bg.gamma_multiply(0.92));
        painter.rect_stroke(screen.shrink(12.0), CornerRadius::same(16), Stroke::new(2.0_f32, p.accent), egui::StrokeKind::Inside);
        painter.text(screen.center(), Align2::CENTER_CENTER, "Drop to add", FontId::new(20.0, bold()), p.fg);
    }
}

impl eframe::App for RaskCut {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_messages(ctx);

        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() {
            self.add_clips(dropped);
        }

        self.handle_keys(ctx);
        self.run_timers(ctx);

        let panel = egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(pal().bg).inner_margin(PAD + 4.0))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 10.0);
                self.ui_top_bar(ui);
                self.ui_clips(ui);
                self.ui_preview(ui);
                self.ui_trim_bar(ui);
                self.ui_controls(ui);
                self.ui_actions(ui);
                self.ui_cuts(ui);
                ui.add_space(2.0);
                self.ui_progress(ui);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(&self.status).size(12.0).color(pal().dim));
                });
                self.ui_buttons(ui);
                ui.min_rect().bottom()
            });

        // Fit the window height to the content.
        let wanted = (panel.inner + PAD + 4.0).ceil();
        if (wanted - self.fitted_h).abs() > 1.0 {
            self.fitted_h = wanted;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(PW + (PAD + 4.0) * 2.0, wanted)));
        }

        self.ui_drop_overlay(ctx);
        self.ui_dialog(ctx);
    }
}

/// The clip's name, shortened in the middle until it fits `max_w` pixels.
fn fit_text(painter: &egui::Painter, path: &std::path::Path, font: FontId, col: Color32, max_w: f32) -> std::sync::Arc<egui::Galley> {
    let mut max_chars = 60;
    loop {
        let g = painter.layout_no_wrap(short_name(path, max_chars), font.clone(), col);
        if g.size().x <= max_w || max_chars <= 4 {
            return g;
        }
        max_chars -= 1;
    }
}
