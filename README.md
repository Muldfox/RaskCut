# RaskCut

Fast video clip cutter for gaming footage. Cut lossless (frame-accurate) for archiving, or squeeze clips under 10 MB for Discord.

## Install

Go to **[Releases](../../releases/latest)** and download `RaskCut-Setup-x.y.z.exe`. FFmpeg is included, nothing else to install.

Windows may say "Windows protected your PC". Click **More info** and then **Run anyway**.

## Features

- Lossless export, frame-accurate for H.264/HEVC, keeps all audio tracks
- Discord export: always under 10 MB, picks resolution automatically
- Combine several cuts into one video, preview it with **Play all**
- Frame stepping with arrow keys, Shift for 1 second, Space to play
- Copies the finished clip to the clipboard, ready to paste into Discord

## Build from source

1. Install Rust from https://rustup.rs
2. `cargo build --release`
3. Put `ffmpeg.exe`, `ffprobe.exe` and `ffplay.exe` next to `target/release/RaskCut.exe`

## License

FFmpeg is bundled under the GPL, see `licenses/` in the installed folder.
