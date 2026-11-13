//! Display helpers for Claude Code's Agent/Task subagent
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListedAgent {
    pub name: String,
    pub r#ref: String,
    pub details: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListedAgentGroup {
    pub title: String,
    pub agents: Vec<ListedAgent>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListAgentsView {
    pub this: Option<ListedAgent>,
    pub groups: Vec<ListedAgentGroup>,
}

impl ListAgentsView {
    pub fn agent_count(&self) -> usize {
        self.groups.iter().map(|g| g.agents.len()).sum()
    }
}

fn name_ref(s: &str) -> ListedAgent {
    let s = s.trim();
    if let Some(body) = s.strip_suffix(']')
        && let Some((name, r)) = body.rsplit_once('[')
    {
        return ListedAgent {
            name: name.trim_end().to_owned(),
            r#ref: r.to_owned(),
            details: Vec::new(),
        };
    }
    ListedAgent {
        name: s.to_owned(),
        ..Default::default()
    }
}

/// ListAgents result text: a "This session is NAME [ref]" line, then groups
/// like "Peer sessions (1):" whose indented rows are "name [ref] · kind · …".
/// None when nothing parses, so the card falls back to the raw text.
pub fn parse_list_agents(result: &str) -> Option<ListAgentsView> {
    let mut v = ListAgentsView::default();
    for line in result.lines() {
        let indented = line.starts_with(char::is_whitespace);
        if let Some(rest) = line.strip_prefix("This session is ")
            && let Some(end) = rest.find(']')
        {
            v.this = Some(name_ref(&rest[..=end]));
        } else if !indented
            && let Some((title, n)) = line
                .trim_end()
                .strip_suffix("):")
                .and_then(|h| h.rsplit_once(" ("))
            && !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
        {
            v.groups.push(ListedAgentGroup {
                title: title.trim().to_owned(),
                agents: Vec::new(),
            });
        } else if indented && let Some(g) = v.groups.last_mut() {
            let mut parts = line.split(" · ").map(str::trim).filter(|p| !p.is_empty());
            let Some(head) = parts.next() else { continue };
            g.agents.push(ListedAgent {
                details: parts.map(str::to_owned).collect(),
                ..name_ref(head)
            });
        }
    }
    (v.this.is_some() || !v.groups.is_empty()).then_some(v)
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

    #[test]
    fn parses_list_agents_self_and_peers() {
        let v = parse_list_agents("This session is velvety-biryani-d6 [5caa52] \u{2014} the name other sessions use to message it.\n\nPeer sessions (1):\n  9ec5ff95-5a21 [6c9da5]  \u{b7}  interactive  \u{b7}  idle  \u{b7}  started 12d ago\n").unwrap();
        assert_eq!(
            v.this.as_ref().map(|a| (a.name.as_str(), a.r#ref.as_str())),
            Some(("velvety-biryani-d6", "5caa52"))
        );
        assert_eq!(v.groups.len(), 1);
        assert_eq!(v.groups[0].title, "Peer sessions");
        assert_eq!(
            v.groups[0].agents,
            vec![ListedAgent {
                name: "9ec5ff95-5a21".into(),
                r#ref: "6c9da5".into(),
                details: vec![
                    "interactive".into(),
                    "idle".into(),
                    "started 12d ago".into()
                ],
            }]
        );
        assert_eq!(v.agent_count(), 1);
        assert_eq!(parse_list_agents("No agents."), None);
    }
}
