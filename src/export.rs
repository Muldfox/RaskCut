//! Exporting: Discord (size-targeted) and lossless (frame-accurate smart cut).

use crate::{app::*, config::*, media::*, util::*};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

/// One clip inside an export run (maps local progress into the total bar).
pub(crate) struct Job {
    pub(crate) tx: Tx,
    pub(crate) prefix: String,
    pub(crate) base: f32,
    pub(crate) span: f32,
}

impl Job {
    pub(crate) fn status(&self, s: &str) {
        self.tx.status(format!("{}{s}", self.prefix));
    }
    pub(crate) fn progress(&self, local: f32) {
        self.tx.send(Msg::Progress(self.base + self.span * local.clamp(0.0, 100.0) / 100.0));
    }
    pub(crate) fn error(&self, title: &str, body: impl Into<String>) {
        self.tx.error(&format!("{}{title}", self.prefix), body);
    }
}

// ─── Encoding ───────────────────────────────────────────────────────────────
/// Output size and frame rate for a Discord export.
pub(crate) struct DiscordTarget {
    pub(crate) h: u32,
    pub(crate) fps: f64,
    /// Source is taller than `h` and needs scaling down.
    pub(crate) scale: bool,
    pub(crate) label: String,
}

/// Picks resolution/fps from the available bitrate. Never upscales.
pub(crate) fn discord_target(kbps: u64, v: Option<&VideoInfo>) -> DiscordTarget {
    let src_h = v.map(|v| v.height).unwrap_or(0);
    let src_fps = v.map(|v| v.fps).unwrap_or(0.0);
    let (h, f) = if kbps >= 4000 && src_fps >= 50.0 {
        (720, 60.0)
    } else if kbps >= 1500 {
        (720, 30.0)
    } else if kbps >= 700 {
        (480, 30.0)
    } else {
        (360, 30.0)
    };
    let scale = src_h == 0 || src_h > h;
    let out_h = if scale { h } else { src_h };
    let fps = if src_fps <= 1.0 || src_fps > f + 0.5 { f } else { src_fps };
    DiscordTarget { h: out_h, fps, scale, label: format!("{out_h}p{}", fps.round()) }
}

/// Filter graph that joins the segments into one video stream [vout] and `k`
/// audio streams [a0]..[a{k-1}]. With `norm`, every piece is scaled and
/// letterboxed to that size first (needed when clips differ), and its audio
/// converted to one common format. Tracks a clip doesn't have become silence.
pub(crate) fn concat_graph(segs: &[Seg], k: usize, norm: Option<(u32, u32)>, post_vf: &str) -> String {
    let mut g = String::new();
    let mut cat = String::new();
    for (i, sg) in segs.iter().enumerate() {
        match norm {
            Some((w, h)) => {
                g.push_str(&format!(
                    "[{i}:v:0]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,format=yuv420p[v{i}];"
                ));
                cat.push_str(&format!("[v{i}]"));
            }
            None => cat.push_str(&format!("[{i}:v:0]")),
        }
        for t in 0..k {
            if t >= sg.info.audio_tracks {
                g.push_str(&format!("anullsrc=r=48000:cl=stereo,atrim=duration={:.3}[s{i}_{t}];", sg.dur()));
                cat.push_str(&format!("[s{i}_{t}]"));
            } else if norm.is_some() {
                g.push_str(&format!("[{i}:a:{t}]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo[s{i}_{t}];"));
                cat.push_str(&format!("[s{i}_{t}]"));
            } else {
                cat.push_str(&format!("[{i}:a:{t}]"));
            }
        }
    }
    g.push_str(&format!("{cat}concat=n={}:v=1:a={k}[vc]", segs.len()));
    for t in 0..k {
        g.push_str(&format!("[a{t}]"));
    }
    g.push_str(&format!(";[vc]{post_vf}[vout]"));
    g
}

/// Most audio tracks any of the clips has.
pub(crate) fn max_tracks(segs: &[Seg]) -> usize {
    segs.iter().map(|s| s.info.audio_tracks).max().unwrap_or(0)
}

/// Video/audio mapping for Discord. Both x264 passes use the same graph, so
/// they see identical frames.
pub(crate) fn discord_streams(segs: &[Seg], t: &DiscordTarget, with_audio: bool) -> Vec<OsString> {
    let k = max_tracks(segs);
    let fps = format!("fps={:.3}", t.fps);
    let mut a = Vec::new();
    if segs.len() == 1 {
        let vf = if t.scale { format!("scale=-2:{},{fps}", t.h) } else { fps };
        a.extend(args!["-map", "0:V:0", "-vf", vf, "-fps_mode", "passthrough"]);
        if with_audio && k > 0 {
            a.extend(args!["-map", "0:a"]);
        }
    } else {
        let multi = multi_clip(segs);
        let norm = multi.then(|| (even(t.h as f64 * segs[0].info.aspect()), t.h));
        let post = if !multi && t.scale { format!("scale=-2:{},{fps}", t.h) } else { fps };
        let g = concat_graph(segs, k, norm, &post);
        a.extend(args!["-filter_complex", g, "-map", "[vout]", "-fps_mode", "passthrough"]);
        for i in 0..k {
            a.extend(args!["-map", format!("[a{i}]")]);
        }
    }
    if k > 0 && (with_audio || segs.len() > 1) {
        a.extend(args!["-c:a", "aac", "-b:a", format!("{DISCORD_AUDIO_KBPS}k")]);
    } else {
        a.extend(args!["-an"]);
    }
    a
}

pub(crate) fn build_discord_gpu_cmd(segs: &[Seg], out: &Path, kbps: u64, t: &DiscordTarget, enc: &str) -> Vec<OsString> {
    let mut a = args!["-y"];
    a.extend(segment_inputs(segs));
    a.extend(discord_streams(segs, t, true));
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

pub(crate) fn build_discord_cpu_cmd(
    segs: &[Seg], out: &Path, kbps: u64, t: &DiscordTarget, passnum: u8, passlog: &Path,
) -> Vec<OsString> {
    let mut a = args!["-y"];
    a.extend(segment_inputs(segs));
    a.extend(discord_streams(segs, t, passnum == 2));
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
pub(crate) fn run_pass(args: Vec<OsString>, total_us: f64, offset: f32, scale: f32, job: &Job) -> (i32, String) {
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

pub(crate) fn err_tail(err: &str, fallback: &str) -> String {
    let lines: Vec<&str> = err.trim().lines().collect();
    if lines.is_empty() {
        fallback.to_string()
    } else {
        lines[lines.len().saturating_sub(8)..].join("\n")
    }
}

pub(crate) fn size_mb(p: &Path) -> Option<f64> {
    std::fs::metadata(p).ok().map(|m| m.len() as f64 / (1024.0 * 1024.0))
}

// ─── Export ─────────────────────────────────────────────────────────────────
pub(crate) struct ExportParams {
    /// One segment = the selection. Several = combined into one video.
    pub(crate) segments: Vec<Seg>,
    pub(crate) mode: Quality,
}

pub(crate) struct Exported {
    pub(crate) path: PathBuf,
    pub(crate) mb: f64,
    pub(crate) detail: String,
}

pub(crate) const READY: &str = "Ready";

/// Exports the segment(s) as ONE file. Returns (final status, saved files).
pub(crate) fn export_thread(p: ExportParams, hw: Option<&'static str>, tx: &Tx) -> (String, Vec<PathBuf>) {
    if let Some(sg) = p.segments.iter().find(|s| !s.path.is_file()) {
        tx.error("Error", format!("{} no longer exists.", file_name(&sg.path)));
        return (READY.into(), vec![]);
    }
    let out_dir = match get_output_dir() {
        Ok(d) => d,
        Err(e) => {
            tx.error("Error", format!("Could not create output folder: {e}"));
            return (READY.into(), vec![]);
        }
    };
    let segs: Vec<Seg> = p
        .segments
        .into_iter()
        .map(|mut s| {
            s.start = s.start.max(0.0);
            s.end = s.end.min(s.info.duration);
            s
        })
        .filter(|s| s.dur() > 0.05)
        .collect();
    if segs.is_empty() {
        tx.error("Error", "Start must be before end.");
        return (READY.into(), vec![]);
    }
    let combined = segs.len() > 1;
    let label = if combined { "combined".to_string() } else { name_time(segs[0].start) };
    let src = segs[0].path.clone();

    tx.send(Msg::Progress(0.0));
    let t0 = Instant::now();
    let job = Job { tx: tx.clone(), prefix: String::new(), base: 0.0, span: 100.0 };
    let r = match p.mode {
        Quality::Discord => {
            let out = unique_out(&out_dir, &src, &label, "mp4");
            export_discord(&segs, &out, hw, &job)
        }
        Quality::Lossless => match lossless_mismatch(&segs) {
            // Clips in different formats can't be joined without re-encoding.
            Some(why) => {
                let out = unique_out(&out_dir, &src, &label, "mp4");
                export_hq(&segs, &out, &why, &job)
            }
            None => {
                let mut ext = ext_lower(&src);
                if !SUPPORTED_FORMATS.contains(&ext.as_str()) {
                    ext = "mkv".into();
                }
                let out = unique_out(&out_dir, &src, &label, &ext);
                export_lossless(&segs, &out, &job)
            }
        },
    };
    let Some(e) = r else { return (READY.into(), vec![]) };

    let elapsed = t0.elapsed().as_secs_f64();
    let total: f64 = segs.iter().map(|s| s.dur()).sum();
    let name = file_name(&e.path);
    let mut clips: Vec<&PathBuf> = segs.iter().map(|s| &s.path).collect();
    clips.dedup();
    let from = if clips.len() > 1 { format!(" from {} clips", clips.len()) } else { String::new() };
    let cuts = if combined { format!("\nCuts: {}{from} ({})", segs.len(), seconds_to_time(total)) } else { String::new() };
    tx.send(Msg::Progress(100.0));
    tx.info(
        "Done",
        format!("Saved {name}{cuts}\nSize: {:.1} MB\nEncoder: {}\nTime: {elapsed:.1}s", e.mb, e.detail),
    );
    (format!("Saved  {name}  \u{b7}  {:.1} MB  \u{b7}  {elapsed:.1}s", e.mb), vec![e.path])
}

pub(crate) fn export_discord(segs: &[Seg], out: &Path, hw: Option<&'static str>, job: &Job) -> Option<Exported> {
    let dur: f64 = segs.iter().map(|s| s.dur()).sum();
    let tracks = max_tracks(segs);
    let target_bits = DISCORD_TARGET_MB * 8.0 * 1024.0 * 1024.0;
    let audio_bits = (DISCORD_AUDIO_KBPS * tracks as u64) as f64 * 1000.0 * dur;
    let mut kbps = (((target_bits - audio_bits) / dur / 1000.0) as i64).max(100) as u64;
    let t = discord_target(kbps, segs[0].info.video.as_ref());
    let res_label = t.label.clone();
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
            let cmd = build_discord_gpu_cmd(segs, out, kbps, &t, enc);
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
        let cmd1 = build_discord_cpu_cmd(segs, out, kbps, &t, 1, &passlog);
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
        let cmd2 = build_discord_cpu_cmd(segs, out, kbps, &t, 2, &passlog);
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

/// Why the clips can't be joined losslessly, or None if they can.
pub(crate) fn lossless_mismatch(segs: &[Seg]) -> Option<String> {
    let a = &segs[0].info;
    let va = a.video.clone().unwrap_or_default();
    for sg in segs.iter().filter(|s| s.path != segs[0].path) {
        let b = &sg.info;
        let vb = b.video.clone().unwrap_or_default();
        if va.codec != vb.codec {
            return Some(format!("{} vs {}", va.codec, vb.codec));
        }
        if (va.width, va.height) != (vb.width, vb.height) {
            return Some(format!("{}x{} vs {}x{}", va.width, va.height, vb.width, vb.height));
        }
        if (va.fps - vb.fps).abs() > 0.01 {
            return Some(format!("{:.0} fps vs {:.0} fps", va.fps, vb.fps));
        }
        if va.pix_fmt != vb.pix_fmt {
            return Some(format!("{} vs {}", va.pix_fmt, vb.pix_fmt));
        }
        if a.audio != b.audio {
            return Some("different audio tracks".into());
        }
    }
    None
}

/// Clips in different formats: one high-quality re-encode at the first clip's
/// size and frame rate, keeping every audio track.
pub(crate) fn export_hq(segs: &[Seg], out: &Path, why: &str, job: &Job) -> Option<Exported> {
    let first = &segs[0].info;
    let h = first.video.as_ref().map(|v| v.height).filter(|h| *h > 0).unwrap_or(1080);
    let (w, h) = (even(h as f64 * first.aspect()), even(h as f64));
    let k = max_tracks(segs);
    let dur: f64 = segs.iter().map(|s| s.dur()).sum();
    let mut a = args!["-y"];
    a.extend(segment_inputs(segs));
    let g = concat_graph(segs, k, Some((w, h)), &format!("fps={:.3}", first.fps()));
    a.extend(args!["-filter_complex", g, "-map", "[vout]", "-fps_mode", "passthrough"]);
    for t in 0..k {
        a.extend(args!["-map", format!("[a{t}]")]);
    }
    a.extend(args!["-c:v", "libx264", "-crf", "17", "-preset", "fast", "-pix_fmt", "yuv420p"]);
    if k > 0 {
        a.extend(args!["-c:a", "aac", "-b:a", "192k"]);
    }
    a.extend(args!["-movflags", "+faststart", out]);
    job.status("Clips differ in format, re-encoding at high quality...");
    let (rc, err) = run_pass(a, (dur * 1_000_000.0).max(1.0), 0.0, 100.0, job);
    match rc {
        0 if out.exists() => Some(Exported {
            path: out.to_path_buf(),
            mb: size_mb(out)?,
            detail: format!("High quality re-encode, {w}x{h} (clips differ: {why})"),
        }),
        -1 => {
            job.error("Error", "Export timed out.");
            None
        }
        _ => {
            job.error("Export failed", err_tail(&err, "Unknown ffmpeg error."));
            None
        }
    }
}

/// Lossless: frame-accurate smart cut when possible, keyframe copy otherwise.
pub(crate) fn export_lossless(segs: &[Seg], out: &Path, job: &Job) -> Option<Exported> {
    job.tx.send(Msg::Indeterminate(true));
    let r = lossless_inner(segs, out, job);
    job.tx.send(Msg::Indeterminate(false));
    let detail = r?;
    Some(Exported { path: out.to_path_buf(), mb: size_mb(out)?, detail })
}

pub(crate) fn lossless_inner(segs: &[Seg], out: &Path, job: &Job) -> Option<String> {
    let info = &segs[0].info;
    let codec = info.video.as_ref().map(|v| v.codec.as_str()).unwrap_or("");
    let tracks_note = if info.audio_tracks > 1 { format!(", {} audio tracks", info.audio_tracks) } else { String::new() };

    if matches!(codec, "h264" | "hevc") {
        match smart_cut(segs, out, job) {
            Ok(reencoded) if reencoded > 0.0 => {
                return Some(format!(
                    "Lossless, frame-accurate ({reencoded:.2}s re-encoded at cut points, rest stream copy{tracks_note})"
                ))
            }
            Ok(_) => return Some(format!("Lossless, frame-accurate (stream copy{tracks_note})")),
            Err(e) => eprintln!("smart cut failed: {e}"),
        }
        job.status("Smart cut failed, using keyframe cut...");
        return keyframe_cut(segs, out, job)
            .map(|_| format!("Lossless, keyframe cut (smart cut failed{tracks_note})"));
    }
    job.status("Exporting (lossless)...");
    let why = if codec.is_empty() { "unknown codec".to_string() } else { format!("{codec} can't be smart cut") };
    keyframe_cut(segs, out, job).map(|_| format!("Lossless, keyframe cut ({why}{tracks_note})"))
}

/// Input args for a stream copy that starts at `t`: fast seek to a point well
/// before, then drop everything before `t` on the output side. Demuxer seeking
/// alone can land on an earlier keyframe (e.g. in MKV).
pub(crate) fn exact_seek_input(path: &Path, t: f64) -> Vec<OsString> {
    let t = t.max(0.0);
    let coarse = (t - 30.0).max(0.0);
    args!["-ss", format!("{coarse:.3}"), "-i", path, "-ss", format!("{:.3}", t - coarse)]
}

pub(crate) fn run_ok(a: Vec<OsString>, what: &str, job: &Job) -> Result<(), String> {
    let (rc, err) = run_pass(a, 1.0, 0.0, 0.0, job);
    match rc {
        0 => Ok(()),
        -1 => Err(format!("{what}: timed out")),
        _ => Err(format!("{what}: {}", err_tail(&err, "failed"))),
    }
}

pub(crate) fn mux_flags(a: &mut Vec<OsString>, out: &Path, hevc: bool) {
    if is_mp4_like(&ext_lower(out)) {
        if hevc {
            a.extend(args!["-tag:v", "hvc1"]);
        }
        a.extend(args!["-movflags", "+faststart"]);
    }
}

/// Old behaviour (fallback): each segment starts on the keyframe at or before
/// its start. Several segments are copied separately, then joined.
pub(crate) fn keyframe_cut(segs: &[Seg], out: &Path, job: &Job) -> Option<()> {
    let r = (|| -> Result<(), String> {
        let one = |sg: &Seg, dst: &Path| -> Result<(), String> {
            let mut a = args![
                "-y", "-ss", format!("{:.3}", sg.start), "-i", &sg.path, "-t", format!("{:.3}", sg.dur()),
                "-map", "0:V", "-map", "0:a?", "-c", "copy", "-avoid_negative_ts", "make_zero",
            ];
            mux_flags(&mut a, dst, false);
            a.push(dst.into());
            run_ok(a, "Copy", job)
        };
        if segs.len() == 1 {
            return one(&segs[0], out);
        }
        let tmp = make_temp_dir("join").map_err(|e| e.to_string())?;
        let r = (|| {
            let mut list = String::new();
            for (i, sg) in segs.iter().enumerate() {
                job.status(&format!("Copying cut {}/{}...", i + 1, segs.len()));
                one(sg, &tmp.join(format!("seg{i}.mkv")))?;
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
pub(crate) fn count_frames(path: &Path, info: &MediaInfo, from: f64, to: f64) -> Option<usize> {
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
pub(crate) fn video_span(p: &Path, fps: f64) -> Option<f64> {
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

/// Frame-accurate lossless cut of one or more segments (from one or several
/// clips in the same format) into ONE file. Per segment:
///   a.ts  = [start, k) re-encoded near-losslessly (only if start isn't a keyframe)
///   b.ts  = [k, end) stream copy, starting on keyframe k
///   audio = all tracks, stream copy, trimmed to exactly match that video
/// Then everything is joined with stream copy. Returns seconds re-encoded.
pub(crate) fn smart_cut(segs: &[Seg], out: &Path, job: &Job) -> Result<f64, String> {
    let v = segs[0].info.video.clone().unwrap_or_default();
    let tracks = segs[0].info.audio_tracks;
    let n = segs.len();
    let tmp = make_temp_dir("smart").map_err(|e| e.to_string())?;
    let r = (|| -> Result<f64, String> {
        let mut vlist = String::new();
        let mut alist = String::new();
        let mut reencoded = 0.0;

        for (i, sg) in segs.iter().enumerate() {
            let (path, info, start, end) = (&sg.path, &sg.info, sg.start, sg.end);
            let fps = info.fps();
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

            if tracks > 0 {
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
        if tracks > 0 {
            std::fs::write(tmp.join("audio.txt"), alist).map_err(|e| e.to_string())?;
            a.extend(args!["-f", "concat", "-safe", "0", "-i", tmp.join("audio.txt")]);
        }
        a.extend(args!["-map", "0:v"]);
        if tracks > 0 {
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
