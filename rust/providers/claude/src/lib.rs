//! Claude Code provider (headless `claude -p` stream-json).
//!
//! Port of Go `engine/claude.go`, `engine/claude_flags.go`, the Claude flag
//! half of `engine/permission.go`, the Claude half of
//! `engine/attachments.go`, `app/claude_status.go` and the Claude branch of
//! `app/model_catalog.go`. The allowlist translation is documented on
//! [`args`].

pub mod args;
pub mod flags;
#[cfg(test)]
mod go_golden;
pub mod input;
mod proc;
mod runner;
pub mod status;
pub mod stream;
#[cfg(feature = "testing")]
pub mod testing;

use std::time::Duration;

use recur_agent::display::canonical_claude_tool;
use recur_agent::{
    Availability, Flags, HistoryMode, McpTransports, ModelChoice, PermissionRoute, Provider,
    ProviderCaps, ProviderError, ProviderStatus, SteerSupport, ToolDisplay, ToolTransport,
    TurnContext, TurnRequest, TurnResult,
};
use recur_domain::{PROVIDER_CLAUDE_CODE, Settings, provider_label};

/// How the provider launches `claude`.
#[derive(Debug, Clone)]
pub struct ClaudeConfig {
    /// The executable; empty = `claude` on `PATH`.
    pub bin: String,
    /// How long a result is held while background tasks still run (a dev
    /// server never finishes). Matches claude's own print-mode ceiling.
    pub background_hold_ceiling: Duration,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            bin: String::new(),
            background_hold_ceiling: Duration::from_secs(10 * 60),
        }
    }
}

/// The production Claude Code provider.
pub struct ClaudeProvider {
    cfg: ClaudeConfig,
}

const CAPS: ProviderCaps = ProviderCaps {
    permission: PermissionRoute::McpPromptTool,
    steer: SteerSupport::Live,
    history: HistoryMode::Native,
    tool_transport: ToolTransport::Mcp,
    native_fork: true,
    images: true,
    can_disable_tools: true,
    max_turns: true,
    partials: true,
    user_mcp: McpTransports {
        http: true,
        stdio: true,
    },
};

impl ClaudeProvider {
    pub fn new(cfg: ClaudeConfig) -> Self {
        Self { cfg }
    }

    fn bin(&self) -> &str {
        if self.cfg.bin.is_empty() {
            "claude"
        } else {
            &self.cfg.bin
        }
    }
}

#[async_trait::async_trait]
impl Provider for ClaudeProvider {
    fn id(&self) -> &str {
        PROVIDER_CLAUDE_CODE
    }

    fn label(&self) -> &str {
        provider_label(PROVIDER_CLAUDE_CODE)
    }

    fn caps(&self) -> &ProviderCaps {
        &CAPS
    }

    fn availability(&self, settings: &Settings) -> Availability {
        if settings.provider_enabled(PROVIDER_CLAUDE_CODE) {
            Availability::Ready
        } else {
            Availability::Unavailable("Claude Code is turned off in Settings → Providers".into())
        }
    }

    async fn status(&self, _settings: &Settings) -> ProviderStatus {
        status::probe(self.bin()).await
    }

    async fn models(&self, _settings: &Settings) -> Result<Vec<ModelChoice>, ProviderError> {
        Ok(flags::catalog())
    }

    fn cached_models(&self, _settings: &Settings) -> Vec<ModelChoice> {
        flags::catalog()
    }

    fn resolve_model(&self, model: &str, flags: &Flags) -> String {
        flags::resolve_model(model, flags)
    }

    fn context_window(&self, model: &str, flags: &Flags) -> i64 {
        flags::context_window(model, flags)
    }

    fn model_label(&self, model: &str) -> String {
        flags::model_label(model)
    }

    fn oneshot_flags(&self, _model: &str) -> Flags {
        Flags::from([("effort".to_owned(), "low".to_owned())])
    }

    async fn run_turn(
        &self,
        req: TurnRequest,
        cx: TurnContext,
    ) -> Result<TurnResult, ProviderError> {
        runner::run(&self.cfg.bin, self.cfg.background_hold_ceiling, req, cx).await
    }

    /// Claude and Codex both persist UUIDs; only Cursor's `agent-` ids are
    /// foreign. A Claude↔Codex switch is caught upstream by the dock-switch
    /// rule (old provider ≠ new provider drops the handle).
    fn resume_handle_fits(&self, handle: &str) -> bool {
        let h = handle.trim();
        h.is_empty() || !h.starts_with("agent-")
    }

    /// Claude defined most of the common vocabulary; only `Agent` (renamed
    /// from `Task` in v2.1.63) maps back to the subagent card's `Task`.
    fn display_tool(&self, name: &str, input: &str) -> ToolDisplay {
        ToolDisplay {
            name: canonical_claude_tool(name).to_owned(),
            input: input.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_and_identity() {
        let p = ClaudeProvider::new(ClaudeConfig::default());
        assert_eq!((p.id(), p.label()), ("claude-code", "Claude Code"));
        assert_eq!(p.availability(&Settings::default()), Availability::Ready);
        let off = Settings {
            disabled_providers: vec!["claude-code".into()],
            ..Default::default()
        };
        assert_eq!(
            p.availability(&off),
            Availability::Unavailable("Claude Code is turned off in Settings → Providers".into())
        );
        assert_eq!(p.caps().permission, PermissionRoute::McpPromptTool);
        assert_eq!(p.oneshot_flags("haiku")["effort"], "low");
    }

    #[test]
    fn resume_handles() {
        let p = ClaudeProvider::new(ClaudeConfig::default());
        assert!(p.resume_handle_fits(""));
        assert!(p.resume_handle_fits("7f0c2c0e-uuid"));
        assert!(!p.resume_handle_fits("agent-123"));
    }

    #[test]
    fn display_maps_agent_to_task() {
        let p = ClaudeProvider::new(ClaudeConfig::default());
        let d = p.display_tool("Agent", r#"{"x":1}"#);
        assert_eq!((d.name.as_str(), d.input.as_str()), ("Task", r#"{"x":1}"#));
        assert_eq!(p.display_tool("Bash", "{}").name, "Bash");
    }
}
