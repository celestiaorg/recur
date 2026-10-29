//! The slice of Go `encoding/json` semantics the Responses wire depends on:
//! typed-struct decoding where `null` is the zero value and any other type
//! mismatch fails the whole decode.

use serde_json::{Map, Value};

/// A decode that Go's `json.Unmarshal` would reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Mismatch;

/// Go `json.Valid`. Invalid UTF-8 was already replaced by the caller.
pub(crate) fn valid(s: &str) -> bool {
    serde_json::from_str::<serde::de::IgnoredAny>(s).is_ok()
}

/// A string field of a struct decode: absent or `null` is "", a non-string
/// fails.
pub(crate) fn str_field(obj: &Map<String, Value>, key: &str) -> Result<String, Mismatch> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(Mismatch),
    }
}

/// An int64 field: absent or `null` is 0; fractions, exponents, overflow and
/// non-numbers fail.
pub(crate) fn int_field(obj: &Map<String, Value>, key: &str) -> Result<i64, Mismatch> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(0),
        Some(Value::Number(n)) => n.as_i64().ok_or(Mismatch),
        Some(_) => Err(Mismatch),
    }
}

/// The object a struct decodes from: `null` decodes as an empty struct, any
/// other non-object fails.
pub(crate) fn as_struct(v: &Value) -> Result<Option<&Map<String, Value>>, Mismatch> {
    match v {
        Value::Null => Ok(None),
        Value::Object(m) => Ok(Some(m)),
        _ => Err(Mismatch),
    }
}

/// A slice field: absent or `null` is nil, a non-array fails.
pub(crate) fn array_field<'a>(
    obj: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a [Value], Mismatch> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(a)) => Ok(a),
        Some(_) => Err(Mismatch),
    }
}

/// Go's `s[:n] + "…"` clip on byte length.
pub(crate) fn clip(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_owned();
    }
    let mut out = String::from_utf8_lossy(&s.as_bytes()[..n]).into_owned();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_bytes() {
        assert_eq!(clip("abc", 5), "abc");
        assert_eq!(clip("abcdef", 3), "abc…");
        assert_eq!(clip("é", 1), "\u{fffd}…");
    }
}
