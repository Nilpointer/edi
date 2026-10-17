//! In-app browser: a native webview (WebKitGTK, WebView2 or WKWebView via
//! wry) placed over a pane, with an address bar drawn by egui.
//!
//! The webview is a native child window, so it always draws on top of egui.
//! The app hides it while menus, popups or dialogs are open.

use eframe::egui;
use std::path::Path;
use std::sync::{Arc, Mutex};
use wry::raw_window_handle::HasWindowHandle;

/// State the webview's callbacks report back to the UI.
#[derive(Default)]
struct Shared {
    url: String,
    title: String,
    loading: bool,
}

pub struct Browser {
    /// Text in the address bar.
    pub address: String,
    /// The address bar is being edited; don't overwrite it from navigation.
    pub editing: bool,
    /// Focus the address bar on the next frame.
    pub focus_address: bool,
    shared: Arc<Mutex<Shared>>,
    webview: Option<wry::WebView>,
    /// URL to load once the webview exists.
    pending: Option<String>,
    pub error: Option<String>,
    last_bounds: Option<egui::Rect>,
    last_visible: Option<bool>,
}

impl Browser {
    pub fn new(url: &str) -> Self {
        Browser {
            address: url.to_string(),
            editing: false,
            focus_address: false,
            shared: Arc::default(),
            webview: None,
            pending: (!url.is_empty()).then(|| url.to_string()),
            error: None,
            last_bounds: None,
            last_visible: None,
        }
    }

    pub fn url(&self) -> String {
        let s = self.shared.lock().unwrap();
        if s.url.is_empty() { self.pending.clone().unwrap_or_default() } else { s.url.clone() }
    }

    pub fn title(&self) -> String {
        let s = self.shared.lock().unwrap();
        if !s.title.is_empty() {
            return s.title.clone();
        }
        drop(s);
        let url = self.url();
        if url.is_empty() { "Browser".into() } else { url }
    }

    pub fn loading(&self) -> bool {
        self.shared.lock().unwrap().loading
    }

    /// Load whatever was typed into the address bar.
    pub fn navigate(&mut self, input: &str) {
        let url = normalize(input);
        if url.is_empty() {
            return;
        }
        self.address = url.clone();
        match &self.webview {
            Some(w) => {
                if let Err(e) = w.load_url(&url) {
                    self.error = Some(e.to_string());
                }
            }
            None => self.pending = Some(url),
        }
    }

    pub fn reload(&self) {
        if let Some(w) = &self.webview {
            let _ = w.reload();
        }
    }

    pub fn back(&self) {
        if let Some(w) = &self.webview {
            let _ = w.evaluate_script("history.back()");
        }
    }

    pub fn forward(&self) {
        if let Some(w) = &self.webview {
            let _ = w.evaluate_script("history.forward()");
        }
    }

    /// Show the webview over `rect` (egui points), creating it on first use.
    pub fn place(&mut self, ctx: &egui::Context, window: &impl HasWindowHandle, rect: egui::Rect, visible: bool) {
        if self.webview.is_none() && visible && self.error.is_none() {
            self.create(ctx, window, rect);
        }
        let Some(w) = &self.webview else { return };
        if self.last_bounds != Some(rect) {
            let _ = w.set_bounds(bounds(ctx, rect));
            self.last_bounds = Some(rect);
        }
        if self.last_visible != Some(visible) {
            let _ = w.set_visible(visible);
            self.last_visible = Some(visible);
        }
        // Keep the address bar in sync with in-page navigation.
        if !self.editing {
            let url = self.shared.lock().unwrap().url.clone();
            if !url.is_empty() && url != self.address {
                self.address = url;
            }
        }
    }

    /// Give keyboard focus back to Edi after the page had it.
    pub fn release_focus(&self) {
        if let Some(w) = &self.webview {
            let _ = w.focus_parent();
        }
    }

    pub fn has_webview(&self) -> bool {
        self.webview.is_some()
    }

    pub fn hide(&mut self) {
        if let (Some(w), false) = (&self.webview, self.last_visible == Some(false)) {
            let _ = w.set_visible(false);
            self.last_visible = Some(false);
        }
    }

    fn create(&mut self, ctx: &egui::Context, window: &impl HasWindowHandle, rect: egui::Rect) {
        if let Err(e) = init_platform() {
            self.error = Some(e);
            return;
        }
        let url = self.pending.take().unwrap_or_else(|| "about:blank".into());
        let (s1, s2) = (self.shared.clone(), self.shared.clone());
        let (c1, c2) = (ctx.clone(), ctx.clone());
        let built = wry::WebViewBuilder::new()
            .with_url(url)
            .with_bounds(bounds(ctx, rect))
            .with_on_page_load_handler(move |event, url| {
                let mut s = s1.lock().unwrap();
                s.loading = matches!(event, wry::PageLoadEvent::Started);
                s.url = url;
                c1.request_repaint();
            })
            .with_document_title_changed_handler(move |title| {
                s2.lock().unwrap().title = title;
                c2.request_repaint();
            })
            .build_as_child(window);
        match built {
            Ok(w) => self.webview = Some(w),
            Err(e) => self.error = Some(format!("Could not start the browser view: {e}")),
        }
    }
}

fn bounds(ctx: &egui::Context, r: egui::Rect) -> wry::Rect {
    let ppp = ctx.pixels_per_point();
    wry::Rect {
        position: wry::dpi::PhysicalPosition::new((r.left() * ppp).round() as i32, (r.top() * ppp).round() as i32).into(),
        size: wry::dpi::PhysicalSize::new((r.width() * ppp).round().max(1.0) as u32, (r.height() * ppp).round().max(1.0) as u32)
            .into(),
    }
}

#[cfg(target_os = "linux")]
fn init_platform() -> Result<(), String> {
    use std::sync::OnceLock;
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| gtk::init().map_err(|e| format!("GTK could not start: {e}"))).clone()
}

#[cfg(not(target_os = "linux"))]
fn init_platform() -> Result<(), String> {
    Ok(())
}

/// Let WebKitGTK process its events. Call every frame while a browser exists.
pub fn pump() {
    #[cfg(target_os = "linux")]
    if gtk::is_initialized_main_thread() {
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
    }
}

/// Turn address-bar input into a URL: paths become file:// URLs, bare hosts
/// like `localhost:3000` get http://.
pub fn normalize(input: &str) -> String {
    let s = input.trim();
    if s.is_empty() {
        return String::new();
    }
    if s.contains("://") || s.starts_with("about:") || s.starts_with("data:") {
        return s.to_string();
    }
    let expanded = match (s.strip_prefix("~/"), std::env::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => Path::new(s).to_path_buf(),
    };
    if expanded.is_absolute() && expanded.exists() {
        return file_url(&expanded);
    }
    format!("http://{s}")
}

/// file:// URL for a local path, percent-encoding the characters that would
/// otherwise change the URL's meaning.
pub fn file_url(path: &Path) -> String {
    let p = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from(if p.starts_with('/') { "file://" } else { "file:///" });
    for c in p.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '#' => out.push_str("%23"),
            '%' => out.push_str("%25"),
            '?' => out.push_str("%3F"),
            c => out.push(c),
        }
    }
    out
}

/// Files the browser can show directly.
pub fn is_web_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
        Some("html" | "htm" | "xhtml" | "svg")
    )
}

/// Open a URL in the system's default browser.
pub fn open_external(url: &str) -> std::io::Result<()> {
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(url).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_addresses() {
        assert_eq!(normalize("localhost:8080/api"), "http://localhost:8080/api");
        assert_eq!(normalize("https://example.com"), "https://example.com");
        assert_eq!(normalize("  "), "");
        let dir = std::env::temp_dir();
        assert!(normalize(&dir.to_string_lossy()).starts_with("file://"));
    }

    #[test]
    fn file_urls_are_escaped() {
        assert_eq!(file_url(Path::new("/tmp/my page#1.html")), "file:///tmp/my%20page%231.html");
        assert!(is_web_file(Path::new("x/index.HTML")) && !is_web_file(Path::new("main.rs")));
    }
}
