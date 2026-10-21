//! The `Provider` trait: everything the engine asks of an agent backend.
//! Every behavior Go branched on by provider name is a capability or a
//! method here, so the engine never compares provider ids.

use std::path::Path;
use std::sync::Arc;

use recur_domain::{CompatibleProvider, Settings};

use crate::catalog::ModelChoice;
use crate::display::ToolDisplay;
use crate::error::ProviderError;
use crate::event::TurnResult;
use crate::quota::UsageProbe;
use crate::turn::{TurnContext, TurnRequest};

/// Provider-owned model dimensions (effort, context, fast, …). Empty keys
/// are ignored.
pub type Flags = std::collections::BTreeMap<String, String>;

#[async_trait::async_trait]
pub trait Provider: Send + Sync + 'static {
    /// The persisted slug (`claude-code`, `codex`, `cursor`, a compatible
    /// provider's id).
    fn id(&self) -> &str;
    /// The picker group label (`Claude Code`, `Cursor`, …).
    fn label(&self) -> &str;
    fn caps(&self) -> &ProviderCaps;

    /// Enabled and configured under these settings. The router never falls
    /// back to another provider: `Unavailable` becomes the visible turn error.
    fn availability(&self, settings: &Settings) -> Availability;

    /// The Settings → Providers status card.
    async fn status(&self, settings: &Settings) -> ProviderStatus;
    /// Validates a draft credential without persisting it (throwaway
    /// process, bridge or ping). Providers without credentials report
    /// [`Provider::status`].
    async fn test_credentials(
        &self,
        settings: &Settings,
        _draft: &CredentialDraft,
    ) -> ProviderStatus {
        self.status(settings).await
    }

    /// The live model list; providers without a catalog API return a static
    /// one.
    async fn models(&self, settings: &Settings) -> Result<Vec<ModelChoice>, ProviderError>;
    /// The last good list (offline fallback when remapping settings slots).
    fn cached_models(&self, settings: &Settings) -> Vec<ModelChoice>;
    /// The wire model id from a bare id and flags (Claude `[1m]` suffix).
    fn resolve_model(&self, model: &str, flags: &Flags) -> String;
    /// The context window for footers, `context_usage` and meter clamps; 0 =
    /// unknown.
    fn context_window(&self, model: &str, flags: &Flags) -> i64;
    /// A human model label for fallback notices and footers.
    fn model_label(&self, model: &str) -> String;
    /// Flags for one-shot title-model turns (commit message, PR text).
    fn oneshot_flags(&self, model: &str) -> Flags;

    /// One agent turn (Go `AgentRunner.Run`). Events go to `cx.sink`
    /// synchronously and in order, each run through a
    /// [`crate::ContextUsageTracker`]; every provider result frame is also
    /// emitted as [`crate::AgentEvent::TurnEnd`]. On cancel the provider
    /// stops the remote run, emits nothing further, and returns
    /// [`ProviderError::canceled`] carrying the partial result.
    async fn run_turn(
        &self,
        req: TurnRequest,
        cx: TurnContext,
    ) -> Result<TurnResult, ProviderError>;

    /// Whether a stored resume handle belongs to this provider. False makes
    /// the engine drop the handle and fork-inject the transcript instead.
    fn resume_handle_fits(&self, handle: &str) -> bool;

    /// A raw tool call in the common vocabulary. Default: passthrough.
    fn display_tool(&self, name: &str, input: &str) -> ToolDisplay {
        ToolDisplay {
            name: name.to_owned(),
            input: input.to_owned(),
        }
    }
    /// A raw tool result as cards show it. Default: passthrough. Call
    /// [`crate::display::display_tool_result`], which also hides Task launch
    /// acks.
    fn display_tool_result(&self, _name: &str, result: &str) -> String {
        result.to_owned()
    }

    /// The subscription quota source for the sidebar footer.
    fn usage_probe(&self) -> Option<Arc<dyn UsageProbe>> {
        None
    }
    /// A worktree is being removed: drop pooled processes bound to it and
    /// provider-side project dirs. `Err` carries the complete sentence the
    /// engine shows in the workspace feed (Go emitted "Could not remove
    /// Cursor project dirs: …" as a Maestro event, or logged it when the
    /// workspace was already gone).
    async fn release_worktree(&self, _worktree: &Path) -> Result<(), ProviderError> {
        Ok(())
    }
    /// Agent slots went to zero.
    async fn on_idle(&self) {}
    /// Settings changed (a key or binary path change may tear down pools).
    async fn settings_changed(&self, _old: &Settings, _new: &Settings) {}
    async fn shutdown(&self) {}
}

/// What a provider supports. The engine reads these instead of provider ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCaps {
    /// How tool permission prompts reach the human.
    pub permission: PermissionRoute,
    /// Mid-turn steering. `None` makes the engine queue the steer as a new
    /// turn.
    pub steer: SteerSupport,
    /// Who owns conversation history across turns.
    pub history: HistoryMode,
    /// How Recur host tools reach the agent.
    pub tool_transport: ToolTransport,
    /// Native fork of a resumed session (Claude `--fork-session`).
    pub native_fork: bool,
    /// Accepts `TurnRequest.images`.
    pub images: bool,
    /// `tools = Some("")` really disables tools. Cursor cannot, so one-shot
    /// turns get Read/Glob/Grep instead.
    pub can_disable_tools: bool,
    /// Honors `TurnRequest.max_turns`.
    pub max_turns: bool,
    /// Streams `ThinkingDelta`/`TextDelta` when `include_partials`.
    pub partials: bool,
    /// User MCP server transports it can wire.
    pub user_mcp: McpTransports,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionRoute {
    /// The engine serves `mcp__recur__permission_request` and the provider
    /// passes it as the CLI prompt tool (Claude).
    McpPromptTool,
    /// The provider calls `cx.permissions` itself (Codex approvals, the
    /// compatible Bash gate through the tool host).
    Callback,
    /// No prompts (Cursor: sandbox off, tools pre-scoped).
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteerSupport {
    None,
    /// A steer enters the running turn (Claude stdin after the next
    /// tool_result, Codex `turn/steer`, compatible between rounds).
    Live,
    /// Live when the backend allows, else cancel-and-follow-up inside the
    /// same `run_turn` (Cursor SteerRun, else CancelRun + one joined Send).
    LiveOrRestart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryMode {
    /// The provider keeps the conversation; a resume handle continues it.
    Native,
    /// The provider rebuilds input from the Recur transcript every turn
    /// (compatible). The engine skips fork preambles and resume-handle checks.
    App,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolTransport {
    /// Host tools over the engine's MCP server (`TurnRequest.mcp_url`).
    Mcp,
    /// Host tools run in-process through `cx.tools` (compatible harness).
    InProcess,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct McpTransports {
    pub http: bool,
    pub stdio: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Ready,
    /// The exact user-facing reason ("Codex is turned off in Settings →
    /// Providers").
    Unavailable(String),
}

/// The Settings → Providers card: the union of the Go per-provider status
/// payloads (`ClaudeStatus`, `CodexStatus`, `CursorStatus`,
/// `CompatibleTestResult`). Each provider fills the fields it has.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderStatus {
    /// The CLI binary or bridge was found (Claude path set, Codex
    /// `binInstalled`, Cursor `bridgeInstalled`).
    pub installed: bool,
    /// The resolved binary or bridge launcher.
    pub path: String,
    /// The CLI version, or the installed cursor-sdk pin.
    pub version: String,
    /// The release Recur would install (Cursor).
    pub pinned_version: String,
    /// The installed bridge is behind `pinned_version`.
    pub update_available: bool,
    /// Logged in / the key works / the ping succeeded.
    pub authenticated: bool,
    /// The API provider label (Claude: "Anthropic API").
    pub provider: String,
    /// Login or plan label ("Claude Team account", "ChatGPT").
    pub login_method: String,
    pub organization: String,
    pub email: String,
    pub plan: String,
    /// A short identity for toasts (email or API key name).
    pub account: String,
    pub api_key_name: String,
    pub name: String,
    /// How many models the key may use.
    pub model_count: i64,
    /// A user-facing reason when not ready.
    pub error: String,
}

/// A credential to test before saving.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CredentialDraft {
    pub api_key: String,
    /// Codex: use ChatGPT login instead of the key.
    pub chatgpt_auth: bool,
    /// Compatible: the draft provider config.
    pub compatible: Option<CompatibleProvider>,
}
