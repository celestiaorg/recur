//! Flag dimensions onto Responses request JSON paths (Go `responses/flags.go`).

use recur_agent::Flags;
use recur_domain::CompatibleFlagDim;
use serde_json::{Map, Value, json};

/// A flag value Go's encoder cannot write (`json: unsupported value`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("json: unsupported value: {0}")]
pub struct UnsupportedValue(pub String);

/// Writes selected flag values into `dst` at each dimension's wire path.
/// Unknown or empty wires are skipped; nested objects are created as needed.
/// Reasoning without a summary gets `summary: auto`, text without a format
/// gets `format: {type: text}`.
pub fn apply_flags(
    dst: &mut Map<String, Value>,
    dims: &[CompatibleFlagDim],
    flags: &Flags,
) -> Result<(), UnsupportedValue> {
    for d in dims {
        if d.wire.is_empty() {
            continue;
        }
        let val = flags.get(&d.id).map_or("", |v| v.trim());
        if val.is_empty() {
            continue;
        }
        set_wire(dst, &d.wire, coerce_wire_value(&d.wire, val)?);
    }
    if let Some(Value::Object(m)) = dst.get_mut("reasoning")
        && !m.contains_key("summary")
    {
        m.insert("summary".into(), json!("auto"));
    }
    // OpenAI defaults the format; LM Studio rejects the request without it.
    if let Some(Value::Object(m)) = dst.get_mut("text")
        && !m.contains_key("format")
    {
        m.insert("format".into(), json!({"type": "text"}));
    }
    Ok(())
}

fn coerce_wire_value(wire: &str, val: &str) -> Result<Value, UnsupportedValue> {
    match wire {
        "temperature" => match val.parse::<f64>() {
            Ok(f) if f.is_nan() => Err(UnsupportedValue("NaN".into())),
            Ok(f) if f.is_infinite() => Err(UnsupportedValue(
                if f > 0.0 { "+Inf" } else { "-Inf" }.into(),
            )),
            Ok(f) => Ok(f.into()),
            Err(_) => Ok(Value::String(val.into())),
        },
        "max_output_tokens" => Ok(val
            .parse::<i64>()
            .map_or_else(|_| Value::String(val.into()), Value::from)),
        _ => Ok(Value::String(val.into())),
    }
}

fn set_wire(dst: &mut Map<String, Value>, path: &str, val: Value) {
    let parts: Vec<&str> = path.split('.').collect();
    let mut cur = dst;
    for (i, p) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            cur.insert((*p).to_owned(), val);
            return;
        }
        let slot = cur
            .entry((*p).to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        if !slot.is_object() {
            *slot = Value::Object(Map::new());
        }
        let Value::Object(next) = slot else {
            return;
        };
        cur = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wires_nested_paths() {
        let mut dst = Map::new();
        let dim = |id: &str, wire: &str| CompatibleFlagDim {
            id: id.into(),
            wire: wire.into(),
            ..Default::default()
        };
        let dims = [
            dim("effort", "reasoning.effort"),
            dim("verbosity", "text.verbosity"),
            dim("temp", "temperature"),
            dim("max", "max_output_tokens"),
            dim("skip", ""),
        ];
        let flags: Flags = [
            ("effort", "high"),
            ("verbosity", "low"),
            ("temp", "0.2"),
            ("max", "1024"),
            ("skip", "x"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        apply_flags(&mut dst, &dims, &flags).unwrap();
        assert_eq!(
            dst["reasoning"],
            json!({"effort": "high", "summary": "auto"})
        );
        assert_eq!(
            dst["text"],
            json!({"verbosity": "low", "format": {"type": "text"}})
        );
        assert_eq!(dst["temperature"], json!(0.2));
        assert_eq!(dst["max_output_tokens"], json!(1024));
        assert!(!dst.contains_key("skip"));
    }

    #[test]
    fn non_finite_temperature_fails_like_go_marshal() {
        let dims = [CompatibleFlagDim {
            id: "t".into(),
            wire: "temperature".into(),
            ..Default::default()
        }];
        let flags: Flags = [("t".to_owned(), "inf".to_owned())].into_iter().collect();
        let err = apply_flags(&mut Map::new(), &dims, &flags).unwrap_err();
        assert_eq!(err.to_string(), "json: unsupported value: +Inf");
    }
}
