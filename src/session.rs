//! Hot exit: open tabs, unsaved text and layout are written to disk
//! periodically and on exit, and restored on the next start.
//!
//! The session file is replaced atomically (write a temp file, sync, rename),
//! so a crash mid-save leaves the previous session intact.

use ropey::Rope;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::JoinHandle;

const FILE: &str = "session.json";

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Session {
    pub tabs: Vec<TabState>,
    pub active: usize,
    pub root: Option<PathBuf>,
    pub sidebar: bool,
    pub minimap: bool,
    pub font_size: f32,
    pub theme: String,
    pub hot_exit: bool,
    pub find: String,
    pub find_case: bool,
}

impl Default for Session {
    fn default() -> Self {
        Session {
            tabs: Vec::new(),
            active: 0,
            root: None,
            sidebar: true,
            minimap: true,
            font_size: 14.0,
            theme: String::new(),
            hot_exit: true,
            find: String::new(),
            find_case: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct TabState {
    pub path: Option<PathBuf>,
    /// Buffer contents when they differ from the file on disk.
    pub unsaved: Option<String>,
    /// Selections as (anchor, head) char positions.
    pub sels: Vec<(usize, usize)>,
    pub scroll: [f32; 2],
    pub syntax: String,
}

/// A session plus the ropes of unsaved buffers. Ropes clone in O(1), so the
/// UI thread only snapshots them; text conversion happens on the writer thread.
pub struct Snapshot {
    pub session: Session,
    pub unsaved: Vec<Option<Rope>>,
}

/// Where the session lives. `EDI_DATA_DIR` overrides it (used by tests).
pub fn dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("EDI_DATA_DIR") {
        return Some(PathBuf::from(d));
    }
    let home = std::env::home_dir();
    if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library/Application Support/edi"))
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("edi"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.map(|h| h.join(".local/share")))
            .map(|p| p.join("edi"))
    }
}

pub fn load() -> Option<Session> {
    let text = std::fs::read_to_string(dir()?.join(FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write(snap: Snapshot) -> std::io::Result<()> {
    let Snapshot { mut session, unsaved } = snap;
    for (tab, rope) in session.tabs.iter_mut().zip(unsaved) {
        tab.unsaved = rope.map(|r| r.to_string());
    }
    let dir = dir().ok_or_else(|| std::io::Error::other("no data directory"))?;
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!("{FILE}.tmp"));
    {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        serde_json::to_writer(&mut f, &session)?;
        f.flush()?;
        f.get_ref().sync_all()?;
    }
    std::fs::rename(&tmp, dir.join(FILE))
}

/// Background writer: saves the newest snapshot it has been sent.
pub struct Writer {
    tx: Option<mpsc::Sender<Snapshot>>,
    thread: Option<JoinHandle<()>>,
}

impl Writer {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<Snapshot>();
        let thread = std::thread::spawn(move || {
            while let Ok(mut snap) = rx.recv() {
                // Skip snapshots that were superseded while we were busy.
                while let Ok(newer) = rx.try_recv() {
                    snap = newer;
                }
                if let Err(e) = write(snap) {
                    eprintln!("edi: failed to save session: {e}");
                }
            }
        });
        Writer { tx: Some(tx), thread: Some(thread) }
    }

    pub fn save(&self, snap: Snapshot) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(snap);
        }
    }

    /// Save one last snapshot and wait until it is on disk.
    pub fn finish(&mut self, snap: Snapshot) {
        self.save(snap);
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_through_writer() {
        let dir = std::env::temp_dir().join(format!("edi-session-test-{}", std::process::id()));
        // SAFETY: tests in this module are the only ones touching EDI_DATA_DIR.
        unsafe { std::env::set_var("EDI_DATA_DIR", &dir) };
        let session = Session {
            tabs: vec![TabState { path: None, sels: vec![(1, 3)], ..Default::default() }],
            active: 0,
            font_size: 16.0,
            ..Default::default()
        };
        let mut w = Writer::new();
        w.finish(Snapshot { session, unsaved: vec![Some(Rope::from_str("hello\n"))] });
        let back = load().expect("session written");
        assert_eq!(back.tabs[0].unsaved.as_deref(), Some("hello\n"));
        assert_eq!(back.tabs[0].sels, vec![(1, 3)]);
        assert_eq!(back.font_size, 16.0);
        assert!(!dir.join("session.json.tmp").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
