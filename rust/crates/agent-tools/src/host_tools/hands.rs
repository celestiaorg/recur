//! Maestro hands: the agent requests, the engine validates and executes within
//! the rails, returning success or the named refusal.

use super::{EMPTY_OBJECT_SCHEMA, schema_summary_only, strict};
use crate::json::{Shape, Ty, field};
use crate::names;
use crate::tool::{Call, Tool, ToolError};

static EDIT_PLAN_IN: Shape = Shape::anon(&[field("plan", Ty::Str)]);

pub struct EditPlanTool;

#[async_trait::async_trait]
impl Tool for EditPlanTool {
    fn name(&self) -> &'static str {
        names::EDIT_PLAN
    }

    fn description(&self) -> &'static str {
        "Rewrite the loop's plan (its definition of done). The engine persists it, logs a \
plan-change deviation, and steers subsequent specialist turns with the new plan. Provide \
the complete new plan text, not a diff."
    }

    fn schema(&self) -> String {
        schema_summary_only("plan")
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &EDIT_PLAN_IN)?;
        if a.str("plan").trim().is_empty() {
            return Err("edit_plan: plan is required".into());
        }
        Ok(h.edit_plan(&c.workspace_id, &c.agent_code, a.str("plan"))
            .await)
    }
}

static NO_ARGS: Shape = Shape::anon(&[]);

pub struct RerunReviewTool;

#[async_trait::async_trait]
impl Tool for RerunReviewTool {
    fn name(&self) -> &'static str {
        names::RERUN_REVIEW
    }

    fn description(&self) -> &'static str {
        "Dispatch a fresh Reviewer pass over the loop's current worktree. Findings are recorded \
like any review and gate the PR as usual."
    }

    fn schema(&self) -> String {
        EMPTY_OBJECT_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        strict(&c.args, &NO_ARGS)?;
        Ok(h.rerun_review(&c.workspace_id, &c.agent_code).await)
    }
}

const DISPATCH_SPECIALIST_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "role": {"type": "string", "enum": ["reviewer","security"], "description": "Which specialist to run"},
    "instruction": {"type": "string", "description": "Concrete instruction for the specialist"}
  },
  "required": ["role","instruction"]
}"#;

static DISPATCH_SPECIALIST_IN: Shape =
    Shape::anon(&[field("role", Ty::Str), field("instruction", Ty::Str)]);

pub struct DispatchSpecialistTool;

#[async_trait::async_trait]
impl Tool for DispatchSpecialistTool {
    fn name(&self) -> &'static str {
        names::DISPATCH_SPECIALIST
    }

    fn description(&self) -> &'static str {
        "Run one bounded specialist turn over the loop's worktree with a concrete instruction. \
role is 'reviewer' (code review, findings) or 'security' (security-focused review). The \
turn is governed like any agent turn; findings are recorded."
    }

    fn schema(&self) -> String {
        DISPATCH_SPECIALIST_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &DISPATCH_SPECIALIST_IN)?;
        let role = a.str("role").trim().to_lowercase();
        if role != "reviewer" && role != "security" {
            return Err(format!(
                "dispatch_specialist: role must be reviewer or security, got {:?}",
                a.str("role")
            )
            .into());
        }
        if a.str("instruction").trim().is_empty() {
            return Err("dispatch_specialist: instruction is required".into());
        }
        Ok(
            h.dispatch_specialist(&c.workspace_id, &c.agent_code, &role, a.str("instruction"))
                .await,
        )
    }
}

const EXTEND_ITERATIONS_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "iterations": {"type": "integer", "minimum": 1, "description": "How many extra fix iterations to request"}
  },
  "required": ["iterations"]
}"#;

static EXTEND_ITERATIONS_IN: Shape = Shape::anon(&[field("iterations", Ty::Int)]);

pub struct ExtendIterationsTool;

#[async_trait::async_trait]
impl Tool for ExtendIterationsTool {
    fn name(&self) -> &'static str {
        names::EXTEND_ITERATIONS
    }

    fn description(&self) -> &'static str {
        "Grant the RUNNING loop extra fix iterations toward convergence. The engine clamps the \
grant to the autonomy level's extension allowance (rail #4 arithmetic is the engine's)."
    }

    fn schema(&self) -> String {
        EXTEND_ITERATIONS_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &EXTEND_ITERATIONS_IN)?;
        let n = a.int("iterations");
        if n < 1 {
            return Err("extend_iterations: iterations must be >= 1".into());
        }
        Ok(h.extend_iterations(&c.workspace_id, &c.agent_code, n).await)
    }
}

const EXTEND_ROUNDS_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "n": {"type": "integer", "minimum": 1, "description": "Extra rounds; default and clamp are the policy extend_rounds"}
  }
}"#;

static EXTEND_ROUNDS_IN: Shape = Shape::anon(&[field("n", Ty::Int)]);

pub struct ExtendRoundsTool;

#[async_trait::async_trait]
impl Tool for ExtendRoundsTool {
    fn name(&self) -> &'static str {
        names::EXTEND_ROUNDS
    }

    fn description(&self) -> &'static str {
        "Grant extra converge rounds. Legal only while the live converge is still \
escalated at the round cap — not agent questions, walk-step waits, or a stopped loop. \
After the loop has stopped, use reenter_phase instead. Optional n defaults to the policy value."
    }

    fn schema(&self) -> String {
        EXTEND_ROUNDS_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &EXTEND_ROUNDS_IN)?;
        Ok(h.extend_rounds(&c.workspace_id, &c.agent_code, a.int("n"))
            .await)
    }
}

const PHASE_ID_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "phase_id": {"type": "string", "description": "Phase id on the engaged loop"}
  },
  "required": ["phase_id"]
}"#;

static PHASE_ID_IN: Shape = Shape::anon(&[field("phase_id", Ty::Str)]);

pub struct ReenterPhaseTool;

#[async_trait::async_trait]
impl Tool for ReenterPhaseTool {
    fn name(&self) -> &'static str {
        names::REENTER_PHASE
    }

    fn description(&self) -> &'static str {
        "Idle only (no live walk: stopped/failed/settled, or paused with no goroutine). \
Start a new walk from a graph phase on the loop's reenter list (usually converge). \
Not for a live walk — including a checkpoint pause (Resume continues that) or a \
round-cap escalation (extend_rounds)."
    }

    fn schema(&self) -> String {
        PHASE_ID_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &PHASE_ID_IN)?;
        if a.str("phase_id").trim().is_empty() {
            return Err("reenter_phase: phase_id is required".into());
        }
        Ok(
            h.reenter_phase(&c.workspace_id, &c.agent_code, a.str("phase_id"))
                .await,
        )
    }
}

pub struct RunPhaseTool;

#[async_trait::async_trait]
impl Tool for RunPhaseTool {
    fn name(&self) -> &'static str {
        names::RUN_PHASE
    }

    fn description(&self) -> &'static str {
        "Run an on-demand phase (e.g. security), then resume the parked graph walk."
    }

    fn schema(&self) -> String {
        PHASE_ID_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &PHASE_ID_IN)?;
        if a.str("phase_id").trim().is_empty() {
            return Err("run_phase: phase_id is required".into());
        }
        Ok(
            h.run_phase(&c.workspace_id, &c.agent_code, a.str("phase_id"))
                .await,
        )
    }
}

pub struct ResumeLoopTool;

#[async_trait::async_trait]
impl Tool for ResumeLoopTool {
    fn name(&self) -> &'static str {
        names::RESUME_LOOP
    }

    fn description(&self) -> &'static str {
        "RESTART a STOPPED or FAILED loop over its existing worktree. The run continues from the \
worktree's current state, steered by the recorded plan. A running loop does not need \
this; a loop settled at pr_open wants send_back instead."
    }

    fn schema(&self) -> String {
        EMPTY_OBJECT_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        strict(&c.args, &NO_ARGS)?;
        Ok(h.resume_loop(&c.workspace_id, &c.agent_code).await)
    }
}
