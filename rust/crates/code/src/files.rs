use std::path::Path;

use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::gostr;
use crate::root::{path_str, rel_slash_str, skip_dir, skip_file};
use crate::walk::{Flow, walk_dir};

const MAX_LIST_FILES: usize = 20_000;

/// Repo-relative forward-slash paths of every file under `root` (Quick Open's
/// walk fallback), skipping heavy/vendor dirs and binary/noise files, sorted.
/// Cancelling stops the walk.
pub fn list_files(cancel: &CancellationToken, root: &Path) -> Result<Vec<String>> {
    let root = path_str(root);
    let mut out = Vec::new();
    walk_dir(&root, |path, d, err| {
        if out.len() >= MAX_LIST_FILES {
            return Ok(Flow::SkipAll);
        }
        let (Some(d), None) = (d, err) else {
            return Ok(Flow::Continue);
        };
        if cancel.is_cancelled() {
            return Err(Error::Canceled);
        }
        if d.is_dir() {
            if path != root && skip_dir(&d.name) {
                return Ok(Flow::SkipDir);
            }
            return Ok(Flow::Continue);
        }
        if !skip_file(&d.name) {
            out.push(rel_slash_str(&root, path));
        }
        Ok(Flow::Continue)
    })?;
    out.sort();
    Ok(out)
}

/// Filters paths by a simple fuzzy query (substring, then subsequence).
pub fn match_files(paths: &[String], query: &str, limit: usize) -> Vec<String> {
    let q = gostr::lower(query);
    let q = q.trim();
    let limit = if limit == 0 { 50 } else { limit };
    if q.is_empty() {
        return paths.iter().take(limit).cloned().collect();
    }
    paths
        .iter()
        .filter(|p| fuzzy_match(gostr::lower(p).as_bytes(), q.as_bytes()))
        .take(limit)
        .cloned()
        .collect()
}

fn fuzzy_match(path: &[u8], query: &[u8]) -> bool {
    if path.windows(query.len()).any(|w| w == query) {
        return true;
    }
    let mut qi = 0;
    for &c in path {
        if qi < query.len() && c == query[qi] {
            qi += 1;
        }
    }
    qi == query.len()
}
