//! Agent transcripts and dock state across workspace remounts
//! (`lib/caches/chat-heap-cache.ts`): messages, composer drafts, queued unsent turns, dock
//! pickers, Simplify, the turn clock and the context ring, keyed `workspaceId\tcode`, byte
//! capped per workspace. A GPUI global ([`ChatHeapCache`]).

use std::collections::BTreeMap;

use gpui_kit::{App, Global};
use recur_agent::context_usage::ContextUsage;
use recur_engine::view::chat::{AgentMsg, ChatAttachmentDraft, ChatLog};
use recur_view_core::models::FlagMap;
use recur_view_core::modes::ChatMode;

use crate::byte_lru::{
    ByteLru, OBJ_HDR, SCALAR_BYTES, WARM_CACHE_MAX_BYTES, array_bytes, string_bytes,
};

/// A queued (or held) send, as the workspace view keeps it (`PendingChatSend`).
#[derive(Clone, Debug, PartialEq)]
pub struct PendingChatSend {
    pub id: String,
    /// The wire text with attachment markers.
    pub marked: String,
    /// The bubble text.
    pub plain: String,
    pub drafts: Vec<ChatAttachmentDraft>,
    pub mode: ChatMode,
    pub model: String,
    pub flags: FlagMap,
    /// The session display_text; empty lets the engine derive it.
    pub display_text: String,
    /// Prompt-action turns never merge with ordinary queued sends.
    pub prompt_action: bool,
    /// Send order, for rejected-steer restore.
    pub seq: i64,
    /// Do not resume this tab's prior provider session.
    pub fresh_session: bool,
}

impl Default for PendingChatSend {
    fn default() -> Self {
        PendingChatSend {
            id: String::new(),
            marked: String::new(),
            plain: String::new(),
            drafts: Vec::new(),
            mode: ChatMode::Agent,
            model: String::new(),
            flags: FlagMap::new(),
            display_text: String::new(),
            prompt_action: false,
            seq: 0,
            fresh_session: false,
        }
    }
}

/// A tab's dock selection (`ChatPickerSnap`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatPickerSnap {
    pub model: String,
    pub flags: FlagMap,
    pub mode: ChatMode,
}

/// One workspace's warm chat state (`ChatHeapSnapshot`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatHeapSnapshot {
    pub chat: BTreeMap<String, Vec<AgentMsg>>,
    pub usage: BTreeMap<String, ContextUsage>,
    pub input: BTreeMap<String, String>,
    pub attachments: BTreeMap<String, Vec<ChatAttachmentDraft>>,
    /// The wall-clock start of an in-flight turn: elapsed must not reset on remount.
    pub turn_started_at: BTreeMap<String, i64>,
    pub stream_seq: BTreeMap<String, i64>,
    pub turn_got_text: BTreeMap<String, bool>,
    pub pickers: BTreeMap<String, ChatPickerSnap>,
    pub pending: BTreeMap<String, Vec<PendingChatSend>>,
    pub hold_pending: BTreeMap<String, bool>,
    pub simplify: BTreeMap<String, bool>,
}

fn s(x: &str) -> usize {
    if x.is_empty() { 0 } else { string_bytes(x) }
}

/// The budgeted size of one transcript row (its set fields, as JSON).
pub fn msg_bytes(m: &AgentMsg) -> usize {
    let mut n = OBJ_HDR
        + s(&m.body)
        + s(&m.tool)
        + s(&m.tool_input)
        + s(&m.tool_id)
        + s(&m.description)
        + s(&m.input)
        + s(&m.result)
        + s(&m.message_id)
        + s(&m.pending_id)
        + s(&m.note_label)
        + s(&m.provider_uuid)
        + SCALAR_BYTES * 4;
    for a in &m.attachments {
        n += OBJ_HDR + s(&a.id) + s(&a.name) + s(&a.mime) + s(&a.url) + s(&a.data_url);
    }
    n += array_bytes(m.children.iter().map(msg_bytes));
    if let Some(log) = &m.log {
        n += match log.as_ref() {
            ChatLog::Fact { text, data, .. } => OBJ_HDR + s(text) + s(data),
            ChatLog::Session(sess) => {
                OBJ_HDR + array_bytes(sess.messages.iter().map(msg_bytes))
            }
            ChatLog::Sessions { sessions, .. } => array_bytes(
                sessions
                    .iter()
                    .map(|x| OBJ_HDR + array_bytes(x.messages.iter().map(msg_bytes))),
            ),
        };
    }
    n
}

fn draft_bytes(d: &ChatAttachmentDraft) -> usize {
    OBJ_HDR + s(&d.id) + s(&d.name) + s(&d.mime) + s(&d.data_url)
}

fn snapshot_bytes(h: &ChatHeapSnapshot) -> usize {
    let key = |k: &String| string_bytes(k);
    let mut n = OBJ_HDR;
    for (k, v) in &h.chat {
        n += key(k) + array_bytes(v.iter().map(msg_bytes));
    }
    n += h.usage.keys().map(|k| key(k) + OBJ_HDR + SCALAR_BYTES * 6).sum::<usize>();
    n += h.input.iter().map(|(k, v)| key(k) + string_bytes(v)).sum::<usize>();
    for (k, v) in &h.attachments {
        n += key(k) + array_bytes(v.iter().map(draft_bytes));
    }
    for (k, v) in &h.pending {
        n += key(k)
            + array_bytes(v.iter().map(|p| {
                OBJ_HDR
                    + s(&p.id)
                    + s(&p.marked)
                    + s(&p.plain)
                    + s(&p.model)
                    + s(&p.display_text)
                    + array_bytes(p.drafts.iter().map(draft_bytes))
            }));
    }
    let scalars = h.turn_started_at.len()
        + h.stream_seq.len()
        + h.turn_got_text.len()
        + h.hold_pending.len()
        + h.simplify.len();
    n += scalars * (SCALAR_BYTES + 32);
    n += h
        .pickers
        .iter()
        .map(|(k, p)| key(k) + OBJ_HDR + string_bytes(&p.model) + p.flags.len() * 48)
        .sum::<usize>();
    n
}

fn prefix(workspace_id: &str) -> String {
    format!("{workspace_id}\t")
}

fn scoped<T: Clone>(workspace_id: &str, rec: &BTreeMap<String, T>) -> BTreeMap<String, T> {
    let p = prefix(workspace_id);
    rec.iter()
        .filter(|(k, _)| k.starts_with(&p))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

pub struct ChatHeapCache {
    cache: ByteLru<ChatHeapSnapshot>,
}

impl Global for ChatHeapCache {}

impl Default for ChatHeapCache {
    fn default() -> Self {
        ChatHeapCache {
            cache: ByteLru::new(WARM_CACHE_MAX_BYTES, snapshot_bytes),
        }
    }
}

impl ChatHeapCache {
    pub fn global(cx: &mut App) -> &mut ChatHeapCache {
        cx.default_global::<ChatHeapCache>()
    }

    /// `getChatHeap` (promotes).
    pub fn get(&mut self, workspace_id: &str) -> Option<ChatHeapSnapshot> {
        if workspace_id.is_empty() {
            return None;
        }
        self.cache.get(workspace_id).cloned()
    }

    /// `putChatHeap`: keys outside the workspace are dropped; a picker-only put (mode change,
    /// no messages yet) keeps the previous transcript and drafts.
    pub fn put(&mut self, workspace_id: &str, snap: &ChatHeapSnapshot) {
        if workspace_id.is_empty() {
            return;
        }
        let chat = scoped(workspace_id, &snap.chat);
        let pickers: BTreeMap<String, ChatPickerSnap> = scoped(workspace_id, &snap.pickers)
            .into_iter()
            .filter(|(_, p)| !p.model.is_empty())
            .collect();
        let pending = scoped(workspace_id, &snap.pending);
        let hold_pending = scoped(workspace_id, &snap.hold_pending);
        let simplify = scoped(workspace_id, &snap.simplify);
        if chat.is_empty() && pickers.is_empty() {
            return;
        }
        if chat.is_empty() {
            let prev = self.cache.peek(workspace_id).cloned().unwrap_or_default();
            self.cache.set(
                workspace_id,
                ChatHeapSnapshot {
                    pickers,
                    simplify: if simplify.is_empty() {
                        prev.simplify.clone()
                    } else {
                        simplify
                    },
                    ..prev
                },
            );
            return;
        }
        let usage = scoped(workspace_id, &snap.usage)
            .into_iter()
            .filter(|(_, u)| u.ready())
            .collect();
        self.cache.set(
            workspace_id,
            ChatHeapSnapshot {
                chat,
                usage,
                input: scoped(workspace_id, &snap.input),
                attachments: scoped(workspace_id, &snap.attachments),
                turn_started_at: scoped(workspace_id, &snap.turn_started_at),
                stream_seq: scoped(workspace_id, &snap.stream_seq),
                turn_got_text: scoped(workspace_id, &snap.turn_got_text),
                pickers,
                pending,
                hold_pending,
                simplify,
            },
        );
    }

    /// `takeChatHeapDrafts`: composer drafts off a workspace heap so a hoist can remap them.
    pub fn take_drafts(
        &mut self,
        workspace_id: &str,
    ) -> (
        BTreeMap<String, String>,
        BTreeMap<String, Vec<ChatAttachmentDraft>>,
    ) {
        let Some(prev) = self.cache.peek(workspace_id).cloned() else {
            return Default::default();
        };
        if prev.input.is_empty() && prev.attachments.is_empty() {
            return Default::default();
        }
        let out = (prev.input.clone(), prev.attachments.clone());
        self.cache.set(
            workspace_id,
            ChatHeapSnapshot {
                input: BTreeMap::new(),
                attachments: BTreeMap::new(),
                ..prev
            },
        );
        out
    }

    /// `dropChatHeapKey`: one chat key out of a workspace heap.
    pub fn drop_key(&mut self, workspace_id: &str, key: &str) {
        if workspace_id.is_empty() || key.is_empty() {
            return;
        }
        let Some(mut prev) = self.cache.peek(workspace_id).cloned() else {
            return;
        };
        prev.chat.remove(key);
        prev.usage.remove(key);
        prev.input.remove(key);
        prev.attachments.remove(key);
        prev.turn_started_at.remove(key);
        prev.stream_seq.remove(key);
        prev.turn_got_text.remove(key);
        prev.pickers.remove(key);
        prev.pending.remove(key);
        prev.hold_pending.remove(key);
        prev.simplify.remove(key);
        self.cache.set(workspace_id, prev);
    }

    /// `pruneChatHeaps`.
    pub fn prune<'a>(&mut self, live: impl IntoIterator<Item = &'a str>) {
        let live: Vec<&str> = live.into_iter().collect();
        self.cache.prune(|k, _| live.contains(&k));
    }

    pub fn reset(&mut self) {
        self.cache.clear();
        self.cache.set_max_bytes(WARM_CACHE_MAX_BYTES);
    }

    pub fn set_max_bytes_for_tests(&mut self, n: usize) {
        self.cache.set_max_bytes(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(b: &str) -> AgentMsg {
        AgentMsg {
            body: b.into(),
            ..Default::default()
        }
    }

    fn map<T: Clone>(pairs: &[(&str, T)]) -> BTreeMap<String, T> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    fn usage(input: i64, window: i64, compacted: bool) -> ContextUsage {
        ContextUsage {
            input_tokens: input,
            context_window: window,
            compacted,
            ..Default::default()
        }
    }

    fn draft(id: &str) -> ChatAttachmentDraft {
        ChatAttachmentDraft {
            id: id.into(),
            ..Default::default()
        }
    }

    fn pending(id: &str) -> PendingChatSend {
        PendingChatSend {
            id: id.into(),
            ..Default::default()
        }
    }

    fn picker(model: &str, mode: ChatMode) -> ChatPickerSnap {
        ChatPickerSnap {
            model: model.into(),
            flags: FlagMap::new(),
            mode,
        }
    }

    // chat-heap-cache.test.ts
    #[test]
    fn round_trips_one_workspace() {
        let mut c = ChatHeapCache::default();
        c.put(
            "ws",
            &ChatHeapSnapshot {
                chat: map(&[("ws\tPA", vec![body("hi")])]),
                usage: map(&[("ws\tPA", usage(1, 10, false))]),
                input: map(&[("ws\tPA", "draft".to_string())]),
                attachments: map(&[("ws\tPA", vec![draft("a1")])]),
                ..Default::default()
            },
        );
        let hit = c.get("ws").expect("hit");
        assert_eq!(hit.chat["ws\tPA"], vec![body("hi")]);
        assert_eq!(hit.usage["ws\tPA"], usage(1, 10, false));
        assert_eq!(hit.input["ws\tPA"], "draft");
        assert_eq!(hit.attachments["ws\tPA"], vec![draft("a1")]);
    }

    #[test]
    fn post_compact_meter_and_turn_clock() {
        let mut c = ChatHeapCache::default();
        c.put(
            "ws",
            &ChatHeapSnapshot {
                chat: map(&[("ws\tPA", vec![body("hi")])]),
                usage: map(&[("ws\tPA", usage(0, 200_000, true))]),
                turn_started_at: map(&[("ws\tPA", 1_700_000_000_000)]),
                stream_seq: map(&[("ws\tPA:text", 12)]),
                turn_got_text: map(&[("ws\tPA", true)]),
                ..Default::default()
            },
        );
        let hit = c.get("ws").expect("hit");
        assert_eq!(hit.usage["ws\tPA"], usage(0, 200_000, true));
        assert_eq!(hit.turn_started_at["ws\tPA"], 1_700_000_000_000);
        assert_eq!(hit.stream_seq["ws\tPA:text"], 12);
        assert!(hit.turn_got_text["ws\tPA"]);
    }

    #[test]
    fn scoping_empty_puts_prune_and_eviction() {
        let mut c = ChatHeapCache::default();
        c.put(
            "ws",
            &ChatHeapSnapshot {
                chat: map(&[("ws\tPA", vec![body("a")]), ("other\tPA", vec![body("b")])]),
                ..Default::default()
            },
        );
        assert_eq!(c.get("ws").expect("hit").chat.keys().collect::<Vec<_>>(), ["ws\tPA"]);
        c.put("ws", &ChatHeapSnapshot::default());
        assert_eq!(c.get("ws").expect("hit").chat["ws\tPA"], vec![body("a")]);
        c.put(
            "b",
            &ChatHeapSnapshot {
                chat: map(&[("b\tPA", vec![body("b")])]),
                ..Default::default()
            },
        );
        c.prune(["ws"]);
        assert!(c.get("ws").is_some());
        assert!(c.get("b").is_none());
        let mut c = ChatHeapCache::default();
        c.set_max_bytes_for_tests(1);
        for k in ["a", "b", "c"] {
            c.put(
                k,
                &ChatHeapSnapshot {
                    chat: map(&[(&format!("{k}\tPA"), vec![body(k)])]),
                    ..Default::default()
                },
            );
        }
        assert!(c.get("a").is_none());
        assert_eq!(c.get("c").expect("hit").chat["c\tPA"], vec![body("c")]);
    }

    #[test]
    fn pending_pickers_and_simplify() {
        let mut c = ChatHeapCache::default();
        c.put(
            "ws",
            &ChatHeapSnapshot {
                chat: map(&[("ws\tPA", vec![body("fix it")])]),
                pending: map(&[("ws\tPA", vec![pending("p1")])]),
                hold_pending: map(&[("ws\tPA", true)]),
                simplify: map(&[("ws\tPA", false)]),
                ..Default::default()
            },
        );
        c.put(
            "ws",
            &ChatHeapSnapshot {
                pickers: map(&[("ws\tPA", picker("claude-code/opus", ChatMode::Agent))]),
                ..Default::default()
            },
        );
        let hit = c.get("ws").expect("hit");
        assert_eq!(hit.pending["ws\tPA"], vec![pending("p1")]);
        assert!(hit.hold_pending["ws\tPA"]);
        assert!(!hit.simplify["ws\tPA"]);
        assert_eq!(hit.pickers["ws\tPA"].model, "claude-code/opus");
    }

    #[test]
    fn pickers_scope_skip_stubs_and_keep_transcript() {
        let mut c = ChatHeapCache::default();
        c.put(
            "ws",
            &ChatHeapSnapshot {
                chat: map(&[("ws\tPA", vec![body("a")])]),
                input: map(&[("ws\tPA", "draft".to_string())]),
                usage: map(&[("ws\tPA", usage(8, 100, false))]),
                pickers: map(&[
                    ("ws\tPA", picker("claude-code/opus", ChatMode::Agent)),
                    ("other\tPA", picker("cursor/gpt", ChatMode::Ask)),
                ]),
                ..Default::default()
            },
        );
        assert_eq!(c.get("ws").expect("hit").pickers.keys().collect::<Vec<_>>(), ["ws\tPA"]);
        c.put(
            "ws",
            &ChatHeapSnapshot {
                pickers: map(&[("ws\tPA", picker("claude-code/opus", ChatMode::Review))]),
                ..Default::default()
            },
        );
        let hit = c.get("ws").expect("hit");
        assert_eq!(hit.chat["ws\tPA"], vec![body("a")]);
        assert_eq!(hit.input["ws\tPA"], "draft");
        assert_eq!(hit.usage["ws\tPA"].input_tokens, 8);
        assert_eq!(hit.pickers["ws\tPA"].mode, ChatMode::Review);
        let mut c = ChatHeapCache::default();
        c.put(
            "ws",
            &ChatHeapSnapshot {
                pickers: map(&[("ws\tPA", picker("claude-code/opus", ChatMode::Ask))]),
                ..Default::default()
            },
        );
        assert_eq!(c.get("ws").expect("hit").pickers["ws\tPA"].model, "claude-code/opus");
        assert!(c.get("ws").expect("hit").chat.is_empty());
        let mut c = ChatHeapCache::default();
        c.put(
            "ws",
            &ChatHeapSnapshot {
                chat: map(&[("ws\tPA", vec![body("a")])]),
                pickers: map(&[("ws\tPA", picker("", ChatMode::Agent))]),
                usage: map(&[("ws\tPA", usage(0, 100, false))]),
                ..Default::default()
            },
        );
        assert!(c.get("ws").expect("hit").pickers.is_empty());
        assert!(c.get("ws").expect("hit").usage.is_empty());
    }

    #[test]
    fn drop_key_and_take_drafts() {
        let mut c = ChatHeapCache::default();
        c.put(
            "ws",
            &ChatHeapSnapshot {
                chat: map(&[("ws\tPA", vec![body("keep")]), ("ws\tPA2", vec![body("gone")])]),
                input: map(&[("ws\tPA", "a".to_string()), ("ws\tPA2", "b".to_string())]),
                attachments: map(&[("ws\tPA2", vec![draft("x")])]),
                pending: map(&[("ws\tPA", vec![pending("keep")]), ("ws\tPA2", vec![pending("gone")])]),
                hold_pending: map(&[("ws\tPA2", true)]),
                simplify: map(&[("ws\tPA", false), ("ws\tPA2", true)]),
                ..Default::default()
            },
        );
        c.drop_key("ws", "ws\tPA2");
        let hit = c.get("ws").expect("hit");
        assert!(!hit.chat.contains_key("ws\tPA2") && !hit.input.contains_key("ws\tPA2"));
        assert!(!hit.attachments.contains_key("ws\tPA2") && !hit.pending.contains_key("ws\tPA2"));
        assert!(!hit.hold_pending.contains_key("ws\tPA2") && !hit.simplify.contains_key("ws\tPA2"));
        assert_eq!(hit.chat["ws\tPA"], vec![body("keep")]);
        assert_eq!(hit.pending["ws\tPA"], vec![pending("keep")]);
        assert!(!hit.simplify["ws\tPA"]);
        let mut c = ChatHeapCache::default();
        c.put(
            "ws",
            &ChatHeapSnapshot {
                chat: map(&[("ws\tPA", vec![body("keep")])]),
                input: map(&[("ws\tPA", "draft".to_string())]),
                attachments: map(&[("ws\tPA", vec![draft("a1")])]),
                ..Default::default()
            },
        );
        let (input, atts) = c.take_drafts("ws");
        assert_eq!(input, map(&[("ws\tPA", "draft".to_string())]));
        assert_eq!(atts, map(&[("ws\tPA", vec![draft("a1")])]));
        let hit = c.get("ws").expect("hit");
        assert_eq!(hit.chat["ws\tPA"], vec![body("keep")]);
        assert!(hit.input.is_empty() && hit.attachments.is_empty());
        let (input, atts) = c.take_drafts("ws");
        assert!(input.is_empty() && atts.is_empty());
    }
}
