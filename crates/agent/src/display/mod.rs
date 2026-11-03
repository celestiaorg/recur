//! Provider-neutral display helpers: feed summaries, chat detail-card
//! bounds, and the display boundary for tool calls.
//!
//! The session store holds whatever the provider sent; the UI shows one
//! vocabulary across providers. Translation lives at this read/render
//! boundary and never on the way in, so changing a mapping re-renders old
//! transcripts instead of leaving them stale.

mod compact;
pub mod tasks;
pub mod todos;
pub mod web;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::Provider;

pub use compact::compact_tool_result;

/// Decodes JSON `null` as the field's default.
pub(crate) fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// Feed summaries stay readable in Activity expand; collapsed rows clip in
/// the UI, so no "…" is baked into stored feed text below this.
pub const FEED_INPUT_LIMIT: usize = 8000;

/// Bounds a chat tool call's full input JSON / result text in the expanded
/// detail card.
pub const CHAT_DETAIL_LIMIT: usize = 16000;

/// Bounds a persisted plan (DefinitionOfDone, plan cards).
pub const PLAN_MAX_PERSIST_CHARS: usize = 256 << 10;

/// A tool call in the common vocabulary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolDisplay {
    /// Common name, e.g. `Bash`.
    pub name: String,
    /// Common input JSON, e.g. `{"command":…}`; empty = none.
    pub input: String,
}

/// A provider's tool result as the cards and transcript show it. Background
/// Task launch acks render as nothing.
pub fn display_tool_result(provider: &dyn Provider, name: &str, raw: &str) -> String {
    let out = provider.display_tool_result(name, raw);
    if is_task_tool(name) && is_subagent_launch_meta(&out) {
        return String::new();
    }
    out
}

/// The feed's one-line summary of a raw provider tool call, via the common
/// shape so the wording matches across providers.
pub fn summarize_display_input(provider: &dyn Provider, name: &str, input: &str) -> String {
    let d = provider.display_tool(name, input);
    summarize_tool_input(&d.name, &d.input)
}

/// Claude Code tool names that drifted from Recur's vocabulary: `Agent` was
/// renamed from `Task` in v2.1.63 and the subagent card is keyed on `Task`.
pub fn canonical_claude_tool(name: &str) -> &str {
    match name {
        "Agent" => "Task",
        _ => name,
    }
}

/// At most `n` bytes plus an ellipsis, cut on a rune start at or before byte
/// `n - 1`.
pub fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_owned();
    }
    let mut cut = n.saturating_sub(1);
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

/// At most `n` runes, no ellipsis (stored field limits).
pub fn hard_truncate_runes(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

pub fn is_task_tool(name: &str) -> bool {
    matches!(name, "Task" | "task" | "Agent")
}

/// Cursor's create-plan tool, raw or common spelling.
pub fn is_create_plan_tool(name: &str) -> bool {
    name == "createPlan" || name == "CreatePlan"
}

/// The plan markdown of a createPlan input; empty when missing, malformed,
/// or not yet filled in (Cursor streams a first `{"plan":""}` frame).
pub fn create_plan_body(raw: &str) -> String {
    #[derive(Deserialize)]
    struct P {
        #[serde(default, deserialize_with = "null_default")]
        plan: String,
    }
    if raw.is_empty() {
        return String::new();
    }
    serde_json::from_str::<P>(raw)
        .map(|p| p.plan.trim().to_owned())
        .unwrap_or_default()
}

/// Claude Code's background-Task ack. Anchored to the ack prefix so a file
/// that merely mentions the phrase is not taken for one.
pub fn is_subagent_launch_meta(s: &str) -> bool {
    s.trim().starts_with("Async agent launched successfully")
}

fn is_tool(name: &str, bare: &str) -> bool {
    name == bare || (name.ends_with(bare) && name[..name.len() - bare.len()].ends_with("__"))
}

pub fn is_list_findings_tool(name: &str) -> bool {
    is_tool(name, "list_findings")
}

/// A tool call's input JSON for the live/history detail card. A mid-JSON
/// byte cut breaks the card's JSON parse, so CreatePlan re-serializes with a
/// cleanly truncated body and Task keeps a parseable envelope.
pub fn chat_tool_input(tool: &str, input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    if is_create_plan_tool(tool) {
        let body = create_plan_body(input);
        if body.is_empty() {
            return truncate(input, CHAT_DETAIL_LIMIT);
        }
        let body = truncate(&body, PLAN_MAX_PERSIST_CHARS);
        let mut m = std::collections::BTreeMap::new();
        m.insert("plan", body);
        return serde_json::to_string(&m).unwrap_or_else(|_| truncate(input, CHAT_DETAIL_LIMIT));
    }
    if is_task_tool(tool) {
        return chat_task_input(input);
    }
    truncate(input, CHAT_DETAIL_LIMIT)
}

fn chat_task_input(input: &str) -> String {
    if input.len() <= CHAT_DETAIL_LIMIT
        && serde_json::from_str::<serde::de::IgnoredAny>(input).is_ok()
    {
        return input.to_owned();
    }
    let mut v = match serde_json::from_str::<Value>(input) {
        Ok(v @ (Value::Object(_) | Value::Null)) => v,
        _ => return truncate(input, CHAT_DETAIL_LIMIT),
    };
    if let Value::Object(m) = &mut v
        && let Some(Value::String(p)) = m.get_mut("prompt")
        && !p.is_empty()
    {
        *p = truncate(p, PLAN_MAX_PERSIST_CHARS);
    }
    serde_json::to_string(&v).unwrap_or_else(|_| truncate(input, CHAT_DETAIL_LIMIT))
}

/// A tool result for the live/history detail card. Listing tools return
/// JSON the card's accordion parses, so they get a compact, still-parseable
/// envelope instead of a byte cut.
pub fn chat_tool_result(tool: &str, result: &str) -> String {
    if result.is_empty() {
        return String::new();
    }
    if is_task_tool(tool) && is_subagent_launch_meta(result) {
        return String::new();
    }
    if let Some(compact) = compact_tool_result(tool, result) {
        return compact;
    }
    truncate(result, CHAT_DETAIL_LIMIT)
}

/// The agent's own one-line description of a call (Claude sets
/// `input.description` on Bash/Task). TaskCreate/TaskUpdate/TaskGet/TaskList
/// `description` is a task BODY, not a row title.
pub fn tool_description(tool: &str, raw: &str) -> String {
    #[derive(Deserialize)]
    struct D {
        #[serde(default, deserialize_with = "null_default")]
        description: String,
    }
    if raw.is_empty() || matches!(tool, "TaskCreate" | "TaskUpdate" | "TaskGet" | "TaskList") {
        return String::new();
    }
    match serde_json::from_str::<Option<D>>(raw) {
        Ok(Some(d)) => d.description.trim().to_owned(),
        _ => String::new(),
    }
}

/// A plan update as "2/5 · <the in-progress item>".
fn summarize_todos(m: &Map<String, Value>) -> String {
    let Some(Value::Array(items)) = m.get("todos") else {
        return String::new();
    };
    if items.is_empty() {
        return String::new();
    }
    let mut done = 0;
    let mut active = "";
    for it in items {
        let status = it.get("status");
        if status == Some(&Value::String("completed".into())) {
            done += 1;
        } else if status == Some(&Value::String("in_progress".into()))
            && let Some(Value::String(s)) = it.get("content")
            && active.is_empty()
        {
            active = s;
        }
    }
    let head = format!("{done}/{}", items.len());
    if !active.is_empty() {
        return truncate(&format!("{head} · {active}"), FEED_INPUT_LIMIT);
    }
    head
}

/// A human-readable tool input for the activity feed and step stream: the
/// salient field per tool (file_path, command, summary) instead of raw JSON.
pub fn summarize_tool_input(tool: &str, raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let fallback = || truncate(raw.trim(), FEED_INPUT_LIMIT);
    let empty = Map::new();
    let parsed = serde_json::from_str::<Value>(raw);
    let input = match &parsed {
        Ok(Value::Object(m)) => m,
        Ok(Value::Null) => &empty,
        _ => return fallback(),
    };
    let s = |k: &str| match input.get(k) {
        Some(Value::String(v)) => v.as_str(),
        _ => "",
    };
    let num = |k: &str| match input.get(k).and_then(Value::as_f64) {
        Some(n) if n > 0.0 => n as i64,
        _ => 0,
    };
    let parts = |a: &str, b: &str, extra: &str| {
        let mut parts = Vec::with_capacity(2);
        match (a.is_empty(), b.is_empty()) {
            (false, false) => parts.push(format!("{a} → {b}")),
            (false, true) => parts.push(a.to_owned()),
            (true, false) => parts.push(b.to_owned()),
            (true, true) => {}
        }
        if !extra.is_empty() {
            parts.push(extra.to_owned());
        }
        parts
    };
    match tool {
        "Read" | "Edit" | "Write" | "Delete" if !s("file_path").is_empty() => {
            return s("file_path").to_owned();
        }
        "Bash" if !s("command").is_empty() => return truncate(s("command"), FEED_INPUT_LIMIT),
        "Glob" => {
            for k in ["pattern", "glob_pattern"] {
                if !s(k).is_empty() {
                    return s(k).to_owned();
                }
            }
        }
        "Grep" => {
            let p = s("pattern");
            if !p.is_empty() {
                let path = s("path");
                if !path.is_empty() {
                    return format!("{:?} in {path}", p);
                }
                return format!("{p:?}");
            }
        }
        "WebFetch" if !s("url").is_empty() => return truncate(s("url"), FEED_INPUT_LIMIT),
        "WebSearch" => {
            for k in ["query", "search_term"] {
                if !s(k).is_empty() {
                    return truncate(s(k), FEED_INPUT_LIMIT);
                }
            }
        }
        "ToolSearch" if !s("query").is_empty() => return s("query").to_owned(),
        "TodoWrite" => return summarize_todos(input),
        "CreatePlan" if !s("plan").is_empty() => {
            return truncate(first_line_of(s("plan")).trim(), FEED_INPUT_LIMIT);
        }
        "Skill" => {
            let sk = s("skill");
            if !sk.is_empty() {
                let a = s("args");
                if !a.is_empty() {
                    return format!("{sk} {}", truncate(a, 120));
                }
                return sk.to_owned();
            }
        }
        "Task" => {
            if !s("subagent_type").is_empty() {
                return s("subagent_type").to_owned();
            }
            if !s("prompt").is_empty() {
                return truncate(s("prompt"), 120);
            }
        }
        "SendMessage" => {
            for k in ["description", "summary"] {
                if !s(k).is_empty() {
                    return truncate(s(k), FEED_INPUT_LIMIT);
                }
            }
            for k in ["prompt", "message"] {
                if !s(k).is_empty() {
                    return truncate(s(k), 120);
                }
            }
        }
        "TaskCreate" if !s("subject").is_empty() => {
            return truncate(s("subject"), FEED_INPUT_LIMIT);
        }
        "TaskUpdate" => {
            let mut id = s("taskId");
            if id.is_empty() {
                id = s("task_id");
            }
            let id = if id.is_empty() {
                String::new()
            } else {
                format!("#{}", id.strip_prefix('#').unwrap_or(id))
            };
            let p = parts(&id, s("status"), s("subject"));
            if !p.is_empty() {
                return truncate(&p.join(" · "), FEED_INPUT_LIMIT);
            }
        }
        "TaskGet" => {
            let mut id = s("taskId");
            if id.is_empty() {
                id = s("task_id");
            }
            if !id.is_empty() {
                return format!("#{}", id.strip_prefix('#').unwrap_or(id));
            }
        }
        "TaskList" => return String::new(),
        _ => {}
    }
    if tool.starts_with("mcp__recur__") {
        let suffix = |sfx: &str| tool.ends_with(sfx);
        if suffix("__done") || suffix("__plan") || suffix("__submit_plan") {
            if !s("summary").is_empty() {
                return truncate(s("summary"), FEED_INPUT_LIMIT);
            }
        } else if suffix("__edit_plan") {
            if !s("plan").is_empty() {
                return truncate(first_line_of(s("plan")).trim(), FEED_INPUT_LIMIT);
            }
        } else if suffix("__update_plan_step") {
            let p = parts(s("step_id"), s("status"), s("note"));
            if !p.is_empty() {
                return truncate(&p.join(" · "), FEED_INPUT_LIMIT);
            }
        } else if suffix("__report_finding") {
            let loc = recur_domain::finding_loc(s("file"), num("line"), num("end_line"));
            let sev = s("severity");
            let mut p = Vec::with_capacity(3);
            if !sev.is_empty() && sev != "info" {
                p.push(sev.to_owned());
            }
            if !loc.is_empty() {
                p.push(loc);
            }
            if !s("message").is_empty() {
                p.push(s("message").to_owned());
            }
            if !p.is_empty() {
                return truncate(&p.join(" · "), FEED_INPUT_LIMIT);
            }
        } else if suffix("__list_findings") {
            let st = s("status");
            if !st.is_empty() && st != "open" {
                return st.to_owned();
            }
            return "open".to_owned();
        } else if suffix("__list_skills") {
            return "skills".to_owned();
        } else if suffix("__list_terminals") {
            return "terminals".to_owned();
        } else if suffix("__read_terminal") {
            if !s("id").is_empty() {
                return truncate(s("id"), FEED_INPUT_LIMIT);
            }
            return "terminal".to_owned();
        } else if suffix("__read_artifact") {
            if !s("id").is_empty() {
                return truncate(s("id"), FEED_INPUT_LIMIT);
            }
            return "artifact".to_owned();
        } else if suffix("__activate_skill") {
            if !s("name").is_empty() {
                return truncate(&format!("/{}", s("name")), FEED_INPUT_LIMIT);
            }
        } else if suffix("__update_finding") {
            let (id, st) = (s("finding_id"), s("status"));
            if !id.is_empty() && !st.is_empty() {
                return truncate(&format!("{id} → {st}"), FEED_INPUT_LIMIT);
            }
            if !id.is_empty() {
                return id.to_owned();
            }
        } else if suffix("__report_result") {
            if !s("summary").is_empty() {
                return truncate(s("summary"), FEED_INPUT_LIMIT);
            }
        } else if suffix("__escalate") || suffix("__ask_user") {
            if !s("question").is_empty() {
                return truncate(s("question"), FEED_INPUT_LIMIT);
            }
        } else if suffix("__switch_mode") {
            if !s("mode").is_empty() {
                return s("mode").to_owned();
            }
        } else if suffix("__open_pr") {
            let t = s("title");
            if !t.is_empty() {
                if input.get("draft") == Some(&Value::Bool(true)) {
                    return truncate(&format!("draft · {t}"), FEED_INPUT_LIMIT);
                }
                return truncate(t, FEED_INPUT_LIMIT);
            }
        } else if suffix("__attach_evidence") && !s("step").is_empty() {
            return truncate(s("step"), FEED_INPUT_LIMIT);
        }
        if !s("summary").is_empty() {
            return truncate(s("summary"), FEED_INPUT_LIMIT);
        }
    }
    if is_linear_mcp_tool(tool) {
        for k in [
            "query", "id", "issue", "team", "name", "title", "urlOrId", "project", "user",
        ] {
            if !s(k).is_empty() {
                return truncate(s(k), FEED_INPUT_LIMIT);
            }
        }
        return String::new();
    }
    fallback()
}

fn first_line_of(s: &str) -> &str {
    s.split('\n').next().unwrap_or_default()
}

fn is_linear_mcp_tool(tool: &str) -> bool {
    let Some(rest) = tool.strip_prefix("mcp__") else {
        return false;
    };
    match rest.split_once("__") {
        Some((server, _)) => server.to_lowercase().contains("linear"),
        None => false,
    }
}

#[cfg(test)]
mod tests;
