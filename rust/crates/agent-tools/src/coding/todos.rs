use std::collections::HashMap;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::json::{Field, Obj, Shape, Ty, field, unmarshal};
use crate::tool::{Call, Tool, ToolError};

/// One session-scoped todo row.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: String,
}

/// In-process todos keyed by Recur session id.
#[derive(Debug, Default)]
pub struct TodoStore {
    m: Mutex<HashMap<String, Vec<TodoItem>>>,
}

impl TodoStore {
    /// A copy of the session's todos.
    pub fn get(&self, session_id: &str) -> Vec<TodoItem> {
        if session_id.is_empty() {
            return Vec::new();
        }
        self.m.lock().get(session_id).cloned().unwrap_or_default()
    }

    /// Replaces the session's todos.
    pub fn put(&self, session_id: &str, items: &[TodoItem]) {
        if session_id.is_empty() {
            return;
        }
        self.m.lock().insert(session_id.to_string(), items.to_vec());
    }
}

/// Merges incoming rows by id: known ids update non-empty fields, new ids
/// append (status defaults to pending), rows without an id are ignored.
fn merge_todos(cur: Vec<TodoItem>, incoming: Vec<TodoItem>) -> Vec<TodoItem> {
    let mut out = cur;
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for (i, t) in out.iter().enumerate() {
        if !t.id.is_empty() {
            by_id.insert(t.id.clone(), i);
        }
    }
    for mut t in incoming {
        if t.id.is_empty() {
            continue;
        }
        if let Some(&i) = by_id.get(&t.id) {
            if !t.content.is_empty() {
                out[i].content = t.content;
            }
            if !t.status.is_empty() {
                out[i].status = t.status;
            }
        } else {
            if t.status.is_empty() {
                t.status = "pending".into();
            }
            by_id.insert(t.id.clone(), out.len());
            out.push(t);
        }
    }
    out
}

/// Go `json.Marshal` of the merged slice: nil (empty) is `null`.
fn todos_json(items: &[TodoItem]) -> String {
    if items.is_empty() {
        return "null".into();
    }
    recur_domain::json::to_string(items).unwrap_or_else(|_| "[]".into())
}

const SCHEMA: &str = r#"{"type":"object","properties":{"todos":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"content":{"type":"string"},"status":{"type":"string"}},"required":["id","content","status"]}}},"required":["todos"]}"#;

const TODO_FIELDS: &[Field] = &[
    field("id", "ID", Ty::Str),
    field("content", "Content", Ty::Str),
    field("status", "Status", Ty::Str),
];

static TODO_ITEM: Shape = Shape {
    name: "TodoItem",
    qualified: "agenttools.TodoItem",
    fields: TODO_FIELDS,
};

static IN: Shape = Shape::anon(&[field("todos", "Todos", Ty::List(&Ty::Struct(&TODO_ITEM)))]);

fn item(o: &Obj) -> TodoItem {
    TodoItem {
        id: o.str("id").into(),
        content: o.str("content").into(),
        status: o.str("status").into(),
    }
}

pub struct TodoTool;

#[async_trait::async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &'static str {
        "TodoWrite"
    }

    fn description(&self) -> &'static str {
        "Replace or merge the session todo list. Each item has id, content, and status (pending|in_progress|completed)."
    }

    fn schema(&self) -> String {
        SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let a = unmarshal(&c.args, &IN)
            .map_err(|e| ToolError(format!("invalid TodoWrite args: {e}")))?;
        let Some(todos) = &c.todos else {
            return Err("todo store unavailable".into());
        };
        let incoming = a.objs("todos").iter().map(item).collect();
        let merged = merge_todos(todos.get(&c.session_id), incoming);
        todos.put(&c.session_id, &merged);
        Ok(todos_json(&merged))
    }
}
