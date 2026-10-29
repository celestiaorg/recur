//! The agent provider abstraction: the `Provider` trait, normalized stream
//! events, model catalog, permissions, and the runner contract every
//! provider implements.
//!
//! It also holds error classification, attachment markers, permission
//! labels, stream display rules, scripted test providers, the context usage
//! tracker, the catalog and quota types, and the pure display rules for
//! context usage, tasks, todos, web tools and MCP.

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
