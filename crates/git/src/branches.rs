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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{
        config_identity, git, init_repo, init_repo_with_remote, rev_parse, temp_dir, write,
    };

    fn show_file(repo: &str, spec: &str) -> String {
        crate::testutil::git_out(repo, &["show", spec])
            .trim()
            .to_string()
    }

    #[tokio::test]
    async fn list_remote_branches_and_parse() {
        let r = init_repo_with_remote();
        let root = temp_dir();
        let m = Manager::new(&r.clone, &root.path);
        assert_eq!(m.remotes().await.expect("remotes"), vec!["origin"]);
        let remote_branches = m.list_remote_branches().await.expect("remote branches");
        assert!(
            remote_branches.iter().all(|b| !b.ends_with("/HEAD")),
            "{remote_branches:?}"
        );
        assert!(
            remote_branches.contains(&"origin/main".to_string()),
            "{remote_branches:?}"
        );
        assert_eq!(
            m.parse_remote_tracking("origin/main").await,
            Some(("origin".into(), "main".into()))
        );
        assert_eq!(m.short_branch_name("origin/main").await, "main");
        assert_eq!(m.short_branch_name("main").await, "main");
        assert_eq!(m.parse_remote_tracking("main").await, None);
    }

    #[tokio::test]
    async fn base_branch_exists_cases() {
        let r = init_repo_with_remote();
        let root = temp_dir();
        let m = Manager::new(&r.clone, &root.path);
        git(&r.clone, &["tag", "v1.0"]);
        let sha = rev_parse(&r.clone, "HEAD");
        for (name, want) in [
            ("main", true),
            ("origin/main", true),
            ("HEAD", false),
            ("origin/HEAD", false),
            ("v1.0", false),
            (sha.as_str(), false),
            ("does-not-exist", false),
            ("", false),
        ] {
            assert_eq!(
                m.base_branch_exists(name).await,
                want,
                "base_branch_exists({name:?})"
            );
        }
    }

    #[tokio::test]
    async fn add_fetches_remote_base() {
        let r = init_repo_with_remote();
        let pusher_dir = temp_dir();
        let pusher = format!("{}/pusher", pusher_dir.path);
        git(&pusher_dir.path, &["clone", &r.bare, &pusher]);
        write(&pusher, "newer.txt", "from upstream\n");
        git(&pusher, &["add", "."]);
        git(&pusher, &["commit", "-m", "upstream advance"]);
        git(&pusher, &["push", "origin", "main"]);

        let root = temp_dir();
        let m = Manager::new(&r.clone, &root.path);
        assert!(
            m.run(["cat-file", "-e", "origin/main:newer.txt"])
                .await
                .is_err(),
            "stale origin/main has newer.txt"
        );
        let (wt, created) = m
            .add("ws-fetch", "ws-fetch", "origin/main")
            .await
            .expect("add off origin/main");
        assert!(created);
        let body = std::fs::read_to_string(format!("{}/newer.txt", wt.path))
            .expect("newer.txt fetched on add");
        assert_eq!(body, "from upstream\n");
    }

    #[test]
    fn parse_remote_tracking_ref_without_git() {
        let remotes = vec!["origin".to_string(), "upstream".to_string()];
        assert_eq!(
            parse_remote_tracking_ref(&remotes, "upstream/main"),
            Some(("upstream".into(), "main".into()))
        );
        let remotes = vec!["origin".to_string(), "origin-fork".to_string()];
        assert_eq!(
            parse_remote_tracking_ref(&remotes, "origin-fork/feature"),
            Some(("origin-fork".into(), "feature".into())),
            "the longest remote name wins"
        );
        assert_eq!(parse_remote_tracking_ref(&remotes, "main"), None);
    }

    #[tokio::test]
    async fn suggest_base_branch_prefers_origin_main() {
        let r = init_repo_with_remote();
        let root = temp_dir();
        assert_eq!(
            Manager::new(&r.clone, &root.path)
                .suggest_base_branch()
                .await,
            "origin/main"
        );
    }

    #[tokio::test]
    async fn suggest_base_branch_falls_back_to_local_main() {
        let repo = init_repo();
        let root = temp_dir();
        assert_eq!(
            Manager::new(&repo.path, &root.path)
                .suggest_base_branch()
                .await,
            "main"
        );
    }

    #[tokio::test]
    async fn add_empty_repo_remote_base_fails() {
        let dir = temp_dir();
        git(&dir.path, &["init", "-b", "main"]);
        // A remote name so remote-tracking parsing succeeds without a real URL.
        git(
            &dir.path,
            &[
                "remote",
                "add",
                "origin",
                "https://example.invalid/repo.git",
            ],
        );
        let root = temp_dir();
        let res = Manager::new(&dir.path, &root.path)
            .add("ws-empty", "ws-empty", "origin/main")
            .await;
        assert!(res.is_err(), "a remote base on an empty repo must fail");
    }

    // --- remote ---

    #[tokio::test]
    async fn upstream_push_and_fetch() {
        let r = init_repo_with_remote();
        config_identity(&r.clone);
        let root = temp_dir();
        let m = Manager::new(&r.clone, &root.path);
        let up = m.upstream(&r.clone).await.expect("upstream");
        assert_eq!(
            up,
            Upstream {
                remote: "origin".into(),
                branch: "main".into(),
                ref_name: "origin/main".into()
            }
        );

        write(&r.clone, "local.txt", "mine\n");
        git(&r.clone, &["add", "local.txt"]);
        git(&r.clone, &["commit", "-m", "local commit"]);
        assert_eq!(
            m.commits_ahead(&r.clone, &up.ref_name)
                .await
                .expect("ahead"),
            1
        );
        m.push_upstream(&r.clone).await.expect("push_upstream");
        assert_eq!(
            m.commits_ahead(&r.clone, &up.ref_name)
                .await
                .expect("ahead"),
            0
        );
        assert_eq!(show_file(&r.bare, "HEAD:local.txt"), "mine");

        write(&r.clone, "pushed.txt", "branch\n");
        git(&r.clone, &["add", "pushed.txt"]);
        git(&r.clone, &["commit", "-m", "via PushBranch"]);
        m.push_branch("origin", "main").await.expect("push_branch");
        assert_eq!(show_file(&r.bare, "HEAD:pushed.txt"), "branch");

        // Advance the remote from another clone; tracking stays stale until fetch.
        let pusher_dir = temp_dir();
        let pusher = format!("{}/pusher", pusher_dir.path);
        git(&pusher_dir.path, &["clone", &r.bare, &pusher]);
        config_identity(&pusher);
        write(&pusher, "remote.txt", "theirs\n");
        git(&pusher, &["add", "remote.txt"]);
        git(&pusher, &["commit", "-m", "remote advance"]);
        git(&pusher, &["push", "origin", "main"]);
        assert_eq!(
            m.commits_behind(&r.clone, "origin/main")
                .await
                .expect("behind"),
            0
        );
        let (_, fetched) = m.fetch_upstream(&r.clone).await;
        fetched.expect("fetch_upstream");
        assert_eq!(
            m.commits_behind(&r.clone, "origin/main")
                .await
                .expect("behind"),
            1
        );
    }

    #[tokio::test]
    async fn fetch_compare_ref_sees_remote_advance_on_local_base_name() {
        let r = init_repo_with_remote();
        config_identity(&r.clone);
        let root = temp_dir();
        let m = Manager::new(&r.clone, &root.path);
        assert_eq!(m.compare_ref("main").await, "origin/main");

        let pusher_dir = temp_dir();
        let pusher = format!("{}/pusher", pusher_dir.path);
        git(&pusher_dir.path, &["clone", &r.bare, &pusher]);
        config_identity(&pusher);
        write(&pusher, "base-moved.txt", "ahead\n");
        git(&pusher, &["add", "base-moved.txt"]);
        git(&pusher, &["commit", "-m", "advance merge base"]);
        git(&pusher, &["push", "origin", "main"]);

        assert_eq!(
            m.commits_behind(&r.clone, "main").await.expect("behind"),
            0,
            "local main is stale"
        );
        assert_eq!(
            m.commits_behind(&r.clone, "origin/main")
                .await
                .expect("behind"),
            0
        );
        let r2 = m.fetch_compare_ref("main").await;
        assert_eq!(r2, "origin/main");
        assert_eq!(m.commits_behind(&r.clone, &r2).await.expect("behind"), 1);
        let (rf, n) = m
            .behind_base(&r.clone, "main", true)
            .await
            .expect("behind_base");
        assert_eq!((rf.as_str(), n), ("origin/main", 1));
    }

    #[tokio::test]
    async fn fetch_compare_ref_local_only_repo_stays_on_local_name() {
        let dir = temp_dir();
        git(&dir.path, &["init", "-b", "main"]);
        config_identity(&dir.path);
        write(&dir.path, "README.md", "x\n");
        git(&dir.path, &["add", "."]);
        git(&dir.path, &["commit", "-m", "initial"]);
        let root = temp_dir();
        assert_eq!(
            Manager::new(&dir.path, &root.path)
                .fetch_compare_ref("main")
                .await,
            "main"
        );
    }

    #[tokio::test]
    async fn fetch_compare_ref_uses_named_remote_base() {
        let r = init_repo_with_remote();
        config_identity(&r.clone);
        git(&r.clone, &["checkout", "-b", "develop"]);
        git(&r.clone, &["push", "-u", "origin", "develop"]);
        git(&r.clone, &["checkout", "main"]);

        let pusher_dir = temp_dir();
        let pusher = format!("{}/pusher", pusher_dir.path);
        git(&pusher_dir.path, &["clone", &r.bare, &pusher]);
        config_identity(&pusher);
        git(&pusher, &["checkout", "develop"]);
        write(&pusher, "develop-ahead.txt", "yes\n");
        git(&pusher, &["add", "develop-ahead.txt"]);
        git(&pusher, &["commit", "-m", "advance develop"]);
        git(&pusher, &["push", "origin", "develop"]);

        let root = temp_dir();
        let m = Manager::new(&r.clone, &root.path);
        assert_eq!(
            m.fetch_compare_ref("origin/develop").await,
            "origin/develop"
        );
        assert_eq!(
            m.commits_behind(&r.clone, "origin/develop")
                .await
                .expect("behind"),
            1
        );
        let (rf, n) = m
            .behind_base(&r.clone, "develop", true)
            .await
            .expect("behind_base");
        assert_eq!((rf.as_str(), n), ("origin/develop", 1));
    }

    #[tokio::test]
    async fn push_upstream_pushes_head_when_local_name_differs() {
        let r = init_repo_with_remote();
        config_identity(&r.clone);
        git(&r.clone, &["checkout", "-b", "work"]);
        git(&r.clone, &["branch", "--set-upstream-to=origin/main"]);
        write(&r.clone, "from-work.txt", "yes\n");
        git(&r.clone, &["add", "from-work.txt"]);
        git(&r.clone, &["commit", "-m", "on work"]);
        let root = temp_dir();
        Manager::new(&r.clone, &root.path)
            .push_upstream(&r.clone)
            .await
            .expect("push_upstream");
        assert_eq!(
            show_file(&r.bare, "HEAD:from-work.txt"),
            "yes",
            "HEAD should have been pushed"
        );
        let main_blob = std::process::Command::new("git")
            .args(["-C", r.clone.as_str(), "show", "main:from-work.txt"])
            .output()
            .expect("git");
        assert!(
            !main_blob.status.success(),
            "local main should not contain from-work.txt"
        );
    }

    #[tokio::test]
    async fn upstream_missing() {
        let dir = temp_dir();
        git(&dir.path, &["init", "-b", "main"]);
        config_identity(&dir.path);
        write(&dir.path, "README.md", "x\n");
        git(&dir.path, &["add", "."]);
        git(&dir.path, &["commit", "-m", "initial"]);
        let root = temp_dir();
        let m = Manager::new(&dir.path, &root.path);
        assert_eq!(m.upstream(&dir.path).await, None);
        assert!(
            m.push_upstream(&dir.path).await.is_err(),
            "push without tracking must fail"
        );
    }

    // --- pr head ---

    #[tokio::test]
    async fn fetch_pr_head_rejects_invalid_number() {
        let (a, b) = (temp_dir(), temp_dir());
        assert!(
            Manager::new(&a.path, &b.path)
                .fetch_pr_head(0)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn fetch_pr_head_force_updates_after_rewind() {
        let r = init_repo_with_remote();
        config_identity(&r.clone);
        git(&r.clone, &["checkout", "-b", "feat/pr"]);
        write(&r.clone, "one.txt", "one\n");
        git(&r.clone, &["add", "."]);
        git(&r.clone, &["commit", "-m", "one"]);
        let sha1 = rev_parse(&r.clone, "HEAD");
        git(
            &r.clone,
            &["push", "origin", &format!("{sha1}:refs/pull/7/head")],
        );

        let root = temp_dir();
        let m = Manager::new(&r.clone, &root.path);
        assert_eq!(
            m.fetch_pr_head(7).await.expect("first fetch"),
            "refs/recur/pr/7"
        );

        git(&r.clone, &["checkout", "--orphan", "tmp-pr"]);
        git(&r.clone, &["reset"]);
        write(&r.clone, "two.txt", "two\n");
        git(&r.clone, &["add", "."]);
        git(&r.clone, &["commit", "-m", "two"]);
        let sha2 = rev_parse(&r.clone, "HEAD");
        git(
            &r.clone,
            &[
                "push",
                "--force",
                "origin",
                &format!("{sha2}:refs/pull/7/head"),
            ],
        );

        let r2 = m.fetch_pr_head(7).await.expect("fetch after force-push");
        assert_eq!(rev_parse(&r.clone, &r2), sha2);
    }
}
