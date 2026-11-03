//! ChatGPT OAuth credentials: the `chatgpt-auth.json` store, status, refresh
//! shared by Settings and every turn, sign-out and the model catalogue.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::login::Attempt;

/// The public Codex client also used by OpenCode. It is not a secret, and
/// does not dynamically register Recur as a ChatGPT agent.
pub const OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
pub(crate) const PROTOCOL_VERSION: &str = "0.163.0";
const AUTH_FILE: &str = "chatgpt-auth.json";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY: usize = 4 << 20;

/// Identity metadata suitable for the settings UI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Account {
    pub id: String,
    pub email: String,
}

/// The signed-in account and the saved ones. Never carries tokens.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Status {
    pub authenticated: bool,
    pub account_id: String,
    pub email: String,
    pub accounts: Vec<Account>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Credentials {
    #[serde(flatten)]
    pub account: Account,
    pub subject: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub client_id: String,
    #[serde(rename = "chatgptAccountId", skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub access_token: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub refresh_token: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id_token: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub scope: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AuthFile {
    pub active: String,
    pub accounts: Vec<Credentials>,
}

impl AuthFile {
    pub fn account(&mut self, id: &str) -> Option<&mut Credentials> {
        self.accounts.iter_mut().find(|a| a.account.id == id)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub scope: String,
}

#[derive(Default)]
pub(crate) struct State {
    pub pending: Option<Arc<Attempt>>,
    pub closed: bool,
}

/// One model the account may use.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Model {
    #[serde(rename = "slug")]
    pub id: String,
    #[serde(rename = "display_name")]
    pub name: String,
    pub visibility: String,
}

/// Shared by Settings and every turn. The state lock is held across a
/// refresh, so concurrent turns cannot race a rotating refresh token.
pub struct Auth {
    pub(crate) dir: PathBuf,
    pub(crate) http: reqwest::Client,
    pub(crate) issuer: String,
    pub(crate) authorize_url: String,
    pub(crate) token_url: String,
    pub(crate) jwks_url: String,
    pub(crate) models_url: String,
    pub(crate) device_code_url: String,
    pub(crate) device_token_url: String,
    pub(crate) verification_url: String,
    pub(crate) callback_ports: Vec<u16>,
    pub(crate) polling_margin: Duration,
    pub(crate) state: tokio::sync::Mutex<State>,
}

impl Auth {
    /// Credentials under `data_dir`. Construction makes no network requests.
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            dir: data_dir,
            http: recur_http::default_client(),
            issuer: "https://auth.openai.com".into(),
            authorize_url: "https://auth.openai.com/oauth/authorize".into(),
            token_url: "https://auth.openai.com/oauth/token".into(),
            jwks_url: "https://auth.openai.com/.well-known/jwks.json".into(),
            models_url: format!("{BASE_URL}/models?client_version={PROTOCOL_VERSION}"),
            device_code_url: "https://auth.openai.com/api/accounts/deviceauth/usercode".into(),
            device_token_url: "https://auth.openai.com/api/accounts/deviceauth/token".into(),
            verification_url: "https://auth.openai.com/codex/device".into(),
            callback_ports: vec![1455, 1457],
            polling_margin: Duration::from_secs(3),
            state: tokio::sync::Mutex::default(),
        }
    }

    pub(crate) fn read(&self) -> Result<AuthFile, String> {
        match std::fs::read(self.dir.join(AUTH_FILE)) {
            Ok(raw) => {
                serde_json::from_slice(&raw).map_err(|e| format!("read ChatGPT credentials: {e}"))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AuthFile::default()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Owner-only and atomic: a temp file in the same dir, then rename.
    pub(crate) fn write(&self, s: &AuthFile) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)
            .map_err(|e| e.to_string())?;
        let raw = serde_json::to_vec(s).map_err(|e| e.to_string())?;
        let tmp = self
            .dir
            .join(format!(".chatgpt-auth-{}", &random_secret()[..12]));
        let res = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .and_then(|mut f| f.write_all(&raw))
            .and_then(|()| std::fs::rename(&tmp, self.dir.join(AUTH_FILE)));
        if res.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        res.map_err(|e| e.to_string())
    }

    /// The active account and the saved ones from this OAuth client.
    pub async fn status(&self) -> Result<Status, String> {
        let _st = self.state.lock().await;
        let mut s = self.read()?;
        let mut out = Status {
            accounts: s
                .accounts
                .iter()
                .filter(|a| a.client_id == OAUTH_CLIENT_ID)
                .map(|a| a.account.clone())
                .collect(),
            ..Default::default()
        };
        let active = s.active.clone();
        if let Some(a) = s
            .account(&active)
            .filter(|a| a.client_id == OAUTH_CLIENT_ID)
        {
            out.account_id = a.account.id.clone();
            out.email = a.account.email.clone();
            out.authenticated = !a.access_token.is_empty() && !a.workspace_id.is_empty();
        }
        Ok(out)
    }

    pub(crate) async fn exchange(&self, form: &[(&str, &str)]) -> Result<TokenResponse, String> {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form)
            .finish();
        let raw = self
            .request(
                self.http
                    .post(&self.token_url)
                    .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(body),
            )
            .await?;
        let mut out: TokenResponse = serde_json::from_slice(&raw)
            .map_err(|_| "invalid ChatGPT token response".to_owned())?;
        if out.access_token.is_empty()
            || (!out.token_type.is_empty() && !out.token_type.eq_ignore_ascii_case("Bearer"))
        {
            return Err("incomplete ChatGPT token response".into());
        }
        if out.expires_in <= 0 {
            out.expires_in = 3600;
        }
        Ok(out)
    }

    /// Sends `req` and returns a success body. Error bodies can contain
    /// credentials or authorization URLs, so only the status is reported.
    pub(crate) async fn request(&self, req: reqwest::RequestBuilder) -> Result<Vec<u8>, String> {
        let mut resp = req
            .header(USER_AGENT, "recur")
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| format!("could not reach OpenAI: {e}"))?;
        let mut raw = Vec::new();
        while raw.len() < MAX_BODY {
            match resp.chunk().await {
                Ok(Some(c)) => raw.extend_from_slice(&c[..c.len().min(MAX_BODY - raw.len())]),
                Ok(None) => break,
                Err(e) => return Err(e.to_string()),
            }
        }
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            if status == 401 || status == 400 {
                return Err(format!(
                    "OpenAI rejected the session (HTTP {status}). Continue with ChatGPT in Settings to sign in again"
                ));
            }
            return Err(format!("OpenAI request failed (HTTP {status})"));
        }
        Ok(raw)
    }

    /// A current access token, refreshed near expiry. The expected account
    /// keeps a running turn from silently switching identities after the
    /// user changes accounts.
    pub async fn access_token(&self, account_id: &str) -> Result<String, String> {
        let st = self.state.lock().await;
        Ok(self.access_credentials(&st, account_id).await?.access_token)
    }

    /// Sets the bearer token and ChatGPT workspace from one credential
    /// snapshot.
    pub async fn authorize(&self, headers: &mut HeaderMap, account_id: &str) -> Result<(), String> {
        let st = self.state.lock().await;
        let a = self.access_credentials(&st, account_id).await?;
        let value = |v: &str| HeaderValue::from_str(v).map_err(|e| e.to_string());
        headers.insert(AUTHORIZATION, value(&format!("Bearer {}", a.access_token))?);
        headers.insert("ChatGPT-Account-Id", value(&a.workspace_id)?);
        headers.insert("Originator", HeaderValue::from_static("recur"));
        headers.insert(USER_AGENT, HeaderValue::from_static("recur"));
        Ok(())
    }

    /// The caller holds the state lock through refresh and persistence.
    async fn access_credentials(
        &self,
        st: &State,
        account_id: &str,
    ) -> Result<Credentials, String> {
        if st.closed {
            return Err("ChatGPT provider is shutting down".into());
        }
        let mut s = self.read()?;
        let active = s.active.clone();
        let Some(a) = s.account(account_id).filter(|a| {
            !account_id.is_empty()
                && active == account_id
                && !a.access_token.is_empty()
                && a.client_id == OAUTH_CLIENT_ID
                && !a.workspace_id.is_empty()
        }) else {
            return Err(
                "sign in using browser or headless login in Settings → Providers → OpenAI".into(),
            );
        };
        if Utc::now() + chrono::Duration::minutes(1) < a.expires_at {
            return Ok(a.clone());
        }
        if a.refresh_token.is_empty() {
            return Err(
                "ChatGPT session expired. Continue with ChatGPT in Settings → Providers → OpenAI"
                    .into(),
            );
        }
        let mut tokens = self
            .exchange(&[
                ("grant_type", "refresh_token"),
                ("client_id", OAUTH_CLIENT_ID),
                ("refresh_token", &a.refresh_token),
            ])
            .await?;
        if tokens.scope.is_empty() {
            tokens.scope = a.scope.clone();
        }
        if tokens.refresh_token.is_empty() {
            tokens.refresh_token = a.refresh_token.clone();
        }
        if !tokens.id_token.is_empty() {
            let identity = self
                .validate_id_token(&tokens.id_token, OAUTH_CLIENT_ID)
                .await?;
            let workspace = identity.account_id();
            if identity.subject != a.subject
                || (!workspace.is_empty() && workspace != a.workspace_id)
            {
                return Err("ChatGPT refresh returned a different account".into());
            }
            a.id_token = tokens.id_token;
        }
        a.access_token = tokens.access_token;
        a.refresh_token = tokens.refresh_token;
        a.scope = tokens.scope;
        a.expires_at = Utc::now() + chrono::Duration::seconds(tokens.expires_in);
        let out = a.clone();
        self.write(&s)?;
        Ok(out)
    }

    /// Removes Recur's active local tokens, keeping account metadata for a
    /// later sign-in, like OpenCode sign-out.
    pub async fn disconnect(&self) -> Result<(), String> {
        let _st = self.state.lock().await;
        let mut s = self.read()?;
        let active = s.active.clone();
        if let Some(a) = s.account(&active) {
            a.access_token.clear();
            a.refresh_token.clear();
            a.id_token.clear();
            a.scope.clear();
            a.expires_at = DateTime::default();
        }
        s.active.clear();
        self.write(&s)
    }

    /// The account's listed models, in server order.
    pub async fn models(&self, account_id: &str) -> Result<Vec<Model>, String> {
        let mut headers = HeaderMap::new();
        self.authorize(&mut headers, account_id).await?;
        let raw = self
            .request(self.http.get(&self.models_url).headers(headers))
            .await?;
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Out {
            models: Vec<Model>,
        }
        let out: Out = serde_json::from_slice(&raw)
            .map_err(|_| "invalid ChatGPT model catalogue".to_owned())?;
        let models: Vec<Model> = out
            .models
            .into_iter()
            .filter(|m| m.visibility == "list" && !m.id.trim().is_empty())
            .map(|mut m| {
                if m.name.is_empty() {
                    m.name = m.id.clone();
                }
                m
            })
            .collect();
        if models.is_empty() {
            return Err("no models are available for this ChatGPT account".into());
        }
        Ok(models)
    }
}

/// 32 random bytes, base64url.
pub(crate) fn random_secret() -> String {
    let mut b = [0u8; 32];
    aws_lc_rs::rand::fill(&mut b).expect("system random");
    URL_SAFE_NO_PAD.encode(b)
}

/// `fut`, unless `cancel` fires first.
pub(crate) async fn or_cancel<T>(
    cancel: &CancellationToken,
    msg: &str,
    fut: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(msg.to_owned()),
        r = fut => r,
    }
}
