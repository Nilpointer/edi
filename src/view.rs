//! The text view: a custom-painted editor widget with multiple carets,
//! gutter, minimap and Sublime-style mouse handling.

use crate::buffer::{is_word_char, Buffer, Sel, TAB_SIZE};
use crate::highlight::{HlCache, Palette, Syntaxes};
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
}

pub struct Tab {
    pub buf: Buffer,
    pub hl: HlCache,
    pub view: ViewState,
}

pub struct ViewCtx<'a> {
    pub syn: &'a Syntaxes,
    pub font_size: f32,
    pub minimap: bool,
    pub grab_focus: bool,
    /// Find-panel query to outline, with case sensitivity.
    pub find: Option<(&'a str, bool)>,
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
    let gutter_w = (digits + 3.0) * m.cw;
    let mm_w = if cx.minimap { MINIMAP_W } else { 0.0 };
    let text_rect = Rect::from_min_max(Pos2::new(full.left() + gutter_w, full.top()), Pos2::new(full.right() - mm_w, full.bottom()));
    let gutter_rect = Rect::from_min_max(full.min, Pos2::new(text_rect.left(), full.bottom()));
    let mm_rect = Rect::from_min_max(Pos2::new(text_rect.right(), full.top()), full.max);
    let id = egui::Id::unique("edi-view");
    let resp = ui.interact(Rect::from_min_max(full.min, text_rect.max), id, Sense::click_and_drag());
    let mm_resp = ui.interact(mm_rect, id.with("minimap"), Sense::click_and_drag());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Text);
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

    let visible_lines = (text_rect.height() / m.lh).floor().max(1.0) as usize;

    // ---------------------------------------------------------------- input
    let before = (buf.version, buf.sels.clone());
    if resp.has_focus() {
        handle_keys(ui, buf, vs, clip, visible_lines);
    }
    handle_mouse(ui, &resp, buf, vs, &m, text_rect, gutter_rect);
    if before.0 != buf.version || before.1 != buf.sels {
        vs.activity = ui.input(|i| i.time);
    }

    // Scrolling.
    if resp.hovered() || mm_resp.hovered() {
        let d = ui.input(|i| i.smooth_scroll_delta);
        vs.scroll -= d;
    }
    let max_y = (buf.line_count().saturating_sub(1)) as f32 * m.lh;
    if vs.reveal {
        reveal(buf, vs, &m, text_rect);
        vs.reveal = false;
    }
    vs.scroll.y = vs.scroll.y.clamp(0.0, max_y);
    vs.scroll.x = vs.scroll.x.max(0.0);

    // Minimap interaction: click or drag centres the viewport there.
    let total = buf.line_count();
    let mm_visible = (mm_rect.height() / MM_LH) as usize;
    let mm_first = minimap_first(vs.scroll.y, max_y, total, mm_visible, m.lh);
    if cx.minimap && (mm_resp.is_pointer_button_down_on() || vs.minimap_drag) {
        vs.minimap_drag = ui.input(|i| i.pointer.primary_down());
        if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
            let line = mm_first as f32 + (p.y - mm_rect.top()) / MM_LH;
            vs.scroll.y = ((line - visible_lines as f32 / 2.0) * m.lh).clamp(0.0, max_y);
        }
    }

    // ------------------------------------------------------------ highlight
    let first = (vs.scroll.y / m.lh).floor() as usize;
    let last = (first + visible_lines + 2).min(total);
    let need = if cx.minimap { last.max(mm_first + mm_visible).min(total) } else { last };
    if !tab.hl.update(buf, cx.syn, need, Duration::from_millis(12)) {
        ui.ctx().request_repaint();
    }

    // ---------------------------------------------------------------- paint
    let painter = ui.painter_at(full);
    painter.rect_filled(full, CornerRadius::ZERO, pal.bg);
    paint_text_area(ui, buf, &tab.hl, vs, &m, &pal, cx, text_rect, gutter_rect, first, last, resp.has_focus());
    if cx.minimap {
        paint_minimap(ui, buf, &tab.hl, &pal, mm_rect, mm_first, mm_visible, first, visible_lines);
    }
    resp
}

fn minimap_first(scroll_y: f32, max_y: f32, total: usize, mm_visible: usize, lh: f32) -> usize {
    if total <= mm_visible || max_y <= 0.0 {
        return 0;
    }
    let first_vis = scroll_y / lh;
    let frac = (scroll_y / max_y).clamp(0.0, 1.0);
    let f = frac * (total - mm_visible) as f32;
    // Never let the viewport fall off the minimap.
    f.min(first_vis).max(0.0) as usize
}

fn reveal(buf: &Buffer, vs: &mut ViewState, m: &Metrics, r: Rect) {
    let s = buf.sels.last().unwrap();
    let (line, col) = buf.pos_to_lc(s.head);
    let y = line as f32 * m.lh;
    let margin = m.lh * 2.0;
    if y < vs.scroll.y + margin {
        vs.scroll.y = (y - margin).max(0.0);
    } else if y + m.lh > vs.scroll.y + r.height() - margin {
        vs.scroll.y = y + m.lh - r.height() + margin;
    }
    let x = buf.visual_col(line, col) as f32 * m.cw;
    let xm = m.cw * 4.0;
    if x < vs.scroll.x + xm {
        vs.scroll.x = (x - xm).max(0.0);
    } else if x > vs.scroll.x + r.width() - xm {
        vs.scroll.x = x - r.width() + xm;
    }
}

fn pos_at(buf: &Buffer, vs: &ViewState, m: &Metrics, r: Rect, p: Pos2) -> usize {
    let (line, vcol) = line_vcol_at(buf, vs, m, r, p);
    buf.line_start(line) + buf.col_from_visual(line, vcol.max(0.0).round() as usize)
}

fn line_vcol_at(buf: &Buffer, vs: &ViewState, m: &Metrics, r: Rect, p: Pos2) -> (usize, f32) {
    let line = ((p.y - r.top() + vs.scroll.y) / m.lh).floor().max(0.0) as usize;
    let line = line.min(buf.line_count() - 1);
    let vcol = (p.x - r.left() + vs.scroll.x - 4.0) / m.cw;
    (line, vcol)
}

fn handle_mouse(ui: &Ui, resp: &egui::Response, buf: &mut Buffer, vs: &mut ViewState, m: &Metrics, r: Rect, gutter: Rect) {
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
    let r = Rect::from_min_max(r.min, r.max);

    // Middle-drag (or shift+right-drag in Sublime) makes a column selection.
    if middle_pressed && resp.hovered() {
        let (line, vcol) = line_vcol_at(buf, vs, m, r, p);
        vs.column_anchor = Some((line, vcol.max(0.0)));
    }
    if let Some((aline, avcol)) = vs.column_anchor {
        if !middle_down {
            vs.column_anchor = None;
        } else {
            let (line, vcol) = line_vcol_at(buf, vs, m, r, p);
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
        let now = Instant::now();
        let count = match vs.last_click {
            Some((t, lp, c)) if now - t < Duration::from_millis(450) && lp.distance(p) < 6.0 => c % 3 + 1,
            _ => 1,
        };
        vs.last_click = Some((now, p, count));

        let at = pos_at(buf, vs, m, r, p);
        let in_gutter = gutter.contains(p);
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
            let at = pos_at(buf, vs, m, r, p);
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

fn handle_keys(ui: &Ui, buf: &mut Buffer, vs: &mut ViewState, clip: &mut Clip, page: usize) {
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
                        buf.move_vertical(-1, shift);
                        true
                    }
                    Key::ArrowDown if !ctrl => {
                        buf.move_vertical(1, shift);
                        true
                    }
                    Key::ArrowUp | Key::ArrowDown => {
                        // Ctrl+Up/Down scrolls without moving the caret.
                        let d = if key == Key::ArrowUp { -1.0 } else { 1.0 };
                        vs.scroll.y += d * 20.0;
                        continue;
                    }
                    Key::PageUp if !ctrl => {
                        buf.move_vertical(-(page as isize), shift);
                        true
                    }
                    Key::PageDown if !ctrl => {
                        buf.move_vertical(page as isize, shift);
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

#[allow(clippy::too_many_arguments)]
fn paint_text_area(
    ui: &Ui,
    buf: &Buffer,
    hl: &HlCache,
    vs: &ViewState,
    m: &Metrics,
    pal: &Palette,
    cx: &ViewCtx,
    r: Rect,
    gutter: Rect,
    first: usize,
    last: usize,
    focused: bool,
) {
    let painter = ui.painter_at(r);
    let gpainter = ui.painter_at(gutter);
    let origin = Pos2::new(r.left() + 4.0 - vs.scroll.x, r.top() - vs.scroll.y);
    let x_of = |line: usize, col: usize| origin.x + buf.visual_col(line, col) as f32 * m.cw;
    let y_of = |line: usize| origin.y + line as f32 * m.lh;

    let caret_lines: Vec<usize> = buf.sels.iter().map(|s| buf.rope.char_to_line(s.head)).collect();

    // Current-line highlight.
    for s in &buf.sels {
        if s.is_empty() {
            let l = buf.rope.char_to_line(s.head);
            if l >= first && l < last {
                let y = y_of(l);
                painter.rect_filled(Rect::from_min_max(Pos2::new(r.left(), y), Pos2::new(r.right(), y + m.lh)), 0.0, pal.line_highlight);
            }
        }
    }

    // Selections.
    for s in buf.sels.iter().filter(|s| !s.is_empty()) {
        let (ls, cs) = buf.pos_to_lc(s.start());
        let (le, ce) = buf.pos_to_lc(s.end());
        for l in ls.max(first)..=le.min(last.saturating_sub(1)) {
            let a = if l == ls { cs } else { 0 };
            let (b, extra) = if l == le { (ce, 0.0) } else { (buf.line_len(l), m.cw * 0.6) };
            let rect = Rect::from_min_max(Pos2::new(x_of(l, a), y_of(l)), Pos2::new(x_of(l, b) + extra, y_of(l) + m.lh));
            painter.rect_filled(rect, 2.0, pal.selection);
        }
    }

    // Outlines: find matches, or other occurrences of the selected word.
    let outline_query: Option<(String, bool, bool)> = match cx.find {
        Some((q, case)) if !q.is_empty() => Some((q.to_string(), case, false)),
        _ => {
            let s = buf.sels.last().unwrap();
            let t = buf.sel_text(s);
            (buf.sels.len() == 1 && !t.is_empty() && t.chars().count() < 100 && t.chars().all(is_word_char))
                .then_some((t, true, true))
        }
    };
    if let Some((q, case, whole_word)) = outline_query {
        let ql = if case { q.clone() } else { q.to_lowercase() };
        let qn = q.chars().count();
        let color = pal.fg.linear_multiply(0.5);
        for l in first..last {
            let text = buf.line_str(l);
            let hay = if case { text.clone() } else { text.to_lowercase() };
            if hay.len() != text.len() {
                continue;
            }
            for (b, _) in hay.match_indices(&ql) {
                let col = text[..b].chars().count();
                if whole_word {
                    let before = text[..b].chars().last();
                    let after = text[b + ql.len()..].chars().next();
                    if before.is_some_and(is_word_char) || after.is_some_and(is_word_char) {
                        continue;
                    }
                }
                let rect = Rect::from_min_max(Pos2::new(x_of(l, col), y_of(l)), Pos2::new(x_of(l, col + qn), y_of(l) + m.lh));
                painter.rect_stroke(rect, 2.0, Stroke::new(1.0, color), StrokeKind::Inside);
            }
        }
    }

    // Text.
    for l in first..last {
        let job = line_job(buf, hl, l, m, pal);
        let galley = painter.layout_job(job);
        let y = y_of(l) + (m.lh - galley.size().y) / 2.0;
        painter.galley(Pos2::new(origin.x, y), galley, pal.fg);
    }

    // Matching brackets for the primary caret.
    let head = buf.sels.last().unwrap().head;
    let pair = buf.matching_bracket(head).or_else(|| head.checked_sub(1).and_then(|h| buf.matching_bracket(h)));
    if let Some((a, b)) = pair {
        for p in [a, b] {
            let (l, c) = buf.pos_to_lc(p);
            if l >= first && l < last {
                let y = y_of(l) + m.lh - 2.0;
                painter.line_segment([Pos2::new(x_of(l, c), y), Pos2::new(x_of(l, c + 1), y)], Stroke::new(1.5, pal.caret));
            }
        }
    }

    // Carets: solid right after activity, then blinking.
    let since = ui.input(|i| i.time) - vs.activity;
    let visible = !focused || since < 0.5 || ((since - 0.5) * 1.8).fract() < 0.6;
    if focused {
        ui.ctx().request_repaint_after(Duration::from_millis(200));
    }
    if visible {
        let caret_color = if focused { pal.caret } else { pal.caret.linear_multiply(0.4) };
        for s in &buf.sels {
            let (l, c) = buf.pos_to_lc(s.head);
            if l >= first && l < last {
                let x = x_of(l, c);
                painter.rect_filled(Rect::from_min_size(Pos2::new(x - 1.0, y_of(l) + 1.0), Vec2::new(2.0, m.lh - 2.0)), 0.0, caret_color);
            }
        }
    }

    // Gutter.
    for l in first..last {
        let current = caret_lines.contains(&l);
        let color = if current { pal.fg } else { pal.gutter_fg };
        gpainter.text(
            Pos2::new(gutter.right() - m.cw * 1.5, y_of(l) + m.lh / 2.0),
            Align2::RIGHT_CENTER,
            (l + 1).to_string(),
            m.font.clone(),
            color,
        );
    }
}

fn line_job(buf: &Buffer, hl: &HlCache, l: usize, m: &Metrics, pal: &Palette) -> LayoutJob {
    let mut job = LayoutJob::default();
    let text = buf.line_str(l);
    let fmt = |color: Color32| TextFormat { font_id: m.font.clone(), color, ..Default::default() };
    let mut vcol = 0;
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
    match hl.line(l) {
        Some(spans) => {
            let mut chars = text.chars();
            for sp in spans {
                let piece: String = chars.by_ref().take(sp.chars).collect();
                let mut out = String::new();
                expand(&piece, &mut out);
                if !out.is_empty() {
                    job.append(&out, 0.0, fmt(sp.color));
                }
            }
        }
        None => {
            let mut out = String::new();
            expand(&text, &mut out);
            job.append(&out, 0.0, fmt(pal.fg));
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
