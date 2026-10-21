//! Go string helpers whose results must match the Go app byte for byte.

use std::sync::LazyLock;

use regex::Regex;

static PRINTABLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\p{L}\p{M}\p{N}\p{P}\p{S} ]$").expect("printable class"));

/// Go `strconv.Quote` (what `%q` prints for a string).
pub(crate) fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
            out.push(c);
            continue;
        }
        let mut buf = [0u8; 4];
        if PRINTABLE.is_match(c.encode_utf8(&mut buf)) {
            out.push(c);
            continue;
        }
        match c {
            '\u{07}' => out.push_str("\\a"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0B}' => out.push_str("\\v"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push('"');
    out
}

/// Go `strings.ToLower`: per-rune simple case mapping.
pub(crate) fn lower(s: &str) -> String {
    s.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

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

/// Go `unicode.ToLower` for one rune (simple mapping).
pub(crate) fn lower_rune(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Go `for _, r := range string(b)`: invalid bytes decode as U+FFFD, one per
/// byte.
pub(crate) fn go_chars(b: &[u8]) -> impl Iterator<Item = char> + '_ {
    let mut chunks = b.utf8_chunks();
    let mut valid = "".chars();
    let mut invalid: &[u8] = &[];
    std::iter::from_fn(move || {
        loop {
            if let Some(c) = valid.next() {
                return Some(c);
            }
            if let Some((_, rest)) = invalid.split_first() {
                invalid = rest;
                return Some('\u{FFFD}');
            }
            let chunk = chunks.next()?;
            valid = chunk.valid().chars();
            invalid = chunk.invalid();
        }
    })
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
        let got: String = go_chars(b"a\xe4\xb8b").collect();
        assert_eq!(got, "a\u{FFFD}\u{FFFD}b");
        assert_eq!(cut_bytes("a世", 2), "a\u{FFFD}");
        assert_eq!(cut_bytes("ab", 5), "ab");
        assert_eq!(truncate_fields(" a  b ", 10), "a b");
        assert_eq!(truncate_fields("abcdef", 4), "abc…");
        assert_eq!(quote("a\"\n\u{1}é"), r#""a\"\n\x01é""#);
    }
}
