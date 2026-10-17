//! Subsequence fuzzy matching in the spirit of Sublime's Goto Anything.

/// Score `cand` against `query`; `None` if not every query char appears in
/// order. Higher is better. Also returns matched char indices for display.
pub fn score(query: &str, cand: &str) -> Option<(i64, Vec<usize>)> {
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).flat_map(|c| c.to_lowercase()).collect();
    let chars: Vec<char> = cand.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();

    let mut positions = Vec::with_capacity(q.len());
    let mut score: i64 = 0;
    let mut qi = 0;
    let mut prev: Option<usize> = None;
    // Prefer matches in the file name part of a path.
    let name_start = cand.rfind('/').map(|b| cand[..b + 1].chars().count()).unwrap_or(0);

    for (i, &c) in lower.iter().enumerate() {
        if qi == q.len() {
            break;
        }
        if c != q[qi] {
            continue;
        }
        let mut s = 1;
        let boundary = i == 0 || matches!(chars[i - 1], '/' | '_' | '-' | ' ' | '.') || (chars[i].is_uppercase() && chars[i - 1].is_lowercase());
        if boundary {
            s += 8;
        }
        if prev == Some(i.wrapping_sub(1)) {
            s += 6;
        } else if let Some(p) = prev {
            s -= ((i - p) as i64).min(5);
        }
        if i >= name_start {
            s += 3;
        }
        score += s;
        positions.push(i);
        prev = Some(i);
        qi += 1;
    }
    if qi < q.len() {
        return None;
    }
    score -= (chars.len() as i64) / 8;
    Some((score, positions))
}

/// Filter and rank items; returns indices into `items` best-first.
pub fn rank<'a>(query: &str, items: impl Iterator<Item = &'a str>, limit: usize) -> Vec<(usize, Vec<usize>)> {
    let mut scored: Vec<(i64, usize, Vec<usize>)> = items
        .enumerate()
        .filter_map(|(i, s)| score(query, s).map(|(sc, pos)| (sc, i, pos)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.truncate(limit);
    scored.into_iter().map(|(_, i, p)| (i, p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence() {
        assert!(score("mrs", "src/main.rs").is_some());
        assert!(score("xyz", "src/main.rs").is_none());
    }

    #[test]
    fn prefers_file_name() {
        let r = rank("main", ["main/foo.rs", "src/main.rs"].into_iter(), 10);
        assert_eq!(r[0].0, 1);
    }
}
