//! The engine's calls into a project's or a PR's forge (Go `github.go`
//! engine glue: `CreateGitHubPR`'s base mapping, `pushGitHubHead`,
//! `githubPRChecks`, PR head fetches and the branch-rules cache).

use std::path::Path;
use std::time::Duration;

use recur_domain::{Check, Pr, PrSource, Project};
use recur_forge::{CreatePr, Forge, HeadRef};
use tokio::time::Instant;

use crate::Engine;
use crate::bus::{BranchRules, EngineEvent};

/// Bounds the check rollup read at PR open.
const PR_CHECKS_TIMEOUT: Duration = Duration::from_secs(15);
/// Bounds a background branch-rules refresh.
const BRANCH_RULES_REFRESH_TIMEOUT: Duration = Duration::from_secs(10);

/// A PR the forge opened (`create_forge_pr`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForgePrCreated {
    pub source: PrSource,
    pub number: i64,
    pub pr_id: String,
}

/// The remote-tracking ref an open forge PR's head is compared against:
/// `origin/<head>` for a same-repo head (the frozen `Pr.branch`, not the
/// local branch of the same name), the forge's local PR ref for a fork.
/// Returns (ref, remote, short); empty when the PR has no remote branch.
pub(crate) fn pr_tracking_ref(pr: &Pr, forge: &dyn Forge) -> (String, String, String) {
    if pr.is_cross_repository {
        return (
            forge.head_ref(pr.number).local_ref,
            String::new(),
            String::new(),
        );
    }
    let short = pr.branch.trim();
    let short = short.strip_prefix("origin/").unwrap_or(short);
    if short.is_empty() {
        return Default::default();
    }
    (format!("origin/{short}"), "origin".into(), short.to_owned())
}

/// Fetches a forge PR head (same-repo and fork PRs) into its local ref and
/// returns that ref.
pub(crate) async fn fetch_forge_pr_head(
    mgr: &recur_git::Manager,
    repo: &str,
    head: &HeadRef,
    number: i64,
) -> Result<String, String> {
    if number <= 0 {
        return Err(format!("fetch PR head: invalid number {number}"));
    }
    let mut remote = crate::prs::default_remote(mgr).await;
    if remote.is_empty() {
        remote = "origin".into();
    }
    let src = format!("+{}:{}", head.remote_ref, head.local_ref);
    recur_git::command(["fetch", "--end-of-options", remote.as_str(), src.as_str()])
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .map_err(|e| format!("git fetch {remote} {}: {e}", head.remote_ref))?;
    if !mgr.commit_exists(&head.local_ref).await {
        return Err(format!(
            "git fetch {remote} {}: missing {}",
            head.remote_ref, head.local_ref
        ));
    }
    Ok(head.local_ref.clone())
}

/// Refreshes the PR's tracking ref from the forge (best effort).
pub(crate) async fn fetch_pr_tracking_head(
    mgr: &recur_git::Manager,
    repo: &str,
    pr: &Pr,
    forge: &dyn Forge,
) {
    if pr.is_cross_repository {
        let _ = fetch_forge_pr_head(mgr, repo, &forge.head_ref(pr.number), pr.number).await;
        return;
    }
    let (_, remote, short) = pr_tracking_ref(pr, forge);
    if !short.is_empty() {
        let _ = mgr.fetch_remote_branch(&remote, &short).await;
    }
}

impl Engine {
    /// The branch name a forge's create/edit `--base` accepts: a stored
    /// local compare ref (`origin/main`) loses its remote prefix.
    pub(crate) async fn forge_pr_base(&self, repo_path: &str, base: &str) -> String {
        let base = base.trim();
        if base.is_empty() || repo_path.is_empty() {
            return base.to_owned();
        }
        recur_git::Manager::new(repo_path, "")
            .short_branch_name(base)
            .await
    }

    /// Pushes the branch and opens the PR on the project's forge. `None`
    /// when no forge is registered (the caller keeps a Local PR);
    /// `Err((forge label, reason))` is the fallback note.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_forge_pr(
        &self,
        proj: &Project,
        worktree: &str,
        branch: &str,
        base: &str,
        title: &str,
        body: &str,
        draft: bool,
    ) -> Option<Result<ForgePrCreated, (String, String)>> {
        let forge = self.project_forge(&proj.repo_path).await?;
        let base = self.forge_pr_base(&proj.repo_path, base).await;
        let res = forge
            .create_pr(CreatePr {
                repo: Path::new(&proj.repo_path),
                worktree: Path::new(worktree),
                branch,
                base: &base,
                title,
                body,
                draft,
            })
            .await;
        Some(match res {
            Ok(n) => Ok(ForgePrCreated {
                source: forge.pr_source(),
                number: n,
                pr_id: forge.imported_pr_id(&proj.id, n),
            }),
            Err(reason) => Err((forge.label().to_owned(), reason)),
        })
    }

    /// Pushes HEAD onto an open forge PR's remote head (forks included).
    /// `force` is `--force-with-lease`. `Err((forge label, reason))`.
    pub(crate) async fn push_forge_pr_head(
        &self,
        worktree: &str,
        repo_path: &str,
        pr: &Pr,
        head: &str,
        force: bool,
    ) -> Result<(), (String, String)> {
        let Some(forge) = self.pr_forge(pr) else {
            return Err((
                String::new(),
                format!("no forge handles {} PRs", pr.source.as_str()),
            ));
        };
        forge
            .push_head(Path::new(worktree), Path::new(repo_path), pr, head, force)
            .await
            .map_err(|e| (forge.label().to_owned(), e.message))
    }

    /// The forge's check rollup at PR open. A fetch error is an empty
    /// rollup that counts as passing (unknown is not a failure).
    pub(crate) async fn forge_pr_checks(
        &self,
        source: &PrSource,
        repo_path: &str,
        number: i64,
    ) -> (Vec<Check>, bool) {
        let Some(forge) = self.forges.for_pr_source(source) else {
            return (Vec::new(), true);
        };
        match tokio::time::timeout(
            PR_CHECKS_TIMEOUT,
            forge.checks(Path::new(repo_path), number, ""),
        )
        .await
        {
            Ok(Ok(r)) => (r.checks, r.ok),
            _ => (Vec::new(), true),
        }
    }

    /// Refreshes an open forge PR's head ref so the titlebar can compare.
    pub(crate) async fn fetch_open_pr_remote(&self, ws: &recur_domain::Workspace) {
        let Some(pr) = self.store.pr_by_workspace(&ws.id) else {
            return;
        };
        if pr.state == "Merged" || pr.group == "merged" {
            return;
        }
        let Some(forge) = self.pr_forge(&pr) else {
            return;
        };
        let Some(proj) = self.store.project(&ws.project_id) else {
            return;
        };
        if proj.repo_path.is_empty() {
            return;
        }
        let mgr = recur_git::Manager::new(&proj.repo_path, &self.worktree_root);
        let _ = tokio::time::timeout(
            recur_git::REMOTE_FETCH_TIMEOUT,
            fetch_pr_tracking_head(&mgr, &proj.repo_path, &pr, forge.as_ref()),
        )
        .await;
    }

    /// Whether `remote/branch` requires a pull request on the project's
    /// forge. Answers from the stored probe (refreshing it in the
    /// background once stale), else probes now. Anything unknown fails
    /// closed (true).
    pub async fn branch_requires_pr(&self, proj: &Project, remote: &str, branch: &str) -> bool {
        let (branch, remote) = (branch.trim(), remote.trim());
        if proj.id.is_empty() || proj.repo_path.is_empty() || remote.is_empty() || branch.is_empty()
        {
            return true;
        }
        let Some(forge) = self.project_forge(&proj.repo_path).await else {
            return true;
        };
        let owner_repo = match forge.remote_repo(Path::new(&proj.repo_path), remote).await {
            Ok(r) if !r.is_empty() => r,
            _ => return true,
        };
        if let Some((cached, at)) =
            self.store
                .github_branch_require_pr(&proj.id, &owner_repo, branch)
        {
            let ttl = self.prs.timing.lock().branch_rules_ttl;
            let age = chrono::Utc::now().signed_duration_since(at);
            if age.to_std().unwrap_or_default() >= ttl {
                self.schedule_branch_rules_refresh(forge, proj, &owner_repo, remote, branch);
            }
            return cached;
        }
        match forge
            .branch_requires_pr(Path::new(&proj.repo_path), &owner_repo, branch)
            .await
        {
            Some(require) => {
                self.store_branch_requires_pr(
                    &proj.id,
                    &owner_repo,
                    remote,
                    branch,
                    require,
                    false,
                );
                require
            }
            None => true,
        }
    }

    fn schedule_branch_rules_refresh(
        &self,
        forge: std::sync::Arc<dyn Forge>,
        proj: &Project,
        owner_repo: &str,
        remote: &str,
        branch: &str,
    ) {
        let key = format!("{}\x00{owner_repo}\x00{branch}", proj.id);
        if !self.prs.branch_rules_inflight.lock().insert(key.clone()) {
            return;
        }
        let e = self.arc();
        let (repo, project_id) = (proj.repo_path.clone(), proj.id.clone());
        let (owner_repo, remote, branch) =
            (owner_repo.to_owned(), remote.to_owned(), branch.to_owned());
        self.tasks.teardown.spawn(async move {
            let probed = tokio::time::timeout_at(
                Instant::now() + BRANCH_RULES_REFRESH_TIMEOUT,
                forge.branch_requires_pr(Path::new(&repo), &owner_repo, &branch),
            )
            .await
            .ok()
            .flatten();
            if let Some(require) = probed {
                e.store_branch_requires_pr(
                    &project_id,
                    &owner_repo,
                    &remote,
                    &branch,
                    require,
                    true,
                );
            }
            e.prs.branch_rules_inflight.lock().remove(&key);
        });
    }

    /// Writes the probe. `notify` is a background refresh whose answer
    /// changed: land calls already returned the first probe.
    fn store_branch_requires_pr(
        &self,
        project_id: &str,
        owner_repo: &str,
        remote: &str,
        branch: &str,
        require: bool,
        notify: bool,
    ) {
        let prev = self
            .store
            .github_branch_require_pr(project_id, owner_repo, branch);
        self.store
            .set_github_branch_require_pr(project_id, owner_repo, branch, require);
        let Some((prev, _)) = prev else {
            return;
        };
        if !notify || prev == require {
            return;
        }
        self.bus.publish(EngineEvent::BranchRules(BranchRules {
            branch: branch.to_owned(),
            project_id: project_id.to_owned(),
            remote: remote.to_owned(),
            repo: owner_repo.to_owned(),
            require_pr: require,
        }));
    }
}
