//! Subagents: create_workspace children and the tools that read, steer,
//! merge or archive them.

use super::strict;
use crate::host::CreateWorkspaceRequest;
use crate::json::{Shape, Ty, field};
use crate::names;
use crate::tool::{Call, Tool, ToolError};

const CREATE_WORKSPACE_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "title": {"type": "string", "description": "Short workspace title. Reuse the same title for the same task so repeat calls dedupe"},
    "prompt": {"type": "string", "description": "First message for the new workspace's chat: the task plus the context it needs to work alone"},
    "project_id": {"type": "string", "description": "Project id; defaults to this workspace's project"},
    "base_branch": {"type": "string", "description": "Standalone only: branch to start from (defaults to the project's base branch). Setting it implies standalone"},
    "mode": {"type": "string", "enum": ["ask", "plan", "agent", "review", "loop"], "description": "Chat mode for the first tab (default agent). loop engages the autopilot with prompt as its goal"},
    "loop_id": {"type": "string", "description": "Bundle [[loop]] id for mode=loop; defaults to Settings"},
    "policy_id": {"type": "string", "description": "Bundle [[policy]] id for mode=loop; defaults to Settings"},
    "standalone": {"type": "boolean", "description": "true: an independent workspace that is never merged back here (its own branch off base_branch, its own PR). Default false: the workspace is a subagent of this one, branched off this branch; when it finishes you get a report and must call merge_child to land its work (or archive_child to close it). Scheduled runs and mode=loop default to standalone"}
  },
  "required": ["title", "prompt"]
}"#;

static CREATE_WORKSPACE_IN: Shape = Shape::anon(&[
    field("title", "Title", Ty::Str),
    field("prompt", "Prompt", Ty::Str),
    field("project_id", "ProjectID", Ty::Str),
    field("base_branch", "BaseBranch", Ty::Str),
    field("mode", "Mode", Ty::Str),
    field("loop_id", "LoopID", Ty::Str),
    field("policy_id", "PolicyID", Ty::Str),
    field("standalone", "Standalone", Ty::OptBool),
]);

pub struct CreateWorkspaceTool;

#[async_trait::async_trait]
impl Tool for CreateWorkspaceTool {
    fn name(&self) -> &'static str {
        names::CREATE_WORKSPACE
    }

    fn description(&self) -> &'static str {
        "Run a SUBAGENT: this is how subagents work in Recur. Creates a NEW workspace (fresh git \
worktree from the base branch) and runs prompt on its first chat tab, or as a loop when mode \
is loop. Use to fan work out into \
independent workspaces, one per task. Give each a stable, descriptive title: a live \
workspace in the project with the same title is returned instead of a duplicate, so \
repeating the call is safe. Model and flags follow Settings (this chat vs that mode's default); Simplify follows this chat. The new \
workspace runs on its own; this chat does not wait for it. Each call starts an agent \
run — one child per distinct task, not extras. Prefer a child for anything beyond a very \
simple change. By default the workspace is a subagent (child) \
of this one: it starts from this branch. When it finishes you get a report; call \
merge_child to land its commits here, or archive_child to close it without merging. \
Pass standalone=true for independent work that should ship on its own."
    }

    fn schema(&self) -> String {
        CREATE_WORKSPACE_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &CREATE_WORKSPACE_IN)?;
        if a.str("title").trim().is_empty() || a.str("prompt").trim().is_empty() {
            return Err("create_workspace: title and prompt are required".into());
        }
        Ok(h.create_workspace(
            &c.workspace_id,
            &c.agent_code,
            CreateWorkspaceRequest {
                project_id: a.str("project_id").into(),
                title: a.str("title").into(),
                prompt: a.str("prompt").into(),
                base_branch: a.str("base_branch").into(),
                mode: a.str("mode").into(),
                loop_id: a.str("loop_id").into(),
                policy_id: a.str("policy_id").into(),
                standalone: a.opt_bool("standalone"),
            },
        )
        .await)
    }
}

const MERGE_CHILD_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "workspace_id": {"type": "string", "description": "Id of the child workspace to merge into this worktree"}
  },
  "required": ["workspace_id"]
}"#;

const CHILD_ID_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "workspace_id": {"type": "string", "description": "Id of the subagent child workspace (from create_workspace or its report)"}
  },
  "required": ["workspace_id"]
}"#;

const MESSAGE_CHILD_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "workspace_id": {"type": "string", "description": "Id of the child workspace (from create_workspace or its report)"},
    "message": {"type": "string", "description": "Instruction for the child (steers a running turn, or starts a new turn on a finished child)"}
  },
  "required": ["workspace_id", "message"]
}"#;

static CHILD_ID_IN: Shape = Shape::anon(&[field("workspace_id", "WorkspaceID", Ty::Str)]);

static MESSAGE_CHILD_IN: Shape = Shape::anon(&[
    field("workspace_id", "WorkspaceID", Ty::Str),
    field("message", "Message", Ty::Str),
]);

fn child_id_arg(c: &Call, tool: &str) -> Result<String, ToolError> {
    let a = strict(&c.args, &CHILD_ID_IN)?;
    let id = a.str("workspace_id").trim();
    if id.is_empty() {
        return Err(format!("{tool}: workspace_id is required").into());
    }
    Ok(id.into())
}

pub struct MergeChildTool;

#[async_trait::async_trait]
impl Tool for MergeChildTool {
    fn name(&self) -> &'static str {
        names::MERGE_CHILD
    }

    fn description(&self) -> &'static str {
        "Land a finished subagent's commits: merge its child workspace branch --no-ff into this \
worktree and archive it. Commit your own changes here first. On conflicts, resolve the \
markers, git commit, and call it again to archive. Refused while the child is still running."
    }

    fn schema(&self) -> String {
        MERGE_CHILD_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let id = child_id_arg(&c, "merge_child")?;
        Ok(h.merge_child(&c.workspace_id, &c.agent_code, &id).await)
    }
}

pub struct ReadChildTool;

#[async_trait::async_trait]
impl Tool for ReadChildTool {
    fn name(&self) -> &'static str {
        names::READ_CHILD
    }

    fn description(&self) -> &'static str {
        "Read a subagent's report: its state, final reply, handoff notes, findings, plan, \
committed and uncommitted changes against this branch with the diff, and artifact ids \
(open those with read_artifact workspace_id). Works on running, finished, and archived children."
    }

    fn schema(&self) -> String {
        CHILD_ID_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let id = child_id_arg(&c, "read_child")?;
        Ok(h.read_child(&c.workspace_id, &c.agent_code, &id).await)
    }
}

pub struct ArchiveChildTool;

#[async_trait::async_trait]
impl Tool for ArchiveChildTool {
    fn name(&self) -> &'static str {
        names::ARCHIVE_CHILD
    }

    fn description(&self) -> &'static str {
        "Close a finished subagent without merging: archives its child workspace once you have \
what you need from its report (research, a review, a spike). Its worktree and branch are \
deleted, so any unmerged commits are gone; merge_child first if you want them. Refused \
while the child is still running."
    }

    fn schema(&self) -> String {
        CHILD_ID_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let id = child_id_arg(&c, "archive_child")?;
        Ok(h.archive_child(&c.workspace_id, &c.agent_code, &id).await)
    }
}

pub struct MessageChildTool;

#[async_trait::async_trait]
impl Tool for MessageChildTool {
    fn name(&self) -> &'static str {
        names::MESSAGE_CHILD
    }

    fn description(&self) -> &'static str {
        "Send a message to a child workspace. A running child is steered mid-turn: returns once it has accepted the message (may take until its next tool call) and it continues. \
A finished (not merged or archived) child starts a new turn with the message and reports back again when it finishes. Works with any harness."
    }

    fn schema(&self) -> String {
        MESSAGE_CHILD_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &MESSAGE_CHILD_IN)?;
        let (id, message) = (a.str("workspace_id").trim(), a.str("message").trim());
        if id.is_empty() || message.is_empty() {
            return Err("message_child: workspace_id and message are required".into());
        }
        Ok(h.message_child(&c.workspace_id, &c.agent_code, id, message)
            .await)
    }
}
