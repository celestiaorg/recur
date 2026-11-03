//! Serde helpers for the bridge wire: `null` decodes as the default.

use serde::Deserialize;
use serde::de::{self, Deserializer};
use serde_json::Value;
use serde_json::value::RawValue;

/// JSON `null` decodes as the field's default.
pub(crate) fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A raw JSON field: the verbatim text, `null` included; absent is empty.
pub(crate) fn raw<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Box::<RawValue>::deserialize(d)?.get().to_owned())
}

/// A JSON number or (for proto int64) a JSON string; `null` and `""` are
/// zero. Fractions and non-numeric strings fail the frame.
pub(crate) fn flex_int<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    match Value::deserialize(d)? {
        Value::Null => Ok(0),
        Value::String(s) if s.is_empty() => Ok(0),
        Value::String(s) => s.parse::<i64>().map_err(de::Error::custom),
        Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| de::Error::custom(format!("invalid int64: {n}"))),
        other => Err(de::Error::custom(format!("invalid int64: {other}"))),
    }
}

/// Decodes a JSON object as `T`, with `null` as `T::default()`. Anything
/// else fails (serde would otherwise take an array positionally).
pub(crate) fn decode_struct<T: serde::de::DeserializeOwned + Default>(raw: &str) -> Result<T, ()> {
    let t = raw.trim_start();
    if !t.starts_with('{') && !t.starts_with("null") {
        return Err(());
    }
    serde_json::from_str::<Option<T>>(raw)
        .map(Option::unwrap_or_default)
        .map_err(drop)
}

/// At most `n` bytes, cut on a char boundary, plus an ellipsis when cut.
pub(crate) fn truncate_bytes(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_owned();
    }
    format!("{}…", &s[..s.floor_char_boundary(n)])
}

#[cfg(test)]
mod tests {
    use super::*;

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
