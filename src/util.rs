//! Process helpers, paths and time formatting.

use crate::config::*;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ─── Process helpers ────────────────────────────────────────────────────────
/// Command that never opens a console window on Windows.
pub(crate) fn command<S: AsRef<std::ffi::OsStr>>(program: S) -> Command {
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
pub(crate) fn tool_path(name: &str) -> PathBuf {
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

pub(crate) fn ffmpeg() -> &'static Path {
    static P: OnceLock<PathBuf> = OnceLock::new();
    P.get_or_init(|| tool_path("ffmpeg"))
}

pub(crate) fn ffprobe() -> &'static Path {
    static P: OnceLock<PathBuf> = OnceLock::new();
    P.get_or_init(|| tool_path("ffprobe"))
}

pub(crate) fn ffplay() -> &'static Path {
    static P: OnceLock<PathBuf> = OnceLock::new();
    P.get_or_init(|| tool_path("ffplay"))
}

pub(crate) fn null_out() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

/// Waits for a child; kills it on timeout. None = timed out.
pub(crate) fn wait_timeout(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
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

pub(crate) struct Captured {
    pub(crate) success: bool,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

/// Runs a command, captures output. None = could not start or timed out.
pub(crate) fn run_capture(mut c: Command, timeout: Duration) -> Option<Captured> {
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
pub(crate) fn make_temp_dir(tag: &str) -> std::io::Result<PathBuf> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let d = std::env::temp_dir().join(format!("fastcut_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

// ─── Version comparison ─────────────────────────────────────────────────────
pub(crate) fn version_tuple(v: &str) -> Vec<u64> {
    v.trim()
        .split('.')
        .map(|x| x.parse::<u64>())
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|_| vec![0])
}

// ─── Paths ──────────────────────────────────────────────────────────────────
pub(crate) fn videos_folder() -> PathBuf {
    let base = if cfg!(target_os = "macos") { "Movies" } else { "Videos" };
    home().join(base)
}

pub(crate) fn get_output_dir() -> std::io::Result<PathBuf> {
    let d = videos_folder().join("Fastcutvideos");
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

pub(crate) fn open_path(path: &Path) -> std::io::Result<()> {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    command(program).arg(path).spawn().map(|_| ())
}

pub(crate) fn ext_lower(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

pub(crate) fn is_supported(p: &Path) -> bool {
    SUPPORTED_FORMATS.contains(&ext_lower(p).as_str())
}

pub(crate) fn is_mp4_like(ext: &str) -> bool {
    matches!(ext, "mp4" | "mov" | "m4v")
}

/// e.g. "Valorant_00-12-40.mp4" or "Valorant_combined.mp4", with _2, _3... if taken.
pub(crate) fn unique_out(dir: &Path, src: &Path, label: &str, ext: &str) -> PathBuf {
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

pub(crate) fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

/// File name without extension, shortened in the middle if it's long
/// (replay names usually differ only in their timestamp at the end).
pub(crate) fn short_name(p: &Path, max: usize) -> String {
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let chars: Vec<char> = stem.chars().collect();
    if chars.len() <= max {
        return stem;
    }
    let keep = max.saturating_sub(3);
    let head: String = chars[..keep / 2].iter().collect();
    let tail: String = chars[chars.len() - (keep - keep / 2)..].iter().collect();
    format!("{head}...{tail}")
}

// ─── Time helpers ───────────────────────────────────────────────────────────
pub(crate) fn time_to_seconds(s: &str) -> Option<f64> {
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
pub(crate) fn seconds_to_time(sec: f64) -> String {
    let cs = (sec.max(0.0) * 100.0).round() as u64;
    let (h, m, s, c) = (cs / 360_000, cs / 6000 % 60, cs / 100 % 60, cs % 100);
    if h > 0 { format!("{h}:{m:02}:{s:02}.{c:02}") } else { format!("{m:02}:{s:02}.{c:02}") }
}

/// "00-12-40" for file names
pub(crate) fn name_time(sec: f64) -> String {
    let t = sec.max(0.0) as u64;
    format!("{:02}-{:02}-{:02}", t / 3600, t / 60 % 60, t % 60)
}
