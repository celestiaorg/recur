//! Helpers from vscode-textmate's `utils.ts` plus the JS built-ins its code
//! relies on (`parseInt`, `\s`).

use crate::onig_lib::CaptureIndex;

/// JS `\s` (WhiteSpace + LineTerminator).
pub fn js_is_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// JS `parseInt(s, 10)`: leading whitespace, optional sign, leading digits.
pub fn js_parse_int(s: &str) -> Option<i64> {
    let t = s.trim_start_matches(js_is_space);
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let n = digits.bytes().take_while(u8::is_ascii_digit).count();
    if n == 0 {
        return None;
    }
    let v = digits[..n].bytes().fold(0i64, |acc, b| {
        acc.saturating_mul(10).saturating_add(i64::from(b - b'0'))
    });
    Some(if neg { -v } else { v })
}

/// One `$1` / `${1:/downcase}` reference in a rule name.
struct CaptureRef {
    start: usize,
    end: usize,
    index: usize,
    command: Option<&'static str>,
}

/// Matches of `/\$(\d+)|\${(\d+):\/(downcase|upcase)}/g`.
fn capture_refs(src: &str) -> Vec<CaptureRef> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'$' {
            i += 1;
            continue;
        }
        let digits = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 {
            let index = src[i + 1..i + 1 + digits].parse().unwrap_or(usize::MAX);
            out.push(CaptureRef {
                start: i,
                end: i + 1 + digits,
                index,
                command: None,
            });
            i += 1 + digits;
            continue;
        }
        if b.get(i + 1) == Some(&b'{') {
            let d = b[i + 2..].iter().take_while(|c| c.is_ascii_digit()).count();
            if d > 0 {
                let after = &src[i + 2 + d..];
                let found = ["downcase", "upcase"].into_iter().find_map(|cmd| {
                    let tail = format!(":/{cmd}}}");
                    after.starts_with(&tail).then_some((cmd, tail.len()))
                });
                if let Some((cmd, tail_len)) = found {
                    let end = i + 2 + d + tail_len;
                    out.push(CaptureRef {
                        start: i,
                        end,
                        index: src[i + 2..i + 2 + d].parse().unwrap_or(usize::MAX),
                        command: Some(cmd),
                    });
                    i = end;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

pub fn has_captures(src: Option<&str>) -> bool {
    src.is_some_and(|s| !capture_refs(s).is_empty())
}

/// `RegexSource.replaceCaptures`.
pub fn replace_captures(src: &str, capture_source: &str, captures: &[CaptureIndex]) -> String {
    let mut out = String::with_capacity(src.len());
    let mut last = 0;
    for r in capture_refs(src) {
        out.push_str(&src[last..r.start]);
        last = r.end;
        let Some(cap) = captures.get(r.index) else {
            out.push_str(&src[r.start..r.end]);
            continue;
        };
        let mut result = capture_source.get(cap.start..cap.end).unwrap_or("");
        while let Some(rest) = result.strip_prefix('.') {
            result = rest;
        }
        match r.command {
            Some("downcase") => out.push_str(&result.to_lowercase()),
            Some("upcase") => out.push_str(&result.to_uppercase()),
            _ => out.push_str(result),
        }
    }
    out.push_str(&src[last..]);
    out
}

/// Escapes `[\-\\\{\}\*\+\?\|\^\$\.\,\[\]\(\)\#\s]`.
pub fn escape_reg_exp_characters(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if "-\\{}*+?|^$.,[]()#".contains(c) || js_is_space(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub fn is_valid_hex_color(hex: &str) -> bool {
    let Some(digits) = hex.strip_prefix('#') else {
        return false;
    };
    matches!(digits.len(), 3 | 4 | 6 | 8) && digits.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Unicode "R" / "AL" characters (vscode-textmate `containsRTL`). Lines with
/// any of them keep consecutive equal-metadata tokens unmerged.
pub fn contains_rtl(s: &str) -> bool {
    s.chars().any(|c| {
        let c = c as u32;
        RTL_RANGES.iter().any(|&(lo, hi)| c >= lo && c <= hi)
    })
}

const RTL_RANGES: &[(u32, u32)] = &[
    (0x05BE, 0x05BE),
    (0x05C0, 0x05C0),
    (0x05C3, 0x05C3),
    (0x05C6, 0x05C6),
    (0x05D0, 0x05F4),
    (0x0608, 0x0608),
    (0x060B, 0x060B),
    (0x060D, 0x060D),
    (0x061B, 0x064A),
    (0x066D, 0x066F),
    (0x0671, 0x06D5),
    (0x06E5, 0x06E6),
    (0x06EE, 0x06EF),
    (0x06FA, 0x0710),
    (0x0712, 0x072F),
    (0x074D, 0x07A5),
    (0x07B1, 0x07EA),
    (0x07F4, 0x07F5),
    (0x07FA, 0x07FA),
    (0x07FE, 0x0815),
    (0x081A, 0x081A),
    (0x0824, 0x0824),
    (0x0828, 0x0828),
    (0x0830, 0x0858),
    (0x085E, 0x088E),
    (0x08A0, 0x08C9),
    (0x200F, 0x200F),
    (0xFB1D, 0xFB1D),
    (0xFB1F, 0xFB28),
    (0xFB2A, 0xFD3D),
    (0xFD50, 0xFDC7),
    (0xFDF0, 0xFDFC),
    (0xFE70, 0xFEFC),
    (0x10800, 0x1091B),
    (0x10920, 0x10A00),
    (0x10A10, 0x10A35),
    (0x10A40, 0x10AE4),
    (0x10AEB, 0x10B35),
    (0x10B40, 0x10BFF),
    (0x10C00, 0x10D23),
    (0x10E80, 0x10EA9),
    (0x10EAD, 0x10F45),
    (0x10F51, 0x10F81),
    (0x10F86, 0x10FF6),
    (0x1E800, 0x1E8CF),
    (0x1E900, 0x1E943),
    (0x1E94B, 0x1EBFF),
    (0x1EC00, 0x1EEBB),
];

/// UTF-16 length, the unit JS string lengths are measured in.
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(start: usize, end: usize) -> CaptureIndex {
        CaptureIndex { start, end }
    }

    #[test]
    fn parse_int_matches_js() {
        assert_eq!(js_parse_int("12"), Some(12));
        assert_eq!(js_parse_int(" 7abc"), Some(7));
        assert_eq!(js_parse_int("-3"), Some(-3));
        assert_eq!(js_parse_int("x1"), None);
        assert_eq!(js_parse_int(""), None);
    }

    #[test]
    fn replaces_numbered_and_case_captures() {
        let line = "foo.Bar";
        let caps = [cap(0, 7), cap(0, 3), cap(3, 7)];
        assert!(has_captures(Some("x.$1")));
        assert!(!has_captures(Some("plain")));
        assert_eq!(replace_captures("a.$1.$2", line, &caps), "a.foo.Bar");
        assert_eq!(replace_captures("${2:/downcase}", line, &caps), "bar");
        assert_eq!(replace_captures("${1:/upcase}", line, &caps), "FOO");
        assert_eq!(replace_captures("$9", line, &caps), "$9");
    }

    #[test]
    fn escapes_regex_characters() {
        assert_eq!(escape_reg_exp_characters("a.b*c d"), "a\\.b\\*c\\ d");
    }

    #[test]
    fn hex_colors() {
        assert!(is_valid_hex_color("#abc"));
        assert!(is_valid_hex_color("#AABBCCDD"));
        assert!(!is_valid_hex_color("#abcde"));
        assert!(!is_valid_hex_color("red"));
    }

    #[test]
    fn rtl_detection() {
        assert!(contains_rtl("abc \u{05D0}"));
        assert!(contains_rtl("\u{10800}"));
        assert!(!contains_rtl("hello"));
    }
}
