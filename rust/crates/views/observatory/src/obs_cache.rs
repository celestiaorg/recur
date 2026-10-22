//! Keep-alive for Observatory UI state across workspace switches
//! (`lib/caches/observatory-cache.ts`): returning to a recently viewed root restores tabs, tree
//! and editor at once, then the Observatory revalidates in the background.

use std::collections::{BTreeMap, BTreeSet};

use gpui_kit::{App, Global};
use recur_domain::{FileContent, OutlineSymbol, TreeEntry};
use recur_editor::session::{ScratchDoc, StashedBuffer};
use recur_editor::view_state::{EditorViewMode, FileViewState};
use recur_view_chat::byte_lru::{ByteLru, WARM_CACHE_MAX_BYTES, string_bytes};
use recur_view_core::prefs::CodePanelKey;
use recur_view_review::review_diff::ReviewTab;

use crate::model::{FocusZone, MainSurface, Pane, TabBuffer};

/// Typical snapshot: the active buffer plus the tree (0.2 to 1MB). Dirty tab drafts are
/// included; clean ones are re-read from disk.
pub const OBSERVATORY_CACHE_MAX_BYTES: usize = WARM_CACHE_MAX_BYTES;

/// Everything the Observatory restores synchronously for a root (`ObservatorySnapshot`).
#[derive(Clone, Debug)]
pub struct ObservatorySnapshot {
    pub tree: Vec<TreeEntry>,
    pub expanded: BTreeSet<String>,
    pub loaded_dirs: BTreeSet<String>,
    pub changed_collapsed: BTreeSet<String>,
    pub changed_paths: Vec<String>,
    pub git_status: BTreeMap<String, String>,
    /// GitStatus had settled when taken (an empty map is also a clean tree).
    pub git_status_ready: bool,
    pub working_status: BTreeMap<String, String>,
    pub staged_files: Vec<String>,
    pub open_files: Vec<String>,
    pub closed_tabs: Vec<String>,
    pub selected_path: String,
    /// The explorer highlight; may be a folder while `selected_path` is the open file.
    pub tree_path: String,
    pub file: Option<FileContent>,
    pub draft: String,
    pub saved_content: String,
    pub outline: Vec<OutlineSymbol>,
    pub view_mode: EditorViewMode,
    pub base_doc: String,
    pub base_doc_path: String,
    pub browse_all: bool,
    pub scope_chosen: bool,
    pub left_kind: CodePanelKey,
    pub right_kind: CodePanelKey,
    pub left_open: Option<bool>,
    pub show_side: Option<bool>,
    pub split_open: bool,
    pub split_path: String,
    pub split_draft: String,
    pub split_saved: String,
    pub split_writable: bool,
    pub focused_pane: Pane,
    pub focus_zone: FocusZone,
    pub main_editor_dirty: bool,
    pub split_editor_dirty: bool,
    pub session_restored: bool,
    /// The editor session for this root settled (resolved for the still-current root).
    pub session_load_settled: bool,
    pub syntax_maps_ready: Option<bool>,
    pub scratch_docs: BTreeMap<String, ScratchDoc>,
    pub scratch_seq: u64,
    pub file_views: BTreeMap<String, FileViewState>,
    pub review_tab: Option<ReviewTab>,
    pub review_selected_path: Option<String>,
    pub review_scroll_top: Option<f32>,
    pub main_surface: MainSurface,
    pub syntax_by_path: Vec<(String, String)>,
    pub syntax_by_file_type: Vec<(String, String)>,
    pub tab_buffers: Option<BTreeMap<String, TabBuffer>>,
    pub stashed_buffers: BTreeMap<String, StashedBuffer>,
    /// The file-tab strip offset when it had layout; restored as is on A→B→A.
    pub file_tab_scroll_left: Option<f32>,
}

impl Default for ObservatorySnapshot {
    fn default() -> Self {
        ObservatorySnapshot {
            tree: Vec::new(),
            expanded: BTreeSet::new(),
            loaded_dirs: BTreeSet::new(),
            changed_collapsed: BTreeSet::new(),
            changed_paths: Vec::new(),
            git_status: BTreeMap::new(),
            git_status_ready: false,
            working_status: BTreeMap::new(),
            staged_files: Vec::new(),
            open_files: Vec::new(),
            closed_tabs: Vec::new(),
            selected_path: String::new(),
            tree_path: String::new(),
            file: None,
            draft: String::new(),
            saved_content: String::new(),
            outline: Vec::new(),
            view_mode: EditorViewMode::Code,
            base_doc: String::new(),
            base_doc_path: String::new(),
            browse_all: false,
            scope_chosen: false,
            left_kind: CodePanelKey::Maestro,
            right_kind: CodePanelKey::Files,
            left_open: None,
            show_side: None,
            split_open: false,
            split_path: String::new(),
            split_draft: String::new(),
            split_saved: String::new(),
            split_writable: false,
            focused_pane: Pane::Main,
            focus_zone: FocusZone::Editor,
            main_editor_dirty: false,
            split_editor_dirty: false,
            session_restored: false,
            session_load_settled: false,
            syntax_maps_ready: None,
            scratch_docs: BTreeMap::new(),
            scratch_seq: 0,
            file_views: BTreeMap::new(),
            review_tab: None,
            review_selected_path: None,
            review_scroll_top: None,
            main_surface: MainSurface::Editor,
            syntax_by_path: Vec::new(),
            syntax_by_file_type: Vec::new(),
            tab_buffers: None,
            stashed_buffers: BTreeMap::new(),
            file_tab_scroll_left: None,
        }
    }
}

const OBJ: usize = 32;

fn strs<'a>(it: impl IntoIterator<Item = &'a String>) -> usize {
    OBJ + it.into_iter().map(|s| 8 + string_bytes(s)).sum::<usize>()
}

fn map_bytes<'a, V: 'a>(
    it: impl IntoIterator<Item = (&'a String, &'a V)>,
    v: impl Fn(&V) -> usize,
) -> usize {
    OBJ + it
        .into_iter()
        .map(|(k, val)| string_bytes(k) + v(val))
        .sum::<usize>()
}

fn tree_bytes(nodes: &[TreeEntry]) -> usize {
    OBJ + nodes
        .iter()
        .map(|n| {
            OBJ + string_bytes(&n.name)
                + string_bytes(&n.path)
                + string_bytes(&n.status)
                + 16
                + tree_bytes(&n.children)
        })
        .sum::<usize>()
}

fn file_bytes(f: &FileContent) -> usize {
    OBJ + string_bytes(&f.path)
        + string_bytes(&f.language)
        + string_bytes(&f.content)
        + string_bytes(&f.mime)
        + 24
}

/// `estimateBytes` over the snapshot: UTF-16 strings plus fixed object and entry overheads.
pub fn snapshot_bytes(s: &ObservatorySnapshot) -> usize {
    let mut n = OBJ * 2;
    n += tree_bytes(&s.tree);
    n += strs(&s.expanded) + strs(&s.loaded_dirs) + strs(&s.changed_collapsed);
    n += strs(&s.changed_paths) + strs(&s.staged_files) + strs(&s.open_files);
    n += strs(&s.closed_tabs);
    n += map_bytes(&s.git_status, |v| string_bytes(v));
    n += map_bytes(&s.working_status, |v| string_bytes(v));
    n += string_bytes(&s.selected_path) + string_bytes(&s.tree_path);
    n += s.file.as_ref().map_or(0, file_bytes);
    n += string_bytes(&s.draft) + string_bytes(&s.saved_content);
    n += OBJ
        + s.outline
            .iter()
            .map(|o| OBJ + string_bytes(&o.name) + string_bytes(&o.kind) + 16)
            .sum::<usize>();
    n += string_bytes(&s.base_doc) + string_bytes(&s.base_doc_path);
    n += string_bytes(&s.split_path) + string_bytes(&s.split_draft) + string_bytes(&s.split_saved);
    n += map_bytes(&s.scratch_docs, |d| {
        OBJ + string_bytes(&d.title) + string_bytes(&d.content) + string_bytes(&d.lang)
    });
    n += map_bytes(&s.file_views, |_| OBJ + 12 * 8);
    n += OBJ
        + s.syntax_by_path
            .iter()
            .chain(&s.syntax_by_file_type)
            .map(|(k, v)| string_bytes(k) + string_bytes(v))
            .sum::<usize>();
    if let Some(bufs) = &s.tab_buffers {
        n += map_bytes(bufs, |b| {
            OBJ + string_bytes(&b.draft) + string_bytes(&b.saved) + file_bytes(&b.file)
        });
    }
    n += map_bytes(&s.stashed_buffers, |b| {
        OBJ + string_bytes(&b.draft)
            + b.saved.as_deref().map_or(0, string_bytes)
            + b.base_hash.as_deref().map_or(0, string_bytes)
    });
    n += 40 * 8;
    n
}

/// `observatoryCacheKey`.
pub fn observatory_cache_key(project_id: &str, workspace_id: &str) -> String {
    format!("{project_id}\0{workspace_id}")
}

/// The app's snapshot LRU.
pub struct ObservatoryCache {
    lru: ByteLru<ObservatorySnapshot>,
}

impl Default for ObservatoryCache {
    fn default() -> Self {
        ObservatoryCache {
            lru: ByteLru::new(OBSERVATORY_CACHE_MAX_BYTES, snapshot_bytes),
        }
    }
}

impl Global for ObservatoryCache {}

impl ObservatoryCache {
    pub fn get(&mut self, key: &str) -> Option<ObservatorySnapshot> {
        self.lru.get(key).cloned()
    }

    pub fn put(&mut self, key: &str, snap: ObservatorySnapshot) {
        if key.is_empty() || key == "\0" {
            return;
        }
        self.lru.set(key, snap);
    }

    /// Drop snapshots whose workspace id is no longer known.
    pub fn prune(&mut self, live: &std::collections::HashSet<String>) {
        self.lru.prune(|key, _| {
            let ws = key.split('\0').nth(1).unwrap_or("");
            ws.is_empty() || live.contains(ws)
        });
    }

    pub fn drop_key(&mut self, key: &str) {
        self.lru.delete(key);
    }

    pub fn set_max_bytes(&mut self, n: usize) {
        self.lru.set_max_bytes(n);
    }
}

pub fn cache(cx: &mut App) -> &mut ObservatoryCache {
    cx.default_global::<ObservatoryCache>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(path: &str) -> ObservatorySnapshot {
        ObservatorySnapshot {
            selected_path: path.into(),
            ..Default::default()
        }
    }

    #[test]
    fn round_trips_a_snapshot() {
        let mut c = ObservatoryCache::default();
        let key = observatory_cache_key("p", "ws");
        c.put(&key, snap("a.go"));
        assert_eq!(c.get(&key).unwrap().selected_path, "a.go");
    }

    #[test]
    fn evicts_the_oldest_when_over_budget() {
        let mut c = ObservatoryCache::default();
        c.set_max_bytes(1);
        c.put(&observatory_cache_key("p", "a"), snap("a.go"));
        c.put(&observatory_cache_key("p", "b"), snap("b.go"));
        c.put(&observatory_cache_key("p", "c"), snap("c.go"));
        assert!(c.get(&observatory_cache_key("p", "a")).is_none());
        assert_eq!(
            c.get(&observatory_cache_key("p", "c"))
                .unwrap()
                .selected_path,
            "c.go"
        );
    }

    #[test]
    fn prunes_and_drops_by_workspace_id() {
        let mut c = ObservatoryCache::default();
        c.put(&observatory_cache_key("p", "keep"), snap("k.go"));
        c.put(&observatory_cache_key("p", "gone"), snap("g.go"));
        c.prune(&["keep".to_string()].into_iter().collect());
        assert!(c.get(&observatory_cache_key("p", "gone")).is_none());
        c.drop_key(&observatory_cache_key("p", "keep"));
        assert!(c.get(&observatory_cache_key("p", "keep")).is_none());
    }

    #[test]
    fn the_empty_root_is_never_cached() {
        let mut c = ObservatoryCache::default();
        c.put("\0", snap("x"));
        assert!(c.get("\0").is_none());
    }
}
