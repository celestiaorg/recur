//! Implementer handoff: the Coder's done notes (what deviated, what worries it,
//! what is unfinished) reach the Reviewer's briefing for the same round, the
//! conductor's context, and, when the plan is structured, a bounded Maestro
//! replan turn before the review.

use recur_agent::display::PLAN_MAX_PERSIST_CHARS;
use recur_agent::{AgentEvent, TurnRequest};
use recur_agent_tools::names::mcp;
use recur_domain::{AgentRole, AgentStep, Plan, Workspace};
use tokio_util::sync::CancellationToken;

use crate::codes::MAESTRO_AGENT_CODE;
use crate::loop_handle::{HANDOFF_MAX_CHARS, accumulate_handoff};
use crate::loops::Walk;
use crate::loops::specialists::display_step_with;
use crate::roles::RoleSpec;
use crate::text::{html_escape, truncate};
use crate::turn::AgentSpec;

/// The replan turn's tools: read-only investigation plus the plan hands.
/// No escalate: a replan that cannot decide leaves the plan as is.
pub fn maestro_replan_tools() -> Vec<String> {
    [
        "Read",
        "Grep",
        "Glob",
        mcp::SUBMIT_PLAN,
        mcp::UPDATE_PLAN_STEP,
        mcp::EDIT_PLAN,
        mcp::LIST_FINDINGS,
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

pub fn maestro_replan_system_prompt(task: &str, plan: &str, handoff: &str) -> String {
    format!(
        "You are the Maestro: the conductor of ONE autonomous coding workflow. The implementer \
just finished a turn and handed off with notes. Decide whether the PLAN needs revising \
before the Reviewer judges the diff. Revise when a step turned out to be too big (split it), \
wrong (drop or rewrite it), or missing (add it). You revise HOW the work is decomposed, never \
WHAT counts as done: the user's task, constraints, and acceptance criteria stay as written: \
do not drop or soften a requirement to fit what landed. If the notes say a requirement \
itself is unachievable, leave the plan and say so; the Reviewer and the human decide. Use \
Read/Grep/Glob only to check what the notes claim. Investigate, do not edit code.\n\n\
To revise: call mcp__recur__submit_plan with the FULL plan (summary, body, every step in \
order: finished steps keep their titles so their status survives; steps other child \
workspaces are working RIGHT NOW keep their titles too, they close when they land). To change one step's \
status, mcp__recur__update_plan_step. If the plan still holds, reply in one sentence and call \
no tools. Be brief.\n\n\
Task: {task}\n\nCurrent plan:\n{plan}\n\nImplementer handoff:\n{handoff}"
    )
}

/// The Reviewer briefing block for this round's handoff.
pub fn handoff_section(notes: &str) -> String {
    let notes = notes.trim();
    if notes.is_empty() {
        return String::new();
    }
    format!(
        "\n\nThe implementer handed off with these notes (deviations from the plan, concerns, \
unfinished work). Judge them: a deviation the plan did not allow, or work left undone \
that the definition of done requires, is a finding. A concern that is real is a finding \
at the severity it deserves:\n\n{notes}"
    )
}

/// A replan that changed the prose or the structured execution metadata
/// (files, depends_on, titles), which the prose mirror omits.
pub fn plan_execution_changed(before: &Workspace, after: &Workspace) -> bool {
    if after.definition_of_done.trim() != before.definition_of_done.trim() {
        return true;
    }
    !plan_shape_equal(before.plan.as_ref(), after.plan.as_ref())
}

pub fn plan_shape_equal(a: Option<&Plan>, b: Option<&Plan>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.steps.len() == b.steps.len()
                && a.summary.trim() == b.summary.trim()
                && a.body.trim() == b.body.trim()
                && a.steps.iter().zip(&b.steps).all(|(x, y)| {
                    x.title == y.title && x.files == y.files && x.depends_on == y.depends_on
                })
        }
        _ => false,
    }
}

impl Walk {
    /// The handoff the Reviewer judges: the board pass's, else this landing's.
    pub(crate) fn review_handoff(&self) -> String {
        let s = self.board_handoff.trim();
        if !s.is_empty() {
            return s.to_owned();
        }
        self.last_handoff.clone()
    }

    /// Stores the round's done notes on the walk and the workspace and shows
    /// them on the feed. An empty note clears this landing but keeps the
    /// board pass's notes: a parent fallback must not wipe its children.
    pub(crate) fn record_handoff(&mut self, code: &str, name: &str, notes: &str) {
        let notes = truncate(notes.trim(), HANDOFF_MAX_CHARS);
        self.last_handoff = notes.clone();
        if !notes.is_empty() {
            self.board_handoff = accumulate_handoff(&self.board_handoff, &notes);
            self.emit_feed(
                code,
                name,
                &format!(
                    "Handoff: {}",
                    html_escape(&truncate(&notes, recur_agent::display::FEED_INPUT_LIMIT))
                ),
            );
        }
        let stored = if self.board_handoff.is_empty() {
            notes
        } else {
            self.board_handoff.clone()
        };
        self.eng
            .store
            .update_workspace(&self.id, |ws| ws.handoff = stored);
    }

    /// Starts a new coding round's notes.
    pub(crate) fn reset_board_handoff(&mut self) {
        self.board_handoff.clear();
        self.last_handoff.clear();
        self.eng
            .store
            .update_workspace(&self.id, |ws| ws.handoff.clear());
    }

    /// The between-rounds replan: one bounded Maestro turn over a non-empty
    /// handoff. The plan tools write the stored plan as they run, so the
    /// walk adopts whatever the store holds either way. True when the plan
    /// changed.
    pub(crate) async fn run_replanner(
        &mut self,
        cancel: &CancellationToken,
        handoff: &str,
    ) -> bool {
        let handoff = handoff.trim();
        if handoff.is_empty() || self.plan.trim().is_empty() {
            return false;
        }
        let Some(before) = self.eng.store.workspace(&self.id) else {
            return false;
        };
        let scope = self.turn_scope(cancel);
        let mut rs = self.slot_role_spec(
            "planner",
            RoleSpec {
                role: AgentRole::Maestro,
                code: MAESTRO_AGENT_CODE.into(),
                name: "Maestro".into(),
                allowed_tools: maestro_replan_tools(),
                ..RoleSpec::default()
            },
        );
        rs.allowed_tools = maestro_replan_tools();
        let agent_id = format!("{}-{}", self.id, rs.code);
        let set = self.eng.settings();
        let (model, flags) =
            self.role_model(&rs, &set.maestro_model().to_string(), &set.maestro_flags());
        self.eng
            .update_agent_status(&self.id, &rs.code, "replanning");

        let mut plan_text = self.plan.clone();
        if let Some(p) = &before.plan
            && !p.steps.is_empty()
        {
            plan_text = p.board();
        }
        let chat_code = self.chat_code();
        let (token, mcp_url) = match self
            .eng
            .register_mcp(&self.id, &rs.code, &chat_code, &rs.allowed_tools, &model)
            .await
        {
            Ok(t) => t,
            Err(_) => {
                self.eng.update_agent_status(&self.id, &rs.code, "idle");
                return false;
            }
        };
        let _mcp = crate::loops::specialists::McpGuard::new(&self.eng, token);

        let mut req = TurnRequest {
            workspace_id: self.id.clone(),
            chat_code,
            agent_id,
            role: AgentRole::Maestro,
            model,
            flags,
            system_prompt: maestro_replan_system_prompt(&self.goal, &plan_text, handoff),
            allowed_tools: Some(rs.allowed_tools.clone()),
            prompt: "Review the implementer handoff against the plan. Revise the plan only if it needs it.".into(),
            worktree: self.worktree.clone().into(),
            permission_mode: set.permission_mode().into(),
            mcp_url,
            ..TurnRequest::default()
        };
        self.apply_goal_images(&mut req);
        let Some(permit) = self.acquire_turn_slot(&scope.token).await else {
            self.eng.update_agent_status(&self.id, &rs.code, "idle");
            return false;
        };
        let eng = self.eng.clone();
        let provider = self.eng.providers().for_model(&req.model);
        let (ws, code) = (self.id.clone(), rs.code.clone());
        let mut spec = AgentSpec::new(req);
        spec.emit = Some(std::sync::Arc::new(move |ev: &AgentEvent| {
            if let AgentEvent::Step(s) = ev
                && !ev.nested()
            {
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
        }));
        let res = self.eng.run_agent(&scope.token, spec).await;
        drop(permit);
        self.eng.publish_resource();
        self.eng.update_agent_status(&self.id, &rs.code, "idle");
        let failed = match &res {
            Ok(r) => r.is_error,
            Err(_) => true,
        };

        let after = self.eng.store.workspace(&self.id);
        let Some(after) = after.filter(|a| plan_execution_changed(&before, a)) else {
            let msg = if failed {
                "Replan turn failed: keeping the current plan."
            } else {
                "Plan still holds."
            };
            self.emit_feed(&rs.code, &rs.name, msg);
            return false;
        };
        self.plan = truncate(&after.definition_of_done, PLAN_MAX_PERSIST_CHARS);
        self.plan_structured = after.plan.as_ref().is_some_and(|p| !p.steps.is_empty());
        let n = after.plan.as_ref().map_or(0, |p| p.steps.len());
        let msg = if failed {
            format!(
                "Replan turn failed after revising the plan: the revision it filed stands ({n} steps)."
            )
        } else {
            format!("Plan revised after the handoff ({n} steps).")
        };
        self.emit_feed(&rs.code, &rs.name, &msg);
        true
    }
}
