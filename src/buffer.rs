//! Text buffer: a rope plus a set of selections and snapshot-based undo.
//!
//! All positions are char indices into the rope. Every editing operation is
//! applied to all selections at once, which is what gives the editor its
//! multi-cursor behaviour.

use ropey::Rope;
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const TAB_SIZE: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sel {
    pub anchor: usize,
    pub head: usize,
    /// Remembered visual column for vertical movement.
    pub goal: Option<usize>,
}

impl Sel {
    pub fn caret(pos: usize) -> Self {
        Sel { anchor: pos, head: pos, goal: None }
    }
    pub fn new(anchor: usize, head: usize) -> Self {
        Sel { anchor, head, goal: None }
    }
    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }
    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    Insert,
    Delete,
    Other,
}

#[derive(Clone)]
struct Snapshot {
    rope: Rope,
    sels: Vec<Sel>,
}

pub struct Buffer {
    pub rope: Rope,
    pub sels: Vec<Sel>,
    pub path: Option<PathBuf>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit: Option<(EditKind, Instant)>,
    saved_rope: Rope,
    /// (version, dirty) memo so the O(n) rope comparison runs once per change.
    dirty_memo: Cell<(u64, bool)>,
    /// Lowest line touched since the highlighter last looked.
    pub changed_from_line: Option<usize>,
    /// Bumped on every text change.
    pub version: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Space,
    Word,
    Punct,
    Newline,
}

fn class(c: char) -> CharClass {
    if c == '\n' {
        CharClass::Newline
    } else if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

pub fn is_word_char(c: char) -> bool {
    class(c) == CharClass::Word
}

impl Buffer {
    pub fn new() -> Self {
        Self::from_rope(Rope::new(), None)
    }

    pub fn open(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        // Normalise line endings; files are written back with \n.
        let text = text.replace("\r\n", "\n");
        Ok(Self::from_rope(Rope::from_str(&text), Some(path.to_path_buf())))
    }

    /// Rebuild a buffer from a saved session. The file on disk (if any)
    /// becomes the saved state; `unsaved` text, if given, is applied on top so
    /// the buffer shows as modified. Fails if there is nothing to restore.
    pub fn from_session(path: Option<&Path>, unsaved: Option<&str>, sels: &[(usize, usize)]) -> std::io::Result<Self> {
        let mut b = match path {
            Some(p) if p.is_file() => Self::open(p)?,
            Some(p) if unsaved.is_some() => {
                let mut b = Self::new();
                b.path = Some(p.to_path_buf());
                b
            }
            Some(p) => return Err(std::io::Error::new(std::io::ErrorKind::NotFound, p.display().to_string())),
            None if unsaved.is_some_and(|t| !t.is_empty()) => Self::new(),
            None => return Err(std::io::Error::other("empty untitled buffer")),
        };
        if let Some(text) = unsaved {
            b.rope = Rope::from_str(text);
            b.mark_changed(0);
        }
        if !sels.is_empty() {
            b.sels = sels.iter().map(|&(a, h)| Sel::new(a, h)).collect();
            b.normalize();
        }
        Ok(b)
    }

    fn from_rope(rope: Rope, path: Option<PathBuf>) -> Self {
        Buffer {
            saved_rope: rope.clone(),
            dirty_memo: Cell::new((0, false)),
            rope,
            sels: vec![Sel::caret(0)],
            path,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            changed_from_line: Some(0),
            version: 0,
        }
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        let path = self.path.clone().ok_or_else(|| std::io::Error::other("no path"))?;
        let mut file = std::io::BufWriter::new(std::fs::File::create(&path)?);
        self.rope.write_to(&mut file)?;
        use std::io::Write;
        file.flush()?;
        self.saved_rope = self.rope.clone();
        self.dirty_memo.set((self.version, false));
        Ok(())
    }

    pub fn is_dirty(&self) -> bool {
        let (v, d) = self.dirty_memo.get();
        if v == self.version {
            return d;
        }
        let d = self.rope != self.saved_rope;
        self.dirty_memo.set((self.version, d));
        d
    }

    pub fn name(&self) -> String {
        match &self.path {
            Some(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            None => {
                let first: String = self.rope.line(0).chars().take(30).collect();
                let first = first.trim();
                if first.is_empty() { "untitled".into() } else { first.into() }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// Line text without the trailing newline.
    pub fn line_str(&self, line: usize) -> String {
        let mut s: String = self.rope.line(line).into();
        if s.ends_with('\n') {
            s.pop();
        }
        s
    }

    pub fn line_len(&self, line: usize) -> usize {
        let l = self.rope.line(line);
        let n = l.len_chars();
        if n > 0 && l.char(n - 1) == '\n' { n - 1 } else { n }
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.rope.line_to_char(line)
    }

    pub fn line_end(&self, line: usize) -> usize {
        self.line_start(line) + self.line_len(line)
    }

    pub fn pos_to_lc(&self, pos: usize) -> (usize, usize) {
        let line = self.rope.char_to_line(pos);
        (line, pos - self.rope.line_to_char(line))
    }

    pub fn lc_to_pos(&self, line: usize, col: usize) -> usize {
        let line = line.min(self.line_count() - 1);
        self.line_start(line) + col.min(self.line_len(line))
    }

    pub fn char_at(&self, pos: usize) -> Option<char> {
        (pos < self.len()).then(|| self.rope.char(pos))
    }

    pub fn slice(&self, start: usize, end: usize) -> String {
        self.rope.slice(start..end).into()
    }

    pub fn sel_text(&self, s: &Sel) -> String {
        self.slice(s.start(), s.end())
    }

    /// Visual column (tabs expanded) of a char column in a line.
    pub fn visual_col(&self, line: usize, col: usize) -> usize {
        let mut v = 0;
        for c in self.rope.line(line).chars().take(col) {
            v = if c == '\t' { (v / TAB_SIZE + 1) * TAB_SIZE } else { v + 1 };
        }
        v
    }

    /// Char column in a line closest to a visual column.
    pub fn col_from_visual(&self, line: usize, target: usize) -> usize {
        let mut v = 0;
        for (i, c) in self.rope.line(line).chars().enumerate() {
            if c == '\n' {
                return i;
            }
            let next = if c == '\t' { (v / TAB_SIZE + 1) * TAB_SIZE } else { v + 1 };
            if next > target {
                return if target - v > (next - v) / 2 { i + 1 } else { i };
            }
            v = next;
        }
        self.line_len(line)
    }

    // ----------------------------------------------------------------------
    // Undo

    fn checkpoint(&mut self, kind: EditKind) {
        let now = Instant::now();
        let coalesce = matches!(self.last_edit, Some((k, t))
            if k == kind && kind != EditKind::Other && now - t < Duration::from_millis(800));
        if !coalesce {
            self.undo.push(Snapshot { rope: self.rope.clone(), sels: self.sels.clone() });
            if self.undo.len() > 1000 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_edit = Some((kind, now));
    }

    /// Break undo coalescing, e.g. after cursor movement.
    pub fn seal_undo(&mut self) {
        self.last_edit = None;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(Snapshot { rope: self.rope.clone(), sels: self.sels.clone() });
            self.restore(s);
        }
    }

    pub fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(Snapshot { rope: self.rope.clone(), sels: self.sels.clone() });
            self.restore(s);
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.rope = s.rope;
        self.sels = s.sels;
        self.last_edit = None;
        self.mark_changed(0);
    }

    fn mark_changed(&mut self, line: usize) {
        self.version += 1;
        self.changed_from_line = Some(self.changed_from_line.map_or(line, |l| l.min(line)));
    }

    // ----------------------------------------------------------------------
    // Core edit primitive

    /// Replace each `(start, end)` range with its string. Ranges must not
    /// overlap. The caret ends up after each inserted text, or at
    /// `start + caret_offset` when one is given.
    pub fn apply(&mut self, kind: EditKind, mut edits: Vec<(usize, usize, String, Option<usize>)>) {
        if edits.is_empty() {
            return;
        }
        self.checkpoint(kind);
        edits.sort_by_key(|e| e.0);
        let first_line = self.rope.char_to_line(edits[0].0);
        let mut delta: isize = 0;
        let mut new_sels = Vec::with_capacity(edits.len());
        for (start, end, text, caret) in edits {
            let s = (start as isize + delta) as usize;
            let e = (end as isize + delta) as usize;
            self.rope.remove(s..e);
            self.rope.insert(s, &text);
            let n = text.chars().count();
            new_sels.push(Sel::caret(s + caret.unwrap_or(n)));
            delta += n as isize - (e - s) as isize;
        }
        self.sels = new_sels;
        self.normalize();
        self.mark_changed(first_line);
    }

    /// Replace the rope wholesale while keeping undo; selections are given.
    fn replace_all(&mut self, text: String, sels: Vec<Sel>, first_line: usize) {
        self.checkpoint(EditKind::Other);
        self.rope = Rope::from_str(&text);
        self.sels = sels;
        self.normalize();
        self.mark_changed(first_line);
    }

    /// Sort selections and merge overlapping ones.
    pub fn normalize(&mut self) {
        let len = self.len();
        for s in &mut self.sels {
            s.anchor = s.anchor.min(len);
            s.head = s.head.min(len);
        }
        self.sels.sort_by_key(|s| s.start());
        let mut out: Vec<Sel> = Vec::with_capacity(self.sels.len());
        for s in self.sels.drain(..) {
            if let Some(last) = out.last_mut() {
                let same = s.start() == last.start() && s.end() == last.end();
                if same || s.start() < last.end() {
                    let (a, b) = (last.start().min(s.start()), last.end().max(s.end()));
                    let forward = last.head >= last.anchor;
                    *last = if forward { Sel::new(a, b) } else { Sel::new(b, a) };
                    continue;
                }
            }
            out.push(s);
        }
        if out.is_empty() {
            out.push(Sel::caret(0));
        }
        self.sels = out;
    }

    // ----------------------------------------------------------------------
    // Typing

    pub fn insert(&mut self, text: &str) {
        let edits = self.sels.iter().map(|s| (s.start(), s.end(), text.to_string(), None)).collect();
        self.apply(EditKind::Insert, edits);
    }

    /// Insert one typed char with bracket/quote auto-pairing.
    pub fn type_char(&mut self, c: char) {
        let closing = match c {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            '"' => Some('"'),
            '\'' => Some('\''),
            '`' => Some('`'),
            _ => None,
        };
        let is_closer = matches!(c, ')' | ']' | '}' | '"' | '\'' | '`');

        // Typing a closer right before the same closer just steps over it.
        if is_closer && self.sels.iter().all(|s| s.is_empty() && self.char_at(s.head) == Some(c)) {
            for s in &mut self.sels {
                *s = Sel::caret(s.head + 1);
            }
            self.seal_undo();
            return;
        }

        let mut edits = Vec::new();
        for s in &self.sels {
            let next = self.char_at(s.end());
            let prev = if s.start() > 0 { self.char_at(s.start() - 1) } else { None };
            let pair = closing.filter(|_| {
                if !s.is_empty() {
                    return true; // wrap the selection
                }
                let next_ok = next.is_none_or(|n| n.is_whitespace() || ")]};,".contains(n));
                let quote = matches!(c, '"' | '\'' | '`');
                let prev_ok = !quote || prev.is_none_or(|p| !is_word_char(p));
                next_ok && prev_ok
            });
            match pair {
                Some(close) if !s.is_empty() => {
                    let inner = self.sel_text(s);
                    let n = inner.chars().count();
                    edits.push((s.start(), s.end(), format!("{c}{inner}{close}"), Some(n + 1)));
                }
                Some(close) => edits.push((s.start(), s.end(), format!("{c}{close}"), Some(1))),
                None => edits.push((s.start(), s.end(), c.to_string(), None)),
            }
        }
        self.apply(EditKind::Insert, edits);
    }

    pub fn newline(&mut self) {
        let mut edits = Vec::new();
        for s in &self.sels {
            let (line, _) = self.pos_to_lc(s.start());
            let text = self.line_str(line);
            let mut indent: String = text.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            let prev = if s.start() > 0 { self.char_at(s.start() - 1) } else { None };
            let next = self.char_at(s.end());
            let opens = matches!(prev, Some('{' | '(' | '[' | ':'));
            let between = matches!((prev, next), (Some('{'), Some('}')) | (Some('('), Some(')')) | (Some('['), Some(']')));
            if opens {
                let inner = format!("{indent}{}", " ".repeat(TAB_SIZE));
                if between {
                    let t = format!("\n{inner}\n{indent}");
                    edits.push((s.start(), s.end(), t, Some(inner.chars().count() + 1)));
                    continue;
                }
                indent = inner;
            }
            edits.push((s.start(), s.end(), format!("\n{indent}"), None));
        }
        self.apply(EditKind::Other, edits);
    }

    pub fn backspace(&mut self, word: bool) {
        let mut edits = Vec::new();
        for s in &self.sels {
            if !s.is_empty() {
                edits.push((s.start(), s.end(), String::new(), None));
                continue;
            }
            let p = s.head;
            if p == 0 {
                edits.push((0, 0, String::new(), None));
                continue;
            }
            if word {
                edits.push((self.word_left(p), p, String::new(), None));
                continue;
            }
            let prev = self.char_at(p - 1);
            let next = self.char_at(p);
            let paired = matches!((prev, next),
                (Some('('), Some(')')) | (Some('['), Some(']')) | (Some('{'), Some('}'))
                | (Some('"'), Some('"')) | (Some('\''), Some('\'')) | (Some('`'), Some('`')));
            if paired {
                edits.push((p - 1, p + 1, String::new(), None));
                continue;
            }
            // Delete back to the previous tab stop inside leading whitespace.
            let (line, col) = self.pos_to_lc(p);
            let before: String = self.line_str(line).chars().take(col).collect();
            if col > 0 && before.chars().all(|c| c == ' ') {
                let n = (col - 1) % TAB_SIZE + 1;
                edits.push((p - n, p, String::new(), None));
                continue;
            }
            edits.push((p - 1, p, String::new(), None));
        }
        self.apply(EditKind::Delete, edits);
    }

    pub fn delete_forward(&mut self, word: bool) {
        let len = self.len();
        let edits = self
            .sels
            .iter()
            .map(|s| {
                if !s.is_empty() {
                    (s.start(), s.end(), String::new(), None)
                } else {
                    let end = if s.head >= len { len } else if word { self.word_right(s.head) } else { s.head + 1 };
                    (s.head, end, String::new(), None)
                }
            })
            .collect();
        self.apply(EditKind::Delete, edits);
    }

    // ----------------------------------------------------------------------
    // Movement

    pub fn word_left(&self, mut p: usize) -> usize {
        while p > 0 && class(self.rope.char(p - 1)) == CharClass::Space {
            p -= 1;
        }
        if p == 0 {
            return 0;
        }
        let k = class(self.rope.char(p - 1));
        if k == CharClass::Newline {
            return p - 1;
        }
        while p > 0 && class(self.rope.char(p - 1)) == k {
            p -= 1;
        }
        p
    }

    pub fn word_right(&self, mut p: usize) -> usize {
        let len = self.len();
        while p < len && class(self.rope.char(p)) == CharClass::Space {
            p += 1;
        }
        if p >= len {
            return len;
        }
        let k = class(self.rope.char(p));
        if k == CharClass::Newline {
            return p + 1;
        }
        while p < len && class(self.rope.char(p)) == k {
            p += 1;
        }
        p
    }

    pub fn word_at(&self, p: usize) -> (usize, usize) {
        let len = self.len();
        let (mut a, mut b) = (p, p);
        while a > 0 && is_word_char(self.rope.char(a - 1)) {
            a -= 1;
        }
        while b < len && is_word_char(self.rope.char(b)) {
            b += 1;
        }
        (a, b)
    }

    fn smart_home(&self, p: usize) -> usize {
        let (line, col) = self.pos_to_lc(p);
        let indent = self.line_str(line).chars().take_while(|c| c.is_whitespace()).count();
        self.line_start(line) + if col == indent { 0 } else { indent }
    }

    /// Move every selection's head; collapse unless `extend`.
    pub fn move_heads(&mut self, extend: bool, f: impl Fn(&Buffer, &Sel) -> usize) {
        let new: Vec<Sel> = self
            .sels
            .iter()
            .map(|s| {
                let h = f(self, s);
                Sel { anchor: if extend { s.anchor } else { h }, head: h, goal: None }
            })
            .collect();
        self.sels = new;
        self.normalize();
        self.seal_undo();
    }

    pub fn move_left(&mut self, extend: bool, word: bool) {
        self.move_heads(extend, |b, s| {
            if !extend && !s.is_empty() && !word {
                s.start()
            } else if word {
                b.word_left(s.head)
            } else {
                s.head.saturating_sub(1)
            }
        });
    }

    pub fn move_right(&mut self, extend: bool, word: bool) {
        let len = self.len();
        self.move_heads(extend, |b, s| {
            if !extend && !s.is_empty() && !word {
                s.end()
            } else if word {
                b.word_right(s.head)
            } else {
                (s.head + 1).min(len)
            }
        });
    }

    pub fn move_vertical(&mut self, lines: isize, extend: bool) {
        let last = self.line_count() as isize - 1;
        let new: Vec<Sel> = self
            .sels
            .iter()
            .map(|s| {
                let (line, col) = self.pos_to_lc(s.head);
                let goal = s.goal.unwrap_or_else(|| self.visual_col(line, col));
                let target = line as isize + lines;
                let h = if target < 0 {
                    0
                } else if target > last {
                    self.len()
                } else {
                    let t = target as usize;
                    self.line_start(t) + self.col_from_visual(t, goal)
                };
                Sel { anchor: if extend { s.anchor } else { h }, head: h, goal: Some(goal) }
            })
            .collect();
        self.sels = new;
        self.normalize();
        self.seal_undo();
    }

    pub fn move_home(&mut self, extend: bool) {
        self.move_heads(extend, |b, s| b.smart_home(s.head));
    }

    pub fn move_end(&mut self, extend: bool) {
        self.move_heads(extend, |b, s| b.line_end(b.rope.char_to_line(s.head)));
    }

    pub fn move_doc(&mut self, end: bool, extend: bool) {
        let len = self.len();
        self.move_heads(extend, |_, _| if end { len } else { 0 });
    }

    // ----------------------------------------------------------------------
    // Selection commands

    pub fn select_all(&mut self) {
        self.sels = vec![Sel::new(0, self.len())];
    }

    pub fn single_selection(&mut self) {
        let primary = *self.sels.last().unwrap();
        self.sels = vec![primary];
    }

    pub fn select_line(&mut self) {
        let lines = self.line_count();
        for s in &mut self.sels {
            let a = self.rope.char_to_line(s.start());
            let b = self.rope.char_to_line(s.end());
            let end = if b + 1 < lines { self.rope.line_to_char(b + 1) } else { self.rope.len_chars() };
            *s = Sel::new(self.rope.line_to_char(a), end);
        }
        self.normalize();
    }

    /// Ctrl+D: select the word under each caret, or add the next occurrence.
    pub fn find_under_expand(&mut self) {
        if self.sels.iter().any(|s| s.is_empty()) {
            for i in 0..self.sels.len() {
                let s = self.sels[i];
                if s.is_empty() {
                    let (a, b) = self.word_at(s.head);
                    self.sels[i] = Sel::new(a, b);
                }
            }
            self.normalize();
            return;
        }
        let last = *self.sels.last().unwrap();
        let needle = self.sel_text(&last);
        if needle.is_empty() {
            return;
        }
        let hay: String = self.rope.to_string();
        let from = self.rope.char_to_byte(last.end());
        let found = hay[from..].find(&needle).map(|i| i + from).or_else(|| hay.find(&needle));
        if let Some(b) = found {
            let a = self.rope.byte_to_char(b);
            let s = Sel::new(a, a + needle.chars().count());
            if !self.sels.iter().any(|x| x.start() == s.start()) {
                self.sels.push(s);
                // Keep the newest selection last so it stays "primary".
                let newest = s;
                self.normalize();
                if let Some(i) = self.sels.iter().position(|x| *x == newest) {
                    let n = self.sels.remove(i);
                    self.sels.push(n);
                }
            }
        }
    }

    pub fn split_into_lines(&mut self) {
        let mut out = Vec::new();
        for s in &self.sels {
            let (a, b) = (s.start(), s.end());
            let la = self.rope.char_to_line(a);
            let lb = self.rope.char_to_line(b);
            if la == lb {
                out.push(*s);
                continue;
            }
            for l in la..=lb {
                let ls = self.line_start(l).max(a);
                let le = self.line_end(l).min(b);
                if l == lb && le == self.line_start(l) && ls == le {
                    continue;
                }
                out.push(Sel::new(ls, le));
            }
        }
        self.sels = out;
        self.normalize();
    }

    pub fn add_cursor_vertical(&mut self, down: bool) {
        let mut extra = Vec::new();
        let edge = if down { self.sels.last() } else { self.sels.first() };
        if let Some(s) = edge {
            let (line, col) = self.pos_to_lc(s.head);
            let goal = s.goal.unwrap_or_else(|| self.visual_col(line, col));
            let target = if down { line + 1 } else { line.wrapping_sub(1) };
            if target < self.line_count() {
                let p = self.line_start(target) + self.col_from_visual(target, goal);
                extra.push(Sel { anchor: p, head: p, goal: Some(goal) });
            }
        }
        self.sels.extend(extra);
        self.normalize();
    }

    pub fn find_all(&mut self, needle: &str, case: bool) -> usize {
        let ranges = self.find_ranges(needle, case);
        if !ranges.is_empty() {
            self.sels = ranges.iter().map(|&(a, b)| Sel::new(a, b)).collect();
        }
        ranges.len()
    }

    /// All (start, end) char ranges matching `needle`.
    pub fn find_ranges(&self, needle: &str, case: bool) -> Vec<(usize, usize)> {
        if needle.is_empty() {
            return Vec::new();
        }
        let hay = self.rope.to_string();
        let (hay, needle) = if case { (hay, needle.to_string()) } else { (hay.to_lowercase(), needle.to_lowercase()) };
        // to_lowercase can change byte lengths for some scripts; fall back to
        // case-sensitive positions if it did.
        if hay.len() != self.rope.len_bytes() {
            return self.find_ranges(&needle, true);
        }
        let n = needle.chars().count();
        hay.match_indices(&needle)
            .map(|(b, _)| {
                let a = self.rope.byte_to_char(b);
                (a, a + n)
            })
            .collect()
    }

    /// Select the next (or previous) match relative to the primary selection.
    pub fn find_next(&mut self, needle: &str, case: bool, forward: bool) -> bool {
        let ranges = self.find_ranges(needle, case);
        if ranges.is_empty() {
            return false;
        }
        let cur = *self.sels.last().unwrap();
        let pick = if forward {
            ranges.iter().find(|r| r.0 >= cur.end()).or(ranges.first())
        } else {
            ranges.iter().rev().find(|r| r.1 <= cur.start()).or(ranges.last())
        };
        let &(a, b) = pick.unwrap();
        self.sels = vec![Sel::new(a, b)];
        true
    }

    pub fn replace_next(&mut self, needle: &str, repl: &str, case: bool) {
        let cur = *self.sels.last().unwrap();
        let matches_cur = {
            let t = self.sel_text(&cur);
            if case { t == needle } else { t.to_lowercase() == needle.to_lowercase() }
        };
        if matches_cur && !needle.is_empty() {
            self.apply(EditKind::Other, vec![(cur.start(), cur.end(), repl.to_string(), None)]);
        }
        self.find_next(needle, case, true);
    }

    pub fn replace_all_matches(&mut self, needle: &str, repl: &str, case: bool) -> usize {
        let ranges = self.find_ranges(needle, case);
        let n = ranges.len();
        let edits = ranges.into_iter().map(|(a, b)| (a, b, repl.to_string(), None)).collect();
        self.apply(EditKind::Other, edits);
        n
    }

    pub fn goto_line(&mut self, line: usize) {
        let l = line.saturating_sub(1).min(self.line_count() - 1);
        let p = self.line_start(l);
        self.sels = vec![Sel::caret(p)];
        self.seal_undo();
    }

    pub fn goto_matching_bracket(&mut self) {
        for i in 0..self.sels.len() {
            let p = self.sels[i].head;
            if let Some(m) = self.matching_bracket(p).or_else(|| p.checked_sub(1).and_then(|q| self.matching_bracket(q))) {
                self.sels[i] = Sel::caret(m.1);
            }
        }
        self.normalize();
    }

    /// If `p` is on a bracket, return (p, matching position).
    pub fn matching_bracket(&self, p: usize) -> Option<(usize, usize)> {
        let c = self.char_at(p)?;
        let (open, close, fwd) = match c {
            '(' => ('(', ')', true),
            '[' => ('[', ']', true),
            '{' => ('{', '}', true),
            ')' => ('(', ')', false),
            ']' => ('[', ']', false),
            '}' => ('{', '}', false),
            _ => return None,
        };
        let mut depth = 0i32;
        const LIMIT: usize = 20_000;
        if fwd {
            for (i, ch) in self.rope.chars_at(p).take(LIMIT).enumerate() {
                if ch == open { depth += 1 } else if ch == close { depth -= 1 }
                if depth == 0 {
                    return Some((p, p + i));
                }
            }
        } else {
            let mut it = self.rope.chars_at(p + 1);
            let mut q = p + 1;
            for _ in 0..LIMIT {
                let ch = it.prev()?;
                q -= 1;
                if ch == close { depth += 1 } else if ch == open { depth -= 1 }
                if depth == 0 {
                    return Some((p, q));
                }
            }
        }
        None
    }

    // ----------------------------------------------------------------------
    // Line-oriented commands. These rebuild whole lines, so they operate on
    // the set of lines touched by the selections.

    /// Sorted, de-duplicated line ranges (first, last) covered by selections.
    fn line_blocks(&self) -> Vec<(usize, usize)> {
        let mut blocks: Vec<(usize, usize)> = self
            .sels
            .iter()
            .map(|s| {
                let a = self.rope.char_to_line(s.start());
                let mut b = self.rope.char_to_line(s.end());
                // A selection ending at column 0 doesn't include that line.
                if b > a && s.end() == self.line_start(b) {
                    b -= 1;
                }
                (a, b)
            })
            .collect();
        blocks.sort();
        let mut out: Vec<(usize, usize)> = Vec::new();
        for (a, b) in blocks {
            match out.last_mut() {
                Some(last) if a <= last.1 + 1 => last.1 = last.1.max(b),
                _ => out.push((a, b)),
            }
        }
        out
    }

    fn lines_vec(&self) -> Vec<String> {
        (0..self.line_count()).map(|l| self.line_str(l)).collect()
    }

    /// Selection positions as (line, col) pairs, for restoring after a line rebuild.
    fn sels_lc(&self) -> Vec<((usize, usize), (usize, usize))> {
        self.sels.iter().map(|s| (self.pos_to_lc(s.anchor), self.pos_to_lc(s.head))).collect()
    }

    fn rebuild(&mut self, lines: Vec<String>, sels_lc: Vec<((usize, usize), (usize, usize))>, first_line: usize) {
        let text = lines.join("\n");
        let tmp = Rope::from_str(&text);
        let to_pos = |(l, c): (usize, usize)| {
            let l = l.min(tmp.len_lines() - 1);
            let line = tmp.line(l);
            let ll = line.len_chars() - if line.chars().last() == Some('\n') { 1 } else { 0 };
            tmp.line_to_char(l) + c.min(ll)
        };
        let sels = sels_lc.into_iter().map(|(a, h)| Sel::new(to_pos(a), to_pos(h))).collect();
        self.replace_all(text, sels, first_line);
    }

    pub fn indent(&mut self) {
        let blocks = self.line_blocks();
        let mut lines = self.lines_vec();
        let pad = " ".repeat(TAB_SIZE);
        let mut sels = self.sels_lc();
        for &(a, b) in &blocks {
            for l in a..=b {
                if !lines[l].is_empty() {
                    lines[l].insert_str(0, &pad);
                }
            }
        }
        for (an, hd) in &mut sels {
            for p in [an, hd] {
                if blocks.iter().any(|&(a, b)| p.0 >= a && p.0 <= b) && !lines[p.0].is_empty() {
                    p.1 += TAB_SIZE;
                }
            }
        }
        self.rebuild(lines, sels, blocks[0].0);
    }

    pub fn unindent(&mut self) {
        let blocks = self.line_blocks();
        let mut lines = self.lines_vec();
        let mut sels = self.sels_lc();
        let mut removed = vec![0usize; lines.len()];
        for &(a, b) in &blocks {
            for l in a..=b {
                let n = if lines[l].starts_with('\t') {
                    1
                } else {
                    lines[l].chars().take(TAB_SIZE).take_while(|c| *c == ' ').count()
                };
                lines[l].drain(..n);
                removed[l] = n;
            }
        }
        for (an, hd) in &mut sels {
            for p in [an, hd] {
                p.1 = p.1.saturating_sub(removed[p.0]);
            }
        }
        self.rebuild(lines, sels, blocks[0].0);
    }

    /// Tab key: indent when selections span lines, otherwise insert spaces.
    pub fn tab(&mut self) {
        let multiline = self.sels.iter().any(|s| self.rope.char_to_line(s.start()) != self.rope.char_to_line(s.end()));
        if multiline {
            return self.indent();
        }
        let edits = self
            .sels
            .iter()
            .map(|s| {
                let (line, col) = self.pos_to_lc(s.start());
                let v = self.visual_col(line, col);
                let n = TAB_SIZE - v % TAB_SIZE;
                (s.start(), s.end(), " ".repeat(n), None)
            })
            .collect();
        self.apply(EditKind::Insert, edits);
    }

    pub fn toggle_comment(&mut self, token: &str) {
        let blocks = self.line_blocks();
        let mut lines = self.lines_vec();
        let mut sels = self.sels_lc();
        let prefix = format!("{token} ");
        let mut shift = vec![0isize; lines.len()];
        for &(a, b) in &blocks {
            let all_commented = (a..=b)
                .filter(|&l| !lines[l].trim().is_empty())
                .all(|l| lines[l].trim_start().starts_with(token));
            let indent = (a..=b)
                .filter(|&l| !lines[l].trim().is_empty())
                .map(|l| lines[l].chars().take_while(|c| c.is_whitespace()).count())
                .min()
                .unwrap_or(0);
            for l in a..=b {
                if lines[l].trim().is_empty() {
                    continue;
                }
                if all_commented {
                    let ws = lines[l].chars().take_while(|c| c.is_whitespace()).count();
                    let rest: String = lines[l].chars().skip(ws).collect();
                    let (strip, n) = if rest.starts_with(&prefix) { (prefix.as_str(), prefix.len()) } else { (token, token.len()) };
                    let rest = &rest[strip.len()..];
                    lines[l] = format!("{}{}", &lines[l].chars().take(ws).collect::<String>(), rest);
                    shift[l] = -(n as isize);
                } else {
                    let head: String = lines[l].chars().take(indent).collect();
                    let tail: String = lines[l].chars().skip(indent).collect();
                    lines[l] = format!("{head}{prefix}{tail}");
                    shift[l] = prefix.chars().count() as isize;
                }
            }
        }
        for (an, hd) in &mut sels {
            for p in [an, hd] {
                p.1 = (p.1 as isize + shift[p.0]).max(0) as usize;
            }
        }
        self.rebuild(lines, sels, blocks[0].0);
    }

    pub fn delete_line(&mut self) {
        let blocks = self.line_blocks();
        let mut lines = self.lines_vec();
        for &(a, b) in blocks.iter().rev() {
            lines.drain(a..=b);
        }
        if lines.is_empty() {
            lines.push(String::new());
        }
        // Put a caret at the start of the line that took each block's place.
        let mut sels = Vec::new();
        let mut removed = 0;
        for &(a, b) in &blocks {
            let l = a - removed;
            sels.push(((l, 0), (l, 0)));
            removed += b - a + 1;
        }
        self.rebuild(lines, sels, blocks[0].0);
    }

    pub fn duplicate(&mut self) {
        if self.sels.iter().all(|s| !s.is_empty()) {
            // Duplicate selected text in place.
            let edits = self
                .sels
                .iter()
                .map(|s| {
                    let t = self.sel_text(s);
                    let n = t.chars().count();
                    (s.end(), s.end(), t, Some(n))
                })
                .collect();
            self.apply(EditKind::Other, edits);
            return;
        }
        let blocks = self.line_blocks();
        let mut lines = self.lines_vec();
        let mut added = 0;
        for &(a, b) in &blocks {
            let copy: Vec<String> = lines[a + added..=b + added].to_vec();
            let n = copy.len();
            let at = b + added + 1;
            lines.splice(at..at, copy);
            added += n;
        }
        // Carets move onto the copy; lines after a block shift by its size.
        let shifted = |line: usize| -> usize {
            blocks.iter().filter(|&&(_, b)| b < line).map(|&(a, b)| b - a + 1).sum()
        };
        let sels: Vec<_> = self
            .sels_lc()
            .into_iter()
            .map(|(an, hd)| {
                let f = |p: (usize, usize)| {
                    let in_block = blocks.iter().find(|&&(a, b)| p.0 >= a && p.0 <= b);
                    let extra = in_block.map_or(0, |&(a, b)| b - a + 1);
                    (p.0 + shifted(p.0) + extra, p.1)
                };
                (f(an), f(hd))
            })
            .collect();
        self.rebuild(lines, sels, blocks[0].0);
    }

    pub fn swap_lines(&mut self, up: bool) {
        let blocks = self.line_blocks();
        let n = self.line_count();
        if (up && blocks[0].0 == 0) || (!up && blocks.last().unwrap().1 + 1 >= n) {
            return;
        }
        let mut lines = self.lines_vec();
        let order: Vec<_> = if up { blocks.clone() } else { blocks.iter().rev().cloned().collect() };
        for (a, b) in order {
            if up {
                let l = lines.remove(a - 1);
                lines.insert(b, l);
            } else {
                let l = lines.remove(b + 1);
                lines.insert(a, l);
            }
        }
        let d: isize = if up { -1 } else { 1 };
        let sels = self
            .sels_lc()
            .into_iter()
            .map(|(an, hd)| (((an.0 as isize + d) as usize, an.1), ((hd.0 as isize + d) as usize, hd.1)))
            .collect();
        let first = if up { blocks[0].0 - 1 } else { blocks[0].0 };
        self.rebuild(lines, sels, first);
    }

    pub fn join_lines(&mut self) {
        let blocks = self.line_blocks();
        let mut lines = self.lines_vec();
        let mut sels = Vec::new();
        let mut removed = 0;
        for &(a, b) in &blocks {
            let (a, b) = (a - removed, b - removed);
            let b = if a == b { (b + 1).min(lines.len() - 1) } else { b };
            if a == b {
                continue;
            }
            let mut joined = lines[a].trim_end().to_string();
            let mut col = joined.chars().count();
            for l in a + 1..=b {
                let t = lines[l].trim();
                if !t.is_empty() {
                    if !joined.is_empty() {
                        joined.push(' ');
                    }
                    col = joined.chars().count();
                    joined.push_str(t);
                }
            }
            lines.splice(a..=b, [joined]);
            sels.push(((a, col), (a, col)));
            removed += b - a;
        }
        if sels.is_empty() {
            return;
        }
        self.rebuild(lines, sels, blocks[0].0);
    }

    pub fn insert_line(&mut self, before: bool) {
        let mut edits = Vec::new();
        for s in &self.sels {
            let line = self.rope.char_to_line(s.head);
            let indent: String = self.line_str(line).chars().take_while(|c| c.is_whitespace()).collect();
            if before {
                let p = self.line_start(line);
                let n = indent.chars().count();
                edits.push((p, p, format!("{indent}\n"), Some(n)));
            } else {
                let p = self.line_end(line);
                edits.push((p, p, format!("\n{indent}"), None));
            }
        }
        edits.sort_by_key(|e| e.0);
        edits.dedup_by_key(|e| e.0);
        self.apply(EditKind::Other, edits);
    }

    pub fn sort_lines(&mut self) {
        let blocks = if self.sels.iter().all(|s| s.is_empty()) {
            vec![(0, self.line_count() - 1)]
        } else {
            self.line_blocks()
        };
        let mut lines = self.lines_vec();
        for &(a, b) in &blocks {
            lines[a..=b].sort_by_key(|l| l.to_lowercase());
        }
        let sels = blocks.iter().map(|&(a, b)| ((a, 0), (b, usize::MAX))).collect();
        self.rebuild(lines, sels, blocks[0].0);
    }

    pub fn transform_case(&mut self, upper: bool) {
        let edits = self
            .sels
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| {
                let t = self.sel_text(s);
                let t = if upper { t.to_uppercase() } else { t.to_lowercase() };
                (s.start(), s.end(), t, None)
            })
            .collect::<Vec<_>>();
        let old: Vec<Sel> = self.sels.clone();
        self.apply(EditKind::Other, edits);
        // Keep the text selected, as Sublime does.
        if self.sels.len() == old.len() {
            self.sels = old;
            self.normalize();
        }
    }

    // ----------------------------------------------------------------------
    // Clipboard

    /// Text to copy. With all-empty selections, whole lines are copied
    /// (returned flag = true), matching Sublime's behaviour.
    pub fn copy_text(&self) -> (String, bool) {
        if self.sels.iter().all(|s| s.is_empty()) {
            let mut out = String::new();
            let mut last_line = None;
            for s in &self.sels {
                let l = self.rope.char_to_line(s.head);
                if last_line == Some(l) {
                    continue;
                }
                last_line = Some(l);
                out.push_str(&self.line_str(l));
                out.push('\n');
            }
            (out, true)
        } else {
            let parts: Vec<String> = self.sels.iter().filter(|s| !s.is_empty()).map(|s| self.sel_text(s)).collect();
            (parts.join("\n"), false)
        }
    }

    pub fn cut(&mut self) -> (String, bool) {
        let (text, whole_lines) = self.copy_text();
        if whole_lines {
            self.delete_line();
        } else {
            let edits = self.sels.iter().map(|s| (s.start(), s.end(), String::new(), None)).collect();
            self.apply(EditKind::Other, edits);
        }
        (text, whole_lines)
    }

    pub fn paste(&mut self, text: &str, whole_lines: bool) {
        let text = text.replace("\r\n", "\n");
        let pieces: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
        let distribute = self.sels.len() > 1 && pieces.len() == self.sels.len();
        let edits = self
            .sels
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let t = if distribute { pieces[i].to_string() } else { text.clone() };
                if whole_lines && s.is_empty() && !distribute {
                    // Whole-line paste goes above the current line.
                    let ls = self.line_start(self.rope.char_to_line(s.head));
                    let caret = s.head - ls + t.chars().count();
                    (ls, ls, t, Some(caret))
                } else {
                    (s.start(), s.end(), t, None)
                }
            })
            .collect();
        self.apply(EditKind::Other, edits);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str) -> Buffer {
        let mut b = Buffer::new();
        b.rope = Rope::from_str(text);
        b
    }

    #[test]
    fn multi_cursor_insert() {
        let mut b = buf("ab\ncd\n");
        b.sels = vec![Sel::caret(1), Sel::caret(4)];
        b.insert("X");
        assert_eq!(b.rope.to_string(), "aXb\ncXd\n");
        assert_eq!(b.sels, vec![Sel::caret(2), Sel::caret(6)]);
    }

    #[test]
    fn undo_redo() {
        let mut b = buf("hello");
        b.sels = vec![Sel::caret(5)];
        b.insert("!");
        b.undo();
        assert_eq!(b.rope.to_string(), "hello");
        b.redo();
        assert_eq!(b.rope.to_string(), "hello!");
    }

    #[test]
    fn find_under_expand_adds_next() {
        let mut b = buf("foo bar foo baz foo");
        b.sels = vec![Sel::caret(1)];
        b.find_under_expand();
        assert_eq!(b.sels, vec![Sel::new(0, 3)]);
        b.find_under_expand();
        b.find_under_expand();
        assert_eq!(b.sels.len(), 3);
        b.insert("x");
        assert_eq!(b.rope.to_string(), "x bar x baz x");
    }

    #[test]
    fn auto_pair_and_step_over() {
        let mut b = buf("");
        b.type_char('(');
        assert_eq!(b.rope.to_string(), "()");
        b.type_char(')');
        assert_eq!(b.rope.to_string(), "()");
        assert_eq!(b.sels[0].head, 2);
    }

    #[test]
    fn toggle_comment_roundtrip() {
        let mut b = buf("    a\n    b\n");
        b.sels = vec![Sel::new(0, 9)];
        b.toggle_comment("//");
        assert_eq!(b.rope.to_string(), "    // a\n    // b\n");
        b.toggle_comment("//");
        assert_eq!(b.rope.to_string(), "    a\n    b\n");
    }

    #[test]
    fn swap_and_duplicate() {
        let mut b = buf("1\n2\n3");
        b.sels = vec![Sel::caret(2)];
        b.swap_lines(true);
        assert_eq!(b.rope.to_string(), "2\n1\n3");
        assert_eq!(b.pos_to_lc(b.sels[0].head), (0, 0));
        b.duplicate();
        assert_eq!(b.rope.to_string(), "2\n2\n1\n3");
        assert_eq!(b.pos_to_lc(b.sels[0].head), (1, 0));
    }

    #[test]
    fn delete_and_join() {
        let mut b = buf("a\nb\nc");
        b.sels = vec![Sel::caret(2)];
        b.delete_line();
        assert_eq!(b.rope.to_string(), "a\nc");
        b.sels = vec![Sel::caret(0)];
        b.join_lines();
        assert_eq!(b.rope.to_string(), "a c");
    }

    #[test]
    fn split_selection_into_lines() {
        let mut b = buf("aa\nbb\ncc");
        b.select_all();
        b.split_into_lines();
        assert_eq!(b.sels.len(), 3);
    }

    #[test]
    fn newline_between_braces() {
        let mut b = buf("{}");
        b.sels = vec![Sel::caret(1)];
        b.newline();
        assert_eq!(b.rope.to_string(), "{\n    \n}");
        assert_eq!(b.pos_to_lc(b.sels[0].head), (1, 4));
    }

    #[test]
    fn indent_unindent() {
        let mut b = buf("a\nb");
        b.select_all();
        b.indent();
        assert_eq!(b.rope.to_string(), "    a\n    b");
        b.unindent();
        assert_eq!(b.rope.to_string(), "a\nb");
    }
}

#[cfg(test)]
mod restore_tests {
    use super::*;

    #[test]
    fn restore_unsaved_over_file() {
        let p = std::env::temp_dir().join(format!("edi-restore-{}.txt", std::process::id()));
        std::fs::write(&p, "on disk\n").unwrap();
        let b = Buffer::from_session(Some(&p), Some("edited\n"), &[(2, 4)]).unwrap();
        assert_eq!(b.rope.to_string(), "edited\n");
        assert!(b.is_dirty());
        assert_eq!(b.sels, vec![Sel::new(2, 4)]);
        let clean = Buffer::from_session(Some(&p), None, &[(99, 99)]).unwrap();
        assert!(!clean.is_dirty());
        // Out-of-range selections are clamped.
        assert_eq!(clean.sels[0].head, clean.len());
        std::fs::remove_file(&p).unwrap();
        assert!(Buffer::from_session(Some(&p), None, &[]).is_err());
        assert!(Buffer::from_session(None, Some(""), &[]).is_err());
    }
}

#[cfg(test)]
mod regress {
    use super::*;

    #[test]
    fn comment_after_split() {
        let mut b = Buffer::new();
        b.rope = Rope::from_str("import os\n\n\nclass A:\n    x = 1\n");
        b.select_all();
        b.split_into_lines();
        b.toggle_comment("#");
        assert_eq!(b.rope.to_string(), "# import os\n\n\n# class A:\n#     x = 1\n");
    }
}
