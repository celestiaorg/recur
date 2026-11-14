//! Per-provider quota sources for `Provider::usage_probe` (one provider's
//! row, fetched the same way the poller collects it).

use std::sync::Arc;

use recur_agent::UsageProbe;

use crate::ctx::Ctx;
use crate::fetcher::Fetcher;
use crate::poller::COLLECT_TIMEOUT;
use crate::types::{PROVIDER_CLAUDE, PROVIDER_CODEX, PROVIDER_CURSOR, ProviderQuota};

fn ctx() -> Ctx {
    Ctx::background().with_timeout(COLLECT_TIMEOUT)
}

/// Claude's OAuth usage endpoint with the Claude Code login.
pub struct ClaudeProbe(pub Arc<Fetcher>);

/// ChatGPT rate limits with the Codex login.
pub struct CodexProbe(pub Arc<Fetcher>);

/// Cursor included usage with the Recur Connect session.
pub struct CursorProbe(pub Arc<Fetcher>);

#[async_trait::async_trait]
impl UsageProbe for ClaudeProbe {
    fn provider_id(&self) -> &str {
        PROVIDER_CLAUDE
    }
    async fn fetch(&self) -> ProviderQuota {
        crate::claude::fetch_claude(&ctx(), &self.0).await
    }
}

#[async_trait::async_trait]
impl UsageProbe for CodexProbe {
    fn provider_id(&self) -> &str {
        PROVIDER_CODEX
    }
    async fn fetch(&self) -> ProviderQuota {
        crate::codex::fetch_codex(&ctx(), &self.0).await
    }
}

#[async_trait::async_trait]
impl UsageProbe for CursorProbe {
    fn provider_id(&self) -> &str {
        PROVIDER_CURSOR
    }
    async fn fetch(&self) -> ProviderQuota {
        crate::cursor::fetch_cursor(&ctx(), &self.0).await
    }
}
