//! Project search and replace-in-files (the editor's own walk; ripgrep is
//! the agent tools' fast path).

use std::fs::{self, File};
use std::ops::ControlFlow;
use std::path::Path;

use recur_domain::path;
use recur_domain::{ReplaceResult, SearchHit, SearchOpts, SearchResult};
use regex::Regex;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::gore;
use crate::root::{path_str, rel_slash_str, skip_dir, skip_file};
use crate::scan::scan_lines;
use crate::walk::{Flow, walk_dir};

const DEFAULT_SEARCH_HITS: i64 = 200;
const MAX_SEARCH_HITS_CAP: i64 = 2000;
const MAX_SEARCH_FILE_BYTES: u64 = 256 * 1024;

/// Case-insensitive substring matches of `query` under `root`.
pub fn search(
    cancel: &CancellationToken,
    root: &Path,
    query: &str,
) -> Result<Option<Vec<SearchHit>>> {
    search_opts(cancel, root, query, &SearchOpts::default()).map(|r| r.hits)
}

/// Walks `root` with regex/glob/pagination options, checking `cancel`
/// between files so a pathological regex over a monorepo, or a user who
/// navigated away, does not pin a core until the walk finishes.
///
/// `offset` is implemented by walking from the top and discarding the first
/// `offset` matches, so page N costs roughly N× page 1; the cheaper
/// mitigation is a bigger first page (`limit`, up to 2000).
pub fn search_opts(
    cancel: &CancellationToken,
    root: &Path,
    query: &str,
    opts: &SearchOpts,
) -> Result<SearchResult> {
    let mut out = SearchResult::default();
    let q = query.trim();
    if q.is_empty() {
        return Ok(out);
    }
    let limit = match opts.limit {
        n if n <= 0 => DEFAULT_SEARCH_HITS,
        n => n.min(MAX_SEARCH_HITS_CAP),
    } as usize;
    let offset = opts.offset.max(0) as usize;
    let matcher = Matcher::new(q, opts)?;
    let root = path_str(root);
    let glob = opts.glob.trim();
    let mut hits: Vec<SearchHit> = Vec::new();
    let mut skipped = 0;
    walk_dir(&root, |full, d, err| {
        if hits.len() >= limit {
            return Ok(Flow::SkipAll);
        }
        let (Some(d), None) = (d, err) else {
            return Ok(Flow::Continue);
        };
        if cancel.is_cancelled() {
            return Err(Error::Canceled);
        }
        if d.is_dir() {
            return Ok(if full != root && skip_dir(&d.name) {
                Flow::SkipDir
            } else {
                Flow::Continue
            });
        }
        if skip_file(&d.name) {
            return Ok(Flow::Continue);
        }
        if d.is_symlink() {
            let Some(resolved) = path::eval_symlinks(full) else {
                return Ok(Flow::Continue);
            };
            match path::rel(&root, &resolved) {
                Some(r) if r != ".." && !r.starts_with("../") => {}
                _ => return Ok(Flow::Continue),
            }
        }
        let rel = rel_slash_str(&root, full);
        if !glob.is_empty() && !match_simple_glob(glob, &rel) {
            return Ok(Flow::Continue);
        }
        let Ok(st) = d.info() else {
            return Ok(Flow::Continue);
        };
        if st.len() > MAX_SEARCH_FILE_BYTES {
            // Reported so the UI can say "we didn't look in N files" instead
            // of implying there were no matches.
            out.skipped_large += 1;
            return Ok(Flow::Continue);
        }
        let Ok(f) = File::open(full) else {
            return Ok(Flow::Continue);
        };
        let full_stop = scan_lines(f, |line_no, raw| {
            let Ok(line) = std::str::from_utf8(raw) else {
                return ControlFlow::Continue(());
            };
            let Some(col) = matcher.find(line) else {
                return ControlFlow::Continue(());
            };
            if skipped < offset {
                skipped += 1;
                return ControlFlow::Continue(());
            }
            hits.push(SearchHit {
                path: rel.clone(),
                line: line_no as i64,
                col: col as i64 + 1,
                text: q.to_string(),
                preview: line.trim().to_string(),
            });
            if hits.len() >= limit {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
        Ok(match full_stop {
            Ok(Some(())) => Flow::SkipAll,
            _ => Flow::Continue,
        })
    })?;
    out.truncated = hits.len() >= limit;
    out.hits = (!hits.is_empty()).then_some(hits);
    Ok(out)
}

enum Matcher {
    Re(Regex),
    Exact(String),
    Fold(String),
}

impl Matcher {
    fn new(q: &str, opts: &SearchOpts) -> Result<Self> {
        if opts.regex {
            return compile_query(q, opts.case_sensitive).map(Self::Re);
        }
        Ok(if opts.case_sensitive {
            Self::Exact(q.to_string())
        } else {
            Self::Fold(q.to_lowercase())
        })
    }

    /// The 0-based byte column of the first match.
    fn find(&self, line: &str) -> Option<usize> {
        match self {
            Self::Re(re) => re.find(line).map(|m| m.start()),
            Self::Exact(n) => line.find(n.as_str()),
            Self::Fold(n) => line.to_lowercase().find(n.as_str()),
        }
    }
}

fn compile_query(q: &str, case_sensitive: bool) -> Result<Regex> {
    let flags = if case_sensitive { "" } else { "(?i)" };
    gore::compile(&format!("{flags}{q}")).map_err(|e| Error::msg(format!("invalid regex: {e}")))
}

/// `*` and `?` globs. The pattern is tried against the whole repo-relative
/// path, and a pattern without `/` (like `*.go`, which cannot cross `/`) also
/// against the basename.
pub(crate) fn match_simple_glob(pattern: &str, name: &str) -> bool {
    if path::glob_match(pattern, name) == Ok(true) {
        return true;
    }
    if pattern.contains('/') {
        return false;
    }
    path::glob_match(pattern, &path::base(name)) == Ok(true)
}

/// Replaces every match under `root` (a worktree). Regex, case and glob
/// follow [`search_opts`]; limit and offset are ignored. Cancelling stops the
/// walk; files already rewritten stay rewritten.
pub fn replace_in_files(
    cancel: &CancellationToken,
    root: &Path,
    query: &str,
    replacement: &str,
    opts: &SearchOpts,
) -> Result<ReplaceResult> {
    let q = query.trim();
    if q.is_empty() {
        return Err(Error::msg("empty search"));
    }
    let re = if opts.regex {
        Some(compile_query(q, opts.case_sensitive)?)
    } else {
        None
    };
    let root = path_str(root);
    let glob = opts.glob.trim();
    let mut result = ReplaceResult::default();
    walk_dir(&root, |full, d, err| {
        let (Some(d), None) = (d, err) else {
            return Ok(Flow::Continue);
        };
        if cancel.is_cancelled() {
            return Err(Error::Canceled);
        }
        if d.is_dir() {
            return Ok(if full != root && skip_dir(&d.name) {
                Flow::SkipDir
            } else {
                Flow::Continue
            });
        }
        if skip_file(&d.name) {
            return Ok(Flow::Continue);
        }
        let rel = rel_slash_str(&root, full);
        if !glob.is_empty() && !match_simple_glob(glob, &rel) {
            return Ok(Flow::Continue);
        }
        match d.info() {
            Ok(st) if st.len() <= MAX_SEARCH_FILE_BYTES => {}
            _ => return Ok(Flow::Continue),
        }
        let Ok(src) = fs::read(full).map(String::from_utf8) else {
            return Ok(Flow::Continue);
        };
        let Ok(src) = src else {
            return Ok(Flow::Continue);
        };
        let (next, n) = if let Some(re) = &re {
            let n = re.find_iter(&src).count();
            if n == 0 {
                return Ok(Flow::Continue);
            }
            (re.replace_all(&src, replacement).into_owned(), n)
        } else if opts.case_sensitive {
            let n = src.matches(q).count();
            if n == 0 {
                return Ok(Flow::Continue);
            }
            (src.replace(q, replacement), n)
        } else {
            let (next, n) = replace_fold(&src, q, replacement);
            if n == 0 {
                return Ok(Flow::Continue);
            }
            (next, n)
        };
        if next == src || fs::write(full, next.as_bytes()).is_err() {
            return Ok(Flow::Continue);
        }
        result.files_changed += 1;
        result.replacements += n as i64;
        Ok(Flow::Continue)
    })?;
    Ok(result)
}

/// Case-insensitive replace: matches in the lowercased text, slices the
/// original by the same byte offsets (as Go does). If lowercasing changed
/// byte lengths so the offsets no longer line up, nothing is replaced.
fn replace_fold(src: &str, query: &str, replacement: &str) -> (String, usize) {
    if query.is_empty() {
        return (src.to_string(), 0);
    }
    let lower = src.to_lowercase();
    let ql = query.to_lowercase();
    let (s, l) = (src.as_bytes(), lower.as_bytes());
    let mut out = Vec::with_capacity(s.len());
    let mut n = 0;
    let mut i = 0;
    loop {
        let Some(rest) = l.get(i..) else {
            return (src.to_string(), 0);
        };
        let Some(idx) = find_bytes(rest, ql.as_bytes()) else {
            match s.get(i..) {
                Some(tail) => out.extend_from_slice(tail),
                None => return (src.to_string(), 0),
            }
            break;
        };
        let idx = idx + i;
        match s.get(i..idx) {
            Some(seg) => out.extend_from_slice(seg),
            None => return (src.to_string(), 0),
        }
        out.extend_from_slice(replacement.as_bytes());
        n += 1;
        i = idx + query.len();
    }
    match String::from_utf8(out) {
        Ok(s) => (s, n),
        Err(_) => (src.to_string(), 0),
    }
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}
