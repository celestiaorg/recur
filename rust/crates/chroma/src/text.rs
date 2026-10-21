//! Go string semantics chroma and highlighting rely on: invalid UTF-8
//! decoding, `html` escaping and unescaping, `strings.ToLower` and
//! `filepath.Base`.

use crate::html_entities::{ENTITY, ENTITY2};

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

/// Go `html.EscapeString`: escapes `<`, `>`, `&`, `'` and `"`.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '\'' => out.push_str("&#39;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            c => out.push(c),
        }
    }
    out
}

const LONGEST_ENTITY_WITHOUT_SEMICOLON: usize = 6;

const REPLACEMENT_TABLE: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

fn entity(name: &[u8]) -> Option<char> {
    let name = std::str::from_utf8(name).ok()?;
    ENTITY
        .binary_search_by(|(k, _)| k.cmp(&name))
        .ok()
        .map(|i| ENTITY[i].1)
}

fn entity2(name: &[u8]) -> Option<[char; 2]> {
    let name = std::str::from_utf8(name).ok()?;
    ENTITY2
        .binary_search_by(|(k, _)| k.cmp(&name))
        .ok()
        .map(|i| ENTITY2[i].1)
}

/// Go `html.UnescapeString`: numeric references, the full HTML5 named set,
/// and legacy names without a trailing semicolon.
pub(crate) fn html_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut src = 0;
    while src < b.len() {
        if b[src] != b'&' {
            let next = b[src..]
                .iter()
                .position(|&c| c == b'&')
                .map_or(b.len(), |i| src + i);
            out.extend_from_slice(&b[src..next]);
            src = next;
            continue;
        }
        src += unescape_entity(&b[src..], &mut out);
    }
    String::from_utf8(out).unwrap_or_else(|e| go_string(e.as_bytes()))
}

/// Decodes the reference at the start of `s` (`s[0] == b'&'`) into `out` and
/// returns how many bytes of `s` it consumed.
fn unescape_entity(s: &[u8], out: &mut Vec<u8>) -> usize {
    let mut i = 1;
    if s.len() <= 1 {
        out.push(s[0]);
        return 1;
    }
    if s[i] == b'#' {
        if s.len() <= 3 {
            out.push(s[0]);
            return 1;
        }
        i += 1;
        let mut c = s[i];
        let hex = c == b'x' || c == b'X';
        if hex {
            i += 1;
        }
        let mut x: i32 = 0;
        while i < s.len() {
            c = s[i];
            i += 1;
            let digit = if hex {
                match c {
                    b'0'..=b'9' => Some(c - b'0'),
                    b'a'..=b'f' => Some(c - b'a' + 10),
                    b'A'..=b'F' => Some(c - b'A' + 10),
                    _ => None,
                }
            } else if c.is_ascii_digit() {
                Some(c - b'0')
            } else {
                None
            };
            if let Some(d) = digit {
                x = x
                    .wrapping_mul(if hex { 16 } else { 10 })
                    .wrapping_add(i32::from(d));
                continue;
            }
            if c != b';' {
                i -= 1;
            }
            break;
        }
        if i <= 3 {
            out.push(s[0]);
            return 1;
        }
        let ch = if (0x80..=0x9F).contains(&x) {
            REPLACEMENT_TABLE[(x - 0x80) as usize]
        } else if x == 0 || (0xD800..=0xDFFF).contains(&x) || x > 0x10FFFF {
            '\u{FFFD}'
        } else {
            u32::try_from(x)
                .ok()
                .and_then(char::from_u32)
                .unwrap_or('\u{FFFD}')
        };
        push_char(out, ch);
        return i;
    }
    while i < s.len() {
        let c = s[i];
        i += 1;
        if c.is_ascii_alphanumeric() {
            continue;
        }
        if c != b';' {
            i -= 1;
        }
        break;
    }
    let name = &s[1..i];
    if !name.is_empty() {
        if let Some(x) = entity(name) {
            push_char(out, x);
            return i;
        }
        if let Some([a, b]) = entity2(name) {
            push_char(out, a);
            push_char(out, b);
            return i;
        }
        let max_len = (name.len() - 1).min(LONGEST_ENTITY_WITHOUT_SEMICOLON);
        for j in (2..=max_len).rev() {
            if let Some(x) = entity(&name[..j]) {
                push_char(out, x);
                return j + 1;
            }
        }
    }
    out.extend_from_slice(&s[..i]);
    i
}

fn push_char(out: &mut Vec<u8>, c: char) {
    let mut buf = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

/// Go `strings.ToLower`: per-rune simple lowercase mapping.
pub(crate) fn go_lower(s: &str) -> String {
    s.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

/// `filepath.Base`.
pub(crate) fn base(p: &str) -> String {
    if p.is_empty() {
        return ".".to_string();
    }
    let p = p.trim_end_matches('/');
    if p.is_empty() {
        return "/".to_string();
    }
    match p.rfind('/') {
        Some(i) => p[i + 1..].to_string(),
        None => p.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_matches_go() {
        for (input, want) in [
            ("A\ttext\nstring", "A\ttext\nstring"),
            ("&amp; &gt; &lt;", "& > <"),
            ("&amp &amp", "& &"),
            ("text &gesl; blah", "text \u{22db}\u{fe00} blah"),
            ("Delta = &#916; ", "Delta = Δ "),
            ("Lambda = &#x3bb; = &#X3Bb ", "Lambda = λ = λ "),
            (
                "&# &#x &#128;43 &copy = &#169f = &#xa9",
                "&# &#x €43 © = ©f = ©",
            ),
            ("Footnote&#x87;", "Footnote‡"),
            ("&", "&"),
            ("text &test", "text &test"),
            ("text &#", "text &#"),
            ("&#9a;", "&#9a;"),
            ("&#0;", "\u{FFFD}"),
            ("&notit;", "¬it;"),
            ("&notin;", "∉"),
            ("&unknown;", "&unknown;"),
        ] {
            assert_eq!(html_unescape(input), want, "html_unescape({input:?})");
        }
        for s in [
            "",
            "abc def",
            "a & b",
            "a&amp;b",
            "a &amp b",
            "&quot;",
            "\"",
            "\"<&>\"",
            "&quot;&lt;&amp;&gt;&quot;",
            "3&5==1 && 0<1, \"0&lt;1\", a+acute=&aacute;",
            "The special characters are: <, >, &, ' and \"",
        ] {
            assert_eq!(html_unescape(&html_escape(s)), s);
        }
    }

    #[test]
    fn escape_round_trips() {
        let s = r#"<a href="x">'&'</a>"#;
        assert_eq!(
            html_escape(s),
            "&lt;a href=&#34;x&#34;&gt;&#39;&amp;&#39;&lt;/a&gt;"
        );
        assert_eq!(html_unescape(&html_escape(s)), s);
    }
}
