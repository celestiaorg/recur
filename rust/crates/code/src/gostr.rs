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
