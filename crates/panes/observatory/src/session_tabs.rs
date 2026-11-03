//! Parked editor tab snapshots across workspace switches.
//! The editor keeps each tab's state for in-workspace switches; this LRU holds a workspace's
//! whole tab map so returning to it reuses parsed buffers instead of building them again.
//! Empty parks never clobber a previous snapshot. States are sized from their document length.

use std::collections::HashMap;

use gpui_kit::{App, Global};
use recur_editor::view::TabSnapshot;
use recur_view_widgets::byte_lru::{ByteLru, WARM_CACHE_MAX_BYTES, string_bytes};

pub const EDITOR_SESSION_TAB_PARK_MAX_BYTES: usize = WARM_CACHE_MAX_BYTES;

/// A parked tab map, in insertion order.
pub type ParkedTabs<T> = Vec<(String, T)>;

/// What a parked tab costs the budget.
pub trait ParkedSize {
    fn parked_bytes(&self) -> usize;
}

impl ParkedSize for TabSnapshot {
    /// UTF-16 source plus about 8× for the syntax state (`parkedTabBytes`).
    fn parked_bytes(&self) -> usize {
        let len16 = |t: &recur_editor::text::Text| t.byte_to_utf16(t.len());
        let doc = len16(&self.state.doc);
        let clean = self.clean.as_ref().map_or(0, len16);
        64 + (doc + clean) * 16
    }
}

fn parked_session_bytes<T: ParkedSize>(tabs: &ParkedTabs<T>) -> usize {
    48 + tabs
        .iter()
        .map(|(p, t)| string_bytes(p) + t.parked_bytes())
        .sum::<usize>()
}

pub struct SessionTabPark<T> {
    lru: ByteLru<ParkedTabs<T>>,
}

impl<T: ParkedSize + Clone> Default for SessionTabPark<T> {
    fn default() -> Self {
        SessionTabPark {
            lru: ByteLru::new(EDITOR_SESSION_TAB_PARK_MAX_BYTES, parked_session_bytes::<T>),
        }
    }
}

impl<T: Clone> SessionTabPark<T> {
    /// A park with its own budget and sizing (tests).
    pub fn with_budget(max_bytes: usize, size_of: fn(&ParkedTabs<T>) -> usize) -> Self {
        SessionTabPark {
            lru: ByteLru::new(max_bytes, size_of),
        }
    }

    /// Parks a copy of `tabs` under `session`; an empty map leaves the old snapshot.
    pub fn park(&mut self, session: &str, tabs: &[(String, T)]) {
        if session.is_empty() || tabs.is_empty() {
            return;
        }
        self.lru.set(session, tabs.to_vec());
    }

    /// Takes ownership of a parked map.
    pub fn take(&mut self, session: &str) -> Option<ParkedTabs<T>> {
        if session.is_empty() {
            return None;
        }
        self.lru.take(session)
    }

    pub fn len(&self) -> usize {
        self.lru.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lru.is_empty()
    }

    /// Drops the sessions `keep` rejects.
    pub fn prune(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.lru.prune(|session, _| keep(session));
    }

    /// Drops the parked tabs `keep` rejects, and sessions left empty.
    pub fn prune_tabs(&mut self, keep: impl Fn(&T) -> bool) {
        let sessions: Vec<String> = self.lru.keys().map(str::to_string).collect();
        for session in sessions {
            let empty = match self.lru.peek_mut(&session) {
                Some(tabs) => {
                    tabs.retain(|(_, t)| keep(t));
                    tabs.is_empty()
                }
                None => continue,
            };
            if empty {
                self.lru.delete(&session);
            } else {
                self.lru.refresh(&session);
            }
        }
    }
}

/// The process-wide park per editor pane (`sessionTabPark(pane)`), so a remounted pane still
/// hits.
pub struct SessionTabParks<T = TabSnapshot> {
    panes: HashMap<String, SessionTabPark<T>>,
}

impl<T> Default for SessionTabParks<T> {
    fn default() -> Self {
        SessionTabParks {
            panes: HashMap::new(),
        }
    }
}

impl Global for SessionTabParks<TabSnapshot> {}

impl<T: ParkedSize + Clone> SessionTabParks<T> {
    pub fn pane(&mut self, pane: &str) -> &mut SessionTabPark<T> {
        self.panes.entry(pane.to_string()).or_default()
    }

    /// `pruneAllSessionTabParks`: theme changes drop clean tabs everywhere.
    pub fn prune_all_tabs(&mut self, keep: impl Fn(&T) -> bool) {
        for park in self.panes.values_mut() {
            park.prune_tabs(&keep);
        }
    }

    /// `pruneSessionTabParks`: drop sessions whose workspace id is gone.
    pub fn prune_workspaces(&mut self, live: &std::collections::HashSet<String>) {
        for park in self.panes.values_mut() {
            park.prune(|session| {
                let ws = session.find('\0').map_or(session, |i| &session[i + 1..]);
                ws.is_empty() || live.contains(ws)
            });
        }
    }
}

pub fn parks(cx: &mut App) -> &mut SessionTabParks {
    cx.default_global::<SessionTabParks<TabSnapshot>>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct Tab {
        n: i32,
        dirty: bool,
    }

    impl ParkedSize for Tab {
        fn parked_bytes(&self) -> usize {
            8
        }
    }

    fn tabs(v: &[(&str, i32)]) -> Vec<(String, Tab)> {
        v.iter()
            .map(|(p, n)| {
                (
                    p.to_string(),
                    Tab {
                        n: *n,
                        dirty: false,
                    },
                )
            })
            .collect()
    }

    fn one(_: &ParkedTabs<Tab>) -> usize {
        1
    }

    fn get(t: &ParkedTabs<Tab>, path: &str) -> Option<i32> {
        t.iter().find(|(p, _)| p == path).map(|(_, t)| t.n)
    }

    #[test]
    fn round_trips_and_transfers_ownership_on_take() {
        let mut park = SessionTabPark::<Tab>::default();
        park.park("ws-a", &tabs(&[("a.go", 1), ("b.go", 2)]));
        let taken = park.take("ws-a").unwrap();
        assert_eq!(
            taken
                .iter()
                .map(|(p, t)| (p.as_str(), t.n))
                .collect::<Vec<_>>(),
            [("a.go", 1), ("b.go", 2)]
        );
        assert!(park.take("ws-a").is_none());
    }

    #[test]
    fn an_empty_park_does_not_clobber() {
        let mut park = SessionTabPark::<Tab>::default();
        park.park("ws-a", &tabs(&[("a.go", 1)]));
        park.park("ws-a", &[]);
        assert_eq!(get(&park.take("ws-a").unwrap(), "a.go"), Some(1));
    }

    #[test]
    fn parks_a_copy() {
        let mut park = SessionTabPark::<Tab>::default();
        let mut live = tabs(&[("a.go", 1)]);
        park.park("ws-a", &live);
        live[0].1.n = 99;
        live.extend(tabs(&[("b.go", 2)]));
        assert_eq!(get(&park.take("ws-a").unwrap(), "a.go"), Some(1));
    }

    #[test]
    fn evicts_the_least_recently_parked_session() {
        let mut park = SessionTabPark::<Tab>::with_budget(2, one);
        park.park("a", &tabs(&[("a.go", 1)]));
        park.park("b", &tabs(&[("b.go", 1)]));
        park.park("c", &tabs(&[("c.go", 1)]));
        assert!(park.take("a").is_none());
        assert_eq!(get(&park.take("b").unwrap(), "b.go"), Some(1));
        assert_eq!(get(&park.take("c").unwrap(), "c.go"), Some(1));
    }

    #[test]
    fn reparking_refreshes_lru_order() {
        let mut park = SessionTabPark::<Tab>::with_budget(2, one);
        park.park("a", &tabs(&[("a.go", 1)]));
        park.park("b", &tabs(&[("b.go", 1)]));
        park.park("a", &tabs(&[("a.go", 2)]));
        park.park("c", &tabs(&[("c.go", 1)]));
        assert!(park.take("b").is_none());
        assert_eq!(get(&park.take("a").unwrap(), "a.go"), Some(2));
    }

    #[test]
    fn prune_drops_rejected_sessions() {
        let mut park = SessionTabPark::<Tab>::default();
        park.park("keep", &tabs(&[("a.go", 1)]));
        park.park("drop", &tabs(&[("b.go", 1)]));
        park.prune(|s| s == "keep");
        assert!(park.take("drop").is_none());
        assert_eq!(get(&park.take("keep").unwrap(), "a.go"), Some(1));
    }

    #[test]
    fn prune_tabs_drops_matches_and_empty_sessions() {
        let mut park = SessionTabPark::<Tab>::default();
        park.park(
            "ws-a",
            &[
                ("a.go".into(), Tab { n: 1, dirty: false }),
                ("b.go".into(), Tab { n: 2, dirty: true }),
            ],
        );
        park.park("ws-b", &[("c.go".into(), Tab { n: 3, dirty: false })]);
        park.prune_tabs(|t| t.dirty);
        let a = park.take("ws-a").unwrap();
        assert_eq!(
            a.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
            ["b.go"]
        );
        assert!(park.take("ws-b").is_none());
    }

    #[test]
    fn prunes_across_every_pane() {
        let mut parks = SessionTabParks::<Tab>::default();
        parks.pane("main").park("ws", &tabs(&[("a.go", 1)]));
        parks.pane("split").park("ws", &tabs(&[("b.go", 1)]));
        parks.prune_all_tabs(|t| t.dirty);
        assert!(parks.pane("main").take("ws").is_none());
        assert!(parks.pane("split").take("ws").is_none());
    }

    #[test]
    fn shares_one_lru_per_pane() {
        let mut parks = SessionTabParks::<Tab>::default();
        parks.pane("main").park("ws", &tabs(&[("a.go", 1)]));
        assert_eq!(
            get(&parks.pane("main").take("ws").unwrap(), "a.go"),
            Some(1)
        );
        assert!(parks.pane("split").take("ws").is_none());
    }

    #[test]
    fn prune_workspaces_drops_sessions_whose_workspace_is_gone() {
        let mut parks = SessionTabParks::<Tab>::default();
        parks.pane("main").park("proj\0live", &tabs(&[("a.go", 1)]));
        parks.pane("main").park("proj\0gone", &tabs(&[("b.go", 1)]));
        parks
            .pane("split")
            .park("proj\0gone", &tabs(&[("c.go", 1)]));
        parks.prune_workspaces(&["live".to_string()].into_iter().collect());
        assert_eq!(
            get(&parks.pane("main").take("proj\0live").unwrap(), "a.go"),
            Some(1)
        );
        assert!(parks.pane("main").take("proj\0gone").is_none());
        assert!(parks.pane("split").take("proj\0gone").is_none());
    }
}
