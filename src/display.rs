//! Mapping between buffer lines and screen rows. Word wrap splits a long
//! line into several rows; folding hides lines. Without either, rows and
//! lines are the same and nothing is stored.

use crate::buffer::{Buffer, TAB_SIZE};

/// One screen row: chars `start..end` of buffer line `line`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

pub enum DisplayMap {
    Identity,
    Mapped {
        rows: Vec<Row>,
        /// Row index of each line's first row. Hidden lines point at the
        /// last row of their fold's header line.
        first_row: Vec<usize>,
    },
}

/// What a map was built from, to know when to rebuild it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MapKey {
    pub version: u64,
    pub fold_gen: u64,
    pub wrap: Option<usize>,
}

impl DisplayMap {
    pub fn build(buf: &Buffer, wrap: Option<usize>) -> Self {
        if wrap.is_none() && !buf.has_folds() {
            return DisplayMap::Identity;
        }
        let n = buf.line_count();
        let hidden = buf.folded_lines();
        let mut hidden = hidden.iter().peekable();
        let mut rows = Vec::with_capacity(n);
        let mut first_row = vec![0; n];
        let mut line = 0;
        while line < n {
            if let Some(&&(a, b)) = hidden.peek() {
                if line == a {
                    let header_row = rows.len().saturating_sub(1);
                    for l in a..=b.min(n - 1) {
                        first_row[l] = header_row;
                    }
                    hidden.next();
                    line = b + 1;
                    continue;
                }
            }
            first_row[line] = rows.len();
            match wrap {
                Some(cols) => {
                    for (start, end) in wrap_line(buf, line, cols) {
                        rows.push(Row { line, start, end });
                    }
                }
                None => rows.push(Row { line, start: 0, end: buf.line_len(line) }),
            }
            line += 1;
        }
        if rows.is_empty() {
            rows.push(Row { line: 0, start: 0, end: buf.line_len(0) });
        }
        DisplayMap::Mapped { rows, first_row }
    }

    pub fn row_count(&self, buf: &Buffer) -> usize {
        match self {
            DisplayMap::Identity => buf.line_count(),
            DisplayMap::Mapped { rows, .. } => rows.len(),
        }
    }

    pub fn row(&self, buf: &Buffer, i: usize) -> Row {
        match self {
            DisplayMap::Identity => Row { line: i, start: 0, end: buf.line_len(i) },
            DisplayMap::Mapped { rows, .. } => rows[i.min(rows.len() - 1)],
        }
    }

    /// Is `i` the last row of its line?
    pub fn is_last_of_line(&self, i: usize) -> bool {
        match self {
            DisplayMap::Identity => true,
            DisplayMap::Mapped { rows, .. } => rows.get(i + 1).is_none_or(|r| r.line != rows[i].line),
        }
    }

    /// Row showing char position `pos`, and the (line, col) to draw it at.
    /// Positions inside a fold are drawn at the end of the header line.
    pub fn place(&self, buf: &Buffer, pos: usize) -> (usize, usize, usize) {
        let (line, col) = buf.pos_to_lc(pos);
        match self {
            DisplayMap::Identity => (line, line, col),
            DisplayMap::Mapped { rows, first_row } => {
                let mut i = first_row[line.min(first_row.len() - 1)];
                if rows[i].line != line {
                    let r = rows[i];
                    return (i, r.line, r.end);
                }
                while col >= rows[i].end && rows.get(i + 1).is_some_and(|r| r.line == line) {
                    i += 1;
                }
                (i, line, col)
            }
        }
    }

    /// First row of `line` (its header's row if hidden).
    pub fn line_row(&self, line: usize) -> usize {
        match self {
            DisplayMap::Identity => line,
            DisplayMap::Mapped { first_row, .. } => first_row[line.min(first_row.len() - 1)],
        }
    }
}

/// Split a line into rows of at most `cols` visual columns, breaking after
/// whitespace where possible. Trailing whitespace may overhang the edge.
pub fn wrap_line(buf: &Buffer, line: usize, cols: usize) -> Vec<(usize, usize)> {
    let cols = cols.max(8);
    let chars: Vec<char> = buf.rope.line(line).chars().take_while(|&c| c != '\n').collect();
    let mut vc = Vec::with_capacity(chars.len() + 1);
    let mut v = 0;
    vc.push(0);
    for &c in &chars {
        v = if c == '\t' { (v / TAB_SIZE + 1) * TAB_SIZE } else { v + 1 };
        vc.push(v);
    }
    let mut segs = Vec::new();
    let (mut start, mut brk) = (0, None);
    for i in 0..chars.len() {
        if chars[i].is_whitespace() {
            brk = Some(i + 1);
            continue;
        }
        while i > start && vc[i + 1] - vc[start] > cols {
            let b = match brk {
                Some(b) if b > start && b <= i => b,
                _ => i,
            };
            segs.push((start, b));
            start = b;
            brk = None;
        }
    }
    segs.push((start, chars.len()));
    segs
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    fn buf(text: &str) -> Buffer {
        let mut b = Buffer::new();
        b.rope = Rope::from_str(text);
        b
    }

    #[test]
    fn wraps_at_word_boundaries() {
        let b = buf("aaa bbb ccc ddd\n");
        assert_eq!(wrap_line(&b, 0, 8), vec![(0, 8), (8, 15)]);
        // A word longer than the width is broken hard.
        let b = buf("abcdefghijklmnop\n");
        assert_eq!(wrap_line(&b, 0, 8), vec![(0, 8), (8, 16)]);
    }

    #[test]
    fn identity_without_wrap_or_folds() {
        let b = buf("a\nb\n");
        assert!(matches!(DisplayMap::build(&b, None), DisplayMap::Identity));
    }

    #[test]
    fn folds_hide_rows_and_wrap_adds_them() {
        let mut b = buf("fn a() {\n    x;\n    y;\n}\nlong line here ok\n");
        b.fold_line(0);
        let m = DisplayMap::build(&b, Some(9));
        // header, "}", then the long line in two rows, then the empty last line.
        let lines: Vec<usize> = (0..m.row_count(&b)).map(|i| m.row(&b, i).line).collect();
        assert_eq!(lines, vec![0, 3, 4, 4, 5]);
        // A position inside the fold is placed at the end of the header row.
        let inside = b.line_start(2);
        assert_eq!(m.place(&b, inside), (0, 0, 8));
        // The second half of the long line is on row 3.
        let p = b.line_start(4) + 12;
        assert_eq!(m.place(&b, p).0, 3);
    }
}
