//! Immutable document text (CodeMirror's `Text`) over a rope. Positions are
//! UTF-8 byte offsets; lines are separated by `\n` only (input `\r\n` / `\r`
//! is normalized on the way in, like CodeMirror's default line splitting).

use std::fmt;

use ropey::{LineType, Rope};
use unicode_segmentation::GraphemeCursor;

const LF: LineType = LineType::LF;

#[derive(Clone, Default)]
pub struct Text(Rope);

/// A line of a [`Text`]: 1-based number and its byte range (excluding the
/// line break).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub number: usize,
    pub from: usize,
    pub to: usize,
}

impl Line {
    pub fn len(&self) -> usize {
        self.to - self.from
    }

    pub fn is_empty(&self) -> bool {
        self.to == self.from
    }
}

/// CodeMirror's default line splitting: `\r\n`, `\r` and `\n` all end lines.
pub fn normalize_newlines(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('\r') {
        return std::borrow::Cow::Borrowed(s);
    }
    std::borrow::Cow::Owned(s.replace("\r\n", "\n").replace('\r', "\n"))
}

impl Text {
    pub fn of(s: &str) -> Text {
        Text(Rope::from_str(&normalize_newlines(s)))
    }

    pub fn empty() -> Text {
        Text(Rope::new())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.len() == 0
    }

    /// Number of lines (at least 1).
    pub fn lines(&self) -> usize {
        self.0.len_lines(LF)
    }

    /// 1-based line `n`.
    pub fn line(&self, n: usize) -> Line {
        let n = n.clamp(1, self.lines());
        let from = self.0.line_to_byte_idx(n - 1, LF);
        let to = if n < self.lines() {
            self.0.line_to_byte_idx(n, LF) - 1
        } else {
            self.0.len()
        };
        Line {
            number: n,
            from,
            to,
        }
    }

    pub fn line_at(&self, pos: usize) -> Line {
        let pos = pos.min(self.len());
        self.line(self.0.byte_to_line_idx(pos, LF) + 1)
    }

    pub fn line_text(&self, n: usize) -> String {
        let l = self.line(n);
        self.slice(l.from, l.to)
    }

    pub fn slice(&self, from: usize, to: usize) -> String {
        let to = to.min(self.len());
        let from = from.min(to);
        self.0.slice(from..to).to_string()
    }

    pub fn replace(&self, from: usize, to: usize, insert: &str) -> Text {
        let mut r = self.0.clone();
        if to > from {
            r.remove(from..to);
        }
        if !insert.is_empty() {
            r.insert(from, insert);
        }
        Text(r)
    }

    pub fn rope(&self) -> &Rope {
        &self.0
    }

    /// Character starting at `pos`, if any.
    pub fn char_at(&self, pos: usize) -> Option<char> {
        (pos < self.len()).then(|| self.0.char(pos))
    }

    /// Character ending at `pos`, if any.
    pub fn char_before(&self, pos: usize) -> Option<char> {
        if pos == 0 || pos > self.len() {
            return None;
        }
        let start = self.0.floor_char_boundary(pos - 1);
        Some(self.0.char(start))
    }

    /// Chars from `pos` forward, with their byte offsets.
    pub fn chars_from(&self, pos: usize) -> impl Iterator<Item = (usize, char)> + '_ {
        let pos = pos.min(self.len());
        self.0.char_indices_at(pos)
    }

    /// Chars before `pos`, nearest first, with their byte offsets.
    pub fn chars_before(&self, pos: usize) -> impl Iterator<Item = (usize, char)> + '_ {
        let mut at = pos.min(self.len());
        std::iter::from_fn(move || {
            if at == 0 {
                return None;
            }
            let start = self.0.floor_char_boundary(at - 1);
            at = start;
            Some((start, self.0.char(start)))
        })
    }

    pub fn utf16_len(&self) -> usize {
        self.0.len_utf16()
    }

    pub fn byte_to_utf16(&self, pos: usize) -> usize {
        self.0.byte_to_utf16_idx(pos.min(self.len()))
    }

    pub fn utf16_to_byte(&self, pos: usize) -> usize {
        self.0.utf16_to_byte_idx(pos.min(self.utf16_len()))
    }

    pub fn clamp_to_char(&self, pos: usize) -> usize {
        self.0.floor_char_boundary(pos.min(self.len()))
    }
}

impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for chunk in self.0.chunks() {
            f.write_str(chunk)?;
        }
        Ok(())
    }
}

impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Text({:?})", self.to_string())
    }
}

impl PartialEq for Text {
    fn eq(&self, other: &Text) -> bool {
        self.0 == other.0
    }
}

/// The next grapheme cluster boundary after (`forward`) or before `pos` in
/// `s` (`findClusterBreak`). Returns `pos` when there is none.
pub fn find_cluster_break(s: &str, pos: usize, forward: bool) -> usize {
    let pos = pos.min(s.len());
    let mut c = GraphemeCursor::new(pos, s.len(), true);
    let r = if forward {
        c.next_boundary(s, 0)
    } else {
        c.prev_boundary(s, 0)
    };
    match r {
        Ok(Some(b)) => b,
        _ => pos,
    }
}

/// Column of byte offset `to` in `s`, expanding tabs (`countColumn`).
pub fn count_column(s: &str, tab_size: usize, to: usize) -> usize {
    let mut n = 0;
    let mut i = 0;
    let to = to.min(s.len());
    while i < to {
        if s.as_bytes()[i] == b'\t' {
            n += tab_size - (n % tab_size);
            i += 1;
        } else {
            n += 1;
            i = find_cluster_break(s, i, true);
        }
    }
    n
}

/// Byte offset of column `col` in `s` (`findColumn`). Too-short strings
/// return their length, or `None` when `strict`.
pub fn find_column(s: &str, col: usize, tab_size: usize, strict: bool) -> Option<usize> {
    let mut i = 0;
    let mut n = 0;
    loop {
        if n >= col {
            return Some(i);
        }
        if i == s.len() {
            break;
        }
        n += if s.as_bytes()[i] == b'\t' {
            tab_size - (n % tab_size)
        } else {
            1
        };
        i = find_cluster_break(s, i, true);
    }
    if strict { None } else { Some(s.len()) }
}

/// Primary collation weight of an NFD character: class, then order within
/// it. Combining marks have none.
fn primary_weight(c: char) -> Option<(u8, u32)> {
    const PUNCT: &str = "_-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";
    if unicode_normalization::char::is_combining_mark(c) {
        return None;
    }
    if c.is_whitespace() {
        return Some((0, c as u32));
    }
    if let Some(i) = PUNCT.find(c) {
        return Some((1, i as u32));
    }
    if let Some(d) = c.to_digit(10) {
        return Some((3, d));
    }
    if c.is_numeric() {
        return Some((3, c as u32));
    }
    if c.is_alphabetic() {
        let lower = c.to_lowercase().next().unwrap_or(c);
        return Some((4, lower as u32));
    }
    Some((2, c as u32))
}

/// `String.prototype.localeCompare` with the root collation, as browsers
/// order completion labels and catalog names: punctuation before digits
/// before letters, letters case-insensitively first, then accents, then
/// lowercase before uppercase.
pub fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    use unicode_normalization::UnicodeNormalization;
    let da: Vec<char> = a.nfd().collect();
    let db: Vec<char> = b.nfd().collect();
    let primary = |s: &[char]| {
        s.iter()
            .filter_map(|&c| primary_weight(c))
            .collect::<Vec<_>>()
    };
    let ord = primary(&da).cmp(&primary(&db));
    if ord.is_ne() {
        return ord;
    }
    let accents = |s: &[char]| {
        s.iter()
            .filter(|&&c| unicode_normalization::char::is_combining_mark(c))
            .copied()
            .collect::<Vec<_>>()
    };
    let ord = accents(&da).cmp(&accents(&db));
    if ord.is_ne() {
        return ord;
    }
    let case = |s: &[char]| {
        s.iter()
            .filter(|c| c.is_alphabetic())
            .map(|c| c.is_uppercase())
            .collect::<Vec<_>>()
    };
    case(&da).cmp(&case(&db))
}

/// UTF-16 length of `s` (the unit CodeMirror columns count in).
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_order() {
        let mut v = vec!["b", "B", "a", "_x", "1", "A", "é", "e", "-y"];
        v.sort_by(|a, b| locale_compare(a, b));
        assert_eq!(v, vec!["_x", "-y", "1", "a", "A", "b", "B", "e", "é"]);
    }

    #[test]
    fn lines_and_offsets() {
        let t = Text::of("ab\r\ncd\rx\n");
        assert_eq!(t.to_string(), "ab\ncd\nx\n");
        assert_eq!(t.lines(), 4);
        assert_eq!(
            t.line(1),
            Line {
                number: 1,
                from: 0,
                to: 2
            }
        );
        assert_eq!(
            t.line(2),
            Line {
                number: 2,
                from: 3,
                to: 5
            }
        );
        assert_eq!(
            t.line(4),
            Line {
                number: 4,
                from: 8,
                to: 8
            }
        );
        assert_eq!(t.line_at(4).number, 2);
        assert_eq!(t.line_at(2).number, 1);
        assert_eq!(t.line_at(8).number, 4);
        assert_eq!(Text::of("").lines(), 1);
        assert_eq!(
            Text::of("").line(1),
            Line {
                number: 1,
                from: 0,
                to: 0
            }
        );
    }

    #[test]
    fn replace_and_slice() {
        let t = Text::of("hello world");
        let u = t.replace(6, 11, "there\nfriend");
        assert_eq!(u.to_string(), "hello there\nfriend");
        assert_eq!(t.to_string(), "hello world");
        assert_eq!(u.slice(6, 11), "there");
        assert_eq!(u.lines(), 2);
    }

    #[test]
    fn chars_around() {
        let t = Text::of("aé😀b");
        assert_eq!(t.char_at(1), Some('é'));
        assert_eq!(t.char_before(3), Some('é'));
        let back: Vec<char> = t.chars_before(t.len()).map(|(_, c)| c).collect();
        assert_eq!(back, ['b', '😀', 'é', 'a']);
        assert_eq!(t.byte_to_utf16(7), 4);
    }

    #[test]
    fn clusters_and_columns() {
        let s = "e\u{301}x";
        assert_eq!(find_cluster_break(s, 0, true), 3);
        assert_eq!(find_cluster_break(s, 3, false), 0);
        assert_eq!(count_column("\tab", 4, 3), 6);
        assert_eq!(find_column("\tab", 5, 4, false), Some(2));
        assert_eq!(find_column("ab", 5, 4, true), None);
    }
}
