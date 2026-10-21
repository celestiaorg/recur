//! Copies glob-matched files from a project's checkout into a new workspace
//! worktree (untracked dotenv, secrets, local config).

use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::time::{Duration, Instant};

use recur_domain::CopyFilesPreview;
use regex::Regex;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::glob::glob_to_regexp;
use crate::gopath;
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

/// Cancellation plus an optional deadline (Go's `context.Context`).
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
                    "Scan timed out — narrow the globs".into()
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
/// worktree keeps its own copies). Per-file failures (unreadable, oversized,
/// unjoinable) are logged and skipped, never failing workspace create. Errors
/// when the walk fails, the match cap is exceeded, `cancel` fires or
/// `deadline` passes, or the patterns use unsupported negation. Returns the
/// number of files handled.
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
    let src_abs = gopath::abs(&src).map_err(|e| Error::msg(e.to_string()))?;
    let dest_abs = gopath::abs(dest).map_err(|e| Error::msg(e.to_string()))?;
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
            return Err(Error::path("lstat", &to, e).into());
        }
        Err(_) => {}
    }
    let info = fs::symlink_metadata(&from).map_err(|e| Error::path("lstat", &from, e))?;
    if !info.file_type().is_file() {
        return Ok(());
    }
    if info.len() > MAX_COPY_FILE_BYTES {
        return Err(CopyErr::Other(format!(
            "file larger than {MAX_COPY_FILE_BYTES} bytes"
        )));
    }
    let mut input = File::open(&from).map_err(|e| Error::path("open", &from, e))?;
    gopath::mkdir_all(&gopath::dir(&to))?;
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(info.permissions().mode() & 0o777)
        .open(&to)
        .map_err(|e| Error::path("open", &to, e))?;
    io::copy(&mut input, &mut out).map_err(|e| Error::path("write", &to, e))?;
    Ok(())
}

/// Joins `root` and `rel` under `root`. Unlike the editor's sandbox it allows
/// `..` inside a file name (`.env..bak`) and rejects it only as a component.
fn join_under_root(root: &str, rel: &str) -> Result<String> {
    let root_abs = gopath::abs(root).map_err(|e| Error::msg(e.to_string()))?;
    let rel = rel.strip_prefix('/').unwrap_or(rel);
    if rel.is_empty() || rel == "." {
        return Err(Error::msg("empty path"));
    }
    if rel.split('/').any(|p| p == "..") {
        return Err(Error::msg("path escapes root"));
    }
    let full = gopath::abs(&gopath::join(&[&root_abs, rel])).map_err(|e| Error::msg(e.to_string()))?;
    if full != root_abs && !full.starts_with(&format!("{root_abs}/")) {
        return Err(Error::msg("path escapes root"));
    }
    Ok(full)
}

fn list(ctx: &Ctx<'_>, root: &str, patterns: &[String], limit: usize) -> Result<Vec<String>> {
    if let Some(e) = ctx.err() {
        return Err(e);
    }
    let root_abs = gopath::abs(root).map_err(|e| Error::msg(e.to_string()))?;
    let st = fs::metadata(&root_abs).map_err(|e| Error::path("stat", &root_abs, e))?;
    if !st.is_dir() {
        return Err(Error::msg(format!("{root_abs} is not a directory")));
    }
    let m = Matcher::new(patterns);
    let mut out = Vec::new();
    walk_dir(&root_abs, |path, d, err| {
        if let Some(e) = ctx.err() {
            return Err(e);
        }
        let (Some(d), None) = (d, err) else {
            return Ok(Flow::Continue);
        };
        if d.is_dir() {
            if path != root_abs && skip_walk_dir(&d.name, patterns) {
                return Ok(Flow::SkipDir);
            }
            return Ok(Flow::Continue);
        }
        if d.is_symlink() {
            return Ok(Flow::Continue);
        }
        let rel = rel_slash_str(&root_abs, path);
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
