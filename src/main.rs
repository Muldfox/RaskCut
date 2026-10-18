// RaskCut, Rust port of the Python/Tkinter version.
// Hide the console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use eframe::egui::{
    self, Align, Align2, Color32, ColorImage, CornerRadius, CursorIcon, FontFamily, FontId, Key,
    Layout, Pos2, Rect, RichText, Sense, Stroke, TextureHandle, TextureOptions, Vec2,
};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ─── Version & update ───────────────────────────────────────────────────────
const APP_VERSION: &str = env!("CARGO_PKG_VERSION"); // from Cargo.toml
/// "owner/repo", set automatically when built by GitHub Actions. Local builds
/// have no update check.
const GITHUB_REPO: Option<&str> = option_env!("GITHUB_REPOSITORY");

// ─── Theme ──────────────────────────────────────────────────────────────────
const BG: Color32 = Color32::from_rgb(0x1a, 0x1a, 0x1a);
const BG2: Color32 = Color32::from_rgb(0x22, 0x22, 0x22);
const BG3: Color32 = Color32::from_rgb(0x2c, 0x2c, 0x2c);
const BORDER: Color32 = Color32::from_rgb(0x3a, 0x3a, 0x3a);
const FG: Color32 = Color32::from_rgb(0xe8, 0xe8, 0xe8);
const FG_DIM: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);
const ACCENT: Color32 = Color32::from_rgb(0x3a, 0x7b, 0xd5);
const ACCENT_HOVER: Color32 = Color32::from_rgb(0x4a, 0x8b, 0xe5);
const GREEN: Color32 = Color32::from_rgb(0x27, 0xae, 0x60);
const RED: Color32 = Color32::from_rgb(0xff, 0x6b, 0x6b);
const PREVIEW_BG: Color32 = Color32::from_rgb(0x0a, 0x0a, 0x0a);

// ─── Constants ──────────────────────────────────────────────────────────────
const SUPPORTED_FORMATS: [&str; 7] = ["mp4", "mov", "avi", "mkv", "wmv", "flv", "webm"];
const THUMB_COUNT: usize = 40;
const PREVIEW_W: usize = 580;
const PREVIEW_H: usize = PREVIEW_W * 9 / 16;
const TRIM_H: f32 = 20.0;
const PAD: f32 = 12.0;
const DECODE_TIMEOUT: Duration = Duration::from_secs(15);
const EXPORT_TIMEOUT: Duration = Duration::from_secs(600);
const SCRUB_DEBOUNCE: Duration = Duration::from_millis(30);
const TRIM_PREVIEW_DELAY: Duration = Duration::from_millis(120);
const DISCORD_TARGET_MB: f64 = 9.5;
const DISCORD_HARD_LIMIT: f64 = 9.9;
const DISCORD_AUDIO_KBPS: u64 = 128; // per audio track
const DISCORD_MAX_RETRIES: u32 = 3;
const KEYFRAME_SEARCH_SECS: f64 = 30.0;

const PW: f32 = PREVIEW_W as f32;
const PH: f32 = PREVIEW_H as f32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Quality {
    Discord,
    Lossless,
}

impl Quality {
    const ALL: [Quality; 2] = [Quality::Discord, Quality::Lossless];
    fn name(self) -> &'static str {
        match self {
            Quality::Discord => "Discord",
            Quality::Lossless => "Lossless",
        }
    }
    fn from_name(s: &str) -> Quality {
        if s == "Lossless" { Quality::Lossless } else { Quality::Discord }
    }
}

// ─── Process helpers ────────────────────────────────────────────────────────
macro_rules! args {
    ($($x:expr),* $(,)?) => { vec![$(OsString::from($x)),*] };
}

/// Command that never opens a console window on Windows.
fn command<S: AsRef<std::ffi::OsStr>>(program: S) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c.stdin(Stdio::null());
    c
}

/// Looks for the tool next to RaskCut.exe first, then on PATH.
fn tool_path(name: &str) -> PathBuf {
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join(&file);
            if p.is_file() {
                return p;
            }
        }
    }
    PathBuf::from(file)
}

fn ffmpeg() -> &'static Path {
    static P: OnceLock<PathBuf> = OnceLock::new();
    P.get_or_init(|| tool_path("ffmpeg"))
}

fn ffprobe() -> &'static Path {
    static P: OnceLock<PathBuf> = OnceLock::new();
    P.get_or_init(|| tool_path("ffprobe"))
}

fn ffplay() -> &'static Path {
    static P: OnceLock<PathBuf> = OnceLock::new();
    P.get_or_init(|| tool_path("ffplay"))
}

fn null_out() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

/// Waits for a child; kills it on timeout. None = timed out.
fn wait_timeout(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return Some(st),
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                return child.wait().ok();
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

struct Captured {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Runs a command, captures output. None = could not start or timed out.
fn run_capture(mut c: Command, timeout: Duration) -> Option<Captured> {
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().ok()?;
    let mut so = child.stdout.take()?;
    let mut se = child.stderr.take()?;
    let t_out = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let t_err = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let status = wait_timeout(&mut child, timeout);
    let stdout = t_out.join().unwrap_or_default();
    let stderr = t_err.join().unwrap_or_default();
    status.map(|st| Captured { success: st.success(), stdout, stderr })
}

/// Fresh temp folder for one job.
fn make_temp_dir(tag: &str) -> std::io::Result<PathBuf> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let d = std::env::temp_dir().join(format!("fastcut_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

// ─── Version comparison ─────────────────────────────────────────────────────
fn version_tuple(v: &str) -> Vec<u64> {
    v.trim()
        .split('.')
        .map(|x| x.parse::<u64>())
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|_| vec![0])
}

// ─── HW encoder detection ───────────────────────────────────────────────────
fn detect_hw_encoder() -> Option<&'static str> {
    let mut c = command(ffmpeg());
    c.args(["-hide_banner", "-encoders"]);
    let listed = run_capture(c, Duration::from_secs(15))?;
    let listed = String::from_utf8_lossy(&listed.stdout).to_string();
    for enc in ["h264_nvenc", "h264_amf", "h264_qsv"] {
        if !listed.contains(enc) {
            continue;
        }
        let mut t = command(ffmpeg());
        t.args([
            "-hide_banner", "-y", "-f", "lavfi", "-i", "color=c=black:s=256x256:d=1",
            "-frames:v", "1", "-c:v", enc, "-f", "null", null_out(),
        ]);
        if let Some(r) = run_capture(t, Duration::from_secs(20)) {
            if r.success {
                return Some(enc);
            }
        }
    }
    None
}

// ─── Settings ───────────────────────────────────────────────────────────────
fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn config_file() -> PathBuf {
    home().join(".fastcut_config.json")
}

struct Settings {
    quality: Quality,
    clipboard: bool,
}

fn load_settings() -> Settings {
    let v = std::fs::read_to_string(config_file())
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    Settings {
        quality: v["quality"].as_str().map(Quality::from_name).unwrap_or(Quality::Discord),
        clipboard: v["clipboard"].as_bool().unwrap_or(true),
    }
}

fn save_settings(s: &Settings) {
    let json = serde_json::json!({ "quality": s.quality.name(), "clipboard": s.clipboard }).to_string();
    if let Err(e) = std::fs::write(config_file(), json) {
        eprintln!("Could not save settings: {e}");
    }
}

// ─── Paths ──────────────────────────────────────────────────────────────────
fn videos_folder() -> PathBuf {
    let base = if cfg!(target_os = "macos") { "Movies" } else { "Videos" };
    home().join(base)
}

fn get_output_dir() -> std::io::Result<PathBuf> {
    let d = videos_folder().join("Fastcutvideos");
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

fn open_path(path: &Path) -> std::io::Result<()> {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    command(program).arg(path).spawn().map(|_| ())
}

fn ext_lower(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

fn is_supported(p: &Path) -> bool {
    SUPPORTED_FORMATS.contains(&ext_lower(p).as_str())
}

fn is_mp4_like(ext: &str) -> bool {
    matches!(ext, "mp4" | "mov" | "m4v")
}

/// e.g. "Valorant_00-12-40.mp4" or "Valorant_combined.mp4", with _2, _3... if taken.
fn unique_out(dir: &Path, src: &Path, label: &str, ext: &str) -> PathBuf {
    let stem = src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "clip".into());
    let base = format!("{stem}_{label}");
    let mut p = dir.join(format!("{base}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{base}_{n}.{ext}"));
        n += 1;
    }
    p
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

// ─── Time helpers ───────────────────────────────────────────────────────────
fn time_to_seconds(s: &str) -> Option<f64> {
    let parts: Vec<f64> = s
        .trim()
        .split(':')
        .map(|p| p.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .ok()?;
    match parts.len() {
        3 => Some(parts[0] * 3600.0 + parts[1] * 60.0 + parts[2]),
        2 => Some(parts[0] * 60.0 + parts[1]),
        _ => parts.first().copied(),
    }
}

/// "mm:ss.cc" or "h:mm:ss.cc"
fn seconds_to_time(sec: f64) -> String {
    let cs = (sec.max(0.0) * 100.0).round() as u64;
    let (h, m, s, c) = (cs / 360_000, cs / 6000 % 60, cs / 100 % 60, cs % 100);
    if h > 0 { format!("{h}:{m:02}:{s:02}.{c:02}") } else { format!("{m:02}:{s:02}.{c:02}") }
}

/// "00-12-40" for file names
fn name_time(sec: f64) -> String {
    let t = sec.max(0.0) as u64;
    format!("{:02}-{:02}-{:02}", t / 3600, t / 60 % 60, t % 60)
}

// ─── Probe ──────────────────────────────────────────────────────────────────
#[derive(Clone, Default)]
struct VideoInfo {
    codec: String,
    height: u32,
    fps: f64,
    pix_fmt: String,
}

#[derive(Clone, Default)]
struct MediaInfo {
    duration: f64,
    start_time: f64,
    audio_tracks: usize,
    video: Option<VideoInfo>,
}

impl MediaInfo {
    fn fps(&self) -> f64 {
        self.video.as_ref().map(|v| v.fps).filter(|f| *f > 1.0).unwrap_or(30.0)
    }
}

fn parse_rate(s: &str) -> f64 {
    match s.split_once('/') {
        Some((a, b)) => {
            let (a, b) = (a.parse::<f64>().unwrap_or(0.0), b.parse::<f64>().unwrap_or(0.0));
            if b > 0.0 { a / b } else { 0.0 }
        }
        None => s.parse().unwrap_or(0.0),
    }
}

fn json_f64(v: &serde_json::Value) -> Option<f64> {
    v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_f64())
}

fn parse_ffmpeg_duration(stderr: &str) -> Option<f64> {
    let i = stderr.find("Duration: ")? + "Duration: ".len();
    let rest = &stderr[i..];
    let end = rest.find(',').unwrap_or(rest.len());
    let p: Vec<f64> = rest[..end]
        .trim()
        .split(':')
        .map(|x| x.parse::<f64>())
        .collect::<Result<_, _>>()
        .ok()?;
    (p.len() == 3).then(|| p[0] * 3600.0 + p[1] * 60.0 + p[2])
}

fn probe_media(path: &Path) -> MediaInfo {
    let mut c = command(ffprobe());
    c.args([
        "-v", "error", "-show_entries",
        "format=duration,start_time:stream=codec_type,codec_name,height,pix_fmt,avg_frame_rate,r_frame_rate:stream_disposition=attached_pic",
        "-of", "json",
    ])
    .arg(path);
    if let Some(out) = run_capture(c, Duration::from_secs(15)) {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
            let mut info = MediaInfo {
                duration: json_f64(&v["format"]["duration"]).unwrap_or(0.0),
                start_time: json_f64(&v["format"]["start_time"]).unwrap_or(0.0),
                ..Default::default()
            };
            for s in v["streams"].as_array().into_iter().flatten() {
                match s["codec_type"].as_str() {
                    Some("audio") => info.audio_tracks += 1,
                    Some("video") if info.video.is_none() && s["disposition"]["attached_pic"] != 1 => {
                        let mut fps = parse_rate(s["avg_frame_rate"].as_str().unwrap_or(""));
                        if fps <= 1.0 {
                            fps = parse_rate(s["r_frame_rate"].as_str().unwrap_or(""));
                        }
                        info.video = Some(VideoInfo {
                            codec: s["codec_name"].as_str().unwrap_or("").to_string(),
                            height: s["height"].as_u64().unwrap_or(0) as u32,
                            fps,
                            pix_fmt: s["pix_fmt"].as_str().unwrap_or("").to_string(),
                        });
                    }
                    _ => {}
                }
            }
            if info.duration > 0.0 {
                return info;
            }
        }
    }
    // Fallback: parse `ffmpeg -i` output (limited info, no smart cut).
    let mut c = command(ffmpeg());
    c.arg("-hide_banner").arg("-i").arg(path);
    let mut info = MediaInfo::default();
    if let Some(out) = run_capture(c, Duration::from_secs(15)) {
        let err = String::from_utf8_lossy(&out.stderr);
        info.duration = parse_ffmpeg_duration(&err).unwrap_or(0.0);
        info.audio_tracks = err.matches("Audio:").count();
        let fps = err
            .lines()
            .find(|l| l.contains("Video:"))
            .and_then(|l| l.split(',').find_map(|p| p.trim().strip_suffix(" fps")?.parse::<f64>().ok()))
            .unwrap_or(0.0);
        info.video = Some(VideoInfo { fps, ..Default::default() });
    }
    info
}

/// First keyframe at/after `start` (relative seconds), plus a safe pre-roll
/// margin (less than the gap to the keyframe before it). k = None means no
/// keyframe inside the clip. Err = could not tell.
fn probe_keyframe(path: &Path, info: &MediaInfo, start: f64, end: f64) -> Result<(Option<f64>, f64), ()> {
    let half_frame = 0.5 / info.fps();
    let from = (start - 2.0).max(0.0);
    let span = (end - from + 1.0).min(KEYFRAME_SEARCH_SECS);
    let mut c = command(ffprobe());
    c.args(args![
        "-v", "error", "-select_streams", "V:0", "-skip_frame", "nokey",
        "-show_entries", "frame=pts_time,best_effort_timestamp_time",
        "-read_intervals", format!("{:.3}%+{:.3}", from + info.start_time, span),
        "-of", "json", path,
    ]);
    let out = run_capture(c, Duration::from_secs(30)).ok_or(())?;
    if !out.success {
        return Err(());
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|_| ())?;
    let mut times: Vec<f64> = v["frames"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| json_f64(&f["pts_time"]).or_else(|| json_f64(&f["best_effort_timestamp_time"])))
        .map(|t| t - info.start_time)
        .collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pos = times.iter().position(|t| *t >= start - half_frame);
    let k = pos.map(|i| times[i]);
    let prev = pos.and_then(|i| i.checked_sub(1)).map(|i| times[i]);
    let margin = match (k, prev) {
        (Some(k), Some(p)) => ((k - p) * 0.5).min(0.5),
        _ => 0.5,
    };
    match k {
        Some(k) if k < end - half_frame => Ok((Some(k), margin)),
        Some(_) => Ok((None, margin)),
        None if from + span >= end => Ok((None, margin)),
        None => Err(()),
    }
}

// ─── Frame grabbing ─────────────────────────────────────────────────────────
/// Decodes one exact frame straight to RGBA.
fn grab_frame(path: &Path, t: f64) -> Option<ColorImage> {
    let (w, h) = (PREVIEW_W, PREVIEW_H);
    let vf = format!(
        "scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2"
    );
    let mut c = command(ffmpeg());
    c.args(args![
        "-hide_banner", "-loglevel", "error", "-ss", format!("{t:.3}"), "-i", path,
        "-map", "0:V:0", "-vf", vf, "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1",
    ]);
    let out = run_capture(c, DECODE_TIMEOUT)?;
    let expected = w * h * 4;
    if out.stdout.len() < expected {
        return None;
    }
    Some(ColorImage::from_rgba_unmultiplied([w, h], &out.stdout[..expected]))
}

// ─── Play selection (ffmpeg -> ffplay, all audio tracks mixed) ─────────────
/// Plays one or more segments back to back, all audio tracks mixed.
fn play_segments(path: &Path, info: &MediaInfo, segs: &[(f64, f64)]) -> Result<(Child, Child), String> {
    if !ffplay().is_file() && which_fails(ffplay()) {
        return Err("Put ffplay.exe in the same folder as RaskCut.exe.".into());
    }
    let n = segs.len();
    let tracks = info.audio_tracks;
    let mut graph = String::new();
    let mut concat_in = String::new();
    for i in 0..n {
        concat_in.push_str(&format!("[{i}:v:0]"));
        match tracks {
            0 => {}
            1 => concat_in.push_str(&format!("[{i}:a:0]")),
            k => {
                for t in 0..k {
                    graph.push_str(&format!("[{i}:a:{t}]"));
                }
                graph.push_str(&format!("amix=inputs={k}:duration=longest:normalize=0[m{i}];"));
                concat_in.push_str(&format!("[m{i}]"));
            }
        }
    }
    let a = usize::from(tracks > 0);
    graph.push_str(&format!("{concat_in}concat=n={n}:v=1:a={a}[vc]"));
    if a == 1 {
        graph.push_str("[a]");
    }
    graph.push_str(";[vc]scale='min(960,iw)':-2[v]");

    let mut c = command(ffmpeg());
    c.args(["-hide_banner", "-loglevel", "error"]);
    c.args(segment_inputs(path, segs));
    c.args(args!["-filter_complex", graph, "-map", "[v]"]);
    if a == 1 {
        c.args(["-map", "[a]", "-c:a", "pcm_s16le"]);
    } else {
        c.arg("-an");
    }
    c.args(["-c:v", "rawvideo", "-pix_fmt", "yuv420p", "-f", "nut", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut ff = c.spawn().map_err(|e| format!("Could not start ffmpeg: {e}"))?;
    let pipe = ff.stdout.take().ok_or("no pipe")?;
    let mut p = command(ffplay());
    p.args(["-hide_banner", "-loglevel", "error", "-autoexit", "-window_title", "RaskCut preview", "-i", "pipe:0"])
        .stdin(Stdio::from(pipe))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match p.spawn() {
        Ok(play) => Ok((ff, play)),
        Err(e) => {
            let _ = ff.kill();
            Err(format!("Could not start ffplay: {e}"))
        }
    }
}

/// One accurate-seek input per segment: -ss s -t d -i path (repeated).
fn segment_inputs(path: &Path, segs: &[(f64, f64)]) -> Vec<OsString> {
    let mut a = Vec::new();
    for &(s, e) in segs {
        a.extend(args!["-ss", format!("{s:.3}"), "-t", format!("{:.3}", e - s), "-i", path]);
    }
    a
}

/// True if a bare tool name can't be run from PATH either.
fn which_fails(p: &Path) -> bool {
    let mut c = command(p);
    c.arg("-version");
    run_capture(c, Duration::from_secs(5)).is_none()
}

// ─── Messages from worker threads ───────────────────────────────────────────
#[derive(Clone, Copy)]
enum DialogKind {
    Info,
    Error,
}

struct Dialog {
    kind: DialogKind,
    title: String,
    body: String,
}

enum Msg {
    Loaded { gen: u64, path: PathBuf, info: MediaInfo },
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
struct Tx {
    tx: Sender<Msg>,
    ctx: egui::Context,
}

impl Tx {
    fn send(&self, m: Msg) {
        let _ = self.tx.send(m);
        self.ctx.request_repaint();
    }
    fn status(&self, s: impl Into<String>) {
        self.send(Msg::Status(s.into()));
    }
    fn error(&self, title: &str, body: impl Into<String>) {
        self.send(Msg::Dialog(DialogKind::Error, title.into(), body.into()));
    }
    fn info(&self, title: &str, body: impl Into<String>) {
        self.send(Msg::Dialog(DialogKind::Info, title.into(), body.into()));
    }
}

/// One clip inside an export run (maps local progress into the total bar).
struct Job {
    tx: Tx,
    prefix: String,
    base: f32,
    span: f32,
}

impl Job {
    fn status(&self, s: &str) {
        self.tx.status(format!("{}{s}", self.prefix));
    }
    fn progress(&self, local: f32) {
        self.tx.send(Msg::Progress(self.base + self.span * local.clamp(0.0, 100.0) / 100.0));
    }
    fn error(&self, title: &str, body: impl Into<String>) {
        self.tx.error(&format!("{}{title}", self.prefix), body);
    }
}

// ─── Encoding ───────────────────────────────────────────────────────────────
/// Picks resolution/fps from the available bitrate. Never upscales.
fn discord_profile(kbps: u64, v: Option<&VideoInfo>) -> (Option<String>, String) {
    let src_h = v.map(|v| v.height).unwrap_or(0);
    let src_fps = v.map(|v| v.fps).unwrap_or(0.0);
    let (h, f) = if kbps >= 4000 && src_fps >= 50.0 {
        (720, 60)
    } else if kbps >= 1500 {
        (720, 30)
    } else if kbps >= 700 {
        (480, 30)
    } else {
        (360, 30)
    };
    let mut filters = Vec::new();
    let out_h = if src_h == 0 || src_h > h {
        filters.push(format!("scale=-2:{h}"));
        h
    } else {
        src_h
    };
    // Always set fps: keeps both x264 passes on identical frame counts.
    let out_f = if src_fps <= 1.0 || src_fps > f as f64 + 0.5 {
        filters.push(format!("fps={f}"));
        f
    } else {
        filters.push(format!("fps={src_fps:.3}"));
        src_fps.round() as u32
    };
    (Some(filters.join(",")), format!("{out_h}p{out_f}"))
}

/// Video/audio mapping for Discord. One segment: plain maps. Several: concat
/// filter that keeps every audio track as its own track. Both passes use the
/// same graph so x264 sees identical frames.
fn discord_streams(n: usize, tracks: usize, vf: &Option<String>, with_audio: bool) -> Vec<OsString> {
    let mut a = Vec::new();
    if n == 1 {
        a.extend(args!["-map", "0:V:0"]);
        if let Some(vf) = vf {
            a.extend(args!["-vf", vf, "-fps_mode", "passthrough"]);
        }
        if with_audio && tracks > 0 {
            a.extend(args!["-map", "0:a"]);
        }
    } else {
        let mut g = String::new();
        for i in 0..n {
            g.push_str(&format!("[{i}:v:0]"));
            for t in 0..tracks {
                g.push_str(&format!("[{i}:a:{t}]"));
            }
        }
        g.push_str(&format!("concat=n={n}:v=1:a={tracks}[vc]"));
        for t in 0..tracks {
            g.push_str(&format!("[a{t}]"));
        }
        g.push_str(&format!(";[vc]{}[vout]", vf.clone().unwrap_or_else(|| "null".into())));
        a.extend(args!["-filter_complex", g, "-map", "[vout]", "-fps_mode", "passthrough"]);
        for t in 0..tracks {
            a.extend(args!["-map", format!("[a{t}]")]);
        }
    }
    if tracks > 0 && (with_audio || n > 1) {
        a.extend(args!["-c:a", "aac", "-b:a", format!("{DISCORD_AUDIO_KBPS}k")]);
    } else {
        a.extend(args!["-an"]);
    }
    a
}

fn build_discord_gpu_cmd(
    path: &Path, segs: &[(f64, f64)], out: &Path, kbps: u64, tracks: usize, vf: &Option<String>, enc: &str,
) -> Vec<OsString> {
    let mut a = args!["-y"];
    a.extend(segment_inputs(path, segs));
    a.extend(discord_streams(segs.len(), tracks, vf, true));
    match enc {
        "h264_nvenc" => a.extend(args![
            "-c:v", "h264_nvenc", "-rc", "cbr", "-b:v", format!("{kbps}k"),
            "-maxrate", format!("{kbps}k"), "-bufsize", format!("{}k", kbps * 2),
        ]),
        "h264_amf" => a.extend(args!["-c:v", "h264_amf", "-rc", "cbr", "-b:v", format!("{kbps}k")]),
        _ => a.extend(args!["-c:v", enc, "-b:v", format!("{kbps}k")]),
    }
    a.extend(args!["-pix_fmt", "yuv420p", "-movflags", "+faststart", out]);
    a
}

#[allow(clippy::too_many_arguments)]
fn build_discord_cpu_cmd(
    path: &Path, segs: &[(f64, f64)], out: &Path, kbps: u64, tracks: usize, vf: &Option<String>,
    passnum: u8, passlog: &Path,
) -> Vec<OsString> {
    let mut a = args!["-y"];
    a.extend(segment_inputs(path, segs));
    a.extend(discord_streams(segs.len(), tracks, vf, passnum == 2));
    a.extend(args![
        "-c:v", "libx264", "-b:v", format!("{kbps}k"),
        "-pass", passnum.to_string(), "-passlogfile", passlog, "-preset", "veryfast", "-pix_fmt", "yuv420p",
    ]);
    if passnum == 1 {
        a.extend(args!["-f", "null", null_out()]);
    } else {
        a.extend(args!["-movflags", "+faststart", out]);
    }
    a
}

/// Runs one ffmpeg pass with live progress. Returns (exit code, stderr).
/// -1 = timed out, -2 = could not start ffmpeg.
fn run_pass(args: Vec<OsString>, total_us: f64, offset: f32, scale: f32, job: &Job) -> (i32, String) {
    let mut c = command(ffmpeg());
    c.args(["-progress", "pipe:1", "-nostats"])
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => return (-2, format!("Could not start ffmpeg: {e}")),
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let (tx, base, span) = (job.tx.clone(), job.base, job.span);
    let t_out = thread::spawn(move || {
        let Some(so) = stdout else { return };
        for line in BufReader::new(so).lines().map_while(Result::ok) {
            if let Some(v) = line.strip_prefix("out_time_us=") {
                if let Ok(us) = v.trim().parse::<i64>() {
                    if us >= 0 {
                        let local = offset + scale.min(us as f32 / total_us as f32 * scale);
                        tx.send(Msg::Progress(base + span * local / 100.0));
                    }
                }
            }
        }
    });
    let t_err = thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(mut se) = stderr {
            let _ = se.read_to_end(&mut b);
        }
        String::from_utf8_lossy(&b).to_string()
    });

    let status = wait_timeout(&mut child, EXPORT_TIMEOUT);
    let _ = t_out.join();
    let err = t_err.join().unwrap_or_default();
    match status {
        None => (-1, "timed out".into()),
        Some(st) => (st.code().unwrap_or(-3), err),
    }
}

fn err_tail(err: &str, fallback: &str) -> String {
    let lines: Vec<&str> = err.trim().lines().collect();
    if lines.is_empty() {
        fallback.to_string()
    } else {
        lines[lines.len().saturating_sub(8)..].join("\n")
    }
}

fn size_mb(p: &Path) -> Option<f64> {
    std::fs::metadata(p).ok().map(|m| m.len() as f64 / (1024.0 * 1024.0))
}

// ─── Export ─────────────────────────────────────────────────────────────────
struct ExportParams {
    path: PathBuf,
    info: MediaInfo,
    /// One segment = Export Clip. Several = combined into one video.
    segments: Vec<(f64, f64)>,
    mode: Quality,
}

struct Exported {
    path: PathBuf,
    mb: f64,
    detail: String,
}

const READY: &str = "Ready";

/// Exports the segment(s) as ONE file. Returns (final status, saved files).
fn export_thread(p: ExportParams, hw: Option<&'static str>, tx: &Tx) -> (String, Vec<PathBuf>) {
    if !p.path.is_file() {
        tx.error("Error", "The source video no longer exists.");
        return (READY.into(), vec![]);
    }
    let out_dir = match get_output_dir() {
        Ok(d) => d,
        Err(e) => {
            tx.error("Error", format!("Could not create output folder: {e}"));
            return (READY.into(), vec![]);
        }
    };
    let segs: Vec<(f64, f64)> = p
        .segments
        .iter()
        .map(|&(s, e)| (s.max(0.0), e.min(p.info.duration)))
        .filter(|(s, e)| e - s > 0.05)
        .collect();
    if segs.is_empty() {
        tx.error("Error", "Start must be before end.");
        return (READY.into(), vec![]);
    }
    let combined = segs.len() > 1;
    let label = if combined { "combined".to_string() } else { name_time(segs[0].0) };

    tx.send(Msg::Progress(0.0));
    let t0 = Instant::now();
    let job = Job { tx: tx.clone(), prefix: String::new(), base: 0.0, span: 100.0 };
    let r = match p.mode {
        Quality::Discord => {
            let out = unique_out(&out_dir, &p.path, &label, "mp4");
            export_discord(&p.path, &p.info, &segs, &out, hw, &job)
        }
        Quality::Lossless => {
            let mut ext = ext_lower(&p.path);
            if !SUPPORTED_FORMATS.contains(&ext.as_str()) {
                ext = "mkv".into();
            }
            let out = unique_out(&out_dir, &p.path, &label, &ext);
            export_lossless(&p.path, &p.info, &segs, &out, &job)
        }
    };
    let Some(e) = r else { return (READY.into(), vec![]) };

    let elapsed = t0.elapsed().as_secs_f64();
    let total: f64 = segs.iter().map(|(s, e)| e - s).sum();
    let name = file_name(&e.path);
    let cuts = if combined { format!("\nCuts: {} combined ({})", segs.len(), seconds_to_time(total)) } else { String::new() };
    tx.send(Msg::Progress(100.0));
    tx.info(
        "Done",
        format!("Saved {name}{cuts}\nSize: {:.1} MB\nEncoder: {}\nTime: {elapsed:.1}s", e.mb, e.detail),
    );
    (format!("Saved  {name}  \u{b7}  {:.1} MB  \u{b7}  {elapsed:.1}s", e.mb), vec![e.path])
}

fn export_discord(
    path: &Path, info: &MediaInfo, segs: &[(f64, f64)], out: &Path, hw: Option<&'static str>, job: &Job,
) -> Option<Exported> {
    let dur: f64 = segs.iter().map(|(s, e)| e - s).sum();
    let tracks = info.audio_tracks;
    let target_bits = DISCORD_TARGET_MB * 8.0 * 1024.0 * 1024.0;
    let audio_bits = (DISCORD_AUDIO_KBPS * tracks as u64) as f64 * 1000.0 * dur;
    let mut kbps = (((target_bits - audio_bits) / dur / 1000.0) as i64).max(100) as u64;
    let (vf, res_label) = discord_profile(kbps, info.video.as_ref());
    let total_us = (dur * 1_000_000.0).max(1.0);
    let mut enc_label: Option<String> = None; // Some = finished OK

    if let Some(enc) = hw {
        for attempt in 1..=DISCORD_MAX_RETRIES {
            job.status(&if attempt == 1 {
                format!("Discord {res_label}: GPU encode...")
            } else {
                format!("Discord {res_label}: optimizing size ({attempt}/{DISCORD_MAX_RETRIES})...")
            });
            let _ = std::fs::remove_file(out);
            let cmd = build_discord_gpu_cmd(path, segs, out, kbps, tracks, &vf, enc);
            let (rc, _) = run_pass(cmd, total_us, 0.0, 100.0, job);
            if rc == -1 {
                job.error("Error", "Export timed out.");
                return None;
            }
            if rc != 0 {
                break; // GPU failed, fall back to CPU
            }
            let Some(mb) = size_mb(out) else { break };
            if mb <= DISCORD_HARD_LIMIT {
                enc_label = Some(format!("GPU {enc}"));
                break;
            }
            kbps = ((kbps as f64 * (DISCORD_TARGET_MB / mb) * 0.97) as u64).max(100);
        }
    }

    if enc_label.is_none() {
        let tmp = match make_temp_dir("pass") {
            Ok(t) => t,
            Err(e) => {
                job.error("Error", format!("Could not create temp folder: {e}"));
                return None;
            }
        };
        let passlog = tmp.join("raskcut_discord_pass");
        let _ = std::fs::remove_file(out);

        job.status(&format!("Discord {res_label}: CPU pass 1 / 2..."));
        let cmd1 = build_discord_cpu_cmd(path, segs, out, kbps, tracks, &vf, 1, &passlog);
        let (rc1, err1) = run_pass(cmd1, total_us, 0.0, 50.0, job);
        if rc1 != 0 {
            let _ = std::fs::remove_dir_all(&tmp);
            match rc1 {
                -1 => job.error("Error", "Export timed out."),
                _ => job.error("Export failed", err_tail(&err1, "Pass 1 failed.")),
            }
            return None;
        }

        job.progress(50.0);
        job.status(&format!("Discord {res_label}: CPU pass 2 / 2..."));
        let cmd2 = build_discord_cpu_cmd(path, segs, out, kbps, tracks, &vf, 2, &passlog);
        let (rc2, err2) = run_pass(cmd2, total_us, 50.0, 50.0, job);
        let _ = std::fs::remove_dir_all(&tmp);

        match rc2 {
            -1 => {
                job.error("Error", "Export timed out.");
                return None;
            }
            0 if out.exists() => enc_label = Some("CPU 2-pass".into()),
            _ => {
                job.error("Export failed", err_tail(&err2, "Unknown ffmpeg error."));
                return None;
            }
        }
    }

    let mb = size_mb(out)?;
    let tracks_note = if tracks > 1 { format!(", {tracks} audio tracks") } else { String::new() };
    Some(Exported {
        path: out.to_path_buf(),
        mb,
        detail: format!("Discord ({}, {res_label}{tracks_note})", enc_label.unwrap_or_default()),
    })
}

/// Lossless: frame-accurate smart cut when possible, keyframe copy otherwise.
fn export_lossless(path: &Path, info: &MediaInfo, segs: &[(f64, f64)], out: &Path, job: &Job) -> Option<Exported> {
    job.tx.send(Msg::Indeterminate(true));
    let r = lossless_inner(path, info, segs, out, job);
    job.tx.send(Msg::Indeterminate(false));
    let detail = r?;
    Some(Exported { path: out.to_path_buf(), mb: size_mb(out)?, detail })
}

fn lossless_inner(path: &Path, info: &MediaInfo, segs: &[(f64, f64)], out: &Path, job: &Job) -> Option<String> {
    let codec = info.video.as_ref().map(|v| v.codec.as_str()).unwrap_or("");
    let tracks_note = if info.audio_tracks > 1 { format!(", {} audio tracks", info.audio_tracks) } else { String::new() };

    if matches!(codec, "h264" | "hevc") {
        match smart_cut(path, info, segs, out, job) {
            Ok(reencoded) if reencoded > 0.0 => {
                return Some(format!(
                    "Lossless, frame-accurate ({reencoded:.2}s re-encoded at cut points, rest stream copy{tracks_note})"
                ))
            }
            Ok(_) => return Some(format!("Lossless, frame-accurate (stream copy{tracks_note})")),
            Err(e) => eprintln!("smart cut failed: {e}"),
        }
        job.status("Smart cut failed, using keyframe cut...");
        return keyframe_cut(path, segs, out, job)
            .map(|_| format!("Lossless, keyframe cut (smart cut failed{tracks_note})"));
    }
    job.status("Exporting (lossless)...");
    let why = if codec.is_empty() { "unknown codec".to_string() } else { format!("{codec} can't be smart cut") };
    keyframe_cut(path, segs, out, job).map(|_| format!("Lossless, keyframe cut ({why}{tracks_note})"))
}

/// Input args for a stream copy that starts at `t`: fast seek to a point well
/// before, then drop everything before `t` on the output side. Demuxer seeking
/// alone can land on an earlier keyframe (e.g. in MKV).
fn exact_seek_input(path: &Path, t: f64) -> Vec<OsString> {
    let t = t.max(0.0);
    let coarse = (t - 30.0).max(0.0);
    args!["-ss", format!("{coarse:.3}"), "-i", path, "-ss", format!("{:.3}", t - coarse)]
}

fn run_ok(a: Vec<OsString>, what: &str, job: &Job) -> Result<(), String> {
    let (rc, err) = run_pass(a, 1.0, 0.0, 0.0, job);
    match rc {
        0 => Ok(()),
        -1 => Err(format!("{what}: timed out")),
        _ => Err(format!("{what}: {}", err_tail(&err, "failed"))),
    }
}

fn mux_flags(a: &mut Vec<OsString>, out: &Path, hevc: bool) {
    if is_mp4_like(&ext_lower(out)) {
        if hevc {
            a.extend(args!["-tag:v", "hvc1"]);
        }
        a.extend(args!["-movflags", "+faststart"]);
    }
}

/// Old behaviour (fallback): each segment starts on the keyframe at or before
/// its start. Several segments are copied separately, then joined.
fn keyframe_cut(path: &Path, segs: &[(f64, f64)], out: &Path, job: &Job) -> Option<()> {
    let r = (|| -> Result<(), String> {
        let one = |s: f64, e: f64, dst: &Path| -> Result<(), String> {
            let mut a = args![
                "-y", "-ss", format!("{s:.3}"), "-i", path, "-t", format!("{:.3}", e - s),
                "-map", "0:V", "-map", "0:a?", "-c", "copy", "-avoid_negative_ts", "make_zero",
            ];
            mux_flags(&mut a, dst, false);
            a.push(dst.into());
            run_ok(a, "Copy", job)
        };
        if segs.len() == 1 {
            return one(segs[0].0, segs[0].1, out);
        }
        let tmp = make_temp_dir("join").map_err(|e| e.to_string())?;
        let r = (|| {
            let mut list = String::new();
            for (i, &(s, e)) in segs.iter().enumerate() {
                job.status(&format!("Copying cut {}/{}...", i + 1, segs.len()));
                one(s, e, &tmp.join(format!("seg{i}.mkv")))?;
                list.push_str(&format!("file 'seg{i}.mkv'\n"));
            }
            std::fs::write(tmp.join("list.txt"), list).map_err(|e| e.to_string())?;
            job.status("Joining...");
            let mut a = args![
                "-y", "-f", "concat", "-safe", "0", "-i", tmp.join("list.txt"),
                "-map", "0:v", "-map", "0:a?", "-c", "copy", "-avoid_negative_ts", "make_zero",
            ];
            mux_flags(&mut a, out, false);
            a.push(out.into());
            run_ok(a, "Join", job)
        })();
        let _ = std::fs::remove_dir_all(&tmp);
        r
    })();
    match r {
        Ok(()) if out.exists() => Some(()),
        Ok(()) => {
            job.error("Export failed", "No output file was created.");
            None
        }
        Err(e) => {
            let _ = std::fs::remove_file(out);
            job.error("Export failed", e);
            None
        }
    }
}

/// Number of video frames whose time is in [from, to), with half a frame of
/// tolerance for timestamp rounding.
fn count_frames(path: &Path, info: &MediaInfo, from: f64, to: f64) -> Option<usize> {
    let hf = 0.5 / info.fps();
    let mut c = command(ffprobe());
    c.args(args![
        "-v", "error", "-select_streams", "V:0", "-show_entries", "packet=pts_time",
        "-read_intervals", format!("{:.3}%{:.3}", (from - 2.0).max(0.0) + info.start_time, to + 1.0 + info.start_time),
        "-of", "csv=p=0", path,
    ]);
    let out = run_capture(c, Duration::from_secs(60))?;
    if !out.success {
        return None;
    }
    let n = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().trim_end_matches(',').parse::<f64>().ok())
        .map(|t| t - info.start_time)
        .filter(|t| *t >= from - hf && *t < to - hf)
        .count();
    Some(n)
}

/// Real length of a video file from its packets (first pts to last pts + duration).
fn video_span(p: &Path, fps: f64) -> Option<f64> {
    let mut c = command(ffprobe());
    c.args(args![
        "-v", "error", "-select_streams", "v:0", "-show_entries", "packet=pts_time,duration_time",
        "-of", "csv=p=0", p,
    ]);
    let out = run_capture(c, Duration::from_secs(60))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for line in text.lines() {
        let mut it = line.split(',');
        let Some(pts) = it.next().and_then(|v| v.trim().parse::<f64>().ok()) else { continue };
        let d = it.next().and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(1.0 / fps);
        lo = lo.min(pts);
        hi = hi.max(pts + d);
    }
    (hi > lo).then_some(hi - lo)
}

/// Frame-accurate lossless cut of one or more segments into ONE file.
/// Per segment:
///   a.ts  = [start, k) re-encoded near-losslessly (only if start isn't a keyframe)
///   b.ts  = [k, end) stream copy, starting on keyframe k
///   audio = all tracks, stream copy, trimmed to exactly match that video
/// Then everything is joined with stream copy. Returns seconds re-encoded.
fn smart_cut(path: &Path, info: &MediaInfo, segs: &[(f64, f64)], out: &Path, job: &Job) -> Result<f64, String> {
    let v = info.video.clone().unwrap_or_default();
    let fps = info.fps();
    let n = segs.len();
    let tmp = make_temp_dir("smart").map_err(|e| e.to_string())?;
    let r = (|| -> Result<f64, String> {
        let mut vlist = String::new();
        let mut alist = String::new();
        let mut reencoded = 0.0;

        for (i, &(start, end)) in segs.iter().enumerate() {
            let tag = if n > 1 { format!("Cut {}/{n}: ", i + 1) } else { String::new() };
            job.status(&format!("{tag}finding keyframe..."));
            let (k, margin) = probe_keyframe(path, info, start, end).map_err(|_| "keyframe probe failed".to_string())?;
            let k = k.unwrap_or(end);
            // No frame between start and the keyframe = start is already on a keyframe.
            let reencode = k - start >= 0.9 / fps;
            let mut seg_len = 0.0;

            if reencode {
                job.status(&format!("{tag}re-encoding first {:.2}s...", k - start));
                // Exact frame count: time-based -t can drop the last frame
                // in containers with millisecond timestamps (MKV).
                let mut a = args![
                    "-y", "-ss", format!("{start:.3}"), "-i", path,
                    "-map", "0:V:0", "-an", "-sn", "-dn", "-fps_mode", "passthrough",
                ];
                match count_frames(path, info, start, k) {
                    Some(nf) if nf > 0 => a.extend(args![
                        "-t", format!("{:.6}", k - start + 1.0 / fps), "-frames:v", nf.to_string()
                    ]),
                    _ => a.extend(args!["-t", format!("{:.6}", k - start)]),
                }
                if v.codec == "hevc" {
                    a.extend(args!["-c:v", "libx265", "-crf", "16", "-preset", "medium", "-x265-params", "log-level=error"]);
                } else {
                    a.extend(args!["-c:v", "libx264", "-crf", "14", "-preset", "medium"]);
                }
                if !v.pix_fmt.is_empty() {
                    a.extend(args!["-pix_fmt", &v.pix_fmt]);
                }
                let f = format!("a{i}.ts");
                a.extend(args!["-f", "mpegts", tmp.join(&f)]);
                run_ok(a, "Re-encode", job)?;
                let len = video_span(&tmp.join(&f), fps).unwrap_or(k - start);
                vlist.push_str(&format!("file '{f}'\nduration {len:.6}\n"));
                seg_len += len;
                reencoded += k - start;
            }

            if k < end {
                job.status(&format!("{tag}copying video..."));
                // Start a little before k: leading non-keyframes are dropped,
                // so the copy begins exactly on keyframe k.
                let f = format!("b{i}.ts");
                let mut b = args!["-y"];
                b.extend(exact_seek_input(path, k - margin));
                b.extend(args![
                    "-t", format!("{:.3}", end - k + margin), "-map", "0:V:0", "-an", "-sn", "-dn",
                    "-c:v", "copy", "-f", "mpegts", tmp.join(&f),
                ]);
                run_ok(b, "Copy", job)?;
                let len = video_span(&tmp.join(&f), fps).unwrap_or(end - k);
                vlist.push_str(&format!("file '{f}'\nduration {len:.6}\n"));
                seg_len += len;
            }

            if info.audio_tracks > 0 {
                job.status(&format!("{tag}copying audio..."));
                // Same length as this segment's video, so sync never drifts.
                let f = format!("aud{i}.mka");
                let mut c = args!["-y"];
                c.extend(exact_seek_input(path, start));
                c.extend(args!["-t", format!("{seg_len:.6}"), "-map", "0:a", "-c", "copy", tmp.join(&f)]);
                run_ok(c, "Audio", job)?;
                alist.push_str(&format!("file '{f}'\nduration {seg_len:.6}\n"));
            }
        }
        std::fs::write(tmp.join("video.txt"), vlist).map_err(|e| e.to_string())?;

        job.status("Joining...");
        let mut a = args!["-y", "-f", "concat", "-safe", "0", "-i", tmp.join("video.txt")];
        if info.audio_tracks > 0 {
            std::fs::write(tmp.join("audio.txt"), alist).map_err(|e| e.to_string())?;
            a.extend(args!["-f", "concat", "-safe", "0", "-i", tmp.join("audio.txt")]);
        }
        a.extend(args!["-map", "0:v"]);
        if info.audio_tracks > 0 {
            a.extend(args!["-map", "1:a"]);
        }
        a.extend(args!["-c", "copy", "-avoid_negative_ts", "make_zero"]);
        mux_flags(&mut a, out, v.codec == "hevc");
        a.push(out.into());
        run_ok(a, "Join", job)?;
        Ok(reencoded)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    if r.is_err() {
        let _ = std::fs::remove_file(out);
    }
    r
}

// ─── Update (GitHub Releases) ───────────────────────────────────────────────
#[derive(Clone)]
struct UpdateInfo {
    version: String,
    installer_url: Option<String>,
    page_url: String,
}

fn check_for_update() -> Option<UpdateInfo> {
    let repo = GITHUB_REPO?;
    let mut c = command("curl");
    c.args(["-sSfL", "--max-time", "15", "-H", "Accept: application/vnd.github+json"])
        .arg(format!("https://api.github.com/repos/{repo}/releases/latest"));
    let out = run_capture(c, Duration::from_secs(20))?;
    if !out.success {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let version = v["tag_name"].as_str()?.trim_start_matches('v').to_string();
    if version_tuple(&version) <= version_tuple(APP_VERSION) {
        return None;
    }
    let installer_url = v["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"].as_str().is_some_and(|n| n.starts_with("RaskCut-Setup-") && n.ends_with(".exe")))
        .and_then(|a| a["browser_download_url"].as_str())
        .map(String::from);
    let page_url = v["html_url"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| format!("https://github.com/{repo}/releases/latest"));
    Some(UpdateInfo { version, installer_url, page_url })
}

/// Installed via the installer (not the portable zip).
fn is_installed() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("unins000.exe").is_file()))
        .unwrap_or(false)
}

/// Installed: download the new installer and run it silently (returns true =
/// quit now). Portable: open the release page in the browser (returns false).
fn do_update(u: &UpdateInfo) -> Result<bool, String> {
    let url = match (&u.installer_url, is_installed()) {
        (Some(url), true) => url.clone(),
        _ => {
            open_path(Path::new(&u.page_url)).map_err(|e| e.to_string())?;
            return Ok(false);
        }
    };
    let tmp = make_temp_dir("update").map_err(|e| e.to_string())?;
    let setup = tmp.join(format!("RaskCut-Setup-{}.exe", u.version));
    let mut c = command("curl");
    c.args(["-sSfL", "--max-time", "600", "-o"]).arg(&setup).arg(&url);
    let out = run_capture(c, Duration::from_secs(620)).ok_or("Download timed out.")?;
    if !out.success || !setup.is_file() {
        return Err(format!("Download failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    command(&setup)
        .args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS"])
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(true)
}

// ─── App state ──────────────────────────────────────────────────────────────
struct Video {
    path: PathBuf,
    info: MediaInfo,
}

#[derive(Clone, Copy, PartialEq)]
enum Handle {
    Start,
    End,
}

struct RaskCut {
    tx: Tx,
    rx: Receiver<Msg>,

    path_text: String,
    start_text: String,
    end_text: String,
    settings: Settings,

    video: Option<Video>,
    gen: u64,
    trim_start: f64,
    trim_end: f64,
    trim_drag: Option<Handle>,
    active: Handle,
    last_trim_x: Option<f32>,
    queue: Vec<(f64, f64)>,

    texture: Option<TextureHandle>,
    cache: HashMap<usize, Arc<ColorImage>>,
    cache_order: VecDeque<usize>,
    decoding: bool,
    pending: Option<(Option<usize>, f64)>,

    scrub_pending: Option<(f32, Instant)>,
    last_scrub_x: Option<f32>,
    preview_pressed: bool,
    trim_preview_pending: Option<(f64, Instant)>,

    status: String,
    progress: f32,
    indeterminate: bool,
    exporting: bool,

    hw_encoder: Option<&'static str>,
    update: Option<UpdateInfo>,
    updating: bool,

    player: Option<(Child, Child)>,
    clipboard: Option<arboard::Clipboard>,

    dialogs: VecDeque<Dialog>,
    fitted_h: f32,
}

impl Drop for RaskCut {
    fn drop(&mut self) {
        self.stop_player();
    }
}

impl RaskCut {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_style(&cc.egui_ctx);
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

        Self {
            tx,
            rx,
            path_text: String::new(),
            start_text: String::new(),
            end_text: String::new(),
            settings: load_settings(),
            video: None,
            gen: 0,
            trim_start: 0.0,
            trim_end: 0.0,
            trim_drag: None,
            active: Handle::Start,
            last_trim_x: None,
            queue: Vec::new(),
            texture: None,
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
            clipboard: None,
            dialogs: VecDeque::new(),
            fitted_h: 0.0,
        }
    }

    fn duration(&self) -> f64 {
        self.video.as_ref().map(|v| v.info.duration).unwrap_or(0.0)
    }

    fn dialog(&mut self, kind: DialogKind, title: &str, body: impl Into<String>) {
        self.dialogs.push_back(Dialog { kind, title: title.into(), body: body.into() });
    }

    // ── Messages ────────────────────────────────────────────────────────────
    fn process_messages(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Loaded { gen, path, info } => {
                    if gen != self.gen {
                        continue;
                    }
                    let duration = info.duration;
                    // Show the loaded file in the window title.
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                        "RaskCut {APP_VERSION}  \u{b7}  {}",
                        file_name(&path)
                    )));
                    self.video = Some(Video { path, info });
                    self.trim_start = 0.0;
                    self.trim_end = duration;
                    self.sync_texts();
                    if duration <= 0.0 {
                        self.dialog(DialogKind::Error, "Error", "Could not read the video duration.");
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
    }

    fn copy_files(&mut self, files: &[PathBuf]) {
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

    // ── Video loading ───────────────────────────────────────────────────────
    fn load_video(&mut self, path: PathBuf) {
        self.scrub_pending = None;
        self.trim_preview_pending = None;
        self.gen += 1;
        self.cache.clear();
        self.cache_order.clear();
        self.decoding = false;
        self.pending = None;
        self.queue.clear();
        self.active = Handle::Start;
        self.stop_player();
        self.status = "Loading...".into();

        let gen = self.gen;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let info = probe_media(&path);
            tx.send(Msg::Loaded { gen, path: path.clone(), info });
            let img = grab_frame(&path, 0.0).map(Arc::new);
            tx.send(Msg::Frame { gen, idx: Some(0), img, from_request: false });
            tx.status(READY);
        });
    }

    fn cache_insert(&mut self, idx: usize, img: Arc<ColorImage>) {
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

    fn show_frame(&mut self, ctx: &egui::Context, img: &Arc<ColorImage>) {
        match &mut self.texture {
            Some(t) => t.set(img.clone(), TextureOptions::LINEAR),
            None => {
                self.texture = Some(ctx.load_texture("preview", img.clone(), TextureOptions::LINEAR))
            }
        }
    }

    /// idx = Some: cached scrub thumbnail. None: exact frame, not cached.
    fn request_frame(&mut self, ctx: &egui::Context, idx: Option<usize>, t: f64) {
        if let Some(img) = idx.and_then(|i| self.cache.get(&i).cloned()) {
            self.show_frame(ctx, &img);
            return;
        }
        if self.decoding {
            self.pending = Some((idx, t));
            return;
        }
        let Some(v) = &self.video else { return };
        self.decoding = true;
        let (path, gen, tx) = (v.path.clone(), self.gen, self.tx.clone());
        thread::spawn(move || {
            let img = grab_frame(&path, t).map(Arc::new);
            tx.send(Msg::Frame { gen, idx, img, from_request: true });
        });
    }

    fn do_scrub(&mut self, ctx: &egui::Context, x: f32) {
        let d = self.duration();
        if d <= 0.0 {
            return;
        }
        let frac = (x.clamp(0.0, PW) / PW) as f64;
        let idx = (frac * (THUMB_COUNT - 1) as f64) as usize;
        let t = idx as f64 / (THUMB_COUNT - 1) as f64 * d;
        self.request_frame(ctx, Some(idx), t);
    }

    fn request_preview_at(&mut self, t: f64) {
        if self.duration() > 0.0 {
            self.trim_preview_pending = Some((t, Instant::now() + TRIM_PREVIEW_DELAY));
        }
    }

    fn run_timers(&mut self, ctx: &egui::Context) {
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
            } else {
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }
    }

    // ── Trim ────────────────────────────────────────────────────────────────
    fn sync_texts(&mut self) {
        self.start_text = seconds_to_time(self.trim_start);
        self.end_text = seconds_to_time(self.trim_end);
    }

    fn trim_set_start(&mut self, t: f64) {
        let t = t.min(self.trim_end - 0.1).max(0.0);
        self.trim_start = t;
        self.active = Handle::Start;
        self.start_text = seconds_to_time(t);
        self.request_preview_at(t);
    }

    fn trim_set_end(&mut self, t: f64) {
        let t = t.min(self.duration()).max(self.trim_start + 0.1);
        self.trim_end = t;
        self.active = Handle::End;
        self.end_text = seconds_to_time(t);
        self.request_preview_at(t);
    }

    fn nudge(&mut self, delta: f64) {
        if self.duration() <= 0.0 {
            return;
        }
        match self.active {
            Handle::Start => self.trim_set_start(self.trim_start + delta),
            Handle::End => self.trim_set_end(self.trim_end + delta),
        }
    }

    fn x_to_time(&self, x: f32) -> f64 {
        (x.clamp(0.0, PW) / PW) as f64 * self.duration()
    }

    fn on_start_entry_change(&mut self) {
        match time_to_seconds(&self.start_text) {
            Some(t) if self.duration() > 0.0 => self.trim_set_start(t),
            _ => self.start_text = seconds_to_time(self.trim_start),
        }
    }

    fn on_end_entry_change(&mut self) {
        match time_to_seconds(&self.end_text) {
            Some(t) if self.duration() > 0.0 => self.trim_set_end(t),
            _ => self.end_text = seconds_to_time(self.trim_end),
        }
    }

    // ── Keyboard: ←/→ frame, Shift 1 s, Space play ─────────────────────────
    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() || !self.dialogs.is_empty() || self.video.is_none() {
            return;
        }
        let mut actions: Vec<(Key, bool)> = Vec::new();
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                egui::Event::Key { key, pressed, modifiers, .. }
                    if matches!(key, Key::ArrowLeft | Key::ArrowRight | Key::Space) =>
                {
                    if *pressed {
                        actions.push((*key, modifiers.shift));
                    }
                    false // consumed, so focused buttons don't react
                }
                _ => true,
            });
        });
        let frame = 1.0 / self.video.as_ref().map(|v| v.info.fps()).unwrap_or(30.0);
        for (key, shift) in actions {
            let step = if shift { 1.0 } else { frame };
            match key {
                Key::ArrowLeft => self.nudge(-step),
                Key::ArrowRight => self.nudge(step),
                Key::Space => self.play(),
                _ => {}
            }
        }
    }

    // ── Actions ─────────────────────────────────────────────────────────────
    fn browse_file(&mut self) {
        let picked = rfd::FileDialog::new()
            .set_title("Select video")
            .set_directory(videos_folder())
            .add_filter("Video files", &SUPPORTED_FORMATS)
            .add_filter("All files", &["*"])
            .pick_file();
        if let Some(p) = picked {
            self.path_text = p.display().to_string();
            self.load_video(p);
        }
    }

    fn on_drop(&mut self, p: PathBuf) {
        if is_supported(&p) && p.is_file() {
            self.path_text = p.display().to_string();
            self.load_video(p);
        } else {
            let list = SUPPORTED_FORMATS.map(|e| format!(".{e}")).join(", ");
            self.dialog(DialogKind::Error, "Error", format!("Unsupported format. Use: {list}"));
        }
    }

    fn open_folder(&mut self) {
        match get_output_dir() {
            Ok(d) => {
                if let Err(e) = open_path(&d) {
                    self.dialog(DialogKind::Error, "Error", format!("Could not open: {e}"));
                }
            }
            Err(_) => self.dialog(DialogKind::Info, "Info", "No clips yet."),
        }
    }

    fn stop_player(&mut self) {
        if let Some((mut ff, mut play)) = self.player.take() {
            let _ = play.kill();
            let _ = ff.kill();
            let _ = play.wait();
            let _ = ff.wait();
        }
    }

    fn play_segs(&mut self, segs: Vec<(f64, f64)>) {
        self.stop_player();
        let Some(v) = &self.video else { return };
        if self.duration() <= 0.0 || segs.is_empty() {
            return;
        }
        match play_segments(&v.path, &v.info, &segs) {
            Ok(p) => self.player = Some(p),
            Err(e) => self.dialog(DialogKind::Error, "Can't play", e),
        }
    }

    fn play(&mut self) {
        self.play_segs(vec![(self.trim_start, self.trim_end)]);
    }

    /// Adds the selection to the combined clip, kept in timeline order with
    /// overlapping cuts merged.
    fn add_cut(&mut self) {
        if self.duration() <= 0.0 {
            return;
        }
        self.queue.push((self.trim_start, self.trim_end));
        self.queue.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for (s, e) in self.queue.drain(..) {
            match merged.last_mut() {
                Some(last) if s <= last.1 => last.1 = last.1.max(e),
                _ => merged.push((s, e)),
            }
        }
        self.queue = merged;
    }

    fn export(&mut self, segments: Vec<(f64, f64)>) {
        if self.exporting || segments.is_empty() {
            return;
        }
        let Some(v) = &self.video else {
            self.dialog(DialogKind::Info, "Info", "Load a clip first.");
            return;
        };
        let params = ExportParams {
            path: v.path.clone(),
            info: v.info.clone(),
            segments,
            mode: self.settings.quality,
        };
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

    fn start_update(&mut self) {
        let Some(u) = self.update.clone() else { return };
        self.updating = true;
        let tx = self.tx.clone();
        thread::spawn(move || match do_update(&u) {
            Ok(true) => tx.send(Msg::Quit),
            Ok(false) => tx.send(Msg::UpdateIdle),
            Err(e) => tx.send(Msg::UpdateFailed(e.to_string())),
        });
    }

    // ── UI pieces ───────────────────────────────────────────────────────────
    fn ui_top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("RaskCut").font(FontId::new(17.0, bold())).color(FG));
            ui.label(RichText::new(APP_VERSION).size(11.0).color(FG_DIM));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(u) = self.update.clone() {
                    let verb = if is_installed() && u.installer_url.is_some() { "Update to" } else { "Download" };
                    let text = if self.updating { "Updating...".to_string() } else { format!("{verb} {}", u.version) };
                    let btn = egui::Button::new(RichText::new(text).color(Color32::WHITE).size(12.0))
                        .fill(GREEN)
                        .corner_radius(CornerRadius::same(12));
                    if ui.add_enabled(!self.updating, btn).clicked() {
                        self.start_update();
                    }
                }
            });
        });
    }

    fn ui_file_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let browse_w = 84.0;
            let edit_w = ui.available_width() - browse_w - ui.spacing().item_spacing.x;
            let r = ui.add_sized(
                [edit_w, 30.0],
                egui::TextEdit::singleline(&mut self.path_text)
                    .hint_text("Drop a video here or browse for one")
                    .vertical_align(Align::Center)
                    .margin(Vec2::new(10.0, 6.0)),
            );
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                let p = PathBuf::from(self.path_text.trim());
                if p.is_file() {
                    self.load_video(p);
                }
            }
            if ui.add_sized([browse_w, 30.0], egui::Button::new("Browse")).clicked() {
                self.browse_file();
            }
        });
    }

    fn ui_preview(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(PW, PH), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let radius = CornerRadius::same(10);
        painter.rect_filled(rect, radius, PREVIEW_BG);

        if let Some(tex) = &self.texture {
            egui::Image::new((tex.id(), rect.size())).corner_radius(radius).paint_at(ui, rect);
        } else {
            painter.text(rect.center(), Align2::CENTER_CENTER, "Drop a video here", FontId::proportional(15.0), FG_DIM);
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
                // Time pill while scrubbing
                let label = seconds_to_time(self.x_to_time(x));
                let pill_pos = Pos2::new(
                    (rect.left() + x).clamp(rect.left() + 34.0, rect.right() - 34.0),
                    rect.top() + 18.0,
                );
                let galley = painter.layout_no_wrap(label, FontId::proportional(12.0), FG);
                let pill = Rect::from_center_size(pill_pos, galley.size() + Vec2::new(14.0, 6.0));
                painter.rect_filled(pill, CornerRadius::same(10), Color32::from_black_alpha(190));
                painter.galley(pill.center() - galley.size() / 2.0, galley, FG);
                painter.line_segment(
                    [Pos2::new(rect.left() + x, pill.bottom()), Pos2::new(rect.left() + x, rect.bottom())],
                    Stroke::new(1.0_f32, Color32::from_white_alpha(90)),
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

    fn ui_trim_bar(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(PW, TRIM_H), Sense::click_and_drag());
        let painter = ui.painter_at(rect.expand(3.0));
        let mid = rect.center().y;
        let track = Rect::from_min_max(Pos2::new(rect.left(), mid - 2.0), Pos2::new(rect.right(), mid + 2.0));
        painter.rect_filled(track, CornerRadius::same(2), BORDER);

        let d = self.duration();
        if d <= 0.0 {
            return;
        }
        let sx = (self.trim_start / d) as f32 * PW;
        let ex = (self.trim_end / d) as f32 * PW;

        // Interaction
        let pointer_x = ui.input(|i| i.pointer.latest_pos()).map(|p| (p.x - rect.left()).clamp(0.0, PW));
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

        let near = |hx: f32| resp.hovered() && pointer_x.is_some_and(|x| (x - hx).abs() <= 8.0);
        if self.trim_drag.is_some() || near(sx) || near(ex) {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }

        // Queued segments
        for &(qs, qe) in &self.queue {
            let r = Rect::from_min_max(
                Pos2::new(rect.left() + (qs / d) as f32 * PW, mid - 2.0),
                Pos2::new(rect.left() + (qe / d) as f32 * PW, mid + 2.0),
            );
            painter.rect_filled(r, CornerRadius::same(2), GREEN.gamma_multiply(0.6));
        }

        // Selection + handles (active handle is highlighted)
        let sx = rect.left() + (self.trim_start / d) as f32 * PW;
        let ex = rect.left() + (self.trim_end / d) as f32 * PW;
        let sel = Rect::from_min_max(Pos2::new(sx, mid - 2.0), Pos2::new(ex, mid + 2.0));
        painter.rect_filled(sel, CornerRadius::same(2), ACCENT);
        for (hx, which) in [(sx, Handle::Start), (ex, Handle::End)] {
            let active = self.active == which;
            let w = if self.trim_drag == Some(which) { 8.0 } else { 6.0 };
            let r = Rect::from_center_size(Pos2::new(hx, mid), Vec2::new(w, TRIM_H - 2.0));
            painter.rect_filled(r, CornerRadius::same(3), if active { Color32::WHITE } else { FG_DIM });
            if active {
                painter.rect_stroke(r.expand(1.5), CornerRadius::same(4), Stroke::new(1.5_f32, ACCENT), egui::StrokeKind::Outside);
            }
        }
    }

    fn ui_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let entry = |ui: &mut egui::Ui, text: &mut String| {
                ui.add(
                    egui::TextEdit::singleline(text)
                        .desired_width(86.0)
                        .horizontal_align(Align::Center)
                        .margin(Vec2::new(6.0, 6.0)),
                )
            };
            ui.label(RichText::new("Start").color(FG_DIM));
            let r = entry(ui, &mut self.start_text);
            if r.gained_focus() {
                self.active = Handle::Start;
            }
            if r.lost_focus() {
                self.on_start_entry_change();
            }
            ui.add_space(8.0);
            ui.label(RichText::new("End").color(FG_DIM));
            let r = entry(ui, &mut self.end_text);
            if r.gained_focus() {
                self.active = Handle::End;
            }
            if r.lost_focus() {
                self.on_end_entry_change();
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                egui::ComboBox::from_id_salt("quality")
                    .selected_text(self.settings.quality.name())
                    .width(100.0)
                    .show_ui(ui, |ui| {
                        for q in Quality::ALL {
                            ui.selectable_value(&mut self.settings.quality, q, q.name());
                        }
                    });
                ui.label(RichText::new("Quality").color(FG_DIM));
            });
        });
    }

    fn ui_actions(&mut self, ui: &mut egui::Ui) {
        let loaded = self.duration() > 0.0;
        ui.horizontal(|ui| {
            let sel = (self.trim_end - self.trim_start).max(0.0);
            let hint = if loaded {
                format!("Selection {}    \u{2190}/\u{2192} frame, Shift 1 s, Space play", seconds_to_time(sel))
            } else {
                "Left click sets start, right click sets end".to_string()
            };
            ui.label(RichText::new(hint).size(11.5).color(FG_DIM));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(loaded, egui::Button::new("Add cut"))
                    .on_hover_text("Add this selection to the combined clip")
                    .clicked()
                {
                    self.add_cut();
                }
                let playing = self.player.is_some();
                let label = if playing { "Stop" } else { "Play" };
                if ui
                    .add_enabled(loaded, egui::Button::new(label))
                    .on_hover_text("Preview the current selection")
                    .clicked()
                {
                    if playing { self.stop_player() } else { self.play() }
                }
            });
        });
    }

    fn ui_queue(&mut self, ui: &mut egui::Ui) {
        if self.queue.is_empty() {
            return;
        }
        let mut remove = None;
        let mut select = None;
        let mut play_all = false;
        let total: f64 = self.queue.iter().map(|(s, e)| e - s).sum();
        egui::Frame::NONE
            .fill(BG2)
            .stroke(Stroke::new(1.0_f32, BORDER))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Vec2::new(10.0, 8.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Combined clip").color(FG).size(12.5));
                    let n = self.queue.len();
                    let cuts = if n == 1 { "1 cut".to_string() } else { format!("{n} cuts") };
                    ui.label(RichText::new(format!("{cuts}  \u{b7}  {}", seconds_to_time(total))).color(FG_DIM).size(12.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(egui::Button::new(RichText::new("Clear").size(12.0)).frame(false)).clicked() {
                            self.queue.clear();
                        }
                        if ui
                            .add(egui::Button::new(RichText::new("Play all").size(12.0)))
                            .on_hover_text("Preview the final combined video")
                            .clicked()
                        {
                            play_all = true;
                        }
                    });
                });
                egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                    for (i, &(s, e)) in self.queue.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let text = format!(
                                "{}.   {}  \u{2192}  {}     {:.1} s",
                                i + 1,
                                seconds_to_time(s),
                                seconds_to_time(e),
                                e - s
                            );
                            if ui
                                .add(egui::Label::new(RichText::new(text).size(12.5)).sense(Sense::click()))
                                .on_hover_text("Click to load this cut into the handles")
                                .clicked()
                            {
                                select = Some((s, e));
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.add(egui::Button::new(RichText::new("\u{d7}").size(14.0)).frame(false)).clicked() {
                                    remove = Some(i);
                                }
                            });
                        });
                    }
                });
            });
        if let Some(i) = remove {
            if i < self.queue.len() {
                self.queue.remove(i);
            }
        }
        if let Some((s, e)) = select {
            self.trim_start = s;
            self.trim_end = e;
            self.sync_texts();
            self.active = Handle::Start;
            self.request_preview_at(s);
        }
        if play_all {
            self.play_segs(self.queue.clone());
        }
    }

    fn ui_progress(&mut self, ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(PW, 4.0), Sense::hover());
        let painter = ui.painter_at(rect);
        let r = CornerRadius::same(2);
        painter.rect_filled(rect, r, BG3);
        if self.indeterminate {
            let t = ui.input(|i| i.time) as f32;
            let seg = PW * 0.25;
            let x = (t * 0.7).fract() * (PW + seg) - seg;
            let bar = Rect::from_min_max(
                Pos2::new(rect.left() + x.max(0.0), rect.top()),
                Pos2::new((rect.left() + x + seg).min(rect.right()), rect.bottom()),
            );
            painter.rect_filled(bar, r, ACCENT);
            ui.ctx().request_repaint();
        } else if self.progress > 0.0 {
            let mut bar = rect;
            bar.set_width(PW * self.progress / 100.0);
            painter.rect_filled(bar, r, ACCENT);
        }
    }

    fn ui_buttons(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let gap = ui.spacing().item_spacing.x;
            let w = (ui.available_width() - gap * 2.0) / 3.0;

            let export_text = if self.exporting { "Exporting..." } else { "Export Clip" };
            let export_btn = egui::Button::new(
                RichText::new(export_text).font(FontId::new(14.0, bold())).color(Color32::WHITE),
            )
            .fill(if self.exporting { ACCENT.gamma_multiply(0.5) } else { ACCENT })
            .corner_radius(CornerRadius::same(8));
            let r = ui.add_sized([w, 38.0], export_btn);
            if r.hovered() && !self.exporting {
                ui.painter().rect_filled(r.rect, CornerRadius::same(8), ACCENT_HOVER.gamma_multiply(0.15));
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            if r.clicked() {
                if self.video.is_none() {
                    self.dialog(DialogKind::Info, "Info", "Load a clip first.");
                } else {
                    self.export(vec![(self.trim_start, self.trim_end)]);
                }
            }

            let q = self.queue.len();
            let queue_btn = egui::Button::new(RichText::new(format!("Export Combined ({q})")).size(14.0))
                .corner_radius(CornerRadius::same(8));
            if ui.add_enabled_ui(q > 0 && !self.exporting, |ui| ui.add_sized([w, 38.0], queue_btn)).inner.clicked() {
                self.export(self.queue.clone());
            }

            let folder_btn = egui::Button::new(RichText::new("Open Folder").size(14.0)).corner_radius(CornerRadius::same(8));
            if ui.add_sized([w, 38.0], folder_btn).clicked() {
                self.open_folder();
            }
        });
        ui.horizontal(|ui| {
            if ui.checkbox(&mut self.settings.clipboard, RichText::new("Copy clips to clipboard after export").size(12.0).color(FG_DIM)).changed() {
                save_settings(&self.settings);
            }
        });
    }

    fn ui_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = self.dialogs.front() else { return };
        let (title, body, kind) = (d.title.clone(), d.body.clone(), d.kind);
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("fastcut_dialog")).show(ctx, |ui| {
            ui.set_width(380.0);
            let color = match kind {
                DialogKind::Info => FG,
                DialogKind::Error => RED,
            };
            ui.label(RichText::new(title).font(FontId::new(16.0, bold())).color(color));
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                ui.label(RichText::new(body).color(FG));
            });
            ui.add_space(10.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let ok = egui::Button::new(RichText::new("OK").color(Color32::WHITE))
                    .fill(ACCENT)
                    .min_size(Vec2::new(80.0, 30.0))
                    .corner_radius(CornerRadius::same(6));
                if ui.add(ok).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                    close = true;
                }
            });
        });
        if close || modal.should_close() {
            self.dialogs.pop_front();
        }
    }

    fn ui_drop_overlay(&self, ctx: &egui::Context) {
        if !ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
        let screen = ctx.content_rect();
        painter.rect_filled(screen, CornerRadius::ZERO, Color32::from_black_alpha(170));
        painter.rect_stroke(screen.shrink(10.0), CornerRadius::same(12), Stroke::new(2.0_f32, ACCENT), egui::StrokeKind::Inside);
        painter.text(screen.center(), Align2::CENTER_CENTER, "Drop to load", FontId::proportional(20.0), FG);
    }
}

impl eframe::App for RaskCut {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_messages(ctx);

        let dropped: Option<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().find_map(|f| f.path.clone()));
        if let Some(p) = dropped {
            self.on_drop(p);
        }

        self.handle_keys(ctx);
        self.run_timers(ctx);

        let panel = egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BG).inner_margin(PAD))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                self.ui_top_bar(ui);
                self.ui_file_row(ui);
                self.ui_preview(ui);
                ui.add_space(2.0);
                self.ui_trim_bar(ui);
                ui.add_space(2.0);
                self.ui_controls(ui);
                self.ui_actions(ui);
                self.ui_queue(ui);
                ui.add_space(2.0);
                self.ui_progress(ui);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(&self.status).size(12.0).color(FG_DIM));
                });
                self.ui_buttons(ui);
                ui.min_rect().bottom()
            });

        // Fit the window height to the content.
        let wanted = (panel.inner + PAD).ceil();
        if (wanted - self.fitted_h).abs() > 1.0 {
            self.fitted_h = wanted;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(PW + PAD * 2.0, wanted)));
        }

        self.ui_drop_overlay(ctx);
        self.ui_dialog(ctx);
    }
}

// ─── Look & feel ────────────────────────────────────────────────────────────
fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

fn setup_style(ctx: &egui::Context) {
    // Use Segoe UI on Windows when available.
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

    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = BG2;
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    v.window_corner_radius = CornerRadius::same(10);
    v.extreme_bg_color = BG3; // text field background
    v.selection.bg_fill = ACCENT.gamma_multiply(0.6);
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    v.override_text_color = Some(FG);

    let r = CornerRadius::same(6);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.inactive.weak_bg_fill = BG3;
    v.widgets.inactive.bg_fill = BG3;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.weak_bg_fill = BORDER;
    v.widgets.hovered.bg_fill = BORDER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x50, 0x50, 0x50));
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.weak_bg_fill = Color32::from_rgb(0x44, 0x44, 0x44);
    v.widgets.active.bg_fill = Color32::from_rgb(0x44, 0x44, 0x44);
    v.widgets.active.corner_radius = r;
    v.widgets.open.weak_bg_fill = BG3;
    v.widgets.open.corner_radius = r;
    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(13.5));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(13.5));
    });
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("RaskCut {APP_VERSION}"))
            .with_inner_size([PW + PAD * 2.0, 620.0])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_drag_and_drop(true)
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
                    .expect("valid icon"),
            ),
        ..Default::default()
    };
    eframe::run_native("RaskCut", options, Box::new(|cc| Ok(Box::new(RaskCut::new(cc)))))
}



