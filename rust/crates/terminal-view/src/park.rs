//! Keeps terminal views alive across dock unmounts (`lib/caches/term-park.ts`).
//!
//! Hiding the dock or switching workspaces parks the view instead of detaching: the session
//! stays attached and the screen keeps updating, so coming back shows it instantly instead of
//! paying attach + replay. Closing a tab must discard, since a parked screen for a dead PTY would
//! lie. Byte-capped like the other warm caches.

use std::collections::{HashSet, VecDeque};

/// Accounting estimate per parked view: a widget budget, not measured memory.
pub const TERM_PARK_ENTRY_BYTES: usize = 1024 * 1024;
/// ~48 parked terminals; the least recently parked drop past this.
pub const TERM_PARK_MAX_BYTES: usize = 48 * 1024 * 1024;

struct Entry<V> {
    id: String,
    view: V,
    workspace_id: String,
}

pub struct Park<V> {
    entries: VecDeque<Entry<V>>,
    discard: HashSet<String>,
    max_bytes: usize,
}

impl<V> Default for Park<V> {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            discard: HashSet::new(),
            max_bytes: TERM_PARK_MAX_BYTES,
        }
    }
}

impl<V> Park<V> {
    /// Parks `view`, returning any views evicted past the budget (the caller detaches them).
    /// An empty id cannot be parked and comes straight back.
    pub fn stash(&mut self, id: &str, view: V, workspace_id: &str) -> Vec<(String, V)> {
        if id.is_empty() {
            return vec![(String::new(), view)];
        }
        self.discard.remove(id);
        self.entries.retain(|e| e.id != id);
        self.entries.push_back(Entry {
            id: id.to_string(),
            view,
            workspace_id: workspace_id.to_string(),
        });
        let mut evicted = Vec::new();
        while self.entries.len() * TERM_PARK_ENTRY_BYTES > self.max_bytes {
            match self.entries.pop_front() {
                Some(e) => evicted.push((e.id, e.view)),
                None => break,
            }
        }
        evicted
    }

    pub fn take(&mut self, id: &str) -> Option<V> {
        let i = self.entries.iter().position(|e| e.id == id)?;
        self.entries.remove(i).map(|e| e.view)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.entries.iter().any(|e| e.id == id)
    }

    /// Marks `id` for a real dispose and returns its parked view if the pane already unmounted.
    pub fn discard(&mut self, id: &str) -> Option<V> {
        if id.is_empty() {
            return None;
        }
        self.discard.insert(id.to_string());
        self.take(id)
    }

    /// Whether the unmounting pane for `id` must dispose rather than park (consumes the flag).
    pub fn should_dispose(&mut self, id: &str) -> bool {
        self.discard.remove(id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drops parked views whose workspace is gone (views parked without one are kept).
    pub fn prune(&mut self, keep_workspace_ids: &HashSet<String>) -> Vec<(String, V)> {
        let mut dropped = Vec::new();
        let mut kept = VecDeque::new();
        for e in self.entries.drain(..) {
            if e.workspace_id.is_empty() || keep_workspace_ids.contains(&e.workspace_id) {
                kept.push_back(e);
            } else {
                dropped.push((e.id, e.view));
            }
        }
        self.entries = kept;
        dropped
    }

    pub fn set_max_bytes(&mut self, n: usize) {
        self.max_bytes = n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stash_then_take_returns_the_same_view() {
        let mut p = Park::default();
        assert!(p.stash("a", 1, "").is_empty());
        assert_eq!(p.len(), 1);
        assert_eq!(p.take("a"), Some(1));
        assert!(p.is_empty());
    }

    #[test]
    fn evicts_the_oldest_over_the_byte_budget() {
        let mut p = Park::default();
        p.set_max_bytes(TERM_PARK_ENTRY_BYTES * 2);
        p.stash("0", 0, "");
        p.stash("1", 1, "");
        let evicted = p.stash("2", 2, "");
        assert_eq!(
            evicted
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            ["0"]
        );
        assert_eq!(p.len(), 2);
        assert_eq!(p.take("0"), None);
    }

    #[test]
    fn discard_takes_a_parked_view_and_flags_a_live_pane() {
        let mut p = Park::default();
        p.stash("a", 1, "");
        assert_eq!(p.discard("a"), Some(1));
        assert!(p.is_empty());
        p.discard("live");
        assert!(p.should_dispose("live"));
        assert!(!p.should_dispose("live"));
    }

    #[test]
    fn prune_drops_views_whose_workspace_is_gone() {
        let mut p = Park::default();
        p.stash("keep", 1, "ws-a");
        p.stash("gone", 2, "ws-b");
        p.stash("orphan", 3, "");
        let keep: HashSet<String> = ["ws-a".to_string()].into();
        assert_eq!(
            p.prune(&keep)
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            ["gone"]
        );
        assert_eq!(p.len(), 2);
        assert!(p.take("keep").is_some() && p.take("orphan").is_some());
    }

    #[test]
    fn empty_ids_are_not_parked() {
        let mut p = Park::default();
        assert_eq!(p.stash("", 9, "").len(), 1);
        assert!(p.is_empty());
    }
}
