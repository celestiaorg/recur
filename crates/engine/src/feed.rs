//! The reload-path activity feed. Agent activity (tool rows and narration) is
//! projected from the specialists' session transcripts; loop-level facts
//! (outcomes, deviations) come from the durable events table. Live streaming is
//! unchanged; this rebuilds what it showed. A loop with no sessions
//! (pre-mirror) replays its events.

use std::collections::HashMap;

use recur_agent::display::{FEED_INPUT_LIMIT, summarize_tool_input};
use recur_domain::{
    AgentRole, BLOCK_TEXT, BLOCK_TOOL_RESULT, BLOCK_TOOL_USE, FEED_KIND_DEVIATION, FEED_KIND_NOTE,
    FEED_KIND_OUTCOME, FEED_KIND_TOOL, FeedItem, Session,
};
use serde::Deserialize;

use crate::Engine;
use crate::session_config::AgentConfig;
use crate::text::{html_escape, truncate};
use crate::turn::transcript::display_tool;

/// One projected row with the millisecond timestamp the two sources merge
/// on.
#[derive(Debug, Clone)]
pub struct TimedFeedItem {
    pub ts: i64,
    pub item: FeedItem,
}

#[cfg(test)]
mod tests;

/// The session kinds whose turns emitted feed rows live (the Maestro's
/// turns never did).
fn feed_role(kind: &str) -> bool {
    kind == AgentRole::Coder.as_str()
        || kind == AgentRole::Reviewer.as_str()
        || kind == AgentRole::Qa.as_str()
}

/// The FeedItem embedded in a `workspace.event` payload.
pub fn feed_item_from_event(payload_json: &str) -> Option<FeedItem> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Payload {
        event: FeedItem,
    }
    let p: Payload = serde_json::from_str(payload_json).ok()?;
    (!p.event.id.is_empty()).then_some(p.event)
}

impl Engine {
    /// A loop's activity feed for the reload path, newest first: the
    /// session projection merged with the loop facts, or the legacy events
    /// replay for a loop without sessions (or whose sessions produced
    /// nothing, e.g. only a Maestro chat).
    pub fn reconstruct_feed(&self, workspace_id: &str) -> Vec<FeedItem> {
        let sessions = self
            .store
            .sessions_for_loop(workspace_id)
            .unwrap_or_default();
        if sessions.is_empty() {
            return self.reconstruct_from_events(workspace_id);
        }
        let mut all = self.feed_rows_from_sessions(&sessions);
        all.extend(self.loop_fact_rows(workspace_id));
        if all.is_empty() {
            return self.reconstruct_from_events(workspace_id);
        }
        // Ties keep insertion order: session rows first, as live a phase
        // outcome follows the turn's tools.
        all.sort_by_key(|r| r.ts);
        all.into_iter().rev().map(|r| r.item).collect()
    }

    /// Tool rows and notes from the specialist transcripts, exactly as the
    /// live walk emitted them: a tool_use is a "Name: <code>summary</code>"
    /// row (its result patched in), assistant text a note.
    fn feed_rows_from_sessions(&self, sessions: &[Session]) -> Vec<TimedFeedItem> {
        let mut out: Vec<TimedFeedItem> = Vec::new();
        for sess in sessions {
            if !feed_role(&sess.kind) {
                continue;
            }
            let cfg = AgentConfig::parse(&sess.config_json);
            // Maestro rows are not click-throughs.
            let click = if cfg.code == "MA" {
                String::new()
            } else {
                cfg.code.clone()
            };
            let mut tool_idx: HashMap<String, usize> = HashMap::new();
            let Ok(Some(msgs)) = self.store.session_transcript(&sess.id) else {
                continue;
            };
            for msg in &msgs {
                for b in msg.blocks() {
                    match b.r#type.as_str() {
                        BLOCK_TEXT => {
                            if msg.role != "assistant" {
                                continue;
                            }
                            let p = b.as_text().unwrap_or_default();
                            let t = truncate(&p.text, FEED_INPUT_LIMIT);
                            if t.is_empty() {
                                continue;
                            }
                            out.push(TimedFeedItem {
                                ts: b.created_at,
                                item: FeedItem {
                                    id: format!("s-{}", b.id),
                                    agent_code: cfg.code.clone(),
                                    name: cfg.name.clone(),
                                    text: html_escape(&t),
                                    time: "now".into(),
                                    kind: FEED_KIND_NOTE.into(),
                                    click_agent: click.clone(),
                                    ..FeedItem::default()
                                },
                            });
                        }
                        BLOCK_TOOL_USE => {
                            let p = b.as_tool_use().unwrap_or_default();
                            // Blocks hold the provider's raw name and args;
                            // the live feed showed the common vocabulary.
                            let prov = self.providers.for_id(&msg.provider);
                            let d = display_tool(prov.as_deref(), &p.name, &p.input);
                            let mut text = html_escape(&d.name);
                            let summary = summarize_tool_input(&d.name, &d.input);
                            if !summary.is_empty() {
                                text.push_str(&format!(": <code>{}</code>", html_escape(&summary)));
                            }
                            out.push(TimedFeedItem {
                                ts: b.created_at,
                                item: FeedItem {
                                    id: format!("s-{}", b.id),
                                    agent_code: cfg.code.clone(),
                                    name: cfg.name.clone(),
                                    text,
                                    time: "now".into(),
                                    kind: FEED_KIND_TOOL.into(),
                                    tool_use_id: p.provider_tool_use_id.clone(),
                                    click_agent: click.clone(),
                                    ..FeedItem::default()
                                },
                            });
                            tool_idx.insert(p.id.clone(), out.len() - 1);
                        }
                        BLOCK_TOOL_RESULT => {
                            let p = b.as_tool_result().unwrap_or_default();
                            if let Some(&idx) = tool_idx.get(&p.tool_use_id) {
                                out[idx].item.tool_result = truncate(&p.result, FEED_INPUT_LIMIT);
                                out[idx].item.is_tool_error = p.is_error;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        out
    }

    /// The loop facts (outcomes and deviations) from the events table. A
    /// re-emitted item keeps its first position with the latest payload, so
    /// a wave card does not jump each time a child settles.
    pub fn loop_fact_rows(&self, workspace_id: &str) -> Vec<TimedFeedItem> {
        let Ok(Some(evs)) = self.store.events_for_loop(workspace_id) else {
            return Vec::new();
        };
        let mut out: Vec<TimedFeedItem> = Vec::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        for ev in evs {
            if ev.r#type != "workspace.event" {
                continue;
            }
            let Some(fi) = feed_item_from_event(&ev.payload_json) else {
                continue;
            };
            if fi.kind != FEED_KIND_OUTCOME && fi.kind != FEED_KIND_DEVIATION && !fi.is_deviation {
                continue;
            }
            if let Some(&idx) = seen.get(&fi.id) {
                out[idx].item = fi;
                continue;
            }
            seen.insert(fi.id.clone(), out.len());
            out.push(TimedFeedItem {
                ts: ev.ts,
                item: fi,
            });
        }
        out
    }

    /// The legacy reload path: every `workspace.event` item oldest to
    /// newest, deduped by id keeping the latest payload, newest first.
    fn reconstruct_from_events(&self, workspace_id: &str) -> Vec<FeedItem> {
        let Ok(Some(evs)) = self.store.events_for_loop(workspace_id) else {
            return Vec::new();
        };
        let mut by_id: HashMap<String, FeedItem> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for ev in evs {
            if ev.r#type != "workspace.event" {
                continue;
            }
            let Some(fi) = feed_item_from_event(&ev.payload_json) else {
                continue;
            };
            if !by_id.contains_key(&fi.id) {
                order.push(fi.id.clone());
            }
            by_id.insert(fi.id.clone(), fi);
        }
        order
            .iter()
            .rev()
            .filter_map(|id| by_id.remove(id))
            .collect()
    }
}
