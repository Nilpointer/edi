# edi

A fast, GPU-drawn text editor in Rust, modelled on Sublime Text's
workflow: multiple cursors, Goto Anything, a command palette, minimap and
Sublime's default Linux key bindings.

It is a clean-room implementation: it uses only Sublime's publicly
documented behaviour and default key bindings, no Sublime code or assets.

```sh
cargo run --release -- [FILE|FOLDER]...
```

## Features

- **Multiple selections**: Ctrl+D adds the next occurrence, Ctrl+click adds a
  caret, Alt+Shift+↑/↓ adds a caret on the line above/below, Ctrl+Shift+L
  splits a selection into lines, middle-drag makes a column selection, and
  Alt+F3 or *Find All* selects every match.
- **Goto Anything** (Ctrl+P): fuzzy file search across the open folder
  (respects `.gitignore`). `file:42` jumps to a line, `:42` goes to a line
  live, and `@name` lists symbols. Ctrl+G and Ctrl+R open it prefixed.
- **Command palette** (Ctrl+Shift+P) with fuzzy matching and key hints.
- **Syntax highlighting** for 68 languages via syntect (which reads
  Sublime-format grammars), cached per line and re-highlighted incrementally
  within a per-frame time budget.
- **Color schemes**: Monokai by default plus the syntect themes, with live
  preview in *UI: Select Color Scheme*.
- **Minimap** with click/drag navigation, **side bar** folder tree, tabs,
  status bar.
- **Find / Replace panel** (Ctrl+F / Ctrl+H) with incremental search,
  outlined matches, case toggle, Replace All.
- Sublime-style editing: auto-pairing brackets/quotes, smart indent on Enter,
  whole-line copy/cut/paste with empty selections, toggle comment, duplicate,
  swap and delete lines, join, sort, case change, jump to bracket, and snapshot
  undo with typing coalescing.
- Unsaved-changes prompts on close/quit, drag-and-drop to open.

## Key bindings

| Keys | Command |
|---|---|
| Ctrl+P / Ctrl+Shift+P | Goto Anything / Command Palette |
| Ctrl+G / Ctrl+R | Goto line / symbol |
| Ctrl+D | Add next occurrence |
| Ctrl+L | Expand selection to line |
| Ctrl+Shift+L | Split selection into lines |
| Alt+Shift+↑ / ↓ | Add caret above / below |
| Ctrl+Shift+K / D | Delete / duplicate line |
| Ctrl+Shift+↑ / ↓ | Swap line up / down |
| Ctrl+/ | Toggle comment |
| Ctrl+Shift+J | Join lines |
| Ctrl+Enter / Ctrl+Shift+Enter | Insert line after / before |
| Ctrl+] / Ctrl+[ | Indent / unindent |
| Ctrl+M | Jump to matching bracket |
| F9 | Sort lines |
| Ctrl+K Ctrl+U / Ctrl+K Ctrl+L | Upper / lower case |
| Ctrl+K Ctrl+B | Toggle side bar |
| Ctrl+F / Ctrl+H, F3 / Shift+F3 | Find / replace, next / previous |
| Ctrl+= / Ctrl+- / Ctrl+0 | Font size |
| Ctrl+PageDown / PageUp | Next / previous tab |

## Layout

| File | Role |
|---|---|
| `src/buffer.rs` | Rope buffer (ropey), selections, every editing command, undo |
| `src/view.rs` | Custom-painted editor widget: text, carets, gutter, minimap, mouse/keyboard |
| `src/highlight.rs` | syntect integration and the incremental per-line highlight cache |
| `src/commands.rs` | Command list and keymap (including chords) |
| `src/fuzzy.rs` | Fuzzy scorer for Goto Anything and the palette |
| `src/app.rs` | Window shell: tabs, side bar, overlays, find panel, dialogs |

`cargo test` covers the buffer operations, keymap and fuzzy matcher.

For development, `EDI_RUN="Command Palette"`, `EDI_QUERY=text` and
`EDI_SCREENSHOT=out.ppm` run commands, fill the overlay and save a
screenshot before exiting. Keyboard input is ignored in that mode.

## Not yet implemented

Hot exit / session restore, split panes, word wrap, code folding,
Find in Files, regex search, plugins, and settings files.
