//! Split panes ("groups", as Sublime calls them): each group shows its own
//! tabs. `EdiApp::active` is always the focused group's active tab.

use super::{editor_id, EdiApp, FindMode};
use crate::view::{self, ViewCtx};
use eframe::egui::{self, Align, Align2, Color32, CornerRadius, FontId, Rect, Sense, Ui, Vec2};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaneLayout {
    #[default]
    Single,
    Columns(u8),
    Rows(u8),
    Grid,
}

impl PaneLayout {
    pub fn groups(self) -> usize {
        match self {
            PaneLayout::Single => 1,
            PaneLayout::Columns(n) | PaneLayout::Rows(n) => n as usize,
            PaneLayout::Grid => 4,
        }
    }

    /// Stable number for the session file and commands.
    pub fn code(self) -> u8 {
        match self {
            PaneLayout::Single => 0,
            PaneLayout::Columns(n) => n.clamp(2, 4) - 1,
            PaneLayout::Rows(n) => n.clamp(2, 3) + 2,
            PaneLayout::Grid => 6,
        }
    }

    pub fn from_code(code: u8) -> Self {
        match code {
            1..=3 => PaneLayout::Columns(code + 1),
            4 | 5 => PaneLayout::Rows(code - 2),
            6 => PaneLayout::Grid,
            _ => PaneLayout::Single,
        }
    }

    fn rects(self, r: Rect) -> Vec<Rect> {
        let split = |r: Rect, n: usize, horizontal: bool| -> Vec<Rect> {
            (0..n)
                .map(|i| {
                    let (a, b) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
                    if horizontal {
                        Rect::from_x_y_ranges(r.left() + r.width() * a..=r.left() + r.width() * b, r.y_range())
                    } else {
                        Rect::from_x_y_ranges(r.x_range(), r.top() + r.height() * a..=r.top() + r.height() * b)
                    }
                })
                .collect()
        };
        match self {
            PaneLayout::Single => vec![r],
            PaneLayout::Columns(n) => split(r, n as usize, true),
            PaneLayout::Rows(n) => split(r, n as usize, false),
            PaneLayout::Grid => split(r, 2, false).into_iter().flat_map(|row| split(row, 2, true)).collect(),
        }
    }
}

enum TabAction {
    Activate(usize),
    Close(usize),
    MoveTo(usize, usize),
}

impl EdiApp {
    /// Make tab `i` the active tab of its group and focus that group.
    pub(super) fn activate(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        self.active = i;
        self.focus_group = self.tabs[i].group;
        self.group_active[self.focus_group] = Some(i);
        self.focus_editor = true;
    }

    pub(super) fn tabs_in_group(&self, g: usize) -> Vec<usize> {
        (0..self.tabs.len()).filter(|&i| self.tabs[i].group == g).collect()
    }

    /// Re-establish the invariants after tabs or the layout changed.
    pub(super) fn normalize_groups(&mut self) {
        let n = self.layout.groups();
        self.group_active.resize(n, None);
        for t in &mut self.tabs {
            t.group = t.group.min(n - 1);
        }
        for g in 0..n {
            let ok = self.group_active[g].is_some_and(|i| i < self.tabs.len() && self.tabs[i].group == g);
            if !ok {
                self.group_active[g] = self.tabs_in_group(g).last().copied();
            }
        }
        self.focus_group = self.focus_group.min(n - 1);
        if let Some(i) = self.group_active[self.focus_group] {
            self.active = i;
        } else if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
    }

    /// Fix group bookkeeping after tab `removed` was taken out of `tabs`.
    pub(super) fn tab_removed(&mut self, removed: usize) {
        for ga in &mut self.group_active {
            *ga = match *ga {
                Some(i) if i == removed => None,
                Some(i) if i > removed => Some(i - 1),
                other => other,
            };
        }
        if self.active > removed {
            self.active -= 1;
        }
        self.normalize_groups();
    }

    pub(super) fn set_layout(&mut self, layout: PaneLayout) {
        self.layout = layout;
        self.normalize_groups();
        if let Some(i) = self.group_active[self.focus_group] {
            self.activate(i);
        }
    }

    pub(super) fn focus_group(&mut self, g: usize) {
        if g >= self.layout.groups() {
            return;
        }
        self.focus_group = g;
        if let Some(i) = self.group_active[g] {
            self.activate(i);
        }
    }

    pub(super) fn move_to_group(&mut self, tab: usize, g: usize) {
        if g >= self.layout.groups() || tab >= self.tabs.len() {
            return;
        }
        self.tabs[tab].group = g;
        self.normalize_groups();
        self.activate(tab);
    }

    /// Step through the focused group's tabs.
    pub(super) fn cycle_tab(&mut self, forward: bool) {
        let tabs = self.tabs_in_group(self.focus_group);
        let Some(pos) = tabs.iter().position(|&i| i == self.active) else { return };
        let n = tabs.len();
        let next = if forward { (pos + 1) % n } else { (pos + n - 1) % n };
        self.activate(tabs[next]);
    }

    /// Alt+1…9 within the focused group; Alt+9 is the last tab.
    pub(super) fn switch_tab(&mut self, n: usize) {
        let tabs = self.tabs_in_group(self.focus_group);
        let pick = if n == 8 { tabs.last() } else { tabs.get(n) };
        if let Some(&i) = pick {
            self.activate(i);
        }
    }

    /// The editor area: every group's tab strip and view.
    pub(super) fn groups_ui(&mut self, ui: &mut Ui, grab: bool) {
        let full = ui.available_rect_before_wrap();
        let rects = self.layout.rects(full);
        let multi = rects.len() > 1;
        let outline = if self.find.open && self.find.mode != FindMode::Files { self.find.matcher() } else { None };
        let mut actions = Vec::new();
        let mut focus = None;
        let mut jump = None;

        for (g, r) in rects.iter().copied().enumerate() {
            let inner = if multi { r.shrink(0.5) } else { r };
            ui.scope_builder(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(Align::Min)), |ui| {
                ui.set_clip_rect(inner);
                let strip = egui::Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .inner_margin(egui::Margin { left: 4, right: 4, top: 4, bottom: 0 })
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        self.tab_strip(ui, g, &mut actions);
                    })
                    .response
                    .rect;
                if multi && g == self.focus_group {
                    let accent = ui.visuals().selection.stroke.color;
                    ui.painter().rect_filled(Rect::from_min_size(strip.min, Vec2::new(strip.width(), 2.0)), 0.0, accent);
                }

                let Some(ti) = self.group_active[g] else {
                    let rest = ui.available_rect_before_wrap();
                    let resp = ui.allocate_rect(rest, Sense::click());
                    let pal = self.syn.palette();
                    ui.painter().rect_filled(rest, CornerRadius::ZERO, pal.bg);
                    ui.painter().text(rest.center(), Align2::CENTER_CENTER, "Empty group", FontId::proportional(14.0), pal.gutter_fg);
                    if resp.clicked() {
                        focus = Some(Err(g));
                    }
                    return;
                };
                let results_matcher = self.tabs[ti].results.as_ref().map(|r| r.matcher.clone());
                let find = outline.as_ref().or(results_matcher.as_ref());
                let cx = ViewCtx {
                    syn: &self.syn,
                    font_size: self.font_size,
                    minimap: self.show_minimap,
                    wrap: self.word_wrap,
                    grab_focus: grab && g == self.focus_group,
                    id: editor_id(g),
                    find,
                };
                let tab = &mut self.tabs[ti];
                let resp = view::show(ui, tab, &cx, &mut self.clip);
                // Clicking into another group focuses it. (Keyboard focus may
                // lag a frame behind group focus, so it doesn't count.)
                if g != self.focus_group && (resp.clicked() || resp.drag_started()) {
                    focus = Some(Ok(ti));
                }
                // Double-clicking a Find Results line opens that match.
                if resp.double_clicked() && tab.results.is_some() {
                    jump = Some((ti, tab.buf.rope.char_to_line(tab.buf.sels.last().unwrap().head)));
                }
            });
        }

        // Separators between groups.
        if multi {
            let stroke = egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
            for r in &rects {
                if r.right() < full.right() - 1.0 {
                    ui.painter().vline(r.right(), r.y_range(), stroke);
                }
                if r.bottom() < full.bottom() - 1.0 {
                    ui.painter().hline(r.x_range(), r.bottom(), stroke);
                }
            }
        }

        match focus {
            Some(Ok(ti)) => self.activate(ti),
            Some(Err(g)) => self.focus_group = g,
            None => {}
        }
        for a in actions {
            match a {
                TabAction::Activate(i) => self.activate(i),
                TabAction::Close(i) => self.close_tab(i, false),
                TabAction::MoveTo(i, g) => self.move_to_group(i, g),
            }
        }
        if let Some((ti, line)) = jump {
            self.open_result(ti, line);
        }
    }

    fn tab_strip(&self, ui: &mut Ui, g: usize, actions: &mut Vec<TabAction>) {
        const PAD_L: f32 = 12.0;
        const GAP: f32 = 8.0;
        const CLOSE: f32 = 20.0;
        const PAD_R: f32 = 6.0;
        const HEIGHT: f32 = 30.0;
        let pal = self.syn.palette();
        let groups = self.layout.groups();
        egui::ScrollArea::horizontal()
            .id_salt(("tab-strip", g))
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    ui.set_min_height(HEIGHT);
                    for i in self.tabs_in_group(g) {
                        let t = &self.tabs[i];
                        let active = self.group_active[g] == Some(i);
                        let color = if active { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
                        let galley = ui.painter().layout_no_wrap(t.buf.name(), FontId::proportional(13.5), color);
                        let width = PAD_L + galley.size().x + GAP + CLOSE + PAD_R;
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, HEIGHT), Sense::click());

                        // The close button is registered after the tab so it is on
                        // top and gets its own clicks.
                        let close_rect = Rect::from_center_size(
                            egui::pos2(rect.right() - PAD_R - CLOSE / 2.0, rect.center().y),
                            Vec2::splat(CLOSE),
                        );
                        let close_resp = ui.interact(close_rect, egui::Id::unique(("tab-close", i)), Sense::click());

                        let fill = if active {
                            pal.bg
                        } else if resp.hovered() {
                            ui.visuals().widgets.hovered.weak_bg_fill
                        } else {
                            Color32::TRANSPARENT
                        };
                        let painter = ui.painter();
                        painter.rect_filled(rect, CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, fill);
                        painter.galley(egui::pos2(rect.left() + PAD_L, rect.center().y - galley.size().y / 2.0), galley, color);

                        // Sublime shows a dot for unsaved files and an × on hover.
                        let c = close_rect.center();
                        if close_resp.hovered() {
                            painter.circle_filled(c, CLOSE / 2.0, ui.visuals().widgets.hovered.bg_fill);
                        }
                        if t.buf.is_dirty() && !close_resp.hovered() {
                            painter.circle_filled(c, 4.0, color);
                        } else if active || resp.hovered() || close_resp.hovered() || t.buf.is_dirty() {
                            let r = 4.5;
                            let stroke =
                                egui::Stroke::new(1.6, if close_resp.hovered() { ui.visuals().strong_text_color() } else { color });
                            painter.line_segment([c + Vec2::new(-r, -r), c + Vec2::new(r, r)], stroke);
                            painter.line_segment([c + Vec2::new(-r, r), c + Vec2::new(r, -r)], stroke);
                        }

                        if close_resp.clicked() || resp.middle_clicked() {
                            actions.push(TabAction::Close(i));
                        } else if resp.clicked() {
                            actions.push(TabAction::Activate(i));
                        }
                        let resp = if close_resp.hovered() {
                            close_resp.on_hover_text("Close");
                            resp
                        } else if let Some(p) = &t.buf.path {
                            resp.on_hover_text(p.display().to_string())
                        } else {
                            resp
                        };
                        resp.context_menu(|ui| {
                            if ui.button("Close").clicked() {
                                actions.push(TabAction::Close(i));
                            }
                            if groups > 1 {
                                ui.separator();
                                for target in (0..groups).filter(|&x| x != g) {
                                    if ui.button(format!("Move to Group {}", target + 1)).clicked() {
                                        actions.push(TabAction::MoveTo(i, target));
                                    }
                                }
                            }
                        });
                    }
                });
            });
    }
}
