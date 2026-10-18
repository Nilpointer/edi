//! The text view: a custom-painted editor widget with multiple carets,
//! gutter, minimap and Sublime-style mouse handling.

use crate::buffer::{is_word_char, Buffer, Sel, TAB_SIZE};
use crate::display::{DisplayMap, MapKey, Row};
use crate::highlight::{HlCache, Palette, Syntaxes};
use crate::search::{Matcher, Search};
use eframe::egui::{
    self, text::LayoutJob, Align2, Color32, CornerRadius, CursorIcon, Event, FontId, Key, Pos2, Rect, Sense, Stroke,
    StrokeKind, TextFormat, Ui, Vec2,
};
use std::time::{Duration, Instant};

const MINIMAP_W: f32 = 110.0;
const MM_LH: f32 = 2.6;
const MM_CW: f32 = 1.1;

#[derive(Default)]
pub struct ViewState {
    pub scroll: Vec2,
    pub reveal: bool,
    last_click: Option<(Instant, Pos2, u8)>,
    /// Selection being dragged: (index in sels, original word/line span, granularity).
    drag: Option<(usize, (usize, usize), u8)>,
    column_anchor: Option<(usize, f32)>,
    minimap_drag: bool,
    /// Time of the last caret/text change, for caret blinking.
    activity: f64,
    /// Line ↔ row mapping (wrap and folds), rebuilt when its inputs change.
    map: Option<(MapKey, DisplayMap)>,
}

impl ViewState {
    /// The display map for the buffer's current state.
    fn map(&mut self, buf: &Buffer, wrap: Option<usize>) -> &DisplayMap {
        let key = MapKey { version: buf.version, fold_gen: buf.fold_gen, wrap };
        if self.map.as_ref().is_none_or(|(k, _)| *k != key) {
            self.map = Some((key, DisplayMap::build(buf, wrap)));
        }
        &self.map.as_ref().unwrap().1
    }
}

pub struct Tab {
    /// Stable identity; indices into the tab list shift as tabs close.
    pub id: u64,
    pub buf: Buffer,
    pub hl: HlCache,
    pub view: ViewState,
    /// Set for a Find Results tab.
    pub results: Option<Results>,
    /// Editor group (split pane) the tab lives in.
    pub group: usize,
    /// Set for a browser tab.
    pub browser: Option<crate::browser::Browser>,
}

/// A Find Results tab: where each of its lines jumps to.
pub struct Results {
    pub locs: Vec<Option<crate::find_files::Loc>>,
    pub matcher: Matcher,
    /// Line of the result F4 last jumped to.
    pub cursor: Option<usize>,
}

pub struct ViewCtx<'a> {
    pub syn: &'a Syntaxes,
    pub font_size: f32,
    pub minimap: bool,
    pub wrap: bool,
    pub grab_focus: bool,
    /// Widget id; one per editor group so each pane keeps its own focus.
    pub id: egui::Id,
    /// Find-panel search whose matches are outlined.
    pub find: Option<&'a Matcher>,
}

pub struct Metrics {
    pub cw: f32,
    pub lh: f32,
    pub font: FontId,
}

pub fn metrics(ui: &Ui, size: f32) -> Metrics {
    let font = FontId::monospace(size);
    let cw = ui.fonts_mut(|f| f.glyph_width(&font, 'M'));
    let lh = (size * 1.45).round();
    Metrics { cw, lh, font }
}

/// Clipboard bookkeeping so whole-line copies paste as whole lines.
#[derive(Default)]
pub struct Clip {
    pub text: String,
    pub whole_lines: bool,
}

pub fn show(ui: &mut Ui, tab: &mut Tab, cx: &ViewCtx, clip: &mut Clip) -> egui::Response {
    let full = ui.available_rect_before_wrap();
    let m = metrics(ui, cx.font_size);
    let pal = cx.syn.palette();
    let buf = &mut tab.buf;
    let vs = &mut tab.view;

    let digits = buf.line_count().to_string().len().max(3) as f32;
    let gutter_w = (digits + 4.0) * m.cw;
    let mm_w = if cx.minimap { MINIMAP_W } else { 0.0 };
    let text_rect = Rect::from_min_max(Pos2::new(full.left() + gutter_w, full.top()), Pos2::new(full.right() - mm_w, full.bottom()));
    let gutter_rect = Rect::from_min_max(full.min, Pos2::new(text_rect.left(), full.bottom()));
    let mm_rect = Rect::from_min_max(Pos2::new(text_rect.right(), full.top()), full.max);
    let id = cx.id;
    let resp = ui.interact(Rect::from_min_max(full.min, text_rect.max), id, Sense::click_and_drag());
    let mm_resp = ui.interact(mm_rect, id.with("minimap"), Sense::click_and_drag());
    if resp.hovered() {
        let over_gutter = ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| gutter_rect.contains(p));
        ui.ctx().set_cursor_icon(if over_gutter { CursorIcon::Default } else { CursorIcon::Text });
    }

    // Keep keyboard focus (and Tab/arrows/Escape) on the editor.
    if resp.clicked() || resp.drag_started() || cx.grab_focus {
        resp.request_focus();
    }
    ui.memory_mut(|mem| {
        mem.set_focus_lock_filter(
            id,
            egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
        )
    });

    let visible_rows = (text_rect.height() / m.lh).floor().max(1.0) as usize;
    let wrap = cx.wrap.then(|| ((text_rect.width() - 12.0) / m.cw).floor().max(8.0) as usize);
    let geo = Geo { m: &m, text: text_rect, gutter: gutter_rect, wrap };

    // ---------------------------------------------------------------- input
    let before = (buf.version, buf.sels.clone());
    if resp.has_focus() {
        handle_keys(ui, buf, vs, clip, visible_rows, wrap);
    }
    handle_mouse(ui, &resp, buf, vs, &geo);
    // Carets never hide inside a fold (find, goto, undo can put them there).
    buf.reveal_carets();
    if before.0 != buf.version || before.1 != buf.sels {
        vs.activity = ui.input(|i| i.time);
    }

    // Scrolling.
    if resp.hovered() || mm_resp.hovered() {
        let d = ui.input(|i| i.smooth_scroll_delta);
        vs.scroll -= d;
    }
    let total_rows = vs.map(buf, wrap).row_count(buf);
    let max_y = total_rows.saturating_sub(1) as f32 * m.lh;
    if vs.reveal {
        reveal(buf, vs, &geo);
        vs.reveal = false;
    }
    vs.scroll.y = vs.scroll.y.clamp(0.0, max_y);
    vs.scroll.x = if wrap.is_some() { 0.0 } else { vs.scroll.x.max(0.0) };

    let first_row = (vs.scroll.y / m.lh).floor() as usize;
    let last_row = (first_row + visible_rows + 2).min(total_rows);
    let map = vs.map(buf, wrap);
    let rows: Vec<(usize, Row)> = (first_row..last_row).map(|i| (i, map.row(buf, i))).collect();
    let first_line = rows.first().map_or(0, |r| r.1.line);
    let last_line = rows.last().map_or(0, |r| r.1.line + 1);

    // Minimap (by buffer line): click or drag centres the viewport there.
    let total = buf.line_count();
    let mm_visible = (mm_rect.height() / MM_LH) as usize;
    let mm_first = minimap_first(first_line, total, mm_visible, last_line - first_line);
    if cx.minimap && (mm_resp.is_pointer_button_down_on() || vs.minimap_drag) {
        vs.minimap_drag = ui.input(|i| i.pointer.primary_down());
        if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
            let line = (mm_first as f32 + (p.y - mm_rect.top()) / MM_LH).max(0.0) as usize;
            let row = vs.map(buf, wrap).line_row(line.min(total - 1));
            vs.scroll.y = ((row as f32 - visible_rows as f32 / 2.0) * m.lh).clamp(0.0, max_y);
        }
    }

    // ------------------------------------------------------------ highlight
    let need = if cx.minimap { last_line.max(mm_first + mm_visible).min(total) } else { last_line };
    if !tab.hl.update(buf, cx.syn, need, Duration::from_millis(12)) {
        ui.ctx().request_repaint();
    }

    // ---------------------------------------------------------------- paint
    let painter = ui.painter_at(full);
    painter.rect_filled(full, CornerRadius::ZERO, pal.bg);
    let gutter_hover = ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| gutter_rect.contains(p));
    let map = vs.map.as_ref().map(|(_, m)| m).unwrap();
    paint_text_area(ui, buf, &tab.hl, vs, map, &rows, &geo, &pal, cx, resp.has_focus(), gutter_hover);
    if cx.minimap {
        paint_minimap(ui, buf, &tab.hl, &pal, mm_rect, mm_first, mm_visible, first_line, last_line - first_line);
    }
    resp
}

/// Layout shared by input and painting.
struct Geo<'a> {
    m: &'a Metrics,
    text: Rect,
    gutter: Rect,
    wrap: Option<usize>,
}

impl Geo<'_> {
    fn origin(&self, vs: &ViewState) -> Pos2 {
        Pos2::new(self.text.left() + 4.0 - vs.scroll.x, self.text.top() - vs.scroll.y)
    }

    /// Fold arrows sit in the rightmost part of the gutter.
    fn in_fold_column(&self, p: Pos2) -> bool {
        self.gutter.contains(p) && p.x > self.gutter.right() - 2.0 * self.m.cw
    }
}

fn minimap_first(first_line: usize, total: usize, mm_visible: usize, visible: usize) -> usize {
    if total <= mm_visible {
        return 0;
    }
    let max_first = total.saturating_sub(visible).max(1);
    let frac = (first_line as f32 / max_first as f32).clamp(0.0, 1.0);
    let f = (frac * (total - mm_visible) as f32) as usize;
    // Never let the viewport fall off the minimap.
    f.min(first_line)
}

fn reveal(buf: &Buffer, vs: &mut ViewState, g: &Geo) {
    let s = *buf.sels.last().unwrap();
    let map = vs.map(buf, g.wrap);
    let (row, line, col) = map.place(buf, s.head);
    let start = map.row(buf, row).start;
    let m = g.m;
    let y = row as f32 * m.lh;
    let margin = m.lh * 2.0;
    if y < vs.scroll.y + margin {
        vs.scroll.y = (y - margin).max(0.0);
    } else if y + m.lh > vs.scroll.y + g.text.height() - margin {
        vs.scroll.y = y + m.lh - g.text.height() + margin;
    }
    if g.wrap.is_none() {
        let x = (buf.visual_col(line, col) - buf.visual_col(line, start)) as f32 * m.cw;
        let xm = m.cw * 4.0;
        if x < vs.scroll.x + xm {
            vs.scroll.x = (x - xm).max(0.0);
        } else if x > vs.scroll.x + g.text.width() - xm {
            vs.scroll.x = x - g.text.width() + xm;
        }
    }
}

/// Row under a point, and the absolute visual column within its line.
fn row_vcol_at(buf: &Buffer, vs: &mut ViewState, g: &Geo, p: Pos2) -> (usize, Row, f32) {
    let o = g.origin(vs);
    let map = vs.map(buf, g.wrap);
    let i = ((p.y - o.y) / g.m.lh).floor().max(0.0) as usize;
    let i = i.min(map.row_count(buf) - 1);
    let row = map.row(buf, i);
    let vcol = buf.visual_col(row.line, row.start) as f32 + (p.x - o.x) / g.m.cw;
    (i, row, vcol)
}

fn pos_at(buf: &Buffer, vs: &mut ViewState, g: &Geo, p: Pos2) -> usize {
    let (i, row, vcol) = row_vcol_at(buf, vs, g, p);
    let col = buf.col_from_visual(row.line, vcol.max(0.0).round() as usize);
    // Stay on this row: the end of a wrapped row is the next row's start.
    let last = vs.map(buf, g.wrap).is_last_of_line(i);
    let hi = if last { row.end } else { row.end.saturating_sub(1).max(row.start) };
    buf.line_start(row.line) + col.clamp(row.start, hi)
}

/// Move carets by screen rows (so wrapped lines move like Sublime).
fn move_rows(buf: &mut Buffer, vs: &mut ViewState, wrap: Option<usize>, delta: isize, extend: bool) {
    let map = vs.map(buf, wrap);
    let total = map.row_count(buf) as isize;
    let new: Vec<Sel> = buf
        .sels
        .iter()
        .map(|s| {
            let (r, line, col) = map.place(buf, s.head);
            let row = map.row(buf, r);
            let goal = s.goal.unwrap_or_else(|| buf.visual_col(line, col) - buf.visual_col(line, row.start));
            let t = r as isize + delta;
            let head = if t < 0 {
                0
            } else if t >= total {
                buf.len()
            } else {
                let t = t as usize;
                let tr = map.row(buf, t);
                let base = buf.visual_col(tr.line, tr.start);
                let c = buf.col_from_visual(tr.line, base + goal);
                let hi = if map.is_last_of_line(t) { tr.end } else { tr.end.saturating_sub(1).max(tr.start) };
                buf.line_start(tr.line) + c.clamp(tr.start, hi)
            };
            Sel { anchor: if extend { s.anchor } else { head }, head, goal: Some(goal) }
        })
        .collect();
    buf.sels = new;
    buf.normalize();
    buf.seal_undo();
}

fn handle_mouse(ui: &Ui, resp: &egui::Response, buf: &mut Buffer, vs: &mut ViewState, g: &Geo) {
    let (pressed, primary_down, middle_pressed, middle_down, mods, pos) = ui.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_down(),
            i.pointer.button_pressed(egui::PointerButton::Middle),
            i.pointer.middle_down(),
            i.modifiers,
            i.pointer.interact_pos(),
        )
    });
    let Some(p) = pos else { return };
    let (m, r) = (g.m, g.text);

    // Middle-drag (or shift+right-drag in Sublime) makes a column selection.
    if middle_pressed && resp.hovered() {
        let (_, row, vcol) = row_vcol_at(buf, vs, g, p);
        vs.column_anchor = Some((row.line, vcol.max(0.0)));
    }
    if let Some((aline, avcol)) = vs.column_anchor {
        if !middle_down {
            vs.column_anchor = None;
        } else {
            let (_, row, vcol) = row_vcol_at(buf, vs, g, p);
            let line = row.line;
            let (l0, l1) = (aline.min(line), aline.max(line));
            let (v0, v1) = (avcol.round() as usize, vcol.max(0.0).round() as usize);
            buf.sels = (l0..=l1)
                .filter(|&l| buf.visual_col(l, buf.line_len(l)) >= v0.min(v1) || l0 == l1)
                .map(|l| {
                    let ls = buf.line_start(l);
                    Sel::new(ls + buf.col_from_visual(l, v0), ls + buf.col_from_visual(l, v1))
                })
                .collect();
            if buf.sels.is_empty() {
                buf.sels.push(Sel::caret(buf.lc_to_pos(line, 0)));
            }
            buf.normalize();
            buf.seal_undo();
        }
        return;
    }

    if pressed && resp.hovered() {
        // Fold arrow in the gutter, or the "…" marker after a folded line.
        let (i, row, _) = row_vcol_at(buf, vs, g, p);
        let first_of_line = row.start == 0;
        if g.in_fold_column(p) && first_of_line && (buf.is_folded(row.line) || buf.foldable(row.line)) {
            buf.toggle_fold(row.line);
            return;
        }
        if buf.is_folded(row.line) && vs.map(buf, g.wrap).is_last_of_line(i) {
            let o = g.origin(vs);
            let x = o.x + (buf.visual_col(row.line, row.end) - buf.visual_col(row.line, row.start)) as f32 * m.cw;
            if p.x >= x && p.x <= x + FOLD_MARK_W * m.cw {
                buf.unfold_line(row.line);
                return;
            }
        }

        let now = Instant::now();
        let count = match vs.last_click {
            Some((t, lp, c)) if now - t < Duration::from_millis(450) && lp.distance(p) < 6.0 => c % 3 + 1,
            _ => 1,
        };
        vs.last_click = Some((now, p, count));

        let at = pos_at(buf, vs, g, p);
        let in_gutter = g.gutter.contains(p);
        let (gran, span) = if in_gutter || count == 3 {
            let l = buf.rope.char_to_line(at);
            let end = if l + 1 < buf.line_count() { buf.line_start(l + 1) } else { buf.len() };
            (3, (buf.line_start(l), end))
        } else if count == 2 {
            (2, buf.word_at(at))
        } else {
            (1, (at, at))
        };
        let new = Sel::new(span.0, span.1);
        let ctrl = mods.ctrl || mods.command;
        if mods.shift && !in_gutter {
            let last = buf.sels.last_mut().unwrap();
            last.head = at;
            last.goal = None;
        } else if ctrl {
            buf.sels.push(new);
        } else {
            buf.sels = vec![new];
        }
        let idx = buf.sels.len() - 1;
        vs.drag = Some((idx, span, gran));
        buf.seal_undo();
    } else if let Some((idx, span, gran)) = vs.drag {
        if !primary_down {
            vs.drag = None;
            buf.normalize();
        } else if idx < buf.sels.len() {
            // Auto-scroll when dragging past the edges.
            if p.y < r.top() {
                vs.scroll.y -= m.lh;
            } else if p.y > r.bottom() {
                vs.scroll.y += m.lh;
            }
            let at = pos_at(buf, vs, g, p);
            let (a, b) = match gran {
                2 => buf.word_at(at),
                3 => {
                    let l = buf.rope.char_to_line(at);
                    (buf.line_start(l), if l + 1 < buf.line_count() { buf.line_start(l + 1) } else { buf.len() })
                }
                _ => (at, at),
            };
            let s = &mut buf.sels[idx];
            if gran == 1 && span.0 == span.1 {
                s.head = at;
            } else if a < span.0 {
                *s = Sel::new(span.1, a);
            } else {
                *s = Sel::new(span.0, b.max(span.1));
            }
            ui.ctx().request_repaint();
        }
    }
}

fn handle_keys(ui: &Ui, buf: &mut Buffer, vs: &mut ViewState, clip: &mut Clip, page: usize, wrap: Option<usize>) {
    let events = ui.input(|i| i.events.clone());
    for ev in events {
        match ev {
            Event::Text(t) => {
                let mut chars = t.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => buf.type_char(c),
                    _ => buf.insert(&t),
                }
                vs.reveal = true;
            }
            Event::Copy => {
                let (t, whole) = buf.copy_text();
                ui.ctx().copy_text(t.clone());
                *clip = Clip { text: t, whole_lines: whole };
            }
            Event::Cut => {
                let (t, whole) = buf.cut();
                ui.ctx().copy_text(t.clone());
                *clip = Clip { text: t, whole_lines: whole };
                vs.reveal = true;
            }
            Event::Paste(t) => {
                let whole = clip.whole_lines && clip.text.replace("\r\n", "\n") == t;
                buf.paste(&t, whole);
                vs.reveal = true;
            }
            Event::Key { key, pressed: true, modifiers: mods, .. } => {
                let ctrl = mods.ctrl || mods.command;
                let shift = mods.shift;
                if mods.alt {
                    continue;
                }
                let handled = match key {
                    Key::ArrowLeft => {
                        buf.move_left(shift, ctrl);
                        true
                    }
                    Key::ArrowRight => {
                        buf.move_right(shift, ctrl);
                        true
                    }
                    Key::ArrowUp if !ctrl => {
                        move_rows(buf, vs, wrap, -1, shift);
                        true
                    }
                    Key::ArrowDown if !ctrl => {
                        move_rows(buf, vs, wrap, 1, shift);
                        true
                    }
                    Key::ArrowUp | Key::ArrowDown => {
                        // Ctrl+Up/Down scrolls without moving the caret.
                        let d = if key == Key::ArrowUp { -1.0 } else { 1.0 };
                        vs.scroll.y += d * 20.0;
                        continue;
                    }
                    Key::PageUp if !ctrl => {
                        move_rows(buf, vs, wrap, -(page as isize), shift);
                        true
                    }
                    Key::PageDown if !ctrl => {
                        move_rows(buf, vs, wrap, page as isize, shift);
                        true
                    }
                    Key::Home if ctrl => {
                        buf.move_doc(false, shift);
                        true
                    }
                    Key::End if ctrl => {
                        buf.move_doc(true, shift);
                        true
                    }
                    Key::Home => {
                        buf.move_home(shift);
                        true
                    }
                    Key::End => {
                        buf.move_end(shift);
                        true
                    }
                    Key::Backspace if !(ctrl && shift) => {
                        buf.backspace(ctrl);
                        true
                    }
                    Key::Delete if !(ctrl && shift) && !shift => {
                        buf.delete_forward(ctrl);
                        true
                    }
                    Key::Enter if !ctrl => {
                        buf.newline();
                        true
                    }
                    Key::Tab if !ctrl && shift => {
                        buf.unindent();
                        true
                    }
                    Key::Tab if !ctrl => {
                        buf.tab();
                        true
                    }
                    Key::Escape => {
                        if buf.sels.len() > 1 {
                            buf.single_selection();
                        } else if let Some(s) = buf.sels.first_mut() {
                            *s = Sel::caret(s.head);
                        }
                        true
                    }
                    _ => false,
                };
                if handled {
                    vs.reveal = true;
                }
            }
            _ => {}
        }
    }
}

/// Width, in columns, of the "…" marker after a folded line.
const FOLD_MARK_W: f32 = 3.0;

#[allow(clippy::too_many_arguments)]
fn paint_text_area(
    ui: &Ui,
    buf: &Buffer,
    hl: &HlCache,
    vs: &ViewState,
    map: &DisplayMap,
    rows: &[(usize, Row)],
    g: &Geo,
    pal: &Palette,
    cx: &ViewCtx,
    focused: bool,
    gutter_hover: bool,
) {
    let (m, r, gutter) = (g.m, g.text, g.gutter);
    let painter = ui.painter_at(r);
    let gpainter = ui.painter_at(gutter);
    let origin = g.origin(vs);
    let Some(&(first_row, _)) = rows.first() else { return };
    let last_row = first_row + rows.len();
    let visible = |row: usize| row >= first_row && row < last_row;
    let y_of = |row: usize| origin.y + row as f32 * m.lh;
    // x of (line, col) on a row starting at char `start` of that line.
    let x_of = |line: usize, start: usize, col: usize| {
        origin.x + (buf.visual_col(line, col) as f32 - buf.visual_col(line, start) as f32) * m.cw
    };
    let place = |pos: usize| {
        let (row, line, col) = map.place(buf, pos);
        (row, line, col, map.row(buf, row).start)
    };

    let caret_lines: Vec<usize> = buf.sels.iter().map(|s| buf.rope.char_to_line(s.head)).collect();

    // Current-line highlight (the caret's row).
    for s in buf.sels.iter().filter(|s| s.is_empty()) {
        let (row, ..) = place(s.head);
        if visible(row) {
            let y = y_of(row);
            painter.rect_filled(Rect::from_min_max(Pos2::new(r.left(), y), Pos2::new(r.right(), y + m.lh)), 0.0, pal.line_highlight);
        }
    }

    // Selections, row by row.
    for s in buf.sels.iter().filter(|s| !s.is_empty()) {
        for &(i, row) in rows {
            let ls = buf.line_start(row.line);
            let (ra, rb) = (ls + row.start, ls + row.end);
            let (a, b) = (s.start().max(ra), s.end().min(rb));
            // The newline at the end of a line counts as selected if the
            // selection continues past it.
            let past_eol = map.is_last_of_line(i) && s.end() > rb && s.start() <= rb;
            if a > b || (a == b && !past_eol) {
                continue;
            }
            let extra = if past_eol { m.cw * 0.6 } else { 0.0 };
            let x0 = x_of(row.line, row.start, a - ls);
            let x1 = x_of(row.line, row.start, b - ls) + extra;
            let rect = Rect::from_min_max(Pos2::new(x0, y_of(i)), Pos2::new(x1, y_of(i) + m.lh));
            painter.rect_filled(rect, 2.0, pal.selection);
        }
    }

    // Outlines: find matches, or other occurrences of the selected word.
    let word_matcher = match cx.find {
        Some(_) => None,
        None => {
            let s = buf.sels.last().unwrap();
            let t = buf.sel_text(s);
            (buf.sels.len() == 1 && !t.is_empty() && t.chars().count() < 100 && t.chars().all(is_word_char))
                .then(|| Search { pattern: t, case: true, regex: false, whole_word: true }.compile().ok())
                .flatten()
        }
    };
    if let Some(matcher) = cx.find.or(word_matcher.as_ref()) {
        let color = pal.fg.linear_multiply(0.5);
        let mut line_cache: Option<(usize, Vec<(usize, usize)>)> = None;
        for &(i, row) in rows {
            if line_cache.as_ref().is_none_or(|(l, _)| *l != row.line) {
                let text = buf.line_str(row.line);
                let hits = matcher.find_iter(&text).map(|(a, b)| (text[..a].chars().count(), text[..b].chars().count())).collect();
                line_cache = Some((row.line, hits));
            }
            for &(ca, cb) in &line_cache.as_ref().unwrap().1 {
                let (a, b) = (ca.max(row.start), cb.min(row.end));
                if a >= b {
                    continue;
                }
                let rect = Rect::from_min_max(
                    Pos2::new(x_of(row.line, row.start, a), y_of(i)),
                    Pos2::new(x_of(row.line, row.start, b), y_of(i) + m.lh),
                );
                painter.rect_stroke(rect, 2.0, Stroke::new(1.0, color), StrokeKind::Inside);
            }
        }
    }

    // Text, plus the "…" marker after folded lines.
    for &(i, row) in rows {
        let job = line_job(buf, hl, row, m, pal);
        let galley = painter.layout_job(job);
        let y = y_of(i) + (m.lh - galley.size().y) / 2.0;
        painter.galley(Pos2::new(origin.x, y), galley, pal.fg);
        if buf.is_folded(row.line) && map.is_last_of_line(i) {
            let x = x_of(row.line, row.start, row.end) + m.cw * 0.5;
            let mark = Rect::from_min_size(Pos2::new(x, y_of(i) + 3.0), Vec2::new((FOLD_MARK_W - 0.5) * m.cw, m.lh - 6.0));
            painter.rect_filled(mark, 3.0, pal.selection);
            painter.text(mark.center(), Align2::CENTER_CENTER, "…", m.font.clone(), pal.gutter_fg);
        }
    }

    // Matching brackets for the primary caret.
    let head = buf.sels.last().unwrap().head;
    let pair = buf.matching_bracket(head).or_else(|| head.checked_sub(1).and_then(|h| buf.matching_bracket(h)));
    if let Some((a, b)) = pair {
        for p in [a, b] {
            let (row, line, col, start) = place(p);
            if visible(row) && buf.rope.char_to_line(p) == line {
                let y = y_of(row) + m.lh - 2.0;
                painter.line_segment(
                    [Pos2::new(x_of(line, start, col), y), Pos2::new(x_of(line, start, col + 1), y)],
                    Stroke::new(1.5, pal.caret),
                );
            }
        }
    }

    // Carets: solid right after activity, then blinking.
    let since = ui.input(|i| i.time) - vs.activity;
    let blink_on = !focused || since < 0.5 || ((since - 0.5) * 1.8).fract() < 0.6;
    if focused {
        ui.ctx().request_repaint_after(Duration::from_millis(200));
    }
    if blink_on {
        let caret_color = if focused { pal.caret } else { pal.caret.linear_multiply(0.4) };
        for s in &buf.sels {
            let (row, line, col, start) = place(s.head);
            if visible(row) {
                let x = x_of(line, start, col);
                painter.rect_filled(Rect::from_min_size(Pos2::new(x - 1.0, y_of(row) + 1.0), Vec2::new(2.0, m.lh - 2.0)), 0.0, caret_color);
            }
        }
    }

    // Gutter: line numbers on each line's first row, then fold arrows.
    let arrow_x = gutter.right() - m.cw;
    for &(i, row) in rows {
        if row.start != 0 {
            continue;
        }
        let current = caret_lines.contains(&row.line);
        let color = if current { pal.fg } else { pal.gutter_fg };
        let cy = y_of(i) + m.lh / 2.0;
        gpainter.text(Pos2::new(gutter.right() - m.cw * 2.2, cy), Align2::RIGHT_CENTER, (row.line + 1).to_string(), m.font.clone(), color);
        let folded = buf.is_folded(row.line);
        if folded || (gutter_hover && buf.foldable(row.line)) {
            let s = m.cw * 0.38;
            let pts = if folded {
                vec![Pos2::new(arrow_x - s * 0.6, cy - s), Pos2::new(arrow_x + s * 0.8, cy), Pos2::new(arrow_x - s * 0.6, cy + s)]
            } else {
                vec![Pos2::new(arrow_x - s, cy - s * 0.6), Pos2::new(arrow_x + s, cy - s * 0.6), Pos2::new(arrow_x, cy + s * 0.8)]
            };
            gpainter.add(egui::Shape::convex_polygon(pts, if folded { pal.fg } else { pal.gutter_fg }, Stroke::NONE));
        }
    }
}

/// Layout for one row: chars `row.start..row.end` of its line, with tabs
/// expanded relative to the line start.
fn line_job(buf: &Buffer, hl: &HlCache, row: Row, m: &Metrics, pal: &Palette) -> LayoutJob {
    let mut job = LayoutJob::default();
    let text = buf.line_str(row.line);
    let fmt = |color: Color32| TextFormat { font_id: m.font.clone(), color, ..Default::default() };
    let mut vcol = buf.visual_col(row.line, row.start);
    let mut expand = |s: &str, out: &mut String| {
        for c in s.chars() {
            if c == '\t' {
                let n = TAB_SIZE - vcol % TAB_SIZE;
                out.extend(std::iter::repeat_n(' ', n));
                vcol += n;
            } else if c != '\n' {
                out.push(c);
                vcol += 1;
            }
        }
    };
    let spans: Vec<(usize, Color32)> = match hl.line(row.line) {
        Some(spans) => spans.iter().map(|sp| (sp.chars, sp.color)).collect(),
        None => vec![(text.chars().count(), pal.fg)],
    };
    // Walk the spans, keeping only the part inside this row.
    let mut pos = 0;
    let mut chars = text.chars();
    for (n, color) in spans {
        let (a, b) = (pos.max(row.start), (pos + n).min(row.end));
        let span: String = chars.by_ref().take(n).collect();
        let piece: String = span.chars().skip(a.saturating_sub(pos)).take(b.saturating_sub(a)).collect();
        pos += n;
        if a < b {
            let mut out = String::new();
            expand(&piece, &mut out);
            if !out.is_empty() {
                job.append(&out, 0.0, fmt(color));
            }
        }
        if pos >= row.end {
            break;
        }
    }
    job
}

#[allow(clippy::too_many_arguments)]
fn paint_minimap(
    ui: &Ui,
    buf: &Buffer,
    hl: &HlCache,
    pal: &Palette,
    r: Rect,
    mm_first: usize,
    mm_visible: usize,
    first: usize,
    visible: usize,
) {
    let painter = ui.painter_at(r);
    let end = (mm_first + mm_visible + 1).min(buf.line_count());
    let max_cols = ((r.width() - 8.0) / MM_CW) as usize;
    for l in mm_first..end {
        let y = r.top() + (l - mm_first) as f32 * MM_LH;
        let text = buf.line_str(l);
        let mut col = 0usize;
        let mut chars = text.chars();
        let spans: Vec<(usize, Color32)> = match hl.line(l) {
            Some(s) => s.iter().map(|s| (s.chars, s.color)).collect(),
            None => vec![(text.chars().count(), pal.fg)],
        };
        for (n, color) in spans {
            let color = color.linear_multiply(0.55);
            let mut run_start: Option<usize> = None;
            for c in chars.by_ref().take(n) {
                let w = if c == '\t' { TAB_SIZE - col % TAB_SIZE } else { 1 };
                if c.is_whitespace() {
                    if let Some(s) = run_start.take() {
                        mm_rect(&painter, r, y, s, col, color);
                    }
                } else if run_start.is_none() {
                    run_start = Some(col);
                }
                col += w;
                if col > max_cols {
                    break;
                }
            }
            if let Some(s) = run_start {
                mm_rect(&painter, r, y, s, col, color);
            }
            if col > max_cols {
                break;
            }
        }
    }
    // Viewport indicator.
    let top = r.top() + (first as f32 - mm_first as f32) * MM_LH;
    let vr = Rect::from_min_size(Pos2::new(r.left(), top), Vec2::new(r.width(), visible as f32 * MM_LH));
    painter.rect_filled(vr, 0.0, pal.fg.linear_multiply(0.06));
}

fn mm_rect(p: &egui::Painter, r: Rect, y: f32, a: usize, b: usize, color: Color32) {
    let x0 = r.left() + 6.0 + a as f32 * MM_CW;
    let x1 = r.left() + 6.0 + b as f32 * MM_CW;
    p.rect_filled(Rect::from_min_max(Pos2::new(x0, y), Pos2::new(x1, y + MM_LH - 0.6)), 0.0, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    fn buf(text: &str) -> Buffer {
        let mut b = Buffer::new();
        b.rope = Rope::from_str(text);
        b
    }

    #[test]
    fn arrows_move_by_wrapped_rows() {
        // "aaaa bbbb cccc" wraps at 8 columns into "aaaa ", "bbbb ", "cccc".
        let mut b = buf("aaaa bbbb cccc\nx\n");
        let mut vs = ViewState::default();
        b.sels = vec![Sel::caret(1)];
        move_rows(&mut b, &mut vs, Some(8), 1, false);
        assert_eq!(b.sels[0].head, 6, "second row, same column");
        move_rows(&mut b, &mut vs, Some(8), 1, false);
        assert_eq!(b.sels[0].head, 11, "third row");
        move_rows(&mut b, &mut vs, Some(8), 1, false);
        assert_eq!(b.pos_to_lc(b.sels[0].head), (1, 1), "next line, clamped to its end");
        // Without wrap the same key moves by whole lines.
        b.sels = vec![Sel::caret(1)];
        move_rows(&mut b, &mut vs, None, 1, false);
        assert_eq!(b.pos_to_lc(b.sels[0].head), (1, 1));
    }

    #[test]
    fn arrows_skip_folded_lines() {
        let mut b = buf("fn a() {\n    x;\n    y;\n}\n");
        b.fold_line(0);
        let mut vs = ViewState::default();
        b.sels = vec![Sel::caret(0)];
        move_rows(&mut b, &mut vs, None, 1, false);
        assert_eq!(b.rope.char_to_line(b.sels[0].head), 3, "jumps over the fold to the closing brace");
    }
}
