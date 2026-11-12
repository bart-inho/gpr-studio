#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod basemap;
mod crs;
mod dataview;
mod demo;
mod draw;
mod formats;
mod gpkg;
mod gps;
mod gpsbatch;
mod mapview;
mod model;
mod pdf;
mod processing;
mod project;
mod render;
mod report;
mod segy_export;
mod selftest;
mod tasks;

use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|a| a == "--selftest").unwrap_or(false) {
        let out = PathBuf::from(args.get(2).cloned().unwrap_or_else(|| "selftest_out".into()));
        let files = args.iter().skip(3).map(PathBuf::from).collect();
        std::process::exit(selftest::run(out, files));
    }
    let mut screenshot: Option<PathBuf> = None;
    let mut files = Vec::new();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--screenshot" && i + 1 < args.len() {
            screenshot = Some(PathBuf::from(&args[i + 1]));
            i += 2;
            continue;
        }
        files.push(PathBuf::from(&args[i]));
        i += 1;
    }
    let mut vp = egui::ViewportBuilder::default().with_inner_size([1500.0, 900.0]).with_min_inner_size([900.0, 560.0]).with_title("GPR Studio");
    if screenshot.is_some() {
        vp = vp.with_active(false);
        #[cfg(windows)]
        {
            vp = vp.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }
    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions { viewport: vp, ..Default::default() };
    #[cfg(target_os = "macos")]
    if screenshot.is_some() {
        options.event_loop_builder = Some(Box::new(|b| {
            use winit::platform::macos::EventLoopBuilderExtMacOS;
            b.with_activate_ignoring_other_apps(false);
        }));
    }
    eframe::run_native("GPR Studio", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, files, screenshot)))))
}
