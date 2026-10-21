//! Go text helpers the engine's user-visible strings depend on.

pub use recur_agent::display::{hard_truncate_runes, truncate};

/// Go `html.EscapeString`.
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

/// Go `time.Now().UnixNano()`.
pub fn unix_nanos() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

/// Go `time.Now().UnixMilli()`.
pub fn unix_millis() -> i64 {
    unix_nanos() / 1_000_000
}

/// Go `time.Time.Format(time.RFC3339)` in local time.
pub fn rfc3339(t: chrono::DateTime<chrono::Local>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// `n one` / `n many` (Go `plural`).
pub fn plural(n: i64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Go `base64.StdEncoding.DecodeString` (padded, not strict): `\r` and
/// `\n` are skipped anywhere, trailing bits are ignored, and an error names
/// the byte offset ("illegal base64 data at input byte N").
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
}
