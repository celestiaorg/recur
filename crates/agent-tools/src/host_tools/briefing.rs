//! Ask Recur coordinator tools: list_workspace_briefs, get_workspace_context,
//! present_workspace.

use recur_domain::APP_AGENT_CODE;

use super::marshal;
use crate::json::{Shape, Ty, field, unmarshal};
use crate::names;
use crate::tool::{Call, Tool, ToolError};

fn require_app_coordinator(c: &Call) -> Result<(), ToolError> {
    if c.agent_code != APP_AGENT_CODE {
        return Err("this tool is only available to the Ask Recur coordinator".into());
    }
    Ok(())
}

pub struct ListWorkspaceBriefsTool;

#[async_trait::async_trait]
impl Tool for ListWorkspaceBriefsTool {
    fn name(&self) -> &'static str {
        names::LIST_WORKSPACE_BRIEFS
    }

    fn description(&self) -> &'static str {
        "List the current Briefing overview: every workspace that needs attention, \
is working, or can be picked up, with deterministic Now/Next copy and routes. \
Always re-query before a factual status claim: the chat transcript is not current truth. \
App coordinator only."
    }

    fn schema(&self) -> String {
        r#"{"type":"object","properties":{}}"#.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        require_app_coordinator(&c)?;
        let h = c.host()?;
        let snap = h.list_workspace_briefs().await;
        marshal(&snap).map_err(|e| ToolError(format!("list_workspace_briefs: {e}")))
    }
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
    "target": {"type": "string", "description": "Navigation target from a brief's primary/secondary action"}
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
