//! Remotes, remote-tracking branches, fetch/push and base-branch choice.

use std::time::Duration;

use tokio::time::Instant;

use crate::error::{Error, ErrorKind, Result};
use crate::version::MERGE_BASE_VERSION;
use crate::worktree::{Manager, MergeResult, exec, is_repo};

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

    /// How far the worktree HEAD is behind [`compare_ref`](Self::compare_ref)
    /// of `base`, the ref Merge lands on: a local name that also has
    /// `origin/<name>` counts the remote tip only, so unpushed local commits
    /// never get pulled into the workspace. `fetch` updates the tracking tip
    /// first. Returns the ref update-with-base should merge.
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
        let n = self.commits_behind(worktree, &compare).await?;
        Ok((compare, n))
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

    /// Lands `branch` on `remote/short` without touching the project folder:
    /// fetches the remote tip, builds the merge, squash or rebased commits
    /// with `git merge-tree`, and pushes them. A local `short` then
    /// fast-forwards when it can; local commits, or edits the fast-forward
    /// would overwrite, leave it as is. A rejected push (the remote moved)
    /// changes nothing locally. Squash and rebase refuse a branch carrying
    /// unpushed commits from local `short`, which they would fold in or rewrite.
    pub async fn land_remote(
        &self,
        branch: &str,
        remote: &str,
        short: &str,
        message: &str,
        method: &str,
        deadline: Instant,
    ) -> Result<MergeResult> {
        self.fetch_remote_branch_by(remote, short, Some(deadline))
            .await?;
        let tracking = format!("refs/remotes/{remote}/{short}");
        let base = self.rev(&tracking).await?;
        let tip = self.rev(branch).await?;
        if method != "merge" && self.branch_exists(short).await {
            let fork = self.run(["merge-base", short, tip.as_str()]).await?;
            let not_base = format!("^{base}");
            let unpushed = self
                .run(["rev-list", "--count", fork.trim(), not_base.as_str()])
                .await?;
            let n = unpushed.trim();
            if n != "0" {
                return Err(Error::msg(format!(
                    "this branch includes {n} unpushed commit(s) from local {short}: push {short} first"
                )));
            }
        }
        let sign = self
            .run(["config", "--type=bool", "commit.gpgsign"])
            .await
            .is_ok_and(|v| v.trim() == "true");
        let new_head = match method {
            "squash" | "merge" => {
                let tree = self.merge_tree(&base, &tip, None).await?;
                let mut args = vec!["commit-tree", tree.as_str(), "-p", base.as_str()];
                if method == "merge" {
                    args.extend(["-p", tip.as_str()]);
                }
                if sign {
                    args.push("-S");
                }
                let msg = if message.is_empty() {
                    format!("Merge {branch} into {short}")
                } else {
                    message.to_string()
                };
                exec(self.git(&args).arg("-F").arg("-").stdin(msg))
                    .await?
                    .trim()
                    .to_string()
            }
            _ => self.replay(&base, &tip, sign).await?,
        };
        let spec = format!("{new_head}:refs/heads/{short}");
        exec(
            self.git(["push", "--end-of-options", remote, spec.as_str()])
                .env("GIT_TERMINAL_PROMPT", "0")
                .deadline(deadline),
        )
        .await
        .map_err(|e| e.wrap(format!("git push {remote} {short}")))?;
        self.fast_forward_local(short, &new_head).await;
        Ok(MergeResult {
            new_head,
            fast_forward: method == "rebase",
            orig_head: base,
        })
    }

    async fn rev(&self, r: &str) -> Result<String> {
        Ok(self
            .run([
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{r}^{{commit}}"),
            ])
            .await
            .map_err(|e| e.wrap(format!("git rev-parse {r}")))?
            .trim()
            .to_string())
    }

    /// The tree merging `theirs` into `ours`, or an error naming the
    /// conflicted paths.
    async fn merge_tree(
        &self,
        ours: &str,
        theirs: &str,
        merge_base: Option<&str>,
    ) -> Result<String> {
        let mut args = vec![
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            "-z",
        ];
        if let Some(mb) = merge_base {
            args.extend(["--merge-base", mb]);
        }
        args.extend([ours, theirs]);
        let ran = self.git(&args).capture().await;
        let out = String::from_utf8_lossy(&ran.stdout);
        let mut fields = out.split('\0');
        let tree = fields.next().unwrap_or_default().trim().to_string();
        match ran.result {
            Ok(()) => Ok(tree),
            Err(e) if tree.is_empty() => {
                let stderr = String::from_utf8_lossy(&ran.stderr);
                let stderr = stderr.trim();
                let e = if stderr.is_empty() { e } else { e.wrap(stderr) };
                Err(e.wrap("git merge-tree"))
            }
            Err(_) => {
                let files: Vec<&str> = fields.take_while(|f| !f.is_empty()).collect();
                Err(Error::with_kind(
                    format!("conflict in {}", files.join(", ")),
                    ErrorKind::Conflict,
                ))
            }
        }
    }

    /// Replays the non-merge commits of `base..tip` onto `base`, keeping each
    /// commit's author and message, signing each when `sign`. Returns the new tip.
    async fn replay(&self, base: &str, tip: &str, sign: bool) -> Result<String> {
        if let Ok(v) = crate::version::probe().await
            && v.less(&MERGE_BASE_VERSION)
        {
            return Err(Error::msg(format!(
                "rebasing onto a remote branch needs Git {MERGE_BASE_VERSION} or newer (found {v}): use squash or merge"
            )));
        }
        let range = format!("{base}..{tip}");
        let list = self
            .run(["rev-list", "--reverse", "--no-merges", range.as_str()])
            .await?;
        let mut head = base.to_string();
        for c in list.split_whitespace() {
            let parent = format!("{c}^");
            let tree = self.merge_tree(&head, c, Some(&parent)).await?;
            let meta = self
                .run(["log", "-1", "--format=%an%x00%ae%x00%aI%x00%B", c])
                .await?;
            let mut f = meta.splitn(4, '\0');
            let (name, email, date, body) = (
                f.next().unwrap_or_default(),
                f.next().unwrap_or_default(),
                f.next().unwrap_or_default(),
                f.next().unwrap_or_default(),
            );
            let mut args = vec!["commit-tree", tree.as_str(), "-p", head.as_str(), "-F", "-"];
            if sign {
                args.push("-S");
            }
            head = exec(
                self.git(&args)
                    .env("GIT_AUTHOR_NAME", name)
                    .env("GIT_AUTHOR_EMAIL", email)
                    .env("GIT_AUTHOR_DATE", date)
                    .stdin(body.trim_end().to_string()),
            )
            .await?
            .trim()
            .to_string();
        }
        Ok(head)
    }

    /// Moves local `short` to `new_head` when that is a fast-forward: by
    /// `merge --ff-only` when the project folder has it checked out (git
    /// refuses if local edits would be overwritten), otherwise `branch -f`
    /// (git refuses if another worktree has it checked out).
    async fn fast_forward_local(&self, short: &str, new_head: &str) {
        if !self.branch_exists(short).await || !self.is_ancestor(short, new_head).await {
            return;
        }
        let checked_out = self
            .run(["symbolic-ref", "--quiet", "HEAD"])
            .await
            .is_ok_and(|r| r.trim() == format!("refs/heads/{short}"));
        let _ = if checked_out {
            self.run(["merge", "--ff-only", "--quiet", new_head]).await
        } else {
            self.run(["branch", "-f", "--end-of-options", short, new_head])
                .await
        };
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
        WithRemote, commit_as_coder, config_identity, git, init_repo, init_repo_with_remote,
        rev_parse, temp_dir, write,
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

    /// The project folder sits on `feature` (two Coder commits) with a dirty
    /// README; local `main` and `origin/main` both trail the remote, which
    /// gained `base.txt`.
    fn stale_dirty_project(feature_base_txt: &str) -> WithRemote {
        let r = init_repo_with_remote();
        config_identity(&r.clone);
        git(&r.clone, &["checkout", "-q", "-b", "moved"]);
        write(&r.clone, "base.txt", "remote\n");
        git(&r.clone, &["add", "base.txt"]);
        git(&r.clone, &["commit", "-q", "-m", "remote moved"]);
        git(&r.clone, &["push", "-q", "origin", "moved:main"]);
        git(
            &r.clone,
            &["update-ref", "refs/remotes/origin/main", "main"],
        );
        git(&r.clone, &["checkout", "-q", "-b", "feature", "main"]);
        write(&r.clone, "a.txt", "a\n");
        commit_as_coder(&r.clone, "a");
        write(&r.clone, "b.txt", "b\n");
        if !feature_base_txt.is_empty() {
            write(&r.clone, "base.txt", feature_base_txt);
        }
        commit_as_coder(&r.clone, "b");
        write(&r.clone, "README.md", "dirty\n");
        r
    }

    #[tokio::test]
    async fn land_remote_leaves_a_stale_dirty_project_folder_alone() {
        let deadline = Instant::now() + Duration::from_secs(30);
        for method in ["squash", "merge", "rebase"] {
            let r = stale_dirty_project("");
            let root = temp_dir();
            let m = Manager::new(&r.clone, &root.path);
            let feature = rev_parse(&r.clone, "feature");
            let res = m
                .land_remote("feature", "origin", "main", "land it", method, deadline)
                .await
                .unwrap_or_else(|e| panic!("{method}: {e}"));
            let tip = rev_parse(&r.bare, "main");
            assert_eq!(res.new_head, tip, "{method}");
            for f in ["a.txt", "b.txt", "base.txt"] {
                show_file(&r.bare, &format!("main:{f}"));
            }
            assert_eq!(rev_parse(&r.clone, "main"), tip, "{method}: local main");
            assert_eq!(rev_parse(&r.clone, "origin/main"), tip, "{method}");
            assert_eq!(rev_parse(&r.clone, "HEAD"), feature, "{method}: HEAD moved");
            let readme = std::fs::read_to_string(format!("{}/README.md", r.clone)).unwrap();
            assert_eq!(readme, "dirty\n", "{method}");
            let log = |f: &str| git(&r.bare, &["log", "-1", &format!("--format={f}"), "main"]);
            match method {
                "squash" => assert_eq!(log("%P").split_whitespace().count(), 1),
                "merge" => assert_eq!(rev_parse(&r.bare, "main^2"), feature),
                _ => {
                    assert_eq!(log("%an %s").trim(), "Coder b");
                    assert_eq!(rev_parse(&r.bare, "main~2"), res.orig_head);
                }
            }
        }
    }

    #[tokio::test]
    async fn land_remote_conflict_changes_nothing() {
        let r = stale_dirty_project("feature\n");
        let root = temp_dir();
        let m = Manager::new(&r.clone, &root.path);
        let before = rev_parse(&r.bare, "main");
        let deadline = Instant::now() + Duration::from_secs(30);
        let err = m
            .land_remote("feature", "origin", "main", "land it", "squash", deadline)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("conflict in base.txt"), "{err}");
        assert_eq!(rev_parse(&r.bare, "main"), before);
        assert_ne!(rev_parse(&r.clone, "main"), before);
    }

    #[tokio::test]
    async fn land_remote_refuses_unpushed_local_base_commits() {
        let r = init_repo_with_remote();
        config_identity(&r.clone);
        write(&r.clone, "local.txt", "unpushed\n");
        git(&r.clone, &["add", "local.txt"]);
        git(&r.clone, &["commit", "-q", "-m", "unpushed local"]);
        git(&r.clone, &["checkout", "-q", "-b", "feature"]);
        write(&r.clone, "a.txt", "a\n");
        commit_as_coder(&r.clone, "a");
        let root = temp_dir();
        let m = Manager::new(&r.clone, &root.path);
        let before = rev_parse(&r.bare, "main");
        let deadline = Instant::now() + Duration::from_secs(30);
        for method in ["squash", "rebase"] {
            let err = m
                .land_remote("feature", "origin", "main", "land it", method, deadline)
                .await
                .unwrap_err();
            assert!(
                err.to_string()
                    .contains("1 unpushed commit(s) from local main"),
                "{method}: {err}"
            );
            assert_eq!(rev_parse(&r.bare, "main"), before, "{method}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn land_remote_signs_when_commit_gpgsign_is_set() {
        use std::os::unix::fs::PermissionsExt;
        let tools = temp_dir();
        let signer = format!("{}/fake-gpg", tools.path);
        std::fs::write(
            &signer,
            "#!/bin/sh\ncat >/dev/null\nprintf '\\n[GNUPG:] SIG_CREATED \\n' >&2\nprintf -- '-----BEGIN PGP SIGNATURE-----\\nstub\\n-----END PGP SIGNATURE-----\\n'\n",
        )
        .unwrap();
        std::fs::set_permissions(&signer, std::fs::Permissions::from_mode(0o755)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        for method in ["squash", "merge", "rebase"] {
            let r = stale_dirty_project("");
            for (k, v) in [
                ("commit.gpgsign", "true"),
                ("gpg.format", "openpgp"),
                ("gpg.program", signer.as_str()),
                ("user.signingkey", "stub"),
            ] {
                git(&r.clone, &["config", k, v]);
            }
            let root = temp_dir();
            let m = Manager::new(&r.clone, &root.path);
            let res = m
                .land_remote("feature", "origin", "main", "land it", method, deadline)
                .await
                .unwrap_or_else(|e| panic!("{method}: {e}"));
            let raw = git(&r.bare, &["cat-file", "commit", &res.new_head]);
            assert!(raw.contains("\ngpgsig "), "{method}: unsigned\n{raw}");
        }
    }
}
