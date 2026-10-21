//! `lib/subagent.ts`: display helpers for Claude Code's Agent/Task subagent
//! tool and SendMessage, kept pure so "no JSON dump, no launch metadata"
//! stays testable. Claude Code renamed Task to Agent in v2.1.63; both are
//! accepted.

use serde_json::{Map, Value};

use super::js;

pub fn is_subagent_tool(tool: &str) -> bool {
    tool == "Task" || tool == "Agent"
}

pub fn is_send_message_tool(tool: &str) -> bool {
    tool == "SendMessage"
}

fn str_of(input: Option<&Map<String, Value>>, key: &str) -> String {
    input
        .and_then(|m| m.get(key))
        .and_then(Value::as_str)
        .map(|s| js::trim(s).to_owned())
        .unwrap_or_default()
}

/// `s.replace(/\s+/g, ' ').trim()`.
fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_space = false;
    for c in s.chars() {
        if js::is_space(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    js::trim(&out).to_owned()
}

/// A Task/Agent tool input.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubagentArgs {
    pub description: String,
    pub prompt: String,
    pub subagent_type: String,
    pub model: String,
    pub background: bool,
}

pub fn parse_subagent_args(input: Option<&Map<String, Value>>) -> SubagentArgs {
    let bg = input.and_then(|m| m.get("run_in_background"));
    SubagentArgs {
        description: str_of(input, "description"),
        prompt: str_of(input, "prompt"),
        subagent_type: str_of(input, "subagent_type"),
        model: str_of(input, "model"),
        background: matches!(bg, Some(Value::Bool(true)))
            || matches!(bg, Some(Value::String(s)) if s == "true"),
    }
}

/// A SendMessage tool input.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SendMessageArgs {
    pub description: String,
    pub message: String,
}

pub fn parse_send_message_args(input: Option<&Map<String, Value>>) -> SendMessageArgs {
    let or = |a: String, b: String| if a.is_empty() { b } else { a };
    SendMessageArgs {
        description: or(str_of(input, "description"), str_of(input, "summary")),
        message: or(str_of(input, "message"), str_of(input, "prompt")),
    }
}

/// Claude Code's background-launch ack is instructions for the model;
/// prefix-matched so a Read of a file that mentions the phrase is not one
/// (engine `is_subagent_launch_meta`).
pub fn is_subagent_launch_ack(result: &str) -> bool {
    js::trim(result).starts_with("Async agent launched successfully")
}

/// The collapsed row's meta: type (and model). Never JSON or the
/// description.
pub fn subagent_meta(r#type: &str, model: &str) -> String {
    match (r#type.is_empty(), model.is_empty()) {
        (false, false) => format!("{type} · {model}"),
        (false, true) => r#type.to_owned(),
        (true, false) => model.to_owned(),
        (true, true) => String::new(),
    }
}

pub fn send_message_label(pending: bool, description: &str) -> String {
    match (pending, description.is_empty()) {
        (true, true) => "Sending message".into(),
        (true, false) => format!("Sending {description}"),
        (false, true) => "Sent a message".into(),
        (false, false) => format!("Sent {description}"),
    }
}

/// The description is the row title; meta is a short prompt preview, never
/// JSON and never the agent id.
pub fn send_message_meta(description: &str, message: &str) -> String {
    if !description.is_empty() {
        return String::new();
    }
    collapse(message)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const LAUNCH_ACK: &str = "Async agent launched successfully. (This tool result is internal metadata – never quote or paste any part of it to the user.)\nagentId: ad643cfdedf8d9846\nUse SendMessage with to: 'ad643cfdedf8d9846', summary: '<5-10 word recap>' to continue this agent.\nYou know nothing about its results until that notification arrives – do not report, assume, or predict them.\noutput_file: /private/tmp/claude-501/tasks/ad643cfdedf8d9846.output\nDo NOT Read or tail this file via the shell tool – it is the full subagent JSONL transcript and reading it will overflow your context.";

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn task_and_agent_not_task_create() {
        assert!(is_subagent_tool("Task"));
        assert!(is_subagent_tool("Agent"));
        assert!(!is_subagent_tool("TaskCreate"));
        assert!(!is_subagent_tool("SendMessage"));
    }

    #[test]
    fn reads_type_prompt_background_not_a_json_dump() {
        let input = obj(json!({
            "description": "Research VS Code tokenization pipeline",
            "subagent_type": "Explore",
            "run_in_background": true,
            "prompt": "Find tokenizeViewport and forceTokenization.",
        }));
        let a = parse_subagent_args(Some(&input));
        assert_eq!(a.subagent_type, "Explore");
        assert!(a.background);
        assert!(a.prompt.contains("tokenizeViewport"));
        let meta = subagent_meta(&a.subagent_type, &a.model);
        assert_eq!(meta, "Explore");
        assert!(!meta.contains('{') && !meta.contains("description"));
    }

    #[test]
    fn hides_the_model_only_launch_metadata() {
        assert!(is_subagent_launch_ack(LAUNCH_ACK));
        assert!(!is_subagent_launch_ack(
            "The tokenizer lives in tokenizationTextModelPart.ts."
        ));
        assert!(!is_subagent_launch_ack(""));
        assert!(!is_subagent_launch_ack(
            "see Async agent launched successfully plus agentId and internal metadata in comments"
        ));
    }

    #[test]
    fn send_message_label_and_meta_never_leak_agent_ids() {
        assert!(is_send_message_tool("SendMessage"));
        let input = obj(json!({
            "to": "ad643cfdedf8d9846",
            "description": "Ask about tokenizeViewport",
            "prompt": "Where is forceTokenization called?",
        }));
        let a = parse_send_message_args(Some(&input));
        assert_eq!(
            send_message_label(false, &a.description),
            "Sent Ask about tokenizeViewport"
        );
        assert_eq!(send_message_meta(&a.description, &a.message), "");
        let meta = send_message_meta("", &a.message);
        assert_eq!(meta, "Where is forceTokenization called?");
        assert!(!meta.contains("ad643cf"));
    }
}
