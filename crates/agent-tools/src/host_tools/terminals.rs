use serde::Serialize;

use super::{EMPTY_OBJECT_SCHEMA, marshal, strict};
use crate::host::{BackgroundRequest, ReadTerminalRequest, TerminalInfo};
use crate::json::{Shape, Ty, field};
use crate::names;
use crate::tool::{Call, Tool, ToolError};

#[derive(Serialize)]
struct TerminalList<'a> {
    count: usize,
    terminals: Option<&'a [TerminalInfo]>,
}

pub struct ListTerminalsTool;

#[async_trait::async_trait]
impl Tool for ListTerminalsTool {
    fn name(&self) -> &'static str {
        names::LIST_TERMINALS
    }

    fn description(&self) -> &'static str {
        "List this workspace's live Observatory terminal tabs (id, title, cwd, whether it is the \
focused tab, background/startedBy for run_background processes, exited/exitCode). Use when the \
user mentions the terminal, a build/test error on screen, or before read_terminal if you need \
an id."
    }

    fn schema(&self) -> String {
        EMPTY_OBJECT_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let list = h.list_terminals(&c.workspace_id, &c.agent_code).await;
        marshal(&TerminalList {
            count: list.as_ref().map_or(0, Vec::len),
            terminals: list.as_deref(),
        })
    }
}

const READ_TERMINAL_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "id": {"type": "string", "description": "Session id from list_terminals; omit to read the focused tab"},
    "tail_lines": {"type": "integer", "description": "Last N lines (default 200, max 2000)"},
    "wait_ms": {"type": "integer", "description": "Wait up to this long (max 300000) before reading; returns early when until matches or the process exits"},
    "until": {"type": "string", "description": "Regex checked against output you have not read yet (since your last read_terminal or run_background of this terminal)"}
  }
}"#;

static READ_TERMINAL_IN: Shape = Shape::anon(&[
    field("id", Ty::Str),
    field("tail_lines", Ty::Int),
    field("wait_ms", Ty::Int),
    field("until", Ty::Str),
]);

pub struct ReadTerminalTool;

#[async_trait::async_trait]
impl Tool for ReadTerminalTool {
    fn name(&self) -> &'static str {
        names::READ_TERMINAL
    }

    fn description(&self) -> &'static str {
        "Read recent output from an Observatory terminal (ANSI stripped). id is a session id from \
list_terminals (or omit to read the focused tab). tail_lines defaults to 200 (max 2000). \
wait_ms (max 300000) waits for output first: it returns as soon as the until regex matches \
output you have not read yet, the process exits, or the wait ends. Only wait when you need the \
result now; otherwise finish your turn (you get a new turn when a background process exits). \
Prefer this over guessing that output lives in a worktree file."
    }

    fn schema(&self) -> String {
        READ_TERMINAL_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let raw = c.args.trim();
        let req = if raw.is_empty() || raw == "null" {
            ReadTerminalRequest::default()
        } else {
            let a = strict(&c.args, &READ_TERMINAL_IN)?;
            ReadTerminalRequest {
                id: a.str("id").into(),
                tail_lines: a.int("tail_lines"),
                wait_ms: a.int("wait_ms"),
                until: a.str("until").into(),
            }
        };
        Ok(
            h.read_terminal(&c.cancel, &c.workspace_id, &c.agent_code, req)
                .await?,
        )
    }
}

const RUN_BACKGROUND_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "command": {"type": "string", "description": "Shell command, run with the login shell (-lc)"},
    "title": {"type": "string", "description": "Tab label (default: the command)"},
    "cwd": {"type": "string", "description": "Working directory; relative to the worktree root (default)"},
    "notify_on_exit": {"type": "boolean", "description": "Start a new turn in this chat when the process exits (default true)"}
  },
  "required": ["command"]
}"#;

static RUN_BACKGROUND_IN: Shape = Shape::anon(&[
    field("command", Ty::Str),
    field("title", Ty::Str),
    field("cwd", Ty::Str),
    field("notify_on_exit", Ty::OptBool),
]);

pub struct RunBackgroundTool;

#[async_trait::async_trait]
impl Tool for RunBackgroundTool {
    fn name(&self) -> &'static str {
        names::RUN_BACKGROUND
    }

    fn description(&self) -> &'static str {
        "Start a long-running process (dev server, watcher, long build or test run) in a Recur \
terminal tab the user can see. Returns its terminal id and its first few seconds of output \
(often enough to confirm a server is up). The workspace owns it: it keeps running across turns \
and model switches until it exits, stop_terminal stops it, or the app quits (closing its tab \
does not stop it; after a quit your next turn says which processes stopped). Read more with \
read_terminal. When it exits on its own you get a new turn with its exit code and last lines, \
unless notify_on_exit is false. Once it is running, tell the user and end your turn rather \
than waiting on it."
    }

    fn schema(&self) -> String {
        RUN_BACKGROUND_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &RUN_BACKGROUND_IN)?;
        let req = BackgroundRequest {
            command: a.str("command").into(),
            title: a.str("title").into(),
            cwd: a.str("cwd").into(),
            notify_on_exit: a.opt_bool("notify_on_exit").unwrap_or(true),
        };
        if req.command.trim().is_empty() {
            return Err("command is required".into());
        }
        Ok(
            h.run_background(&c.cancel, &c.workspace_id, &c.agent_code, req)
                .await?,
        )
    }
}

const STOP_TERMINAL_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "id": {"type": "string", "description": "Terminal id from run_background or list_terminals"}
  },
  "required": ["id"]
}"#;

static STOP_TERMINAL_IN: Shape = Shape::anon(&[field("id", Ty::Str)]);

pub struct StopTerminalTool;

#[async_trait::async_trait]
impl Tool for StopTerminalTool {
    fn name(&self) -> &'static str {
        names::STOP_TERMINAL
    }

    fn description(&self) -> &'static str {
        "Stop a background process started with run_background (its whole process tree) and \
close its tab. Tabs the user opened are refused. No exit turn follows a stop."
    }

    fn schema(&self) -> String {
        STOP_TERMINAL_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &STOP_TERMINAL_IN)?;
        let id = a.str("id").trim();
        if id.is_empty() {
            return Err("id is required".into());
        }
        Ok(h.stop_terminal(&c.workspace_id, &c.agent_code, id).await?)
    }
}
