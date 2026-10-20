//! JSON: one value per line, validated first so errors carry a position.

use super::Lines;

pub(super) fn format(text: &str) -> Result<Lines, String> {
    serde_json::from_str::<serde::de::IgnoredAny>(text).map_err(|e| format!("Invalid JSON: {e}"))?;
    let mut lines: Lines = Vec::new();
    let mut cur = String::new();
    let mut depth = 0;
    let mut chars = text.chars().peekable();
    let flush = |lines: &mut Lines, cur: &mut String, depth| {
        if !cur.is_empty() {
            lines.push((depth, std::mem::take(cur)));
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                cur.push(c);
                while let Some(c) = chars.next() {
                    cur.push(c);
                    match c {
                        '\\' => cur.extend(chars.next()),
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '{' | '[' => {
                cur.push(c);
                // Keep empty containers on one line.
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
                if matches!((c, chars.peek()), ('{', Some('}')) | ('[', Some(']'))) {
                    cur.extend(chars.next());
                } else {
                    flush(&mut lines, &mut cur, depth);
                    depth += 1;
                }
            }
            '}' | ']' => {
                flush(&mut lines, &mut cur, depth);
                depth -= 1;
                cur.push(c);
            }
            ',' => {
                cur.push(c);
                flush(&mut lines, &mut cur, depth);
            }
            ':' => cur.push_str(": "),
            c if c.is_whitespace() => {}
            c => cur.push(c),
        }
    }
    flush(&mut lines, &mut cur, depth);
    Ok(lines)
}
