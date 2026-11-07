//! Watches the open folder for changes made outside Edi, so the sidebar,
//! Goto Anything and open tabs stay current.

use eframe::egui;
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};

/// Entries the sidebar leaves out. Their folders aren't watched either:
/// build output and dependencies change constantly and can be huge.
pub fn skipped(name: &str) -> bool {
    matches!(name, ".git" | ".DS_Store" | "target" | "node_modules")
}

fn is_skipped(root: &Path, p: &Path) -> bool {
    p.strip_prefix(root).is_ok_and(|r| r.components().any(|c| c.as_os_str().to_str().is_some_and(skipped)))
}

/// What changed on disk since the last poll.
#[derive(Default)]
pub struct Changes {
    /// Files or folders that appeared, disappeared or were renamed.
    pub moved: HashSet<PathBuf>,
    /// Files whose contents may have changed, including moved ones.
    pub touched: HashSet<PathBuf>,
    /// The watcher lost events; everything should be re-read.
    pub rescan: bool,
    /// Some folders could not be watched. Reported once.
    pub incomplete: bool,
}

type Shared = Arc<Mutex<notify::RecommendedWatcher>>;

pub struct Watch {
    rx: mpsc::Receiver<notify::Event>,
    /// Folders to add watches for, handled off the UI thread (Linux only).
    add_tx: Option<mpsc::Sender<PathBuf>>,
    incomplete: Arc<AtomicBool>,
    reported: bool,
    _watcher: Shared,
}

impl Watch {
    /// Start watching `root`. Events wake the UI through `ctx`.
    pub fn new(root: &Path, ctx: egui::Context) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let r = root.to_path_buf();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(mut ev) = res else { return };
            // Reading a file reports access events; they never change anything.
            if matches!(ev.kind, EventKind::Access(_)) {
                return;
            }
            // Drop build output etc. here so it doesn't wake the UI.
            ev.paths.retain(|p| !is_skipped(&r, p));
            if (!ev.paths.is_empty() || ev.need_rescan()) && tx.send(ev).is_ok() {
                ctx.request_repaint();
            }
        })?;
        let incomplete = Arc::new(AtomicBool::new(false));
        let mut add_tx = None;
        if cfg!(target_os = "linux") {
            // inotify watches folder by folder anyway; doing it ourselves
            // lets us leave out skipped and gitignored folders. Walking a
            // big tree takes a while, so a thread does it.
            watcher.watch(root, RecursiveMode::NonRecursive)?;
            let watcher: Shared = Arc::new(Mutex::new(watcher));
            let (atx, arx) = mpsc::channel::<PathBuf>();
            let (w, inc, top) = (watcher.clone(), incomplete.clone(), root.to_path_buf());
            std::thread::spawn(move || {
                let mut count = 0;
                for dir in arx {
                    // The root itself is watched already.
                    add_tree(&w, &dir, dir != top, &mut count, &inc);
                }
            });
            let _ = atx.send(root.to_path_buf());
            add_tx = Some(atx);
            return Ok(Watch { rx, add_tx, incomplete, reported: false, _watcher: watcher });
        }
        watcher.watch(root, RecursiveMode::Recursive)?;
        Ok(Watch { rx, add_tx, incomplete, reported: false, _watcher: Arc::new(Mutex::new(watcher)) })
    }

    pub fn poll(&mut self) -> Changes {
        let mut c = Changes::default();
        while let Ok(ev) = self.rx.try_recv() {
            c.rescan |= ev.need_rescan();
            // Plain writes are content changes. (Windows reports them as
            // Modify(Any), so that one doesn't count as a move.)
            let structural = matches!(
                ev.kind,
                EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_)) | EventKind::Any | EventKind::Other
            );
            for p in ev.paths {
                if structural {
                    // New or moved-in folders need watches of their own.
                    // Symlinks aren't followed: they may point anywhere.
                    if let Some(tx) = &self.add_tx
                        && std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir())
                    {
                        let _ = tx.send(p.clone());
                    }
                    c.moved.insert(p.clone());
                }
                c.touched.insert(p);
            }
        }
        if !self.reported && self.incomplete.load(Ordering::Relaxed) {
            self.reported = true;
            c.incomplete = true;
        }
        c
    }
}

/// Watch `dir` (unless `include_self` is false) and every folder below it,
/// without following symlinks and leaving out skipped and gitignored ones.
fn add_tree(w: &Mutex<notify::RecommendedWatcher>, dir: &Path, include_self: bool, count: &mut usize, incomplete: &AtomicBool) {
    const MAX_DIRS: usize = 50_000;
    let walk = ignore::WalkBuilder::new(dir)
        .hidden(false)
        .filter_entry(|e| !e.file_name().to_str().is_some_and(skipped))
        .build();
    for e in walk.flatten() {
        if !e.file_type().is_some_and(|t| t.is_dir()) || (e.depth() == 0 && !include_self) {
            continue;
        }
        if *count >= MAX_DIRS {
            incomplete.store(true, Ordering::Relaxed);
            return;
        }
        match w.lock().unwrap().watch(e.path(), RecursiveMode::NonRecursive) {
            Ok(()) => *count += 1,
            // Already gone again, e.g. a temporary folder.
            Err(err) if matches!(err.kind, notify::ErrorKind::PathNotFound) => {}
            Err(_) => incomplete.store(true, Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Poll until `done` holds for the accumulated changes, or time out.
    fn wait(w: &mut Watch, done: impl Fn(&Changes) -> bool) -> Changes {
        let mut all = Changes::default();
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            let c = w.poll();
            all.moved.extend(c.moved);
            all.touched.extend(c.touched);
            if done(&all) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        all
    }

    #[test]
    fn reports_outside_changes() {
        let root = std::env::temp_dir().join(format!("edi-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("sub/a.txt"), "a").unwrap();
        let root = root.canonicalize().unwrap();
        let mut w = Watch::new(&root, egui::Context::default()).unwrap();
        // Let the background thread add the nested watches.
        std::thread::sleep(Duration::from_millis(200));

        // Changes in a nested folder, a new folder, and a skipped folder.
        std::fs::write(root.join("sub/a.txt"), "changed").unwrap();
        std::fs::write(root.join("target/out.o"), "x").unwrap();
        std::fs::create_dir(root.join("new")).unwrap();
        let a = root.join("sub/a.txt");
        let c = wait(&mut w, |c| c.touched.contains(&a) && c.moved.contains(&root.join("new")));
        assert!(c.touched.contains(&a));
        assert!(!c.moved.contains(&a), "a content change is not a move");
        assert!(c.moved.contains(&root.join("new")));
        assert!(!c.touched.iter().any(|p| p.starts_with(root.join("target"))));

        // Files in a folder created after watching started are seen too.
        std::thread::sleep(Duration::from_millis(200));
        let b = root.join("new/b.txt");
        std::fs::write(&b, "b").unwrap();
        std::fs::remove_file(&a).unwrap();
        let c = wait(&mut w, |c| c.moved.contains(&b) && c.moved.contains(&a));
        assert!(c.moved.contains(&b), "new file in new folder");
        assert!(c.moved.contains(&a), "deleted file");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
