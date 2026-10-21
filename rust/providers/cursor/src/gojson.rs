//! Go `encoding/json` decode and encode rules the bridge wire depends on.

use serde::de::{self, Deserializer};
use serde::ser::{SerializeMap, SerializeSeq, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

/// A non-pointer Go field: JSON `null` leaves the zero value.
pub(crate) fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A `json.RawMessage` field: the verbatim text, `null` included; absent is
/// empty.
pub(crate) fn raw<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Box::<RawValue>::deserialize(d)?.get().to_owned())
}

/// Go `flexInt`: a JSON number or (for proto int64) a JSON string; `null`
/// and `""` are zero. Fractions and non-numeric strings fail the frame, as in
/// Go.
pub(crate) fn flex_int<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    match Value::deserialize(d)? {
        Value::Null => Ok(0),
        Value::String(s) if s.is_empty() => Ok(0),
        Value::String(s) => s.parse::<i64>().map_err(de::Error::custom),
        Value::Number(n) => n.as_i64().ok_or_else(|| {
            de::Error::custom(format!("json: cannot unmarshal number {n} into int64"))
        }),
        other => Err(de::Error::custom(format!(
            "json: cannot unmarshal {other} into int64"
        ))),
    }
}

/// Go `int` fields: a JSON number without a fraction; `null` is zero.
pub(crate) fn go_int<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    match Option::<Value>::deserialize(d)? {
        None => Ok(0),
        Some(Value::Number(n)) => n.as_i64().ok_or_else(|| {
            de::Error::custom(format!("json: cannot unmarshal number {n} into int"))
        }),
        Some(other) => Err(de::Error::custom(format!(
            "json: cannot unmarshal {other} into int"
        ))),
    }
}

/// A decoded JSON value re-encoded the way Go encodes `map[string]any`: keys
/// sorted, every number a float64.
pub(crate) struct GoAny<'a>(pub &'a Value);

impl Serialize for GoAny<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Number(n) => {
                let raw = RawValue::from_string(go_float(n.as_f64().unwrap_or_default()))
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

/// `json.Marshal` of a decoded value.
pub(crate) fn marshal_any(v: &Value) -> String {
    recur_domain::json::to_string(&GoAny(v)).unwrap_or_default()
}

/// Go `encoding/json` float64 text: shortest round-trip digits, exponent form
/// below 1e-6 or from 1e21 up.
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

/// Go's `json.Unmarshal` into a struct: an object decodes, `null` leaves the
/// zero value, anything else fails. (A serde struct would also take an array
/// positionally.)
pub(crate) fn decode_struct<T: serde::de::DeserializeOwned + Default>(raw: &str) -> Result<T, ()> {
    let t = raw.trim_start();
    if t.starts_with("null") {
        return serde_json::from_str::<Option<T>>(raw)
            .map(Option::unwrap_or_default)
            .map_err(drop);
    }
    if !t.starts_with('{') {
        return Err(());
    }
    serde_json::from_str::<T>(raw).map_err(drop)
}

/// `strings.TrimSpace`.
pub(crate) fn trim(s: &str) -> &str {
    s.trim_matches(char::is_whitespace)
}

/// Go `truncate` in this package: at most `n` bytes plus an ellipsis. Go
/// slices bytes; a cut inside a rune backs off to its start so the result
/// stays valid UTF-8.
pub(crate) fn truncate_bytes(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_owned();
    }
    let mut cut = n;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marshal_any_sorts_and_floats() {
        let v: Value =
            serde_json::from_str(r#"{"b":1,"a":[2.5,"<x>"],"c":{"z":1e21,"y":null}}"#).unwrap();
        assert_eq!(
            marshal_any(&v),
            r#"{"a":[2.5,"\u003cx\u003e"],"b":1,"c":{"y":null,"z":1e+21}}"#
        );
    }

    #[test]
    fn flex_int_cases() {
        #[derive(Deserialize)]
        struct T {
            #[serde(default, deserialize_with = "flex_int")]
            a: i64,
        }
        let p = |s: &str| serde_json::from_str::<T>(s).map(|t| t.a);
        assert_eq!(p(r#"{"a":42}"#).unwrap(), 42);
        assert_eq!(p(r#"{"a":"42"}"#).unwrap(), 42);
        assert_eq!(p(r#"{"a":null}"#).unwrap(), 0);
        assert_eq!(p(r#"{"a":""}"#).unwrap(), 0);
        assert_eq!(p(r#"{}"#).unwrap(), 0);
        assert!(p(r#"{"a":1.5}"#).is_err());
        assert!(p(r#"{"a":"x"}"#).is_err());
    }

    #[test]
    fn truncate_bytes_is_rune_safe() {
        assert_eq!(truncate_bytes("abc", 3), "abc");
        assert_eq!(truncate_bytes("abcd", 3), "abc…");
        assert_eq!(truncate_bytes("aé", 2), "a…");
    }
}
