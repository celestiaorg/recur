//! Quick Open / @-mention ranking over the resident file index, and the
//! `files.exclude` matchers baked into the index when it is built.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;
use recur_domain::FileSearchHit;
use recur_domain::path;
use regex::Regex;
use tokio_util::sync::CancellationToken;

use crate::fuzzy::score_lower;
use crate::glob::glob_to_regexp;

/// Scores `paths` for `query` and returns up to `limit` hits (0 means 50),
/// best first: score descending, then path ascending. An empty query returns
/// the first `limit` paths with score 0. The corpus is already
/// exclude-filtered. `cancel` may stop a long scan (short queries walk the
/// whole corpus); the hits so far are returned.
pub fn rank_files(
    cancel: Option<&CancellationToken>,
    paths: &[String],
    basenames: &[String],
    query: &str,
    limit: usize,
) -> Vec<FileSearchHit> {
    let limit = if limit == 0 { 50 } else { limit };
    let q = query.to_lowercase();
    let q = q.trim();
    if q.is_empty() {
        return paths
            .iter()
            .take(limit)
            .map(|p| FileSearchHit {
                path: p.clone(),
                score: 0,
                dir: false,
            })
            .collect();
    }
    // A bounded best-N list instead of collecting every subsequence match
    // and sorting: short queries can hit most of a monorepo.
    let mut out: Vec<FileSearchHit> = Vec::with_capacity(limit);
    for (i, p) in paths.iter().enumerate() {
        if i & 511 == 0 && cancel.is_some_and(|c| c.is_cancelled()) {
            return out;
        }
        let base = match basenames.get(i) {
            Some(b) if !b.is_empty() => b.clone(),
            _ => path::base(p),
        };
        let pl = p.to_lowercase();
        let bl = base.to_lowercase();
        let Some(base_score) = score_lower(pl.as_bytes(), q.as_bytes()) else {
            continue;
        };
        let score = score_file_lower(&pl, &bl, &base, q, base_score);
        if out.len() == limit {
            let worst = &out[limit - 1];
            if score < worst.score || (score == worst.score && *p >= worst.path) {
                continue;
            }
        }
        let at = out
            .partition_point(|prev| prev.score > score || (prev.score == score && prev.path < *p));
        out.insert(
            at,
            FileSearchHit {
                path: p.clone(),
                score,
                dir: false,
            },
        );
        out.truncate(limit);
    }
    out
}

fn score_file_lower(pl: &str, bl: &str, basename: &str, q: &str, base_score: i64) -> i64 {
    let mut score = base_score;
    if bl == q {
        score += 5000;
    } else if bl.starts_with(q) {
        score += 2000;
    } else if bl.contains(q) {
        score += 1200;
    } else if camel_hump_match(basename, q) {
        score += 900;
    } else if word_boundary_match(bl, q) {
        score += 700;
    }
    if pl.contains(&format!("/{q}")) {
        score += 200;
    }
    score + (80 - basename.len() as i64).max(0)
}

/// Repo-relative directories implied by file paths (`src/lib/a.ts` →
/// `src/lib`, `src`), first seen first. Empty directories are absent.
pub(crate) fn unique_parent_dirs(paths: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for p in paths {
        let mut p = p.as_str();
        while let Some(i) = p.rfind('/').filter(|&i| i > 0) {
            p = &p[..i];
            if !seen.insert(p) {
                break;
            }
            out.push(p.to_string());
        }
    }
    out
}

/// [`rank_files`] when `files_only`, otherwise [`rank_files_and_dirs`].
pub fn rank_hits(
    cancel: Option<&CancellationToken>,
    paths: &[String],
    basenames: &[String],
    query: &str,
    limit: usize,
    files_only: bool,
) -> Vec<FileSearchHit> {
    if files_only {
        rank_files(cancel, paths, basenames, query, limit)
    } else {
        rank_files_and_dirs(cancel, paths, basenames, query, limit)
    }
}

/// Ranks files, then their parent directories (trailing `/`, `dir` set), and
/// merges by score.
pub fn rank_files_and_dirs(
    cancel: Option<&CancellationToken>,
    paths: &[String],
    basenames: &[String],
    query: &str,
    limit: usize,
) -> Vec<FileSearchHit> {
    let files = rank_files(cancel, paths, basenames, query, limit);
    let dirs = unique_parent_dirs(paths);
    if dirs.is_empty() {
        return files;
    }
    let bases: Vec<String> = dirs.iter().map(|d| path::base(d)).collect();
    let dir_hits = rank_files(cancel, &dirs, &bases, query, limit);
    merge_file_dir_hits(files, dir_hits, limit)
}

fn merge_file_dir_hits(
    files: Vec<FileSearchHit>,
    mut dirs: Vec<FileSearchHit>,
    limit: usize,
) -> Vec<FileSearchHit> {
    let limit = if limit == 0 { 50 } else { limit };
    for d in &mut dirs {
        d.dir = true;
        if !d.path.is_empty() && !d.path.ends_with('/') {
            d.path.push('/');
        }
    }
    if dirs.is_empty() {
        return files;
    }
    if files.is_empty() {
        dirs.truncate(limit);
        return dirs;
    }
    let mut out = Vec::with_capacity(limit);
    let (mut i, mut j) = (0, 0);
    while out.len() < limit && (i < files.len() || j < dirs.len()) {
        let take_dir = if i >= files.len() {
            true
        } else if j >= dirs.len() {
            false
        } else {
            dirs[j].score > files[i].score
                || (dirs[j].score == files[i].score && dirs[j].path < files[i].path)
        };
        if take_dir {
            out.push(dirs[j].clone());
            j += 1;
        } else {
            out.push(files[i].clone());
            i += 1;
        }
    }
    out
}

fn camel_hump_match(text: &str, q: &str) -> bool {
    let mut humps: Vec<String> = Vec::new();
    let mut cur = String::new();
    for (i, r) in text.chars().enumerate() {
        if i > 0 && (r.is_uppercase() || (matches!(r, '_' | '-' | '.') && !cur.is_empty())) {
            if !cur.is_empty() {
                humps.push(std::mem::take(&mut cur));
            }
            if r.is_alphanumeric() {
                cur.push(r);
            }
            continue;
        }
        if r.is_alphanumeric() {
            cur.push(r);
        }
    }
    if !cur.is_empty() {
        humps.push(cur);
    }
    if humps.is_empty() {
        return false;
    }
    let initials: String = humps
        .iter()
        .filter_map(|h| h.chars().next().and_then(|c| c.to_lowercase().next()))
        .collect();
    if initials.starts_with(q) {
        return true;
    }
    let q = q.as_bytes();
    let mut qi = 0;
    for h in &humps {
        let hl = h.to_lowercase();
        for (i, &c) in hl.as_bytes().iter().enumerate() {
            if qi >= q.len() {
                break;
            }
            if c == q[qi] {
                qi += 1;
            } else if i > 0 {
                break;
            }
        }
        if qi >= q.len() {
            return true;
        }
    }
    qi >= q.len()
}

pub(crate) fn word_boundary_match(lower_text: &str, q: &str) -> bool {
    if q.is_empty() {
        return false;
    }
    if lower_text.starts_with(q) {
        return true;
    }
    let t = lower_text.as_bytes();
    let n = t.len() as isize - q.len() as isize;
    (0..n.max(0) as usize)
        .any(|i| matches!(t[i], b'/' | b'_' | b'-' | b'.') && t[i + 1..].starts_with(q.as_bytes()))
}

/// One compiled `files.exclude` pattern.
enum ExcludeMatcher {
    /// `path == t`, `…/t`, `t/…` or `…/t/…`.
    PathOrUnder {
        target: String,
    },
    Suffix(String),
    Re(Regex),
}

impl ExcludeMatcher {
    fn matches(&self, path: &str) -> bool {
        match self {
            Self::PathOrUnder { target } => {
                path == target
                    || path
                        .strip_suffix(target.as_str())
                        .is_some_and(|h| h.ends_with('/'))
                    || path
                        .strip_prefix(target.as_str())
                        .is_some_and(|t| t.starts_with('/'))
                    || path.contains(&format!("/{target}/"))
            }
            Self::Suffix(ext) => path.ends_with(ext.as_str()),
            Self::Re(re) => re.is_match(path),
        }
    }
}

/// `files.exclude` compiled for index builds and file-system splices.
#[derive(Clone, Default)]
pub struct ExcludeSet {
    matchers: Vec<Arc<ExcludeMatcher>>,
}

static EXCLUDE_CACHE: LazyLock<Mutex<HashMap<String, Arc<ExcludeMatcher>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Trims blanks and leading slashes, keeping order.
pub(crate) fn normalize_exclude_patterns(patterns: &[String]) -> Vec<String> {
    patterns
        .iter()
        .map(|p| p.trim().trim_start_matches('/'))
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect()
}

/// `files.exclude` as negated ripgrep `--glob`s. The always-on `.git` and
/// `node_modules` globs stay in [`crate::rg::files_args`]; directory-shaped
/// patterns also get a `!pat/**` companion so rg prunes their children.
pub(crate) fn exclude_to_rg_globs(patterns: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for p in normalize_exclude_patterns(patterns) {
        let bare = p.strip_prefix("**/").unwrap_or(&p);
        if matches!(
            bare,
            ".git" | ".git/**" | "node_modules" | "node_modules/**"
        ) {
            continue;
        }
        let neg = if p.starts_with('!') {
            p.clone()
        } else {
            format!("!{p}")
        };
        out.push(neg.clone());
        if should_emit_descendant_glob(&p) {
            out.push(format!("{neg}/**"));
        }
    }
    out
}

/// Whether `pat` should also exclude children (a directory-style prune).
/// Patterns ending in `/**` and file globs like `**/*.map` do not.
fn should_emit_descendant_glob(pat: &str) -> bool {
    let p = pat.strip_prefix('!').unwrap_or(pat);
    if p.ends_with("/**") {
        return false;
    }
    let base = p.rsplit('/').next().unwrap_or(p);
    !base.contains(['*', '?'])
}

pub(crate) fn compile_excludes(patterns: &[String]) -> ExcludeSet {
    let mut matchers = Vec::new();
    for pat in patterns {
        let p = pat.trim();
        if p.is_empty() {
            continue;
        }
        let p = p.trim_start_matches('/');
        let mut cache = EXCLUDE_CACHE.lock();
        let m = cache
            .entry(p.to_string())
            .or_insert_with(|| Arc::new(compile_exclude_pattern(p)))
            .clone();
        matchers.push(m);
    }
    ExcludeSet { matchers }
}

/// O(path) string checks for the common shapes (`**/name`, `name/**`,
/// `**/*.ext`, plain names); a regex otherwise.
fn compile_exclude_pattern(p: &str) -> ExcludeMatcher {
    if let Some(m) = fast_exclude_matcher(p) {
        return m;
    }
    match Regex::new(&glob_to_regexp(p)) {
        Ok(re) => ExcludeMatcher::Re(re),
        Err(e) => panic!("regexp: Compile({:?}): {e}", p),
    }
}

fn fast_exclude_matcher(p: &str) -> Option<ExcludeMatcher> {
    let wild = |s: &str| s.contains(['*', '?']);
    if let Some(rest) = p.strip_prefix("**/") {
        if rest.is_empty() {
            return None;
        }
        if !wild(rest) {
            return Some(ExcludeMatcher::PathOrUnder {
                target: rest.to_string(),
            });
        }
        if let Some(ext) = rest.strip_prefix("*.")
            && !wild(ext)
        {
            return Some(ExcludeMatcher::Suffix(rest[1..].to_string()));
        }
        return None;
    }
    if let Some(prefix) = p.strip_suffix("/**")
        && !prefix.is_empty()
        && !wild(prefix)
    {
        return Some(ExcludeMatcher::PathOrUnder {
            target: prefix.to_string(),
        });
    }
    if !wild(p) {
        return Some(ExcludeMatcher::PathOrUnder {
            target: p.to_string(),
        });
    }
    None
}

pub(crate) fn excluded(path: &str, ex: &ExcludeSet) -> bool {
    if ex.matchers.is_empty() {
        return false;
    }
    let rel = path.trim_start_matches('/');
    ex.matchers.iter().any(|m| m.matches(rel))
}

#[cfg(test)]
#[path = "filesearch_tests.rs"]
mod tests;
