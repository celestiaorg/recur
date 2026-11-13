//! Copies glob-matched files from a project's checkout into a new workspace
//! worktree (untracked dotenv, secrets, local config). A pattern ending in
//! `/` copies matching directories whole (`node_modules/`, `target/`).

use std::fs;
use std::io;
use std::time::{Duration, Instant};

use recur_domain::CopyFilesPreview;
use recur_domain::path;
use regex::Regex;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::glob::glob_to_regexp;
use crate::root::rel_slash_str;
use crate::walk::{Flow, walk_dir};

const MAX_COPY_FILES: usize = 10_000;
const MAX_PREVIEW_FILES: usize = 250;
const MAX_COPY_FILE_BYTES: u64 = 32 << 20;
const PREVIEW_TIMEOUT: Duration = Duration::from_secs(15);

const ERR_NEGATION: &str = "negation (!) is not supported";

fn err_too_many(limit: usize) -> String {
    format!("more than {limit} matching files; narrow the globs")
}

/// Splits the CopyFiles text into globs. Blank lines, `#` comments and `!`
/// lines are dropped; backslashes become slashes; leading slashes go.
pub fn parse_patterns(text: &str) -> Vec<String> {
    text.split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('!'))
        .map(|l| l.replace('\\', "/").trim_start_matches('/').to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn has_negation(text: &str) -> bool {
    text.split('\n').any(|l| l.trim().starts_with('!'))
}

/// Whether a repo-relative path matches any pattern. A pattern without a
/// slash matches at any depth (`.env*` → `apps/web/.env.local`).
pub fn matches(rel: &str, patterns: &[String]) -> bool {
    let rel = rel.replace('\\', "/");
    let rel = rel.trim_start_matches('/');
    if rel.is_empty() || rel == "." {
        return false;
    }
    Matcher::new(patterns).matches(rel)
}

/// Cancellation plus an optional deadline.
struct Ctx<'a> {
    cancel: &'a CancellationToken,
    deadline: Option<Instant>,
}

impl Ctx<'_> {
    fn err(&self) -> Option<Error> {
        if self.cancel.is_cancelled() {
            return Some(Error::Canceled);
        }
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            return Some(Error::DeadlineExceeded);
        }
        None
    }
}

/// The files under `root` that match `patterns`, for the settings page. A
/// missing root or no patterns is an empty preview. Bounded by `cancel` and
/// `deadline` (15 s from now when `None`).
pub fn preview(
    cancel: &CancellationToken,
    deadline: Option<Instant>,
    root: &str,
    patterns: &str,
) -> CopyFilesPreview {
    preview_capped(cancel, deadline, root, patterns, MAX_COPY_FILES)
}

fn preview_capped(
    cancel: &CancellationToken,
    deadline: Option<Instant>,
    root: &str,
    patterns: &str,
    scan_limit: usize,
) -> CopyFilesPreview {
    let ctx = Ctx {
        cancel,
        deadline: Some(deadline.unwrap_or_else(|| Instant::now() + PREVIEW_TIMEOUT)),
    };
    let root = recur_git::expand_home(root).trim().to_string();
    let mut out = CopyFilesPreview {
        root: root.clone(),
        ..Default::default()
    };
    if has_negation(patterns) {
        out.error = ERR_NEGATION.into();
        return out;
    }
    let pats = parse_patterns(patterns);
    if root.is_empty() || pats.is_empty() {
        return out;
    }
    let mut files = match list(&ctx, &root, &pats, scan_limit + 1) {
        Ok(files) => files,
        Err(e) => {
            out.error = match e {
                Error::Canceled | Error::DeadlineExceeded => {
                    "Scan timed out: narrow the globs".into()
                }
                e if e.is_not_exist() => format!("Checkout not found at {root}"),
                e => e.to_string(),
            };
            return out;
        }
    };
    if files.len() > scan_limit {
        out.error = err_too_many(scan_limit);
        return out;
    }
    out.count = files.len() as i64;
    if files.len() > MAX_PREVIEW_FILES {
        out.truncated = true;
        files.truncate(MAX_PREVIEW_FILES);
    }
    out.files = Some(files);
    out
}

/// Writes matching files from `src` into `dest`, keeping relative paths and
/// modes and creating directories. Existing dest files stay (a restored
/// worktree keeps its own copies). A directory pattern (`node_modules/`)
/// copies each matching directory whole, symlinks included, past the caps.
/// Per-file failures (unreadable, oversized, unjoinable) are logged and
/// skipped, never failing workspace create. Errors when the walk fails, the
/// match cap is exceeded, `cancel` fires or `deadline` passes, or the patterns
/// use unsupported negation. Returns the number of new files and directories.
pub fn copy(
    cancel: &CancellationToken,
    deadline: Option<Instant>,
    src: &str,
    dest: &str,
    patterns: &str,
) -> Result<usize> {
    copy_capped(cancel, deadline, src, dest, patterns, MAX_COPY_FILES)
}

fn copy_capped(
    cancel: &CancellationToken,
    deadline: Option<Instant>,
    src: &str,
    dest: &str,
    patterns: &str,
    scan_limit: usize,
) -> Result<usize> {
    let ctx = Ctx { cancel, deadline };
    let src = recur_git::expand_home(src).trim().to_string();
    let dest = dest.trim();
    if has_negation(patterns) {
        return Err(Error::msg(ERR_NEGATION));
    }
    let pats = parse_patterns(patterns);
    if src.is_empty() || dest.is_empty() || pats.is_empty() {
        return Ok(0);
    }
    let src_abs = path::abs(&src).map_err(|e| Error::msg(e.to_string()))?;
    let dest_abs = path::abs(dest).map_err(|e| Error::msg(e.to_string()))?;
    if src_abs == dest_abs {
        return Ok(0);
    }
    let files = list(&ctx, &src_abs, &pats, scan_limit + 1)?;
    if files.len() > scan_limit {
        return Err(Error::msg(err_too_many(scan_limit)));
    }
    let mut n = 0;
    for rel in &files {
        if let Some(e) = ctx.err() {
            return Err(e);
        }
        if let Some(dir) = rel.strip_suffix('/') {
            match copy_dir(&ctx, &src_abs, &dest_abs, dir) {
                Ok(new) => n += usize::from(new),
                Err(e @ (Error::Canceled | Error::DeadlineExceeded)) => return Err(e),
                Err(e) => tracing::warn!("recur: copy files: skip {rel}: {e}"),
            }
            continue;
        }
        match copy_one(&src_abs, &dest_abs, rel) {
            Ok(()) => n += 1,
            Err(CopyErr::Exists) => {}
            Err(CopyErr::Other(e)) => tracing::warn!("recur: copy files: skip {rel}: {e}"),
        }
    }
    Ok(n)
}

enum CopyErr {
    Exists,
    Other(String),
}

impl From<Error> for CopyErr {
    fn from(e: Error) -> Self {
        Self::Other(e.to_string())
    }
}

fn copy_one(src_root: &str, dest_root: &str, rel: &str) -> std::result::Result<(), CopyErr> {
    let from = join_under_root(src_root, rel)?;
    let to = join_under_root(dest_root, rel)?;
    match fs::symlink_metadata(&to) {
        Ok(_) => return Err(CopyErr::Exists),
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            return Err(Error::path(&to, e).into());
        }
        Err(_) => {}
    }
    let info = fs::symlink_metadata(&from).map_err(|e| Error::path(&from, e))?;
    if !info.file_type().is_file() {
        return Ok(());
    }
    if info.len() > MAX_COPY_FILE_BYTES {
        return Err(CopyErr::Other(format!(
            "file larger than {MAX_COPY_FILE_BYTES} bytes"
        )));
    }
    Error::mkdir_all(&path::dir(&to))?;
    // std clones on APFS and uses copy_file_range (a reflink on btrfs/XFS).
    fs::copy(&from, &to).map_err(|e| Error::path(&to, e))?;
    Ok(())
}

/// Copies the directory `rel` whole via [`copy_tree`]. Returns whether the
/// destination was new (an existing one is merged into and not counted).
fn copy_dir(ctx: &Ctx<'_>, src_root: &str, dest_root: &str, rel: &str) -> Result<bool> {
    let from = join_under_root(src_root, rel)?;
    let to = join_under_root(dest_root, rel)?;
    let new = match fs::symlink_metadata(&to) {
        Ok(_) => false,
        Err(e) if e.kind() == io::ErrorKind::NotFound => true,
        Err(e) => return Err(Error::path(&to, e)),
    };
    copy_tree(ctx, &from, &to)?;
    Ok(new)
}

/// Merges the tree at `from` into `to`: missing directories, symlinks and
/// regular files are created, existing entries stay. Per-entry failures are
/// logged and skipped; only `ctx` stops it.
fn copy_tree(ctx: &Ctx<'_>, from: &str, to: &str) -> Result<()> {
    walk_dir(from, |p, d, err| {
        if let Some(e) = ctx.err() {
            return Err(e);
        }
        let rel = rel_slash_str(from, p);
        let dst = if rel == "." {
            to.to_string()
        } else {
            format!("{to}/{rel}")
        };
        let res = match (d, err) {
            (_, Some(e)) => Err(e),
            (None, None) => Ok(()),
            (Some(d), None) if d.is_dir() => fs::create_dir_all(&dst),
            (Some(_), None) if fs::symlink_metadata(&dst).is_ok() => Ok(()),
            (Some(d), None) if d.is_symlink() => {
                fs::read_link(p).and_then(|t| std::os::unix::fs::symlink(t, &dst))
            }
            (Some(d), None) if d.is_file() => fs::copy(p, &dst).map(|_| ()),
            // Sockets, FIFOs, devices.
            (Some(_), None) => Ok(()),
        };
        Ok(match res {
            Ok(()) => Flow::Continue,
            Err(e) => {
                tracing::warn!("recur: copy files: skip {dst}: {e}");
                // SkipDir on a file would skip its siblings too.
                if d.is_some_and(|d| d.is_dir()) {
                    Flow::SkipDir
                } else {
                    Flow::Continue
                }
            }
        })
    })
}

/// Joins `root` and `rel` under `root`. Unlike the editor's sandbox it allows
/// `..` inside a file name (`.env..bak`) and rejects it only as a component.
fn join_under_root(root: &str, rel: &str) -> Result<String> {
    let root_abs = path::abs(root).map_err(|e| Error::msg(e.to_string()))?;
    let rel = rel.strip_prefix('/').unwrap_or(rel);
    if rel.is_empty() || rel == "." {
        return Err(Error::msg("empty path"));
    }
    if rel.split('/').any(|p| p == "..") {
        return Err(Error::msg("path escapes root"));
    }
    let full = path::abs(&path::join(&[&root_abs, rel])).map_err(|e| Error::msg(e.to_string()))?;
    if full != root_abs && !full.starts_with(&format!("{root_abs}/")) {
        return Err(Error::msg("path escapes root"));
    }
    Ok(full)
}

fn list(ctx: &Ctx<'_>, root: &str, patterns: &[String], limit: usize) -> Result<Vec<String>> {
    if let Some(e) = ctx.err() {
        return Err(e);
    }
    let root_abs = path::abs(root).map_err(|e| Error::msg(e.to_string()))?;
    let st = fs::metadata(&root_abs).map_err(|e| Error::path(&root_abs, e))?;
    if !st.is_dir() {
        return Err(Error::msg(format!("{root_abs} is not a directory")));
    }
    let m = Matcher::new(patterns);
    // `node_modules/` also matches the directory itself, listed as `rel/`.
    let dir_pats: Vec<String> = patterns
        .iter()
        .filter_map(|p| p.strip_suffix('/'))
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    let dirs = Matcher::new(&dir_pats);
    let mut out = Vec::new();
    walk_dir(&root_abs, |path, d, err| {
        if let Some(e) = ctx.err() {
            return Err(e);
        }
        let (Some(d), None) = (d, err) else {
            return Ok(Flow::Continue);
        };
        if d.is_dir() && path == root_abs {
            return Ok(Flow::Continue);
        }
        let rel = rel_slash_str(&root_abs, path);
        if d.is_dir() {
            if d.name == ".git" || !dirs.matches(&rel) {
                return Ok(if skip_walk_dir(&d.name, patterns) {
                    Flow::SkipDir
                } else {
                    Flow::Continue
                });
            }
            out.push(format!("{rel}/"));
            return Ok(if out.len() >= limit {
                Flow::SkipAll
            } else {
                Flow::SkipDir
            });
        }
        if d.is_symlink() {
            return Ok(Flow::Continue);
        }
        if rel.is_empty() || rel == "." || rel.starts_with("../") || !m.matches(&rel) {
            return Ok(Flow::Continue);
        }
        out.push(rel);
        Ok(if out.len() >= limit {
            Flow::SkipAll
        } else {
            Flow::Continue
        })
    })?;
    out.sort();
    Ok(out)
}

struct Matcher(Vec<Regex>);

impl Matcher {
    fn new(patterns: &[String]) -> Self {
        Self(
            patterns
                .iter()
                .map(|p| {
                    let src = glob_to_regexp(&expand_pattern(p));
                    match Regex::new(&src) {
                        Ok(re) => re,
                        Err(e) => panic!("regexp: Compile({src}): {e}"),
                    }
                })
                .collect(),
        )
    }

    fn matches(&self, rel: &str) -> bool {
        self.0.iter().any(|re| re.is_match(rel))
    }
}

/// Gitignore-style globs as path-anchored globs: one with no slash (besides a
/// trailing one) matches at any depth, and a trailing slash means the whole
/// directory (`secrets/` → `**/secrets/**`).
fn expand_pattern(p: &str) -> String {
    let any_depth = !p.strip_suffix('/').unwrap_or(p).contains('/');
    let p = if p.ends_with('/') {
        format!("{p}**")
    } else {
        p.to_string()
    };
    if any_depth { format!("**/{p}") } else { p }
}

/// Always prunes `.git`, and well-known heavy dirs unless a pattern names
/// them or is a catch-all.
fn skip_walk_dir(name: &str, patterns: &[String]) -> bool {
    if name == ".git" {
        return true;
    }
    if !HEAVY_DIRS.contains(&name) {
        return false;
    }
    !patterns
        .iter()
        .any(|p| p == "**" || p == "**/*" || p == "*" || p.contains(name))
}

const HEAVY_DIRS: &[&str] = &[
    "node_modules",
    "dist",
    "build",
    "vendor",
    ".next",
    "coverage",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".tox",
    ".venv",
    "venv",
    ".turbo",
    ".cache",
    "target",
    "Pods",
    ".gradle",
    ".idea",
    "out",
    "obj",
];

#[cfg(test)]
#[path = "copyfiles_tests.rs"]
mod tests;
