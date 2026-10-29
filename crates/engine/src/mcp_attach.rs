//! Settings → MCP servers onto a turn. A server whose id or preset an issue
//! tracker claims (Linear) is built by that tracker (`IssueTracker::agent_mcp`:
//! URL by mode, key, team hint); the rest are custom HTTP or stdio servers.

use std::sync::Arc;

use recur_agent::{McpAttach, TurnRequest};
use recur_agent_tools::names::mcp as tool;
use recur_domain::{
    AgentRole, CHAT_MODE_AGENT, CHAT_MODE_ASK, CHAT_MODE_LOOP, MCP_TRANSPORT_STDIO, McpServer,
    Project, Settings, effective_skill_enabled, mcp_catalog, mcp_global_disabled,
};
use recur_issues::IssueTracker;

use crate::Engine;
use crate::composer::skills::join_append_prompt;

/// Adds the Settings → MCP servers that are on for this project onto `req`
/// (`user_mcp` plus an `mcp__<id>__*` allowlist entry each). `mode` picks a
/// tracker's readonly vs write endpoint.
pub fn attach_user_mcp(
    req: &mut TurnRequest,
    mode: &str,
    set: &Settings,
    project: Option<&Project>,
    trackers: &[Arc<dyn IssueTracker>],
) {
    let (proj_off, proj_on) = match project {
        Some(p) => (p.disabled_mcp.as_slice(), p.enabled_mcp.as_slice()),
        None => (&[][..], &[][..]),
    };
    let global_off = mcp_global_disabled(&set.mcp_servers);
    let mut hints: Vec<String> = Vec::new();
    let mut extra: Vec<String> = Vec::new();
    for s in mcp_catalog(&set.mcp_servers) {
        if !effective_skill_enabled(&s.id, &global_off, proj_off, proj_on) {
            continue;
        }
        let Some((att, hint)) = build_mcp_attach(&s, mode, set, trackers) else {
            continue;
        };
        req.user_mcp.push(att);
        extra.push(format!("mcp__{}__*", s.id));
        if let Some(h) = hint
            && !hints.contains(&h)
        {
            hints.push(h);
        }
    }
    if !extra.is_empty() {
        req.allowed_tools.get_or_insert_with(Vec::new).extend(extra);
    }
    for h in hints {
        req.append_system_prompt = join_append_prompt(&req.append_system_prompt, &h);
    }
}

/// One catalog server as a turn attachment, plus the tracker's system hint.
/// `None` when the server cannot be wired (no key, no command, no URL).
fn build_mcp_attach(
    s: &McpServer,
    mode: &str,
    set: &Settings,
    trackers: &[Arc<dyn IssueTracker>],
) -> Option<(McpAttach, Option<String>)> {
    if let Some(t) = trackers
        .iter()
        .find(|t| t.mcp_preset().is_some_and(|p| p == s.id || p == s.preset))
    {
        let ep = t.agent_mcp(set, mode)?;
        return Some((
            McpAttach {
                name: s.id.clone(),
                url: ep.url,
                headers: ep.headers,
                ..McpAttach::default()
            },
            t.agent_hint(set),
        ));
    }
    if s.transport == MCP_TRANSPORT_STDIO {
        if s.command.is_empty() {
            return None;
        }
        return Some((
            McpAttach {
                name: s.id.clone(),
                command: s.command.clone(),
                args: s.args.clone(),
                env: s.env.clone(),
                ..McpAttach::default()
            },
            None,
        ));
    }
    if s.url.is_empty() {
        return None;
    }
    Some((
        McpAttach {
            name: s.id.clone(),
            url: s.url.clone(),
            headers: s.headers.clone(),
            ..McpAttach::default()
        },
        None,
    ))
}

/// The allowlist holds no file-edit tool (bare Bash alone is read-only).
/// Empty means unrestricted, not read-only.
pub fn is_read_only_role(req: &TurnRequest) -> bool {
    match &req.allowed_tools {
        Some(tools) if !tools.is_empty() => !tools
            .iter()
            .any(|t| matches!(t.as_str(), "Edit" | "Write" | "NotebookEdit")),
        _ => false,
    }
}

/// Readonly vs write tracker endpoints: the Maestro conductor (it can
/// ask_user) is loop, read-only turns ask, everything else agent.
pub fn tracker_mode_for(req: &TurnRequest) -> &'static str {
    let has_ask = req
        .allowed_tools
        .as_ref()
        .is_some_and(|t| t.iter().any(|n| n == tool::ASK_USER));
    if req.role == AgentRole::Maestro && has_ask {
        return CHAT_MODE_LOOP;
    }
    if is_read_only_role(req) {
        return CHAT_MODE_ASK;
    }
    CHAT_MODE_AGENT
}

impl Engine {
    /// Settings → MCP onto a workspace turn. Workspace-less Ask Recur stays
    /// on its briefing tools: custom MCP is not filtered to readonly there.
    pub fn attach_workspace_user_mcp(&self, req: &mut TurnRequest, workspace_id: &str, mode: &str) {
        if workspace_id.is_empty() {
            return;
        }
        let project = self
            .store
            .workspace(workspace_id)
            .and_then(|ws| self.store.project(&ws.project_id));
        attach_user_mcp(
            req,
            mode,
            &self.settings(),
            project.as_ref(),
            &self.trackers.all(),
        );
    }
}

#[cfg(test)]
mod tests;
