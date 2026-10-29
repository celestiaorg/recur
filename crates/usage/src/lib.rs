//! Subscription-quota snapshots for Claude, Codex and Cursor.
//!
//! [`Poller`] caches one [`Snapshot`] over a [`Fetcher`] and publishes
//! `provider.usage`; [`UsageService`] is the Settings-facing API services wrap;
//! [`ClaudeProbe`], [`CodexProbe`] and [`CursorProbe`] serve
//! `Provider::usage_probe`.

pub mod claude;
pub mod codex;
pub mod connect;
pub mod ctx;
pub mod cursor;
pub mod fetcher;
mod http;
mod parse;
pub mod poller;
pub mod probe;
pub mod service;
pub mod types;

pub use claude::ClaudeOAuth;
pub use codex::{CodexAuth, format_codex_plan};
pub use connect::{cursor_connect_tokens_quota, cursor_connecting_quota, cursor_no_session_quota};
pub use ctx::{Ctx, CtxError};
pub use cursor::{
    CursorTok, delete_recur_cursor_auth, has_recur_cursor_auth_file, recur_cursor_api_key,
};
pub use fetcher::{Fetcher, Hook, hook};
pub use http::default_client;
pub use poller::{Poller, Publisher, merge_stale};
pub use probe::{ClaudeProbe, CodexProbe, CursorProbe};
pub use service::UsageService;
pub use types::*;

#[cfg(test)]
mod tests;
