//! ChatGPT account provider: Codex-compatible browser and device-code OAuth,
//! then Recur's own coding harness against
//! `chatgpt.com/backend-api/codex/responses`. No Codex install, CLI login or
//! API key. See `docs/chatgpt-provider.md`.

pub mod auth;
pub mod login;
mod oidc;

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;
use recur_agent::{
    Availability, CredentialDraft, Flags, ModelChoice, Provider, ProviderCaps, ProviderError,
    ProviderStatus, ToolDisplay, TurnContext, TurnRequest, TurnResult,
};
use recur_domain::{CompatibleProvider as Config, PROVIDER_CHATGPT, Settings, provider_label};
use recur_provider_compatible::{
    Authorize, Client, HARNESS_CAPS, compatible_catalog_models, run_harness,
};
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};

pub use auth::{Account, Auth, Model, Status};
pub use login::{LOGIN_BROWSER, LOGIN_HEADLESS, Login};

/// The Settings card: account metadata only, never OAuth tokens.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChatGPTStatus {
    #[serde(flatten)]
    pub status: Status,
    pub model_count: i64,
    pub error: String,
}

/// The `chatgpt` provider. Its [`Auth`] is shared with Settings, so sign-in,
/// refresh and every turn serialize on one credential store.
pub struct ChatGPTProvider {
    auth: Arc<Auth>,
    settings: RwLock<Settings>,
    http: reqwest::Client,
    data_dir: PathBuf,
}

impl ChatGPTProvider {
    /// Credentials live in `<data_dir>/chatgpt-auth.json`.
    pub fn new(data_dir: PathBuf, settings: Settings) -> Self {
        Self {
            auth: Arc::new(Auth::new(data_dir.clone())),
            settings: RwLock::new(settings),
            http: recur_http::default_client(),
            data_dir,
        }
    }

    /// Uses `auth` instead of the data dir's store (tests).
    pub fn with_auth(mut self, auth: Arc<Auth>) -> Self {
        self.auth = auth;
        self
    }

    /// Uses `http` for turns (tests, shared pools).
    pub fn with_http(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    pub fn auth(&self) -> &Arc<Auth> {
        &self.auth
    }

    /// The account, with its token checked (and refreshed) so an expired
    /// session shows as signed out.
    pub async fn account_status(&self, settings: &Settings) -> ChatGPTStatus {
        let mut status = match self.auth.status().await {
            Ok(s) => s,
            Err(error) => {
                return ChatGPTStatus {
                    error,
                    ..Default::default()
                };
            }
        };
        if status.authenticated
            && let Err(error) = self.auth.access_token(&status.account_id).await
        {
            status.authenticated = false;
            return ChatGPTStatus {
                status,
                error,
                ..Default::default()
            };
        }
        let model_count =
            if status.authenticated && status.account_id == settings.chat_gpt_account_id {
                settings.chat_gpt_models.len() as i64
            } else {
                0
            };
        ChatGPTStatus {
            status,
            model_count,
            error: String::new(),
        }
    }

    /// The endpoint and account a turn binds to.
    fn snapshot(&self) -> (Config, String) {
        let s = self.settings.read();
        (s.chatgpt_provider(), s.chat_gpt_account_id.clone())
    }
}

/// Authorizes each request as one account, so a running turn cannot pick up
/// a different identity after the user switches.
struct Bound {
    auth: Arc<Auth>,
    account: String,
}

#[async_trait::async_trait]
impl Authorize for Bound {
    async fn authorize(&self, headers: &mut HeaderMap) -> Result<(), String> {
        self.auth.authorize(headers, &self.account).await
    }
}

#[async_trait::async_trait]
impl Provider for ChatGPTProvider {
    fn id(&self) -> &str {
        PROVIDER_CHATGPT
    }

    fn label(&self) -> &str {
        provider_label(PROVIDER_CHATGPT)
    }

    fn caps(&self) -> &ProviderCaps {
        &HARNESS_CAPS
    }

    fn availability(&self, settings: &Settings) -> Availability {
        if !settings.provider_enabled(PROVIDER_CHATGPT) || !settings.chatgpt_configured() {
            return Availability::Unavailable(
                "connect ChatGPT in Settings → Providers → OpenAI".into(),
            );
        }
        Availability::Ready
    }

    async fn status(&self, settings: &Settings) -> ProviderStatus {
        let st = self.account_status(settings).await;
        ProviderStatus {
            authenticated: st.status.authenticated,
            email: st.status.email.clone(),
            account: st.status.email,
            name: provider_label(PROVIDER_CHATGPT).into(),
            model_count: st.model_count,
            error: st.error,
            ..Default::default()
        }
    }

    async fn test_credentials(
        &self,
        settings: &Settings,
        _draft: &CredentialDraft,
    ) -> ProviderStatus {
        self.status(settings).await
    }

    async fn models(&self, settings: &Settings) -> Result<Vec<ModelChoice>, ProviderError> {
        Ok(self.cached_models(settings))
    }

    fn cached_models(&self, settings: &Settings) -> Vec<ModelChoice> {
        compatible_catalog_models(&settings.chatgpt_provider())
    }

    fn resolve_model(&self, model: &str, _flags: &Flags) -> String {
        model.to_owned()
    }

    fn context_window(&self, model: &str, _flags: &Flags) -> i64 {
        self.snapshot()
            .0
            .model_by_id(model)
            .map_or(0, |m| m.context_window)
    }

    fn model_label(&self, model: &str) -> String {
        match self.snapshot().0.model_by_id(model) {
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
        let (config, account) = self.snapshot();
        let client = Client::new(&config)
            .with_http(self.http.clone())
            .with_chatgpt(Arc::new(Bound {
                auth: Arc::clone(&self.auth),
                account,
            }));
        run_harness(&config, &client, Some(self.data_dir.clone()), req, cx).await
    }

    /// History lives in the Recur store, so any stored handle is foreign.
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
        *self.settings.write() = new.clone();
    }

    /// Stops any sign-in and refuses tokens to late turns.
    async fn shutdown(&self) {
        self.auth.close().await;
    }
}

#[cfg(test)]
mod tests;
