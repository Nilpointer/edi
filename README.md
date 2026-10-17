# edi

A fast, GPU-drawn text editor in Rust, modelled on Sublime Text's
workflow: multiple cursors, Goto Anything, a command palette, minimap and
Sublime's default Linux key bindings.

It is a clean-room implementation: it uses only Sublime's publicly
documented behaviour and default key bindings, no Sublime code or assets.

```sh
cargo run --release -- [FILE|FOLDER]...
```

## Download

[Releases](https://github.com/Nilpointer/edi/releases) provide:

| Platform | File | Notes |
|---|---|---|
| Linux | `Edi-<v>-x86_64.AppImage` | Run directly, or `--install` it (below). glibc 2.35+ |
| Linux | `Edi-<v>-x86_64.flatpak` | `flatpak install --user Edi-<v>-x86_64.flatpak` |
| Linux | `edi-<v>-linux-x86_64.tar.gz` | Plain binary |
| macOS | `Edi-<v>-macos-universal.dmg` / `.zip` | Apple Silicon + Intel. Not notarized: right-click → Open the first time |
| Windows | `edi-<v>-windows-x86_64.zip` / `.exe` | Portable, no installer. SmartScreen may warn (unsigned) |

Releases are built by `.github/workflows/release.yml` when a `v*` tag is pushed.

## Install (AppImage)

```sh
./packaging/build-appimage.sh          # → target/appimage/Edi-<version>-x86_64.AppImage
./target/appimage/Edi-0.1.0-x86_64.AppImage --install
```

`--install` copies the AppImage to `~/.local/share/edi/`, adds Edi to the
application menu with its icon, and links `~/.local/bin/edi`. `--uninstall`
removes all of that. Run the AppImage without arguments (or with files and
folders) to use it without installing.

The build script compiles inside an Ubuntu 22.04 container (podman or
docker), so the AppImage runs on any x86_64 distro with glibc 2.35 or newer
(Ubuntu 22.04+, Debian 12+, Fedora 36+). It downloads pinned, SHA-256-checked
releases of `appimagetool` and the AppImage runtime.

## Features

- **Multiple selections**: Ctrl+D adds the next occurrence, Ctrl+click adds a
  caret, Alt+Shift+↑/↓ adds a caret on the line above/below, Ctrl+Shift+L
  splits a selection into lines, middle-drag makes a column selection, and
  Alt+F3 or *Find All* selects every match.
- **Goto Anything** (Ctrl+P): fuzzy file search across the open folder
  (respects `.gitignore`). `file:42` jumps to a line, `:42` goes to a line
  live, and `@name` lists symbols. Ctrl+G and Ctrl+R open it prefixed.
- **Command palette** (Ctrl+Shift+P) with fuzzy matching and key hints.
- **Menu bar** laid out like Sublime's (File, Edit, Selection, Find, View,
  Goto, Tools, Preferences, Help) with shortcuts shown, check marks for
  toggles, and Syntax / Color Scheme / Switch File submenus.
- **Syntax highlighting** for 68 languages via syntect (which reads
  Sublime-format grammars), cached per line and re-highlighted incrementally
  within a per-frame time budget.
- **Color schemes**: Monokai by default plus the syntect themes, with live
  preview in *UI: Select Color Scheme*.
- **Minimap** with click/drag navigation, **side bar** folder tree, tabs,
  status bar.
- **Find / Replace panel** (Ctrl+F / Ctrl+H) with incremental search,
  outlined matches and Replace All. Toggles for regular expressions (Alt+R),
  case (Alt+C) and whole word (Alt+W); regex replacements expand `$1` /
  `${name}`, and invalid patterns show their error.
- **Find in Files** (Ctrl+Shift+F): searches the open folder in the
  background (respecting `.gitignore`, skipping binary files). *Where* takes
  folders, include filters (`*.rs`), exclude filters (`-*.lock`) and
  `<open files>` (unsaved edits count). Results open in a *Find Results* tab;
  double-click a line or press F4 / Shift+F4 to jump between matches.
- **Split panes**: Sublime-style groups, each with its own tabs — layouts
  for 1–4 columns, 2–3 rows or a 2×2 grid (Alt+Shift+1/2/3/4/8/9/5). Ctrl+1–4
  focuses a group, Ctrl+Shift+1–4 moves the current file there; right-click
  a tab to move or close it.
- **Word wrap** (Alt+Z or *View › Word Wrap*): wraps at word boundaries,
  and ↑/↓/PageUp/PageDown move by screen rows.
- **Code folding** by indentation: click the gutter arrows, Ctrl+Shift+[ /
  Ctrl+Shift+] to fold / unfold at the caret, Ctrl+K Ctrl+1 to fold all,
  Ctrl+K Ctrl+J to unfold all. Folded lines show a clickable `…`; folds move
  with edits, and a caret landing inside a fold opens it.
- Sublime-style editing: auto-pairing brackets/quotes, smart indent on Enter,
  whole-line copy/cut/paste with empty selections, toggle comment, duplicate,
  swap and delete lines, join, sort, case change, jump to bracket, and snapshot
  undo with typing coalescing.
- **Hot exit / session restore**: quitting never asks about unsaved files.
  Open tabs, unsaved text, selections, scroll positions, the open folder and
  view settings come back on the next start. The session is also autosaved
  every couple of seconds while you work, and written atomically, so unsaved
  work survives crashes. Toggle it under *Preferences › Hot Exit*; when off,
  quitting asks about unsaved files again. Stored in `~/.local/share/edi/`
  (Linux), `~/Library/Application Support/edi/` (macOS) or `%APPDATA%\edi\`
  (Windows).
- **Single instance**: only one Edi runs. Opening files from the file
  manager ("Open with"), the app menu or `edi file.txt` in a terminal hands
  them to the running window, which opens them and comes to the front. Use
  `edi -n` / `--new-instance` for a separate window (it doesn't touch the
  hot-exit session).
- Unsaved-changes prompt when closing a modified tab, drag-and-drop to open.

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
| Ctrl+PageDown / PageUp | Next / previous tab (in the focused group) |
| Ctrl+Shift+F, F4 / Shift+F4 | Find in Files, next / previous result |
| Alt+R / Alt+C / Alt+W | Find: regex / case / whole word |
| Alt+Z | Word wrap |
| Ctrl+Shift+[ / Ctrl+Shift+] | Fold / unfold |
| Ctrl+K Ctrl+1 / Ctrl+K Ctrl+J | Fold all / unfold all |
| Alt+Shift+1/2/3/4, 8/9, 5 | Layout: single, 2–4 columns, 2–3 rows, grid |
| Ctrl+1–4 / Ctrl+Shift+1–4 | Focus group / move file to group |
| Alt+1 … Alt+9 | Switch to tab 1–8 / last tab |

## Layout

| File | Role |
|---|---|
| `src/buffer.rs` | Rope buffer (ropey), selections, every editing command, undo |
| `src/view.rs` | Custom-painted editor widget: text, carets, gutter, minimap, mouse/keyboard |
| `src/highlight.rs` | syntect integration and the incremental per-line highlight cache |
| `src/commands.rs` | Command list and keymap (including chords) |
| `src/fuzzy.rs` | Fuzzy scorer for Goto Anything and the palette |
| `src/app.rs` | Window shell: tabs, side bar, overlays, find panel, dialogs |
| `src/app/menu.rs` | Menu bar |
| `src/app/groups.rs` | Split panes: layouts, per-group tabs and views |
| `src/display.rs` | Line ↔ screen-row mapping for word wrap and folding |
| `src/search.rs` | Search options, regex compilation, replacement expansion |
| `src/find_files.rs` | Find in Files: background search and the results buffer |
| `src/session.rs` | Hot exit: session format, atomic background writer |
| `src/instance.rs` | Single instance: lock file plus local socket, forwards files to the primary |
| `assets/edi.svg` | App icon (PNG sizes in `assets/icons/`) |
| `packaging/` | AppImage build script and `AppRun`, Flatpak manifest, macOS `Info.plist`, desktop entry |
| `build.rs` | Embeds the icon into `edi.exe` on Windows |

`cargo test` covers the buffer operations, keymap and fuzzy matcher.

For development, `EDI_RUN="Command Palette"`, `EDI_QUERY=text` and
`EDI_SCREENSHOT=out.ppm` run commands, fill the overlay and save a
screenshot before exiting. `EDI_POINTER="c:25,12;m:80,170"` injects clicks
(`c`) and pointer moves (`m`), e.g. to open menus. Keyboard input is ignored
in that mode.

## Not yet implemented

Replace in Files, plugins, and settings files.

## License

MIT, see [LICENSE](LICENSE).
