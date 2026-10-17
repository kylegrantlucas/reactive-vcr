//! `reactive-vcr-gui`: drop in a video and a song, shape the audio-reactive VHS look live,
//! and render it.

mod analysis;
mod app;
mod editor;
mod meters;
mod preview;
mod render;

use std::path::PathBuf;

use app::{Startup, VcrApp};
use eframe::egui;

const USAGE: &str = "usage: reactive-vcr-gui [VIDEO] [AUDIO] [MAP]";

fn main() -> eframe::Result {
    let mut paths = Vec::new();
    for arg in std::env::args_os().skip(1) {
        if arg == "-h" || arg == "--help" {
            println!("{USAGE}");
            return Ok(());
        }
        paths.push(PathBuf::from(arg));
    }
    if paths.len() > 3 {
        eprintln!("{USAGE}");
        std::process::exit(2);
    }
    if let Err(e) = reactive_vcr_gst::init() {
        eprintln!("reactive-vcr-gui: {e:#}");
        std::process::exit(1);
    }
    let mut paths = paths.into_iter();
    let startup = Startup {
        video: paths.next(),
        audio: paths.next(),
        mapping: paths.next(),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("reactive-vcr")
            .with_app_id("reactive-vcr")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "reactive-vcr",
        options,
        Box::new(|cc| Ok(Box::new(VcrApp::new(cc, startup)))),
    )
}
