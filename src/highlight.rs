//! Syntax highlighting via syntect, with an incremental per-line cache.

use crate::buffer::Buffer;
use eframe::egui::Color32;
use std::io::Cursor;
use std::path::Path;
use std::time::{Duration, Instant};
use syntect::highlighting::{HighlightIterator, HighlightState, Highlighter, Theme, ThemeSet};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

/// A Monokai-style colour scheme, the classic Sublime look.
const MONOKAI: &str = include_str!("../assets/Monokai.tmTheme");

pub struct Syntaxes {
    pub ss: SyntaxSet,
    pub themes: Vec<(String, Theme)>,
    pub theme: usize,
}

pub struct Palette {
    pub bg: Color32,
    pub fg: Color32,
    pub caret: Color32,
    pub line_highlight: Color32,
    pub selection: Color32,
    pub gutter_fg: Color32,
}

fn c32(c: syntect::highlighting::Color) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

impl Syntaxes {
    pub fn load() -> Self {
        let ss = SyntaxSet::load_defaults_newlines();
        let mut themes = Vec::new();
        if let Ok(t) = ThemeSet::load_from_reader(&mut Cursor::new(MONOKAI)) {
            themes.push(("Monokai".to_string(), t));
        }
        let mut defaults: Vec<_> = ThemeSet::load_defaults().themes.into_iter().collect();
        defaults.sort_by(|a, b| a.0.cmp(&b.0));
        themes.extend(defaults);
        Syntaxes { ss, themes, theme: 0 }
    }

    pub fn theme(&self) -> &Theme {
        &self.themes[self.theme].1
    }

    pub fn palette(&self) -> Palette {
        let s = &self.theme().settings;
        let bg = s.background.map(c32).unwrap_or(Color32::from_rgb(0x27, 0x28, 0x22));
        let fg = s.foreground.map(c32).unwrap_or(Color32::from_rgb(0xf8, 0xf8, 0xf2));
        let dark = bg.r() as u32 + bg.g() as u32 + bg.b() as u32 <= 384;
        let selection = s.selection.map(c32).unwrap_or(Color32::from_rgb(0x49, 0x48, 0x3e));
        Palette {
            bg,
            fg,
            caret: s.caret.map(c32).unwrap_or(fg),
            line_highlight: s
                .line_highlight
                .map(c32)
                .unwrap_or(if dark { Color32::from_white_alpha(10) } else { Color32::from_black_alpha(10) }),
            selection,
            gutter_fg: s.gutter_foreground.map(c32).unwrap_or(fg.linear_multiply(0.45)),
        }
    }

    pub fn dark(&self) -> bool {
        let bg = self.palette().bg;
        bg.r() as u32 + bg.g() as u32 + bg.b() as u32 <= 384
    }

    pub fn detect(&self, path: Option<&Path>, first_line: &str) -> &SyntaxReference {
        path.and_then(|p| self.ss.find_syntax_for_file(p).ok().flatten())
            .or_else(|| self.ss.find_syntax_by_first_line(first_line))
            .unwrap_or_else(|| self.ss.find_syntax_plain_text())
    }

    pub fn by_name(&self, name: &str) -> &SyntaxReference {
        self.ss.find_syntax_by_name(name).unwrap_or_else(|| self.ss.find_syntax_plain_text())
    }

    /// Sorted names of user-visible syntaxes.
    pub fn syntax_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .ss
            .syntaxes()
            .iter()
            .filter(|s| !s.hidden)
            .map(|s| s.name.clone())
            .collect();
        v.sort_by_key(|s| s.to_lowercase());
        v.dedup();
        v
    }
}

/// Line comment token for a syntax, used by toggle comment.
pub fn comment_token(syntax: &str) -> &'static str {
    match syntax {
        "Python" | "Ruby" | "Ruby on Rails" | "Perl" | "R" | "YAML" | "TOML" | "Makefile" | "Bourne Again Shell (bash)"
        | "Shell-Unix-Generic" | "Graphviz (DOT)" | "Tcl" | "Git Ignore" | "Git Config" => "#",
        "SQL" | "Lua" | "Haskell" | "Literate Haskell" => "--",
        "LaTeX" | "TeX" | "Erlang" | "MATLAB" => "%",
        "Lisp" | "Clojure" => ";",
        "Batch File" => "REM",
        "HTML" | "XML" | "Markdown" => "<!--",
        _ => "//",
    }
}

#[derive(Clone)]
pub struct Span {
    pub chars: usize,
    pub color: Color32,
}

pub struct HlCache {
    pub syntax: String,
    theme: usize,
    /// `states[i]` is the parser state at the start of line `i`.
    states: Vec<(ParseState, HighlightState)>,
    lines: Vec<Vec<Span>>,
}

impl HlCache {
    pub fn new(syntax: &str) -> Self {
        HlCache { syntax: syntax.to_string(), theme: usize::MAX, states: Vec::new(), lines: Vec::new() }
    }

    pub fn set_syntax(&mut self, syntax: &str) {
        self.syntax = syntax.to_string();
        self.lines.clear();
        self.states.clear();
    }

    pub fn line(&self, i: usize) -> Option<&[Span]> {
        self.lines.get(i).map(|v| v.as_slice())
    }

    /// Bring the cache up to `upto` (exclusive). Returns false if the time
    /// budget ran out before it got there.
    pub fn update(&mut self, buf: &mut Buffer, syn: &Syntaxes, upto: usize, budget: Duration) -> bool {
        if self.theme != syn.theme {
            self.theme = syn.theme;
            self.lines.clear();
            self.states.clear();
        }
        if let Some(l) = buf.changed_from_line.take() {
            self.lines.truncate(l);
            self.states.truncate(l + 1);
        }
        let upto = upto.min(buf.line_count());
        if self.lines.len() >= upto {
            return true;
        }
        let syntax = syn.by_name(&self.syntax);
        let highlighter = Highlighter::new(syn.theme());
        if self.states.is_empty() {
            self.states.push((ParseState::new(syntax), HighlightState::new(&highlighter, ScopeStack::new())));
        }
        let start = Instant::now();
        while self.lines.len() < upto {
            let i = self.lines.len();
            let (mut ps, mut hs) = self.states[i].clone();
            let mut text: String = buf.rope.line(i).into();
            if !text.ends_with('\n') {
                text.push('\n');
            }
            let spans = match ps.parse_line(&text, &syn.ss) {
                Ok(ops) => HighlightIterator::new(&mut hs, &ops, &text, &highlighter)
                    .map(|(style, s)| Span { chars: s.chars().count(), color: c32(style.foreground) })
                    .collect(),
                Err(_) => vec![Span { chars: text.chars().count(), color: syn.palette().fg }],
            };
            self.lines.push(spans);
            self.states.push((ps, hs));
            if i % 64 == 0 && start.elapsed() > budget {
                return self.lines.len() >= upto;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn loads_syntaxes_and_monokai() {
        let s = super::Syntaxes::load();
        assert_eq!(s.themes[0].0, "Monokai");
        assert!(s.syntax_names().iter().any(|n| n == "Rust"));
    }
}
