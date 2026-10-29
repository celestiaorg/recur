//! Settings → Providers probes for Codex: throwaway app-servers, so a bad key
//! or missing binary never wedges a loop.

use std::path::PathBuf;
use std::time::Duration;

use recur_agent::ProviderStatus;
use recur_domain::{PROVIDER_CODEX, Settings};
use tokio_util::sync::CancellationToken;

use crate::client::Client;
use crate::locate::locate;
use crate::rpc::Ctx;
use crate::server::{Server, ServerConfig};
use crate::types::{Model, format_plan};

pub const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// Where probes run: Recur's `CODEX_HOME` and data dir (also the probe
/// cwd).
#[derive(Debug, Clone)]
pub struct ProbeEnv {
    pub home: PathBuf,
    pub data_dir: PathBuf,
    pub env_scrub: Vec<String>,
    pub seed_auth_from: Option<PathBuf>,
}

/// A successful account probe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    pub login_method: String,
    pub email: String,
    pub plan: String,
    /// `None` when `model/list` failed.
    pub models: Option<Vec<Model>>,
}

/// ChatGPT-login precedence: with the toggle on, a leftover API key is
/// ignored so Settings' hidden key field cannot keep winning.
pub fn auth_choice(api_key: &str, chatgpt_auth: bool) -> (String, bool) {
    if chatgpt_auth {
        (String::new(), true)
    } else {
        (api_key.trim().to_owned(), false)
    }
}

pub fn login_method(account_type: &str, has_api_key: bool) -> String {
    match account_type.trim().to_lowercase().as_str() {
        "chatgpt" | "chat_gpt" | "openai" => return "ChatGPT account".into(),
        "apikey" | "api_key" | "api" => return "API key".into(),
        _ => {}
    }
    if has_api_key {
        "API key".into()
    } else if !account_type.is_empty() {
        account_type.into()
    } else {
        "ChatGPT account".into()
    }
}

async fn start(
    env: &ProbeEnv,
    bin: &str,
    api_key: &str,
    chatgpt_auth: bool,
    ctx: &Ctx,
) -> Result<Server, String> {
    let cfg = ServerConfig {
        bin: bin.into(),
        home: env.home.clone(),
        worktree: env.data_dir.clone(),
        api_key: api_key.into(),
        config_overrides: vec![r#"approval_policy="never""#.into()],
        seed_auth_from: if api_key.is_empty() && chatgpt_auth {
            env.seed_auth_from.clone()
        } else {
            None
        },
        env_scrub: env.env_scrub.clone(),
    };
    Server::start(&cfg, ctx).await
}

/// Reads the account (and models) through a throwaway app-server.
pub async fn probe(
    env: &ProbeEnv,
    bin: &str,
    api_key: &str,
    chatgpt_auth: bool,
) -> Result<Probe, String> {
    let ctx = Ctx::new(CancellationToken::new()).with_timeout(PROBE_TIMEOUT);
    let srv = start(env, bin, api_key, chatgpt_auth, &ctx).await?;
    let out = async {
        let client = Client::new(srv.conn().clone());
        let acct = client
            .account_read(&ctx)
            .await
            .map_err(|e| format!("account/read: {e}"))?;
        let Some(account) = acct.account else {
            return Err(
                "not authenticated: sign in with ChatGPT in Settings or set an API key".to_owned(),
            );
        };
        let models = client.model_list(&ctx, 50).await.ok();
        Ok(Probe {
            login_method: login_method(&account.typ, !api_key.is_empty()),
            email: account.email.trim().to_owned(),
            plan: format_plan(&account.plan_type),
            models,
        })
    }
    .await;
    srv.close().await;
    out
}

/// The status card without touching settings, plus the models the probe listed.
pub async fn status(settings: &Settings, env: &ProbeEnv) -> (ProviderStatus, Option<Vec<Model>>) {
    let bin = match locate(&settings.codex_bin_path) {
        Ok(bin) => bin,
        Err(e) => {
            return (
                ProviderStatus {
                    error: e,
                    ..Default::default()
                },
                None,
            );
        }
    };
    let mut out = ProviderStatus {
        installed: true,
        path: bin.clone(),
        ..Default::default()
    };
    if !settings.codex_configured() {
        out.error =
            "Enable Codex with an API key or ChatGPT auth (Settings → Providers → Codex).".into();
        return (out, None);
    }
    let (key, chat) = auth_choice(&settings.codex_api_key, settings.codex_chat_gpt_auth);
    match probe(env, &bin, &key, chat).await {
        Ok(p) => {
            fill(&mut out, &p);
            (out, p.models)
        }
        Err(e) => {
            out.error = e;
            (out, None)
        }
    }
}

fn fill(out: &mut ProviderStatus, p: &Probe) {
    out.authenticated = true;
    out.login_method = p.login_method.clone();
    out.email = p.email.clone();
    out.account = p.email.clone();
    out.plan = p.plan.clone();
}

/// Validates a draft key (or ChatGPT auth) before save. An empty draft key
/// falls back to the saved one.
pub async fn test_key(
    settings: &Settings,
    env: &ProbeEnv,
    api_key: &str,
    chatgpt_auth: bool,
) -> ProviderStatus {
    let api_key = if api_key.is_empty() {
        settings.codex_api_key.as_str()
    } else {
        api_key
    };
    let use_chat = chatgpt_auth || settings.codex_chat_gpt_auth;
    if api_key.is_empty() && !use_chat {
        return ProviderStatus {
            error: "Enter an API key or enable ChatGPT auth first.".into(),
            ..Default::default()
        };
    }
    let (key, chat) = auth_choice(api_key, use_chat);
    let bin = match locate(&settings.codex_bin_path) {
        Ok(bin) => bin,
        Err(e) => {
            return ProviderStatus {
                error: e,
                ..Default::default()
            };
        }
    };
    let mut out = ProviderStatus {
        installed: true,
        path: bin.clone(),
        ..Default::default()
    };
    match probe(env, &bin, &key, chat).await {
        Ok(p) => fill(&mut out, &p),
        Err(e) => out.error = e,
    }
    out
}

/// The live `model/list` for the picker. Turned off or unconfigured lists
/// nothing without probing.
pub async fn list_models(
    settings: &Settings,
    env: &ProbeEnv,
) -> Result<Option<Vec<Model>>, String> {
    if !settings.provider_enabled(PROVIDER_CODEX) || !settings.codex_configured() {
        return Ok(None);
    }
    let bin = locate(&settings.codex_bin_path)?;
    let (key, chat) = auth_choice(&settings.codex_api_key, settings.codex_chat_gpt_auth);
    let ctx = Ctx::new(CancellationToken::new()).with_timeout(PROBE_TIMEOUT);
    let srv = start(env, &bin, &key, chat, &ctx).await?;
    let models = Client::new(srv.conn().clone())
        .model_list(&ctx, 50)
        .await
        .map_err(|e| e.to_string());
    srv.close().await;
    models.map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_and_login() {
        assert_eq!(auth_choice(" sk ", false), ("sk".into(), false));
        assert_eq!(auth_choice("sk", true), (String::new(), true));
        assert_eq!(login_method("chatgpt", false), "ChatGPT account");
        assert_eq!(login_method("apiKey", false), "API key");
        assert_eq!(login_method("", true), "API key");
        assert_eq!(login_method("enterprise_sso", false), "enterprise_sso");
        assert_eq!(login_method("", false), "ChatGPT account");
    }

    fn env(dir: &std::path::Path) -> ProbeEnv {
        ProbeEnv {
            home: dir.join("codex-home"),
            data_dir: dir.into(),
            env_scrub: Vec::new(),
            seed_auth_from: None,
        }
    }

    #[tokio::test]
    async fn status_without_credentials_or_binary() {
        let dir = tempfile::tempdir().unwrap();
        let missing = Settings {
            codex_bin_path: "/no/such/codex".into(),
            ..Default::default()
        };
        let (st, _) = status(&missing, &env(dir.path())).await;
        if st.installed {
            // A real codex on this machine was found past the bad path.
            assert_eq!(
                st.error,
                "Enable Codex with an API key or ChatGPT auth (Settings → Providers → Codex)."
            );
        } else {
            assert!(
                st.error.starts_with("codex binary not found"),
                "{}",
                st.error
            );
        }
        let blank = test_key(&Settings::default(), &env(dir.path()), "", false).await;
        assert_eq!(
            blank.error,
            "Enter an API key or enable ChatGPT auth first."
        );
        let off = Settings {
            disabled_providers: vec!["codex".into()],
            codex_api_key: "sk".into(),
            ..Default::default()
        };
        assert_eq!(list_models(&off, &env(dir.path())).await, Ok(None));
    }
}
