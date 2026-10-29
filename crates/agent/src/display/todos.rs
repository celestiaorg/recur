//! TodoWrite / Cursor updateTodos checklist rules (frontend `lib/todos.ts`).
//! A flat per-agent checklist, distinct from the workspace Plan.

use recur_domain::ChatItem;
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Todo {
    pub content: String,
    pub status: String,
}

/// The `todos` of a TodoWrite input; empty on junk. Items with blank content
/// are dropped and a missing status reads as pending.
pub fn parse_todos(input: &str) -> Vec<Todo> {
    let Ok(v) = serde_json::from_str::<Value>(input) else {
        return Vec::new();
    };
    let Some(Value::Array(raw)) = v.get("todos") else {
        return Vec::new();
    };
    raw.iter()
        .filter(|t| t.is_object() || t.is_array())
        .map(|t| Todo {
            content: match t.get("content") {
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            },
            status: match t.get("status") {
                Some(Value::String(s)) => s.clone(),
                _ => "pending".to_owned(),
            },
        })
        .filter(|t| !t.content.trim().is_empty())
        .collect()
}

/// (completed, total).
pub fn todo_counts(todos: &[Todo]) -> (usize, usize) {
    (
        todos.iter().filter(|t| t.status == "completed").count(),
        todos.len(),
    )
}

/// The latest non-empty TodoWrite list in a transcript (agents replace the
/// whole list each call).
pub fn latest_todos(items: &[ChatItem]) -> Vec<Todo> {
    items
        .iter()
        .rev()
        .filter(|m| m.tool == "TodoWrite")
        .map(|m| parse_todos(&m.input))
        .find(|t| !t.is_empty())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn todo(c: &str, s: &str) -> Todo {
        Todo {
            content: c.into(),
            status: s.into(),
        }
    }

    #[test]
    fn reads_the_common_shape() {
        let got = parse_todos(
            r#"{"todos":[{"content":"Read files","status":"completed"},{"content":"Edit panel","status":"in_progress"},{"content":"  ","status":"pending"},{"content":"x"}]}"#,
        );
        assert_eq!(
            got,
            [
                todo("Read files", "completed"),
                todo("Edit panel", "in_progress"),
                todo("x", "pending")
            ]
        );
    }

    #[test]
    fn empty_on_junk() {
        assert!(parse_todos("").is_empty());
        assert!(parse_todos("{").is_empty());
        assert!(parse_todos(r#"{"todos":"nope"}"#).is_empty());
    }

    #[test]
    fn latest_uses_the_last_list_with_items() {
        let item = |tool: &str, input: &str| ChatItem {
            tool: tool.into(),
            input: input.into(),
            ..Default::default()
        };
        let todos = latest_todos(&[
            item(
                "TodoWrite",
                r#"{"todos":[{"content":"old","status":"pending"}]}"#,
            ),
            item("Read", r#"{"file_path":"x"}"#),
            item(
                "TodoWrite",
                r#"{"todos":[{"content":"new","status":"completed"}]}"#,
            ),
            item("TodoWrite", r#"{"todos":[]}"#),
        ]);
        assert_eq!(todos, [todo("new", "completed")]);
        assert_eq!(todo_counts(&todos), (1, 1));
    }
}
