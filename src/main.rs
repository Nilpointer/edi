// No console window behind the editor on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod browser;
mod buffer;
mod commands;
mod display;
mod find_files;
mod fuzzy;
mod highlight;
mod instance;
mod pretty;
mod search;
mod session;
mod view;

use std::path::PathBuf;

fn main() -> eframe::Result {
    // The in-app browser embeds WebKitGTK as a child window, which only works
    // on X11, so on Linux both winit and GTK use X11 (XWayland on Wayland).
    // EDI_NATIVE_WAYLAND=1 keeps native Wayland without the browser view.
    #[cfg(target_os = "linux")]
    let force_x11 = std::env::var_os("EDI_NATIVE_WAYLAND").is_none();
    #[cfg(target_os = "linux")]
    if force_x11 {
        // SAFETY: nothing else is running yet, so no thread reads the environment.
        unsafe { std::env::set_var("GDK_BACKEND", "x11") };
    }
    let mut new_instance = false;
    let args: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .filter(|a| {
            let flag = a == "-n" || a == "--new-instance";
            new_instance |= flag;
            !flag
        })
        .map(PathBuf::from)
        .collect();

    // Hand the files to a running Edi if there is one. Automated screenshot
    // runs stay out of it unless they use a private EDI_DATA_DIR.
    let isolated = std::env::var_os("EDI_SCREENSHOT").is_some() && std::env::var_os("EDI_DATA_DIR").is_none();
    let primary = if new_instance || isolated {
        None
    } else {
        match instance::start(&args) {
            instance::Startup::Forwarded => return Ok(()),
            instance::Startup::Primary(p) => Some(p),
            instance::Startup::Standalone => None,
        }
    };

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
    #[cfg(target_os = "linux")]
    let options = eframe::NativeOptions {
        event_loop_builder: force_x11.then(|| -> eframe::EventLoopBuilderHook {
            Box::new(|b| {
                use winit::platform::x11::EventLoopBuilderExtX11;
                b.with_x11();
            })
        }),
        ..options
    };
    eframe::run_native("edi", options, Box::new(move |cc| Ok(Box::new(app::EdiApp::new(cc, args, primary, new_instance)))))
}
