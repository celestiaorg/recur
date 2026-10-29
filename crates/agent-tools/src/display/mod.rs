//! Display parsing for Recur tool cards (permissions, plans, briefings, open PR,
//! reenter phase). Tool inputs arrive as parsed JSON values; trimming uses
//! JavaScript's whitespace set for `trim()` and `\s`.

pub mod briefing_tools;
pub mod open_pr_tool;
pub mod permission_tools;
pub mod plan_tools;
pub mod reenter_phase_tool;

use serde_json::Value;

/// JavaScript `\s` (WhiteSpace and LineTerminator).
pub(crate) fn js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// JavaScript `String.prototype.trim`.
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(js_space)
}

/// JavaScript `s.replace(/\s+/g, ' ')`.
pub(crate) fn collapse_space(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_space = false;
    for c in s.chars() {
        if js_space(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// A trimmed string field, or empty when the value is not a string.
pub(crate) fn str_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => js_trim(s).to_string(),
        _ => String::new(),
    }
}

/// A non-empty Recur tool name or its `mcp__<server>__` form.
pub(crate) fn is_tool(tool: Option<&str>, bare: &str) -> bool {
    tool.is_some_and(|t| t == bare || t.ends_with(&format!("__{bare}")))
}

/// JavaScript `/^prefix/i` for an ASCII prefix (non-ASCII never folds to ASCII).
pub(crate) fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// JavaScript `/needle/i` for an ASCII needle.
pub(crate) fn contains_ci(s: &str, needle: &str) -> bool {
    s.to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}
