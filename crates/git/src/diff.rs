//! Diff statistics, PR-wide and working-tree diffs, the commit log, and the
//! unified-diff parser behind the review UI.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use chrono::{DateTime, Utc};
use recur_domain::{Commit, DiffLine, DiffStat, FileDiff};
use tokio::io::AsyncReadExt;

use crate::command::command;
use crate::error::{Error, ErrorKind, Result};
use crate::text::atoi;
use crate::version::{log_numstat_flags, probe};
use crate::working_diff::{
    MAX_SYNTHETIC_ADD_BYTES, merge_base, omitted_untracked_diff, read_capped, synthetic_add_diff,
};
use crate::worktree::Manager;
use recur_domain::html_escape;

/// Caps so a dirty worktree (unignored build output, data dumps) cannot blow
/// the diff-sync deadline. Partial counts beat stalls.
pub(crate) const UNTRACKED_MAX_FILES: usize = 200;
pub(crate) const UNTRACKED_MAX_PATHS: usize = 2000;
pub(crate) const UNTRACKED_MAX_BYTES: usize = 2 << 20;
const UNTRACKED_FILE_CAP: usize = 512 << 10;

/// Rolled-up additions/deletions of the worktree versus `base_branch`,
/// including uncommitted changes and the line counts of untracked text files
/// (the sidebar +N −M). An empty base means uncommitted-vs-HEAD. With a base,
/// its merge-base with HEAD is preferred so only the branch's own changes
/// count. Binary files contribute 0.
pub async fn diff_stat_vs_base(worktree: &str, base_branch: &str) -> Result<(i64, i64)> {
    let worktree = worktree.trim();
    if worktree.is_empty() {
        return Err(Error::msg("diffstat: empty worktree"));
    }
    let mut base = "HEAD".to_string();
    let b = base_branch.trim();
    if !b.is_empty() {
        base = b.to_string();
        // stdout only: stderr warnings must not land in the SHA.
        if let Ok(out) = command(["-C", worktree, "merge-base", b, "HEAD"])
            .output()
            .await
        {
            let mb = String::from_utf8_lossy(&out).into_owned();
            if !mb.trim().is_empty() {
                base = mb.trim().to_string();
            }
        }
    }
    // Working tree (index + unstaged) vs base: the branch's outstanding size.
    let out = command(["-C", worktree, "diff", "--numstat", base.as_str()])
        .output()
        .await
        .map_err(|e| {
            let stderr = String::from_utf8_lossy(e.stderr()).into_owned();
            let msg = stderr.trim();
            if matches!(e.kind(), ErrorKind::Exit { .. }) && !msg.is_empty() {
                e.wrap(format!("git diff --numstat: {msg}"))
            } else {
                e.wrap("git diff --numstat")
            }
        })?;
    let (add, del) = parse_numstat(&String::from_utf8_lossy(&out));
    Ok((add + untracked_add_lines(worktree).await, del))
}

/// Newline counts of untracked, non-ignored text files, so new agent files
/// show in the sidebar before they are staged. Bounded by file count and
/// bytes; each file read is an await point, so dropping the future stops the
/// scan.
pub(crate) async fn untracked_add_lines(worktree: &str) -> i64 {
    let Ok(out) = command([
        "-C",
        worktree,
        "ls-files",
        "-z",
        "--others",
        "--exclude-standard",
    ])
    .output()
    .await
    else {
        return 0;
    };
    let (mut total, mut files, mut bytes_read) = (0, 0, 0);
    for rel in out.split(|&b| b == 0) {
        if rel.is_empty() {
            continue;
        }
        if files >= UNTRACKED_MAX_FILES || bytes_read >= UNTRACKED_MAX_BYTES {
            break;
        }
        let path = Path::new(worktree).join(OsStr::from_bytes(rel));
        let (n, read, ok) = count_text_file_lines(&path, UNTRACKED_FILE_CAP).await;
        bytes_read += read;
        if ok {
            total += n;
            files += 1;
        }
    }
    total
}

/// Lines in `path` (a non-empty file without a trailing newline counts its
/// last line) and bytes read, capped. Binary or unreadable files are not ok.
async fn count_text_file_lines(path: &Path, max_bytes: usize) -> (i64, usize, bool) {
    let Ok(f) = tokio::fs::File::open(path).await else {
        return (0, 0, false);
    };
    let mut data = Vec::new();
    if f.take(max_bytes as u64)
        .read_to_end(&mut data)
        .await
        .is_err()
    {
        return (0, data.len(), false);
    }
    if data.is_empty() {
        return (0, 0, true); // empty file: +0, like git's empty-blob numstat
    }
    if data.contains(&0) || std::str::from_utf8(&data).is_err() {
        return (0, data.len(), false);
    }
    let mut n = data.iter().filter(|&&b| b == b'\n').count() as i64;
    if data.last() != Some(&b'\n') {
        n += 1;
    }
    (n, data.len(), true)
}

/// A parsed (additions, deletions) pair for one file or a whole diff.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffStats {
    pub add: i64,
    pub del: i64,
}

/// The PR-wide diff versus the base: changed files with parsed hunks and the
/// rolled-up stats.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrDiff {
    pub files: Vec<FileDiff>,
    pub add: i64,
    pub del: i64,
    /// The untracked path list hit [`UNTRACKED_MAX_PATHS`]; omitted files are
    /// not in `files`.
    pub truncated: bool,
}

impl PrDiff {
    fn new(files: Vec<FileDiff>, truncated: bool) -> PrDiff {
        let add = files.iter().map(|f| f.diff.add).sum();
        let del = files.iter().map(|f| f.diff.del).sum();
        PrDiff {
            files,
            add,
            del,
            truncated,
        }
    }
}

impl Manager {
    /// How many commits `base_branch` is ahead of the worktree HEAD
    /// (`rev-list --count HEAD..<base>`). No fetch; fetch first for a fresher
    /// remote tip.
    pub async fn commits_behind(&self, worktree: &str, base_branch: &str) -> Result<i64> {
        if worktree.is_empty() || base_branch.is_empty() {
            return Err(Error::msg("commits-behind: worktree and base are required"));
        }
        self.rev_list_count(worktree, &format!("HEAD..{base_branch}"))
            .await
    }

    /// How many commits HEAD is ahead of `base_branch`. 0 means nothing to land.
    pub async fn commits_ahead(&self, worktree: &str, base_branch: &str) -> Result<i64> {
        if worktree.is_empty() || base_branch.is_empty() {
            return Err(Error::msg("commits-ahead: worktree and base are required"));
        }
        self.rev_list_count(worktree, &format!("{base_branch}..HEAD"))
            .await
    }

    async fn rev_list_count(&self, worktree: &str, spec: &str) -> Result<i64> {
        let out = self
            .run_in(worktree, ["rev-list", "--count", spec])
            .await
            .map_err(|e| e.wrap(format!("git rev-list {spec}")))?;
        let s = out.trim();
        let n = s
            .parse::<i64>()
            .map_err(|e| Error::msg(format!("parse rev-list count {s:?}: {e}")))?;
        Ok(n.max(0))
    }

    /// The branch's own changes versus the base (`git diff <merge-base> HEAD`),
    /// parsed. An up-to-date branch is an empty diff, not an
    /// error.
    pub async fn diff_pr(&self, worktree: &str, base_branch: &str) -> Result<PrDiff> {
        let mut base = base_branch.to_string();
        if let Ok(mb) = self
            .run_in(worktree, ["merge-base", base_branch, "HEAD"])
            .await
            && !mb.trim().is_empty()
        {
            base = mb.trim().to_string();
        }
        let out = self
            .run_in(worktree, ["diff", "--no-color", base.as_str(), "HEAD"])
            .await
            .map_err(|e| e.wrap(format!("git diff {base_branch}..HEAD")))?;
        Ok(PrDiff::new(parse_unified_diff(&out), false))
    }

    /// The Observatory files-changed view: the working tree (staged, unstaged
    /// and untracked text files) versus the merge-base with `base_branch`, or
    /// HEAD when empty (the empty tree when HEAD is unborn).
    pub async fn diff_working_vs_base(&self, worktree: &str, base_branch: &str) -> Result<PrDiff> {
        let worktree = worktree.trim();
        if worktree.is_empty() {
            return Err(Error::msg("diff: empty worktree"));
        }
        let against = working_diff_against(worktree, base_branch).await?;
        let out = self
            .run_in(
                worktree,
                [
                    "diff",
                    "--no-color",
                    "--no-renames",
                    "--end-of-options",
                    against.as_str(),
                ],
            )
            .await
            .map_err(|e| e.wrap(format!("git diff {against}")))?;
        let mut files = parse_unified_diff(&out);
        let seen: HashSet<String> = files.iter().map(|f| f.path.clone()).collect();
        let (mut bodies, mut bytes_read) = (0, 0);
        let (untracked, list_truncated) = untracked_rel_paths(worktree).await;
        for rel in untracked {
            let shown = String::from_utf8_lossy(&rel).into_owned();
            if seen.contains(&shown) {
                continue;
            }
            if bodies >= UNTRACKED_MAX_FILES || bytes_read >= UNTRACKED_MAX_BYTES {
                files.push(omitted_untracked_diff(&shown));
                continue;
            }
            let abs = Path::new(worktree).join(OsStr::from_bytes(&rel));
            match read_capped(&abs, MAX_SYNTHETIC_ADD_BYTES).await {
                Err(_) => files.push(FileDiff {
                    path: shown,
                    status: "u".into(),
                    ..FileDiff::default()
                }),
                Ok((body, truncated)) => {
                    bytes_read += body.len();
                    files.push(synthetic_add_diff(&shown, &body, truncated));
                    bodies += 1;
                }
            }
        }
        Ok(PrDiff::new(files, list_truncated))
    }

    /// Commits for the Commits tab, newest first: `merge-base..HEAD` with a
    /// base, or recent HEAD history (capped) without one. Each carries
    /// hash/message/author, a diff stat, and role/code/iteration derived from
    /// the message. File diffs are filled lazily by
    /// [`commit_diff`](Self::commit_diff).
    pub async fn log_commits(&self, worktree: &str, base_branch: &str) -> Result<Vec<Commit>> {
        // %x1f between scalar fields, %x1d after the (possibly multiline) body
        // so numstat starts at a known boundary; %x1e leads each commit.
        const FMT: &str = "%x1e%h%x1f%H%x1f%an%x1f%ae%x1f%aI%x1f%s%x1f%b%x1d";
        let mut args: Vec<String> = vec!["log".into(), "--no-color".into(), "--numstat".into()];
        args.extend(
            log_numstat_flags(&probe().await)
                .iter()
                .map(|s| s.to_string()),
        );
        args.push(format!("--format={FMT}"));
        let rev = match base_branch.trim() {
            "" => {
                args.extend(["-n".to_string(), LOG_COMMITS_HEAD_LIMIT.to_string()]);
                "HEAD".to_string()
            }
            b => {
                let mut base = b.to_string();
                if let Ok(mb) = self.run_in(worktree, ["merge-base", b, "HEAD"]).await
                    && !mb.trim().is_empty()
                {
                    base = mb.trim().to_string();
                }
                format!("{base}..HEAD")
            }
        };
        args.push(rev.clone());
        let out = self
            .run_in(worktree, &args)
            .await
            .map_err(|e| e.wrap(format!("git log {rev}")))?;

        let now = Utc::now();
        let mut commits = Vec::new();
        for rec in out.split('\x1e') {
            if rec.trim().is_empty() {
                continue;
            }
            let (payload, numstat) = rec.split_once('\x1d').unwrap_or((rec, ""));
            let fields: Vec<&str> = payload.splitn(7, '\x1f').collect();
            if fields.len() < 6 {
                continue;
            }
            let author = fields[2].trim().to_string();
            let msg = fields[5].trim().to_string();
            let body = fields
                .get(6)
                .map(|b| b.trim().to_string())
                .unwrap_or_default();
            let (role, code, iter_label, color) =
                classify_commit_message(&format!("{msg}\n{body}"), &author);
            let (add, del) = parse_numstat(numstat);
            commits.push(Commit {
                hash: fields[0].trim().to_string(),
                sha: fields[1].trim().to_string(),
                message: msg,
                body,
                author,
                author_role: role,
                agent_code: code,
                color,
                iter_label,
                diff: DiffStat { add, del },
                when: format_commit_when(fields[4], now),
                ..Commit::default()
            });
        }
        Ok(commits)
    }

    /// One commit's own diff (`git show --format= <hash>`). A
    /// merge commit has no patch here, so eager fills never persist a
    /// whole-branch delta; the drill-in uses
    /// [`commit_diff_first_parent`](Self::commit_diff_first_parent).
    pub async fn commit_diff(&self, worktree: &str, hash: &str) -> Result<Vec<FileDiff>> {
        self.show_commit_diff(worktree, hash, false).await
    }

    /// `git show --first-parent`: merge commits diff against their first
    /// parent (GitHub's commit view).
    pub async fn commit_diff_first_parent(
        &self,
        worktree: &str,
        hash: &str,
    ) -> Result<Vec<FileDiff>> {
        self.show_commit_diff(worktree, hash, true).await
    }

    async fn show_commit_diff(
        &self,
        worktree: &str,
        hash: &str,
        first_parent: bool,
    ) -> Result<Vec<FileDiff>> {
        let mut args = vec!["show"];
        if first_parent {
            args.push("--first-parent");
        }
        args.extend(["--no-color", "--format=", hash]);
        let out = self
            .run_in(worktree, &args)
            .await
            .map_err(|e| e.wrap(format!("git show {hash}")))?;
        Ok(parse_unified_diff(&out))
    }
}

/// The tree the working diff compares to: the merge-base with `base_branch`,
/// HEAD when empty, or the empty tree when HEAD is unborn.
async fn working_diff_against(worktree: &str, base_branch: &str) -> Result<String> {
    let mut against = "HEAD".to_string();
    let b = base_branch.trim();
    if !b.is_empty() {
        against = merge_base(worktree, b).await;
    }
    if against != "HEAD" || commit_ref_exists(worktree, "HEAD").await {
        return Ok(against);
    }
    empty_tree_oid(worktree)
        .await
        .map_err(|e| e.wrap("git diff HEAD"))
}

pub(crate) async fn commit_ref_exists(worktree: &str, r: &str) -> bool {
    let spec = format!("{r}^{{commit}}");
    command([
        "-C",
        worktree,
        "rev-parse",
        "--verify",
        "--quiet",
        spec.as_str(),
    ])
    .output()
    .await
    .is_ok()
}

/// This repository's empty-tree object id (SHA-1 or SHA-256), written if needed.
async fn empty_tree_oid(worktree: &str) -> Result<String> {
    let out = command(["-C", worktree, "hash-object", "-w", "-t", "tree", "--stdin"])
        .stdin(Vec::new())
        .output()
        .await?;
    let oid = String::from_utf8_lossy(&out)
        .into_owned()
        .trim()
        .to_string();
    if oid.is_empty() {
        return Err(Error::msg("empty oid"));
    }
    Ok(oid)
}

/// Untracked, non-ignored paths as raw bytes, capped at
/// [`UNTRACKED_MAX_PATHS`]; `truncated` when more existed.
async fn untracked_rel_paths(worktree: &str) -> (Vec<Vec<u8>>, bool) {
    let Ok(out) = command([
        "-C",
        worktree,
        "ls-files",
        "-z",
        "--others",
        "--exclude-standard",
    ])
    .output()
    .await
    else {
        return (Vec::new(), false);
    };
    let mut paths = Vec::new();
    for rel in out.split(|&b| b == 0) {
        if rel.is_empty() {
            continue;
        }
        if paths.len() >= UNTRACKED_MAX_PATHS {
            return (paths, true);
        }
        paths.push(rel.to_vec());
    }
    (paths, false)
}

/// Caps working-copy history so `git log HEAD` on a long main branch does
/// not dump thousands of rows into the Commits tab.
const LOG_COMMITS_HEAD_LIMIT: usize = 100;

/// Sums `--numstat` additions/deletions; unparseable fields (binary `-`) are 0.
pub(crate) fn parse_numstat(out: &str) -> (i64, i64) {
    let (mut add, mut del) = (0, 0);
    for line in out.trim().split('\n') {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 2 {
            continue;
        }
        add += atoi(fields[0]).unwrap_or(0);
        del += atoi(fields[1]).unwrap_or(0);
    }
    (add, del)
}

/// Role, 2-letter code, iteration label and UI color from a commit message
/// and author. Legacy engine commits said `recur(<Role>): iteration N — …`;
/// current ones are plain summaries, so the role falls back to the author's
/// initials.
pub(crate) fn classify_commit_message(
    msg: &str,
    author_name: &str,
) -> (String, String, String, String) {
    let (mut role, mut code, mut color) = (
        "Author".to_string(),
        author_initials(author_name),
        "var(--indigo)",
    );
    let low = msg.to_lowercase();
    let roles = [
        (
            "security reviewer",
            "Security Reviewer",
            "SR",
            "var(--coral)",
        ),
        ("reviewer", "Reviewer", "RV", "var(--indigo)"),
        ("tester", "Tester", "TE", "var(--emerald)"),
        ("coder", "Coder", "CO", "var(--cyan)"),
    ];
    if let Some((_, r, c, col)) = roles.iter().find(|(needle, ..)| low.contains(needle)) {
        role = (*r).to_string();
        code = (*c).to_string();
        color = col;
    }
    let mut iter_label = String::new();
    if let Some(i) = low.find("iteration ") {
        let rest = &low[i + "iteration ".len()..];
        let n = rest.bytes().take_while(u8::is_ascii_digit).count();
        if n > 0 {
            iter_label = format!("iteration {}", &rest[..n]);
        }
    }
    (role, code, iter_label, color.to_string())
}

/// The 2-letter avatar for a git author: first letters of the first and last
/// words, or the first two of a single word. Empty names become `?`.
pub(crate) fn author_initials(name: &str) -> String {
    let fields: Vec<&str> = name.split_whitespace().collect();
    if fields.is_empty() {
        return "?".to_string();
    }
    let take = |s: &str, n: usize| -> Vec<char> {
        s.chars()
            .filter(|&c| c.is_alphabetic() || c.is_numeric())
            .flat_map(char::to_uppercase)
            .take(n)
            .collect()
    };
    let letters: Vec<char> = if fields.len() == 1 {
        take(fields[0], 2)
    } else {
        let mut l = take(fields[0], 1);
        l.extend(take(fields[fields.len() - 1], 1));
        l
    };
    if letters.is_empty() {
        "?".to_string()
    } else {
        letters.into_iter().collect()
    }
}

pub(crate) fn format_commit_when(iso: &str, now: DateTime<Utc>) -> String {
    let Ok(t) = DateTime::parse_from_rfc3339(iso.trim()) else {
        return "now".to_string();
    };
    let d = (now - t.with_timezone(&Utc)).max(chrono::TimeDelta::zero());
    if d < chrono::TimeDelta::minutes(1) {
        "now".to_string()
    } else if d < chrono::TimeDelta::hours(1) {
        format!("{}m", d.num_minutes())
    } else if d < chrono::TimeDelta::hours(24) {
        format!("{}h", d.num_hours())
    } else {
        format!("{}d", d.num_hours() / 24)
    }
}

fn push_line(f: &mut FileDiff, line: DiffLine) {
    f.lines.get_or_insert_with(Vec::new).push(line);
}

pub fn parse_unified_diff(diff: &str) -> Vec<FileDiff> {
    let mut files = Vec::new();
    let mut cur: Option<FileDiff> = None;
    let (mut old_line, mut new_line) = (0i64, 0i64);
    // Inside a hunk, a deleted "-- x" line reads "--- x" and an added "++ x"
    // reads "+++ x": those are content, not file headers.
    let mut in_hunk = false;

    for line in diff.split('\n') {
        if line.starts_with("diff --git ") {
            files.extend(cur.take());
            cur = Some(FileDiff {
                path: path_from_diff_header(line),
                status: "m".into(),
                ..FileDiff::default()
            });
            old_line = 0;
            new_line = 0;
            in_hunk = false;
            continue;
        }
        let Some(f) = cur.as_mut() else { continue };
        if line.starts_with("new file mode") {
            f.status = "a".into();
        } else if line.starts_with("deleted file mode") {
            f.status = "d".into();
        } else if let Some(p) = line.strip_prefix("rename from ") {
            f.status = "r".into();
            f.old_path = p.trim().to_string();
        } else if let Some(p) = line.strip_prefix("rename to ") {
            f.path = p.trim().to_string();
        } else if line.starts_with("Binary files ") {
            f.lines = None;
        } else if !in_hunk && line.starts_with("--- ") {
            // Old-file header: the new-file path is authoritative.
        } else if !in_hunk && line.starts_with("+++ ") {
            let p = path_from_file_header(line);
            if !p.is_empty() {
                f.path = p;
            }
        } else if line.starts_with("@@") {
            let Some(h) = parse_hunk_header(line) else {
                continue;
            };
            old_line = h.old_start;
            new_line = h.new_start;
            in_hunk = true;
            push_line(
                f,
                DiffLine {
                    r#type: "hunk".into(),
                    code_html: html_escape(&format_hunk_label(&h)),
                    ..DiffLine::default()
                },
            );
        } else if let Some(code) = line.strip_prefix('+') {
            push_line(
                f,
                DiffLine {
                    new: new_line.to_string(),
                    r#type: "add".into(),
                    code_html: html_escape(code),
                    ..DiffLine::default()
                },
            );
            f.diff.add += 1;
            new_line += 1;
        } else if let Some(code) = line.strip_prefix('-') {
            push_line(
                f,
                DiffLine {
                    old: old_line.to_string(),
                    r#type: "del".into(),
                    code_html: html_escape(code),
                    ..DiffLine::default()
                },
            );
            f.diff.del += 1;
            old_line += 1;
        } else if let Some(code) = line.strip_prefix(' ') {
            push_line(
                f,
                DiffLine {
                    old: old_line.to_string(),
                    new: new_line.to_string(),
                    r#type: "ctx".into(),
                    code_html: html_escape(code),
                    ..DiffLine::default()
                },
            );
            old_line += 1;
            new_line += 1;
        }
        // "\ No newline at end of file", "index …", mode lines: not rows.
    }
    files.extend(cur);
    files
}

/// The display path from `diff --git a/x b/y`, preferring the new side.
fn path_from_diff_header(line: &str) -> String {
    let rest = line.strip_prefix("diff --git ").unwrap_or(line);
    if let Some(i) = rest.find(" b/") {
        return rest[i + 3..].trim().to_string();
    }
    if rest.starts_with("a/") {
        let half = rest.find(' ').map_or(rest, |sp| &rest[..sp]);
        return half.strip_prefix("a/").unwrap_or(half).to_string();
    }
    rest.trim().to_string()
}

/// The path from `+++ b/path`; empty for `+++ /dev/null` (a deletion).
fn path_from_file_header(line: &str) -> String {
    let p = line.strip_prefix("+++ ").unwrap_or(line).trim();
    if p == "/dev/null" {
        return String::new();
    }
    p.strip_prefix("b/").unwrap_or(p).to_string()
}

/// A parsed `@@ … @@` line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HunkHeader {
    old_start: i64,
    old_count: i64,
    new_start: i64,
    new_count: i64,
    context: String,
}

/// Parses `@@ -<old>[,<n>] +<new>[,<n>] @@ …`; a missing count is 1.
pub(crate) fn parse_hunk_header(line: &str) -> Option<HunkHeader> {
    let body = match line.find("@@ ") {
        Some(i) => &line[i + 3..],
        None => line,
    };
    let end = body.find(" @@")?;
    let mut h = HunkHeader {
        old_count: 1,
        new_count: 1,
        context: body[end + 3..].trim().to_string(),
        ..HunkHeader::default()
    };
    for r in body[..end].split_whitespace() {
        if let Some(rest) = r.strip_prefix('-') {
            (h.old_start, h.old_count) = range_pair(rest);
        } else if let Some(rest) = r.strip_prefix('+') {
            (h.new_start, h.new_count) = range_pair(rest);
        }
    }
    Some(h)
}

/// A GitHub-style label (`Lines 173–188 · class Game:`), never the raw `@@`.
pub(crate) fn format_hunk_label(h: &HunkHeader) -> String {
    let label = line_range_label(h);
    if h.context.is_empty() {
        label
    } else {
        format!("{label} · {}", h.context)
    }
}

fn line_range_label(h: &HunkHeader) -> String {
    // The new side, unless this is a pure deletion.
    let (start, count) = if h.new_count == 0 {
        (h.old_start, h.old_count)
    } else {
        (h.new_start, h.new_count)
    };
    if count <= 1 {
        format!("Line {start}")
    } else {
        format!("Lines {start}–{}", start + count - 1)
    }
}

fn range_pair(r: &str) -> (i64, i64) {
    match r.split_once(',') {
        Some((s, c)) => (atoi(s).unwrap_or(0), atoi(c).unwrap_or(1)),
        None => (atoi(r).unwrap_or(0), 1),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::testutil::{commit_as_coder, git, git_as, init_repo, temp_dir, write, write_bytes};

    fn mgr(repo: &str) -> (Manager, crate::testutil::Dir) {
        let root = temp_dir();
        (Manager::new(repo, &root.path), root)
    }

    fn lines(f: &FileDiff) -> &[DiffLine] {
        f.lines.as_deref().unwrap_or_default()
    }

    #[tokio::test]
    async fn diff_pr_shapes() {
        let repo = init_repo();
        let r = repo.path.as_str();
        git(r, &["checkout", "-b", "recur/feature"]);
        write(r, "README.md", "hello world\nsecond line\n");
        write(r, "feature.txt", "alpha\nbeta\n");
        commit_as_coder(r, "recur(Coder): iteration 1 — add a feature");
        let (m, _root) = mgr(r);

        let pr = m.diff_pr(r, "main").await.expect("diff_pr");
        assert_eq!(pr.files.len(), 2, "{:?}", pr.files);
        let by: HashMap<&str, &FileDiff> = pr.files.iter().map(|f| (f.path.as_str(), f)).collect();
        let readme = by["README.md"];
        assert_eq!(readme.status, "m");
        let (mut hunk, mut add, mut del) = (false, false, false);
        for l in lines(readme) {
            match l.r#type.as_str() {
                "hunk" => {
                    hunk = true;
                    assert!(
                        l.code_html.starts_with("Line"),
                        "hunk label should be GitHub-style: {}",
                        l.code_html
                    );
                    assert!(
                        !l.code_html.contains("@@"),
                        "hunk label must not show raw @@: {}",
                        l.code_html
                    );
                }
                "add" => {
                    add = true;
                    assert!(!l.new.is_empty() && l.old.is_empty(), "{l:?}");
                }
                "del" => {
                    del = true;
                    assert!(!l.old.is_empty() && l.new.is_empty(), "{l:?}");
                }
                _ => {}
            }
        }
        assert!(
            hunk && add && del,
            "README diff missing line kinds: {:?}",
            readme.lines
        );
        let feat = by["feature.txt"];
        assert_eq!(feat.status, "a");
        assert_eq!((feat.diff.add, feat.diff.del), (2, 0));
        let new_lines: Vec<&str> = lines(feat)
            .iter()
            .filter(|l| l.r#type == "add")
            .map(|l| l.new.as_str())
            .collect();
        assert_eq!(new_lines, vec!["1", "2"]);
        assert!(pr.add > 0);

        let commits = m.log_commits(r, "main").await.expect("log_commits");
        assert_eq!(commits.len(), 1, "{commits:?}");
        let c = &commits[0];
        assert_eq!(
            (c.author_role.as_str(), c.agent_code.as_str()),
            ("Coder", "CO")
        );
        assert_eq!(c.iter_label, "iteration 1");
        assert!(c.diff.add > 0);
        assert_eq!(
            m.commit_diff(r, &c.hash).await.expect("commit_diff").len(),
            2
        );
    }

    #[tokio::test]
    async fn diff_working_vs_base_includes_uncommitted() {
        let repo = init_repo();
        let r = repo.path.as_str();
        git(r, &["checkout", "-b", "recur/feature"]);
        write(r, "feature.txt", "alpha\n");
        commit_as_coder(r, "add feature");
        write(r, "feature.txt", "alpha\nbeta\n");
        write(r, "scratch.txt", "untracked\n");
        let (m, _root) = mgr(r);

        let pr = m.diff_pr(r, "main").await.expect("diff_pr");
        assert!(
            pr.files.len() == 1 && pr.files[0].path == "feature.txt",
            "{:?}",
            pr.files
        );
        let got = m
            .diff_working_vs_base(r, "main")
            .await
            .expect("working vs base");
        let by: HashMap<&str, &FileDiff> = got.files.iter().map(|f| (f.path.as_str(), f)).collect();
        assert!(by.contains_key("feature.txt"));
        let scratch = by["scratch.txt"];
        assert_eq!(scratch.status, "a");
        assert!(scratch.diff.add >= 1);
        let head_only = m.diff_working_vs_base(r, "").await.expect("vs HEAD");
        let paths: Vec<&str> = head_only.files.iter().map(|f| f.path.as_str()).collect();
        assert!(
            paths.contains(&"feature.txt") && paths.contains(&"scratch.txt"),
            "{paths:?}"
        );
    }

    #[tokio::test]
    async fn diff_working_vs_base_unborn_head() {
        let dir = temp_dir();
        let d = dir.path.as_str();
        git(d, &["init", "-b", "main"]);
        write(d, "staged.txt", "in index\n");
        git(d, &["add", "staged.txt"]);
        write(d, "loose.txt", "untracked\n");
        let (m, _root) = mgr(d);
        let got = m.diff_working_vs_base(d, "").await.expect("unborn");
        let paths: Vec<&str> = got.files.iter().map(|f| f.path.as_str()).collect();
        assert!(
            paths.contains(&"staged.txt") && paths.contains(&"loose.txt"),
            "{paths:?}"
        );
    }

    #[tokio::test]
    async fn commit_diff_merge_commit() {
        let repo = init_repo();
        let r = repo.path.as_str();
        git(r, &["checkout", "-b", "feature"]);
        write(r, "side.txt", "from feature\n");
        commit_as_coder(r, "add side");
        git(r, &["checkout", "main"]);
        git(r, &["merge", "--no-ff", "-m", "merge feature", "feature"]);
        let (m, _root) = mgr(r);
        let hash = crate::testutil::rev_parse(r, "HEAD");
        assert!(
            m.commit_diff(r, &hash)
                .await
                .expect("commit_diff")
                .is_empty(),
            "no first-parent diff by default"
        );
        let files = m
            .commit_diff_first_parent(r, &hash)
            .await
            .expect("first parent");
        assert!(files.iter().any(|f| f.path == "side.txt"), "{files:?}");
    }

    #[tokio::test]
    async fn commit_diff_sync_with_base_merge() {
        let repo = init_repo();
        let r = repo.path.as_str();
        git(r, &["checkout", "-b", "feature"]);
        write(r, "feat.txt", "on feature\n");
        commit_as_coder(r, "add feat");
        git(r, &["checkout", "main"]);
        write(r, "m.txt", "from main\n");
        commit_as_coder(r, "advance main");
        git(r, &["checkout", "feature"]);
        git(r, &["merge", "--no-ff", "-m", "sync with base", "main"]);
        let (m, _root) = mgr(r);
        let hash = crate::testutil::rev_parse(r, "HEAD");
        assert!(
            m.commit_diff(r, &hash)
                .await
                .expect("commit_diff")
                .is_empty(),
            "eager fill must not embed the base delta"
        );
        let files = m
            .commit_diff_first_parent(r, &hash)
            .await
            .expect("first parent");
        assert!(files.iter().any(|f| f.path == "m.txt"), "{files:?}");
        let commits = m.log_commits(r, "main").await.expect("log");
        assert_eq!(commits.len(), 2, "merge + feat");
        assert_eq!(commits[0].message, "sync with base");
        assert!(
            commits[0].diff.add >= 1,
            "first-parent numstat for the merge: {:?}",
            commits[0].diff
        );
        assert_eq!(commits[1].message, "add feat");
    }

    #[tokio::test]
    async fn diff_stat_vs_base_includes_working_tree() {
        let repo = init_repo();
        let r = repo.path.as_str();
        git(r, &["checkout", "-b", "recur/feature"]);
        write(r, "feat.txt", "one\ntwo\n");
        commit_as_coder(r, "recur(Coder): iteration 1 — feat");
        let (add, del) = diff_stat_vs_base(r, "main").await.expect("committed");
        assert!(add >= 2 && del == 0, "+{add} -{del}");
        write(r, "feat.txt", "one\ntwo\nthree\n");
        let (add2, del2) = diff_stat_vs_base(r, "main").await.expect("dirty");
        assert!(add2 > add && del2 == 0, "+{add2} -{del2}");
        let (add_head, del_head) = diff_stat_vs_base(r, "").await.expect("vs HEAD");
        assert!(add_head >= 1 && del_head == 0);
        write(r, "brand-new.txt", "alpha\nbeta\ngamma\n");
        let (add3, _) = diff_stat_vs_base(r, "main").await.expect("untracked");
        assert!(
            add3 >= add2 + 3,
            "untracked file should add ≥3 lines: {add2} → {add3}"
        );
    }

    /// Dropping the scan stops it promptly.
    #[tokio::test]
    async fn untracked_add_lines_honors_cancel() {
        let repo = init_repo();
        for i in 0..40 {
            write(&repo.path, &format!("bulk/f{i}.txt"), "x\n");
        }
        let started = std::time::Instant::now();
        let res =
            tokio::time::timeout(std::time::Duration::ZERO, untracked_add_lines(&repo.path)).await;
        assert!(res.is_err(), "a zero timeout should cancel the scan");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "cancel was not prompt"
        );
        assert_eq!(untracked_add_lines(&repo.path).await, 40);
    }

    #[test]
    fn parse_unified_diff_hunk_content_dashes() {
        let diff = "diff --git a/q.sql b/q.sql\n--- a/q.sql\n+++ b/q.sql\n@@ -1,2 +1,2 @@\n SELECT 1;\n--- old comment\n+++ new thing\n";
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 1);
        let l = lines(&files[0]);
        assert!(
            l.iter()
                .any(|l| l.r#type == "del" && l.code_html.contains("-- old comment")),
            "{l:?}"
        );
        assert!(
            l.iter()
                .any(|l| l.r#type == "add" && l.code_html.contains("++ new thing")),
            "{l:?}"
        );
    }

    #[test]
    fn format_hunk_label_cases() {
        for (line, want) in [
            (
                "@@ -173,12 +173,16 @@ class Game:",
                "Lines 173–188 · class Game:",
            ),
            ("@@ -1,2 +1,2 @@", "Lines 1–2"),
            ("@@ -0,0 +1,1 @@", "Line 1"),
            ("@@ -5 +5 @@ def foo():", "Line 5 · def foo():"),
            ("@@ -10,3 +0,0 @@", "Lines 10–12"),
        ] {
            let h =
                parse_hunk_header(line).unwrap_or_else(|| panic!("parse_hunk_header({line:?})"));
            assert_eq!(format_hunk_label(&h), want, "{line}");
        }
    }

    #[test]
    fn parse_unified_diff_edge_cases() {
        assert!(parse_unified_diff("").is_empty());
        let diff = "diff --git a/x.html b/x.html\nnew file mode 100644\n--- /dev/null\n+++ b/x.html\n@@ -0,0 +1,1 @@\n+<script>alert('xss')</script>\n";
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].status, "a");
        let code = lines(&files[0])
            .iter()
            .rev()
            .find(|l| l.r#type == "add")
            .map(|l| l.code_html.clone())
            .unwrap_or_default();
        assert!(
            !code.contains("<script>") && code.contains("&lt;") && code.contains("&gt;"),
            "{code}"
        );

        let bin = parse_unified_diff(
            "diff --git a/img.png b/img.png\nnew file mode 100644\nBinary files /dev/null and b/img.png differ\n",
        );
        assert_eq!(bin.len(), 1);
        assert!(bin[0].lines.is_none());
        assert_eq!(
            (bin[0].path.as_str(), bin[0].status.as_str()),
            ("img.png", "a")
        );

        let modified = parse_unified_diff(
            "diff --git a/img.png b/img.png\nindex 1111111..2222222 100644\nBinary files a/img.png and b/img.png differ\n",
        );
        assert_eq!(modified.len(), 1);
        assert_eq!(
            (modified[0].path.as_str(), modified[0].status.as_str()),
            ("img.png", "m")
        );
        assert!(modified[0].lines.is_none());

        let ren = parse_unified_diff(
            "diff --git a/old.sql b/new.sql\nsimilarity index 100%\nrename from old.sql\nrename to new.sql\n",
        );
        assert_eq!(ren.len(), 1);
        assert_eq!(
            (
                ren[0].path.as_str(),
                ren[0].old_path.as_str(),
                ren[0].status.as_str()
            ),
            ("new.sql", "old.sql", "r")
        );
        assert_eq!((ren[0].diff.add, ren[0].diff.del), (0, 0));
        assert!(ren[0].lines.is_none());
    }

    #[tokio::test]
    async fn log_commits_without_base_lists_head() {
        let repo = init_repo();
        let (m, _root) = mgr(&repo.path);
        let commits = m.log_commits(&repo.path, "").await.expect("log");
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].message, "initial");
    }

    #[tokio::test]
    async fn log_commits_numstat_per_commit() {
        let repo = init_repo();
        let r = repo.path.as_str();
        git(r, &["checkout", "-b", "recur/feature"]);
        write(r, "one.txt", "a\n");
        commit_as_coder(r, "recur(Coder): iteration 1 — first");
        write(r, "two.txt", "a\nb\nc\n");
        write(r, "three.txt", "x\ny\n");
        commit_as_coder(r, "recur(Reviewer): iteration 2 — second");
        let (m, _root) = mgr(r);
        let commits = m.log_commits(r, "main").await.expect("log");
        assert_eq!(commits.len(), 2);
        assert_eq!((commits[0].diff.add, commits[0].diff.del), (5, 0));
        assert_eq!((commits[1].diff.add, commits[1].diff.del), (1, 0));
        assert_eq!(
            (
                commits[0].agent_code.as_str(),
                commits[1].agent_code.as_str()
            ),
            ("RV", "CO")
        );
    }

    #[tokio::test]
    async fn log_commits_author_when_and_sha() {
        let repo = init_repo();
        let r = repo.path.as_str();
        git(r, &["checkout", "-b", "recur/feature"]);
        write(r, "tab.txt", "on\n");
        git(r, &["add", "-A"]);
        let me = [
            ("GIT_AUTHOR_NAME", "mycodecrafting"),
            ("GIT_AUTHOR_EMAIL", "me@example.com"),
            ("GIT_COMMITTER_NAME", "mycodecrafting"),
            ("GIT_COMMITTER_EMAIL", "me@example.com"),
        ];
        git_as(
            r,
            &[
                "commit",
                "-m",
                "Enable the forwarding deposit tab",
                "-m",
                "Turn the tab on by default\nin the bridge app.",
            ],
            &me,
        );
        let (m, _root) = mgr(r);
        let commits = m.log_commits(r, "main").await.expect("log");
        assert_eq!(commits.len(), 1);
        let c = &commits[0];
        assert_eq!(
            (c.author.as_str(), c.author_role.as_str()),
            ("mycodecrafting", "Author")
        );
        assert_eq!(c.agent_code, "MY");
        assert!(!c.when.is_empty());
        assert_eq!(c.sha.len(), 40);
        assert!(!c.hash.is_empty() && c.hash != c.sha);
        assert_eq!(c.message, "Enable the forwarding deposit tab");
        assert!(
            c.body.contains("Turn the tab on by default") && c.body.contains("bridge app"),
            "{}",
            c.body
        );
    }

    #[test]
    fn classify_commit_message_cases() {
        for (msg, author, role, code) in [
            ("fix typo", "Ada", "Author", "AD"),
            ("wip", "Josh Smith", "Author", "JS"),
            ("hello", "mycodecrafting", "Author", "MY"),
            ("", "   ", "Author", "?"),
            (
                "recur(Coder): iteration 1 — add a feature",
                "Recur",
                "Coder",
                "CO",
            ),
            (
                "recur(Reviewer): iteration 2 — nits",
                "Recur",
                "Reviewer",
                "RV",
            ),
        ] {
            let (r, c, _, _) = classify_commit_message(msg, author);
            assert_eq!(
                (r.as_str(), c.as_str()),
                (role, code),
                "classify_commit_message({msg:?}, {author:?})"
            );
        }
    }

    #[test]
    fn format_commit_when_buckets() {
        let now = DateTime::parse_from_rfc3339("2026-10-20T12:00:00Z")
            .expect("now")
            .with_timezone(&Utc);
        assert_eq!(format_commit_when("2026-10-20T11:59:30Z", now), "now");
        assert_eq!(format_commit_when("2026-10-20T11:15:00+00:00", now), "45m");
        assert_eq!(format_commit_when("2026-10-20T09:00:00Z", now), "3h");
        assert_eq!(format_commit_when("2026-10-17T12:00:00Z", now), "3d");
        assert_eq!(format_commit_when("not a time", now), "now");
        assert_eq!(format_commit_when("2027-01-01T00:00:00Z", now), "now");
    }

    #[tokio::test]
    async fn change_status_matches_diff_working_vs_base_paths() {
        let repo = init_repo();
        let r = repo.path.as_str();
        write(r, "extra.txt", "delete me\n");
        write(r, "old.txt", "rename me\n");
        write_bytes(r, "img.bin", &[0x00, 0x01, 0xff, 0x89]);
        commit_as_coder(r, "base files");
        git(r, &["checkout", "-b", "recur/feat"]);
        write(r, "README.md", "hello\nchanged\n");
        write(r, "added.go", "package a\n");
        git(r, &["add", "added.go"]);
        std::fs::remove_file(format!("{r}/extra.txt")).expect("rm");
        git(r, &["mv", "old.txt", "new.txt"]);
        write(r, "untracked.go", "package u\n");
        write_bytes(r, "img.bin", &[0x00, 0x02, 0xff, 0x89, 0x0a]);
        write_bytes(r, "new.bin", &[0, 1, 2, 3, 0]);

        let st = crate::working_diff::change_status(r, "main")
            .await
            .unwrap_or_default();
        let (m, _root) = mgr(r);
        let pr = m.diff_working_vs_base(r, "main").await.expect("working");
        let got: std::collections::HashSet<&str> =
            pr.files.iter().map(|f| f.path.as_str()).collect();
        for p in st.keys() {
            assert!(
                got.contains(p.as_str()),
                "change_status has {p}, the working diff misses it: {got:?}"
            );
        }
        for p in &got {
            assert!(
                st.contains_key(*p),
                "the working diff has {p}, change_status misses it: {st:?}"
            );
        }
    }

    #[tokio::test]
    async fn diff_working_vs_base_untracked_overflow_keeps_paths() {
        let repo = init_repo();
        let r = repo.path.as_str();
        let n = UNTRACKED_MAX_FILES + 10;
        for i in 0..n {
            write(r, &format!("u/{i:03}.txt"), "x\n");
        }
        let st = crate::working_diff::change_status(r, "main")
            .await
            .unwrap_or_default();
        let (m, _root) = mgr(r);
        let pr = m.diff_working_vs_base(r, "main").await.expect("working");
        let got: std::collections::HashSet<&str> =
            pr.files.iter().map(|f| f.path.as_str()).collect();
        for p in st.keys() {
            assert!(
                got.contains(p.as_str()),
                "{p} dropped after the untracked overflow"
            );
        }
        let omitted = pr
            .files
            .iter()
            .filter(|f| {
                f.status == "u"
                    && lines(f).len() == 1
                    && lines(f)[0].code_html.contains("too many untracked")
            })
            .count();
        assert!(
            omitted > 0,
            "want omitted-note rows for {n} untracked files"
        );
    }

    #[tokio::test]
    async fn diff_working_vs_base_untracked_path_cap_signals_truncation() {
        let repo = init_repo();
        let r = repo.path.as_str();
        for i in 0..UNTRACKED_MAX_PATHS + 2 {
            write(r, &format!("u/{i:04}.txt"), "x\n");
        }
        let (m, _root) = mgr(r);
        let pr = m.diff_working_vs_base(r, "").await.expect("working");
        assert!(pr.truncated, "truncated should be set past the path cap");
        assert_eq!(
            pr.files.iter().filter(|f| f.path.starts_with("u/")).count(),
            UNTRACKED_MAX_PATHS
        );
    }

    #[tokio::test]
    async fn diff_working_vs_base_untracked_byte_budget() {
        let repo = init_repo();
        let r = repo.path.as_str();
        let chunk = "x".repeat(crate::working_diff::MAX_SYNTHETIC_ADD_BYTES);
        for i in 0..6 {
            write(r, &format!("big/{i}.txt"), &chunk);
        }
        let (m, _root) = mgr(r);
        let pr = m.diff_working_vs_base(r, "main").await.expect("working");
        let big = |f: &&FileDiff| f.path.starts_with("big/");
        let omitted = pr
            .files
            .iter()
            .filter(big)
            .filter(|f| {
                f.status == "u"
                    && lines(f).len() == 1
                    && lines(f)[0].code_html.contains("too many untracked")
            })
            .count();
        let with_body = pr
            .files
            .iter()
            .filter(big)
            .filter(|f| lines(f).len() > 1)
            .count();
        assert!(omitted > 0, "the byte budget should omit some bodies");
        assert!(with_body > 0, "files under the budget should get bodies");
    }

    #[tokio::test]
    async fn diff_working_vs_base_untracked_binary_has_no_lines() {
        let repo = init_repo();
        write_bytes(&repo.path, "pic.png", b"\x00PNG\x89binary");
        let (m, _root) = mgr(&repo.path);
        let pr = m
            .diff_working_vs_base(&repo.path, "main")
            .await
            .expect("working");
        let pic = pr
            .files
            .iter()
            .find(|f| f.path == "pic.png")
            .expect("pic.png listed");
        assert_eq!(pic.status, "a");
        assert!(pic.lines.is_none(), "an untracked binary has no lines");
    }
}
