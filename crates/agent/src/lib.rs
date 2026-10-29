//! The agent provider abstraction: the `Provider` trait, normalized stream
//! events, model catalog, permissions, and the runner contract every
//! provider implements.
//!
//! Port of the provider-neutral contracts in Go `internal/engine`
//! (`runner.go`, `toolview.go`, the classification half of `agenterr.go`,
//! the marker half of `attachments.go`, the label half of `permission.go`,
//! the display half of `streamjson.go`, `scripted.go`, mirror's
//! `contextUsageTracker`), the catalog types of `app/model_catalog.go`, the
//! quota types of `usage/types.go`, and the pure rules of the frontend
//! `contextUsage`, `tasktools`, `todos`, `webtools` and `mcp` modules.

pub mod attach;
pub mod catalog;
pub mod context_usage;
pub mod display;
pub mod error;
pub mod event;
pub mod mcp;
pub mod permission;
pub mod provider;
pub mod quota;
#[cfg(feature = "testing")]
pub mod testing;
pub mod turn;

pub use attach::UserImage;
pub use catalog::{FlagDimension, FlagOption, ModelChoice, ModelGroup};
pub use display::ToolDisplay;
pub use error::{ErrorKind, ProviderError};
pub use event::{
    AgentEvent, ContextUsageTracker, McpServerStatus, TokenUsage, ToolResultEvent, ToolStep,
    TurnResult,
};
pub use provider::{
    Availability, CredentialDraft, Flags, HistoryMode, McpTransports, PermissionRoute, Provider,
    ProviderCaps, ProviderStatus, SteerSupport, ToolTransport,
};
pub use quota::{ProviderQuota, Snapshot, UsageProbe};
pub use turn::{
    EventSink, HistorySource, McpAttach, PermissionAbandoned, PermissionAsker, SessionRoute, Steer,
    SteerInbox, ToolCallMeta, ToolHost, ToolOutcome, ToolSpec, TurnContext, TurnRequest,
    standing_in_user_prompt,
};
