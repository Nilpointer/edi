//! XML and HTML: one element per line, indented by nesting. Elements with
//! text content stay on one line, so mixed content keeps its spacing. HTML
//! is parsed leniently: void elements, optional end tags and raw-text
//! elements (script, style, pre, textarea) are understood.

use super::{Lines, css, json};
use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Open,
    Close,
    /// Self-closing and void tags, comments, CDATA, processing instructions, doctype.
    Leaf,
    Text,
    /// The verbatim content of an HTML raw-text element.
    Raw,
}

struct Tok<'a> {
    kind: Kind,
    /// Tag name; lower-cased for HTML.
    name: String,
    text: Cow<'a, str>,
    /// Whitespace separated this token from the previous one in the source.
    space: bool,
}

const VOID: &[&str] =
    &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];
const RAW: &[&str] = &["script", "style", "pre", "textarea"];
const INLINE: &[&str] = &[
    "a", "abbr", "b", "bdi", "bdo", "br", "button", "cite", "code", "data", "del", "dfn", "em", "i", "img", "input",
    "ins", "kbd", "label", "mark", "q", "s", "samp", "small", "span", "strong", "sub", "sup", "time", "u", "var", "wbr",
];
/// Elements whose start tag ends an open <p>.
const BLOCK: &[&str] = &[
    "address", "article", "aside", "blockquote", "div", "dl", "fieldset", "figure", "footer", "form", "h1", "h2", "h3",
    "h4", "h5", "h6", "header", "main", "nav", "ol", "p", "pre", "section", "table", "ul",
];

/// Does a `<open>` start tag implicitly end an open `<top>` element?
fn closes(open: &str, top: &str) -> bool {
    match open {
        "li" => top == "li",
        "dt" | "dd" => matches!(top, "dt" | "dd"),
        "tr" => matches!(top, "tr" | "td" | "th"),
        "td" | "th" => matches!(top, "td" | "th"),
        "option" => top == "option",
        _ => top == "p" && BLOCK.contains(&open),
    }
}

fn tag_name(raw: &str, html: bool) -> String {
    let t = raw.trim_start_matches(['<', '/']);
    let n = t.split(|c: char| c.is_whitespace() || c == '/' || c == '>').next().unwrap_or("");
    if html { n.to_ascii_lowercase() } else { n.to_string() }
}

/// Collapse whitespace outside quoted attribute values, so tags written
/// across several lines end up on one.
fn tidy_tag(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut quote = None;
    for c in raw.chars() {
        match quote {
            Some(q) => {
                out.push(c);
                if c == q {
                    quote = None;
                }
            }
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                out.push(c);
            }
            None if c.is_whitespace() => {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            None => out.push(c),
        }
    }
    if out.ends_with(" >") {
        out.remove(out.len() - 2);
    }
    out
}

/// Does a tag (rather than a literal '<') start at byte `i`?
fn tag_at(b: &[u8], i: usize) -> bool {
    b[i] == b'<' && b.get(i + 1).is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, b'/' | b'!' | b'?' | b'_' | b':'))
}

/// Length of the tag at the start of `s`, up to its '>' outside quotes and
/// outside a doctype's [internal subset].
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    let mut brackets = 0i32;
    for (k, &c) in s.as_bytes().iter().enumerate().skip(1) {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(c),
            (None, b'[') => brackets += 1,
            (None, b']') => brackets -= 1,
            (None, b'>') if brackets <= 0 => return Some(k + 1),
            _ => {}
        }
    }
    None
}

fn tokenize(text: &str, html: bool) -> Result<Vec<Tok<'_>>, String> {
    let lang = if html { "HTML" } else { "XML" };
    let unterminated = |what: &str| format!("Invalid {lang}: unterminated {what}");
    let lower = text.to_ascii_lowercase();
    let b = text.as_bytes();
    let mut toks = Vec::new();
    let mut space = false;
    let mut i = 0;
    while i < text.len() {
        if !tag_at(b, i) {
            let mut end = i + 1;
            while end < text.len() && !tag_at(b, end) {
                end += 1;
            }
            let chunk = &text[i..end];
            let t = chunk.trim();
            if t.is_empty() {
                space = true;
            } else {
                let text = if html { Cow::Owned(t.split_whitespace().collect::<Vec<_>>().join(" ")) } else { Cow::Borrowed(t) };
                let space_before = space || chunk.starts_with(char::is_whitespace);
                toks.push(Tok { kind: Kind::Text, name: String::new(), text, space: space_before });
                space = chunk.ends_with(char::is_whitespace);
            }
            i = end;
            continue;
        }
        let rest = &text[i..];
        let (len, mut kind, tidy) = if rest.starts_with("<!--") {
            (rest.find("-->").ok_or_else(|| unterminated("comment"))? + 3, Kind::Leaf, false)
        } else if rest.starts_with("<![CDATA[") {
            (rest.find("]]>").ok_or_else(|| unterminated("CDATA section"))? + 3, Kind::Leaf, false)
        } else if rest.starts_with("<?") {
            (rest.find("?>").ok_or_else(|| unterminated("processing instruction"))? + 2, Kind::Leaf, false)
        } else {
            let len = tag_end(rest).ok_or_else(|| unterminated("tag"))?;
            let raw = &rest[..len];
            let kind = if raw.starts_with("<!") || raw.ends_with("/>") {
                Kind::Leaf
            } else if raw.starts_with("</") {
                Kind::Close
            } else {
                Kind::Open
            };
            (len, kind, true)
        };
        let raw = &rest[..len];
        let name = tag_name(raw, html);
        if html && kind == Kind::Open && VOID.contains(&name.as_str()) {
            kind = Kind::Leaf;
        }
        let raw_element = html && kind == Kind::Open && RAW.contains(&name.as_str());
        let text_ = if tidy { Cow::Owned(tidy_tag(raw)) } else { Cow::Borrowed(raw) };
        toks.push(Tok { kind, name, text: text_, space });
        space = false;
        i += len;
        if raw_element {
            let close = format!("</{}", toks.last().unwrap().name);
            let end = lower[i..].find(&close).map_or(text.len(), |p| i + p);
            if end > i {
                toks.push(Tok { kind: Kind::Raw, name: String::new(), text: Cow::Borrowed(&text[i..end]), space: false });
            }
            i = end;
        }
    }
    Ok(toks)
}

/// If the element opened at `i` should stay on one line, the index of the
/// run's last token, and whether that is the element's end tag (an HTML
/// element may instead end implicitly).
fn inline_end(toks: &[Tok], i: usize, html: bool) -> Option<(usize, bool)> {
    let name = toks[i].name.as_str();
    let mut stack: Vec<&str> = Vec::new();
    let mut text_child = false;
    for (j, t) in toks.iter().enumerate().skip(i + 1) {
        match t.kind {
            Kind::Close if stack.is_empty() => {
                return if t.name == name {
                    (j == i + 1 || html || text_child).then_some((j, true))
                } else {
                    html.then_some((j - 1, false))
                };
            }
            Kind::Close => {
                if stack.pop() != Some(t.name.as_str()) {
                    return None;
                }
            }
            Kind::Open if html && stack.is_empty() && closes(&t.name, name) => return Some((j - 1, false)),
            Kind::Open => {
                if html && !INLINE.contains(&t.name.as_str()) {
                    return None;
                }
                stack.push(&t.name);
            }
            Kind::Leaf => {
                if html && !t.text.starts_with("<!--") && !INLINE.contains(&t.name.as_str()) {
                    return None;
                }
            }
            Kind::Text => text_child |= stack.is_empty(),
            Kind::Raw => {
                if !matches!(name, "pre" | "textarea") {
                    return None;
                }
            }
        }
    }
    None
}

fn join_inline(toks: &[Tok]) -> String {
    let mut s = String::new();
    for (k, t) in toks.iter().enumerate() {
        if k > 0 && t.space {
            s.push(' ');
        }
        s.push_str(&t.text);
    }
    s
}

/// Strip the common indentation and surrounding blank lines.
fn dedent(raw: &str) -> Vec<String> {
    let lines: Vec<&str> = raw.lines().map(str::trim_end).collect();
    let Some(first) = lines.iter().position(|l| !l.is_empty()) else { return Vec::new() };
    let last = lines.iter().rposition(|l| !l.is_empty()).unwrap_or(first);
    let lines = &lines[first..=last];
    let lead = |l: &str| l.chars().take_while(|c| c.is_whitespace()).count();
    let indent = lines.iter().filter(|l| !l.is_empty()).map(|l| lead(l)).min().unwrap_or(0);
    lines.iter().map(|l| l.chars().skip(indent).collect()).collect()
}

/// The content of a <script> or <style>: CSS and JSON are formatted too.
fn raw_lines(open: &Tok, raw: &str) -> Lines {
    let parsed = match open.name.as_str() {
        "style" => css::format(raw).ok(),
        "script" if open.text.to_ascii_lowercase().contains("json") => json::format(raw).ok(),
        _ => None,
    };
    parsed.unwrap_or_else(|| dedent(raw).into_iter().map(|l| (0, l)).collect())
}

pub(super) fn format(text: &str, html: bool) -> Result<Lines, String> {
    let toks = tokenize(text, html)?;
    let mut lines: Lines = Vec::new();
    let mut open: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        match t.kind {
            Kind::Open => {
                if html {
                    while open.last().is_some_and(|top| closes(&t.name, top)) {
                        open.pop();
                    }
                }
                if let Some((end, _)) = inline_end(&toks, i, html) {
                    lines.push((open.len(), join_inline(&toks[i..=end])));
                    i = end + 1;
                    continue;
                }
                lines.push((open.len(), t.text.to_string()));
                open.push(&t.name);
            }
            Kind::Close => {
                let Some(pos) = open.iter().rposition(|n| *n == t.name) else {
                    if html {
                        lines.push((open.len(), t.text.to_string()));
                        i += 1;
                        continue;
                    }
                    return Err(format!("Invalid XML: unexpected {}", t.text));
                };
                if !html && pos + 1 != open.len() {
                    return Err(format!("Invalid XML: {} does not match <{}>", t.text, open[open.len() - 1]));
                }
                open.truncate(pos);
                lines.push((open.len(), t.text.to_string()));
            }
            Kind::Raw => {
                let depth = open.len();
                lines.extend(raw_lines(&toks[i - 1], &t.text).into_iter().map(|(d, l)| (depth + d, l)));
            }
            Kind::Leaf | Kind::Text => lines.push((open.len(), t.text.to_string())),
        }
        i += 1;
    }
    if let (false, Some(n)) = (html, open.last()) {
        return Err(format!("Invalid XML: <{n}> is never closed"));
    }
    Ok(lines)
}
