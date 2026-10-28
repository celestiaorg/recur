//! The slice of Go `encoding/json` semantics the Responses wire depends on:
//! `map[string]any` re-encoding (sorted keys, float64 numbers) and typed-struct decoding where `null` is the zero value and any
//! other type mismatch fails the whole decode.

use serde_json::{Map, Number, Value};

/// A decode that Go's `json.Unmarshal` would reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Mismatch;

/// `json.Marshal` of a value that went through `map[string]any`: keys sorted
/// at every level.
pub(crate) fn marshal_sorted(v: &Value) -> String {
    serde_json::to_string(&sorted(v)).unwrap_or_default()
}

fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for k in keys {
                out.insert(k.clone(), sorted(&m[k]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// Go `json.Valid`. Invalid UTF-8 was already replaced by the caller.
pub(crate) fn valid(s: &str) -> bool {
    serde_json::from_str::<serde::de::IgnoredAny>(s).is_ok()
}

/// A Go float64 as a JSON number: integral values print without a fraction
/// (`1`, not `1.0`), as Go does.
pub(crate) fn go_float(f: f64) -> Value {
    // 2^63: every integral f64 below it converts to i64 exactly.
    const I64_LIMIT: f64 = 9_223_372_036_854_775_808.0;
    if f.is_finite()
        && f.fract() == 0.0
        && f.abs() < I64_LIMIT
        && !(f == 0.0 && f.is_sign_negative())
    {
        return Value::Number(Number::from(f as i64));
    }
    Number::from_f64(f).map_or(Value::Null, Value::Number)
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
    fn floats_print_like_go() {
        assert_eq!(go_float(1.0).to_string(), "1");
        assert_eq!(go_float(0.2).to_string(), "0.2");
        assert_eq!(go_float(-3.0).to_string(), "-3");
        assert_eq!(go_float(f64::NAN), Value::Null);
    }

    #[test]
    fn marshal_sorts_nested_keys() {
        let v = serde_json::json!({"b": {"z": 1, "a": "<&>"}, "a": [ {"y": 1, "x": 2} ]});
        assert_eq!(
            marshal_sorted(&v),
            r#"{"a":[{"x":2,"y":1}],"b":{"a":"<&>","z":1}}"#
        );
    }

    #[test]
    fn clip_bytes() {
        assert_eq!(clip("abc", 5), "abc");
        assert_eq!(clip("abcdef", 3), "abc…");
        assert_eq!(clip("é", 1), "\u{fffd}…");
    }
}
