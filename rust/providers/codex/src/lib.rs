//! OpenAI Codex provider (`codex app-server` JSON-RPC over stdio).
//!
//! Port of Go `engine/codex.go`, `internal/codex/*`,
//! `app/codex_service.go`, the Codex branch of `app/model_catalog.go` and
//! of `engine/toolview.go`. The allowlist translation is documented on
//! [`runner`]. App-server is spawned per turn (the MCP URL is per agent), so
//! there is no warm pool; thread history persists in Recur's `CODEX_HOME`.

pub mod approvals;
pub mod catalog;
pub mod client;
pub mod display;
pub mod items;
pub mod locate;
pub mod mcp;
mod proc;
pub mod rpc;
pub mod runner;
pub mod server;
pub mod status;
pub mod types;

use std::path::PathBuf;

use parking_lot::Mutex;
use recur_agent::{
    Availability, CredentialDraft, ErrorKind, Flags, HistoryMode, McpTransports, ModelChoice,
    PermissionRoute, Provider, ProviderCaps, ProviderError, ProviderStatus, SteerSupport,
    ToolDisplay, ToolTransport, TurnContext, TurnRequest, TurnResult,
};
use recur_domain::{PROVIDER_CODEX, Settings, provider_label};

/// How the provider finds its binary, home and settings.
#[derive(Debug, Clone, Default)]
pub struct CodexConfig {
    /// Recur's data dir; `CODEX_HOME` is `<data_dir>/codex-home` and probes
    /// run there. Empty = the system temp dir (Go's fallback).
    pub data_dir: PathBuf,
    /// An explicit `CODEX_HOME` (tests); `None` = under `data_dir`.
    pub home: Option<PathBuf>,
    /// Settings at construction; [`Provider::settings_changed`] keeps them
    /// current (binary path, key, ChatGPT auth, network access).
    pub settings: Settings,
    /// Environment variables spawned app-servers must not inherit (tracker
    /// secrets), for probes and turns; turns add `TurnRequest::env_scrub`.
    pub env_scrub: Vec<String>,
    /// The machine login seeded for ChatGPT auth; `None` =
    /// `~/.codex/auth.json`.
    pub seed_auth_from: Option<PathBuf>,
}

/// The production Codex provider.
pub struct CodexProvider {
    cfg: CodexConfig,
    settings: Mutex<Settings>,
    /// The last good `model/list` (offline fallback for slot remaps).
    last_models: Mutex<Option<Vec<types::Model>>>,
}

const CAPS: ProviderCaps = ProviderCaps {
    permission: PermissionRoute::Callback,
    steer: SteerSupport::Live,
    history: HistoryMode::Native,
    tool_transport: ToolTransport::Mcp,
    native_fork: false,
    images: false,
    can_disable_tools: true,
    max_turns: false,
    partials: true,
    user_mcp: McpTransports {
        http: true,
        stdio: true,
    },
};

impl CodexProvider {
    pub fn new(cfg: CodexConfig) -> Self {
        Self {
            settings: Mutex::new(cfg.settings.clone()),
            cfg,
            last_models: Mutex::default(),
        }
    }

    fn data_dir(&self) -> PathBuf {
        if self.cfg.data_dir.as_os_str().is_empty() {
            std::env::temp_dir()
        } else {
            self.cfg.data_dir.clone()
        }
    }

    fn probe_env(&self) -> status::ProbeEnv {
        let data_dir = self.data_dir();
        status::ProbeEnv {
            home: self
                .cfg
                .home
                .clone()
                .unwrap_or_else(|| locate::home_dir(&data_dir)),
            data_dir,
            env_scrub: self.cfg.env_scrub.clone(),
            seed_auth_from: self
                .cfg
                .seed_auth_from
                .clone()
                .or_else(server::default_seed_auth_path),
        }
    }

    /// Seeds the cache a successful live list fills.
    #[cfg(feature = "testing")]
    pub fn remember_models(&self, models: Vec<types::Model>) {
        self.remember(Some(models));
    }

    fn remember(&self, models: Option<Vec<types::Model>>) {
        if models.is_some() {
            *self.last_models.lock() = models;
        }
    }
}

#[async_trait::async_trait]
impl Provider for CodexProvider {
    fn id(&self) -> &str {
        PROVIDER_CODEX
    }

    fn label(&self) -> &str {
        provider_label(PROVIDER_CODEX)
    }

    fn caps(&self) -> &ProviderCaps {
        &CAPS
    }

    fn availability(&self, settings: &Settings) -> Availability {
        if !settings.provider_enabled(PROVIDER_CODEX) {
            return Availability::Unavailable("Codex is turned off in Settings → Providers".into());
        }
        if !settings.codex_configured() {
            return Availability::Unavailable(
                "a role is set to a Codex model but Codex is not configured (Settings → Providers → Codex)"
                    .into(),
            );
        }
        Availability::Ready
    }

    async fn status(&self, settings: &Settings) -> ProviderStatus {
        let (st, models) = status::status(settings, &self.probe_env()).await;
        self.remember(models);
        st
    }

    async fn test_credentials(
        &self,
        settings: &Settings,
        draft: &CredentialDraft,
    ) -> ProviderStatus {
        status::test_key(
            settings,
            &self.probe_env(),
            &draft.api_key,
            draft.chatgpt_auth,
        )
        .await
    }

    /// The live list. Turned off or unconfigured lists nothing; a failed
    /// probe is an error (the picker omits the group and keeps the cache).
    async fn models(&self, settings: &Settings) -> Result<Vec<ModelChoice>, ProviderError> {
        match status::list_models(settings, &self.probe_env()).await {
            Ok(None) => Ok(Vec::new()),
            Ok(Some(models)) => {
                let rows = catalog::catalog_models(&models);
                self.remember(Some(models));
                Ok(rows)
            }
            Err(e) => Err(ProviderError::new(ErrorKind::Unavailable, e)),
        }
    }

    fn cached_models(&self, _settings: &Settings) -> Vec<ModelChoice> {
        self.last_models
            .lock()
            .as_deref()
            .map(catalog::catalog_models)
            .unwrap_or_default()
    }

    fn resolve_model(&self, model: &str, _flags: &Flags) -> String {
        model.to_owned()
    }

    /// Codex reports its window on every usage frame
    /// (`modelContextWindow`); nothing is known up front.
    fn context_window(&self, _model: &str, _flags: &Flags) -> i64 {
        0
    }

    fn model_label(&self, model: &str) -> String {
        catalog::friendly_model_label("", model)
    }

    fn oneshot_flags(&self, _model: &str) -> Flags {
        Flags::from([("effort".to_owned(), "low".to_owned())])
    }

    async fn run_turn(
        &self,
        req: TurnRequest,
        cx: TurnContext,
    ) -> Result<TurnResult, ProviderError> {
        let set = self.settings.lock().clone();
        let bin = locate::locate(&set.codex_bin_path)
            .map_err(|e| ProviderError::new(ErrorKind::Unavailable, e))?;
        let env = self.probe_env();
        let rc = runner::RunConfig {
            bin,
            api_key: if set.codex_chat_gpt_auth {
                String::new()
            } else {
                set.codex_api_key
            },
            chatgpt_auth: set.codex_chat_gpt_auth,
            home: env.home,
            network_access: set.codex_network_access,
            env_scrub: env.env_scrub,
            seed_auth_from: env.seed_auth_from,
        };
        runner::run(rc, req, cx).await
    }

    /// Codex and Claude both persist UUIDs; only Cursor's `agent-` ids are
    /// foreign.
    fn resume_handle_fits(&self, handle: &str) -> bool {
        let h = handle.trim();
        h.is_empty() || !h.starts_with("agent-")
    }

    fn display_tool(&self, name: &str, input: &str) -> ToolDisplay {
        ToolDisplay {
            name: display::canonical_tool(name).to_owned(),
            input: display::tool_input(name, input),
        }
    }

    async fn settings_changed(&self, _old: &Settings, new: &Settings) {
        *self.settings.lock() = new.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_wording() {
        let p = CodexProvider::new(CodexConfig::default());
        assert_eq!((p.id(), p.label()), ("codex", "Codex"));
        let off = Settings {
            disabled_providers: vec!["codex".into()],
            ..Default::default()
        };
        assert_eq!(
            p.availability(&off),
            Availability::Unavailable("Codex is turned off in Settings → Providers".into())
        );
        assert_eq!(
            p.availability(&Settings::default()),
            Availability::Unavailable(
                "a role is set to a Codex model but Codex is not configured (Settings → Providers → Codex)".into()
            )
        );
        let chat = Settings {
            codex_chat_gpt_auth: true,
            ..Default::default()
        };
        assert_eq!(p.availability(&chat), Availability::Ready);
        assert_eq!(p.caps().permission, PermissionRoute::Callback);
    }

    #[test]
    fn display_and_handles() {
        let p = CodexProvider::new(CodexConfig::default());
        let d = p.display_tool("shell", r#"{"command":"ls"}"#);
        assert_eq!(
            (d.name.as_str(), d.input.as_str()),
            ("Bash", r#"{"command":"ls"}"#)
        );
        assert!(p.resume_handle_fits("019a-uuid"));
        assert!(!p.resume_handle_fits("agent-x"));
        assert_eq!(p.model_label("gpt-5.6-sol"), "GPT-5.6 Sol");
        assert_eq!(p.context_window("gpt-5.6-sol", &Flags::new()), 0);
        assert!(p.cached_models(&Settings::default()).is_empty());
    }
}
