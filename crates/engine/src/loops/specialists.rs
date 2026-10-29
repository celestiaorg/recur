//! The specialist turns of a walk (Go `loop.go`): the Maestro's planning
//! turn, the Coder (with the open-findings nudge and the engine commit),
//! and the separate Reviewer (rail #2), plus the feed projection of their
//! streams.

use std::sync::Arc;

use parking_lot::Mutex;
use recur_agent::display::{FEED_INPUT_LIMIT, PLAN_MAX_PERSIST_CHARS};
use recur_agent::{AgentEvent, Provider, ToolStep, TurnRequest, TurnResult};
use recur_agent_tools::names::mcp;
use recur_domain::{
    Agent, AgentRole, AgentStep, FEED_KIND_NOTE, FeedItem, Finding, parse_model_ref,
};
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::chat_shared::FIRST_WORKSPACE_RENAME_PROMPT;
use crate::codes::MAESTRO_AGENT_CODE;
use crate::loop_registry::LOOP_MAP_SEP;
use crate::loops::failure::LoopError;
use crate::loops::handoff::handoff_section;
use crate::loops::{TurnScope, Walk};
use crate::plan::first_paragraph;
use crate::roles::{
    RoleSpec, coder_conflict_prompt, coder_dispatch_display, coder_disputed_findings_section,
    coder_fix_prompt, coder_open_findings_nudge_prompt, coder_resume_prompt, coder_system_prompt,
    maestro_plan_system_prompt, maestro_plan_tools, plan_section, reviewer_prompt_for, role_spec,
};
use crate::text::{html_escape, truncate};
use crate::turn::AgentSpec;
use crate::turn::oneshot::{loop_commit_message, status_paths};

/// Ends a turn's MCP registration when the turn scope ends.
pub(crate) struct McpGuard {
    eng: Arc<Engine>,
    token: String,
}

impl McpGuard {
    pub fn new(eng: &Arc<Engine>, token: String) -> Self {
        Self {
            eng: Arc::clone(eng),
            token,
        }
    }
}

impl Drop for McpGuard {
    fn drop(&mut self) {
        self.eng.unregister_mcp(&self.token);
    }
}

/// The AgentStep tint for a tool name.
pub fn step_kind(tool: &str) -> &'static str {
    match tool {
        "Read" | "Grep" => "read",
        "Edit" | "Write" | "Delete" => "edit",
        "Bash" => "run",
        _ => "note",
    }
}

/// A step as the agent panel shows it: (tint, "Tool summary").
pub(crate) fn display_step_with(p: Option<&dyn Provider>, s: &ToolStep) -> (String, String) {
    let d = crate::turn::transcript::display_tool(p, &s.name, &s.input);
    (
        step_kind(&d.name).to_owned(),
        format!("{} {}", d.name, s.summary),
    )
}

/// The provider error as the classified turn failure (its partial result
/// carries stderr/subtype/text).
pub(crate) fn turn_failure(
    role: &str,
    scope: &TurnScope,
    out: &Result<TurnResult, recur_agent::ProviderError>,
) -> Option<LoopError> {
    match out {
        Ok(res) if res.is_error => Some(LoopError::turn(role, None, res)),
        Ok(_) => None,
        Err(e) => {
            let e = scope.provider_err(e.clone());
            Some(LoopError::turn(role, Some(&e), &e.partial))
        }
    }
}

/// Feed + agent.step projection of a specialist's stream.
#[derive(Clone)]
pub(crate) struct FeedProjector {
    eng: Arc<Engine>,
    provider: Option<Arc<dyn Provider>>,
    workspace_id: String,
    code: String,
    name: String,
    chat_code: String,
}

impl FeedProjector {
    pub fn new(
        eng: &Arc<Engine>,
        workspace_id: &str,
        rs: &RoleSpec,
        chat_code: &str,
        model: &str,
    ) -> Self {
        Self {
            eng: Arc::clone(eng),
            provider: eng.providers().for_model(model),
            workspace_id: workspace_id.to_owned(),
            code: rs.code.clone(),
            name: rs.name.clone(),
            chat_code: chat_code.to_owned(),
        }
    }

    /// Go `onAgentEvent`.
    pub fn on_event(&self, ev: &AgentEvent) {
        if ev.nested() {
            return;
        }
        let eng = &self.eng;
        let ws = &self.workspace_id;
        match ev {
            AgentEvent::Step(s) => {
                let (kind, text) = display_step_with(self.provider.as_deref(), s);
                eng.append_agent_step(
                    ws,
                    &self.code,
                    AgentStep {
                        kind,
                        text,
                        ..AgentStep::default()
                    },
                );
                let d = crate::turn::transcript::display_tool(
                    self.provider.as_deref(),
                    &s.name,
                    &s.input,
                );
                let mut text = html_escape(&d.name);
                if !s.summary.is_empty() {
                    text.push_str(&format!(": <code>{}</code>", html_escape(&s.summary)));
                }
                eng.emit_workspace_event_item(
                    ws,
                    FeedItem {
                        agent_code: self.code.clone(),
                        name: self.name.clone(),
                        text,
                        tool_use_id: s.id.clone(),
                        chat_code: self.chat_code.clone(),
                        ..FeedItem::default()
                    },
                );
            }
            AgentEvent::ToolResult(r) => {
                eng.attach_tool_result(ws, &r.id, &r.text, r.is_error);
            }
            AgentEvent::Text { text, .. }
            | AgentEvent::Notice { text }
            | AgentEvent::User { text, .. } => {
                let t = truncate(text, FEED_INPUT_LIMIT);
                if !t.is_empty() {
                    eng.emit_workspace_event_item(
                        ws,
                        FeedItem {
                            agent_code: self.code.clone(),
                            name: self.name.clone(),
                            text: html_escape(&t),
                            kind: FEED_KIND_NOTE.into(),
                            ..FeedItem::default()
                        },
                    );
                }
            }
            AgentEvent::Init { session_id, .. } => {
                eng.set_agent_session(ws, &self.code, session_id);
            }
            _ => {}
        }
    }
}

/// What the planning turn's stream yielded.
#[derive(Default)]
struct PlannerHarvest {
    text_blocks: Vec<String>,
    create_plan: String,
}

impl Walk {
    fn agent_record(
        &self,
        rs: &RoleSpec,
        role: &str,
        model: &str,
        status: &str,
        color: &str,
        task: &str,
    ) -> Agent {
        Agent {
            id: format!("{}-{}", self.id, rs.code),
            workspace_id: self.id.clone(),
            code: rs.code.clone(),
            name: rs.name.clone(),
            role: role.into(),
            model: parse_model_ref(model).id,
            status: status.into(),
            color: color.into(),
            task: task.to_owned(),
            cur: -1,
            ..Agent::default()
        }
    }

    /// The Maestro's planning turn (PRD §7.2): a real turn that decomposes
    /// the task into a plan and definition of done before any code. A
    /// genuine failure degrades loudly to the bare task; a transient one is
    /// returned so Retry re-enters the planning checkpoint (D56); an
    /// escalation instead of a plan is left for the next checkpoint.
    pub(crate) async fn run_planner(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<(), LoopError> {
        let scope = self.turn_scope(cancel);
        let rs = self.slot_role_spec(
            "planner",
            RoleSpec {
                role: AgentRole::Maestro,
                code: MAESTRO_AGENT_CODE.into(),
                name: "Maestro".into(),
                allowed_tools: maestro_plan_tools(),
                ..RoleSpec::default()
            },
        );
        let agent_id = format!("{}-{}", self.id, rs.code);
        let set = self.eng.settings();
        let (model, flags) =
            self.role_model(&rs, &set.maestro_model().to_string(), &set.maestro_flags());
        let goal_label = self.goal_label();
        self.eng.upsert_agent(
            &self.id,
            self.agent_record(
                &rs,
                "plans & orchestrates",
                &model,
                "planning",
                "var(--amber)",
                &goal_label,
            ),
        );

        let mut allowed = rs.allowed_tools.clone();
        let chat_code = self.chat_code();
        let nudge = self
            .eng
            .loop_rename_nudge
            .lock()
            .remove(&format!("{}{LOOP_MAP_SEP}{chat_code}", self.id))
            .unwrap_or_default();
        if !nudge.is_empty() {
            allowed.push(mcp::RENAME_SESSION.into());
            if nudge == FIRST_WORKSPACE_RENAME_PROMPT {
                allowed.push(mcp::RENAME_WORKSPACE.into());
                allowed.push(mcp::RENAME_BRANCH.into());
            }
        }
        let (token, mcp_url) = match self
            .eng
            .register_mcp(&self.id, &rs.code, &chat_code, &allowed, &model)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                self.eng.update_agent_status(&self.id, &rs.code, "idle");
                return Err(e.into());
            }
        };
        let _mcp = McpGuard::new(&self.eng, token);
        let (inbox, _inject) = self.h.bind_inject(&rs.code);

        // A structured plan filed during this turn is recognised whatever
        // role code filed it.
        let plan_before = self
            .eng
            .store
            .workspace(&self.id)
            .map(|w| w.definition_of_done)
            .unwrap_or_default();

        let mut sys = maestro_plan_system_prompt(&self.goal);
        let planner_role = self.phase_role_id("planner");
        let goal = self.goal.clone();
        let overlay = self.bundle_system_prompt(&planner_role, "", &goal, "");
        if !overlay.is_empty() {
            sys = overlay;
        }
        let (plan_user, display) = self.expand_phase_prompt(cancel, "plan", &goal).await?;
        let plan_display = if display.is_empty() {
            goal_label
        } else {
            display
        };

        let mut req = TurnRequest {
            workspace_id: self.id.clone(),
            chat_code,
            agent_id,
            role: AgentRole::Maestro,
            model: model.clone(),
            flags,
            system_prompt: sys,
            append_system_prompt: nudge,
            allowed_tools: Some(allowed),
            prompt: plan_user,
            display_text: plan_display,
            worktree: self.worktree.clone().into(),
            permission_mode: set.permission_mode().into(),
            mcp_url,
            ..TurnRequest::default()
        };
        self.apply_goal_images(&mut req);

        let Some(permit) = self.acquire_turn_slot(&scope.token).await else {
            // Cancelled: the outer checkpoint settles the walk.
            return Ok(());
        };

        // Planning turns span tool calls with narration in between; every
        // text block is kept so an empty final result still has the brief.
        let harvest = Arc::new(Mutex::new(PlannerHarvest::default()));
        let provider = self.eng.providers().for_model(&model);
        let (eng, ws, code) = (Arc::clone(&self.eng), self.id.clone(), rs.code.clone());
        let hv = Arc::clone(&harvest);
        let mut spec = AgentSpec::new(req);
        spec.steer = Some(inbox);
        spec.emit = Some(Arc::new(move |ev: &AgentEvent| {
            if ev.nested() {
                return;
            }
            match ev {
                AgentEvent::Text { text, .. } => {
                    let t = text.trim();
                    if !t.is_empty() {
                        hv.lock().text_blocks.push(t.to_owned());
                    }
                }
                AgentEvent::Init { session_id, .. } => {
                    eng.set_agent_session(&ws, &code, session_id)
                }
                AgentEvent::PlanProposal { body } if !body.is_empty() => {
                    hv.lock().create_plan = body.clone();
                }
                AgentEvent::Step(s) => {
                    let (kind, text) = display_step_with(provider.as_deref(), s);
                    eng.append_agent_step(
                        &ws,
                        &code,
                        AgentStep {
                            kind,
                            text,
                            ..AgentStep::default()
                        },
                    );
                }
                _ => {}
            }
        }));
        let out = self.eng.run_agent(&scope.token, spec).await;
        drop(permit);
        self.eng.publish_resource();
        self.eng.update_agent_status(&self.id, &rs.code, "idle");

        if let Some(te) = turn_failure(&rs.name, &scope, &out) {
            let reason = te
                .turn_failure()
                .map(|t| t.reason.clone())
                .unwrap_or_default();
            if te.turn_failure().is_some_and(|t| t.transient) {
                // D56: park at the planning checkpoint; coding without a plan
                // is the bug.
                self.emit_feed(
                    &rs.code,
                    &rs.name,
                    &format!(
                        "The planning turn failed — not starting without a plan. Retry, or switch the planner model in Settings and retry. {}",
                        html_escape(&truncate(&reason, 200))
                    ),
                );
                return Err(te);
            }
            self.emit_feed(
                &rs.code,
                &rs.name,
                &format!(
                    "The planning turn failed — proceeding with the bare task. {}",
                    html_escape(&truncate(&reason, 200))
                ),
            );
            return Ok(());
        }
        if self.h.has_pending_escalation() {
            // Escalated instead of planning: its narration is not a plan.
            return Ok(());
        }
        if let Some(ws) = self.eng.store.workspace(&self.id)
            && let Some(p) = &ws.plan
            && (p.source == "loop-planner"
                || (!p.steps.is_empty() && ws.definition_of_done != plan_before))
        {
            // submit_plan already persisted and mirrored the plan.
            self.plan = truncate(&ws.definition_of_done, PLAN_MAX_PERSIST_CHARS);
            self.plan_structured = !p.steps.is_empty();
            return Ok(());
        }
        let res = out.unwrap_or_default();
        let (blocks, create_plan) = {
            let h = harvest.lock();
            (h.text_blocks.join("\n\n"), h.create_plan.trim().to_owned())
        };
        let from_create_plan = !create_plan.is_empty();
        let mut text = create_plan;
        if text.is_empty() {
            text = res.text.trim().to_owned();
        }
        if text.is_empty() {
            text = blocks;
        }
        if text.is_empty() {
            self.emit_feed(
                &rs.code,
                &rs.name,
                "The planning turn produced no plan — proceeding with the bare task.",
            );
            return Ok(());
        }
        // Bounded: the plan rides in every Coder system prompt.
        let text = truncate(&text, PLAN_MAX_PERSIST_CHARS);
        self.plan = text.clone();
        if from_create_plan {
            self.eng.adopt_prose_plan(&self.id, &rs.code, &text);
            self.plan_structured = false;
        } else {
            let t = text.clone();
            self.eng
                .store
                .update_workspace(&self.id, |lp| lp.definition_of_done = t);
        }
        self.emit_feed(
            &rs.code,
            &rs.name,
            &format!(
                "Plan &amp; definition-of-done: {} <span class=\"muted\">(full plan in the Plan panel)</span>",
                html_escape(&truncate(&first_paragraph(&text), 300))
            ),
        );
        Ok(())
    }

    /// The Coder: edits in the worktree, nudged back while findings stay
    /// open, then the ENGINE commits (PRD §11). Findings close only through
    /// the Coder's update_finding; review re-files anything still broken.
    pub(crate) async fn run_coder(
        &mut self,
        cancel: &CancellationToken,
        iter: i64,
        prev_findings: Vec<Finding>,
    ) -> Result<(), LoopError> {
        let scope = self.turn_scope(cancel);
        let rs = self.slot_role_spec("implementer", role_spec(&AgentRole::Coder));
        let agent_id = format!("{}-{}", self.id, rs.code);
        let set = self.eng.settings();
        let (model, flags) =
            self.role_model(&rs, &set.coder_model().to_string(), &set.coder_flags());
        let goal_label = self.goal_label();
        self.eng.upsert_agent(
            &self.id,
            self.agent_record(
                &rs,
                "writes & edits code",
                &model,
                "working",
                "var(--cyan)",
                &goal_label,
            ),
        );

        // The first post-resume turn continues the killed session (D55).
        let (mut resume_session, resumed) = self.resumed_role(&rs.code);
        let goal = self.goal.clone();
        let mut prompt = goal.clone();
        let mut coder_display = String::new();
        let mut sys = coder_system_prompt(&goal, &self.plan, self.plan_structured);
        let implementer = self.phase_role_id("implementer");
        if !implementer.is_empty() {
            let extra = plan_section(&self.plan);
            sys = self.bundle_system_prompt(&implementer, &sys, &goal, &extra);
        }
        if !self.pending_conflicts.is_empty() {
            // A child-branch merge left conflicts: this turn resolves them.
            prompt = coder_conflict_prompt(&goal, &self.pending_conflicts);
            self.pending_conflicts.clear();
        } else if iter > 1 && !prev_findings.is_empty() {
            prompt = coder_fix_prompt(&goal, &prev_findings);
            coder_display = coder_dispatch_display(&prompt).to_owned();
            if !self.phase_slot("fix").is_empty() {
                let (expanded, display) = self.expand_phase_prompt(cancel, "fix", &prompt).await?;
                prompt = expanded;
                if !display.is_empty() {
                    coder_display = display;
                }
            }
        } else if resumed {
            // Restating the bare goal would invite redoing half-done work.
            prompt = coder_resume_prompt(&goal, iter, !resume_session.is_empty());
        } else if !self.phase_slot("implement").is_empty() {
            let (expanded, display) = self
                .expand_phase_prompt(cancel, "implement", &prompt)
                .await?;
            prompt = expanded;
            if !display.is_empty() {
                coder_display = display;
            }
        }
        if coder_display.is_empty() {
            coder_display = coder_dispatch_display(&prompt).to_owned();
        }
        if coder_display.is_empty() && prompt == goal {
            coder_display = goal_label;
        }

        // The agent's MCP token: findings and done are attributed to this
        // loop + agent; unregistering at turn end unblocks permission waits.
        let chat_code = self.chat_code();
        let (token, mcp_url) = self
            .eng
            .register_mcp(&self.id, &rs.code, &chat_code, &rs.allowed_tools, &model)
            .await?;
        let _mcp = McpGuard::new(&self.eng, token);

        let Some(permit) = self.acquire_turn_slot(&scope.token).await else {
            return Err(scope
                .err()
                .unwrap_or_else(|| LoopError::msg(crate::loops::CONTEXT_CANCELED)));
        };

        let projector = FeedProjector::new(&self.eng, &self.id, &rs, &chat_code, &model);
        // Re-enter with a nudge when the agent stops with findings open
        // (skipped done, ignored the refusal) instead of advancing.
        const MAX_FINDING_NUDGES: usize = 3;
        let mut nudge = 0usize;
        loop {
            let (inbox, inject) = self.h.bind_inject(&rs.code);
            let mut req = TurnRequest {
                workspace_id: self.id.clone(),
                chat_code: chat_code.clone(),
                agent_id: agent_id.clone(),
                role: AgentRole::Coder,
                model: model.clone(),
                flags: flags.clone(),
                system_prompt: sys.clone(),
                append_system_prompt: self.eng.simplify_coder_append(&self.id),
                allowed_tools: Some(rs.allowed_tools.clone()),
                prompt: self.apply_pending_steer(&prompt),
                display_text: coder_display.clone(),
                worktree: self.worktree.clone().into(),
                permission_mode: set.permission_mode().into(),
                mcp_url: mcp_url.clone(),
                resume_session: resume_session.clone(),
                ..TurnRequest::default()
            };
            self.apply_goal_images(&mut req);
            let mut spec = AgentSpec::new(req);
            spec.steer = Some(inbox);
            let p = projector.clone();
            spec.emit = Some(Arc::new(move |ev: &AgentEvent| p.on_event(ev)));
            let out = self.eng.run_agent(&scope.token, spec).await;
            drop(inject);
            self.eng.publish_resource();
            if let Some(te) = turn_failure(&rs.name, &scope, &out) {
                self.eng.update_agent_status(&self.id, &rs.code, "idle");
                // Classified (D56): a transient failure is retryable.
                return Err(te);
            }
            let res = out.unwrap_or_default();
            let open = self.eng.store.open_findings(&self.id).unwrap_or_default();
            if open.is_empty() {
                break;
            }
            // Not a failure: the Reviewer re-judges and the rails still gate
            // any PR.
            if nudge >= MAX_FINDING_NUDGES {
                self.emit_feed(
                    &rs.code,
                    &rs.name,
                    &format!(
                        "{} finding(s) still open after {MAX_FINDING_NUDGES} nudge(s) — proceeding to review anyway.",
                        open.len()
                    ),
                );
                break;
            }
            self.emit_feed(
                &rs.code,
                &rs.name,
                &format!(
                    "{} finding(s) still open — nudging Coder to close them before review.",
                    open.len()
                ),
            );
            prompt = coder_open_findings_nudge_prompt(&goal, &open);
            if !res.session_id.is_empty() {
                resume_session = res.session_id;
            }
            nudge += 1;
        }
        drop(permit);
        self.eng.update_agent_status(&self.id, &rs.code, "idle");

        self.commit_worktree(&rs.code, &rs.name, "").await?;
        let notes = self.h.take_done_notes();
        self.record_handoff(&rs.code, &rs.name, &notes);
        Ok(())
    }

    /// Stages and commits through the walk's own manager as the user's git
    /// identity (D17). Clean trees are a no-op. An empty `msg` is built from
    /// the Coder's done summary or the goal.
    pub(crate) async fn commit_worktree(
        &self,
        code: &str,
        name: &str,
        msg: &str,
    ) -> Result<(), LoopError> {
        let msg = if msg.is_empty() {
            loop_commit_message(
                &self.h.take_done_summary(),
                &self.goal_label(),
                &status_paths(&self.worktree).await,
            )
        } else {
            msg.to_owned()
        };
        let (author, email) = recur_git::identity(&self.worktree)
            .await
            .map_err(|e| LoopError::msg(format!("engine commit identity: {e}")))?;
        let Some(git) = &self.git else {
            return Err(LoopError::msg("engine commit: no worktree manager"));
        };
        let commit = git
            .commit(&self.worktree, &msg, &author, &email)
            .await
            .map_err(|e| LoopError::msg(format!("engine commit: {e}")))?;
        if commit.committed {
            self.eng
                .update_loop_diff(&self.id, commit.additions, commit.deletions)
                .await;
            self.emit_feed(
                code,
                name,
                &format!(
                    "Committed <code>{}</code> (+{} −{}) to <code>{}</code>.",
                    html_escape(&commit.hash),
                    commit.additions,
                    commit.deletions,
                    html_escape(&self.branch())
                ),
            );
        } else {
            self.emit_feed(code, name, "No file changes to commit this iteration.");
        }
        Ok(())
    }

    /// A commit message for a tree no Coder turn wrote (start=review
    /// converged first try): the title model writes it from the diff. ""
    /// on failure.
    pub(crate) async fn reviewed_commit_message(&self, cancel: &CancellationToken) -> String {
        if recur_git::stage(&self.worktree, &[".".to_owned()])
            .await
            .is_err()
        {
            return String::new();
        }
        let Ok(diff) = recur_git::staged_diff(&self.worktree).await else {
            return String::new();
        };
        let recent = recur_git::recent_commit_subjects(&self.worktree, 8).await;
        self.eng
            .generate_commit_message(cancel, &self.id, &self.worktree, &diff, &recent)
            .await
            .unwrap_or_default()
    }

    /// A SEPARATE Reviewer (rail #2, asserted structurally) reviews the
    /// diff and files findings through MCP. Returns this iteration's open
    /// filings (engine-counted, never the Reviewer's words).
    pub(crate) async fn run_reviewer(
        &mut self,
        cancel: &CancellationToken,
        iter: i64,
    ) -> Result<Vec<Finding>, LoopError> {
        let scope = self.turn_scope(cancel);
        let rs = self.slot_role_spec("reviewer", role_spec(&AgentRole::Reviewer));
        let coder = self.slot_role_spec("implementer", role_spec(&AgentRole::Coder));
        if rs.code == coder.code {
            return Err(LoopError::msg(format!(
                "rail #2 violation: reviewer and coder share agent code {:?}",
                rs.code
            )));
        }
        let reviewer_id = format!("{}-{}", self.id, rs.code);
        let coder_id = format!("{}-{}", self.id, coder.code);
        if reviewer_id == coder_id {
            return Err(LoopError::msg(format!(
                "rail #2 violation: reviewer and coder share agent id {:?}",
                reviewer_id
            )));
        }
        let set = self.eng.settings();
        let (model, flags) = self.role_model(
            &rs,
            &set.reviewer_model().to_string(),
            &set.reviewer_flags(),
        );
        let goal_label = self.goal_label();
        self.eng.upsert_agent(
            &self.id,
            self.agent_record(
                &rs,
                "reviews the diff",
                &model,
                "working",
                "var(--indigo)",
                &goal_label,
            ),
        );
        // Every report_finding of this turn counts for this iteration.
        self.h.begin_review_iteration(iter);

        let chat_code = self.chat_code();
        let (token, mcp_url) = self
            .eng
            .register_mcp(&self.id, &rs.code, &chat_code, &rs.allowed_tools, &model)
            .await?;
        let _mcp = McpGuard::new(&self.eng, token);
        let (inbox, _inject) = self.h.bind_inject(&rs.code);

        let goal = self.goal.clone();
        let mut sys = reviewer_prompt_for(&self.eng.store, &self.id, &goal, &self.plan, iter);
        let reviewer_role = self.phase_role_id("reviewer");
        if !reviewer_role.is_empty() {
            let overlay = self.bundle_system_prompt(&reviewer_role, "", &goal, "");
            if !overlay.is_empty() {
                sys = overlay;
                let plan = self.plan.trim();
                if !plan.is_empty() {
                    sys.push_str("\n\nPlan / definition of done:\n");
                    sys.push_str(plan);
                }
                sys.push_str(&coder_disputed_findings_section(
                    &self.eng.store,
                    &self.id,
                    iter,
                ));
            }
        }
        let brief = self.findings_threshold_briefing();
        if !brief.is_empty() {
            sys.push_str("\n\n");
            sys.push_str(&brief);
        }
        sys.push_str(&handoff_section(&self.review_handoff()));
        let mut review_user = format!("Review the current diff for iteration {iter} of: {goal}");
        let mut review_display =
            format!("Review the current diff for iteration {iter} of: {goal_label}");
        let mut slot = if iter > 1 { "rereview" } else { "review" };
        if slot == "rereview" && self.phase_slot(slot).is_empty() {
            slot = "review";
        }
        if !self.phase_slot(slot).is_empty() {
            let (expanded, display) = self.expand_phase_prompt(cancel, slot, &review_user).await?;
            review_user = expanded;
            if !display.is_empty() {
                review_display = display;
            }
        }

        let req = TurnRequest {
            workspace_id: self.id.clone(),
            chat_code: chat_code.clone(),
            agent_id: reviewer_id,
            role: AgentRole::Reviewer,
            model: model.clone(),
            flags,
            system_prompt: sys,
            allowed_tools: Some(rs.allowed_tools.clone()),
            prompt: self.apply_pending_steer(&review_user),
            display_text: review_display,
            worktree: self.worktree.clone().into(),
            permission_mode: set.permission_mode().into(),
            mcp_url,
            ..TurnRequest::default()
        };
        let Some(permit) = self.acquire_turn_slot(&scope.token).await else {
            return Err(scope
                .err()
                .unwrap_or_else(|| LoopError::msg(crate::loops::CONTEXT_CANCELED)));
        };
        let projector = FeedProjector::new(&self.eng, &self.id, &rs, &chat_code, &model);
        let mut spec = AgentSpec::new(req);
        spec.steer = Some(inbox);
        spec.emit = Some(Arc::new(move |ev: &AgentEvent| projector.on_event(ev)));
        let out = self.eng.run_agent(&scope.token, spec).await;
        drop(permit);
        self.eng.publish_resource();
        self.eng.update_agent_status(&self.id, &rs.code, "idle");
        if let Some(te) = turn_failure(&rs.name, &scope, &out) {
            return Err(te);
        }
        Ok(self
            .eng
            .store
            .findings(&self.id)
            .into_iter()
            .filter(|f| f.iteration == iter && f.status == "open")
            .collect())
    }

    /// The base this walk's diffs, PRs and rails compare against: the live
    /// row (a mid-run retarget), else the dispatch snapshot, the project's,
    /// then main.
    pub(crate) fn base_branch(&self) -> String {
        if let Some(rec) = self.eng.store.workspace(&self.id)
            && !rec.base_branch.is_empty()
        {
            return rec.base_branch;
        }
        if !self.base_branch.is_empty() {
            return self.base_branch.clone();
        }
        if let Some(p) = self.eng.store.project(&self.project_id)
            && !p.base_branch.is_empty()
        {
            return p.base_branch;
        }
        "main".into()
    }
}
