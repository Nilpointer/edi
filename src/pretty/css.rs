//! CSS, SCSS and Less: one declaration per line, rules indented by nesting,
//! a blank line between top-level rules.

use super::Lines;

/// Byte offset of the first ':' outside parentheses and quotes.
fn top_colon(s: &str) -> Option<usize> {
    let mut parens = 0;
    let mut quote = None;
    for (k, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(') => parens += 1,
            (None, ')') => parens -= 1,
            (None, ':') if parens == 0 => return Some(k),
            _ => {}
        }
    }
    None
}

fn push_decl(lines: &mut Lines, depth: usize, decl: &str, semi: bool) {
    let d = decl.trim();
    if d.is_empty() {
        return;
    }
    let d = match top_colon(d) {
        Some(p) if !d.starts_with('@') => format!("{}: {}", d[..p].trim_end(), d[p + 1..].trim_start()),
        _ => d.to_string(),
    };
    lines.push((depth, if semi { format!("{d};") } else { d }));
}

pub(super) fn format(text: &str) -> Result<Lines, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut lines: Lines = Vec::new();
    let mut cur = String::new();
    let mut depth = 0;
    let mut parens = 0usize;
    // Open SCSS #{…} interpolations.
    let mut interp = 0usize;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '"' | '\'' => {
                let start = i;
                i += 1;
                while i < chars.len() && chars[i] != c {
                    i += if chars[i] == '\\' { 2 } else { 1 };
                }
                i = (i + 1).min(chars.len());
                cur.extend(&chars[start..i]);
                continue;
            }
            '/' if next == Some('*') || next == Some('/') && parens == 0 => {
                let end = if next == Some('*') {
                    (i + 2..chars.len().saturating_sub(1))
                        .find(|&k| chars[k] == '*' && chars[k + 1] == '/')
                        .map_or(chars.len(), |k| k + 2)
                } else {
                    (i..chars.len()).find(|&k| chars[k] == '\n').unwrap_or(chars.len())
                };
                let comment: String = chars[i..end].iter().collect();
                if cur.is_empty() || next == Some('/') {
                    lines.push((depth, comment));
                } else {
                    cur.push_str(&comment);
                }
                i = end;
                continue;
            }
            '(' => {
                parens += 1;
                cur.push(c);
            }
            ')' => {
                parens = parens.saturating_sub(1);
                cur.push(c);
            }
            '{' if cur.ends_with('#') => {
                interp += 1;
                cur.push(c);
            }
            '}' if interp > 0 => {
                interp -= 1;
                cur.push(c);
            }
            '{' if parens == 0 => {
                let sel = std::mem::take(&mut cur);
                let sel = sel.trim();
                let open = if sel.is_empty() { "{".to_string() } else { format!("{sel} {{") };
                let k = (i + 1..chars.len()).find(|&k| !chars[k].is_whitespace()).unwrap_or(chars.len());
                if chars.get(k) == Some(&'}') {
                    lines.push((depth, format!("{open}}}")));
                    if depth == 0 {
                        lines.push((0, String::new()));
                    }
                    i = k + 1;
                    continue;
                }
                lines.push((depth, open));
                depth += 1;
            }
            ';' if parens == 0 => push_decl(&mut lines, depth, &std::mem::take(&mut cur), true),
            '}' => {
                push_decl(&mut lines, depth, &std::mem::take(&mut cur), true);
                if depth == 0 {
                    return Err("Invalid CSS: unexpected }".into());
                }
                depth -= 1;
                lines.push((depth, "}".into()));
                if depth == 0 {
                    lines.push((0, String::new()));
                }
            }
            c if c.is_whitespace() => {
                if !cur.is_empty() && !cur.ends_with(' ') {
                    cur.push(' ');
                }
            }
            c => cur.push(c),
        }
        i += 1;
    }
    push_decl(&mut lines, depth, &cur, false);
    if depth > 0 {
        return Err("Invalid CSS: missing }".into());
    }
    while lines.last().is_some_and(|l| l.1.is_empty()) {
        lines.pop();
    }
    Ok(lines)
}
