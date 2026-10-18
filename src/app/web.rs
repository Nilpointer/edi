//! Browser tabs and previews: HTML files open as file:// URLs, files in a
//! web service project open the project's preview URL.

use super::EdiApp;
use crate::browser::{self, Browser};
use crate::buffer::Buffer;
use crate::highlight::HlCache;
use crate::view::{Tab, ViewState};
use super::groups::PaneLayout;
use eframe::egui::{self, Align2, FontId, Key, Rect, Sense, TextEdit, Ui, Vec2};

impl EdiApp {
    pub(super) fn add_browser_tab(&mut self, url: &str, group: usize) {
        let mut b = Browser::new(url);
        b.focus_address = url.is_empty();
        let tab = Tab {
            id: self.alloc_tab_id(),
            buf: Buffer::scratch("Browser", ""),
            hl: HlCache::new("Plain Text"),
            view: ViewState::default(),
            results: None,
            group,
            browser: Some(b),
        };
        self.tabs.push(tab);
        self.group_active[group] = Some(self.tabs.len() - 1);
    }

    /// What the preview icons open for tab `i`, if anything.
    pub(super) fn preview_target(&self, i: usize) -> Option<String> {
        let path = self.tabs.get(i)?.buf.path.as_ref()?;
        if browser::is_web_file(path) {
            return Some(browser::file_url(path));
        }
        let root = self.root.as_ref().filter(|r| path.starts_with(r))?;
        self.preview_urls.get(root).cloned()
    }

    /// Show `url` in a browser tab beside the editor, reusing an existing one.
    /// An empty `url` just opens or focuses the browser's address bar.
    pub(super) fn open_preview(&mut self, url: &str) {
        let editor = self.active;
        if let Some(i) = self.tabs.iter().position(|t| t.browser.is_some()) {
            let b = self.tabs[i].browser.as_mut().unwrap();
            if url.is_empty() {
                b.focus_address = true;
            } else {
                b.navigate(url);
            }
            let g = self.tabs[i].group;
            self.group_active[g] = Some(i);
            if g == self.tabs[editor].group || url.is_empty() {
                self.activate(i);
            }
            return;
        }
        // Like IntelliJ's side preview: put the browser next to the editor.
        if self.layout.groups() == 1 {
            self.set_layout(PaneLayout::Columns(2));
        }
        let g = (self.focus_group + 1) % self.layout.groups();
        self.add_browser_tab(url, g);
        if url.is_empty() {
            self.activate(self.tabs.len() - 1);
        } else {
            self.activate(editor);
        }
    }

    pub(super) fn open_external(&mut self, url: &str) {
        match browser::open_external(url) {
            Ok(()) => self.set_status(format!("Opened {url} in the system browser")),
            Err(e) => self.set_status(format!("Unable to open the system browser: {e}")),
        }
    }

    /// Remember an http(s) address typed for this project as its preview URL.
    fn remember_project_url(&mut self, url: &str) {
        if let Some(root) = &self.root {
            if url.starts_with("http://") || url.starts_with("https://") {
                self.preview_urls.insert(root.clone(), url.to_string());
            }
        }
    }

    /// Address bar and webview for browser tab `ti`. Returns the page rect.
    pub(super) fn browser_ui(&mut self, ui: &mut Ui, ti: usize, frame: &eframe::Frame, visible: bool) -> Rect {
        let ctx = ui.ctx().clone();
        let mut go = None;
        let mut external = None;
        let b = self.tabs[ti].browser.as_mut().unwrap();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            if ui.button("◀").on_hover_text("Back").clicked() {
                b.back();
            }
            if ui.button("▶").on_hover_text("Forward").clicked() {
                b.forward();
            }
            let reload = if b.loading() { "✕" } else { "⟳" };
            if ui.button(reload).on_hover_text("Reload").clicked() {
                b.reload();
            }
            let w = (ui.available_width() - 40.0).max(80.0);
            let te = ui.add(
                TextEdit::singleline(&mut b.address)
                    .hint_text("file path, localhost:8080, or URL")
                    .desired_width(w)
                    .font(FontId::monospace(13.0)),
            );
            if std::mem::take(&mut b.focus_address) {
                te.request_focus();
            }
            b.editing = te.has_focus();
            if te.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                go = Some(b.address.clone());
            }
            if ui.button("↗").on_hover_text("Open in system browser").clicked() {
                external = Some(b.url());
            }
        });
        ui.add_space(4.0);
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::click());
        // Shown while the webview is hidden (menus open) or still starting.
        ui.painter().rect_filled(rect, 0.0, self.syn.palette().bg);

        let b = self.tabs[ti].browser.as_mut().unwrap();
        if let Some(addr) = go {
            b.navigate(&addr);
            let url = b.address.clone();
            self.remember_project_url(&url);
        }
        let b = self.tabs[ti].browser.as_mut().unwrap();
        b.place(&ctx, frame, rect, visible);
        if let Some(err) = b.error.clone() {
            let pal = self.syn.palette();
            ui.painter().rect_filled(rect, 0.0, pal.bg);
            ui.painter().text(rect.center() - Vec2::new(0.0, 14.0), Align2::CENTER_CENTER, err, FontId::proportional(14.0), pal.fg);
            let hint = if std::env::var_os("EDI_NATIVE_WAYLAND").is_some() {
                "The browser view needs X11; unset EDI_NATIVE_WAYLAND. You can still open pages in the system browser (↗)."
            } else {
                "You can still open pages in the system browser (↗)."
            };
            ui.painter().text(rect.center() + Vec2::new(0.0, 12.0), Align2::CENTER_CENTER, hint, FontId::proportional(12.0), pal.gutter_fg);
        }
        let title = b.title();
        self.tabs[ti].buf.label = Some(title);
        if let Some(url) = external {
            if !url.is_empty() {
                self.open_external(&url);
            }
        }
        rect
    }

    /// IntelliJ-style quick links over the editor's top-right corner.
    pub(super) fn preview_icons(&mut self, ui: &mut Ui, ti: usize, editor: Rect) {
        let Some(url) = self.preview_target(ti) else { return };
        if !ui.rect_contains_pointer(editor) {
            return;
        }
        // `editor` already ends where the minimap starts.
        let right = editor.right() - 8.0;
        let size = Vec2::new(28.0, 24.0);
        let preview = Rect::from_min_size(egui::pos2(right - 2.0 * size.x - 4.0, editor.top() + 8.0), size);
        let external = Rect::from_min_size(egui::pos2(right - size.x, editor.top() + 8.0), size);
        let p = ui.put(preview, egui::Button::new("🌐")).on_hover_text("Preview in Edi (Ctrl+Alt+P)");
        let e = ui.put(external, egui::Button::new("↗")).on_hover_text("Open in system browser (Ctrl+Alt+O)");
        if p.clicked() {
            self.open_preview(&url);
        }
        if e.clicked() {
            self.open_external(&url);
        }
    }
}
