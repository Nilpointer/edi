// No console window behind the editor on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod buffer;
mod commands;
mod fuzzy;
mod highlight;
mod session;
mod view;

use std::path::PathBuf;

fn main() -> eframe::Result {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icons/edi-256.png")).expect("valid icon");
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("edi")
            // Inside a Flatpak the app id must match the desktop file id.
            .with_app_id(std::env::var("FLATPAK_ID").unwrap_or_else(|_| "edi".into()))
            .with_icon(std::sync::Arc::new(icon))
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([400.0, 300.0])
            .with_drag_and_drop(true)
            .with_active(std::env::var_os("EDI_SCREENSHOT").is_none()),
        ..Default::default()
    };
    eframe::run_native("edi", options, Box::new(|cc| Ok(Box::new(app::EdiApp::new(cc, args)))))
}
