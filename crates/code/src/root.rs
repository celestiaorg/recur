//! Worktree roots and the sandbox every path goes through, so the UI cannot
//! escape the project repo or loop worktree it browses.

use std::fs;
use std::path::{Path, PathBuf};

use recur_domain::path;

use crate::error::{Error, Result};

/// The absolute root to browse for a project/loop pair. A non-empty
/// `loop_worktree` that exists wins (loop-scoped observatory); one that is
/// missing is [`Error::WorktreeMissing`], never the project checkout. An empty
/// `loop_worktree` uses the project's repo path.
pub fn resolve_root(repo_path: &str, loop_worktree: &str) -> Result<PathBuf> {
    resolve_root_info(repo_path, loop_worktree).map(|(root, _)| root)
}

/// [`resolve_root`] plus whether the root it picked is the loop worktree
/// (which decides whether editing is allowed).
pub fn resolve_root_info(repo_path: &str, loop_worktree: &str) -> Result<(PathBuf, bool)> {
    let wt = loop_worktree.trim();
    if !wt.is_empty() {
        let abs = path::abs(&recur_git::expand_home(wt)).map_err(|e| Error::msg(e.to_string()))?;
        if fs::metadata(&abs).is_ok_and(|m| m.is_dir()) {
            return Ok((PathBuf::from(abs), true));
        }
        return Err(Error::WorktreeMissing(wt.to_string()));
    }
    let repo = repo_path.trim();
    if repo.is_empty() {
        return Err(Error::msg("no repository path"));
    }
    let abs = path::abs(&recur_git::expand_home(repo)).map_err(|e| Error::msg(e.to_string()))?;
    if !fs::metadata(&abs).is_ok_and(|m| m.is_dir()) {
        return Err(Error::msg(format!(
            "repository path {:?} is not a directory",
            repo
        )));
    }
    Ok((PathBuf::from(abs), false))
}

/// Joins `root` and `rel` and ensures the result stays under `root`. `rel`
/// uses forward slashes; empty means the root itself.
pub fn safe_join(root: &Path, rel: &str) -> Result<PathBuf> {
    safe_join_str(&path_str(root), rel).map(PathBuf::from)
}

pub(crate) fn safe_join_str(root: &str, rel: &str) -> Result<String> {
    let root_abs = path::abs(root).map_err(|e| Error::msg(e.to_string()))?;
    let rel = rel.strip_prefix('/').unwrap_or(rel);
    if rel == "." || rel.is_empty() {
        return Ok(root_abs);
    }
    if rel.contains("..") {
        return Err(Error::msg("path escapes root"));
    }
    let full = path::join(&[&root_abs, rel]);
    let full_abs = path::abs(&full).map_err(|e| Error::msg(e.to_string()))?;
    if full_abs != root_abs && !full_abs.starts_with(&format!("{root_abs}/")) {
        return Err(Error::msg("path escapes root"));
    }
    Ok(full_abs)
}

/// The forward-slash path of `abs` relative to `root`.
pub fn rel_slash(root: &Path, abs: &Path) -> String {
    rel_slash_str(&path_str(root), &path_str(abs))
}

pub(crate) fn rel_slash_str(root: &str, abs: &str) -> String {
    path::rel(root, abs).unwrap_or_else(|| abs.to_string())
}

pub(crate) fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Directories that search, symbol and metrics walks do not descend into.
/// The file tree applies the user's `files.exclude` globs instead.
pub(crate) fn skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "node_modules"
            | "dist"
            | "build"
            | "vendor"
            | ".next"
            | "coverage"
            | "__pycache__"
            | ".pytest_cache"
            | ".mypy_cache"
            | ".tox"
            | ".venv"
            | "venv"
            | ".turbo"
            | ".cache"
            | "bin"
            | "target"
            | "Pods"
            | ".gradle"
            | ".idea"
            | "out"
            | "obj"
    ) || name.starts_with('.')
}

/// Binary and noise files the walks skip, by lowercase extension. Only the
/// last extension counts, so `.min.js` / `.min.css` never hit (the extension is
/// `.js`).
pub(crate) fn skip_file(name: &str) -> bool {
    matches!(
        path::ext(name).to_ascii_lowercase().as_str(),
        ".png"
            | ".jpg"
            | ".jpeg"
            | ".gif"
            | ".webp"
            | ".ico"
            | ".pdf"
            | ".zip"
            | ".gz"
            | ".tar"
            | ".woff"
            | ".woff2"
            | ".ttf"
            | ".eot"
            | ".mp4"
            | ".mov"
            | ".wasm"
            | ".exe"
            | ".dylib"
            | ".so"
            | ".a"
            | ".lock"
            | ".map"
            | ".min.js"
            | ".min.css"
    )
}
