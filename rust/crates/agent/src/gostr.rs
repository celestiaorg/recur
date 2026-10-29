//! Go and JavaScript string semantics the ported rules depend on.

use std::borrow::Cow;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// The first `n` bytes of `s`; a char cut in half becomes U+FFFD.
pub(crate) fn byte_prefix(s: &str, n: usize) -> Cow<'_, str> {
    String::from_utf8_lossy(&s.as_bytes()[..n.min(s.len())])
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
/// keys sorted, every number as a float64.
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
                let mut entries: Vec<_> = m.iter().collect();
                entries.sort_unstable_by_key(|(k, _)| k.as_str());
                let mut map = s.serialize_map(Some(entries.len()))?;
                for (k, v) in entries {
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
    fn byte_prefix_cuts_bytes() {
        assert_eq!(byte_prefix("a\u{2014}b", 2), "a\u{FFFD}");
        assert_eq!(byte_prefix("ab", 9), "ab");
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
