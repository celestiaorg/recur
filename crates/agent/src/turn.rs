//! One agent turn: what the engine asks for and the channels a provider talks
//! back through.

use std::path::PathBuf;
use std::sync::Arc;

use recur_domain::{AgentRole, Message, PermissionDecision, PermissionRequest};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::attach::UserImage;
use crate::event::AgentEvent;
use crate::provider::Flags;

/// Everything the engine decides for a turn. Empty string fields
/// mean unset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnRequest {
    /// The owning workspace; empty for app-scoped (Ask Recur) turns.
    pub workspace_id: String,
    /// The stable agent id (`<workspace>-<code>`).
    pub agent_id: String,
    /// The conductor tab (PA*, MA) of a loop specialist turn.
    pub chat_code: String,
    /// The workspace has a parent and cannot nest children.
    pub chat_child: bool,
    /// Selects the system prompt and tools; empty for one-shots.
    pub role: AgentRole,
    /// The BARE provider model id; empty = provider default.
    pub model: String,
    pub flags: Flags,
    /// The role / Recur overlay. Claude appends it to its own prompt.
    pub system_prompt: String,
    /// Extra overlay (chat-mode notes, simplify), concatenated after
    /// `system_prompt`.
    pub append_system_prompt: String,
    /// Per-turn context (the surface or PR the operator is looking at).
    /// Providers without a system prompt API put it on every user message.
    pub turn_append_prompt: String,
    /// Re-attach role and append notes in the user message even when
    /// resuming (mode remounts, human dock-mode changes). See
    /// [`standing_in_user_prompt`].
    pub refresh_standing: bool,
    /// The scoped tool allowlist. `None` = no restriction (provider
    /// default); `Some(vec![])` = nothing allowed where the provider gates
    /// on it (the compatible harness).
    pub allowed_tools: Option<Vec<String>>,
    /// Claude `--tools`: the complete built-in toolbox. `Some("")` disables
    /// all tools.
    pub tools: Option<String>,
    /// The Claude `--permission-mode` value from Settings; empty fails
    /// closed to `dontAsk`.
    pub permission_mode: String,
    /// The full wire text, persisted as the user row; may contain
    /// `⟦attachment:<id>⟧` markers.
    pub prompt: String,
    /// An optional UI label for the user row.
    pub display_text: String,
    /// The engine skipped recording the user row (Retry, remounts); history
    /// rebuilders must not append the prompt twice.
    pub skip_record_user: bool,
    /// The durable id of this turn's user row.
    pub user_message_id: String,
    /// Pasted images, in marker order ([`crate::attach`]).
    pub images: Vec<UserImage>,
    /// The process working directory (the workspace worktree).
    pub worktree: PathBuf,
    /// This agent's Recur MCP endpoint
    /// (`http://127.0.0.1:<port>/<token>/mcp`); empty when host tools are not
    /// served over MCP.
    pub mcp_url: String,
    /// Also wire the Playwright browser MCP server (QA role).
    pub playwright: bool,
    /// Extra MCP servers from Settings → MCP. Never log headers or env.
    pub user_mcp: Vec<McpAttach>,
    /// Environment variables removed from spawned CLIs (tracker secrets).
    pub env_scrub: Vec<String>,
    /// The provider session handle to continue; empty starts fresh.
    pub resume_session: String,
    /// With `resume_session`, ask for a provider-side fork.
    pub fork_session: bool,
    /// Put the prompt on stdin so a large fork transcript cannot blow
    /// ARG_MAX.
    pub prompt_on_stdin: bool,
    /// How the turn is keyed in the session store and on the bus.
    pub route: SessionRoute,
    /// Stream text/thinking deltas.
    pub include_partials: bool,
    /// Skip reasoning (cheap one-shots).
    pub skip_thinking: bool,
    /// Cap tool-loop rounds; 0 = provider default (unlimited).
    pub max_turns: u32,
    /// The Recur transcript session this turn mirrors into
    /// ([`crate::HistoryMode::App`] providers rebuild input from it).
    pub recur_session_id: String,
    /// Drop transcript messages through this id (inclusive) when rebuilding
    /// input; set from a fresh_session cutoff.
    pub history_after_id: String,
}

/// Keys a turn in the session store and on `session.*` bus events. An empty
/// `workspace_id` with a `kind` is an app-scoped session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionRoute {
    pub workspace_id: String,
    pub kind: String,
    pub code: String,
    pub name: String,
}

/// One extra MCP server for a turn (`name` is the mcp config key).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpAttach {
    pub name: String,
    pub url: String,
    pub headers: std::collections::BTreeMap<String, String>,
    pub command: String,
    pub args: Vec<String>,
    pub env: std::collections::BTreeMap<String, String>,
}

/// Whether role and `append_system_prompt` notes ride in the user message.
/// Claude uses CLI flags instead; Cursor has no system prompt API and Codex
/// needs the append notes in-band on a fresh thread. `resumed` is true only
/// when the turn actually continued a live provider session: a failed resume
/// that fell back to a new one must re-attach the notes.
pub fn standing_in_user_prompt(req: &TurnRequest, resumed: bool) -> bool {
    req.refresh_standing || !resumed
}

/// The channels a provider reports through during [`crate::Provider::run_turn`].
pub struct TurnContext {
    pub cancel: CancellationToken,
    pub sink: Arc<dyn EventSink>,
    /// Present iff the provider steers live and the engine armed steering.
    pub steer: Option<SteerInbox>,
    /// Absent when nobody can answer (approvals auto-resolve).
    pub permissions: Option<Arc<dyn PermissionAsker>>,
    /// The Recur transcript, for [`crate::HistoryMode::App`] providers.
    pub history: Option<Arc<dyn HistorySource>>,
    /// Host and coding tools, for [`crate::ToolTransport::InProcess`]
    /// providers.
    pub tools: Option<Arc<dyn ToolHost>>,
}

impl TurnContext {
    pub fn new(cancel: CancellationToken, sink: Arc<dyn EventSink>) -> Self {
        Self {
            cancel,
            sink,
            steer: None,
            permissions: None,
            history: None,
            tools: None,
        }
    }
}

/// Receives a turn's events synchronously and in order; never drops. The
/// engine's sink runs mirror (store write), then session broadcast, then the
/// caller's callback.
pub trait EventSink: Send + Sync {
    fn emit(&self, ev: AgentEvent);
}

impl<F: Fn(AgentEvent) + Send + Sync> EventSink for F {
    fn emit(&self, ev: AgentEvent) {
        self(ev)
    }
}

/// Mid-turn steers for a running turn (D49).
pub struct SteerInbox(pub mpsc::Receiver<Steer>);

impl SteerInbox {
    /// The next steer, waiting for one.
    pub async fn recv(&mut self) -> Option<Steer> {
        self.0.recv().await
    }

    /// The next queued steer, without waiting.
    pub fn try_next(&mut self) -> Option<Steer> {
        self.0.try_recv().ok()
    }

    /// Every queued steer with a non-blank prompt; blank ones are nacked.
    pub fn drain(&mut self) -> Vec<Steer> {
        let mut out = Vec::new();
        while let Some(s) = self.try_next() {
            if s.prompt.trim().is_empty()
                && s.display.as_deref().is_none_or(|d| d.trim().is_empty())
            {
                s.ack(false);
            } else {
                out.push(s);
            }
        }
        out
    }
}

/// One steer. `display` set means a "plain" steer (workspace chat follow-up):
/// the composer text goes to the model as is, shows as a user row, and is
/// acked only once really delivered. `None` is a conductor steer: it gets the
/// Maestro prefix and is acked on receipt.
#[derive(Debug)]
pub struct Steer {
    pub prompt: String,
    pub display: Option<String>,
    /// `true` = delivered into the running turn. Dropping the steer unacked
    /// answers `false` and the engine falls back to a new turn.
    pub ack: oneshot::Sender<bool>,
}

impl Steer {
    /// A steer and the receiver its delivery answer arrives on.
    pub fn new(
        prompt: impl Into<String>,
        display: Option<String>,
    ) -> (Self, oneshot::Receiver<bool>) {
        let (ack, rx) = oneshot::channel();
        (
            Self {
                prompt: prompt.into(),
                display,
                ack,
            },
            rx,
        )
    }

    pub fn is_plain(&self) -> bool {
        self.display.is_some()
    }

    /// The trimmed model prompt and UI text of a plain steer. A blank prompt
    /// falls back to the display text; a blank display to the prompt.
    pub fn plain_parts(&self) -> (String, String) {
        let prompt = self.prompt.trim();
        let display = self.display.as_deref().unwrap_or_default().trim();
        if prompt.is_empty() {
            return (display.to_owned(), display.to_owned());
        }
        let display = if display.is_empty() { prompt } else { display };
        (prompt.to_owned(), display.to_owned())
    }

    /// The user-message body written into the running turn: the composer text
    /// for a plain steer, the Maestro-prefixed summary otherwise.
    pub fn stdin_text(&self) -> String {
        if self.is_plain() {
            return self.plain_parts().0;
        }
        let msg = self.prompt.trim();
        if msg.is_empty() {
            return String::new();
        }
        format_steer_user_message(msg)
    }

    /// The user row for a delivered plain steer; `None` for conductor steers
    /// and blank ones. `live_steer` is false when delivery started a new
    /// provider run.
    pub fn user_event(&self, live_steer: bool) -> Option<AgentEvent> {
        if !self.is_plain() {
            return None;
        }
        let (text, display) = self.plain_parts();
        if text.is_empty() {
            return None;
        }
        let display = if display == text {
            String::new()
        } else {
            display
        };
        Some(AgentEvent::User {
            text,
            display,
            live_steer,
        })
    }

    /// Answers the engine; a closed receiver is fine.
    pub fn ack(self, delivered: bool) {
        let _ = self.ack.send(delivered);
    }
}

/// The canonical steer text, used as a new agent's prompt suffix and as an
/// in-flight user message.
pub fn format_steer_user_message(summary: &str) -> String {
    format!("Maestro steer (follow this; it supersedes earlier plan guidance):\n{summary}")
}

/// One follow-up user message for a batch of steers (Cursor can only Send
/// once after CancelRun). Plain steers join their prompts; conductor steers
/// share one Maestro prefix.
pub fn follow_up_text(steers: &[Steer]) -> String {
    let parts: Vec<String> = if steers.iter().all(Steer::is_plain) {
        steers.iter().map(|s| s.plain_parts().0).collect()
    } else {
        steers.iter().map(|s| s.prompt.trim().to_owned()).collect()
    };
    let joined = parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if joined.is_empty() || steers.iter().all(Steer::is_plain) {
        return joined;
    }
    format_steer_user_message(&joined)
}

/// Asks the human about a tool call (Codex approvals). Returns promptly when
/// the turn is cancelled.
#[async_trait::async_trait]
pub trait PermissionAsker: Send + Sync {
    /// Blocks until the human answers, the turn ends, or the broker's timeout
    /// denies. `Err` means the request was abandoned (the turn went away).
    async fn ask(&self, req: PermissionRequest) -> Result<PermissionDecision, PermissionAbandoned>;
}

/// The asking turn ended before an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("permission request abandoned")]
pub struct PermissionAbandoned;

/// The Recur transcript for providers that rebuild history each turn.
#[async_trait::async_trait]
pub trait HistorySource: Send + Sync {
    /// The session's messages in order; empty when unavailable.
    async fn transcript(&self, session_id: &str) -> Vec<Message>;
    /// The session's fresh_session cutoff (`nativeHistoryAfter`); empty when
    /// none.
    async fn history_after(&self, session_id: &str) -> String;
}

/// Host and coding tools for in-process providers. Bound to one turn.
#[async_trait::async_trait]
pub trait ToolHost: Send + Sync {
    /// The registry filtered by the turn's allowlist, `permission_request`
    /// removed, under the names the model should call.
    fn tools(&self, allowed: Option<&[String]>) -> Vec<ToolSpec>;
    /// Runs one call. Unknown and disallowed tools come back as error
    /// outcomes with fixed wording.
    async fn call(&self, name: &str, args: &str, meta: ToolCallMeta) -> ToolOutcome;
}

/// One advertised tool.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// The JSON schema of the arguments.
    pub schema: Value,
}

#[derive(Debug, Clone)]
pub struct ToolCallMeta {
    /// The provider's call id.
    pub call_id: String,
    pub cancel: CancellationToken,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolOutcome {
    pub text: String,
    pub is_error: bool,
    /// The tool ends the turn after this call (ask_user, a remounting
    /// switch_mode).
    pub halt: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standing_in_user_prompt_cases() {
        let resume = TurnRequest {
            resume_session: "s".into(),
            ..Default::default()
        };
        assert!(
            standing_in_user_prompt(&TurnRequest::default(), false),
            "fresh turn"
        );
        assert!(!standing_in_user_prompt(&resume, true), "ordinary resume");
        assert!(
            standing_in_user_prompt(&resume, false),
            "failed resume fallback"
        );
        let remount = TurnRequest {
            refresh_standing: true,
            ..resume
        };
        assert!(standing_in_user_prompt(&remount, true), "mode remount");
    }

    #[test]
    fn steer_text() {
        let (plain, _) = Steer::new(" do it ", Some("Do it".into()));
        assert_eq!(plain.stdin_text(), "do it");
        assert_eq!(
            plain.user_event(true),
            Some(AgentEvent::User {
                text: "do it".into(),
                display: "Do it".into(),
                live_steer: true
            })
        );
        let (same, _) = Steer::new("x", Some(String::new()));
        assert_eq!(same.plain_parts(), ("x".into(), "x".into()));
        assert!(
            matches!(same.user_event(false), Some(AgentEvent::User { ref display, .. }) if display.is_empty())
        );
        let (blank, _) = Steer::new(" ", Some("shown".into()));
        assert_eq!(blank.plain_parts(), ("shown".into(), "shown".into()));
        let (conductor, _) = Steer::new(" tighten ", None);
        assert_eq!(
            conductor.stdin_text(),
            "Maestro steer (follow this; it supersedes earlier plan guidance):\ntighten"
        );
        assert_eq!(conductor.user_event(true), None);
        assert_eq!(follow_up_text(&[plain, same]), "do it\n\nx");
        let (a, _) = Steer::new("a", None);
        let (b, _) = Steer::new(" ", None);
        let (c, _) = Steer::new("c", None);
        assert_eq!(
            follow_up_text(&[a, b, c]),
            format_steer_user_message("a\n\nc")
        );
        assert_eq!(follow_up_text(&[]), "");
    }

    #[tokio::test]
    async fn steer_ack_and_drop() {
        let (tx, rx) = mpsc::channel(4);
        let mut inbox = SteerInbox(rx);
        let (s1, a1) = Steer::new("one", None);
        let (s2, a2) = Steer::new("  ", None);
        let (s3, a3) = Steer::new("three", None);
        for s in [s1, s2, s3] {
            tx.send(s).await.unwrap();
        }
        let got = inbox.drain();
        assert_eq!(got.len(), 2);
        assert!(!a2.await.unwrap());
        let mut got = got.into_iter();
        got.next().unwrap().ack(true);
        drop(got);
        assert!(a1.await.unwrap());
        assert!(
            a3.await.is_err(),
            "dropped unacked steer reads as not delivered"
        );
    }
}
