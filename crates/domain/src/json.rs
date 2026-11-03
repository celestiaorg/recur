//! JSON decoding semantics that serde does not have by default.
//!
//! Domain types derive serde with their JSON field names, so `serde_json` reads and
//! writes compatible documents.

use serde::Deserialize;

/// Decodes JSON `null` as the zero value.
pub(crate) fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A raw JSON message held as its text. Empty text is the nil
/// message and encodes as `null`.
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

/// `v` with integral floats as integers, so documents from encoders that
/// write `1` or `1.0` for the same `float64` compare equal.
pub fn canonical(v: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 9e15 => Value::from(f as i64),
            _ => Value::Number(n),
        },
        Value::Array(a) => Value::Array(a.into_iter().map(canonical).collect()),
        Value::Object(m) => Value::Object(m.into_iter().map(|(k, v)| (k, canonical(v))).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

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
        assert_eq!(
            serde_json::to_string(&R { v: String::new() }).unwrap(),
            r#"{"v":null}"#
        );
        assert!(serde_json::to_string(&R { v: "{".into() }).is_err());
    }

    #[test]
    fn canonical_folds_integral_floats() {
        let v = serde_json::json!({"a": [1.0, 0.5, -0.0], "b": 2});
        assert_eq!(canonical(v), serde_json::json!({"a": [1, 0.5, 0], "b": 2}));
    }
}
