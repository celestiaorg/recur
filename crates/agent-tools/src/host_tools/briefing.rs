//! Ask Recur coordinator tools: list_workspace_briefs, get_workspace_context,
//! present_workspace, message_workspace, answer_question.

use super::{marshal, strict};
use crate::json::{Shape, Ty, field, unmarshal};
use crate::names;
use crate::tool::{Call, Tool, ToolError};

fn require_app_coordinator(c: &Call) -> Result<(), ToolError> {
    if !c.agent_code.starts_with("AP") {
        return Err("this tool is only available to the Ask Recur coordinator".into());
    }
    Ok(())
}

/// Pick up rows a plain call lists (the most recent); the rest are counted in `omitted`.
const PICKUP_SHOWN: usize = 25;
/// Free-text fields (now, next, last summary) per row.
const BRIEF_TEXT_MAX: usize = 240;

static LIST_BRIEFS_IN: Shape =
    Shape::anon(&[field("bucket", Ty::OptStr), field("project", Ty::OptStr)]);

pub struct ListWorkspaceBriefsTool;

#[async_trait::async_trait]
impl Tool for ListWorkspaceBriefsTool {
    fn name(&self) -> &'static str {
        names::LIST_WORKSPACE_BRIEFS
    }

    fn description(&self) -> &'static str {
        "List the current Briefing overview: every workspace that needs attention or is \
working, and the 25 most recent that can be picked up (omitted counts the rest; pass \
bucket=pickup for all of them), with deterministic Now/Next copy. Filter with bucket \
(needs, working, pickup) and project (name or id). Always re-query before a factual status \
claim: the chat transcript is not current truth. App coordinator only."
    }

    fn schema(&self) -> String {
        r#"{
  "type": "object",
  "properties": {
    "bucket": {"type": "string", "enum": ["needs", "working", "pickup"], "description": "Only this bucket, every row of it"},
    "project": {"type": "string", "description": "Only this project (name or id)"}
  }
}"#
        .into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        require_app_coordinator(&c)?;
        let h = c.host()?;
        let a = strict(&c.args, &LIST_BRIEFS_IN)?;
        let bucket = a.opt_str("bucket").unwrap_or_default().trim().to_owned();
        let project = a.opt_str("project").unwrap_or_default().trim().to_owned();
        let snap = h.list_workspace_briefs().await;
        marshal(&lean_briefs(&snap, &bucket, &project))
            .map_err(|e| ToolError(format!("list_workspace_briefs: {e}")))
    }
}

/// The agent-facing list: the fields a coordinator reasons with (no UI routes or raw
/// timestamps), free text clipped, and Pick up capped unless asked for by bucket.
fn lean_briefs(
    snap: &recur_domain::BriefingSnapshot,
    bucket: &str,
    project: &str,
) -> serde_json::Value {
    use serde_json::{Map, Value, json};
    let clip = |s: &str| -> String {
        let s = s.trim();
        match s.char_indices().nth(BRIEF_TEXT_MAX) {
            Some((i, _)) => format!("{}…", &s[..i]),
            None => s.to_owned(),
        }
    };
    let rows: Vec<_> = snap
        .workspaces
        .iter()
        .flatten()
        .filter(|b| {
            (bucket.is_empty() || b.bucket == bucket)
                && (project.is_empty()
                    || b.project_id == project
                    || b.project_name.eq_ignore_ascii_case(project))
        })
        .collect();
    let counts = if bucket.is_empty() && project.is_empty() {
        snap.counts.clone()
    } else {
        use recur_domain::{BRIEF_BUCKET_NEEDS, BRIEF_BUCKET_PICKUP, BRIEF_BUCKET_WORKING};
        let n = |k: &str| rows.iter().filter(|b| b.bucket == k).count() as i64;
        recur_domain::BriefingCounts {
            needs: n(BRIEF_BUCKET_NEEDS),
            working: n(BRIEF_BUCKET_WORKING),
            pickup: n(BRIEF_BUCKET_PICKUP),
        }
    };
    let (mut out, mut pickup) = (Vec::new(), 0);
    for b in rows {
        if b.bucket == "pickup" && bucket.is_empty() {
            pickup += 1;
            if pickup > PICKUP_SHOWN {
                continue;
            }
        }
        let mut o = Map::new();
        let mut put = |k: &str, v: Value| {
            let empty = match &v {
                Value::String(s) => s.is_empty(),
                Value::Number(n) => n.as_i64() == Some(0),
                Value::Bool(b) => !b,
                Value::Array(a) => a.is_empty(),
                _ => false,
            };
            if !empty {
                o.insert(k.into(), v);
            }
        };
        put("workspaceId", json!(b.workspace_id));
        put("title", json!(b.title));
        put("projectName", json!(b.project_name));
        put("branch", json!(b.branch));
        put("bucket", json!(b.bucket));
        put("reasons", json!(b.reasons.clone().unwrap_or_default()));
        put("state", json!(b.state));
        put("now", json!(clip(&b.now)));
        put("next", json!(clip(&b.next)));
        put("lastSummary", json!(clip(&b.last_summary)));
        put("quietAge", json!(b.quiet_age));
        put("changedSinceViewed", json!(b.changed_since_viewed));
        put("planSummary", json!(clip(&b.plan_summary)));
        put("planDone", json!(b.plan_done));
        put("planTotal", json!(b.plan_total));
        put("activeAgents", json!(b.active_agents));
        put("activeAgentTask", json!(clip(&b.active_agent_task)));
        put("latestChatTitle", json!(b.latest_chat_title));
        put("diffAdd", json!(b.diff_add));
        put("diffDel", json!(b.diff_del));
        put("prNumber", json!(b.pr_number));
        out.push(Value::Object(o));
    }
    let mut res = json!({"counts": counts, "workspaces": out});
    if pickup > PICKUP_SHOWN {
        res["omitted"] = json!({"pickup": pickup - PICKUP_SHOWN});
    }
    res
}

static WORKSPACE_ID_IN: Shape = Shape::anon(&[field("workspace_id", Ty::Str)]);

pub struct GetWorkspaceContextTool;

#[async_trait::async_trait]
impl Tool for GetWorkspaceContextTool {
    fn name(&self) -> &'static str {
        names::GET_WORKSPACE_CONTEXT
    }

    fn description(&self) -> &'static str {
        "Get bounded detail for one workspace: plan steps, active agents, open asks, \
recent chat headers, PR state, and a short conversation excerpt. App coordinator only."
    }

    fn schema(&self) -> String {
        r#"{
  "type": "object",
  "properties": {
    "workspace_id": {"type": "string", "description": "Workspace id from list_workspace_briefs"}
  },
  "required": ["workspace_id"]
}"#
        .into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        require_app_coordinator(&c)?;
        let h = c.host()?;
        let a = unmarshal(&c.args, &WORKSPACE_ID_IN)
            .map_err(|_| ToolError::from("get_workspace_context: invalid arguments"))?;
        let id = a.str("workspace_id").trim();
        if id.is_empty() {
            return Err("get_workspace_context: workspace_id is required".into());
        }
        let ctx = h.workspace_context(id).await?;
        marshal(&ctx).map_err(|e| ToolError(format!("get_workspace_context: {e}")))
    }
}

static READ_WORKSPACE_IN: Shape =
    Shape::anon(&[field("workspace_id", Ty::Str), field("diff", Ty::OptBool)]);

pub struct ReadWorkspaceTool;

#[async_trait::async_trait]
impl Tool for ReadWorkspaceTool {
    fn name(&self) -> &'static str {
        names::READ_WORKSPACE
    }

    fn description(&self) -> &'static str {
        "Look into one workspace, running or finished: its state, latest reply (partial while \
running), handoff, plan, findings, files changed against the project's base branch, \
uncommitted files, artifacts, and the last tool calls of its most recent chat. Use it for \
progress questions. App coordinator only."
    }

    fn schema(&self) -> String {
        r#"{
  "type": "object",
  "properties": {
    "workspace_id": {"type": "string", "description": "Workspace id from list_workspace_briefs"},
    "diff": {"type": "boolean", "description": "Include the diff bodies (default false: file lists only)"}
  },
  "required": ["workspace_id"]
}"#
        .into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        require_app_coordinator(&c)?;
        let h = c.host()?;
        let a = strict(&c.args, &READ_WORKSPACE_IN)?;
        let id = a.str("workspace_id").trim();
        if id.is_empty() {
            return Err("read_workspace: workspace_id is required".into());
        }
        Ok(h.read_workspace(id, a.opt_bool("diff").unwrap_or(false))
            .await)
    }
}

static TARGET_IN: Shape = Shape::anon(&[field("target", Ty::Str)]);

pub struct PresentWorkspaceTool;

#[async_trait::async_trait]
impl Tool for PresentWorkspaceTool {
    fn name(&self) -> &'static str {
        names::PRESENT_WORKSPACE
    }

    fn description(&self) -> &'static str {
        "Pin a workspace, chat, question, or PR card in the Ask Recur transcript \
for the human to open. Does not switch the visible UI. target is workspace:<id> \
(optional :plan, :findings, or :review), chat:<workspaceId>:<code>, question:<id>, or pr:<id>. \
Validated against live state. \
App coordinator only."
    }

    fn schema(&self) -> String {
        r#"{
  "type": "object",
  "properties": {
    "target": {"type": "string", "description": "workspace:<id> (optional :plan, :findings, :review), chat:<workspaceId>:<code>, question:<id>, or pr:<id>"}
  },
  "required": ["target"]
}"#
        .into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        require_app_coordinator(&c)?;
        let h = c.host()?;
        let a = unmarshal(&c.args, &TARGET_IN)
            .map_err(|_| ToolError::from("present_workspace: invalid arguments"))?;
        Ok(h.present_workspace(a.str("target").trim()).await)
    }
}

static MESSAGE_WORKSPACE_IN: Shape =
    Shape::anon(&[field("workspace_id", Ty::Str), field("message", Ty::Str)]);

pub struct MessageWorkspaceTool;

#[async_trait::async_trait]
impl Tool for MessageWorkspaceTool {
    fn name(&self) -> &'static str {
        names::MESSAGE_WORKSPACE
    }

    fn description(&self) -> &'static str {
        "Direct any workspace's agent. A running agent is steered mid-turn and continues; \
an idle workspace starts a new Agent chat turn with the message. A top-level workspace then \
reports back here in a new turn when it finishes. Refused while it waits on a question (use \
answer_question) or is archived. App coordinator only."
    }

    fn schema(&self) -> String {
        r#"{
  "type": "object",
  "properties": {
    "workspace_id": {"type": "string", "description": "Workspace id from list_workspace_briefs"},
    "message": {"type": "string", "description": "Instruction for the workspace's agent, with the context it needs"}
  },
  "required": ["workspace_id", "message"]
}"#
        .into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        require_app_coordinator(&c)?;
        let h = c.host()?;
        let a = strict(&c.args, &MESSAGE_WORKSPACE_IN)?;
        let (id, message) = (a.str("workspace_id").trim(), a.str("message").trim());
        if id.is_empty() || message.is_empty() {
            return Err("message_workspace: workspace_id and message are required".into());
        }
        Ok(h.message_workspace(&c.agent_code, id, message).await)
    }
}

static ANSWER_QUESTION_IN: Shape =
    Shape::anon(&[field("question_id", Ty::Str), field("answer", Ty::Str)]);

pub struct AnswerQuestionTool;

#[async_trait::async_trait]
impl Tool for AnswerQuestionTool {
    fn name(&self) -> &'static str {
        names::ANSWER_QUESTION
    }

    fn description(&self) -> &'static str {
        "Answer a workspace agent's pending question (an open ask of kind question) on the \
human's behalf; the agent resumes with the answer. Only relay what the human decided, never \
answer on your own. App coordinator only."
    }

    fn schema(&self) -> String {
        r#"{
  "type": "object",
  "properties": {
    "question_id": {"type": "string", "description": "Question id from get_workspace_context open_asks"},
    "answer": {"type": "string", "description": "The human's answer"}
  },
  "required": ["question_id", "answer"]
}"#
        .into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        require_app_coordinator(&c)?;
        let h = c.host()?;
        let a = strict(&c.args, &ANSWER_QUESTION_IN)?;
        let (id, answer) = (a.str("question_id").trim(), a.str("answer").trim());
        if id.is_empty() || answer.is_empty() {
            return Err("answer_question: question_id and answer are required".into());
        }
        Ok(h.answer_question(id, answer).await)
    }
}

static CREATE_PROJECT_IN: Shape = Shape::anon(&[
    field("name", Ty::Str),
    field("path", Ty::OptStr),
    field("clone_url", Ty::OptStr),
]);

pub struct CreateProjectTool;

#[async_trait::async_trait]
impl Tool for CreateProjectTool {
    fn name(&self) -> &'static str {
        names::CREATE_PROJECT
    }

    fn description(&self) -> &'static str {
        "Add a project to Recur, creating it on disk when it does not exist yet: a new folder \
becomes a git repo on main with an empty first commit, or clone_url clones a repo into it. An \
existing folder is only registered, never written to. Without path the folder is named after \
the project, in the new projects folder from Settings (default: beside the other projects). \
Only when the human asks for a new project. App coordinator only."
    }

    fn schema(&self) -> String {
        r#"{
  "type": "object",
  "properties": {
    "name": {"type": "string", "description": "Project name"},
    "path": {"type": "string", "description": "Folder for the project (~ allowed); default beside the existing projects"},
    "clone_url": {"type": "string", "description": "Git URL to clone instead of starting an empty repo"}
  },
  "required": ["name"]
}"#
        .into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        require_app_coordinator(&c)?;
        let h = c.host()?;
        let a = strict(&c.args, &CREATE_PROJECT_IN)?;
        let req = crate::CreateProjectRequest {
            name: a.str("name").trim().to_owned(),
            path: a.opt_str("path").unwrap_or_default().trim().to_owned(),
            clone_url: a.opt_str("clone_url").unwrap_or_default().trim().to_owned(),
        };
        if req.name.is_empty() {
            return Err("create_project: name is required".into());
        }
        Ok(h.create_project(req).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_domain::{BriefingSnapshot, WorkspaceBrief};

    fn brief(id: &str, bucket: &str, project: &str) -> WorkspaceBrief {
        WorkspaceBrief {
            workspace_id: id.into(),
            title: id.into(),
            bucket: bucket.into(),
            project_name: project.into(),
            now: "x".repeat(1000),
            ..WorkspaceBrief::default()
        }
    }

    #[test]
    fn lean_briefs_cap_pick_up_and_filter() {
        let mut ws = vec![brief("N1", "needs", "api")];
        ws.extend((0..40).map(|i| {
            brief(
                &format!("P{i}"),
                "pickup",
                if i % 2 == 0 { "api" } else { "web" },
            )
        }));
        let snap = BriefingSnapshot {
            workspaces: Some(ws),
            ..BriefingSnapshot::default()
        };
        let all = lean_briefs(&snap, "", "");
        let rows = all["workspaces"].as_array().unwrap();
        assert_eq!(rows.len(), 1 + PICKUP_SHOWN);
        assert_eq!(all["omitted"]["pickup"], 40 - PICKUP_SHOWN);
        assert!(rows[0].get("primary").is_none());
        assert_eq!(
            rows[0]["now"].as_str().unwrap().chars().count(),
            BRIEF_TEXT_MAX + 1
        );

        let pickup = lean_briefs(&snap, "pickup", "");
        assert_eq!(pickup["workspaces"].as_array().unwrap().len(), 40);
        assert!(pickup.get("omitted").is_none());

        let web = lean_briefs(&snap, "", "WEB");
        assert_eq!(web["workspaces"].as_array().unwrap().len(), 20);
        assert_eq!(
            web["counts"],
            serde_json::json!({"needs": 0, "working": 0, "pickup": 20})
        );
    }
}
