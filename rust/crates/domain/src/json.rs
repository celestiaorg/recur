//! Go `encoding/json` compatibility.
//!
//! Domain types derive serde with the Go JSON tags, so `serde_json` already
//! reads and writes the same documents. [`to_string`] additionally matches
//! `json.Marshal` byte for byte: HTML-safe string escaping (`<`, `>`, `&`,
//! U+2028, U+2029) and compacted raw JSON fragments.

use std::io;

use serde::{Deserialize, Serialize};
use serde_json::ser::Formatter;

/// Encodes `value` exactly like Go's `json.Marshal`.
pub fn to_string<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    let mut out = Vec::new();
    value.serialize(&mut serde_json::Serializer::with_formatter(
        &mut out,
        GoFormatter,
    ))?;
    String::from_utf8(out).map_err(serde::ser::Error::custom)
}

struct GoFormatter;

impl Formatter for GoFormatter {
    fn write_string_fragment<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        write_html_escaped(writer, fragment)
    }

    fn write_raw_fragment<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        // Go compacts Marshaler (json.RawMessage) output and HTML-escapes it.
        let mut in_string = false;
        let mut escaped = false;
        let mut start = 0;
        for (i, c) in fragment.char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    in_string = false;
                }
                continue;
            }
            match c {
                '"' => in_string = true,
                ' ' | '\t' | '\n' | '\r' => {
                    write_html_escaped(writer, &fragment[start..i])?;
                    start = i + 1;
                }
                _ => {}
            }
        }
        write_html_escaped(writer, &fragment[start..])
    }
}

fn write_html_escaped<W: ?Sized + io::Write>(writer: &mut W, s: &str) -> io::Result<()> {
    let mut start = 0;
    for (i, c) in s.char_indices() {
        let esc = match c {
            '<' => "\\u003c",
            '>' => "\\u003e",
            '&' => "\\u0026",
            '\u{2028}' => "\\u2028",
            '\u{2029}' => "\\u2029",
            _ => continue,
        };
        writer.write_all(&s.as_bytes()[start..i])?;
        writer.write_all(esc.as_bytes())?;
        start = i + c.len_utf8();
    }
    writer.write_all(&s.as_bytes()[start..])
}

/// Serializes a Go `float64` field: integral values print without a
/// fractional part (`0`, not `0.0`), as Go does.
pub mod float {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
        // 2^63: every integral f64 below it converts to i64 exactly.
        const I64_LIMIT: f64 = 9_223_372_036_854_775_808.0;
        if v.is_finite()
            && v.fract() == 0.0
            && v.abs() < I64_LIMIT
            && !(*v == 0.0 && v.is_sign_negative())
        {
            s.serialize_i64(*v as i64)
        } else {
            s.serialize_f64(*v)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        f64::deserialize(d)
    }
}

/// Decodes JSON `null` as the zero value, as Go does for non-pointer fields.
pub(crate) fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A Go `json.RawMessage` held as its JSON text. Empty text is the nil
/// message and encodes as `null`, like Go.
pub mod raw {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::value::RawValue;

    pub fn serialize<S: Serializer>(v: &str, s: S) -> Result<S::Ok, S::Error> {
        if v.is_empty() {
            return s.serialize_unit();
        }
        let raw = RawValue::from_string(v.to_owned()).map_err(serde::ser::Error::custom)?;
        raw.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        Ok(Box::<RawValue>::deserialize(d)?.get().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Serialize)]
    struct Sample {
        text: String,
        #[serde(with = "float")]
        progress: f64,
        #[serde(with = "float")]
        half: f64,
        #[serde(with = "raw")]
        raw: String,
        #[serde(with = "raw")]
        nil: String,
    }

    #[test]
    fn marshal_matches_go() {
        let v = Sample {
            text: "a<b>&c\u{2028}\n\"".into(),
            progress: 1.0,
            half: 0.5,
            raw: "{ \"k\" : [1, 2], \"s\": \"x y<\" }".into(),
            nil: String::new(),
        };
        assert_eq!(
            to_string(&v).unwrap(),
            r#"{"text":"a\u003cb\u003e\u0026c\u2028\n\"","progress":1,"half":0.5,"raw":{"k":[1,2],"s":"x y\u003c"},"nil":null}"#
        );
    }

    #[test]
    fn marshal_sorts_maps() {
        let m: BTreeMap<&str, i32> = [("b", 1), ("a", 2)].into_iter().collect();
        assert_eq!(to_string(&m).unwrap(), r#"{"a":2,"b":1}"#);
    }

    #[test]
    fn raw_round_trips() {
        #[derive(Serialize, serde::Deserialize)]
        struct R {
            #[serde(with = "raw")]
            v: String,
        }
        let r: R = serde_json::from_str(r#"{"v": {"a": 1}}"#).unwrap();
        assert_eq!(r.v, r#"{"a": 1}"#);
        let r: R = serde_json::from_str(r#"{"v": null}"#).unwrap();
        assert_eq!(r.v, "null");
        assert!(serde_json::to_string(&R { v: "{".into() }).is_err());
    }
}
