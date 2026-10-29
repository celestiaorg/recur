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

use recur_domain::path;
use tokio::time::Instant;

use crate::command::{Command, command};
use crate::error::{Error, Result};
use crate::scm::identity;
use crate::text::{atoi, eq_fold};
use recur_project::home::expand_home;

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
        let wt_path = path::join(&[&self.root, &worktree_rel_path(workspace_id)]);

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
                    "stale worktree registration at {:?} could not be cleared",
                    wt_path
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
                Error::io_path(&wt_path, &e)
                    .wrap(format!("remove stale worktree path {:?}", wt_path))
            })?;
        }

        let parent = path::dir(&wt_path);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o755)
            .create(&parent)
            .map_err(|e| Error::io_path(&parent, &e).wrap("create worktrees root"))?;

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
                    "cannot create branch {:?}: existing branch {:?} blocks it (git cannot nest refs under a branch of the same name) — pick a different prefix in Settings or rename that branch",
                    branch, blocked
                )));
            }
            let mut base_branch = base_branch.to_string();
            if base_branch.is_empty() {
                base_branch = self.default_branch().await;
            }
            // Remote bases: fetch the tip once so spin-off doesn't need a pull.
            self.refresh_remote_base_if_needed(&base_branch)
                .await
                .map_err(|e| e.wrap(format!("refresh base {:?}", base_branch)))?;
            if !self.commit_exists(&base_branch).await {
                if self.has_any_commit().await {
                    return Err(Error::msg(format!(
                        "base branch {:?} does not exist (or has no commits)",
                        base_branch
                    )));
                }
                if self.parse_remote_tracking(&base_branch).await.is_some() {
                    return Err(Error::msg(format!(
                        "base branch {:?} is a remote ref but is missing — fetch failed or remote is empty",
                        base_branch
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
                    .map_err(|e| e.wrap(format!("prepare base branch {:?}", base_branch)))?;
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
            Ok(subj) => subj.trim().to_lowercase().contains("autostash"),
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
                "refuse to remove worktree path {:?}: not under managed root {:?}",
                path, self.root
            )));
        }
        let git_err = self.force_remove_worktree(path).await.err();
        // Wipe the directory even when git never knew about it (orphan).
        if let Err(e) = remove_all(path) {
            return Err(Error::io_path(path, &e).wrap(format!("remove worktree dir {:?}", path)));
        }
        if matches!(self.worktree_at(path).await, Ok(Some(_))) {
            let _ = self.run(["worktree", "prune"]).await;
            // Locked entries survive prune; a second force-force clears them.
            let _ = self.force_remove_worktree(path).await;
        }
        if matches!(self.worktree_at(path).await, Ok(Some(_))) {
            return Err(match git_err {
                Some(e) => e.wrap(format!("git worktree remove {:?}", path)),
                None => Error::msg(format!("worktree {:?} still registered after remove", path)),
            });
        }
        // Nested <root>/<project>/<slug>: drop the project dir when this was
        // its last worktree (fails, ignored, when not empty).
        let parent = path::dir(path);
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
        match path::rel(&root, &p) {
            Some(rel) => !(rel == "." || rel == ".." || rel.starts_with("../")),
            None => false,
        }
    }

    /// Every worktree registered for the repository (the main checkout first).
    pub async fn list(&self) -> Result<Vec<Worktree>> {
        let mut attempt = 0;
        loop {
            match self.run(["worktree", "list", "--porcelain"]).await {
                Ok(out) => return Ok(parse_porcelain(&out)),
                // A concurrent `git worktree add` creates `.git/worktrees/<name>/` before it
                // writes the files `list` reads there; that window lasts milliseconds.
                Err(e) if attempt < 5 && half_added_worktree(e.message()) => {
                    attempt += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(20 * attempt)).await;
                }
                Err(e) => return Err(e.wrap("git worktree list")),
            }
        }
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
        Ok(()) => Ok(String::from_utf8_lossy(&ran.stdout).into_owned()),
        Err(e) => {
            let msg = String::from_utf8_lossy(&ran.stderr).into_owned();
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
            && !String::from_utf8_lossy(&out).into_owned().trim().is_empty()
        {
            return true;
        }
    }
    match command(["-C", worktree, "status", "--porcelain"])
        .output()
        .await
    {
        Ok(out) => porcelain_has_conflict(&String::from_utf8_lossy(&out)),
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
    let Ok(abs) = path::abs(path) else {
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
    let top = path::clean(String::from_utf8_lossy(&out).into_owned().trim());
    if top.is_empty() {
        return None;
    }
    path::abs(&top).ok()
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
    let abs = path::abs(p)?;
    Ok(path::eval_symlinks(&abs).unwrap_or(abs))
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
    let mut abs = path::abs(path).ok()?;
    if std::fs::metadata(&abs).is_ok_and(|m| !m.is_dir()) {
        abs = path::dir(&abs);
    }
    let top_abs = show_toplevel(&abs).await?;
    Some(path::eval_symlinks(&top_abs).unwrap_or(top_abs))
}

/// Whether `a` and `b` are the same directory, resolving symlinks.
fn same_dir_path(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (path::eval_symlinks(a), path::eval_symlinks(b)) {
        (Some(ae), Some(be)) => ae == be,
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
        Some((project, slug)) => path::join(&[&project, &slug]),
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
    let out = String::from_utf8_lossy(&out).into_owned();
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
    let name = name.trim().to_lowercase();
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
        let out = String::from_utf8_lossy(&out).into_owned();
        let msg = out.trim();
        return Err(Error::msg(if msg.is_empty() {
            format!("invalid branch name {:?}", branch)
        } else {
            format!("invalid branch name {:?}: {msg}", branch)
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
            "rename branch: {:?} already exists",
            new_name
        )));
    }
    let (out, res) = command(["-C", worktree, "branch", "-m", new_name.as_str()])
        .combined_output()
        .await;
    if let Err(e) = res {
        let out = String::from_utf8_lossy(&out).into_owned();
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
    let invalid = || Error::msg(format!("invalid worktree id {:?}", id));
    if id.is_empty() || id == "." || id == ".." {
        return Err(invalid());
    }
    if id.contains('/') || id.contains('\\') {
        return Err(Error::msg(format!(
            "invalid worktree id {:?}: must not contain path separators",
            id
        )));
    }
    let rel = worktree_rel_path(id);
    if rel.is_empty() || rel == "." || rel == ".." || path::is_abs(&rel) {
        return Err(invalid());
    }
    if rel
        .split('/')
        .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return Err(invalid());
    }
    if path::clean(&rel) != rel {
        return Err(invalid());
    }
    Ok(())
}

/// An absolute, symlink-resolved path. When trailing components do not exist
/// yet, the longest existing prefix is resolved and the rest re-attached.
fn resolve_path_prefix(path: &str) -> std::io::Result<String> {
    let abs = path::abs(path)?;
    if let Some(resolved) = path::eval_symlinks(&abs) {
        return Ok(resolved);
    }
    let mut suffix = String::new();
    let mut cur = abs.clone();
    loop {
        let parent = path::dir(&cur);
        if parent == cur {
            return Ok(abs);
        }
        suffix = path::join(&[&path::base(&cur), &suffix]);
        if let Some(resolved) = path::eval_symlinks(&parent) {
            return Ok(path::join(&[&resolved, &suffix]));
        }
        cur = parent;
    }
}

/// `git worktree list` failing on a sibling another `git worktree add` is still writing.
fn half_added_worktree(message: &str) -> bool {
    message.contains("failed to read") && message.contains("worktrees/")
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
    let abs = path::abs(p).unwrap_or_else(|_| p.to_string());
    path::eval_symlinks(&abs).unwrap_or(abs)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sibling_still_being_added_is_retried_and_other_failures_are_not() {
        assert!(half_added_worktree(
            "fatal: failed to read .git/worktrees/bubbly-churro/commondir: Undefined error: 0: exit status 128"
        ));
        assert!(!half_added_worktree("fatal: not a git repository"));
    }
    use crate::testutil::{clean, exists, git, rev_parse, temp_dir, write};

    fn mgr(repo: &str, root: &str) -> Manager {
        Manager::new(repo, root)
    }

    #[tokio::test]
    async fn is_repo_only_for_toplevels() {
        assert!(!is_repo("").await);
        let plain = temp_dir();
        assert!(
            !is_repo(&plain.path).await,
            "plain folder must not report as a git repo"
        );
        let repo = crate::testutil::init_repo();
        assert!(
            is_repo(&repo.path).await,
            "{} should be a git repo",
            repo.path
        );
        // A nested folder sits inside a work tree but is not a workspace host.
        let sub = format!("{}/nested", repo.path);
        std::fs::create_dir(&sub).expect("mkdir");
        assert!(
            !is_repo(&sub).await,
            "subdir {sub} must not count as a project root"
        );
    }

    #[tokio::test]
    async fn repo_root_resolves_nested_paths() {
        let plain = temp_dir();
        assert_eq!(repo_root(&plain.path).await, None);
        let repo = crate::testutil::init_repo();
        let top = repo_root(&repo.path).await.expect("repo_root(repo)");
        let want = path::eval_symlinks(&repo.path).unwrap_or_default();
        assert!(
            top == want || top == path::clean(&repo.path),
            "repo_root = {top}, want {want}"
        );
        write(&repo.path, "nested/a.txt", "x\n");
        let from_file = repo_root(&format!("{}/nested/a.txt", repo.path)).await;
        assert_eq!(from_file.as_deref(), Some(top.as_str()));
    }

    fn flip_case(p: &str) -> String {
        let mut b = p.as_bytes().to_vec();
        for i in (0..b.len()).rev() {
            if b[i].is_ascii_lowercase() {
                b[i] = b[i].to_ascii_uppercase();
                break;
            }
            if b[i].is_ascii_uppercase() {
                b[i] = b[i].to_ascii_lowercase();
                break;
            }
        }
        String::from_utf8(b).unwrap_or_else(|_| p.to_string())
    }

    #[test]
    fn path_has_prefix_cases() {
        assert!(path_has_prefix("/foo/bar", "/foo"));
        assert!(!path_has_prefix("/foobar", "/foo"));
        assert!(path_has_prefix("/foo", "/foo"));
        let tmp = temp_dir();
        let dir = tmp.path.clone();
        let alt = flip_case(&dir);
        if alt == dir || std::fs::metadata(&alt).is_err() {
            return; // case-sensitive volume: folding must not invent a match
        }
        std::fs::create_dir(format!("{dir}/bar")).expect("mkdir");
        let child = format!("{alt}/bar");
        assert!(
            path_has_prefix(&child, &dir),
            "path_has_prefix({child}, {dir}) on a case-insensitive volume"
        );
        assert!(same_path(&alt, &dir));
        assert_eq!(align_under_root(&child, &dir), format!("{dir}/bar"));
    }

    #[tokio::test]
    async fn add_list_remove() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (wt, _) = m.add("LOOP-1", "rate-limit", "main").await.expect("add");
        assert_eq!(wt.branch, "recur/rate-limit");
        assert!(exists(&wt.path), "worktree dir not created");
        let list = m.list().await.expect("list");
        let found = list.iter().find(|w| clean(&w.path) == clean(&wt.path));
        assert_eq!(
            found.map(|w| w.branch.as_str()),
            Some("recur/rate-limit"),
            "added worktree not listed: {list:?}"
        );
        m.remove(&wt.path).await.expect("remove");
        assert!(!exists(&wt.path), "worktree dir still present after remove");
    }

    #[test]
    fn worktree_rel_path_cases() {
        for (id, want) in [
            ("recur.flaky-ramen", "recur/flaky-ramen"),
            ("flaky-ramen", "flaky-ramen"),
            ("LOOP-1", "LOOP-1"),
            ("my-app.bubbly-pretzel-2", "my-app/bubbly-pretzel-2"),
            ("", ""),
        ] {
            assert_eq!(worktree_rel_path(id), want, "worktree_rel_path({id:?})");
        }
        assert_eq!(workspace_slug("recur.flaky-ramen"), "flaky-ramen");
        assert_eq!(workspace_slug("flaky-ramen"), "flaky-ramen");
    }

    #[tokio::test]
    async fn add_nested_project_layout() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (wt, _) = m
            .add("kitchen.bubbly-pretzel", "bubbly-pretzel", "main")
            .await
            .expect("add");
        let want = format!("{}/kitchen/bubbly-pretzel", root.path);
        assert_eq!(wt.path, want);
        assert_eq!(wt.branch, "recur/bubbly-pretzel");
        assert!(exists(&want), "nested dir missing");
        m.remove(&wt.path).await.expect("remove");
        assert!(!exists(&want), "nested worktree still present");
        assert!(
            !exists(&format!("{}/kitchen", root.path)),
            "empty project dir not pruned"
        );
    }

    #[test]
    fn validate_managed_workspace_id_cases() {
        for id in [
            "flaky-ramen",
            "kitchen.bubbly-pretzel",
            "LOOP-1",
            "my-app.x-2",
        ] {
            assert!(
                validate_managed_workspace_id(id).is_ok(),
                "{id} should be valid"
            );
        }
        for id in ["", ".", "..", "../escape", "foo/bar", "foo\\bar"] {
            assert!(
                validate_managed_workspace_id(id).is_err(),
                "{id:?} should be invalid"
            );
        }
    }

    #[tokio::test]
    async fn current_branch_follows_renames() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (wt, _) = m.add("LOOP-CB", "cb", "main").await.expect("add");
        assert_eq!(current_branch(&wt.path).await.expect("current"), "recur/cb");
        git(&wt.path, &["branch", "-m", "renamed/feature"]);
        assert_eq!(
            current_branch(&wt.path).await.expect("current"),
            "renamed/feature"
        );
        assert!(current_branch("").await.is_err());
    }

    #[tokio::test]
    async fn sanitize_branch_name_cases() {
        for (input, want) in [
            ("add-dark-mode", "recur/add-dark-mode"),
            ("Add Dark Mode", "recur/add-dark-mode"),
            ("recur/already-prefixed", "recur/already-prefixed"),
            ("feature/foo", "feature/foo"),
            ("Fix bug.", "recur/fix-bug"),
            (".net support", "recur/net-support"),
            ("fix db.lock", "recur/fix-db"),
        ] {
            assert_eq!(
                sanitize_branch_name(input).await.expect(input),
                want,
                "sanitize_branch_name({input:?})"
            );
        }
        assert!(sanitize_branch_name("   ").await.is_err());
    }

    #[test]
    fn prefixed_branch_cases() {
        for (prefix, slug, want) in [
            ("recur", "rate-limit", "recur/rate-limit"),
            ("", "rate-limit", "rate-limit"),
            ("josh", "rate-limit", "josh/rate-limit"),
            ("/josh/", "/rate-limit/", "josh/rate-limit"),
        ] {
            assert_eq!(prefixed_branch(prefix, slug), want);
        }
    }

    #[test]
    fn normalize_branch_prefix_cases() {
        for (input, want) in [
            ("", ""),
            ("  ", ""),
            ("Recur", "recur"),
            ("My User", "my-user"),
            ("foo/bar", "foo/bar"),
        ] {
            assert_eq!(
                normalize_branch_prefix(input),
                want,
                "normalize_branch_prefix({input:?})"
            );
        }
    }

    #[tokio::test]
    async fn sanitize_branch_name_with_prefix_cases() {
        for (prefix, input, want) in [
            ("josh", "add-dark-mode", "josh/add-dark-mode"),
            ("", "add-dark-mode", "add-dark-mode"),
            ("josh", "feature/foo", "feature/foo"),
            ("MyUser", "Add Dark Mode", "myuser/add-dark-mode"),
        ] {
            let got = sanitize_branch_name_with_prefix(prefix, input)
                .await
                .expect(input);
            assert_eq!(
                got, want,
                "sanitize_branch_name_with_prefix({prefix:?}, {input:?})"
            );
        }
    }

    #[tokio::test]
    async fn add_uses_branch_prefix() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let (custom, _) = mgr(&repo.path, &root.path)
            .with_branch_prefix("josh")
            .add("p.custom-ws", "custom-ws", "main")
            .await
            .expect("add");
        assert_eq!(custom.branch, "josh/custom-ws");
        let (none, _) = mgr(&repo.path, &root.path)
            .with_branch_prefix("")
            .add("p.none-ws", "none-ws", "main")
            .await
            .expect("add");
        assert_eq!(none.branch, "none-ws");
    }

    #[tokio::test]
    async fn add_rejects_prefix_that_exists_as_branch() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        mgr(&repo.path, &root.path)
            .run(["branch", "josh"])
            .await
            .expect("git branch josh");
        let err = mgr(&repo.path, &root.path)
            .with_branch_prefix("josh")
            .add("p.collide-ws", "collide-ws", "main")
            .await
            .expect_err("prefix blocked by a branch must fail");
        assert!(
            err.message().contains("josh") && err.message().contains("blocks"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn rename_branch_cases() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (wt, _) = m.add("LOOP-RB", "rb", "main").await.expect("add");
        assert_eq!(
            rename_branch(&wt.path, "add-dark-mode")
                .await
                .expect("rename"),
            "recur/add-dark-mode"
        );
        assert_eq!(
            current_branch(&wt.path).await.expect("current"),
            "recur/add-dark-mode"
        );
        assert_eq!(
            rename_branch(&wt.path, "add-dark-mode")
                .await
                .expect("idempotent"),
            "recur/add-dark-mode"
        );
        git(&wt.path, &["branch", "recur/taken"]);
        assert!(
            rename_branch(&wt.path, "taken").await.is_err(),
            "rename onto an existing branch must fail"
        );
    }

    #[tokio::test]
    async fn list_local_branches_includes_new() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        m.run(["branch", "feature/x"]).await.expect("branch");
        let branches = m.list_local_branches().await.expect("list");
        assert!(
            branches.contains(&"main".into()) && branches.contains(&"feature/x".into()),
            "{branches:?}"
        );
    }

    #[tokio::test]
    async fn add_empty_repo_creates_initial_commit() {
        let dir = temp_dir();
        let d = dir.path.as_str();
        git(d, &["init", "-b", "main"]);
        git(d, &["config", "user.name", "test"]);
        git(d, &["config", "user.email", "test@example.com"]);
        // An untracked file the initial commit must not pick up.
        write(d, "scratch.txt", "leave me\n");
        let root = temp_dir();
        let m = mgr(d, &root.path);
        let (wt, _) = m
            .add("LOOP-EMPTY", "first-task", "main")
            .await
            .expect("add on empty repo");
        assert_eq!(wt.branch, "recur/first-task");
        assert!(
            m.commit_exists("main").await,
            "expected an initial commit on main"
        );
        let author = m
            .run(["log", "-1", "--format=%an <%ae>"])
            .await
            .expect("log");
        assert_eq!(author.trim(), "test <test@example.com>");
        let st = m.run(["status", "--porcelain"]).await.expect("status");
        assert!(
            st.contains("scratch.txt"),
            "scratch.txt should stay untracked: {st}"
        );
        let tracked = m
            .run(["-C", wt.path.as_str(), "ls-files"])
            .await
            .unwrap_or_default();
        assert!(
            !tracked.contains("scratch.txt"),
            "initial commit must not include untracked files"
        );
    }

    #[tokio::test]
    async fn add_is_idempotent() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (wt1, created1) = m
            .add("LOOP-2", "feature-x", "main")
            .await
            .expect("first add");
        assert!(created1, "first add should report created");
        let (wt2, created2) = m
            .add("LOOP-2", "feature-x", "main")
            .await
            .expect("second add");
        assert!(!created2, "second add should report not created");
        assert_eq!(clean(&wt1.path), clean(&wt2.path));
    }

    #[tokio::test]
    async fn add_does_not_reuse_leftover_branch() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (wt1, _) = m.add("first-ws", "feature-x", "main").await.expect("add");
        write(&wt1.path, "stale.txt", "old work\n");
        m.commit(&wt1.path, "stale work", "test", "test@example.com")
            .await
            .expect("commit");
        let stale_head = rev_parse(&repo.path, "recur/feature-x");
        let main_head = rev_parse(&repo.path, "main");
        assert_ne!(stale_head, main_head);
        m.remove(&wt1.path).await.expect("remove");
        let (wt2, created) = m
            .add("second-ws", "feature-x", "main")
            .await
            .expect("add over leftover");
        assert!(created);
        assert_eq!(wt2.branch, "recur/feature-x-2");
        assert_eq!(
            rev_parse(&repo.path, &wt2.branch),
            main_head,
            "new branch must start at main, not the leftover"
        );
        assert!(
            !exists(&format!("{}/stale.txt", wt2.path)),
            "new worktree must not carry stale.txt"
        );
    }

    #[tokio::test]
    async fn add_skips_taken_numeric_suffixes() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        for b in ["recur/feature-x", "recur/feature-x-2"] {
            m.run(["branch", b]).await.expect("branch");
        }
        let (wt, _) = m.add("third-ws", "feature-x", "main").await.expect("add");
        assert_eq!(wt.branch, "recur/feature-x-3");
    }

    #[tokio::test]
    async fn restore_reuses_existing_branch() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (wt1, _) = m
            .add("archived-ws", "archived-ws", "main")
            .await
            .expect("add");
        write(&wt1.path, "kept.txt", "keep me\n");
        m.commit(&wt1.path, "keep", "test", "test@example.com")
            .await
            .expect("commit");
        let want_head = rev_parse(&repo.path, "recur/archived-ws");
        m.remove(&wt1.path).await.expect("remove");
        let (wt2, created) = m
            .restore("archived-ws", "recur/archived-ws", "main")
            .await
            .expect("restore");
        assert!(created, "restore after remove should allocate a worktree");
        assert_eq!(wt2.branch, "recur/archived-ws");
        assert_eq!(rev_parse(&repo.path, &wt2.branch), want_head);
        assert!(
            exists(&format!("{}/kept.txt", wt2.path)),
            "restore should keep the branch's file"
        );
    }

    #[tokio::test]
    async fn restore_empty_branch_uses_default_prefix() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let (wt1, _) = mgr(&repo.path, &root.path)
            .add("legacy-ws", "legacy-ws", "main")
            .await
            .expect("add");
        write(&wt1.path, "kept.txt", "keep me\n");
        mgr(&repo.path, &root.path)
            .commit(&wt1.path, "keep", "test", "test@example.com")
            .await
            .expect("commit");
        let want_head = rev_parse(&repo.path, "recur/legacy-ws");
        mgr(&repo.path, &root.path)
            .remove(&wt1.path)
            .await
            .expect("remove");
        let (wt2, _) = mgr(&repo.path, &root.path)
            .with_branch_prefix("josh")
            .restore("legacy-ws", "", "main")
            .await
            .expect("restore");
        assert_eq!(
            wt2.branch, "recur/legacy-ws",
            "empty branch must restore recur/<slug>, not the live prefix"
        );
        assert_eq!(rev_parse(&repo.path, &wt2.branch), want_head);
    }

    #[tokio::test]
    async fn restore_uses_recorded_suffixed_branch() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        m.run(["branch", "recur/archived-ws"])
            .await
            .expect("leftover");
        let (wt1, _) = m
            .add("archived-ws", "archived-ws", "main")
            .await
            .expect("add");
        assert_eq!(wt1.branch, "recur/archived-ws-2");
        write(&wt1.path, "kept.txt", "keep me\n");
        m.commit(&wt1.path, "keep", "test", "test@example.com")
            .await
            .expect("commit");
        let want_head = rev_parse(&repo.path, "recur/archived-ws-2");
        m.remove(&wt1.path).await.expect("remove");
        let (wt2, _) = m
            .restore("archived-ws", "recur/archived-ws-2", "main")
            .await
            .expect("restore");
        assert_eq!(wt2.branch, "recur/archived-ws-2");
        assert_eq!(rev_parse(&repo.path, &wt2.branch), want_head);
        assert!(exists(&format!("{}/kept.txt", wt2.path)));
    }

    #[tokio::test]
    async fn add_at_ref_fast_forwards_ancestor_branch() {
        let repo = crate::testutil::init_repo();
        let r = repo.path.as_str();
        git(r, &["branch", "feat/oauth"]);
        write(r, "fresh.txt", "new\n");
        git(r, &["add", "."]);
        git(r, &["commit", "-m", "fresh"]);
        let fresh = rev_parse(r, "HEAD");
        assert_ne!(rev_parse(r, "feat/oauth"), fresh);
        let root = temp_dir();
        let (wt, _) = mgr(r, &root.path)
            .add_at_ref("from-pr", "from-pr", "feat/oauth", &fresh)
            .await
            .expect("add_at_ref");
        assert_eq!(wt.branch, "feat/oauth");
        assert_eq!(rev_parse(&wt.path, "HEAD"), fresh);
    }

    #[tokio::test]
    async fn add_at_ref_falls_back_when_local_diverged() {
        let repo = crate::testutil::init_repo();
        let r = repo.path.as_str();
        git(r, &["checkout", "-b", "feat/oauth"]);
        write(r, "stale.txt", "old\n");
        git(r, &["add", "."]);
        git(r, &["commit", "-m", "stale"]);
        let stale = rev_parse(r, "HEAD");
        git(r, &["checkout", "main"]);
        write(r, "fresh.txt", "new\n");
        git(r, &["add", "."]);
        git(r, &["commit", "-m", "fresh"]);
        let fresh = rev_parse(r, "HEAD");
        let root = temp_dir();
        let (wt, _) = mgr(r, &root.path)
            .add_at_ref("from-pr", "from-pr", "feat/oauth", &fresh)
            .await
            .expect("add_at_ref");
        assert_ne!(
            wt.branch, "feat/oauth",
            "a diverged local branch must not be reused"
        );
        assert_eq!(rev_parse(&wt.path, "HEAD"), fresh);
        assert_eq!(
            rev_parse(r, "feat/oauth"),
            stale,
            "diverged commits must stay reachable"
        );
    }

    #[tokio::test]
    async fn add_at_ref_falls_back_when_branch_checked_out() {
        let repo = crate::testutil::init_repo();
        let r = repo.path.as_str();
        git(r, &["checkout", "-b", "feat/oauth"]);
        write(r, "head.txt", "pr\n");
        git(r, &["add", "."]);
        git(r, &["commit", "-m", "pr head"]);
        git(r, &["checkout", "main"]);
        let root = temp_dir();
        let m = mgr(r, &root.path);
        m.add_at_ref("one", "one", "feat/oauth", "feat/oauth")
            .await
            .expect("first");
        let (wt2, _) = m
            .add_at_ref("two", "two", "feat/oauth", "feat/oauth")
            .await
            .expect("second");
        assert_ne!(wt2.branch, "feat/oauth");
    }

    #[tokio::test]
    async fn add_repairs_moved_worktree() {
        let repo = crate::testutil::init_repo();
        let old_root = temp_dir();
        let new_root = temp_dir();
        let (wt, _) = mgr(&repo.path, &old_root.path)
            .add("LOOP-moved", "moved-feat", "main")
            .await
            .expect("add");
        let new_path = format!("{}/LOOP-moved", new_root.path);
        std::fs::rename(&wt.path, &new_path).expect("move worktree");
        let (reused, _) = mgr(&repo.path, &new_root.path)
            .add("LOOP-moved", "moved-feat", "main")
            .await
            .expect("re-add");
        assert_eq!(clean(&reused.path), clean(&new_path));
        assert_eq!(reused.branch, "recur/moved-feat");
    }

    #[tokio::test]
    async fn remove_missing_is_no_op() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        mgr(&repo.path, &root.path)
            .remove(&format!("{}/nope", root.path))
            .await
            .expect("no-op remove");
    }

    #[tokio::test]
    async fn delete_branch_if_at_base_cases() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (wt, _) = m.add("empty-ws", "empty-ws", "main").await.expect("add");
        m.remove(&wt.path).await.expect("remove");
        m.delete_branch_if_at_base(&wt.branch, "main")
            .await
            .expect("delete");
        assert!(
            !m.commit_exists("recur/empty-ws").await,
            "branch at base should be deleted"
        );

        let (wt2, _) = m.add("kept-ws", "kept-ws", "main").await.expect("add kept");
        write(&wt2.path, "work.txt", "x\n");
        m.commit(&wt2.path, "work", "test", "test@example.com")
            .await
            .expect("commit");
        m.remove(&wt2.path).await.expect("remove kept");
        m.delete_branch_if_at_base("recur/kept-ws", "main")
            .await
            .expect("keep");
        assert!(
            m.commit_exists("recur/kept-ws").await,
            "a branch with unique commits must stay"
        );
    }

    #[tokio::test]
    async fn delete_local_branch_cases() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (wt, _) = m.add("del-ws", "del-ws", "main").await.expect("add");
        write(&wt.path, "work.txt", "x\n");
        m.commit(&wt.path, "work", "test", "test@example.com")
            .await
            .expect("commit");
        m.remove(&wt.path).await.expect("remove");
        m.delete_local_branch(&wt.branch).await.expect("delete");
        assert!(!m.commit_exists(&wt.branch).await);
        m.delete_local_branch("no-such-branch")
            .await
            .expect("missing branch is a no-op");
    }

    #[tokio::test]
    async fn remove_wipes_orphan_directory() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let orphan = format!("{}/herbed-ciabatta", root.path);
        write(&orphan, "stale.txt", "old\n");
        mgr(&repo.path, &root.path)
            .remove(&orphan)
            .await
            .expect("remove orphan");
        assert!(!exists(&orphan), "orphan dir still present");
    }

    #[tokio::test]
    async fn remove_refuses_path_outside_managed_root() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let outside = temp_dir();
        write(&outside.path, "keep.txt", "safe\n");
        assert!(
            m.remove(&outside.path).await.is_err(),
            "remove outside the root must fail"
        );
        assert!(
            exists(&format!("{}/keep.txt", outside.path)),
            "remove must not touch outside paths"
        );
        assert!(
            m.remove(&root.path).await.is_err(),
            "removing the managed root itself must fail"
        );
    }

    #[tokio::test]
    async fn remove_locked_worktree() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (wt, _) = m.add("locked-ws", "locked-ws", "main").await.expect("add");
        git(&repo.path, &["worktree", "lock", wt.path.as_str()]);
        m.remove(&wt.path).await.expect("remove locked");
        assert!(!exists(&wt.path));
        assert!(
            matches!(m.worktree_at(&wt.path).await, Ok(None)),
            "locked worktree still registered"
        );
        let (wt2, created) = m
            .add("locked-ws", "locked-ws", "main")
            .await
            .expect("re-add");
        assert!(created, "expected a fresh worktree");
        assert!(exists(&format!("{}/README.md", wt2.path)));
    }

    #[tokio::test]
    async fn add_wipes_orphan_then_creates_fresh() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let orphan = format!("{}/herbed-ciabatta", root.path);
        write(&orphan, "stale.txt", "old\n");
        let (wt, created) = mgr(&repo.path, &root.path)
            .add("herbed-ciabatta", "herbed-ciabatta", "main")
            .await
            .expect("add");
        assert!(created);
        assert!(
            !exists(&format!("{}/stale.txt", wt.path)),
            "leftover must not survive"
        );
        assert!(exists(&format!("{}/README.md", wt.path)));
    }

    #[test]
    fn discard_risk_summary() {
        for (r, want) in [
            (DiscardRisk::default(), ""),
            (
                DiscardRisk {
                    dirty_count: 1,
                    unmerged: false,
                },
                "1 uncommitted change",
            ),
            (
                DiscardRisk {
                    dirty_count: 3,
                    unmerged: false,
                },
                "3 uncommitted changes",
            ),
            (
                DiscardRisk {
                    dirty_count: 0,
                    unmerged: true,
                },
                "unmerged changes",
            ),
            (
                DiscardRisk {
                    dirty_count: 2,
                    unmerged: true,
                },
                "unmerged + dirty",
            ),
        ] {
            assert_eq!(r.summary(), want, "{r:?}");
        }
    }

    #[tokio::test]
    async fn inspect_discard_risk_cases() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let (wt, _) = mgr(&repo.path, &root.path)
            .add("risk-ws", "risk-ws", "main")
            .await
            .expect("add");
        assert_eq!(inspect_discard_risk(&wt.path).await.summary(), "");
        write(&wt.path, "dirty.txt", "x\n");
        let risk = inspect_discard_risk(&wt.path).await;
        assert!(
            risk.dirty_count >= 1 && !risk.summary().is_empty(),
            "{risk:?}"
        );
        assert_eq!(
            inspect_discard_risk(&format!("{}/missing", root.path))
                .await
                .summary(),
            ""
        );
    }

    #[tokio::test]
    async fn commit_stages_and_counts_diff() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (wt, _) = m.add("LOOP-C", "commit-test", "main").await.expect("add");
        let res = m
            .commit(&wt.path, "noop", "Coder", "coder@recur.local")
            .await
            .expect("noop commit");
        assert!(!res.committed, "a clean worktree must not commit: {res:?}");
        write(&wt.path, "new.txt", "a\nb\nc\n");
        let res = m
            .commit(
                &wt.path,
                "recur(Coder): iteration 1 — add file",
                "Coder",
                "coder@recur.local",
            )
            .await
            .expect("commit");
        assert!(res.committed && !res.hash.is_empty(), "{res:?}");
        assert_eq!(res.additions, 3);
        let out = m
            .run_in(&wt.path, ["log", "-1", "--pretty=%an|%s"])
            .await
            .expect("log");
        assert_eq!(out.trim(), "Coder|recur(Coder): iteration 1 — add file");
    }

    #[tokio::test]
    async fn commits_behind_and_ahead() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (wt, _) = m
            .add("LOOP-BEHIND", "feature-behind", "main")
            .await
            .expect("add");
        assert_eq!(m.commits_behind(&wt.path, "main").await.expect("behind"), 0);
        assert_eq!(m.commits_ahead(&wt.path, "main").await.expect("ahead"), 0);
        write(&repo.path, "extra.txt", "base moved\n");
        git(&repo.path, &["add", "extra.txt"]);
        git(&repo.path, &["commit", "-m", "advance main"]);
        assert_eq!(m.commits_behind(&wt.path, "main").await.expect("behind"), 1);
        assert_eq!(m.commits_ahead(&wt.path, "main").await.expect("ahead"), 0);
        write(&wt.path, "feature.txt", "own commit\n");
        assert!(
            m.commit(&wt.path, "add feature", "Coder", "coder@recur.local")
                .await
                .expect("commit")
                .committed
        );
        assert_eq!(m.commits_ahead(&wt.path, "main").await.expect("ahead"), 1);
    }

    // --- merge_test.go ---

    async fn add_branch_commit(
        m: &Manager,
        id: &str,
        branch: &str,
        file: &str,
        content: &str,
    ) -> (Worktree, String) {
        let (wt, _) = m.add(id, branch, "main").await.expect("add");
        write(&wt.path, file, content);
        let res = m
            .commit(
                &wt.path,
                &format!("recur(Coder): {file}"),
                "Coder",
                "coder@recur.local",
            )
            .await
            .expect("commit");
        assert!(res.committed);
        let head = m.ref_hash(&wt.branch).await;
        (wt, head)
    }

    async fn commit_count(m: &Manager, r: &str) -> i64 {
        m.run(["rev-list", "--count", r])
            .await
            .expect("rev-list")
            .trim()
            .parse()
            .expect("count")
    }

    async fn commit_in_repo(m: &Manager, file: &str) {
        let id = ["-c", "user.name=test", "-c", "user.email=test@example.com"];
        m.run(id.iter().copied().chain(["add", file]))
            .await
            .expect("add");
        m.run(id.iter().copied().chain(["commit", "-m", "base moved"]))
            .await
            .expect("commit");
    }

    #[tokio::test]
    async fn merge_commit_preserves_topology() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let base_before = m.ref_hash("main").await;
        let (wt, branch_head) =
            add_branch_commit(&m, "LOOP-mc", "merge-feature", "mc.txt", "merge commit\n").await;
        let mr = m
            .merge(&wt.branch, "main", &format!("Merge {}", wt.branch), "merge")
            .await
            .expect("merge");
        let base_after = m.ref_hash("main").await;
        assert_ne!(base_after, base_before, "base ref did not advance");
        assert_eq!(mr.new_head, base_after);
        assert!(!mr.fast_forward);
        assert_ne!(
            base_after, branch_head,
            "--no-ff merge commit should differ from the branch head"
        );
        let author = m
            .run(["log", "-1", "--format=%an <%ae>"])
            .await
            .expect("log");
        assert_eq!(
            author.trim(),
            "test <test@example.com>",
            "merge author is the user's git config"
        );
    }

    #[tokio::test]
    async fn squash_merge_lands_one_commit() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let before = commit_count(&m, "main").await;
        let (wt, _) =
            add_branch_commit(&m, "LOOP-sq", "squash-feature", "sq.txt", "squash\n").await;
        write(&wt.path, "sq2.txt", "second\n");
        m.commit(
            &wt.path,
            "recur(Coder): sq2.txt",
            "Coder",
            "coder@recur.local",
        )
        .await
        .expect("second commit");
        let mr = m
            .merge(
                &wt.branch,
                "main",
                &format!("Squash {}", wt.branch),
                "squash",
            )
            .await
            .expect("squash");
        assert!(!mr.new_head.is_empty());
        assert_eq!(commit_count(&m, "main").await, before + 1);
    }

    #[tokio::test]
    async fn squash_merge_dirty_base_checkout() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (wt, _) = add_branch_commit(&m, "LOOP-sqd", "squash-dirty", "feat.txt", "feat\n").await;
        write(&wt.path, "README.md", "from branch\n");
        m.commit(
            &wt.path,
            "recur(Coder): README.md",
            "Coder",
            "coder@recur.local",
        )
        .await
        .expect("commit");
        write(&repo.path, "README.md", "local dirty\n");
        let err = m
            .merge(
                &wt.branch,
                "main",
                &format!("Squash {}", wt.branch),
                "squash",
            )
            .await
            .expect_err("dirty base");
        let dirty = err.dirty_base().expect("want DirtyBaseError");
        assert_eq!(dirty.base, "main");
        assert!(
            dirty.files.contains(&"README.md".to_string()),
            "{:?}",
            dirty.files
        );
        let msg = err.to_string();
        assert!(
            !msg.contains("git merge --squash") && !msg.contains("exit status"),
            "{msg}"
        );
        assert!(
            msg.contains("project folder") && msg.contains("README.md"),
            "{msg}"
        );
        let got = std::fs::read_to_string(format!("{}/README.md", repo.path)).expect("read");
        assert_eq!(got.trim(), "local dirty");
    }

    #[tokio::test]
    async fn rebase_merge_dirty_base_checkout() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (wt, _) = add_branch_commit(&m, "LOOP-rbd", "rebase-dirty", "feat.txt", "feat\n").await;
        write(&wt.path, "README.md", "from branch\n");
        m.commit(
            &wt.path,
            "recur(Coder): README.md",
            "Coder",
            "coder@recur.local",
        )
        .await
        .expect("commit");
        write(&repo.path, "README.md", "local dirty\n");
        let base_before = m.ref_hash("main").await;
        let err = m
            .merge(&wt.branch, "main", "", "rebase")
            .await
            .expect_err("dirty base");
        let dirty = err.dirty_base().expect("want DirtyBaseError");
        assert!(
            dirty.files.contains(&"README.md".to_string()),
            "{:?}",
            dirty.files
        );
        let got = std::fs::read_to_string(format!("{}/README.md", repo.path)).expect("read");
        assert_eq!(
            got.trim(),
            "local dirty",
            "uncommitted work must be preserved"
        );
        assert_eq!(
            m.ref_hash("main").await,
            base_before,
            "a dirty refusal must not move main"
        );
    }

    #[tokio::test]
    async fn rebase_merge_conflict_preserves_unrelated_dirty() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        write(&repo.path, "notes.txt", "committed notes\n");
        m.run(["add", "notes.txt"]).await.expect("add");
        m.run(["commit", "-m", "notes"]).await.expect("commit");
        let (wt, _) =
            add_branch_commit(&m, "LOOP-rbc", "rebase-conflict", "feat.txt", "feat\n").await;
        write(&wt.path, "README.md", "from branch\n");
        m.commit(
            &wt.path,
            "recur(Coder): README.md",
            "Coder",
            "coder@recur.local",
        )
        .await
        .expect("commit");
        write(&repo.path, "README.md", "from main\n");
        m.run(["add", "README.md"]).await.expect("add");
        m.run(["commit", "-m", "main README"])
            .await
            .expect("commit");
        let base_before = m.ref_hash("main").await;
        write(&repo.path, "notes.txt", "UNRELATED DIRTY WORK\n");
        let err = m
            .merge(&wt.branch, "main", "", "rebase")
            .await
            .expect_err("conflict");
        assert!(
            err.dirty_base().is_none(),
            "want a conflict, not DirtyBaseError: {err}"
        );
        assert_eq!(m.ref_hash("main").await, base_before, "conflict moved main");
        let got = std::fs::read_to_string(format!("{}/notes.txt", repo.path)).expect("read");
        assert_eq!(
            got.trim(),
            "UNRELATED DIRTY WORK",
            "unrelated dirty work must be preserved"
        );
    }

    #[test]
    fn dirty_base_error_text() {
        let err = DirtyBaseError {
            base: "main".into(),
            files: vec!["frontend/bindings/foo.ts".into()],
        };
        let got = err.to_string();
        assert!(
            got.contains("project folder") && got.contains("main") && got.contains("foo.ts"),
            "{got}"
        );
        assert!(!got.contains("git merge"), "{got}");
    }

    #[test]
    fn overwritten_paths_cases() {
        let tracked = "error: Your local changes to the following files would be overwritten by merge:\n\tfrontend/bindings/recur/internal/app/workspacesservice.ts\nPlease commit your changes or stash them before you merge.\nAborting\nMerge with strategy ort failed.";
        assert_eq!(
            overwritten_paths(tracked),
            vec!["frontend/bindings/recur/internal/app/workspacesservice.ts"]
        );
        let untracked = "error: The following untracked working tree files would be overwritten by merge:\n\tuntracked.txt\nPlease move or remove them before you merge.\nAborting";
        assert_eq!(overwritten_paths(untracked), vec!["untracked.txt"]);
    }

    #[tokio::test]
    async fn update_worktree_with_base_merge() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (wt, _) =
            add_branch_commit(&m, "LOOP-ubm", "update-merge", "feat.txt", "feature\n").await;
        write(&repo.path, "base.txt", "from base\n");
        commit_in_repo(&m, "base.txt").await;
        let base_head = m.ref_hash("main").await;
        m.update_worktree_with_base(&wt.path, "main", "merge")
            .await
            .expect("update");
        m.run_in(
            &wt.path,
            ["merge-base", "--is-ancestor", base_head.as_str(), "HEAD"],
        )
        .await
        .expect("base tip should be an ancestor after merge");
        assert_eq!(m.commits_behind(&wt.path, "main").await.expect("behind"), 0);
        let author = m
            .run_in(&wt.path, ["log", "-1", "--format=%an <%ae>"])
            .await
            .expect("log");
        assert_eq!(author.trim(), "test <test@example.com>");
    }

    #[tokio::test]
    async fn update_worktree_with_base_rebase() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (wt, _) =
            add_branch_commit(&m, "LOOP-ubr", "update-rebase", "feat.txt", "feature\n").await;
        write(&repo.path, "base.txt", "from base\n");
        commit_in_repo(&m, "base.txt").await;
        let base_head = m.ref_hash("main").await;
        m.update_worktree_with_base(&wt.path, "main", "rebase")
            .await
            .expect("update");
        m.run_in(
            &wt.path,
            ["merge-base", "--is-ancestor", base_head.as_str(), "HEAD"],
        )
        .await
        .expect("base tip should be an ancestor after rebase");
        let parents = m
            .run_in(&wt.path, ["rev-list", "--parents", "-n", "1", "HEAD"])
            .await
            .expect("rev-list");
        assert!(
            parents.split_whitespace().count() <= 2,
            "expected linear history, got {parents}"
        );
    }

    #[tokio::test]
    async fn update_worktree_with_base_conflict_aborts_clean() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (wt, _) = add_branch_commit(
            &m,
            "LOOP-ubc",
            "update-conflict",
            "same.txt",
            "branch side\n",
        )
        .await;
        write(&repo.path, "same.txt", "base side\n");
        commit_in_repo(&m, "same.txt").await;
        assert!(
            m.update_worktree_with_base(&wt.path, "main", "merge")
                .await
                .is_err(),
            "expected a conflict"
        );
        assert!(
            m.is_clean(&wt.path).await.expect("is_clean"),
            "worktree should be clean after the abort"
        );
    }

    #[tokio::test]
    async fn rebase_merge_is_linear() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let before = commit_count(&m, "main").await;
        let (wt, _) =
            add_branch_commit(&m, "LOOP-rb", "rebase-feature", "rb.txt", "rebase\n").await;
        let mr = m
            .merge(&wt.branch, "main", "", "rebase")
            .await
            .expect("rebase merge");
        assert!(mr.fast_forward);
        assert_eq!(commit_count(&m, "main").await, before + 1);
    }

    #[tokio::test]
    async fn merge_into_worktree() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (parent, _) = m
            .add("LOOP-parent", "parent", "main")
            .await
            .expect("add parent");
        let (child_a, _) = m
            .add("LOOP-childa", "child-a", &parent.branch)
            .await
            .expect("add child A");
        write(&child_a.path, "a.txt", "from a\n");
        m.commit(&child_a.path, "step A", "test", "test@example.com")
            .await
            .expect("commit A");
        let conflicts = m
            .merge_into(
                &parent.path,
                &child_a.branch,
                "Merge step A",
                "test",
                "test@example.com",
            )
            .await
            .expect("merge A");
        assert!(conflicts.is_empty(), "{conflicts:?}");
        assert!(exists(&format!("{}/a.txt", parent.path)));
        let subject = m
            .run_in(&parent.path, ["log", "-1", "--format=%s"])
            .await
            .unwrap_or_default();
        assert_eq!(subject.trim(), "Merge step A");

        let (child_b, _) = m
            .add("LOOP-childb", "child-b", &parent.branch)
            .await
            .expect("add child B");
        write(&child_b.path, "README.md", "child B says hi\n");
        m.commit(&child_b.path, "step B", "test", "test@example.com")
            .await
            .expect("commit B");
        write(&parent.path, "README.md", "parent says hi\n");
        m.commit(&parent.path, "parent edit", "test", "test@example.com")
            .await
            .expect("commit parent");
        let conflicts = m
            .merge_into(
                &parent.path,
                &child_b.branch,
                "Merge step B",
                "test",
                "test@example.com",
            )
            .await
            .expect("merge B");
        assert_eq!(conflicts, vec!["README.md"]);
        assert!(
            m.merge_head(&parent.path).await,
            "merge should be left in progress"
        );
        write(&parent.path, "README.md", "both say hi\n");
        let res = m
            .commit(&parent.path, "resolve", "test", "test@example.com")
            .await
            .expect("resolve");
        assert!(res.committed);
        assert!(
            !m.merge_head(&parent.path).await,
            "MERGE_HEAD should be gone after resolving"
        );
        let parents = m
            .run_in(&parent.path, ["log", "-1", "--format=%P"])
            .await
            .unwrap_or_default();
        assert_eq!(
            parents.split_whitespace().count(),
            2,
            "resolving commit should be a merge: {parents}"
        );
    }

    #[tokio::test]
    async fn commit_concludes_ours_only_merge() {
        let repo = crate::testutil::init_repo();
        let root = temp_dir();
        let m = mgr(&repo.path, &root.path);
        let (parent, _) = m
            .add("LOOP-ours", "parent-ours", "main")
            .await
            .expect("add parent");
        let (child, _) = m
            .add("LOOP-ourschild", "child-ours", &parent.branch)
            .await
            .expect("add child");
        write(&child.path, "README.md", "child says hi\n");
        m.commit(&child.path, "child edit", "test", "test@example.com")
            .await
            .expect("commit child");
        write(&parent.path, "README.md", "parent says hi\n");
        m.commit(&parent.path, "parent edit", "test", "test@example.com")
            .await
            .expect("commit parent");
        let conflicts = m
            .merge_into(
                &parent.path,
                &child.branch,
                "Merge child",
                "test",
                "test@example.com",
            )
            .await
            .expect("merge");
        assert_eq!(conflicts, vec!["README.md"]);
        write(&parent.path, "README.md", "parent says hi\n");
        let res = m
            .commit(&parent.path, "keep ours", "test", "test@example.com")
            .await
            .expect("commit");
        assert!(
            res.committed,
            "an ours-only resolution must still commit the merge"
        );
        assert!(!m.merge_head(&parent.path).await);
        let parents = m
            .run_in(&parent.path, ["log", "-1", "--format=%P"])
            .await
            .unwrap_or_default();
        assert_eq!(parents.split_whitespace().count(), 2, "{parents}");
    }

    #[test]
    fn parse_porcelain_worktrees() {
        let out = "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\nworktree /wt/a\nHEAD def\ndetached\n\n";
        let list = parse_porcelain(out);
        assert_eq!(
            list,
            vec![
                Worktree {
                    path: "/repo".into(),
                    branch: "main".into(),
                    head: "abc".into()
                },
                Worktree {
                    path: "/wt/a".into(),
                    branch: String::new(),
                    head: "def".into()
                },
            ]
        );
    }
}
