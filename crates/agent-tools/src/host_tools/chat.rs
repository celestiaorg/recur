use serde::Serialize;

use super::loop_flow::{QUESTION_IN, question_options};
use super::strict;
use crate::host::{AskUserRequest, PermissionAsk, PermissionVerdict};
use crate::json::{Shape, Ty, compact, field, unmarshal};
use crate::names;
use crate::tool::{Call, Tool, ToolError};

const PERMISSION_REQUEST_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "tool_name": {"type": "string", "description": "The tool awaiting permission"},
    "input": {"type": "object", "description": "The arguments that tool was called with"},
    "tool_use_id": {"type": "string", "description": "Correlates with the assistant's tool_use block"}
  },
  "required": ["tool_name"]
}"#;

static PERMISSION_REQUEST_IN: Shape = Shape::anon(&[
    field("tool_name", Ty::Str),
    field("input", Ty::Raw),
    field("tool_use_id", Ty::Str),
]);

#[derive(Serialize)]
struct Deny<'a> {
    behavior: &'a str,
    message: &'a str,
}

/// The permission JSON document Claude Code reads from
/// --permission-prompt-tool: allow echoes the input untouched (updatedInput is
/// required on allow); deny carries the reason.
pub fn marshal_permission_verdict(v: &PermissionVerdict, input: &str) -> String {
    let mut reason = v.reason.as_str();
    if v.allow {
        match compact(input) {
            Ok(input) => return format!(r#"{{"behavior":"allow","updatedInput":{input}}}"#),
            // An un-encodable input cannot be echoed back, and allow without
            // updatedInput is rejected by the CLI, so fail closed.
            Err(_) => reason = "could not encode the approved input",
        }
    }
    let mut reason = reason.trim();
    if reason.is_empty() {
        reason = "the human denied this tool call";
    }
    serde_json::to_string(&Deny {
        behavior: "deny",
        message: reason,
    })
    .unwrap_or_default()
}

pub struct PermissionRequestTool;

#[async_trait::async_trait]
impl Tool for PermissionRequestTool {
    fn name(&self) -> &'static str {
        names::PERMISSION_REQUEST
    }

    fn description(&self) -> &'static str {
        "Permission broker for this Recur workspace. Claude Code calls this itself via \
--permission-prompt-tool when a tool needs approval; do not call it directly."
    }

    fn schema(&self) -> String {
        PERMISSION_REQUEST_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        // Not strict: the payload comes from the CLI, not the model, so a field
        // added by a future Claude Code must not break every request.
        let a = unmarshal(&c.args, &PERMISSION_REQUEST_IN)
            .map_err(|e| ToolError(format!("permission_request: {e}")))?;
        if a.str("tool_name").trim().is_empty() {
            return Err("permission_request: tool_name is required".into());
        }
        let input = match a.str("input") {
            "" => "{}",
            s => s,
        };
        let verdict = h
            .request_permission(
                &c.cancel,
                &c.workspace_id,
                &c.agent_code,
                PermissionAsk {
                    tool: a.str("tool_name").into(),
                    input: input.into(),
                    tool_use_id: a.str("tool_use_id").into(),
                },
            )
            .await
            .map_err(|e| ToolError(format!("permission_request: {e}")))?;
        Ok(marshal_permission_verdict(&verdict, input))
    }
}

const ASK_USER_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "question": {"type": "string", "description": "The question for the human"},
    "context": {"type": "string", "description": "Short one-read context"},
    "options": {
      "type": "array",
      "description": "Optional 2-4 choices (shortcuts; free-text is always available)",
      "items": {
        "type": "object",
        "properties": {
          "text": {"type": "string"},
          "tag": {"type": "string"},
          "desc": {"type": "string"}
        },
        "required": ["text"]
      }
    }
  },
  "required": ["question"]
}"#;

pub struct AskUserTool;

#[async_trait::async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &'static str {
        names::ASK_USER
    }

    fn description(&self) -> &'static str {
        "Ask the human a clarifying question. Use for interactive chats (including Maestro) when \
you need a decision without pausing the whole workspace. Provide a clear question and \
optional 2-4 options. This ends your current turn: the chat goes idle and their answer \
arrives later as a user message. An inbox notification is raised. Do NOT use escalate \
from interactive chats: that is for non-interactive workflow agents only."
    }

    fn schema(&self) -> String {
        ASK_USER_SCHEMA.into()
    }

    fn halts(&self) -> bool {
        true
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &QUESTION_IN)?;
        if a.str("question").trim().is_empty() {
            return Err("ask_user: question is required".into());
        }
        let options = question_options(&a, "ask_user: each option needs non-empty text")?;
        Ok(h.ask_user(
            &c.workspace_id,
            &c.agent_code,
            AskUserRequest {
                question: a.str("question").into(),
                context: a.str("context").into(),
                options,
            },
        )
        .await)
    }
}

const SWITCH_MODE_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "mode": {"type": "string", "enum": ["ask", "plan", "agent", "review"]}
  },
  "required": ["mode"]
}"#;

static SWITCH_MODE_IN: Shape = Shape::anon(&[field("mode", Ty::Str)]);

pub struct SwitchModeTool;

#[async_trait::async_trait]
impl Tool for SwitchModeTool {
    fn name(&self) -> &'static str {
        names::SWITCH_MODE
    }

    fn description(&self) -> &'static str {
        "Switch this interactive chat's agent mode. Privilege-safe: agent may switch to \
ask/plan/review; ask/plan/review may switch among each other; never upgrade \
ask/plan/review to agent (ask the user to flip the dock instead). Not available on \
Maestro. A successful switch remounts this turn immediately with the new tool policy. \
Stop after the call and continue on the remounted turn."
    }

    fn schema(&self) -> String {
        SWITCH_MODE_SCHEMA.into()
    }

    fn halts(&self) -> bool {
        true
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &SWITCH_MODE_IN)?;
        Ok(
            h.switch_mode(&c.workspace_id, &c.agent_code, a.str("mode").trim())
                .await,
        )
    }
}

const RENAME_SESSION_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "title": {"type": "string"}
  },
  "required": ["title"]
}"#;

static TITLE_IN: Shape = Shape::anon(&[field("title", Ty::Str)]);

pub struct RenameSessionTool;

#[async_trait::async_trait]
impl Tool for RenameSessionTool {
    fn name(&self) -> &'static str {
        names::RENAME_SESSION
    }

    fn description(&self) -> &'static str {
        "Rename the current chat/session to a short 3–5 word topic title. No confirmation."
    }

    fn schema(&self) -> String {
        RENAME_SESSION_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &TITLE_IN)?;
        if a.str("title").trim().is_empty() {
            return Err("rename_session: title is required".into());
        }
        Ok(
            h.rename_session(&c.workspace_id, &c.agent_code, &c.chat_code, a.str("title"))
                .await,
        )
    }
}

const RENAME_WORKSPACE_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "title": {"type": "string", "description": "Short workspace title (at most 8 words, sentence case)"}
  },
  "required": ["title"]
}"#;

pub struct RenameWorkspaceTool;

#[async_trait::async_trait]
impl Tool for RenameWorkspaceTool {
    fn name(&self) -> &'static str {
        names::RENAME_WORKSPACE
    }

    fn description(&self) -> &'static str {
        "Rename this workspace (sidebar title) to a short topic title (at most 8 words, sentence \
case). Allowed during workflows. No confirmation."
    }

    fn schema(&self) -> String {
        RENAME_WORKSPACE_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &TITLE_IN)?;
        if a.str("title").trim().is_empty() {
            return Err("rename_workspace: title is required".into());
        }
        Ok(
            h.rename_workspace(&c.workspace_id, &c.agent_code, a.str("title"))
                .await,
        )
    }
}

const RENAME_BRANCH_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "name": {"type": "string", "description": "Kebab-case branch topic; the configured prefix is applied if omitted"}
  },
  "required": ["name"]
}"#;

static NAME_IN: Shape = Shape::anon(&[field("name", Ty::Str)]);

pub struct RenameBranchTool;

#[async_trait::async_trait]
impl Tool for RenameBranchTool {
    fn name(&self) -> &'static str {
        names::RENAME_BRANCH
    }

    fn description(&self) -> &'static str {
        "Rename this workspace's git branch to a kebab-case topic (e.g. add-dark-mode). The \
configured branch prefix (default recur/) is applied if omitted. Allowed during planning; \
refused while a workflow is coding, reviewing, or verifying. No confirmation."
    }

    fn schema(&self) -> String {
        RENAME_BRANCH_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &NAME_IN)?;
        if a.str("name").trim().is_empty() {
            return Err("rename_branch: name is required".into());
        }
        Ok(
            h.rename_branch(&c.workspace_id, &c.agent_code, a.str("name"))
                .await,
        )
    }
}

const OPEN_PR_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "title": {"type": "string", "description": "PR title, under 80 characters"},
    "body": {"type": "string", "description": "Markdown description of ALL workspace changes versus the base branch"},
    "draft": {"type": "boolean", "description": "Open as a GitHub draft PR (gh pr create --draft). Ignored if a PR is already open on GitHub."}
  },
  "required": ["title", "body"]
}"#;

static OPEN_PR_IN: Shape = Shape::anon(&[
    field("title", Ty::Str),
    field("body", Ty::Str),
    field("draft", Ty::Bool),
]);

pub struct OpenPrTool;

#[async_trait::async_trait]
impl Tool for OpenPrTool {
    fn name(&self) -> &'static str {
        names::OPEN_PR
    }

    fn description(&self) -> &'static str {
        "Open a pull request from this workspace. Recur pushes the branch and creates the GitHub \
PR when the project opts in (otherwise a Local PR). Do not run `gh pr create` yourself. \
Provide a title under 80 characters and a markdown body describing ALL workspace changes \
versus the base branch: not only this chat turn. Set draft to true for a GitHub draft \
PR. Commit uncommitted work first. If a GitHub PR is already open, this pushes new \
commits and does not rewrite the description. If only a Local PR is open, title and \
body update that Local PR and retry GitHub create when the project opts in. Refused \
on the main working copy and while a workflow is running. After a workflow idles (including \
Review & fix, Docs, and Security review & fix), open a PR from here or the titlebar. \
The run does not self-open one."
    }

    fn schema(&self) -> String {
        OPEN_PR_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &OPEN_PR_IN)?;
        if a.str("title").trim().is_empty() {
            return Err("open_pr: title is required".into());
        }
        if a.str("body").trim().is_empty() {
            return Err("open_pr: body is required".into());
        }
        Ok(h.open_pr(
            &c.workspace_id,
            &c.agent_code,
            a.str("title"),
            a.str("body"),
            a.bool("draft"),
        )
        .await)
    }
}
