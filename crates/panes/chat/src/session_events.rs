//! Live chat events as the chat surfaces consume them: the session's workspace (`""` for Ask
//! Recur), its code, and the
//! payload with the same filters the listeners applied.

use recur_agent::context_usage::ContextUsage;
use recur_engine::view::chat::{CompactEvt, FallbackEvt, StreamOpts, ToolExtras, WorkspaceStep};
use recur_services::AppEvent;
use recur_services::events::SessionTag;

/// A cumulative text or thinking snapshot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamEvt {
    pub text: String,
    /// Token estimate of redacted thinking (thinking only).
    pub tokens: i64,
    pub seq: i64,
    pub replace: bool,
    pub done: bool,
    pub parent_id: String,
    pub uuid: String,
    pub owner_code: String,
}

impl StreamEvt {
    pub fn opts(&self) -> StreamOpts {
        StreamOpts {
            done: self.done,
            parent_id: self.parent_id.clone(),
            uuid: self.uuid.clone(),
        }
    }
}

/// A turn footer with the final context usage when the provider reported it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnEndEvt {
    pub duration_ms: i64,
    pub usage: Option<ContextUsage>,
}

#[derive(Clone, Debug)]
pub enum SessionSignal {
    Delta(StreamEvt),
    Thinking(StreamEvt),
    Step(WorkspaceStep),
    ToolResult {
        id: String,
        result: String,
        is_error: bool,
        extras: Option<ToolExtras>,
    },
    ToolInput {
        id: String,
        extras: ToolExtras,
    },
    ContextUsage(ContextUsage),
    TurnEnd(TurnEndEvt),
    Compact(CompactEvt),
    Notice(String),
    User(String),
    Fallback(FallbackEvt),
}

/// One live chat event: workspace id (`""` for Ask Recur), code, payload.
#[derive(Clone, Debug)]
pub struct SessionEvent {
    pub workspace_id: String,
    pub code: String,
    pub signal: SessionSignal,
}

/// `sessionWorkspaceId`: Ask Recur sessions are `""`; others need a workspace.
fn session_workspace_id(tag: &SessionTag) -> Option<String> {
    if tag.surface == "app" {
        return Some(tag.workspace_id.clone());
    }
    (!tag.workspace_id.is_empty()).then(|| tag.workspace_id.clone())
}

fn usage(input: i64, creation: i64, read: i64, window: i64) -> Option<ContextUsage> {
    let u = ContextUsage {
        input_tokens: input,
        cache_creation_tokens: creation,
        cache_read_tokens: read,
        context_window: window,
        compacting: false,
        compacted: false,
    };
    (window > 0 && u.used_tokens() > 0).then_some(u)
}

/// Decodes a `session.*` event; `None` for other events and for payloads the
/// listeners dropped.
pub fn session_event(ev: &AppEvent) -> Option<SessionEvent> {
    let (tag, signal) = match ev {
        AppEvent::SessionDelta(d) => {
            if d.text.is_empty() {
                return None;
            }
            (
                &d.tag,
                SessionSignal::Delta(StreamEvt {
                    text: d.text.clone(),
                    tokens: 0,
                    seq: d.seq.unwrap_or(0),
                    replace: d.replace,
                    done: d.done,
                    parent_id: d.parent_id.clone().unwrap_or_default(),
                    uuid: d.uuid.clone(),
                    owner_code: d.tag.owner_code.clone(),
                }),
            )
        }
        AppEvent::SessionThinking(d) => {
            let text = d.text.clone().unwrap_or_default();
            let tokens = d.tokens.unwrap_or(0);
            if text.is_empty() && tokens == 0 {
                return None;
            }
            (
                &d.tag,
                SessionSignal::Thinking(StreamEvt {
                    text,
                    tokens,
                    seq: d.seq.unwrap_or(0),
                    replace: d.replace,
                    done: d.done,
                    parent_id: d.parent_id.clone().unwrap_or_default(),
                    uuid: d.uuid.clone(),
                    owner_code: d.tag.owner_code.clone(),
                }),
            )
        }
        AppEvent::SessionStep(d) => (
            &d.tag,
            SessionSignal::Step(WorkspaceStep {
                id: d.id.clone(),
                tool: d.tool.clone(),
                tool_input: d.tool_input.clone(),
                description: d.description.clone(),
                input: d.input.clone(),
                parent_id: d.parent_id.clone(),
                uuid: d.uuid.clone(),
                owner_code: d.tag.owner_code.clone(),
            }),
        ),
        AppEvent::SessionToolResult(d) => {
            let extras = (!d.input.is_empty() || !d.tool_input.is_empty()).then(|| ToolExtras {
                input: d.input.clone(),
                tool_input: d.tool_input.clone(),
            });
            (
                &d.tag,
                SessionSignal::ToolResult {
                    id: d.id.clone(),
                    result: d.result.clone(),
                    is_error: d.is_error,
                    extras,
                },
            )
        }
        AppEvent::SessionToolInput(d) => {
            if d.input.is_empty() && d.tool_input.is_empty() {
                return None;
            }
            (
                &d.tag,
                SessionSignal::ToolInput {
                    id: d.id.clone(),
                    extras: ToolExtras {
                        input: d.input.clone(),
                        tool_input: d.tool_input.clone(),
                    },
                },
            )
        }
        AppEvent::SessionContextUsage(d) => (
            &d.tag,
            SessionSignal::ContextUsage(usage(
                d.input_tokens,
                d.cache_creation_tokens,
                d.cache_read_tokens,
                d.context_window,
            )?),
        ),
        AppEvent::SessionTurnEnd(d) => (
            &d.tag,
            SessionSignal::TurnEnd(TurnEndEvt {
                duration_ms: d.duration_ms,
                usage: usage(
                    d.input_tokens.unwrap_or(0),
                    d.cache_creation_tokens.unwrap_or(0),
                    d.cache_read_tokens.unwrap_or(0),
                    d.context_window.unwrap_or(0),
                ),
            }),
        ),
        AppEvent::SessionCompact(d) => (
            &d.tag,
            SessionSignal::Compact(CompactEvt {
                status: d.status.clone(),
                trigger: d.trigger.clone(),
                summary: d.summary.clone(),
                pre_tokens: d.pre_tokens,
            }),
        ),
        AppEvent::SessionNotice(d) => {
            if d.text.trim().is_empty() {
                return None;
            }
            (&d.tag, SessionSignal::Notice(d.text.clone()))
        }
        AppEvent::SessionUser(d) => {
            if d.text.is_empty() {
                return None;
            }
            (&d.tag, SessionSignal::User(d.text.clone()))
        }
        AppEvent::SessionFallback(d) => {
            if d.to.is_empty() && d.retracted.is_empty() {
                return None;
            }
            (
                &d.tag,
                SessionSignal::Fallback(FallbackEvt {
                    from: d.from.clone(),
                    to: d.to.clone(),
                    from_label: d.from_label.clone(),
                    to_label: d.to_label.clone(),
                    retracted: d.retracted.clone(),
                }),
            )
        }
        _ => return None,
    };
    Some(SessionEvent {
        workspace_id: session_workspace_id(tag)?,
        code: tag.code.clone(),
        signal,
    })
}

/// An `agent.updated` for a workspace agent: `(workspace id, agent id, status)`.
pub fn agent_updated(ev: &AppEvent) -> Option<(&str, &str, &str)> {
    match ev {
        AppEvent::AgentUpdated(a) => Some((&a.workspace_id, &a.agent_id, &a.status)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wire(name: &str, data: serde_json::Value) -> AppEvent {
        AppEvent::from_wire(name, data).expect("event")
    }

    #[test]
    fn decodes_with_the_svelte_filters() {
        let e = session_event(&wire(
            "session.delta",
            json!({"workspaceId": "w", "code": "MA", "text": "hi", "seq": 2, "replace": true}),
        ))
        .expect("delta");
        assert_eq!((e.workspace_id.as_str(), e.code.as_str()), ("w", "MA"));
        assert!(matches!(e.signal, SessionSignal::Delta(ref d) if d.seq == 2 && d.replace));
        assert!(
            session_event(&wire(
                "session.delta",
                json!({"workspaceId": "w", "code": "MA", "text": ""})
            ))
            .is_none()
        );
        assert!(
            session_event(&wire("session.delta", json!({"code": "MA", "text": "x"}))).is_none()
        );
        let app = session_event(&wire(
            "session.delta",
            json!({"surface": "app", "code": "APP", "text": "x"}),
        ))
        .expect("app");
        assert_eq!(app.workspace_id, "");
        assert!(
            session_event(&wire(
                "session.thinking",
                json!({"workspaceId": "w", "code": "PA"})
            ))
            .is_none()
        );
        assert!(
            session_event(&wire(
                "session.notice",
                json!({"workspaceId": "w", "code": "PA", "text": "  "})
            ))
            .is_none()
        );
        let t = session_event(&wire(
            "session.turn_end",
            json!({"workspaceId": "w", "code": "PA", "durationMs": 1200, "inputTokens": 5, "contextWindow": 100}),
        ))
        .expect("turn");
        assert!(matches!(
            t.signal,
            SessionSignal::TurnEnd(TurnEndEvt {
                duration_ms: 1200,
                usage: Some(_)
            })
        ));
    }
}
