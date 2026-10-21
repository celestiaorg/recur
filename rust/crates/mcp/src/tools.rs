//! The tool surface served over MCP and the allowlist helpers the engine
//! uses when registering a turn.

use std::sync::Arc;

use recur_agent_tools::names::{PERMISSION_REQUEST, RECUR_MCP_PREFIX};
use recur_agent_tools::{Tool, host_tools};

/// A Recur host tool on the MCP surface. Coding tools are harness-only and
/// are not served here.
pub(crate) struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: String,
    pub tool: Arc<dyn Tool>,
}

pub(crate) fn all_tools() -> Vec<ToolSpec> {
    host_tools()
        .into_iter()
        .map(|tool| ToolSpec {
            name: tool.name(),
            description: tool.description(),
            schema: tool.schema(),
            tool,
        })
        .collect()
}

/// Bare Recur tool names from a runner `--allowedTools` list. Always a
/// (possibly empty) list so registration can tell "scope to nothing" from
/// "unrestricted". Never adds permission_request: that is Claude-only and the
/// engine appends it with [`append_permission_request`].
pub fn names_from_allowed_tools(allowed_tools: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in allowed_tools {
        let Some(name) = t.strip_prefix(RECUR_MCP_PREFIX) else {
            continue;
        };
        if name.is_empty() || out.iter().any(|n| n == name) {
            continue;
        }
        out.push(name.to_string());
    }
    out
}

/// Adds permission_request if missing. Claude Code resolves
/// `--permission-prompt-tool` against tools/list at startup, so Claude
/// bindings need it; Cursor bindings must not have it.
pub fn append_permission_request(mut names: Vec<String>) -> Vec<String> {
    if !names.iter().any(|n| n == PERMISSION_REQUEST) {
        names.push(PERMISSION_REQUEST.to_string());
    }
    names
}

/// 16 random bytes as 32 hex characters: the per-turn URL token.
pub(crate) fn new_token() -> String {
    let mut b = [0u8; 16];
    if getrandom::fill(&mut b).is_err() {
        return "0".repeat(32);
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}
