//! Markdown: align the columns of every table; everything else, including
//! tables inside fenced code blocks, is left alone.

#[derive(Clone, Copy)]
enum Align {
    None,
    Left,
    Center,
    Right,
}

/// Split a table row into trimmed cells, honouring `\|` escapes.
fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = if t.ends_with('|') && !t.ends_with("\\|") { &t[..t.len() - 1] } else { t };
    let mut cells = vec![String::new()];
    let mut chars = t.chars();
    while let Some(c) = chars.next() {
        let cell = cells.last_mut().unwrap();
        match c {
            '\\' => {
                cell.push(c);
                cell.extend(chars.next());
            }
            '|' => cells.push(String::new()),
            c => cell.push(c),
        }
    }
    cells.iter().map(|c| c.trim().to_string()).collect()
}

/// The column alignments, if `line` is a table's delimiter row.
fn delimiter(line: &str) -> Option<Vec<Align>> {
    if !line.contains('-') {
        return None;
    }
    split_row(line)
        .iter()
        .map(|c| {
            let dashes = c.trim_start_matches(':').trim_end_matches(':');
            if dashes.is_empty() || !dashes.chars().all(|c| c == '-') {
                return None;
            }
            Some(match (c.starts_with(':'), c.ends_with(':')) {
                (true, true) => Align::Center,
                (true, false) => Align::Left,
                (false, true) => Align::Right,
                (false, false) => Align::None,
            })
        })
        .collect()
}

fn table(rows: &[&str], aligns: &[Align], indent: &str) -> Vec<String> {
    let mut cells: Vec<Vec<String>> =
        rows.iter().enumerate().filter(|&(k, _)| k != 1).map(|(_, r)| split_row(r)).collect();
    let n = cells.iter().map(Vec::len).max().unwrap_or(0).max(aligns.len());
    for r in &mut cells {
        r.resize(n, String::new());
    }
    let width = |s: &str| s.chars().count();
    let widths: Vec<usize> = (0..n).map(|c| cells.iter().map(|r| width(&r[c])).max().unwrap_or(0).max(3)).collect();
    let align = |c: usize| aligns.get(c).copied().unwrap_or(Align::None);
    let row = |parts: Vec<String>| format!("{indent}| {} |", parts.join(" | "));
    let render = |r: &[String]| {
        row(r
            .iter()
            .enumerate()
            .map(|(c, s)| {
                let pad = widths[c] - width(s);
                match align(c) {
                    Align::Right => format!("{}{s}", " ".repeat(pad)),
                    Align::Center => format!("{}{s}{}", " ".repeat(pad / 2), " ".repeat(pad - pad / 2)),
                    _ => format!("{s}{}", " ".repeat(pad)),
                }
            })
            .collect())
    };
    let delim = row((0..n)
        .map(|c| {
            let w = widths[c];
            match align(c) {
                Align::None => "-".repeat(w),
                Align::Left => format!(":{}", "-".repeat(w - 1)),
                Align::Right => format!("{}:", "-".repeat(w - 1)),
                Align::Center => format!(":{}:", "-".repeat(w - 2)),
            }
        })
        .collect());
    let mut out = vec![render(&cells[0]), delim];
    out.extend(cells[1..].iter().map(|r| render(r)));
    out
}

/// The text with its tables aligned, and how many tables there were.
pub(super) fn format(text: &str) -> (String, usize) {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut fence: Option<&str> = None;
    let mut count = 0;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim_start();
        if let Some(f) = fence {
            if t.starts_with(f) {
                fence = None;
            }
        } else if t.starts_with("```") || t.starts_with("~~~") {
            fence = Some(&t[..3]);
        } else if let Some(aligns) = lines
            .get(i + 1)
            .filter(|_| line.contains('|'))
            .and_then(|l| delimiter(l))
            .filter(|a| a.len() == split_row(line).len())
        {
            let end = (i + 2..lines.len()).find(|&k| !lines[k].contains('|') || lines[k].trim().is_empty());
            let end = end.unwrap_or(lines.len());
            out.extend(table(&lines[i..end], &aligns, &line[..line.len() - t.len()]));
            count += 1;
            i = end;
            continue;
        }
        out.push(line.to_string());
        i += 1;
    }
    (out.join("\n"), count)
}
