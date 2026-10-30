// RaskCut: fast game clip cutter. See README.md.
// Hide the console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

/// Builds a `Vec<OsString>` from strings and paths (ffmpeg command lines).
macro_rules! args {
    ($($x:expr),* $(,)?) => { vec![$(::std::ffi::OsString::from($x)),*] };
}

mod app;
mod config;
mod export;
mod media;
mod theme;
mod ui;
mod update;
mod util;
mod widgets;

use app::RaskCut;
use config::{APP_VERSION, PAD, PW};
use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("RaskCut {APP_VERSION}"))
            .with_inner_size([PW + (PAD + 4.0) * 2.0, 640.0])
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



