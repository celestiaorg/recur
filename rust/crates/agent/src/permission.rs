//! Human-facing permission prompt text, shared by every provider that asks
//! (Claude's prompt tool, Codex approvals, the compatible Bash gate).

use serde_json::Value;

use crate::gostr::byte_prefix;

/// The MCP tool Claude Code calls to ask permission under
/// `--permission-mode default`. Claude invokes it itself, so unlike every
/// other `mcp__recur__*` tool it never appears in a turn's allowlist.
pub const PROMPT_TOOL: &str = "mcp__recur__permission_request";

/// Titles for `mcp__recur__*` tools in prompts and inbox rows.
pub const RECUR_TOOL_LABELS: &[(&str, &str)] = &[
    ("report_finding", "Report finding"),
    ("list_findings", "List findings"),
    ("update_finding", "Update finding"),
    ("done", "Complete turn"),
    ("report_result", "Report result"),
    ("plan", "Record plan"),
    ("escalate", "Escalate"),
    ("ask_user", "Ask user"),
    ("switch_mode", "Switch mode"),
    ("create_artifact", "Create artifact"),
    ("list_artifacts", "List artifacts"),
    ("read_artifact", "Read artifact"),
    ("present_artifact", "Present artifact"),
    ("rename_session", "Rename chat"),
    ("rename_workspace", "Rename workspace"),
    ("rename_branch", "Rename branch"),
    ("open_pr", "Open PR"),
    ("create_workspace", "Create workspace"),
    ("merge_child", "Merge child workspace"),
    ("read_child", "Read child workspace"),
    ("archive_child", "Archive child workspace"),
    ("message_child", "Message child workspace"),
    ("attach_evidence", "Attach evidence"),
    ("edit_plan", "Edit plan"),
    ("rerun_review", "Re-run review"),
    ("dispatch_specialist", "Dispatch specialist"),
    ("extend_iterations", "Extend iterations"),
    ("resume_loop", "Resume loop"),
    ("submit_plan", "Submit plan"),
    ("update_plan_step", "Update plan step"),
    ("list_skills", "List skills"),
    ("activate_skill", "Activate skill"),
    ("list_terminals", "List terminals"),
    ("read_terminal", "Read terminal"),
    ("list_workspace_briefs", "List workspaces"),
    ("get_workspace_context", "Get workspace"),
    ("present_workspace", "Present workspace"),
];

/// The human-facing tool name: Recur MCP tools get a short title, other
/// `mcp__server__tool` names a spaced tool segment, everything else (Bash,
/// Write) stays as is.
pub fn tool_label(tool: &str) -> String {
    let tool = tool.trim();
    if tool.is_empty() {
        return String::new();
    }
    if let Some(name) = tool.strip_prefix("mcp__recur__") {
        if let Some((_, label)) = RECUR_TOOL_LABELS.iter().find(|(n, _)| *n == name) {
            return (*label).to_owned();
        }
        return humanize_snake(name);
    }
    if tool.starts_with("mcp__") {
        return humanize_snake(tool.rsplit("__").next().unwrap_or_default());
    }
    tool.to_owned()
}

fn humanize_snake(s: &str) -> String {
    let s = s.replace('_', " ");
    let s = s.trim();
    let mut c = s.chars();
    match c.next() {
        Some(f) if f.is_ascii_lowercase() => format!("{}{}", f.to_ascii_uppercase(), c.as_str()),
        _ => s.to_owned(),
    }
}

/// One line per tool call: the label plus the field that tells the human
/// what will happen, so the yes/no is informed without expanding anything.
pub fn summary(tool: &str, input: &str) -> String {
    let label = tool_label(tool);
    let fields = match serde_json::from_str::<Value>(input) {
        Ok(Value::Object(m)) if !m.is_empty() => m,
        _ => return label,
    };
    for k in [
        "command",
        "file_path",
        "path",
        "url",
        "pattern",
        "query",
        "status",
        "summary",
        "message",
        "finding_id",
        "name",
        "role",
        "instruction",
        "step_id",
        "plan",
        "title",
    ] {
        if let Some(Value::String(s)) = fields.get(k)
            && !s.trim().is_empty()
        {
            return format!("{label}: {}", truncate_line(s, 160));
        }
    }
    let mut keys: Vec<&str> = fields.keys().map(String::as_str).collect();
    keys.sort_unstable();
    format!("{label} ({})", keys.join(", "))
}

fn truncate_line(s: &str, max: usize) -> String {
    let mut s = s.trim().to_owned();
    if let Some(i) = s.find('\n') {
        s.truncate(i);
        s.push_str(" …");
    }
    if s.len() > max {
        return format!("{}…", byte_prefix(&s, max));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        for (tool, want) in [
            ("Bash", "Bash"),
            ("mcp__recur__list_findings", "List findings"),
            ("mcp__recur__report_finding", "Report finding"),
            ("mcp__recur__activate_skill", "Activate skill"),
            ("mcp__recur__message_child", "Message child workspace"),
            ("mcp__recur__unknown_widget", "Unknown widget"),
            ("mcp__other__do_thing", "Do thing"),
            ("  ", ""),
        ] {
            assert_eq!(tool_label(tool), want, "{tool}");
        }
    }

    #[test]
    fn summary_shows_the_deciding_detail() {
        for (tool, input, want) in [
            (
                "Bash",
                r#"{"command":"rm -rf build","description":"clean"}"#,
                "Bash: rm -rf build",
            ),
            (
                "Write",
                r#"{"file_path":"/tmp/x.go","content":"package x"}"#,
                "Write: /tmp/x.go",
            ),
            (
                "WebFetch",
                r#"{"url":"https://example.com"}"#,
                "WebFetch: https://example.com",
            ),
            ("Weird", r#"{"zeta":1,"alpha":2}"#, "Weird (alpha, zeta)"),
            ("Broken", "not json", "Broken"),
            ("Empty", "{}", "Empty"),
            (
                "mcp__recur__list_findings",
                r#"{"status":"open"}"#,
                "List findings: open",
            ),
            (
                "mcp__recur__update_finding",
                r#"{"finding_id":"F1","status":"resolved"}"#,
                "Update finding: resolved",
            ),
            (
                "mcp__recur__done",
                r#"{"summary":"wired the toggle"}"#,
                "Complete turn: wired the toggle",
            ),
            (
                "mcp__recur__open_pr",
                r#"{"title":"Ship the gateway","body":"long"}"#,
                "Open PR: Ship the gateway",
            ),
            (
                "mcp__recur__create_workspace",
                r#"{"title":"ACME-12: Fix login","prompt":"long"}"#,
                "Create workspace: ACME-12: Fix login",
            ),
        ] {
            assert_eq!(summary(tool, input), want, "{tool} {input}");
        }
        let got = summary("Bash", r#"{"command":"line one\nline two"}"#);
        assert_eq!(got, "Bash: line one …");
    }

    #[test]
    fn summary_caps_long_values() {
        let got = summary("Bash", &format!(r#"{{"command":"{}"}}"#, "x".repeat(200)));
        assert_eq!(got, format!("Bash: {}…", "x".repeat(160)));
    }
}
