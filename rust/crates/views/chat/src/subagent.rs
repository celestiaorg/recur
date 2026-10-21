//! Display helpers for the subagent (`Task` / `Agent`) and `SendMessage` tools
//! (`lib/subagent.ts`): never a JSON dump, never the model-only launch metadata.

use serde_json::Value;

pub fn is_subagent_tool(tool: &str) -> bool {
    tool == "Task" || tool == "Agent"
}

pub fn is_send_message_tool(tool: &str) -> bool {
    tool == "SendMessage"
}

fn s(input: Option<&Value>, key: &str) -> String {
    input
        .and_then(|v| v.get(key))
        .and_then(Value::as_str)
        .map(|x| x.trim().to_string())
        .unwrap_or_default()
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubagentArgs {
    pub description: String,
    pub prompt: String,
    pub subagent_type: String,
    pub model: String,
    pub background: bool,
}

/// `parseSubagentArgs`.
pub fn parse_subagent_args(input: Option<&Value>) -> SubagentArgs {
    let bg = input.and_then(|v| v.get("run_in_background"));
    SubagentArgs {
        description: s(input, "description"),
        prompt: s(input, "prompt"),
        subagent_type: s(input, "subagent_type"),
        model: s(input, "model"),
        background: matches!(bg, Some(Value::Bool(true)))
            || matches!(bg, Some(Value::String(v)) if v == "true"),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SendMessageArgs {
    pub description: String,
    pub message: String,
}

/// `parseSendMessageArgs`.
pub fn parse_send_message_args(input: Option<&Value>) -> SendMessageArgs {
    let d = s(input, "description");
    let m = s(input, "message");
    SendMessageArgs {
        description: if d.is_empty() { s(input, "summary") } else { d },
        message: if m.is_empty() { s(input, "prompt") } else { m },
    }
}

/// `isSubagentLaunchAck`: Claude Code's background-launch ack is instructions for the model.
pub fn is_subagent_launch_ack(result: &str) -> bool {
    result
        .trim()
        .starts_with("Async agent launched successfully")
}

/// `subagentMeta`: type (and model) for a collapsed row.
pub fn subagent_meta(kind: &str, model: &str) -> String {
    match (kind.is_empty(), model.is_empty()) {
        (false, false) => format!("{kind} · {model}"),
        (false, true) => kind.to_string(),
        _ => model.to_string(),
    }
}

/// `sendMessageLabel`.
pub fn send_message_label(pending: bool, description: &str) -> String {
    match (pending, description.is_empty()) {
        (true, false) => format!("Sending {description}"),
        (true, true) => "Sending message".into(),
        (false, false) => format!("Sent {description}"),
        (false, true) => "Sent a message".into(),
    }
}

/// `sendMessageMeta`: a short prompt preview, never the agent id.
pub fn send_message_meta(description: &str, message: &str) -> String {
    if !description.is_empty() {
        return String::new();
    }
    collapse(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // subagent.test.ts
    #[test]
    fn tools_and_args() {
        assert!(is_subagent_tool("Task") && is_subagent_tool("Agent"));
        assert!(!is_subagent_tool("TaskCreate") && !is_subagent_tool("SendMessage"));
        let a = parse_subagent_args(Some(&json!({
            "description": "Research VS Code tokenization pipeline",
            "subagent_type": "Explore",
            "run_in_background": true,
            "prompt": "Find tokenizeViewport and forceTokenization.",
        })));
        assert_eq!(a.subagent_type, "Explore");
        assert!(a.background);
        assert!(a.prompt.contains("tokenizeViewport"));
        let meta = subagent_meta(&a.subagent_type, &a.model);
        assert_eq!(meta, "Explore");
        assert!(!meta.contains('{') && !meta.contains("description"));
    }

    #[test]
    fn launch_ack() {
        let ack = "Async agent launched successfully. (This tool result is internal metadata – never quote or paste any part of it to the user.)\nagentId: ad643cfdedf8d9846";
        assert!(is_subagent_launch_ack(ack));
        assert!(!is_subagent_launch_ack(
            "The tokenizer lives in tokenizationTextModelPart.ts."
        ));
        assert!(!is_subagent_launch_ack(""));
        assert!(!is_subagent_launch_ack(
            "see Async agent launched successfully plus agentId and internal metadata in comments"
        ));
    }

    #[test]
    fn send_message() {
        assert!(is_send_message_tool("SendMessage"));
        let a = parse_send_message_args(Some(&json!({
            "to": "ad643cfdedf8d9846",
            "description": "Ask about tokenizeViewport",
            "prompt": "Where is forceTokenization called?",
        })));
        assert_eq!(send_message_label(false, &a.description), "Sent Ask about tokenizeViewport");
        assert_eq!(send_message_meta(&a.description, &a.message), "");
        assert_eq!(send_message_meta("", &a.message), "Where is forceTokenization called?");
    }
}
