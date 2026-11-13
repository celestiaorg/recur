//! Maestro hands: actions the conductor requests through MCP and the ENGINE
//! validates and executes within the rails. Each returns the tool result:
//! success, or the named reason it refused (the refusal also lands on the feed,
//! so chat and feed agree).

use std::sync::Arc;

use recur_agent::display::PLAN_MAX_PERSIST_CHARS;
use recur_agent::{AgentEvent, TurnRequest};
use recur_bundles::{
    ROLE_MODE_REVIEW, VERB_ARTIFACT, VERB_CONVERGE, VERB_ENSURE_PR, VERB_PLAN, VERB_VERIFY,
    find_role,
};
use recur_domain::{Agent, AgentRole, Checkpoint, Workspace, parse_model_ref};
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::codes::loop_owner_code;
use crate::loop_handle::EscalationAnswer;
use crate::loops::converge::{cap_extension, policy_extend_rounds};
use crate::loops::specialists::McpGuard;
use crate::plan::{cap_plan_for_persist, first_paragraph};
use crate::publish::run_progress;
use crate::roles::{
    model_for_slot, role_spec, security_reviewer_role, security_reviewer_system_prompt,
};
use crate::text::{html_escape, plural, truncate};
use crate::turn::AgentSpec;

/// The ad-hoc (chat-dispatched) reviewer prompt.
pub fn dispatch_reviewer_system_prompt(task: &str) -> String {
    format!(
        "You are a code Reviewer dispatched into a workspace. Review READ-ONLY \
(Read, Grep, `git diff`/`git log`): you cannot edit files. For EACH real issue \
call mcp__recur__report_finding with file, line (optional end_line for a range), severity, and a clear message. \
On a re-review, mark prior findings resolved or invalid with mcp__recur__update_finding. \
Before filing, call mcp__recur__list_findings with status \"all\". Never re-file a finding the human \
ignored or marked invalid (updatedBy human), even reworded or at moved lines, and do not duplicate one \
that is still open. On a re-review, verify each open finding against the current code and mark it resolved once the issue \
is gone; if a resolved finding's fix did not hold, file it again as a new finding. If AGENTS.md has a \
`## Reviews` section, follow it on what not to flag. \
Do not file style nits as findings. If the work is sound, report no findings.\n\n\
Instruction: {task}"
    )
}

/// The rerun_review hand's instruction.
pub const RERUN_REVIEW_INSTRUCTION: &str =
    "Re-review the current diff against the base branch and file findings for any real issues.";

impl Engine {
    /// Rewrites a loop's plan (the edit_plan tool and the plan editor):
    /// logged as a deviation and steered into a running walk. A structured
    /// plan takes the text as its body and re-mirrors the prose.
    pub fn edit_plan(&self, workspace_id: &str, plan: &str) -> String {
        let plan = plan.trim();
        if plan.is_empty() {
            return "Refused: the new plan is empty.".into();
        }
        let Some(ws) = self.store.workspace(workspace_id) else {
            return "Refused: that workspace no longer exists.".into();
        };
        if !ws.has_run() {
            return "Refused: only a workspace with a loop has a plan to edit.".into();
        }
        self.store.update_workspace(workspace_id, |l| {
            if let Some(p) = l.plan.as_mut() {
                p.body = plan.to_owned();
                if p.summary.trim().is_empty() {
                    p.summary = first_paragraph(plan);
                }
                p.approved = false;
                cap_plan_for_persist(p);
                l.definition_of_done = truncate(&p.text(), PLAN_MAX_PERSIST_CHARS);
                if let Some(prog) = run_progress(&l.state, l.plan.as_ref()) {
                    l.progress = prog;
                }
                return;
            }
            l.definition_of_done = truncate(plan, PLAN_MAX_PERSIST_CHARS);
        });
        self.emit_deviation(
            workspace_id,
            &format!("Plan edited: {}", html_escape(&truncate(plan, 200))),
            "default = the previous plan",
            "the plan was rewritten (chat or plan editor); specialists are steered with the new plan.",
        );
        if let Some(lp) = self.running_loop_by_id(workspace_id) {
            let mut steer = plan.to_owned();
            if let Some(fresh) = self.store.workspace(workspace_id)
                && !fresh.definition_of_done.trim().is_empty()
            {
                steer = fresh.definition_of_done;
            }
            lp.queue_steer(&format!(
                "The plan (definition of done) has been UPDATED. Work to this plan now:\n{steer}"
            ));
        }
        if let Some(fresh) = self.store.workspace(workspace_id) {
            self.publish_loop_updated(
                &fresh.id,
                fresh.state.clone(),
                &fresh.phase,
                fresh.iteration,
            );
        }
        self.touch_workspace_activity(workspace_id, "Plan edited", self.now());
        "Plan updated: logged as a deviation and steered into subsequent turns.".into()
    }

    /// Runs ONE bounded specialist turn (reviewer | security) over the
    /// worktree. Refused while the loop runs (it already reviews every
    /// iteration; an ad-hoc reviewer would double-count findings). On a
    /// settled loop findings persist at the loop's iteration.
    pub fn dispatch_specialist_turn(
        &self,
        workspace_id: &str,
        role: &str,
        instruction: &str,
    ) -> String {
        let Some(ws) = self.store.workspace(workspace_id) else {
            return "Refused: that workspace no longer exists.".into();
        };
        if ws.worktree_path.is_empty() {
            return "Refused: the workspace has no worktree to inspect.".into();
        }
        if self.running_loop_by_id(workspace_id).is_some() {
            return "Refused: the loop is running; it already reviews every iteration; wait for it to settle.".into();
        }
        let mut rs = role_spec(&AgentRole::Reviewer);
        rs.mode = ROLE_MODE_REVIEW.into();
        let mut sys = dispatch_reviewer_system_prompt(instruction);
        if role == "security" {
            rs = security_reviewer_role();
            rs.mode = ROLE_MODE_REVIEW.into();
            sys = security_reviewer_system_prompt(instruction);
            if let Ok(cat) = self.discover_loop_catalog()
                && let Some(r) = find_role(&cat.roles, "recur.roles.security-reviewer")
            {
                rs.model_slot = r.model_slot.clone();
                if !r.mode.is_empty() {
                    rs.mode = r.mode.clone();
                }
            }
        }
        let set = self.settings();
        let (model, flags) = model_for_slot(
            &set,
            &rs,
            &set.reviewer_model().to_string(),
            &set.reviewer_flags(),
        );
        let agent_id = format!("{}-{}", ws.id, rs.code);
        self.upsert_agent(
            &ws.id,
            Agent {
                id: agent_id.clone(),
                workspace_id: ws.id.clone(),
                code: rs.code.clone(),
                name: rs.name.clone(),
                role: "dispatched from chat".into(),
                model: parse_model_ref(&model).id,
                status: "working".into(),
                color: "var(--indigo)".into(),
                task: truncate(instruction, 120),
                cur: -1,
                ..Agent::default()
            },
        );
        let reply = format!(
            "{} dispatched over the worktree: findings will appear as they are filed.",
            rs.name
        );
        let eng = self.arc();
        let instruction = instruction.to_owned();
        tokio::spawn(async move {
            eng.run_dispatched_specialist(ws, rs, sys, model, flags, agent_id, instruction)
                .await;
        });
        reply
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_dispatched_specialist(
        self: Arc<Self>,
        ws: Workspace,
        rs: crate::roles::RoleSpec,
        sys: String,
        model: String,
        flags: std::collections::BTreeMap<String, String>,
        agent_id: String,
        instruction: String,
    ) {
        let set = self.settings();
        let cancel = CancellationToken::new();
        let timer = {
            let t = cancel.clone();
            let d = set.agent_turn_timeout();
            tokio::spawn(async move {
                tokio::time::sleep(d).await;
                t.cancel();
            })
        };
        self.dispatched_specialist_turn(
            &cancel,
            &ws,
            &rs,
            sys,
            model,
            flags,
            agent_id,
            instruction,
        )
        .await;
        timer.abort();
        self.update_agent_status(&ws.id, &rs.code, "idle");
    }

    #[allow(clippy::too_many_arguments)]
    async fn dispatched_specialist_turn(
        self: &Arc<Self>,
        cancel: &CancellationToken,
        ws: &Workspace,
        rs: &crate::roles::RoleSpec,
        sys: String,
        model: String,
        flags: std::collections::BTreeMap<String, String>,
        agent_id: String,
        instruction: String,
    ) {
        let Some(permit) = self.acquire_agent(cancel).await else {
            self.emit_workspace_event(
                &ws.id,
                &rs.code,
                &rs.name,
                "Could not start: every agent slot is busy.",
            );
            return;
        };
        let owner = loop_owner_code(ws);
        let (token, mcp_url) = match self
            .register_mcp(&ws.id, &rs.code, &owner, &rs.allowed_tools, &model)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                self.emit_workspace_event(
                    &ws.id,
                    &rs.code,
                    &rs.name,
                    &format!("Could not start: {e}"),
                );
                return;
            }
        };
        let _mcp = McpGuard::new(self, token);
        let set = self.settings();
        let req = TurnRequest {
            workspace_id: ws.id.clone(),
            chat_code: owner,
            agent_id,
            role: AgentRole::Reviewer,
            model,
            flags,
            system_prompt: sys,
            allowed_tools: Some(rs.allowed_tools.clone()),
            prompt: instruction,
            worktree: ws.worktree_path.clone().into(),
            permission_mode: set.permission_mode().into(),
            mcp_url,
            ..TurnRequest::default()
        };
        let (eng, id, code) = (Arc::clone(self), ws.id.clone(), rs.code.clone());
        let mut spec = AgentSpec::new(req);
        spec.emit = Some(Arc::new(move |ev: &AgentEvent| {
            if let AgentEvent::Init { session_id, .. } = ev {
                eng.set_agent_session(&id, &code, session_id);
            }
        }));
        let out = self.run_agent(cancel, spec).await;
        drop(permit);
        self.publish_resource();
        let failed = match &out {
            Ok(r) if r.is_error => {
                Some(crate::text::first_non_empty_str(&r.text, "<nil>").to_owned())
            }
            Ok(_) => None,
            Err(e) => {
                Some(crate::text::first_non_empty_str(&e.partial.text, &e.to_string()).to_owned())
            }
        };
        if let Some(why) = failed {
            self.emit_workspace_event(
                &ws.id,
                &rs.code,
                &rs.name,
                &format!("Dispatched turn failed: {}", html_escape(&why)),
            );
            return;
        }
        let open = self.store.open_findings_count(&ws.id) as i64;
        self.emit_workspace_event(
            &ws.id,
            &rs.code,
            &rs.name,
            &format!(
                "Dispatched turn finished: {} now open.",
                plural(open, "finding", "findings")
            ),
        );
    }

    /// Grants a RUNNING loop extra fix iterations, clamped to the autonomy
    /// allowance (the same arithmetic a cap escalation grants).
    pub fn extend_loop_iterations(&self, workspace_id: &str, agent_code: &str, n: i64) -> String {
        let Some(lp) = self.running_loop_for_hand(workspace_id, agent_code) else {
            return "Refused: the loop is not running; there is no iteration budget to extend."
                .into();
        };
        let autonomy = lp.lock().autonomy.clone();
        let allow = cap_extension(&autonomy);
        let granted = n.min(allow);
        if granted < 1 {
            return "Refused: nothing to grant.".into();
        }
        lp.grant_extension(granted);
        self.emit_workspace_event(
            workspace_id,
            "MA",
            "Maestro",
            &format!(
                "Extended the convergence budget by {granted} iteration(s) (autonomy allows at most {allow} per grant)."
            ),
        );
        if granted < n {
            return format!(
                "Granted {granted} extra iteration(s): clamped from {n} by the autonomy level's allowance."
            );
        }
        format!("Granted {granted} extra iteration(s).")
    }

    /// The human keep-iterating grant while the live converge is escalated
    /// at its round cap.
    pub async fn extend_loop_rounds(&self, workspace_id: &str, agent_code: &str, n: i64) -> String {
        let Some(lp) = self.running_loop_for_hand(workspace_id, agent_code) else {
            return "Refused: the loop is not running; there is no round budget to extend.".into();
        };
        let (at_cap, guard, autonomy) = {
            let s = lp.lock();
            (
                s.escalated && s.round_cap_esc,
                Arc::clone(&s.guard),
                s.autonomy.clone(),
            )
        };
        if !at_cap {
            return "Refused: extend_rounds is only legal when the live converge is escalated at the round cap.".into();
        }
        let allow = policy_extend_rounds(&guard, &autonomy).max(1);
        let mut granted = n;
        if granted < 1 {
            granted = allow;
        }
        granted = granted.min(allow);
        if self
            .answer_escalation_to(
                workspace_id,
                Some(lp),
                "",
                EscalationAnswer {
                    choice: "Keep iterating".into(),
                    grant: granted,
                    ..EscalationAnswer::default()
                },
            )
            .await
            .is_none()
        {
            return "Refused: that cap question was already answered.".into();
        }
        self.emit_workspace_event(
            workspace_id,
            "MA",
            "Maestro",
            &format!(
                "Extended the convergence budget by {granted} round(s) (policy allows at most {allow} per grant)."
            ),
        );
        format!("Granted {granted} extra round(s).")
    }

    /// Starts a new graph walk from `phase_id`. Idle only; the phase must be
    /// on the loop's reenter list.
    pub async fn reenter_phase(
        &self,
        workspace_id: &str,
        agent_code: &str,
        phase_id: &str,
    ) -> String {
        if let Some(lp) = self.running_loop_for_hand(workspace_id, agent_code) {
            if lp.is_paused() {
                return "Refused: reenter_phase is idle only; this walk is still running (paused at a checkpoint). Resume continues it.".into();
            }
            return "Refused: reenter_phase is idle only.".into();
        }
        let Some(ws) = self.store.workspace(workspace_id) else {
            return "Refused: that workspace no longer exists.".into();
        };
        let mut rec = self.chat_scoped_workspace(ws, agent_code);
        let g = match self.resolve_guard(&rec.loop_id, &rec.policy_id, &rec.autonomy) {
            Ok(g) => g,
            Err(reason) => return format!("Refused: {reason}"),
        };
        let phase_id = phase_id.trim();
        if !g.loop_def.reenter_ids().iter().any(|id| id == phase_id) {
            return format!("Refused: {phase_id} is not on this loop's reenter list.");
        }
        rec.checkpoint = Some(Checkpoint {
            phase_id: phase_id.to_owned(),
            ..Checkpoint::default()
        });
        rec.retryable = true;
        self.start_scoped_walk(rec).await
    }

    /// Runs a dispatch=on_demand phase, then resumes the parked walk. A live
    /// walk queues it for its next graph checkpoint.
    pub async fn run_on_demand_phase(
        &self,
        workspace_id: &str,
        agent_code: &str,
        phase_id: &str,
    ) -> String {
        let phase_id = phase_id.trim();
        if let Some(lp) = self.running_loop_for_hand(workspace_id, agent_code) {
            let (guard, cur, gated) = {
                let s = lp.lock();
                (
                    Arc::clone(&s.guard),
                    s.current_phase.clone(),
                    s.paused || s.escalated,
                )
            };
            let Some(ph) = guard
                .loop_def
                .phase_by_id(phase_id)
                .filter(|p| p.dispatch == "on_demand")
            else {
                return "Refused: run_phase only accepts on-demand phase ids.".into();
            };
            // The walk's current phase, else its checkpointed one.
            let cur = if cur.id.trim().is_empty() {
                self.store
                    .workspace(workspace_id)
                    .and_then(|ws| ws.checkpoint)
                    .and_then(|cp| guard.loop_def.phase_by_id(cp.phase_id.trim()).cloned())
            } else {
                Some(cur)
            };
            if let Some(cur) = cur {
                match cur.uses.as_str() {
                    VERB_CONVERGE | VERB_VERIFY => {
                        return format!(
                            "Refused: cannot nest run_phase inside a live {} phase.",
                            cur.uses
                        );
                    }
                    VERB_ENSURE_PR | VERB_ARTIFACT => {
                        return "Refused: the current phase is finishing; wait until it settles, then run_phase.".into();
                    }
                    VERB_PLAN if !gated => {
                        return "Refused: cannot nest run_phase inside a live plan phase.".into();
                    }
                    _ => {}
                }
            }
            lp.lock().pending_on_demand = ph.id.trim().to_owned();
            return "Queued: will run at the next graph checkpoint, then resume the parked walk."
                .into();
        }
        let Some(ws) = self.store.workspace(workspace_id) else {
            return "Refused: that workspace no longer exists.".into();
        };
        let mut rec = self.chat_scoped_workspace(ws.clone(), agent_code);
        let g = match self.resolve_guard(&rec.loop_id, &rec.policy_id, &rec.autonomy) {
            Ok(g) => g,
            Err(reason) => return format!("Refused: {reason}"),
        };
        let Some(ph) = g
            .loop_def
            .phase_by_id(phase_id)
            .filter(|p| p.dispatch == "on_demand")
        else {
            return "Refused: run_phase only accepts on-demand phase ids.".into();
        };
        // simplify: the workspace checkpoint is shared; persist per-chat when
        // two idle walks must independently resume a parked phase.
        let owner = agent_code.trim();
        let mut resume = String::new();
        if let Some(cp) = &rec.checkpoint
            && self.running_loop_by_id(workspace_id).is_none()
            && (owner.is_empty() || owner == ws.loop_chat_code.trim())
        {
            resume = cp.phase_id.clone();
        }
        rec.checkpoint = Some(Checkpoint {
            phase_id: ph.id.clone(),
            resume_phase_id: resume,
            ..Checkpoint::default()
        });
        rec.retryable = true;
        self.start_scoped_walk(rec).await
    }

    /// Rebuilds the run record from this chat's `[[loop]]` / `[[policy]]`
    /// and goal.
    pub fn chat_scoped_workspace(&self, mut ws: Workspace, agent_code: &str) -> Workspace {
        let code = agent_code.trim();
        if code.is_empty() {
            return ws;
        }
        let owner = ws.loop_chat_code.trim().to_owned();
        ws.loop_chat_code = code.to_owned();
        let (lid, pid) = self.chat_loop_policy(&ws.id, code);
        if !lid.is_empty() {
            ws.loop_id = lid;
        }
        if !pid.is_empty() {
            ws.policy_id = pid;
        }
        let (g, d) = self.chat_loop_goal(&ws.id, code);
        ws.replace_goal(&g, &d);
        // simplify: the workspace checkpoint is shared; until per-chat
        // checkpoints exist, drop a sibling's (or off-graph) phase so the
        // walk starts at this loop's entry.
        if let Some(cp) = &ws.checkpoint {
            let owned = owner.is_empty() || owner == code;
            if !owned {
                ws.checkpoint = None;
            } else if let Ok(g) = self.resolve_guard(&ws.loop_id, &ws.policy_id, &ws.autonomy)
                && g.loop_def.phase_by_id(cp.phase_id.trim()).is_none()
            {
                ws.checkpoint = None;
            }
        }
        ws
    }

    /// Writes a chat-scoped run's identity onto the row so Resume reloads
    /// this chat's graph (and, for a scoped walk, its retryable mark).
    pub(crate) fn persist_scoped_identity(&self, rec: &Workspace, with_retryable: bool) {
        let rec = rec.clone();
        self.store.update_workspace(&rec.id, |l| {
            let c = rec.loop_chat_code.trim();
            if !c.is_empty() {
                l.loop_chat_code = c.to_owned();
            }
            let id = rec.loop_id.trim();
            if !id.is_empty() {
                l.loop_id = id.to_owned();
            }
            let id = rec.policy_id.trim();
            if !id.is_empty() {
                l.policy_id = id.to_owned();
            }
            l.replace_goal(&rec.goal, &rec.goal_display);
            l.checkpoint = rec.checkpoint.clone();
            if with_retryable {
                l.retryable = rec.retryable;
            }
        });
    }

    /// Starts this chat's walk: a live sibling keeps its key; an idle
    /// workspace records this chat as the owner so Resume walks its graph.
    pub(crate) async fn start_scoped_walk(&self, rec: Workspace) -> String {
        let d = self.dispatch_mu.lock().await;
        if !rec.loop_chat_code.trim().is_empty() && self.running_loop_by_id(&rec.id).is_some() {
            if self
                .running_loop_by_chat(&rec.id, &rec.loop_chat_code)
                .is_some()
            {
                return "Refused: reenter_phase is idle only.".into();
            }
            return self.start_additional_chat_walk(rec).await;
        }
        let reason = self.idle_walk_refuse(&rec.id);
        if !reason.is_empty() {
            return reason;
        }
        self.persist_scoped_identity(&rec, true);
        self.dispatch_idle_walk_locked(&d, &rec.id).await
    }

    /// Host `resume_loop`: restart this chat's walk (a second walk beside a
    /// live sibling), else the workspace's.
    pub async fn resume_loop_hand(&self, workspace_id: &str, agent_code: &str) -> String {
        if self
            .running_loop_by_chat(workspace_id, agent_code)
            .is_some()
        {
            return "Refused: the loop is already running.".into();
        }
        if !agent_code.trim().is_empty() && self.running_loop_by_id(workspace_id).is_some() {
            let Some(ws) = self.store.workspace(workspace_id) else {
                return "Refused: that workspace no longer exists.".into();
            };
            let mut rec = self.chat_scoped_workspace(ws, agent_code);
            rec.retryable = true;
            rec.checkpoint = None;
            return self.start_additional_chat_walk(rec).await;
        }
        self.restart(workspace_id).await
    }
}
