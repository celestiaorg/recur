//! Lenient JSON decoding: `null` decodes to the zero value and missing fields
//! default, so every decoded struct is `#[serde(default)]` and nulls are
//! dropped before decoding.

use serde::de::DeserializeOwned;
use serde_json::Value;

/// Decodes `T` (the whole input must be one JSON value).
pub fn decode<T: DeserializeOwned>(raw: &str) -> Result<T, serde_json::Error> {
    let mut v: Value = serde_json::from_str(raw)?;
    strip_nulls(&mut v);
    serde_json::from_value(v)
}

/// The first JSON value; trailing text (gh appends error lines to combined
/// output) is ignored.
pub fn decode_first<T: DeserializeOwned>(raw: &str) -> Result<T, serde_json::Error> {
    let mut stream = serde_json::Deserializer::from_str(raw).into_iter::<Value>();
    let mut v = match stream.next() {
        Some(v) => v?,
        None => serde_json::from_str::<Value>("")?,
    };
    strip_nulls(&mut v);
    serde_json::from_value(v)
}

pub fn decode_value<T: DeserializeOwned>(mut v: Value) -> Result<T, serde_json::Error> {
    strip_nulls(&mut v);
    serde_json::from_value(v)
}

/// Object members that are null are removed (field default); null array
/// elements become `{}` (a default struct).
fn strip_nulls(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.retain(|_, x| !x.is_null());
            m.values_mut().for_each(strip_nulls);
        }
        Value::Array(a) => {
            for x in a.iter_mut() {
                if x.is_null() {
                    *x = Value::Object(Default::default());
                } else {
                    strip_nulls(x);
                }
            }
        }
        _ => {}
    }
}

/// JSON for `gh api --input -` payloads.
pub fn marshal<T: serde::Serialize + ?Sized>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_default()
}
