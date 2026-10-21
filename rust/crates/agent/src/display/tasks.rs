//! Claude Code TaskCreate / TaskUpdate / TaskGet / TaskList card rules
//! (frontend `lib/tasktools.ts`). These are not the Task subagent tool:
//! TaskCreate's `description` is the task BODY and never the row title.

use serde_json::Value;

use crate::gostr::{js_space, js_trim};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeTaskView {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub status: String,
    pub owner: String,
    pub blocks: Vec<String>,
    pub blocked_by: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeTaskArgs {
    pub task_id: String,
    pub subject: String,
    pub description: String,
    pub status: String,
    pub owner: String,
    pub add_blocks: Vec<String>,
    pub add_blocked_by: Vec<String>,
}

/// Options for [`claude_task_has_body`].
#[derive(Debug, Clone, Copy, Default)]
pub struct BodyOpts<'a> {
    pub result_error: bool,
    pub result: &'a str,
    pub pending: bool,
}

pub fn is_claude_task_tool(tool: &str) -> bool {
    matches!(tool, "TaskCreate" | "TaskUpdate" | "TaskGet" | "TaskList")
}

pub fn format_task_id(id: &str) -> String {
    let t = js_trim(id);
    if t.is_empty() {
        return String::new();
    }
    if t.starts_with('#') {
        t.to_owned()
    } else {
        format!("#{t}")
    }
}

/// Chip copy: "in_progress" → "in progress".
pub fn pretty_task_status(status: &str) -> String {
    js_trim(&status.replace('_', " ")).to_owned()
}

/// The `.tc-finding-status` chip variant for a task status.
pub fn task_status_chip_class(status: &str) -> &str {
    match status {
        "in_progress" => "doing",
        "completed" => "done",
        "deleted" => "invalid",
        _ => status,
    }
}

fn str_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => js_trim(s).to_owned(),
        _ => String::new(),
    }
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(items)) => items
            .iter()
            .map(|x| str_of(Some(x)))
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn first_nonempty(candidates: impl IntoIterator<Item = String>) -> String {
    candidates
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if js_space(c) {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(c);
    }
    out
}

pub fn parse_claude_task_args(input: Option<&Value>) -> ClaudeTaskArgs {
    let get = |k: &str| input.and_then(|v| v.get(k));
    ClaudeTaskArgs {
        task_id: first_nonempty([str_of(get("taskId")), str_of(get("task_id"))]),
        subject: str_of(get("subject")),
        description: str_of(get("description")),
        status: str_of(get("status")),
        owner: str_of(get("owner")),
        add_blocks: str_list(get("addBlocks")),
        add_blocked_by: str_list(get("addBlockedBy")),
    }
}

/// Strips an ASCII-case-insensitive prefix.
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

/// `^Task #(\S+) created successfully` (case-insensitive): the id.
fn task_created_id(t: &str) -> Option<&str> {
    let rest = strip_prefix_ci(t, "Task #")?;
    let end = rest.find(js_space).unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    strip_prefix_ci(&rest[end..], " created successfully")?;
    Some(&rest[..end])
}

/// The id from a TaskCreate result: "Task #1 created successfully: …" or a
/// JSON ack.
pub fn parse_task_create_id(result: &str) -> String {
    let t = js_trim(result);
    if t.is_empty() {
        return String::new();
    }
    if let Some(id) = task_created_id(t) {
        return id.to_owned();
    }
    if let Ok(raw) = serde_json::from_str::<Value>(t) {
        return first_nonempty([
            str_of(raw.get("taskId")),
            str_of(raw.get("id")),
            str_of(raw.get("task").and_then(|x| x.get("id"))),
        ]);
    }
    String::new()
}

fn as_task_view(raw: Option<&Value>) -> Option<ClaudeTaskView> {
    let o = raw.filter(|v| v.is_object() || v.is_array())?;
    let get = |k: &str| o.get(k);
    let id = first_nonempty([str_of(get("id")), str_of(get("taskId"))]);
    let subject = first_nonempty([
        str_of(get("subject")),
        str_of(get("title")),
        str_of(get("content")),
    ]);
    if id.is_empty() && subject.is_empty() {
        return None;
    }
    Some(ClaudeTaskView {
        id,
        subject,
        description: str_of(get("description")),
        status: first_nonempty([str_of(get("status")), "pending".to_owned()]),
        owner: str_of(get("owner")),
        blocks: str_list(get("blocks")),
        blocked_by: str_list(get("blockedBy")),
    })
}

/// A TaskList result: JSON `{tasks:[…]}` or `#1 [pending] Subject` lines.
/// `Some(vec![])` is a known empty list; `None` means no known shape
/// matched (show raw).
pub fn parse_task_list(result: &str) -> Option<Vec<ClaudeTaskView>> {
    let t = js_trim(result);
    if t.is_empty() || t.eq_ignore_ascii_case("no tasks found") {
        return Some(Vec::new());
    }
    if let Ok(raw) = serde_json::from_str::<Value>(t) {
        let list = match &raw {
            Value::Array(a) => Some(a),
            _ => match (
                raw.get("tasks"),
                raw.get("data").and_then(|d| d.get("tasks")),
            ) {
                (Some(Value::Array(a)), _) | (_, Some(Value::Array(a))) => Some(a),
                _ => None,
            },
        };
        return list.map(|l| l.iter().filter_map(|v| as_task_view(Some(v))).collect());
    }
    let out: Vec<_> = t.split('\n').filter_map(parse_task_list_line).collect();
    (!out.is_empty()).then_some(out)
}

fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// The start of the maximal JS-whitespace run ending at byte `at`; `None`
/// when no whitespace precedes it.
fn space_run_start(s: &str, at: usize) -> Option<usize> {
    let start = s[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| js_space(*c))
        .last()
        .map(|(i, _)| i)?;
    Some(start)
}

fn trim_end_js(s: &str) -> &str {
    s.trim_end_matches(js_space)
}

/// `^#(\S+)\s+\[([^\]]+)\]\s+(.*)$` on a trimmed line, then the trailing
/// `[blocked by …]` and `(owner)` suffixes.
fn parse_task_list_line(line: &str) -> Option<ClaudeTaskView> {
    let line = js_trim(line);
    let rest = line.strip_prefix('#')?;
    let id_end = rest.find(js_space).unwrap_or(rest.len());
    if id_end == 0 {
        return None;
    }
    let id = &rest[..id_end];
    let after = &rest[id_end..];
    let after_ws = after.trim_start_matches(js_space);
    if after_ws.len() == after.len() {
        return None;
    }
    let inner = after_ws.strip_prefix('[')?;
    let close = inner.find(']')?;
    if close == 0 {
        return None;
    }
    let status = &inner[..close];
    let tail = &inner[close + 1..];
    let tail_ws = tail.trim_start_matches(js_space);
    if tail_ws.len() == tail.len() || tail_ws.contains(is_line_terminator) {
        return None;
    }
    let mut rest = tail_ws;

    let mut blocked_by = Vec::new();
    if let Some((idx, inner)) = blocked_suffix(rest) {
        blocked_by = inner
            .split(',')
            .map(|s| js_trim(s.strip_prefix('#').unwrap_or(s)).to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        rest = trim_end_js(&rest[..idx]);
    }
    let mut owner = String::new();
    if let Some((idx, o)) = owner_suffix(rest) {
        owner = o.to_owned();
        rest = trim_end_js(&rest[..idx]);
    }
    Some(ClaudeTaskView {
        id: id.to_owned(),
        subject: rest.to_owned(),
        description: String::new(),
        status: status.to_owned(),
        owner,
        blocks: Vec::new(),
        blocked_by,
    })
}

/// `\s+\[blocked by ([^\]]+)\]\s*$` (case-insensitive): match index and the
/// captured list.
fn blocked_suffix(s: &str) -> Option<(usize, &str)> {
    const OPEN: &str = "[blocked by ";
    let body = trim_end_js(s);
    let inner_end = body.len().checked_sub(1).filter(|_| body.ends_with(']'))?;
    let lower = body.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(OPEN) {
        let p = from + i;
        let content = &body[p + OPEN.len()..inner_end.max(p + OPEN.len())];
        if !content.is_empty()
            && !content.contains(']')
            && let Some(start) = space_run_start(body, p)
        {
            return Some((start, content));
        }
        from = p + 1;
    }
    None
}

/// `\s+\(([\w.-]+)\)\s*$`: match index and the owner.
fn owner_suffix(s: &str) -> Option<(usize, &str)> {
    let body = trim_end_js(s);
    let inner = body.strip_suffix(')')?;
    let open = inner.rfind('(')?;
    let owner = &inner[open + 1..];
    if owner.is_empty()
        || !owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
    {
        return None;
    }
    Some((space_run_start(body, open)?, owner))
}

/// A TaskGet result: JSON, or the CLI's multiline ack
/// (`Task #1: Subject`, `Status:`, `Description:`, `Blocked by:`, `Blocks:`).
pub fn parse_task_get(result: &str) -> Option<ClaudeTaskView> {
    let t = js_trim(result);
    if t.is_empty() || t.eq_ignore_ascii_case("task not found") {
        return None;
    }
    if let Ok(raw) = serde_json::from_str::<Value>(t) {
        let task = match raw.get("task") {
            None | Some(Value::Null) => Some(&raw),
            some => some,
        };
        if let Some(view) = as_task_view(task) {
            return Some(view);
        }
    }
    let lines: Vec<&str> = t.split('\n').collect();
    let (id, subject) = task_get_head(lines[0])?;
    let mut i = 1;
    let mut status = "";
    if let Some(s) = lines.get(i).and_then(|l| l.strip_prefix("Status:")) {
        status = js_trim(s);
        i += 1;
    }
    let mut description = String::new();
    if let Some(d) = lines.get(i).and_then(|l| l.strip_prefix("Description:")) {
        description.push_str(d);
        i += 1;
        while i < lines.len()
            && !lines[i].starts_with("Blocked by:")
            && !lines[i].starts_with("Blocks:")
        {
            description.push('\n');
            description.push_str(lines[i]);
            i += 1;
        }
        description = js_trim(&description).to_owned();
    }
    let ids = |s: &str| -> Vec<String> {
        s.split(',')
            .map(|x| js_trim(&x.replace('#', "")).to_owned())
            .filter(|x| !x.is_empty())
            .collect()
    };
    let mut blocked_by = Vec::new();
    let mut blocks = Vec::new();
    for line in &lines[i..] {
        if let Some(s) = line.strip_prefix("Blocked by:") {
            blocked_by = ids(s);
        } else if let Some(s) = line.strip_prefix("Blocks:") {
            blocks = ids(s);
        }
    }
    Some(ClaudeTaskView {
        id: id.to_owned(),
        subject: js_trim(subject).to_owned(),
        description,
        status: if status.is_empty() { "pending" } else { status }.to_owned(),
        owner: String::new(),
        blocks,
        blocked_by,
    })
}

/// `^Task #(\S+):\s*(.*)$`: the id is the non-space run up to its last
/// colon (greedy `\S+` backtracks to the final `:`).
fn task_get_head(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("Task #")?;
    let run_end = rest.find(js_space).unwrap_or(rest.len());
    let colon = rest[..run_end].rfind(':').filter(|&c| c > 0)?;
    let subject = rest[colon + 1..].trim_start_matches(js_space);
    if subject.contains(is_line_terminator) {
        return None;
    }
    Some((&rest[..colon], subject))
}

/// (completed, total).
pub fn claude_task_counts(tasks: &[ClaudeTaskView]) -> (usize, usize) {
    (
        tasks.iter().filter(|t| t.status == "completed").count(),
        tasks.len(),
    )
}

/// The verb for the collapsed row: never the task body, never JSON.
pub fn claude_task_label(
    tool: &str,
    pending: bool,
    args: &ClaudeTaskArgs,
    list: Option<(usize, usize)>,
) -> String {
    let pick = |p: &str, d: &str| if pending { p } else { d }.to_owned();
    match tool {
        "TaskCreate" => pick("Creating task", "Created task"),
        "TaskUpdate" => match args.status.as_str() {
            "in_progress" => pick("Starting task", "Started task"),
            "completed" => pick("Completing task", "Completed task"),
            "deleted" => pick("Deleting task", "Deleted task"),
            _ => pick("Updating task", "Updated task"),
        },
        "TaskGet" => pick("Reading task", "Read task"),
        "TaskList" => match list {
            _ if pending => "Listing tasks".to_owned(),
            Some((done, total)) if total > 0 => format!("Listed tasks · {done}/{total}"),
            _ => "Listed tasks".to_owned(),
        },
        _ => pick("Updating task", "Updated task"),
    }
}

/// Dim header summary: the subject only; the `#id` is a chip.
pub fn claude_task_meta(tool: &str, args: &ClaudeTaskArgs, result: &str) -> String {
    match tool {
        "TaskCreate" | "TaskUpdate" => collapse(&args.subject),
        "TaskGet" => {
            let got = parse_task_get(result)
                .map(|v| v.subject)
                .unwrap_or_default();
            collapse(if got.is_empty() { &args.subject } else { &got })
        }
        "TaskList" => parse_task_list(result)
            .and_then(|l| l.into_iter().find(|t| t.status == "in_progress"))
            .map(|t| collapse(&t.subject))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// The header chip status (TaskUpdate args, or a TaskGet result).
pub fn claude_task_chip_status(tool: &str, args: &ClaudeTaskArgs, result: &str) -> String {
    match tool {
        "TaskUpdate" => args.status.clone(),
        "TaskGet" => parse_task_get(result).map(|v| v.status).unwrap_or_default(),
        _ => String::new(),
    }
}

/// Whether the expanded card says something the header does not.
/// Status-only TaskUpdate and subject-only TaskCreate stay one-liners.
pub fn claude_task_has_body(tool: &str, args: &ClaudeTaskArgs, opts: BodyOpts<'_>) -> bool {
    if opts.result_error {
        return true;
    }
    match tool {
        "TaskCreate" => !args.description.is_empty(),
        "TaskUpdate" => {
            !args.description.is_empty()
                || !args.owner.is_empty()
                || !args.add_blocks.is_empty()
                || !args.add_blocked_by.is_empty()
        }
        "TaskGet" => {
            if opts.pending {
                return true;
            }
            match parse_task_get(opts.result) {
                Some(g) => {
                    !g.description.is_empty()
                        || !g.owner.is_empty()
                        || !g.blocks.is_empty()
                        || !g.blocked_by.is_empty()
                }
                None => !js_trim(opts.result).is_empty(),
            }
        }
        "TaskList" => true,
        _ => false,
    }
}

/// Provider success acks that only restate the header (hidden).
pub fn claude_task_result_is_ack(tool: &str, result: &str) -> bool {
    let t = js_trim(result);
    if t.is_empty() {
        return true;
    }
    match tool {
        "TaskCreate" => task_created_id(t).is_some(),
        "TaskUpdate" => strip_prefix_ci(t, "Updated task #").is_some(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(v: Value) -> ClaudeTaskArgs {
        parse_claude_task_args(Some(&v))
    }

    fn create_args() -> ClaudeTaskArgs {
        args(json!({
            "subject": "Guard theme re-apply (registry + host)",
            "description": "setActiveTheme memoize; applyColorMap no-op on identical maps.",
            "activeForm": "Guarding theme re-apply",
        }))
    }

    #[test]
    fn task_tools_not_the_subagent() {
        for t in ["TaskCreate", "TaskUpdate", "TaskGet", "TaskList"] {
            assert!(is_claude_task_tool(t));
        }
        assert!(!is_claude_task_tool("Task") && !is_claude_task_tool("TodoWrite"));
    }

    #[test]
    fn create_never_uses_description_as_title() {
        let a = create_args();
        assert_eq!(
            claude_task_label("TaskCreate", false, &a, None),
            "Created task"
        );
        assert_eq!(
            claude_task_label("TaskCreate", true, &a, None),
            "Creating task"
        );
        assert_eq!(
            claude_task_meta("TaskCreate", &a, ""),
            "Guard theme re-apply (registry + host)"
        );
    }

    #[test]
    fn update_status_only() {
        let a = args(json!({"taskId": "1", "status": "in_progress"}));
        assert_eq!(
            claude_task_label("TaskUpdate", false, &a, None),
            "Started task"
        );
        assert_eq!(
            claude_task_label("TaskUpdate", true, &a, None),
            "Starting task"
        );
        assert_eq!(claude_task_meta("TaskUpdate", &a, ""), "");
        assert!(!claude_task_has_body("TaskUpdate", &a, BodyOpts::default()));
        assert_eq!(claude_task_chip_status("TaskUpdate", &a, ""), "in_progress");
        let st = |s: Option<&str>| {
            let v = match s {
                Some(s) => json!({"taskId": "2", "status": s}),
                None => json!({"taskId": "2"}),
            };
            claude_task_label("TaskUpdate", false, &args(v), None)
        };
        assert_eq!(st(Some("completed")), "Completed task");
        assert_eq!(st(Some("deleted")), "Deleted task");
        assert_eq!(st(None), "Updated task");
    }

    #[test]
    fn list_progress_in_label() {
        let a = args(json!({}));
        assert_eq!(
            claude_task_label("TaskList", true, &a, None),
            "Listing tasks"
        );
        assert_eq!(
            claude_task_label("TaskList", false, &a, Some((2, 6))),
            "Listed tasks · 2/6"
        );
        assert_eq!(
            claude_task_meta(
                "TaskList",
                &a,
                "#1 [in_progress] Guard theme re-apply (registry + host)\n#2 [pending] Next"
            ),
            "Guard theme re-apply (registry + host)"
        );
    }

    #[test]
    fn create_id() {
        assert_eq!(
            parse_task_create_id(
                "Task #1 created successfully: Guard theme re-apply (registry + host)"
            ),
            "1"
        );
        assert_eq!(parse_task_create_id(r#"{"task":{"id":"9"}}"#), "9");
        assert_eq!(parse_task_create_id("nope"), "");
    }

    #[test]
    fn list_line_format() {
        let rows = parse_task_list(
            "#1 [in_progress] Guard theme re-apply (registry + host)\n\
             #2 [pending] Stop plain-frame dispatch (coder) [blocked by #1]\n\
             #3 [completed] Add tm-registry applyColorMap guard tests",
        )
        .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, "1");
        assert_eq!(rows[0].status, "in_progress");
        assert_eq!(rows[0].subject, "Guard theme re-apply (registry + host)");
        assert_eq!(rows[1].subject, "Stop plain-frame dispatch");
        assert_eq!(rows[1].owner, "coder");
        assert_eq!(rows[1].blocked_by, ["1"]);
        assert_eq!(claude_task_counts(&rows), (1, 3));
    }

    #[test]
    fn list_shapes() {
        for empty in [
            "No tasks found",
            r#"{"tasks":[]}"#,
            "[]",
            r#"{"data":{"tasks":[]}}"#,
        ] {
            assert_eq!(parse_task_list(empty), Some(vec![]), "{empty}");
        }
        let rows = parse_task_list(
            r#"{"tasks":[{"id":"4","subject":"Gate scan","status":"pending","blockedBy":[]}]}"#,
        )
        .unwrap();
        assert_eq!(
            (rows[0].id.as_str(), rows[0].subject.as_str()),
            ("4", "Gate scan")
        );
        assert_eq!(parse_task_list("• Task 1 — Gate scan (in progress)"), None);
        assert_eq!(parse_task_list(r#"{"ok":true}"#), None);
    }

    #[test]
    fn get_multiline() {
        let got = parse_task_get(
            "Task #1: Guard theme re-apply (registry + host)\n\
             Status: in_progress\n\
             Description: setActiveTheme memoize.\nKeep id equality a no-op.\n\
             Blocked by: #2\n\
             Blocks: #3, #4",
        )
        .unwrap();
        assert_eq!(got.id, "1");
        assert_eq!(got.subject, "Guard theme re-apply (registry + host)");
        assert_eq!(got.status, "in_progress");
        assert_eq!(
            got.description,
            "setActiveTheme memoize.\nKeep id equality a no-op."
        );
        assert_eq!(got.blocked_by, ["2"]);
        assert_eq!(got.blocks, ["3", "4"]);
        assert_eq!(
            claude_task_meta(
                "TaskGet",
                &args(json!({"taskId": "1"})),
                "Task #1: Guard theme re-apply (registry + host)\nStatus: in_progress\nDescription: x"
            ),
            "Guard theme re-apply (registry + host)"
        );
        assert_eq!(
            parse_task_get("Task #a:b: x").map(|v| v.id),
            Some("a:b".into())
        );
    }

    #[test]
    fn acks_ids_status_copy() {
        assert!(claude_task_result_is_ack(
            "TaskCreate",
            "Task #1 created successfully: Guard theme"
        ));
        assert!(claude_task_result_is_ack(
            "TaskUpdate",
            "Updated task #1 status"
        ));
        assert!(!claude_task_result_is_ack(
            "TaskUpdate",
            "Task #1 not found"
        ));
        assert_eq!(format_task_id("1"), "#1");
        assert_eq!(format_task_id("#1"), "#1");
        assert_eq!(pretty_task_status("in_progress"), "in progress");
        assert_eq!(task_status_chip_class("in_progress"), "doing");
        assert_eq!(task_status_chip_class("completed"), "done");
        assert_eq!(task_status_chip_class("deleted"), "invalid");
        assert_eq!(task_status_chip_class("pending"), "pending");
    }

    #[test]
    fn has_body() {
        assert!(claude_task_has_body(
            "TaskCreate",
            &create_args(),
            BodyOpts::default()
        ));
        assert!(!claude_task_has_body(
            "TaskCreate",
            &args(json!({"subject": "Only a title"})),
            BodyOpts::default()
        ));
        assert!(claude_task_has_body(
            "TaskList",
            &args(json!({})),
            BodyOpts::default()
        ));
        let get = args(json!({"taskId": "1"}));
        assert!(claude_task_has_body(
            "TaskGet",
            &get,
            BodyOpts {
                pending: true,
                ..Default::default()
            }
        ));
        assert!(!claude_task_has_body(
            "TaskGet",
            &get,
            BodyOpts {
                result: "Task #1: Guard theme\nStatus: pending\nDescription: ",
                ..Default::default()
            }
        ));
        assert!(claude_task_has_body(
            "TaskGet",
            &get,
            BodyOpts {
                result: "• Task 1 — Guard theme (in progress)",
                ..Default::default()
            }
        ));
    }
}
