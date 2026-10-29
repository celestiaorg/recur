//! Source-control actions on a working copy: identity, discard, stage,
//! commit the index, staged diff and recent subjects.

use std::collections::HashMap;

use recur_domain::path;

use crate::error::{Error, Result};
use crate::status::{status_map, status_map_now};
use crate::worktree::{CommitResult, Manager, is_repo, remove_all};

/// git's effective author in `dir` (`git var GIT_AUTHOR_IDENT`), so env, repo
/// and global config match a plain `git commit`. No fallback: when git cannot
/// resolve an identity (e.g. `user.useConfigOnly` without `user.email`) this
/// is an error.
pub async fn identity(dir: &str) -> Result<(String, String)> {
    let out = run_git(dir, ["var", "GIT_AUTHOR_IDENT"])
        .await
        .map_err(|e| e.wrap("git identity"))?;
    let ident = out.trim();
    parse_git_ident(ident)
        .ok_or_else(|| Error::msg(format!("git identity: unparseable {:?}", ident)))
}

/// Splits `Name <email> timestamp tz` from `git var GIT_*_IDENT`.
fn parse_git_ident(s: &str) -> Option<(String, String)> {
    let i = s.rfind("> ")?;
    let lt = s[..i].rfind(" <")?;
    let name = s[..lt].trim();
    let email = &s[lt + 2..i];
    if name.is_empty() || email.is_empty() {
        return None;
    }
    Some((name.to_string(), email.to_string()))
}

/// Porcelain status versus HEAD (uncommitted only).
pub async fn working_status(dir: &str) -> Option<HashMap<String, String>> {
    status_map(dir).await
}

/// Restores a tracked path to HEAD, or removes an untracked one.
pub async fn discard_file(dir: &str, rel: &str) -> Result<()> {
    if !is_repo(dir).await {
        return Err(Error::msg("not a git repository"));
    }
    let rel = rel.strip_prefix('/').unwrap_or(rel);
    if rel.is_empty() || rel.contains("..") {
        return Err(Error::msg("invalid path"));
    }
    // Always a fresh status: joining an in-flight poll could see a snapshot
    // from before the caller's own write.
    let st = status_map_now(dir).await.unwrap_or_default();
    let Some(letter) = st.get(rel) else {
        return Ok(()); // already clean versus HEAD
    };
    let full = path::join(&[dir, rel]);
    if letter == "u" {
        return remove_all(&full).map_err(|e| Error::io_path(&full, &e));
    }
    // Restore a tracked file (also resurrects a deleted one).
    if let Err(e) = run_git(
        dir,
        [
            "restore",
            "--source=HEAD",
            "--staged",
            "--worktree",
            "--",
            rel,
        ],
    )
    .await
    {
        // Older git: checkout.
        if run_git(dir, ["checkout", "HEAD", "--", rel]).await.is_err() {
            return Err(e.wrap(format!("discard {rel}")));
        }
    }
    Ok(())
}

/// Hard-resets to HEAD and removes untracked files.
pub async fn discard_all(dir: &str) -> Result<()> {
    if !is_repo(dir).await {
        return Err(Error::msg("not a git repository"));
    }
    run_git(dir, ["reset", "--hard", "HEAD"])
        .await
        .map_err(|e| e.wrap("git reset --hard"))?;
    run_git(dir, ["clean", "-fd"])
        .await
        .map_err(|e| e.wrap("git clean"))?;
    Ok(())
}

/// Adds `paths` to the index.
pub async fn stage(dir: &str, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    if !is_repo(dir).await {
        return Err(Error::msg("not a git repository"));
    }
    let args = ["add", "--"]
        .into_iter()
        .map(str::to_string)
        .chain(paths.iter().cloned());
    run_git(dir, args).await.map_err(|e| e.wrap("git add"))?;
    Ok(())
}

/// Removes `paths` from the index, keeping the working tree.
pub async fn unstage(dir: &str, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    if !is_repo(dir).await {
        return Err(Error::msg("not a git repository"));
    }
    let restore = ["restore", "--staged", "--"]
        .into_iter()
        .map(str::to_string)
        .chain(paths.iter().cloned());
    if let Err(e) = run_git(dir, restore).await {
        let reset = ["reset", "HEAD", "--"]
            .into_iter()
            .map(str::to_string)
            .chain(paths.iter().cloned());
        if run_git(dir, reset).await.is_err() {
            return Err(e.wrap("git unstage"));
        }
    }
    Ok(())
}

impl Manager {
    /// Commits only the index (no `git add -A`). A no-op when nothing is staged.
    pub async fn commit_staged(
        &self,
        worktree: &str,
        message: &str,
        author_name: &str,
        author_email: &str,
    ) -> Result<CommitResult> {
        if !is_repo(worktree).await {
            return Err(Error::msg("not a git repository"));
        }
        // `diff --cached --quiet` exits 1 when something is staged.
        if run_git(worktree, ["diff", "--cached", "--quiet"])
            .await
            .is_ok()
        {
            return Ok(CommitResult::default());
        }
        let (additions, deletions) = self.staged_diff_stat(worktree).await;
        self.commit_with_identity(worktree, message, author_name, author_email)
            .await
            .map_err(|e| e.wrap("git commit"))?;
        let hash = self
            .run_in(worktree, ["rev-parse", "--short", "HEAD"])
            .await
            .unwrap_or_default();
        Ok(CommitResult {
            hash: hash.trim().to_string(),
            committed: true,
            additions,
            deletions,
        })
    }
}

/// Paths in the index (staged for commit). A plain (non-git) folder returns
/// none rather than git's `--no-index` "unknown option cached" failure.
pub async fn staged_paths(dir: &str) -> Result<Vec<String>> {
    if dir.is_empty() || !is_repo(dir).await {
        return Ok(Vec::new());
    }
    let out = run_git(dir, ["diff", "--cached", "--name-only", "-z"]).await?;
    if out.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(out
        .split('\0')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect())
}

/// The index versus HEAD as a unified diff (`git diff --cached`). Empty
/// means nothing is staged (or `dir` is not a work tree).
pub async fn staged_diff(dir: &str) -> Result<String> {
    if dir.is_empty() || !is_repo(dir).await {
        return Ok(String::new());
    }
    run_git(dir, ["diff", "--cached", "--"])
        .await
        .map_err(|e| e.wrap("git diff --cached"))
}

/// Up to `n` recent commit subjects, newest first. Best-effort: empty on
/// failure so callers can still generate without style hints.
pub async fn recent_commit_subjects(dir: &str, n: i64) -> Vec<String> {
    if n <= 0 {
        return Vec::new();
    }
    let count = format!("-{n}");
    match run_git(dir, ["log", count.as_str(), "--pretty=%s"]).await {
        Ok(out) if !out.trim().is_empty() => out
            .split('\n')
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// git `-C dir` from the current directory (Go `runGit`).
pub(crate) async fn run_git<I, S>(dir: &str, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    Manager::default().run_in(dir, args).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{init_repo, temp_dir, write};

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn staged_paths_non_git() {
        let dir = temp_dir();
        write(&dir.path, "a.txt", "x\n");
        assert_eq!(
            staged_paths(&dir.path).await.expect("staged_paths"),
            Vec::<String>::new()
        );
        assert_eq!(staged_diff(&dir.path).await.expect("staged_diff"), "");
        let err = stage(&dir.path, &strings(&["a.txt"]))
            .await
            .expect_err("stage outside a repo");
        assert!(err.message().contains("not a git repository"), "{err}");
    }

    #[tokio::test]
    async fn discard_and_human_commit() {
        let repo = init_repo();
        let root = temp_dir();
        let m = Manager::new(&repo.path, &root.path);
        let (wt, _) = m.add("LOOP-SCM", "scm-test", "main").await.expect("add");
        let w = wt.path.as_str();

        write(w, "edit.txt", "hello\n");
        let st = working_status(w).await.unwrap_or_default();
        assert_eq!(st.get("edit.txt").map(String::as_str), Some("u"), "{st:?}");
        discard_file(w, "edit.txt")
            .await
            .expect("discard untracked");
        assert!(
            std::fs::metadata(format!("{w}/edit.txt")).is_err(),
            "untracked file should be removed"
        );

        write(w, "README.md", "base\n");
        m.commit(w, "seed", "Coder", "coder@recur.local")
            .await
            .expect("seed");
        write(w, "README.md", "changed\n");
        discard_file(w, "README.md").await.expect("discard tracked");
        assert_eq!(
            std::fs::read_to_string(format!("{w}/README.md")).unwrap_or_default(),
            "base\n"
        );

        write(w, "yours.txt", "x\n");
        let (name, email) = identity(w).await.expect("identity");
        assert!(!name.is_empty() && !email.is_empty());
        assert!(
            m.commit(w, "my quick fix", &name, &email)
                .await
                .expect("commit")
                .committed
        );

        write(w, "a.txt", "a\n");
        write(w, "b.txt", "b\n");
        stage(w, &strings(&["a.txt"])).await.expect("stage");
        assert_eq!(staged_paths(w).await.expect("staged"), vec!["a.txt"]);
        assert!(
            m.commit_staged(w, "only a", &name, &email)
                .await
                .expect("commit_staged")
                .committed
        );
        let st = working_status(w).await.unwrap_or_default();
        assert_eq!(
            st.get("b.txt").map(String::as_str),
            Some("u"),
            "b.txt should stay untracked: {st:?}"
        );
        discard_all(w).await.expect("discard_all");
        assert_eq!(working_status(w).await, None, "discard_all left changes");
    }

    #[tokio::test]
    async fn staged_diff_and_recent_subjects() {
        let repo = init_repo();
        let root = temp_dir();
        let m = Manager::new(&repo.path, &root.path);
        let (wt, _) = m
            .add("LOOP-DIFF", "staged-diff", "main")
            .await
            .expect("add");
        let w = wt.path.as_str();
        assert_eq!(staged_diff(w).await.expect("staged_diff").trim(), "");
        write(w, "new.txt", "hello\n");
        stage(w, &strings(&["new.txt"])).await.expect("stage");
        let diff = staged_diff(w).await.expect("staged_diff");
        assert!(
            diff.contains("new.txt") && diff.contains("+hello"),
            "{diff}"
        );
        let (name, email) = identity(w).await.expect("identity");
        m.commit_staged(w, "add new.txt helper", &name, &email)
            .await
            .expect("commit_staged");
        let subjects = recent_commit_subjects(w, 5).await;
        assert_eq!(
            subjects.first().map(String::as_str),
            Some("add new.txt helper"),
            "{subjects:?}"
        );
    }

    #[tokio::test]
    async fn unstage_keeps_the_file() {
        let repo = init_repo();
        let root = temp_dir();
        let m = Manager::new(&repo.path, &root.path);
        let (wt, _) = m.add("LOOP-US", "unstage", "main").await.expect("add");
        let w = wt.path.as_str();
        write(w, "s.txt", "s\n");
        stage(w, &strings(&["s.txt"])).await.expect("stage");
        unstage(w, &strings(&["s.txt"])).await.expect("unstage");
        assert!(staged_paths(w).await.expect("staged").is_empty());
        assert!(
            std::fs::metadata(format!("{w}/s.txt")).is_ok(),
            "file should stay on disk, untracked"
        );
    }

    #[test]
    fn parse_git_ident_splits_name_and_email() {
        assert_eq!(
            parse_git_ident("Jane Q <jane@example.com> 1700000000 +0100"),
            Some(("Jane Q".to_string(), "jane@example.com".to_string()))
        );
        assert_eq!(parse_git_ident("no email here"), None);
        assert_eq!(parse_git_ident(" <a@b> 1 +0000"), None);
    }
}
