//! Search options shared by the find panel and Find in Files. Plain-text
//! searches are compiled to escaped regexes, so there is one matching engine.

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Search {
    pub pattern: String,
    pub case: bool,
    pub regex: bool,
    pub whole_word: bool,
}

#[derive(Clone)]
pub struct Matcher {
    pub re: Regex,
    /// Expand `$1`, `${name}` in replacements (regex mode only).
    expand: bool,
}

impl Search {
    pub fn compile(&self) -> Result<Matcher, String> {
        if self.pattern.is_empty() {
            return Err(String::new());
        }
        let mut pat = if self.regex { self.pattern.clone() } else { regex::escape(&self.pattern) };
        if self.whole_word {
            pat = format!(r"\b(?:{pat})\b");
        }
        RegexBuilder::new(&pat)
            .case_insensitive(!self.case)
            .multi_line(true)
            .size_limit(1 << 24)
            .build()
            .map(|re| Matcher { re, expand: self.regex })
            .map_err(|e| match e {
                regex::Error::Syntax(s) => s.lines().last().unwrap_or("invalid regex").trim().to_string(),
                e => e.to_string(),
            })
    }
}

impl Matcher {
    /// Byte ranges of all non-empty matches in `hay`.
    pub fn find_iter<'a>(&'a self, hay: &'a str) -> impl Iterator<Item = (usize, usize)> + 'a {
        self.re.find_iter(hay).filter(|m| !m.is_empty()).map(|m| (m.start(), m.end()))
    }

    /// The replacement for the match at `range` in `hay`.
    pub fn replacement(&self, hay: &str, range: (usize, usize), repl: &str) -> String {
        if !self.expand {
            return repl.to_string();
        }
        match self.re.captures_at(hay, range.0) {
            Some(caps) if caps.get(0).is_some_and(|m| m.start() == range.0 && m.end() == range.1) => {
                let mut out = String::new();
                caps.expand(repl, &mut out);
                out
            }
            _ => repl.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(pattern: &str, regex: bool, case: bool, whole_word: bool) -> Search {
        Search { pattern: pattern.into(), regex, case, whole_word }
    }

    #[test]
    fn plain_text_is_escaped() {
        let m = s("a.b", false, true, false).compile().unwrap();
        assert_eq!(m.find_iter("a.b axb").collect::<Vec<_>>(), vec![(0, 3)]);
    }

    #[test]
    fn case_and_whole_word() {
        let m = s("foo", false, false, true).compile().unwrap();
        assert_eq!(m.find_iter("Foo food foo").count(), 2);
    }

    #[test]
    fn regex_replacement_expands_groups() {
        let m = s(r"(\w+)=(\d+)", true, true, false).compile().unwrap();
        let hay = "x=1 y=22";
        let r: Vec<_> = m.find_iter(hay).map(|r| m.replacement(hay, r, "$2:$1")).collect();
        assert_eq!(r, vec!["1:x", "22:y"]);
    }

    #[test]
    fn invalid_regex_reports_error() {
        assert!(s("(", true, true, false).compile().is_err());
        assert!(s("", false, true, false).compile().is_err());
    }
}
