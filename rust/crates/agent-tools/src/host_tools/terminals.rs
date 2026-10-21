use serde::Serialize;

use super::{EMPTY_OBJECT_SCHEMA, marshal, strict};
use crate::host::TerminalInfo;
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
focused tab). Use when the user mentions the terminal, a build/test error on screen, or \
before read_terminal if you need an id."
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
    "tail_lines": {"type": "integer", "description": "Last N lines (default 200, max 2000)"}
  }
}"#;

static READ_TERMINAL_IN: Shape = Shape::anon(&[
    field("id", "ID", Ty::Str),
    field("tail_lines", "TailLines", Ty::Int),
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
Prefer this over guessing that output lives in a worktree file."
    }

    fn schema(&self) -> String {
        READ_TERMINAL_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let raw = c.args.trim();
        let (id, tail) = if raw.is_empty() || raw == "null" {
            (String::new(), 0)
        } else {
            let a = strict(&c.args, &READ_TERMINAL_IN)?;
            (a.str("id").to_string(), a.int("tail_lines"))
        };
        Ok(h.read_terminal(&c.workspace_id, &c.agent_code, &id, tail)
            .await?)
    }
}
