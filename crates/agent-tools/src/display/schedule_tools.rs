//! schedule_wakeup and schedule tool chat cards, and the delay rules the
//! wakeup tool and card share.

use serde_json::Value;

use super::is_tool;

const TOOLS: [&str; 4] = [
    "schedule_wakeup",
    "create_schedule",
    "list_schedules",
    "delete_schedule",
];

pub fn is_schedule_tool(tool: &str) -> bool {
    TOOLS.iter().any(|t| is_tool(Some(tool), t))
}

/// schedule_wakeup's shortest and longest delay.
pub const WAKEUP_MIN_SECS: i64 = 60;
pub const WAKEUP_MAX_SECS: i64 = 24 * 60 * 60;

pub fn clamp_wakeup_delay(secs: i64) -> i64 {
    secs.clamp(WAKEUP_MIN_SECS, WAKEUP_MAX_SECS)
}

/// "10 min", "2 h", "1 h 30 min" (rounded to the minute).
pub fn delay_label(secs: i64) -> String {
    let mins = (secs + 30) / 60;
    match (mins / 60, mins % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// The one-line row: "Check back in 10 min · reason", "Created schedule
/// Nightly", "Listed schedules · 2", "Deleted schedule Nightly".
pub fn schedule_tool_label(
    tool: &str,
    input: &Value,
    result: &str,
    pending: bool,
    failed: bool,
) -> String {
    let result: Value = serde_json::from_str(result).unwrap_or_default();
    let named = |verb: &str, v: &Value| match v.get("name").and_then(Value::as_str) {
        Some(n) if !n.trim().is_empty() => format!("{verb} schedule {}", n.trim()),
        _ => format!("{verb} schedule"),
    };
    let pick = |a: &str, b: &str| if pending { a } else { b }.to_owned();
    if is_tool(Some(tool), "create_schedule") {
        let edit = input
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty());
        if failed {
            return if edit {
                "Edit schedule failed"
            } else {
                "Create schedule failed"
            }
            .into();
        }
        let (doing, done) = if edit {
            ("Editing", "Edited")
        } else {
            ("Creating", "Created")
        };
        return named(&pick(doing, done), input);
    }
    if is_tool(Some(tool), "delete_schedule") {
        if failed {
            return "Delete schedule failed".into();
        }
        return named(&pick("Deleting", "Deleted"), &result);
    }
    if is_tool(Some(tool), "list_schedules") {
        if failed {
            return "List schedules failed".into();
        }
        return match result.get("count").and_then(Value::as_i64) {
            Some(n) if !pending => format!("Listed schedules · {n}"),
            _ => pick("Listing schedules", "Listed schedules"),
        };
    }
    wakeup_label(input, failed)
}

/// The collapsed-row title: "Check back in 10 min · reason".
fn wakeup_label(input: &Value, failed: bool) -> String {
    if failed {
        return "Check back failed".into();
    }
    let secs = input.get("delay_seconds").and_then(Value::as_i64);
    let reason = input
        .get("reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let head = match secs {
        Some(s) => format!("Check back in {}", delay_label(clamp_wakeup_delay(s))),
        None => "Check back later".into(),
    };
    if reason.is_empty() {
        head
    } else {
        format!("{head} · {reason}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn labels() {
        assert_eq!(delay_label(600), "10 min");
        assert_eq!(delay_label(3600), "1 h");
        assert_eq!(delay_label(5400), "1 h 30 min");
        assert_eq!(clamp_wakeup_delay(5), 60);
        assert_eq!(clamp_wakeup_delay(1_000_000), WAKEUP_MAX_SECS);
        assert_eq!(
            wakeup_label(&json!({"delay_seconds": 600, "reason": " CI run "}), false),
            "Check back in 10 min · CI run"
        );
        assert_eq!(
            wakeup_label(&json!({"delay_seconds": 1}), false),
            "Check back in 1 min"
        );
        assert_eq!(wakeup_label(&json!({}), true), "Check back failed");
    }

    #[test]
    fn schedule_rows() {
        let none = Value::Null;
        let l = |tool, input: &Value, result, pending, failed| {
            schedule_tool_label(tool, input, result, pending, failed)
        };
        let create = json!({"name": "Nightly"});
        assert_eq!(
            l("mcp__recur__create_schedule", &create, "", true, false),
            "Creating schedule Nightly"
        );
        assert_eq!(
            l("mcp__recur__create_schedule", &create, "{}", false, false),
            "Created schedule Nightly"
        );
        assert_eq!(
            l(
                "mcp__recur__create_schedule",
                &create,
                "bad cron",
                false,
                true
            ),
            "Create schedule failed"
        );
        let edit = json!({"name": "Nightly", "id": "s1"});
        assert_eq!(
            l("mcp__recur__create_schedule", &edit, "{}", false, false),
            "Edited schedule Nightly"
        );
        assert_eq!(
            l(
                "mcp__recur__list_schedules",
                &none,
                r#"{"count":2}"#,
                false,
                false
            ),
            "Listed schedules · 2"
        );
        assert_eq!(
            l(
                "mcp__recur__delete_schedule",
                &json!({"id": "s1"}),
                r#"{"name":"Nightly"}"#,
                false,
                false
            ),
            "Deleted schedule Nightly"
        );
        assert_eq!(
            l(
                "mcp__recur__schedule_wakeup",
                &json!({"delay_seconds": 120}),
                "",
                false,
                false
            ),
            "Check back in 2 min"
        );
        assert!(is_schedule_tool("mcp__recur__list_schedules"));
        assert!(!is_schedule_tool("mcp__recur__list_findings"));
    }
}
