//! OpenAI-compatible `/v1/responses` provider: Recur owns the agent loop, the
//! coding tools and the host tools. See `docs/openai-compatible-provider.md`.
//!
//! The engine passes the turn's tools as `TurnContext::tools` (build one with
//! [`HarnessTools::new`]) and the Recur transcript as `TurnContext::history`.

pub mod catalog;
mod chatgpt;
pub mod client;
pub mod flags;
mod gojson;
mod harness;
pub mod history;
pub mod prompt;
pub mod sse;
pub mod status;
pub mod tools;
pub mod types;

use std::path::PathBuf;

use parking_lot::RwLock;
use recur_agent::{
    Availability, CredentialDraft, Flags, HistoryMode, McpTransports, ModelChoice, PermissionRoute,
    Provider, ProviderCaps, ProviderError, ProviderStatus, SteerSupport, ToolDisplay,
    ToolTransport, TurnContext, TurnRequest, TurnResult,
};
use recur_domain::{CompatibleProvider as Config, Settings};

pub use catalog::{compatible_catalog_models, compatible_flag_dims};
pub use client::{Authorize, Client, ClientError};
pub use harness::{HARNESS_PARALLEL_LIMIT, responses_fresh_input, terminal_response_error};
pub use status::{CompatibleTestResult, compatible_flag_preset, test_compatible_provider};
pub use tools::{HarnessTools, harness_agent_code, harness_tools, tool_allowed_on_turn};

/// What Recur's own harness offers: callback permissions, live steers, the
/// Recur transcript as history and in-process tools.
pub const HARNESS_CAPS: ProviderCaps = ProviderCaps {
    permission: PermissionRoute::Callback,
    steer: SteerSupport::Live,
    history: HistoryMode::App,
    tool_transport: ToolTransport::InProcess,
    native_fork: false,
    images: true,
    can_disable_tools: true,
    max_turns: true,
    partials: true,
    user_mcp: McpTransports {
        http: false,
        stdio: false,
    },
};

/// One user-configured endpoint, registered under its own slug.
///
/// The config is the one turns run with; `settings_changed` refreshes it from
/// Settings by slug. The picker label is fixed at construction: services label
/// groups with `Settings::display_label` and re-register a provider whose name
/// changed.
pub struct CompatibleProvider {
    id: String,
    label: String,
    config: RwLock<Config>,
    http: reqwest::Client,
    data_dir: Option<PathBuf>,
}

impl CompatibleProvider {
    pub fn new(config: Config) -> Self {
        Self {
            id: config.id.clone(),
            label: if config.name.trim().is_empty() {
                config.id.clone()
            } else {
                config.name.clone()
            },
            config: RwLock::new(config),
            http: recur_http::default_client(),
            data_dir: None,
        }
    }

    /// Uses `http` for every request (tests, shared pools).
    pub fn with_http(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    /// Resolves stored chat images under `dir` instead of the managed data
    /// root.
    pub fn with_data_dir(mut self, dir: PathBuf) -> Self {
        self.data_dir = Some(dir);
        self
    }

    /// The config turns run with.
    pub fn config(&self) -> Config {
        self.config.read().clone()
    }

    fn client(&self, config: &Config) -> Client {
        Client::new(config).with_http(self.http.clone())
    }

    /// The saved config when Settings has this slug, else the constructed one.
    fn settings_config(&self, settings: &Settings) -> Config {
        settings
            .compatible_by_id(&self.id)
            .cloned()
            .unwrap_or_else(|| self.config())
    }

    async fn test(&self, config: Config) -> ProviderStatus {
        let r = status::test_with(config.clone(), self.http.clone()).await;
        ProviderStatus {
            authenticated: r.ok,
            name: config.name,
            model_count: config.models.len() as i64,
            error: r.error,
            ..Default::default()
        }
    }
}

#[async_trait::async_trait]
impl Provider for CompatibleProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn caps(&self) -> &ProviderCaps {
        &HARNESS_CAPS
    }

    fn availability(&self, settings: &Settings) -> Availability {
        let Some(p) = settings.compatible_by_id(&self.id) else {
            return Availability::Unavailable(format!(
                "unknown agent provider {:?} — pick a model from Settings → Models",
                self.id
            ));
        };
        if !settings.provider_enabled(&self.id) {
            return Availability::Unavailable(format!(
                "{} is turned off in Settings → Providers",
                p.name
            ));
        }
        if !p.configured() {
            return Availability::Unavailable(format!(
                "{} is not configured (Settings → Providers)",
                p.name
            ));
        }
        Availability::Ready
    }

    async fn status(&self, settings: &Settings) -> ProviderStatus {
        self.test(self.settings_config(settings)).await
    }

    async fn test_credentials(
        &self,
        settings: &Settings,
        draft: &CredentialDraft,
    ) -> ProviderStatus {
        let config = draft
            .compatible
            .clone()
            .unwrap_or_else(|| self.settings_config(settings));
        self.test(config).await
    }

    async fn models(&self, settings: &Settings) -> Result<Vec<ModelChoice>, ProviderError> {
        Ok(compatible_catalog_models(&self.settings_config(settings)))
    }

    fn cached_models(&self, settings: &Settings) -> Vec<ModelChoice> {
        compatible_catalog_models(&self.settings_config(settings))
    }

    fn resolve_model(&self, model: &str, _flags: &Flags) -> String {
        model.to_owned()
    }

    fn context_window(&self, model: &str, _flags: &Flags) -> i64 {
        self.config
            .read()
            .model_by_id(model)
            .map_or(0, |m| m.context_window)
    }

    fn model_label(&self, model: &str) -> String {
        match self.config.read().model_by_id(model) {
            Some(m) if !m.label.is_empty() => m.label.clone(),
            _ => model.to_owned(),
        }
    }

    fn oneshot_flags(&self, _model: &str) -> Flags {
        Flags::new()
    }

    async fn run_turn(
        &self,
        req: TurnRequest,
        cx: TurnContext,
    ) -> Result<TurnResult, ProviderError> {
        let config = self.config();
        let client = self.client(&config);
        let data_root = self
            .data_dir
            .clone()
            .or_else(|| recur_store::paths::data_dir().ok());
        run_harness(&config, &client, data_root, req, cx).await
    }

    /// History lives in the Recur store, so any stored handle (a previous
    /// provider's session id) is foreign.
    fn resume_handle_fits(&self, handle: &str) -> bool {
        handle.trim().is_empty()
    }

    fn display_tool(&self, name: &str, input: &str) -> ToolDisplay {
        ToolDisplay {
            name: recur_agent::display::canonical_claude_tool(name).to_owned(),
            input: input.to_owned(),
        }
    }

    async fn settings_changed(&self, _old: &Settings, new: &Settings) {
        if let Some(p) = new.compatible_by_id(&self.id) {
            *self.config.write() = p.clone();
        }
    }
}

/// Runs one turn of Recur's coding agent against `client` (the ChatGPT
/// provider drives the same loop over its own client).
pub async fn run_harness(
    provider: &Config,
    client: &Client,
    data_root: Option<PathBuf>,
    req: TurnRequest,
    cx: TurnContext,
) -> Result<TurnResult, ProviderError> {
    harness::Harness {
        provider,
        client,
        data_root,
    }
    .run(req, cx)
    .await
}

#[cfg(test)]
mod golden_tests;
#[cfg(test)]
mod tests;
