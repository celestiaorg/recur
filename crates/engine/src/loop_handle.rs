//! The loop control plane (the state shared between a loop's
//! walk task and MCP / UI callers): commands, pause/stop/escalation flags,
//! the human's answer, pending agent escalations, Maestro steers, granted
//! extensions, done summaries, findings and verify evidence.
//!
//! These live in [`LoopState`] behind [`LoopHandle::lock`], and
//! [`LoopHandle::broadcast`] is the cond broadcast. `send` mutates state and
//! wakes waiters *before* queueing the command. The walk owns everything
//! else about a loop.
//!
//! The branch (and worktree, chat code) live here under the mutex, since
//! the fswatch task writes the branch while the walk reads it.

use std::sync::Arc;

use parking_lot::{Mutex, MutexGuard};
use recur_agent::{Steer, SteerInbox};
use recur_agent_tools::{EscalationRequest, Evidence};
use recur_bundles::{LoopPhase, Role};
use recur_domain::{AutonomyLevel, WorkspaceState};
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

use crate::guard::ResolvedGuard;
use crate::roles::{RoleSpec, slot_role_spec};

/// A control message to a running loop (PRD §6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Pause,
    Resume,
    Stop,
    /// Carries the human's answer; the blocked loop applies it.
    AnswerEscalation(EscalationAnswer),
}

/// The human's reply to an escalation (PRD §13). `resolved` is the final
/// single-line answer; `grant` > 0 is extra converge rounds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EscalationAnswer {
    pub choice: String,
    pub free_text: String,
    pub resolved: String,
    pub grant: i64,
}

/// The `[[loop]]` role slots and catalog a done/finding attribution reads.
#[derive(Debug, Clone, Default)]
pub struct RoleView {
    /// The guard loop's `roles`.
    pub loop_roles: std::collections::BTreeMap<String, String>,
    /// The phase the walk is in now, when it has role slots.
    pub phase_roles: Option<std::collections::BTreeMap<String, String>>,
    /// The bundle catalog's roles snapshotted for the run.
    pub catalog_roles: Arc<Vec<Role>>,
}

/// Everything a walk shares with its controllers.
#[derive(Debug, Default)]
pub struct LoopState {
    /// The conductor tab that owns this walk ("" = legacy MA-owned).
    pub chat_code: String,
    pub branch: String,
    pub worktree: String,

    pub paused: bool,
    pub stopped: bool,
    /// Composer Stop: unwind as idle, not StateStopped.
    pub park_idle: bool,
    pub escalated: bool,
    /// True only while the round-cap escalation waits for its answer.
    pub round_cap_esc: bool,
    pub answer: Option<EscalationAnswer>,
    /// An agent's escalate call, consumed at the next checkpoint. First wins.
    pub pending_esc: Option<EscalationRequest>,

    /// The latest within-rails Maestro re-plan.
    pub pending_steer: String,
    /// Extra fix iterations granted from chat, consumed at the next boundary.
    pub granted_extra: i64,
    /// The in-flight specialist's steer channel (set for one turn).
    pub active_inject: Option<mpsc::Sender<Steer>>,
    /// The in-flight specialist's code (CO/WR/…), for done routing.
    pub done_turn_code: String,
    /// This iteration's successful done summary (commit takes it).
    pub done_summary: String,
    /// Every successful done this walk, for the Maestro wrap-up.
    pub last_done_summary: String,
    pub done_notes: String,
    /// A queued run_phase, applied at a graph checkpoint.
    pub pending_on_demand: String,

    /// This walk's last setState (siblings overwrite the workspace row).
    pub exit_state: WorkspaceState,
    pub exit_phase: String,
    /// The walk's last setState / setIteration (the workspace row is shared
    /// with sibling walks). Empty before the first setState.
    pub walk_state: WorkspaceState,
    pub walk_iter: i64,
    /// The `[[loop.phase]]` the walk is inside right now.
    pub current_phase: LoopPhase,

    /// Fixed at dispatch: the run's goal, autonomy and resolved guard.
    pub goal: String,
    pub autonomy: AutonomyLevel,
    pub guard: Arc<ResolvedGuard>,

    pub roles: RoleView,
}

#[derive(Debug, Default)]
struct FindingTally {
    cur_iter: i64,
    iter_finds: i64,
}

/// One running walk's control plane.
pub struct LoopHandle {
    /// The workspace id.
    pub id: String,
    pub project_id: String,
    /// The `Engine::loops` key: the workspace id, or workspace + `\x1e` +
    /// chat when a second walk is live.
    pub map_key: String,
    state: Mutex<LoopState>,
    cond: Notify,
    cmd_tx: mpsc::Sender<Command>,
    cmd_rx: Mutex<mpsc::Receiver<Command>>,
    findings: Mutex<FindingTally>,
    evidence: Mutex<Vec<Evidence>>,
}

/// Buffered so a send never blocks.
const CMD_BUFFER: usize = 8;

/// The cap on an accumulated handoff note.
pub const HANDOFF_MAX_CHARS: usize = 4000;

/// Appends a handoff note, capped.
pub fn accumulate_handoff(prev: &str, next: &str) -> String {
    let next = crate::text::truncate(next.trim(), HANDOFF_MAX_CHARS);
    if next.is_empty() {
        return prev.to_owned();
    }
    if prev.is_empty() {
        return next;
    }
    crate::text::truncate(&format!("{prev}\n{next}"), HANDOFF_MAX_CHARS)
}

impl LoopHandle {
    pub fn new(
        id: impl Into<String>,
        project_id: impl Into<String>,
        map_key: impl Into<String>,
        state: LoopState,
    ) -> Arc<Self> {
        let (cmd_tx, cmd_rx) = mpsc::channel(CMD_BUFFER);
        Arc::new(Self {
            id: id.into(),
            project_id: project_id.into(),
            map_key: map_key.into(),
            state: Mutex::new(state),
            cond: Notify::new(),
            cmd_tx,
            cmd_rx: Mutex::new(cmd_rx),
            findings: Mutex::new(FindingTally::default()),
            evidence: Mutex::new(Vec::new()),
        })
    }

    /// The shared state. Never hold across an await.
    pub fn lock(&self) -> MutexGuard<'_, LoopState> {
        self.state.lock()
    }

    /// Wakes every waiter.
    pub fn broadcast(&self) {
        self.cond.notify_waiters();
    }

    /// Waits while `blocked(state)` holds, until it clears or `cancel`
    /// fires. State is re-checked after every wake.
    pub async fn wait_while(
        &self,
        cancel: &CancellationToken,
        blocked: impl Fn(&LoopState) -> bool,
    ) {
        loop {
            let notified = self.cond.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if cancel.is_cancelled() || !blocked(&self.state.lock()) {
                return;
            }
            tokio::select! {
                _ = &mut notified => {}
                _ = cancel.cancelled() => return,
            }
        }
    }

    /// Delivers a command (never blocks). Pause/Resume/Stop/Answer flip the
    /// shared flags and wake waiters first; Stop also wins over a pause or an
    /// escalation wait.
    pub fn send(&self, c: Command) {
        match &c {
            Command::Pause => self.state.lock().paused = true,
            Command::Resume => {
                self.state.lock().paused = false;
                self.broadcast();
            }
            Command::Stop => {
                let mut s = self.state.lock();
                s.stopped = true;
                s.paused = false;
                s.escalated = false;
                drop(s);
                self.broadcast();
            }
            Command::AnswerEscalation(ans) => {
                let mut s = self.state.lock();
                s.answer = Some(ans.clone());
                s.escalated = false;
                s.paused = false;
                drop(s);
                self.broadcast();
            }
        }
        let _ = self.cmd_tx.try_send(c);
    }

    /// Drains queued commands (their effect was applied in `send`).
    pub fn drain_commands(&self) {
        let mut rx = self.cmd_rx.lock();
        while rx.try_recv().is_ok() {}
    }

    pub fn is_paused(&self) -> bool {
        self.state.lock().paused
    }

    /// Waits while paused; true when the walk should keep going (not
    /// stopped, not cancelled). The paused-state publish is the walk's.
    pub async fn wait_resumed(&self, cancel: &CancellationToken) -> bool {
        self.wait_while(cancel, |s| s.paused && !s.stopped).await;
        !self.is_stopped() && !cancel.is_cancelled()
    }

    /// Marks the walk to park idle when cancelled (composer Stop).
    pub fn request_idle(&self) {
        self.state.lock().park_idle = true;
        self.broadcast();
    }

    pub fn wants_idle(&self) -> bool {
        self.state.lock().park_idle
    }

    pub fn is_stopped(&self) -> bool {
        self.state.lock().stopped
    }

    /// Records an agent's escalation for the next checkpoint. Ignored while
    /// one is pending or active (first wins).
    pub fn request_escalation(&self, req: EscalationRequest) {
        let mut s = self.state.lock();
        if s.pending_esc.is_some() || s.escalated {
            return;
        }
        s.pending_esc = Some(req);
    }

    pub fn take_pending_escalation(&self) -> Option<EscalationRequest> {
        self.state.lock().pending_esc.take()
    }

    pub fn has_pending_escalation(&self) -> bool {
        self.state.lock().pending_esc.is_some()
    }

    /// Credits extra fix iterations (already clamped by the engine).
    pub fn grant_extension(&self, n: i64) {
        self.state.lock().granted_extra += n;
    }

    pub fn take_granted_extension(&self) -> i64 {
        std::mem::take(&mut self.state.lock().granted_extra)
    }

    /// Records a Maestro re-plan: newly spawned specialists read it from
    /// `pending_steer`; the in-flight one gets it as a conductor steer on
    /// its next tool result. A full buffer drops the live copy.
    pub fn queue_steer(&self, summary: &str) {
        let summary = summary.trim();
        if summary.is_empty() {
            return;
        }
        let tx = {
            let mut s = self.state.lock();
            s.pending_steer = summary.to_owned();
            s.active_inject.clone()
        };
        if let Some(tx) = tx {
            let (steer, _ack) = Steer::new(summary, None);
            let _ = tx.try_send(steer);
        }
    }

    /// Arms mid-turn steers for one specialist turn. The returned guard
    /// clears the channel and the done owner when dropped.
    pub fn bind_inject(self: &Arc<Self>, agent_code: &str) -> (SteerInbox, InjectBinding) {
        let (tx, rx) = mpsc::channel(1);
        {
            let mut s = self.state.lock();
            s.active_inject = Some(tx.clone());
            s.done_turn_code = agent_code.to_owned();
        }
        (
            SteerInbox(rx),
            InjectBinding {
                handle: Arc::clone(self),
                tx,
                code: agent_code.to_owned(),
            },
        )
    }

    pub fn holds_done_turn(&self, agent_code: &str) -> bool {
        self.state.lock().done_turn_code == agent_code
    }

    /// The implementer slot's role spec on the walk's current phase.
    pub fn slot_role_spec(&self, slot: &str, fallback: RoleSpec) -> RoleSpec {
        let view = self.state.lock().roles.clone();
        slot_role_spec(&view, slot, fallback)
    }

    /// Whether `agent_code`'s done may set this walk's summary: the in-flight
    /// specialist, or the configured implementer.
    pub fn accepts_done_summary(&self, agent_code: &str) -> bool {
        if agent_code.trim().is_empty() {
            return false;
        }
        if self.holds_done_turn(agent_code) {
            return true;
        }
        self.slot_role_spec(
            "implementer",
            crate::roles::role_spec(&recur_domain::AgentRole::Coder),
        )
        .code
            == agent_code
    }

    pub fn note_done_summary(&self, summary: &str, notes: &str) {
        let summary = summary.trim();
        if summary.is_empty() {
            return;
        }
        let mut s = self.state.lock();
        s.done_summary = summary.to_owned();
        s.last_done_summary = accumulate_handoff(&s.last_done_summary, summary);
        s.done_notes = notes.trim().to_owned();
    }

    /// Keeps the notes of a done the open-findings gate refused.
    pub fn note_done_notes(&self, notes: &str) {
        let notes = notes.trim();
        if notes.is_empty() {
            return;
        }
        self.state.lock().done_notes = notes.to_owned();
    }

    pub fn take_done_notes(&self) -> String {
        std::mem::take(&mut self.state.lock().done_notes)
    }

    pub fn take_done_summary(&self) -> String {
        std::mem::take(&mut self.state.lock().done_summary)
    }

    /// Every successful done summary of this walk.
    pub fn wrap_up_done(&self) -> String {
        self.state.lock().last_done_summary.clone()
    }

    /// Clears the verify-evidence collector (the store side is
    /// `Engine::begin_verify`).
    pub fn reset_evidence(&self) {
        self.evidence.lock().clear();
    }

    pub fn collect_evidence(&self, e: Evidence) {
        self.evidence.lock().push(e);
    }

    pub fn evidence_this_verify(&self) -> Vec<Evidence> {
        self.evidence.lock().clone()
    }

    /// Sets the iteration incoming findings are stamped with and resets the
    /// per-iteration count.
    pub fn begin_review_iteration(&self, iter: i64) {
        let mut f = self.findings.lock();
        f.cur_iter = iter;
        f.iter_finds = 0;
    }

    /// Counts one finding of the active review; returns its iteration.
    pub fn collect_finding(&self) -> i64 {
        let mut f = self.findings.lock();
        f.iter_finds += 1;
        f.cur_iter
    }

    pub fn findings_this_iteration(&self) -> i64 {
        self.findings.lock().iter_finds
    }

    pub fn chat_code(&self) -> String {
        self.state.lock().chat_code.clone()
    }

    pub fn branch(&self) -> String {
        self.state.lock().branch.clone()
    }

    pub fn set_branch(&self, branch: &str) {
        self.state.lock().branch = branch.to_owned();
    }

    /// This walk's last state and phase.
    pub fn exit(&self) -> (WorkspaceState, String) {
        let s = self.state.lock();
        (s.exit_state.clone(), s.exit_phase.clone())
    }
}

/// Clears a specialist turn's steer binding on drop.
pub struct InjectBinding {
    handle: Arc<LoopHandle>,
    tx: mpsc::Sender<Steer>,
    code: String,
}

impl Drop for InjectBinding {
    fn drop(&mut self) {
        let mut s = self.handle.state.lock();
        if s.active_inject
            .as_ref()
            .is_some_and(|t| t.same_channel(&self.tx))
        {
            s.active_inject = None;
        }
        if s.done_turn_code == self.code {
            s.done_turn_code.clear();
        }
    }
}

#[cfg(test)]
mod tests;
