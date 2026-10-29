//! Per-file undo history that outlives an editor (`cm-editor-history.ts`):
//! history and selection serialized under a `scope\0path` key, kept within
//! a 20 MiB byte budget, newest first.

use std::collections::VecDeque;

use crate::state::{EditorState, StateConfig};

/// The closed-file undo ceiling (VS Code's 20 MiB).
pub const EDITOR_HISTORY_MAX_BYTES: usize = 20 * 1024 * 1024;
/// Documents longer than this (UTF-16 units) keep live-only undo.
pub const EDITOR_HISTORY_DOC_CAP: usize = EDITOR_HISTORY_MAX_BYTES / 4;
/// Undo events kept per file.
pub const EDITOR_HISTORY_MIN_DEPTH: usize = 250;

pub fn editor_history_key(scope: &str, path: &str) -> String {
    format!("{scope}\0{path}")
}

#[derive(Clone, Debug)]
struct Snap {
    json: serde_json::Value,
    doc: String,
    scroll_top: f32,
    scroll_left: f32,
    bytes: usize,
    depth: usize,
}

/// A rebuilt state and the scroll it was captured at.
pub struct RestoredEditor {
    pub state: EditorState,
    pub scroll_top: f32,
    pub scroll_left: f32,
}

/// The app's history cache (the module-level map of `cm-editor-history.ts`).
#[derive(Default)]
pub struct EditorHistory(pub EditorHistoryCache);

impl gpui_kit::Global for EditorHistory {}

/// The history cache. One per app, like the module-level map.
pub struct EditorHistoryCache {
    entries: VecDeque<(String, Snap)>,
    total_bytes: usize,
    max_bytes: usize,
}

impl Default for EditorHistoryCache {
    fn default() -> Self {
        EditorHistoryCache {
            entries: VecDeque::new(),
            total_bytes: 0,
            max_bytes: EDITOR_HISTORY_MAX_BYTES,
        }
    }
}

impl EditorHistoryCache {
    pub fn with_max_bytes(max_bytes: usize) -> EditorHistoryCache {
        EditorHistoryCache {
            max_bytes,
            ..Default::default()
        }
    }

    fn drop_key(&mut self, key: &str) -> Option<Snap> {
        let i = self.entries.iter().position(|(k, _)| k == key)?;
        let (_, snap) = self.entries.remove(i)?;
        self.total_bytes = self.total_bytes.saturating_sub(snap.bytes);
        Some(snap)
    }

    fn evict(&mut self) {
        while self.total_bytes > self.max_bytes && self.entries.len() > 1 {
            if let Some((_, snap)) = self.entries.pop_front() {
                self.total_bytes = self.total_bytes.saturating_sub(snap.bytes);
            }
        }
    }

    fn touch(&mut self, key: &str, snap: Snap) {
        self.drop_key(key);
        self.total_bytes += snap.bytes;
        self.entries.push_back((key.to_string(), snap));
        self.evict();
    }

    /// `captureEditorHistoryFromState`. Unedited states and documents past
    /// the cap are skipped; a shallower stack for the same text never
    /// replaces a deeper one.
    pub fn capture(&mut self, key: &str, state: &EditorState, scroll_top: f32, scroll_left: f32) {
        if key.is_empty() {
            return;
        }
        let depth = state.history.undo_depth() + state.history.redo_depth();
        if depth == 0 || state.doc.utf16_len() > EDITOR_HISTORY_DOC_CAP {
            return;
        }
        let json = state.to_json_with_history();
        let doc = state.doc.to_string();
        if let Some((_, existing)) = self.entries.iter().find(|(k, _)| k == key)
            && existing.doc == doc
            && existing.depth > depth
        {
            return;
        }
        let bytes = serde_json::to_string(&json).map_or(0, |s| s.len());
        self.touch(
            key,
            Snap {
                json,
                doc,
                scroll_top,
                scroll_left,
                bytes,
                depth,
            },
        );
    }

    /// `restoreEditorState`: rebuild a state for `doc`, or `None` when the
    /// snapshot is missing or for different text (which drops it).
    pub fn restore(&mut self, key: &str, doc: &str, config: StateConfig) -> Option<RestoredEditor> {
        let snap = self
            .entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, s)| s.clone())?;
        if snap.doc != doc {
            self.drop_key(key);
            return None;
        }
        self.touch(key, snap.clone());
        match EditorState::from_json_with_history(&snap.json, config) {
            Some(state) => Some(RestoredEditor {
                state,
                scroll_top: snap.scroll_top,
                scroll_left: snap.scroll_left,
            }),
            None => {
                self.drop_key(key);
                None
            }
        }
    }

    pub fn forget(&mut self, key: &str) {
        self.drop_key(key);
    }

    /// Drop a file and everything under it (`forgetEditorHistoryTree`).
    pub fn forget_tree(&mut self, scope: &str, path: &str) {
        if path.is_empty() {
            return;
        }
        let prefix = editor_history_key(scope, path);
        let child = format!("{prefix}/");
        let doomed: Vec<String> = self
            .entries
            .iter()
            .filter(|(k, _)| *k == prefix || k.starts_with(&child))
            .map(|(k, _)| k.clone())
            .collect();
        for k in doomed {
            self.drop_key(&k);
        }
    }

    /// Follow a rename/move (`remapEditorHistory`).
    pub fn remap(&mut self, scope: &str, from: &str, to: &str) {
        if from.is_empty() || to.is_empty() || from == to {
            return;
        }
        let from_key = editor_history_key(scope, from);
        let child = format!("{from_key}/");
        let moves: Vec<(String, String)> = self
            .entries
            .iter()
            .filter_map(|(k, _)| {
                if *k == from_key {
                    Some((k.clone(), editor_history_key(scope, to)))
                } else {
                    k.strip_prefix(&from_key)
                        .filter(|_| k.starts_with(&child))
                        .map(|rest| (k.clone(), editor_history_key(scope, &format!("{to}{rest}"))))
                }
            })
            .collect();
        for (old, new) in moves {
            if let Some(snap) = self.drop_key(&old) {
                self.touch(&new, snap);
            }
        }
    }

    /// `(files, bytes)`.
    pub fn stats(&self) -> (usize, usize) {
        (self.entries.len(), self.total_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::Change;
    use crate::state::TransactionSpec;

    fn edited(doc: &str) -> EditorState {
        let s = EditorState::create(doc, StateConfig::default());
        s.update(
            TransactionSpec::default()
                .changes(Change::insert(0, "x"))
                .user_event("input.type"),
        )
        .state()
        .clone()
    }

    #[test]
    fn captures_restores_and_evicts() {
        let mut c = EditorHistoryCache::default();
        let s = edited("abc");
        let k = editor_history_key("p", "a.rs");
        c.capture(&k, &s, 12.0, 0.0);
        let r = c.restore(&k, "xabc", StateConfig::default()).unwrap();
        assert_eq!(r.scroll_top, 12.0);
        assert_eq!(r.state.history.undo_depth(), 1);
        assert!(c.restore(&k, "different", StateConfig::default()).is_none());
        assert_eq!(c.stats().0, 0);
        c.capture(
            &k,
            &EditorState::create("clean", StateConfig::default()),
            0.0,
            0.0,
        );
        assert_eq!(c.stats().0, 0);

        let mut small = EditorHistoryCache::with_max_bytes(1);
        small.capture("a", &edited("1"), 0.0, 0.0);
        small.capture("b", &edited("2"), 0.0, 0.0);
        assert_eq!(small.stats().0, 1);
        assert!(small.restore("b", "x2", StateConfig::default()).is_some());
    }

    #[test]
    fn remaps_and_forgets_trees() {
        let mut c = EditorHistoryCache::default();
        c.capture(&editor_history_key("p", "d/a"), &edited("1"), 0.0, 0.0);
        c.capture(&editor_history_key("p", "d"), &edited("2"), 0.0, 0.0);
        c.remap("p", "d", "e");
        assert!(
            c.restore(
                &editor_history_key("p", "e/a"),
                "x1",
                StateConfig::default()
            )
            .is_some()
        );
        c.forget_tree("p", "e");
        assert_eq!(c.stats().0, 0);
    }
}
