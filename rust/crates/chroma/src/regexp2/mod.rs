//! A port of the parts of `github.com/dlclark/regexp2` chroma uses: .NET
//! regular-expression syntax and its backtracking semantics (variable-length
//! lookbehind, named groups, backreferences, inline options), so lexer rules
//! match exactly what they matched in Go.

mod charclass;
mod runner;
mod syntax;
mod unicode;
#[allow(clippy::all)]
mod unicode_tables;

use std::collections::HashMap;
use std::time::{Duration, Instant};

pub(crate) use runner::Timeout;
pub(crate) use syntax::ParseError;
pub use unicode::{is_letter, is_number, to_upper};

/// A compiled pattern (regexp2 `Regexp` with no options).
pub(crate) struct Regex {
    pattern: String,
    code: syntax::Code,
    capslist: Option<Vec<String>>,
    /// Bounds each match attempt; chroma uses 250ms for lexer rules.
    pub timeout: Option<Duration>,
}

impl std::fmt::Debug for Regex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Regex").field(&self.pattern).finish()
    }
}

/// One match: every group's last capture, in group order, with its name.
pub(crate) struct Match {
    pub index: usize,
    pub groups: Vec<(String, Option<(usize, usize)>)>,
}

impl Match {
    /// The text of group `i` (empty when it did not participate).
    pub(crate) fn text(&self, text: &[char], i: usize) -> String {
        match self.groups.get(i).and_then(|g| g.1) {
            Some((start, len)) => text[start..start + len].iter().collect(),
            None => String::new(),
        }
    }
}

impl Regex {
    pub(crate) fn new(pattern: &str) -> Result<Regex, ParseError> {
        let tree = syntax::parse(pattern)?;
        let code = syntax::write(&tree)?;
        Ok(Regex {
            pattern: pattern.to_string(),
            code,
            capslist: tree.caplist.clone(),
            timeout: None,
        })
    }

    /// Matches starting exactly at `start` (chroma accepts a rule only when
    /// `FindRunesMatchStartingAt(text, start)` begins at `start`).
    pub(crate) fn match_at(&self, text: &[char], start: usize) -> Result<Option<Match>, Timeout> {
        let deadline = self.timeout.map(|t| Instant::now() + t);
        let mut r = runner::Runner::new(&self.code, text, deadline);
        if r.attempt(start)? {
            Ok(Some(self.finish(&mut r, start)))
        } else {
            Ok(None)
        }
    }

    /// The first match at or after `start` (regexp2 `FindRunesMatchStartingAt`).
    pub(crate) fn find_from(&self, text: &[char], start: usize) -> Result<Option<Match>, Timeout> {
        let deadline = self.timeout.map(|t| Instant::now() + t);
        let mut r = runner::Runner::new(&self.code, text, deadline);
        for pos in start..=text.len() {
            if r.attempt(pos)? {
                return Ok(Some(self.finish(&mut r, pos)));
            }
        }
        Ok(None)
    }

    fn finish(&self, r: &mut runner::Runner<'_>, start: usize) -> Match {
        r.caps.tidy();
        let groups = (0..r.caps.matchcount.len())
            .map(|i| {
                let c = r.caps.matchcount[i];
                let span = (c > 0).then(|| {
                    let m = &r.caps.matches[i];
                    (m[(c - 1) * 2] as usize, m[c * 2 - 1] as usize)
                });
                (self.group_name_from_number(i as i32), span)
            })
            .collect();
        Match {
            index: start,
            groups,
        }
    }

    /// regexp2 `GroupNameFromNumber`.
    pub(crate) fn group_name_from_number(&self, i: i32) -> String {
        let Some(list) = &self.capslist else {
            return if i >= 0 && (i as usize) < self.code.capsize {
                i.to_string()
            } else {
                String::new()
            };
        };
        let slot = match &self.code.caps {
            Some(caps) => match caps.get(&i) {
                Some(&s) => s,
                None => return String::new(),
            },
            None => i,
        };
        usize::try_from(slot)
            .ok()
            .and_then(|s| list.get(s))
            .cloned()
            .unwrap_or_default()
    }

    /// `len(GetGroupNumbers())`: the number of groups including group 0.
    pub(crate) fn group_count(&self) -> usize {
        self.code
            .caps
            .as_ref()
            .map_or(self.code.capsize, HashMap::len)
    }
}

/// regexp2 `Escape`: backslashes before metacharacters, escapes for
/// non-printable characters.
pub(crate) fn escape(input: &str) -> String {
    const META: &str = "\\.+*?()|[]{}^$# ";
    let mut b = String::new();
    for r in input.chars() {
        if unicode::is_print(r) {
            if META.contains(r) {
                b.push('\\');
            }
            b.push(r);
            continue;
        }
        match r {
            '\u{7}' => b.push_str("\\a"),
            '\u{C}' => b.push_str("\\f"),
            '\n' => b.push_str("\\n"),
            '\r' => b.push_str("\\r"),
            '\t' => b.push_str("\\t"),
            '\u{B}' => b.push_str("\\v"),
            r if (r as u32) < 0x100 => b.push_str(&format!("\\x{:02x}", r as u32)),
            r => b.push_str(&format!("\\u{:x}", r as u32)),
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn m(pattern: &str, text: &str, start: usize) -> Option<Vec<String>> {
        let re = Regex::new(pattern).unwrap_or_else(|e| panic!("{pattern}: {e:?}"));
        let t = chars(text);
        re.match_at(&t, start)
            .ok()
            .flatten()
            .map(|m| (0..m.groups.len()).map(|i| m.text(&t, i)).collect())
    }

    #[test]
    fn literals_classes_and_quantifiers() {
        assert_eq!(m("abc", "abcd", 0), Some(vec!["abc".into()]));
        assert_eq!(m("a+", "aaab", 0), Some(vec!["aaa".into()]));
        assert_eq!(m("a+?", "aaab", 0), Some(vec!["a".into()]));
        assert_eq!(m("[a-c]{2,3}", "cbad", 0), Some(vec!["cba".into()]));
        assert_eq!(m("x{2}", "xxx", 0), Some(vec!["xx".into()]));
        assert_eq!(m(r"\d+", "x12", 1), Some(vec!["12".into()]));
        assert_eq!(m(r"\d+", "x12", 0), None);
        assert_eq!(m("[]a]+", "]a]b", 0), Some(vec!["]a]".into()]));
        assert_eq!(m("[^]]+", "ab]", 0), Some(vec!["ab".into()]));
        assert_eq!(m(r"[~()[\]]+", "([~])x", 0), Some(vec!["([~])".into()]));
        assert_eq!(m(r"[a-z-[aeiou]]+", "bcdea", 0), Some(vec!["bcd".into()]));
        assert_eq!(m(r"\<\>", "<>", 0), Some(vec!["<>".into()]));
    }

    #[test]
    fn groups_alternation_and_backrefs() {
        assert_eq!(
            m("(a|ab)(c|bcd)", "abcd", 0),
            Some(vec!["abcd".into(), "a".into(), "bcd".into()])
        );
        assert_eq!(m(r"(\w)\1", "aab", 0), Some(vec!["aa".into(), "a".into()]));
        assert_eq!(
            m(r#"(?<q>['"]).*?\k<q>"#, r#"'a"b'c"#, 0),
            Some(vec![r#"'a"b'"#.into(), "'".into()])
        );
        assert_eq!(m("(x)?y", "y", 0), Some(vec!["y".into(), String::new()]));
        let re = Regex::new(r"(?<level>=*)(\k<level>)").unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(re.group_name_from_number(1), "1");
        assert_eq!(re.group_name_from_number(2), "level");
        assert_eq!(re.group_count(), 3);
    }

    #[test]
    fn anchors_and_options() {
        assert_eq!(m("(?m)^b$", "a\nb\nc", 2), Some(vec!["b".into()]));
        assert_eq!(m("^b", "a\nb", 2), None);
        assert_eq!(m(r"a\Z", "a\n", 0), Some(vec!["a".into()]));
        assert_eq!(m("a$", "a\n", 0), Some(vec!["a".into()]));
        assert_eq!(m(r"a\z", "a\n", 0), None);
        assert_eq!(m(r"\Ga", "ba", 1), Some(vec!["a".into()]));
        assert_eq!(m("(?i)hello", "HeLLo", 0), Some(vec!["HeLLo".into()]));
        assert_eq!(m("(?i:k)", "\u{212A}", 0), Some(vec!["\u{212A}".into()]));
        assert_eq!(m("(?s)a.b", "a\nb", 0), Some(vec!["a\nb".into()]));
        assert_eq!(m("a.b", "a\nb", 0), None);
        assert_eq!(
            m("(?x) a b # comment\n c", "abc", 0),
            Some(vec!["abc".into()])
        );
        assert_eq!(m(r"\bfoo\b", "foo bar", 0), Some(vec!["foo".into()]));
        assert_eq!(m(r"\bfoo\b", "foobar", 0), None);
    }

    #[test]
    fn lookarounds_including_variable_lookbehind() {
        assert_eq!(m(r"foo(?=bar)", "foobar", 0), Some(vec!["foo".into()]));
        assert_eq!(m(r"foo(?!bar)", "foobar", 0), None);
        assert_eq!(
            m(r"(?<=for\s+)\w+", "for   item", 6),
            Some(vec!["item".into()])
        );
        assert_eq!(m(r"(?<!:\s*)x", "a:  x", 4), None);
        assert_eq!(m(r"(?<!:\s*)x", "a   x", 4), Some(vec!["x".into()]));
        assert_eq!(
            m(r"(?<=\$if.+?(?:&&|\|\|)?)(linux)", "$if foo linux", 8),
            Some(vec!["linux".into(), "linux".into()])
        );
        assert_eq!(m(r"(?=:if +)?\[", "[", 0), Some(vec!["[".into()]));
    }

    #[test]
    fn lazy_loops_and_empty_iterations() {
        assert_eq!(
            m(r"(a*)*b", "aab", 0),
            Some(vec!["aab".into(), String::new()])
        );
        assert_eq!(
            m(r"(a|)+b", "aab", 0),
            Some(vec!["aab".into(), String::new()])
        );
        assert_eq!(m(r"(?:ab){2,3}?", "ababab", 0), Some(vec!["abab".into()]));
        assert_eq!(
            m(r"/\*(.|\n)*?\*/", "/* a\n*/ b */", 0),
            Some(vec!["/* a\n*/".into(), "\n".into()])
        );
        assert_eq!(m(r"\d+i", "12i", 0), Some(vec!["12i".into()]));
    }

    #[test]
    fn find_from_scans_forward() {
        let re = Regex::new(r"=end\s+pod").unwrap_or_else(|e| panic!("{e:?}"));
        let t = chars("x =end  pod y");
        let found = re.find_from(&t, 0).ok().flatten().map(|m| m.index);
        assert_eq!(found, Some(2));
    }

    #[test]
    fn escape_matches_regexp2() {
        assert_eq!(escape("a.b*c"), r"a\.b\*c");
        assert_eq!(escape("x y#"), r"x\ y\#");
        assert_eq!(escape("\t\u{1}"), "\\t\\x01");
        assert_eq!(escape("«»"), "«»");
    }

    #[test]
    fn invalid_patterns_error() {
        assert!(Regex::new(r"\h").is_err());
        assert!(Regex::new("a**").is_err());
        assert!(Regex::new("(a").is_err());
        assert!(Regex::new("[a").is_err());
    }
}
