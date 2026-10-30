//! App state and logic: clips, cuts, playback, export, messages from workers.

use crate::{config::*, export::*, media::*, theme::*, update::*, util::*};
use eframe::egui::{self, ColorImage, Key, TextureHandle, TextureOptions};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Child;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

// ─── Messages from worker threads ───────────────────────────────────────────
#[derive(Clone, Copy)]
pub(crate) enum DialogKind {
    Info,
    Error,
}

pub(crate) struct Dialog {
    pub(crate) kind: DialogKind,
    pub(crate) title: String,
    pub(crate) body: String,
}

pub(crate) enum Msg {
    Loaded { id: u64, info: MediaInfo },
    Frame { gen: u64, idx: Option<usize>, img: Option<Arc<ColorImage>>, from_request: bool },
    Status(String),
    Progress(f32),
    Indeterminate(bool),
    Dialog(DialogKind, String, String),
    ExportDone { status: String, files: Vec<PathBuf> },
    HwEncoder(Option<&'static str>),
    UpdateAvailable(UpdateInfo),
    UpdateFailed(String),
    UpdateIdle,
    Quit,
}

#[derive(Clone)]
pub(crate) struct Tx {
    pub(crate) tx: Sender<Msg>,
    pub(crate) ctx: egui::Context,
}

impl Tx {
    pub(crate) fn send(&self, m: Msg) {
        let _ = self.tx.send(m);
        self.ctx.request_repaint();
    }
    pub(crate) fn status(&self, s: impl Into<String>) {
        self.send(Msg::Status(s.into()));
    }
    pub(crate) fn error(&self, title: &str, body: impl Into<String>) {
        self.send(Msg::Dialog(DialogKind::Error, title.into(), body.into()));
    }
    pub(crate) fn info(&self, title: &str, body: impl Into<String>) {
        self.send(Msg::Dialog(DialogKind::Info, title.into(), body.into()));
    }
}

// ─── App state ──────────────────────────────────────────────────────────────
/// One loaded video file.
pub(crate) struct Clip {
    pub(crate) id: u64,
    pub(crate) path: PathBuf,
    /// None while ffprobe is still reading it.
    pub(crate) info: Option<MediaInfo>,
    /// The selection last used on this clip, restored when switching back.
    pub(crate) sel: (f64, f64),
}

/// A part of one clip that goes into the final video.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Cut {
    pub(crate) clip: u64,
    pub(crate) start: f64,
    pub(crate) end: f64,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Handle {
    Start,
    End,
}

pub(crate) struct RaskCut {
    pub(crate) tx: Tx,
    pub(crate) rx: Receiver<Msg>,

    pub(crate) start_text: String,
    pub(crate) end_text: String,
    pub(crate) settings: Settings,

    pub(crate) clips: Vec<Clip>,
    /// Index into `clips` of the clip shown in the preview.
    pub(crate) current: Option<usize>,
    pub(crate) next_id: u64,
    pub(crate) title_dirty: bool,

    pub(crate) gen: u64,
    pub(crate) trim_start: f64,
    pub(crate) trim_end: f64,
    pub(crate) trim_drag: Option<Handle>,
    pub(crate) active: Handle,
    pub(crate) last_trim_x: Option<f32>,
    /// All cuts, in clip order then timeline order.
    pub(crate) queue: Vec<Cut>,
    /// Index of the cut being edited live with the handles.
    pub(crate) editing: Option<usize>,

    pub(crate) texture: Option<TextureHandle>,
    pub(crate) logo: Option<TextureHandle>,
    pub(crate) cache: HashMap<usize, Arc<ColorImage>>,
    pub(crate) cache_order: VecDeque<usize>,
    pub(crate) decoding: bool,
    pub(crate) pending: Option<(Option<usize>, f64)>,

    pub(crate) scrub_pending: Option<(f32, Instant)>,
    pub(crate) last_scrub_x: Option<f32>,
    pub(crate) preview_pressed: bool,
    pub(crate) trim_preview_pending: Option<(f64, Instant)>,

    pub(crate) status: String,
    pub(crate) progress: f32,
    pub(crate) indeterminate: bool,
    pub(crate) exporting: bool,

    pub(crate) hw_encoder: Option<&'static str>,
    pub(crate) update: Option<UpdateInfo>,
    pub(crate) updating: bool,

    pub(crate) player: Option<(Child, Child)>,
    /// The running preview is the current selection (not the final video).
    pub(crate) playing_sel: bool,
    pub(crate) clipboard: Option<arboard::Clipboard>,

    pub(crate) dialogs: VecDeque<Dialog>,
    pub(crate) fitted_h: f32,
}

impl Drop for RaskCut {
    fn drop(&mut self) {
        self.stop_player();
    }
}

impl RaskCut {
    pub(crate) fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = load_settings();
        DARK_MODE.store(settings.dark, std::sync::atomic::Ordering::Relaxed);
        setup_fonts(&cc.egui_ctx);
        apply_visuals(&cc.egui_ctx);
        let logo = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).ok().map(|i| {
            let img = ColorImage::from_rgba_unmultiplied([i.width as usize, i.height as usize], &i.rgba);
            cc.egui_ctx.load_texture("logo", img, TextureOptions::LINEAR)
        });
        let (tx, rx) = channel();
        let tx = Tx { tx, ctx: cc.egui_ctx.clone() };

        // Background init: ffmpeg check, HW encoder, update check.
        let t = tx.clone();
        thread::spawn(move || {
            let mut c = command(ffmpeg());
            c.arg("-version");
            if run_capture(c, Duration::from_secs(10)).is_none_or(|o| !o.success) {
                t.error(
                    "ffmpeg not found",
                    "Put ffmpeg.exe, ffprobe.exe and ffplay.exe in the same folder as RaskCut.exe.",
                );
                return;
            }
            t.send(Msg::HwEncoder(detect_hw_encoder()));
        });
        if cfg!(windows) {
            let t = tx.clone();
            thread::spawn(move || {
                if let Some(v) = check_for_update() {
                    t.send(Msg::UpdateAvailable(v));
                }
            });
        }

        let mut app = Self {
            tx,
            rx,
            start_text: String::new(),
            end_text: String::new(),
            settings,
            clips: Vec::new(),
            current: None,
            next_id: 1,
            title_dirty: false,
            gen: 0,
            trim_start: 0.0,
            trim_end: 0.0,
            trim_drag: None,
            active: Handle::Start,
            last_trim_x: None,
            queue: Vec::new(),
            editing: None,
            texture: None,
            logo,
            cache: HashMap::new(),
            cache_order: VecDeque::new(),
            decoding: false,
            pending: None,
            scrub_pending: None,
            last_scrub_x: None,
            preview_pressed: false,
            trim_preview_pending: None,
            status: READY.into(),
            progress: 0.0,
            indeterminate: false,
            exporting: false,
            hw_encoder: None,
            update: None,
            updating: false,
            player: None,
            playing_sel: false,
            clipboard: None,
            dialogs: VecDeque::new(),
            fitted_h: 0.0,
        };
        // "Open with RaskCut" / dropping videos on RaskCut.exe
        let paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
        if !paths.is_empty() {
            app.add_clips(paths);
        }
        app
    }

    // ── Clips ───────────────────────────────────────────────────────────────
    /// The clip shown in the preview, if it has finished loading.
    pub(crate) fn cur(&self) -> Option<(&Clip, &MediaInfo)> {
        let c = self.clips.get(self.current?)?;
        Some((c, c.info.as_ref()?))
    }

    pub(crate) fn cur_id(&self) -> Option<u64> {
        self.current.and_then(|i| self.clips.get(i)).map(|c| c.id)
    }

    pub(crate) fn clip_index(&self, id: u64) -> Option<usize> {
        self.clips.iter().position(|c| c.id == id)
    }

    pub(crate) fn duration(&self) -> f64 {
        self.cur().map(|(_, i)| i.duration).unwrap_or(0.0)
    }

    /// Adds videos (sorted by name, so replay clips land in recorded order)
    /// and shows the first of them.
    pub(crate) fn add_clips(&mut self, mut paths: Vec<PathBuf>) {
        let bad: Vec<String> = paths.iter().filter(|p| !(is_supported(p) && p.is_file())).map(|p| file_name(p)).collect();
        paths.retain(|p| is_supported(p) && p.is_file());
        paths.sort_by_key(|p| file_name(p).to_lowercase());
        if !bad.is_empty() {
            let list = SUPPORTED_FORMATS.map(|e| format!(".{e}")).join(", ");
            self.dialog(DialogKind::Error, "Unsupported file", format!("Skipped: {}\n\nSupported: {list}", bad.join(", ")));
        }
        let mut show = None;
        for p in paths {
            if let Some(i) = self.clips.iter().position(|c| c.path == p) {
                show.get_or_insert(i);
                continue;
            }
            let id = self.next_id;
            self.next_id += 1;
            self.clips.push(Clip { id, path: p.clone(), info: None, sel: (0.0, 0.0) });
            show.get_or_insert(self.clips.len() - 1);
            let tx = self.tx.clone();
            thread::spawn(move || {
                let info = probe_media(&p);
                tx.send(Msg::Loaded { id, info });
            });
        }
        if let Some(i) = show {
            self.switch_clip(i);
        }
    }

    /// Shows another clip, remembering the selection of the current one.
    pub(crate) fn switch_clip(&mut self, i: usize) {
        if i >= self.clips.len() {
            return;
        }
        self.finish_editing();
        if let Some(c) = self.current.and_then(|c| self.clips.get_mut(c)) {
            if c.info.is_some() {
                c.sel = (self.trim_start, self.trim_end);
            }
        }
        self.current = Some(i);
        self.gen += 1;
        self.cache.clear();
        self.cache_order.clear();
        self.decoding = false;
        self.pending = None;
        self.scrub_pending = None;
        self.trim_preview_pending = None;
        self.active = Handle::Start;
        self.title_dirty = true;
        self.enter_clip();
    }

    /// Restores the current clip's selection and preview (once it's loaded).
    pub(crate) fn enter_clip(&mut self) {
        let Some(c) = self.current.and_then(|i| self.clips.get(i)) else { return };
        if c.info.is_none() {
            self.status = "Loading...".into();
            return;
        }
        let sel = c.sel;
        self.trim_start = sel.0;
        self.trim_end = sel.1;
        self.sync_texts();
        self.trim_preview_pending = Some((sel.0, Instant::now()));
        if self.status == "Loading..." {
            self.status = READY.into();
        }
    }

    pub(crate) fn remove_clip(&mut self, i: usize) {
        let Some(c) = self.clips.get(i) else { return };
        let id = c.id;
        self.editing = None;
        self.queue.retain(|q| q.clip != id);
        self.clips.remove(i);
        match self.current {
            Some(cur) if cur == i => {
                self.current = None;
                if self.clips.is_empty() {
                    self.stop_player();
                    self.texture = None;
                    self.title_dirty = true;
                    self.status = READY.into();
                } else {
                    self.switch_clip(i.min(self.clips.len() - 1));
                }
            }
            Some(cur) if cur > i => self.current = Some(cur - 1),
            _ => {}
        }
        self.normalize_cuts();
    }

    pub(crate) fn dialog(&mut self, kind: DialogKind, title: &str, body: impl Into<String>) {
        self.dialogs.push_back(Dialog { kind, title: title.into(), body: body.into() });
    }

    // ── Messages ────────────────────────────────────────────────────────────
    pub(crate) fn process_messages(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Loaded { id, info } => {
                    let Some(i) = self.clip_index(id) else { continue };
                    if info.duration <= 0.0 {
                        let name = file_name(&self.clips[i].path);
                        self.dialog(DialogKind::Error, "Error", format!("Could not read {name}."));
                        self.remove_clip(i);
                        continue;
                    }
                    self.clips[i].sel = (0.0, info.duration);
                    self.clips[i].info = Some(info);
                    if self.current == Some(i) {
                        self.enter_clip();
                    }
                }
                Msg::Frame { gen, idx, img, from_request } => {
                    if gen != self.gen {
                        continue;
                    }
                    if let Some(img) = img {
                        if let Some(i) = idx {
                            self.cache_insert(i, img.clone());
                        }
                        self.show_frame(ctx, &img);
                    }
                    if from_request {
                        self.decoding = false;
                        if let Some((i, t)) = self.pending.take() {
                            self.request_frame(ctx, i, t);
                        }
                    }
                }
                Msg::Status(s) => self.status = s,
                Msg::Progress(p) => self.progress = p.clamp(0.0, 100.0),
                Msg::Indeterminate(b) => self.indeterminate = b,
                Msg::Dialog(k, t, b) => self.dialog(k, &t, b),
                Msg::ExportDone { status, files } => {
                    self.exporting = false;
                    self.indeterminate = false;
                    self.status = status;
                    if self.settings.clipboard && !files.is_empty() {
                        self.copy_files(&files);
                    }
                }
                Msg::HwEncoder(e) => self.hw_encoder = e,
                Msg::UpdateAvailable(u) => self.update = Some(u),
                Msg::UpdateIdle => self.updating = false,
                Msg::UpdateFailed(e) => {
                    self.updating = false;
                    self.dialog(DialogKind::Error, "Update failed", e);
                }
                Msg::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
        if self.title_dirty {
            self.title_dirty = false;
            let title = match self.current.and_then(|i| self.clips.get(i)) {
                Some(c) => format!("RaskCut {APP_VERSION}  \u{b7}  {}", file_name(&c.path)),
                None => format!("RaskCut {APP_VERSION}"),
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
    }

    pub(crate) fn copy_files(&mut self, files: &[PathBuf]) {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        let ok = self.clipboard.as_mut().map(|c| c.set().file_list(files).is_ok()).unwrap_or(false);
        if ok {
            self.status.push_str("  \u{b7}  copied to clipboard");
        } else {
            self.status.push_str("  \u{b7}  clipboard copy failed");
        }
    }

    // ── Preview frames ──────────────────────────────────────────────────────
    pub(crate) fn cache_insert(&mut self, idx: usize, img: Arc<ColorImage>) {
        if self.cache.insert(idx, img).is_none() {
            self.cache_order.push_back(idx);
        }
        while self.cache.len() > THUMB_COUNT {
            match self.cache_order.pop_front() {
                Some(old) => {
                    self.cache.remove(&old);
                }
                None => break,
            }
        }
    }

    pub(crate) fn show_frame(&mut self, ctx: &egui::Context, img: &Arc<ColorImage>) {
        match &mut self.texture {
            Some(t) => t.set(img.clone(), TextureOptions::LINEAR),
            None => {
                self.texture = Some(ctx.load_texture("preview", img.clone(), TextureOptions::LINEAR))
            }
        }
    }

    /// idx = Some: cached scrub thumbnail. None: exact frame, not cached.
    pub(crate) fn request_frame(&mut self, ctx: &egui::Context, idx: Option<usize>, t: f64) {
        if let Some(img) = idx.and_then(|i| self.cache.get(&i).cloned()) {
            self.show_frame(ctx, &img);
            return;
        }
        if self.decoding {
            self.pending = Some((idx, t));
            return;
        }
        let Some((c, _)) = self.cur() else { return };
        let path = c.path.clone();
        self.decoding = true;
        let (gen, tx) = (self.gen, self.tx.clone());
        thread::spawn(move || {
            let img = grab_frame(&path, t).map(Arc::new);
            tx.send(Msg::Frame { gen, idx, img, from_request: true });
        });
    }

    pub(crate) fn do_scrub(&mut self, ctx: &egui::Context, x: f32) {
        let d = self.duration();
        if d <= 0.0 {
            return;
        }
        let frac = (x.clamp(0.0, PW) / PW) as f64;
        let idx = (frac * (THUMB_COUNT - 1) as f64) as usize;
        let t = idx as f64 / (THUMB_COUNT - 1) as f64 * d;
        self.request_frame(ctx, Some(idx), t);
    }

    pub(crate) fn request_preview_at(&mut self, t: f64) {
        if self.duration() > 0.0 {
            self.trim_preview_pending = Some((t, Instant::now() + TRIM_PREVIEW_DELAY));
        }
    }

    pub(crate) fn run_timers(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        if let Some((x, due)) = self.scrub_pending {
            if now >= due {
                self.scrub_pending = None;
                self.do_scrub(ctx, x);
            } else {
                ctx.request_repaint_after(due - now);
            }
        }
        if let Some((t, due)) = self.trim_preview_pending {
            if now >= due {
                self.trim_preview_pending = None;
                // Exact frame so the handle shows precisely what gets cut.
                self.request_frame(ctx, None, t);
            } else {
                ctx.request_repaint_after(due - now);
            }
        }
        // Forget the player once ffplay has been closed.
        if let Some((ff, play)) = &mut self.player {
            if matches!(play.try_wait(), Ok(Some(_))) {
                let _ = ff.kill();
                let _ = ff.wait();
                self.player = None;
                self.playing_sel = false;
            } else {
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }
    }

    // ── Trim ────────────────────────────────────────────────────────────────
    pub(crate) fn sync_texts(&mut self) {
        self.start_text = seconds_to_time(self.trim_start);
        self.end_text = seconds_to_time(self.trim_end);
    }

    pub(crate) fn trim_set_start(&mut self, t: f64) {
        let t = t.min(self.trim_end - 0.1).max(0.0);
        self.trim_start = t;
        self.active = Handle::Start;
        self.start_text = seconds_to_time(t);
        self.request_preview_at(t);
        self.sync_editing();
    }

    pub(crate) fn trim_set_end(&mut self, t: f64) {
        let t = t.min(self.duration()).max(self.trim_start + 0.1);
        self.trim_end = t;
        self.active = Handle::End;
        self.end_text = seconds_to_time(t);
        self.request_preview_at(t);
        self.sync_editing();
    }

    /// While a cut is selected, the handles edit it directly.
    pub(crate) fn sync_editing(&mut self) {
        if let Some(i) = self.editing {
            if let Some(c) = self.queue.get_mut(i) {
                c.start = self.trim_start;
                c.end = self.trim_end;
            }
        }
    }

    pub(crate) fn nudge(&mut self, delta: f64) {
        if self.duration() <= 0.0 {
            return;
        }
        match self.active {
            Handle::Start => self.trim_set_start(self.trim_start + delta),
            Handle::End => self.trim_set_end(self.trim_end + delta),
        }
    }

    pub(crate) fn x_to_time(&self, x: f32) -> f64 {
        (x.clamp(0.0, PW) / PW) as f64 * self.duration()
    }

    pub(crate) fn on_start_entry_change(&mut self) {
        match time_to_seconds(&self.start_text) {
            Some(t) if self.duration() > 0.0 => self.trim_set_start(t),
            _ => self.start_text = seconds_to_time(self.trim_start),
        }
    }

    pub(crate) fn on_end_entry_change(&mut self) {
        match time_to_seconds(&self.end_text) {
            Some(t) if self.duration() > 0.0 => self.trim_set_end(t),
            _ => self.end_text = seconds_to_time(self.trim_end),
        }
    }

    // ── Keyboard: arrows frame, Shift 1 s, Space play, Esc done ─────────────
    pub(crate) fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() || !self.dialogs.is_empty() || self.cur().is_none() {
            return;
        }
        let mut actions: Vec<(Key, bool)> = Vec::new();
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                egui::Event::Key { key, pressed, modifiers, .. }
                    if matches!(key, Key::ArrowLeft | Key::ArrowRight | Key::Space | Key::Escape) =>
                {
                    if *pressed {
                        actions.push((*key, modifiers.shift));
                    }
                    false // consumed, so focused buttons don't react
                }
                _ => true,
            });
        });
        let frame = 1.0 / self.cur().map(|(_, i)| i.fps()).unwrap_or(30.0);
        for (key, shift) in actions {
            let step = if shift { 1.0 } else { frame };
            match key {
                Key::ArrowLeft => self.nudge(-step),
                Key::ArrowRight => self.nudge(step),
                Key::Space => self.toggle_play_selection(),
                Key::Escape => self.finish_editing(),
                _ => {}
            }
        }
    }

    // ── Actions ─────────────────────────────────────────────────────────────
    pub(crate) fn browse_files(&mut self) {
        let picked = rfd::FileDialog::new()
            .set_title("Add clips")
            .set_directory(videos_folder())
            .add_filter("Video files", &SUPPORTED_FORMATS)
            .add_filter("All files", &["*"])
            .pick_files();
        if let Some(p) = picked {
            self.add_clips(p);
        }
    }

    pub(crate) fn open_folder(&mut self) {
        match get_output_dir() {
            Ok(d) => {
                if let Err(e) = open_path(&d) {
                    self.dialog(DialogKind::Error, "Error", format!("Could not open: {e}"));
                }
            }
            Err(_) => self.dialog(DialogKind::Info, "Info", "No clips yet."),
        }
    }

    pub(crate) fn stop_player(&mut self) {
        self.playing_sel = false;
        if let Some((mut ff, mut play)) = self.player.take() {
            let _ = play.kill();
            let _ = ff.kill();
            let _ = play.wait();
            let _ = ff.wait();
        }
    }

    pub(crate) fn play_segs(&mut self, segs: Vec<Seg>) {
        self.stop_player();
        self.playing_sel = false;
        if segs.is_empty() {
            return;
        }
        match play_segments(&segs) {
            Ok(p) => self.player = Some(p),
            Err(e) => self.dialog(DialogKind::Error, "Can't play", e),
        }
    }

    pub(crate) fn seg_of(&self, c: &Cut) -> Option<Seg> {
        let clip = self.clips.get(self.clip_index(c.clip)?)?;
        Some(Seg { path: clip.path.clone(), info: clip.info.clone()?, start: c.start, end: c.end })
    }

    /// What Play and Export work on: all cuts if there are any, otherwise the
    /// selection in the current clip.
    pub(crate) fn targets(&mut self) -> Vec<Seg> {
        self.finish_editing();
        if self.queue.is_empty() {
            let Some(id) = self.cur_id() else { return vec![] };
            let sel = Cut { clip: id, start: self.trim_start, end: self.trim_end };
            self.seg_of(&sel).into_iter().collect()
        } else {
            self.queue.iter().filter_map(|c| self.seg_of(c)).collect()
        }
    }

    /// Big Play button: the final video (all cuts, or the selection if none).
    pub(crate) fn toggle_play(&mut self) {
        if self.player.is_some() {
            self.stop_player();
        } else {
            let t = self.targets();
            self.play_segs(t);
        }
    }

    /// Small play button / Space: just the selection you're working on.
    pub(crate) fn toggle_play_selection(&mut self) {
        if self.player.is_some() {
            self.stop_player();
            return;
        }
        let Some(id) = self.cur_id() else { return };
        let sel = Cut { clip: id, start: self.trim_start, end: self.trim_end };
        if let Some(seg) = self.seg_of(&sel) {
            self.play_segs(vec![seg]);
            self.playing_sel = true;
        }
    }

    /// Keeps cuts in clip order, then timeline order, and merges overlapping
    /// cuts in the same clip.
    pub(crate) fn normalize_cuts(&mut self) {
        let order: HashMap<u64, usize> = self.clips.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
        self.queue.retain(|c| order.contains_key(&c.clip));
        self.queue.sort_by(|a, b| {
            (order[&a.clip], a.start).partial_cmp(&(order[&b.clip], b.start)).unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut merged: Vec<Cut> = Vec::new();
        for c in self.queue.drain(..) {
            match merged.last_mut() {
                Some(last) if last.clip == c.clip && c.start <= last.end => last.end = last.end.max(c.end),
                _ => merged.push(c),
            }
        }
        self.queue = merged;
    }

    pub(crate) fn finish_editing(&mut self) {
        if self.editing.take().is_some() {
            self.normalize_cuts();
        }
    }

    pub(crate) fn add_cut(&mut self) {
        let Some(id) = self.cur_id() else { return };
        if self.duration() <= 0.0 {
            return;
        }
        if self.editing.is_some() {
            // The selection IS the cut being edited: confirm it and start fresh.
            self.finish_editing();
            return;
        }
        self.queue.push(Cut { clip: id, start: self.trim_start, end: self.trim_end });
        self.normalize_cuts();
    }

    /// Loads a cut into the handles (switching clip if needed) for editing.
    pub(crate) fn select_cut(&mut self, i: usize) {
        let Some(target) = self.queue.get(i).copied() else { return };
        self.finish_editing();
        // Finishing may have re-sorted or merged; find the cut again.
        let Some(i) = self.queue.iter().position(|c| c.clip == target.clip && c.start <= target.start && c.end >= target.end) else {
            return;
        };
        let cut = self.queue[i];
        if self.cur_id() != Some(cut.clip) {
            if let Some(ci) = self.clip_index(cut.clip) {
                self.switch_clip(ci);
            }
        }
        self.trim_start = cut.start;
        self.trim_end = cut.end;
        self.sync_texts();
        self.active = Handle::Start;
        self.editing = Some(i);
        self.trim_preview_pending = Some((cut.start, Instant::now()));
    }

    pub(crate) fn remove_cut(&mut self, i: usize) {
        self.editing = None;
        if i < self.queue.len() {
            self.queue.remove(i);
        }
        self.normalize_cuts();
    }

    pub(crate) fn export_main(&mut self) {
        if self.cur().is_none() && self.queue.is_empty() {
            self.dialog(DialogKind::Info, "Info", "Add a clip first.");
            return;
        }
        let t = self.targets();
        self.export(t);
    }

    pub(crate) fn export(&mut self, segments: Vec<Seg>) {
        if self.exporting || segments.is_empty() {
            return;
        }
        let params = ExportParams { segments, mode: self.settings.quality };
        save_settings(&self.settings);
        self.exporting = true;
        self.progress = 0.0;
        let (hw, tx) = (self.hw_encoder, self.tx.clone());
        thread::spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| export_thread(params, hw, &tx)));
            let (status, files) = r.unwrap_or_else(|_| {
                tx.error("Error", "Unexpected error during export.");
                (READY.to_string(), vec![])
            });
            tx.send(Msg::ExportDone { status, files });
        });
    }

    pub(crate) fn toggle_theme(&mut self, ctx: &egui::Context) {
        self.settings.dark = !self.settings.dark;
        DARK_MODE.store(self.settings.dark, std::sync::atomic::Ordering::Relaxed);
        apply_visuals(ctx);
        save_settings(&self.settings);
    }

    pub(crate) fn start_update(&mut self) {
        let Some(u) = self.update.clone() else { return };
        self.updating = true;
        let tx = self.tx.clone();
        thread::spawn(move || match do_update(&u) {
            Ok(true) => tx.send(Msg::Quit),
            Ok(false) => tx.send(Msg::UpdateIdle),
            Err(e) => tx.send(Msg::UpdateFailed(e.to_string())),
        });
    }
}
