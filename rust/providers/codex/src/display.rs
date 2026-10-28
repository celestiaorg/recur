//! The display boundary for Codex tool calls (the Codex branch of Go
//! `toolview.go`): raw `shell` / `edit` / `write` / `delete` calls render in
//! the common cards; `mcp__<server>__<tool>` names already are common.

use std::collections::BTreeMap;

use serde_json::Value;

/// Cursor and Codex tool names in Recur's common vocabulary (Go
/// `canonicalTool`); unknown names pass through.
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
        "createPlan" => "CreatePlan",
        _ => name,
    }
}

/// Codex tool arguments in the common card shape (`command`, `file_path`,
/// `content`); anything unrecognized passes through.
pub fn tool_input(name: &str, raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let a = match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(a)) => a,
        Ok(Value::Null) => Default::default(),
        _ => return raw.to_owned(),
    };
    let s = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or_default();
    let mut out: BTreeMap<&str, &str> = BTreeMap::new();
    match name {
        "shell" => {
            if !s("command").is_empty() {
                out.insert("command", s("command"));
            }
        }
        "edit" | "write" | "Write" | "read" | "delete" => {
            let path = if s("path").is_empty() {
                s("file_path")
            } else {
                s("path")
            };
            if !path.is_empty() {
                out.insert("file_path", path);
            }
            if matches!(name, "write" | "Write") && !s("content").is_empty() {
                out.insert("content", s("content"));
            }
        }
        _ => return raw.to_owned(),
    }
    if out.is_empty() {
        return raw.to_owned();
    }
    serde_json::to_string(&out).unwrap_or_else(|_| raw.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs() {
        assert_eq!(
            tool_input("shell", r#"{"command":"ls","timeout":5}"#),
            r#"{"command":"ls"}"#
        );
        assert_eq!(
            tool_input("edit", r#"{"path":"/a"}"#),
            r#"{"file_path":"/a"}"#
        );
        assert_eq!(
            tool_input("write", r#"{"file_path":"/a","content":"x"}"#),
            r#"{"content":"x","file_path":"/a"}"#
        );
        assert_eq!(
            tool_input("mcp__recur__done", r#"{"summary":"s"}"#),
            r#"{"summary":"s"}"#
        );
        assert_eq!(tool_input("shell", "{}"), "{}");
        assert_eq!(tool_input("shell", "[1]"), "[1]");
        assert_eq!(tool_input("shell", ""), "");
        assert_eq!(canonical_tool("shell"), "Bash");
        assert_eq!(canonical_tool("mcp__x__y"), "mcp__x__y");
    }
}
