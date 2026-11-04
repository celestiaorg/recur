//! Loops: the walk task that drives one run through its `[[loop]]`
//! graph under its `[[policy]]` guard, converge rounds and the rails, the board
//! and fan-out, handoff and replanning, browser verify, escalations, resume and
//! restart, the Maestro conductor and its hands, loop engage from a chat, and
//! the PR a converged walk opens.
//!
//! One task per walk. [`Walk`] owns the walk-local state; everything an MCP
//! callback or the UI also reads or writes lives in the walk's [`LoopHandle`]
//! (see `loop_handle`).

pub mod board;
pub mod converge;
pub mod engage;
pub mod escalation;
pub mod failure;
pub mod fanout;
pub mod handoff;
pub mod hands;
pub mod lifecycle;
pub mod maestro;
pub mod open_pr;
pub mod prompts;
pub mod rails;
pub mod resume;
pub mod specialists;
pub mod verify;
pub mod walker;
pub mod wrap_up;

#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use recur_agent::{ProviderError, TurnRequest, error::ErrorKind};
use recur_bundles::{Catalog, LoopPhase};
use recur_domain::{AutonomyLevel, FeedItem, Workspace, WorkspaceState};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::guard::ResolvedGuard;
use crate::loop_handle::{LoopHandle, LoopState, RoleView};
use crate::roles::{RoleSpec, model_for_slot};
use crate::text::{go_parse_duration, html_escape};
use failure::{LoopError, transient_failure};
use resume::{ResumePoint, resume_point_for};

pub use failure::TurnFailure;

/// The walker's "keep walking" outcome of a phase.
pub const STATE_ADVANCE: &str = "advance";

/// The error text of a turn cut by its deadline.
pub const DEADLINE_EXCEEDED: &str = "context deadline exceeded";
/// The error text of a cancelled turn.
pub const CONTEXT_CANCELED: &str = "context canceled";

/// One running walk (the state its task owns).
pub struct Walk {
    pub(crate) eng: Arc<Engine>,
    pub(crate) h: Arc<LoopHandle>,
    pub(crate) id: String,
    pub(crate) project_id: String,
    pub(crate) goal: String,
    /// Short UI label when `goal` is prompt-action wire.
    pub(crate) goal_display: String,
    /// Dispatch snapshot; the live base is the workspace row's.
    pub(crate) base_branch: String,
    pub(crate) worktree: String,
    pub(crate) autonomy: AutonomyLevel,
    pub(crate) loop_id: String,
    pub(crate) policy_id: String,
    pub(crate) guard: Arc<ResolvedGuard>,
    /// Ignore a sibling chat's checkpoint; start at the `[[loop]]` entry.
    pub(crate) fresh_walk: bool,
    /// simplify: additional-walk reenter; per-chat checkpoints when two
    /// walks need independent resume.
    pub(crate) start_phase: String,
    /// resolveGuard's refusal: idle honestly on missing refs.
    pub(crate) missing_guard: String,
    pub(crate) on_demand_walk: bool,
    pub(crate) resume_phase_id: String,
    /// The bundle catalog snapshotted for the run.
    pub(crate) catalog: Catalog,
    /// This walk's own worktree manager, bound to its project's repo (D22).
    pub(crate) git: Option<recur_git::Manager>,
    /// The planning turn's brief (or the persisted DefinitionOfDone).
    pub(crate) plan: String,
    /// The run's plan carries structured steps.
    pub(crate) plan_structured: bool,
    /// Where a re-dispatched walk re-enters (D55).
    pub(crate) resume: Option<ResumePoint>,
    /// Parent steps merged since the board last started a wave.
    pub(crate) board_unblocked: Vec<String>,
    /// This landing's implementer notes (replan).
    pub(crate) last_handoff: String,
    /// Child notes accumulated within one board pass.
    pub(crate) board_handoff: String,
    /// Paths a child-branch merge left conflicted; the next coder turn
    /// resolves them.
    pub(crate) pending_conflicts: Vec<String>,
    /// A child workspace working one plan step for its parent.
    pub(crate) parent_id: String,
    pub(crate) started_at: Option<Instant>,
    /// The board's event budget (tests shrink it).
    pub(crate) board_max_rounds: usize,
}

/// A phase/policy-timeout scope around one specialist turn:
/// a child token cancelled by the parent or the deadline.
pub(crate) struct TurnScope {
    pub token: CancellationToken,
    timed_out: Arc<AtomicBool>,
    #[cfg_attr(not(test), allow(dead_code))]
    deadline: Option<Instant>,
    timer: Option<JoinHandle<()>>,
}

impl TurnScope {
    pub fn new(parent: &CancellationToken, timeout: Option<Duration>) -> Self {
        let token = parent.child_token();
        let timed_out = Arc::new(AtomicBool::new(false));
        let (deadline, timer) = match timeout {
            Some(d) => {
                let t = token.clone();
                let flag = Arc::clone(&timed_out);
                let h = tokio::spawn(async move {
                    tokio::time::sleep(d).await;
                    flag.store(true, Ordering::SeqCst);
                    t.cancel();
                });
                (Some(Instant::now() + d), Some(h))
            }
            None => (None, None),
        };
        Self {
            token,
            timed_out,
            deadline,
            timer,
        }
    }

    #[cfg(test)]
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub fn timed_out(&self) -> bool {
        self.timed_out.load(Ordering::SeqCst)
    }

    /// Why the turn scope ended: deadline, cancel, or `None` while live.
    pub fn err(&self) -> Option<LoopError> {
        if self.timed_out() {
            Some(LoopError::msg(DEADLINE_EXCEEDED))
        } else if self.token.is_cancelled() {
            Some(LoopError::msg(CONTEXT_CANCELED))
        } else {
            None
        }
    }

    /// A provider error for the scope: a run cut by the deadline is
    /// a deadline-exceeded error, not a plain cancel.
    pub fn provider_err(&self, e: ProviderError) -> ProviderError {
        if self.timed_out() && e.kind == ErrorKind::Canceled {
            let partial = e.partial;
            let mut out = ProviderError::new(ErrorKind::Failed, DEADLINE_EXCEEDED);
            out.partial = partial;
            return out;
        }
        e
    }
}

impl Drop for TurnScope {
    fn drop(&mut self) {
        if let Some(t) = self.timer.take() {
            t.abort();
        }
    }
}

/// The parts of a walk the settled wrap-up needs after the task returned.
#[derive(Debug, Clone, Default)]
pub struct WrapUpInputs {
    pub id: String,
    pub goal: String,
    pub worktree: String,
    pub parent_id: String,
    pub review_handoff: String,
    pub started_at: Option<Instant>,
}

impl Walk {
    /// Builds a walk from a durable record: the chat-scoped
    /// goal, the resume point, the resolved guard (or the reason it cannot
    /// resolve), the catalog snapshot, the checkpoint's phase and a persisted
    /// escalation answer to apply. The handle's map key is set at dispatch
    /// ([`Walk::rekey`]).
    pub(crate) fn new(eng: &Arc<Engine>, l: &Workspace) -> Walk {
        let (mut goal, mut display) = (l.goal.clone(), l.goal_display.clone());
        let code = l.loop_chat_code.trim();
        if !code.is_empty() {
            let (g, d) = eng.chat_loop_goal(&l.id, code);
            if !g.is_empty() {
                goal = g;
                display = d;
            }
        }
        let resume = resume_point_for(l, eng.agent_sessions(&l.id));
        let mut loop_id = l.loop_id.clone();
        let mut policy_id = l.policy_id.clone();
        let (guard, missing_guard) = match eng.guard_for_workspace(l) {
            Ok(g) => {
                if loop_id.is_empty() {
                    loop_id = g.loop_def.id.clone();
                }
                if policy_id.is_empty() {
                    policy_id = g.policy.id.clone();
                }
                (g, String::new())
            }
            Err(reason) => (ResolvedGuard::default(), reason),
        };
        let catalog = eng.discover_loop_catalog().unwrap_or_default();
        let mut start_phase = String::new();
        let mut resume_phase_id = String::new();
        let mut on_demand_walk = false;
        if let Some(cp) = &l.checkpoint {
            let id = cp.phase_id.trim();
            if !id.is_empty() {
                start_phase = id.to_owned();
            }
            let rid = cp.resume_phase_id.trim();
            if !rid.is_empty() {
                resume_phase_id = rid.to_owned();
            }
            if guard
                .loop_def
                .phase_by_id(id)
                .is_some_and(|ph| ph.dispatch == "on_demand")
            {
                on_demand_walk = true;
            }
        }
        let answer = eng.store.escalation(&l.id).and_then(|esc| {
            if escalation::should_seed_escalation_answer(l, &esc) {
                escalation::restore_escalation_answer(&esc)
            } else {
                None
            }
        });
        let guard = Arc::new(guard);
        let phase_roles = l
            .checkpoint
            .as_ref()
            .and_then(|cp| guard.loop_def.phase_by_id(cp.phase_id.trim()))
            .map(|ph| ph.roles.clone());
        let chat_code = l.loop_chat_code.trim().to_owned();
        let state = LoopState {
            chat_code,
            branch: l.branch.clone(),
            worktree: l.worktree_path.clone(),
            answer,
            walk_iter: l.iteration,
            goal: goal.clone(),
            autonomy: l.autonomy.clone(),
            guard: Arc::clone(&guard),
            roles: RoleView {
                loop_roles: guard.loop_def.roles.clone(),
                phase_roles,
                catalog_roles: Arc::new(catalog.roles.clone()),
            },
            ..LoopState::default()
        };
        let h = LoopHandle::new(&l.id, &l.project_id, &l.id, state);
        let handoff = l.handoff.trim().to_owned();
        Walk {
            eng: Arc::clone(eng),
            h,
            id: l.id.clone(),
            project_id: l.project_id.clone(),
            goal,
            goal_display: display,
            base_branch: l.base_branch.clone(),
            worktree: l.worktree_path.clone(),
            autonomy: l.autonomy.clone(),
            loop_id,
            policy_id,
            guard,
            fresh_walk: false,
            start_phase,
            missing_guard,
            on_demand_walk,
            resume_phase_id,
            catalog,
            git: None,
            plan: l.definition_of_done.clone(),
            plan_structured: l.plan.as_ref().is_some_and(|p| !p.steps.is_empty()),
            resume,
            board_unblocked: Vec::new(),
            last_handoff: handoff.clone(),
            board_handoff: handoff,
            pending_conflicts: Vec::new(),
            parent_id: l.parent_id.trim().to_owned(),
            started_at: Some(Instant::now()),
            board_max_rounds: board::BOARD_MAX_ROUNDS,
        }
    }

    /// A bare walk over a workspace id.
    #[cfg(test)]
    pub(crate) fn bare(eng: &Arc<Engine>, id: &str) -> Walk {
        let mut w = Walk::new(
            eng,
            &Workspace {
                id: id.to_owned(),
                ..Workspace::default()
            },
        );
        w.missing_guard.clear();
        w.guard = Arc::new(ResolvedGuard::default());
        w.h.lock().guard = Arc::clone(&w.guard);
        w.started_at = None;
        w
    }

    /// Replaces the guard (tests build walks over synthetic graphs).
    #[cfg(test)]
    pub(crate) fn set_guard(&mut self, g: ResolvedGuard) {
        self.guard = Arc::new(g);
        let mut s = self.h.lock();
        s.guard = Arc::clone(&self.guard);
        s.roles.loop_roles = self.guard.loop_def.roles.clone();
    }

    /// Re-creates the handle under its registry key, keeping the state.
    pub(crate) fn rekey(&mut self, map_key: &str) {
        let st = std::mem::take(&mut *self.h.lock());
        self.h = LoopHandle::new(&self.id, &self.project_id, map_key, st);
    }

    pub(crate) fn chat_code(&self) -> String {
        self.h.chat_code()
    }

    pub(crate) fn branch(&self) -> String {
        self.h.branch()
    }

    /// The short UI string for this walk's goal.
    pub(crate) fn goal_label(&self) -> String {
        match self.goal_display.trim() {
            "" => self.goal.clone(),
            s => s.to_owned(),
        }
    }

    /// The inputs of the Maestro wrap-up once the walk returned.
    pub(crate) fn wrap_up_inputs(&self) -> WrapUpInputs {
        WrapUpInputs {
            id: self.id.clone(),
            goal: self.goal.clone(),
            worktree: self.worktree.clone(),
            parent_id: self.parent_id.clone(),
            review_handoff: self.review_handoff(),
            started_at: self.started_at,
        }
    }

    // ---- emit helpers (write through the engine so events persist + fan out)

    /// Records this walk's state (walk-local exit + the workspace row).
    pub(crate) fn set_state(&self, state: WorkspaceState, phase: &str) {
        {
            let mut s = self.h.lock();
            s.walk_state = state.clone();
            s.exit_state = state.clone();
            s.exit_phase = phase.to_owned();
        }
        self.eng.set_workspace_state(&self.id, state, phase);
    }

    pub(crate) fn emit_feed(&self, code: &str, name: &str, text_html: &str) {
        self.eng.emit_workspace_event_item(
            &self.id,
            FeedItem {
                agent_code: code.to_owned(),
                name: name.to_owned(),
                text: text_html.to_owned(),
                chat_code: self.chat_code(),
                ..FeedItem::default()
            },
        );
    }

    /// Records the current iteration on the row and its checkpoint (D55).
    pub(crate) fn set_iteration(&self, iter: i64) {
        self.h.lock().walk_iter = iter;
        self.eng.store.update_workspace(&self.id, |lp| {
            lp.iteration = iter;
            if let Some(cp) = lp.checkpoint.as_mut() {
                cp.iteration = iter;
            }
        });
    }

    /// The phase the walker is inside now, else the checkpointed one.
    pub(crate) fn walk_phase(&self) -> Option<LoopPhase> {
        let cur = self.h.lock().current_phase.clone();
        if !cur.id.trim().is_empty() {
            return Some(cur);
        }
        let ws = self.eng.store.workspace(&self.id)?;
        let id = ws.checkpoint.as_ref()?.phase_id.trim().to_owned();
        if id.is_empty() {
            return None;
        }
        self.guard.loop_def.phase_by_id(&id).cloned()
    }

    /// Sets the phase the walker is inside (and the role view done/finding
    /// attribution reads).
    pub(crate) fn set_current_phase(&self, ph: &LoopPhase) {
        let mut s = self.h.lock();
        s.current_phase = ph.clone();
        s.roles.phase_roles = Some(ph.roles.clone());
    }

    /// The catalog snapshot, discovered on first use when empty.
    pub(crate) fn loop_catalog(&mut self) -> Catalog {
        if !self.catalog.roles.is_empty()
            || !self.catalog.loops.is_empty()
            || !self.catalog.policies.is_empty()
        {
            return self.catalog.clone();
        }
        let Ok(cat) = self.eng.discover_loop_catalog() else {
            return Catalog::default();
        };
        self.catalog = cat.clone();
        self.h.lock().roles.catalog_roles = Arc::new(cat.roles.clone());
        cat
    }

    /// The role spec a phase slot resolves to.
    pub(crate) fn slot_role_spec(&mut self, slot: &str, fallback: RoleSpec) -> RoleSpec {
        let id = self.phase_role_id(slot);
        if id.is_empty() {
            return fallback;
        }
        let cat = self.loop_catalog();
        let Some(role) = recur_bundles::find_role(&cat.roles, &id) else {
            return fallback;
        };
        let mut spec = fallback;
        if !role.code.is_empty() {
            spec.code = role.code.clone();
        }
        if !role.name.is_empty() {
            spec.name = role.name.clone();
        }
        spec.allowed_tools = crate::roles::role_allowlist(role);
        spec.model_slot = role.model_slot.clone();
        spec.mode = role.mode.clone();
        spec
    }

    /// The Settings model + flags for a role's slot.
    pub(crate) fn role_model(
        &self,
        rs: &RoleSpec,
        fallback_model: &str,
        fallback_flags: &BTreeMap<String, String>,
    ) -> (String, BTreeMap<String, String>) {
        model_for_slot(&self.eng.settings(), rs, fallback_model, fallback_flags)
    }

    /// Appends the pending Maestro steer to a NEW agent's prompt; the
    /// restored answer it carried is now consumed.
    pub(crate) fn apply_pending_steer(&self, prompt: &str) -> String {
        let steer = self.h.lock().pending_steer.clone();
        if steer.is_empty() {
            return prompt.to_owned();
        }
        self.eng.store.mark_escalation_applied(&self.id);
        format!(
            "{prompt}\n\n{}",
            recur_agent::turn::format_steer_user_message(&steer)
        )
    }

    /// Attaches the opening user pastes to a specialist turn.
    pub(crate) fn apply_goal_images(&self, req: &mut TurnRequest) {
        let imgs = self.eng.loop_goal_images(&self.id, &self.chat_code());
        if imgs.is_empty() {
            return;
        }
        req.prompt = recur_agent::attach::ensure_markers(&req.prompt, &imgs);
        req.images = imgs;
    }

    /// The phase/policy timeout around one specialist turn.
    pub(crate) fn turn_scope(&self, cancel: &CancellationToken) -> TurnScope {
        let ph = self.h.lock().current_phase.clone();
        TurnScope::new(cancel, self.phase_timeout(&ph))
    }

    pub(crate) fn phase_timeout(&self, ph: &LoopPhase) -> Option<Duration> {
        let mut raw = ph.timeout.trim();
        if raw.is_empty() {
            raw = self.guard.policy.timeout.trim();
        }
        if raw.is_empty() {
            return None;
        }
        match go_parse_duration(raw) {
            Ok(ns) if ns > 0 => Some(Duration::from_nanos(ns as u64)),
            _ => None,
        }
    }

    // ---- checkpoints, pause, stop

    /// A safe point between phases: drains commands, services a pending
    /// agent escalation (blocking until answered), and waits while paused.
    /// False when the walk should stop.
    pub(crate) async fn checkpoint(&mut self, cancel: &CancellationToken) -> bool {
        self.h.drain_commands();
        if let Some(req) = self.h.take_pending_escalation() {
            let (ans, keep_going) = self.escalate(cancel, req, escalation::ESC_KIND_AGENT).await;
            if !keep_going {
                return false;
            }
            self.steer_agent_answer(&ans);
        }
        if !self.h.is_paused() {
            return !self.h.is_stopped() && !cancel.is_cancelled();
        }
        self.set_state(WorkspaceState::Paused, "paused");
        self.h.wait_resumed(cancel).await
    }

    /// True when the walk exits because the app is quitting, not a Stop:
    /// the persisted state is left for Recover (a Pause accepted but not yet
    /// checkpointed is recorded first).
    pub(crate) fn leave_state_on_shutdown(&self) -> bool {
        // Same lock Stop holds when accepting a running stop.
        let t = self.eng.teardown.lock();
        if !*t {
            return false;
        }
        let (stopped, paused, escalated) = {
            let s = self.h.lock();
            (s.stopped, s.paused, s.escalated)
        };
        if stopped {
            return false;
        }
        if paused && !escalated {
            self.set_state(WorkspaceState::Paused, "paused");
        }
        true
    }

    /// Records a Stop/cancel terminal state.
    pub(crate) fn finish_stopped(&self) {
        if self.leave_state_on_shutdown() {
            return;
        }
        self.set_state(WorkspaceState::Stopped, "stopped");
        self.emit_feed("MA", "Maestro", "Loop stopped.");
    }

    /// Parks idle when composer Stop asked for it; otherwise Stop.
    pub(crate) fn finish_walk_abort(&self) {
        if self.h.wants_idle() {
            if let Some(ws) = self.eng.store.workspace(&self.id)
                && ws.state == WorkspaceState::Paused
                && ws.phase == "idle"
            {
                return;
            }
            self.set_state(WorkspaceState::Paused, "idle");
            self.emit_feed("MA", "Maestro", "Walk cancelled.");
            return;
        }
        self.finish_stopped();
    }

    /// Records a failed walk. A cancelled walk is a stop; a transient
    /// provider failure is retryable (D56); anything else is terminal.
    pub(crate) fn fail(&self, cancel: &CancellationToken, err: &LoopError) {
        if cancel.is_cancelled() {
            self.finish_walk_abort();
            return;
        }
        let (reason, transient) = transient_failure(err);
        if transient {
            self.eng.fail_retryable(&self.id, &reason);
            return;
        }
        self.eng.record_failure(&self.id, &err.to_string(), false);
        self.set_state(WorkspaceState::Failed, "failed");
        self.emit_feed(
            "MA",
            "Maestro",
            &format!("Loop failed: {}", html_escape(&err.to_string())),
        );
    }
}
