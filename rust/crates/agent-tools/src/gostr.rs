//! Go string formatting that reaches agents verbatim: `strconv.Quote` (`%q`)
//! and Go's byte-wise handling of invalid UTF-8.

use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};

/// Go `strconv.IsPrint`: letters, marks, numbers, punctuation, symbols and
/// the ASCII space.
fn is_print(c: char) -> bool {
    let r = c as u32;
    if r <= 0xFF {
        return (0x20..=0x7E).contains(&r) || (r >= 0xA1 && r != 0xAD);
    }
    matches!(
        c.general_category_group(),
        GeneralCategoryGroup::Letter
            | GeneralCategoryGroup::Mark
            | GeneralCategoryGroup::Number
            | GeneralCategoryGroup::Punctuation
            | GeneralCategoryGroup::Symbol
    )
}

/// Go `strconv.Quote` (the `%q` verb).
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ if is_print(c) => out.push(c),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            _ if (c as u32) < 0x20 || c as u32 == 0x7F => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            _ if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push('"');
    out
}

/// Bytes to text the way Go sees them: every byte of an invalid UTF-8
/// sequence becomes one U+FFFD (`utf8.DecodeRune` width 1), which is also what
/// `json.Marshal` writes for a Go string holding those bytes.
pub fn lossy(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        out.push_str(chunk.valid());
        for _ in chunk.invalid() {
            out.push('\u{FFFD}');
        }
    }
    out
}

/// Go `strings.ToLower`: per-rune `unicode.ToLower` (U+0130 maps to a plain
/// `i`; Rust's context-free full mapping would add a combining dot).
pub fn to_lower(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            let lower: Vec<char> = if c == '\u{130}' {
                vec!['i']
            } else {
                c.to_lowercase().collect()
            };
            lower
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_matches_go() {
        assert_eq!(
            quote("fatal\0é\u{200b}\t\"q\""),
            "\"fatal\\x00é\\u200b\\t\\\"q\\\"\""
        );
        assert_eq!(
            quote("\u{7f}\u{a0}\u{ad}\u{1F600}\u{E0001}"),
            "\"\\x7f\\u00a0\\u00ad😀\\U000e0001\""
        );
        assert_eq!(quote("a\u{7}\u{b}\\"), r#""a\a\v\\""#);
    }

    #[test]
    fn lossy_replaces_each_invalid_byte() {
        assert_eq!(
            lossy(b"caf\xe9 \xe2\x82\n"),
            "caf\u{FFFD} \u{FFFD}\u{FFFD}\n"
        );
    }
}
