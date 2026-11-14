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

fn active_account(dir: &Path) -> Option<Account> {
    let raw = std::fs::read(dir.join("chatgpt-auth.json")).ok()?;
    let file: AuthFile = serde_json::from_slice(&raw).ok()?;
    file.accounts
        .into_iter()
        .find(|a| a.id == file.active)
        .filter(|a| !a.access_token.is_empty())
}

pub(crate) async fn fetch_chatgpt(ctx: &Ctx, f: &Fetcher) -> ProviderQuota {
    let out = ProviderQuota {
        id: PROVIDER_CHATGPT.into(),
        name: "ChatGPT".into(),
        ..Default::default()
    };
    let fail = |error: &str, hint: String| ProviderQuota {
        error: error.into(),
        hint,
        ..out.clone()
    };
    // An empty row still replaces the last windows after sign-out; the
    // popover hides it.
    let Some(acct) = f.data_dir().as_deref().and_then(active_account) else {
        return out;
    };
    let mut auth = CodexAuth {
        access_token: acct.access_token,
        id_token: acct.id_token,
        account_id: acct.workspace_id,
        email: acct.email,
        has_api_key: false,
        has_chatgpt: true,
    };
    if let Some(refresh) = &f.chatgpt_refresh {
        // Detached so the collect deadline cannot drop a refresh between
        // OpenAI rotating the refresh token and the provider saving it.
        let task = tokio::spawn(refresh(acct.id));
        match ctx.run(task).await {
            Ok(Ok(Ok(token))) => auth.access_token = token,
            Err(e) => return fail("Could not reach ChatGPT", e.to_string()),
            Ok(Err(e)) => return fail("ChatGPT usage unavailable", e.to_string()),
            Ok(Ok(Err(e))) if e.starts_with("could not reach") => {
                return fail("Could not reach ChatGPT", e);
            }
            Ok(Ok(Err(e))) if e.starts_with("OpenAI rejected") => {
                return fail(
                    "ChatGPT login expired",
                    format!("Sign in with ChatGPT again in {SETTINGS_PATH}."),
                );
            }
            Ok(Ok(Err(e))) => return fail("ChatGPT usage unavailable", e),
        }
    }
    fetch_wham_usage(ctx, f, auth, out, "ChatGPT", SETTINGS_PATH).await
}
