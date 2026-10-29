//! Hand-offs to the desktop: the file manager and a terminal.

use std::path::Path;
use std::process::{Child, Command, Stdio};

/// Show `path` in the system's file manager: its folder opens with the file
/// selected where the file manager supports that.
pub fn show_in_file_manager(path: &Path) -> std::io::Result<()> {
    if cfg!(target_os = "macos") {
        return detach(Command::new("open").arg("-R").arg(path).spawn());
    }
    if cfg!(windows) {
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path);
        return detach(Command::new("explorer").arg(arg).spawn());
    }
    // Nautilus, Dolphin, Nemo, Thunar… implement the FileManager1 D-Bus
    // interface, which selects the file. Otherwise just open the folder.
    // dbus-send splits array items on commas.
    let uri = crate::browser::file_url(path).replace(',', "%2C");
    let shown = host_command("dbus-send")
        .args(["--session", "--print-reply", "--reply-timeout=3000", "--dest=org.freedesktop.FileManager1"])
        .args(["/org/freedesktop/FileManager1", "org.freedesktop.FileManager1.ShowItems"])
        .arg(format!("array:string:{uri}"))
        .arg("string:")
        .output()
        .is_ok_and(|o| o.status.success());
    if shown {
        return Ok(());
    }
    let dir = path.parent().unwrap_or(path);
    detach(host_command("xdg-open").arg(dir).spawn())
}

/// Open the system's default terminal in `dir`.
pub fn open_terminal(dir: &Path) -> std::io::Result<()> {
    if cfg!(target_os = "macos") {
        return detach(Command::new("open").args(["-a", "Terminal"]).arg(dir).spawn());
    }
    if cfg!(windows) {
        // Windows Terminal when installed, else a classic console.
        if let Ok(c) = Command::new("wt").arg("-d").arg(dir).spawn() {
            return detach(Ok(c));
        }
        return detach(Command::new("cmd").args(["/C", "start", "", "cmd"]).current_dir(dir).spawn());
    }
    // There is no single "default terminal" on Linux: honour $TERMINAL and
    // xdg-terminal-exec, then the desktop's own terminal, then common ones.
    // The shell runs on the host, so this also works from the Flatpak. With
    // "probe" as second argument it only checks that a terminal exists.
    const SCRIPT: &str = r#"
dir=$1
cd "$dir" || exit 1
if [ "$2" = probe ]; then
    run() { command -v "$1" >/dev/null 2>&1 && exit 0; }
else
    run() { command -v "$1" >/dev/null 2>&1 && exec "$@"; }
fi
set -- "$dir"
[ -n "$TERMINAL" ] && run "$TERMINAL"
run xdg-terminal-exec
case "$XDG_CURRENT_DESKTOP" in
*KDE*) run konsole --workdir "$1" ;;
*XFCE*) run xfce4-terminal --working-directory="$1" ;;
*MATE*) run mate-terminal --working-directory="$1" ;;
*LXQt*) run qterminal --workdir "$1" ;;
esac
run ptyxis --new-window --working-directory="$1"
run kgx --working-directory="$1"
run gnome-terminal --working-directory="$1"
run konsole --workdir "$1"
run xfce4-terminal --working-directory="$1"
run x-terminal-emulator
for t in alacritty kitty foot wezterm tilix terminator urxvt xterm; do run "$t"; done
exit 127
"#;
    let script = || {
        let mut c = host_command("sh");
        c.args(["-c", SCRIPT, "sh"]).arg(dir).stdout(Stdio::null()).stderr(Stdio::null());
        c
    };
    if script().arg("probe").status()?.code() == Some(127) {
        return Err(std::io::Error::other("no terminal emulator found (set $TERMINAL)"));
    }
    detach(script().spawn())
}

/// A command that runs on the host, escaping the Flatpak sandbox if needed.
fn host_command(program: &str) -> Command {
    if std::env::var_os("FLATPAK_ID").is_some() {
        let mut c = Command::new("flatpak-spawn");
        c.args(["--host", program]);
        c
    } else {
        Command::new(program)
    }
}

/// Let a launched program run on its own; reap it when it exits.
fn detach(child: std::io::Result<Child>) -> std::io::Result<()> {
    let mut child = child?;
    std::thread::spawn(move || child.wait());
    Ok(())
}
