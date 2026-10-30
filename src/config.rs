//! Version, constants and saved settings.

use std::path::PathBuf;
use std::time::Duration;

// ─── Version & update ───────────────────────────────────────────────────────
pub(crate) const APP_VERSION: &str = env!("CARGO_PKG_VERSION"); // from Cargo.toml
/// "owner/repo", set automatically when built by GitHub Actions. Local builds
/// have no update check.
pub(crate) const GITHUB_REPO: Option<&str> = option_env!("GITHUB_REPOSITORY");

// ─── Constants ──────────────────────────────────────────────────────────────
pub(crate) const SUPPORTED_FORMATS: [&str; 7] = ["mp4", "mov", "avi", "mkv", "wmv", "flv", "webm"];
pub(crate) const THUMB_COUNT: usize = 40;
pub(crate) const PREVIEW_W: usize = 580;
pub(crate) const PREVIEW_H: usize = PREVIEW_W * 9 / 16;
pub(crate) const PAD: f32 = 12.0;
pub(crate) const DECODE_TIMEOUT: Duration = Duration::from_secs(15);
pub(crate) const EXPORT_TIMEOUT: Duration = Duration::from_secs(600);
pub(crate) const SCRUB_DEBOUNCE: Duration = Duration::from_millis(30);
pub(crate) const TRIM_PREVIEW_DELAY: Duration = Duration::from_millis(120);
pub(crate) const DISCORD_TARGET_MB: f64 = 9.5;
pub(crate) const DISCORD_HARD_LIMIT: f64 = 9.9;
pub(crate) const DISCORD_AUDIO_KBPS: u64 = 128; // per audio track
pub(crate) const DISCORD_MAX_RETRIES: u32 = 3;
pub(crate) const KEYFRAME_SEARCH_SECS: f64 = 30.0;

pub(crate) const PW: f32 = PREVIEW_W as f32;
pub(crate) const PH: f32 = PREVIEW_H as f32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Quality {
    Discord,
    Lossless,
}

impl Quality {
    pub(crate) const ALL: [Quality; 2] = [Quality::Discord, Quality::Lossless];
    pub(crate) fn name(self) -> &'static str {
        match self {
            Quality::Discord => "Discord",
            Quality::Lossless => "Lossless",
        }
    }
    pub(crate) fn from_name(s: &str) -> Quality {
        if s == "Lossless" { Quality::Lossless } else { Quality::Discord }
    }
}

// ─── Settings ───────────────────────────────────────────────────────────────
pub(crate) fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn config_file() -> PathBuf {
    home().join(".fastcut_config.json")
}

pub(crate) struct Settings {
    pub(crate) quality: Quality,
    pub(crate) clipboard: bool,
    pub(crate) dark: bool,
}

pub(crate) fn load_settings() -> Settings {
    let v = std::fs::read_to_string(config_file())
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    Settings {
        quality: v["quality"].as_str().map(Quality::from_name).unwrap_or(Quality::Discord),
        clipboard: v["clipboard"].as_bool().unwrap_or(true),
        dark: v["theme"].as_str() != Some("light"),
    }
}

pub(crate) fn save_settings(s: &Settings) {
    let json = serde_json::json!({
        "quality": s.quality.name(),
        "clipboard": s.clipboard,
        "theme": if s.dark { "dark" } else { "light" },
    })
    .to_string();
    if let Err(e) = std::fs::write(config_file(), json) {
        eprintln!("Could not save settings: {e}");
    }
}
