//! Converge rounds and their rails: the Coder edits, the engine commits, a
//! separate Reviewer files findings, and the ENGINE declares convergence off
//! the remaining open findings (PRD §7.3). At the round cap the policy's
//! `on_max_rounds` proceeds, fails, or escalates (rail #4). Also the plan
//! adoption and approval gate of the plan verb.

use recur_agent::display::PLAN_MAX_PERSIST_CHARS;
use recur_agent_tools::{EscalationOption, EscalationRequest};
use recur_domain::{AutonomyLevel, ConvPoint, Finding, Group, WorkspaceState};
use tokio_util::sync::CancellationToken;

use crate::governor::AgentPermit;
use crate::loop_handle::EscalationAnswer;
use crate::loops::Walk;
use crate::loops::escalation::{
    ESC_KIND_AGENT, ESC_KIND_CAP, ESC_KIND_ENSURE_PR, ESC_KIND_PLAN, persisted_escalation_kind,
    restore_escalation_answer,
};
use crate::loops::failure::LoopError;
use crate::loops::walker::advance;
use crate::plan::cap_plan_for_persist;
use crate::text::{html_escape, plural, truncate};

/// How a cap-escalation answer is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapAnswer {
    Stop,
    KeepIterating,
    ChangeApproach,
    Ship,
    /// Free text instead of an option: steered verbatim, never
    /// keyword-guessed; the engine only enforces the cap arithmetic.
    Guidance,
}

/// Maps a cap answer to an action by the exact option text the engine
/// offered; free text is guidance; empty stops (the safe default).
pub fn classify_cap_answer(ans: &EscalationAnswer) -> CapAnswer {
    match ans.choice.trim().to_lowercase().as_str() {
        "keep iterating" => return CapAnswer::KeepIterating,
        "change approach" => return CapAnswer::ChangeApproach,
        "ship with caveats" => return CapAnswer::Ship,
        "stop" | "i'll set the run config" => return CapAnswer::Stop,
        _ => {}
    }
    if !ans.free_text.trim().is_empty() {
        return CapAnswer::Guidance;
    }
    CapAnswer::Stop
}

/// Extra iterations a keep-iterating grant gives, by autonomy (bounded so a
/// repeated grant still re-asks).
pub fn cap_extension(a: &AutonomyLevel) -> i64 {
    match a {
        AutonomyLevel::Cautious => 1,
        AutonomyLevel::Aggressive => 3,
        _ => 2,
    }
}

/// The open findings a re-entry coder turn fixes (start=review entries skip
/// the coder first and stay unseeded).
pub fn seed_fix_findings(start_iter: i64, skip_coder: bool, open: Vec<Finding>) -> Vec<Finding> {
    if start_iter > 1 && !skip_coder && !open.is_empty() {
        return open;
    }
    Vec::new()
}

/// The last iteration of one converge call: `max_iter` rounds from
/// `start_iter` (not an absolute 1..max window).
pub fn converge_round_window(start_iter: i64, max_iter: i64) -> i64 {
    start_iter.max(1) + max_iter.max(1) - 1
}

/// The convergence-bar color for a finding count.
pub fn conv_color(findings: i64) -> &'static str {
    match findings {
        0 => "var(--emerald)",
        1..=3 => "var(--amber)",
        _ => "var(--coral)",
    }
}

impl Walk {
    /// Drives code↔review to convergence. Returns [`advance`] on
    /// convergence (or a proceed cap), or the terminal state.
    pub(crate) async fn converge(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<WorkspaceState, LoopError> {
        let mut max_iter = self.guard.policy.max_rounds;
        if let Some(ph) = self.walk_phase() {
            max_iter = self.guard.phase_max_rounds(&ph);
        }
        let mut prev_findings: Vec<Finding> = Vec::new();
        let mut extra_cap = 0i64;

        // Resuming (D55): pick up at the checkpointed iteration; skip the
        // coder when it already committed, unless findings are still open.
        let mut start_iter = 1;
        let mut skip_coder = false;
        let mut resume_entry = false;
        let open_count = self.eng.store.open_findings_count(&self.id);
        if let Some(r) = self.resume.as_mut()
            && !r.start_consumed
        {
            start_iter = r.iteration;
            skip_coder = r.skip_coder && open_count == 0;
            r.start_consumed = true;
            resume_entry = true;
        } else if let Some(ws) = self.eng.store.workspace(&self.id)
            && ws.iteration > 0
        {
            start_iter = ws.iteration + 1;
        }
        // start=review is per entry: reenter_phase after a Review & fix walk
        // still opens on the reviewer.
        if let Some(ph) = self.walk_phase()
            && ph.start == "review"
            && !resume_entry
        {
            skip_coder = true;
        }
        if start_iter > 1 && !skip_coder {
            let seeded = seed_fix_findings(
                start_iter,
                skip_coder,
                self.eng.store.open_findings(&self.id).unwrap_or_default(),
            );
            if !seeded.is_empty() {
                prev_findings = seeded;
            }
        }
        let end_iter = converge_round_window(start_iter, max_iter);

        let mut iter = start_iter;
        while iter <= end_iter + extra_cap {
            if !self.checkpoint(cancel).await {
                return Ok(WorkspaceState::Stopped);
            }
            extra_cap += self.h.take_granted_extension();

            // The board: the entry's first coding pass works a structured
            // plan; the review then judges the aggregate diff.
            let mut board_ran = false;
            if iter == start_iter && !skip_coder && prev_findings.is_empty() {
                let (ran, stop) = self.run_board(cancel, iter).await?;
                if let Some(st) = stop {
                    return Ok(st);
                }
                if ran {
                    skip_coder = true;
                    board_ran = true;
                }
            }

            let mut skipped_coder = false;
            if skip_coder {
                skip_coder = false;
                skipped_coder = true;
                self.set_iteration(iter);
                let msg = if board_ran {
                    "Board finished: reviewing the aggregate diff.".to_owned()
                } else if self.resume.is_none() {
                    "Reviewing the current worktree first.".to_owned()
                } else {
                    format!(
                        "Iteration {iter} was already coded before the interruption: re-reviewing the committed diff."
                    )
                };
                self.emit_feed("MA", "Maestro", &msg);
            } else {
                self.reset_board_handoff();
                self.set_state(WorkspaceState::Coding, &format!("iteration {iter}"));
                self.set_iteration(iter);
                self.run_coder(cancel, iter, prev_findings.clone()).await?;
                // A non-empty handoff gives the Maestro one bounded turn to
                // revise the plan before the Reviewer judges the diff.
                if !self.last_handoff.is_empty() && self.checkpoint(cancel).await {
                    let h = self.last_handoff.clone();
                    self.run_replanner(cancel, &h).await;
                }
            }

            if !self.checkpoint(cancel).await {
                return Ok(WorkspaceState::Stopped);
            }

            self.set_state(
                WorkspaceState::Reviewing,
                &format!("reviewing iteration {iter}"),
            );
            self.run_reviewer(cancel, iter).await?;

            // The bar is remaining opens, not this turn's filings.
            let bar_findings = self.eng.store.open_findings(&self.id).unwrap_or_default();
            self.record_convergence(iter, bar_findings.len() as i64);
            self.eng.gov.add_iters(1);
            self.eng.publish_resource();

            if self.converge_bar_passed() {
                // start=review skipped the only commit site: commit the
                // reviewed tree before ensure_pr / the PR diff.
                if skipped_coder {
                    let msg = self.reviewed_commit_message(cancel).await;
                    self.commit_worktree("MA", "Maestro", &msg).await?;
                }
                self.emit_feed(
                    "MA",
                    "Maestro",
                    &format!("Converged after {iter} iteration(s): 0 open findings."),
                );
                return Ok(advance());
            }

            self.emit_feed(
                "MA",
                "Maestro",
                &format!(
                    "Iteration {iter}: {} finding(s), sending back to the Coder.",
                    bar_findings.len()
                ),
            );
            prev_findings = bar_findings;

            if iter >= end_iter + extra_cap {
                match self.cap_on_max_rounds().as_str() {
                    "proceed" => {
                        self.emit_feed("MA", "Maestro", "Round cap reached: proceeding.");
                        return Ok(advance());
                    }
                    "fail" => {
                        self.emit_feed(
                            "MA",
                            "Maestro",
                            &format!("Stopped at the {iter}-iteration cap with open findings."),
                        );
                        self.set_state(WorkspaceState::Failed, "round cap");
                        return Ok(WorkspaceState::Failed);
                    }
                    _ => {}
                }
                let (next, more) = self.handle_cap(cancel, iter).await;
                if more > 0 {
                    extra_cap += more;
                    iter += 1;
                    continue;
                }
                return Ok(next);
            }
            iter += 1;
        }
        Ok(WorkspaceState::Stopped)
    }

    fn cap_on_max_rounds(&self) -> String {
        let on_max = match self.walk_phase() {
            Some(ph) => self.guard.phase_on_max_rounds(&ph),
            None => self.guard.policy.on_max_rounds.clone(),
        };
        if on_max.is_empty() {
            "escalate".into()
        } else {
            on_max
        }
    }

    /// Asks the human at `on_max_rounds = escalate`; `more > 0` extends the
    /// cap.
    async fn handle_cap(&mut self, cancel: &CancellationToken, iter: i64) -> (WorkspaceState, i64) {
        self.h.lock().round_cap_esc = true;
        let open = self.eng.store.open_findings_count(&self.id) as i64;
        let out = self.escalate_at_cap_apply(cancel, iter, open).await;
        self.h.lock().round_cap_esc = false;
        out
    }

    /// Raises the rail-#4 escalation and maps the answer to an action.
    pub(crate) async fn escalate_at_cap_apply(
        &mut self,
        cancel: &CancellationToken,
        iter: i64,
        open: i64,
    ) -> (WorkspaceState, i64) {
        self.escalate_at_cap_apply_msg(cancel, iter, open, "", false)
            .await
    }

    /// [`escalate_at_cap_apply`](Self::escalate_at_cap_apply) with an
    /// optional question (the PR-door bar is not a round cap). `ship_anyway`
    /// offers Ship with caveats even while the bar blocks.
    pub(crate) async fn escalate_at_cap_apply_msg(
        &mut self,
        cancel: &CancellationToken,
        iter: i64,
        open: i64,
        question: &str,
        ship_anyway: bool,
    ) -> (WorkspaceState, i64) {
        let opt = |text: &str, tag: &str, desc: &str| EscalationOption {
            text: text.into(),
            tag: tag.into(),
            desc: desc.into(),
        };
        let mut opts = vec![
            opt(
                "Keep iterating",
                "more effort",
                "Give me more fix iterations to converge.",
            ),
            opt(
                "Change approach",
                "rethink",
                "The current direction isn't working; I'll try a different one.",
            ),
        ];
        if ship_anyway {
            opts.push(opt(
                "Ship with caveats",
                "accept risk",
                "Open the PR anyway, noting remaining findings.",
            ));
        } else if self.eng.can_open_pr(&self.id).0 {
            opts.push(opt(
                "Ship with caveats",
                "accept risk",
                "Open the PR noting remaining risk (rails currently allow it).",
            ));
        }
        opts.push(opt(
            "Stop",
            "halt",
            "Stop the loop; I'll leave the work as-is for you to inspect.",
        ));
        let question = if question.trim().is_empty() {
            format!(
                "I've run {iter} iteration(s) without converging: {} remain. How should I proceed?",
                plural(open, "open finding", "open findings")
            )
        } else {
            question.to_owned()
        };
        let req = EscalationRequest {
            question,
            context: format!(
                "The reviewer keeps finding issues on the current diff (loop {}). I won't open a PR with unresolved findings, and I won't spin forever: this decision is yours.",
                self.id
            ),
            options: opts,
        };
        let kind = if ship_anyway {
            ESC_KIND_ENSURE_PR
        } else {
            ESC_KIND_CAP
        };
        let (ans, keep_going) = self.escalate(cancel, req, kind).await;
        if !keep_going {
            return (WorkspaceState::Stopped, 0);
        }
        let mut grant = ans.grant;
        if grant < 1 {
            grant = self.policy_extend_rounds();
        }
        match classify_cap_answer(&ans) {
            CapAnswer::KeepIterating => {
                self.emit_feed(
                    "MA",
                    "Maestro",
                    "You chose to keep iterating: extending the convergence budget.",
                );
                (WorkspaceState::Coding, grant)
            }
            CapAnswer::ChangeApproach => {
                self.emit_feed(
                    "MA",
                    "Maestro",
                    "You asked me to change approach: continuing with that guidance.",
                );
                (WorkspaceState::Coding, grant)
            }
            CapAnswer::Guidance => {
                self.h.queue_steer(&ans.free_text);
                self.emit_feed(
                    "MA",
                    "Maestro",
                    &format!(
                        "Continuing with your guidance: {}",
                        html_escape(&truncate(&ans.free_text, 160))
                    ),
                );
                (WorkspaceState::Coding, grant)
            }
            CapAnswer::Ship => {
                if !ship_anyway {
                    let (ok, reason) = self.eng.can_open_pr(&self.id);
                    if !ok {
                        self.set_state(
                            WorkspaceState::Stopped,
                            "policy bar blocked ship-with-caveats",
                        );
                        self.eng.store.mark_escalation_applied(&self.id);
                        self.emit_feed(
                            "MA",
                            "Maestro",
                            &format!("Can't ship: {}. Stopping instead.", html_escape(&reason)),
                        );
                        return (WorkspaceState::Stopped, 0);
                    }
                }
                self.emit_feed("MA", "Maestro", "Shipping with caveats per your decision.");
                (WorkspaceState::PrOpen, 0)
            }
            CapAnswer::Stop => {
                self.set_state(WorkspaceState::Stopped, "stopped per escalation answer");
                self.eng.store.mark_escalation_applied(&self.id);
                self.emit_feed("MA", "Maestro", "Stopping per your decision.");
                (WorkspaceState::Stopped, 0)
            }
        }
    }

    /// Takes an agent slot. A pending keep-iterating answer is applied once
    /// the turn it grants is about to run.
    pub(crate) async fn acquire_turn_slot(
        &self,
        cancel: &CancellationToken,
    ) -> Option<AgentPermit> {
        let permit = self.eng.acquire_agent(cancel).await?;
        self.mark_cap_answer_applied();
        Some(permit)
    }

    /// Records that a cap answer's continuation is underway. Guidance on the
    /// current answer stays pending until it reaches a prompt.
    pub(crate) fn mark_cap_answer_applied(&self) {
        if self.current_answer_needs_steer() {
            return;
        }
        self.eng.store.mark_escalation_applied(&self.id);
    }

    fn current_answer_needs_steer(&self) -> bool {
        let Some(esc) = self.eng.store.escalation(&self.id) else {
            return false;
        };
        if esc.applied || esc.status != "answered" {
            return false;
        }
        match persisted_escalation_kind(&esc).as_str() {
            ESC_KIND_AGENT => true,
            ESC_KIND_CAP | ESC_KIND_ENSURE_PR => restore_escalation_answer(&esc)
                .is_some_and(|a| classify_cap_answer(&a) == CapAnswer::Guidance),
            _ => false,
        }
    }

    pub(crate) fn policy_extend_rounds(&self) -> i64 {
        policy_extend_rounds(&self.guard, &self.autonomy)
    }

    /// Seeds the run from a plan already on the workspace (a Plan-mode chat
    /// filed it, or Run-as-loop): the planning turn is skipped.
    pub(crate) fn adopt_existing_plan(&mut self) -> bool {
        let Some(ws) = self.eng.store.workspace(&self.id) else {
            return false;
        };
        let Some(plan) = &ws.plan else {
            return false;
        };
        if ws.definition_of_done.trim().is_empty() {
            return false;
        }
        self.plan = truncate(&ws.definition_of_done, PLAN_MAX_PERSIST_CHARS);
        self.plan_structured = !plan.steps.is_empty();
        let state = if plan.approved { "approved" } else { "draft" };
        self.emit_feed(
            crate::codes::MAESTRO_AGENT_CODE,
            "Maestro",
            &format!(
                "Using the existing {state} plan ({}, {} steps): skipping the planning turn.",
                html_escape(&plan.source),
                plan.steps.len()
            ),
        );
        true
    }

    /// The engine's bar after a review turn: remaining opens.
    fn converge_bar_passed(&self) -> bool {
        let th = match self.walk_phase() {
            Some(ph) => self.guard.phase_threshold(&ph),
            None => self.guard.policy.findings_threshold.clone(),
        };
        self.eng.findings_bar_blocks(&self.id, &th).is_none()
    }

    /// Pauses a fresh run after planning for sign-off (Cautious/Balanced
    /// ask, Aggressive proceeds; only structured plans gate unless the
    /// policy says always). The answer's free text is folded into the plan.
    pub(crate) async fn gate_plan_approval(
        &mut self,
        cancel: &CancellationToken,
    ) -> (WorkspaceState, bool) {
        let Some(ws) = self.eng.store.workspace(&self.id) else {
            return (WorkspaceState::default(), false);
        };
        if crate::guard::skip_plan_approval(&self.guard.policy, &ws) {
            return (WorkspaceState::default(), false);
        }
        let Some(plan) = &ws.plan else {
            return (WorkspaceState::default(), false);
        };
        if (plan.steps.is_empty() && self.guard.policy.plan_approval != "always") || plan.approved {
            return (WorkspaceState::default(), false);
        }
        let (ans, keep_going) = self
            .escalate(
                cancel,
                EscalationRequest {
                    question: "Review the plan before I start coding.".into(),
                    context: truncate(&ws.definition_of_done, 4000),
                    options: vec![
                        EscalationOption {
                            text: "Approve and proceed".into(),
                            tag: "recommended".into(),
                            desc: "Start coding to this plan as written.".into(),
                        },
                        EscalationOption {
                            text: "Proceed with my notes".into(),
                            tag: "notes".into(),
                            desc:
                                "Add guidance in the text box: I fold it into the plan and start."
                                    .into(),
                        },
                    ],
                },
                ESC_KIND_PLAN,
            )
            .await;
        if !keep_going {
            self.finish_walk_abort();
            return (WorkspaceState::Stopped, true);
        }
        let t = ans.free_text.trim();
        if !t.is_empty() {
            self.fold_plan_notes(t);
        }
        self.eng.approve_plan(&self.id);
        self.eng.store.mark_escalation_applied(&self.id);
        self.eng.mark_group(&self.id, Group::Active);
        (WorkspaceState::default(), false)
    }

    /// Appends plan-approval guidance to the plan body so every later
    /// re-mirror keeps it.
    pub(crate) fn fold_plan_notes(&mut self, t: &str) {
        let note = format!("\n\nHuman guidance at plan approval:\n{t}");
        let n = note.clone();
        let updated = self.eng.store.update_workspace(&self.id, |lp| {
            if let Some(p) = lp.plan.as_mut() {
                p.body = format!("{}{n}", p.body.trim()).trim().to_owned();
                cap_plan_for_persist(p);
                lp.definition_of_done = truncate(&p.text(), PLAN_MAX_PERSIST_CHARS);
                return;
            }
            lp.definition_of_done = truncate(
                &format!("{}{n}", lp.definition_of_done),
                PLAN_MAX_PERSIST_CHARS,
            );
        });
        self.plan = match updated {
            Some(u) => truncate(&u.definition_of_done, PLAN_MAX_PERSIST_CHARS),
            None => truncate(&format!("{}{note}", self.plan), PLAN_MAX_PERSIST_CHARS),
        };
        self.emit_feed(
            crate::codes::MAESTRO_AGENT_CODE,
            "Maestro",
            &format!(
                "Folding your approval notes into the plan: {}",
                html_escape(&truncate(t, 200))
            ),
        );
    }

    /// Appends one iteration's open-finding count to the convergence chart
    /// and publishes it.
    fn record_convergence(&self, iter: i64, findings: i64) {
        let point = ConvPoint {
            iteration: iter,
            findings,
            color: conv_color(findings).into(),
        };
        let Some(updated) = self.eng.store.update_workspace(&self.id, |lp| {
            let conv = lp.convergence.get_or_insert_with(Vec::new);
            if let Some(p) = conv.iter_mut().find(|p| p.iteration == iter) {
                *p = point;
                return;
            }
            conv.push(point);
        }) else {
            return;
        };
        self.eng.publish_loop_updated(
            &self.id,
            updated.state.clone(),
            &updated.phase,
            updated.iteration,
        );
    }
}

/// The rounds a keep-iterating grant adds: the policy's, else by autonomy.
pub fn policy_extend_rounds(g: &crate::guard::ResolvedGuard, a: &AutonomyLevel) -> i64 {
    if g.policy.extend_rounds > 0 {
        return g.policy.extend_rounds;
    }
    cap_extension(a)
}
