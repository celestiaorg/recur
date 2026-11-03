//! Text helpers the engine's user-visible strings depend on.

pub use recur_agent::display::{hard_truncate_runes, truncate};

/// Escapes `<`, `>`, `&`, `'` and `"` as HTML entities.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '\'' => out.push_str("&#39;"),
            '"' => out.push_str("&#34;"),
            _ => out.push(c),
        }
    }
    out
}

/// Unescapes diff code HTML: the five `html_escape` entities,
/// `&quot;`/`&apos;`/`&nbsp;` and numeric references. Anything else stays
/// literal.
// simplify: not the full HTML5 named-entity table; diff code HTML only ever
// carries the entities above.
pub fn html_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest[1..].find(';').map(|e| e + 1).filter(|&e| e <= 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => name.strip_prefix('#').and_then(|n| {
                let v = match n.strip_prefix(['x', 'X']) {
                    Some(h) => u32::from_str_radix(h, 16).ok(),
                    None => n.parse().ok(),
                }?;
                Some(char::from_u32(v).unwrap_or('\u{fffd}'))
            }),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn unix_nanos() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

pub fn unix_millis() -> i64 {
    unix_nanos() / 1_000_000
}

/// `t` as RFC 3339 in local time, whole seconds.
pub fn rfc3339(t: chrono::DateTime<chrono::Local>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// `n one` / `n many`.
pub fn plural(n: i64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Standard padded base64, not strict: `\r` and `\n` are skipped anywhere,
/// trailing bits are ignored, and an error names the byte offset ("illegal
/// base64 data at input byte N").
pub fn go_base64_decode(s: &str) -> Result<Vec<u8>, String> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let corrupt = |at: usize| format!("illegal base64 data at input byte {at}");
    let src = s.as_bytes();
    let skip_nl = |mut i: usize| {
        while i < src.len() && (src[i] == b'\n' || src[i] == b'\r') {
            i += 1;
        }
        i
    };
    let mut out = Vec::with_capacity(src.len() / 4 * 3);
    let mut si = 0usize;
    loop {
        let mut dbuf = [0u8; 4];
        let mut dlen = 4;
        let mut j = 0usize;
        let mut tail_err: Option<String> = None;
        while j < 4 {
            if si == src.len() {
                if j == 0 {
                    return Ok(out);
                }
                return Err(corrupt(si - j));
            }
            let c = src[si];
            si += 1;
            if let Some(v) = val(c) {
                dbuf[j] = v;
                j += 1;
                continue;
            }
            if c == b'\n' || c == b'\r' {
                continue;
            }
            if c != b'=' {
                return Err(corrupt(si - 1));
            }
            match j {
                0 | 1 => return Err(corrupt(si - 1)),
                2 => {
                    si = skip_nl(si);
                    if si == src.len() {
                        return Err(corrupt(src.len()));
                    }
                    if src[si] != b'=' {
                        return Err(corrupt(si - 1));
                    }
                    si += 1;
                }
                _ => {}
            }
            si = skip_nl(si);
            if si < src.len() {
                tail_err = Some(corrupt(si));
            }
            dlen = j;
            break;
        }
        let v = (u32::from(dbuf[0]) << 18)
            | (u32::from(dbuf[1]) << 12)
            | (u32::from(dbuf[2]) << 6)
            | u32::from(dbuf[3]);
        let bytes = [(v >> 16) as u8, (v >> 8) as u8, v as u8];
        out.extend_from_slice(&bytes[..dlen - 1]);
        if let Some(e) = tail_err {
            return Err(e);
        }
        if dlen < 4 {
            return Ok(out);
        }
    }
}

/// `a` when it has non-space text, else `b`.
pub fn first_non_empty_str<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.trim().is_empty() { b } else { a }
}

/// Equal under simple Unicode case folding.
pub fn equal_fold(a: &str, b: &str) -> bool {
    let fold = |c: char| {
        let mut lower = c.to_lowercase();
        match (lower.next(), lower.next()) {
            (Some(l), None) => l,
            _ => c,
        }
    };
    a.chars().count() == b.chars().count()
        && a.chars()
            .zip(b.chars())
            .all(|(x, y)| x == y || fold(x) == fold(y))
}

/// Case-insensitive equality with an ASCII `lit`: only ASCII case and
/// the two runes that simple-fold into ASCII letters (U+017F ſ → s, U+212A
/// Kelvin sign → k) match.
pub fn equal_fold_ascii(s: &str, lit: &str) -> bool {
    let mut a = s.chars();
    for l in lit.chars() {
        let Some(c) = a.next() else {
            return false;
        };
        let ok = c.eq_ignore_ascii_case(&l)
            || (c == '\u{17f}' && l.eq_ignore_ascii_case(&'s'))
            || (c == '\u{212a}' && l.eq_ignore_ascii_case(&'k'));
        if !ok {
            return false;
        }
    }
    a.next().is_none()
}

/// A duration like `1h30m` in nanoseconds: `[-+]?([0-9]*(\.[0-9]*)?[a-z]+)+`.
pub fn go_parse_duration(orig: &str) -> Result<i64, String> {
    const LIMIT: u64 = 1 << 63;
    let invalid = || format!("time: invalid duration {orig:?}");
    let mut s = orig.as_bytes();
    let mut neg = false;
    if let Some(&c @ (b'-' | b'+')) = s.first() {
        neg = c == b'-';
        s = &s[1..];
    }
    if s == b"0" {
        return Ok(0);
    }
    if s.is_empty() {
        return Err(invalid());
    }
    let mut d: u64 = 0;
    while !s.is_empty() {
        if !(s[0] == b'.' || s[0].is_ascii_digit()) {
            return Err(invalid());
        }
        // [0-9]*
        let mut v: u64 = 0;
        let mut i = 0;
        while i < s.len() && s[i].is_ascii_digit() {
            if v > LIMIT / 10 {
                return Err(invalid());
            }
            v = v * 10 + u64::from(s[i] - b'0');
            if v > LIMIT {
                return Err(invalid());
            }
            i += 1;
        }
        let pre = i > 0;
        s = &s[i..];
        // (\.[0-9]*)?
        let (mut f, mut scale, mut post) = (0u64, 1f64, false);
        if s.first() == Some(&b'.') {
            s = &s[1..];
            let mut i = 0;
            let mut overflow = false;
            while i < s.len() && s[i].is_ascii_digit() {
                if !overflow {
                    if f > (LIMIT - 1) / 10 {
                        overflow = true;
                    } else {
                        let y = f * 10 + u64::from(s[i] - b'0');
                        if y > LIMIT {
                            overflow = true;
                        } else {
                            f = y;
                            scale *= 10.0;
                        }
                    }
                }
                i += 1;
            }
            post = i > 0;
            s = &s[i..];
        }
        if !pre && !post {
            return Err(invalid());
        }
        let i = s
            .iter()
            .position(|&c| c == b'.' || c.is_ascii_digit())
            .unwrap_or(s.len());
        if i == 0 {
            return Err(format!("time: missing unit in duration {orig:?}"));
        }
        let unit_name = std::str::from_utf8(&s[..i]).unwrap_or_default();
        s = &s[i..];
        let unit: u64 = match unit_name {
            "ns" => 1,
            "us" | "\u{b5}s" | "\u{3bc}s" => 1_000,
            "ms" => 1_000_000,
            "s" => 1_000_000_000,
            "m" => 60_000_000_000,
            "h" => 3_600_000_000_000,
            _ => {
                return Err(format!(
                    "time: unknown unit {unit_name:?} in duration {orig:?}"
                ));
            }
        };
        if v > LIMIT / unit {
            return Err(invalid());
        }
        v *= unit;
        if f > 0 {
            v += (f as f64 * (unit as f64 / scale)) as u64;
            if v > LIMIT {
                return Err(invalid());
            }
        }
        d += v;
        if d > LIMIT {
            return Err(invalid());
        }
    }
    if neg {
        return Ok((d as i64).wrapping_neg());
    }
    if d > LIMIT - 1 {
        return Err(invalid());
    }
    Ok(d as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_like_go() {
        assert_eq!(
            html_escape(r#"<a href="x">'&'</a>"#),
            "&lt;a href=&#34;x&#34;&gt;&#39;&amp;&#39;&lt;/a&gt;"
        );
    }

    #[test]
    fn base64_like_go() {
        assert_eq!(go_base64_decode("aGk=").unwrap(), b"hi");
        assert_eq!(go_base64_decode("aG\nk=").unwrap(), b"hi");
        assert_eq!(go_base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(go_base64_decode("").unwrap(), b"");
        assert_eq!(
            go_base64_decode("aGk").unwrap_err(),
            "illegal base64 data at input byte 0"
        );
        assert_eq!(
            go_base64_decode("not base64!").unwrap_err(),
            "illegal base64 data at input byte 3"
        );
        assert_eq!(
            go_base64_decode("aGk=x").unwrap_err(),
            "illegal base64 data at input byte 4"
        );
        // Trailing bits are ignored (not strict).
        assert_eq!(go_base64_decode("aGl=").unwrap(), b"hi");
    }

    #[test]
    fn plural_forms() {
        assert_eq!(plural(1, "finding", "findings"), "1 finding");
        assert_eq!(plural(0, "finding", "findings"), "0 findings");
    }

    #[test]
    fn go_durations() {
        assert_eq!(go_parse_duration("50ms"), Ok(50_000_000));
        assert_eq!(go_parse_duration("4h"), Ok(4 * 3_600_000_000_000));
        assert_eq!(go_parse_duration("1.5s"), Ok(1_500_000_000));
        assert!(go_parse_duration("").is_err());
        assert!(go_parse_duration("10").is_err());
    }
}
