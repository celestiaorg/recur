//! The working copy versus a base: change letters, one file's diff, and file
//! contents at a revision.

use std::path::Path;

use recur_domain::{DiffLine, FileDiff};
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;

use crate::command::command;
use crate::diff::parse_unified_diff_plain;
use crate::error::{Error, Result};
use crate::status::{
    CHANGE_STATUS_FLIGHTS, StatusLetters, hooks, parse_status_porcelain, status_map,
};
use crate::text::go_string;
use recur_chroma::{highlight_files, html_escape};

/// Caps for the synthetic diff of an untracked or new file: bytes read and
/// lines rendered are both bounded.
pub(crate) const MAX_SYNTHETIC_ADD_BYTES: usize = 512 * 1024;
const MAX_SYNTHETIC_ADD_LINES: usize = 4000;

/// The merge-base of `base_branch` and HEAD in `dir`, falling back to
/// `base_branch` when there is none (unrelated histories, missing ref).
pub async fn merge_base(dir: &str, base_branch: &str) -> String {
    merge_base_by(dir, base_branch, None).await
}

async fn merge_base_by(dir: &str, base_branch: &str, deadline: Option<Instant>) -> String {
    let base_branch = base_branch.trim();
    if dir.is_empty() || base_branch.is_empty() {
        return base_branch.to_string();
    }
    let mut cmd = command([
        "-C",
        dir,
        "merge-base",
        "--end-of-options",
        base_branch,
        "HEAD",
    ]);
    if let Some(d) = deadline {
        cmd = cmd.deadline(d);
    }
    match cmd.output().await {
        Ok(out) => {
            let mb = go_string(&out);
            let mb = mb.trim();
            if mb.is_empty() {
                base_branch.to_string()
            } else {
                mb.to_string()
            }
        }
        Err(_) => base_branch.to_string(),
    }
}

/// Path → `m|a|d|u` for changes since `base_branch`'s merge-base with HEAD,
/// working tree included: everything that differs from where the worktree
/// spun off. An empty base falls back to porcelain status versus HEAD.
/// Concurrent calls for the same directory and base share one computation.
pub async fn change_status(dir: &str, base_branch: &str) -> Option<StatusLetters> {
    let base_branch = base_branch.trim();
    if dir.is_empty() {
        return None;
    }
    if base_branch.is_empty() {
        return status_map(dir).await;
    }
    let key = format!("{dir}\0{base_branch}");
    let (dir, base) = (dir.to_string(), base_branch.to_string());
    CHANGE_STATUS_FLIGHTS
        .run(key, move |deadline| change_status_work(dir, base, deadline))
        .await
}

async fn change_status_work(
    dir: String,
    base_branch: String,
    deadline: Instant,
) -> Option<StatusLetters> {
    hooks::fire("vs-base", &dir).await;
    let mb = merge_base_by(&dir, &base_branch, Some(deadline)).await;
    let out = command([
        "-C",
        dir.as_str(),
        "diff",
        "--name-status",
        "--no-renames",
        "--end-of-options",
        mb.as_str(),
    ])
    .deadline(deadline)
    .output()
    .await;
    let Ok(out) = out else {
        // Fall back to porcelain so the UI still shows something.
        return tokio::time::timeout_at(deadline, status_map(&dir))
            .await
            .ok()
            .flatten();
    };
    let mut m = parse_name_status(&go_string(&out));
    // Untracked files never appear in a diff against the base; fold them in.
    if let Ok(Some(st)) = tokio::time::timeout_at(deadline, status_map(&dir)).await {
        for (p, letter) in st {
            if letter == "u" {
                m.entry(p).or_insert_with(|| "u".to_string());
            }
        }
    }
    (!m.is_empty()).then_some(m)
}

fn parse_name_status(out: &str) -> StatusLetters {
    let mut m = StatusLetters::new();
    let out = out.trim();
    if out.is_empty() {
        return m;
    }
    for line in out.split('\n') {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((code, path)) = line.split_once('\t') else {
            continue;
        };
        let path = path.strip_suffix('/').unwrap_or(path);
        if path.is_empty() {
            continue;
        }
        let letter = match code.as_bytes().first() {
            Some(b'A' | b'C') => "a",
            Some(b'D') => "d",
            _ => "m",
        };
        m.insert(path.to_string(), letter.to_string());
    }
    m
}

/// One path's diff versus `base_branch`'s merge-base with HEAD (HEAD when
/// empty), working tree included. Untracked files render as a full-file
/// addition. Only the first [`recur_chroma::MAX_HIGHLIGHT_LINES`] lines
/// are highlighted.
pub async fn working_diff(dir: &str, base_branch: &str, rel: &str) -> Result<FileDiff> {
    let rel = rel.strip_prefix('/').unwrap_or(rel);
    if dir.is_empty() || rel.is_empty() {
        return Err(Error::msg("working diff: dir and path required"));
    }
    let mut against = "HEAD".to_string();
    let b = base_branch.trim();
    if !b.is_empty() {
        against = merge_base(dir, b).await;
    }

    if let Ok(out) = command([
        "-C",
        dir,
        "diff",
        "--no-color",
        "--end-of-options",
        against.as_str(),
        "--",
        rel,
    ])
    .output()
    .await
    {
        let out = go_string(&out);
        if !out.trim().is_empty()
            && let Some(fd) = first_highlighted(&out)
        {
            return Ok(fd);
        }
    }

    // A cheap single-path porcelain instead of a full change scan.
    let st = path_porcelain(dir, rel).await;
    if st.is_empty() {
        // Clean, or not a work tree. Never `diff --cached` here: outside a repo
        // git implies --no-index and rejects --cached.
        return Err(Error::msg(format!("no diff for {rel} against {against}")));
    }
    // Staged-only changes can be invisible to the working-tree diff.
    if st != "u"
        && st != "a"
        && st != "?"
        && let Ok(cached) = command([
            "-C",
            dir,
            "diff",
            "--no-color",
            "--cached",
            "--end-of-options",
            against.as_str(),
            "--",
            rel,
        ])
        .output()
        .await
        && let Some(fd) = first_highlighted(&go_string(&cached))
    {
        return Ok(fd);
    }

    // Untracked or new: render the whole (bounded) file as an addition.
    let abs = crate::gopath::join(&[dir, rel]);
    let (body, truncated) = read_capped(Path::new(&abs), MAX_SYNTHETIC_ADD_BYTES)
        .await
        .map_err(|e| e.wrap(format!("read {rel}")))?;
    let mut files = vec![synthetic_add_diff(rel, &body, truncated)];
    highlight_files(&mut files);
    Ok(files.remove(0))
}

fn first_highlighted(diff: &str) -> Option<FileDiff> {
    let mut files = parse_unified_diff_plain(diff);
    if files.is_empty() {
        return None;
    }
    files.truncate(1);
    highlight_files(&mut files);
    files.pop()
}

/// The status letter for one path, or empty.
async fn path_porcelain(dir: &str, rel: &str) -> String {
    let Ok(out) = command(["-C", dir, "status", "--porcelain", "-uall", "--", rel])
        .output()
        .await
    else {
        return String::new();
    };
    let m = parse_status_porcelain(&go_string(&out)).unwrap_or_default();
    if let Some(s) = m.get(rel).filter(|s| !s.is_empty()) {
        return s.clone();
    }
    // An untracked directory may be reported for the path; take any hit.
    m.into_values().next().unwrap_or_default()
}

/// Reads at most `limit` bytes of `path`, reporting whether it was longer.
pub(crate) async fn read_capped(path: &Path, limit: usize) -> Result<(Vec<u8>, bool)> {
    let shown = path.to_string_lossy();
    let f = tokio::fs::File::open(path)
        .await
        .map_err(|e| Error::io_path("open", &shown, &e))?;
    let mut buf = Vec::new();
    // One extra byte, so a file exactly at the limit is not truncated.
    f.take(limit as u64 + 1)
        .read_to_end(&mut buf)
        .await
        .map_err(|e| Error::io_path("read", &shown, &e))?;
    if buf.len() > limit {
        buf.truncate(limit);
        return Ok((buf, true));
    }
    Ok((buf, false))
}

pub(crate) fn omitted_untracked_diff(path: &str) -> FileDiff {
    FileDiff {
        path: path.to_string(),
        status: "u".into(),
        lines: Some(vec![DiffLine {
            r#type: "hunk".into(),
            code_html: "diff omitted — too many untracked files".into(),
            ..DiffLine::default()
        }]),
        ..FileDiff::default()
    }
}

fn content_looks_binary(content: &[u8]) -> bool {
    !content.is_empty() && (content.contains(&0) || std::str::from_utf8(content).is_err())
}

/// A whole new file as an addition (bounded); binary content has no lines.
pub(crate) fn synthetic_add_diff(path: &str, content: &[u8], truncated: bool) -> FileDiff {
    let mut f = FileDiff {
        path: path.to_string(),
        status: "a".into(),
        ..FileDiff::default()
    };
    if content_looks_binary(content) {
        return f;
    }
    let text = go_string(content).replace("\r\n", "\n");
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let mut truncated = truncated;
    if lines.len() > MAX_SYNTHETIC_ADD_LINES {
        lines.truncate(MAX_SYNTHETIC_ADD_LINES);
        truncated = true;
    }
    let mut out = vec![DiffLine {
        r#type: "hunk".into(),
        code_html: format!("@@ -0,0 +1,{} @@", lines.len()),
        ..DiffLine::default()
    }];
    for (i, line) in lines.iter().enumerate() {
        out.push(DiffLine {
            new: (i + 1).to_string(),
            r#type: "add".into(),
            code_html: html_escape(line),
            ..DiffLine::default()
        });
        f.diff.add += 1;
    }
    if truncated {
        out.push(DiffLine {
            r#type: "hunk".into(),
            code_html: format!(
                "@@ truncated — showing the first {} lines of a new file @@",
                lines.len()
            ),
            ..DiffLine::default()
        });
    }
    f.lines = Some(out);
    f
}

fn rev_path_spec(dir: &str, rev: &str, rel: &str) -> Result<String> {
    let rel = rel.strip_prefix('/').unwrap_or(rel);
    let rev = rev.trim();
    if dir.is_empty() || rev.is_empty() || rel.is_empty() {
        return Err(Error::msg("git rev path: dir, rev, and path required"));
    }
    Ok(format!("{rev}:{rel}"))
}

/// The object size of `rel` at `rev` without reading the blob; `None` when
/// the path does not exist at that revision.
pub async fn file_size_at_rev(dir: &str, rev: &str, rel: &str) -> Result<Option<i64>> {
    let spec = rev_path_spec(dir, rev, rel)?;
    let Ok(out) = command([
        "-C",
        dir,
        "cat-file",
        "-s",
        "--end-of-options",
        spec.as_str(),
    ])
    .output()
    .await
    else {
        return Ok(None);
    };
    let s = go_string(&out);
    let n: i64 = s.trim().parse().map_err(|e| {
        Error::msg(format!(
            "file size: {}",
            crate::text::num_error("ParseInt", s.trim(), &e)
        ))
    })?;
    if n < 0 {
        return Err(Error::msg("file size: negative"));
    }
    Ok(Some(n))
}

/// Streams `rel` at `rev` into `dst`, failing after `limit` bytes. `false`
/// when the path does not exist at that revision.
pub async fn copy_file_at_rev<W: AsyncWrite + Unpin>(
    dir: &str,
    rev: &str,
    rel: &str,
    dst: &mut W,
    limit: i64,
) -> Result<bool> {
    let spec = rev_path_spec(dir, rev, rel)?;
    let limit = limit.max(0) as u64;
    let mut running =
        command(["-C", dir, "show", "--end-of-options", spec.as_str()]).spawn_stdout()?;
    let mut copied: u64 = 0;
    let mut copy_err = None;
    if let Some(stdout) = running.stdout.as_mut() {
        let mut reader = stdout.take(limit + 1);
        let mut buf = [0u8; 32 * 1024];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if let Err(e) = dst.write_all(&buf[..n]).await {
                        copy_err = Some(e);
                        break;
                    }
                    copied += n as u64;
                }
                Err(e) => {
                    copy_err = Some(e);
                    break;
                }
            }
        }
    }
    if copied > limit {
        running.kill().await;
        return Err(Error::msg(format!("file too large ({copied} bytes)")));
    }
    if running.wait().await.is_err() {
        return Ok(false);
    }
    if let Some(e) = copy_err {
        return Err(e.into());
    }
    Ok(true)
}

/// The contents of `rel` at `rev` (e.g. a merge-base hash) as raw bytes;
/// `None` when the path does not exist there (added or untracked).
pub async fn show_file(dir: &str, rev: &str, rel: &str) -> Result<Option<Vec<u8>>> {
    let rel = rel.strip_prefix('/').unwrap_or(rel);
    let rev = rev.trim();
    if dir.is_empty() || rev.is_empty() || rel.is_empty() {
        return Err(Error::msg("show file: dir, rev, and path required"));
    }
    let spec = format!("{rev}:{rel}");
    Ok(
        command(["-C", dir, "show", "--end-of-options", spec.as_str()])
            .output()
            .await
            .ok(),
    )
}

/// `rel` at the merge-base of `base_branch` and HEAD (HEAD when empty);
/// empty when the file did not exist there.
pub async fn file_at_base(dir: &str, base_branch: &str, rel: &str) -> Result<Vec<u8>> {
    let mut against = "HEAD".to_string();
    let b = base_branch.trim();
    if !b.is_empty() {
        against = merge_base(dir, b).await;
    }
    Ok(show_file(dir, &against, rel).await?.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{git, temp_dir, write};

    fn repo_with(files: &[(&str, &str)]) -> crate::testutil::Dir {
        let root = temp_dir();
        git(&root.path, &["init", "-b", "main"]);
        for (f, c) in files {
            write(&root.path, f, c);
        }
        git(&root.path, &["add", "."]);
        git(&root.path, &["commit", "-m", "init"]);
        root
    }

    #[tokio::test]
    async fn working_diff_modified() {
        let root = repo_with(&[("a.go", "package a\n")]);
        write(&root.path, "a.go", "package a\nfunc X() {}\n");
        let fd = working_diff(&root.path, "main", "a.go")
            .await
            .expect("working_diff");
        assert_eq!(fd.status, "m");
        assert!(fd.diff.add >= 1, "{:?}", fd.diff);
    }

    #[tokio::test]
    async fn working_diff_clean_path_no_cached_outside_repo() {
        // Outside a repo `git diff --cached` fails with "unknown option
        // cached"; working_diff must not take that path.
        let root = temp_dir();
        write(&root.path, "a.txt", "x\n");
        let err = working_diff(&root.path, "", "a.txt")
            .await
            .expect_err("non-git dir");
        assert!(
            !err.message().contains("cached") && !err.message().contains("no-index"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn working_diff_untracked() {
        let root = repo_with(&[("seed.go", "package s\n")]);
        write(&root.path, "new.go", "package n\nfunc Y() {}\n");
        let fd = working_diff(&root.path, "main", "new.go")
            .await
            .expect("working_diff");
        assert_eq!(fd.status, "a");
        assert!(
            fd.diff.add >= 1 && fd.lines.as_ref().is_some_and(|l| l.len() >= 2),
            "{fd:?}"
        );
    }

    #[tokio::test]
    async fn file_at_base_and_rev_helpers() {
        let root = repo_with(&[("a.go", "package a\n")]);
        let r = root.path.as_str();
        git(r, &["checkout", "-b", "recur/feat"]);
        write(r, "a.go", "package a\nfunc X() {}\n");
        git(r, &["add", "."]);
        git(r, &["commit", "-m", "feat"]);

        assert_eq!(
            file_at_base(r, "main", "a.go").await.expect("base"),
            b"package a\n"
        );
        assert_eq!(
            file_at_base(r, "main", "nope.go").await.expect("missing"),
            b""
        );

        let against = merge_base(r, "main").await;
        assert_eq!(
            file_size_at_rev(r, &against, "a.go").await.expect("size"),
            Some("package a\n".len() as i64)
        );
        let mut buf = Vec::new();
        assert!(
            copy_file_at_rev(r, &against, "a.go", &mut buf, 1 << 20)
                .await
                .expect("copy")
        );
        assert_eq!(buf, b"package a\n");
        assert_eq!(
            file_size_at_rev(r, &against, "nope.go")
                .await
                .expect("missing size"),
            None
        );
        let mut small = Vec::new();
        let err = copy_file_at_rev(r, &against, "a.go", &mut small, 3)
            .await
            .expect_err("over the limit");
        assert_eq!(err.message(), "file too large (4 bytes)");
    }

    #[tokio::test]
    async fn change_status_versus_base_includes_committed() {
        let root = repo_with(&[("base.go", "package b\n")]);
        let r = root.path.as_str();
        git(r, &["checkout", "-b", "recur/feat"]);
        write(r, "feat.go", "package f\n");
        git(r, &["add", "."]);
        git(r, &["commit", "-m", "feat"]);
        let st = change_status(r, "main").await.unwrap_or_default();
        assert_eq!(st.get("feat.go").map(String::as_str), Some("a"), "{st:?}");
        assert_eq!(status_map(r).await, None, "porcelain should be clean");
        let fd = working_diff(r, "main", "feat.go")
            .await
            .expect("working_diff");
        assert!(fd.status == "a" && fd.diff.add >= 1, "{fd:?}");
    }

    #[test]
    fn synthetic_add_diff_skips_binary() {
        let bin = synthetic_add_diff("x.png", b"\x00PNG", false);
        assert_eq!(bin.status, "a");
        assert!(
            bin.lines.is_none(),
            "binary synthetic add should have no lines"
        );
        let text = synthetic_add_diff("a.txt", b"hello\n", false);
        assert!(
            text.lines.as_ref().is_some_and(|l| l.len() >= 2),
            "text synthetic add should have hunk+line"
        );
    }

    #[test]
    fn parse_name_status_letters() {
        let m = parse_name_status(
            "A\tnew.go\nD\tgone.go\nM\tsrc/x.go\nC100\tcopy.go\nT\tlink\nX\tweird\n",
        );
        let get = |k: &str| m.get(k).map(String::as_str);
        assert_eq!(get("new.go"), Some("a"));
        assert_eq!(get("gone.go"), Some("d"));
        assert_eq!(get("src/x.go"), Some("m"));
        assert_eq!(get("copy.go"), Some("a"));
        assert_eq!(get("link"), Some("m"));
        assert_eq!(get("weird"), Some("m"));
    }
}
