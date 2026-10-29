//! JavaScript string and regex semantics the view rules use. JS `\s`
//! and `String.prototype.trim` use ECMAScript WhiteSpace + LineTerminator,
//! which differs from Rust's `char::is_whitespace` (JS adds U+FEFF and drops
//! U+0085); JS `\w`, `\d` and `\b` are ASCII.

use serde_json::Value;

/// The members of JS `\s`, for use inside a regex character class.
pub const WS_CLASS: &str = r"\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";

/// JS `\s` for one character.
pub fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// JS `\w` for one character.
pub fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `String.prototype.trim`.
pub fn trim(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `String.prototype.trimEnd`.
pub fn trim_end(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

/// The character before byte offset `i` (`None` at the start).
pub fn char_before(s: &str, i: usize) -> Option<char> {
    s[..i].chars().next_back()
}

/// The character at byte offset `i` (`None` at the end).
pub fn char_at(s: &str, i: usize) -> Option<char> {
    s[i..].chars().next()
}

/// `(?:^|(?<=\s))` at byte offset `i`.
pub fn space_before(s: &str, i: usize) -> bool {
    char_before(s, i).is_none_or(is_space)
}

/// `(?=\s|$)` at byte offset `i`.
pub fn space_after(s: &str, i: usize) -> bool {
    char_at(s, i).is_none_or(is_space)
}

/// A JSON number rendered the way JS `String(n)` does for the integers the
/// wire carries.
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

/// `String(v ?? '')` for a JSON value.
pub fn string_of(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => number_string(n),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| match x {
                Value::Null => String::new(),
                x => string_of(Some(x)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
    }
}

/// JS truthiness of a JSON value.
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// `typeof v === 'string' ? v : ''`.
pub fn str_field(v: Option<&Value>) -> &str {
    v.and_then(Value::as_str).unwrap_or_default()
}

/// `typeof v === 'number' ? v : 0` for the integer fields the wire carries.
pub fn num_field(v: Option<&Value>) -> i64 {
    v.and_then(|x| x.as_i64().or_else(|| x.as_f64().map(|f| f as i64)))
        .unwrap_or_default()
}

/// `JSON.parse` that only accepts a plain object.
pub fn parse_object(s: &str) -> Option<serde_json::Map<String, Value>> {
    match serde_json::from_str::<Value>(s) {
        Ok(Value::Object(o)) => Some(o),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_follows_ecmascript() {
        assert!(is_space('\u{FEFF}'));
        assert!(!is_space('\u{85}'));
        assert_eq!(trim("\u{FEFF} a \u{A0}"), "a");
        assert_eq!(trim("\u{85}a"), "\u{85}a");
    }

    #[test]
    fn strings_of_json_values() {
        assert_eq!(string_of(Some(&serde_json::json!(12))), "12");
        assert_eq!(string_of(Some(&serde_json::json!(2.0))), "2");
        assert_eq!(string_of(Some(&serde_json::json!(null))), "");
        assert_eq!(string_of(None), "");
        assert!(truthy(Some(&serde_json::json!("x"))));
        assert!(!truthy(Some(&serde_json::json!(0))));
    }
}
