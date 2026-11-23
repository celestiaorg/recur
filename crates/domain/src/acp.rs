//! Agents Recur drives over the Agent Client Protocol (ACP v1, JSON-RPC over
//! the agent's stdio). One generic provider (`recur-provider-acp`) serves each
//! entry; adding an ACP agent is one row here.

/// One ACP agent Recur knows how to launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcpAgent {
    /// The persisted provider slug (model refs are `<id>/<model>`).
    pub id: &'static str,
    /// The picker group and Settings card label.
    pub label: &'static str,
    /// The binary looked up on `PATH`.
    pub bin: &'static str,
    /// Arguments that start the ACP stdio server.
    pub args: &'static [&'static str],
    /// Overrides binary resolution (development and tests).
    pub env_bin: &'static str,
    /// How to install the CLI, shown when the binary is missing.
    pub install_hint: &'static str,
    /// How to sign in, shown when the agent answers "Authentication required".
    pub login_hint: &'static str,
    /// The agent's own tools Recur replaces, each with the Recur tool to use
    /// instead (asking the human, skills, subagents, todos, background work,
    /// schedules), switched off through [`AcpAgent::tool_block`].
    pub blocked_tools: &'static [(&'static str, &'static str)],
    pub tool_block: AcpToolBlock,
}

/// How an agent's [`AcpAgent::blocked_tools`] are switched off. Nothing is
/// written into the worktree or the user's own agent config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpToolBlock {
    /// Inline `OPENCODE_CONFIG_CONTENT` permission denies, which remove the
    /// tools from the model.
    OpenCodeConfig,
    /// A Recur-owned `--gemini-dir` whose `PreToolUse` hook denies the calls
    /// with the Recur tool to use. The tools stay declared to the model.
    GeminiDirHook,
}

pub const PROVIDER_ANTIGRAVITY: &str = "antigravity";
pub const PROVIDER_OPENCODE: &str = "opencode";

pub const ACP_AGENTS: &[AcpAgent] = &[
    AcpAgent {
        id: PROVIDER_ANTIGRAVITY,
        label: "Antigravity",
        bin: "agy",
        args: &["acp"],
        env_bin: "RECUR_AGY",
        install_hint: "Install the Antigravity CLI (agy) from https://antigravity.google",
        login_hint: "Run `agy` in a terminal and sign in with Google",
        blocked_tools: &[
            ("ask_question", "mcp__recur__ask_user"),
            ("invoke_subagent", "mcp__recur__create_workspace"),
            ("define_subagent", "mcp__recur__create_workspace"),
            ("manage_subagents", "mcp__recur__create_workspace"),
            ("swarm", "mcp__recur__create_workspace"),
            ("run_workflow", "mcp__recur__create_workspace"),
            ("send_message", "mcp__recur__message_child"),
            ("manage_task", "mcp__recur__run_background"),
            ("schedule", "mcp__recur__schedule_wakeup"),
        ],
        tool_block: AcpToolBlock::GeminiDirHook,
    },
    AcpAgent {
        id: PROVIDER_OPENCODE,
        label: "OpenCode",
        bin: "opencode",
        args: &["acp"],
        env_bin: "RECUR_OPENCODE",
        install_hint: "Install OpenCode from https://opencode.ai",
        login_hint: "Run `opencode auth login` in a terminal",
        blocked_tools: &[
            ("question", "mcp__recur__ask_user"),
            ("skill", "mcp__recur__activate_skill"),
            ("task", "mcp__recur__create_workspace"),
            ("todowrite", "mcp__recur__update_todos"),
            ("todoread", "mcp__recur__update_todos"),
        ],
        tool_block: AcpToolBlock::OpenCodeConfig,
    },
];

/// The ACP agent registered under `id`.
pub fn acp_agent(id: &str) -> Option<&'static AcpAgent> {
    ACP_AGENTS.iter().find(|a| a.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup() {
        assert_eq!(acp_agent("antigravity").map(|a| a.bin), Some("agy"));
        assert!(acp_agent("codex").is_none());
        let mut ids: Vec<_> = ACP_AGENTS.iter().map(|a| a.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), ACP_AGENTS.len());
    }
}
