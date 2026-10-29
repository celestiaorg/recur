//! The graph walk (Go `walker.go`): over a `[[loop]]`'s phases from the
//! entry (or the checkpointed phase), one verb per phase, retries and
//! `on_fail`, on-demand phases parked and resumed, and the walk-step cap.

use recur_bundles::{
    LoopPhase, VERB_ARTIFACT, VERB_CONVERGE, VERB_ENSURE_PR, VERB_IMPLEMENT, VERB_PLAN,
    VERB_REVIEW, VERB_VERIFY, WALK_STEP_CAP, same_or_stricter,
};
use recur_domain::{Checkpoint, Group, WorkspaceState};
use tokio_util::sync::CancellationToken;

use crate::loops::failure::LoopError;
use crate::loops::{STATE_ADVANCE, Walk, board::BOARD_CAP_PHASE};
use crate::text::html_escape;

/// Go `errVerifyRejected`.
pub const ERR_VERIFY_REJECTED: &str = "verify rejected";
/// Go `errPhaseFailed`.
pub const ERR_PHASE_FAILED: &str = "phase failed";

/// The workspace state a phase outcome names.
pub(crate) fn advance() -> WorkspaceState {
    WorkspaceState::from(STATE_ADVANCE)
}

/// One phase attempt's outcome: the next phase id (or a terminal state's
/// name), whether the walk is done, and the failure to retry.
pub(crate) type Attempt = (String, bool, Option<LoopError>);

impl Walk {
    /// Walks the graph to a terminal or idle state.
    pub(crate) async fn walk_loop(&mut self, cancel: &CancellationToken) {
        let mut start = self.guard.loop_def.entry.clone();
        let sp = self.start_phase.trim().to_owned();
        if !sp.is_empty() {
            start = sp;
        } else if !self.fresh_walk
            && let Some(ws) = self.eng.store.workspace(&self.id)
            && let Some(cp) = &ws.checkpoint
        {
            let id = cp.phase_id.trim();
            if !id.is_empty() {
                if self.guard.loop_def.phase_by_id(id).is_none() {
                    self.emit_feed(
                        "MA",
                        "Maestro",
                        &format!("Checkpoint phase {} is gone — idling.", html_escape(id)),
                    );
                    self.set_state(WorkspaceState::Paused, "phase missing");
                    return;
                }
                start = id.to_owned();
            }
            let rid = cp.resume_phase_id.trim();
            if !rid.is_empty() && self.resume_phase_id.is_empty() {
                self.resume_phase_id = rid.to_owned();
            }
            if self
                .guard
                .loop_def
                .phase_by_id(&start)
                .is_some_and(|ph| ph.dispatch == "on_demand")
            {
                self.on_demand_walk = true;
            }
        }

        let mut steps = 0usize;
        let mut id = start;
        loop {
            if !self.checkpoint(cancel).await {
                self.finish_walk_abort();
                return;
            }
            let pending = self.take_pending_on_demand();
            if !pending.is_empty() {
                self.resume_phase_id = id.clone();
                self.on_demand_walk = true;
                let resume = id.clone();
                let p = pending.clone();
                self.eng.store.update_workspace(&self.id, |ws| {
                    let cp = ws.checkpoint.get_or_insert_with(Checkpoint::default);
                    cp.phase_id = p;
                    cp.resume_phase_id = resume;
                });
                id = pending;
                continue;
            }
            let Some(ph) = self.guard.loop_def.phase_by_id(&id).cloned() else {
                self.emit_feed(
                    "MA",
                    "Maestro",
                    &format!("Phase {} is gone — idling.", html_escape(&id)),
                );
                self.set_state(WorkspaceState::Paused, "phase missing");
                return;
            };
            if ph.dispatch == "on_demand" && !self.on_demand_walk {
                self.emit_feed(
                    "MA",
                    "Maestro",
                    &format!("Skipping on-demand phase {}.", html_escape(&ph.id)),
                );
                id = std::mem::take(&mut self.resume_phase_id);
                if id.is_empty() {
                    self.set_state(WorkspaceState::Paused, "idle");
                    return;
                }
                continue;
            }
            self.set_walk_phase(&ph.id);
            self.note_threshold_override(&ph);
            let (mut next, done) = self.run_loop_phase(cancel, &ph).await;
            if done {
                self.finish_stopped_if_in_flight();
                return;
            }
            steps += 1;
            if steps >= WALK_STEP_CAP {
                self.emit_feed("MA", "Maestro", "Walk-step cap reached — escalating.");
                let open = self.eng.store.open_findings_count(&self.id) as i64;
                let (st, more) = self.escalate_at_cap_apply(cancel, steps as i64, open).await;
                if more == 0 && st != WorkspaceState::PrOpen {
                    self.finish_walk_abort();
                    return;
                }
                steps = 0;
                if st == WorkspaceState::PrOpen {
                    let pr = self.ensure_pr_phase_id();
                    if !pr.is_empty() {
                        next = pr.clone();
                        self.set_walk_phase(&pr);
                    }
                }
                self.mark_cap_answer_applied();
            }
            if next.is_empty() {
                if self.on_demand_walk && !self.resume_phase_id.is_empty() {
                    id = std::mem::take(&mut self.resume_phase_id);
                    self.on_demand_walk = false;
                    continue;
                }
                self.set_state(WorkspaceState::Paused, "idle");
                return;
            }
            id = next;
        }
    }

    /// Runs a phase with its retries, then `on_fail`.
    pub(crate) async fn run_loop_phase(
        &mut self,
        cancel: &CancellationToken,
        ph: &LoopPhase,
    ) -> (String, bool) {
        let attempts = 1 + self.phase_retries(ph);
        let mut last_err = None;
        for i in 0..attempts {
            // The policy timeout wraps each specialist turn, not the plan
            // gate / cap / pause waits.
            let (next, done, err) = self.run_phase_attempt(cancel, ph).await;
            if cancel.is_cancelled() {
                self.finish_walk_abort();
                return (next, true);
            }
            let Some(err) = err else {
                return (next, done);
            };
            if i + 1 < attempts {
                self.emit_feed(
                    "MA",
                    "Maestro",
                    &format!(
                        "Phase {} failed ({}) — retrying.",
                        html_escape(&ph.id),
                        html_escape(&err.to_string())
                    ),
                );
            }
            last_err = Some(err);
        }
        self.take_on_fail(cancel, ph, last_err).await
    }

    pub(crate) async fn run_phase_attempt(
        &mut self,
        cancel: &CancellationToken,
        ph: &LoopPhase,
    ) -> Attempt {
        self.set_current_phase(ph);
        match ph.uses.as_str() {
            VERB_PLAN => {
                self.set_state(WorkspaceState::Planning, "decomposing the task");
                if !self.adopt_existing_plan()
                    && let Err(e) = self.run_planner(cancel).await
                {
                    return (String::new(), false, Some(e));
                }
                if let (n, true) = self.gate_plan_approval(cancel).await {
                    return (n.as_str().to_owned(), true, None);
                }
                (ph.next.clone(), false, None)
            }
            VERB_CONVERGE => {
                let st = match self.converge(cancel).await {
                    Ok(st) => st,
                    Err(e) => return (String::new(), false, Some(e)),
                };
                match st {
                    WorkspaceState::Stopped => {
                        self.finish_stopped_if_in_flight();
                        return (st.as_str().to_owned(), true, None);
                    }
                    WorkspaceState::Failed | WorkspaceState::Paused => {
                        return (st.as_str().to_owned(), true, None);
                    }
                    WorkspaceState::PrOpen => {
                        let n = ph.next.trim();
                        if !n.is_empty() {
                            self.set_walk_phase(n);
                        }
                        self.mark_cap_answer_applied();
                    }
                    _ => {}
                }
                (ph.next.clone(), false, None)
            }
            VERB_IMPLEMENT => {
                self.set_state(WorkspaceState::Coding, "implementing");
                if let Err(e) = self.run_coder(cancel, 1, Vec::new()).await {
                    return (String::new(), false, Some(e));
                }
                (ph.next.clone(), false, None)
            }
            VERB_REVIEW => {
                self.set_state(WorkspaceState::Reviewing, "reviewing");
                if let Err(e) = self.run_reviewer(cancel, 1).await {
                    return (String::new(), false, Some(e));
                }
                (ph.next.clone(), false, None)
            }
            VERB_ENSURE_PR => {
                let th = self.guard.phase_threshold(ph);
                if let Some(reason) = self.eng.findings_bar_blocks(&self.id, &th) {
                    self.emit_feed(
                        "MA",
                        "Maestro",
                        &format!("Policy bar blocked the PR: {}", html_escape(&reason)),
                    );
                    let iter = self
                        .eng
                        .store
                        .workspace(&self.id)
                        .map_or(0, |ws| ws.iteration);
                    let open = self.eng.store.open_findings_count(&self.id) as i64;
                    let question =
                        format!("Policy bar blocked the PR: {reason}. How should I proceed?");
                    let (st, more) = self
                        .escalate_at_cap_apply_msg(cancel, iter, open, &question, true)
                        .await;
                    if more > 0 {
                        if self.rewind_ensure_pr() {
                            return (String::new(), true, None);
                        }
                        self.mark_cap_answer_applied();
                        return (self.inbound_graph_phase(&ph.id), false, None);
                    }
                    if st != WorkspaceState::PrOpen {
                        self.finish_walk_abort();
                        return (String::new(), true, None);
                    }
                    // Ship with caveats: fall through and open the PR.
                }
                if let Err(e) = self.open_pr(cancel).await
                    && !cancel.is_cancelled()
                {
                    self.emit_feed(
                        "MA",
                        "Maestro",
                        &format!("Could not open the PR: {}", html_escape(&e.to_string())),
                    );
                }
                self.eng.mark_group(&self.id, Group::Ready);
                self.set_state(WorkspaceState::PrOpen, "PR open — ready for your review");
                let n = ph.next.trim();
                if !n.is_empty() {
                    self.set_walk_phase(n);
                }
                self.mark_cap_answer_applied();
                (ph.next.clone(), ph.next.is_empty(), None)
            }
            VERB_VERIFY => {
                self.set_state(WorkspaceState::Verifying, "verifying");
                let passed = match self.run_verify(cancel).await {
                    Ok(p) => p,
                    Err(e) => return (String::new(), false, Some(e)),
                };
                if passed {
                    return (ph.next.clone(), false, None);
                }
                if !ph.on_reject.is_empty() {
                    return (ph.on_reject.clone(), false, None);
                }
                self.fail(cancel, &LoopError::msg(ERR_VERIFY_REJECTED));
                (String::new(), true, None)
            }
            VERB_ARTIFACT => {
                self.set_state(WorkspaceState::Paused, "artifact");
                (ph.next.clone(), ph.next.is_empty(), None)
            }
            other => {
                self.emit_feed(
                    "MA",
                    "Maestro",
                    &format!("Unknown verb {}", html_escape(other)),
                );
                (String::new(), true, None)
            }
        }
    }

    pub(crate) fn phase_retries(&self, ph: &LoopPhase) -> i64 {
        if let Some(n) = ph.retries
            && n >= 0
        {
            return n;
        }
        if self.guard.policy.retries > 0 {
            return self.guard.policy.retries;
        }
        0
    }

    /// The graph's ensure_pr phase (on-demand phases excluded).
    pub(crate) fn ensure_pr_phase_id(&self) -> String {
        self.guard
            .loop_def
            .phases
            .iter()
            .filter(|p| p.dispatch != "on_demand")
            .find(|p| p.uses == VERB_ENSURE_PR)
            .map(|p| p.id.clone())
            .unwrap_or_default()
    }

    /// The graph phase whose `next` is `id` (on-demand phases excluded).
    pub(crate) fn inbound_graph_phase(&self, id: &str) -> String {
        self.guard
            .loop_def
            .phases
            .iter()
            .filter(|p| p.dispatch != "on_demand")
            .find(|p| p.next == id)
            .map(|p| p.id.clone())
            .unwrap_or_default()
    }

    fn take_pending_on_demand(&self) -> String {
        std::mem::take(&mut self.h.lock().pending_on_demand)
    }

    pub(crate) async fn take_on_fail(
        &mut self,
        cancel: &CancellationToken,
        ph: &LoopPhase,
        err: Option<LoopError>,
    ) -> (String, bool) {
        let mut action = ph.on_fail.clone();
        if action.is_empty() {
            action = if ph.dispatch == "on_demand" {
                "skip".into()
            } else {
                self.guard.policy.on_fail.clone()
            };
        }
        if action.is_empty() {
            action = "fail".into();
        }
        let err = err.unwrap_or_else(|| LoopError::msg(ERR_PHASE_FAILED));
        match action.as_str() {
            "skip" => {
                if ph.dispatch == "on_demand" {
                    return (String::new(), false);
                }
                (ph.next.clone(), false)
            }
            "escalate" => {
                let iter = self
                    .eng
                    .store
                    .workspace(&self.id)
                    .map_or(0, |ws| ws.iteration);
                let open = self.eng.store.open_findings_count(&self.id) as i64;
                let (_, more) = self.escalate_at_cap_apply(cancel, iter, open).await;
                if more == 0 {
                    self.finish_walk_abort();
                    return (String::new(), true);
                }
                self.mark_cap_answer_applied();
                (ph.id.clone(), false)
            }
            _ => {
                self.fail(cancel, &err);
                (String::new(), true)
            }
        }
    }

    /// Records stopped unless this walk already settled. Walk-local
    /// outcome, not the row: a sibling may have overwritten it.
    pub(crate) fn finish_stopped_if_in_flight(&self) {
        let (state, phase) = self.h.exit();
        if crate::attention::chat_inbox::loop_settled(&state, &phase) {
            return;
        }
        let Some(ws) = self.eng.store.workspace(&self.id) else {
            return;
        };
        match ws.state {
            WorkspaceState::Stopped
            | WorkspaceState::Failed
            | WorkspaceState::Merged
            | WorkspaceState::PrOpen => return,
            WorkspaceState::Paused
                if matches!(
                    ws.phase.as_str(),
                    "idle" | "artifact" | "phase missing" | "loop refs missing" | BOARD_CAP_PHASE
                ) =>
            {
                return;
            }
            _ => {}
        }
        self.finish_stopped();
    }

    fn note_threshold_override(&self, ph: &LoopPhase) {
        let Some(f) = &ph.findings else {
            return;
        };
        if same_or_stricter(f, &self.guard.policy.findings_threshold).is_ok() {
            return;
        }
        self.emit_feed(
            "MA",
            "Maestro",
            &format!(
                "Phase {} findings_threshold is looser than policy {} — using the policy bar.",
                html_escape(&ph.id),
                html_escape(&self.guard.policy.id)
            ),
        );
    }

    /// Checkpoints the phase the walk is in (its resume position).
    pub(crate) fn set_walk_phase(&self, id: &str) {
        let id = id.to_owned();
        self.eng.store.update_workspace(&self.id, |ws| {
            ws.checkpoint
                .get_or_insert_with(Checkpoint::default)
                .phase_id = id;
        });
    }
}
