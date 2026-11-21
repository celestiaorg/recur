//! Todo checklist rows for chat cards: TodoWrite / Cursor updateTodos input,
//! and the plan board update_todos returns.

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

/// The checklist in an update_todos result (`Plan::board`): the lines after
/// "Steps:", as "S1 [doing] Title files=… note=…", with plan statuses mapped
/// to TodoWrite's (skipped reads as completed).
pub fn parse_board_todos(board: &str) -> Vec<Todo> {
    board
        .lines()
        .skip_while(|l| l.trim() != "Steps:")
        .filter_map(|l| {
            let (id, rest) = l.trim().split_once(" [")?;
            let (status, rest) = rest.split_once("] ")?;
            if id.is_empty() || id.contains(char::is_whitespace) {
                return None;
            }
            let end = [" files=", " after=", " child=", " note="]
                .iter()
                .filter_map(|k| rest.find(k))
                .min()
                .unwrap_or(rest.len());
            Some(Todo {
                content: rest[..end].trim().to_owned(),
                status: match status {
                    "doing" => "in_progress",
                    "done" | "skipped" => "completed",
                    _ => "pending",
                }
                .to_owned(),
            })
        })
        .collect()
}

/// (completed, total).
pub fn todo_counts(todos: &[Todo]) -> (usize, usize) {
    (
        todos.iter().filter(|t| t.status == "completed").count(),
        todos.len(),
    )
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
    fn reads_an_update_todos_board() {
        let got = parse_board_todos(
            "Brief [x] y\n\nSteps:\nS1 [done] Read files note=ok\nS2 [doing] Edit panel files=a.rs\nS3 [pending] Ship\nS4 [skipped] Drop",
        );
        assert_eq!(
            got,
            [
                todo("Read files", "completed"),
                todo("Edit panel", "in_progress"),
                todo("Ship", "pending"),
                todo("Drop", "completed"),
            ]
        );
        assert!(parse_board_todos("no todo with id \"S9\"").is_empty());
    }

    #[test]
    fn empty_on_junk() {
        assert!(parse_todos("").is_empty());
        assert!(parse_todos("{").is_empty());
        assert!(parse_todos(r#"{"todos":"nope"}"#).is_empty());
    }
}
