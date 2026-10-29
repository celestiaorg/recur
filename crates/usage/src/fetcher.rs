//! One snapshot collection across Claude, Codex and Cursor.
//! Tests and services replace the hook fields.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::claude::{ClaudeOAuth, fetch_claude};
use crate::codex::{CodexAuth, fetch_codex};
use crate::connect::cursor_connecting_quota;
use crate::ctx::Ctx;
use crate::cursor::{CursorTok, fetch_cursor};
use crate::http::default_client;
use crate::types::{ProviderQuota, Snapshot};

/// When Codex or Cursor also run, Claude's CLI and HTTP steps share this
/// budget so a hung `claude auth status` cannot spend the whole collect.
pub const CLAUDE_COLLECT_BUDGET: Duration = Duration::from_secs(12);

pub type Hook<T> = Option<Arc<dyn Fn() -> T + Send + Sync>>;

/// `(previous key, next key)`.
pub type MintedKeyHook = Option<Arc<dyn Fn(&str, &str) + Send + Sync>>;

/// Gathers one snapshot. Every hook is optional; `None` is the production
/// path (keychain, auth files, `claude auth status`, the Recur data dir).
pub struct Fetcher {
    pub http: reqwest::Client,
    pub data_dir: Hook<Option<PathBuf>>,
    /// Unix milliseconds.
    pub now: Hook<i64>,

    /// A fixed Claude token; skips expiry handling (tests).
    pub claude_token: Hook<Result<String, String>>,
    /// Reads the Claude Code keychain item and credential files.
    pub claude_creds: Hook<ClaudeOAuth>,
    /// Runs `claude auth status` so the CLI rotates an expired access token,
    /// the way Settings → Providers already does.
    pub nudge_claude: Hook<()>,
    pub codex_auth: Hook<Result<CodexAuth, String>>,
    pub cursor_token: Hook<Result<CursorTok, String>>,

    pub claude_usage_url: String,
    pub codex_usage_url: String,
    pub codex_monthly_url: String,
    pub cursor_period_url: String,
    pub cursor_agg_url: String,
    pub cursor_profile_url: String,
    pub cursor_me_url: String,
    pub cursor_web_me_url: String,
    pub cursor_get_me_url: String,
    pub cursor_login_url: String,
    pub cursor_poll_url: String,
    pub cursor_refresh_url: String,
    pub cursor_mint_url: String,
    pub cursor_list_keys_url: String,
    pub cursor_revoke_key_url: String,
    /// Called when Connect (or a later fetch) mints or rotates a user API
    /// key: `(previous, next)`, previous empty on first mint.
    pub on_minted_api_key: MintedKeyHook,

    pub want_claude: bool,
    pub want_codex: bool,
    pub want_cursor: bool,
    /// Consulted on every collect so Settings flips apply without restarting
    /// the poller; the bools above are the fallback.
    pub want: Hook<(bool, bool, bool)>,
    /// When Cursor is not fetched, a local-only row (Disconnect tokens, or a
    /// no-session Connect hint) with no network call.
    pub cursor_stub: Hook<Option<ProviderQuota>>,

    pub(crate) connect: Mutex<ConnectState>,
}

#[derive(Default)]
pub(crate) struct ConnectState {
    pub connecting: bool,
    pub cancel: Option<CancellationToken>,
    pub url: String,
}

impl Default for Fetcher {
    fn default() -> Self {
        Self {
            http: default_client(),
            data_dir: None,
            now: None,
            claude_token: None,
            claude_creds: None,
            nudge_claude: None,
            codex_auth: None,
            cursor_token: None,
            claude_usage_url: String::new(),
            codex_usage_url: String::new(),
            codex_monthly_url: String::new(),
            cursor_period_url: String::new(),
            cursor_agg_url: String::new(),
            cursor_profile_url: String::new(),
            cursor_me_url: String::new(),
            cursor_web_me_url: String::new(),
            cursor_get_me_url: String::new(),
            cursor_login_url: String::new(),
            cursor_poll_url: String::new(),
            cursor_refresh_url: String::new(),
            cursor_mint_url: String::new(),
            cursor_list_keys_url: String::new(),
            cursor_revoke_key_url: String::new(),
            on_minted_api_key: None,
            want_claude: false,
            want_codex: false,
            want_cursor: false,
            want: None,
            cursor_stub: None,
            connect: Mutex::default(),
        }
    }
}

/// A hook from a closure.
pub fn hook<T>(f: impl Fn() -> T + Send + Sync + 'static) -> Hook<T> {
    Some(Arc::new(f))
}

fn or<'a>(set: &'a str, default: &'a str) -> &'a str {
    if set.is_empty() { default } else { set }
}

impl Fetcher {
    pub(crate) fn now_ms(&self) -> i64 {
        match &self.now {
            Some(f) => f(),
            None => chrono::Utc::now().timestamp_millis(),
        }
    }

    /// The managed data root; `None` disables Connect token files.
    pub(crate) fn data_dir(&self) -> Option<PathBuf> {
        self.data_dir.as_ref().and_then(|f| f())
    }

    pub(crate) fn url<'a>(&'a self, set: &'a str, default: &'a str) -> &'a str {
        or(set, default)
    }

    fn wants(&self) -> (bool, bool, bool) {
        match &self.want {
            Some(f) => f(),
            None => (self.want_claude, self.want_codex, self.want_cursor),
        }
    }

    pub(crate) fn cursor_connecting(&self) -> bool {
        self.connect.lock().connecting
    }

    /// Fetches every requested provider. A 429 on one provider is marked
    /// `rate_limited` so the poller can keep the last-known row.
    pub async fn collect(&self, ctx: &Ctx) -> Snapshot {
        let fetched_at = self.now_ms();
        let mut providers = Vec::new();
        let (claude, codex, cursor) = self.wants();
        if claude {
            let cctx = claude_fetch_ctx(ctx, codex || cursor);
            providers.push(fetch_claude(&cctx, self).await);
        }
        if codex {
            providers.push(fetch_codex(ctx, self).await);
        }
        if cursor {
            providers.push(fetch_cursor(ctx, self).await);
        } else if self.cursor_connecting() {
            providers.push(cursor_connecting_quota());
        } else if let Some(q) = self.cursor_stub.as_ref().and_then(|f| f()) {
            providers.push(q);
        }
        Snapshot {
            fetched_at,
            stale: false,
            providers: Some(providers),
        }
    }
}

/// Claude's context: capped at [`CLAUDE_COLLECT_BUDGET`] when other providers
/// share the collect.
pub(crate) fn claude_fetch_ctx(ctx: &Ctx, others: bool) -> Ctx {
    if others {
        ctx.with_timeout(CLAUDE_COLLECT_BUDGET)
    } else {
        ctx.with_cancel()
    }
}
