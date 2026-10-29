use std::collections::HashSet;

use recur_domain::{STEP_DOING, STEP_DONE, STEP_PENDING, STEP_SKIPPED};

use super::strict;
use crate::host::{PlanStepMeta, PlanSubmission};
use crate::json::{Shape, Ty, Val, field, strict_unmarshal, unmarshal_ty};
use crate::names;
use crate::tool::{Call, Tool, ToolError};

const SUBMIT_PLAN_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "summary": {"type": "string", "description": "Short headline of what this change does and why (one or two sentences)"},
    "body": {"type": "string", "description": "The FULL detailed plan as markdown: relevant files, approach, edge cases, and a definition of done. Other agents execute from this, not from chat. Do not leave the detail only in the conversation."},
    "steps": {
      "type": "array",
      "description": "Ordered, concrete checklist — each one actionable sentence naming the file/function and the change. A step may be a string, or an object {title, files, depends_on}: files = repo-relative paths the step touches; depends_on = ids (S1, S2, … by position in this list) that must finish first. Steps with disjoint files and no unmet depends_on can run in parallel.",
      "items": {"anyOf": [
        {"type": "string"},
        {"type": "object", "properties": {
          "title": {"type": "string"},
          "files": {"type": "array", "items": {"type": "string"}},
          "depends_on": {"type": "array", "items": {"type": "string"}}
        }, "required": ["title"]}
      ]}
    },
    "attachments": {
      "type": "array",
      "description": "Optional ids of artifacts (from create_artifact) or user-attached images (⟦attachment:id⟧) that belong to this plan",
      "items": {"type": "string"}
    }
  },
  "required": ["summary","body","steps"]
}"#;

static SUBMIT_PLAN_IN: Shape = Shape::anon(&[
    field("summary", Ty::Str),
    field("body", Ty::Str),
    field("steps", Ty::List(&Ty::Raw)),
    field("attachments", Ty::List(&Ty::Str)),
]);

static STEP_OBJECT: Shape = Shape::anon(&[
    field("title", Ty::Str),
    field("files", Ty::List(&Ty::Str)),
    field("depends_on", Ty::List(&Ty::Str)),
]);

pub struct SubmitPlanTool;

#[async_trait::async_trait]
impl Tool for SubmitPlanTool {
    fn name(&self) -> &'static str {
        names::SUBMIT_PLAN
    }

    fn description(&self) -> &'static str {
        "File the STRUCTURED plan for this task. Put the FULL detailed plan in body (relevant \
files, approach, edge cases, definition of done) — that is what other agents execute \
from, not the chat transcript. Also provide a short summary headline, an ordered \
checklist of concrete steps (each one actionable sentence naming the file/function and \
the change), and optional attachments (artifact ids from create_artifact, or user image \
ids from ⟦attachment:id⟧). The engine persists this as the workspace's plan artifact — \
the human reviews, edits, and approves it in the Plan panel."
    }

    fn schema(&self) -> String {
        SUBMIT_PLAN_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &SUBMIT_PLAN_IN)?;
        if a.str("summary").trim().is_empty() {
            return Err("submit_plan: summary is required".into());
        }
        if a.str("body").trim().is_empty() {
            return Err(
                "submit_plan: body is required — file the full detailed plan, not just a headline"
                    .into(),
            );
        }
        let (steps, step_meta) = parse_plan_steps(&a.strs("steps"))?;
        if steps.is_empty() {
            return Err("submit_plan: at least one non-empty step is required".into());
        }
        let mut attachment_ids = Vec::new();
        let mut seen = HashSet::new();
        for id in a.strs("attachments") {
            let t = id.trim();
            if t.is_empty() || !seen.insert(t.to_string()) {
                continue;
            }
            attachment_ids.push(t.to_string());
            if attachment_ids.len() >= 20 {
                break;
            }
        }
        Ok(h.submit_plan(
            &c.workspace_id,
            &c.agent_code,
            PlanSubmission {
                summary: a.str("summary").into(),
                body: a.str("body").into(),
                steps,
                step_meta,
                attachment_ids,
                step_ids: Vec::new(),
            },
        )
        .await)
    }
}

/// Each step is a string or {title, files, depends_on}. Blank titles are
/// dropped; meta stays aligned with the returned titles.
fn parse_plan_steps(raw: &[String]) -> Result<(Vec<String>, Vec<PlanStepMeta>), ToolError> {
    let mut titles = Vec::new();
    let mut meta = Vec::new();
    for (i, r) in raw.iter().enumerate() {
        if let Ok(Val::Str(title)) = unmarshal_ty(r, &Ty::Str) {
            let t = title.trim();
            if !t.is_empty() {
                titles.push(t.to_string());
                meta.push(PlanStepMeta::default());
            }
            continue;
        }
        let obj = strict_unmarshal(r, &STEP_OBJECT).map_err(|e| {
            ToolError(format!(
                "submit_plan: steps[{i}] must be a string or {{title, files, depends_on}}: {e}"
            ))
        })?;
        let t = obj.str("title").trim();
        if t.is_empty() {
            continue;
        }
        titles.push(t.to_string());
        meta.push(PlanStepMeta {
            files: trim_list(&obj.strs("files")),
            depends_on: trim_list(&obj.strs("depends_on")),
            explicit: true,
        });
    }
    Ok((titles, meta))
}

fn trim_list(items: &[String]) -> Vec<String> {
    items
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

const UPDATE_PLAN_STEP_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "step_id": {"type": "string", "description": "The plan step id to update"},
    "status": {"type": "string", "enum": ["pending","doing","done","skipped"], "description": "The step's new status"},
    "note": {"type": "string", "description": "Optional short note (e.g. why a step was skipped)"}
  },
  "required": ["step_id","status"]
}"#;

static UPDATE_PLAN_STEP_IN: Shape = Shape::anon(&[
    field("step_id", Ty::Str),
    field("status", Ty::Str),
    field("note", Ty::Str),
]);

pub struct UpdatePlanStepTool;

#[async_trait::async_trait]
impl Tool for UpdatePlanStepTool {
    fn name(&self) -> &'static str {
        names::UPDATE_PLAN_STEP
    }

    fn description(&self) -> &'static str {
        "Update one plan step's status (pending/doing/done/skipped) by its step id, with an \
optional short note. Use it as work advances so plan progress stays honest."
    }

    fn schema(&self) -> String {
        UPDATE_PLAN_STEP_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &UPDATE_PLAN_STEP_IN)?;
        if a.str("step_id").trim().is_empty() {
            return Err("update_plan_step: step_id is required".into());
        }
        let status = a.str("status").trim().to_lowercase();
        if ![STEP_PENDING, STEP_DOING, STEP_DONE, STEP_SKIPPED].contains(&status.as_str()) {
            return Err(format!(
                "update_plan_step: status must be pending/doing/done/skipped, got {:?}",
                a.str("status")
            )
            .into());
        }
        Ok(h.update_plan_step(
            &c.workspace_id,
            &c.agent_code,
            a.str("step_id"),
            &status,
            a.str("note"),
        )
        .await)
    }
}
