//! Command registry and keymap. Bindings follow Sublime Text's Linux defaults.

use eframe::egui::{Key, Modifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    NewFile,
    OpenFile,
    OpenFolder,
    Save,
    SaveAs,
    SaveAll,
    CloseTab,
    NextTab,
    PrevTab,
    Quit,
    CommandPalette,
    GotoAnything,
    GotoLine,
    GotoSymbol,
    Find,
    Replace,
    FindNext,
    FindPrev,
    FindAllUnder,
    ToggleSidebar,
    ToggleMinimap,
    SelectTheme,
    SetSyntax,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Undo,
    Redo,
    SelectAll,
    SelectLine,
    FindUnderExpand,
    SplitSelectionIntoLines,
    SingleSelection,
    AddCursorAbove,
    AddCursorBelow,
    DuplicateLine,
    DeleteLine,
    SwapLineUp,
    SwapLineDown,
    ToggleComment,
    JoinLines,
    InsertLineAfter,
    InsertLineBefore,
    Indent,
    Unindent,
    SortLines,
    UpperCase,
    LowerCase,
    GotoBracket,
    Cut,
    Copy,
    Paste,
    CloseAll,
    About,
    /// Alt+1…9: switch to the n-th tab (0-based).
    SwitchTab(u8),
}

/// (command, palette caption, default keys). Chords are space-separated.
pub const COMMANDS: &[(Cmd, &str, &str)] = &[
    (Cmd::NewFile, "File: New File", "ctrl+n"),
    (Cmd::OpenFile, "File: Open File…", "ctrl+o"),
    (Cmd::OpenFolder, "File: Open Folder…", ""),
    (Cmd::Save, "File: Save", "ctrl+s"),
    (Cmd::SaveAs, "File: Save As…", "ctrl+shift+s"),
    (Cmd::SaveAll, "File: Save All", ""),
    (Cmd::CloseTab, "File: Close", "ctrl+w"),
    (Cmd::NextTab, "View: Next Tab", "ctrl+pagedown"),
    (Cmd::PrevTab, "View: Previous Tab", "ctrl+pageup"),
    (Cmd::Quit, "File: Quit", "ctrl+q"),
    (Cmd::CommandPalette, "Command Palette", "ctrl+shift+p"),
    (Cmd::GotoAnything, "Goto Anything", "ctrl+p"),
    (Cmd::GotoLine, "Goto Line…", "ctrl+g"),
    (Cmd::GotoSymbol, "Goto Symbol…", "ctrl+r"),
    (Cmd::Find, "Find…", "ctrl+f"),
    (Cmd::Replace, "Replace…", "ctrl+h"),
    (Cmd::FindNext, "Find Next", "f3"),
    (Cmd::FindPrev, "Find Previous", "shift+f3"),
    (Cmd::FindAllUnder, "Quick Find All", "alt+f3"),
    (Cmd::ToggleSidebar, "View: Toggle Side Bar", "ctrl+k ctrl+b"),
    (Cmd::ToggleMinimap, "View: Toggle Minimap", ""),
    (Cmd::SelectTheme, "UI: Select Color Scheme", ""),
    (Cmd::SetSyntax, "Set Syntax…", ""),
    (Cmd::ZoomIn, "View: Font Size Increase", "ctrl+="),
    (Cmd::ZoomOut, "View: Font Size Decrease", "ctrl+-"),
    (Cmd::ZoomReset, "View: Font Size Reset", "ctrl+0"),
    (Cmd::Undo, "Edit: Undo", "ctrl+z"),
    (Cmd::Redo, "Edit: Redo", "ctrl+shift+z"),
    (Cmd::Redo, "Edit: Redo", "ctrl+y"),
    (Cmd::SelectAll, "Selection: Select All", "ctrl+a"),
    (Cmd::SelectLine, "Selection: Expand to Line", "ctrl+l"),
    (Cmd::FindUnderExpand, "Selection: Add Next Occurrence", "ctrl+d"),
    (Cmd::SplitSelectionIntoLines, "Selection: Split into Lines", "ctrl+shift+l"),
    (Cmd::SingleSelection, "Selection: Single Selection", ""),
    (Cmd::AddCursorAbove, "Selection: Add Previous Line", "alt+shift+up"),
    (Cmd::AddCursorBelow, "Selection: Add Next Line", "alt+shift+down"),
    (Cmd::DuplicateLine, "Edit: Duplicate Line", "ctrl+shift+d"),
    (Cmd::DeleteLine, "Edit: Delete Line", "ctrl+shift+k"),
    (Cmd::SwapLineUp, "Edit: Swap Line Up", "ctrl+shift+up"),
    (Cmd::SwapLineDown, "Edit: Swap Line Down", "ctrl+shift+down"),
    (Cmd::ToggleComment, "Edit: Toggle Comment", "ctrl+/"),
    (Cmd::JoinLines, "Edit: Join Lines", "ctrl+shift+j"),
    (Cmd::InsertLineAfter, "Edit: Insert Line After", "ctrl+enter"),
    (Cmd::InsertLineBefore, "Edit: Insert Line Before", "ctrl+shift+enter"),
    (Cmd::Indent, "Edit: Indent", "ctrl+]"),
    (Cmd::Unindent, "Edit: Unindent", "ctrl+["),
    (Cmd::SortLines, "Edit: Sort Lines", "f9"),
    (Cmd::UpperCase, "Edit: Convert to Upper Case", "ctrl+k ctrl+u"),
    (Cmd::LowerCase, "Edit: Convert to Lower Case", "ctrl+k ctrl+l"),
    (Cmd::GotoBracket, "Goto: Jump to Matching Bracket", "ctrl+m"),
    // Clipboard keys are delivered by egui as Cut/Copy/Paste events, so these
    // have no binding here; `label` still shows the usual shortcut.
    (Cmd::Cut, "Edit: Cut", ""),
    (Cmd::Copy, "Edit: Copy", ""),
    (Cmd::Paste, "Edit: Paste", ""),
    (Cmd::CloseAll, "File: Close All", ""),
    (Cmd::About, "Help: About edi", ""),
    (Cmd::SwitchTab(0), "Switch to Tab 1", "alt+1"),
    (Cmd::SwitchTab(1), "Switch to Tab 2", "alt+2"),
    (Cmd::SwitchTab(2), "Switch to Tab 3", "alt+3"),
    (Cmd::SwitchTab(3), "Switch to Tab 4", "alt+4"),
    (Cmd::SwitchTab(4), "Switch to Tab 5", "alt+5"),
    (Cmd::SwitchTab(5), "Switch to Tab 6", "alt+6"),
    (Cmd::SwitchTab(6), "Switch to Tab 7", "alt+7"),
    (Cmd::SwitchTab(7), "Switch to Tab 8", "alt+8"),
    (Cmd::SwitchTab(8), "Switch to Tab 9", "alt+9"),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stroke {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Key,
}

impl Stroke {
    pub fn from_event(key: Key, m: Modifiers) -> Self {
        Stroke { ctrl: m.ctrl || m.command, shift: m.shift, alt: m.alt, key }
    }

    fn parse(s: &str) -> Option<Self> {
        let mut st = Stroke { ctrl: false, shift: false, alt: false, key: Key::Escape };
        let mut key = None;
        for part in s.split('+') {
            match part {
                "ctrl" => st.ctrl = true,
                "shift" => st.shift = true,
                "alt" => st.alt = true,
                "pageup" => key = Some(Key::PageUp),
                "pagedown" => key = Some(Key::PageDown),
                k => {
                    let mut c = k.chars();
                    let cap = c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
                    key = Key::from_name(k).or_else(|| Key::from_name(&cap));
                }
            }
        }
        st.key = key?;
        Some(st)
    }
}

pub struct Keymap {
    bindings: Vec<(Vec<Stroke>, Cmd)>,
    pending: Option<Stroke>,
}

pub enum KeyResult {
    Cmd(Cmd),
    /// First stroke of a chord was consumed; waiting for the next.
    Pending,
    None,
}

impl Keymap {
    pub fn new() -> Self {
        let bindings = COMMANDS
            .iter()
            .filter(|(_, _, k)| !k.is_empty())
            .filter_map(|(c, _, k)| {
                let strokes: Option<Vec<Stroke>> = k.split(' ').map(Stroke::parse).collect();
                strokes.map(|s| (s, *c))
            })
            .collect();
        Keymap { bindings, pending: None }
    }

    pub fn press(&mut self, stroke: Stroke) -> KeyResult {
        if let Some(first) = self.pending.take() {
            return match self.bindings.iter().find(|(s, _)| s.len() == 2 && s[0] == first && s[1] == stroke) {
                Some((_, c)) => KeyResult::Cmd(*c),
                None => KeyResult::None,
            };
        }
        if self.bindings.iter().any(|(s, _)| s.len() == 2 && s[0] == stroke) {
            self.pending = Some(stroke);
            return KeyResult::Pending;
        }
        match self.bindings.iter().find(|(s, _)| s.len() == 1 && s[0] == stroke) {
            Some((_, c)) => KeyResult::Cmd(*c),
            None => KeyResult::None,
        }
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Human-readable binding for the palette.
    pub fn label(cmd: Cmd) -> String {
        match cmd {
            Cmd::Cut => return "Ctrl+X".into(),
            Cmd::Copy => return "Ctrl+C".into(),
            Cmd::Paste => return "Ctrl+V".into(),
            _ => {}
        }
        COMMANDS
            .iter()
            .find(|(c, _, k)| *c == cmd && !k.is_empty())
            .map(|(_, _, k)| {
                k.split(' ')
                    .map(|s| s.split('+').map(|p| {
                        let mut c = p.chars();
                        match c.next() {
                            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                            None => "+".into(),
                        }
                    }).collect::<Vec<_>>().join("+"))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_bindings() {
        let n = COMMANDS.iter().filter(|c| !c.2.is_empty()).count();
        assert_eq!(Keymap::new().bindings.len(), n);
    }

    #[test]
    fn chord() {
        let mut k = Keymap::new();
        let ctrl = |key| Stroke { ctrl: true, shift: false, alt: false, key };
        assert!(matches!(k.press(ctrl(Key::K)), KeyResult::Pending));
        assert!(matches!(k.press(ctrl(Key::B)), KeyResult::Cmd(Cmd::ToggleSidebar)));
    }
}
