# RaskCut

**Cut your game clips and share them on Discord in seconds.**

RaskCut is a small, fast clip cutter written in Rust. Load the replay clips your GPU or recorder saved, pick the moments you want, and export a video that uploads to Discord on a free account. No Nitro, no upload sites, no video editor to learn.

<p align="center">
  <img src="docs/screenshot.png" alt="RaskCut" width="600">
</p>

## What it does

- **Fast.** Written in Rust. Opens instantly, scrubs instantly, exports in seconds. Uses your GPU for encoding when it can (NVIDIA, AMD or Intel).
- **Always under 10 MB for Discord.** No Nitro needed.
- **Highlights from several clips.** Load all the clips from a match, pick moments from each, and get one video.
- **Ready to paste.** The finished video is copied to your clipboard. Open Discord, press Ctrl+V, done.
- **Lossless when you want to keep it.** Cuts on the exact frame and keeps the original quality and every audio track (game, mic, voice chat).

Works with clips from NVIDIA Instant Replay, AMD Adrenalin, OBS replay buffer, Steam and Xbox Game Bar. Supports MP4, MKV, MOV, AVI, WMV, FLV and WebM.

### How the 10 MB limit works

RaskCut takes the total length of your video and works out how much data each second can use so the file ends up just under 10 MB. Short clips get high quality, long ones get a lower resolution so they still look clean. If a file still comes out too big, it's re-encoded smaller automatically.

## How to use it

1. Drop one or more clips into RaskCut, or click **Add clips**.
2. Drag the handles to pick a moment. Press **Space** to check how it looks and sounds.
3. Click **Add cut**. Repeat for every moment you want, in any of the clips.
4. Press **Play** to watch the final video, all cuts back to back.
5. Pick **Discord** or **Lossless**, click **Export**, and paste it into Discord.

Just want one clip, no highlights? Skip **Add cut** and export the selection directly.

## Install

Download `RaskCut-Setup-x.y.z.exe` from [Releases](https://github.com/Muldfox/RaskCut/releases/latest) and run it. Everything is included, and the installed version updates itself.

Windows may say "Windows protected your PC". Click **More info** and then **Run anyway**.

Prefer no installer? Grab the portable zip from the same page, unzip it anywhere and run `RaskCut.exe`.

## Build from source

1. Install Rust from https://rustup.rs
2. `cargo build --release`
3. Put `ffmpeg.exe`, `ffprobe.exe` and `ffplay.exe` next to `target/release/RaskCut.exe`

## License

Video processing is done by [FFmpeg](https://ffmpeg.org), which is bundled under the GPL. Its license and source code information are in the `licenses` folder of the installation.

*Rask is Swedish for quick.*
