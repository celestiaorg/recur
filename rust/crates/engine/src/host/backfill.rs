//! MCP argument backfill (Go `mcp_tool_backfill.go`): providers that store
//! an empty tool_use (Codex mcpToolCall items often record `{}`) get the
//! real `tools/call` arguments patched in, and the live row is refreshed
//! with `session.tool_input`.

use std::time::Duration;

use crate::Engine;
use crate::bus::{EngineEvent, SessionToolInput};

/// Retries the patch after the provider's own item mirror lands (often
/// after the MCP handler returned).
const MCP_ARGS_BACKFILL_DELAY: Duration = Duration::from_millis(150);

/// Arguments worth writing: not empty, `{}` or `null`.
pub fn mcp_args_worth_backfill(args: &str) -> bool {
    let cur = args.trim();
    !cur.is_empty() && cur != "{}" && cur != "null"
}

impl Engine {
    /// Host `note_tool_args`: patch now and once more after a short delay.
    /// Never blocks the tool call.
    pub fn note_mcp_tool_args(&self, workspace_id: &str, agent_code: &str, tool: &str, args: &str) {
        if tool.trim().is_empty() || !mcp_args_worth_backfill(args) {
            return;
        }
        self.backfill_mcp_tool_input(workspace_id, agent_code, tool, args);
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let eng = self.arc();
        let (ws, code, tool, args) = (
            workspace_id.to_owned(),
            agent_code.to_owned(),
            tool.to_owned(),
            args.to_owned(),
        );
        rt.spawn(async move {
            tokio::time::sleep(MCP_ARGS_BACKFILL_DELAY).await;
            eng.backfill_mcp_tool_input(&ws, &code, &tool, &args);
        });
    }

    /// Patches the newest matching empty tool_use of the agent's session and
    /// publishes `session.tool_input` for the live row.
    pub fn backfill_mcp_tool_input(
        &self,
        workspace_id: &str,
        agent_code: &str,
        tool: &str,
        args: &str,
    ) {
        if !mcp_args_worth_backfill(args) {
            return;
        }
        let bare = tool.trim();
        if bare.is_empty() {
            return;
        }
        let Some(a) = self.store.agent(workspace_id, agent_code) else {
            return;
        };
        if a.session_id.is_empty() {
            return;
        }
        let Some(tu) = self
            .store
            .latest_tool_use_by_name_suffix(&a.session_id, &format!("__{bare}"))
            .or_else(|| {
                self.store
                    .latest_tool_use_by_name_suffix(&a.session_id, bare)
            })
        else {
            return;
        };
        if mcp_args_worth_backfill(&tu.input) {
            return;
        }
        let _ = self.store.patch_tool_use_input(&tu.id, args);
        let id = if tu.provider_tool_use_id.is_empty() {
            tu.id.clone()
        } else {
            tu.provider_tool_use_id.clone()
        };
        let full = if tu.name.is_empty() {
            format!("mcp__recur__{bare}")
        } else {
            tu.name.clone()
        };
        self.bus
            .publish(EngineEvent::SessionToolInput(SessionToolInput {
                code: agent_code.to_owned(),
                id,
                input: args.to_owned(),
                tool_input: recur_agent::display::summarize_tool_input(&full, args),
                workspace_id: workspace_id.to_owned(),
            }));
    }
}
