//! The main menu bar, laid out like Sublime Text's.

use super::EdiApp;
use crate::commands::{Cmd, Keymap};
use eframe::egui::{self, AtomExt, Button, Ui};

/// What a menu click asks for; applied after the menu is drawn so the menu
/// closures only need shared access to the app.
enum Pick {
    Cmd(Cmd),
    Theme(usize),
    Syntax(String),
}

/// Adds menu items that report their command into `pick`.
struct Menu<'a> {
    pick: &'a mut Option<Pick>,
}

impl Menu<'_> {
    fn item(&mut self, ui: &mut Ui, label: &str, cmd: Cmd) {
        self.item_if(ui, true, label, cmd);
    }

    fn item_if(&mut self, ui: &mut Ui, enabled: bool, label: &str, cmd: Cmd) {
        let button = Button::new(label).shortcut_text(Keymap::label(cmd));
        if ui.add_enabled(enabled, button).clicked() {
            *self.pick = Some(Pick::Cmd(cmd));
        }
    }

    fn check(&mut self, ui: &mut Ui, on: bool, label: &str, cmd: Cmd) {
        let button = Button::new(checked(on, label)).shortcut_text(Keymap::label(cmd));
        if ui.add(button).clicked() {
            *self.pick = Some(Pick::Cmd(cmd));
        }
    }
}

/// A label with a leading check mark in a fixed-width slot, so checked and
/// unchecked items stay aligned.
fn checked<'a>(on: bool, label: &str) -> (egui::Atom<'a>, String) {
    let mark = if on { "✔" } else { "" };
    (mark.atom_size(egui::vec2(18.0, 0.0)), label.to_string())
}

impl EdiApp {
    pub(super) fn menu_ui(&mut self, ui: &mut Ui) {
        let mut pick = None;
        let tab = self.tabs.get(self.active);
        let can_undo = tab.is_some_and(|t| t.buf.can_undo());
        let can_redo = tab.is_some_and(|t| t.buf.can_redo());
        let has_sel = tab.is_some_and(|t| t.buf.sels.iter().any(|s| !s.is_empty()));
        let syntax = tab.map(|t| t.hl.syntax.clone()).unwrap_or_default();

        egui::MenuBar::new().ui(ui, |ui| {
            let mut m = Menu { pick: &mut pick };

            ui.menu_button("File", |ui| {
                ui.set_min_width(210.0);
                m.item(ui, "New File", Cmd::NewFile);
                ui.separator();
                m.item(ui, "Open File…", Cmd::OpenFile);
                m.item(ui, "Open Folder…", Cmd::OpenFolder);
                ui.separator();
                m.item(ui, "Save", Cmd::Save);
                m.item(ui, "Save As…", Cmd::SaveAs);
                m.item(ui, "Save All", Cmd::SaveAll);
                ui.separator();
                m.item(ui, "Close File", Cmd::CloseTab);
                m.item(ui, "Close All Files", Cmd::CloseAll);
                ui.separator();
                m.item(ui, "Exit", Cmd::Quit);
            });

            ui.menu_button("Edit", |ui| {
                ui.set_min_width(210.0);
                m.item_if(ui, can_undo, "Undo", Cmd::Undo);
                m.item_if(ui, can_redo, "Redo", Cmd::Redo);
                ui.separator();
                // With no selection Sublime cuts/copies the whole line.
                m.item(ui, "Cut", Cmd::Cut);
                m.item(ui, "Copy", Cmd::Copy);
                m.item(ui, "Paste", Cmd::Paste);
                ui.separator();
                ui.menu_button("Line", |ui| {
                    ui.set_min_width(210.0);
                    m.item(ui, "Indent", Cmd::Indent);
                    m.item(ui, "Unindent", Cmd::Unindent);
                    ui.separator();
                    m.item(ui, "Swap Line Up", Cmd::SwapLineUp);
                    m.item(ui, "Swap Line Down", Cmd::SwapLineDown);
                    m.item(ui, "Duplicate Line", Cmd::DuplicateLine);
                    m.item(ui, "Delete Line", Cmd::DeleteLine);
                    m.item(ui, "Join Lines", Cmd::JoinLines);
                });
                ui.menu_button("Comment", |ui| {
                    ui.set_min_width(210.0);
                    m.item(ui, "Toggle Comment", Cmd::ToggleComment);
                });
                ui.menu_button("Text", |ui| {
                    ui.set_min_width(210.0);
                    m.item(ui, "Insert Line Before", Cmd::InsertLineBefore);
                    m.item(ui, "Insert Line After", Cmd::InsertLineAfter);
                });
                ui.menu_button("Convert Case", |ui| {
                    ui.set_min_width(210.0);
                    m.item_if(ui, has_sel, "Upper Case", Cmd::UpperCase);
                    m.item_if(ui, has_sel, "Lower Case", Cmd::LowerCase);
                });
                ui.separator();
                m.item(ui, "Sort Lines", Cmd::SortLines);
            });

            ui.menu_button("Selection", |ui| {
                ui.set_min_width(210.0);
                m.item(ui, "Split into Lines", Cmd::SplitSelectionIntoLines);
                m.item(ui, "Add Previous Line", Cmd::AddCursorAbove);
                m.item(ui, "Add Next Line", Cmd::AddCursorBelow);
                m.item(ui, "Single Selection", Cmd::SingleSelection);
                ui.separator();
                m.item(ui, "Select All", Cmd::SelectAll);
                m.item(ui, "Expand Selection to Line", Cmd::SelectLine);
            });

            ui.menu_button("Find", |ui| {
                ui.set_min_width(210.0);
                m.item(ui, "Find…", Cmd::Find);
                m.item(ui, "Find Next", Cmd::FindNext);
                m.item(ui, "Find Previous", Cmd::FindPrev);
                ui.separator();
                m.item(ui, "Replace…", Cmd::Replace);
                ui.separator();
                m.item(ui, "Quick Add Next", Cmd::FindUnderExpand);
                m.item(ui, "Quick Find All", Cmd::FindAllUnder);
            });

            ui.menu_button("View", |ui| {
                ui.set_min_width(210.0);
                m.check(ui, self.show_sidebar, "Side Bar", Cmd::ToggleSidebar);
                m.check(ui, self.show_minimap, "Minimap", Cmd::ToggleMinimap);
                ui.separator();
                ui.menu_button("Syntax", |ui| {
                    ui.set_min_width(210.0);
                    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                        for name in self.syn.syntax_names() {
                            if ui.radio(name == syntax, &name).clicked() {
                                *m.pick = Some(Pick::Syntax(name));
                            }
                        }
                    });
                });
                ui.menu_button("Font Size", |ui| {
                    ui.set_min_width(210.0);
                    m.item(ui, "Larger", Cmd::ZoomIn);
                    m.item(ui, "Smaller", Cmd::ZoomOut);
                    ui.separator();
                    m.item(ui, "Reset", Cmd::ZoomReset);
                });
            });

            ui.menu_button("Goto", |ui| {
                ui.set_min_width(210.0);
                m.item(ui, "Goto Anything…", Cmd::GotoAnything);
                ui.separator();
                m.item(ui, "Goto Symbol…", Cmd::GotoSymbol);
                m.item(ui, "Goto Line…", Cmd::GotoLine);
                ui.separator();
                m.item(ui, "Jump to Matching Bracket", Cmd::GotoBracket);
                ui.separator();
                ui.menu_button("Switch File", |ui| {
                    ui.set_min_width(210.0);
                    m.item(ui, "Next File", Cmd::NextTab);
                    m.item(ui, "Previous File", Cmd::PrevTab);
                    ui.separator();
                    for (i, t) in self.tabs.iter().enumerate().take(9) {
                        let button = Button::new(checked(i == self.active, &t.buf.name()))
                            .shortcut_text(Keymap::label(Cmd::SwitchTab(i as u8)));
                        if ui.add(button).clicked() {
                            *m.pick = Some(Pick::Cmd(Cmd::SwitchTab(i as u8)));
                        }
                    }
                });
            });

            ui.menu_button("Tools", |ui| {
                ui.set_min_width(210.0);
                m.item(ui, "Command Palette…", Cmd::CommandPalette);
            });

            ui.menu_button("Preferences", |ui| {
                ui.set_min_width(210.0);
                ui.menu_button("Color Scheme", |ui| {
                    ui.set_min_width(210.0);
                    for (i, (name, _)) in self.syn.themes.iter().enumerate() {
                        if ui.radio(i == self.syn.theme, name).clicked() {
                            *m.pick = Some(Pick::Theme(i));
                        }
                    }
                });
                m.item(ui, "Select Color Scheme…", Cmd::SelectTheme);
                ui.separator();
                m.check(ui, self.hot_exit, "Hot Exit", Cmd::ToggleHotExit);
                ui.separator();
                ui.menu_button("Font Size", |ui| {
                    ui.set_min_width(210.0);
                    m.item(ui, "Larger", Cmd::ZoomIn);
                    m.item(ui, "Smaller", Cmd::ZoomOut);
                    ui.separator();
                    m.item(ui, "Reset", Cmd::ZoomReset);
                });
            });

            ui.menu_button("Help", |ui| {
                ui.set_min_width(210.0);
                m.item(ui, "About edi", Cmd::About);
            });
        });

        let ctx = ui.ctx().clone();
        match pick {
            Some(Pick::Cmd(cmd)) => self.run(cmd, &ctx),
            Some(Pick::Theme(i)) => {
                self.syn.theme = i;
                self.apply_visuals(&ctx);
                self.focus_editor = true;
            }
            Some(Pick::Syntax(name)) => {
                if let Some(t) = self.tabs.get_mut(self.active) {
                    t.hl.set_syntax(&name);
                }
                self.focus_editor = true;
            }
            None => {}
        }
    }
}
