//! Per-workspace git worktrees: one worktree + branch per workspace, created
//! off the project's base branch, so agents run in isolation and never touch
//! the user's checkout. The default branch is `recur/<slug>`; a custom prefix
//! or none can replace `recur`. Worktrees live at
//! `<root>/<project-slug>/<workspace-slug>` (workspace id `{project}.{slug}`).

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fmt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::time::Duration;

use tokio::time::Instant;

use crate::command::{Command, command};
use crate::error::{Error, Result};
use crate::gopath;
use crate::home::expand_home;
use crate::scm::identity;
use crate::text::{atoi, eq_fold, go_lower, go_string, quote};

/// One checked-out worktree of a repository.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Worktree {
    /// Absolute path to the worktree directory.
    pub path: String,
    /// Branch checked out there, e.g. `recur/rate-limit`.
    pub branch: String,
    /// Current commit hash (empty for a brand-new branch).
    pub head: String,
}

/// Creates and tears down worktrees for one repository.
#[derive(Debug, Clone, Default)]
pub struct Manager {
    /// The source checkout, with a leading `~` expanded.
    pub repo_path: String,
    /// Where worktrees are created (Recur's managed worktrees dir).
    pub root: String,
    /// Prefix for new workspace branches (prefix/slug). `None` uses
    /// [`DEFAULT_BRANCH_PREFIX`]; `Some("")` means no prefix.
    branch_prefix: Option<String>,
}

/// The historical workspace branch namespace.
pub const DEFAULT_BRANCH_PREFIX: &str = "recur";

/// Bounds a single repo probe so a hung network mount cannot block forever.
pub(crate) const IS_REPO_TIMEOUT: Duration = Duration::from_secs(10);

impl Manager {
    /// A manager for `repo_path` (leading `~` expanded) with worktrees under
    /// `root`. New branches use [`DEFAULT_BRANCH_PREFIX`] until
    /// [`with_branch_prefix`](Self::with_branch_prefix).
    pub fn new(repo_path: &str, root: &str) -> Self {
        Self {
            repo_path: expand_home(repo_path),
            root: root.to_string(),
            branch_prefix: None,
        }
    }

    /// Sets the prefix for new workspace branches. An empty prefix means no
    /// prefix: the branch is just the slug. Always call this when the prefix
    /// comes from Settings so "none" is distinct from the default.
    pub fn with_branch_prefix(mut self, prefix: &str) -> Self {
        self.branch_prefix = Some(normalize_branch_prefix(prefix));
        self
    }

    /// The path segment prepended to new workspace branches. Empty means none.
    pub fn prefix(&self) -> String {
        self.branch_prefix
            .clone()
            .unwrap_or_else(|| DEFAULT_BRANCH_PREFIX.to_string())
    }

    /// git in the repository (Go `m.run`).
    pub(crate) fn git<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        command(args).current_dir(&self.repo_path)
    }

    /// git `-C worktree` (Go `m.runIn`).
    pub(crate) fn git_in<I, S>(&self, worktree: &str, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        command(["-C", worktree])
            .args(args)
            .current_dir(&self.repo_path)
    }

    pub(crate) async fn run<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        exec(self.git(args)).await
    }

    pub(crate) async fn run_in<I, S>(&self, worktree: &str, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        exec(self.git_in(worktree, args)).await
    }

    /// `git branch --list` short names matching `glob`. `None` means the list
    /// command failed; callers fall back to show-ref.
    async fn listed_local_branches(&self, glob: &str) -> Option<HashSet<String>> {
        let out = self
            .run([
                "--no-pager",
                "branch",
                "--list",
                "--format",
                "%(refname:short)",
                glob,
            ])
            .await
            .ok()?;
        Some(
            out.split('\n')
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .collect(),
        )
    }

    /// `prefix/slug`, or that plus `-N` when it is already a local head. One
    /// `git branch --list '<branch>*'` collects the taken names.
    async fn unique_branch(&self, slug: &str) -> String {
        let branch = prefixed_branch(&self.prefix(), slug);
        let taken = self.listed_local_branches(&format!("{branch}*")).await;
        let in_taken = async |name: &str| match &taken {
            Some(t) => t.contains(name),
            None => self.branch_exists(name).await,
        };
        if !in_taken(&branch).await {
            return branch;
        }
        let mut n = 2;
        loop {
            let candidate = format!("{branch}-{n}");
            if !in_taken(&candidate).await {
                return candidate;
            }
            n += 1;
        }
    }

    /// Creates the worktree for `workspace_id` on a unique branch
    /// `prefix/<slug>` (`prefix/<slug>-N` when taken), based on `base_branch`.
    /// `{project}.{slug}` lands at `<root>/<project>/<slug>`; a legacy id stays
    /// one segment. `created` is true only when a new worktree was allocated,
    /// so callers run `setup_cmd` exactly once per worktree lifetime.
    ///
    /// A leftover local branch is never checked out: recycled ids spin off the
    /// current base, not an old tip ([`restore`](Self::restore) resumes one).
    /// A repo with no commits gets an empty initial commit on `base_branch`;
    /// the user's untracked files are not staged.
    pub async fn add(
        &self,
        workspace_id: &str,
        slug: &str,
        base_branch: &str,
    ) -> Result<(Worktree, bool)> {
        self.add_inner(workspace_id, slug, base_branch, "").await
    }

    /// Creates a worktree starting at `start_point`. A free local `branch` is
    /// created off that tip; an existing one that is an ancestor of
    /// `start_point` is fast-forwarded (like `gh pr checkout`). A local branch
    /// that is ahead or diverged is left alone and [`add`](Self::add)
    /// allocates a unique branch instead, as it does when git refuses (branch
    /// checked out in another worktree).
    pub async fn add_at_ref(
        &self,
        workspace_id: &str,
        slug: &str,
        branch: &str,
        start_point: &str,
    ) -> Result<(Worktree, bool)> {
        let branch = branch.trim();
        let start_point = start_point.trim();
        if branch.is_empty() || start_point.is_empty() {
            return self.add(workspace_id, slug, start_point).await;
        }
        if self.branch_exists(branch).await && !self.same_commit(branch, start_point).await {
            if !self.is_ancestor(branch, start_point).await {
                return self.add(workspace_id, slug, start_point).await;
            }
            if self
                .run(["branch", "-f", branch, start_point])
                .await
                .is_err()
            {
                return self.add(workspace_id, slug, start_point).await;
            }
        }
        match self
            .add_inner(workspace_id, slug, start_point, branch)
            .await
        {
            Ok(r) => Ok(r),
            Err(_) => self.add(workspace_id, slug, start_point).await,
        }
    }

    /// Points the worktree HEAD at `remote/branch` when that tracking ref
    /// exists (same-repo PR heads). Fork PRs have none, so this is a no-op.
    pub async fn try_set_upstream(&self, worktree: &str, branch: &str) {
        let branch = branch.trim();
        let worktree = worktree.trim();
        if branch.is_empty() || worktree.is_empty() {
            return;
        }
        let mut remote = self.default_remote().await;
        if remote.is_empty() {
            remote = "origin".to_string();
        }
        let r = format!("{remote}/{branch}");
        if !self.commit_exists(&r).await {
            return;
        }
        let _ = self
            .run_in(worktree, ["branch", &format!("--set-upstream-to={r}")])
            .await;
    }

    /// Recreates the worktree for `workspace_id` on `branch` (the recorded
    /// branch, possibly `prefix/<id>-N`). An existing local head is checked
    /// out as-is so archive resume keeps history; a missing one is created off
    /// `base_branch`. An empty branch means `recur/<slug>`, the historical
    /// namespace, so a later prefix change cannot orphan an existing head.
    pub async fn restore(
        &self,
        workspace_id: &str,
        branch: &str,
        base_branch: &str,
    ) -> Result<(Worktree, bool)> {
        let mut branch = branch.trim().to_string();
        if branch.is_empty() {
            branch = prefixed_branch(DEFAULT_BRANCH_PREFIX, &workspace_slug(workspace_id));
        }
        self.add_inner(
            workspace_id,
            &workspace_slug(workspace_id),
            base_branch,
            &branch,
        )
        .await
    }

    async fn add_inner(
        &self,
        workspace_id: &str,
        slug: &str,
        base_branch: &str,
        restore_branch: &str,
    ) -> Result<(Worktree, bool)> {
        validate_managed_workspace_id(workspace_id)?;
        let mut branch = if restore_branch.is_empty() {
            prefixed_branch(&self.prefix(), slug)
        } else {
            restore_branch.to_string()
        };
        let wt_path = gopath::join(&[&self.root, &worktree_rel_path(workspace_id)]);

        // A registered worktree at the path is reused only while its directory
        // is still on disk. A stale registration after a partial delete must
        // not block a fresh add for a recycled id.
        if let Some(existing) = self.worktree_at(&wt_path).await? {
            if std::fs::metadata(&wt_path).is_ok() {
                return Ok((existing, false));
            }
            // Dir gone but still registered (often locked; prune skips those).
            let _ = self.force_remove_worktree(&wt_path).await;
            let _ = self.run(["worktree", "prune"]).await;
            if matches!(self.worktree_at(&wt_path).await, Ok(Some(_))) {
                return Err(Error::msg(format!(
                    "stale worktree registration at {} could not be cleared",
                    quote(&wt_path)
                )));
            }
        }

        // On disk but unregistered (the managed root was renamed): repair and
        // reuse when valid, otherwise wipe the leftover so a recycled id does
        // not inherit an out-of-date checkout.
        if std::fs::metadata(&wt_path).is_ok_and(|m| m.is_dir()) {
            let _ = self.run(["worktree", "repair", wt_path.as_str()]).await;
            if let Some(existing) = self.worktree_at(&wt_path).await? {
                return Ok((existing, false));
            }
            remove_all(&wt_path).map_err(|e| {
                Error::io_path("unlinkat", &wt_path, &e)
                    .wrap(format!("remove stale worktree path {}", quote(&wt_path)))
            })?;
        }

        let parent = gopath::dir(&wt_path);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o755)
            .create(&parent)
            .map_err(|e| Error::io_path("mkdir", &parent, &e).wrap("create worktrees root"))?;

        let mut args: Vec<String> = vec!["worktree".into(), "add".into()];
        if !restore_branch.is_empty() && self.branch_exists(&branch).await {
            // Resume: check out the recorded branch as-is so history is kept.
            args.extend([wt_path.clone(), branch.clone()]);
        } else {
            if restore_branch.is_empty() {
                branch = self.unique_branch(slug).await;
            }
            let blocked = self.blocking_ref_for_branch(&branch).await;
            if !blocked.is_empty() {
                return Err(Error::msg(format!(
                    "cannot create branch {}: existing branch {} blocks it (git cannot nest refs under a branch of the same name) — pick a different prefix in Settings or rename that branch",
                    quote(&branch),
                    quote(&blocked)
                )));
            }
            let mut base_branch = base_branch.to_string();
            if base_branch.is_empty() {
                base_branch = self.default_branch().await;
            }
            // Remote bases: fetch the tip once so spin-off doesn't need a pull.
            self.refresh_remote_base_if_needed(&base_branch)
                .await
                .map_err(|e| e.wrap(format!("refresh base {}", quote(&base_branch))))?;
            if !self.commit_exists(&base_branch).await {
                if self.has_any_commit().await {
                    return Err(Error::msg(format!(
                        "base branch {} does not exist (or has no commits)",
                        quote(&base_branch)
                    )));
                }
                if self.parse_remote_tracking(&base_branch).await.is_some() {
                    return Err(Error::msg(format!(
                        "base branch {} is a remote ref but is missing — fetch failed or remote is empty",
                        quote(&base_branch)
                    )));
                }
                self.ensure_initial_commit(&base_branch).await?;
            }
            args.extend([
                "-b".to_string(),
                branch.clone(),
                wt_path.clone(),
                base_branch,
            ]);
        }
        self.run(&args)
            .await
            .map_err(|e| e.wrap("git worktree add"))?;

        let head = self
            .run(["-C", wt_path.as_str(), "rev-parse", "HEAD"])
            .await
            .unwrap_or_default();
        Ok((
            Worktree {
                path: wt_path,
                branch,
                head: head.trim().to_string(),
            },
            true,
        ))
    }

    /// Creates an empty first commit on `base_branch` so worktrees can be
    /// allocated from a fresh `git init`. Untracked files are left alone.
    async fn ensure_initial_commit(&self, base_branch: &str) -> Result<()> {
        let base_branch = if base_branch.is_empty() {
            "main"
        } else {
            base_branch
        };
        // Align the unborn HEAD name with the requested base before committing.
        if let Ok(cur) = self.run(["symbolic-ref", "--short", "HEAD"]).await {
            let cur = cur.trim();
            if !cur.is_empty() && cur != base_branch {
                self.run(["checkout", "-B", base_branch])
                    .await
                    .map_err(|e| e.wrap(format!("prepare base branch {}", quote(base_branch))))?;
            }
        }
        let (name, email) = identity(&self.repo_path).await?;
        let user_name = format!("user.name={name}");
        let user_email = format!("user.email={email}");
        let author = format!("{name} <{email}>");
        self.run([
            "-c",
            &user_name,
            "-c",
            &user_email,
            "commit",
            "--allow-empty",
            "-m",
            "Initial commit",
            "--author",
            &author,
        ])
        .await
        .map_err(|e| e.wrap(format!("create initial commit on {base_branch}")))?;
        // If HEAD was already on a different born branch, ensure the base exists.
        if !self.commit_exists(base_branch).await {
            self.run(["branch", "-M", base_branch])
                .await
                .map_err(|e| e.wrap(format!("rename initial branch to {base_branch}")))?;
        }
        Ok(())
    }

    /// The repository's current branch name, or `main`.
    async fn default_branch(&self) -> String {
        if let Ok(cur) = self.run(["symbolic-ref", "--short", "HEAD"]).await {
            let t = cur.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
        "main".to_string()
    }

    /// Stages everything in the worktree (`git add -A`) and commits it with
    /// the given author/committer and message. A clean worktree is a no-op
    /// returning `committed == false`, so a turn that changed nothing does not
    /// create an empty commit.
    pub async fn commit(
        &self,
        worktree: &str,
        message: &str,
        author_name: &str,
        author_email: &str,
    ) -> Result<CommitResult> {
        self.run_in(worktree, ["add", "-A"])
            .await
            .map_err(|e| e.wrap("git add"))?;
        if self.is_clean(worktree).await? {
            // A merge whose resolved tree equals HEAD is porcelain-clean but
            // still needs a two-parent commit to clear MERGE_HEAD.
            match self
                .run_in(worktree, ["rev-parse", "-q", "--verify", "MERGE_HEAD"])
                .await
            {
                Ok(out) if !out.trim().is_empty() => {}
                _ => return Ok(CommitResult::default()),
            }
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

    pub(crate) async fn commit_with_identity(
        &self,
        worktree: &str,
        message: &str,
        author_name: &str,
        author_email: &str,
    ) -> Result<String> {
        let user_name = format!("user.name={author_name}");
        let user_email = format!("user.email={author_email}");
        let author = format!("{author_name} <{author_email}>");
        self.run_in(
            worktree,
            [
                "-c",
                &user_name,
                "-c",
                &user_email,
                "commit",
                "-m",
                message,
                "--author",
                &author,
            ],
        )
        .await
    }

    /// The branch's changes versus `base_branch`'s merge-base with HEAD: the
    /// changed paths and the unified diff body. Best-effort: on git errors it
    /// returns what it has so a rail check never wedges.
    pub async fn changed_diff(&self, worktree: &str, base_branch: &str) -> (Vec<String>, String) {
        let mut base = base_branch.to_string();
        if let Ok(mb) = self
            .run_in(worktree, ["merge-base", "HEAD", base_branch])
            .await
        {
            let t = mb.trim();
            if !t.is_empty() {
                base = t.to_string();
            }
        }
        let mut paths = Vec::new();
        if let Ok(names) = self
            .run_in(worktree, ["diff", "--name-only", base.as_str(), "HEAD"])
            .await
        {
            paths = names
                .trim()
                .split('\n')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect();
        }
        let body = self
            .run_in(worktree, ["diff", base.as_str(), "HEAD"])
            .await
            .unwrap_or_default();
        (paths, body)
    }

    /// Integrates `branch` into `base_branch` on disk in the source checkout
    /// (`repo_path`), so the base ref advances where the user works. `method`
    /// is `merge` (default, `--no-ff`), `squash` or `rebase`. Author and
    /// committer are the user's git config. On failure it aborts so the repo is
    /// never left half-merged.
    ///
    /// This moves the shared checkout's HEAD: serialize it per repository.
    pub async fn merge(
        &self,
        branch: &str,
        base_branch: &str,
        message: &str,
        method: &str,
    ) -> Result<MergeResult> {
        if branch.is_empty() || base_branch.is_empty() {
            return Err(Error::msg("merge: branch and base are required"));
        }
        // Land onto a local branch. Remote-tracking bases must not rely on git
        // DWIM (multi-remote repos can pick the wrong remote): create or check
        // out the short name from the selected ref.
        let base = if let Some((remote, short)) = self.parse_remote_tracking(base_branch).await {
            if self.branch_exists(&short).await {
                if let Err(e) = self.run(["checkout", short.as_str()]).await {
                    let fallback = e.clone().wrap(format!("git checkout {short}"));
                    return Err(merge_op_error(&short, &e, fallback));
                }
            } else {
                let mut start = base_branch.to_string();
                if !self.commit_exists(&start).await {
                    start = format!("refs/remotes/{remote}/{short}");
                }
                if let Err(e) = self
                    .run(["checkout", "-B", short.as_str(), start.as_str()])
                    .await
                {
                    let fallback = e.clone().wrap(format!("git checkout -B {short} {start}"));
                    return Err(merge_op_error(&short, &e, fallback));
                }
            }
            short
        } else {
            if let Err(e) = self.run(["checkout", base_branch]).await {
                let fallback = e.clone().wrap(format!("git checkout {base_branch}"));
                return Err(merge_op_error(base_branch, &e, fallback));
            }
            base_branch.to_string()
        };
        match method {
            "squash" => self.merge_squash(branch, &base, message).await,
            "rebase" => self.merge_rebase(branch, &base).await,
            _ => self.merge_commit(branch, &base, message).await,
        }
    }

    /// A `--no-ff` merge commit, preserving the branch topology.
    async fn merge_commit(
        &self,
        branch: &str,
        base_branch: &str,
        message: &str,
    ) -> Result<MergeResult> {
        let mut args = vec!["merge", "--no-edit", "--no-ff"];
        if !message.is_empty() {
            args.extend(["-m", message]);
        }
        args.push(branch);
        if let Err(e) = self.run(&args).await {
            let _ = self.run(["merge", "--abort"]).await;
            let fallback = e
                .clone()
                .wrap(format!("git merge {branch} into {base_branch}"));
            return Err(merge_op_error(base_branch, &e, fallback));
        }
        let head = self.run(["rev-parse", "HEAD"]).await.unwrap_or_default();
        Ok(MergeResult {
            new_head: head.trim().to_string(),
            fast_forward: false,
        })
    }

    /// Squashes every branch commit into one commit on the base.
    async fn merge_squash(
        &self,
        branch: &str,
        base_branch: &str,
        message: &str,
    ) -> Result<MergeResult> {
        if let Err(e) = self.run(["merge", "--squash", branch]).await {
            let _ = self.run(["merge", "--abort"]).await;
            let fallback = e
                .clone()
                .wrap(format!("git merge --squash {branch} into {base_branch}"));
            return Err(merge_op_error(base_branch, &e, fallback));
        }
        let message = if message.is_empty() {
            format!("Squash-merge {branch} into {base_branch}")
        } else {
            message.to_string()
        };
        if let Err(e) = self
            .run(["commit", "--no-edit", "-m", message.as_str()])
            .await
        {
            let _ = self.run(["reset", "--hard", "HEAD"]).await;
            return Err(e.wrap(format!("git commit (squash {branch})")));
        }
        let head = self.run(["rev-parse", "HEAD"]).await.unwrap_or_default();
        Ok(MergeResult {
            new_head: head.trim().to_string(),
            fast_forward: false,
        })
    }

    /// Replays branch commits onto the base linearly. Cherry-picks the
    /// `(merge-base, branch]` range so the branch ref (which may be checked out
    /// in a worktree) is never checked out in the main repo.
    async fn merge_rebase(&self, branch: &str, base_branch: &str) -> Result<MergeResult> {
        let orig_base = self
            .run(["rev-parse", base_branch])
            .await
            .unwrap_or_default()
            .trim()
            .to_string();
        let mb = self
            .run(["merge-base", base_branch, branch])
            .await
            .map_err(|e| e.wrap(format!("git merge-base {base_branch} {branch}")))?;
        let range = format!("{}..{branch}", mb.trim());
        if let Err(e) = self.run(["cherry-pick", range.as_str()]).await {
            let _ = self.run(["cherry-pick", "--abort"]).await;
            let head = self.run(["rev-parse", "HEAD"]).await.unwrap_or_default();
            if head.trim() != orig_base {
                // Abort could not restore. Skipped when it did: reset --hard
                // would wipe uncommitted project-folder edits abort preserves.
                let _ = self.run(["reset", "--hard", orig_base.as_str()]).await;
            }
            let fallback = e.clone().wrap(format!(
                "git rebase (cherry-pick) {branch} onto {base_branch}"
            ));
            return Err(merge_op_error(base_branch, &e, fallback));
        }
        let head = self.run(["rev-parse", "HEAD"]).await.unwrap_or_default();
        Ok(MergeResult {
            new_head: head.trim().to_string(),
            fast_forward: true,
        })
    }

    /// Brings a worktree up to date with `base_branch` by merge (default) or
    /// rebase. On conflict the operation is aborted, leaving the worktree
    /// clean, and an error is returned. The shared base branch is untouched.
    pub async fn update_worktree_with_base(
        &self,
        worktree: &str,
        base_branch: &str,
        method: &str,
    ) -> Result<()> {
        if let Err(e) = self
            .begin_update_with_base(worktree, base_branch, method)
            .await
        {
            let _ = self.abort_update_with_base(worktree, method).await;
            return Err(e);
        }
        Ok(())
    }

    /// Starts a merge or rebase of `base_branch` into the worktree, as the
    /// user's git identity. On conflict the worktree is left conflicted (not
    /// aborted) so a coder can resolve the markers; then call
    /// [`finish_update_with_base`](Self::finish_update_with_base) or
    /// [`abort_update_with_base`](Self::abort_update_with_base).
    pub async fn begin_update_with_base(
        &self,
        worktree: &str,
        base_branch: &str,
        method: &str,
    ) -> Result<()> {
        self.begin_update(worktree, base_branch, method, false)
            .await
    }

    /// [`begin_update_with_base`](Self::begin_update_with_base) with
    /// `--autostash`, so a dirty working copy can pull without committing.
    pub async fn begin_update_with_base_autostash(
        &self,
        worktree: &str,
        base_branch: &str,
        method: &str,
    ) -> Result<()> {
        self.begin_update(worktree, base_branch, method, true).await
    }

    /// The SHA of `refs/stash`, or empty when the stash is empty.
    pub async fn stash_tip(&self, worktree: &str) -> String {
        match self
            .run_in(worktree, ["rev-parse", "-q", "--verify", "refs/stash"])
            .await
        {
            Ok(out) => out.trim().to_string(),
            Err(_) => String::new(),
        }
    }

    /// Whether `--autostash` stored a new stash because reapplying leftover
    /// edits conflicted after a successful merge/rebase. git still exits 0;
    /// the edits stay in the stash until the user pops them.
    pub async fn autostash_left_in_stash(&self, worktree: &str, tip_before: &str) -> bool {
        let tip = self.stash_tip(worktree).await;
        if tip.is_empty() || tip == tip_before {
            return false;
        }
        match self
            .run_in(worktree, ["log", "-1", "--format=%s", "refs/stash"])
            .await
        {
            Ok(subj) => go_lower(subj.trim()).contains("autostash"),
            Err(_) => false,
        }
    }

    /// Discards uncommitted tracked changes so the tree matches HEAD (after an
    /// autostash-apply conflict, whose edits are already stashed). Untracked
    /// files are left alone.
    pub async fn reset_worktree_to_head(&self, worktree: &str) -> Result<()> {
        self.run_in(worktree, ["reset", "--hard", "HEAD"])
            .await
            .map_err(|e| e.wrap("git reset --hard HEAD"))?;
        Ok(())
    }

    async fn begin_update(
        &self,
        worktree: &str,
        base_branch: &str,
        method: &str,
        autostash: bool,
    ) -> Result<()> {
        if worktree.is_empty() || base_branch.is_empty() {
            return Err(Error::msg(
                "update-with-base: worktree and base are required",
            ));
        }
        let verb = if method == "rebase" {
            "rebase"
        } else {
            "merge"
        };
        let mut args = vec![verb];
        if verb == "merge" {
            args.push("--no-edit");
        }
        if autostash {
            args.push("--autostash");
        }
        args.push(base_branch);
        self.run_in(worktree, &args).await.map_err(|e| {
            if verb == "rebase" {
                e.wrap(format!("git rebase onto {base_branch}"))
            } else {
                e.wrap(format!("git merge {base_branch} into worktree"))
            }
        })?;
        Ok(())
    }

    /// Stages all resolutions and completes the merge or rebase started by
    /// [`begin_update_with_base`](Self::begin_update_with_base).
    pub async fn finish_update_with_base(&self, worktree: &str, method: &str) -> Result<()> {
        self.run_in(worktree, ["add", "-A"])
            .await
            .map_err(|e| e.wrap("git add (resolve)"))?;
        if method == "rebase" {
            // GIT_EDITOR=true skips the message editor on continue.
            exec(
                self.git_in(worktree, ["rebase", "--continue"])
                    .env("GIT_EDITOR", "true"),
            )
            .await
            .map_err(|e| e.wrap("git rebase --continue"))?;
        } else {
            self.run_in(worktree, ["commit", "--no-edit"])
                .await
                .map_err(|e| e.wrap("git commit (merge resolve)"))?;
        }
        Ok(())
    }

    /// Aborts whichever merge or rebase is in progress, judged by
    /// `MERGE_HEAD` / `REBASE_HEAD` rather than the caller's intent.
    pub async fn abort_in_progress_update(&self, worktree: &str) -> Result<()> {
        if self.ref_set(worktree, "MERGE_HEAD").await {
            return self.abort_update_with_base(worktree, "merge").await;
        }
        if self.ref_set(worktree, "REBASE_HEAD").await {
            return self.abort_update_with_base(worktree, "rebase").await;
        }
        self.abort_update_with_base(worktree, "merge").await
    }

    /// Aborts an in-progress merge or rebase in the worktree.
    pub async fn abort_update_with_base(&self, worktree: &str, method: &str) -> Result<()> {
        let verb = if method == "rebase" {
            "rebase"
        } else {
            "merge"
        };
        self.run_in(worktree, [verb, "--abort"]).await.map(|_| ())
    }

    async fn ref_set(&self, worktree: &str, r: &str) -> bool {
        matches!(self.run_in(worktree, ["rev-parse", "-q", "--verify", r]).await, Ok(out) if !out.trim().is_empty())
    }

    /// Whether a merge or rebase is in progress (or conflicts remain).
    pub async fn update_in_progress(&self, worktree: &str) -> bool {
        if self.ref_set(worktree, "MERGE_HEAD").await || self.ref_set(worktree, "REBASE_HEAD").await
        {
            return true;
        }
        match self.run_in(worktree, ["status", "--porcelain"]).await {
            Ok(st) => porcelain_has_conflict(&st),
            Err(_) => false,
        }
    }

    /// The commit hash `r` points at, or empty on error.
    pub async fn ref_hash(&self, r: &str) -> String {
        match self.run(["rev-parse", r]).await {
            Ok(out) => out.trim().to_string(),
            Err(_) => String::new(),
        }
    }

    /// Whether the worktree has no staged or unstaged changes.
    pub(crate) async fn is_clean(&self, worktree: &str) -> Result<bool> {
        let out = self
            .run_in(worktree, ["status", "--porcelain"])
            .await
            .map_err(|e| e.wrap("git status"))?;
        Ok(out.trim().is_empty())
    }

    /// Additions and deletions of the staged changes; zeros when unparseable.
    pub(crate) async fn staged_diff_stat(&self, worktree: &str) -> (i64, i64) {
        let Ok(out) = self
            .run_in(worktree, ["diff", "--cached", "--numstat"])
            .await
        else {
            return (0, 0);
        };
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

    /// Tears down the worktree at `path` (force-removing local edits) and
    /// deletes its directory. Paths outside `root` (or equal to it) are
    /// refused so a wrong path can never wipe the user's checkout. A path
    /// already gone and unregistered is a no-op.
    pub async fn remove(&self, path: &str) -> Result<()> {
        if path.trim().is_empty() {
            return Ok(());
        }
        if !self.under_managed_root(path) {
            return Err(Error::msg(format!(
                "refuse to remove worktree path {}: not under managed root {}",
                quote(path),
                quote(&self.root)
            )));
        }
        let git_err = self.force_remove_worktree(path).await.err();
        // Wipe the directory even when git never knew about it (orphan).
        if let Err(e) = remove_all(path) {
            return Err(Error::io_path("unlinkat", path, &e)
                .wrap(format!("remove worktree dir {}", quote(path))));
        }
        if matches!(self.worktree_at(path).await, Ok(Some(_))) {
            let _ = self.run(["worktree", "prune"]).await;
            // Locked entries survive prune; a second force-force clears them.
            let _ = self.force_remove_worktree(path).await;
        }
        if matches!(self.worktree_at(path).await, Ok(Some(_))) {
            return Err(match git_err {
                Some(e) => e.wrap(format!("git worktree remove {}", quote(path))),
                None => Error::msg(format!(
                    "worktree {} still registered after remove",
                    quote(path)
                )),
            });
        }
        // Nested <root>/<project>/<slug>: drop the project dir when this was
        // its last worktree (fails, ignored, when not empty).
        let parent = gopath::dir(path);
        if self.under_managed_root(&parent) {
            let _ = std::fs::remove_dir(&parent);
        }
        Ok(())
    }

    /// Deletes a local branch whose tip still equals `base` (a fresh branch
    /// with no unique commits), so a failed setup's next allocation reuses
    /// the natural name. Branches that moved off base are left alone.
    pub async fn delete_branch_if_at_base(&self, branch: &str, base: &str) -> Result<()> {
        let branch = branch.trim();
        let base = base.trim();
        if branch.is_empty() || base.is_empty() || !self.branch_exists(branch).await {
            return Ok(());
        }
        let tip = self
            .run(["rev-parse", branch])
            .await
            .map_err(|e| e.wrap(format!("rev-parse {branch}")))?;
        let base_tip = self
            .run(["rev-parse", base])
            .await
            .map_err(|e| e.wrap(format!("rev-parse {base}")))?;
        if tip.trim() != base_tip.trim() {
            return Ok(());
        }
        self.delete_local_branch(branch).await
    }

    /// Force-deletes a local branch (`git branch -D`). No-op when empty or
    /// missing. Remotes are untouched.
    pub async fn delete_local_branch(&self, branch: &str) -> Result<()> {
        let branch = branch.trim();
        if branch.is_empty() || !self.branch_exists(branch).await {
            return Ok(());
        }
        self.run(["branch", "-D", "--", branch])
            .await
            .map_err(|e| e.wrap(format!("git branch -D {branch}")))?;
        Ok(())
    }

    /// `git worktree remove --force`, retried with a second `--force` when
    /// the worktree is locked.
    async fn force_remove_worktree(&self, path: &str) -> Result<()> {
        if self
            .run(["worktree", "remove", "--force", path])
            .await
            .is_ok()
        {
            return Ok(());
        }
        self.run(["worktree", "remove", "--force", "--force", path])
            .await
            .map(|_| ())
    }

    /// Whether `path` is strictly inside `root`, symlinks resolved (including
    /// missing leaf components) so a link escaping the root is rejected before
    /// deleting, and `/var` vs `/private/var` still compare equal.
    fn under_managed_root(&self, path: &str) -> bool {
        if self.root.trim().is_empty() || path.trim().is_empty() {
            return false;
        }
        let (Ok(root), Ok(p)) = (resolve_path_prefix(&self.root), resolve_path_prefix(path)) else {
            return false;
        };
        if p == root {
            return false;
        }
        match gopath::rel(&root, &p) {
            Some(rel) => !(rel == "." || rel == ".." || rel.starts_with("../")),
            None => false,
        }
    }

    /// Every worktree registered for the repository (the main checkout first).
    pub async fn list(&self) -> Result<Vec<Worktree>> {
        let out = self
            .run(["worktree", "list", "--porcelain"])
            .await
            .map_err(|e| e.wrap("git worktree list"))?;
        Ok(parse_porcelain(&out))
    }

    /// The registered worktree at `path` (after symlink/abs cleanup), if any.
    pub(crate) async fn worktree_at(&self, path: &str) -> Result<Option<Worktree>> {
        let list = self.list().await?;
        let want = clean_path(path);
        Ok(list.into_iter().find(|w| clean_path(&w.path) == want))
    }

    /// Local branches that resolve to a commit, newest tip first. An unborn
    /// repo returns none.
    pub async fn list_local_branches(&self) -> Result<Vec<String>> {
        let out = self
            .run([
                "for-each-ref",
                "--format=%(refname:short)",
                "--sort=-committerdate",
                "refs/heads/",
            ])
            .await?;
        Ok(out
            .split('\n')
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Whether `r` resolves to a commit (false for unborn HEAD or missing refs).
    pub async fn commit_exists(&self, r: &str) -> bool {
        self.commit_exists_by(r, None).await
    }

    pub(crate) async fn commit_exists_by(&self, r: &str, deadline: Option<Instant>) -> bool {
        if r.is_empty() {
            return false;
        }
        let mut cmd = self.git([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{r}^{{commit}}"),
        ]);
        if let Some(d) = deadline {
            cmd = cmd.deadline(d);
        }
        exec(cmd).await.is_ok()
    }

    async fn same_commit(&self, a: &str, b: &str) -> bool {
        if a.is_empty() || b.is_empty() {
            return false;
        }
        let Ok(ha) = self
            .run([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{a}^{{commit}}"),
            ])
            .await
        else {
            return false;
        };
        let Ok(hb) = self
            .run([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{b}^{{commit}}"),
            ])
            .await
        else {
            return false;
        };
        ha.trim() == hb.trim()
    }

    async fn is_ancestor(&self, ancestor: &str, desc: &str) -> bool {
        if ancestor.is_empty() || desc.is_empty() {
            return false;
        }
        self.run(["merge-base", "--is-ancestor", ancestor, desc])
            .await
            .is_ok()
    }

    /// Whether the repository has at least one commit on any ref.
    async fn has_any_commit(&self) -> bool {
        matches!(self.run(["rev-list", "-n", "1", "--all"]).await, Ok(out) if !out.trim().is_empty())
    }

    /// An existing local branch that would D/F-conflict with creating
    /// `refs/heads/<branch>` (creating `josh/foo` fails when `josh` exists).
    async fn blocking_ref_for_branch(&self, branch: &str) -> String {
        let branch = branch.trim_matches('/');
        let segs: Vec<&str> = branch.split('/').collect();
        let mut acc = String::new();
        for seg in &segs[..segs.len().saturating_sub(1)] {
            if acc.is_empty() {
                acc = (*seg).to_string();
            } else {
                acc = format!("{acc}/{seg}");
            }
            if self.branch_exists(&acc).await {
                return acc;
            }
        }
        String::new()
    }

    /// Whether a local branch exists.
    pub(crate) async fn branch_exists(&self, branch: &str) -> bool {
        self.run([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .await
        .is_ok()
    }

    /// Whether `name` is a local head or a remote-tracking branch, the set the
    /// base-branch picker offers. Tags, SHAs, `HEAD` and symbolic remote HEADs
    /// are rejected so a merge-base cannot collapse onto the worktree's HEAD.
    pub async fn base_branch_exists(&self, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() || name == "HEAD" || name.ends_with("/HEAD") {
            return false;
        }
        if self.branch_exists(name).await {
            return true;
        }
        self.run([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/remotes/{name}"),
        ])
        .await
        .is_ok()
    }

    /// Files git still marks unmerged in the worktree (`--diff-filter=U`).
    pub async fn unmerged_paths(&self, worktree: &str) -> Vec<String> {
        match self
            .run_in(worktree, ["diff", "--name-only", "--diff-filter=U"])
            .await
        {
            Ok(out) => out
                .trim()
                .split('\n')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Whether a merge is in progress (`MERGE_HEAD` set).
    pub async fn merge_head(&self, worktree: &str) -> bool {
        self.ref_set(worktree, "MERGE_HEAD").await
    }

    /// Whether the in-progress `MERGE_HEAD` is the same commit as `r`.
    pub async fn merge_head_equals(&self, worktree: &str, r: &str) -> bool {
        if worktree.trim().is_empty() || r.trim().is_empty() {
            return false;
        }
        let Ok(head) = self
            .run_in(worktree, ["rev-parse", "-q", "--verify", "MERGE_HEAD"])
            .await
        else {
            return false;
        };
        let Ok(other) = self
            .run_in(
                worktree,
                ["rev-parse", "-q", "--verify", &format!("{r}^{{commit}}")],
            )
            .await
        else {
            return false;
        };
        head.trim() == other.trim()
    }

    /// Merges `branch` into the branch checked out at `worktree` with a
    /// `--no-ff` commit as the given identity. A content conflict leaves the
    /// merge in progress and returns the conflicted paths (a later
    /// [`commit`](Self::commit) concludes it). Other failures abort.
    pub async fn merge_into(
        &self,
        worktree: &str,
        branch: &str,
        message: &str,
        author_name: &str,
        author_email: &str,
    ) -> Result<Vec<String>> {
        if worktree.trim().is_empty() || branch.trim().is_empty() {
            return Err(Error::msg("merge into: worktree and branch are required"));
        }
        let user_name = format!("user.name={author_name}");
        let user_email = format!("user.email={author_email}");
        let mut args = vec![
            "-c",
            &user_name,
            "-c",
            &user_email,
            "merge",
            "--no-ff",
            "--no-edit",
        ];
        if !message.is_empty() {
            args.extend(["-m", message]);
        }
        args.push(branch);
        if let Err(merr) = exec(self.git_in(worktree, &args).env("GIT_EDITOR", "true")).await {
            let conflicts = self.unmerged_paths(worktree).await;
            if !conflicts.is_empty() {
                return Ok(conflicts);
            }
            let _ = self.run_in(worktree, ["merge", "--abort"]).await;
            return Err(merr.wrap(format!("git merge {branch}")));
        }
        Ok(Vec::new())
    }
}

/// Runs a prepared git command and returns stdout, or an error that leads
/// with git's stderr (Go `m.runEnv`).
pub(crate) async fn exec(cmd: Command) -> Result<String> {
    let ran = cmd.capture().await;
    match ran.result {
        Ok(()) => Ok(go_string(&ran.stdout)),
        Err(e) => {
            let msg = go_string(&ran.stderr);
            let msg = msg.trim();
            Err(if msg.is_empty() { e } else { e.wrap(msg) })
        }
    }
}

/// Outcome of committing a worktree's changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitResult {
    /// The new commit's short hash (empty when nothing was committed).
    pub hash: String,
    /// False when the worktree had nothing to commit.
    pub committed: bool,
    pub additions: i64,
    pub deletions: i64,
}

/// Outcome of a local merge: the base ref's new hash.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeResult {
    pub new_head: String,
    pub fast_forward: bool,
}

/// A local merge could not land because the base checkout (the project
/// folder, not the workspace worktree) has uncommitted changes git would
/// overwrite.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirtyBaseError {
    pub base: String,
    pub files: Vec<String>,
}

impl fmt::Display for DirtyBaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let base = if self.base.is_empty() {
            "base"
        } else {
            &self.base
        };
        write!(
            f,
            "the project folder ({base}) has uncommitted changes that would be overwritten"
        )?;
        if !self.files.is_empty() {
            let shown = &self.files[..self.files.len().min(3)];
            write!(f, " ({}", shown.join(", "))?;
            if self.files.len() > 3 {
                write!(f, ", and {} more", self.files.len() - 3)?;
            }
            write!(f, ")")?;
        }
        write!(f, " — commit, stash, or discard them there, then try again")
    }
}

impl std::error::Error for DirtyBaseError {}

fn dirty_base_from_git(base: &str, err: &Error) -> Option<DirtyBaseError> {
    if !err.message().contains("would be overwritten by") {
        return None;
    }
    Some(DirtyBaseError {
        base: base.to_string(),
        files: overwritten_paths(err.message()),
    })
}

fn merge_op_error(base: &str, err: &Error, fallback: Error) -> Error {
    match dirty_base_from_git(base, err) {
        Some(d) => d.into(),
        None => fallback,
    }
}

/// The file list from git's "would be overwritten by merge/checkout" stderr.
pub(crate) fn overwritten_paths(msg: &str) -> Vec<String> {
    const MARKER: &str = "would be overwritten by";
    let Some(i) = msg.find(MARKER) else {
        return Vec::new();
    };
    let rest = &msg[i..];
    let Some(nl) = rest.find('\n') else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for line in rest[nl + 1..].split('\n') {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("Please ") || line.starts_with("Aborting") || line.starts_with("error:")
        {
            break;
        }
        files.push(line.to_string());
    }
    files
}

fn porcelain_has_conflict(st: &str) -> bool {
    st.split('\n').any(|line| {
        let b = line.as_bytes();
        if b.len() < 2 {
            return false;
        }
        let (a, b) = (b[0], b[1]);
        a == b'U' || b == b'U' || (a == b'A' && b == b'A') || (a == b'D' && b == b'D')
    })
}

/// Uncommitted or conflicted state that discard/delete would throw away.
/// Missing or unreadable worktrees report none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiscardRisk {
    pub dirty_count: i64,
    pub unmerged: bool,
}

impl DiscardRisk {
    /// A short phrase for confirm copy, or empty when clean.
    pub fn summary(&self) -> String {
        match (self.unmerged, self.dirty_count) {
            (true, n) if n > 0 => "unmerged + dirty".to_string(),
            (true, _) => "unmerged changes".to_string(),
            (false, 1) => "1 uncommitted change".to_string(),
            (false, n) if n > 1 => format!("{n} uncommitted changes"),
            _ => String::new(),
        }
    }
}

/// Dirty/unmerged state for a worktree path.
pub async fn inspect_discard_risk(worktree: &str) -> DiscardRisk {
    if worktree.trim().is_empty() || std::fs::metadata(worktree).is_err() {
        return DiscardRisk::default();
    }
    let st = crate::status::status_map(worktree).await;
    let dirty_count = st.map_or(0, |m| i64::try_from(m.len()).unwrap_or(i64::MAX));
    DiscardRisk {
        dirty_count,
        unmerged: worktree_unmerged(worktree).await,
    }
}

/// Merge/rebase conflicts or an in-progress merge/rebase.
async fn worktree_unmerged(worktree: &str) -> bool {
    for r in ["MERGE_HEAD", "REBASE_HEAD"] {
        if let Ok(out) = command(["-C", worktree, "rev-parse", "-q", "--verify", r])
            .output()
            .await
            && !go_string(&out).trim().is_empty()
        {
            return true;
        }
    }
    match command(["-C", worktree, "status", "--porcelain"])
        .output()
        .await
    {
        Ok(out) => porcelain_has_conflict(&go_string(&out)),
        Err(_) => false,
    }
}

/// Whether `path` is the root of a git work tree (checkout or linked
/// worktree), not merely a folder inside an enclosing repo. Empty, missing
/// and plain directories are false. Gates worktree-based workspaces; non-git
/// folders may still open as a working copy.
pub async fn is_repo(path: &str) -> bool {
    let path = expand_home(path);
    let path = path.trim();
    if path.is_empty() {
        return false;
    }
    let Ok(abs) = gopath::abs(path) else {
        return false;
    };
    // --show-toplevel must equal the project path so a docs/ subfolder (or a
    // folder under a git-tracked $HOME) is not treated as a workspace host.
    let Some(top_abs) = show_toplevel(&abs).await else {
        return false;
    };
    same_dir_path(&abs, &top_abs)
}

async fn show_toplevel(abs: &str) -> Option<String> {
    let (out, res) = command(["-C", abs, "rev-parse", "--show-toplevel"])
        .timeout(IS_REPO_TIMEOUT)
        .combined_output()
        .await;
    res.ok()?;
    let top = gopath::clean(go_string(&out).trim());
    if top.is_empty() {
        return None;
    }
    gopath::abs(&top).ok()
}

/// An absolute, clean path with `~` expanded and symlinks resolved when it
/// exists. Missing paths still return the absolute path so callers can
/// compare identity before stat.
pub fn canon_path(p: &str) -> Result<String> {
    let p = expand_home(p);
    let p = p.trim();
    if p.is_empty() {
        return Err(Error::msg("empty path"));
    }
    let abs = gopath::abs(p)?;
    Ok(gopath::eval_symlinks(&abs).unwrap_or(abs))
}

/// Whether `path` is `root` or below it. Both should be [`canon_path`]
/// results. On macOS a case-folded prefix matches only when both prefixes are
/// the same inode (a case-insensitive volume).
pub fn path_has_prefix(path: &str, root: &str) -> bool {
    if path.is_empty() || root.is_empty() {
        return false;
    }
    if same_path(path, root) {
        return true;
    }
    let prefix = format!("{root}/");
    if path.starts_with(&prefix) {
        return true;
    }
    if fold_path_case()
        && path.len() >= prefix.len()
        && let (Some(head), Some(root_part)) = (path.get(..prefix.len()), path.get(..root.len()))
        && eq_fold(head, &prefix)
    {
        return same_path_on_disk(root_part, root);
    }
    false
}

/// Whether `a` and `b` name the same location. Byte-equal paths match without
/// touching disk; a case-folded pair matches only when both exist and are the
/// same inode.
pub fn same_path(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if !fold_path_case() || a.is_empty() || b.is_empty() || !eq_fold(a, b) {
        return false;
    }
    same_path_on_disk(a, b)
}

/// Rewrites `abs` to use `root`'s exact prefix bytes when
/// [`path_has_prefix`]. A case-folded match would otherwise make a relative
/// path climb out and back in.
pub fn align_under_root(abs: &str, root: &str) -> String {
    if !path_has_prefix(abs, root) {
        return abs.to_string();
    }
    if same_path(abs, root) || abs.len() < root.len() {
        return root.to_string();
    }
    match abs.get(root.len()..) {
        Some(rest) => format!("{root}{rest}"),
        None => abs.to_string(),
    }
}

fn fold_path_case() -> bool {
    cfg!(target_os = "macos")
}

fn same_path_on_disk(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(fa), Ok(fb)) => fa.dev() == fb.dev() && fa.ino() == fb.ino(),
        _ => false,
    }
}

/// The git toplevel containing `path` (file or directory). Unlike
/// [`is_repo`], a nested folder resolves to the enclosing checkout.
pub async fn repo_root(path: &str) -> Option<String> {
    let path = expand_home(path);
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    let mut abs = gopath::abs(path).ok()?;
    if std::fs::metadata(&abs).is_ok_and(|m| !m.is_dir()) {
        abs = gopath::dir(&abs);
    }
    let top_abs = show_toplevel(&abs).await?;
    Some(gopath::eval_symlinks(&top_abs).unwrap_or(top_abs))
}

/// Whether `a` and `b` are the same directory, resolving symlinks.
fn same_dir_path(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (gopath::eval_symlinks(a), gopath::eval_symlinks(b)) {
        (Ok(ae), Ok(be)) => ae == be,
        _ => false,
    }
}

/// `prefix/slug`; an empty prefix returns the slug alone. Both sides are
/// trimmed of slashes.
pub fn prefixed_branch(prefix: &str, slug: &str) -> String {
    let prefix = prefix.trim_matches('/');
    let slug = slug.trim_matches('/');
    if prefix.is_empty() {
        return slug.to_string();
    }
    if slug.is_empty() {
        return prefix.to_string();
    }
    format!("{prefix}/{slug}")
}

/// The default workspace branch (`recur/<slug>`). Callers honoring Settings
/// use [`prefixed_branch`] with the resolved prefix.
pub fn branch_for_slug(slug: &str) -> String {
    prefixed_branch(DEFAULT_BRANCH_PREFIX, slug)
}

/// A free-form prefix as a git-safe path fragment (lowercase, illegal
/// characters to hyphens). Empty stays empty so "no prefix" survives.
pub fn normalize_branch_prefix(prefix: &str) -> String {
    let prefix = prefix.trim();
    if prefix.is_empty() {
        return String::new();
    }
    match sanitize_git_path(prefix) {
        Ok(name) => name.trim_matches('/').to_string(),
        Err(_) => String::new(),
    }
}

/// Splits a `{project}.{slug}` workspace id at the last dot. Legacy ids (no
/// dot) and empty segments return `None`.
pub fn split_workspace_id(id: &str) -> Option<(String, String)> {
    let id = id.trim();
    let i = id.rfind('.')?;
    if i == 0 || i == id.len() - 1 {
        return None;
    }
    let (project, slug) = (&id[..i], &id[i + 1..]);
    if project == "." || project == ".." || slug == "." || slug == ".." {
        return None;
    }
    Some((project.to_string(), slug.to_string()))
}

/// The leaf of a workspace id (`flaky-ramen` for `recur.flaky-ramen`).
/// Legacy ids are returned unchanged.
pub fn workspace_slug(id: &str) -> String {
    match split_workspace_id(id) {
        Some((_, slug)) => slug,
        None => id.trim().to_string(),
    }
}

/// The directory under the worktrees root for a workspace id:
/// `{project}.{slug}` → `{project}/{slug}`; legacy ids stay one segment.
pub fn worktree_rel_path(workspace_id: &str) -> String {
    let workspace_id = workspace_id.trim();
    match split_workspace_id(workspace_id) {
        Some((project, slug)) => gopath::join(&[&project, &slug]),
        None => workspace_id.to_string(),
    }
}

/// The checked-out branch in `worktree` (`git branch --show-current`). Empty
/// means detached HEAD. The live branch is the source of truth: humans and
/// agents may rename or switch branches mid-session.
pub async fn current_branch(worktree: &str) -> Result<String> {
    if worktree.trim().is_empty() {
        return Err(Error::msg("current branch: empty worktree"));
    }
    let (out, res) = command(["-C", worktree, "branch", "--show-current"])
        .combined_output()
        .await;
    let out = go_string(&out);
    if let Err(e) = res {
        let msg = out.trim();
        return Err(if msg.is_empty() {
            e.wrap("git branch --show-current")
        } else {
            e.wrap(format!("git branch --show-current: {msg}"))
        });
    }
    Ok(out.trim().to_string())
}

/// A free-form topic as a git-safe branch, under [`DEFAULT_BRANCH_PREFIX`]
/// when it has no slash.
pub async fn sanitize_branch_name(name: &str) -> Result<String> {
    sanitize_branch_name_with_prefix(DEFAULT_BRANCH_PREFIX, name).await
}

/// A free-form topic as a git-safe branch name: spaces become hyphens, other
/// illegal characters are dropped, and a name with no slash gets `prefix/`
/// (none for an empty prefix). Checked with `git check-ref-format`.
pub async fn sanitize_branch_name_with_prefix(prefix: &str, name: &str) -> Result<String> {
    let mut path = apply_branch_prefix(prefix, &sanitize_git_path(name)?);
    if path.len() > 80 {
        path = join_branch_components(path[..80].trim_matches(['.', '-', '/']));
        if path.is_empty() {
            return Err(Error::msg("invalid branch name"));
        }
        path = apply_branch_prefix(prefix, &path);
    }
    check_ref_format(&path).await?;
    Ok(path)
}

fn apply_branch_prefix(prefix: &str, name: &str) -> String {
    if name.contains('/') {
        return name.to_string();
    }
    let out = prefixed_branch(&normalize_branch_prefix(prefix), name);
    if out.is_empty() {
        name.to_string()
    } else {
        out
    }
}

/// Lowercases `name`, maps illegal characters to hyphens, and collapses empty
/// path components. Empty input is an error.
fn sanitize_git_path(name: &str) -> Result<String> {
    let name = go_lower(name.trim());
    if name.is_empty() {
        return Err(Error::msg("empty branch name"));
    }
    let mut b = String::with_capacity(name.len());
    let mut last_dash = false;
    for r in name.chars() {
        if r.is_ascii_lowercase() || r.is_ascii_digit() || r == '.' || r == '_' || r == '/' {
            b.push(r);
            last_dash = false;
        } else if !last_dash {
            b.push('-');
            last_dash = true;
        }
    }
    let mut name = b.trim_matches(['-', '/']).to_string();
    while name.contains("//") {
        name = name.replace("//", "/");
    }
    while name.contains("..") {
        name = name.replace("..", ".");
    }
    let name = join_branch_components(&name);
    if name.is_empty() {
        return Err(Error::msg("invalid branch name"));
    }
    Ok(name)
}

/// Trims leading/trailing dots and hyphens and strips trailing `.lock` so
/// check-ref-format accepts the component.
fn sanitize_branch_component(p: &str) -> &str {
    let mut p = p.trim_matches(['.', '-']);
    while let Some(stripped) = p.strip_suffix(".lock") {
        p = stripped.trim_matches(['.', '-']);
    }
    p
}

fn join_branch_components(name: &str) -> String {
    name.split('/')
        .map(sanitize_branch_component)
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) async fn check_ref_format(branch: &str) -> Result<()> {
    let refname = format!("refs/heads/{branch}");
    let (out, res) = command(["check-ref-format", "--normalize", refname.as_str()])
        .timeout(IS_REPO_TIMEOUT)
        .combined_output()
        .await;
    if res.is_err() {
        let out = go_string(&out);
        let msg = out.trim();
        return Err(Error::msg(if msg.is_empty() {
            format!("invalid branch name {}", quote(branch))
        } else {
            format!("invalid branch name {}: {msg}", quote(branch))
        }));
    }
    Ok(())
}

/// Renames the branch checked out in `worktree` (`git branch -m`), sanitized
/// with [`DEFAULT_BRANCH_PREFIX`]. Fails on detached HEAD, an empty worktree,
/// or a target that already exists.
pub async fn rename_branch(worktree: &str, new_name: &str) -> Result<String> {
    rename_branch_with_prefix(worktree, DEFAULT_BRANCH_PREFIX, new_name).await
}

/// [`rename_branch`] using `prefix` when `new_name` has no slash.
pub async fn rename_branch_with_prefix(
    worktree: &str,
    prefix: &str,
    new_name: &str,
) -> Result<String> {
    if worktree.trim().is_empty() {
        return Err(Error::msg("rename branch: empty worktree"));
    }
    let new_name = sanitize_branch_name_with_prefix(prefix, new_name)
        .await
        .map_err(|e| e.wrap("rename branch"))?;
    let current = current_branch(worktree).await?;
    if current.is_empty() {
        return Err(Error::msg("rename branch: detached HEAD"));
    }
    if current == new_name {
        return Ok(new_name);
    }
    let head_ref = format!("refs/heads/{new_name}");
    if command([
        "-C",
        worktree,
        "show-ref",
        "--verify",
        "--quiet",
        head_ref.as_str(),
    ])
    .run()
    .await
    .is_ok()
    {
        return Err(Error::msg(format!(
            "rename branch: {} already exists",
            quote(&new_name)
        )));
    }
    let (out, res) = command(["-C", worktree, "branch", "-m", new_name.as_str()])
        .combined_output()
        .await;
    if let Err(e) = res {
        let out = go_string(&out);
        let msg = out.trim();
        return Err(if msg.is_empty() {
            e.wrap(format!("git branch -m {new_name}"))
        } else {
            e.wrap(format!("git branch -m {new_name}: {msg}"))
        });
    }
    Ok(new_name)
}

/// Rejects ids that would escape the root when joined (separators, `..`,
/// empty). `{project}.{slug}` becomes two segments; legacy ids stay one.
pub(crate) fn validate_managed_workspace_id(id: &str) -> Result<()> {
    let id = id.trim();
    let invalid = || Error::msg(format!("invalid worktree id {}", quote(id)));
    if id.is_empty() || id == "." || id == ".." {
        return Err(invalid());
    }
    if id.contains('/') || id.contains('\\') {
        return Err(Error::msg(format!(
            "invalid worktree id {}: must not contain path separators",
            quote(id)
        )));
    }
    let rel = worktree_rel_path(id);
    if rel.is_empty() || rel == "." || rel == ".." || gopath::is_abs(&rel) {
        return Err(invalid());
    }
    if rel
        .split('/')
        .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return Err(invalid());
    }
    if gopath::clean(&rel) != rel {
        return Err(invalid());
    }
    Ok(())
}

/// An absolute, symlink-resolved path. When trailing components do not exist
/// yet, the longest existing prefix is resolved and the rest re-attached.
fn resolve_path_prefix(path: &str) -> std::io::Result<String> {
    let abs = gopath::abs(path)?;
    if let Ok(resolved) = gopath::eval_symlinks(&abs) {
        return Ok(resolved);
    }
    let mut suffix = String::new();
    let mut cur = abs.clone();
    loop {
        let parent = gopath::dir(&cur);
        if parent == cur {
            return Ok(abs);
        }
        suffix = gopath::join(&[&gopath::base(&cur), &suffix]);
        if let Ok(resolved) = gopath::eval_symlinks(&parent) {
            return Ok(gopath::join(&[&resolved, &suffix]));
        }
        cur = parent;
    }
}

/// Parses `git worktree list --porcelain`.
pub(crate) fn parse_porcelain(out: &str) -> Vec<Worktree> {
    let mut list = Vec::new();
    let mut cur = Worktree::default();
    let mut flush = |cur: &mut Worktree| {
        if !cur.path.is_empty() {
            list.push(std::mem::take(cur));
        } else {
            *cur = Worktree::default();
        }
    };
    for line in out.split('\n') {
        let line = line.trim_end_matches('\r');
        if let Some(p) = line.strip_prefix("worktree ") {
            flush(&mut cur);
            cur.path = p.to_string();
        } else if let Some(h) = line.strip_prefix("HEAD ") {
            cur.head = h.to_string();
        } else if let Some(b) = line.strip_prefix("branch ") {
            // "branch refs/heads/<name>" → the short name.
            cur.branch = b.strip_prefix("refs/heads/").unwrap_or(b).to_string();
        } else if line.is_empty() {
            flush(&mut cur);
        }
    }
    flush(&mut cur);
    list
}

pub(crate) fn clean_path(p: &str) -> String {
    let abs = gopath::abs(p).unwrap_or_else(|_| p.to_string());
    gopath::eval_symlinks(&abs).unwrap_or(abs)
}

/// Go `os.RemoveAll`: removes a file or tree; a missing path is not an error.
pub(crate) fn remove_all(path: &str) -> std::io::Result<()> {
    let res = match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) => Err(e),
    };
    match res {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}
