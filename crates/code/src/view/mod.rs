//! The UI's pure file rules, so `recur-app` does not re-derive them:
//! `lib/file-index.ts`, `lib/changed-tree.ts`, `lib/finding-paths.ts` and
//! `lib/skills.ts`. (`lib/fuzzy.ts` and `lib/glob.ts` are
//! [`crate::fuzzy`] and [`crate::glob`].)

pub mod changed_tree;
pub mod file_index;
pub mod finding_paths;
pub mod skills;

/// JavaScript `parseInt(s, 10) || 0`: optional leading whitespace and sign,
/// then decimal digits; anything else is 0.
pub(crate) fn js_parse_int(s: &str) -> i64 {
    let s = s.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}');
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let end = digits.bytes().take_while(u8::is_ascii_digit).count();
    let n = digits[..end].parse::<i64>().unwrap_or(0);
    if neg { -n } else { n }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parse_int_like_js() {
        use super::js_parse_int;
        assert_eq!(js_parse_int("12"), 12);
        assert_eq!(js_parse_int(" 7x"), 7);
        assert_eq!(js_parse_int("-3"), -3);
        assert_eq!(js_parse_int(""), 0);
        assert_eq!(js_parse_int("x1"), 0);
    }
}
