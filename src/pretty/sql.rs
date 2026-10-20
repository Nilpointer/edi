//! SQL: each clause starts a line, column lists and value lists get one item
//! per line, AND/OR conditions are broken out, subqueries are indented and
//! keywords are upper-cased. Never fails: unknown syntax is just spaced.

use super::Lines;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum T {
    Word,
    Quoted,
    Str,
    Num,
    Op,
    /// `.` and `::`, written without spaces.
    Glue,
    Open,
    Close,
    Comma,
    Semi,
    LineComment,
    BlockComment,
}

struct Tok {
    t: T,
    s: String,
    /// Whitespace preceded the token in the source.
    space: bool,
}

const KEYWORDS: &[&str] = &[
    "ADD", "ALL", "ALTER", "AND", "AS", "ASC", "BETWEEN", "BY", "CASCADE", "CASE", "CAST", "CHECK", "COLUMN",
    "CONFLICT", "CONSTRAINT", "CREATE", "CROSS", "DEFAULT", "DELETE", "DESC", "DISTINCT", "DO", "DROP", "ELSE", "END",
    "EXCEPT", "EXISTS", "FALSE", "FETCH", "FIRST", "FOREIGN", "FROM", "FULL", "GROUP", "HAVING", "IF", "ILIKE", "IN",
    "INDEX", "INNER", "INSERT", "INTERSECT", "INTO", "IS", "JOIN", "KEY", "LAST", "LEFT", "LIKE", "LIMIT", "NATURAL",
    "NEXT", "NOT", "NOTHING", "NULL", "NULLS", "OFFSET", "ON", "ONLY", "OR", "ORDER", "OUTER", "OVER", "PARTITION",
    "PRIMARY", "RECURSIVE", "REFERENCES", "REPLACE", "RETURNING", "RIGHT", "ROWS", "SELECT", "SET", "TABLE", "THEN",
    "TRUE", "UNION", "UNIQUE", "UPDATE", "USING", "VALUES", "VIEW", "WHEN", "WHERE", "WINDOW", "WITH",
];
/// Keywords that are also functions, so `CAST(` keeps its parenthesis attached.
const FUNC_KEYWORDS: &[&str] = &["CAST", "IF", "LEFT", "REPLACE", "RIGHT"];
const LIST_CLAUSES: &[&str] = &["SELECT", "GROUP BY", "ORDER BY", "SET", "VALUES", "RETURNING"];
const JOIN_MODIFIERS: &[&str] = &["LEFT", "RIGHT", "FULL", "INNER", "CROSS", "NATURAL", "OUTER"];

fn tokenize(text: &str) -> Vec<Tok> {
    let c: Vec<char> = text.chars().collect();
    let n = c.len();
    let ident = |ch: char| ch.is_alphanumeric() || matches!(ch, '_' | '$' | '#');
    let mut toks = Vec::new();
    let mut i = 0;
    let mut space = false;
    while i < n {
        let ch = c[i];
        let next = c.get(i + 1).copied();
        let start = i;
        let find = |from: usize, pat: &str| {
            let p: Vec<char> = pat.chars().collect();
            (from..n).find(|&k| c[k..].starts_with(&p)).map_or(n, |k| k + p.len())
        };
        let t = if ch.is_whitespace() {
            space = true;
            i += 1;
            continue;
        } else if ch == '-' && next == Some('-') {
            i = (i..n).find(|&k| c[k] == '\n').unwrap_or(n);
            T::LineComment
        } else if ch == '/' && next == Some('*') {
            i = find(i + 2, "*/");
            T::BlockComment
        } else if ch == '\'' {
            i += 1;
            while i < n {
                if c[i] == '\'' {
                    if c.get(i + 1) == Some(&'\'') {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i = (i + 1).min(n);
            T::Str
        } else if matches!(ch, '"' | '`' | '[') {
            let close = if ch == '[' { ']' } else { ch };
            i = (i + 1..n).find(|&k| c[k] == close).map_or(n, |k| k + 1);
            T::Quoted
        } else if ch == '$' && next.is_some_and(|x| x == '$' || x.is_alphabetic() || x == '_') {
            // Postgres dollar quoting: $$…$$ or $tag$…$tag$.
            match (i + 1..n).take_while(|&k| c[k] == '$' || ident(c[k])).find(|&k| c[k] == '$') {
                Some(e) => {
                    let tag: String = c[i..=e].iter().collect();
                    i = find(e + 1, &tag);
                    T::Str
                }
                None => {
                    i = (i + 1..n).find(|&k| !ident(c[k])).unwrap_or(n);
                    T::Word
                }
            }
        } else if ch.is_ascii_digit() || ch == '.' && next.is_some_and(|x| x.is_ascii_digit()) {
            i += 1;
            while i < n && (c[i].is_ascii_alphanumeric() || c[i] == '.' || matches!(c[i], '+' | '-') && matches!(c[i - 1], 'e' | 'E')) {
                i += 1;
            }
            T::Num
        } else if ident(ch) || matches!(ch, '@' | '?') || ch == ':' && next.is_some_and(|x| x.is_alphabetic() || x == '_') {
            i += 1;
            while i < n && ident(c[i]) {
                i += 1;
            }
            T::Word
        } else if ch == '.' || ch == ':' && next == Some(':') {
            i += if ch == '.' { 1 } else { 2 };
            T::Glue
        } else {
            i += 1;
            match ch {
                '(' => T::Open,
                ')' => T::Close,
                ',' => T::Comma,
                ';' => T::Semi,
                _ => {
                    let rest: String = c[start..n.min(start + 3)].iter().collect();
                    let len = ["->>", "#>>"].iter().find(|o| rest.starts_with(*o)).map(|_| 3).or_else(|| {
                        ["<=", ">=", "<>", "!=", "||", "->", "=>", ":=", "<<", ">>", "@>", "<@", "&&", "#>", "~*", "!~"]
                            .iter()
                            .find(|o| rest.starts_with(*o))
                            .map(|_| 2)
                    });
                    i = start + len.unwrap_or(1);
                    T::Op
                }
            }
        };
        toks.push(Tok { t, s: c[start..i].iter().collect(), space });
        space = false;
    }
    toks
}

fn upper(toks: &[Tok], j: usize) -> String {
    toks.get(j).filter(|t| t.t == T::Word).map(|t| t.s.to_ascii_uppercase()).unwrap_or_default()
}

/// The clause the word at `k` starts, if any. `fresh` is true at the start
/// of a statement or subquery.
fn clause_at(toks: &[Tok], k: usize, fresh: bool) -> Option<&'static str> {
    let prev = if k > 0 { upper(toks, k - 1) } else { String::new() };
    let prev = prev.as_str();
    Some(match upper(toks, k).as_str() {
        "SELECT" => "SELECT",
        "FROM" if !matches!(prev, "DELETE" | "DISTINCT") => "FROM",
        "WHERE" => "WHERE",
        "HAVING" => "HAVING",
        "LIMIT" => "LIMIT",
        "OFFSET" => "OFFSET",
        "FETCH" => "FETCH",
        "WINDOW" => "WINDOW",
        "VALUES" => "VALUES",
        "RETURNING" => "RETURNING",
        "SET" if !matches!(prev, "CHARACTER" | "DELETE" | "UPDATE") => "SET",
        "UPDATE" if !matches!(prev, "DO" | "FOR" | "ON") => "UPDATE",
        "DELETE" if prev != "ON" => "DELETE",
        "INSERT" => "INSERT",
        "CREATE" | "ALTER" | "DROP" if fresh => "CREATE",
        "WITH" if fresh => "WITH",
        "GROUP" if upper(toks, k + 1) == "BY" => "GROUP BY",
        "ORDER" if upper(toks, k + 1) == "BY" => "ORDER BY",
        "UNION" | "INTERSECT" | "EXCEPT" => "UNION",
        "JOIN" if !JOIN_MODIFIERS.contains(&prev) => "JOIN",
        w if JOIN_MODIFIERS.contains(&w)
            && !JOIN_MODIFIERS.contains(&prev)
            && (upper(toks, k + 1) == "JOIN" || upper(toks, k + 2) == "JOIN" || upper(toks, k + 3) == "JOIN") =>
        {
            "JOIN"
        }
        _ => return None,
    })
}

/// Is there a comma at this nesting level before the clause or group ends?
fn has_top_comma(toks: &[Tok], from: usize) -> bool {
    let mut depth = 0;
    for j in from..toks.len() {
        match toks[j].t {
            T::Open => depth += 1,
            T::Close if depth == 0 => return false,
            T::Close => depth -= 1,
            T::Semi => return false,
            T::Comma if depth == 0 => return true,
            T::Word if depth == 0 && clause_at(toks, j, false).is_some() => return false,
            _ => {}
        }
    }
    false
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Frame {
    /// The top level or a subquery: clauses start lines.
    Sub,
    /// A parenthesised list with one item per line, e.g. column definitions.
    List,
    Inline,
}

struct Ctx {
    kind: Frame,
    /// Depth of the frame's clause lines.
    base: usize,
    /// Depth of the closing parenthesis.
    close: usize,
    clause: &'static str,
    list: bool,
    /// The next token starts a list item.
    item_start: bool,
    /// Inside BETWEEN … AND, whose AND must not break the line.
    between: bool,
}

impl Ctx {
    fn new(kind: Frame, base: usize, close: usize) -> Self {
        let list = kind == Frame::List;
        Ctx { kind, base, close, clause: "", list, item_start: list, between: false }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Prev {
    None,
    Open,
    Glue,
    Unary,
    /// A name that may be a function: `(` attaches when it did in the source.
    Func,
    Keyword,
    Op,
    Comma,
    Other,
}

struct W {
    lines: Lines,
    cur: String,
    depth: usize,
    prev: Prev,
}

impl W {
    fn newline(&mut self, depth: usize) {
        if !self.cur.is_empty() {
            self.lines.push((self.depth, std::mem::take(&mut self.cur)));
        }
        self.depth = depth;
        self.prev = Prev::None;
    }

    fn put(&mut self, t: T, s: &str, prev: Prev) {
        let space = !matches!(self.prev, Prev::None | Prev::Open | Prev::Glue | Prev::Unary)
            && !matches!(t, T::Close | T::Comma | T::Semi | T::Glue)
            && !(self.prev == Prev::Func && t == T::Open);
        if space {
            self.cur.push(' ');
        }
        self.cur.push_str(s);
        self.prev = prev;
    }
}

pub(super) fn format(text: &str) -> Lines {
    let toks = tokenize(text);
    let mut w = W { lines: Vec::new(), cur: String::new(), depth: 0, prev: Prev::None };
    let mut stack = vec![Ctx::new(Frame::Sub, 0, 0)];
    for (k, tok) in toks.iter().enumerate() {
        let up = tok.s.to_ascii_uppercase();
        let word = tok.t == T::Word;
        let kw = word && KEYWORDS.contains(&up.as_str());
        let f = stack.last_mut().unwrap();
        if f.kind == Frame::Sub && word {
            if let Some(clause) = clause_at(&toks, k, f.clause.is_empty()) {
                w.newline(f.base);
                f.clause = clause;
                f.between = false;
                f.list = LIST_CLAUSES.contains(&clause) && has_top_comma(&toks, k + 1);
                f.item_start = f.list;
            } else if matches!(up.as_str(), "AND" | "OR") && matches!(f.clause, "WHERE" | "HAVING" | "JOIN") {
                if f.between && up == "AND" {
                    f.between = false;
                } else {
                    w.newline(f.base + 1);
                }
            }
        }
        if word && up == "BETWEEN" {
            f.between = true;
        }
        let modifier = kw && matches!(up.as_str(), "SELECT" | "DISTINCT" | "ALL" | "GROUP" | "ORDER" | "BY" | "SET" | "VALUES" | "RETURNING");
        if f.item_start && !modifier && tok.t != T::Close {
            w.newline(if f.kind == Frame::Sub { f.base + 1 } else { f.base });
            f.item_start = false;
        }
        match tok.t {
            T::Open => {
                let next = toks[k + 1..].iter().find(|t| !matches!(t.t, T::LineComment | T::BlockComment));
                let kind = if next.is_some_and(|t| t.t == T::Word && matches!(t.s.to_ascii_uppercase().as_str(), "SELECT" | "WITH")) {
                    Frame::Sub
                } else if f.kind == Frame::Sub && f.clause == "CREATE" && has_top_comma(&toks, k + 1) {
                    Frame::List
                } else {
                    Frame::Inline
                };
                if w.prev == Prev::Func && tok.space {
                    w.prev = Prev::Other;
                }
                w.put(T::Open, "(", Prev::Open);
                let d = w.depth;
                stack.push(Ctx::new(kind, d + 1, d));
            }
            T::Close => {
                if stack.len() > 1 {
                    let c = stack.pop().unwrap();
                    if c.kind != Frame::Inline {
                        w.newline(c.close);
                    }
                }
                w.put(T::Close, ")", Prev::Other);
            }
            T::Comma => {
                w.put(T::Comma, ",", Prev::Comma);
                if f.list && f.kind != Frame::Inline {
                    f.item_start = true;
                }
            }
            T::Semi => {
                w.put(T::Semi, ";", Prev::Other);
                stack = vec![Ctx::new(Frame::Sub, 0, 0)];
                w.newline(0);
                if k + 1 < toks.len() {
                    w.lines.push((0, String::new()));
                }
            }
            T::LineComment => {
                w.put(T::LineComment, &tok.s, Prev::Other);
                w.newline(w.depth);
            }
            T::Op => {
                let unary = matches!(tok.s.as_str(), "-" | "+")
                    && matches!(w.prev, Prev::None | Prev::Open | Prev::Comma | Prev::Op | Prev::Keyword | Prev::Unary);
                w.put(T::Op, &tok.s, if unary { Prev::Unary } else { Prev::Op });
            }
            T::Glue => w.put(T::Glue, &tok.s, Prev::Glue),
            T::Word if kw => {
                let prev = if FUNC_KEYWORDS.contains(&up.as_str()) { Prev::Func } else { Prev::Keyword };
                w.put(T::Word, &up, prev);
            }
            T::Word | T::Quoted => w.put(tok.t, &tok.s, Prev::Func),
            T::Str | T::Num | T::BlockComment => w.put(tok.t, &tok.s, Prev::Other),
        }
    }
    w.newline(0);
    while w.lines.last().is_some_and(|l| l.1.is_empty()) {
        w.lines.pop();
    }
    w.lines
}
