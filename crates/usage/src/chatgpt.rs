//! Direct ChatGPT account rate limits: the same `wham/usage` endpoint as
//! Codex, with the login Recur's ChatGPT provider keeps in `chatgpt-auth.json`.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::codex::{CodexAuth, fetch_wham_usage};
use crate::ctx::Ctx;
use crate::fetcher::Fetcher;
use crate::types::{PROVIDER_CHATGPT, ProviderQuota};

const SETTINGS_PATH: &str = "Settings → Providers → OpenAI";

#[derive(Deserialize, Default)]
#[serde(default)]
struct AuthFile {
    active: String,
    accounts: Vec<Account>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Account {
    id: String,
    email: String,
    #[serde(rename = "chatgptAccountId")]
    workspace_id: String,
    access_token: String,
    id_token: String,
    expires_at: Option<DateTime<Utc>>,
}

const STALE: &str = "ChatGPT token stale";

fn active_account(dir: &Path) -> Result<Account, String> {
    let raw = std::fs::read(dir.join("chatgpt-auth.json")).map_err(|e| e.to_string())?;
    let file: AuthFile = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
    file.accounts
        .into_iter()
        .find(|a| a.id == file.active)
        .filter(|a| !a.access_token.is_empty())
        .ok_or_else(|| "no ChatGPT account".into())
}

/// Whether the ChatGPT provider has a signed-in active account.
pub(crate) fn has_chatgpt_account(dir: &Path) -> bool {
    active_account(dir).is_ok()
}

/// The active account's saved access token. Only the ChatGPT provider
/// refreshes it, so an expired one is reported as stale instead of requested.
pub(crate) fn default_chatgpt_auth(
    data_dir: Option<&Path>,
    now_ms: i64,
) -> Result<CodexAuth, String> {
    let acct = active_account(data_dir.ok_or("no data dir")?)?;
    if acct
        .expires_at
        .is_some_and(|t| t.timestamp_millis() <= now_ms)
    {
        return Err(STALE.into());
    }
    Ok(CodexAuth {
        access_token: acct.access_token,
        id_token: acct.id_token,
        account_id: acct.workspace_id,
        email: acct.email,
        has_api_key: false,
        has_chatgpt: true,
    })
}

pub(crate) async fn fetch_chatgpt(ctx: &Ctx, f: &Fetcher) -> ProviderQuota {
    let out = ProviderQuota {
        id: PROVIDER_CHATGPT.into(),
        name: "ChatGPT".into(),
        ..Default::default()
    };
    let auth = match &f.chatgpt_auth {
        Some(h) => h(),
        None => default_chatgpt_auth(f.data_dir().as_deref(), f.now_ms()),
    };
    match auth {
        Ok(a) if a.has_chatgpt => fetch_wham_usage(ctx, f, a, out, "ChatGPT", SETTINGS_PATH).await,
        Err(e) if e == STALE => ProviderQuota {
            error: "ChatGPT session needs a refresh".into(),
            hint: "Start a ChatGPT chat in Recur to refresh it.".into(),
            ..out
        },
        _ => ProviderQuota {
            error: "Not signed in to ChatGPT".into(),
            hint: format!("Sign in with ChatGPT in {SETTINGS_PATH}."),
            ..out
        },
    }
}
