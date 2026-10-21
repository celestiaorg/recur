//! The turn pipeline (E2): the provider router every agent turn goes
//! through (Go `provider.go` `runAgent` / `runnerFor`), the session mirror
//! and live broadcast that wrap every turn (Go `mirroringRunner`), per-turn
//! MCP registration, and the user-visible turn outcome.
//!
//! Routing happens in exactly one place: the qualified model ref
//! (`provider/id`) picks the provider; the provider gets the bare id. One
//! wrap point mirrors every provider's events, so no turn can bypass the
//! transcript or the live stream.

pub mod broadcast;
pub mod checkpoint;
pub mod finalize;
pub mod mirror;
pub mod oneshot;
pub mod sessions;
pub mod stream;
#[cfg(test)]
pub(crate) mod test_support;
pub mod tools;
pub mod transcript;

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use recur_agent::error::ErrorKind;
use recur_agent::{
    AgentEvent, Availability, EventSink, Flags, HistoryMode, HistorySource, PermissionAbandoned,
    PermissionAsker, PermissionRoute, Provider, ProviderError, SteerInbox, ToolTransport,
    TurnContext, TurnRequest, TurnResult,
};
use recur_domain::{Message, PermissionDecision, PermissionRequest, parse_model_ref};
use tokio_util::sync::CancellationToken;

use crate::bus::SessionRoute as BusRoute;
use crate::composer::skills::{apply_comment_hygiene, apply_skill_guidance};
use crate::host::EngineHost;
use crate::mcp_attach::tracker_mode_for;
use crate::session_config::AgentConfig;
use crate::{Engine, EngineError};
use broadcast::SessionBroadcast;
use mirror::{SessionMirror, resolve_session_user_message_id, user_bubble_text};

/// A caller's view of a turn's events, after the mirror and the broadcast.
pub type Emit = Arc<dyn Fn(&AgentEvent) + Send + Sync>;

/// One agent turn as the engine dispatches it. `req.model` is the qualified
/// model ref until routing strips it to the provider's bare id.
#[derive(Default)]
pub struct AgentSpec {
    pub req: TurnRequest,
    /// The armed mid-turn steer channel (Go `Inject` / `InjectAck`).
    pub steer: Option<SteerInbox>,
    pub emit: Option<Emit>,
}

impl AgentSpec {
    pub fn new(req: TurnRequest) -> Self {
        Self {
            req,
            ..Self::default()
        }
    }
}

/// The Recur MCP server could not start, so agents could not report
/// findings, completion or escalations: the turn fails (transient; Retry can
/// succeed because the bind is retried).
pub const ERR_MCP_UNAVAILABLE: &str = "the Recur MCP server could not start, so agents have no way to report findings, completion, or escalations";

const COMPAT_TURN_PREFIX: &str = "compat\x00";

fn compat_turn_token(workspace_id: &str, agent_code: &str) -> String {
    format!("{COMPAT_TURN_PREFIX}{workspace_id}\x00{agent_code}")
}

fn parse_compat_turn_token(token: &str) -> Option<(&str, &str)> {
    token.strip_prefix(COMPAT_TURN_PREFIX)?.split_once('\x00')
}

/// The session config's fresh_session cutoff.
pub fn native_history_after_id(config_json: &str) -> String {
    AgentConfig::parse(config_json)
        .native_history_after
        .trim()
        .to_owned()
}

/// What a turn's events pass through: dedupe, context-window stamp, the
/// session mirror, the live broadcast, then the caller.
struct TurnSink {
    provider: Arc<dyn Provider>,
    model: String,
    flags: Flags,
    wrap: bool,
    seen_steps: Mutex<HashSet<String>>,
    mirror: Mutex<Option<SessionMirror>>,
    broadcast: Mutex<Option<SessionBroadcast>>,
    turn_end: AtomicBool,
    emit: Option<Emit>,
}

impl EventSink for TurnSink {
    fn emit(&self, mut ev: AgentEvent) {
        if !self.wrap {
            if let Some(f) = &self.emit {
                f(&ev);
            }
            return;
        }
        match &mut ev {
            // A follow-up that restarted the provider run may reuse call ids.
            AgentEvent::User {
                live_steer: false, ..
            } => self.seen_steps.lock().clear(),
            // Providers can re-emit the same tool_use on several lines; each
            // would mint a row and a live step.
            AgentEvent::Step(s)
                if !s.id.is_empty() && !self.seen_steps.lock().insert(s.id.clone()) =>
            {
                return;
            }
            // The active context limit, before the mirror and the broadcast.
            AgentEvent::TurnEnd(r) => {
                let w = self.provider.context_window(&self.model, &self.flags);
                if w > 0 {
                    r.usage.context_window = w;
                }
                if r.duration_ms > 0 {
                    self.turn_end.store(true, Ordering::SeqCst);
                }
            }
            AgentEvent::Usage(u) => {
                let w = self.provider.context_window(&self.model, &self.flags);
                if w > 0 {
                    u.context_window = w;
                }
            }
            _ => {}
        }
        if let Some(m) = self.mirror.lock().as_mut() {
            m.event(&ev);
        }
        if let Some(b) = self.broadcast.lock().as_mut() {
            b.emit(&ev);
        }
        if let Some(f) = &self.emit {
            f(&ev);
        }
    }
}

/// The permission prompt for callback providers (Codex approvals): fills
/// the surface and routes through the broker, abandoned when the turn ends.
struct TurnPermissions {
    eng: Arc<Engine>,
    cancel: CancellationToken,
}

#[async_trait::async_trait]
impl PermissionAsker for TurnPermissions {
    async fn ask(
        &self,
        mut req: PermissionRequest,
    ) -> Result<PermissionDecision, PermissionAbandoned> {
        if req.surface.is_empty() {
            req.surface = self
                .eng
                .permission_surface_for(&req.workspace_id, &req.agent_code)
                .to_owned();
        }
        self.eng.perms.request(&self.cancel, req).await
    }
}

/// The Recur transcript for providers that rebuild history each turn.
struct StoreHistory {
    eng: Arc<Engine>,
}

#[async_trait::async_trait]
impl HistorySource for StoreHistory {
    async fn transcript(&self, session_id: &str) -> Vec<Message> {
        self.eng
            .store
            .session_transcript(session_id)
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    async fn history_after(&self, session_id: &str) -> String {
        self.eng
            .store
            .get_session(session_id)
            .map(|s| native_history_after_id(&s.config_json))
            .unwrap_or_default()
    }
}

impl Engine {
    /// The provider a turn routes to. Never falls back to another provider:
    /// quietly running a different model than the one chosen is worse than
    /// a visible failure. A test pin wins (and skips availability).
    pub fn resolve_provider(&self, provider_id: &str) -> Result<Arc<dyn Provider>, ProviderError> {
        if self.providers.is_pinned() {
            return self.providers.for_model("").ok_or_else(|| {
                ProviderError::new(
                    ErrorKind::Unavailable,
                    "engine: agent runner was explicitly set to nil; no turn can run",
                )
            });
        }
        let Some(p) = self.providers.get(provider_id) else {
            return Err(ProviderError::new(
                ErrorKind::Unavailable,
                format!(
                    "unknown agent provider {} — pick a model from Settings → Models",
                    recur_bundles::go::quote(provider_id)
                ),
            ));
        };
        match p.availability(&self.settings()) {
            Availability::Ready => Ok(p),
            Availability::Unavailable(reason) => {
                Err(ProviderError::new(ErrorKind::Unavailable, reason))
            }
        }
    }

    /// The single choke point every agent turn goes through: skill and
    /// comment-hygiene overlays, Settings → MCP (real agent turns only),
    /// provider routing by the model ref, partials (except one-shots),
    /// tracker secrets scrubbed, then the mirrored run.
    pub async fn run_agent(
        &self,
        cancel: &CancellationToken,
        spec: AgentSpec,
    ) -> Result<TurnResult, ProviderError> {
        let AgentSpec {
            mut req,
            steer,
            emit,
        } = spec;
        apply_skill_guidance(&mut req);
        apply_comment_hygiene(&mut req);
        if !req.skip_thinking {
            let ws = req.workspace_id.clone();
            let mode = tracker_mode_for(&req);
            self.attach_workspace_user_mcp(&mut req, &ws, mode);
        }
        let r = parse_model_ref(&req.model);
        let provider = self.resolve_provider(&r.provider)?;
        req.model = r.id;
        if !req.skip_thinking {
            req.include_partials = true;
        }
        req.env_scrub = self.trackers.secret_env();
        self.run_mirrored(cancel, provider, &r.provider, req, steer, emit)
            .await
    }

    /// One turn under the session mirror and the live broadcast (Go
    /// `mirroringRunner.Run`).
    async fn run_mirrored(
        &self,
        cancel: &CancellationToken,
        provider: Arc<dyn Provider>,
        provider_id: &str,
        mut req: TurnRequest,
        steer: Option<SteerInbox>,
        emit: Option<Emit>,
    ) -> Result<TurnResult, ProviderError> {
        let mirror = self.attach_mirror(&mut req, provider_id);
        let broadcast = self.new_session_broadcast(&req, Arc::clone(&provider));
        if !req.skip_record_user
            && let Some(bc) = &broadcast
        {
            let shown = user_bubble_text(&req.prompt, &req.display_text);
            if !shown.is_empty() {
                bc.user(&shown);
            }
        }
        let sink = Arc::new(TurnSink {
            wrap: mirror.is_some() || broadcast.is_some(),
            provider: Arc::clone(&provider),
            model: req.model.clone(),
            flags: req.flags.clone(),
            seen_steps: Mutex::new(HashSet::new()),
            mirror: Mutex::new(mirror),
            broadcast: Mutex::new(broadcast),
            turn_end: AtomicBool::new(false),
            emit,
        });
        let caps = *provider.caps();
        let mut cx = TurnContext::new(cancel.clone(), sink.clone());
        cx.steer = steer;
        if caps.permission == PermissionRoute::Callback {
            cx.permissions = Some(Arc::new(TurnPermissions {
                eng: self.arc(),
                cancel: cancel.clone(),
            }));
        }
        if caps.history == HistoryMode::App {
            cx.history = Some(Arc::new(StoreHistory { eng: self.arc() }));
        }
        if caps.tool_transport == ToolTransport::InProcess {
            let host: Arc<dyn recur_agent_tools::Host> =
                Arc::new(EngineHost::new(Arc::downgrade(&self.arc())));
            cx.tools = Some(Arc::new(tools::InProcessTools::new(
                Some(host),
                Arc::clone(&self.todos),
                &req,
            )));
        }
        let out = provider.run_turn(req, cx).await;
        if let Some(m) = sink.mirror.lock().as_mut() {
            m.finish();
        }
        let emitted = sink.turn_end.load(Ordering::SeqCst);
        match out {
            Ok(mut res) => {
                res.turn_end_emitted |= emitted;
                Ok(res)
            }
            Err(mut e) => {
                e.partial.turn_end_emitted |= emitted;
                Err(e)
            }
        }
    }

    /// Ensures the durable session the turn drives, records the user row
    /// (unless Retry/remount), and returns its mirror. `None` without a
    /// route or a workspace role, or without a database.
    fn attach_mirror(&self, req: &mut TurnRequest, provider_id: &str) -> Option<SessionMirror> {
        let sess = if !req.route.kind.is_empty() {
            self.ensure_routed_session(req, provider_id)?
        } else if !req.workspace_id.is_empty() && !req.role.is_empty() {
            let (code, name) = self.agent_identity(req);
            self.ensure_owned_agent_session(
                &req.workspace_id,
                req.role.as_str(),
                &code,
                &name,
                provider_id,
                &req.chat_code,
            )?
        } else {
            return None;
        };
        let m = SessionMirror::new(Arc::clone(&self.store), &sess.id, &req.model, provider_id);
        req.recur_session_id = sess.id.clone();
        if req.history_after_id.is_empty() {
            req.history_after_id = native_history_after_id(&sess.config_json);
        }
        if !req.skip_record_user {
            let id = m.record_user(
                &req.user_message_id,
                &req.prompt,
                &req.display_text,
                &req.images,
            );
            if !id.is_empty() {
                req.user_message_id = id;
            }
        } else {
            req.user_message_id =
                resolve_session_user_message_id(&self.store, &sess.id, &req.user_message_id);
        }
        Some(m)
    }

    /// The live stream for a turn: routed turns by their route (app turns
    /// carry `surface: "app"`), workspace role turns by their agent code and
    /// conductor tab; one-shots get none.
    fn new_session_broadcast(
        &self,
        req: &TurnRequest,
        provider: Arc<dyn Provider>,
    ) -> Option<SessionBroadcast> {
        let route = if !req.route.kind.is_empty() {
            let code = if req.route.code.is_empty() {
                self.agent_identity(req).0
            } else {
                req.route.code.clone()
            };
            BusRoute {
                surface: if req.route.workspace_id.is_empty() {
                    "app".into()
                } else {
                    String::new()
                },
                workspace_id: req.route.workspace_id.clone(),
                code,
                owner_code: String::new(),
            }
        } else if !req.workspace_id.is_empty() && !req.role.is_empty() {
            BusRoute {
                surface: String::new(),
                workspace_id: req.workspace_id.clone(),
                code: self.agent_identity(req).0,
                owner_code: req.chat_code.clone(),
            }
        } else {
            return None;
        };
        Some(SessionBroadcast::new(self.arc(), route, provider))
    }

    /// Starts the MCP server if needed and returns the turn's token and URL.
    /// Sequential turns of one agent reuse the URL; an overlapping turn
    /// mints its own token. Only the Recur tools in the allowlist are
    /// advertised, plus `permission_request` for providers that prompt
    /// through it. In-process providers get a turn token (for clearing
    /// "remember for this turn" approvals) and no URL.
    ///
    /// A failure fails the turn: an agent without its structured-signal
    /// tools does not degrade, it lies (a Reviewer that cannot report
    /// findings reports none, and converge reads zero as success).
    pub async fn register_mcp(
        &self,
        workspace_id: &str,
        agent_code: &str,
        chat_code: &str,
        allowed_tools: &[String],
        model: &str,
    ) -> Result<(String, String), EngineError> {
        let provider = self.providers.for_model(model);
        let caps = provider.as_ref().map(|p| *p.caps());
        if caps.is_some_and(|c| c.tool_transport == ToolTransport::InProcess) {
            return Ok((compat_turn_token(workspace_id, agent_code), String::new()));
        }
        self.ensure_mcp()
            .await
            .map_err(|e| EngineError(format!("{ERR_MCP_UNAVAILABLE}: {e}")))?;
        let mut names = recur_mcp::names_from_allowed_tools(allowed_tools);
        if caps.is_some_and(|c| c.permission == PermissionRoute::McpPromptTool) {
            names = recur_mcp::append_permission_request(names);
        }
        // Always a list, so zero Recur tools scopes to nothing rather than
        // falling back to unrestricted.
        Ok(self
            .mcp
            .register_owned(workspace_id, agent_code, chat_code, Some(&names)))
    }

    /// Ends a turn's registration: its URL 404s until the next register and
    /// pending permission prompts resolve. Parked ask_user questions
    /// survive (they resume on answer).
    pub fn unregister_mcp(&self, token: &str) {
        if let Some((ws, code)) = parse_compat_turn_token(token) {
            self.perms.end_turn(ws, code);
            return;
        }
        if token.is_empty() {
            return;
        }
        if let Some((ws, code)) = self.mcp.unregister(token) {
            self.perms.end_turn(&ws, &code);
        }
    }
}

#[cfg(test)]
mod tests;
