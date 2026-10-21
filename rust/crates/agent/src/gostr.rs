//! Go and JavaScript string semantics the ported rules depend on.

use std::borrow::Cow;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// Go `strings.ToLower`: per-rune simple case mapping (no final sigma, and
/// U+0130 lowers to a plain `i`).
pub(crate) fn go_lower(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\u{130}' {
            out.push('i');
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// Go `strings.ToUpper`: per-rune simple case mapping. A rune whose full
/// uppercase is several runes has no simple mapping and stays as is.
pub(crate) fn go_upper(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mut up = c.to_uppercase();
            match (up.next(), up.next()) {
                (Some(u), None) => u,
                _ => c,
            }
        })
        .collect()
}

/// Decodes bytes the way Go's JSON encoder and decoder coerce invalid UTF-8:
/// every byte of an invalid sequence becomes its own U+FFFD.
pub(crate) fn go_lossy(mut b: &[u8]) -> Cow<'_, str> {
    if let Ok(s) = std::str::from_utf8(b) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(b.len() + 8);
    loop {
        match std::str::from_utf8(b) {
            Ok(s) => {
                out.push_str(s);
                return Cow::Owned(out);
            }
            Err(e) => {
                let (good, rest) = b.split_at(e.valid_up_to());
                out.push_str(std::str::from_utf8(good).unwrap_or_default());
                let bad = e.error_len().unwrap_or(rest.len());
                out.extend(std::iter::repeat_n('\u{FFFD}', bad));
                b = &rest[bad..];
            }
        }
    }
}

/// Go `s[:n]` on a string as it reaches the UI: a cut inside a rune leaves
/// stray bytes, which Go's JSON encoding turns into U+FFFD each.
pub(crate) fn go_prefix(s: &str, n: usize) -> Cow<'_, str> {
    go_lossy(&s.as_bytes()[..n.min(s.len())])
}

/// Go `strconv.Quote` (what `%q` prints for a string).
pub(crate) fn go_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ if go_is_print(c) => out.push(c),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            _ if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            _ if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push('"');
    out
}

/// Go `strconv.IsPrint`.
// simplify: above Latin-1 this tests control, white space, format and
// private-use characters rather than Go's full category tables, so an
// unassigned code point prints raw where Go escapes it. Port Go's isPrint
// tables if a quoted value ever needs that precision.
fn go_is_print(c: char) -> bool {
    let u = c as u32;
    if u <= 0xff {
        return (0x20..=0x7e).contains(&u) || ((0xa1..=0xff).contains(&u) && u != 0xad);
    }
    !(c.is_control()
        || c.is_whitespace()
        || is_format(u)
        || (0xe000..=0xf8ff).contains(&u)
        || u >= 0xf0000
        || (u & 0xfffe) == 0xfffe
        || (0xfdd0..=0xfdef).contains(&u))
}

/// Unicode general category Cf above Latin-1.
fn is_format(u: u32) -> bool {
    matches!(
        u,
        0x600..=0x605
            | 0x61c
            | 0x6dd
            | 0x70f
            | 0x890..=0x891
            | 0x8e2
            | 0x180e
            | 0x200b..=0x200f
            | 0x202a..=0x202e
            | 0x2060..=0x2064
            | 0x2066..=0x206f
            | 0xfeff
            | 0xfff9..=0xfffb
            | 0x110bd
            | 0x110cd
            | 0x13430..=0x1343f
            | 0x1bca0..=0x1bca3
            | 0x1d173..=0x1d17a
            | 0xe0001
            | 0xe0020..=0xe007f
    )
}

/// JavaScript `String.prototype.trim`: white space plus line terminators,
/// which includes U+FEFF and excludes U+0085.
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(js_space)
}

/// JavaScript regex `\s`.
pub(crate) fn js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

/// Decodes JSON `null` as the zero value, as Go does for non-pointer fields.
pub(crate) fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// Serializes a decoded JSON value the way Go re-encodes `map[string]any`:
/// keys sorted (serde_json's map already is), every number as a float64.
pub(crate) struct GoAny<'a>(pub &'a Value);

impl Serialize for GoAny<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        match self.0 {
            Value::Number(n) => {
                let f = n.as_f64().unwrap_or_default();
                let raw = serde_json::value::RawValue::from_string(go_float(f))
                    .map_err(serde::ser::Error::custom)?;
                raw.serialize(s)
            }
            Value::Array(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for v in items {
                    seq.serialize_element(&GoAny(v))?;
                }
                seq.end()
            }
            Value::Object(m) => {
                let mut map = s.serialize_map(Some(m.len()))?;
                for (k, v) in m {
                    map.serialize_entry(k, &GoAny(v))?;
                }
                map.end()
            }
            other => other.serialize(s),
        }
    }
}

/// Go `encoding/json` float64 text: shortest round-trip digits, exponent
/// form below 1e-6 or from 1e21 up.
fn go_float(f: f64) -> String {
    let abs = f.abs();
    if abs != 0.0 && !(1e-6..1e21).contains(&abs) {
        let e = format!("{f:e}");
        match e.split_once('e') {
            Some((m, x)) if !x.starts_with('-') => format!("{m}e+{x}"),
            _ => e,
        }
    } else {
        format!("{f}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_upper_follow_go() {
        assert_eq!(go_lower("Rate LİMIT ΟΔΟΣ"), "rate limit οδοσ");
        assert_eq!(go_upper("warn ß"), "WARN ß");
    }

    #[test]
    fn lossy_replaces_each_bad_byte() {
        assert_eq!(go_lossy(b"a\xe2\x80"), "a\u{FFFD}\u{FFFD}");
        assert_eq!(go_lossy(b"ok"), "ok");
        assert_eq!(go_prefix("a\u{2014}b", 2), "a\u{FFFD}");
        assert_eq!(go_prefix("ab", 9), "ab");
    }

    #[test]
    fn quote_matches_strconv() {
        assert_eq!(go_quote("a\"b\\c"), r#""a\"b\\c""#);
        assert_eq!(
            go_quote("tab\there\n\u{0}\u{7f}"),
            r#""tab\there\n\x00\x7f""#
        );
        assert_eq!(
            go_quote("é ✓ \u{a0}\u{ad}\u{2028}"),
            "\"é ✓ \\u00a0\\u00ad\\u2028\""
        );
        assert_eq!(
            go_quote("\u{2060}\u{e000}\u{10ffff}"),
            "\"\\u2060\\ue000\\U0010ffff\""
        );
    }

    #[test]
    fn js_trim_set() {
        assert_eq!(js_trim("\u{feff} x \u{a0}"), "x");
        assert_eq!(js_trim("\u{85}x"), "\u{85}x");
    }

    #[test]
    fn go_float_text() {
        for (f, want) in [
            (1.0, "1"),
            (0.5, "0.5"),
            (-0.0, "-0"),
            (1e21, "1e+21"),
            (1.5e-7, "1.5e-7"),
            (12345678901234567890.0, "12345678901234567000"),
            (1e20, "100000000000000000000"),
        ] {
            assert_eq!(go_float(f), want, "{f}");
        }
    }
}
