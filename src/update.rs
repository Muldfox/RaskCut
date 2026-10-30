//! Update check and install via GitHub Releases.

use crate::{config::*, util::*};
use std::path::Path;
use std::time::Duration;

// ─── Update (GitHub Releases) ───────────────────────────────────────────────
#[derive(Clone)]
pub(crate) struct UpdateInfo {
    pub(crate) version: String,
    pub(crate) installer_url: Option<String>,
    pub(crate) page_url: String,
}

pub(crate) fn check_for_update() -> Option<UpdateInfo> {
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
pub(crate) fn is_installed() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("unins000.exe").is_file()))
        .unwrap_or(false)
}

/// Installed: download the new installer and run it silently (returns true =
/// quit now). Portable: open the release page in the browser (returns false).
pub(crate) fn do_update(u: &UpdateInfo) -> Result<bool, String> {
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
