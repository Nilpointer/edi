//! Application shell: tabs, side bar, overlays (command palette, Goto
//! Anything), find panel, status bar.

mod groups;
mod menu;
mod web;

use crate::buffer::Buffer;
use groups::PaneLayout;
use crate::commands::{Cmd, KeyResult, Keymap, Stroke, COMMANDS};
use crate::find_files;
use crate::fuzzy;
use crate::highlight::{comment_token, HlCache, Syntaxes};
use crate::instance::Primary;
use crate::search::{Matcher, Search};
use crate::session::{self, Session, Snapshot, TabState};
use crate::system;
use crate::view::{Clip, Results, Tab, ViewState};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Event, FontId, Key, Layout, Modifiers, RichText, Sense, TextEdit, Ui,
    Vec2, ViewportCommand,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

const DEFAULT_FONT_SIZE: f32 = 14.0;
const EDITOR_ID: &str = "edi-view";

/// Focus id of group `g`'s editor view.
fn editor_id(g: usize) -> egui::Id {
    egui::Id::unique((EDITOR_ID, g))
}
/// Find in Files "Where" token for searching the open buffers.
pub(crate) const OPEN_FILES: &str = "<open files>";

#[derive(Clone, PartialEq)]
enum Mode {
    Commands,
    Anything,
    Theme { original: usize },
    Syntax,
    OpenFile,
    OpenFolder,
    SaveAs,
}

#[derive(Clone)]
enum Action {
    Cmd(Cmd),
    Open(PathBuf, Option<usize>),
    Tab(usize),
    Line(usize),
    Theme(usize),
    Syntax(String),
    Path(PathBuf, bool),
}

#[derive(Clone)]
struct Item {
    label: String,
    detail: String,
    hits: Vec<usize>,
    action: Action,
}

struct Overlay {
    mode: Mode,
    query: String,
    selected: usize,
    items: Vec<Item>,
    computed_for: Option<String>,
    just_opened: bool,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum FindMode {
    #[default]
    Find,
    Replace,
    Files,
}

#[derive(Default)]
struct FindPanel {
    open: bool,
    mode: FindMode,
    search: Search,
    replacement: String,
    /// Find in Files: folders and filters, e.g. `src, *.rs, -*.lock`.
    where_: String,
    focus: bool,
    compiled: Option<(Search, Result<Matcher, String>)>,
}

impl FindPanel {
    /// The compiled search, recompiled only when the options change.
    fn matcher(&mut self) -> Option<Matcher> {
        if self.compiled.as_ref().is_none_or(|(s, _)| *s != self.search) {
            self.compiled = Some((self.search.clone(), self.search.compile()));
        }
        self.compiled.as_ref().and_then(|(_, r)| r.as_ref().ok().cloned())
    }

    fn error(&self) -> Option<&str> {
        match &self.compiled {
            Some((s, Err(e))) if *s == self.search && !e.is_empty() => Some(e),
            _ => None,
        }
    }
}

enum Dialog {
    /// Unsaved changes in the tab with this id.
    CloseTab(u64),
    Quit,
    About,
}

pub struct EdiApp {
    syn: Syntaxes,
    tabs: Vec<Tab>,
    active: usize,
    root: Option<PathBuf>,
    files: Vec<String>,
    files_rx: Option<mpsc::Receiver<Vec<String>>>,
    dir_cache: HashMap<PathBuf, Vec<(PathBuf, bool)>>,
    keymap: Keymap,
    overlay: Option<Overlay>,
    find: FindPanel,
    dialog: Option<Dialog>,
    clip: Clip,
    font_size: f32,
    show_sidebar: bool,
    show_minimap: bool,
    word_wrap: bool,
    focus_editor: bool,
    status: Option<(String, Instant)>,
    title: String,
    quit_confirmed: bool,
    pending_line: Option<usize>,
    frame: u64,
    /// Hot exit: keep unsaved work in the session instead of prompting on quit.
    hot_exit: bool,
    session: Option<session::Writer>,
    session_key: u64,
    session_saved: Instant,
    /// Set when this is the primary instance: files sent by later launches.
    primary: Option<Primary>,
    layout: PaneLayout,
    /// Preview URL per project folder, for web service projects.
    preview_urls: HashMap<PathBuf, String>,
    /// Active tab of each group.
    group_active: Vec<Option<usize>>,
    focus_group: usize,
    /// Ids of tabs still to close for Close All / Left / Right.
    close_queue: Vec<u64>,
    next_tab_id: u64,
    /// Running Find in Files search, with the pattern and flags for the header.
    fif: Option<(find_files::Job, Matcher, String, String)>,
}

impl EdiApp {
    pub fn new(cc: &eframe::CreationContext<'_>, args: Vec<PathBuf>, primary: Option<Primary>, new_instance: bool) -> Self {
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let mut app = EdiApp {
            syn: Syntaxes::load(),
            tabs: Vec::new(),
            active: 0,
            root: None,
            files: Vec::new(),
            files_rx: None,
            dir_cache: HashMap::new(),
            keymap: Keymap::new(),
            overlay: None,
            find: FindPanel::default(),
            dialog: None,
            clip: Clip::default(),
            font_size: DEFAULT_FONT_SIZE,
            show_sidebar: true,
            show_minimap: true,
            word_wrap: false,
            focus_editor: true,
            status: None,
            title: String::new(),
            quit_confirmed: false,
            pending_line: None,
            frame: 0,
            hot_exit: true,
            session: None,
            session_key: 0,
            session_saved: Instant::now(),
            primary: None,
            fif: None,
            close_queue: Vec::new(),
            next_tab_id: 0,
            layout: PaneLayout::Single,
            preview_urls: HashMap::new(),
            group_active: vec![None],
            focus_group: 0,
        };
        // Automated screenshot runs must not read or clobber the real session.
        // A forced second instance (-n) must not fight the primary over the session.
        let use_session = !new_instance && (std::env::var_os("EDI_SCREENSHOT").is_none() || std::env::var_os("EDI_DATA_DIR").is_some());
        if let Some(p) = &primary {
            let _ = p.ctx.set(cc.egui_ctx.clone());
        }
        app.primary = primary;
        if use_session {
            if let Some(s) = session::load() {
                app.apply_session(s);
            }
            app.session = Some(session::Writer::new());
        }
        for a in args {
            let a = a.canonicalize().unwrap_or(a);
            if a.is_dir() {
                app.set_root(a);
            } else {
                app.open(&a);
            }
        }
        if app.tabs.is_empty() {
            app.new_tab(Buffer::new());
        }
        app.apply_visuals(&cc.egui_ctx);
        app
    }

    /// Open files that later launches handed to this instance, and raise the window.
    fn take_forwarded(&mut self, ctx: &egui::Context) {
        let Some(p) = &self.primary else { return };
        let requests: Vec<_> = p.requests.try_iter().collect();
        if requests.is_empty() {
            return;
        }
        for path in requests.into_iter().flat_map(|r| r.paths) {
            let path = path.canonicalize().unwrap_or(path);
            if path.is_dir() {
                self.set_root(path);
            } else {
                self.open(&path);
            }
        }
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    fn apply_session(&mut self, s: Session) {
        self.font_size = s.font_size.clamp(7.0, 48.0);
        self.show_sidebar = s.sidebar;
        self.show_minimap = s.minimap;
        self.word_wrap = s.word_wrap;
        self.hot_exit = s.hot_exit;
        self.find.search = s.search;
        self.find.where_ = s.find_where;
        if let Some(i) = self.syn.themes.iter().position(|t| t.0 == s.theme) {
            self.syn.theme = i;
        }
        if let Some(root) = s.root.filter(|r| r.is_dir()) {
            self.set_root(root);
        }
        self.layout = PaneLayout::from_code(s.layout);
        self.preview_urls = s.preview_urls;
        self.normalize_groups();
        let mut active = 0;
        for (i, t) in s.tabs.iter().enumerate() {
            if let Some(url) = &t.browser_url {
                if i == s.active {
                    active = self.tabs.len();
                }
                self.add_browser_tab(url, t.group.min(self.layout.groups() - 1));
                continue;
            }
            let Ok(buf) = Buffer::from_session(t.path.as_deref(), t.unsaved.as_deref(), &t.sels) else { continue };
            if i == s.active {
                active = self.tabs.len();
            }
            self.focus_group = t.group.min(self.layout.groups() - 1);
            self.new_tab(buf);
            let tab = self.tabs.last_mut().unwrap();
            if self.syn.ss.find_syntax_by_name(&t.syntax).is_some() {
                tab.hl.set_syntax(&t.syntax);
            }
            tab.view.scroll = Vec2::new(t.scroll[0], t.scroll[1]);
        }
        self.normalize_groups();
        self.activate(active.min(self.tabs.len().saturating_sub(1)));
    }

    fn snapshot(&self) -> Snapshot {
        let mut tabs = Vec::new();
        let mut unsaved = Vec::new();
        let mut active = 0;
        for (i, t) in self.tabs.iter().enumerate() {
            if t.results.is_some() {
                continue;
            }
            if let Some(b) = &t.browser {
                if i == self.active {
                    active = tabs.len();
                }
                tabs.push(TabState { browser_url: Some(b.url()), group: t.group, ..Default::default() });
                unsaved.push(None);
                continue;
            }
            let dirty = t.buf.is_dirty();
            // Without hot exit only files on disk are remembered.
            if t.buf.path.is_none() && (!self.hot_exit || t.buf.len() == 0) {
                continue;
            }
            if i == self.active {
                active = tabs.len();
            }
            tabs.push(TabState {
                path: t.buf.path.clone(),
                unsaved: None,
                sels: t.buf.sels.iter().map(|s| (s.anchor, s.head)).collect(),
                scroll: [t.view.scroll.x, t.view.scroll.y],
                syntax: t.hl.syntax.clone(),
                group: t.group,
                browser_url: None,
            });
            unsaved.push((self.hot_exit && dirty).then(|| t.buf.rope.clone()));
        }
        let session = Session {
            tabs,
            active,
            root: self.root.clone(),
            sidebar: self.show_sidebar,
            minimap: self.show_minimap,
            word_wrap: self.word_wrap,
            font_size: self.font_size,
            theme: self.syn.themes[self.syn.theme].0.clone(),
            hot_exit: self.hot_exit,
            layout: self.layout.code(),
            preview_urls: self.preview_urls.clone(),
            search: self.find.search.clone(),
            find_where: self.find.where_.clone(),
        };
        Snapshot { session, unsaved }
    }

    /// Cheap fingerprint of everything the session stores.
    fn session_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for t in &self.tabs {
            (&t.buf.path, t.buf.version, t.view.scroll.x.to_bits(), t.view.scroll.y.to_bits(), &t.hl.syntax).hash(&mut h);
            for s in &t.buf.sels {
                (s.anchor, s.head).hash(&mut h);
            }
        }
        (self.active, &self.root, self.show_sidebar, self.show_minimap, self.word_wrap, self.hot_exit, self.layout.code()).hash(&mut h);
        for t in &self.tabs {
            t.group.hash(&mut h);
            if let Some(b) = &t.browser {
                b.url().hash(&mut h);
            }
        }
        let mut urls: Vec<_> = self.preview_urls.iter().collect();
        urls.sort();
        urls.hash(&mut h);
        (self.font_size.to_bits(), self.syn.theme, &self.find.search, &self.find.where_).hash(&mut h);
        h.finish()
    }

    /// Save the session at most every two seconds, and only after changes.
    fn autosave(&mut self, ctx: &egui::Context) {
        if self.session.is_none() {
            return;
        }
        let key = self.session_key();
        if key == self.session_key {
            return;
        }
        let wait = std::time::Duration::from_secs(2);
        if self.session_saved.elapsed() < wait {
            ctx.request_repaint_after(wait - self.session_saved.elapsed());
            return;
        }
        let snap = self.snapshot();
        if let Some(w) = &self.session {
            w.save(snap);
        }
        self.session_key = key;
        self.session_saved = Instant::now();
    }

    fn apply_visuals(&self, ctx: &egui::Context) {
        let pal = self.syn.palette();
        let mut v = if self.syn.dark() { egui::Visuals::dark() } else { egui::Visuals::light() };
        let shade = |c: Color32, f: f32| {
            let m = |x: u8| (x as f32 * f).clamp(0.0, 255.0) as u8;
            Color32::from_rgb(m(c.r()), m(c.g()), m(c.b()))
        };
        let chrome = if self.syn.dark() { shade(pal.bg, 0.78) } else { shade(pal.bg, 0.93) };
        v.panel_fill = chrome;
        v.window_fill = shade(pal.bg, if self.syn.dark() { 1.15 } else { 0.98 });
        v.extreme_bg_color = pal.bg;
        v.selection.bg_fill = pal.selection;
        ctx.set_visuals(v);
    }

    fn set_status(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), Instant::now()));
    }

    fn set_root(&mut self, root: PathBuf) {
        self.dir_cache.clear();
        self.files.clear();
        let (tx, rx) = mpsc::channel();
        let r = root.clone();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            for entry in ignore::WalkBuilder::new(&r).hidden(true).build().flatten() {
                if entry.file_type().is_some_and(|t| t.is_file()) {
                    if let Ok(rel) = entry.path().strip_prefix(&r) {
                        out.push(rel.to_string_lossy().into_owned());
                    }
                    if out.len() >= 200_000 {
                        break;
                    }
                }
            }
            out.sort();
            let _ = tx.send(out);
        });
        self.files_rx = Some(rx);
        self.root = Some(root);
        self.show_sidebar = true;
    }

    fn new_tab(&mut self, buf: Buffer) {
        let first: String = buf.line_str(0);
        let syntax = self.syn.detect(buf.path.as_deref(), &first).name.clone();
        let group = self.focus_group;
        let id = self.alloc_tab_id();
        self.tabs.push(Tab { id, buf, hl: HlCache::new(&syntax), view: ViewState::default(), results: None, group, browser: None });
        self.activate(self.tabs.len() - 1);
    }

    fn open(&mut self, path: &Path) {
        let path = path.canonicalize().unwrap_or(path.to_path_buf());
        if let Some(i) = self.tabs.iter().position(|t| t.buf.path.as_deref() == Some(&path)) {
            self.activate(i);
            return;
        }
        match Buffer::open(&path) {
            Ok(buf) => {
                // Replace a pristine untitled tab, as Sublime does.
                let blank = self.tabs.len() == 1 && self.tabs[0].buf.path.is_none() && self.tabs[0].buf.len() == 0;
                if blank {
                    self.tabs.clear();
                    self.tab_removed(0);
                }
                self.new_tab(buf);
            }
            Err(e) => self.set_status(format!("Unable to open {}: {e}", path.display())),
        }
    }

    fn tab(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    fn close_tab(&mut self, i: usize, force: bool) {
        if i >= self.tabs.len() {
            return;
        }
        if !force && self.tabs[i].buf.is_dirty() {
            self.dialog = Some(Dialog::CloseTab(self.tabs[i].id));
            return;
        }
        let group = self.tabs[i].group;
        self.tabs.remove(i);
        self.tab_removed(i);
        if self.tabs.is_empty() {
            self.new_tab(Buffer::new());
        }
        // Stay in the group the tab was closed from, if it still has tabs.
        match self.group_active.get(group).copied().flatten() {
            Some(t) if group == self.focus_group => self.activate(t),
            _ => self.focus_editor = true,
        }
    }

    fn alloc_tab_id(&mut self) -> u64 {
        self.next_tab_id += 1;
        self.next_tab_id
    }

    fn tab_index(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// Close several tabs, asking about each unsaved one in turn. Tabs are
    /// queued by id, so closing or moving tabs while a prompt is open can't
    /// redirect the batch to other tabs.
    fn close_many(&mut self, tabs: Vec<usize>) {
        self.close_queue = tabs.iter().filter_map(|&i| self.tabs.get(i).map(|t| t.id)).collect();
        self.process_close_queue();
    }

    fn process_close_queue(&mut self) {
        while self.dialog.is_none() {
            let Some(id) = self.close_queue.pop() else { break };
            // Tabs closed in the meantime are simply skipped.
            if let Some(i) = self.tab_index(id) {
                self.close_tab(i, false);
            }
        }
    }

    fn save(&mut self, i: usize) -> bool {
        let Some(t) = self.tabs.get_mut(i) else { return false };
        if t.buf.path.is_none() {
            self.activate(i);
            self.open_overlay(Mode::SaveAs);
            return false;
        }
        match t.buf.save() {
            Ok(()) => {
                let name = t.buf.name();
                // A new extension may mean a new syntax.
                let syntax = self.syn.detect(t.buf.path.as_deref(), &t.buf.line_str(0)).name.clone();
                if t.hl.syntax == "Plain Text" && syntax != t.hl.syntax {
                    t.hl.set_syntax(&syntax);
                }
                self.set_status(format!("Saved {name}"));
                // Live reload: refresh browser tabs showing local pages.
                for t in &self.tabs {
                    if let Some(b) = &t.browser {
                        let url = b.url();
                        if url.starts_with("file://") || url.contains("://localhost") || url.contains("://127.0.0.1") {
                            b.reload();
                        }
                    }
                }
                if let Some(root) = &self.root {
                    self.dir_cache.retain(|k, _| !k.starts_with(root) || k != root);
                }
                true
            }
            Err(e) => {
                self.set_status(format!("Unable to save: {e}"));
                false
            }
        }
    }

    // ------------------------------------------------------------------
    // Commands

    fn run(&mut self, cmd: Cmd, ctx: &egui::Context) {
        let mut reveal = true;
        match cmd {
            Cmd::NewFile => self.new_tab(Buffer::new()),
            Cmd::OpenFile => self.open_overlay(Mode::OpenFile),
            Cmd::OpenFolder => self.open_overlay(Mode::OpenFolder),
            Cmd::Save => {
                self.save(self.active);
            }
            Cmd::SaveAs => self.open_overlay(Mode::SaveAs),
            Cmd::SaveAll => {
                for i in 0..self.tabs.len() {
                    if self.tabs[i].buf.is_dirty() && self.tabs[i].buf.path.is_some() {
                        self.save(i);
                    }
                }
            }
            Cmd::CloseTab => self.close_tab(self.active, false),
            Cmd::NextTab => self.cycle_tab(true),
            Cmd::PrevTab => self.cycle_tab(false),
            Cmd::Layout(code) => self.set_layout(PaneLayout::from_code(code)),
            Cmd::FocusGroup(g) => self.focus_group(g as usize),
            Cmd::MoveToGroup(g) => self.move_to_group(self.active, g as usize),
            Cmd::Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
            Cmd::Cut => ctx.send_viewport_cmd(ViewportCommand::RequestCut),
            Cmd::Copy => ctx.send_viewport_cmd(ViewportCommand::RequestCopy),
            Cmd::Paste => ctx.send_viewport_cmd(ViewportCommand::RequestPaste),
            Cmd::CloseAll => self.close_many((0..self.tabs.len()).collect()),
            Cmd::About => {
                self.close_queue.clear();
                self.dialog = Some(Dialog::About);
            }
            Cmd::ToggleHotExit => {
                self.hot_exit = !self.hot_exit;
                self.set_status(if self.hot_exit { "Hot exit on" } else { "Hot exit off: unsaved files will prompt on quit" });
            }
            Cmd::SwitchTab(n) => self.switch_tab(n as usize),
            Cmd::CommandPalette => self.toggle_overlay(Mode::Commands, ""),
            Cmd::GotoAnything => self.toggle_overlay(Mode::Anything, ""),
            Cmd::GotoLine => self.toggle_overlay(Mode::Anything, ":"),
            Cmd::GotoSymbol => self.toggle_overlay(Mode::Anything, "@"),
            Cmd::Find | Cmd::Replace | Cmd::FindInFiles => {
                let sel = self.tab().map(|t| t.buf.sel_text(t.buf.sels.last().unwrap())).unwrap_or_default();
                if !sel.is_empty() && !sel.contains('\n') {
                    self.find.search.pattern = if self.find.search.regex { regex::escape(&sel) } else { sel };
                }
                self.find.open = true;
                self.find.mode = match cmd {
                    Cmd::Replace => FindMode::Replace,
                    Cmd::FindInFiles => FindMode::Files,
                    _ => FindMode::Find,
                };
                if cmd == Cmd::FindInFiles && self.find.where_.is_empty() {
                    self.find.where_ = self.default_where();
                }
                self.find.focus = true;
                reveal = false;
            }
            Cmd::ToggleRegex => self.find.search.regex = !self.find.search.regex,
            Cmd::ToggleCase => self.find.search.case = !self.find.search.case,
            Cmd::ToggleWholeWord => self.find.search.whole_word = !self.find.search.whole_word,
            Cmd::NextResult | Cmd::PrevResult => self.step_result(cmd == Cmd::NextResult),
            Cmd::FindNext | Cmd::FindPrev => self.find_step(cmd == Cmd::FindNext),
            Cmd::FindAllUnder => {
                let fallback = self.find.matcher();
                if let Some(t) = self.tab() {
                    let s = *t.buf.sels.last().unwrap();
                    let needle = if s.is_empty() {
                        let (a, b) = t.buf.word_at(s.head);
                        t.buf.slice(a, b)
                    } else {
                        t.buf.sel_text(&s)
                    };
                    // Like Sublime: the word under the caret matches whole words only.
                    let m = Search { pattern: needle, case: true, regex: false, whole_word: s.is_empty() }.compile().ok().or(fallback);
                    if let Some(m) = m {
                        let n = t.buf.find_all(&m);
                        self.set_status(format!("{n} matches"));
                    }
                }
            }
            Cmd::ToggleSidebar => self.show_sidebar = !self.show_sidebar,
            Cmd::ToggleMinimap => self.show_minimap = !self.show_minimap,
            Cmd::ToggleWordWrap => self.word_wrap = !self.word_wrap,
            Cmd::PreviewInEdi => match self.preview_target(self.active) {
                Some(url) => self.open_preview(&url),
                None => self.open_preview(""),
            },
            Cmd::OpenInBrowser => {
                let url = self.tabs.get(self.active).and_then(|t| t.browser.as_ref().map(|b| b.url()));
                match url.or_else(|| self.preview_target(self.active)) {
                    Some(url) => self.open_external(&url),
                    None => self.set_status("Nothing to open: not an HTML file, and no preview URL set for this project"),
                }
            }
            Cmd::ShowInFileManager => match self.tab().and_then(|t| t.buf.path.clone()).or_else(|| self.root.clone()) {
                Some(p) => match system::show_in_file_manager(&p) {
                    Ok(()) => self.set_status(format!("Showed {} in the file manager", p.display())),
                    Err(e) => self.set_status(format!("Unable to open the file manager: {e}")),
                },
                None => self.set_status("Nothing to show: the file isn't saved yet"),
            },
            Cmd::OpenTerminal => match self.file_dir() {
                Some(dir) => match system::open_terminal(&dir) {
                    Ok(()) => self.set_status(format!("Opened a terminal in {}", dir.display())),
                    Err(e) => self.set_status(format!("Unable to open a terminal: {e}")),
                },
                None => self.set_status("No folder: the file isn't saved yet"),
            },
            Cmd::NewBrowserTab => {
                let g = self.focus_group;
                self.add_browser_tab("", g);
                self.activate(self.tabs.len() - 1);
            }
            Cmd::SelectTheme => self.open_overlay(Mode::Theme { original: self.syn.theme }),
            Cmd::SetSyntax => self.open_overlay(Mode::Syntax),
            Cmd::ZoomIn => self.font_size = (self.font_size + 1.0).min(48.0),
            Cmd::ZoomOut => self.font_size = (self.font_size - 1.0).max(7.0),
            Cmd::ZoomReset => self.font_size = DEFAULT_FONT_SIZE,
            Cmd::PrettyPrint(fmt) => {
                let Some(t) = self.tabs.get_mut(self.active) else { return };
                match t.buf.pretty_print(fmt, &t.hl.syntax) {
                    Ok(f) => self.set_status(format!("Pretty printed as {}", f.name())),
                    Err(e) => self.set_status(e),
                }
            }
            _ => {
                let Some(t) = self.tabs.get_mut(self.active) else { return };
                let b = &mut t.buf;
                match cmd {
                    Cmd::Undo => b.undo(),
                    Cmd::Redo => b.redo(),
                    Cmd::SelectAll => b.select_all(),
                    Cmd::SelectLine => b.select_line(),
                    Cmd::FindUnderExpand => b.find_under_expand(),
                    Cmd::SplitSelectionIntoLines => b.split_into_lines(),
                    Cmd::SingleSelection => b.single_selection(),
                    Cmd::AddCursorAbove => b.add_cursor_vertical(false),
                    Cmd::AddCursorBelow => b.add_cursor_vertical(true),
                    Cmd::DuplicateLine => b.duplicate(),
                    Cmd::DeleteLine => b.delete_line(),
                    Cmd::SwapLineUp => b.swap_lines(true),
                    Cmd::SwapLineDown => b.swap_lines(false),
                    Cmd::ToggleComment => b.toggle_comment(comment_token(&t.hl.syntax)),
                    Cmd::JoinLines => b.join_lines(),
                    Cmd::InsertLineAfter => b.insert_line(false),
                    Cmd::InsertLineBefore => b.insert_line(true),
                    Cmd::Indent => b.indent(),
                    Cmd::Unindent => b.unindent(),
                    Cmd::SortLines => b.sort_lines(),
                    Cmd::UpperCase => b.transform_case(true),
                    Cmd::LowerCase => b.transform_case(false),
                    Cmd::GotoBracket => b.goto_matching_bracket(),
                    Cmd::Fold => b.fold_at_carets(),
                    Cmd::Unfold => b.unfold_at_carets(),
                    Cmd::FoldAll => b.fold_all(),
                    Cmd::UnfoldAll => b.unfold_all(),
                    _ => {}
                }
            }
        }
        if reveal {
            if let Some(t) = self.tabs.get_mut(self.active) {
                t.view.reveal = true;
            }
            if self.overlay.is_none() && !matches!(cmd, Cmd::Find | Cmd::Replace) {
                self.focus_editor = true;
            }
        }
    }

    fn find_step(&mut self, forward: bool) {
        let Some(m) = self.find.matcher() else {
            self.find.open = true;
            self.find.focus = true;
            return;
        };
        let pattern = self.find.search.pattern.clone();
        if let Some(t) = self.tab() {
            let found = t.buf.find_next(&m, forward);
            t.view.reveal = true;
            if !found {
                self.set_status(format!("Unable to find {pattern}"));
            }
        }
    }

    fn start_find_in_files(&mut self, m: Matcher) {
        let w = find_files::Where::parse(&self.find.where_, self.root.as_deref());
        let buffers = self
            .tabs
            .iter()
            .filter(|t| t.results.is_none())
            .map(|t| (t.buf.path.clone().unwrap_or_else(|| PathBuf::from(t.buf.name())), t.buf.rope.clone()))
            .collect();
        let s = &self.find.search;
        let mut flags = Vec::new();
        if s.regex {
            flags.push("regex");
        }
        if s.case {
            flags.push("case sensitive");
        }
        if s.whole_word {
            flags.push("whole word");
        }
        let flags = if flags.is_empty() { String::new() } else { format!(" ({})", flags.join(", ")) };
        let job = find_files::start(m.clone(), w, buffers);
        // Replacing a running job drops (and so cancels) it.
        self.fif = Some((job, m, s.pattern.clone(), flags));
        self.find.open = false;
        self.focus_editor = true;
    }

    fn poll_find_in_files(&mut self, ctx: &egui::Context) {
        let Some((job, ..)) = &self.fif else { return };
        let Ok(outcome) = job.rx.try_recv() else {
            let n = job.scanned.load(std::sync::atomic::Ordering::Relaxed);
            self.status = Some((format!("Searching… {n} files"), Instant::now()));
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        };
        let (_, matcher, pattern, flags) = self.fif.take().unwrap();
        let (text, locs) = find_files::render(&outcome, &pattern, &flags, self.root.as_deref());
        let hits: usize = outcome.files.iter().map(|f| f.hits.len()).sum();
        let buf = Buffer::scratch("Find Results", &text);
        let results = Results { locs, matcher, cursor: None };
        // Reuse an existing Find Results tab, like Sublime.
        match self.tabs.iter().position(|t| t.results.is_some()) {
            Some(i) => {
                self.tabs[i].buf = buf;
                self.tabs[i].hl.set_syntax("Plain Text");
                self.tabs[i].view = ViewState::default();
                self.tabs[i].results = Some(results);
                self.activate(i);
            }
            None => {
                self.new_tab(buf);
                self.tabs.last_mut().unwrap().results = Some(results);
            }
        }
        self.set_status(format!("{hits} matching lines in {} files", outcome.files.len()));
    }

    /// Open the location a Find Results line points to.
    fn open_result(&mut self, tab: usize, line: usize) -> bool {
        let Some(loc) = self.tabs.get(tab).and_then(|t| t.results.as_ref()?.locs.get(line)?.clone()) else {
            return false;
        };
        if let Some(r) = self.tabs[tab].results.as_mut() {
            r.cursor = Some(line);
        }
        let find_files::Loc { path, line: l, col: c, .. } = loc;
        if path.is_file() {
            self.open(&path);
        } else if let Some(i) = self.tabs.iter().position(|t| t.results.is_none() && t.buf.name() == path.to_string_lossy()) {
            self.activate(i); // an untitled buffer
        } else {
            return false;
        }
        let rows_per_line = (self.font_size * 1.45).round();
        if let Some(t) = self.tabs.get_mut(self.active) {
            let p = t.buf.lc_to_pos(l, c);
            t.buf.sels = vec![crate::buffer::Sel::caret(p)];
            t.view.scroll.y = l.saturating_sub(10) as f32 * rows_per_line;
            t.view.reveal = true;
        }
        self.focus_editor = true;
        true
    }

    /// F4 / Shift+F4: jump to the next or previous Find Results line.
    fn step_result(&mut self, forward: bool) {
        let Some(ri) = self.tabs.iter().position(|t| t.results.is_some()) else {
            self.set_status("No Find Results");
            return;
        };
        let r = self.tabs[ri].results.as_ref().unwrap();
        let is_hit = |i: usize| r.locs[i].as_ref().is_some_and(|l| l.is_match);
        let n = r.locs.len();
        let start = r.cursor;
        let order: Vec<usize> = if forward {
            let from = start.map_or(0, |c| c + 1);
            (from..n).chain(0..from).collect()
        } else {
            let from = start.unwrap_or(0);
            (0..from).rev().chain((from..n).rev()).collect()
        };
        match order.into_iter().find(|&i| is_hit(i)) {
            Some(line) => {
                if let Some(t) = self.tabs.get_mut(ri) {
                    let p = t.buf.line_start(line);
                    t.buf.sels = vec![crate::buffer::Sel::caret(p)];
                    t.view.reveal = true;
                }
                self.open_result(ri, line);
            }
            None => self.set_status("No results"),
        }
    }

    /// Folder of the active file, else the open folder.
    fn file_dir(&self) -> Option<PathBuf> {
        let file = self.tabs.get(self.active).and_then(|t| t.buf.path.as_ref()?.parent().map(Path::to_path_buf));
        file.or_else(|| self.root.clone())
    }

    /// Default "Where" for Find in Files: the open folder, else open files.
    fn default_where(&self) -> String {
        match &self.root {
            Some(r) => r.display().to_string(),
            None => OPEN_FILES.to_string(),
        }
    }

    // ------------------------------------------------------------------
    // Overlays

    fn toggle_overlay(&mut self, mode: Mode, prefix: &str) {
        if self.overlay.as_ref().is_some_and(|o| o.mode == mode && o.query == prefix) {
            self.close_overlay(false);
        } else {
            self.open_overlay(mode);
            if let Some(o) = &mut self.overlay {
                o.query = prefix.to_string();
            }
        }
    }

    fn open_overlay(&mut self, mode: Mode) {
        let query = match &mode {
            Mode::OpenFile | Mode::OpenFolder | Mode::SaveAs => {
                // A never-saved buffer is saved to the home folder by default.
                let untitled = mode == Mode::SaveAs && self.tab().is_some_and(|t| t.buf.path.is_none());
                let base = self
                    .tab()
                    .and_then(|t| t.buf.path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)))
                    .or_else(|| if untitled { std::env::home_dir() } else { None })
                    .or_else(|| self.root.clone())
                    .or_else(|| std::env::current_dir().ok())
                    .unwrap_or_else(|| PathBuf::from("/"));
                let mut s = base.to_string_lossy().into_owned();
                if !s.ends_with(['/', std::path::MAIN_SEPARATOR]) {
                    s.push(std::path::MAIN_SEPARATOR);
                }
                if mode == Mode::SaveAs {
                    if let Some(t) = self.tab() {
                        match &t.buf.path {
                            Some(p) => s.push_str(&p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                            None => s.push_str(&format!("{}.txt", suggested_stem(&t.buf))),
                        }
                    }
                }
                s
            }
            _ => String::new(),
        };
        self.overlay = Some(Overlay { mode, query, selected: 0, items: Vec::new(), computed_for: None, just_opened: true });
    }

    fn close_overlay(&mut self, accepted: bool) {
        if let Some(Overlay { mode: Mode::Theme { original }, .. }) = &self.overlay {
            if !accepted {
                self.syn.theme = *original;
            }
        }
        self.overlay = None;
        self.focus_editor = true;
    }

    fn compute_items(&self, mode: &Mode, query: &str) -> Vec<Item> {
        let mk = |label: String, detail: String, hits: Vec<usize>, action: Action| Item { label, detail, hits, action };
        match mode {
            Mode::Commands => {
                let mut seen = Vec::new();
                let cmds: Vec<_> = COMMANDS
                    .iter()
                    .filter(|c| !matches!(c.0, Cmd::SwitchTab(_)))
                    .filter(|c| !seen.contains(&c.0) && { seen.push(c.0); true })
                    .collect();
                fuzzy::rank(query, cmds.iter().map(|c| c.1), 200)
                    .into_iter()
                    .map(|(i, hits)| mk(cmds[i].1.to_string(), Keymap::label(cmds[i].0), hits, Action::Cmd(cmds[i].0)))
                    .collect()
            }
            Mode::Anything => {
                if let Some(rest) = query.strip_prefix(':') {
                    let n = rest.trim().parse().unwrap_or(0);
                    return vec![mk(format!("Go to line {rest}"), String::new(), vec![], Action::Line(n))];
                }
                if let Some(rest) = query.strip_prefix('@') {
                    return self.symbols(rest);
                }
                let (q, line) = match query.rsplit_once(':') {
                    Some((f, l)) if l.chars().all(|c| c.is_ascii_digit()) => (f, l.parse().ok()),
                    _ => (query, None),
                };
                let mut out: Vec<Item> = Vec::new();
                // Open tabs first, then project files.
                let names: Vec<String> = self.tabs.iter().map(|t| t.buf.name()).collect();
                for (i, hits) in fuzzy::rank(q, names.iter().map(String::as_str), 20) {
                    let detail = self.tabs[i].buf.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
                    let action = match (&self.tabs[i].buf.path, line) {
                        (Some(p), Some(_)) => Action::Open(p.clone(), line),
                        _ => Action::Tab(i),
                    };
                    out.push(mk(names[i].clone(), detail, hits, action));
                }
                if let Some(root) = &self.root {
                    let open: Vec<&PathBuf> = self.tabs.iter().filter_map(|t| t.buf.path.as_ref()).collect();
                    for (i, hits) in fuzzy::rank(q, self.files.iter().map(String::as_str), 100) {
                        let p = root.join(&self.files[i]);
                        if open.contains(&&p) {
                            continue;
                        }
                        let rel = &self.files[i];
                        let name = Path::new(rel).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let off = rel.chars().count() - name.chars().count();
                        let name_hits = hits.iter().filter(|&&h| h >= off).map(|h| h - off).collect();
                        out.push(mk(name, rel.clone(), name_hits, Action::Open(p, line)));
                    }
                }
                out
            }
            Mode::Theme { .. } => {
                let names: Vec<&str> = self.syn.themes.iter().map(|t| t.0.as_str()).collect();
                fuzzy::rank(query, names.iter().copied(), 100)
                    .into_iter()
                    .map(|(i, hits)| mk(names[i].to_string(), String::new(), hits, Action::Theme(i)))
                    .collect()
            }
            Mode::Syntax => {
                let names = self.syn.syntax_names();
                fuzzy::rank(query, names.iter().map(String::as_str), 300)
                    .into_iter()
                    .map(|(i, hits)| mk(names[i].clone(), String::new(), hits, Action::Syntax(names[i].clone())))
                    .collect()
            }
            Mode::OpenFile | Mode::OpenFolder | Mode::SaveAs => {
                let (dir, partial) = match query.rfind(['/', std::path::MAIN_SEPARATOR]) {
                    Some(i) => (&query[..=i], &query[i + 1..]),
                    None => ("./", query),
                };
                let dir = expand_home(dir);
                let mut entries: Vec<(String, bool)> = std::fs::read_dir(&dir)
                    .map(|rd| {
                        rd.flatten()
                            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path().is_dir()))
                            .filter(|(n, d)| (!n.starts_with('.') || partial.starts_with('.')) && (*d || *mode != Mode::OpenFolder))
                            .collect()
                    })
                    .unwrap_or_default();
                entries.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.to_lowercase().cmp(&b.0.to_lowercase())));
                let mut out: Vec<Item> = fuzzy::rank(partial, entries.iter().map(|e| e.0.as_str()), 300)
                    .into_iter()
                    .map(|(i, hits)| {
                        let (n, d) = &entries[i];
                        let label = if *d { format!("{n}{}", std::path::MAIN_SEPARATOR) } else { n.clone() };
                        mk(label, String::new(), hits, Action::Path(dir.join(n), *d))
                    })
                    .collect();
                if *mode != Mode::OpenFile {
                    let target = expand_home(query);
                    let label = if *mode == Mode::OpenFolder { "Open this folder" } else { "Save here" };
                    out.insert(0, mk(label.into(), target.display().to_string(), vec![], Action::Path(target, false)));
                }
                out
            }
        }
    }

    /// Heuristic symbol list for Goto Symbol.
    fn symbols(&self, query: &str) -> Vec<Item> {
        const KW: &[&str] = &[
            "fn ", "pub fn ", "async fn ", "pub async fn ", "pub(crate) fn ", "struct ", "pub struct ", "enum ", "pub enum ",
            "trait ", "pub trait ", "impl ", "mod ", "pub mod ", "def ", "async def ", "class ", "function ", "func ",
            "interface ", "type ", "export function ", "export class ", "export default function ", "macro_rules! ",
        ];
        let Some(t) = self.tabs.get(self.active) else { return vec![] };
        let mut syms: Vec<(String, usize)> = Vec::new();
        for l in 0..t.buf.line_count() {
            let line = t.buf.line_str(l);
            let trimmed = line.trim_start();
            if let Some(kw) = KW.iter().find(|k| trimmed.starts_with(*k)) {
                let rest = &trimmed[kw.len()..];
                let name: String = rest.chars().take_while(|c| !matches!(c, '(' | '{' | ':' | ';' | '=' | '<')).collect();
                let name = name.trim();
                if !name.is_empty() {
                    syms.push((name.to_string(), l));
                }
            }
        }
        fuzzy::rank(query, syms.iter().map(|s| s.0.as_str()), 300)
            .into_iter()
            .map(|(i, hits)| Item {
                label: syms[i].0.clone(),
                detail: format!("line {}", syms[i].1 + 1),
                hits,
                action: Action::Line(syms[i].1 + 1),
            })
            .collect()
    }

    fn accept(&mut self, action: Action, ctx: &egui::Context) {
        let mode = self.overlay.as_ref().map(|o| o.mode.clone());
        match action {
            Action::Cmd(c) => {
                self.close_overlay(true);
                self.run(c, ctx);
            }
            Action::Open(p, line) => {
                self.close_overlay(true);
                self.open(&p);
                if let Some(l) = line {
                    self.goto(l);
                }
            }
            Action::Tab(i) => {
                self.close_overlay(true);
                self.activate(i);
            }
            Action::Line(l) => {
                self.close_overlay(true);
                self.goto(l);
            }
            Action::Theme(i) => {
                self.syn.theme = i;
                self.apply_visuals(ctx);
                self.close_overlay(true);
            }
            Action::Syntax(name) => {
                if let Some(t) = self.tab() {
                    t.hl.set_syntax(&name);
                }
                self.close_overlay(true);
            }
            Action::Path(p, is_dir) => match mode {
                Some(Mode::OpenFolder) if !is_dir || p.is_dir() && self.overlay.as_ref().is_some_and(|o| o.selected == 0) => {
                    self.close_overlay(true);
                    self.set_root(p);
                }
                Some(_) if is_dir => {
                    if let Some(o) = &mut self.overlay {
                        o.query = format!("{}{}", p.display(), std::path::MAIN_SEPARATOR);
                        o.selected = 0;
                    }
                }
                Some(Mode::SaveAs) => {
                    self.close_overlay(true);
                    let i = self.active;
                    if let Some(t) = self.tabs.get_mut(i) {
                        t.buf.path = Some(p);
                    }
                    if self.save(i) {
                        let t = &mut self.tabs[i];
                        let syntax = self.syn.detect(t.buf.path.as_deref(), &t.buf.line_str(0)).name.clone();
                        t.hl.set_syntax(&syntax);
                    }
                }
                _ => {
                    self.close_overlay(true);
                    self.open(&p);
                }
            },
        }
    }

    fn goto(&mut self, line: usize) {
        if let Some(t) = self.tabs.get_mut(self.active) {
            t.buf.goto_line(line);
            t.view.reveal = true;
            // Centre the target line like Sublime does.
            t.view.scroll.y = (line.saturating_sub(10)) as f32 * (self.font_size * 1.45).round();
        }
    }

    fn overlay_ui(&mut self, ctx: &egui::Context) {
        let Some(ov) = &mut self.overlay else { return };
        if ov.computed_for.as_deref() != Some(&ov.query) {
            let (mode, q) = (ov.mode.clone(), ov.query.clone());
            let items = self.compute_items(&mode, &q);
            let ov = self.overlay.as_mut().unwrap();
            ov.items = items;
            ov.selected = 0;
            ov.computed_for = Some(q.clone());
            // Live preview for ":line".
            if let (Mode::Anything, Some(rest)) = (&mode, q.strip_prefix(':')) {
                if let Ok(n) = rest.trim().parse::<usize>() {
                    self.goto(n);
                }
            }
        }
        let ov = self.overlay.as_mut().unwrap();
        let n = ov.items.len();
        let (down, up, enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if n > 0 {
            if down {
                ov.selected = (ov.selected + 1) % n;
            }
            if up {
                ov.selected = (ov.selected + n - 1) % n;
            }
        }
        if esc {
            self.close_overlay(false);
            return;
        }
        // Colour-scheme preview follows the selection.
        if let (Mode::Theme { .. }, Some(Item { action: Action::Theme(i), .. })) = (&ov.mode, ov.items.get(ov.selected)) {
            if self.syn.theme != *i {
                self.syn.theme = *i;
                self.apply_visuals(ctx);
            }
        }
        let ov = self.overlay.as_mut().unwrap();
        if enter {
            let action = ov.items.get(ov.selected).map(|i| i.action.clone()).or_else(|| match ov.mode {
                Mode::SaveAs | Mode::OpenFile => Some(Action::Path(expand_home(&ov.query), false)),
                _ => None,
            });
            if let Some(a) = action {
                self.accept(a, ctx);
            }
            return;
        }

        let width = (ctx.content_rect().width() - 40.0).clamp(300.0, 640.0);
        let mut clicked = None;
        let hint = match ov.mode {
            Mode::Commands => "Command Palette",
            Mode::Anything => "Goto Anything — file, :line, @symbol",
            Mode::Theme { .. } => "Select Color Scheme",
            Mode::Syntax => "Set Syntax",
            Mode::OpenFile => "Open File",
            Mode::OpenFolder => "Open Folder",
            Mode::SaveAs => "Save As",
        };
        egui::Area::new(egui::Id::unique("overlay"))
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 36.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(8.0).show(ui, |ui| {
                    ui.set_width(width);
                    let te = ui.add(
                        TextEdit::singleline(&mut ov.query)
                            .hint_text(hint)
                            .desired_width(f32::INFINITY)
                            .font(FontId::proportional(16.0)),
                    );
                    if ov.just_opened {
                        te.request_focus();
                        // Place the cursor at the end of any prefilled text.
                        if let Some(mut state) = TextEdit::load_state(ctx, te.id) {
                            let end = egui::text::CCursor::new(ov.query.chars().count());
                            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
                            state.store(ctx, te.id);
                        }
                        ov.just_opened = false;
                    }
                    ui.add_space(4.0);
                    let rows = 12usize;
                    let start = ov.selected.saturating_sub(rows - 1);
                    for (i, item) in ov.items.iter().enumerate().skip(start).take(rows) {
                        let selected = i == ov.selected;
                        let h = if item.detail.is_empty() || ov.mode == Mode::Commands { 26.0 } else { 40.0 };
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, h), Sense::click());
                        if selected {
                            ui.painter().rect_filled(rect, CornerRadius::same(4), ui.visuals().selection.bg_fill);
                        } else if resp.hovered() {
                            ui.painter().rect_filled(rect, CornerRadius::same(4), ui.visuals().widgets.hovered.bg_fill);
                        }
                        let text_color = if selected { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
                        let accent = Color32::from_rgb(0xf0, 0xb4, 0x29);
                        let mut job = egui::text::LayoutJob::default();
                        for (ci, ch) in item.label.chars().enumerate() {
                            let hit = item.hits.contains(&ci);
                            job.append(
                                &ch.to_string(),
                                0.0,
                                egui::TextFormat {
                                    font_id: FontId::proportional(14.0),
                                    color: if hit { accent } else { text_color },
                                    ..Default::default()
                                },
                            );
                        }
                        let galley = ui.painter().layout_job(job);
                        let top = if h > 30.0 { rect.top() + 4.0 } else { rect.center().y - galley.size().y / 2.0 };
                        ui.painter().galley(egui::pos2(rect.left() + 8.0, top), galley, text_color);
                        if !item.detail.is_empty() {
                            let weak = ui.visuals().weak_text_color();
                            if ov.mode == Mode::Commands {
                                ui.painter().text(rect.right_center() - Vec2::new(8.0, 0.0), Align2::RIGHT_CENTER, &item.detail, FontId::monospace(12.0), weak);
                            } else {
                                ui.painter().text(egui::pos2(rect.left() + 8.0, rect.bottom() - 5.0), Align2::LEFT_BOTTOM, &item.detail, FontId::proportional(11.5), weak);
                            }
                        }
                        if resp.clicked() {
                            clicked = Some(item.action.clone());
                        }
                    }
                    if ov.items.is_empty() {
                        ui.weak("No matches");
                    }
                });
            });
        if let Some(a) = clicked {
            self.accept(a, ctx);
        }
    }

    // ------------------------------------------------------------------
    // Panels

    fn sidebar_ui(&mut self, ui: &mut Ui) {
        let Some(root) = self.root.clone() else {
            ui.add_space(8.0);
            ui.weak("No folder open");
            if ui.button("Open Folder…").clicked() {
                self.open_overlay(Mode::OpenFolder);
            }
            ui.add_space(8.0);
            ui.label(RichText::new("OPEN FILES").small().weak());
            let mut switch = None;
            for (i, t) in self.tabs.iter().enumerate() {
                if ui.selectable_label(i == self.active, t.buf.name()).clicked() {
                    switch = Some(i);
                }
            }
            if let Some(i) = switch {
                self.activate(i);
            }
            return;
        };
        ui.add_space(4.0);
        ui.label(RichText::new("FOLDERS").small().weak());
        let active_path = self.tabs.get(self.active).and_then(|t| t.buf.path.clone());
        let mut open = None;
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
            egui::CollapsingHeader::new(RichText::new(name).strong())
                .id_salt(&root)
                .default_open(true)
                .show(ui, |ui| self.dir_ui(ui, &root, active_path.as_deref(), &mut open));
        });
        if let Some(p) = open {
            self.open(&p);
        }
    }

    fn dir_ui(&mut self, ui: &mut Ui, dir: &Path, active: Option<&Path>, open: &mut Option<PathBuf>) {
        let entries = self
            .dir_cache
            .entry(dir.to_path_buf())
            .or_insert_with(|| {
                let mut v: Vec<(PathBuf, bool)> = std::fs::read_dir(dir)
                    .map(|rd| {
                        rd.flatten()
                            .filter(|e| !matches!(e.file_name().to_str(), Some(".git" | ".DS_Store" | "target" | "node_modules")))
                            .map(|e| (e.path(), e.path().is_dir()))
                            .collect()
                    })
                    .unwrap_or_default();
                v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.file_name().cmp(&b.0.file_name())));
                v
            })
            .clone();
        for (path, is_dir) in entries {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if is_dir {
                egui::CollapsingHeader::new(name).id_salt(&path).default_open(false).show(ui, |ui| self.dir_ui(ui, &path, active, open));
            } else if ui.selectable_label(active == Some(path.as_path()), name).clicked() {
                *open = Some(path);
            }
        }
    }

    fn find_ui(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let mut action: Option<&str> = None;
        let mode = self.find.mode;
        let label_w = 64.0;
        let buttons_w = if mode == FindMode::Files { 120.0 } else { 250.0 };
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let s = &mut self.find.search;
            for (on, text, tip) in [
                (&mut s.regex, ".*", "Regular expression (Alt+R)"),
                (&mut s.case, "Aa", "Case sensitive (Alt+C)"),
                (&mut s.whole_word, "“”", "Whole word (Alt+W)"),
            ] {
                if ui.selectable_label(*on, RichText::new(text).monospace()).on_hover_text(tip).clicked() {
                    *on = !*on;
                    action = Some("incremental");
                }
            }
            ui.add_sized([label_w, 20.0], egui::Label::new("Find:"));
            let w = (ui.available_width() - buttons_w).max(120.0);
            let te = ui.add(
                TextEdit::singleline(&mut self.find.search.pattern).hint_text("Find").desired_width(w).font(FontId::monospace(13.0)),
            );
            if self.find.focus {
                te.request_focus();
                self.find.focus = false;
            }
            if te.changed() {
                action = Some("incremental");
            }
            if te.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                action = Some(if mode == FindMode::Files {
                    "files"
                } else if ui.input(|i| i.modifiers.shift) {
                    "prev"
                } else if ui.input(|i| i.modifiers.alt) {
                    "all"
                } else {
                    "next"
                });
                te.request_focus();
            }
            if (te.has_focus() || te.lost_focus()) && ui.input(|i| i.key_pressed(Key::Escape)) {
                action = Some("close");
            }
            if mode == FindMode::Files {
                if ui.button("Find").clicked() {
                    action = Some("files");
                }
            } else {
                if ui.button("Find").clicked() {
                    action = Some("next");
                }
                if ui.button("Find Prev").clicked() {
                    action = Some("prev");
                }
                if ui.button("Find All").clicked() {
                    action = Some("all");
                }
            }
            if ui.button("×").clicked() {
                action = Some("close");
            }
        });
        if mode == FindMode::Files {
            ui.horizontal(|ui| {
                ui.add_space(96.0);
                ui.add_sized([label_w, 20.0], egui::Label::new("Where:"));
                let w = (ui.available_width() - buttons_w).max(120.0);
                let te = ui.add(
                    TextEdit::singleline(&mut self.find.where_)
                        .hint_text("Folders and filters, e.g. src, *.rs, -*.lock, <open files>")
                        .desired_width(w)
                        .font(FontId::monospace(13.0)),
                );
                if te.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    action = Some("files");
                }
                if (te.has_focus() || te.lost_focus()) && ui.input(|i| i.key_pressed(Key::Escape)) {
                    action = Some("close");
                }
            });
        }
        if mode == FindMode::Replace {
            ui.horizontal(|ui| {
                ui.add_space(96.0);
                ui.add_sized([label_w, 20.0], egui::Label::new("Replace:"));
                let w = (ui.available_width() - buttons_w).max(120.0);
                let te = ui.add(
                    TextEdit::singleline(&mut self.find.replacement).hint_text("Replace").desired_width(w).font(FontId::monospace(13.0)),
                );
                if te.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    action = Some("replace");
                    te.request_focus();
                }
                if ui.button("Replace").clicked() {
                    action = Some("replace");
                }
                if ui.button("Replace All").clicked() {
                    action = Some("replace_all");
                }
            });
        }
        let matcher = self.find.matcher();
        if let Some(err) = self.find.error() {
            ui.horizontal(|ui| {
                ui.add_space(96.0 + label_w + 8.0);
                ui.label(RichText::new(err).small().color(ui.visuals().error_fg_color));
            });
        }
        ui.add_space(4.0);

        let Some(act) = action else { return };
        if act == "close" {
            self.find.open = false;
            self.focus_editor = true;
            return;
        }
        if act == "files" {
            match matcher {
                Some(m) => self.start_find_in_files(m),
                None => self.set_status("Enter a valid search"),
            }
            return;
        }
        if mode == FindMode::Files {
            return;
        }
        let Some(m) = matcher else { return };
        let r = self.find.replacement.clone();
        let Some(t) = self.tabs.get_mut(self.active) else { return };
        let mut status = None;
        match act {
            "incremental" => {
                let start = t.buf.sels.last().unwrap().start();
                t.buf.sels = vec![crate::buffer::Sel::caret(start)];
                t.buf.find_next(&m, true);
            }
            "next" => {
                t.buf.find_next(&m, true);
            }
            "prev" => {
                t.buf.find_next(&m, false);
            }
            "all" => {
                let n = t.buf.find_all(&m);
                self.find.open = false;
                self.focus_editor = true;
                status = Some(format!("{n} matches"));
            }
            "replace" => t.buf.replace_next(&m, &r),
            "replace_all" => {
                let n = t.buf.replace_all_matches(&m, &r);
                status = Some(format!("Replaced {n} occurrences"));
            }
            _ => {}
        }
        if let Some(t) = self.tabs.get_mut(self.active) {
            t.view.reveal = true;
        }
        if let Some(s) = status {
            self.set_status(s);
        }
        ctx.request_repaint();
    }

    fn status_ui(&mut self, ui: &mut Ui) {
        let Some(t) = self.tabs.get(self.active) else { return };
        let b = &t.buf;
        let s = b.sels.last().unwrap();
        let (line, col) = b.pos_to_lc(s.head);
        let mut left = if b.sels.len() > 1 {
            format!("{} selection regions", b.sels.len())
        } else if !s.is_empty() {
            format!("{} characters selected", s.end() - s.start())
        } else {
            format!("Line {}, Column {}", line + 1, b.visual_col(line, col) + 1)
        };
        if self.keymap.is_pending() {
            left.push_str("   ctrl+k was pressed, waiting for second key…");
        }
        if let Some((msg, at)) = &self.status {
            if at.elapsed().as_secs() < 4 {
                left.push_str("   ");
                left.push_str(msg);
            }
        }
        let syntax = t.hl.syntax.clone();
        let mut set_syntax = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(left).small());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new(&syntax).small()).frame(false)).clicked() {
                    set_syntax = true;
                }
                ui.label(RichText::new(format!("Spaces: {}", crate::buffer::TAB_SIZE)).small());
            });
        });
        if set_syntax {
            self.open_overlay(Mode::Syntax);
        }
    }

    fn dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(d) = &self.dialog else { return };
        let (title, msg) = match d {
            Dialog::CloseTab(id) => {
                let name = self.tab_index(*id).map(|i| self.tabs[i].buf.name()).unwrap_or_default();
                ("Unsaved changes", format!("Save changes to {name} before closing?"))
            }
            Dialog::Quit => {
                let n = self.tabs.iter().filter(|t| t.buf.is_dirty()).count();
                ("Quit", format!("{n} file(s) have unsaved changes. Quit anyway?"))
            }
            Dialog::About => (
                "About edi",
                format!("edi {}\nA Sublime-style text editor written in Rust.", env!("CARGO_PKG_VERSION")),
            ),
        };
        let mut choice = None;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(msg);
                ui.add_space(8.0);
                ui.horizontal(|ui| match d {
                    Dialog::CloseTab(_) => {
                        if ui.button("Save").clicked() {
                            choice = Some("save");
                        }
                        if ui.button("Don't Save").clicked() {
                            choice = Some("discard");
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some("cancel");
                        }
                    }
                    Dialog::About => {
                        if ui.button("OK").clicked() {
                            choice = Some("cancel");
                        }
                    }
                    Dialog::Quit => {
                        if ui.button("Save All & Quit").clicked() {
                            choice = Some("save");
                        }
                        if ui.button("Discard & Quit").clicked() {
                            choice = Some("discard");
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some("cancel");
                        }
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            choice = Some("cancel");
        }
        let Some(c) = choice else { return };
        let d = self.dialog.take().unwrap();
        match (d, c) {
            (Dialog::CloseTab(id), "save") => match self.tab_index(id) {
                Some(i) if self.save(i) => self.close_tab(i, true),
                // Save As still pending, or the save failed: stop the batch
                // rather than closing more tabs behind it.
                _ => self.close_queue.clear(),
            },
            (Dialog::CloseTab(id), "discard") => {
                if let Some(i) = self.tab_index(id) {
                    self.close_tab(i, true);
                }
            }
            (Dialog::Quit, "save") => {
                let all_saved = (0..self.tabs.len()).filter(|&i| self.tabs[i].buf.is_dirty()).collect::<Vec<_>>().into_iter().all(|i| self.save(i));
                if all_saved {
                    self.quit_confirmed = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            (Dialog::Quit, "discard") => {
                self.quit_confirmed = true;
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            // Cancelling one file's prompt cancels the rest of a multi-close.
            (Dialog::CloseTab(_), _) => self.close_queue.clear(),
            _ => {}
        }
        self.focus_editor = true;
    }

    /// Route key events through the keymap; matched ones are removed so the
    /// editor view never sees them.
    fn dispatch_keys(&mut self, ctx: &egui::Context) {
        let groups = self.layout.groups();
        let editor_focused = ctx.memory(|m| m.focused().is_none_or(|f| (0..groups).any(|g| f == editor_id(g))));
        let mut cmds = Vec::new();
        ctx.input_mut(|i| {
            i.events.retain(|ev| {
                let Event::Key { key, physical_key, pressed: true, modifiers, .. } = ev else { return true };
                // Shift+digit produces a symbol on most layouts; bindings like
                // Alt+Shift+2 mean the digit key itself.
                let is_digit = |k: &Key| matches!(k, Key::Num0 | Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 | Key::Num6 | Key::Num7 | Key::Num8 | Key::Num9);
                let key = match physical_key {
                    Some(p) if is_digit(p) && (modifiers.ctrl || modifiers.alt || modifiers.command) => p,
                    _ => key,
                };
                let stroke = Stroke::from_event(*key, *modifiers);
                // In text fields, only bindings with ctrl/alt or function keys apply.
                let fkey = matches!(key, Key::F1 | Key::F2 | Key::F3 | Key::F4 | Key::F9);
                if !editor_focused && !(stroke.ctrl || stroke.alt || fkey) {
                    return true;
                }
                if !editor_focused && stroke.ctrl && matches!(key, Key::A | Key::Z | Key::Y) {
                    return true;
                }
                match self.keymap.press(stroke) {
                    KeyResult::Cmd(c) => {
                        cmds.push(c);
                        false
                    }
                    KeyResult::Pending => false,
                    KeyResult::None => true,
                }
            })
        });
        for c in cmds {
            self.run(c, ctx);
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let t = self.tabs.get(self.active).map(|t| {
            let name = t.buf.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| t.buf.name());
            format!("{}{} — edi", name, if t.buf.is_dirty() { " •" } else { "" })
        });
        let t = t.unwrap_or_else(|| "edi".into());
        if t != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(t.clone()));
            self.title = t;
        }
    }
}

impl EdiApp {
    /// Development aid: `EDI_RUN` (comma-separated palette captions),
    /// `EDI_QUERY` (overlay text) and `EDI_SCREENSHOT` (PPM path, then exit).
    fn debug_hooks(&mut self, ctx: &egui::Context) {
        if std::env::var_os("EDI_SCREENSHOT").is_some() {
            // Never let stray keystrokes reach an automated run.
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, Event::Key { .. } | Event::Text(_))));
        }
        if self.frame == 5 {
            if let Ok(run) = std::env::var("EDI_RUN") {
                for name in run.split(',') {
                    if let Some(c) = COMMANDS.iter().find(|c| c.1 == name.trim()) {
                        self.run(c.0, ctx);
                    }
                }
            }
            if let Ok(q) = std::env::var("EDI_QUERY") {
                match &mut self.overlay {
                    Some(o) => o.query = q,
                    None => self.find.search.pattern = q,
                }
            }
        }
        if self.frame == screenshot_frame() {
            if let Ok(path) = std::env::var("EDI_SCREENSHOT") {
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                    let mut out = format!("P6 {} {} 255\n", img.size[0], img.size[1]).into_bytes();
                    for p in &img.pixels {
                        out.extend_from_slice(&[p.r(), p.g(), p.b()]);
                    }
                    let _ = std::fs::write(&path, out);
                    std::process::exit(0);
                })));
            }
        }
        if self.frame < screenshot_frame() + 20 && std::env::var_os("EDI_SCREENSHOT").is_some() {
            ctx.request_repaint();
        }
    }
}

/// What one `EDI_POINTER` step does at its position.
#[derive(Clone, Copy)]
enum PointerStep {
    Move,
    Click(egui::PointerButton),
    /// Press or release the primary button, for drags.
    Press,
    Release,
}

/// `EDI_POINTER`: `;`-separated steps, `c:x,y` (click), `r:x,y` (right
/// click), `m:x,y` (move), `p:x,y` / `u:x,y` (press / release, for drags),
/// one every 8 frames starting at frame 10.
fn pointer_steps() -> &'static [(PointerStep, egui::Pos2)] {
    static STEPS: std::sync::OnceLock<Vec<(PointerStep, egui::Pos2)>> = std::sync::OnceLock::new();
    STEPS.get_or_init(|| {
        let spec = std::env::var("EDI_POINTER").unwrap_or_default();
        spec.split(';')
        .filter_map(|step| {
            let (kind, xy) = step.split_once(':')?;
            let (x, y) = xy.split_once(',')?;
            let button = match kind {
                "c" => PointerStep::Click(egui::PointerButton::Primary),
                "r" => PointerStep::Click(egui::PointerButton::Secondary),
                "p" => PointerStep::Press,
                "u" => PointerStep::Release,
                _ => PointerStep::Move,
            };
            Some((button, egui::pos2(x.trim().parse().ok()?, y.trim().parse().ok()?)))
        })
        .collect()
    })
}

fn screenshot_frame() -> u64 {
    40.max(10 + pointer_steps().len() as u64 * 8 + 20)
}

/// File name stem for saving an untitled buffer: its tab name, minus
/// characters that aren't allowed in file names.
fn suggested_stem(buf: &Buffer) -> String {
    let s: String = buf.name().chars().filter(|c| !c.is_control() && !r#"/\:*?"<>|"#.contains(*c)).collect();
    let s = s.trim().trim_end_matches('.').trim();
    if s.is_empty() { "untitled".into() } else { s.into() }
}

fn expand_home(s: &str) -> PathBuf {
    match (s.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(s),
    }
}

impl eframe::App for EdiApp {
    fn on_exit(&mut self) {
        let snap = self.snapshot();
        if let Some(mut w) = self.session.take() {
            w.finish(snap);
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        if self.frame < 10 {
            return;
        }
        let i = (self.frame - 10) as usize;
        let Some(&(step, pos)) = pointer_steps().get(i / 8) else { return };
        let press = |button, pressed| Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE };
        let primary = egui::PointerButton::Primary;
        match (i % 8, step) {
            (0, _) => raw.events.push(Event::PointerMoved(pos)),
            (1, PointerStep::Click(b)) => raw.events.push(press(b, true)),
            (2, PointerStep::Click(b)) => raw.events.push(press(b, false)),
            (1, PointerStep::Press) => raw.events.push(press(primary, true)),
            (1, PointerStep::Release) => raw.events.push(press(primary, false)),
            _ => {}
        }
    }

    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        crate::browser::pump();
        self.frame += 1;
        self.debug_hooks(&ctx);

        self.take_forwarded(&ctx);
        self.poll_find_in_files(&ctx);
        if let Some(rx) = &self.files_rx {
            if let Ok(files) = rx.try_recv() {
                self.files = files;
                self.files_rx = None;
                if let Some(o) = &mut self.overlay {
                    o.computed_for = None;
                }
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }

        // Dropped files open in tabs; dropped folders become the project.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        for p in dropped {
            if p.is_dir() { self.set_root(p) } else { self.open(&p) }
        }

        if ctx.input(|i| i.viewport().close_requested())
            && !self.hot_exit
            && !self.quit_confirmed
            && self.tabs.iter().any(|t| t.buf.is_dirty())
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            // Quitting supersedes a pending multi-close.
            self.close_queue.clear();
            self.dialog = Some(Dialog::Quit);
        }

        if self.dialog.is_none() {
            self.dispatch_keys(&ctx);
        }
        self.process_close_queue();
        if let Some(l) = self.pending_line.take() {
            self.goto(l);
        }
        self.overlay_ui(&ctx);
        self.dialog_ui(&ctx);
        self.update_title(&ctx);
        self.autosave(&ctx);

        egui::Panel::top("menu")
            .frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(4, 2)))
            .show(ui, |ui| self.menu_ui(ui));
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(8, 3)))
            .show(ui, |ui| self.status_ui(ui));
        if self.find.open {
            egui::Panel::bottom("find").frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(8, 2))).show(ui, |ui| self.find_ui(ui, &ctx));
        }
        if self.show_sidebar {
            egui::Panel::left("sidebar").resizable(true).default_size(220.0).size_range(120.0..=500.0).show(ui, |ui| self.sidebar_ui(ui));
        }

        // Escape in the editor also dismisses the find panel.
        let groups = self.layout.groups();
        let editor_has_focus = ctx.memory(|m| m.focused().is_some_and(|f| (0..groups).any(|g| f == editor_id(g))));
        if self.find.open && editor_has_focus && ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.find.open = false;
        }
        let grab = self.focus_editor && self.overlay.is_none() && self.dialog.is_none() && !self.find.focus;
        self.focus_editor = false;
        egui::CentralPanel::no_frame().show(ui, |ui| self.groups_ui(ui, grab, frame));
    }
}
