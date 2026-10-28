//! `workspace-land-cache.ts`: the last titlebar git snapshot per workspace, shared by the
//! workspace land chrome and PR detail's titlebar actions.

use std::collections::{HashMap, VecDeque};

use recur_domain::WorkspaceLandState;

/// `WorkspaceLandSnap`: the last titlebar git snapshot. `require_pr` is unknown until git
/// answers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Land {
    pub behind_base: i64,
    pub ahead_base: i64,
    pub dirty: bool,
    pub unmerged: bool,
    pub base: String,
    pub remote: String,
    pub pr_id: String,
    pub pr_number: i64,
    pub pr_source: String,
    pub pr_draft: bool,
    pub ahead_remote: i64,
    pub behind_remote: i64,
    pub require_pr: Option<bool>,
}

impl From<&WorkspaceLandState> for Land {
    fn from(st: &WorkspaceLandState) -> Self {
        Land {
            behind_base: st.behind_base,
            ahead_base: st.ahead_base,
            dirty: st.dirty,
            unmerged: st.unmerged,
            base: st.base.clone(),
            remote: st.remote.clone(),
            pr_id: st.pr_id.clone(),
            pr_number: st.pr_number,
            pr_source: st.pr_source.clone(),
            pr_draft: st.pr_draft,
            ahead_remote: st.ahead_remote,
            behind_remote: st.behind_remote,
            require_pr: Some(st.require_pr),
        }
    }
}

impl Land {
    /// The resident bytes the web cache budgeted for one snapshot.
    fn bytes(&self, id: &str) -> usize {
        let s = |x: &str| 24 + x.len() * 2;
        32 + s(id) + s(&self.base) + s(&self.remote) + s(&self.pr_id) + s(&self.pr_source) + 13 * 32
    }
}

/// `workspace-land-cache.ts`: insertion-order LRU with a 4 MiB budget; the newest entry is
/// always kept.
#[derive(Default)]
pub struct LandCache {
    map: HashMap<String, Land>,
    order: VecDeque<String>,
    bytes: usize,
}

pub const LAND_CACHE_MAX_BYTES: usize = 8 * 1024 * 1024;

impl LandCache {
    pub fn get(&mut self, id: &str) -> Option<Land> {
        let hit = self.map.get(id).cloned()?;
        self.order.retain(|k| k != id);
        self.order.push_back(id.to_string());
        Some(hit)
    }

    pub fn put(&mut self, id: &str, land: &Land) {
        if id.is_empty() {
            return;
        }
        self.delete(id);
        self.bytes += land.bytes(id);
        self.map.insert(id.to_string(), land.clone());
        self.order.push_back(id.to_string());
        while self.bytes > LAND_CACHE_MAX_BYTES && self.order.len() > 1 {
            if let Some(old) = self.order.pop_front() {
                self.delete_slot(&old);
            }
        }
    }

    fn delete_slot(&mut self, id: &str) {
        if let Some(l) = self.map.remove(id) {
            self.bytes -= l.bytes(id);
        }
    }

    pub fn delete(&mut self, id: &str) {
        self.delete_slot(id);
        self.order.retain(|k| k != id);
    }
}

impl gpui_kit::Global for LandCache {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn land_cache_is_lru() {
        let mut c = LandCache::default();
        c.put("a", &Land::default());
        c.put("b", &Land::default());
        assert!(c.get("a").is_some());
        c.delete("a");
        assert!(c.get("a").is_none());
        assert!(c.get("b").is_some());
    }
}
