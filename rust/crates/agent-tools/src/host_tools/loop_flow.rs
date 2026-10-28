use super::{schema_summary_only, strict};
use crate::host::{EscalationOption, EscalationRequest, Evidence};
use crate::json::{Obj, Shape, Ty, field};
use crate::names;
use crate::tool::{Call, Tool, ToolError};

const DONE_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "summary": {"type": "string", "description": "One line: what this turn did"},
    "notes": {"type": "string", "description": "Optional handoff: deviations from the plan, concerns, unfinished work, steps to split. Empty when nothing deviated."}
  },
  "required": ["summary"]
}"#;

static DONE_IN: Shape = Shape::anon(&[field("summary", Ty::Str), field("notes", Ty::Str)]);

pub struct DoneTool;

#[async_trait::async_trait]
impl Tool for DoneTool {
    fn name(&self) -> &'static str {
        names::DONE
    }

    fn description(&self) -> &'static str {
        "Signal that this loop-specialist turn is complete and hand back to the Maestro. \
Provide a short summary of what you did. Use notes for the handoff the Reviewer and \
Maestro cannot see from the diff: deviations from the plan, concerns, work you could \
not finish, or a step that should be split. Leave notes empty when nothing deviated. \
Fails if any findings are still open — close each with update_finding first. \
Not for interactive human chat turns."
    }

    fn schema(&self) -> String {
        DONE_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &DONE_IN)?;
        if a.str("summary").trim().is_empty() {
            return Err("done: summary is required".into());
        }
        h.done(
            &c.workspace_id,
            &c.agent_code,
            a.str("summary"),
            a.str("notes"),
        )?;
        Ok("Acknowledged — turn marked done.".into())
    }
}

const REPORT_RESULT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "passed": {"type": "boolean", "description": "Whether the tests/checks passed"},
    "summary": {"type": "string", "description": "Short summary of the result"}
  },
  "required": ["passed"]
}"#;

// passed must be distinguishable from omitted, so it is a *bool.
static REPORT_RESULT_IN: Shape =
    Shape::anon(&[field("passed", Ty::OptBool), field("summary", Ty::Str)]);

pub struct ReportResultTool;

#[async_trait::async_trait]
impl Tool for ReportResultTool {
    fn name(&self) -> &'static str {
        names::REPORT_RESULT
    }

    fn description(&self) -> &'static str {
        "Report a test/verification result: whether it passed, with a short summary."
    }

    fn schema(&self) -> String {
        REPORT_RESULT_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &REPORT_RESULT_IN)?;
        let Some(passed) = a.opt_bool("passed") else {
            return Err("report_result: passed is required".into());
        };
        h.report_result(&c.workspace_id, &c.agent_code, passed, a.str("summary"));
        Ok(format!(
            "Recorded result: {}.",
            if passed { "passed" } else { "failed" }
        ))
    }
}

static PLAN_IN: Shape = Shape::anon(&[field("summary", Ty::Str)]);

pub struct PlanTool;

#[async_trait::async_trait]
impl Tool for PlanTool {
    fn name(&self) -> &'static str {
        names::PLAN
    }

    fn description(&self) -> &'static str {
        "Record a short plan summary for the task."
    }

    fn schema(&self) -> String {
        schema_summary_only("summary")
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &PLAN_IN)?;
        if a.str("summary").trim().is_empty() {
            return Err("plan: summary is required".into());
        }
        h.plan(&c.workspace_id, &c.agent_code, a.str("summary"));
        Ok("Plan recorded.".into())
    }
}

const ESCALATE_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "question": {"type": "string", "description": "The scoped decision you need from the human"},
    "context": {"type": "string", "description": "One-read context: what you built, what's ambiguous, what you considered"},
    "options": {
      "type": "array",
      "description": "2-4 reasoned options, each with a short trade-off",
      "items": {
        "type": "object",
        "properties": {
          "text": {"type": "string", "description": "The option, e.g. 'Reject with 429'"},
          "tag": {"type": "string", "description": "Short label, e.g. simplest / smoothest / recommended"},
          "desc": {"type": "string", "description": "The trade-off of this option"}
        },
        "required": ["text"]
      }
    }
  },
  "required": ["question"]
}"#;

static OPTION: Shape = Shape::anon(&[
    field("text", Ty::Str),
    field("tag", Ty::Str),
    field("desc", Ty::Str),
]);

/// The escalate and ask_user input.
pub(super) static QUESTION_IN: Shape = Shape::anon(&[
    field("question", Ty::Str),
    field("context", Ty::Str),
    field("options", Ty::List(&Ty::Struct(&OPTION))),
]);

/// Options in order; Err names the tool when one has blank text.
pub(super) fn question_options(
    a: &Obj,
    blank_text: &str,
) -> Result<Vec<EscalationOption>, ToolError> {
    a.objs("options")
        .iter()
        .map(|o| {
            if o.str("text").trim().is_empty() {
                return Err(ToolError::from(blank_text));
            }
            Ok(EscalationOption {
                text: o.str("text").into(),
                tag: o.str("tag").into(),
                desc: o.str("desc").into(),
            })
        })
        .collect()
}

pub struct EscalateTool;

#[async_trait::async_trait]
impl Tool for EscalateTool {
    fn name(&self) -> &'static str {
        names::ESCALATE
    }

    fn description(&self) -> &'static str {
        "Pause the loop and ask the human a scoped decision you should NOT guess on (genuine \
ambiguity, not laziness). Provide a clear question, 2-4 reasoned options each with a \
short trade-off, and one-read context: what you built, what's ambiguous, what you \
considered. The loop blocks until the human answers, then resumes with their guidance."
    }

    fn schema(&self) -> String {
        ESCALATE_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &QUESTION_IN)?;
        if a.str("question").trim().is_empty() {
            return Err("escalate: question is required".into());
        }
        let options = question_options(&a, "escalate: each option needs a non-empty text")?;
        h.escalate(
            &c.workspace_id,
            &c.agent_code,
            EscalationRequest {
                question: a.str("question").into(),
                options,
                context: a.str("context").into(),
            },
        );
        Ok("Escalation raised — the loop is paused until the human answers.".into())
    }
}

const ATTACH_EVIDENCE_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "step": {"type": "string", "description": "What you did/observed in the running app for this step"},
    "screenshot_path": {"type": "string", "description": "Absolute path to a screenshot captured for this step (optional)"},
    "passed": {"type": "boolean", "description": "Whether this verification step passed"}
  },
  "required": ["step","passed"]
}"#;

static ATTACH_EVIDENCE_IN: Shape = Shape::anon(&[
    field("step", Ty::Str),
    field("screenshot_path", Ty::Str),
    field("passed", Ty::OptBool),
]);

pub struct AttachEvidenceTool;

#[async_trait::async_trait]
impl Tool for AttachEvidenceTool {
    fn name(&self) -> &'static str {
        names::ATTACH_EVIDENCE
    }

    fn description(&self) -> &'static str {
        "Record one browser-verification evidence step: a short description of what you \
did/observed in the running app, the absolute path to a screenshot you captured, and \
whether the step passed. The engine collects these to decide whether the verify passed; \
a failing step blocks opening a PR (rail #1)."
    }

    fn schema(&self) -> String {
        ATTACH_EVIDENCE_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &ATTACH_EVIDENCE_IN)?;
        if a.str("step").trim().is_empty() {
            return Err("attach_evidence: step is required".into());
        }
        let Some(passed) = a.opt_bool("passed") else {
            return Err("attach_evidence: passed is required".into());
        };
        h.attach_evidence(
            &c.workspace_id,
            &c.agent_code,
            Evidence {
                step: a.str("step").into(),
                screenshot_path: a.str("screenshot_path").into(),
                passed,
            },
        );
        Ok(format!(
            "Recorded evidence ({}).",
            if passed { "passed" } else { "failed" }
        ))
    }
}

static SEND_BACK_IN: Shape = Shape::anon(&[field("instruction", Ty::Str)]);

pub struct SendBackTool;

#[async_trait::async_trait]
impl Tool for SendBackTool {
    fn name(&self) -> &'static str {
        names::SEND_BACK
    }

    fn description(&self) -> &'static str {
        "Send an open PR back into the loop for rework. Use this when the loop has already opened \
a PR (state pr_open) and the human wants corrections, more work, or an update with the \
latest base branch — plan alone cannot restart a finished loop. Provide a concrete \
instruction for the Coder."
    }

    fn schema(&self) -> String {
        schema_summary_only("instruction")
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &SEND_BACK_IN)?;
        if a.str("instruction").trim().is_empty() {
            return Err("send_back: instruction is required".into());
        }
        let status = h
            .send_back(&c.workspace_id, &c.agent_code, a.str("instruction"))
            .await;
        if status.is_empty() {
            return Ok("Send-back requested.".into());
        }
        Ok(status)
    }
}
