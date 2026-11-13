//! Escalations (PRD §13): a walk raises a question and blocks at a safe
//! checkpoint until the human answers ("an escalation is just a pause whose
//! resume payload is the human's answer"), the answer is delivered to the live
//! walk or re-dispatches an interrupted one, and a persisted answer that
//! arrived after a restart is applied by the new walk.

use std::sync::Arc;

use parking_lot::Mutex;
use recur_agent_tools::EscalationRequest;
use recur_bundles::{VERB_CONVERGE, VERB_ENSURE_PR};
use recur_domain::{Escalation, EscalationOption, Group, InboxItem, Workspace, WorkspaceState};
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::attention::chat_inbox::chat_inbox_target;
use crate::bus::{EngineEvent, EscalationRaised, EscalationResolved};
use crate::codes::MAESTRO_AGENT_CODE;
use crate::lifecycle::PHASE_ANSWERED_WAITING;
use crate::loop_handle::{Command, EscalationAnswer, LoopHandle};
use crate::loops::Walk;
use crate::publish::terminal;
use crate::text::{equal_fold_ascii, html_escape, truncate, unix_nanos};

/// Engine-stamped escalation provenance (agents cannot set it).
pub const ESC_KIND_AGENT: &str = "agent";
pub const ESC_KIND_CAP: &str = "cap";
pub const ESC_KIND_ENSURE_PR: &str = "ensure_pr";
pub const ESC_KIND_PLAN: &str = "plan";

/// Raised/answered counters (PRD §13 escalation precision).
#[derive(Debug, Default)]
pub struct EscPrecision(Mutex<(i64, i64)>);

impl EscPrecision {
    fn raised(&self) {
        self.0.lock().0 += 1;
    }

    fn answered(&self) {
        self.0.lock().1 += 1;
    }
}

impl Walk {
    /// Raises an escalation and blocks until the human answers. Returns the
    /// answer and whether the walk keeps going (false when Stop or cancel
    /// won). A stopping walk raises nothing; an answer already delivered
    /// (seeded after a restart) is returned without re-raising.
    pub(crate) async fn escalate(
        &self,
        cancel: &CancellationToken,
        req: EscalationRequest,
        kind: &str,
    ) -> (EscalationAnswer, bool) {
        let (stopping, pending) = {
            let mut s = self.h.lock();
            let pending = s.answer.take();
            if pending.is_some() {
                s.escalated = false;
            }
            (s.stopped, pending)
        };
        if stopping || cancel.is_cancelled() {
            return (EscalationAnswer::default(), false);
        }
        if let Some(ans) = pending {
            return (ans, true);
        }
        let esc = self
            .eng
            .raise_escalation(&self.id, &self.chat_code(), &req, kind);
        {
            let mut s = self.h.lock();
            s.escalated = true;
            s.answer = None;
        }
        // Raised before the state flips, so an observer keyed on escalated
        // always finds the event.
        self.eng.publish_escalation_raised(&self.id, esc);
        self.eng.mark_group(&self.id, Group::Needs);
        self.set_state(WorkspaceState::Escalated, "asking a question");
        self.eng.publish_resource();

        if self.await_answer(cancel).await {
            let ans = self.h.lock().answer.take().unwrap_or_default();
            return (ans, true);
        }
        // Stop / composer Stop, not Shutdown: leave the row open on quit so
        // Recover still surfaces the question.
        let explicit = {
            let s = self.h.lock();
            s.stopped || s.park_idle
        };
        if explicit {
            self.eng.retire_walk_escalation(&self.id, &self.chat_code());
        }
        (EscalationAnswer::default(), false)
    }

    /// Blocks while escalated; true when answered, false when Stop or cancel
    /// woke the walk.
    async fn await_answer(&self, cancel: &CancellationToken) -> bool {
        self.h
            .wait_while(cancel, |s| s.escalated && !s.stopped)
            .await;
        !(self.h.is_stopped() || cancel.is_cancelled())
    }

    fn take_answer(&self) -> EscalationAnswer {
        self.h.lock().answer.take().unwrap_or_default()
    }

    fn clear_answer(&self) {
        self.h.lock().answer = None;
    }

    /// Applies a persisted answer that arrived after a restart (this walk is
    /// not sitting in an escalation wait). True when the walk is finished
    /// (Stop, Ship, or idle with no next).
    pub(crate) async fn apply_answered_cap(&mut self, cancel: &CancellationToken) -> bool {
        if self.h.lock().answer.is_none() {
            return false;
        }
        let Some(esc) = self.eng.store.escalation(&self.id) else {
            self.clear_answer();
            return false;
        };
        let kind = persisted_escalation_kind(&esc);
        match kind.as_str() {
            ESC_KIND_CAP | ESC_KIND_ENSURE_PR => {
                let iter = self
                    .resume
                    .as_ref()
                    .map(|r| r.iteration)
                    .filter(|i| *i > 0)
                    .unwrap_or(1);
                let door = kind == ESC_KIND_ENSURE_PR;
                let open = self.eng.store.open_findings_count(&self.id) as i64;
                let (next, more) = self
                    .escalate_at_cap_apply_msg(cancel, iter, open, "", door)
                    .await;
                if more > 0 {
                    if door {
                        if self.rewind_ensure_pr() {
                            return true;
                        }
                        self.mark_cap_answer_applied();
                        return false;
                    }
                    self.h.grant_extension(more);
                    return false;
                }
                if next == WorkspaceState::PrOpen {
                    if !door
                        && let Some(ph) = self.walk_phase()
                        && ph.uses == VERB_CONVERGE
                    {
                        let n = ph.next.trim();
                        if !n.is_empty() {
                            self.set_walk_phase(n);
                            self.mark_cap_answer_applied();
                            return false;
                        }
                        self.set_state(WorkspaceState::Paused, "idle");
                        self.mark_cap_answer_applied();
                        return true;
                    }
                    // Ship with caveats after a restart: open the PR here,
                    // then follow its next edge.
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
                    self.set_state(WorkspaceState::PrOpen, "PR open: ready for your review");
                    if door && self.continue_ensure_pr_graph() {
                        self.mark_cap_answer_applied();
                        return false;
                    }
                    self.mark_cap_answer_applied();
                }
                true
            }
            ESC_KIND_PLAN => {
                let ans = self.take_answer();
                let t = ans.free_text.trim();
                if !t.is_empty() {
                    self.fold_plan_notes(t);
                }
                self.eng.approve_plan(&self.id);
                self.eng.store.mark_escalation_applied(&self.id);
                false
            }
            _ => {
                // Agent MCP: consume so a later escalate cannot steal it.
                let ans = self.take_answer();
                self.steer_agent_answer(&ans);
                false
            }
        }
    }

    /// Hands an agent escalation's answer to the next specialist prompt.
    /// Applied stays false until the steer lands in a prompt.
    pub(crate) fn steer_agent_answer(&self, ans: &EscalationAnswer) {
        let mut guidance = ans.free_text.trim();
        if guidance.is_empty() {
            guidance = ans.resolved.trim();
        }
        if guidance.is_empty() {
            guidance = ans.choice.trim();
        }
        if guidance.is_empty() {
            self.eng.store.mark_escalation_applied(&self.id);
        } else {
            self.h.queue_steer(guidance);
        }
    }

    /// Sends a keep-iterating answer at the PR door back to the inbound
    /// graph phase. True when the walk should stop (nothing inbound).
    pub(crate) fn rewind_ensure_pr(&self) -> bool {
        let prev = self.inbound_graph_phase(&self.checkpoint_ensure_pr_phase_id());
        if prev.is_empty() {
            self.finish_walk_abort();
            return true;
        }
        self.set_walk_phase(&prev);
        false
    }

    fn checkpoint_ensure_pr_phase_id(&self) -> String {
        let id = self.ensure_pr_phase_id();
        if let Some(ws) = self.eng.store.workspace(&self.id)
            && let Some(cp) = &ws.checkpoint
        {
            let cid = cp.phase_id.trim();
            if !cid.is_empty()
                && self
                    .guard
                    .loop_def
                    .phase_by_id(cid)
                    .is_some_and(|ph| ph.uses == VERB_ENSURE_PR)
            {
                return cid.to_owned();
            }
        }
        id
    }

    /// Follows the checkpointed ensure_pr phase's next edge after a restarted
    /// Ship with caveats opened the PR.
    fn continue_ensure_pr_graph(&self) -> bool {
        let Some(ph) = self
            .guard
            .loop_def
            .phase_by_id(&self.checkpoint_ensure_pr_phase_id())
        else {
            return false;
        };
        if ph.next.trim().is_empty() {
            return false;
        }
        let next = ph.next.clone();
        self.set_walk_phase(&next);
        true
    }
}

impl Engine {
    /// Builds, persists and records an escalation (the engine stamps id,
    /// status and kind; the agent supplies only the content), with a feed
    /// line, the actionable inbox row and an OS notification.
    pub fn raise_escalation(
        &self,
        workspace_id: &str,
        agent_code: &str,
        req: &EscalationRequest,
        kind: &str,
    ) -> Escalation {
        let esc = Escalation {
            id: format!("esc-{workspace_id}-{}", unix_nanos()),
            workspace_id: workspace_id.to_owned(),
            agent_code: agent_code.trim().to_owned(),
            question: req.question.clone(),
            context: req.context.clone(),
            options: Some(
                req.options
                    .iter()
                    .map(|o| EscalationOption {
                        text: o.text.clone(),
                        tag: o.tag.clone(),
                        desc: o.desc.clone(),
                    })
                    .collect(),
            ),
            status: "open".into(),
            kind: kind.to_owned(),
            asked_ago: "now".into(),
            ..Escalation::default()
        };
        self.store.add_escalation(esc.clone());
        self.esc_precision.raised();
        self.emit_workspace_event(
            workspace_id,
            "MA",
            "Maestro",
            &format!(
                "Paused to ask: {}",
                html_escape(&truncate(&req.question, 160))
            ),
        );
        let short = self
            .store
            .workspace(workspace_id)
            .map(|l| l.short)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| workspace_id.to_owned());
        let code = esc_chat_code(agent_code);
        let target = chat_inbox_target(workspace_id, code);
        self.store.add_inbox_item(InboxItem {
            id: esc_inbox_id(workspace_id, code),
            group: "needs".into(),
            unread: true,
            kind: "pause".into(),
            color: "var(--coral)".into(),
            title: format!(
                "<b>{}</b> paused: needs your decision",
                html_escape(workspace_id)
            ),
            sub: format!(
                "{} · {}",
                html_escape(&truncate(&short, 64)),
                html_escape(&truncate(&req.question, 64))
            ),
            time: "now".into(),
            action: "Answer".into(),
            action_class: "urgent".into(),
            target: target.clone(),
            workspace_id: workspace_id.to_owned(),
        });
        self.publish_inbox_updated();
        self.publish_notify("A loop needs you", &truncate(&req.question, 120), &target);
        esc
    }

    /// Persists and publishes `escalation.raised` with the full question.
    pub fn publish_escalation_raised(&self, workspace_id: &str, esc: Escalation) {
        let ev = EngineEvent::EscalationRaised(EscalationRaised {
            escalation: esc,
            workspace_id: workspace_id.to_owned(),
        });
        self.persist_event(workspace_id, "", &ev);
        self.bus.publish(ev);
    }

    /// Delivers the human's answer and resumes the walk. `None` when there
    /// was nothing to answer (already answered or no escalation).
    pub async fn answer_escalation(
        &self,
        workspace_id: &str,
        choice: &str,
        free_text: &str,
    ) -> Option<Escalation> {
        self.answer_escalation_for(workspace_id, "", choice, free_text)
            .await
    }

    /// Answers the escalation `agent_code` owns ("" = the workspace-only
    /// fallback: one open row, or the lex-first of several).
    pub async fn answer_escalation_for(
        &self,
        workspace_id: &str,
        agent_code: &str,
        choice: &str,
        free_text: &str,
    ) -> Option<Escalation> {
        self.answer_escalation_to(
            workspace_id,
            None,
            agent_code,
            EscalationAnswer {
                choice: choice.to_owned(),
                free_text: free_text.to_owned(),
                ..EscalationAnswer::default()
            },
        )
        .await
    }

    /// Persists the answer and delivers it to `lp` when given (extend_rounds
    /// on a chat-keyed walk), else to the owning walk, else re-dispatches
    /// it. A second answer never resurrects a settled loop.
    pub(crate) async fn answer_escalation_to(
        &self,
        workspace_id: &str,
        lp: Option<Arc<LoopHandle>>,
        agent_code: &str,
        mut ans: EscalationAnswer,
    ) -> Option<Escalation> {
        let mut resolved = ans.free_text.trim().to_owned();
        if resolved.is_empty() {
            resolved = ans.choice.trim().to_owned();
        }
        ans.resolved = resolved;

        let mut chat = agent_code.trim().to_owned();
        if let Some(lp) = &lp {
            let c = lp.chat_code();
            if !c.trim().is_empty() {
                chat = c.trim().to_owned();
            }
        }
        let (esc, answered) =
            self.store
                .answer_escalation_for(workspace_id, &chat, &ans.choice, &ans.free_text);
        let esc = match esc {
            Some(e) if answered => e,
            _ => return None,
        };
        let resolved = esc.answer.clone();
        self.esc_precision.answered();

        self.store
            .remove_inbox_item(&esc_inbox_id(workspace_id, &esc.agent_code));
        let still_open = self.store.has_open_escalation(workspace_id);
        if !still_open {
            self.store
                .remove_inbox_by_target(&format!("workspace:{workspace_id}"));
            self.mark_group(workspace_id, Group::Active);
        }
        self.publish_inbox_updated();
        let cmd = Command::AnswerEscalation(ans);
        let lp = match lp {
            Some(lp) => Some(lp),
            None => {
                let code = esc.agent_code.trim();
                if code.is_empty() {
                    self.running_loop_by_id(workspace_id)
                } else {
                    self.running_loop_by_chat(workspace_id, code)
                }
            }
        };
        if let Some(lp) = lp {
            // Clear escalated before the walk wakes so the UI is not stuck on
            // Needs you, and so the walk's own next state (Stop included)
            // always lands after this one.
            if !still_open
                && self
                    .store
                    .workspace(workspace_id)
                    .is_some_and(|r| r.state == WorkspaceState::Escalated)
            {
                self.set_workspace_state(workspace_id, WorkspaceState::Coding, "resuming");
            }
            lp.send(cmd);
        } else if !self
            .resume_after_answer(workspace_id, esc.agent_code.trim(), cmd)
            .await
            && (esc.agent_code.trim().is_empty() || !still_open)
            && let Some(rec) = self.store.workspace(workspace_id)
            && !terminal(&rec.state)
        {
            // Nothing started: rest paused for Resume, unless another chat's
            // question keeps the workspace escalated.
            self.set_workspace_state(workspace_id, WorkspaceState::Paused, PHASE_ANSWERED_WAITING);
        }
        self.emit_workspace_event(
            workspace_id,
            "MA",
            "Maestro",
            &format!(
                "Answer received: {}. Resuming.",
                html_escape(&truncate(&resolved, 160))
            ),
        );
        self.bus
            .publish(EngineEvent::EscalationResolved(EscalationResolved {
                agent_code: Some(esc.agent_code.clone()),
                answer: resolved,
                open: Some(still_open),
                workspace_id: workspace_id.to_owned(),
            }));
        self.publish_resource();
        Some(esc)
    }

    /// Closes this chat's open question without an answer (Stop / composer
    /// Stop). A sibling walk's row stays open. "" = the lex-first open row.
    pub fn retire_walk_escalation(&self, workspace_id: &str, agent_code: &str) -> bool {
        let (esc, answered) =
            self.store
                .answer_escalation_for(workspace_id, agent_code, "cancelled", "");
        let esc = match esc {
            Some(e) if answered => e,
            _ => return false,
        };
        self.store
            .remove_inbox_item(&esc_inbox_id(workspace_id, &esc.agent_code));
        let still_open = self.store.has_open_escalation(workspace_id);
        if !still_open {
            self.store
                .remove_inbox_by_target(&format!("workspace:{workspace_id}"));
            self.mark_group(workspace_id, Group::Active);
        }
        self.publish_inbox_updated();
        self.bus
            .publish(EngineEvent::EscalationResolved(EscalationResolved {
                agent_code: Some(esc.agent_code),
                answer: "cancelled".into(),
                open: Some(still_open),
                workspace_id: workspace_id.to_owned(),
            }));
        true
    }

    /// Closes every unanswered question on the workspace (a not-running
    /// Stop: Recover left the rows open with no walk).
    pub(crate) fn retire_open_escalations(&self, workspace_id: &str) {
        while self.retire_walk_escalation(workspace_id, "") {}
    }

    /// Best-effort re-dispatch of a not-running walk after its escalation
    /// was answered; the new walk seeds the persisted answer. True only when
    /// a walk is running for it. Serialized under `dispatch_mu` and never
    /// resurrects a settled loop or clobbers a live sibling.
    pub(crate) async fn resume_after_answer(
        &self,
        workspace_id: &str,
        agent_code: &str,
        cmd: Command,
    ) -> bool {
        let d = self.dispatch_mu.lock().await;
        if let Some(lp) = self.running_loop_for_hand(workspace_id, agent_code) {
            drop(d);
            lp.send(cmd);
            return true;
        }
        if !agent_code.trim().is_empty() && self.running_loop_by_id(workspace_id).is_some() {
            return false;
        }
        let Some(rec) = self.store.workspace(workspace_id) else {
            return false;
        };
        if terminal(&rec.state) {
            return false;
        }
        let rec = self.chat_scoped_workspace(rec, agent_code);
        // Only a real interrupted loop (the seeded demo has no repo).
        if self
            .store
            .project(&rec.project_id)
            .is_none_or(|p| p.repo_path.is_empty())
        {
            return false;
        }
        if !agent_code.trim().is_empty() {
            // Persist this chat's identity so a no-slot Resume reloads it.
            self.persist_scoped_identity(&rec, false);
        }
        if !self.gov.try_acquire_loop() {
            return false;
        }
        let res = self.dispatch(rec).await;
        drop(d);
        if res.is_err() {
            self.gov.release_loop();
            return false;
        }
        true
    }

    /// Raised/answered escalation counts (status/telemetry).
    pub fn escalation_stats(&self) -> (i64, i64) {
        *self.esc_precision.0.lock()
    }
}

/// The chat a loop question opens: the asking walk's chat, Maestro when empty.
fn esc_chat_code(agent_code: &str) -> &str {
    match agent_code.trim() {
        "" => MAESTRO_AGENT_CODE,
        c => c,
    }
}

/// The Needs-you row for one chat's open loop question.
pub(crate) fn esc_inbox_id(workspace_id: &str, agent_code: &str) -> String {
    format!("ib-esc-{workspace_id}-{}", esc_chat_code(agent_code))
}

/// The engine-stamped kind, falling back to option-text heuristics only for
/// rows raised before Kind existed.
pub fn persisted_escalation_kind(esc: &Escalation) -> String {
    let k = esc.kind.trim();
    if !k.is_empty() {
        return k.to_owned();
    }
    if cap_escalation(esc) {
        if ensure_pr_door_escalation(esc) {
            return ESC_KIND_ENSURE_PR.into();
        }
        return ESC_KIND_CAP.into();
    }
    if plan_approval_escalation(esc) {
        return ESC_KIND_PLAN.into();
    }
    ESC_KIND_AGENT.into()
}

/// Whether a new walk must apply a persisted answer. Legacy rows defaulted
/// applied to true; Recover's stuck-answer states still carry a decision.
pub fn should_seed_escalation_answer(ws: &Workspace, esc: &Escalation) -> bool {
    if esc.status != "answered" {
        return false;
    }
    if !esc.applied {
        return true;
    }
    if ws.state == WorkspaceState::Escalated {
        return true;
    }
    ws.state == WorkspaceState::Paused && ws.phase == PHASE_ANSWERED_WAITING
}

/// Rebuilds the in-memory answer from the durable record. Old rows stored
/// only Answer: it maps to Choice when it matches an offered option, else
/// FreeText.
pub fn restore_escalation_answer(esc: &Escalation) -> Option<EscalationAnswer> {
    let mut choice = esc.choice.trim().to_owned();
    let mut free = esc.free_text.trim().to_owned();
    let resolved = esc.answer.trim().to_owned();
    if choice.is_empty() && free.is_empty() && resolved.is_empty() {
        return None;
    }
    if choice.is_empty() && free.is_empty() && !resolved.is_empty() {
        if escalation_choice_match(esc, &resolved) {
            choice = resolved.clone();
        } else {
            free = resolved.clone();
        }
    }
    Some(EscalationAnswer {
        choice,
        free_text: free,
        resolved,
        grant: 0,
    })
}

fn options(esc: &Escalation) -> &[EscalationOption] {
    esc.options.as_deref().unwrap_or_default()
}

fn escalation_choice_match(esc: &Escalation, resolved: &str) -> bool {
    let want = resolved.trim().to_lowercase();
    options(esc)
        .iter()
        .any(|o| o.text.trim().to_lowercase() == want)
}

/// A rail-#4 cap question (the engine's own option text).
pub fn cap_escalation(esc: &Escalation) -> bool {
    options(esc).iter().any(|o| {
        matches!(
            o.text.trim().to_lowercase().as_str(),
            "keep iterating" | "ship with caveats"
        )
    })
}

/// The ensure_pr policy-bar raise: its unique question prefix, or the
/// "anyway" ship option round-cap raises never carry.
pub fn ensure_pr_door_escalation(esc: &Escalation) -> bool {
    if esc.question.starts_with("Policy bar blocked the PR:") {
        return true;
    }
    options(esc).iter().any(|o| {
        equal_fold_ascii(o.text.trim(), "Ship with caveats")
            && o.desc.to_lowercase().contains("anyway")
    })
}

pub fn plan_approval_escalation(esc: &Escalation) -> bool {
    options(esc)
        .iter()
        .any(|o| o.text.trim().to_lowercase() == "approve and proceed")
}
