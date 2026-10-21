//! Human-facing tool names on permission prompts (`lib/permissionTools.ts`).
//! The wire field stays the raw tool name (remember/approve key off it);
//! only display goes through here.

use super::js_trim;
use crate::names::RECUR_MCP_PREFIX;

fn recur_tool_label(name: &str) -> Option<&'static str> {
    Some(match name {
        "report_finding" => "Report finding",
        "list_findings" => "List findings",
        "update_finding" => "Update finding",
        "done" => "Complete turn",
        "report_result" => "Report result",
        "plan" => "Record plan",
        "escalate" => "Escalate",
        "ask_user" => "Ask user",
        "switch_mode" => "Switch mode",
        "create_artifact" => "Create artifact",
        "list_artifacts" => "List artifacts",
        "read_artifact" => "Read artifact",
        "present_artifact" => "Present artifact",
        "rename_session" => "Rename chat",
        "rename_workspace" => "Rename workspace",
        "rename_branch" => "Rename branch",
        "open_pr" => "Open PR",
        "create_workspace" => "Create workspace",
        "merge_child" => "Merge child workspace",
        "read_child" => "Read child workspace",
        "archive_child" => "Archive child workspace",
        "message_child" => "Message child workspace",
        "attach_evidence" => "Attach evidence",
        "edit_plan" => "Edit plan",
        "rerun_review" => "Re-run review",
        "dispatch_specialist" => "Dispatch specialist",
        "extend_iterations" => "Extend iterations",
        "resume_loop" => "Resume loop",
        "submit_plan" => "Submit plan",
        "update_plan_step" => "Update plan step",
        "list_skills" => "List skills",
        "activate_skill" => "Activate skill",
        _ => return None,
    })
}

fn humanize_snake(s: &str) -> String {
    let spaced = s.replace('_', " ");
    let spaced = js_trim(&spaced);
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Friendly tool title for permission cards (`mcp__recur__list_findings` →
/// "List findings"; other MCP servers humanize their last segment).
pub fn permission_tool_label(tool: &str) -> String {
    let t = js_trim(tool);
    if t.is_empty() {
        return String::new();
    }
    if let Some(name) = t.strip_prefix(RECUR_MCP_PREFIX) {
        return recur_tool_label(name).map_or_else(|| humanize_snake(name), String::from);
    }
    if t.starts_with("mcp__") {
        return humanize_snake(t.rsplit("__").next().unwrap_or(t));
    }
    t.to_string()
}

#[cfg(test)]
mod tests {
    use super::permission_tool_label;

    #[test]
    fn labels() {
        assert_eq!(
            permission_tool_label("mcp__recur__list_findings"),
            "List findings"
        );
        assert_eq!(permission_tool_label("mcp__recur__done"), "Complete turn");
        assert_eq!(
            permission_tool_label("mcp__recur__get_workspace_context"),
            "Get workspace context"
        );
        assert_eq!(
            permission_tool_label("mcp__playwright__browser_navigate"),
            "Browser navigate"
        );
        assert_eq!(permission_tool_label("  Bash "), "Bash");
        assert_eq!(permission_tool_label(""), "");
        assert_eq!(permission_tool_label("mcp__recur__"), "");
    }
}
