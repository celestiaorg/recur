//! submit_plan / update_plan_step / edit_plan chat cards (`lib/plantools.ts`):
//! friendly labels, never a JSON dump.

use std::collections::HashSet;

use serde_json::Value;

use super::{collapse_space, is_tool, js_trim};

/// One checklist row parsed from submit_plan args.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStepView {
    pub id: String,
    pub title: String,
    pub status: String,
}

pub fn is_submit_plan_tool(tool: Option<&str>) -> bool {
    is_tool(tool, "submit_plan")
}

pub fn is_update_plan_step_tool(tool: Option<&str>) -> bool {
    is_tool(tool, "update_plan_step")
}

pub fn is_edit_plan_tool(tool: Option<&str>) -> bool {
    is_tool(tool, "edit_plan")
}

/// Row title for submit_plan (progress lives in the label, not meta).
pub fn submit_plan_label(pending: bool, step_count: usize) -> String {
    if pending {
        return "Submitting plan".into();
    }
    match step_count {
        0 => "Submitted plan".into(),
        1 => "Submitted plan · 1 step".into(),
        n => format!("Submitted plan · {n} steps"),
    }
}

pub fn update_plan_step_label(pending: bool) -> &'static str {
    if pending {
        "Updating plan step"
    } else {
        "Updated plan step"
    }
}

/// Row title for edit_plan (rewrites the loop's definition of done).
pub fn edit_plan_label(pending: bool) -> &'static str {
    if pending {
        "Editing plan"
    } else {
        "Edited plan"
    }
}

/// First non-blank line with whitespace collapsed: collapsed-row meta.
pub fn prose_preview(text: &str) -> String {
    text.split('\n')
        .find(|l| !js_trim(l).is_empty())
        .map(|l| js_trim(&collapse_space(l)).to_string())
        .unwrap_or_default()
}

fn string_field(raw: &Value, key: &str) -> String {
    match raw.get(key) {
        Some(Value::String(s)) => js_trim(s).to_string(),
        _ => String::new(),
    }
}

pub fn parse_submit_plan_body(raw: &Value) -> String {
    string_field(raw, "body")
}

/// Attachment ids from submit_plan args: strings only, trimmed, deduped.
pub fn parse_submit_plan_attachments(raw: &Value) -> Vec<String> {
    let Value::Array(items) = raw else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    items
        .iter()
        .filter_map(Value::as_str)
        .map(js_trim)
        .filter(|id| !id.is_empty() && seen.insert(id.to_string()))
        .map(String::from)
        .collect()
}

pub fn parse_edit_plan(raw: &Value) -> String {
    string_field(raw, "plan")
}

/// Collapsed-row meta: "S2 → done · optional note".
pub fn update_plan_step_meta(step_id: &str, status: &str, note: &str) -> String {
    let n = collapse_space(note);
    let n = js_trim(&n);
    let mut parts: Vec<String> = Vec::new();
    match (step_id.is_empty(), status.is_empty()) {
        (false, false) => parts.push(format!("{step_id} → {status}")),
        (false, true) => parts.push(step_id.into()),
        (true, false) => parts.push(status.into()),
        (true, true) => {}
    }
    if !n.is_empty() {
        parts.push(n.into());
    }
    parts.join(" · ")
}

/// submit_plan `steps` as checklist rows: strings, or legacy objects with
/// thought/title/content and optional step_id/id/status.
pub fn parse_submit_plan_steps(raw: &Value) -> Vec<PlanStepView> {
    let Value::Array(items) = raw else {
        return Vec::new();
    };
    let mut out: Vec<PlanStepView> = Vec::new();
    for s in items {
        match s {
            Value::String(t) => {
                let title = js_trim(t);
                if title.is_empty() {
                    continue;
                }
                out.push(PlanStepView {
                    id: format!("S{}", out.len() + 1),
                    title: title.into(),
                    status: "pending".into(),
                });
            }
            Value::Object(o) => {
                let title = ["thought", "title", "content"]
                    .iter()
                    .find_map(|k| o.get(*k).and_then(Value::as_str))
                    .map(js_trim)
                    .unwrap_or_default();
                if title.is_empty() {
                    continue;
                }
                let non_empty =
                    |k: &str| o.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
                let id = non_empty("step_id")
                    .or_else(|| non_empty("id"))
                    .map_or_else(|| format!("S{}", out.len() + 1), String::from);
                let status = non_empty("status").unwrap_or("pending").to_string();
                out.push(PlanStepView {
                    id,
                    title: title.into(),
                    status,
                });
            }
            _ => {}
        }
    }
    out
}

/// The step title from the engine ack "Step S3 (title…) → done.".
pub fn parse_update_plan_step_title(result: &str) -> String {
    let r = js_trim(result);
    if r.is_empty() {
        return String::new();
    }
    let arrow = if r.contains('→') { "→" } else { "->" };
    let close = format!(") {arrow} ");
    let (Some(open), Some(end)) = (r.find(" ("), r.rfind(&close)) else {
        return String::new();
    };
    if end <= open {
        return String::new();
    }
    js_trim(&r[open + 2..end]).to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn detectors_and_labels() {
        assert!(is_submit_plan_tool(Some("mcp__recur__submit_plan")));
        assert!(is_update_plan_step_tool(Some("update_plan_step")));
        assert!(!is_edit_plan_tool(Some("mcp__recur__submit_plan")));
        assert_eq!(submit_plan_label(true, 3), "Submitting plan");
        assert_eq!(submit_plan_label(false, 1), "Submitted plan · 1 step");
        assert_eq!(submit_plan_label(false, 3), "Submitted plan · 3 steps");
        assert_eq!(submit_plan_label(false, 0), "Submitted plan");
        assert_eq!(
            update_plan_step_meta("S2", "done", "  was\n split  "),
            "S2 → done · was split"
        );
        assert_eq!(update_plan_step_meta("", "done", ""), "done");
    }

    #[test]
    fn parses_steps_and_args() {
        let steps = parse_submit_plan_steps(&json!([
            " First ",
            "",
            {"title": "Second", "status": "done"},
            {"thought": "", "title": "ignored when thought is a string"},
            {"content": "Third", "step_id": "S9"},
            5
        ]));
        assert_eq!(
            steps,
            vec![
                PlanStepView {
                    id: "S1".into(),
                    title: "First".into(),
                    status: "pending".into()
                },
                PlanStepView {
                    id: "S2".into(),
                    title: "Second".into(),
                    status: "done".into()
                },
                PlanStepView {
                    id: "S9".into(),
                    title: "Third".into(),
                    status: "pending".into()
                },
            ]
        );
        assert_eq!(
            parse_submit_plan_attachments(&json!([" a1 ", "a1", 3, "", "a2"])),
            ["a1", "a2"]
        );
        assert_eq!(parse_submit_plan_body(&json!({"body": " B "})), "B");
        assert_eq!(parse_edit_plan(&json!({"plan": 5})), "");
        assert_eq!(prose_preview("\n  \n first   line \nsecond"), "first line");
    }

    #[test]
    fn recovers_step_title_from_ack() {
        assert_eq!(
            parse_update_plan_step_title("Step S3 (Wire it (twice)) → done."),
            "Wire it (twice)"
        );
        assert_eq!(
            parse_update_plan_step_title("Step S3 (Wire) -> done."),
            "Wire"
        );
        assert_eq!(parse_update_plan_step_title("Step updated."), "");
    }
}
