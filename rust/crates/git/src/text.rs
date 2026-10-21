//! Go string semantics the port relies on: invalid UTF-8 decoding and
//! `os.PathError` style messages.

/// Bytes to a `String` the way Go turns invalid UTF-8 into runes: each
/// invalid byte becomes one U+FFFD (Rust's lossy decoding merges some runs).
pub(crate) fn go_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                out.push_str(s);
                return out;
            }
            Err(e) => {
                let (valid, after) = rest.split_at(e.valid_up_to());
                out.push_str(std::str::from_utf8(valid).unwrap_or_default());
                let bad = e.error_len().unwrap_or(after.len());
                for _ in 0..bad {
                    out.push('\u{FFFD}');
                }
                rest = &after[bad..];
            }
        }
    }
}

/// Go's `syscall.Errno` text: the C message with a lowercase first letter and
/// no Rust `(os error N)` suffix.
pub(crate) fn errno_text(e: &std::io::Error) -> String {
    let full = e.to_string();
    let msg = match full.rfind(" (os error ") {
        Some(i) if e.raw_os_error().is_some() => &full[..i],
        _ => full.as_str(),
    };
    let mut chars = msg.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Go `*os.PathError` text: `op path: errno`.
pub(crate) fn path_error(op: &str, path: &str, e: &std::io::Error) -> String {
    format!("{op} {path}: {}", errno_text(e))
}

/// Go `%q` for a string: double-quoted with Go escapes.
pub(crate) fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            c if is_go_printable(c) => out.push(c),
            c if (c as u32) < 0x80 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push('"');
    out
}

/// Approximates Go `strconv.IsPrint`: graphic characters and the ASCII space.
fn is_go_printable(c: char) -> bool {
    if c == ' ' {
        return true;
    }
    if c.is_control() || c.is_whitespace() {
        return false;
    }
    !matches!(c as u32, 0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB | 0xE000..=0xF8FF | 0xF0000..=0x10FFFF)
}

/// Go `strings.ToLower`: per-rune simple lowercase mapping.
pub(crate) fn go_lower(s: &str) -> String {
    s.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

/// Go `strings.EqualFold`: equal under simple Unicode case folding.
pub(crate) fn eq_fold(a: &str, b: &str) -> bool {
    let mut x = a.chars();
    let mut y = b.chars();
    loop {
        match (x.next(), y.next()) {
            (None, None) => return true,
            (Some(c), Some(d)) => {
                if c != d && !fold_eq(c, d) {
                    return false;
                }
            }
            _ => return false,
        }
    }
}

fn fold_eq(c: char, d: char) -> bool {
    let lower = |ch: char| ch.to_lowercase().next().unwrap_or(ch);
    let upper = |ch: char| ch.to_uppercase().next().unwrap_or(ch);
    lower(c) == lower(d) || upper(c) == upper(d)
}

/// Go `*strconv.NumError` text for a failed integer parse.
pub(crate) fn num_error(func: &str, s: &str, e: &std::num::ParseIntError) -> String {
    let reason = match e.kind() {
        std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow => {
            "value out of range"
        }
        _ => "invalid syntax",
    };
    format!("strconv.{func}: parsing {}: {reason}", quote(s))
}

/// Go `strconv.Atoi`.
pub(crate) fn atoi(s: &str) -> Option<i64> {
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_string_replaces_each_invalid_byte() {
        assert_eq!(go_string(b"a\xe2\x82b"), "a\u{FFFD}\u{FFFD}b");
        assert_eq!(go_string(b"ok \xff"), "ok \u{FFFD}");
        assert_eq!(go_string("é".as_bytes()), "é");
    }

    #[test]
    fn quote_matches_go() {
        assert_eq!(quote("a b"), "\"a b\"");
        assert_eq!(quote("we\nird"), "\"we\\nird\"");
        assert_eq!(quote("x\"y\\"), "\"x\\\"y\\\\\"");
        assert_eq!(quote("\u{1}"), "\"\\x01\"");
        assert_eq!(quote("café"), "\"café\"");
    }
}
