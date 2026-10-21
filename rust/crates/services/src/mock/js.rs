//! The JavaScript value semantics the mock bridge's resolvers rely on, over
//! `serde_json::Value`: truthiness, `String(x)`, `Number(x)`, `??`, `||` and
//! object spread. An absent key is JS `undefined`.

use serde_json::{Map, Value};

/// JS truthiness. `None` is `undefined`.
pub(crate) fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

/// `x ?? fallback`.
pub(crate) fn nullish<'a>(v: Option<&'a Value>, fallback: &'a Value) -> &'a Value {
    match v {
        None | Some(Value::Null) => fallback,
        Some(v) => v,
    }
}

/// `x || fallback`.
pub(crate) fn or<'a>(v: Option<&'a Value>, fallback: &'a Value) -> &'a Value {
    match v {
        Some(v) if truthy(Some(v)) => v,
        _ => fallback,
    }
}

/// `String(x)`.
pub(crate) fn string(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number_string(n),
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|v| {
                if v.is_null() {
                    String::new()
                } else {
                    string(v)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

fn number_string(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    match n.as_f64() {
        Some(f) if f.fract() == 0.0 && f.abs() < 1e21 => format!("{f:.0}"),
        Some(f) => f.to_string(),
        None => n.to_string(),
    }
}

/// `String(x ?? '')`: the resolvers' usual argument read.
pub(crate) fn str_arg(args: &[Value], i: usize) -> String {
    match args.get(i) {
        None | Some(Value::Null) => String::new(),
        Some(v) => string(v),
    }
}

/// `Number(x)` (NaN for anything non-numeric).
pub(crate) fn number(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                0.0
            } else {
                t.parse().unwrap_or(f64::NAN)
            }
        }
        Some(Value::Array(_) | Value::Object(_)) => f64::NAN,
    }
}

/// A JS number as JSON: integral values stay integers, as `JSON.stringify`
/// prints them.
pub(crate) fn num(f: f64) -> Value {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.0e15 {
        Value::from(f as i64)
    } else if f.is_finite() {
        Value::from(f)
    } else {
        Value::Null
    }
}

/// `{...a, ...b, …}`: later keys win; non-objects spread nothing.
pub(crate) fn spread(parts: &[&Value]) -> Value {
    let mut out = Map::new();
    for p in parts {
        if let Value::Object(m) = p {
            for (k, v) in m {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    Value::Object(out)
}

/// `obj.key` on any value (`undefined` for non-objects).
pub(crate) fn get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.as_object().and_then(|m| m.get(key))
}

/// `obj.key` as a string, `''` when absent or not a string.
pub(crate) fn get_str<'a>(v: &'a Value, key: &str) -> &'a str {
    get(v, key).and_then(Value::as_str).unwrap_or("")
}

/// `obj.key = value` (no-op on non-objects).
pub(crate) fn set(v: &mut Value, key: &str, value: Value) {
    if let Value::Object(m) = v {
        m.insert(key.to_owned(), value);
    }
}

/// `delete obj.key`.
pub(crate) fn delete(v: &mut Value, key: &str) {
    if let Value::Object(m) = v {
        m.remove(key);
    }
}

/// An array's elements, empty for anything else (the resolvers' `x ?? []`).
pub(crate) fn items(v: Option<&Value>) -> Vec<Value> {
    match v {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    }
}

/// `Date.now()`.
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn js_semantics() {
        assert!(!truthy(None));
        assert!(!truthy(Some(&json!(""))));
        assert!(!truthy(Some(&json!(0))));
        assert!(truthy(Some(&json!([]))));
        assert_eq!(string(&json!(3)), "3");
        assert_eq!(string(&json!(["a", null, 1])), "a,,1");
        assert_eq!(str_arg(&[json!(null)], 0), "");
        assert_eq!(str_arg(&[], 3), "");
        assert!(number(Some(&json!("x"))).is_nan());
        assert_eq!(number(Some(&json!(" 4 "))), 4.0);
        assert_eq!(num(-3.0), json!(-3));
        assert_eq!(
            spread(&[&json!({"a": 1, "b": 2}), &json!(null), &json!({"b": 3})]),
            json!({"a": 1, "b": 3})
        );
        assert_eq!(nullish(Some(&json!(false)), &json!(true)), &json!(false));
        assert_eq!(or(Some(&json!(false)), &json!(true)), &json!(true));
    }
}
