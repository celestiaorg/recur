//! The explorer tree: directory listings annotated with git change status
//! and check-ignore, plus the per-file diff and base content.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;

use recur_domain::{FileDiff, TreeEntry};

use crate::error::{Error, Result};
use crate::file::MAX_EDIT_BYTES;
use crate::gopath;
use crate::gostr;
use crate::root::{path_str, rel_slash_str, safe_join_str};
use crate::walk::read_dir_sorted;

/// Path → `m`/`a`/`d`/`u` for changes at `root`. With `base_branch` (a loop
/// worktree) the status is versus the merge-base with that base, the
/// spin-off point; otherwise porcelain versus HEAD.
pub async fn git_status(root: &Path, base_branch: &str) -> HashMap<String, String> {
    recur_git::change_status(&path_str(root), base_branch)
        .await
        .unwrap_or_default()
}

/// The diff of `rel` versus `base_branch`'s merge-base (or HEAD).
pub async fn file_diff(root: &Path, base_branch: &str, rel: &str) -> Result<FileDiff> {
    Ok(recur_git::working_diff(&path_str(root), base_branch, rel).await?)
}

/// `rel` at the merge-base of `base_branch` and HEAD. Empty when the file
/// did not exist at the spin-off point (added or untracked).
pub async fn file_at_base(root: &Path, base_branch: &str, rel: &str) -> Result<String> {
    let content = recur_git::file_at_base(&path_str(root), base_branch, rel).await?;
    if content.len() > MAX_EDIT_BYTES {
        return Err(Error::msg(format!(
            "file too large ({} bytes)",
            content.len()
        )));
    }
    Ok(String::from_utf8_lossy(&content).into_owned())
}

/// The tree under `root/rel`. Depth 1 (and 0) lists immediate children (the
/// UI expands a directory by listing it); larger depths recurse. A missing
/// `rel` is [`Error::NotFound`] so live-refresh can tell a deletion from a
/// transient failure. Entries carry change status versus `base_branch`.
///
/// Refreshing several directories at once should use [`list_trees`], which
/// shares one status pass.
pub async fn list_tree(
    root: &Path,
    rel: &str,
    depth: i64,
    base_branch: &str,
) -> Result<Vec<TreeEntry>> {
    let root_s = path_str(root);
    let idx = StatusIndex::new(git_status(root, base_branch).await);
    let mut out = list_tree_with(&root_s, rel, depth, &idx)?;
    let ignored = recur_git::ignored_set(&root_s, &collect_tree_paths(&out)).await;
    if let Some(ignored) = ignored.filter(|s| !s.is_empty()) {
        mark_tree_ignored(&mut out, &ignored);
    }
    Ok(out)
}

/// Lists several directories, computing git status and the check-ignore set
/// exactly once each for the batch: the observatory's live refresh re-lists
/// every expanded directory on each coalesced file-system batch, and per
/// directory git status forked three processes per directory per tick.
/// Status is still fresh on every call, never cached across calls.
///
/// Directories that cannot be listed (typically just deleted) are absent from
/// the map rather than failing the batch. Keys are the requested paths with a
/// leading slash dropped and `.` as `""`.
pub async fn list_trees(
    root: &Path,
    rels: &[String],
    depth: i64,
    base_branch: &str,
) -> Result<BTreeMap<String, Vec<TreeEntry>>> {
    let mut out = BTreeMap::new();
    if rels.is_empty() {
        return Ok(out);
    }
    let root_s = path_str(root);
    let idx = StatusIndex::new(git_status(root, base_branch).await);
    let mut seen = HashSet::new();
    for rel in rels {
        let mut key = rel.strip_prefix('/').unwrap_or(rel).to_string();
        if key == "." {
            key.clear();
        }
        if !seen.insert(key.clone()) {
            continue;
        }
        if let Ok(kids) = list_tree_with(&root_s, &key, depth, &idx) {
            out.insert(key, kids);
        }
    }
    let paths: Vec<String> = out.values().flat_map(|e| collect_tree_paths(e)).collect();
    if let Some(ignored) = recur_git::ignored_set(&root_s, &paths)
        .await
        .filter(|s| !s.is_empty())
    {
        for ents in out.values_mut() {
            mark_tree_ignored(ents, &ignored);
        }
    }
    Ok(out)
}

fn list_tree_with(root: &str, rel: &str, depth: i64, idx: &StatusIndex) -> Result<Vec<TreeEntry>> {
    let base = safe_join_str(root, rel)?;
    let depth = if depth <= 0 { 1 } else { depth };
    read_dir(root, &base, depth, idx).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            Error::NotFound(rel.to_string())
        } else if e.raw_os_error() == Some(20) {
            Error::path("readdirent", &base, e)
        } else {
            Error::path("open", &base, e)
        }
    })
}

/// Change status plus the deleted paths indexed by parent directory, so
/// injecting deletions is not O(directories × changed files) per refresh.
struct StatusIndex {
    by_path: HashMap<String, String>,
    /// Parent dir (`""` = repo root) → deleted child paths.
    deleted_by: HashMap<String, Vec<String>>,
}

impl StatusIndex {
    fn new(statuses: HashMap<String, String>) -> Self {
        let mut deleted_by: HashMap<String, Vec<String>> = HashMap::new();
        for (p, st) in &statuses {
            if st != "d" || p.is_empty() {
                continue;
            }
            let mut parent = gopath::dir(p);
            if parent == "." {
                parent.clear();
            }
            deleted_by.entry(parent).or_default().push(p.clone());
        }
        for v in deleted_by.values_mut() {
            v.sort();
        }
        Self {
            by_path: statuses,
            deleted_by,
        }
    }
}

fn collect_tree_paths(ents: &[TreeEntry]) -> Vec<String> {
    let mut paths = Vec::new();
    fn walk(es: &[TreeEntry], paths: &mut Vec<String>) {
        for e in es {
            if !e.path.is_empty() && e.path != "." {
                paths.push(e.path.clone());
            }
            walk(&e.children, paths);
        }
    }
    walk(ents, &mut paths);
    paths
}

fn mark_tree_ignored(ents: &mut [TreeEntry], ignored: &HashSet<String>) {
    for e in ents {
        if ignored.contains(&e.path) {
            e.ignored = true;
        }
        mark_tree_ignored(&mut e.children, ignored);
    }
}

fn read_dir(root: &str, abs: &str, depth: i64, idx: &StatusIndex) -> io::Result<Vec<TreeEntry>> {
    let entries = read_dir_sorted(Path::new(abs))?;
    let mut parent = rel_slash_str(root, abs);
    if parent == "." {
        parent.clear();
    }
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        // Unfiltered: the UI applies files.exclude globs.
        let full = gopath::join(&[abs, &e.name]);
        let is_dir = e.is_dir() || (e.is_symlink() && fs::metadata(&e.path).is_ok_and(|m| m.is_dir()));
        let mut ent = TreeEntry {
            name: e.name.clone(),
            path: rel_slash_str(root, &full),
            dir: is_dir,
            ..Default::default()
        };
        if let Some(st) = idx.by_path.get(&ent.path) {
            ent.status = st.clone();
        }
        seen.insert(ent.path.clone());
        if is_dir
            && depth > 1
            && let Ok(kids) = read_dir(root, &full, depth - 1, idx)
        {
            ent.children = kids;
        }
        out.push(ent);
    }
    // Deleted paths missing from disk, so the explorer still shows them.
    for p in idx.deleted_by.get(&parent).into_iter().flatten() {
        if seen.contains(p) {
            continue;
        }
        out.push(TreeEntry {
            name: gopath::base(p),
            path: p.clone(),
            status: "d".into(),
            ..Default::default()
        });
    }
    out.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| gostr::lower(&a.name).cmp(&gostr::lower(&b.name)))
    });
    Ok(out)
}
