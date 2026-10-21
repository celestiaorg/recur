//! Per-tool presentation for transcript rows (`AgentChat.svelte`'s script): the row label, the
//! dim meta line, the icon, the in-flight copy, and the parsed payloads each tool card shows.
//! Never a JSON dump where a structured view exists.

use std::sync::LazyLock;

use recur_agent::display::tasks::{
    ClaudeTaskArgs, ClaudeTaskView, claude_task_counts, claude_task_label, claude_task_meta,
    format_task_id, is_claude_task_tool, parse_claude_task_args, parse_task_create_id,
    parse_task_get, parse_task_list,
};
use recur_agent::display::todos::{Todo, parse_todos, todo_counts};
use recur_agent::display::web::{web_fetch_prompt, web_fetch_url, web_search_query};
use recur_agent_tools::display::briefing_tools::{
    HydrateProject, HydrateWorkspace, ListedWorkspaceBriefs, PresentWorkspaceView,
    WorkspaceContextView, briefing_counts_meta, get_workspace_context_label, hydrate_present_brief,
    is_briefing_tool, is_get_workspace_context_tool, is_list_workspace_briefs_tool,
    is_present_workspace_tool, list_workspace_briefs_label, parse_list_workspace_briefs,
    parse_present_workspace, parse_present_workspace_target, parse_workspace_context,
    present_workspace_label,
};
use recur_agent_tools::display::open_pr_tool::{
    OpenPrCard, is_open_pr_tool, open_pr_label, parse_open_pr_card,
};
use recur_agent_tools::display::plan_tools::{
    PlanStepView, edit_plan_label, is_edit_plan_tool, is_submit_plan_tool,
    is_update_plan_step_tool, parse_edit_plan, parse_submit_plan_attachments,
    parse_submit_plan_body, parse_submit_plan_steps, parse_update_plan_step_title, prose_preview,
    submit_plan_label, update_plan_step_label, update_plan_step_meta,
};
use recur_agent_tools::display::reenter_phase_tool::{
    is_reenter_phase_tool, parse_reenter_phase_id, reenter_phase_failed, reenter_phase_label,
};
use recur_domain::{Finding, Pr, WorkspaceBrief};
use recur_engine::view::artifacts::{ListedArtifacts, resolve_list_artifacts};
use recur_engine::view::chat::{AgentMsg, is_subagent_launch_meta, is_task_tool};
use recur_issues::linear::tools::{
    LinearArgs, LinearView, is_linear_tool, linear_input_body, linear_tool_label, linear_tool_meta,
    parse_linear_args, parse_linear_result,
};
use recur_textmate::{DiffRow, diff_lines, looks_like_unified_diff, parse_unified_diff};
use recur_ui::IconName;
use regex::Regex;
use serde_json::Value;

use crate::findings_ui::{
    ListedFinding, ListedFindings, finding_loc_suffix, finding_matches_ref, listed_from_store,
    parse_listed_finding, resolve_list_findings,
};
use recur_engine::view::subagent::{
    SubagentArgs, is_send_message_tool, is_subagent_launch_ack, is_subagent_tool,
    parse_send_message_args, parse_subagent_args, send_message_label, send_message_meta,
};

/// What the per-tool presentation reads beyond the row itself.
#[derive(Clone, Copy, Default)]
pub struct ToolCx<'a> {
    /// The worktree root, stripped from displayed paths.
    pub root_path: &'a str,
    /// The host opens files (file chips are links; meta drops the path).
    pub has_open_file: bool,
    /// Workspace findings, to resolve `update_finding` rows by id.
    pub findings: &'a [Finding],
    pub prs: &'a [Pr],
    pub workspace_id: &'a str,
    /// The overview briefs and the workspace / project lists `present_workspace` hydrates from.
    pub briefs: &'a [WorkspaceBrief],
    pub workspaces: &'a [HydrateWorkspace],
    pub projects: &'a [HydrateProject],
}

/// The step icon kinds (`ToolIcon` plus the thought / compaction / fallback rows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepIcon {
    Term,
    File,
    Pencil,
    Trash,
    Search,
    Globe,
    Bot,
    Todo,
    Plan,
    Finding,
    Done,
    Tools,
    Artifact,
    Plug,
    Workspace,
    Linear,
    Think,
    Compact,
    Fallback,
}

impl StepIcon {
    /// The `stepIcon` snippet's icon names.
    pub fn icon(self) -> IconName {
        match self {
            StepIcon::Term => IconName::ChevronsRight,
            StepIcon::File => IconName::File,
            StepIcon::Pencil => IconName::Pencil,
            StepIcon::Trash => IconName::Trash,
            StepIcon::Search => IconName::Search,
            StepIcon::Globe => IconName::Globe,
            StepIcon::Bot => IconName::Bot,
            StepIcon::Todo => IconName::ListTodo,
            StepIcon::Plan => IconName::NotebookPen,
            StepIcon::Finding => IconName::HatGlasses,
            StepIcon::Artifact => IconName::Grid,
            StepIcon::Workspace => IconName::Box,
            StepIcon::Done => IconName::CircleCheck,
            StepIcon::Tools => IconName::PocketKnife,
            StepIcon::Think => IconName::Lightbulb,
            StepIcon::Compact => IconName::Minimize,
            StepIcon::Fallback => IconName::Replace,
            StepIcon::Linear => IconName::Linear,
            StepIcon::Plug => IconName::Flask,
        }
    }
}

fn ends(tool: &str, name: &str) -> bool {
    tool.ends_with(&format!("__{name}")) || tool == name
}

pub fn base_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// `relPath`: strip the worktree root from a tool's absolute path.
pub fn rel_path<'a>(root: &str, p: &'a str) -> std::borrow::Cow<'a, str> {
    if !root.is_empty()
        && let Some(rest) = p.strip_prefix(root)
    {
        let rel = rest.strip_prefix('/').unwrap_or(rest);
        return if rel.is_empty() {
            ".".into()
        } else {
            rel.to_string().into()
        };
    }
    p.into()
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn str_field(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.trim().to_string(),
        _ => String::new(),
    }
}

/// `prettyResult`: pretty-print a JSON result so unmapped tools do not dump one line.
pub fn pretty_result(text: &str) -> String {
    let t = text.trim();
    if t.is_empty() || !(t.starts_with('{') || t.starts_with('[')) {
        return text.to_string();
    }
    match serde_json::from_str::<Value>(t) {
        Ok(v) => js_pretty(&v),
        Err(_) => text.to_string(),
    }
}

/// `JSON.stringify(v, null, 2)`, keys in insertion order.
pub fn js_pretty(v: &Value) -> String {
    fn go(v: &Value, ind: usize, out: &mut String) {
        let pad = |n: usize| "  ".repeat(n);
        match v {
            Value::Object(m) if m.is_empty() => out.push_str("{}"),
            Value::Array(a) if a.is_empty() => out.push_str("[]"),
            Value::Object(m) => {
                out.push_str("{\n");
                for (i, (k, x)) in m.iter().enumerate() {
                    out.push_str(&pad(ind + 1));
                    out.push_str(&serde_json::to_string(k).unwrap_or_default());
                    out.push_str(": ");
                    go(x, ind + 1, out);
                    if i + 1 < m.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str(&pad(ind));
                out.push('}');
            }
            Value::Array(a) => {
                out.push_str("[\n");
                for (i, x) in a.iter().enumerate() {
                    out.push_str(&pad(ind + 1));
                    go(x, ind + 1, out);
                    if i + 1 < a.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str(&pad(ind));
                out.push(']');
            }
            Value::String(x) => out.push_str(&serde_json::to_string(x).unwrap_or_default()),
            Value::Null => out.push_str("null"),
            other => out.push_str(&recur_engine::view::js::string_of(Some(other))),
        }
    }
    let mut out = String::new();
    go(v, 0, &mut out);
    out
}

static SELECT_Q: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^select:(.+)$").expect("select"));
static SELECT_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^select:").expect("select"));
static CHILD_REFUSED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(merge|read|archive|message)_child:").expect("child"));
static CHILD_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(merge|read|archive|message)_child:\s*").expect("child"));
static CREATE_ART_FAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(create_artifact:|unsupported artifact)").expect("create art")
});
static PRESENTED_ART: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^Presented artifact\s+\S+\s+\((.+)\)\.?$").expect("presented")
});
static PRESENT_ART_FAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(Artifact not found|Could not list artifacts)").expect("present fail")
});
static RENAMED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^Renamed (?:chat|workspace|branch) to\s+(.+?)\.?$").expect("renamed")
});
static RENAME_FAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^rename_(?:session|workspace|branch):").expect("rename"));
static SWITCHED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)Switched chat mode from\s+(\w+)\s+to\s+(\w+)").expect("switched")
});
static SWITCH_FAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(Cannot switch|Unknown mode|switch_mode is only)").expect("switch fail")
});
static SKILL_CONTENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)<skill_content\s+name="([^"]+)""#).expect("skill"));

/// One lint diagnostic (`LintDiagnostic`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LintDiagnostic {
    pub severity: String,
    pub message: String,
    pub line: i64,
    pub end_line: i64,
    pub source: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LintFile {
    pub path: String,
    pub diagnostics: Vec<LintDiagnostic>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LintResult {
    pub files: Vec<LintFile>,
    pub total_files: i64,
    pub total_diagnostics: i64,
}

fn lint_severity(raw: Option<&Value>) -> String {
    match raw {
        Some(Value::String(s)) if !s.is_empty() => s.to_lowercase(),
        Some(Value::Number(n)) => match n.as_f64().map(|f| f as i64) {
            Some(1) => "error",
            Some(2) => "warning",
            Some(3) => "info",
            Some(4) => "hint",
            _ => "info",
        }
        .into(),
        _ => "info".into(),
    }
}

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
}

fn lint_line(d: &Value) -> i64 {
    if let Some(l) = num(d.pointer("/range/start/line")) {
        return l as i64 + 1;
    }
    let line = d
        .get("startLineNumber")
        .filter(|v| !v.is_null())
        .or_else(|| d.get("line").filter(|v| !v.is_null()))
        .or_else(|| d.get("lnum"));
    match num(line) {
        Some(l) if l > 0. => l as i64,
        _ => 0,
    }
}

fn lint_end_line(d: &Value) -> i64 {
    if let Some(l) = num(d.pointer("/range/end/line")) {
        return l as i64 + 1;
    }
    let line = d
        .get("endLineNumber")
        .filter(|v| !v.is_null())
        .or_else(|| d.get("endLine"));
    match num(line) {
        Some(l) if l > 0. => l as i64,
        _ => 0,
    }
}

fn parse_lint_diagnostic(d: &Value) -> LintDiagnostic {
    let sev = d
        .get("severity")
        .filter(|v| !v.is_null())
        .or_else(|| d.get("severityName"));
    let message = d
        .get("message")
        .filter(|v| !v.is_null())
        .or_else(|| d.get("msg"))
        .map(|v| recur_engine::view::js::string_of(Some(v)))
        .unwrap_or_default();
    let source = match (d.get("source"), d.get("code")) {
        (Some(Value::String(s)), _) => s.clone(),
        (_, Some(Value::String(c))) => c.clone(),
        _ => String::new(),
    };
    LintDiagnostic {
        severity: lint_severity(sev),
        message,
        line: lint_line(d),
        end_line: lint_end_line(d),
        source,
    }
}

/// `lintLocSuffix`.
pub fn lint_loc_suffix(d: &LintDiagnostic) -> String {
    if d.line == 0 {
        return String::new();
    }
    if d.end_line != 0 && d.end_line != d.line {
        return format!(":{}–{}", d.line, d.end_line);
    }
    format!(":{}", d.line)
}

/// `lintSevClass`: the `.tc-sev` class.
pub fn lint_sev_class(sev: &str) -> &'static str {
    match sev {
        "error" | "fatal" => "high",
        "warning" | "warn" => "medium",
        _ => "info",
    }
}

/// `PLAN_STEP_MARK`.
pub fn plan_step_mark(status: &str) -> &'static str {
    match status {
        "doing" | "in_progress" => "◐",
        "done" | "completed" => "●",
        "skipped" => "⊘",
        _ => "○",
    }
}

/// `x.toFixed(1)` for a non-negative finite `x`: the exact binary value rounded to tenths, ties
/// up, as ECMAScript specifies (Rust's formatting rounds ties to even).
pub fn js_to_fixed_1(x: f64) -> String {
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i64 - 1075;
    let mant = (bits & ((1 << 52) - 1)) | (1 << 52);
    let tenths: u128 = if x == 0. {
        0
    } else if exp >= 0 {
        ((mant as u128) << exp) * 10
    } else {
        let shift = (-exp) as u32;
        let scaled = mant as u128 * 10;
        if shift >= 128 {
            0
        } else {
            let q = scaled >> shift;
            let r = scaled & ((1u128 << shift) - 1);
            if r >= 1u128 << (shift - 1) { q + 1 } else { q }
        }
    };
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// `formatDuration`: a finished turn's wall time.
pub fn format_duration(ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    let sec = ms as f64 / 1000.;
    if sec < 60. {
        return if sec < 10. {
            format!("{}s", js_to_fixed_1(sec))
        } else {
            format!("{}s", sec.round() as i64)
        };
    }
    let total = sec.round() as i64;
    let m = total / 60;
    let s = format!("{:02}", total % 60);
    if m >= 60 {
        return format!("{}h {}m {s}s", m / 60, m % 60);
    }
    format!("{m}m {s}s")
}

/// One transcript row with its parsed input, as the tool card reads it.
pub struct ToolView<'a> {
    pub m: &'a AgentMsg,
    pub cx: ToolCx<'a>,
    input: Option<Value>,
}

impl<'a> ToolView<'a> {
    pub fn new(m: &'a AgentMsg, cx: ToolCx<'a>) -> Self {
        let input = if m.input.is_empty() {
            None
        } else {
            serde_json::from_str::<Value>(&m.input)
                .ok()
                .filter(|v| v.is_object() || v.is_array())
        };
        ToolView { m, cx, input }
    }

    fn tool(&self) -> &str {
        &self.m.tool
    }

    /// `parsedInput`.
    pub fn input(&self) -> Option<&Value> {
        self.input.as_ref()
    }

    fn arg(&self, key: &str) -> Option<&Value> {
        self.input.as_ref().and_then(|v| v.get(key))
    }

    fn arg_str(&self, key: &str) -> Option<&str> {
        self.arg(key).and_then(Value::as_str)
    }

    fn null_input(&self) -> Value {
        self.input.clone().unwrap_or(Value::Null)
    }

    pub fn rel(&self, p: &'a str) -> std::borrow::Cow<'a, str> {
        rel_path(self.cx.root_path, p)
    }

    // ---- predicates ----

    pub fn is_report_finding(&self) -> bool {
        ends(self.tool(), "report_finding")
    }
    pub fn is_list_findings(&self) -> bool {
        ends(self.tool(), "list_findings")
    }
    pub fn is_update_finding(&self) -> bool {
        ends(self.tool(), "update_finding")
    }
    pub fn is_skill_activation(&self) -> bool {
        self.tool() == "Skill" || ends(self.tool(), "activate_skill")
    }
    pub fn is_list_skills(&self) -> bool {
        ends(self.tool(), "list_skills")
    }
    pub fn is_ask_user(&self) -> bool {
        ends(self.tool(), "ask_user")
    }
    pub fn is_switch_mode(&self) -> bool {
        ends(self.tool(), "switch_mode")
    }
    pub fn is_rename_session(&self) -> bool {
        ends(self.tool(), "rename_session")
    }
    pub fn is_rename_workspace(&self) -> bool {
        ends(self.tool(), "rename_workspace")
    }
    pub fn is_rename_branch(&self) -> bool {
        ends(self.tool(), "rename_branch")
    }
    pub fn is_rename_tool(&self) -> bool {
        self.is_rename_session() || self.is_rename_workspace() || self.is_rename_branch()
    }
    pub fn is_open_pr(&self) -> bool {
        is_open_pr_tool(Some(self.tool()))
    }
    pub fn is_reenter_phase(&self) -> bool {
        is_reenter_phase_tool(Some(self.tool()))
    }
    pub fn is_merge_child(&self) -> bool {
        ends(self.tool(), "merge_child")
    }
    pub fn is_read_child(&self) -> bool {
        ends(self.tool(), "read_child")
    }
    pub fn is_archive_child(&self) -> bool {
        ends(self.tool(), "archive_child")
    }
    pub fn is_message_child(&self) -> bool {
        ends(self.tool(), "message_child")
    }
    pub fn is_child_tool(&self) -> bool {
        self.is_merge_child()
            || self.is_read_child()
            || self.is_archive_child()
            || self.is_message_child()
    }
    pub fn is_list_artifacts(&self) -> bool {
        ends(self.tool(), "list_artifacts")
    }
    pub fn is_create_artifact(&self) -> bool {
        ends(self.tool(), "create_artifact")
    }
    pub fn is_present_artifact(&self) -> bool {
        ends(self.tool(), "present_artifact")
    }
    pub fn is_list_workspace_briefs(&self) -> bool {
        is_list_workspace_briefs_tool(Some(self.tool()))
    }
    pub fn is_get_workspace_context(&self) -> bool {
        is_get_workspace_context_tool(Some(self.tool()))
    }
    pub fn is_present_workspace(&self) -> bool {
        is_present_workspace_tool(Some(self.tool()))
    }
    pub fn is_briefing(&self) -> bool {
        is_briefing_tool(Some(self.tool()))
    }
    pub fn is_done(&self) -> bool {
        ends(self.tool(), "done")
    }
    pub fn is_submit_plan(&self) -> bool {
        is_submit_plan_tool(Some(self.tool()))
    }
    pub fn is_update_plan_step(&self) -> bool {
        is_update_plan_step_tool(Some(self.tool()))
    }
    pub fn is_edit_plan(&self) -> bool {
        is_edit_plan_tool(Some(self.tool()))
    }
    pub fn is_create_plan(&self) -> bool {
        self.tool() == "CreatePlan" || self.tool() == "createPlan"
    }
    pub fn is_read_lints(&self) -> bool {
        self.tool() == "readLints" || self.tool() == "ReadLints"
    }
    pub fn is_linear(&self) -> bool {
        is_linear_tool(self.tool())
    }
    pub fn is_claude_task(&self) -> bool {
        is_claude_task_tool(self.tool())
    }
    pub fn is_subagent(&self) -> bool {
        is_subagent_tool(self.tool())
    }
    pub fn is_task(&self) -> bool {
        is_task_tool(self.tool())
    }
    pub fn is_send_message(&self) -> bool {
        is_send_message_tool(self.tool())
    }

    /// `isSubagentCreate`: create_workspace that made (or is making) a subagent: the fan-out
    /// card is its row, so the step is dropped. Refusals and standalone creations stay.
    pub fn is_subagent_create(&self) -> bool {
        if !ends(self.tool(), "create_workspace") || self.m.result_error {
            return false;
        }
        if self.arg("standalone") == Some(&Value::Bool(true)) {
            return false;
        }
        let r = &self.m.result;
        r.is_empty() || r.starts_with("Created subagent")
    }

    // ---- files ----

    /// `fileOf`: Read/Edit/Write use `file_path`; report_finding uses `file`; update_finding
    /// resolves the path from the store.
    pub fn file_of(&self) -> String {
        if self.is_update_finding() {
            return self
                .resolve_update_finding()
                .map(|f| f.file)
                .unwrap_or_default();
        }
        let fp = self
            .arg("file_path")
            .filter(|v| !v.is_null())
            .or_else(|| self.arg("file"));
        match fp {
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        }
    }

    // ---- findings ----

    fn update_finding_result(&self) -> Option<ListedFinding> {
        if self.m.result.is_empty() || self.m.result_error {
            return None;
        }
        match serde_json::from_str::<Value>(&self.m.result) {
            Ok(v @ Value::Object(_)) if recur_engine::view::js::truthy(v.get("id")) => {
                Some(parse_listed_finding(&v))
            }
            _ => None,
        }
    }

    fn update_finding_from_input(&self) -> Option<ListedFinding> {
        self.input.as_ref()?;
        let id = self.arg_str("finding_id").unwrap_or("").to_string();
        let status = self.arg_str("status").unwrap_or("").to_string();
        if id.is_empty() && status.is_empty() {
            return None;
        }
        Some(ListedFinding {
            id,
            status,
            status_reason: self.arg_str("reason").unwrap_or("").to_string(),
            ..Default::default()
        })
    }

    fn finding_from_store(&self, id: &str) -> Option<ListedFinding> {
        if id.is_empty() {
            return None;
        }
        self.cx
            .findings
            .iter()
            .find(|f| finding_matches_ref(f, id))
            .map(listed_from_store)
    }

    /// `resolveUpdateFinding`: identity from the args, file/message/severity from the store,
    /// the JSON result as a fallback.
    pub fn resolve_update_finding(&self) -> Option<ListedFinding> {
        let from_input = self.update_finding_from_input();
        let from_result = self.update_finding_result();
        let id = from_input
            .as_ref()
            .map(|f| f.id.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| from_result.as_ref().map(|f| f.id.clone()))
            .unwrap_or_default();
        let from_store = self.finding_from_store(&id);
        if from_store.is_none() && from_result.is_none() && from_input.is_none() {
            return None;
        }
        let pick =
            |f: fn(&ListedFinding) -> &String, order: [&Option<ListedFinding>; 3]| -> String {
                order
                    .iter()
                    .filter_map(|o| o.as_ref())
                    .map(f)
                    .find(|s| !s.is_empty())
                    .cloned()
                    .unwrap_or_default()
            };
        let line_of = |f: fn(&ListedFinding) -> i64| -> i64 {
            [&from_store, &from_result]
                .iter()
                .filter_map(|o| o.as_ref())
                .map(f)
                .find(|n| *n > 0)
                .unwrap_or(0)
        };
        Some(ListedFinding {
            id,
            status: pick(|f| &f.status, [&from_input, &from_result, &from_store]),
            file: pick(|f| &f.file, [&from_store, &from_result, &None]),
            line: line_of(|f| f.line),
            end_line: line_of(|f| f.end_line),
            severity: pick(|f| &f.severity, [&from_store, &from_result, &None]),
            message: pick(|f| &f.message, [&from_store, &from_result, &None]),
            status_reason: pick(
                |f| &f.status_reason,
                [&from_input, &from_result, &from_store],
            ),
            ..Default::default()
        })
    }

    pub fn finding_severity(&self) -> String {
        if self.is_update_finding() {
            return self
                .resolve_update_finding()
                .map(|f| f.severity)
                .unwrap_or_default();
        }
        self.arg_str("severity").unwrap_or("").to_string()
    }

    pub fn finding_file(&self) -> String {
        if self.is_update_finding() {
            return self
                .resolve_update_finding()
                .map(|f| f.file)
                .unwrap_or_default();
        }
        self.arg_str("file").unwrap_or("").to_string()
    }

    fn positive(&self, key: &str) -> i64 {
        match num(self.arg(key)) {
            Some(n) if n > 0. => n as i64,
            _ => 0,
        }
    }

    pub fn finding_line(&self) -> i64 {
        if self.is_update_finding() {
            return self
                .resolve_update_finding()
                .map(|f| f.line.max(0))
                .unwrap_or(0);
        }
        self.positive("line")
    }

    pub fn finding_end_line(&self) -> i64 {
        if self.is_update_finding() {
            return self
                .resolve_update_finding()
                .map(|f| f.end_line.max(0))
                .unwrap_or(0);
        }
        self.positive("end_line")
    }

    pub fn finding_loc_suffix(&self) -> String {
        let line = self.finding_line();
        if line == 0 {
            return String::new();
        }
        finding_loc_suffix(line, self.finding_end_line())
    }

    pub fn finding_message(&self) -> String {
        if self.is_update_finding() {
            return self
                .resolve_update_finding()
                .map(|f| f.message)
                .unwrap_or_default();
        }
        self.arg_str("message").unwrap_or("").to_string()
    }

    pub fn finding_status(&self) -> String {
        if !self.is_update_finding() {
            return String::new();
        }
        self.resolve_update_finding()
            .map(|f| f.status)
            .unwrap_or_default()
    }

    pub fn finding_status_reason(&self) -> String {
        if !self.is_update_finding() {
            return String::new();
        }
        self.resolve_update_finding()
            .map(|f| f.status_reason)
            .unwrap_or_default()
    }

    pub fn list_findings_result(&self) -> Option<ListedFindings> {
        resolve_list_findings(&self.m.result, self.m.result_error, self.cx.findings)
    }

    pub fn list_artifacts_result(&self) -> Option<ListedArtifacts> {
        resolve_list_artifacts(&self.m.result, self.m.result_error)
    }

    // ---- todos / plans ----

    pub fn todo_items(&self) -> Vec<Todo> {
        parse_todos(&self.m.input)
    }

    pub fn plan_body(&self) -> String {
        str_field(self.arg("plan"))
    }

    pub fn submit_plan_summary(&self) -> String {
        str_field(self.arg("summary"))
    }

    pub fn submit_plan_body(&self) -> String {
        parse_submit_plan_body(&self.null_input())
    }

    pub fn submit_plan_att_ids(&self) -> Vec<String> {
        parse_submit_plan_attachments(self.arg("attachments").unwrap_or(&Value::Null))
    }

    pub fn submit_plan_steps(&self) -> Vec<PlanStepView> {
        parse_submit_plan_steps(self.arg("steps").unwrap_or(&Value::Null))
    }

    pub fn edit_plan_text(&self) -> String {
        parse_edit_plan(&self.null_input())
    }

    pub fn update_plan_step_id(&self) -> String {
        self.arg_str("step_id").unwrap_or("").to_string()
    }

    pub fn update_plan_step_status(&self) -> String {
        self.arg_str("status").unwrap_or("").to_string()
    }

    pub fn update_plan_step_note(&self) -> String {
        str_field(self.arg("note"))
    }

    pub fn update_plan_step_title(&self) -> String {
        if self.m.result_error {
            return String::new();
        }
        parse_update_plan_step_title(&self.m.result)
    }

    // ---- Claude tasks / subagents ----

    pub fn claude_task_args(&self) -> ClaudeTaskArgs {
        parse_claude_task_args(self.input.as_ref())
    }

    pub fn claude_task_list_items(&self) -> Option<Vec<ClaudeTaskView>> {
        parse_task_list(&self.m.result)
    }

    pub fn claude_task_list_counts(&self) -> Option<(usize, usize)> {
        if self.tool() != "TaskList" || self.m.result_pending || self.m.result_error {
            return None;
        }
        let items = self.claude_task_list_items()?;
        if items.is_empty() {
            return None;
        }
        let c = claude_task_counts(&items);
        (c.1 > 0).then_some(c)
    }

    pub fn claude_task_get_view(&self) -> Option<ClaudeTaskView> {
        if self.m.result_error {
            return None;
        }
        parse_task_get(&self.m.result)
    }

    /// `claudeTaskRowId`.
    pub fn claude_task_row_id(&self) -> String {
        let args = self.claude_task_args();
        if !args.task_id.is_empty() {
            return format_task_id(&args.task_id);
        }
        match self.tool() {
            "TaskCreate" => format_task_id(&parse_task_create_id(&self.m.result)),
            "TaskGet" => format_task_id(
                &self
                    .claude_task_get_view()
                    .map(|v| v.id)
                    .unwrap_or_default(),
            ),
            _ => String::new(),
        }
    }

    pub fn subagent(&self) -> SubagentArgs {
        parse_subagent_args(self.input.as_ref().and_then(Value::as_object))
    }

    /// `taskBackground`.
    pub fn task_background(&self) -> bool {
        self.subagent().background || is_subagent_launch_ack(&self.m.result)
    }

    // ---- ToolSearch / lints / web / linear ----

    pub fn tool_search_query(&self) -> String {
        self.arg_str("query").unwrap_or("").to_string()
    }

    pub fn tool_search_max(&self) -> i64 {
        match num(self.arg("max_results")) {
            Some(n) if n > 0. => n as i64,
            _ => 0,
        }
    }

    pub fn read_lints_paths(&self) -> Vec<String> {
        match self.arg("paths") {
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// `readLintsResult`.
    pub fn read_lints_result(&self) -> Option<LintResult> {
        if self.m.result.is_empty() || self.m.result_error {
            return None;
        }
        let raw: Value = serde_json::from_str(&self.m.result).ok()?;
        let Value::Array(file_diags) = raw.get("fileDiagnostics")? else {
            return None;
        };
        let files: Vec<LintFile> = file_diags
            .iter()
            .map(|f| LintFile {
                path: f
                    .get("path")
                    .filter(|v| !v.is_null())
                    .or_else(|| f.get("file"))
                    .map(|v| recur_engine::view::js::string_of(Some(v)))
                    .unwrap_or_default(),
                diagnostics: match f.get("diagnostics") {
                    Some(Value::Array(d)) => d.iter().map(parse_lint_diagnostic).collect(),
                    _ => Vec::new(),
                },
            })
            .collect();
        let total_diagnostics = match raw.get("totalDiagnostics") {
            Some(Value::Number(n)) => n.as_f64().unwrap_or(0.) as i64,
            _ => files.iter().map(|f| f.diagnostics.len() as i64).sum(),
        };
        let total_files = match raw.get("totalFiles") {
            Some(Value::Number(n)) => n.as_f64().unwrap_or(0.) as i64,
            _ => files.len() as i64,
        };
        Some(LintResult {
            files,
            total_files,
            total_diagnostics,
        })
    }

    pub fn fetch_url(&self) -> String {
        web_fetch_url(self.input.as_ref(), &self.m.tool_input)
    }

    pub fn fetch_prompt(&self) -> String {
        web_fetch_prompt(self.input.as_ref(), &self.m.tool_input)
    }

    pub fn search_query(&self) -> String {
        web_search_query(self.input.as_ref(), &self.m.tool_input)
    }

    /// `webHasCard`: expand only with a prompt, page body or error to show.
    pub fn web_has_card(&self) -> bool {
        if self.m.result_pending || self.m.result_error || !self.m.result.trim().is_empty() {
            return true;
        }
        self.tool() == "WebFetch" && !self.fetch_prompt().is_empty()
    }

    pub fn linear_args(&self) -> LinearArgs {
        parse_linear_args(&self.null_input())
    }

    pub fn linear_view(&self) -> Option<LinearView> {
        if self.m.result_error || self.m.result_pending || self.m.result.is_empty() {
            return None;
        }
        parse_linear_result(&self.m.result)
    }

    pub fn linear_input_body(&self) -> String {
        linear_input_body(&self.null_input())
    }

    // ---- skill / switch_mode / rename / child / artifacts / PR ----

    /// `skillActivationName`.
    pub fn skill_activation_name(&self) -> String {
        for key in ["skill", "name"] {
            let v = str_field(self.arg(key));
            if !v.is_empty() {
                return v;
            }
        }
        let ti = self.m.tool_input.trim();
        let ti = ti.strip_prefix('/').unwrap_or(ti);
        if !ti.is_empty() && !ti.starts_with('{') && ti.to_lowercase() != "skill" {
            return ti.split_whitespace().next().unwrap_or("").to_string();
        }
        SKILL_CONTENT
            .captures(&self.m.result)
            .map(|c| c[1].to_string())
            .unwrap_or_default()
    }

    pub fn switch_mode_target(&self) -> String {
        self.arg_str("mode").unwrap_or("").trim().to_lowercase()
    }

    /// `switchModeTransition`: "Switched chat mode from X to Y", else the requested target.
    pub fn switch_mode_transition(&self) -> Option<(String, String)> {
        let r = self.m.result.trim();
        if let Some(c) = SWITCHED.captures(r) {
            return Some((c[1].to_lowercase(), c[2].to_lowercase()));
        }
        let to = self.switch_mode_target();
        (!to.is_empty()).then(|| (String::new(), to))
    }

    pub fn switch_mode_failed(&self) -> bool {
        if self.m.result_error {
            return true;
        }
        let r = self.m.result.trim();
        if r.is_empty() || self.m.result_pending {
            return false;
        }
        SWITCH_FAIL.is_match(r)
    }

    pub fn ask_user_question(&self) -> String {
        str_field(self.arg("question"))
    }

    pub fn ask_user_context(&self) -> String {
        str_field(self.arg("context"))
    }

    pub fn done_summary(&self) -> String {
        self.arg_str("summary").unwrap_or("").to_string()
    }

    pub fn rename_tool_value(&self) -> String {
        let raw = self
            .arg("title")
            .filter(|v| !v.is_null())
            .or_else(|| self.arg("name"));
        if let Some(Value::String(s)) = raw
            && !s.trim().is_empty()
        {
            return s.trim().to_string();
        }
        RENAMED
            .captures(self.m.result.trim())
            .map(|c| c[1].trim().to_string())
            .unwrap_or_default()
    }

    pub fn rename_tool_failed(&self) -> bool {
        if self.m.result_error {
            return true;
        }
        let r = self.m.result.trim();
        if r.is_empty() || self.m.result_pending {
            return false;
        }
        RENAME_FAIL.is_match(r)
    }

    pub fn rename_tool_noun(&self) -> &'static str {
        if self.is_rename_workspace() {
            "workspace"
        } else if self.is_rename_branch() {
            "branch"
        } else {
            "chat"
        }
    }

    pub fn child_tool_id(&self) -> String {
        self.arg_str("workspace_id").unwrap_or("").to_string()
    }

    /// Engine refusals come back as "merge_child: …" text, not tool errors.
    pub fn child_tool_failed(&self) -> bool {
        self.m.result_error || CHILD_REFUSED.is_match(&self.m.result)
    }

    fn child_tool_verb(&self) -> (&'static str, &'static str, &'static str) {
        if self.is_merge_child() {
            ("Merging", "Merged", "Merge")
        } else if self.is_archive_child() {
            ("Archiving", "Archived", "Archive")
        } else if self.is_message_child() {
            ("Messaging", "Messaged", "Message")
        } else {
            ("Reading", "Read", "Read")
        }
    }

    pub fn child_tool_label(&self) -> String {
        let (doing, done, base) = self.child_tool_verb();
        let id = self.child_tool_id();
        let who = if id.is_empty() {
            "subagent".to_string()
        } else {
            format!("subagent {id}")
        };
        if self.child_tool_failed() {
            return format!("{base} refused: {who}");
        }
        if self.m.result_pending {
            format!("{doing} {who}")
        } else {
            format!("{done} {who}")
        }
    }

    /// One-line result for the row: the refusal text or the merge outcome.
    pub fn child_tool_note(&self) -> String {
        if self.m.result_pending || self.m.result.is_empty() {
            return String::new();
        }
        let first_line = self.m.result.split('\n').next().unwrap_or("");
        let first = CHILD_PREFIX.replace(first_line, "").trim().to_string();
        if self.is_read_child() && !self.child_tool_failed() {
            return String::new();
        }
        first
    }

    fn create_artifact_result(&self) -> Option<(String, String, String)> {
        let r = self.m.result.trim();
        let i = r.find('{')?;
        let o: Value = serde_json::from_str(&r[i..]).ok()?;
        Some((
            str_field(o.get("title")),
            str_field(o.get("kind")).to_lowercase(),
            str_field(o.get("id")),
        ))
    }

    pub fn create_artifact_title(&self) -> String {
        let t = str_field(self.arg("title"));
        if !t.is_empty() {
            return t;
        }
        self.create_artifact_result()
            .map(|r| r.0)
            .unwrap_or_default()
    }

    pub fn create_artifact_kind(&self) -> String {
        let k = str_field(self.arg("kind"));
        if !k.is_empty() {
            return k.to_lowercase();
        }
        self.create_artifact_result()
            .map(|r| r.1)
            .unwrap_or_default()
    }

    pub fn create_artifact_failed(&self) -> bool {
        if self.m.result_error {
            return true;
        }
        let r = self.m.result.trim();
        if r.is_empty() || self.m.result_pending {
            return false;
        }
        CREATE_ART_FAIL.is_match(r)
    }

    pub fn present_artifact_id(&self) -> String {
        str_field(self.arg("id"))
    }

    /// "Presented artifact <id> (<title>)."
    pub fn present_artifact_title(&self) -> String {
        if let Some(c) = PRESENTED_ART.captures(self.m.result.trim()) {
            let t = c[1].trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
        self.present_artifact_id()
    }

    pub fn present_artifact_failed(&self) -> bool {
        if self.m.result_error {
            return true;
        }
        let r = self.m.result.trim();
        if r.is_empty() || self.m.result_pending {
            return false;
        }
        PRESENT_ART_FAIL.is_match(r)
    }

    pub fn open_pr_card(&self) -> OpenPrCard {
        parse_open_pr_card(
            &self.null_input(),
            &self.m.result,
            self.m.result_pending,
            self.m.result_error,
        )
    }

    /// `openPRId`: the store PR this card opened, scoped to the workspace.
    pub fn open_pr_id(&self) -> String {
        let card = self.open_pr_card();
        if card.number == 0 {
            return String::new();
        }
        let ws = self.cx.workspace_id;
        let hits: Vec<&Pr> = self
            .cx
            .prs
            .iter()
            .filter(|p| p.number == card.number as i64 && (ws.is_empty() || p.workspace_id == ws))
            .collect();
        if hits.len() == 1 {
            return hits[0].id.clone();
        }
        if !ws.is_empty()
            && let Some(ranked) = recur_view_core::pr::pr_for_workspace(self.cx.prs, ws)
            && ranked.number == card.number as i64
        {
            return ranked.id.clone();
        }
        hits.first().map(|p| p.id.clone()).unwrap_or_default()
    }

    pub fn reenter_phase_id(&self) -> String {
        parse_reenter_phase_id(&self.null_input())
    }

    // ---- briefing tools ----

    pub fn list_workspace_briefs_result(&self) -> Option<ListedWorkspaceBriefs> {
        if self.m.result_error {
            return None;
        }
        parse_list_workspace_briefs(&self.m.result)
    }

    pub fn workspace_context_result(&self) -> Option<WorkspaceContextView> {
        if self.m.result_error {
            return None;
        }
        parse_workspace_context(&self.m.result)
    }

    pub fn present_workspace_input_id(&self) -> String {
        let a = str_field(self.arg("workspace_id"));
        if !a.is_empty() {
            return a;
        }
        str_field(self.arg("workspaceId"))
    }

    /// `presentWorkspaceResult`.
    pub fn present_workspace_result(&self) -> Option<PresentWorkspaceView> {
        let target = parse_present_workspace_target(&self.null_input());
        let hydrate = |card: PresentWorkspaceView| {
            hydrate_present_brief(&card, self.cx.briefs, self.cx.workspaces, self.cx.projects)
        };
        if self.m.result_pending {
            let kind = target.split(':').next().unwrap_or("").to_string();
            return Some(hydrate(PresentWorkspaceView {
                ok: false,
                target,
                kind,
                brief: None,
                error: String::new(),
            }));
        }
        let card = parse_present_workspace(&self.m.result, &target);
        if card.target.is_empty()
            && card.brief.is_none()
            && card.error.is_empty()
            && self.m.result.is_empty()
        {
            return None;
        }
        Some(hydrate(card))
    }

    // ---- Edit / Write ----

    /// `editDiff`: Claude's old/new strings diffed here, or Cursor's unified-diff result.
    pub fn edit_diff(&self) -> Vec<DiffRow> {
        if let Some(old) = self.arg("old_string") {
            let s = |v: Option<&Value>| match v {
                Some(Value::String(s)) => s.clone(),
                None | Some(Value::Null) => String::new(),
                Some(other) => recur_engine::view::js::string_of(Some(other)),
            };
            return diff_lines(&s(Some(old)), &s(self.arg("new_string")));
        }
        parse_unified_diff(&self.m.result)
    }

    /// `hasEditDiff`.
    pub fn has_edit_diff(&self) -> bool {
        if self.tool() != "Edit" {
            return false;
        }
        self.arg("old_string").is_some() || looks_like_unified_diff(&self.m.result)
    }

    /// `writeBody`: args.content, else a non-diff result.
    pub fn write_body(&self) -> String {
        if let Some(Value::String(c)) = self.arg("content") {
            return c.clone();
        }
        if self.m.result_error {
            return String::new();
        }
        let r = &self.m.result;
        if self.tool() == "Write" && !r.is_empty() && !looks_like_unified_diff(r) {
            return r.clone();
        }
        if self.tool() == "Edit"
            && !r.is_empty()
            && !looks_like_unified_diff(r)
            && self.arg("old_string").is_none()
        {
            return r.clone();
        }
        String::new()
    }

    pub fn has_write_body(&self) -> bool {
        (self.tool() == "Write" || self.tool() == "Edit") && !self.write_body().is_empty()
    }

    /// `bashCommand`.
    pub fn bash_command(&self) -> String {
        match self.arg("command") {
            Some(Value::String(c)) => c.clone(),
            _ => self.m.input.clone(),
        }
    }

    /// `prettyInput`.
    pub fn pretty_input(&self) -> String {
        match &self.input {
            Some(v) => js_pretty(v),
            None => self.m.input.clone(),
        }
    }

    /// `grepFlags`: a Grep call's options as terse chips.
    pub fn grep_flags(&self) -> Vec<String> {
        let mut out = Vec::new();
        for k in ["-i", "-n", "-A", "-B", "-C", "multiline", "head_limit"] {
            match self.arg(k) {
                Some(Value::Bool(true)) => out.push(k.to_string()),
                Some(v @ Value::Number(_)) => out.push(format!(
                    "{k} {}",
                    recur_engine::view::js::string_of(Some(v))
                )),
                _ => {}
            }
        }
        if let Some(g) = self.arg_str("glob").filter(|s| !s.is_empty()) {
            out.push(format!("glob {g}"));
        }
        if let Some(t) = self.arg_str("type").filter(|s| !s.is_empty()) {
            out.push(format!("type {t}"));
        }
        out
    }

    // ---- the row ----

    /// `toolIcon`.
    pub fn icon(&self) -> StepIcon {
        if self.is_done() {
            return StepIcon::Done;
        }
        if self.is_ask_user() {
            return StepIcon::Bot;
        }
        if self.is_switch_mode() {
            return StepIcon::Plan;
        }
        if self.is_open_pr() {
            return StepIcon::File;
        }
        if self.is_report_finding() || self.is_list_findings() || self.is_update_finding() {
            return StepIcon::Finding;
        }
        if self.is_list_artifacts() || self.is_create_artifact() || self.is_present_artifact() {
            return StepIcon::Artifact;
        }
        if self.is_briefing() {
            return StepIcon::Workspace;
        }
        if self.is_linear() {
            return StepIcon::Linear;
        }
        if self.is_submit_plan() || self.is_update_plan_step() || self.is_edit_plan() {
            return StepIcon::Plan;
        }
        match self.tool() {
            "Bash" => StepIcon::Term,
            "Read" => StepIcon::File,
            "Edit" | "Write" | "NotebookEdit" => StepIcon::Pencil,
            "Delete" => StepIcon::Trash,
            "TodoWrite" => StepIcon::Todo,
            "CreatePlan" | "createPlan" => StepIcon::Plan,
            "Grep" | "Glob" | "LSP" | "readLints" | "ReadLints" => StepIcon::Search,
            "WebFetch" | "WebSearch" => StepIcon::Globe,
            "Task" | "task" | "Agent" | "SendMessage" | "TaskCreate" | "TaskUpdate" | "TaskGet"
            | "TaskList" => StepIcon::Bot,
            "ToolSearch" => StepIcon::Tools,
            _ => StepIcon::Plug,
        }
    }

    /// `pendingLabel`: what an in-flight call is doing.
    pub fn pending_label(&self) -> String {
        let f = self.file_of();
        let name = if f.is_empty() { "" } else { base_name(&f) };
        let named = |with: &str, without: &str| {
            if name.is_empty() {
                without.to_string()
            } else {
                format!("{with} {name}…")
            }
        };
        match self.tool() {
            "Edit" => return named("Editing", "Applying edit…"),
            "Write" => return named("Writing", "Writing file…"),
            "Read" => return named("Reading", "Reading file…"),
            "Delete" => return named("Deleting", "Deleting file…"),
            "Bash" => return "Running command…".into(),
            "Grep" | "Glob" => return "Searching…".into(),
            "WebFetch" => return "Fetching page…".into(),
            "WebSearch" => return "Searching the web…".into(),
            "ToolSearch" => {
                return if SELECT_PREFIX.is_match(&self.tool_search_query()) {
                    "Selecting tools…"
                } else {
                    "Searching tools…"
                }
                .into();
            }
            "TodoWrite" => return "Updating todos…".into(),
            "TaskCreate" => return "Creating task…".into(),
            "TaskUpdate" => {
                return match self.claude_task_args().status.as_str() {
                    "in_progress" => "Starting task…",
                    "completed" => "Completing task…",
                    "deleted" => "Deleting task…",
                    _ => "Updating task…",
                }
                .into();
            }
            "TaskGet" => return "Reading task…".into(),
            "TaskList" => return "Listing tasks…".into(),
            "SendMessage" => return "Sending message…".into(),
            "Task" | "task" | "Agent" => {
                let bg = self.task_background();
                return match self.subagent().subagent_type.to_lowercase().as_str() {
                    "explore" if bg => "Exploring in the background…",
                    "explore" => "Exploring…",
                    "shell" if bg => "Running shell agent in the background…",
                    "shell" => "Running shell agent…",
                    "browser-use" if bg => "Browsing in the background…",
                    "browser-use" => "Browsing…",
                    "best-of-n-runner" => "Running parallel attempt…",
                    _ if bg => "Running a subagent in the background…",
                    _ => "Running subagent…",
                }
                .into();
            }
            "CreatePlan" | "createPlan" => return "Writing plan…".into(),
            "readLints" | "ReadLints" => return "Reading lints…".into(),
            _ => {}
        }
        if self.is_report_finding() {
            return "Reporting finding…".into();
        }
        if self.is_list_findings() {
            return "Listing findings…".into();
        }
        if self.is_list_artifacts() {
            return "Listing artifacts…".into();
        }
        if self.is_update_finding() {
            return "Updating finding…".into();
        }
        if self.is_done() {
            return "Wrapping up…".into();
        }
        if self.is_ask_user() {
            return "Asking…".into();
        }
        if self.is_switch_mode() {
            let to = self.switch_mode_target();
            return if to.is_empty() {
                "Switching mode…".into()
            } else {
                format!(
                    "Switching to {}…",
                    recur_view_core::modes::ChatMode::from_key(&to).label()
                )
            };
        }
        if self.is_rename_tool() {
            let t = self.rename_tool_value();
            let noun = self.rename_tool_noun();
            return if t.is_empty() {
                format!("Renaming {noun}…")
            } else {
                format!("Renaming {noun} to {t}…")
            };
        }
        if self.is_open_pr() {
            let t = self.open_pr_card().title;
            return if t.is_empty() {
                "Opening pull request…".into()
            } else {
                format!("Opening {t}…")
            };
        }
        if self.is_child_tool() {
            return format!("{}…", self.child_tool_label());
        }
        if self.is_reenter_phase() {
            let p = self.reenter_phase_id();
            return if p.is_empty() {
                "Re-entering phase…".into()
            } else {
                format!("Re-entering {p}…")
            };
        }
        if self.is_create_artifact() {
            let t = self.create_artifact_title();
            return if t.is_empty() {
                "Creating artifact…".into()
            } else {
                format!("Creating {t}…")
            };
        }
        if self.is_present_artifact() {
            let t = self.present_artifact_title();
            return if t.is_empty() {
                "Presenting artifact…".into()
            } else {
                format!("Presenting {t}…")
            };
        }
        if self.is_list_workspace_briefs() {
            return "Listing workspaces…".into();
        }
        if self.is_get_workspace_context() {
            let t = self.present_workspace_input_id();
            return if t.is_empty() {
                "Looking up workspace…".into()
            } else {
                format!("Looking up {t}…")
            };
        }
        if self.is_present_workspace() {
            let t = parse_present_workspace_target(&self.null_input());
            return if t.is_empty() {
                "Presenting workspace…".into()
            } else {
                format!("Presenting {t}…")
            };
        }
        if self.is_submit_plan() {
            return "Submitting plan…".into();
        }
        if self.is_update_plan_step() {
            return "Updating plan step…".into();
        }
        if self.is_edit_plan() {
            return "Editing plan…".into();
        }
        if self.is_linear() {
            return format!(
                "{}…",
                linear_tool_label(self.tool(), true, false, &self.linear_args(), None)
            );
        }
        if self.tool().is_empty() {
            "Running…".into()
        } else {
            format!("Running {}…", self.tool())
        }
    }

    /// `toolLabel`: the row title: the agent's own description when it wrote one, else a
    /// derived verb (present tense while in flight).
    pub fn label(&self) -> String {
        let m = self.m;
        if self.is_claude_task() {
            return claude_task_label(
                self.tool(),
                m.result_pending,
                &self.claude_task_args(),
                self.claude_task_list_counts(),
            );
        }
        if self.is_send_message() {
            let a = parse_send_message_args(self.input.as_ref().and_then(Value::as_object));
            let d = if a.description.is_empty() {
                m.description.clone()
            } else {
                a.description
            };
            return send_message_label(m.result_pending, &d);
        }
        if !m.description.is_empty() {
            return m.description.clone();
        }
        if self.is_linear() {
            return linear_tool_label(
                self.tool(),
                m.result_pending,
                m.result_error,
                &self.linear_args(),
                self.linear_view().as_ref(),
            );
        }
        if m.result_pending
            || (self.is_task() && is_subagent_launch_meta(&m.result) && !m.result_error)
        {
            let p = self.pending_label();
            return p.strip_suffix('…').map(str::to_string).unwrap_or(p);
        }
        let f = self.file_of();
        let named = |verb: &str, none: &str| {
            if f.is_empty() {
                none.to_string()
            } else {
                format!("{verb} {}", base_name(&f))
            }
        };
        match self.tool() {
            "Bash" => return "Ran command".into(),
            "Read" => return named("Read", "Read file"),
            "Edit" => {
                return if self.has_write_body() && !self.has_edit_diff() {
                    named("Wrote", "Wrote file")
                } else {
                    named("Edited", "Edited file")
                };
            }
            "Write" => return named("Wrote", "Wrote file"),
            "Grep" => return "Grep".into(),
            "Glob" => return "Glob".into(),
            "LSP" => return "Queried language server".into(),
            "WebFetch" => return "Fetched a page".into(),
            "WebSearch" => return "Searched the web".into(),
            "Task" | "task" | "Agent" => {
                return if m.result_pending || (is_subagent_launch_ack(&m.result) && !m.result_error) {
                    "Running a subagent"
                } else {
                    "Ran a subagent"
                }
                .into();
            }
            "NotebookEdit" => return "Edited notebook".into(),
            "ToolSearch" => {
                return if SELECT_PREFIX.is_match(&self.tool_search_query()) {
                    "Selected tools"
                } else {
                    "Searched tools"
                }
                .into();
            }
            "Delete" => return named("Deleted", "Deleted file"),
            "TodoWrite" => {
                let (done, total) = todo_counts(&self.todo_items());
                return if total > 0 {
                    format!("Updated todos · {done}/{total}")
                } else {
                    "Updated todos".into()
                };
            }
            "CreatePlan" | "createPlan" => return "Wrote plan".into(),
            "readLints" | "ReadLints" => {
                return if m.result_pending {
                    "Reading lints"
                } else {
                    "Read lints"
                }
                .into();
            }
            _ => {}
        }
        if self.is_report_finding() {
            return if m.result_pending {
                "Reporting finding"
            } else {
                "Reported finding"
            }
            .into();
        }
        if self.is_list_findings() {
            return if m.result_pending {
                "Listing findings"
            } else {
                "Listed findings"
            }
            .into();
        }
        if self.is_update_finding() {
            let st = self
                .resolve_update_finding()
                .map(|f| f.status)
                .unwrap_or_default();
            if m.result_pending {
                return "Updating finding".into();
            }
            return match st.as_str() {
                "resolved" => "Marked resolved",
                "invalid" => "Marked invalid",
                _ => "Updated finding",
            }
            .into();
        }
        if self.is_done() {
            return if m.result_pending {
                "Wrapping up"
            } else {
                "Turn complete"
            }
            .into();
        }
        if self.is_ask_user() {
            if m.result_error {
                return "Ask failed".into();
            }
            return if m.result_pending { "Asking" } else { "Asked" }.into();
        }
        if self.is_switch_mode() {
            if m.result_error {
                return "Mode switch failed".into();
            }
            return if m.result_pending {
                "Switching mode"
            } else {
                "Switched mode"
            }
            .into();
        }
        if self.is_rename_tool() {
            if m.result_error || self.rename_tool_failed() {
                return "Rename failed".into();
            }
            let t = self.rename_tool_value();
            let noun = self.rename_tool_noun();
            let verb = if m.result_pending {
                "Renaming"
            } else {
                "Renamed"
            };
            return if t.is_empty() {
                format!("{verb} {noun}")
            } else {
                format!("{verb} {noun} to {t}")
            };
        }
        if self.is_child_tool() {
            return self.child_tool_label();
        }
        if self.is_open_pr() {
            return open_pr_label(&self.open_pr_card());
        }
        if self.is_reenter_phase() {
            return reenter_phase_label(
                m.result_pending,
                m.result_error || reenter_phase_failed(&m.result, false),
                &self.reenter_phase_id(),
            );
        }
        if self.is_list_artifacts() {
            return if m.result_pending {
                "Listing artifacts"
            } else {
                "Listed artifacts"
            }
            .into();
        }
        if self.is_create_artifact() {
            if m.result_error || self.create_artifact_failed() {
                return "Create artifact failed".into();
            }
            let t = self.create_artifact_title();
            let verb = if m.result_pending {
                "Creating"
            } else {
                "Created"
            };
            return if t.is_empty() {
                format!("{verb} artifact")
            } else {
                format!("{verb} {t}")
            };
        }
        if self.is_present_artifact() {
            if m.result_error || self.present_artifact_failed() {
                return "Present artifact failed".into();
            }
            let t = self.present_artifact_title();
            let verb = if m.result_pending {
                "Presenting"
            } else {
                "Presented"
            };
            return if t.is_empty() {
                format!("{verb} artifact")
            } else {
                format!("{verb} {t}")
            };
        }
        if self.is_list_workspace_briefs() {
            return list_workspace_briefs_label(
                m.result_pending,
                self.list_workspace_briefs_result().as_ref(),
            );
        }
        if self.is_get_workspace_context() {
            let title = self
                .workspace_context_result()
                .map(|c| c.brief.title)
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| self.present_workspace_input_id());
            return get_workspace_context_label(m.result_pending, &title, m.result_error);
        }
        if self.is_present_workspace() {
            return present_workspace_label(
                m.result_pending,
                self.present_workspace_result().as_ref(),
            );
        }
        if self.is_submit_plan() {
            return submit_plan_label(m.result_pending, self.submit_plan_steps().len());
        }
        if self.is_update_plan_step() {
            return update_plan_step_label(m.result_pending).into();
        }
        if self.is_edit_plan() {
            return edit_plan_label(m.result_pending).into();
        }
        if let Some(rest) = self.tool().strip_prefix("mcp__") {
            let name = rest.rsplit("__").next().unwrap_or(rest);
            return name.replace('_', " ");
        }
        if self.tool().is_empty() {
            "Tool".into()
        } else {
            self.tool().to_string()
        }
    }

    /// `toolSearchSelects`.
    pub fn tool_search_selects(&self) -> Option<Vec<String>> {
        let q = self.tool_search_query();
        let c = SELECT_Q.captures(&q)?;
        Some(
            c[1].split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        )
    }

    /// `toolMeta`: the dim header summary (Grep/Glob rebuilt with relative paths).
    pub fn meta(&self) -> String {
        let m = self.m;
        if self.tool() == "TodoWrite" {
            return self
                .todo_items()
                .into_iter()
                .find(|t| t.status == "in_progress")
                .map(|t| t.content)
                .unwrap_or_default();
        }
        if self.is_create_plan() {
            return self
                .plan_body()
                .split('\n')
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
                .unwrap_or_default();
        }
        if self.is_submit_plan() {
            return collapse_ws(&self.submit_plan_summary());
        }
        if self.is_update_plan_step() {
            let id = self.update_plan_step_id();
            let title = self.update_plan_step_title();
            let note = collapse_ws(&self.update_plan_step_note());
            let mut parts = Vec::new();
            if !id.is_empty() {
                parts.push(id.clone());
            }
            if !title.is_empty() {
                parts.push(title);
            } else if !note.is_empty() {
                parts.push(note.clone());
            }
            let joined = parts.join(" · ");
            return if joined.is_empty() {
                update_plan_step_meta(&id, &self.update_plan_step_status(), &note)
            } else {
                joined
            };
        }
        if self.is_edit_plan() {
            return prose_preview(&self.edit_plan_text());
        }
        if self.is_claude_task() {
            return claude_task_meta(self.tool(), &self.claude_task_args(), &m.result);
        }
        if self.is_subagent() || self.is_task() {
            return String::new();
        }
        if self.is_send_message() {
            let a = parse_send_message_args(self.input.as_ref().and_then(Value::as_object));
            let d = if a.description.is_empty() {
                m.description.clone()
            } else {
                a.description
            };
            return send_message_meta(&d, &a.message);
        }
        if self.is_report_finding() || self.is_update_finding() {
            let file = self.finding_file();
            let loc = self.finding_loc_suffix();
            let msg = collapse_ws(&self.finding_message());
            let mut parts = Vec::new();
            if !self.cx.has_open_file && !file.is_empty() {
                parts.push(format!("{}{loc}", rel_path(self.cx.root_path, &file)));
            } else if !self.cx.has_open_file && !loc.is_empty() {
                parts.push(format!("line {}", &loc[1..]));
            }
            if !msg.is_empty() {
                parts.push(msg);
            }
            return parts.join(" · ");
        }
        if self.is_list_findings() {
            if let Some(listed) = self.list_findings_result() {
                let st = if listed.status.is_empty() {
                    "open"
                } else {
                    &listed.status
                };
                return if listed.count == 0 {
                    format!("no {st}")
                } else {
                    format!("{} {st}", listed.count)
                };
            }
            let st = self.arg_str("status").unwrap_or("");
            return if st.is_empty() {
                "open".into()
            } else {
                st.to_string()
            };
        }
        if self.is_list_artifacts() {
            return match self.list_artifacts_result() {
                Some(l) if l.count == 0 => "none".into(),
                Some(l) => l.count.to_string(),
                None => String::new(),
            };
        }
        if self.is_done() {
            return collapse_ws(&self.done_summary());
        }
        if self.is_ask_user() {
            return collapse_ws(&self.ask_user_question());
        }
        if self.is_switch_mode()
            || self.is_rename_tool()
            || self.is_child_tool()
            || self.is_open_pr()
            || self.is_reenter_phase()
            || self.is_create_artifact()
            || self.is_present_artifact()
        {
            return String::new();
        }
        if self.is_briefing() {
            if self.is_list_workspace_briefs() {
                return self
                    .list_workspace_briefs_result()
                    .map(|l| briefing_counts_meta(&l.counts))
                    .unwrap_or_default();
            }
            if self.is_get_workspace_context() {
                return self
                    .workspace_context_result()
                    .map(|c| c.brief.now)
                    .unwrap_or_default();
            }
            return self
                .present_workspace_result()
                .map(|c| {
                    let now = c.brief.as_ref().map(|b| b.now.clone()).unwrap_or_default();
                    if now.is_empty() { c.target } else { now }
                })
                .unwrap_or_default();
        }
        if self.is_read_lints() {
            if let Some(lint) = self.read_lints_result() {
                let (n, files) = (lint.total_diagnostics, lint.total_files);
                let fs = |n: i64| format!("{n} file{}", if n == 1 { "" } else { "s" });
                if n == 0 {
                    return if files != 0 {
                        format!("{} · clean", fs(files))
                    } else {
                        "clean".into()
                    };
                }
                return format!("{n} issue{} · {}", if n == 1 { "" } else { "s" }, fs(files));
            }
            let paths = self.read_lints_paths();
            return match paths.len() {
                0 => String::new(),
                1 => rel_path(self.cx.root_path, &paths[0]).into_owned(),
                n => format!("{n} files"),
            };
        }
        if self.tool() == "ToolSearch" {
            if let Some(sel) = self.tool_search_selects().filter(|s| !s.is_empty()) {
                return sel
                    .iter()
                    .map(|n| pretty_tool_name(n))
                    .collect::<Vec<_>>()
                    .join(", ");
            }
            return self.tool_search_query();
        }
        if self.tool() == "Delete" {
            if !self.file_of().is_empty() {
                return String::new();
            }
            let ti = m.tool_input.trim();
            return if ti.starts_with('{') {
                String::new()
            } else {
                ti.to_string()
            };
        }
        if self.tool() == "WebFetch" {
            if !self.fetch_url().is_empty() {
                return String::new();
            }
            let ti = m.tool_input.trim();
            return if ti.starts_with('{') {
                String::new()
            } else {
                ti.to_string()
            };
        }
        if self.tool() == "WebSearch" {
            let q = self.search_query();
            return if q.is_empty() {
                String::new()
            } else {
                format!("\"{q}\"")
            };
        }
        if self.is_linear() {
            let args = self.linear_args();
            let view = self.linear_view();
            let label = linear_tool_label(
                self.tool(),
                m.result_pending,
                m.result_error,
                &args,
                view.as_ref(),
            );
            return linear_tool_meta(&args, view.as_ref(), &label);
        }
        if (self.tool() == "Grep" || self.tool() == "Glob")
            && let Some(pattern) = self.arg_str("pattern")
        {
            let path = self
                .arg_str("path")
                .filter(|p| !p.is_empty())
                .map(|p| rel_path(self.cx.root_path, p).into_owned())
                .unwrap_or_default();
            return if path.is_empty() {
                format!("\"{pattern}\"")
            } else {
                format!("\"{pattern}\" in {path}")
            };
        }
        m.tool_input.clone()
    }

    /// Whether the row expands (`toolCard`'s `expandable`).
    pub fn expandable(&self) -> bool {
        let m = self.m;
        let tool_search_sel = (self.tool() == "ToolSearch")
            .then(|| self.tool_search_selects())
            .flatten();
        let empty_result = m.result.trim().is_empty();
        let delete_quiet =
            self.tool() == "Delete" && !m.result_pending && !m.result_error && empty_result;
        let lin_quiet = self.is_linear()
            && !m.result_pending
            && !m.result_error
            && empty_result
            && self.linear_input_body().is_empty();
        let web_quiet =
            (self.tool() == "WebFetch" || self.tool() == "WebSearch") && !self.web_has_card();
        let claude_task = self.is_claude_task();
        let claude_body = claude_task
            && recur_agent::display::tasks::claude_task_has_body(
                self.tool(),
                &self.claude_task_args(),
                recur_agent::display::tasks::BodyOpts {
                    result_error: m.result_error,
                    result: &m.result,
                    pending: m.result_pending,
                },
            );
        (!(tool_search_sel.is_some_and(|s| !s.is_empty()) && !m.result_error)
            && !delete_quiet
            && !lin_quiet
            && !web_quiet
            && !(claude_task && !claude_body && !m.result_error))
            || !m.children.is_empty()
    }

    /// Whether the header keeps the meta line (bodies already show the full content).
    pub fn shows_meta(&self, open: bool) -> bool {
        let meta = self.meta();
        if meta.is_empty() || meta == self.file_of() {
            return false;
        }
        if open
            && (self.is_report_finding()
                || self.is_update_finding()
                || self.is_list_findings()
                || self.is_list_artifacts()
                || self.is_done()
                || self.is_ask_user()
                || self.is_read_lints()
                || self.is_submit_plan()
                || self.is_update_plan_step()
                || self.is_edit_plan()
                || self.is_briefing()
                || self.is_linear())
        {
            return false;
        }
        true
    }
}

/// `prettyToolName`.
pub fn pretty_tool_name(name: &str) -> String {
    match name.strip_prefix("mcp__") {
        Some(rest) => rest.rsplit("__").next().unwrap_or(rest).replace('_', " "),
        None => name.to_string(),
    }
}

/// `diffCounts`.
pub fn diff_counts(rows: &[DiffRow]) -> (usize, usize) {
    use recur_textmate::DiffKind;
    let add = rows.iter().filter(|r| r.kind == DiffKind::Add).count();
    let del = rows.iter().filter(|r| r.kind == DiffKind::Del).count();
    (add, del)
}

/// `thinkingPreview`.
pub fn thinking_preview(m: &AgentMsg) -> String {
    collapse_ws(&m.body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str, f: impl FnOnce(&mut AgentMsg)) -> AgentMsg {
        let mut m = AgentMsg {
            tool: name.into(),
            ..Default::default()
        };
        f(&mut m);
        m
    }

    fn view(m: &AgentMsg) -> ToolView<'_> {
        ToolView::new(
            m,
            ToolCx {
                root_path: "/wt",
                has_open_file: true,
                ..Default::default()
            },
        )
    }

    #[test]
    fn file_tools_read_their_paths() {
        let m = tool("Read", |m| {
            m.input = json!({"file_path": "/wt/src/a.go"}).to_string()
        });
        let v = view(&m);
        assert_eq!(v.file_of(), "/wt/src/a.go");
        assert_eq!(v.label(), "Read a.go");
        assert_eq!(v.icon(), StepIcon::File);
        let m = tool("Edit", |m| {
            m.input = json!({"file_path": "/wt/b.rs", "old_string": "a\nb", "new_string": "a\nc"})
                .to_string();
        });
        let v = view(&m);
        assert!(v.has_edit_diff());
        assert_eq!(v.label(), "Edited b.rs");
        assert_eq!(diff_counts(&v.edit_diff()), (1, 1));
        let m = tool("Edit", |m| {
            m.input = json!({"file_path": "/wt/new.txt"}).to_string();
            m.result = "hello\n".into();
        });
        assert_eq!(view(&m).label(), "Wrote new.txt");
        let m = tool("Edit", |m| {
            m.input = json!({"file_path": "/wt/a.go"}).to_string();
            m.result_pending = true;
        });
        assert_eq!(view(&m).label(), "Editing a.go");
        assert_eq!(view(&m).pending_label(), "Editing a.go…");
    }

    #[test]
    fn grep_meta_uses_relative_paths() {
        let m = tool("Grep", |m| {
            m.input = json!({"pattern": "TODO", "path": "/wt/src", "-i": true, "head_limit": 5, "glob": "*.go"}).to_string();
            m.tool_input = "TODO in /wt/src".into();
        });
        let v = view(&m);
        assert_eq!(v.meta(), "\"TODO\" in src");
        assert_eq!(v.grep_flags(), ["-i", "head_limit 5", "glob *.go"]);
    }

    #[test]
    fn tool_search_selects_are_flat_rows() {
        let m = tool("ToolSearch", |m| {
            m.input = json!({"query": "select:mcp__recur__list_findings,Read"}).to_string();
        });
        let v = view(&m);
        assert_eq!(v.label(), "Selected tools");
        assert_eq!(v.meta(), "list findings, Read");
        assert!(!v.expandable());
        let m = tool("ToolSearch", |m| {
            m.input = json!({"query": "notebook", "max_results": 3}).to_string();
        });
        let v = view(&m);
        assert_eq!(v.label(), "Searched tools");
        assert!(v.expandable());
        assert_eq!(v.tool_search_max(), 3);
    }

    #[test]
    fn finding_rows() {
        let m = tool("mcp__recur__report_finding", |m| {
            m.input = json!({"file": "store.go", "line": 1276, "end_line": 1286, "severity": "high", "message": "activity hook\ncan miss"}).to_string();
        });
        let v = view(&m);
        assert_eq!(v.label(), "Reported finding");
        assert_eq!(v.finding_severity(), "high");
        assert_eq!(v.finding_loc_suffix(), ":1276–1286");
        assert_eq!(v.meta(), "activity hook can miss");
        let store = vec![Finding {
            id: "uuid-1".into(),
            short_id: "find_1".into(),
            file: "a/b.go".into(),
            line: 3,
            severity: "medium".into(),
            message: "msg".into(),
            ..Default::default()
        }];
        let m = tool("mcp__recur__update_finding", |m| {
            m.input = json!({"finding_id": "find_1", "status": "resolved", "reason": "fixed"})
                .to_string();
        });
        let v = ToolView::new(
            &m,
            ToolCx {
                findings: &store,
                ..Default::default()
            },
        );
        assert_eq!(v.label(), "Marked resolved");
        assert_eq!(v.file_of(), "a/b.go");
        assert_eq!(v.finding_status(), "resolved");
        assert_eq!(v.finding_status_reason(), "fixed");
        assert_eq!(v.meta(), "a/b.go:3 · msg");
    }

    #[test]
    fn subagent_and_task_labels() {
        let m = tool("Task", |m| {
            m.input = json!({"description": "", "subagent_type": "Explore", "run_in_background": true, "prompt": "p"}).to_string();
            m.result_pending = true;
        });
        let v = view(&m);
        assert_eq!(v.pending_label(), "Exploring in the background…");
        assert_eq!(v.label(), "Exploring in the background");
        let m = tool("Agent", |m| m.result = "done".into());
        assert_eq!(view(&m).label(), "Ran a subagent");
        assert_eq!(view(&m).meta(), "");
    }

    #[test]
    fn mode_rename_child_and_artifact_rows() {
        let m = tool("mcp__recur__switch_mode", |m| {
            m.input = json!({"mode": "plan"}).to_string();
            m.result = "Switched chat mode from agent to plan.".into();
        });
        let v = view(&m);
        assert_eq!(
            v.switch_mode_transition(),
            Some(("agent".into(), "plan".into()))
        );
        assert!(!v.switch_mode_failed());
        let m = tool("mcp__recur__switch_mode", |m| {
            m.input = json!({"mode": "loop"}).to_string();
            m.result_pending = true;
        });
        assert_eq!(view(&m).pending_label(), "Switching to Loop…");
        let m = tool("mcp__recur__rename_branch", |m| {
            m.result = "Renamed branch to feat/x.".into();
        });
        let v = view(&m);
        assert_eq!(v.rename_tool_value(), "feat/x");
        assert_eq!(v.label(), "Renamed branch to feat/x");
        let m = tool("mcp__recur__merge_child", |m| {
            m.input = json!({"workspace_id": "child-1"}).to_string();
            m.result = "merge_child: conflicts in a.go\nmore".into();
        });
        let v = view(&m);
        assert!(v.child_tool_failed());
        assert_eq!(v.label(), "Merge refused: subagent child-1");
        assert_eq!(v.child_tool_note(), "conflicts in a.go");
        let m = tool("mcp__recur__create_artifact", |m| {
            m.result =
                "Created artifact {\"id\":\"a1\",\"title\":\"Notes\",\"kind\":\"MD\"}".into();
        });
        let v = view(&m);
        assert_eq!(v.create_artifact_title(), "Notes");
        assert_eq!(v.create_artifact_kind(), "md");
        let m = tool("mcp__recur__present_artifact", |m| {
            m.result = "Presented artifact a1 (Notes).".into();
        });
        assert_eq!(view(&m).label(), "Presented Notes");
    }

    #[test]
    fn lint_results() {
        let m = tool("readLints", |m| {
            m.result = json!({"fileDiagnostics": [{"path": "/wt/a.ts", "diagnostics": [
                {"severity": 1, "message": "boom", "range": {"start": {"line": 4}, "end": {"line": 6}}, "source": "ts"}
            ]}], "totalFiles": 1}).to_string();
        });
        let v = view(&m);
        let lint = v.read_lints_result().expect("parsed");
        assert_eq!(lint.total_diagnostics, 1);
        let d = &lint.files[0].diagnostics[0];
        assert_eq!((d.severity.as_str(), d.line, d.end_line), ("error", 5, 7));
        assert_eq!(lint_loc_suffix(d), ":5–7");
        assert_eq!(lint_sev_class(&d.severity), "high");
        assert_eq!(v.meta(), "1 issue · 1 file");
    }

    #[test]
    fn durations_and_pretty_json() {
        assert_eq!(format_duration(0), "");
        assert_eq!(format_duration(1200), "1.2s");
        assert_eq!(format_duration(1000), "1.0s");
        assert_eq!(format_duration(1250), "1.3s");
        assert_eq!(format_duration(12_400), "12s");
        assert_eq!(format_duration(103_000), "1m 43s");
        assert_eq!(format_duration(3_723_000), "1h 2m 03s");
        assert_eq!(
            pretty_result("{\"b\":1,\"a\":[1,2]}"),
            "{\n  \"b\": 1,\n  \"a\": [\n    1,\n    2\n  ]\n}"
        );
        assert_eq!(pretty_result("plain"), "plain");
        assert_eq!(pretty_tool_name("mcp__linear__list_issues"), "list issues");
    }

    #[test]
    fn subagent_create_is_dropped() {
        let m = tool("mcp__recur__create_workspace", |m| {
            m.result = "Created subagent ws-2".into()
        });
        assert!(view(&m).is_subagent_create());
        let m = tool("mcp__recur__create_workspace", |m| {
            m.input = json!({"standalone": true}).to_string();
        });
        assert!(!view(&m).is_subagent_create());
    }
}
