//! Find in Files: a cancellable background search over folders (honouring
//! .gitignore) and/or the open buffers.

use crate::search::Matcher;
use ropey::Rope;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};

const MAX_FILE_BYTES: u64 = 16 << 20;
const MAX_HITS: usize = 50_000;

/// What the "Where" field asks for, e.g. `src, tests, *.rs, -*.lock, <open files>`.
#[derive(Default, Debug, PartialEq)]
pub struct Where {
    pub paths: Vec<PathBuf>,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub open_files: bool,
}

impl Where {
    /// Relative folders resolve against `base` (the project folder).
    pub fn parse(s: &str, base: Option<&Path>) -> Self {
        let mut w = Where::default();
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            if part == crate::app::OPEN_FILES {
                w.open_files = true;
            } else if let Some(glob) = part.strip_prefix('-') {
                w.exclude.push(glob.to_string());
            } else if part.contains(['*', '?']) && !part.contains(['/', '\\']) {
                w.include.push(part.to_string());
            } else {
                let p = PathBuf::from(part);
                let p = if p.is_relative() { base.map(|b| b.join(&p)).unwrap_or(p) } else { p };
                w.paths.push(p);
            }
        }
        if w.paths.is_empty() && !w.open_files {
            match base {
                Some(b) => w.paths.push(b.to_path_buf()),
                None => w.open_files = true,
            }
        }
        w
    }
}

/// One matching line.
pub struct Hit {
    /// 0-based line number.
    pub line: usize,
    pub text: String,
    /// Matched char ranges within the line.
    pub cols: Vec<(usize, usize)>,
}

pub struct FileHits {
    /// File path, or the tab name for untitled buffers.
    pub path: PathBuf,
    pub hits: Vec<Hit>,
}

pub struct Outcome {
    pub files: Vec<FileHits>,
    pub scanned: usize,
    pub truncated: bool,
}

pub struct Job {
    pub cancel: Arc<AtomicBool>,
    pub scanned: Arc<AtomicUsize>,
    pub rx: mpsc::Receiver<Outcome>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Matches in `text`, grouped by line.
pub fn search_text(m: &Matcher, text: &str) -> Vec<Hit> {
    let mut hits: Vec<Hit> = Vec::new();
    let mut line = 0;
    let mut line_start = 0;
    let mut scanned_to = 0;
    for (a, b) in m.find_iter(text) {
        // Advance the line counter to the match start.
        for (i, _) in text[scanned_to..a].match_indices('\n') {
            line += 1;
            line_start = scanned_to + i + 1;
        }
        scanned_to = a;
        let line_end = text[line_start..].find('\n').map_or(text.len(), |i| line_start + i);
        let line_text = &text[line_start..line_end];
        let ca = text[line_start..a].chars().count();
        let cb = ca + text[a..b.min(line_end).max(a)].chars().count();
        match hits.last_mut() {
            Some(h) if h.line == line => h.cols.push((ca, cb)),
            _ => hits.push(Hit { line, text: line_text.to_string(), cols: vec![(ca, cb)] }),
        }
    }
    hits
}

fn search_file(m: &Matcher, path: &Path) -> Option<Vec<Hit>> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return None; // binary
    }
    let text = String::from_utf8(bytes).ok()?;
    let hits = search_text(m, &text.replace("\r\n", "\n"));
    (!hits.is_empty()).then_some(hits)
}

/// Start a search. `buffers` are (path or name, contents) of open tabs; they
/// are searched instead of the file on disk so unsaved edits count.
pub fn start(m: Matcher, w: Where, buffers: Vec<(PathBuf, Rope)>) -> Job {
    let cancel = Arc::new(AtomicBool::new(false));
    let scanned = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::channel();
    let (c, n) = (cancel.clone(), scanned.clone());
    std::thread::spawn(move || {
        let mut files = Vec::new();
        let mut total = 0;
        let mut truncated = false;
        let mut push = |path: PathBuf, hits: Vec<Hit>, files: &mut Vec<FileHits>| {
            total += hits.len();
            files.push(FileHits { path, hits });
            total >= MAX_HITS
        };

        let mut done_paths = std::collections::HashSet::new();
        if w.open_files {
            for (path, rope) in &buffers {
                n.fetch_add(1, Ordering::Relaxed);
                done_paths.insert(path.clone());
                let hits = search_text(&m, &rope.to_string());
                if !hits.is_empty() && push(path.clone(), hits, &mut files) {
                    truncated = true;
                    break;
                }
            }
        }

        if !truncated && !w.paths.is_empty() {
            let mut builder = ignore::WalkBuilder::new(&w.paths[0]);
            for p in &w.paths[1..] {
                builder.add(p);
            }
            builder.hidden(true);
            if !w.include.is_empty() || !w.exclude.is_empty() {
                let mut ov = ignore::overrides::OverrideBuilder::new(&w.paths[0]);
                for g in &w.include {
                    let _ = ov.add(g);
                }
                for g in &w.exclude {
                    let _ = ov.add(&format!("!{g}"));
                }
                if let Ok(ov) = ov.build() {
                    builder.overrides(ov);
                }
            }
            // Open buffers win over the file on disk, so unsaved edits count.
            let open: std::collections::HashMap<&Path, &Rope> = buffers.iter().map(|(p, r)| (p.as_path(), r)).collect();
            let mut paths: Vec<PathBuf> = builder
                .build()
                .flatten()
                .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
                .map(|e| e.into_path())
                .collect();
            paths.sort();
            for path in paths {
                if c.load(Ordering::Relaxed) {
                    return;
                }
                if done_paths.contains(&path) {
                    continue;
                }
                n.fetch_add(1, Ordering::Relaxed);
                let hits = match open.get(path.as_path()) {
                    Some(rope) => Some(search_text(&m, &rope.to_string())).filter(|h| !h.is_empty()),
                    None => search_file(&m, &path),
                };
                if let Some(hits) = hits {
                    if push(path, hits, &mut files) {
                        truncated = true;
                        break;
                    }
                }
            }
        }
        if !c.load(Ordering::Relaxed) {
            let _ = tx.send(Outcome { files, scanned: n.load(Ordering::Relaxed), truncated });
        }
    });
    Job { cancel, scanned, rx }
}

/// Where a line of the results buffer points: file, 0-based line and column,
/// and whether it is a match (false for the file-name header).
#[derive(Clone)]
pub struct Loc {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub is_match: bool,
}

/// Render results like Sublime's "Find Results" buffer. Returns the text and,
/// for every line of it, the location it jumps to.
pub fn render(o: &Outcome, pattern: &str, flags: &str) -> (String, Vec<Option<Loc>>) {
    let mut text = String::new();
    let mut locs = Vec::new();
    let mut line = |s: &str, loc: Option<Loc>, text: &mut String| {
        text.push_str(s);
        text.push('\n');
        locs.push(loc);
    };
    line(&format!("Searching {} files for \"{pattern}\"{flags}", o.scanned), None, &mut text);
    line("", None, &mut text);
    let mut matches = 0;
    for f in &o.files {
        let header = Loc { path: f.path.clone(), line: 0, col: 0, is_match: false };
        line(&format!("{}:", f.path.display()), Some(header), &mut text);
        for h in &f.hits {
            matches += h.cols.len();
            let col = h.cols.first().map_or(0, |c| c.0);
            let loc = Loc { path: f.path.clone(), line: h.line, col, is_match: true };
            line(&format!("{:>6}:  {}", h.line + 1, h.text), Some(loc), &mut text);
        }
        line("", None, &mut text);
    }
    let files = o.files.len();
    let mut summary = format!(
        "{matches} match{} across {files} file{}",
        if matches == 1 { "" } else { "es" },
        if files == 1 { "" } else { "s" }
    );
    if o.truncated {
        summary.push_str(&format!(" (stopped after {MAX_HITS} matching lines)"));
    }
    line(&summary, None, &mut text);
    (text, locs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::Search;

    fn m(p: &str, regex: bool) -> Matcher {
        Search { pattern: p.into(), regex, case: true, whole_word: false }.compile().unwrap()
    }

    #[test]
    fn groups_matches_by_line() {
        let hits = search_text(&m("ab", false), "ab ab\nx\näab\n");
        assert_eq!(hits.len(), 2);
        assert_eq!((hits[0].line, hits[0].cols.clone()), (0, vec![(0, 2), (3, 5)]));
        assert_eq!((hits[1].line, hits[1].text.as_str(), hits[1].cols.clone()), (2, "äab", vec![(1, 3)]));
    }

    #[test]
    fn parses_where() {
        let base = Path::new("/proj");
        let w = Where::parse("src, *.rs, -*.lock, <open files>", Some(base));
        assert_eq!(w.paths, vec![PathBuf::from("/proj/src")]);
        assert_eq!(w.include, vec!["*.rs"]);
        assert_eq!(w.exclude, vec!["*.lock"]);
        assert!(w.open_files);
        assert_eq!(Where::parse("", Some(base)).paths, vec![PathBuf::from("/proj")]);
        assert!(Where::parse("", None).open_files);
    }

    #[test]
    fn searches_a_folder_with_filters() {
        let dir = std::env::temp_dir().join(format!("edi-fif-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.rs"), "fn needle() {}\n").unwrap();
        std::fs::write(dir.join("sub/b.txt"), "one\nneedle here\n").unwrap();
        std::fs::write(dir.join("c.bin"), b"needle\0\0").unwrap();
        let w = Where::parse(&format!("{}, -*.txt", dir.display()), None);
        let job = start(m("needle", false), w, vec![]);
        let o = job.rx.recv().unwrap();
        let names: Vec<_> = o.files.iter().map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["a.rs"]);
        let (text, locs) = render(&o, "needle", "");
        assert!(text.contains("     1:  fn needle() {}"));
        assert_eq!(locs.iter().flatten().count(), 2); // file header + one hit
        assert_eq!(locs.iter().flatten().filter(|l| l.is_match).count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
