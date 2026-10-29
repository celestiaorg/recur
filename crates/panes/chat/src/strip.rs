//! The agent-panel tab strip across workspace remounts: the
//! warm tier that paints the strip at once on A→B→A after a hot shell was evicted.

use gpui_kit::{App, Global};
use recur_domain::WorkspaceChatTab;

use recur_view_widgets::byte_lru::{
    ByteLru, CHROME_CACHE_MAX_BYTES, OBJ_HDR, SCALAR_BYTES, array_bytes, string_bytes,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatStripSnapshot {
    pub tabs: Vec<WorkspaceChatTab>,
    pub closed: Vec<WorkspaceChatTab>,
    pub active_code: String,
    /// The strip's scroll offset when it had live layout; `None` when it was missing or parked
    /// (a fabricated 0 would pin the restore and suppress the reveal chase).
    pub tab_scroll_left: Option<f32>,
}

fn tab_bytes(t: &WorkspaceChatTab) -> usize {
    OBJ_HDR + string_bytes(&t.code) + string_bytes(&t.title) + SCALAR_BYTES * 3
}

fn strip_bytes(s: &ChatStripSnapshot) -> usize {
    OBJ_HDR
        + array_bytes(s.tabs.iter().map(tab_bytes))
        + array_bytes(s.closed.iter().map(tab_bytes))
        + string_bytes(&s.active_code)
        + SCALAR_BYTES
}

pub struct ChatStripCache {
    cache: ByteLru<ChatStripSnapshot>,
}

impl Global for ChatStripCache {}

impl Default for ChatStripCache {
    fn default() -> Self {
        ChatStripCache {
            cache: ByteLru::new(CHROME_CACHE_MAX_BYTES, strip_bytes),
        }
    }
}

impl ChatStripCache {
    pub fn global(cx: &mut App) -> &mut ChatStripCache {
        cx.default_global::<ChatStripCache>()
    }

    /// `getChatStrip` (promotes).
    pub fn get(&mut self, workspace_id: &str) -> Option<ChatStripSnapshot> {
        if workspace_id.is_empty() {
            return None;
        }
        self.cache.get(workspace_id).cloned()
    }

    /// `putChatStrip`: empty tabs never clobber; omitted fields keep the previous values.
    pub fn put(&mut self, workspace_id: &str, snap: ChatStripSnapshot) {
        if workspace_id.is_empty() || snap.tabs.is_empty() {
            return;
        }
        let prev = self.cache.peek(workspace_id).cloned().unwrap_or_default();
        self.cache.set(
            workspace_id,
            ChatStripSnapshot {
                closed: if snap.closed.is_empty() {
                    prev.closed
                } else {
                    snap.closed
                },
                active_code: if snap.active_code.is_empty() {
                    prev.active_code
                } else {
                    snap.active_code
                },
                tab_scroll_left: snap.tab_scroll_left.or(prev.tab_scroll_left),
                tabs: snap.tabs,
            },
        );
    }

    /// `pruneChatStrips`.
    pub fn prune<'a>(&mut self, live: impl IntoIterator<Item = &'a str>) {
        let live: Vec<&str> = live.into_iter().collect();
        self.cache.prune(|k, _| live.contains(&k));
    }

    pub fn reset(&mut self) {
        self.cache.clear();
        self.cache.set_max_bytes(CHROME_CACHE_MAX_BYTES);
    }

    pub fn set_max_bytes_for_tests(&mut self, n: usize) {
        self.cache.set_max_bytes(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(code: &str, title: &str) -> WorkspaceChatTab {
        WorkspaceChatTab {
            code: code.into(),
            title: title.into(),
            ..Default::default()
        }
    }

    fn snap(tabs: Vec<WorkspaceChatTab>, active: &str, scroll: Option<f32>) -> ChatStripSnapshot {
        ChatStripSnapshot {
            tabs,
            closed: Vec::new(),
            active_code: active.into(),
            tab_scroll_left: scroll,
        }
    }

    #[test]
    fn round_trips_tabs_active_and_scroll() {
        let mut c = ChatStripCache::default();
        c.put(
            "ws",
            snap(vec![tab("PA", "New chat"), tab("MA", "Loop")], "MA", None),
        );
        let hit = c.get("ws").expect("hit");
        assert_eq!(hit.active_code, "MA");
        assert_eq!(
            hit.tabs.iter().map(|t| t.code.as_str()).collect::<Vec<_>>(),
            ["PA", "MA"]
        );
        c.put("ws", snap(vec![tab("PA", "PA")], "PA", Some(64.)));
        assert_eq!(c.get("ws").expect("hit").tab_scroll_left, Some(64.));
        c.put(
            "ws",
            snap(vec![tab("PA", "PA"), tab("MA", "MA")], "MA", None),
        );
        assert_eq!(c.get("ws").expect("hit").tab_scroll_left, Some(64.));
        c.put("ws", snap(vec![tab("PA", "PA")], "PA", Some(0.)));
        assert_eq!(c.get("ws").expect("hit").tab_scroll_left, Some(0.));
    }

    #[test]
    fn empty_tabs_prune_and_evict() {
        let mut c = ChatStripCache::default();
        c.put("ws", snap(vec![tab("PA", "PA")], "PA", None));
        c.put("ws", snap(vec![], "MA", None));
        assert_eq!(c.get("ws").expect("hit").tabs.len(), 1);
        c.put("a", snap(vec![tab("PA", "PA")], "", None));
        c.put("b", snap(vec![tab("PA", "PA")], "", None));
        c.prune(["a"]);
        assert!(c.get("a").is_some() && c.get("b").is_none());
        let mut c = ChatStripCache::default();
        c.set_max_bytes_for_tests(1);
        for k in ["a", "b", "c"] {
            c.put(k, snap(vec![tab("PA", "PA")], "", None));
        }
        assert!(c.get("a").is_none());
        assert_eq!(c.get("c").expect("hit").tabs.len(), 1);
    }
}
