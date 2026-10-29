//! Transcript flattening: the durable session transcript, the single source of
//! truth the mirror wrote, turned into UI-ready chat items, plus the
//! stored-prompt readers Retry and loop goals replay.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use recur_agent::display::{
    chat_tool_input, chat_tool_result, is_subagent_launch_meta, is_task_tool, summarize_tool_input,
    tool_description,
};
use recur_agent::{Flags, Provider, ToolDisplay, UserImage};
use recur_domain::{
    BLOCK_COMPACT, BLOCK_FALLBACK, BLOCK_IMAGE, BLOCK_TEXT, BLOCK_THINKING, BLOCK_TOOL_RESULT,
    BLOCK_TOOL_USE, ChatImageRef, ChatItem, ContentBlock, Message, STREAM_PENDING,
    STREAM_STREAMING, parse_model_ref,
};

use crate::Engine;
use crate::attachments::{user_image_from_stored, user_images_by_ids};
use crate::codes::{CHAT_RETRY_DISPLAY_SENTINEL, normalize_pair_chat_code};
use crate::registry::ProviderRegistry;
use crate::session_config::AgentConfig;

/// The first line of a fork-inject preamble.
pub const FORK_PREAMBLE_LEAD: &str = "This chat was forked from an earlier conversation.";

/// A switch_mode remount's user display ("Switched to Plan: continuing").
pub fn is_mode_switch_continue_display(s: &str) -> bool {
    s.starts_with("Switched to ") && s.ends_with(": continuing")
}

/// A remount continuation row, by its display or wire text.
pub fn user_message_is_mode_switch_continue(m: &Message) -> bool {
    m.blocks().iter().any(|b| {
        b.as_text().is_some_and(|p| {
            is_mode_switch_continue_display(&p.display_text)
                || is_mode_switch_continue_display(&p.text)
        })
    })
}

/// A role=user message that is a human chat turn (text/image), not a
/// provider tool_result carrier.
pub fn user_message_is_chat_prompt(m: &Message) -> bool {
    let mut has = false;
    for b in m.blocks() {
        match b.r#type.as_str() {
            BLOCK_TOOL_RESULT | BLOCK_TOOL_USE | BLOCK_THINKING => return false,
            BLOCK_TEXT | BLOCK_IMAGE => has = true,
            _ => {}
        }
    }
    has
}

/// The bubble text after a fork inject when no display text was stored:
/// what follows `</transcript>`; empty when nothing does (image-first
/// composer); unchanged without the lead.
pub fn user_text_after_fork_preamble(s: &str) -> String {
    if !s.contains(FORK_PREAMBLE_LEAD) {
        return s.to_owned();
    }
    const CLOSE: &str = "</transcript>";
    let Some(i) = s.rfind(CLOSE) else {
        return s.to_owned();
    };
    let rest = s[i + CLOSE.len()..].trim_start_matches('\n');
    if rest.trim().is_empty() {
        return String::new();
    }
    rest.to_owned()
}

/// The content-block stamp, else the message's.
pub fn chat_item_ts(m: &Message, b: Option<&ContentBlock>) -> i64 {
    if let Some(b) = b
        && b.created_at > 0
    {
        return b.created_at;
    }
    if m.created_at > 0 {
        return m.created_at;
    }
    if m.completed_at > 0 {
        return m.completed_at;
    }
    m.started_at
}

/// A user chat item from text + image blocks, restoring attachment markers
/// so the UI places thumbnails inline. Once a display label has been used,
/// later blocks without one are host wire and stay hidden.
pub fn flatten_user_message(m: &Message) -> Option<ChatItem> {
    let mut text = String::new();
    let mut atts: Vec<ChatImageRef> = Vec::new();
    let mut has = false;
    let mut used_display = false;
    for b in m.blocks() {
        match b.r#type.as_str() {
            BLOCK_TEXT => {
                let Some(p) = b.as_text() else { continue };
                let shown = if !p.display_text.is_empty() {
                    used_display = true;
                    if p.display_text == recur_agent::attach::DISPLAY_HIDDEN_SENTINEL {
                        continue;
                    }
                    p.display_text
                } else if used_display {
                    continue;
                } else {
                    let label = crate::roles::coder_dispatch_display(&p.text);
                    if !label.is_empty() {
                        label.to_owned()
                    } else {
                        let stripped = user_text_after_fork_preamble(&p.text);
                        if stripped != p.text {
                            used_display = true;
                        }
                        stripped
                    }
                };
                if shown == CHAT_RETRY_DISPLAY_SENTINEL || is_mode_switch_continue_display(&shown) {
                    continue;
                }
                if !shown.trim().is_empty() {
                    has = true;
                }
                text.push_str(&shown);
            }
            BLOCK_IMAGE => {
                let Some(p) = b.as_image() else { continue };
                has = true;
                let marker = recur_agent::attach::marker(&p.id);
                if !text.contains(&marker) {
                    text.push_str(&marker);
                }
                atts.push(ChatImageRef {
                    id: p.id,
                    name: p.name,
                    mime: p.mime,
                    url: format!("/{}", p.path),
                });
            }
            _ => {}
        }
    }
    if !has {
        return None;
    }
    Some(ChatItem {
        role: "user".into(),
        kind: "text".into(),
        text,
        attachments: atts,
        ts: chat_item_ts(m, None),
        ..ChatItem::default()
    })
}

/// The stored wire and images of one human chat turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LastChatPrompt {
    pub wire: String,
    pub imgs: Vec<UserImage>,
}

/// Rebuilds a stored user row's provider wire (text + attachment markers)
/// and reloads its images; images whose file is gone are dropped.
pub fn reconstruct_chat_prompt(data_dir: &Path, m: &Message) -> Option<LastChatPrompt> {
    let mut wire = String::new();
    let mut imgs = Vec::new();
    let mut has = false;
    for b in m.blocks() {
        match b.r#type.as_str() {
            BLOCK_TEXT => {
                let Some(p) = b.as_text() else { continue };
                if p.text == CHAT_RETRY_DISPLAY_SENTINEL {
                    continue;
                }
                if !p.text.trim().is_empty() {
                    has = true;
                }
                wire.push_str(&p.text);
            }
            BLOCK_IMAGE => {
                let Some(p) = b.as_image() else { continue };
                let Some(img) = user_image_from_stored(data_dir, &p) else {
                    continue;
                };
                wire.push_str(&recur_agent::attach::marker(&p.id));
                imgs.push(img);
                has = true;
            }
            _ => {}
        }
    }
    has.then_some(LastChatPrompt { wire, imgs })
}

/// One flattened row plus the provider tool-use id it nests under.
struct ChatNode {
    item: ChatItem,
    parent: String,
}

/// How to unpack a tool result: results are stored in the originating
/// provider's envelope.
struct RawToolRef {
    provider: String,
    name: String,
}

/// A raw tool call in the common vocabulary. A provider that is not
/// registered gets the default (Claude) rules.
pub(crate) fn display_tool(p: Option<&dyn Provider>, name: &str, input: &str) -> ToolDisplay {
    match p {
        Some(p) => p.display_tool(name, input),
        None => ToolDisplay {
            name: recur_agent::display::canonical_claude_tool(name).to_owned(),
            input: input.to_owned(),
        },
    }
}

/// A raw tool result as cards show it (Task launch acks render empty).
pub(crate) fn display_tool_result(p: Option<&dyn Provider>, name: &str, raw: &str) -> String {
    match p {
        Some(p) => recur_agent::display::display_tool_result(p, name, raw),
        None if is_task_tool(name) && is_subagent_launch_meta(raw) => String::new(),
        None => raw.to_owned(),
    }
}

fn model_label(p: Option<&dyn Provider>, model: &str) -> String {
    if model.trim().is_empty() {
        return String::new();
    }
    match p {
        Some(p) => p.model_label(model),
        None => String::new(),
    }
}

/// A session's messages as chat items. `model` / `flags` are the session's
/// dock selection (footer context windows).
pub fn flatten_transcript(
    providers: &ProviderRegistry,
    msgs: &[Message],
    model: &str,
    flags: &Flags,
) -> Vec<ChatItem> {
    let mut cache: HashMap<String, Option<Arc<dyn Provider>>> = HashMap::new();
    let mut provider_for = |id: &str| -> Option<Arc<dyn Provider>> {
        cache
            .entry(id.to_owned())
            .or_insert_with(|| providers.for_id(id))
            .clone()
    };
    let mut nodes: Vec<ChatNode> = Vec::new();
    let mut tool_idx: HashMap<String, usize> = HashMap::new();
    let mut tool_raw: HashMap<String, RawToolRef> = HashMap::new();
    // After a crash a tool_use may have no result: live turns keep their
    // spinner, finished transcripts do not. No stream status anywhere
    // (legacy rows) keeps "no result means pending".
    let any_status = msgs.iter().any(|m| !m.stream_status.is_empty());
    let session_live = msgs
        .iter()
        .any(|m| m.stream_status == STREAM_STREAMING || m.stream_status == STREAM_PENDING);
    for m in msgs {
        if m.role == "user" && user_message_is_chat_prompt(m) {
            if let Some(it) = flatten_user_message(m) {
                nodes.push(ChatNode {
                    item: it,
                    parent: String::new(),
                });
            }
            if m.duration_ms > 0 {
                let p = provider_for(&m.provider);
                nodes.push(ChatNode {
                    item: turn_chat_item(p.as_deref(), m, model, flags),
                    parent: String::new(),
                });
            }
            continue;
        }
        for b in m.blocks() {
            let parent = b.parent_tool_use_id();
            match b.r#type.as_str() {
                BLOCK_TEXT => {
                    if let Some(p) = b.as_text()
                        && !p.text.trim().is_empty()
                    {
                        let shown = if p.display_text.is_empty() {
                            p.text
                        } else {
                            p.display_text
                        };
                        nodes.push(ChatNode {
                            item: ChatItem {
                                role: m.role.clone(),
                                kind: "text".into(),
                                text: shown,
                                ts: chat_item_ts(m, Some(b)),
                                ..ChatItem::default()
                            },
                            parent,
                        });
                    }
                }
                BLOCK_THINKING => {
                    if let Some(p) = b.as_thinking()
                        && (!p.thinking.trim().is_empty() || p.estimated_tokens > 0)
                    {
                        nodes.push(ChatNode {
                            item: ChatItem {
                                role: "assistant".into(),
                                kind: "thinking".into(),
                                text: p.thinking,
                                tokens: p.estimated_tokens,
                                ts: chat_item_ts(m, Some(b)),
                                ..ChatItem::default()
                            },
                            parent,
                        });
                    }
                }
                BLOCK_COMPACT => {
                    if let Some(p) = b.as_compact() {
                        nodes.push(ChatNode {
                            item: ChatItem {
                                role: "assistant".into(),
                                kind: "compact".into(),
                                text: p.summary,
                                tokens: p.pre_tokens,
                                description: p.trigger,
                                ts: chat_item_ts(m, Some(b)),
                                ..ChatItem::default()
                            },
                            parent,
                        });
                    }
                }
                BLOCK_FALLBACK => {
                    if let Some(p) = b.as_fallback()
                        && !p.to.trim().is_empty()
                    {
                        // Fallback rows are written by the default provider;
                        // legacy ones carry no provider id.
                        let prov = provider_for(&m.provider)
                            .or_else(|| provider_for(&parse_model_ref("").provider));
                        let mut to = model_label(prov.as_deref(), &p.to);
                        if to.is_empty() {
                            to = p.to.clone();
                        }
                        let mut from = model_label(prov.as_deref(), &p.from);
                        if from.is_empty() {
                            from = p.from.clone();
                        }
                        nodes.push(ChatNode {
                            item: ChatItem {
                                role: "assistant".into(),
                                kind: "fallback".into(),
                                text: to,
                                description: from,
                                ts: chat_item_ts(m, Some(b)),
                                ..ChatItem::default()
                            },
                            parent,
                        });
                    }
                }
                BLOCK_TOOL_USE => {
                    if let Some(p) = b.as_tool_use() {
                        // Blocks hold the provider's raw call; the item carries
                        // the common vocabulary, derived here so a mapping fix
                        // re-renders history.
                        let prov = provider_for(&m.provider);
                        let d = display_tool(prov.as_deref(), &p.name, &p.input);
                        nodes.push(ChatNode {
                            item: ChatItem {
                                role: "assistant".into(),
                                kind: "tool".into(),
                                tool_input: summarize_tool_input(&d.name, &d.input),
                                description: tool_description(&d.name, &d.input),
                                input: chat_tool_input(&d.name, &d.input),
                                tool: d.name,
                                tool_id: p.provider_tool_use_id.clone(),
                                result_pending: !any_status || session_live,
                                ts: chat_item_ts(m, Some(b)),
                                ..ChatItem::default()
                            },
                            parent,
                        });
                        tool_idx.insert(p.id.clone(), nodes.len() - 1);
                        tool_raw.insert(
                            p.id,
                            RawToolRef {
                                provider: m.provider.clone(),
                                name: p.name,
                            },
                        );
                    }
                }
                BLOCK_TOOL_RESULT => {
                    if let Some(p) = b.as_tool_result()
                        && let Some(&i) = tool_idx.get(&p.tool_use_id)
                    {
                        let r = &tool_raw[&p.tool_use_id];
                        let prov = provider_for(&r.provider);
                        let shown = display_tool_result(prov.as_deref(), &r.name, &p.result);
                        let item = &mut nodes[i].item;
                        item.result = chat_tool_result(&item.tool, &shown);
                        item.result_error = p.is_error;
                        item.result_truncated = p.truncated;
                        item.result_pending = false;
                    }
                }
                _ => {}
            }
        }
        // A turn ends where the provider stamped its duration.
        if m.duration_ms > 0 {
            let p = provider_for(&m.provider);
            nodes.push(ChatNode {
                item: turn_chat_item(p.as_deref(), m, model, flags),
                parent: String::new(),
            });
        }
    }
    assemble_chat_items(nodes)
}

/// Hangs nested subagent rows under their Task parent and drops orphans
/// whose parent was never seen.
fn assemble_chat_items(nodes: Vec<ChatNode>) -> Vec<ChatItem> {
    let mut children: HashMap<String, Vec<ChatItem>> = HashMap::new();
    let mut top = Vec::new();
    for n in nodes {
        if n.parent.is_empty() {
            top.push(n.item);
        } else {
            children.entry(n.parent).or_default().push(n.item);
        }
    }
    fn attach(
        mut items: Vec<ChatItem>,
        children: &mut HashMap<String, Vec<ChatItem>>,
    ) -> Vec<ChatItem> {
        for it in &mut items {
            if it.kind != "tool" || it.tool_id.is_empty() {
                continue;
            }
            if let Some(ch) = children.remove(&it.tool_id) {
                it.children = attach(ch, children);
            }
        }
        items
    }
    attach(top, &mut children)
}

/// The footer of a finished turn: wall time plus context usage so the
/// composer meter reloads from the transcript.
fn turn_chat_item(p: Option<&dyn Provider>, m: &Message, model: &str, flags: &Flags) -> ChatItem {
    ChatItem {
        role: "assistant".into(),
        kind: "turn".into(),
        duration_ms: m.duration_ms,
        message_id: m.id.clone(),
        input_tokens: m.input_tokens,
        cache_creation_tokens: m.cache_creation_tokens,
        cache_read_tokens: m.cache_read_tokens,
        context_window: context_window_for_message(p, m, model, flags),
        ts: chat_item_ts(m, None),
        ..ChatItem::default()
    }
}

/// The meter limit for a finished turn: the provider's window for the
/// message's model (or the session's) and the dock flags when a model or a
/// context flag is known, else the stored window, else the provider default.
pub fn context_window_for_message(
    p: Option<&dyn Provider>,
    m: &Message,
    session_model: &str,
    flags: &Flags,
) -> i64 {
    let used = m.input_tokens + m.cache_read_tokens + m.cache_creation_tokens;
    if used <= 0 && m.context_window <= 0 {
        return 0;
    }
    let Some(p) = p else {
        return m.context_window.max(0);
    };
    let model = if m.model_id.is_empty() {
        session_model
    } else {
        &m.model_id
    };
    let bare = parse_model_ref(model).id;
    if !model.is_empty() || flags.contains_key("context") {
        let w = p.context_window(&bare, flags);
        if w > 0 {
            return w;
        }
    }
    if m.context_window > 0 {
        return m.context_window;
    }
    p.context_window("", flags)
}

/// Turn footers dropped, leaving conversation content (tests).
pub fn content_items(items: &[ChatItem]) -> Vec<ChatItem> {
    items.iter().filter(|i| i.kind != "turn").cloned().collect()
}

impl Engine {
    /// The full transcript of one coding-agent chat tab (PA, PA2, …; empty
    /// means PA).
    pub async fn workspace_chat_items(&self, workspace_id: &str, code: &str) -> Vec<ChatItem> {
        self.session_chat_items(workspace_id, &normalize_pair_chat_code(code))
            .await
    }

    /// One (workspace, agent code) transcript flattened for the UI, with
    /// "this turn changed files" marks from the checkpoints. Empty when the
    /// agent has not run.
    pub async fn session_chat_items(&self, workspace_id: &str, agent_code: &str) -> Vec<ChatItem> {
        let Some(sess) = self.store.session_for_loop_agent(workspace_id, agent_code) else {
            return Vec::new();
        };
        let Ok(Some(msgs)) = self.store.session_transcript(&sess.id) else {
            return Vec::new();
        };
        let cfg = AgentConfig::parse(&sess.config_json);
        let mut items = flatten_transcript(&self.providers, &msgs, &cfg.model, &cfg.flags);
        if let Some(ws) = self.store.workspace(workspace_id) {
            self.mark_checkpoints(&ws, &msgs, &mut items).await;
        }
        items
    }

    fn tab_transcript(&self, workspace_id: &str, code: &str) -> Vec<Message> {
        self.store
            .session_for_loop_agent(workspace_id, code)
            .and_then(|s| self.store.session_transcript(&s.id).ok().flatten())
            .unwrap_or_default()
    }

    /// The newest human chat prompt on this tab (stored wire + images). Retry
    /// replays it instead of the bubble's display text. Stops at the newest
    /// prompt rather than falling through to an older one.
    pub fn last_chat_user_prompt(&self, workspace_id: &str, code: &str) -> Option<LastChatPrompt> {
        let msgs = self.tab_transcript(workspace_id, code);
        let m = msgs.iter().rev().find(|m| {
            m.role == "user"
                && user_message_is_chat_prompt(m)
                && !user_message_is_mode_switch_continue(m)
        })?;
        reconstruct_chat_prompt(&self.data_dir, m)
    }

    /// The oldest human chat prompt on this tab.
    pub fn first_chat_user_prompt(&self, workspace_id: &str, code: &str) -> Option<LastChatPrompt> {
        let msgs = self.tab_transcript(workspace_id, code);
        let m = msgs.iter().find(|m| {
            m.role == "user"
                && user_message_is_chat_prompt(m)
                && !user_message_is_mode_switch_continue(m)
        })?;
        reconstruct_chat_prompt(&self.data_dir, m)
    }

    /// The loop's opening paste set: ids recorded at engage/create, else the
    /// first chat turn's images (legacy).
    pub fn loop_goal_images(&self, workspace_id: &str, code: &str) -> Vec<UserImage> {
        if let Some(ids) = self.chat_loop_goal_images(workspace_id, code) {
            return user_images_by_ids(&self.data_dir, workspace_id, &ids);
        }
        self.first_chat_user_prompt(workspace_id, code)
            .map(|p| p.imgs)
            .unwrap_or_default()
    }

    /// The recorded goal image ids (`None` = never recorded).
    pub fn chat_loop_goal_images(&self, workspace_id: &str, code: &str) -> Option<Vec<String>> {
        if code.trim().is_empty() {
            return None;
        }
        let sess = self.store.session_for_loop_agent(workspace_id, code)?;
        serde_json::from_str::<AgentConfig>(&sess.config_json)
            .ok()?
            .goal_images
    }
}

#[cfg(test)]
mod tests;
