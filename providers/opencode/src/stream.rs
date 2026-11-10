//! Parses `opencode run --format json` lines into Recur's normalized
//! [`AgentEvent`]s and a [`TurnResult`].
//!
//! Every line is `{type, timestamp, sessionID, ...}`. The CLI emits finished
//! text/reasoning parts, completed or failed tool parts, step finishes, and
//! errors. It does not emit an explicit end event; process exit is the end.

use std::time::Instant;

use recur_agent::event::{AgentEvent, TokenUsage, ToolResultEvent, ToolStep, TurnResult};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RunLine {
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "sessionID")]
    session_id: String,
    part: Option<Part>,
    error: Option<RunError>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Part {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    text: String,
    #[serde(rename = "callID")]
    call_id: String,
    tool: String,
    state: Option<ToolState>,
    tokens: Option<Tokens>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ToolState {
    status: String,
    input: Value,
    output: String,
    error: String,
    title: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Tokens {
    input: f64,
    output: f64,
    cache: CacheTokens,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CacheTokens {
    read: f64,
    write: f64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RunError {
    name: String,
    data: Option<RunErrorData>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RunErrorData {
    message: String,
}

/// Accumulates one run's lines.
#[derive(Debug)]
pub struct TurnState {
    pub session_id: String,
    pub text: String,
    pub error: String,
    pub usage: TokenUsage,
    pub steps: i64,
    started: Instant,
}

impl Default for TurnState {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            text: String::new(),
            error: String::new(),
            usage: TokenUsage::default(),
            steps: 0,
            started: Instant::now(),
        }
    }
}

fn input_json(input: &Value) -> String {
    match input {
        Value::Null => String::new(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

impl TurnState {
    /// Feeds one stdout line. Unrecognized lines and OpenCode's own log noise
    /// are ignored.
    pub fn ingest(&mut self, line: &str) -> Vec<AgentEvent> {
        let line = line.trim();
        if line.is_empty() {
            return Vec::new();
        }
        let Ok(line) = serde_json::from_str::<RunLine>(line) else {
            return Vec::new();
        };
        if self.session_id.is_empty() && !line.session_id.is_empty() {
            self.session_id = line.session_id;
        }
        match line.kind.as_str() {
            "text" => {
                let Some(part) = line.part else {
                    return Vec::new();
                };
                if part.text.is_empty() {
                    return Vec::new();
                }
                if !self.text.is_empty() {
                    self.text.push('\n');
                }
                self.text.push_str(&part.text);
                vec![AgentEvent::Text {
                    text: part.text,
                    uuid: part.id,
                    parent: String::new(),
                }]
            }
            "reasoning" => {
                let Some(part) = line.part else {
                    return Vec::new();
                };
                if part.text.is_empty() {
                    return Vec::new();
                }
                vec![AgentEvent::Thinking {
                    text: part.text,
                    signature: String::new(),
                    uuid: part.id,
                    parent: String::new(),
                }]
            }
            "tool_use" => {
                let Some(part) = line.part else {
                    return Vec::new();
                };
                let Some(state) = part.state else {
                    return Vec::new();
                };
                let input = input_json(&state.input);
                let step = AgentEvent::Step(ToolStep {
                    id: part.call_id.clone(),
                    name: part.tool.clone(),
                    summary: state.title.clone(),
                    input: input.clone(),
                    parent: String::new(),
                    uuid: part.id.clone(),
                });
                let (text, is_error) = match state.status.as_str() {
                    "completed" => (state.output, false),
                    "error" => (state.error, true),
                    _ => return Vec::new(),
                };
                let result = AgentEvent::ToolResult(ToolResultEvent {
                    id: part.call_id,
                    name: part.tool,
                    input,
                    text,
                    is_error,
                    truncated: false,
                    parent: String::new(),
                    uuid: part.id,
                });
                vec![step, result]
            }
            "step_finish" => {
                self.steps += 1;
                let Some(part) = line.part else {
                    return Vec::new();
                };
                let Some(tokens) = part.tokens else {
                    return Vec::new();
                };
                self.usage = TokenUsage {
                    input_tokens: tokens.input as i64,
                    output_tokens: tokens.output as i64,
                    cache_creation_tokens: tokens.cache.write as i64,
                    cache_read_tokens: tokens.cache.read as i64,
                    context_window: 0,
                };
                vec![AgentEvent::Usage(self.usage.clone())]
            }
            "error" => {
                let Some(error) = line.error else {
                    return Vec::new();
                };
                let message = error
                    .data
                    .as_ref()
                    .filter(|d| !d.message.is_empty())
                    .map(|d| d.message.clone())
                    .unwrap_or(error.name);
                if !message.is_empty() {
                    if !self.error.is_empty() {
                        self.error.push('\n');
                    }
                    self.error.push_str(&message);
                    return vec![AgentEvent::Notice { text: message }];
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// The final result after the process exits. `ok` is the exit status.
    /// Stderr alone does not fail a turn: wrappers such as a mise shim
    /// print progress there on every run. It is kept for a failed turn.
    pub fn finish(self, ok: bool, stderr: &str) -> TurnResult {
        let is_error = !ok || !self.error.trim().is_empty();
        let stderr = if is_error {
            [self.error.trim(), stderr.trim()]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            String::new()
        };
        let duration_ms = self.started.elapsed().as_millis() as i64;
        TurnResult {
            session_id: self.session_id,
            subtype: if is_error { "error" } else { "success" }.into(),
            is_error,
            num_turns: self.steps.max(1),
            duration_ms,
            text: self.text,
            usage: self.usage,
            stderr,
            ..Default::default()
        }
    }

    /// A canceled turn keeps whatever it produced.
    pub fn partial(self, stderr: &str) -> TurnResult {
        let mut result = self.finish(false, stderr);
        result.subtype = "canceled".into();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_reasoning_tools_and_usage() {
        let mut s = TurnState::default();
        let lines = [
            r#"{"type":"text","timestamp":1,"sessionID":"ses_1","part":{"id":"p1","type":"text","text":"Hello"}}"#,
            r#"{"type":"reasoning","timestamp":2,"sessionID":"ses_1","part":{"id":"p2","type":"reasoning","text":"Thinking"}}"#,
            r#"{"type":"tool_use","timestamp":3,"sessionID":"ses_1","part":{"id":"p3","type":"tool","callID":"call_1","tool":"bash","state":{"status":"completed","input":{"command":"ls"},"output":"a\nb","title":"ls"}}}"#,
            r#"{"type":"step_finish","timestamp":4,"sessionID":"ses_1","part":{"id":"p4","type":"step-finish","tokens":{"input":12,"output":3,"reasoning":1,"cache":{"read":5,"write":0}}}}"#,
        ];
        let mut events = Vec::new();
        for line in lines {
            events.extend(s.ingest(line));
        }
        assert_eq!(s.session_id, "ses_1");
        assert_eq!(s.text, "Hello");
        assert!(matches!(&events[0], AgentEvent::Text { text, .. } if text == "Hello"));
        assert!(matches!(&events[1], AgentEvent::Thinking { text, .. } if text == "Thinking"));
        assert!(
            matches!(&events[2], AgentEvent::Step(step) if step.name == "bash" && step.id == "call_1")
        );
        assert!(matches!(&events[3], AgentEvent::ToolResult(r) if r.text == "a\nb"));
        assert!(
            matches!(&events[4], AgentEvent::Usage(u) if u.input_tokens == 12 && u.cache_read_tokens == 5)
        );
        let result = s.finish(true, "");
        assert!(!result.is_error);
        assert_eq!(result.session_id, "ses_1");
        assert_eq!(result.text, "Hello");
        assert_eq!(result.num_turns, 1);
    }

    #[test]
    fn error_event_and_exit_failure() {
        let mut s = TurnState::default();
        let evs = s.ingest(
            r#"{"type":"error","timestamp":1,"sessionID":"ses_2","error":{"name":"APIError","data":{"message":"bad key"}}}"#,
        );
        assert!(matches!(&evs[0], AgentEvent::Notice { text } if text == "bad key"));
        let r = s.finish(false, "exited 1");
        assert!(r.is_error);
        assert!(r.stderr.contains("bad key"));
        assert!(r.stderr.contains("exited 1"));
    }

    #[test]
    fn stderr_alone_does_not_fail_a_turn() {
        // A mise wrapper around `opencode` prints this on every run.
        let mise = "mise by @jdx – installing 1 tool\n\
                    mise ⇢ opencode@1.18.31  1ms · already installed";
        let mut s = TurnState::default();
        assert!(
            s.ingest("mise ~/.config/mise/config.toml tools: opencode@1.18.31")
                .is_empty()
        );
        s.ingest(
            r#"{"type":"text","timestamp":1,"sessionID":"ses_3","part":{"id":"p1","type":"text","text":"Hi"}}"#,
        );
        let r = s.finish(true, mise);
        assert!(!r.is_error, "{r:?}");
        assert_eq!((r.subtype.as_str(), r.stderr.as_str()), ("success", ""));
        assert_eq!(r.text, "Hi");

        let failed = TurnState::default().finish(false, mise);
        assert!(failed.is_error && failed.stderr.contains("installing 1 tool"));
    }

    #[test]
    fn ignores_noise() {
        let mut s = TurnState::default();
        assert!(s.ingest("not json").is_empty());
        assert!(s.ingest("").is_empty());
    }
}
