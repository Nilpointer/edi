//! Single instance: the first Edi becomes the primary and listens on a local
//! socket; later launches hand their files to it and exit.
//!
//! The primary holds an exclusive lock on `instance.lock` for its lifetime.
//! The OS drops that lock when the process ends, even on a crash, so a stale
//! socket can never block startup, and two simultaneous launches can't both
//! become primary.

use eframe::egui;
use interprocess::local_socket::{prelude::*, ListenerOptions, Name, Stream};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// What a secondary launch sends to the primary.
#[derive(Serialize, Deserialize, Default)]
pub struct Request {
    /// Absolute paths of files and folders to open; empty just raises the window.
    pub paths: Vec<PathBuf>,
}

pub enum Startup {
    /// We are the primary. Keep this alive for the life of the process.
    Primary(Primary),
    /// Another instance took our files; this process should exit.
    Forwarded,
    /// No single-instance coordination (disabled or unavailable).
    Standalone,
}

pub struct Primary {
    _lock: File,
    pub requests: mpsc::Receiver<Request>,
    /// Set by the app so the listener can wake the UI.
    pub ctx: Arc<OnceLock<egui::Context>>,
}

/// Folder holding the lock file and, on Unix, the socket.
fn runtime_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("EDI_DATA_DIR") {
        return Some(PathBuf::from(d));
    }
    if cfg!(target_os = "linux") {
        if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute()) {
            return Some(d.join("edi"));
        }
    }
    crate::session::dir()
}

fn socket_name(dir: &Path) -> std::io::Result<Name<'static>> {
    if cfg!(windows) {
        // Named pipes live in one global namespace, so include the user.
        let user = std::env::var("USERNAME").unwrap_or_default();
        format!("edi-{user}.sock").to_ns_name::<interprocess::local_socket::GenericNamespaced>().map(|n| n.into_owned())
    } else {
        dir.join("edi.sock").into_os_string().to_fs_name::<interprocess::local_socket::GenericFilePath>().map(|n| n.into_owned())
    }
}

pub fn start(paths: &[PathBuf]) -> Startup {
    match try_start(paths) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("edi: single-instance setup failed, running standalone: {e}");
            Startup::Standalone
        }
    }
}

fn try_start(paths: &[PathBuf]) -> std::io::Result<Startup> {
    let dir = runtime_dir().ok_or_else(|| std::io::Error::other("no runtime directory"))?;
    std::fs::create_dir_all(&dir)?;
    let lock = File::options().create(true).truncate(false).write(true).open(dir.join("instance.lock"))?;
    let name = socket_name(&dir)?;

    match lock.try_lock() {
        Ok(()) => {
            // We own the lock, so any existing socket file is stale.
            let listener = ListenerOptions::new().name(name).try_overwrite(true).create_sync()?;
            let (tx, rx) = mpsc::channel();
            let ctx: Arc<OnceLock<egui::Context>> = Arc::default();
            let wake = ctx.clone();
            std::thread::spawn(move || {
                for conn in listener.incoming().flatten() {
                    let mut line = String::new();
                    if BufReader::new(conn).read_line(&mut line).is_err() {
                        continue;
                    }
                    let Ok(req) = serde_json::from_str::<Request>(&line) else { continue };
                    if tx.send(req).is_err() {
                        break;
                    }
                    if let Some(ctx) = wake.get() {
                        ctx.request_repaint();
                    }
                }
            });
            Ok(Startup::Primary(Primary { _lock: lock, requests: rx, ctx }))
        }
        Err(std::fs::TryLockError::WouldBlock) => {
            let req = Request { paths: paths.iter().map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone())).collect() };
            let mut msg = serde_json::to_vec(&req)?;
            msg.push(b'\n');
            // The primary may still be starting up; give it a moment.
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match Stream::connect(name.borrow()) {
                    Ok(mut conn) => {
                        conn.write_all(&msg)?;
                        conn.flush()?;
                        return Ok(Startup::Forwarded);
                    }
                    Err(e) if Instant::now() > deadline => return Err(e),
                    Err(_) => std::thread::sleep(Duration::from_millis(100)),
                }
            }
        }
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}
