//! Direct ChatGPT account rate limits: the same `wham/usage` endpoint as
//! Codex, with the login Recur's ChatGPT provider keeps in `chatgpt-auth.json`.

use std::path::Path;

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
}

/// The active account's saved access token. A stale token is not refreshed
/// here; the usage request then reports an expired login.
pub(crate) fn default_chatgpt_auth(data_dir: Option<&Path>) -> Result<CodexAuth, String> {
    let dir = data_dir.ok_or("no data dir")?;
    let raw = std::fs::read(dir.join("chatgpt-auth.json")).map_err(|e| e.to_string())?;
    let file: AuthFile = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
    let acct = file
        .accounts
        .into_iter()
        .find(|a| a.id == file.active)
        .filter(|a| !a.access_token.is_empty())
        .ok_or("no ChatGPT account")?;
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
        None => default_chatgpt_auth(f.data_dir().as_deref()),
    };
    match auth {
        Ok(a) if a.has_chatgpt => fetch_wham_usage(ctx, f, a, out, "ChatGPT", SETTINGS_PATH).await,
        _ => ProviderQuota {
            error: "Not signed in to ChatGPT".into(),
            hint: format!("Sign in with ChatGPT in {SETTINGS_PATH}."),
            ..out
        },
    }
}
