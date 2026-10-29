//! Cursor's tool vocabulary in the common one, at the display boundary (Go
//! `engine/cursortools.go` and the Cursor branch of `toolview.go`).
//!
//! The session store keeps what Cursor sent (`shell` with
//! `{command, timeout}` and its `{"status":…,"value":{…}}` result); the UI
//! shows one vocabulary across providers, so a Cursor `shell` renders in the
//! same card as a Claude `Bash`. Shapes are transcribed from live bridge
//! traffic: Cursor documents neither the tool list nor the schemas.

use serde_json::{Map, Value};

use recur_agent::display::{FEED_INPUT_LIMIT, ToolDisplay, truncate};

use crate::events::{TOOL_NAME_MCP, tool_call_args};

/// A Cursor tool name in Recur's vocabulary. Unknown names pass through: a
/// tool added after this build renders with its raw name and a generic icon.
pub fn canonical_tool(name: &str) -> &str {
    match name {
        "shell" => "Bash",
        "read" => "Read",
        "edit" => "Edit",
        "write" => "Write",
        "glob" => "Glob",
        "grep" => "Grep",
        "delete" => "Delete",
        "updateTodos" => "TodoWrite",
        "task" => "Task",
        // Plan mode's create-plan tool: args carry the markdown, the result
        // is an empty ack.
        "createPlan" => "CreatePlan",
        other => other,
    }
}

/// A raw Cursor tool call in the common vocabulary. MCP calls only unwrap
/// their routing envelope; everything else maps name and input.
pub fn display_tool(name: &str, input: &str) -> ToolDisplay {
    if let Some((common, params)) = cursor_mcp_display(name, input) {
        return ToolDisplay {
            name: common,
            input: params,
        };
    }
    ToolDisplay {
        name: canonical_tool(name).to_owned(),
        input: cursor_tool_input(name, input),
    }
}

/// Unwraps `{providerIdentifier, toolName, args}` into `mcp__<server>__<tool>`
/// and the tool's own parameters; `None` for non-MCP calls.
pub(crate) fn cursor_mcp_display(name: &str, input: &str) -> Option<(String, String)> {
    if name != TOOL_NAME_MCP {
        return None;
    }
    match tool_call_args(input) {
        Some(a) if !a.tool_name.is_empty() => {
            let common = if a.provider_identifier.is_empty() {
                format!("mcp__{}", a.tool_name)
            } else {
                format!("mcp__{}__{}", a.provider_identifier, a.tool_name)
            };
            Some((common, a.params))
        }
        _ => Some((name.to_owned(), input.to_owned())),
    }
}

/// Cursor's arguments rewritten into the canonical input the cards read
/// (`command`, `file_path`, `pattern`, …). Empty when there is nothing useful
/// to show.
pub fn cursor_tool_input(name: &str, raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let a: Map<String, Value> = match serde_json::from_str::<Option<Map<String, Value>>>(raw) {
        Ok(m) => m.unwrap_or_default(),
        Err(_) => return String::new(),
    };
    let s = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or("");
    let mut out = Map::new();
    let mut put = |k: &str, v: &str| {
        if !v.is_empty() {
            out.insert(k.to_owned(), Value::String(v.to_owned()));
        }
    };
    match name {
        // `timeout` is plumbing, not worth a chip.
        "shell" => put("command", s("command")),
        "read" | "edit" | "delete" => put("file_path", s("path")),
        "glob" => {
            put("pattern", s("globPattern"));
            put("path", s("targetDirectory"));
        }
        "grep" => {
            put("pattern", s("pattern"));
            put("path", s("path"));
        }
        "updateTodos" => {
            if let Some(t) = a.get("todos") {
                out.insert("todos".into(), normalize_todos(t.clone()));
            }
        }
        // Claude Code's Task shape: row title, the prompt the subagent runs,
        // optional type and model.
        "task" => {
            put("description", s("description"));
            put("prompt", s("prompt"));
            put("subagent_type", s("subagent_type"));
            put("model", s("model"));
        }
        // Unknown keys stay out of the card.
        "createPlan" => put("plan", s("plan")),
        // An unmapped tool keeps its arguments: a raw JSON card beats an
        // empty one.
        _ => return raw.to_owned(),
    }
    if out.is_empty() {
        return String::new();
    }
    Value::Object(out).to_string()
}

/// A completed Cursor tool call as the text cards and transcript show.
/// Cursor nests results under `{"status":…,"value":{…}}` with a per-tool
/// shape.
pub fn cursor_tool_result_text(name: &str, raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Envelope {
        // Decoded only so a non-string status fails like Go's typed decode.
        #[serde(rename = "status", deserialize_with = "crate::wire::null_default")]
        _status: String,
        #[serde(deserialize_with = "crate::wire::raw")]
        value: String,
    }
    let Ok(env) = crate::wire::decode_struct::<Envelope>(raw) else {
        return String::new();
    };
    if env.value.is_empty() {
        return String::new();
    }
    let value_raw = env.value;
    let v: Map<String, Value> = match serde_json::from_str::<Option<Map<String, Value>>>(&value_raw)
    {
        Ok(m) => m.unwrap_or_default(),
        Err(_) => return String::new(),
    };
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("");

    match name {
        // What a terminal shows: stdout, then stderr, then a non-zero exit.
        "shell" => {
            let mut b = s("stdout").to_owned();
            let e = s("stderr");
            if !e.is_empty() {
                if !b.is_empty() {
                    b.push('\n');
                }
                b.push_str(e);
            }
            if let Some(code) = v.get("exitCode").and_then(Value::as_f64)
                && code != 0.0
            {
                if !b.is_empty() {
                    b.push('\n');
                }
                b.push_str(&format!("exit {}", code as i64));
            }
            b
        }
        "read" => s("content").to_owned(),
        // The edit's diff is in the RESULT; its args carry only the path.
        "edit" => s("diffString").to_owned(),
        "glob" => v
            .get("files")
            .and_then(Value::as_array)
            .map(|fs| {
                fs.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
        "grep" => grep_result_text(&v),
        "delete" | "updateTodos" => String::new(),
        "task" => task_result_text(&value_raw),
        // Ack only: the plan body is in the args.
        "createPlan" => String::new(),
        TOOL_NAME_MCP => mcp_result_text(&value_raw),
        // Unknown tool: the raw value rather than dropping the output.
        _ => truncate(value_raw.trim(), FEED_INPUT_LIMIT),
    }
}

/// The subagent's return text from Cursor's Task result. Undocumented shape:
/// common string fields first, then a compact dump so the card is never
/// empty when the provider sent something.
fn task_result_text(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    if let Ok(s) = serde_json::from_str::<String>(value) {
        return s;
    }
    let v: Map<String, Value> = match serde_json::from_str::<Option<Map<String, Value>>>(value) {
        Ok(m) => m.unwrap_or_default(),
        Err(_) => return truncate(value.trim(), FEED_INPUT_LIMIT),
    };
    ["text", "result", "output", "message", "content"]
        .iter()
        .filter_map(|k| v.get(*k).and_then(Value::as_str))
        .find(|s| !s.is_empty())
        .map_or_else(|| truncate(value.trim(), FEED_INPUT_LIMIT), str::to_owned)
}

/// Cursor's camelCase "inProgress" → the common "in_progress"; anything else
/// passes through so a new status shows as-is.
pub(crate) fn normalize_todos(v: Value) -> Value {
    let Value::Array(items) = v else {
        return v;
    };
    Value::Array(
        items
            .into_iter()
            .map(|it| match it {
                Value::Object(mut m) => {
                    if m.get("status").and_then(Value::as_str) == Some("inProgress") {
                        m.insert("status".into(), Value::String("in_progress".into()));
                    }
                    Value::Object(m)
                }
                other => other,
            })
            .collect(),
    )
}

/// `workspaceResults → <dir> → output.matches[].file` as a file list.
fn grep_result_text(v: &Map<String, Value>) -> String {
    let mut lines = Vec::new();
    if let Some(ws) = v.get("workspaceResults").and_then(Value::as_object) {
        for res in ws.values() {
            let matches = res
                .get("output")
                .and_then(|o| o.get("matches"))
                .and_then(Value::as_array);
            for m in matches.into_iter().flatten() {
                if let Some(f) = m.get("file").and_then(Value::as_str) {
                    lines.push(f);
                }
            }
        }
    }
    lines.join("\n")
}

/// MCP result text, wrapped one level deeper than the MCP spec:
/// `{"content":[{"text":{"text":"…"}}]}`. Any shape mismatch reads as empty,
/// as Go's typed decode fails.
fn mcp_result_text(value: &str) -> String {
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Inner {
        #[serde(deserialize_with = "crate::wire::null_default")]
        text: String,
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Item {
        #[serde(deserialize_with = "crate::wire::null_default")]
        text: Inner,
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct V {
        #[serde(deserialize_with = "crate::wire::null_default")]
        content: Vec<Option<Item>>,
    }
    match crate::wire::decode_struct::<V>(value) {
        Ok(v) => v
            .content
            .into_iter()
            .flatten()
            .map(|c| c.text.text)
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
pub(crate) mod tests;
