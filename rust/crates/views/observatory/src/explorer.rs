//! The file explorer's data side: loading the root and expanded folders, git status, the
//! Changed / All files scope, event-driven live refresh with its gone-confirm window, file
//! operations (new, rename, move, copy / cut / paste, duplicate, delete), the context menu and
//! the explorer's keyboard map.

use recur_ui::guard::Guarded;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use gpui_kit::{AppContext as _, Context, PromptLevel, Task, Window};
use recur_code::glob::{CompiledGlob, is_excluded_by};
use recur_domain::TreeEntry;
use recur_editor::session::remap_open_files;
use recur_ui::widgets::ToastKind;
use recur_view_core::{bridge_error_message, call, toast};

use crate::explorer_visible::is_path_visible_in_explorer_tree;
use crate::model::{base_name, is_under, join_path, parent_path, rewrite_under};
use crate::obs::{
    CtxMenu, DraftEdit, DraftMode, ExplorerReveal, FileClip, GONE_CONFIRM_MAX_MS, GONE_CONFIRM_MS,
    LIVE_REFRESH_MS, Observatory, OpenRequest, REVIEW_REFRESH_MS, SKELETON_DELAY_MS,
};

pub use recur_view_core::prefs::explorer_exclude;
#[cfg(test)]
use recur_view_core::{LocalStorage, prefs::EXPLORER_PREFS_KEY};

/// `patchChildren`: merge one folder's fresh listing into the tree.
pub fn patch_children(nodes: &[TreeEntry], path: &str, kids: &[TreeEntry]) -> Vec<TreeEntry> {
    nodes
        .iter()
        .map(|n| {
            if n.path == path {
                TreeEntry {
                    children: merge_children(&n.children, kids),
                    ..n.clone()
                }
            } else if !n.children.is_empty() {
                TreeEntry {
                    children: patch_children(&n.children, path, kids),
                    ..n.clone()
                }
            } else {
                n.clone()
            }
        })
        .collect()
}

/// `mergeChildren`: a fresh listing that keeps already-fetched grandchildren.
pub fn merge_children(existing: &[TreeEntry], fresh: &[TreeEntry]) -> Vec<TreeEntry> {
    fresh
        .iter()
        .map(|e| match existing.iter().find(|x| x.path == e.path) {
            Some(prev) if !prev.children.is_empty() || prev.dir => TreeEntry {
                children: prev.children.clone(),
                ..e.clone()
            },
            _ => e.clone(),
        })
        .collect()
}

/// `findEntry`.
pub fn find_entry<'a>(nodes: &'a [TreeEntry], path: &str) -> Option<&'a TreeEntry> {
    for n in nodes {
        if n.path == path {
            return Some(n);
        }
        if !n.children.is_empty()
            && let Some(hit) = find_entry(&n.children, path)
        {
            return Some(hit);
        }
    }
    None
}

/// `uniqueSiblingName`: `base`, else `base 2`… (files keep their extension), else `copy`.
pub fn unique_sibling_name(siblings: &[TreeEntry], base: &str, dir: bool) -> String {
    let taken: BTreeSet<&str> = siblings.iter().map(|k| k.name.as_str()).collect();
    if !taken.contains(base) {
        return base.to_string();
    }
    if dir {
        for i in 2..200 {
            let n = format!("{base} {i}");
            if !taken.contains(n.as_str()) {
                return n;
            }
        }
        return format!("{base} copy");
    }
    let (stem, ext) = match base.rfind('.') {
        Some(d) if d > 0 => (&base[..d], &base[d..]),
        _ => (base, ""),
    };
    for i in 2..200 {
        let n = format!("{stem} {i}{ext}");
        if !taken.contains(n.as_str()) {
            return n;
        }
    }
    format!("{stem} copy{ext}")
}

/// `ancestorDirsOf`: every directory on the way to the paths, shallowest first.
pub fn ancestor_dirs_of<'a>(paths: impl IntoIterator<Item = &'a String>) -> Vec<String> {
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    for p in paths {
        for (i, _) in p.match_indices('/') {
            dirs.insert(p[..i].to_string());
        }
    }
    let mut v: Vec<String> = dirs.into_iter().collect();
    v.sort_by_key(|d| d.split('/').count());
    v
}

/// `remapKeyedRecord`: move entries under `from` to `to`, dropping ones they replace.
pub fn remap_keyed<V: Clone>(
    rec: &BTreeMap<String, V>,
    from: &str,
    to: &str,
) -> BTreeMap<String, V> {
    let claimed: BTreeSet<String> = rec
        .keys()
        .filter(|p| is_under(p, from))
        .map(|p| rewrite_under(p, from, to))
        .collect();
    let mut next = BTreeMap::new();
    for (p, v) in rec {
        if is_under(p, from) || claimed.contains(p) {
            continue;
        }
        next.insert(p.clone(), v.clone());
    }
    for (p, v) in rec {
        if is_under(p, from) {
            next.insert(rewrite_under(p, from, to), v.clone());
        }
    }
    next
}

pub fn remap_set(set: &BTreeSet<String>, from: &str, to: &str) -> BTreeSet<String> {
    set.iter()
        .map(|d| {
            if is_under(d, from) {
                rewrite_under(d, from, to)
            } else {
                d.clone()
            }
        })
        .collect()
}

/// Directory → worst status among its descendants (d > a > u > m).
pub fn dir_status_index(git_status: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let rank = |s: &str| match s {
        "d" => 4,
        "a" => 3,
        "u" => 2,
        "m" => 1,
        _ => 0,
    };
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for (p, st) in git_status {
        if st.is_empty() {
            continue;
        }
        for (i, _) in p.match_indices('/') {
            let d = &p[..i];
            let cur = out.get(d).map_or(0, |c| rank(c));
            if rank(st) > cur {
                out.insert(d.to_string(), st.clone());
            }
        }
    }
    out
}

/// `compactStatus`: drop empty entries.
pub fn compact_status(raw: BTreeMap<String, String>) -> BTreeMap<String, String> {
    raw.into_iter().filter(|(_, s)| !s.is_empty()).collect()
}

/// `isNotFound`: the backend's stable `not found:` prefix (a genuine missing path).
pub fn is_not_found(msg: &str) -> bool {
    msg.contains("not found:")
}

/// `isWorktreeMissing`: the recorded worktree path is gone (`code.ErrWorktreeMissing`'s
/// stable "worktree is not a directory" sentinel).
pub fn is_worktree_missing(msg: &str) -> bool {
    msg.to_lowercase().contains("worktree is not a directory")
}

/// `isDirectoryError`: `\bis a directory\b`, case-insensitive (git submodule leaves).
pub fn is_directory_error(msg: &str) -> bool {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\bis a directory\b").expect("directory regex")
    });
    RE.is_match(msg)
}

/// `visibleEntries`: drop excluded entries (and, in Changed scope, unchanged ones).
pub fn visible_entries<'a>(
    entries: &'a [TreeEntry],
    matchers: &[CompiledGlob],
    changed: Option<(&BTreeSet<String>, &BTreeSet<String>)>,
) -> Vec<&'a TreeEntry> {
    visible_entries_by(entries, |p| is_excluded_by(p, matchers), changed)
}

/// [`visible_entries`] with the exclusion test supplied (the Observatory memoizes it).
pub fn visible_entries_by<'a>(
    entries: &'a [TreeEntry],
    excluded: impl Fn(&str) -> bool,
    changed: Option<(&BTreeSet<String>, &BTreeSet<String>)>,
) -> Vec<&'a TreeEntry> {
    entries
        .iter()
        .filter(|e| {
            if excluded(&e.path) {
                return false;
            }
            if let Some((set, dirs)) = changed {
                return set.contains(&e.path) || (e.dir && dirs.contains(&e.path));
            }
            true
        })
        .collect()
}

fn now_since(t: Option<Instant>) -> u64 {
    t.map_or(u64::MAX, |t| t.elapsed().as_millis() as u64)
}

impl Observatory {
    // ---- loading state ----

    pub(crate) fn begin_tree_load(&mut self, cx: &mut Context<Self>) {
        self.tree_loading = true;
        if self.skeleton_timer.is_some() {
            return;
        }
        self.skeleton_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(SKELETON_DELAY_MS))
                .await;
            let _ = this.update(cx, |o, cx| {
                o.skeleton_timer = None;
                if o.tree_loading {
                    o.show_tree_skeleton = true;
                    cx.notify();
                }
            });
        }));
    }

    pub(crate) fn end_tree_load(&mut self) {
        self.tree_loading = false;
        self.show_tree_skeleton = false;
        self.skeleton_timer = None;
    }

    /// The single writer of the file-load indicator: `file_loading` is the honest state,
    /// `show_file_loading` its debounced display (delayed, then kept a minimum span).
    pub(crate) fn set_file_loading(&mut self, next: bool, cx: &mut Context<Self>) {
        self.file_loading = next;
        if next {
            self.file_loading_hide_timer = None;
            if self.file_loading_timer.is_some() || self.show_file_loading {
                return;
            }
            self.file_loading_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(SKELETON_DELAY_MS))
                    .await;
                let _ = this.update(cx, |o, cx| {
                    o.file_loading_timer = None;
                    if !o.file_loading {
                        return;
                    }
                    o.show_file_loading = true;
                    o.file_loading_shown_at = Some(Instant::now());
                    cx.notify();
                });
            }));
            return;
        }
        self.file_loading_timer = None;
        if !self.show_file_loading || self.file_loading_hide_timer.is_some() {
            return;
        }
        let shown_for = now_since(self.file_loading_shown_at);
        let min = crate::obs::FILE_LOAD_MIN_VISIBLE_MS;
        if shown_for >= min {
            self.show_file_loading = false;
            return;
        }
        self.file_loading_hide_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(min - shown_for))
                .await;
            let _ = this.update(cx, |o, cx| {
                o.file_loading_hide_timer = None;
                o.show_file_loading = false;
                cx.notify();
            });
        }));
    }

    /// Everything scoped to one project / worktree, cleared together.
    pub(crate) fn reset_explorer_data(&mut self, cx: &mut Context<Self>) {
        self.begin_tree_load(cx);
        self.tree.clear();
        self.expanded.clear();
        self.loaded_dirs.clear();
        self.loading_dirs.clear();
        self.dir_not_found_since.clear();
        self.maybe_clear_gone_confirm();
        self.changed_paths.clear();
        self.git_status.clear();
        self.paths_gen += 1;
        self.git_status_ready = false;
        self.working_status.clear();
        self.staged_files.clear();
        self.working_changed();
        self.worktree_missing = false;
        self.outline.clear();
        self.hits.clear();
        self.refs.clear();
        self.search_skipped_large = 0;
        self.search_truncated = false;
        self.findings.clear();
        self.expanded_lists.clear();
        self.set_file_loading(false, cx);
        self.file_load_gen += 1;
        self.hydrate_failed_path.clear();
    }

    // ---- the tree ----

    /// `refreshTree`: tree, git status and working / staged state in one batch. `soft` keeps
    /// the current tree visible and merges the fresh root (stale-while-revalidate).
    pub(crate) fn refresh_tree(&mut self, soft: bool, cx: &mut Context<Self>) -> Task<()> {
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        if pid.is_empty() && wid.is_empty() {
            self.end_tree_load();
            return Task::ready(());
        }
        if !wid.is_empty() && self.worktree_path(cx).is_empty() {
            self.skipped_tree_for_create = true;
            if !soft {
                self.end_tree_load();
            }
            return Task::ready(());
        }
        self.skipped_tree_for_create = false;
        let token = self.root_token;
        if !soft {
            self.begin_tree_load(cx);
        }
        let is_git = self.is_git(cx);
        let writable = self.tree_writable();
        let svc = self.services.clone();
        cx.spawn(async move |this, cx| {
            let tree = svc.code.list_tree(&pid, &wid, "").await;
            let st = if is_git {
                call(cx, svc.code.git_status(&pid, &wid), "Git status failed").await
            } else {
                Some(BTreeMap::new())
            };
            let ws = if writable && is_git {
                svc.code.working_status(&pid, &wid).await.ok()
            } else {
                None
            };
            let staged = if writable && is_git {
                svc.code.staged_files(&pid, &wid).await.ok()
            } else {
                None
            };
            let _ = this.update(cx, |o, cx| {
                if token != o.root_token {
                    return;
                }
                match &tree {
                    Err(e) if is_worktree_missing(&bridge_error_message(e)) => {
                        o.worktree_missing = true
                    }
                    Err(e) => toast(
                        ToastKind::Error,
                        format!("Tree failed: {}", bridge_error_message(e)),
                        cx,
                    ),
                    Ok(_) => o.worktree_missing = false,
                }
                if let Ok(t) = tree {
                    if soft {
                        o.tree = merge_children(&o.tree, &t);
                        o.loaded_dirs.insert(String::new());
                    } else {
                        o.tree = t;
                        o.expanded.clear();
                        o.loaded_dirs = [String::new()].into_iter().collect();
                        o.dir_not_found_since.clear();
                        o.maybe_clear_gone_confirm();
                    }
                }
                if let Some(st) = st {
                    let prev = o.git_status.clone();
                    o.git_status = compact_status(st);
                    o.changed_paths = o.git_status.keys().cloned().collect();
                    o.paths_gen += 1;
                    o.maybe_hydrate_restored_current(&prev, cx);
                }
                if let Some(ws) = ws {
                    o.working_status = compact_status(ws);
                }
                if let Some(staged) = staged {
                    o.staged_files = staged;
                }
                o.working_changed();
                o.git_status_ready = true;
                if !soft {
                    o.end_tree_load();
                }
                if soft && o.loaded_dirs.len() > 1 {
                    o.refresh_loaded_dirs(cx).detach();
                }
                o.sync_review_props_later(cx);
                cx.notify();
            });
        })
    }

    /// `ensureDirLoaded`: list a folder (`force` re-lists quietly, keeping children shown).
    pub(crate) fn ensure_dir_loaded(
        &mut self,
        path: &str,
        force: bool,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        if !force && (self.loaded_dirs.contains(path) || self.loading_dirs.contains(path)) {
            return Task::ready(());
        }
        let quiet = force && self.loaded_dirs.contains(path);
        if !quiet {
            self.loading_dirs.insert(path.to_string());
            cx.notify();
        }
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        let svc = self.services.clone();
        let path = path.to_string();
        cx.spawn(async move |this, cx| {
            let kids = call(cx, svc.code.list_tree(&pid, &wid, &path), "Tree failed").await;
            let _ = this.update(cx, |o, cx| {
                if let Some(kids) = kids {
                    o.tree = patch_children(&o.tree, &path, &kids);
                    o.loaded_dirs.insert(path.clone());
                    o.dir_not_found_since.remove(&path);
                }
                if !quiet {
                    o.loading_dirs.remove(&path);
                }
                cx.notify();
            });
        })
    }

    /// `toggleDir`: in Changed scope collapse is local; otherwise load then expand.
    pub(crate) fn toggle_dir(
        &mut self,
        path: &str,
        force_open: bool,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        if self.filter_changed() {
            if force_open || self.changed_collapsed.contains(path) {
                self.changed_collapsed.remove(path);
            } else {
                self.changed_collapsed.insert(path.to_string());
            }
            cx.notify();
            return Task::ready(());
        }
        if self.expanded.contains(path) && !force_open {
            self.expanded.remove(path);
            cx.notify();
            return Task::ready(());
        }
        let load = self.ensure_dir_loaded(path, false, cx);
        let path = path.to_string();
        cx.spawn(async move |this, cx| {
            load.await;
            let _ = this.update(cx, |o, cx| {
                o.expanded.insert(path);
                cx.notify();
            });
        })
    }

    /// `revealInTree`: expand the ancestors of a path (one `ListTrees` for unloaded ones).
    pub(crate) fn reveal_in_tree(&mut self, file_path: &str, cx: &mut Context<Self>) -> Task<()> {
        let parts: Vec<&str> = file_path.split('/').filter(|s| !s.is_empty()).collect();
        if parts.len() <= 1 {
            return Task::ready(());
        }
        let mut dirs: Vec<String> = Vec::new();
        let mut acc = String::new();
        for p in &parts[..parts.len() - 1] {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(p);
            dirs.push(acc.clone());
        }
        if self.filter_changed() {
            for d in &dirs {
                self.changed_collapsed.remove(d);
            }
            cx.notify();
            return Task::ready(());
        }
        let token = self.root_token;
        let missing: Vec<String> = dirs
            .iter()
            .filter(|d| !self.loaded_dirs.contains(*d))
            .cloned()
            .collect();
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        let svc = self.services.clone();
        cx.spawn(async move |this, cx| {
            if !missing.is_empty() {
                let Ok(batch) = svc.code.list_trees(&pid, &wid, &missing).await else {
                    return;
                };
                let ok = this
                    .update(cx, |o, _| {
                        if token != o.root_token {
                            return false;
                        }
                        let mut next = o.tree.clone();
                        for d in &missing {
                            let Some(kids) = batch.get(d) else { continue };
                            next = if d.is_empty() {
                                merge_children(&next, kids)
                            } else {
                                patch_children(&next, d, kids)
                            };
                            o.loaded_dirs.insert(d.clone());
                        }
                        o.tree = next;
                        true
                    })
                    .unwrap_or(false);
                if !ok {
                    return;
                }
            }
            let _ = this.update(cx, |o, cx| {
                if token != o.root_token {
                    return;
                }
                o.expanded.extend(dirs);
                cx.notify();
            });
        })
    }

    /// `expandChangedAncestors`: load and expand every changed file's folders in one batch.
    pub(crate) fn expand_changed_ancestors(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let scope = self.scope_paths();
        let dirs = ancestor_dirs_of(scope.iter());
        if dirs.is_empty() {
            return Task::ready(());
        }
        let missing: Vec<String> = dirs
            .iter()
            .filter(|d| !self.loaded_dirs.contains(*d))
            .cloned()
            .collect();
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        let svc = self.services.clone();
        cx.spawn(async move |this, cx| {
            if !missing.is_empty() {
                let Ok(batch) = svc.code.list_trees(&pid, &wid, &missing).await else {
                    return;
                };
                let _ = this.update(cx, |o, _| {
                    let mut next = o.tree.clone();
                    for d in &missing {
                        let Some(kids) = batch.get(d) else { continue };
                        next = if d.is_empty() {
                            merge_children(&next, kids)
                        } else {
                            patch_children(&next, d, kids)
                        };
                        o.loaded_dirs.insert(d.clone());
                    }
                    o.tree = next;
                });
            }
            let _ = this.update(cx, |o, cx| {
                o.expanded.extend(dirs);
                cx.notify();
            });
        })
    }

    /// `setBrowse`: Changed vs All files, remembered per root.
    pub(crate) fn set_browse(&mut self, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.browse_all = all;
        self.scope_chosen = true;
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        self.state.update(cx, |s, _| {
            crate::session_host::save_explorer_scope(s.storage_mut(), &pid, &wid, all)
        });
        self.select_code_panel(recur_view_core::prefs::CodePanelKey::Files, window, cx);
        if all {
            let expand = self.expand_changed_ancestors(cx);
            cx.spawn(async move |this, cx| {
                expand.await;
                let _ = this.update(cx, |o, cx| {
                    o.refresh_loaded_dirs(cx).detach();
                    o.refresh_changed_files(cx).detach();
                    o.refresh_open_file_if_stale(cx).detach();
                });
            })
            .detach();
        }
        cx.notify();
    }

    // ---- git status ----

    pub(crate) fn refresh_git_status(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        if pid.is_empty() && wid.is_empty() {
            return Task::ready(());
        }
        self.git_status_gen += 1;
        let token = self.git_status_gen;
        if (!wid.is_empty() && self.worktree_path(cx).is_empty()) || !self.is_git(cx) {
            self.working_status_gen += 1;
            self.git_status.clear();
            self.paths_gen += 1;
            self.working_status.clear();
            self.staged_files.clear();
            self.working_changed();
            cx.notify();
            return Task::ready(());
        }
        let svc = self.services.clone();
        cx.spawn(async move |this, cx| {
            let st = call(cx, svc.code.git_status(&pid, &wid), "Git status failed").await;
            let next = this
                .update(cx, |o, cx| {
                    if token != o.git_status_gen {
                        return None;
                    }
                    let st = st?;
                    let next = compact_status(st);
                    if next != o.git_status {
                        let prev = std::mem::replace(&mut o.git_status, next);
                        o.paths_gen += 1;
                        o.maybe_hydrate_restored_current(&prev, cx);
                        o.sync_review_props_later(cx);
                        cx.notify();
                    }
                    Some(o.refresh_working_status(cx))
                })
                .ok()
                .flatten();
            if let Some(t) = next {
                t.await;
            }
        })
    }

    pub(crate) fn refresh_working_status(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if !self.tree_writable() || !self.is_git(cx) || self.worktree_path(cx).is_empty() {
            self.working_status_gen += 1;
            self.working_status.clear();
            self.staged_files.clear();
            self.working_changed();
            cx.notify();
            return Task::ready(());
        }
        self.working_status_gen += 1;
        let token = self.working_status_gen;
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        let svc = self.services.clone();
        cx.spawn(async move |this, cx| {
            let st = call(
                cx,
                svc.code.working_status(&pid, &wid),
                "Working status failed",
            )
            .await;
            let staged = svc.code.staged_files(&pid, &wid).await.ok();
            let _ = this.update(cx, |o, cx| {
                if token != o.working_status_gen {
                    return;
                }
                // A watcher event refreshes the status; one that changed nothing redraws
                // nothing (the changes list beside the commit message is costly on a large
                // change set).
                let mut changed = false;
                if let Some(st) = st.map(compact_status)
                    && st != o.working_status
                {
                    o.working_status = st;
                    changed = true;
                }
                if let Some(s) = staged
                    && s != o.staged_files
                {
                    o.staged_files = s;
                    changed = true;
                }
                if !changed {
                    return;
                }
                o.working_changed();
                o.sync_review_props_later(cx);
                cx.notify();
            });
        })
    }

    pub(crate) fn refresh_changed_files(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let wid = self.props.workspace_id.clone();
        if wid.is_empty() || !self.is_git(cx) {
            if !self.is_git(cx) {
                self.changed_paths.clear();
                self.paths_gen += 1;
            }
            return Task::ready(());
        }
        self.changed_files_gen += 1;
        let token = self.changed_files_gen;
        let svc = self.services.clone();
        cx.spawn(async move |this, cx| {
            let Ok(ch) = svc.code.list_changed_files(&wid).await else {
                return;
            };
            let _ = this.update(cx, |o, cx| {
                // A watcher event re-lists them; the same list redraws nothing.
                if token != o.changed_files_gen || ch == o.changed_paths {
                    return;
                }
                // Changed files appearing for the first time while the full tree shows keep
                // the tree; the scope flips only when the user picks it.
                if !o.scope_chosen
                    && o.props.loop_scoped
                    && !o.browse_all
                    && o.changed_paths.is_empty()
                    && !ch.is_empty()
                {
                    o.browse_all = true;
                }
                o.changed_paths = ch;
                o.paths_gen += 1;
                o.sync_review_props_later(cx);
                cx.notify();
            });
        })
    }

    /// `softRefresh`: re-list the given parents and status, keeping the explorer's scroll.
    pub(crate) fn soft_refresh(
        &mut self,
        parents: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let mut uniq: Vec<String> = Vec::new();
        for p in parents {
            if !uniq.contains(&p) {
                uniq.push(p);
            }
        }
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        let svc = self.services.clone();
        let scroll = self.files_scroll.offset();
        cx.spawn(async move |this, cx| {
            for p in uniq {
                if p.is_empty() {
                    if let Some(t) =
                        call(cx, svc.code.list_tree(&pid, &wid, ""), "Tree failed").await
                    {
                        let _ = this.update(cx, |o, _| o.tree = merge_children(&o.tree, &t));
                    }
                    continue;
                }
                let load = this
                    .update(cx, |o, cx| {
                        if !o.expanded.contains(&p) && !o.loaded_dirs.contains(&p) {
                            return None;
                        }
                        Some(o.ensure_dir_loaded(&p, true, cx))
                    })
                    .ok()
                    .flatten();
                if let Some(load) = load {
                    load.await;
                }
            }
            if let Ok(t) = this.update(cx, |o, cx| o.refresh_git_status(cx)) {
                t.await;
            }
            if !wid.is_empty()
                && let Some(ch) = call(
                    cx,
                    svc.code.list_changed_files(&wid),
                    "Changed files failed",
                )
                .await
            {
                let _ = this.update(cx, |o, _| {
                    o.changed_paths = ch;
                    o.paths_gen += 1;
                });
            }
            let _ = this.update(cx, |o, cx| {
                o.files_scroll.set_offset(scroll);
                cx.notify();
            });
        })
    }

    // ---- live refresh ----

    /// `scheduleLiveRefresh`: collapse same-tick FS events into one refresh.
    pub(crate) fn schedule_live_refresh(&mut self, cx: &mut Context<Self>) {
        if !self.live_refresh_alive || self.live_refresh_timer.is_some() {
            return;
        }
        self.live_refresh_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(LIVE_REFRESH_MS))
                .await;
            let _ = this.update(cx, |o, cx| {
                o.live_refresh_timer = None;
                if !o.live_refresh_alive {
                    return;
                }
                o.refresh_changed_files(cx).detach();
                o.refresh_git_status(cx).detach();
                o.refresh_open_file_if_stale(cx).detach();
                o.schedule_review_refresh(cx);
                if o.browse_all
                    || !o.show_changed_first()
                    || o.refresh_loaded_dirs_pending
                    || o.worktree_missing
                {
                    o.refresh_loaded_dirs(cx).detach();
                }
            });
        }));
    }

    pub(crate) fn schedule_review_refresh(&mut self, cx: &mut Context<Self>) {
        if self.main_surface(cx) != crate::model::MainSurface::Review
            || self.review_refresh_timer.is_some()
        {
            return;
        }
        self.review_refresh_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(REVIEW_REFRESH_MS))
                .await;
            let _ = this.update(cx, |o, cx| {
                o.review_refresh_timer = None;
                if o.main_surface(cx) != crate::model::MainSurface::Review {
                    return;
                }
                // A swap while a line-note composer is open would re-anchor the draft.
                if o.review_composer_open(cx) {
                    return;
                }
                o.review_refresh += 1;
                o.sync_review_props_later(cx);
            });
        }));
    }

    pub(crate) fn clear_live_refresh_timers(&mut self) {
        self.refresh_loaded_dirs_pending = false;
        self.refresh_open_file_pending = false;
        self.live_refresh_timer = None;
        self.review_refresh_timer = None;
        self.gone_confirm_timer = None;
    }

    pub(crate) fn stop_live_refresh(&mut self) {
        self.live_refresh_alive = false;
        self.clear_live_refresh_timers();
    }

    pub(crate) fn arm_gone_confirm(&mut self, cx: &mut Context<Self>) {
        if !self.live_refresh_alive || self.gone_confirm_timer.is_some() {
            return;
        }
        self.gone_confirm_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(GONE_CONFIRM_MS))
                .await;
            let tasks = this
                .update(cx, |o, cx| {
                    o.gone_confirm_timer = None;
                    if !o.live_refresh_alive {
                        return None;
                    }
                    Some((o.refresh_open_file_if_stale(cx), o.refresh_loaded_dirs(cx)))
                })
                .ok()
                .flatten();
            let Some((a, b)) = tasks else { return };
            a.await;
            b.await;
            let _ = this.update(cx, |o, cx| {
                if o.live_refresh_alive && o.gone_confirm_still_due() {
                    o.arm_gone_confirm(cx);
                }
            });
        }));
    }

    fn gone_confirm_still_due(&self) -> bool {
        if self
            .open_file_not_found_since
            .is_some_and(|t| (t.elapsed().as_millis() as u64) < GONE_CONFIRM_MAX_MS)
        {
            return true;
        }
        self.dir_not_found_since
            .values()
            .any(|t| (t.elapsed().as_millis() as u64) < GONE_CONFIRM_MAX_MS)
    }

    pub(crate) fn maybe_clear_gone_confirm(&mut self) {
        if !self.dir_not_found_since.is_empty() || self.open_file_not_found_since.is_some() {
            return;
        }
        self.gone_confirm_timer = None;
    }

    /// `refreshLoadedDirs`: re-list the folders on screen in one batch; a folder missing for
    /// `GONE_CONFIRM_MS` across refreshes leaves the tree, a single miss never does.
    pub(crate) fn refresh_loaded_dirs(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if !self.live_refresh_alive {
            return Task::ready(());
        }
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        if pid.is_empty() && wid.is_empty() {
            return Task::ready(());
        }
        if self.refresh_loaded_dirs_in_flight {
            self.refresh_loaded_dirs_pending = true;
            return Task::ready(());
        }
        self.refresh_loaded_dirs_in_flight = true;
        self.refresh_loaded_dirs_pending = false;
        let token = self.root_token;
        let mut dirs: Vec<String> = self.loaded_dirs.iter().cloned().collect();
        dirs.sort_by_key(|d| d.split('/').count());
        let svc = self.services.clone();
        cx.spawn(async move |this, cx| {
            let batch = svc.code.list_trees(&pid, &wid, &dirs).await;
            let _ = this.update(cx, |o, cx| {
                o.refresh_loaded_dirs_in_flight = false;
                let pending = o.refresh_loaded_dirs_pending;
                let missing = o.worktree_missing;
                let mut changed = false;
                if token == o.root_token {
                    match batch {
                        Err(e) => {
                            if is_worktree_missing(&bridge_error_message(&e)) {
                                o.worktree_missing = true;
                            }
                        }
                        Ok(batch) => {
                            o.worktree_missing = false;
                            let mut next = o.tree.clone();
                            let mut gone: Vec<String> = Vec::new();
                            for dir in &dirs {
                                if let Some(kids) = batch.get(dir) {
                                    next = if dir.is_empty() {
                                        merge_children(&next, kids)
                                    } else {
                                        patch_children(&next, dir, kids)
                                    };
                                    o.dir_not_found_since.remove(dir);
                                    continue;
                                }
                                if dir.is_empty() {
                                    continue;
                                }
                                match o.dir_not_found_since.get(dir) {
                                    None => {
                                        o.dir_not_found_since.insert(dir.clone(), Instant::now());
                                        o.arm_gone_confirm(cx);
                                    }
                                    Some(first)
                                        if first.elapsed().as_millis() as u64
                                            >= GONE_CONFIRM_MS =>
                                    {
                                        gone.push(dir.clone());
                                    }
                                    Some(_) => o.arm_gone_confirm(cx),
                                }
                            }
                            changed = next != o.tree || !gone.is_empty();
                            o.tree = next;
                            for d in &gone {
                                o.dir_not_found_since.remove(d);
                                o.loaded_dirs.remove(d);
                                o.expanded.remove(d);
                            }
                            o.maybe_clear_gone_confirm();
                        }
                    }
                }
                if pending {
                    o.schedule_live_refresh(cx);
                }
                // Every watcher event re-lists the folders on screen; a listing that changed
                // nothing must not redraw the explorer (and every other region of the view).
                if changed || o.worktree_missing != missing {
                    cx.notify();
                }
            });
        })
    }

    // ---- tree selection ----

    /// `shouldSelectInExplorer`.
    pub(crate) fn should_select_in_explorer(
        &self,
        path: &str,
        explorer: ExplorerReveal,
        cx: &gpui_kit::App,
    ) -> bool {
        if explorer != ExplorerReveal::IfVisible {
            return true;
        }
        if !self.files_explorer_is_showing(cx) {
            return false;
        }
        let matchers = self.exclude_matchers(cx);
        let filter = self.filter_changed();
        if filter {
            let tree =
                recur_code::view::changed_tree::build_tree_from_paths(&self.review_paths(cx));
            is_path_visible_in_explorer_tree(
                &tree,
                path,
                |d| !self.changed_collapsed.contains(d),
                None,
            )
        } else {
            let roots: Vec<TreeEntry> = visible_entries(&self.tree, &matchers, None)
                .into_iter()
                .cloned()
                .collect();
            let kids: &dyn Fn(&TreeEntry) -> Vec<&TreeEntry> =
                &|n| visible_entries(&n.children, &matchers, None);
            is_path_visible_in_explorer_tree(
                &roots,
                path,
                |d| self.expanded.contains(d),
                Some(kids),
            )
        }
    }

    /// `scrollExplorerRow`: scroll the Files list to the row (below the stuck ancestors).
    pub(crate) fn scroll_explorer_row(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.tree_path != path || !self.files_explorer_is_showing(cx) {
            return;
        }
        self.pending_explorer_scroll = Some(path.to_string());
        cx.notify();
    }

    /// `syncExplorerToPath`.
    pub(crate) fn sync_explorer_to_path(
        &mut self,
        path: &str,
        explorer: ExplorerReveal,
        cx: &mut Context<Self>,
    ) {
        if explorer == ExplorerReveal::IfVisible {
            self.scroll_explorer_row(path, cx);
            return;
        }
        let revealed = self.reveal_in_tree(path, cx);
        if explorer == ExplorerReveal::Scroll {
            let p = path.to_string();
            cx.spawn(async move |this, cx| {
                revealed.await;
                let _ = this.update(cx, |o, cx| o.scroll_explorer_row(&p, cx));
            })
            .detach();
        } else {
            revealed.detach();
        }
    }

    /// The git letter for a row: live status, else the listing's, else the worst child's.
    pub(crate) fn entry_status(
        &self,
        e: &TreeEntry,
        dir_index: &BTreeMap<String, String>,
    ) -> String {
        if let Some(s) = self.git_status.get(&e.path).filter(|s| !s.is_empty()) {
            return s.clone();
        }
        if !e.status.is_empty() {
            return e.status.clone();
        }
        if !e.dir {
            return String::new();
        }
        dir_index.get(&e.path).cloned().unwrap_or_default()
    }

    /// `onNode`: folders toggle, files open (Diff first in Changed scope or when deleted).
    pub(crate) fn on_node(&mut self, e: &TreeEntry, window: &mut Window, cx: &mut Context<Self>) {
        self.ctx_menu = None;
        if e.dir {
            self.tree_path = e.path.clone();
            self.toggle_dir(&e.path, false, cx).detach();
            cx.notify();
        } else {
            let st = self.entry_status(e, &self.dir_status());
            let prefer = self.filter_changed() || st == "d";
            self.open_path(
                OpenRequest::new(e.path.clone()).prefer_diff(prefer),
                window,
                cx,
            )
            .detach();
        }
    }

    /// A row click: a second click starts a rename.
    pub(crate) fn on_node_click(
        &mut self,
        e: &TreeEntry,
        click_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.skip_node_click {
            return;
        }
        if click_count > 1 {
            self.start_rename(&e.path, &e.name, e.dir, window, cx);
            return;
        }
        self.on_node(e, window, cx);
    }

    /// `actionDir`: where toolbar New File / New Folder land.
    pub(crate) fn action_dir(&self, cx: &gpui_kit::App) -> String {
        let path = if self.tree_path.is_empty() {
            &self.selected_path
        } else {
            &self.tree_path
        };
        if path.is_empty() {
            return String::new();
        }
        let tree = self.active_tree(cx);
        if find_entry(&tree, path).is_some_and(|e| e.dir) {
            return path.clone();
        }
        parent_path(path).to_string()
    }

    /// `activeTree`: the derived Changed tree, else the listing.
    pub(crate) fn active_tree(&self, cx: &gpui_kit::App) -> Vec<TreeEntry> {
        if self.filter_changed() {
            recur_code::view::changed_tree::build_tree_from_paths(&self.review_paths(cx))
        } else {
            self.tree.clone()
        }
    }

    // ---- context menu ----

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open_ctx(
        &mut self,
        x: f32,
        y: f32,
        path: &str,
        dir: bool,
        name: &str,
        tab: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let pad = 8.;
        let mw = 220.;
        let mh = if tab { 110. } else { 360. };
        let vw = f32::from(window.viewport_size().width);
        let vh = f32::from(window.viewport_size().height);
        let mut x = x;
        let mut y = y;
        if x + mw > vw - pad {
            x = vw - mw - pad;
        }
        if y + mh > vh - pad {
            y = vh - mh - pad;
        }
        self.ctx_menu = Some(CtxMenu {
            x: x.max(pad),
            y: y.max(pad),
            path: path.to_string(),
            dir,
            name: name.to_string(),
            tab,
        });
        cx.notify();
    }

    pub(crate) fn close_ctx(&mut self, cx: &mut Context<Self>) {
        if self.ctx_menu.take().is_some() {
            cx.notify();
        }
    }

    // ---- confirmation ----

    /// `askConfirm`: the native question dialog; true only for the confirm button.
    pub(crate) fn ask_confirm(
        &self,
        title: &str,
        message: &str,
        confirm: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let answers = [
            gpui_kit::PromptButton::Cancel("Cancel".into()),
            gpui_kit::PromptButton::Ok(confirm.to_string().into()),
        ];
        let rx = window.prompt(PromptLevel::Info, title, Some(message), &answers, cx);
        cx.background_spawn(async move { matches!(rx.await, Ok(1)) })
    }

    // ---- drafts (new / rename) ----

    #[allow(clippy::too_many_arguments)]
    fn begin_draft(
        &mut self,
        mode: DraftMode,
        parent: &str,
        path: &str,
        name: &str,
        dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = name.to_string();
        let input = cx.new(|cx| {
            let mut s = gpui_kit::base::input::InputState::new(window, cx);
            s.set_value(name, window, cx);
            s
        });
        let sub = cx.guarded_subscribe_in(
            &input,
            window,
            |o, _, ev: &gpui_kit::base::input::InputEvent, window, cx| match ev {
                gpui_kit::base::input::InputEvent::PressEnter { .. } => {
                    o.commit_draft(window, cx);
                }
                gpui_kit::base::input::InputEvent::Blur => {
                    o.commit_draft(window, cx);
                }
                gpui_kit::base::input::InputEvent::Change => cx.notify(),
                _ => {}
            },
        );
        self._subscriptions.push(sub);
        let focus = gpui_kit::Focusable::focus_handle(input.read(cx), cx);
        let inp = input.clone();
        window.defer(cx, move |window, cx| {
            window.focus(&focus, cx);
            inp.update(cx, |s, cx| s.select_all(window, cx));
        });
        self.draft_edit = Some(DraftEdit {
            mode,
            parent: parent.to_string(),
            path: path.to_string(),
            dir,
            input,
        });
        cx.notify();
    }

    /// `startNew`.
    pub(crate) fn start_new(
        &mut self,
        folder: bool,
        parent: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ctx_menu = None;
        if !self.tree_writable() {
            toast(ToastKind::Info, "Create files in a loop worktree", cx);
            return;
        }
        let parent = parent.to_string();
        let open = if parent.is_empty() {
            Task::ready(())
        } else {
            self.toggle_dir(&parent, true, cx)
        };
        cx.spawn_in(window, async move |this, cx| {
            open.await;
            let _ = this.update_in(cx, |o, window, cx| {
                let (mode, name) = if folder {
                    (DraftMode::NewFolder, "new-folder")
                } else {
                    (DraftMode::NewFile, "untitled")
                };
                o.begin_draft(mode, &parent, "", name, folder, window, cx);
            });
        })
        .detach();
    }

    /// `startRename`.
    pub(crate) fn start_rename(
        &mut self,
        path: &str,
        name: &str,
        dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ctx_menu = None;
        if !self.tree_writable() || path.is_empty() {
            return;
        }
        let parent = parent_path(path).to_string();
        self.begin_draft(DraftMode::Rename, &parent, path, name, dir, window, cx);
    }

    pub(crate) fn cancel_draft(&mut self, cx: &mut Context<Self>) {
        self.draft_edit = None;
        cx.notify();
    }

    /// `commitDraft`.
    pub(crate) fn commit_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.draft_edit.take() else {
            return;
        };
        let name = ed.name(cx).trim().to_string();
        cx.notify();
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            toast(ToastKind::Info, "Invalid name", cx);
            return;
        }
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        let svc = self.services.clone();
        if ed.mode == DraftMode::Rename {
            let to = join_path(&ed.parent, &name);
            if to.is_empty() || to == ed.path {
                return;
            }
            let confirm = self.confirm_discard_remap_dest(&ed.path, &to, window, cx);
            cx.spawn_in(window, async move |this, cx| {
                if !confirm.await {
                    return;
                }
                if call(
                    cx,
                    svc.code.rename(&pid, &wid, &ed.path, &to),
                    "Rename failed",
                )
                .await
                .is_none()
                {
                    return;
                }
                let refresh = this
                    .update_in(cx, |o, window, cx| {
                        o.remap_open_path(&ed.path, &to, window, cx);
                        o.soft_refresh(vec![ed.parent.clone(), parent_path(&to).to_string()], cx)
                    })
                    .ok();
                if let Some(r) = refresh {
                    r.await;
                }
                cx.update(|_, cx| toast(ToastKind::Success, "Renamed", cx))
                    .ok();
            })
            .detach();
            return;
        }
        let dest = join_path(&ed.parent, &name);
        if dest.is_empty() {
            toast(ToastKind::Info, "Invalid name", cx);
            return;
        }
        let new_file = ed.mode == DraftMode::NewFile;
        cx.spawn_in(window, async move |this, cx| {
            let ok = if new_file {
                call(cx, svc.code.create_file(&pid, &wid, &dest), "Create failed").await
            } else {
                call(
                    cx,
                    svc.code.mkdir(&pid, &wid, &dest),
                    "Create folder failed",
                )
                .await
            };
            if ok.is_none() {
                return;
            }
            if let Ok(r) = this.update(cx, |o, cx| o.soft_refresh(vec![ed.parent.clone()], cx)) {
                r.await;
            }
            if !ed.parent.is_empty()
                && let Ok(t) = this.update(cx, |o, cx| o.toggle_dir(&ed.parent, true, cx))
            {
                t.await;
            }
            let _ = this.update_in(cx, |o, window, cx| {
                if new_file {
                    o.open_path(OpenRequest::new(dest.clone()), window, cx)
                        .detach();
                } else {
                    o.toggle_dir(&dest, true, cx).detach();
                }
            });
        })
        .detach();
    }

    // ---- moves and remaps ----

    /// `remapWouldDiscardUnsaved`: a move would replace an unsaved destination tab.
    pub(crate) fn remap_would_discard_unsaved(&self, from: &str, to: &str) -> bool {
        if from.is_empty() || to.is_empty() || from == to {
            return false;
        }
        let hit = |p: &str| is_under(p, to) && !is_under(p, from) && self.tab_is_dirty(p);
        self.tab_buffers.keys().any(|p| hit(p))
            || self.stashed_buffers.keys().any(|p| hit(p))
            || self.dirty_tabs.iter().any(|p| hit(p))
    }

    pub(crate) fn confirm_discard_remap_dest(
        &self,
        from: &str,
        to: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        if !self.remap_would_discard_unsaved(from, to) {
            return Task::ready(true);
        }
        self.ask_confirm(
            "Unsaved edits",
            "Discard unsaved edits?",
            "Discard",
            window,
            cx,
        )
    }

    /// `remapOpenPath`: follow a rename / move through tabs, buffers, views and the tree.
    pub(crate) fn remap_open_path(
        &mut self,
        from: &str,
        to: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.capture_editor_history(window, cx);
        crate::editor_host::remap_history(&self.editor_session_key, from, to, cx);
        self.open_files = remap_open_files(&self.open_files, from, to);
        self.tab_buffers = remap_keyed(&self.tab_buffers, from, to);
        let dirty: BTreeMap<String, ()> = self.dirty_tabs.iter().map(|p| (p.clone(), ())).collect();
        self.dirty_tabs = remap_keyed(&dirty, from, to).into_keys().collect();
        self.file_views = remap_keyed(&self.file_views, from, to);
        self.stashed_buffers = remap_keyed(&self.stashed_buffers, from, to);
        for p in [
            &mut self.selected_path,
            &mut self.tree_path,
            &mut self.editor_path,
            &mut self.split_path,
            &mut self.base_doc_path,
        ] {
            if is_under(p, from) {
                *p = rewrite_under(p, from, to);
            }
        }
        if let Some(f) = self.file.as_mut()
            && is_under(&f.path, from)
        {
            f.path = rewrite_under(&f.path, from, to);
        }
        self.expanded = remap_set(&self.expanded, from, to);
        self.loaded_dirs = remap_set(&self.loaded_dirs, from, to);
        self.sync_editors(window, cx);
        cx.notify();
    }

    /// `dropOpenPath`: a deleted path leaves the strip, the stash and the panes.
    pub(crate) fn drop_open_path(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_files.retain(|p| !is_under(p, path));
        self.stashed_buffers.retain(|p, _| !is_under(p, path));
        if is_under(&self.tree_path, path) {
            self.tree_path.clear();
        }
        if is_under(&self.editor_path, path) {
            self.editor_path.clear();
        }
        if is_under(&self.selected_path, path) && !self.selected_path.is_empty() {
            self.file = None;
            self.selected_is_dir = false;
            self.base_doc.clear();
            self.base_doc_path.clear();
            self.selected_path.clear();
            self.draft.clear();
            self.saved_content.clear();
            if let Some(next) = self.open_files.first().cloned() {
                self.open_path(OpenRequest::new(next), window, cx).detach();
            } else {
                self.detach_lsp(cx);
            }
        }
        if self.open_files.is_empty() {
            self.persist_or_clear_editor_session(cx);
        }
        let scope = self.editor_session_key.clone();
        let path = path.to_string();
        cx.defer(move |cx| crate::editor_host::forget_history_tree(&scope, &path, cx));
        self.sync_editors(window, cx);
        cx.notify();
    }

    pub(crate) fn move_path(
        &mut self,
        src: &str,
        dest: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let confirm = self.confirm_discard_remap_dest(src, dest, window, cx);
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        let svc = self.services.clone();
        let (src, dest) = (src.to_string(), dest.to_string());
        cx.spawn_in(window, async move |this, cx| {
            if !confirm.await {
                return false;
            }
            if call(cx, svc.code.rename(&pid, &wid, &src, &dest), "Move failed")
                .await
                .is_none()
            {
                return false;
            }
            let refresh = this
                .update_in(cx, |o, window, cx| {
                    o.remap_open_path(&src, &dest, window, cx);
                    o.soft_refresh(
                        vec![
                            parent_path(&src).to_string(),
                            parent_path(&dest).to_string(),
                        ],
                        cx,
                    )
                })
                .ok();
            if let Some(r) = refresh {
                r.await;
            }
            cx.update(|_, cx| toast(ToastKind::Success, "Moved", cx))
                .ok();
            true
        })
    }

    pub(crate) fn delete_path(
        &mut self,
        path: &str,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ctx_menu = None;
        if !self.tree_writable() || path.is_empty() {
            return;
        }
        let confirm = self.ask_confirm(
            "Delete",
            &format!("Delete “{name}”? This cannot be undone."),
            "Delete",
            window,
            cx,
        );
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        let svc = self.services.clone();
        let path = path.to_string();
        cx.spawn_in(window, async move |this, cx| {
            if !confirm.await {
                return;
            }
            if call(cx, svc.code.delete(&pid, &wid, &path), "Delete failed")
                .await
                .is_none()
            {
                return;
            }
            let refresh = this
                .update_in(cx, |o, window, cx| {
                    o.drop_open_path(&path, window, cx);
                    o.soft_refresh(vec![parent_path(&path).to_string()], cx)
                })
                .ok();
            if let Some(r) = refresh {
                r.await;
            }
            cx.update(|_, cx| toast(ToastKind::Success, "Deleted", cx))
                .ok();
        })
        .detach();
    }

    pub(crate) fn clip_copy(&mut self, path: &str, dir: bool, cx: &mut Context<Self>) {
        self.ctx_menu = None;
        if path.is_empty() {
            return;
        }
        self.file_clip = Some(FileClip {
            cut: false,
            path: path.to_string(),
            dir,
        });
        toast(ToastKind::Info, "Copied", cx);
        cx.notify();
    }

    pub(crate) fn clip_cut(&mut self, path: &str, dir: bool, cx: &mut Context<Self>) {
        self.ctx_menu = None;
        if !self.tree_writable() || path.is_empty() {
            return;
        }
        self.file_clip = Some(FileClip {
            cut: true,
            path: path.to_string(),
            dir,
        });
        toast(ToastKind::Info, "Cut", cx);
        cx.notify();
    }

    fn siblings_of(&self, parent: &str) -> Vec<TreeEntry> {
        if parent.is_empty() {
            self.tree.clone()
        } else {
            find_entry(&self.tree, parent)
                .map(|e| e.children.clone())
                .unwrap_or_default()
        }
    }

    pub(crate) fn clip_paste(
        &mut self,
        into_dir: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ctx_menu = None;
        let Some(clip) = self.file_clip.clone() else {
            return;
        };
        if !self.tree_writable() {
            return;
        }
        let load = if into_dir.is_empty() {
            self.soft_refresh(vec![String::new()], cx)
        } else {
            self.ensure_dir_loaded(into_dir, true, cx)
        };
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        let svc = self.services.clone();
        let into = into_dir.to_string();
        cx.spawn_in(window, async move |this, cx| {
            load.await;
            let dest = this
                .update(cx, |o, _| {
                    let base = clip
                        .path
                        .rsplit('/')
                        .next()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("copy");
                    let name = unique_sibling_name(&o.siblings_of(&into), base, clip.dir);
                    join_path(&into, &name)
                })
                .unwrap_or_default();
            if dest.is_empty() {
                return;
            }
            if clip.cut {
                if dest == clip.path {
                    return;
                }
                let moved = this
                    .update_in(cx, |o, window, cx| {
                        o.move_path(&clip.path, &dest, window, cx)
                    })
                    .ok();
                if let Some(m) = moved
                    && m.await
                {
                    let _ = this.update(cx, |o, cx| {
                        o.file_clip = None;
                        cx.notify();
                    });
                }
                return;
            }
            if call(
                cx,
                svc.code.copy(&pid, &wid, &clip.path, &dest),
                "Paste failed",
            )
            .await
            .is_none()
            {
                return;
            }
            if let Ok(r) = this.update(cx, |o, cx| o.soft_refresh(vec![into.clone()], cx)) {
                r.await;
            }
            if !into.is_empty()
                && let Ok(t) = this.update(cx, |o, cx| o.toggle_dir(&into, true, cx))
            {
                t.await;
            }
            cx.update(|_, cx| toast(ToastKind::Success, "Pasted", cx))
                .ok();
        })
        .detach();
    }

    pub(crate) fn duplicate_path(
        &mut self,
        path: &str,
        dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ctx_menu = None;
        if !self.tree_writable() || path.is_empty() {
            return;
        }
        let parent = parent_path(path).to_string();
        let load = if parent.is_empty() {
            self.soft_refresh(vec![String::new()], cx)
        } else {
            self.ensure_dir_loaded(&parent, true, cx)
        };
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        let svc = self.services.clone();
        let path = path.to_string();
        cx.spawn_in(window, async move |this, cx| {
            load.await;
            let dest = this
                .update(cx, |o, _| {
                    let base = path
                        .rsplit('/')
                        .next()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("copy");
                    join_path(
                        &parent,
                        &unique_sibling_name(&o.siblings_of(&parent), base, dir),
                    )
                })
                .unwrap_or_default();
            if dest.is_empty() {
                return;
            }
            if call(
                cx,
                svc.code.copy(&pid, &wid, &path, &dest),
                "Duplicate failed",
            )
            .await
            .is_none()
            {
                return;
            }
            if let Ok(r) = this.update(cx, |o, cx| o.soft_refresh(vec![parent.clone()], cx)) {
                r.await;
            }
            cx.update(|_, cx| toast(ToastKind::Success, "Duplicated", cx))
                .ok();
        })
        .detach();
    }

    pub(crate) fn copy_rel_path(&mut self, path: &str, cx: &mut Context<Self>) {
        self.ctx_menu = None;
        if path.is_empty() {
            return;
        }
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(path.to_string()));
        toast(ToastKind::Info, "Copied path", cx);
        cx.notify();
    }

    pub(crate) fn copy_abs_path(&mut self, path: &str, cx: &mut Context<Self>) {
        self.ctx_menu = None;
        if path.is_empty() {
            return;
        }
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        let svc = self.services.clone();
        let path = path.to_string();
        cx.spawn(async move |_, cx| {
            let Some(abs) = call(cx, svc.code.abs_path(&pid, &wid, &path), "Path failed").await
            else {
                return;
            };
            cx.update(|cx| {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(abs));
                toast(ToastKind::Info, "Copied absolute path", cx);
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn reveal_path(&mut self, path: &str, cx: &mut Context<Self>) {
        self.ctx_menu = None;
        if path.is_empty() {
            return;
        }
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        let svc = self.services.clone();
        let path = path.to_string();
        cx.spawn(async move |_, cx| {
            let _ = call(
                cx,
                svc.code.reveal_in_finder(&pid, &wid, &path),
                "Reveal failed",
            )
            .await;
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn open_external(&mut self, path: &str, cx: &mut Context<Self>) {
        self.ctx_menu = None;
        if path.is_empty() {
            return;
        }
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        let svc = self.services.clone();
        let path = path.to_string();
        cx.spawn(async move |_, cx| {
            let _ = call(
                cx,
                svc.code.open_in_editor(&pid, &wid, &path, 0),
                "Open failed",
            )
            .await;
        })
        .detach();
        cx.notify();
    }

    // ---- drag and drop ----

    /// `treeDropTarget`: a folder row takes the drop; a file row means its folder.
    pub(crate) fn tree_drop_target(&self, row_path: Option<(&str, bool)>) -> Option<String> {
        if !self.tree_writable() {
            return None;
        }
        Some(match row_path {
            None => String::new(),
            Some((p, true)) => p.to_string(),
            Some((p, false)) => parent_path(p).to_string(),
        })
    }

    /// `onTreeDrop`: move each dragged path into the folder (never into itself).
    pub(crate) fn on_tree_drop(
        &mut self,
        dir: String,
        srcs: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tree_drop_dir = None;
        let this = cx.entity().downgrade();
        cx.spawn_in(window, async move |_, cx| {
            for src in srcs {
                let src = src.trim_end_matches('/').to_string();
                let dest = join_path(&dir, base_name(&src));
                if dest.is_empty() || dest == src || dir == src || is_under(&dir, &src) {
                    continue;
                }
                let moved = this
                    .update_in(cx, |o, window, cx| o.move_path(&src, &dest, window, cx))
                    .ok();
                if let Some(m) = moved {
                    m.await;
                }
            }
        })
        .detach();
        cx.notify();
    }

    // ---- keyboard ----

    /// `onFilesKeydown`: the explorer's TextMate / Finder-first shortcuts. Returns whether the
    /// key was handled.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn on_files_key(
        &mut self,
        key: &str,
        cmd: bool,
        alt: bool,
        shift: bool,
        typing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // ⇧⌘N is always consumed here so the app's blank workspace cannot steal it.
        if cmd && shift && !alt && key == "n" {
            if self.draft_edit.is_none()
                && self.ctx_menu.is_none()
                && !typing
                && self.tree_writable()
            {
                let dir = self.action_dir(cx);
                self.start_new(true, &dir, window, cx);
            }
            return true;
        }
        if self.draft_edit.is_some() || self.ctx_menu.is_some() || typing {
            return false;
        }
        let path = if self.tree_path.is_empty() {
            self.selected_path.clone()
        } else {
            self.tree_path.clone()
        };
        let entry = || -> Option<TreeEntry> {
            if path.is_empty() {
                None
            } else {
                find_entry(&self.active_tree(cx), &path).cloned()
            }
        };
        let w = self.tree_writable();
        if cmd && !alt && !shift && key == "down" && !path.is_empty() {
            let e = entry();
            if !e.as_ref().is_some_and(|e| e.dir) {
                let st = e
                    .map(|e| self.entry_status(&e, &self.dir_status()))
                    .unwrap_or_default();
                let prefer = self.filter_changed() || st == "d";
                self.open_path(OpenRequest::new(path).prefer_diff(prefer), window, cx)
                    .detach();
                return true;
            }
        }
        if cmd && alt && !shift && key == "n" && w {
            let dir = self.action_dir(cx);
            self.start_new(false, &dir, window, cx);
            return true;
        }
        if cmd && alt && !shift && key == "c" && !path.is_empty() {
            self.copy_abs_path(&path, cx);
            return true;
        }
        if cmd && alt && shift && key == "c" && !path.is_empty() {
            self.copy_rel_path(&path, cx);
            return true;
        }
        // ⌥⌘R is always consumed while Files has focus (it is also Toggle Review).
        if cmd && alt && !shift && key == "r" {
            if !path.is_empty() {
                self.reveal_path(&path, cx);
            }
            return true;
        }
        if cmd && !alt && !shift && key == "x" && !path.is_empty() && w {
            let dir = entry().is_some_and(|e| e.dir);
            self.clip_cut(&path, dir, cx);
            return true;
        }
        if cmd && !alt && !shift && key == "c" && !path.is_empty() {
            let dir = entry().is_some_and(|e| e.dir);
            self.clip_copy(&path, dir, cx);
            return true;
        }
        if cmd && !alt && !shift && key == "v" && w && self.file_clip.is_some() {
            let into = if entry().is_some_and(|e| e.dir) {
                path.clone()
            } else {
                parent_path(&path).to_string()
            };
            self.clip_paste(&into, window, cx);
            return true;
        }
        if cmd && !alt && !shift && key == "d" && !path.is_empty() && w {
            let dir = entry().is_some_and(|e| e.dir);
            self.duplicate_path(&path, dir, window, cx);
            return true;
        }
        if !cmd && !alt && !shift && (key == "enter" || key == "f2") && !path.is_empty() && w {
            let e = entry();
            let name = e
                .as_ref()
                .map(|e| e.name.clone())
                .unwrap_or_else(|| base_name(&path).to_string());
            self.start_rename(&path, &name, e.is_some_and(|e| e.dir), window, cx);
            return true;
        }
        if cmd && !alt && !shift && (key == "backspace" || key == "delete") && !path.is_empty() && w
        {
            let name = entry().map(|e| e.name).unwrap_or_else(|| path.clone());
            self.delete_path(&path, &name, window, cx);
            return true;
        }
        false
    }
}

/// `withFilesScroll`-free spawn helper: await a task from another task's continuation.
#[cfg(test)]
mod tests {
    use super::*;

    fn e(path: &str, dir: bool, kids: Vec<TreeEntry>) -> TreeEntry {
        TreeEntry {
            name: base_name(path).into(),
            path: path.into(),
            dir,
            children: kids,
            ..Default::default()
        }
    }

    #[test]
    fn patch_and_merge_keep_loaded_grandchildren() {
        let tree = vec![e(
            "a",
            true,
            vec![e("a/b", true, vec![e("a/b/c.go", false, vec![])])],
        )];
        let fresh = vec![e("a/b", true, vec![]), e("a/z.go", false, vec![])];
        let next = patch_children(&tree, "a", &fresh);
        assert_eq!(next[0].children.len(), 2);
        assert_eq!(next[0].children[0].children[0].path, "a/b/c.go");
        assert!(find_entry(&next, "a/z.go").is_some());
    }

    #[test]
    fn unique_names_keep_extensions() {
        let kids = vec![
            e("x.go", false, vec![]),
            e("x 2.go", false, vec![]),
            e("d", true, vec![]),
        ];
        assert_eq!(unique_sibling_name(&kids, "y.go", false), "y.go");
        assert_eq!(unique_sibling_name(&kids, "x.go", false), "x 3.go");
        assert_eq!(unique_sibling_name(&kids, "d", true), "d 2");
        assert_eq!(
            unique_sibling_name(&[e(".env", false, vec![])], ".env", false),
            ".env 2"
        );
    }

    #[test]
    fn ancestors_shallowest_first() {
        let paths = vec![
            "a/b/c.go".to_string(),
            "a/d.go".to_string(),
            "x/y/z".to_string(),
        ];
        assert_eq!(ancestor_dirs_of(&paths), ["a", "x", "a/b", "x/y"]);
    }

    #[test]
    fn remap_moves_children_and_drops_replaced() {
        let mut m = BTreeMap::new();
        m.insert("a/x".to_string(), 1);
        m.insert("b/x".to_string(), 2);
        m.insert("c".to_string(), 3);
        let next = remap_keyed(&m, "a", "b");
        assert_eq!(next.get("b/x"), Some(&1));
        assert_eq!(next.get("c"), Some(&3));
        assert_eq!(next.len(), 2);
    }

    #[test]
    fn dir_status_takes_the_worst_child() {
        let mut st = BTreeMap::new();
        st.insert("a/b/c".to_string(), "m".to_string());
        st.insert("a/d".to_string(), "a".to_string());
        let idx = dir_status_index(&st);
        assert_eq!(idx.get("a").map(String::as_str), Some("a"));
        assert_eq!(idx.get("a/b").map(String::as_str), Some("m"));
    }

    #[test]
    fn exclude_prefs_default_and_saved() {
        let mut s = LocalStorage::in_memory();
        assert_eq!(explorer_exclude(&s).len(), 5);
        s.set(EXPLORER_PREFS_KEY, r#"{"exclude":["**/dist",""]}"#);
        assert_eq!(explorer_exclude(&s), ["**/dist", ""]);
    }
}
