//! Claude Code provider (headless `claude -p` stream-json).
//!
//! The allowlist translation is documented on [`args`].

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
    /// The catalog from the last probe that reported models, so
    /// account-only models do not flicker out when a probe fails.
    probed: parking_lot::Mutex<Option<Vec<ModelChoice>>>,
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
        Self {
            cfg,
            probed: parking_lot::Mutex::default(),
        }
    }

    /// The last good probed catalog, probing only until one succeeds
    /// (schedule validation runs on every save).
    pub async fn known_models(&self, settings: &Settings) -> Vec<ModelChoice> {
        if let Some(last) = self.probed.lock().clone() {
            return last;
        }
        self.models(settings).await.unwrap_or_default()
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

    /// The pinned list plus any extra models the local CLI reports for the
    /// account.
    async fn models(&self, settings: &Settings) -> Result<Vec<ModelChoice>, ProviderError> {
        let probed = status::probe_models(self.bin()).await;
        if probed.is_empty() {
            return Ok(self.cached_models(settings));
        }
        let models = flags::with_probed(&probed);
        *self.probed.lock() = Some(models.clone());
        Ok(models)
    }

    fn cached_models(&self, _settings: &Settings) -> Vec<ModelChoice> {
        self.probed.lock().clone().unwrap_or_else(flags::catalog)
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

    fn defers_prompt(&self) -> bool {
        true
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

    fn script(dir: &std::path::Path, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("claude");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    /// A failed probe keeps the last good list; `known_models` spawns
    /// claude only until a probe succeeds.
    #[tokio::test]
    async fn probed_models_survive_failures_and_cache() {
        let dir = tempfile::tempdir().unwrap();
        let ran = dir.path().join("ran");
        let reply = r#"{"type":"control_response","response":{"request_id":"models","response":{"models":[{"value":"claude-mythos-5-1","displayName":"Mythos 5.1"}]}}}"#;
        let bin = script(
            dir.path(),
            &format!(
                "touch '{}'\n[ -f '{}.off' ] || echo '{reply}'",
                ran.display(),
                ran.display()
            ),
        );
        let p = ClaudeProvider::new(ClaudeConfig {
            bin,
            ..Default::default()
        });
        let set = Settings::default();
        let mythos = |m: &[ModelChoice]| m.iter().any(|m| m.label == "Mythos 5.1");

        assert!(mythos(&p.known_models(&set).await));
        assert!(ran.exists());

        std::fs::remove_file(&ran).unwrap();
        assert!(mythos(&p.known_models(&set).await));
        assert!(!ran.exists(), "spawned despite a cached probe");

        std::fs::write(ran.with_extension("off"), "").unwrap();
        assert!(mythos(&p.models(&set).await.unwrap()));
        assert!(ran.exists());
        assert!(mythos(&p.cached_models(&set)));
    }

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
