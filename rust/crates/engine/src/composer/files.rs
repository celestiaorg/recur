//! `@path[:line[:col][-line[:col]]]` mentions (Go `file_mentions.go`): the
//! attached-files system instruction mapping absolute paths to the short
//! chips left in the body.

use std::collections::HashSet;
use std::sync::LazyLock;

use recur_bundles::go;
use regex::Regex;

/// Workspace file mentions. The left boundary is checked by hand (RE2 has
/// no lookbehind). Keep in sync with the frontend `FILE_AT_RE`.
pub const FILE_AT_PATTERN: &str = r"@((?:/(?:[^\s@:]+(?:/[^\s@:]+)*)|(?:[\w.-]+/)+[\w./+-]+|[\w.-]+/|[\w.-]+\.[A-Za-z][\w.-]*|\.[A-Za-z0-9_][\w.-]*)(?::\d+(?::\d+)?(?:-\d+(?::\d+)?)?)?)";

pub(crate) static FILE_AT_RE: LazyLock<Regex> =
    LazyLock::new(|| go::compile_regexp(FILE_AT_PATTERN).expect("file mention pattern"));

/// The loc suffix: `[:line[:col][-line[:col]]]`.
static FILE_LOC_RE: LazyLock<Regex> = LazyLock::new(|| {
    go::compile_regexp(r":(\d+)(?::(\d+))?(?:-(\d+)(?::(\d+))?)?$").expect("file loc pattern")
});

/// `@token` not covered above (bare dir names like `@src`); existence-gated.
static LOOSE_AT_RE: LazyLock<Regex> =
    LazyLock::new(|| go::compile_regexp(r"@([^\s@]+)").expect("loose mention pattern"));

/// One resolved file or directory mention.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileMention {
    /// The chip as the preamble names it (usually worktree-relative).
    pub chip: String,
    pub abs: String,
    pub dir: bool,
    pub start_line: i64,
    pub end_line: i64,
    pub start_col: i64,
    pub end_col: i64,
}

fn reserved_at_chip(chip: &str) -> bool {
    let s = chip.trim().to_lowercase();
    s == "terminal" || s.starts_with("terminal:") || s == "artifact" || s.starts_with("artifact:")
}

fn atoi_positive(s: &str) -> i64 {
    go::atoi(s).ok().filter(|n| *n > 0).unwrap_or(0)
}

/// Splits a chip into its path and loc. An invalid line (`:0`, overflow)
/// drops the suffix and attaches the whole file; an invalid column keeps
/// the line(s) and drops the span.
pub fn split_file_mention_chip(chip: &str) -> (String, i64, i64, i64, i64) {
    let chip = chip.trim();
    if chip.is_empty() {
        return (String::new(), 0, 0, 0, 0);
    }
    let Some(c) = FILE_LOC_RE.captures(chip) else {
        return (chip.to_owned(), 0, 0, 0, 0);
    };
    let whole = c.get(0).map_or(0, |m| m.start());
    let path = &chip[..whole];
    if path.is_empty() {
        return (chip.to_owned(), 0, 0, 0, 0);
    }
    let group = |i: usize| c.get(i).map_or(0, |m| atoi_positive(m.as_str()));
    let start_line = group(1);
    if start_line == 0 {
        return (path.to_owned(), 0, 0, 0, 0);
    }
    let start_col = group(2);
    let mut end_line = group(3);
    let end_col = group(4);
    if start_col > 0 && end_col > 0 {
        let end_ln = if end_line == 0 { start_line } else { end_line };
        if end_ln < start_line || (end_ln == start_line && end_col < start_col) {
            return (path.to_owned(), start_line, 0, 0, 0);
        }
        if end_ln == start_line {
            return (path.to_owned(), start_line, 0, start_col, end_col);
        }
        return (path.to_owned(), start_line, end_ln, start_col, end_col);
    }
    if end_line <= start_line {
        end_line = 0;
    }
    (path.to_owned(), start_line, end_line, 0, 0)
}

/// `path`, `path:line`, `path:start-end`, or a column span (both ends
/// always `line:col`).
pub fn format_file_mention_chip(
    path: &str,
    start_line: i64,
    end_line: i64,
    start_col: i64,
    mut end_col: i64,
) -> String {
    let path = path.trim();
    if path.is_empty() {
        return String::new();
    }
    if start_line <= 0 {
        return path.to_owned();
    }
    if start_col > 0 && end_col > 0 {
        let end_ln = end_line.max(start_line);
        if end_ln == start_line && end_col < start_col {
            end_col = start_col;
        }
        return format!("{path}:{start_line}:{start_col}-{end_ln}:{end_col}");
    }
    if end_line > start_line {
        return format!("{path}:{start_line}-{end_line}");
    }
    format!("{path}:{start_line}")
}

/// Go `filepath.EvalSymlinks`; the path unchanged when it cannot resolve.
fn eval_symlinks(p: &str) -> Option<String> {
    std::fs::canonicalize(p)
        .ok()
        .map(|r| r.to_string_lossy().into_owned())
        .filter(|r| !r.is_empty())
}

/// A chip resolved against the worktree, symlinks followed when it exists,
/// so the same file mentioned two ways collapses to one row.
fn canonicalize_abs(chip: &str, worktree: &str) -> Option<String> {
    if chip.is_empty() {
        return None;
    }
    let abs = if go::is_abs(chip) {
        go::clean(chip)
    } else {
        if worktree.is_empty() {
            return None;
        }
        go::clean(&go::join(&[worktree, chip]))
    };
    Some(eval_symlinks(&abs).unwrap_or(abs))
}

fn canonical_worktree(worktree: &str) -> String {
    let worktree = worktree.trim_end_matches('/');
    if worktree.is_empty() {
        return String::new();
    }
    eval_symlinks(worktree).unwrap_or_else(|| go::clean(worktree))
}

fn at_mention_boundary(text: &str, start: usize) -> bool {
    start == 0 || matches!(text.as_bytes()[start - 1], b' ' | b'\t' | b'\n' | b'\r')
}

fn mention_preamble_path(path_chip: &str, abs: &str, worktree: &str) -> String {
    if go::is_abs(path_chip)
        && !worktree.is_empty()
        && let Ok(rel) = go::rel(worktree, abs)
        && !rel.is_empty()
        && !rel.starts_with("..")
    {
        return rel;
    }
    let p = go::clean(path_chip);
    if p == "." || p.is_empty() {
        return path_chip.to_owned();
    }
    p
}

fn append_mention(
    out: &mut Vec<FileMention>,
    seen: &mut HashSet<String>,
    chip: &str,
    worktree: &str,
    dirs_only: bool,
) {
    if chip.is_empty() || reserved_at_chip(chip) {
        return;
    }
    let (path_chip, mut start_line, mut end_line, mut start_col, mut end_col) =
        split_file_mention_chip(chip);
    let path_chip = path_chip.trim_end_matches('/');
    let Some(abs) = canonicalize_abs(path_chip, worktree) else {
        return;
    };
    let Ok(info) = std::fs::metadata(&abs) else {
        return;
    };
    let dir = info.is_dir();
    if dirs_only && !dir {
        return;
    }
    if dir {
        (start_line, end_line, start_col, end_col) = (0, 0, 0, 0);
    }
    let seen_key = if start_line > 0 {
        format_file_mention_chip(&abs, start_line, end_line, start_col, end_col)
    } else {
        abs.clone()
    };
    if !seen.insert(seen_key) {
        return;
    }
    let chip = format_file_mention_chip(
        &mention_preamble_path(path_chip, &abs, worktree),
        start_line,
        end_line,
        start_col,
        end_col,
    );
    out.push(FileMention {
        chip,
        abs,
        dir,
        start_line,
        end_line,
        start_col,
        end_col,
    });
}

/// Unique `@file` / `@dir` mentions in first-seen order, deduped by
/// canonical path plus span. A mention starts the text or follows
/// whitespace (not emails, decorators, scoped packages) and must exist.
pub fn collect_file_mentions(text: &str, worktree: &str) -> Vec<FileMention> {
    if text.is_empty() || !text.contains('@') {
        return Vec::new();
    }
    let worktree = canonical_worktree(worktree);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (re, dirs_only) in [(&*FILE_AT_RE, false), (&*LOOSE_AT_RE, true)] {
        for c in re.captures_iter(text) {
            let (Some(m), Some(chip)) = (c.get(0), c.get(1)) else {
                continue;
            };
            if !at_mention_boundary(text, m.start()) {
                continue;
            }
            append_mention(&mut out, &mut seen, chip.as_str(), &worktree, dirs_only);
        }
    }
    out
}

/// Prepends the attached-files instruction (absolute path → short chip, a
/// size per file, a focus note for spans). Unchanged with no mentions.
pub fn wrap_attached_file_mentions(text: &str, worktree: &str) -> String {
    let mentions = collect_file_mentions(text, worktree);
    if mentions.is_empty() {
        return text.to_owned();
    }
    let (dirs, files): (Vec<&FileMention>, Vec<&FileMention>) =
        mentions.iter().partition(|m| m.dir);
    let mut b = String::from("<system_instruction>\n");
    if !files.is_empty() {
        b.push_str("The user has attached these files. Read them before proceeding.\n");
        for m in &files {
            let note = mention_focus_note(m);
            match std::fs::metadata(&m.abs) {
                Ok(info) => b.push_str(&format!(
                    "- {} as @{} ({}){note}\n",
                    m.abs,
                    m.chip,
                    format_attach_size(info.len())
                )),
                Err(_) => b.push_str(&format!("- {} as @{}{note}\n", m.abs, m.chip)),
            }
        }
    }
    if !dirs.is_empty() {
        b.push_str("The user pointed at these directories. Explore them as needed; do not dump the whole tree.\n");
        for m in &dirs {
            b.push_str(&format!("- {} as @{}\n", m.abs, m.chip));
        }
    }
    b.push_str("</system_instruction>\n\n");
    b.push_str(text);
    b
}

fn mention_focus_note(m: &FileMention) -> String {
    if m.start_line <= 0 {
        return String::new();
    }
    if m.start_col > 0 && m.end_col > 0 {
        if m.end_line > m.start_line {
            return format!(
                " — focus on line {} column {} through line {} column {}",
                m.start_line, m.start_col, m.end_line, m.end_col
            );
        }
        if m.end_col == m.start_col {
            return format!(" — focus on line {}, column {}", m.start_line, m.start_col);
        }
        return format!(
            " — focus on line {}, columns {}–{}",
            m.start_line, m.start_col, m.end_col
        );
    }
    if m.end_line > m.start_line {
        return format!(" — focus on lines {}–{}", m.start_line, m.end_line);
    }
    format!(" — focus on line {}", m.start_line)
}

/// `512 B`, `1.5 KB`, `12 KB`, `3.2 MB`.
pub fn format_attach_size(n: u64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    if n < 1024 * 1024 {
        let v = n as f64 / 1024.0;
        return if v < 10.0 {
            format!("{v:.1} KB")
        } else {
            format!("{v:.0} KB")
        };
    }
    let v = n as f64 / (1024.0 * 1024.0);
    if v < 10.0 {
        format!("{v:.1} MB")
    } else {
        format!("{v:.0} MB")
    }
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
