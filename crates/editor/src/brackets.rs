//! Bracket matching (`@codemirror/language` `matchBrackets` and the
//! `bracketMatching` highlight). Without a syntax tree every match is a
//! plain character scan over `()[]{}`.

use crate::state::EditorState;

pub const DEFAULT_SCAN_DIST: usize = 10_000;
pub const DEFAULT_BRACKETS: &str = "()[]{}";

/// A bracket and, when found, its partner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BracketMatch {
    pub start: (usize, usize),
    pub end: Option<(usize, usize)>,
    pub matched: bool,
}

/// `matchBrackets(state, pos, dir)`: the bracket before (`dir < 0`) or
/// after (`dir > 0`) `pos` and its match.
pub fn match_brackets(state: &EditorState, pos: usize, dir: i32) -> Option<BracketMatch> {
    match_plain_brackets(state, pos, dir, DEFAULT_SCAN_DIST, DEFAULT_BRACKETS)
}

fn match_plain_brackets(
    state: &EditorState,
    pos: usize,
    dir: i32,
    max_scan: usize,
    brackets: &str,
) -> Option<BracketMatch> {
    let doc = &state.doc;
    if if dir < 0 { pos == 0 } else { pos == doc.len() } {
        return None;
    }
    let start_ch = if dir < 0 {
        doc.char_before(pos)?
    } else {
        doc.char_at(pos)?
    };
    let bracket = brackets.find(start_ch)?;
    if (bracket % 2 == 0) != (dir > 0) {
        return None;
    }
    let start = if dir < 0 {
        (pos - 1, pos)
    } else {
        (pos, pos + 1)
    };
    let bytes = brackets.as_bytes();
    let mut depth = 0i32;
    let mut distance = 0usize;
    let line = doc.line_at(pos);
    let mut chunks: Box<dyn Iterator<Item = (usize, String)>> = if dir > 0 {
        let first = std::iter::once((pos, doc.slice(pos, line.to)));
        let rest = (line.number + 1..=doc.lines()).flat_map(move |n| {
            let l = doc.line(n);
            [
                (l.from - 1, "\n".to_string()),
                (l.from, doc.slice(l.from, l.to)),
            ]
        });
        Box::new(first.chain(rest))
    } else {
        let first = std::iter::once((line.from, doc.slice(line.from, pos)));
        let rest = (1..line.number).rev().flat_map(move |n| {
            let l = doc.line(n);
            [(l.to, "\n".to_string()), (l.from, doc.slice(l.from, l.to))]
        });
        Box::new(first.chain(rest))
    };
    loop {
        let Some((base, text)) = chunks.next() else {
            return Some(BracketMatch {
                start,
                end: None,
                matched: false,
            });
        };
        if distance > max_scan {
            return None;
        }
        if dir < 0 {
            distance += text.len();
        }
        let tb = text.as_bytes();
        let positions: Box<dyn Iterator<Item = usize>> = if dir > 0 {
            Box::new(0..tb.len())
        } else {
            Box::new((0..tb.len()).rev())
        };
        for i in positions {
            let Some(found) = bytes.iter().position(|&b| b == tb[i]) else {
                continue;
            };
            if (found % 2 == 0) == (dir > 0) {
                depth += 1;
            } else if depth == 1 {
                let at = base + i;
                return Some(BracketMatch {
                    start,
                    end: Some((at, at + 1)),
                    matched: found >> 1 == bracket >> 1,
                });
            } else {
                depth -= 1;
            }
        }
        if dir > 0 {
            distance += text.len();
        }
    }
}

/// `bracketDeco`: `(from, to, matched)` marks (`cm-matchingBracket` /
/// `cm-nonmatchingBracket`) for brackets beside each cursor.
pub fn bracket_highlights(state: &EditorState) -> Vec<(usize, usize, bool)> {
    let mut out = Vec::new();
    for range in &state.selection.ranges {
        if !range.is_empty() {
            continue;
        }
        let head = range.head();
        let found = match_brackets(state, head, -1)
            .or_else(|| {
                (head > 0)
                    .then(|| match_brackets(state, head - 1, 1))
                    .flatten()
            })
            .or_else(|| match_brackets(state, head, 1))
            .or_else(|| {
                (head < state.doc.len())
                    .then(|| match_brackets(state, head + 1, -1))
                    .flatten()
            });
        if let Some(m) = found {
            out.push((m.start.0, m.start.1, m.matched));
            if let Some(e) = m.end {
                out.push((e.0, e.1, m.matched));
            }
        }
    }
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{StateConfig, TransactionSpec};

    fn st(doc: &str, pos: usize) -> EditorState {
        let s = EditorState::create(doc, StateConfig::default());
        s.update(TransactionSpec::default().cursor(pos))
            .state()
            .clone()
    }

    #[test]
    fn matches_across_lines_and_reports_mismatches() {
        let s = st("f(a, [b]\n)", 0);
        assert_eq!(
            match_brackets(&s, 1, 1),
            Some(BracketMatch {
                start: (1, 2),
                end: Some((9, 10)),
                matched: true
            })
        );
        assert_eq!(match_brackets(&s, 10, -1).unwrap().end, Some((1, 2)));
        let s = st("(]", 0);
        assert!(!match_brackets(&s, 0, 1).unwrap().matched);
        let s = st("(", 0);
        assert_eq!(match_brackets(&s, 0, 1).unwrap().end, None);
        assert_eq!(match_brackets(&s, 0, -1), None);
    }

    #[test]
    fn highlights_brackets_beside_the_cursor() {
        let s = st("x(y)", 1);
        assert_eq!(bracket_highlights(&s), vec![(1, 2, true), (3, 4, true)]);
        let s = st("x(y)", 4);
        assert_eq!(bracket_highlights(&s), vec![(1, 2, true), (3, 4, true)]);
        assert!(bracket_highlights(&st("xy", 1)).is_empty());
    }
}
