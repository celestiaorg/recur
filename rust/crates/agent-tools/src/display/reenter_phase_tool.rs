//! mcp__recur__reenter_phase chat cards (`lib/reenter-phase-tool.ts`): never
//! the args JSON, never the engine ack.

use serde_json::Value;

use super::{is_tool, js_trim, starts_with_ci};

pub fn is_reenter_phase_tool(tool: Option<&str>) -> bool {
    is_tool(tool, "reenter_phase")
}

/// `phase_id` (or `phaseId`), trimmed.
pub fn parse_reenter_phase_id(raw: &Value) -> String {
    let Value::Object(o) = raw else {
        return String::new();
    };
    let v = match o.get("phase_id") {
        Some(v) if !v.is_null() => Some(v),
        _ => o.get("phaseId"),
    };
    match v {
        Some(Value::String(s)) => js_trim(s).to_string(),
        _ => String::new(),
    }
}

/// Engine refusals count as failures; the restart ack does not.
pub fn reenter_phase_failed(result: &str, error: bool) -> bool {
    if error {
        return true;
    }
    let r = js_trim(result);
    !r.is_empty() && (starts_with_ci(r, "Refused:") || starts_with_ci(r, "reenter_phase:"))
}

/// The collapsed-row title.
pub fn reenter_phase_label(pending: bool, failed: bool, phase: &str) -> String {
    if failed {
        return "Re-enter failed".into();
    }
    match (pending, phase.is_empty()) {
        (true, false) => format!("Re-entering {phase}"),
        (true, true) => "Re-entering phase".into(),
        (false, false) => format!("Re-entered {phase}"),
        (false, true) => "Re-entered phase".into(),
    }
}

// Port of reenter-phase-tool.test.ts.
#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn matches_recur_mcp_and_bare_names() {
        assert!(is_reenter_phase_tool(Some("mcp__recur__reenter_phase")));
        assert!(is_reenter_phase_tool(Some("reenter_phase")));
        assert!(!is_reenter_phase_tool(Some("mcp__recur__run_phase")));
        assert!(!is_reenter_phase_tool(None));
    }

    #[test]
    fn reads_phase_id_and_ignores_other_args() {
        assert_eq!(
            parse_reenter_phase_id(&json!({"phase_id": "converge"})),
            "converge"
        );
        assert_eq!(
            parse_reenter_phase_id(&json!({"phaseId": "review"})),
            "review"
        );
        assert_eq!(
            parse_reenter_phase_id(&json!({"phase_id": "  plan  "})),
            "plan"
        );
        assert_eq!(parse_reenter_phase_id(&json!({})), "");
        assert_eq!(parse_reenter_phase_id(&Value::Null), "");
    }

    #[test]
    fn names_the_phase_without_json_or_ack() {
        assert_eq!(
            reenter_phase_label(true, false, "converge"),
            "Re-entering converge"
        );
        assert_eq!(
            reenter_phase_label(false, false, "converge"),
            "Re-entered converge"
        );
        assert_eq!(
            reenter_phase_label(false, true, "converge"),
            "Re-enter failed"
        );
        assert_eq!(reenter_phase_label(false, false, ""), "Re-entered phase");
        let label = reenter_phase_label(false, false, "converge");
        assert!(
            !label.contains('{')
                && !label.contains("phase_id")
                && !label.contains("Loop restarting")
        );
    }

    #[test]
    fn refusals_fail_restart_ack_does_not() {
        assert!(!reenter_phase_failed(
            "Loop restarting — it continues from the current worktree, steered by the recorded plan.",
            false
        ));
        assert!(reenter_phase_failed(
            "Refused: reenter_phase is idle only.",
            false
        ));
        assert!(reenter_phase_failed(
            "Refused: converge is not on this loop's reenter list.",
            false
        ));
        assert!(reenter_phase_failed(
            "reenter_phase: phase_id is required",
            false
        ));
        assert!(reenter_phase_failed("", true));
    }
}
