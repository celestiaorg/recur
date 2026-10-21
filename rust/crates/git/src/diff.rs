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
use crate::highlight::highlight_files;
use crate::text::{atoi, go_lower, go_string, html_escape};
use crate::version::{log_numstat_flags, probe};
use crate::working_diff::{
    MAX_SYNTHETIC_ADD_BYTES, merge_base, omitted_untracked_diff, read_capped, synthetic_add_diff,
};
use crate::worktree::Manager;

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
            let mb = go_string(&out);
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
            let stderr = go_string(e.stderr());
            let msg = stderr.trim();
            if matches!(e.kind(), ErrorKind::Exit { .. }) && !msg.is_empty() {
                e.wrap(format!("git diff --numstat: {msg}"))
            } else {
                e.wrap("git diff --numstat")
            }
        })?;
    let (add, del) = parse_numstat(&go_string(&out));
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
/// rolled-up stats. Empty `files` is Go's nil slice.
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
        let n = out.trim().parse::<i64>().map_err(|e| {
            Error::msg(format!(
                "parse rev-list count {}: strconv.Atoi: parsing {}: {e}",
                crate::text::quote(out.trim()),
                crate::text::quote(out.trim())
            ))
        })?;
        Ok(n.max(0))
    }

    /// The branch's own changes versus the base (`git diff <merge-base> HEAD`),
    /// parsed and highlighted. An up-to-date branch is an empty diff, not an
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
        Ok(PrDiff::new(parse_highlighted_patch(&out), false))
    }

    /// The Observatory files-changed view: the working tree (staged, unstaged
    /// and untracked text files) versus the merge-base with `base_branch`, or
    /// HEAD when empty (the empty tree when HEAD is unborn). Lines are plain
    /// escaped: highlighting a 100-file working diff froze the review.
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
        let mut files = parse_unified_diff_plain(&out);
        let seen: HashSet<String> = files.iter().map(|f| f.path.clone()).collect();
        let (mut bodies, mut bytes_read) = (0, 0);
        let (untracked, list_truncated) = untracked_rel_paths(worktree).await;
        for rel in untracked {
            let shown = go_string(&rel);
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

    /// One commit's own diff (`git show --format= <hash>`), highlighted. A
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
        Ok(parse_highlighted_patch(&out))
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
    let oid = go_string(&out).trim().to_string();
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
    let low = go_lower(msg);
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
            .filter(|&c| crate::chroma::is_letter(c) || crate::chroma::is_number(c))
            .map(crate::chroma::to_upper)
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

/// The fast path: HTML-escaped code, no highlighting. Callers that want
/// highlighting run [`highlight_files`] afterward (capped).
pub fn parse_unified_diff_plain(diff: &str) -> Vec<FileDiff> {
    parse_unified_diff(diff)
}

/// Escapes, then highlights within [`highlight_files`]' caps, so a large
/// commit cannot highlight unboundedly while the UI waits.
pub(crate) fn parse_highlighted_patch(diff: &str) -> Vec<FileDiff> {
    let mut files = parse_unified_diff_plain(diff);
    highlight_files(&mut files);
    files
}

fn push_line(f: &mut FileDiff, line: DiffLine) {
    f.lines.get_or_insert_with(Vec::new).push(line);
}

fn parse_unified_diff(diff: &str) -> Vec<FileDiff> {
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
