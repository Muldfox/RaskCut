//! Reading videos with ffprobe/ffmpeg: info, keyframes, preview frames, playback.

use crate::{config::*, util::*};
use eframe::egui::ColorImage;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::Duration;

// ─── HW encoder detection ───────────────────────────────────────────────────
pub(crate) fn detect_hw_encoder() -> Option<&'static str> {
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

// ─── Probe ──────────────────────────────────────────────────────────────────
#[derive(Clone, Default)]
pub(crate) struct VideoInfo {
    pub(crate) codec: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) fps: f64,
    pub(crate) pix_fmt: String,
}

#[derive(Clone, Default)]
pub(crate) struct MediaInfo {
    pub(crate) duration: f64,
    pub(crate) start_time: f64,
    pub(crate) audio_tracks: usize,
    /// One "codec/rate/channels" signature per audio track (to check whether
    /// clips can be joined without re-encoding).
    pub(crate) audio: Vec<String>,
    pub(crate) video: Option<VideoInfo>,
}

impl MediaInfo {
    pub(crate) fn fps(&self) -> f64 {
        self.video.as_ref().map(|v| v.fps).filter(|f| *f > 1.0).unwrap_or(30.0)
    }

    /// Width / height, 16:9 if unknown.
    pub(crate) fn aspect(&self) -> f64 {
        match &self.video {
            Some(v) if v.width > 0 && v.height > 0 => v.width as f64 / v.height as f64,
            _ => 16.0 / 9.0,
        }
    }
}

/// Rounds to the nearest even number (video sizes must be even).
pub(crate) fn even(x: f64) -> u32 {
    (((x / 2.0).round() as u32) * 2).max(2)
}

/// One piece of the final video: a time range in one source clip.
#[derive(Clone)]
pub(crate) struct Seg {
    pub(crate) path: PathBuf,
    pub(crate) info: MediaInfo,
    pub(crate) start: f64,
    pub(crate) end: f64,
}

impl Seg {
    pub(crate) fn dur(&self) -> f64 {
        self.end - self.start
    }
}

/// True if the segments come from more than one file.
pub(crate) fn multi_clip(segs: &[Seg]) -> bool {
    segs.iter().any(|s| s.path != segs[0].path)
}

pub(crate) fn parse_rate(s: &str) -> f64 {
    match s.split_once('/') {
        Some((a, b)) => {
            let (a, b) = (a.parse::<f64>().unwrap_or(0.0), b.parse::<f64>().unwrap_or(0.0));
            if b > 0.0 { a / b } else { 0.0 }
        }
        None => s.parse().unwrap_or(0.0),
    }
}

pub(crate) fn json_f64(v: &serde_json::Value) -> Option<f64> {
    v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_f64())
}

pub(crate) fn parse_ffmpeg_duration(stderr: &str) -> Option<f64> {
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

pub(crate) fn probe_media(path: &Path) -> MediaInfo {
    let mut c = command(ffprobe());
    c.args([
        "-v", "error", "-show_entries",
        "format=duration,start_time:stream=codec_type,codec_name,width,height,pix_fmt,avg_frame_rate,r_frame_rate,sample_rate,channels:stream_disposition=attached_pic",
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
                    Some("audio") => {
                        info.audio_tracks += 1;
                        info.audio.push(format!(
                            "{}/{}/{}",
                            s["codec_name"].as_str().unwrap_or("?"),
                            s["sample_rate"].as_str().unwrap_or("?"),
                            s["channels"].as_u64().unwrap_or(0)
                        ));
                    }
                    Some("video") if info.video.is_none() && s["disposition"]["attached_pic"] != 1 => {
                        let mut fps = parse_rate(s["avg_frame_rate"].as_str().unwrap_or(""));
                        if fps <= 1.0 {
                            fps = parse_rate(s["r_frame_rate"].as_str().unwrap_or(""));
                        }
                        info.video = Some(VideoInfo {
                            codec: s["codec_name"].as_str().unwrap_or("").to_string(),
                            width: s["width"].as_u64().unwrap_or(0) as u32,
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
        info.audio = vec!["?".into(); info.audio_tracks];
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
pub(crate) fn probe_keyframe(path: &Path, info: &MediaInfo, start: f64, end: f64) -> Result<(Option<f64>, f64), ()> {
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
pub(crate) fn grab_frame(path: &Path, t: f64) -> Option<ColorImage> {
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
/// Plays one or more segments (from one or several clips) back to back,
/// with all audio tracks mixed.
pub(crate) fn play_segments(segs: &[Seg]) -> Result<(Child, Child), String> {
    if !ffplay().is_file() && which_fails(ffplay()) {
        return Err("Put ffplay.exe in the same folder as RaskCut.exe.".into());
    }
    let first = &segs[0].info;
    let w = even(first.video.as_ref().map(|v| v.width).filter(|w| *w > 0).unwrap_or(960).min(960) as f64);
    let h = even(w as f64 / first.aspect());
    let any_audio = segs.iter().any(|s| s.info.audio_tracks > 0);
    let mut graph = String::new();
    let mut cat = String::new();
    for (i, sg) in segs.iter().enumerate() {
        // Same size for every piece, even when clips differ.
        graph.push_str(&format!(
            "[{i}:v:0]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,format=yuv420p[v{i}];"
        ));
        cat.push_str(&format!("[v{i}]"));
        if any_audio {
            let norm = "aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo";
            match sg.info.audio_tracks {
                0 => graph.push_str(&format!("anullsrc=r=48000:cl=stereo,atrim=duration={:.3}[m{i}];", sg.dur())),
                1 => graph.push_str(&format!("[{i}:a:0]{norm}[m{i}];")),
                k => {
                    for t in 0..k {
                        graph.push_str(&format!("[{i}:a:{t}]"));
                    }
                    graph.push_str(&format!("amix=inputs={k}:duration=longest:normalize=0,{norm}[m{i}];"));
                }
            }
            cat.push_str(&format!("[m{i}]"));
        }
    }
    let a = usize::from(any_audio);
    graph.push_str(&format!("{cat}concat=n={}:v=1:a={a}[v]", segs.len()));
    if a == 1 {
        graph.push_str("[a]");
    }

    let mut c = command(ffmpeg());
    c.args(["-hide_banner", "-loglevel", "error"]);
    c.args(segment_inputs(segs));
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
pub(crate) fn segment_inputs(segs: &[Seg]) -> Vec<OsString> {
    let mut a = Vec::new();
    for sg in segs {
        a.extend(args!["-ss", format!("{:.3}", sg.start), "-t", format!("{:.3}", sg.dur()), "-i", &sg.path]);
    }
    a
}

/// True if a bare tool name can't be run from PATH either.
pub(crate) fn which_fails(p: &Path) -> bool {
    let mut c = command(p);
    c.arg("-version");
    run_capture(c, Duration::from_secs(5)).is_none()
}
