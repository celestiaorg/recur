//! Loose JSON readers over `map[string]any`-style payloads, JWT claims and
//! email discovery.

use base64::Engine as _;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use serde_json::{Map, Value};

use crate::types::clamp_pct;

pub(crate) type Obj = Map<String, Value>;

/// Unpadded URL-safe base64 (non-canonical trailing bits accepted).
const RAW_URL: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_allow_trailing_bits(true)
        .with_decode_padding_mode(DecodePaddingMode::RequireNone),
);

/// Padded URL-safe base64.
const PADDED_URL: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::URL_SAFE,
    GeneralPurposeConfig::new()
        .with_decode_allow_trailing_bits(true)
        .with_decode_padding_mode(DecodePaddingMode::RequireCanonical),
);

pub(crate) fn as_float(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

pub(crate) fn as_int64(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        Some(Value::String(s)) => {
            let s = s.trim();
            if s.is_empty() {
                return 0;
            }
            s.parse::<i64>()
                .ok()
                .or_else(|| s.parse::<f64>().ok().map(|f| f as i64))
                .unwrap_or(0)
        }
        _ => 0,
    }
}

pub(crate) fn as_string(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.trim().to_owned(),
        _ => String::new(),
    }
}

pub(crate) fn as_bool(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        other => as_float(other) != 0.0,
    }
}

pub(crate) fn map_of(v: Option<&Value>) -> Option<&Obj> {
    v.and_then(Value::as_object)
}

pub(crate) fn slice_of(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// Unix seconds, unix milliseconds, or RFC 3339; 0 when absent.
pub(crate) fn parse_resets_at(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::String(s)) => {
            let s = s.trim();
            if s.is_empty() {
                return 0;
            }
            if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
                return t.timestamp_millis();
            }
            resets_from_number(s.parse().unwrap_or(0.0))
        }
        other => resets_from_number(as_float(other)),
    }
}

fn resets_from_number(n: f64) -> i64 {
    if n <= 0.0 {
        0
    } else if n > 1e12 {
        n as i64
    } else {
        (n as i64) * 1000
    }
}

pub(crate) fn decode_map(raw: &[u8]) -> Result<Obj, String> {
    match serde_json::from_slice::<Value>(raw).map_err(|e| e.to_string())? {
        Value::Object(m) => Ok(m),
        Value::Null => Ok(Obj::new()),
        _ => Err("json: cannot unmarshal into map[string]interface {}".into()),
    }
}

pub(crate) fn first_map<'a>(m: &'a Obj, keys: &[&str]) -> Option<&'a Obj> {
    keys.iter().find_map(|k| map_of(m.get(*k)))
}

pub(crate) fn first_string(m: &Obj, keys: &[&str]) -> String {
    keys.iter()
        .map(|k| as_string(m.get(*k)))
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

pub(crate) fn first_any<'a>(m: &'a Obj, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| m.get(*k).filter(|v| !v.is_null()))
}

pub(crate) fn email_from_cursor_json(m: Option<&Obj>) -> String {
    let Some(m) = m else {
        return String::new();
    };
    for k in [
        "email",
        "userEmail",
        "user_email",
        "cachedEmail",
        "cached_email",
        "emailAddress",
        "email_address",
    ] {
        let s = as_string(m.get(k));
        if looks_like_email(&s) {
            return s;
        }
    }
    m.values()
        .map(|v| email_from_cursor_json(v.as_object()))
        .find(|e| !e.is_empty())
        .unwrap_or_default()
}

pub(crate) fn jwt_payload(tok: &str) -> Option<Obj> {
    let parts: Vec<&str> = tok.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    decode_jwt_payload(parts[1]).ok()
}

pub(crate) fn jwt_claim(tok: &str, key: &str) -> String {
    jwt_payload(tok)
        .map(|m| as_string(m.get(key)))
        .unwrap_or_default()
}

pub(crate) fn email_from_jwt(tok: &str) -> String {
    let Some(m) = jwt_payload(tok) else {
        return String::new();
    };
    for k in [
        "email",
        "email_address",
        "emailAddress",
        "preferred_username",
        "upn",
        "sub",
    ] {
        let s = as_string(m.get(k));
        if looks_like_email(&s) {
            return s;
        }
    }
    String::new()
}

pub(crate) fn looks_like_email(s: &str) -> bool {
    let s = s.trim();
    match s.find('@') {
        Some(at) => at > 0 && at < s.len() - 1 && !s.contains([' ', '\t']),
        None => false,
    }
}

/// How soon before expiry Recur rotates Connect credentials (session JWT and
/// minted user API key). A week covers a long weekend if Recur is closed over
/// the last days of a 60/90-day token.
pub(crate) const CURSOR_REFRESH_LEEWAY_MS: i64 = 7 * 24 * 3600 * 1000;

pub(crate) fn jwt_needs_refresh(access: &str) -> bool {
    let parts: Vec<&str> = access.split('.').collect();
    if parts.len() < 2 {
        return false;
    }
    let Ok(payload) = decode_jwt_payload(parts[1]) else {
        return false;
    };
    let exp = as_float(payload.get("exp"));
    if exp <= 0.0 {
        return false;
    }
    let now_s = chrono::Utc::now().timestamp() + CURSOR_REFRESH_LEEWAY_MS / 1000;
    now_s >= exp as i64
}

fn decode_jwt_payload(seg: &str) -> Result<Obj, String> {
    let raw = RAW_URL
        .decode(seg.trim_end_matches('='))
        .or_else(|_| PADDED_URL.decode(seg))
        .map_err(|e| e.to_string())?;
    decode_map(&raw)
}

pub(crate) fn used_pct_from(m: Option<&Obj>) -> Option<f64> {
    let m = m?;
    [
        "used_percentage",
        "used_percent",
        "utilization",
        "usedPercent",
        "percent",
    ]
    .iter()
    .find_map(|k| m.get(*k).filter(|v| !v.is_null()))
    .map(|v| clamp_pct(as_float(Some(v))))
}

/// A percent field clamped to 0..=100; `None` when absent or `null`.
pub(crate) fn clamp_pct_field(m: &Obj, keys: &[&str]) -> Option<f64> {
    first_any(m, keys).map(|v| clamp_pct(as_float(Some(v))))
}
