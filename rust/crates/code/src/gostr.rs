//! Go string helpers whose results must match the Go app byte for byte.

use std::sync::LazyLock;

use regex::Regex;

/// Go `strings.Join(strings.Fields(s), " ")`.
pub(crate) fn collapse_space(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Go `utf8.RuneStart`.
pub(crate) fn rune_start(b: u8) -> bool {
    b & 0xC0 != 0x80
}

static UPPER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{Lu}$").expect("Lu"));
static LETTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{L}$").expect("L"));
static DIGIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{Nd}$").expect("Nd"));

fn in_class(re: &Regex, c: char) -> bool {
    let mut buf = [0u8; 4];
    re.is_match(c.encode_utf8(&mut buf))
}

/// Go `unicode.IsUpper` (category Lu).
pub(crate) fn is_upper(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_uppercase()
    } else {
        in_class(&UPPER, c)
    }
}

/// Go `unicode.IsLetter` (category L).
pub(crate) fn is_letter(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphabetic()
    } else {
        in_class(&LETTER, c)
    }
}

/// Go `unicode.IsDigit` (category Nd).
pub(crate) fn is_digit(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_digit()
    } else {
        in_class(&DIGIT, c)
    }
}

/// Go `s[:n]` as the UI receives it: a rune cut in half becomes one U+FFFD
/// per leftover byte (how `encoding/json` writes invalid UTF-8).
pub(crate) fn cut_bytes(s: &str, n: usize) -> String {
    if n >= s.len() {
        return s.to_string();
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = s[..end].to_string();
    out.extend(std::iter::repeat_n('\u{FFFD}', n - end));
    out
}

/// `strings.Join(strings.Fields(s), " ")` cut to `n` bytes with `…` (Go's
/// `s[:n-1] + "…"` when longer).
pub(crate) fn truncate_fields(s: &str, n: usize) -> String {
    let s = collapse_space(s);
    if s.len() <= n {
        return s;
    }
    cut_bytes(&s, n.saturating_sub(1)) + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_semantics() {
        assert_eq!(cut_bytes("a世", 2), "a\u{FFFD}");
        assert_eq!(cut_bytes("ab", 5), "ab");
        assert_eq!(truncate_fields(" a  b ", 10), "a b");
        assert_eq!(truncate_fields("abcdef", 4), "abc…");
    }
}
