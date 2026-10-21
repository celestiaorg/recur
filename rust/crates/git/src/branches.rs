//! Remotes, remote-tracking branches, fetch/push and base-branch choice.

use std::time::Duration;

use tokio::time::Instant;

use crate::error::{Error, Result};
use crate::worktree::{Manager, exec, is_repo};

/// The Select group heading for local heads in the base-branch picker.
pub const LOCAL_BRANCH_GROUP: &str = "Local";

/// Bounds one remote fetch so a hung network or credential prompt cannot
/// consume the whole worktree-creation budget.
pub const REMOTE_FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// Where [`Manager::fetch_pr_head`] stores GitHub's `pull/N/head`.
pub fn pr_head_ref(number: i64) -> String {
    format!("refs/recur/pr/{number}")
}

/// Matches `r` against an already-loaded remotes list (longest remote-name
/// prefix wins). Spawns no git: callers holding [`Manager::remotes`] use this
/// to avoid one subprocess per ref.
pub fn parse_remote_tracking_ref(remotes: &[String], r: &str) -> Option<(String, String)> {
    let r = r.trim();
    if r.is_empty() || remotes.is_empty() {
        return None;
    }
    let r = r.strip_prefix("refs/remotes/").unwrap_or(r);
    let mut ordered: Vec<&String> = remotes.iter().collect();
    ordered.sort_by_key(|name| std::cmp::Reverse(name.len()));
    for remote in ordered {
        if let Some(branch) = r
            .strip_prefix(remote.as_str())
            .and_then(|rest| rest.strip_prefix('/'))
        {
            if branch.is_empty() || branch == "HEAD" {
                return None;
            }
            return Some((remote.clone(), branch.to_string()));
        }
    }
    None
}

/// A branch's configured remote-tracking ref (`origin/main`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Upstream {
    /// `origin`
    pub remote: String,
    /// `main`
    pub branch: String,
    /// `origin/main`
    pub ref_name: String,
}

impl Manager {
    /// Configured remote names (origin, upstream, …).
    pub async fn remotes(&self) -> Result<Vec<String>> {
        let out = self.run(["remote"]).await?;
        Ok(out
            .split('\n')
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Remote-tracking short names (`origin/main`), newest tip first, without
    /// symbolic remote HEADs.
    pub async fn list_remote_branches(&self) -> Result<Vec<String>> {
        let out = self
            .run([
                "for-each-ref",
                "--format=%(refname:short)",
                "--sort=-committerdate",
                "refs/remotes/",
            ])
            .await?;
        Ok(out
            .split('\n')
            .map(str::trim)
            .filter(|n| !n.is_empty() && !n.ends_with("/HEAD"))
            .map(str::to_string)
            .collect())
    }

    /// `(remote, branch)` when `r` is `remote/branch` for a configured remote
    /// (longest name wins). The tracking ref need not exist yet.
    pub async fn parse_remote_tracking(&self, r: &str) -> Option<(String, String)> {
        let remotes = self.remotes().await.ok()?;
        parse_remote_tracking_ref(&remotes, r)
    }

    /// The branch part of a remote-tracking ref (`origin/main` → `main`).
    /// Local refs and unknown shapes come back trimmed.
    pub async fn short_branch_name(&self, r: &str) -> String {
        let r = r.trim();
        match self.parse_remote_tracking(r).await {
            Some((_, branch)) => branch,
            None => r.to_string(),
        }
    }

    /// `origin` when present, otherwise the first remote, otherwise empty.
    pub(crate) async fn default_remote(&self) -> String {
        let Ok(remotes) = self.remotes().await else {
            return String::new();
        };
        if remotes.iter().any(|r| r == "origin") {
            return "origin".to_string();
        }
        remotes.into_iter().next().unwrap_or_default()
    }

    /// The ref behind/update-with-base should compare against. A
    /// remote-tracking name is kept; a local name becomes `origin/<name>` when
    /// that tracking ref exists, so a stale local branch does not hide that the
    /// remote merge base moved. Local-only repos stay on the local name.
    pub async fn compare_ref(&self, base: &str) -> String {
        let base = base.trim();
        if base.is_empty() {
            return String::new();
        }
        if self.parse_remote_tracking(base).await.is_some() {
            return base.to_string();
        }
        let short = base.strip_prefix("refs/heads/").unwrap_or(base);
        let remote = self.default_remote().await;
        if remote.is_empty() {
            return base.to_string();
        }
        let r = format!("{remote}/{short}");
        if self.commit_exists(&r).await {
            r
        } else {
            base.to_string()
        }
    }

    /// Fetches the remote-tracking counterpart of `base` (best-effort) and
    /// returns [`compare_ref`](Self::compare_ref). Call before
    /// [`commits_behind`](Self::commits_behind) when the UI must know whether
    /// GitHub's merge base moved.
    pub async fn fetch_compare_ref(&self, base: &str) -> String {
        let base = base.trim();
        if base.is_empty() {
            return String::new();
        }
        let (remote, branch) = match self.parse_remote_tracking(base).await {
            Some(rb) => rb,
            None => (
                self.default_remote().await,
                base.strip_prefix("refs/heads/").unwrap_or(base).to_string(),
            ),
        };
        if !remote.is_empty() && !branch.is_empty() {
            let _ = self.fetch_remote_branch(&remote, &branch).await;
        }
        self.compare_ref(base).await
    }

    /// How far the worktree HEAD is behind the newest tip of `base`. For a
    /// local name that also has `origin/<name>`, both tips are counted and the
    /// farther wins, so a GitHub merge-base advance and a local-main commit
    /// both show. `fetch` updates the tracking tip first. Returns the ref
    /// update-with-base should merge.
    pub async fn behind_base(
        &self,
        worktree: &str,
        base: &str,
        fetch: bool,
    ) -> Result<(String, i64)> {
        let base = base.trim();
        if worktree.is_empty() || base.is_empty() {
            return Err(Error::msg("behind-base: worktree and base are required"));
        }
        let compare = if fetch {
            self.fetch_compare_ref(base).await
        } else {
            self.compare_ref(base).await
        };
        let stored = self.commits_behind(worktree, base).await;
        if compare.is_empty() || compare == base {
            return stored.map(|n| (base.to_string(), n));
        }
        let Ok(n_remote) = self.commits_behind(worktree, &compare).await else {
            return stored.map(|n| (base.to_string(), n));
        };
        match stored {
            Ok(n_stored) if n_remote <= n_stored => Ok((base.to_string(), n_stored)),
            _ => Ok((compare, n_remote)),
        }
    }

    /// Updates the tracking tip for `remote/branch` (`git fetch <remote>
    /// <branch>`), with terminal credential prompts disabled.
    pub async fn fetch_remote_branch(&self, remote: &str, branch: &str) -> Result<()> {
        self.fetch_remote_branch_by(remote, branch, None).await
    }

    async fn fetch_remote_branch_by(
        &self,
        remote: &str,
        branch: &str,
        deadline: Option<Instant>,
    ) -> Result<()> {
        let remote = remote.trim();
        let branch = branch.trim();
        if remote.is_empty() || branch.is_empty() {
            return Err(Error::msg("fetch: remote and branch are required"));
        }
        let mut cmd = self
            .git(["fetch", "--end-of-options", remote, branch])
            .env("GIT_TERMINAL_PROMPT", "0");
        if let Some(d) = deadline {
            cmd = cmd.deadline(d);
        }
        exec(cmd)
            .await
            .map_err(|e| e.wrap(format!("git fetch {remote} {branch}")))?;
        Ok(())
    }

    /// Fetches GitHub's `pull/N/head` (same-repo and fork PRs) into
    /// `refs/recur/pr/N` and returns that ref, to spin a worktree off a pull
    /// request Recur did not open.
    pub async fn fetch_pr_head(&self, number: i64) -> Result<String> {
        if number <= 0 {
            return Err(Error::msg(format!(
                "fetch PR head: invalid number {number}"
            )));
        }
        let mut remote = self.default_remote().await;
        if remote.is_empty() {
            remote = "origin".to_string();
        }
        let dst = pr_head_ref(number);
        let src = format!("+pull/{number}/head:{dst}");
        exec(
            self.git(["fetch", "--end-of-options", remote.as_str(), src.as_str()])
                .env("GIT_TERMINAL_PROMPT", "0"),
        )
        .await
        .map_err(|e| e.wrap(format!("git fetch {remote} pull/{number}/head")))?;
        if !self.commit_exists(&dst).await {
            return Err(Error::msg(format!(
                "git fetch {remote} pull/{number}/head: missing {dst}"
            )));
        }
        Ok(dst)
    }

    /// The worktree HEAD's tracking ref (`@{upstream}`); `None` when HEAD is
    /// detached or has no upstream.
    pub async fn upstream(&self, worktree: &str) -> Option<Upstream> {
        if worktree.trim().is_empty() {
            return None;
        }
        let out = self
            .run_in(worktree, ["rev-parse", "--abbrev-ref", "@{upstream}"])
            .await
            .ok()?;
        let r = out.trim();
        if r.is_empty() {
            return None;
        }
        let (remote, branch) = self.parse_remote_tracking(r).await?;
        Some(Upstream {
            remote,
            branch,
            ref_name: r.to_string(),
        })
    }

    /// Fetches the worktree's tracking remote/branch. The upstream comes back
    /// even when the fetch fails, so callers can compare against the
    /// last-known tip.
    pub async fn fetch_upstream(&self, worktree: &str) -> (Option<Upstream>, Result<()>) {
        let Some(up) = self.upstream(worktree).await else {
            return (
                None,
                Err(Error::msg("this branch is not tracking a remote")),
            );
        };
        let res = self.fetch_remote_branch(&up.remote, &up.branch).await;
        (Some(up), res)
    }

    /// Pushes HEAD to the upstream branch (`git push <remote>
    /// HEAD:refs/heads/<branch>`). The local name may differ from the upstream
    /// branch (`work` tracking `origin/main`).
    pub async fn push_upstream(&self, worktree: &str) -> Result<()> {
        let Some(up) = self.upstream(worktree).await else {
            return Err(Error::msg("this branch is not tracking a remote"));
        };
        let spec = format!("HEAD:refs/heads/{}", up.branch);
        exec(
            self.git_in(
                worktree,
                [
                    "push",
                    "--end-of-options",
                    up.remote.as_str(),
                    spec.as_str(),
                ],
            )
            .env("GIT_TERMINAL_PROMPT", "0"),
        )
        .await
        .map_err(|e| e.wrap(format!("git push {} {spec}", up.remote)))?;
        Ok(())
    }

    /// Pushes the local branch to `remote` (`refs/heads/<branch>`), after a
    /// land onto a remote-tracking base.
    pub async fn push_branch(&self, remote: &str, branch: &str) -> Result<()> {
        let remote = remote.trim();
        let branch = branch.trim();
        if remote.is_empty() || branch.is_empty() {
            return Err(Error::msg("push: remote and branch are required"));
        }
        let spec = format!("refs/heads/{branch}:refs/heads/{branch}");
        exec(
            self.git(["push", "--end-of-options", remote, spec.as_str()])
                .env("GIT_TERMINAL_PROMPT", "0"),
        )
        .await
        .map_err(|e| e.wrap(format!("git push {remote} {spec}")))?;
        Ok(())
    }

    /// A sensible default base for a new project: origin/main → main →
    /// origin/master → master → origin's HEAD → current checkout → first
    /// local → first remote → `main`.
    pub async fn suggest_base_branch(&self) -> String {
        for r in ["origin/main", "main", "origin/master", "master"] {
            if self.commit_exists(r).await {
                return r.to_string();
            }
        }
        // origin/HEAD often tracks the remote's default branch.
        for args in [
            [
                "symbolic-ref",
                "--quiet",
                "--short",
                "refs/remotes/origin/HEAD",
            ],
            ["symbolic-ref", "--quiet", "--short", "HEAD"],
        ] {
            if let Ok(out) = self.run(args).await {
                let b = out.trim();
                if !b.is_empty() && self.commit_exists(b).await {
                    return b.to_string();
                }
            }
        }
        if let Ok(locals) = self.list_local_branches().await
            && let Some(b) = locals.into_iter().find(|b| !b.is_empty())
        {
            return b;
        }
        if let Ok(remotes) = self.list_remote_branches().await
            && let Some(b) = remotes.into_iter().find(|b| !b.is_empty())
        {
            return b;
        }
        "main".to_string()
    }

    /// Fetches a remote-tracking base before spinning a worktree off it, with
    /// a short sub-timeout. On failure, a fresh deadline checks whether a
    /// stale tip is still usable.
    pub(crate) async fn refresh_remote_base_if_needed(&self, base_branch: &str) -> Result<()> {
        let Some((remote, branch)) = self.parse_remote_tracking(base_branch).await else {
            return Ok(());
        };
        let Err(err) = self
            .fetch_remote_branch_by(
                &remote,
                &branch,
                Some(Instant::now() + REMOTE_FETCH_TIMEOUT),
            )
            .await
        else {
            return Ok(());
        };
        if self
            .commit_exists_by(base_branch, Some(Instant::now() + Duration::from_secs(5)))
            .await
        {
            return Ok(());
        }
        Err(err)
    }
}

/// [`Manager::suggest_base_branch`] for any checkout path. Non-git and empty
/// paths return `main`.
pub async fn suggest_base_branch_for_repo(repo_path: &str) -> String {
    let repo_path = repo_path.trim();
    if repo_path.is_empty() || !is_repo(repo_path).await {
        return "main".to_string();
    }
    Manager::new(repo_path, "").suggest_base_branch().await
}
