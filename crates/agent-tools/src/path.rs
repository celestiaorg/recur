//! Worktree sandbox for the coding tools. Paths are UTF-8 strings.

use std::fs;

use recur_domain::path::{base, dir, eval_symlinks, is_abs, join, rel};

const ESCAPES: &str = "path escapes worktree";

/// Resolves `p` (relative to the worktree, or absolute) to a canonical path
/// inside the worktree. A missing leaf is resolved through its nearest
/// existing ancestor, so a symlink cannot smuggle a write outside it.
pub fn resolve_path(worktree: &str, p: &str) -> Result<String, String> {
    let wt = abs_eval(worktree.trim()).map_err(|e| format!("worktree: {e}"))?;
    let p = p.trim();
    if p.is_empty() {
        return Err("path is required".into());
    }
    let abs = if is_abs(p) {
        abs_eval_existing(p)?
    } else {
        abs_eval_existing(&safe_join(&wt, p)?)?
    };
    match rel(&wt, &abs) {
        Some(r) if r != ".." && !r.starts_with("../") => Ok(abs),
        _ => Err(ESCAPES.into()),
    }
}

/// Joins a slash path under root, refusing any `..`.
pub fn safe_join(root: &str, rel_path: &str) -> Result<String, String> {
    let root_abs = abs(root)?;
    let rel_path = rel_path.strip_prefix('/').unwrap_or(rel_path);
    if rel_path == "." || rel_path.is_empty() {
        return Ok(root_abs);
    }
    if rel_path.contains("..") {
        return Err("path escapes root".into());
    }
    let full = abs(&join(&[&root_abs, rel_path]))?;
    if full != root_abs && !full.starts_with(&format!("{root_abs}/")) {
        return Err("path escapes root".into());
    }
    Ok(full)
}

pub(crate) fn abs(p: &str) -> Result<String, String> {
    recur_domain::path::abs(p).map_err(|e| e.to_string())
}

pub(crate) fn abs_eval(p: &str) -> Result<String, String> {
    let a = abs(p)?;
    Ok(eval_symlinks(&a).unwrap_or(a))
}

/// Canonicalizes p, walking up to the nearest existing ancestor so a missing
/// leaf under a symlink cannot fail open to a lexical in-worktree path that
/// `mkdir_all` would follow outside the sandbox.
fn abs_eval_existing(p: &str) -> Result<String, String> {
    let a = abs(p)?;
    if let Some(eval) = eval_symlinks(&a) {
        return Ok(eval);
    }
    let mut rest: Vec<String> = Vec::new();
    let mut cur = a.clone();
    loop {
        if fs::symlink_metadata(&cur).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(ESCAPES.into());
        }
        let parent = dir(&cur);
        if parent == cur {
            if rest.is_empty() {
                return Ok(a);
            }
            return Ok(join_owned(&cur, &rest));
        }
        rest.insert(0, base(&cur));
        if let Some(eval) = eval_symlinks(&parent) {
            return Ok(join_owned(&eval, &rest));
        }
        cur = parent;
    }
}

fn join_owned(first: &str, rest: &[String]) -> String {
    let mut parts: Vec<&str> = vec![first];
    parts.extend(rest.iter().map(String::as_str));
    join(&parts)
}

pub(crate) fn mkdir_all(path: &str) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("{path}: {e}"))
}

pub(crate) fn read_file(path: &str) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("{path}: {e}"))
}

/// Writes `data` to `path` with mode 0o644.
pub(crate) fn write_file(path: &str, data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .open(path)
        .map_err(|e| format!("{path}: {e}"))?;
    f.write_all(data).map_err(|e| format!("{path}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of path_test.go TestResolvePathSandbox.
    #[test]
    fn resolve_path_sandbox() {
        let wt = tempfile::tempdir().unwrap();
        let wt_s = wt.path().to_str().unwrap();
        fs::write(wt.path().join("in.txt"), "ok").unwrap();
        let got = resolve_path(wt_s, "in.txt").unwrap();
        assert!(got.ends_with("in.txt"), "{got}");
        assert!(resolve_path(wt_s, "../nope").is_err());
        let outside = tempfile::tempdir().unwrap();
        let out_file = outside.path().join("x.txt");
        assert!(resolve_path(wt_s, out_file.to_str().unwrap()).is_err());
        std::os::unix::fs::symlink(outside.path(), wt.path().join("link")).unwrap();
        assert!(resolve_path(wt_s, "link/newdir/file.txt").is_err());
    }

    #[test]
    fn eval_symlinks_resolves_relative_links() {
        let wt = tempfile::tempdir().unwrap();
        let root = eval_symlinks(wt.path().to_str().unwrap()).unwrap();
        fs::create_dir_all(format!("{root}/sub/deep")).unwrap();
        std::os::unix::fs::symlink("sub/deep", format!("{root}/l")).unwrap();
        assert_eq!(
            eval_symlinks(&format!("{root}/l/../x")),
            None,
            "missing leaf after .. fails"
        );
        assert_eq!(
            eval_symlinks(&format!("{root}/l/..")),
            Some(format!("{root}/sub"))
        );
        assert_eq!(
            eval_symlinks(&format!("{root}/l")),
            Some(format!("{root}/sub/deep"))
        );
    }
}
