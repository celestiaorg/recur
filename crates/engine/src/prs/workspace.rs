//! Hand-driven landing from a workspace: merge without a PR, the titlebar's
//! land state, update with base, pull the open PR's remote head, and open or
//! sync the workspace's PR (Local, or on the project's forge when the project
//! opts in).

use std::time::Duration;

use recur_domain::{
    Commit, DiffStat, InboxItem, MergeMethod, Pr, PrSource, Project, SquashCommitMessage,
    SummaryPara, UPDATE_WORKSPACE_CONFLICT, UPDATE_WORKSPACE_NOOP, UPDATE_WORKSPACE_UPDATED,
    UpdateBaseMethod, UpdateWorkspaceResult, Workspace, WorkspaceLandState, WorkspaceState,
    effective_merge_method, effective_squash_commit_message, effective_update_base_method,
    normalize_merge_method, normalize_update_base_method, run_active,
};
use recur_forge::model::{commit_subjects, pr_description, single_line};

use crate::Engine;
use crate::core::{EngineError, Result};
use crate::loops::open_pr::{
    engine_focus, pr_fallback_intro, pr_fallback_markdown, pr_title, upsert_description_comment,
};
use crate::prs::forge_glue::{fetch_pr_tracking_head, pr_tracking_ref};
use crate::prs::merge::format_squash_commit_message;
use crate::prs::refresh::behind_base;
use crate::prs::{is_forge_source, none_if_empty, touch_pr_updated};
use crate::text::{hard_truncate_runes, html_escape, plural, truncate};
use crate::turn::oneshot::cap_github_pr_body;
use crate::workspace::create::DEFAULT_WORKSPACE_TITLE;

const LAND_TIMEOUT: Duration = Duration::from_secs(60);
/// Time past a land's deadline for [`recur_git::Manager::push_land`] to undo
/// a timed-out push, so the deadline never strands an unpushed land.
pub(crate) const PUSH_UNDO_GRACE: Duration = Duration::from_secs(10);
const LAND_STATE_TIMEOUT: Duration = Duration::from_secs(10);

/// While the autopilot owns the worktree. Once a loop idles the human may
/// merge, update or open a PR like on a hand-driven workspace.
pub fn land_locked_by_run(ws: &Workspace) -> bool {
    ws.has_run() && run_active(&ws.state)
}

/// The on-disk commit message for a no-PR land: the workspace name as the
/// squash subject (plus commit details by the squash setting), else
/// `Merge <name> into <base>`.
pub fn workspace_merge_commit_message(
    ws: &Workspace,
    base: &str,
    method: &MergeMethod,
    commits: &[Commit],
    squash: &SquashCommitMessage,
) -> String {
    let mut title = single_line(&ws.goal);
    if title.is_empty() || title == DEFAULT_WORKSPACE_TITLE {
        title = "Workspace changes".into();
    }
    if *method == MergeMethod::Squash {
        return format_squash_commit_message(&title, 0, false, commits, "", squash).combined();
    }
    format!("Merge {title} into {base}")
}

/// A hand-driven workspace PR's summary: what changed, no convergence
/// claims.
pub fn workspace_pr_summary(
    name: &str,
    file_count: i64,
    add: i64,
    del: i64,
    commit_count: i64,
) -> Vec<SummaryPara> {
    let name = name.trim();
    let label = if name.is_empty() {
        "this workspace".to_owned()
    } else {
        html_escape(name)
    };
    vec![
        SummaryPara {
            kind: "p".into(),
            html: format!("Changes from the <b>{label}</b> workspace."),
            ..SummaryPara::default()
        },
        SummaryPara {
            kind: "bullets".into(),
            items: vec![
                format!(
                    "{} across {}.",
                    plural(commit_count, "commit", "commits"),
                    plural(file_count, "file", "files")
                ),
                format!("Net diff <code>+{add} −{del}</code> versus the base branch."),
            ],
            ..SummaryPara::default()
        },
    ]
}

fn workspace_base(ws: &Workspace, proj: &Project) -> String {
    [ws.base_branch.trim(), proj.base_branch.trim()]
        .into_iter()
        .find(|b| !b.is_empty())
        .unwrap_or("main")
        .to_owned()
}

fn deadline_err() -> EngineError {
    EngineError::new("context deadline exceeded")
}

/// Each commit's own diff for the Commits tab (best effort).
async fn fill_commit_files(
    mgr: &recur_git::Manager,
    worktree: &str,
    commits: &mut [Commit],
    when_now: bool,
) {
    for c in commits.iter_mut() {
        if let Ok(cf) = mgr.commit_diff(worktree, &c.hash).await {
            c.files = Some(cf);
        }
        if when_now || c.when.is_empty() {
            c.when = "now".into();
        }
    }
}

impl Engine {
    /// The name the user sees for a PR source ("Local" or the forge's).
    pub(crate) fn pr_source_label(&self, source: &PrSource) -> String {
        match self.forges.for_pr_source(source) {
            Some(f) if is_forge_source(source) => f.label().to_owned(),
            _ => "Local".into(),
        }
    }

    /// A project's forge name for copy, "forge" when none is registered.
    async fn forge_word(&self, repo_path: &str) -> String {
        let l = self.project_forge_label(repo_path).await;
        if l.is_empty() { "forge".into() } else { l }
    }

    /// Lands a hand-driven workspace onto its merge target. An open forge
    /// PR is refused (merge from the PR), as is a base the forge requires a
    /// PR for. A remote-tracking base (including `main` upgraded to
    /// `origin/main`) is pushed; a local base stays on disk. Idempotent once
    /// merged.
    pub async fn merge_workspace(
        &self,
        workspace_id: &str,
        method: &MergeMethod,
    ) -> Result<Workspace> {
        let ws = self
            .store
            .workspace(workspace_id)
            .ok_or_else(|| EngineError(format!("workspace {workspace_id:?} not found")))?;
        if ws.is_main || ws.is_pr_chat_host() {
            return Err(
                "the main working copy can't merge: its checkout is already the project".into(),
            );
        }
        if land_locked_by_run(&ws) {
            return Err("a workflow is running: pause it before merging from the workspace".into());
        }
        if ws.state == WorkspaceState::Merged {
            return Ok(ws);
        }
        let proj = self
            .store
            .project(&ws.project_id)
            .filter(|p| !p.repo_path.is_empty())
            .ok_or_else(|| {
                EngineError(format!("project for workspace {workspace_id:?} not found"))
            })?;
        let method = if method.is_empty() {
            effective_merge_method(&proj, &self.settings())
        } else {
            normalize_merge_method(method.as_str())
        };
        if let Some(existing) = self.store.pr_by_workspace(workspace_id)
            && let Some(forge) = self.pr_forge(&existing)
            && existing.state != "Merged"
            && existing.state != "Closed"
        {
            return Err(EngineError(format!(
                "this workspace has a {} PR: merge from the PR",
                forge.label()
            )));
        }
        let mut base = workspace_base(&ws, &proj);
        if base.is_empty() {
            base = "main".into();
        }
        let mut branch = self.sync_workspace_branch(workspace_id).await;
        if branch.is_empty() {
            branch = ws.branch.clone();
        }
        let ws = self.store.workspace(workspace_id).unwrap_or(ws);
        let deadline = tokio::time::Instant::now() + LAND_TIMEOUT;
        tokio::time::timeout_at(
            deadline + PUSH_UNDO_GRACE,
            self.merge_workspace_inner(&ws, &proj, &branch, &base, &method, deadline),
        )
        .await
        .unwrap_or_else(|_| Err(deadline_err()))
    }

    async fn merge_workspace_inner(
        &self,
        ws: &Workspace,
        proj: &Project,
        branch: &str,
        base: &str,
        method: &MergeMethod,
        deadline: tokio::time::Instant,
    ) -> Result<Workspace> {
        let workspace_id = ws.id.as_str();
        let mgr = recur_git::Manager::new(&proj.repo_path, &self.worktree_root);
        let risk = recur_git::inspect_discard_risk(&ws.worktree_path).await;
        if risk.dirty_count > 0 || risk.unmerged {
            return Err("commit or discard uncommitted changes first: they would not be included in the merge".into());
        }
        if let Ok(behind) = mgr.commits_behind(&ws.worktree_path, base).await
            && behind > 0
        {
            return Err(EngineError(format!(
                "this branch is {behind} commits behind {base}: update with base first"
            )));
        }
        // As in land state: `main` upgrades to `origin/main` when that
        // tracking ref exists, so the Merge tooltip's "(push)" is real.
        let compare = mgr.compare_ref(base).await;
        let tracking = mgr.parse_remote_tracking(&compare).await;
        let tracking = match tracking {
            Some((remote, short)) => {
                let short = if short.is_empty() {
                    mgr.short_branch_name(&compare).await
                } else {
                    short
                };
                if proj.pr_on_github && self.branch_requires_pr(proj, &remote, &short).await {
                    let label = self.forge_word(&proj.repo_path).await;
                    return Err(EngineError(format!(
                        "{remote}/{short} requires a {label} pull request: Open PR instead of merging here"
                    )));
                }
                Some((remote, short))
            }
            None => None,
        };
        let diff = mgr
            .diff_pr(&ws.worktree_path, base)
            .await
            .map_err(|e| EngineError(format!("compute diff: {e}")))?;
        let commits = mgr
            .log_commits(&ws.worktree_path, base)
            .await
            .unwrap_or_default();
        if diff.files.is_empty() && commits.is_empty() {
            return Err("nothing to merge: commit changes in this workspace first".into());
        }
        let sq = effective_squash_commit_message(proj, &self.settings());
        let msg = workspace_merge_commit_message(ws, base, method, &commits, &sq);
        let res = match mgr.merge(branch, base, &msg, method.as_str()).await {
            Ok(r) => r,
            Err(e) => {
                self.emit_workspace_event(
                    workspace_id,
                    "AR",
                    "You",
                    &format!("Local merge failed: {}", html_escape(&e.to_string())),
                );
                if e.dirty_base().is_some() {
                    return Err(EngineError(e.to_string()));
                }
                return Err(EngineError(format!("merge into {base}: {e}")));
            }
        };
        if let Some((remote, short)) = &tracking
            && let Err(e) = mgr.push_land(remote, short, &res, deadline).await
        {
            self.emit_workspace_event(
                workspace_id,
                "AR",
                "You",
                &format!(
                    "Could not push <code>{}</code> to <code>{}</code>: {}",
                    html_escape(short),
                    html_escape(remote),
                    html_escape(&e.to_string())
                ),
            );
            return Err(EngineError(format!("push {short} to {remote}: {e}")));
        }
        match self.store.pr_by_workspace(workspace_id) {
            Some(existing) if existing.state != "Merged" => {
                self.mark_merged(&existing.id, "").await;
            }
            _ => self.set_workspace_state(workspace_id, WorkspaceState::Merged, "merged"),
        }
        let mut landed = mgr.short_branch_name(base).await;
        if landed.is_empty() {
            landed = base.to_owned();
        }
        let label = html_escape(method.label());
        let note = match &tracking {
            Some((remote, short)) => format!(
                "{label} to <code>{}/{}</code>.",
                html_escape(remote),
                html_escape(short)
            ),
            None => format!(
                "{label} into <code>{}</code> on disk: no PR.",
                html_escape(&landed)
            ),
        };
        self.emit_workspace_event(workspace_id, "AR", "You", &note);
        self.store
            .workspace(workspace_id)
            .ok_or_else(|| EngineError(format!("workspace {workspace_id:?} not found after merge")))
    }

    /// Whether a hand-driven workspace can merge or should commit or update
    /// first; the main copy reports dirty plus ahead/behind its upstream.
    /// Zeros when the worktree is missing.
    pub async fn workspace_land_state(&self, workspace_id: &str) -> WorkspaceLandState {
        let Some(ws) = self
            .store
            .workspace(workspace_id)
            .filter(|w| !w.worktree_path.is_empty())
        else {
            return WorkspaceLandState::default();
        };
        if ws.is_main {
            return self.main_working_copy_land_state(&ws).await;
        }
        let mut base = ws.base_branch.trim().to_owned();
        if base.is_empty()
            && let Some(p) = self.store.project(&ws.project_id)
        {
            base = p.base_branch.trim().to_owned();
        }
        let mut st = WorkspaceLandState {
            base: base.clone(),
            behind_base: ws.behind_base,
            ..WorkspaceLandState::default()
        };
        let proj = self.store.project(&ws.project_id);
        let probe = async {
            let risk = recur_git::inspect_discard_risk(&ws.worktree_path).await;
            st.unmerged = risk.unmerged;
            st.dirty = risk.dirty_count > 0 && !risk.unmerged;
            let mut mgr = None;
            let mut require = None;
            if let Some(proj) = proj
                .as_ref()
                .filter(|p| !p.repo_path.is_empty() && !base.is_empty())
            {
                let m = recur_git::Manager::new(&proj.repo_path, &self.worktree_root);
                let compare = m.compare_ref(&base).await;
                if let Some((remote, short)) = m.parse_remote_tracking(&compare).await {
                    st.remote = remote.clone();
                    let short = if short.is_empty() {
                        m.short_branch_name(&compare).await
                    } else {
                        short
                    };
                    if proj.pr_on_github {
                        require = Some((remote, short));
                    }
                }
                if let Ok((_, n)) = m.behind_base(&ws.worktree_path, &base, false).await {
                    st.behind_base = n;
                }
                if let Ok(n) = m.commits_ahead(&ws.worktree_path, &compare).await {
                    st.ahead_base = n;
                }
                mgr = Some(m);
            }
            self.attach_open_pr_land(&mut st, &ws.id, &ws.worktree_path, mgr.as_ref())
                .await;
            require
        };
        let require = tokio::time::timeout(LAND_STATE_TIMEOUT, probe)
            .await
            .ok()
            .flatten();
        if let Some((remote, short)) = require
            && let Some(proj) = self.store.project(&ws.project_id)
        {
            st.require_pr = self.branch_requires_pr(&proj, &remote, &short).await;
        }
        st
    }

    /// Stamps an open PR onto the land state so the titlebar can switch to
    /// View PR / Pull updates / Push updates.
    pub(crate) async fn attach_open_pr_land(
        &self,
        st: &mut WorkspaceLandState,
        workspace_id: &str,
        worktree: &str,
        mgr: Option<&recur_git::Manager>,
    ) {
        let Some(pr) = self.store.pr_by_workspace(workspace_id) else {
            return;
        };
        if pr.state == "Merged" || pr.group == "merged" {
            return;
        }
        st.pr_id = pr.id.clone();
        st.pr_number = pr.number;
        st.pr_source = pr.source.as_str().to_owned();
        st.pr_draft = pr.draft;
        let Some(forge) = self.pr_forge(&pr) else {
            return;
        };
        let Some(mgr) = mgr.filter(|_| !worktree.is_empty()) else {
            return;
        };
        let (r, _, _) = pr_tracking_ref(&pr, forge.as_ref());
        if r.is_empty() {
            return;
        }
        match mgr.commits_ahead(worktree, &r).await {
            Ok(n) => st.ahead_remote = n,
            // The tracking ref may not exist yet (offline, never fetched):
            // unique local commits count as unpushed.
            Err(_) if st.ahead_base > 0 => st.ahead_remote = st.ahead_base,
            Err(_) => {}
        }
        if let Ok(n) = mgr.commits_behind(worktree, &r).await {
            st.behind_remote = n;
        }
    }

    async fn main_working_copy_land_state(&self, ws: &Workspace) -> WorkspaceLandState {
        let mut st = WorkspaceLandState::default();
        let _ = tokio::time::timeout(LAND_STATE_TIMEOUT, async {
            let risk = recur_git::inspect_discard_risk(&ws.worktree_path).await;
            st.unmerged = risk.unmerged;
            st.dirty = risk.dirty_count > 0 && !risk.unmerged;
            if !recur_git::is_repo(&ws.worktree_path).await {
                return;
            }
            let mgr = recur_git::Manager::new(&ws.worktree_path, &self.worktree_root);
            let Some(up) = mgr.upstream(&ws.worktree_path).await else {
                return;
            };
            st.base = up.ref_name.clone();
            st.remote = up.remote.clone();
            if let Ok(n) = mgr.commits_behind(&ws.worktree_path, &up.ref_name).await {
                st.behind_base = n;
            }
            if let Ok(n) = mgr.commits_ahead(&ws.worktree_path, &up.ref_name).await {
                st.ahead_base = n;
            }
        })
        .await;
        st
    }

    /// Brings a hand-driven workspace up to date with its merge base: a
    /// clean merge or rebase in the worktree, or markers left for /update
    /// (status conflict). A clean update with an open forge PR also pushes
    /// it (force-with-lease after a rebase).
    pub async fn update_workspace_with_base(
        &self,
        workspace_id: &str,
        method: &str,
    ) -> Result<UpdateWorkspaceResult> {
        let ws = self
            .store
            .workspace(workspace_id)
            .ok_or_else(|| EngineError(format!("workspace {workspace_id:?} not found")))?;
        if ws.is_main || ws.is_pr_chat_host() {
            return Err("the main working copy is already the project checkout".into());
        }
        if land_locked_by_run(&ws) {
            return Err(
                "a workflow is running: pause it before updating from the workspace".into(),
            );
        }
        if ws.state == WorkspaceState::Merged {
            return Ok(UpdateWorkspaceResult {
                status: UPDATE_WORKSPACE_NOOP.into(),
                workspace: ws,
                ..UpdateWorkspaceResult::default()
            });
        }
        let proj = self
            .store
            .project(&ws.project_id)
            .filter(|p| !p.repo_path.is_empty())
            .ok_or_else(|| {
                EngineError(format!("project for workspace {workspace_id:?} not found"))
            })?;
        let base = workspace_base(&ws, &proj);
        let resolved = if method.trim().is_empty() {
            effective_update_base_method(&proj, &self.settings())
        } else {
            normalize_update_base_method(method)
        };
        tokio::time::timeout(
            LAND_TIMEOUT,
            self.update_workspace_with_base_inner(ws, proj, base, resolved),
        )
        .await
        .unwrap_or_else(|_| Err(deadline_err()))
    }

    async fn update_workspace_with_base_inner(
        &self,
        ws: Workspace,
        proj: Project,
        base: String,
        resolved: UpdateBaseMethod,
    ) -> Result<UpdateWorkspaceResult> {
        let workspace_id = ws.id.clone();
        let mgr = recur_git::Manager::new(&proj.repo_path, &self.worktree_root);
        let (base, behind, _) = behind_base(&mgr, &ws.worktree_path, &base, true).await;
        let result = |status: &str, ws: Workspace, base: &str| UpdateWorkspaceResult {
            status: status.into(),
            workspace: ws,
            base: base.to_owned(),
            ..UpdateWorkspaceResult::default()
        };
        if mgr.update_in_progress(&ws.worktree_path).await {
            return Ok(result(UPDATE_WORKSPACE_CONFLICT, ws, &base));
        }
        if behind == 0 {
            return Ok(result(UPDATE_WORKSPACE_NOOP, ws, &base));
        }
        match mgr
            .begin_update_with_base(&ws.worktree_path, &base, resolved.as_str())
            .await
        {
            Ok(()) => {
                self.sync_workspace_diff(&workspace_id, true).await;
                self.emit_workspace_event(
                    &workspace_id,
                    "AR",
                    "You",
                    &format!(
                        "Updated with latest <code>{}</code> ({}).",
                        html_escape(&mgr.short_branch_name(&base).await),
                        html_escape(resolved.label())
                    ),
                );
                let ws = self.store.workspace(&workspace_id).unwrap_or_default();
                let mut res = result(UPDATE_WORKSPACE_UPDATED, ws.clone(), &base);
                self.sync_forge_pr_after_base_update(&ws, &proj, &mgr, &base, &resolved, &mut res)
                    .await;
                Ok(res)
            }
            Err(e) => {
                if mgr.update_in_progress(&ws.worktree_path).await {
                    self.emit_workspace_event(
                        &workspace_id,
                        "AR",
                        "You",
                        "Update with base hit conflicts: resolve them in chat (/update).",
                    );
                    return Ok(result(UPDATE_WORKSPACE_CONFLICT, ws, &base));
                }
                Err(EngineError(format!("update with {base}: {e}")))
            }
        }
    }

    /// Pushes a clean update-with-base onto the open forge PR. Only a
    /// rebase force-pushes (with lease); a merge update is a regular push,
    /// and a remote that moved leaves the local update for Pull / Force push.
    /// No fetch before a force: the lease must compare against the last
    /// known remote head.
    async fn sync_forge_pr_after_base_update(
        &self,
        ws: &Workspace,
        proj: &Project,
        mgr: &recur_git::Manager,
        base: &str,
        method: &UpdateBaseMethod,
        res: &mut UpdateWorkspaceResult,
    ) {
        let Some(existing) = self.store.pr_by_workspace(&ws.id) else {
            return;
        };
        if existing.state == "Merged" || existing.group == "merged" {
            return;
        }
        let Some(forge) = self.pr_forge(&existing) else {
            return;
        };
        let force = *method == UpdateBaseMethod::Rebase;
        if let Err(e) = self
            .sync_existing_workspace_pr(ws, proj, &existing, mgr, base, "", "", force, false)
            .await
        {
            self.emit_workspace_event(
                &ws.id,
                "AR",
                "You",
                &format!(
                    "Updated with base but could not push to {} PR: {}",
                    forge.label(),
                    html_escape(e.0.trim())
                ),
            );
            return;
        }
        res.pushed_pr = existing.number;
        res.force_pushed = force;
    }

    /// Fetches the open forge PR's remote head and merges or rebases it into
    /// the worktree (Update-with-base prefs), for when the forge is ahead.
    /// Conflicts leave markers (status conflict).
    pub async fn pull_workspace_pr(&self, workspace_id: &str) -> Result<UpdateWorkspaceResult> {
        let ws = self
            .store
            .workspace(workspace_id)
            .ok_or_else(|| EngineError(format!("workspace {workspace_id:?} not found")))?;
        if ws.is_main || ws.is_pr_chat_host() {
            return Err("the main working copy has no workspace PR".into());
        }
        if land_locked_by_run(&ws) {
            return Err("a workflow is running: pause it before pulling".into());
        }
        let pr = self
            .store
            .pr_by_workspace(workspace_id)
            .filter(|p| p.state != "Merged" && p.group != "merged")
            .ok_or_else(|| EngineError::new("no open pull request for this workspace"))?;
        let proj = self.store.project(&ws.project_id);
        let Some(forge) = self.pr_forge(&pr) else {
            let label = self
                .forge_word(proj.as_ref().map_or("", |p| p.repo_path.as_str()))
                .await;
            return Err(EngineError(format!(
                "this is a Local PR: there is no {label} head to pull"
            )));
        };
        let proj = proj.filter(|p| !p.repo_path.is_empty()).ok_or_else(|| {
            EngineError(format!("project for workspace {workspace_id:?} not found"))
        })?;
        let (r, _, _) = pr_tracking_ref(&pr, forge.as_ref());
        if r.is_empty() {
            return Err("the open PR has no remote branch".into());
        }
        let method = effective_update_base_method(&proj, &self.settings());
        let label = forge.label().to_owned();
        let work = async {
            let mgr = recur_git::Manager::new(&proj.repo_path, &self.worktree_root);
            let result = |status: &str, ws: Workspace| UpdateWorkspaceResult {
                status: status.into(),
                workspace: ws,
                base: r.clone(),
                ..UpdateWorkspaceResult::default()
            };
            if mgr.update_in_progress(&ws.worktree_path).await {
                return Ok(result(UPDATE_WORKSPACE_CONFLICT, ws.clone()));
            }
            fetch_pr_tracking_head(&mgr, &proj.repo_path, &pr, forge.as_ref()).await;
            let behind = mgr.commits_behind(&ws.worktree_path, &r).await.unwrap_or(0);
            if behind == 0 {
                return Ok(result(UPDATE_WORKSPACE_NOOP, ws.clone()));
            }
            match mgr
                .begin_update_with_base(&ws.worktree_path, &r, method.as_str())
                .await
            {
                Ok(()) => {
                    self.sync_workspace_diff(workspace_id, true).await;
                    let base = workspace_base(&ws, &proj);
                    let _ = self
                        .refresh_workspace_pr_snapshot(&ws, &pr, &mgr, &base)
                        .await;
                    self.publish_pr_updated(&pr.id);
                    self.emit_workspace_event(
                        workspace_id,
                        "AR",
                        "You",
                        &format!(
                            "Pulled updates from {label} PR <code>#{}</code> ({}).",
                            pr.number,
                            html_escape(method.label())
                        ),
                    );
                    let ws = self.store.workspace(workspace_id).unwrap_or_default();
                    Ok(result(UPDATE_WORKSPACE_UPDATED, ws))
                }
                Err(e) => {
                    if mgr.update_in_progress(&ws.worktree_path).await {
                        self.emit_workspace_event(
                            workspace_id,
                            "AR",
                            "You",
                            &format!(
                                "Pull from {label} hit conflicts: resolve them in chat (/update)."
                            ),
                        );
                        return Ok(result(UPDATE_WORKSPACE_CONFLICT, ws.clone()));
                    }
                    Err(EngineError(format!(
                        "pull from {label} PR #{}: {e}",
                        pr.number
                    )))
                }
            }
        };
        tokio::time::timeout(LAND_TIMEOUT, work)
            .await
            .unwrap_or_else(|_| Err(deadline_err()))
    }

    /// Opens a PR from a workspace the autopilot is not driving: the diff
    /// and commits versus the base as a Local PR (or a forge PR when the
    /// project opts in). An open PR is pushed and refreshed instead, keeping
    /// title, body and comments. `force` is the titlebar's Force push.
    pub async fn open_workspace_pr(&self, workspace_id: &str, force: bool) -> Result<String> {
        self.open_workspace_pr_inner(workspace_id, "", "", false, force)
            .await
            .map(|(id, _)| id)
    }

    /// The agent's `open_pr`: like [`Engine::open_workspace_pr`] with the
    /// agent-written title and body. An open forge PR ignores them (push and
    /// refresh); an open Local PR takes them (and retries the forge). Never
    /// force-pushes.
    pub async fn open_workspace_pr_with(
        &self,
        workspace_id: &str,
        title: &str,
        body: &str,
        draft: bool,
    ) -> Result<String> {
        self.open_workspace_pr_inner(workspace_id, title, body, draft, false)
            .await
            .map(|(id, _)| id)
    }

    /// Returns (PR id, the forge create failure reason when it fell back to
    /// a Local PR).
    pub(crate) async fn open_workspace_pr_inner(
        &self,
        workspace_id: &str,
        title: &str,
        body: &str,
        draft: bool,
        force: bool,
    ) -> Result<(String, String)> {
        let ws = self
            .store
            .workspace(workspace_id)
            .ok_or_else(|| EngineError(format!("workspace {workspace_id:?} not found")))?;
        if ws.is_main || ws.is_pr_chat_host() {
            return Err("the main working copy can't open a PR: create a workspace for changes you want to review".into());
        }
        if land_locked_by_run(&ws) {
            return Err("a workflow is running: wait until it pauses, then open a PR from the titlebar or /open-pr".into());
        }
        let proj = self
            .store
            .project(&ws.project_id)
            .filter(|p| !p.repo_path.is_empty())
            .ok_or_else(|| {
                EngineError(format!("project for workspace {workspace_id:?} not found"))
            })?;
        let base = workspace_base(&ws, &proj);
        tokio::time::timeout(
            LAND_TIMEOUT,
            self.open_workspace_pr_body(ws, proj, base, title, body, draft, force),
        )
        .await
        .unwrap_or_else(|_| Err(deadline_err()))
    }

    #[allow(clippy::too_many_arguments)]
    async fn open_workspace_pr_body(
        &self,
        ws: Workspace,
        proj: Project,
        base: String,
        title: &str,
        body: &str,
        draft: bool,
        force: bool,
    ) -> Result<(String, String)> {
        let workspace_id = ws.id.clone();
        let mgr = recur_git::Manager::new(&proj.repo_path, &self.worktree_root);
        let risk = recur_git::inspect_discard_risk(&ws.worktree_path).await;
        if risk.dirty_count > 0 || risk.unmerged {
            return Err(
                "commit or discard uncommitted changes first: they would not be included in the PR"
                    .into(),
            );
        }
        if let Some(existing) = self.store.open_pr_by_workspace(&workspace_id) {
            // Already open: retry the forge when only a Local fallback
            // exists, else push and refresh. A forge PR's title and body
            // stay put; a forge row wins over a leftover Local one.
            return self
                .sync_existing_workspace_pr(
                    &ws, &proj, &existing, &mgr, &base, title, body, force, draft,
                )
                .await;
        }
        let diff = mgr
            .diff_pr(&ws.worktree_path, &base)
            .await
            .map_err(|e| EngineError(format!("compute diff: {e}")))?;
        let mut commits = mgr
            .log_commits(&ws.worktree_path, &base)
            .await
            .unwrap_or_default();
        fill_commit_files(&mgr, &ws.worktree_path, &mut commits, false).await;
        if diff.files.is_empty() && commits.is_empty() {
            return Err("nothing to open a PR for: commit changes in this workspace first".into());
        }
        let mut pr_num = self.store.next_pr_number();
        let mut pr_id = pr_num.to_string();
        let mut source = PrSource::Local;

        let name = ws.goal_label().trim().to_owned();
        let agent_title = single_line(title).trim().to_owned();
        let mut title = if agent_title.is_empty() {
            pr_title(if name.is_empty() {
                "Workspace changes"
            } else {
                &name
            })
        } else {
            hard_truncate_runes(&agent_title, 80).to_owned()
        };
        let mut pr_body = cap_github_pr_body(body.trim());
        if pr_body.is_empty() {
            let (_, diff_body) = mgr.changed_diff(&ws.worktree_path, &base).await;
            let cancel = tokio_util::sync::CancellationToken::new();
            if let Ok((t, b)) = self
                .generate_pr_description(
                    &cancel,
                    &workspace_id,
                    &ws.worktree_path,
                    &diff_body,
                    &commit_subjects(&commits),
                )
                .await
            {
                if agent_title.is_empty() && !t.is_empty() {
                    title = t;
                }
                pr_body = b;
            }
        }
        if pr_body.is_empty() {
            pr_body = pr_fallback_markdown(
                &pr_fallback_intro(&ws.goal, &ws.goal_display),
                &commits,
                diff.files.len() as i64,
                diff.add,
                diff.del,
                &[],
            );
        }
        // The live worktree branch wins over the cached name.
        let mut branch = self.sync_workspace_branch(&workspace_id).await;
        if branch.is_empty() {
            branch = ws.branch.clone();
        }
        let mut forge_fail = String::new();
        if proj.pr_on_github {
            match self
                .create_forge_pr(
                    &proj,
                    &ws.worktree_path,
                    &branch,
                    &base,
                    &title,
                    &pr_body,
                    draft,
                )
                .await
            {
                Some(Ok(created)) => {
                    let label = self.pr_source_label(&created.source);
                    source = created.source;
                    pr_num = created.number;
                    pr_id = created.pr_id;
                    let kind = if draft {
                        format!("{label} draft PR")
                    } else {
                        format!("{label} PR")
                    };
                    self.emit_workspace_event(
                        &workspace_id,
                        "AR",
                        "You",
                        &format!("Opened {kind} #{pr_num}."),
                    );
                }
                Some(Err((label, reason))) => {
                    self.emit_workspace_event(
                        &workspace_id,
                        "AR",
                        "You",
                        &format!(
                            "{label} PR not opened ({}): opening a Local PR instead.",
                            html_escape(&reason)
                        ),
                    );
                    forge_fail = reason;
                }
                None => {}
            }
        }
        let (checks, checks_ok) = if source == PrSource::Local {
            (Vec::new(), true)
        } else {
            self.forge_pr_checks(&source, &proj.repo_path, pr_num).await
        };
        let file_count = diff.files.len() as i64;
        let commit_count = commits.len() as i64;
        let mut pr = Pr {
            id: pr_id,
            workspace_id: workspace_id.clone(),
            project_id: ws.project_id.clone(),
            group: "you".into(),
            number: pr_num,
            source: source.clone(),
            branch,
            base,
            title,
            draft,
            summary: Some(workspace_pr_summary(
                &name,
                file_count,
                diff.add,
                diff.del,
                commit_count,
            )),
            focus: engine_focus(&diff.files, &[], None),
            state: "In review".into(),
            diff: DiffStat {
                add: diff.add,
                del: diff.del,
            },
            file_count,
            checks_ok,
            when: "now".into(),
            commits: none_if_empty(commits),
            files: none_if_empty(diff.files),
            checks: none_if_empty(checks),
            ..Pr::default()
        };
        if source == PrSource::Local {
            upsert_description_comment(&mut pr, &pr_body);
        }
        let saved = self
            .store
            .add_pr(pr)
            .map_err(|e| EngineError(format!("persist PR: {e}")))?;
        self.drop_unmerged_sibling_prs(&workspace_id, &saved.id);
        let mut opened = format!("{} PR", self.pr_source_label(&source));
        if draft && source != PrSource::Local {
            opened = format!("{} draft PR", self.pr_source_label(&source));
        }
        self.emit_workspace_event(
            &workspace_id,
            "AR",
            "You",
            &format!(
                "Opened a {opened} <code>#{}</code> (+{} −{} across {}) from this workspace.",
                saved.number,
                saved.diff.add,
                saved.diff.del,
                plural(saved.file_count, "file", "files")
            ),
        );
        self.publish_pr_updated(&saved.id);
        self.emit_workspace_pr_inbox(&saved, &name);
        Ok((saved.id, forge_fail))
    }

    /// Pushes new commits onto an already-open PR and refreshes the Recur
    /// snapshot. A forge PR's title and body stay; a Local PR takes a new
    /// agent title/body (and retries the forge when the project opts in).
    /// `force` is an explicit `--force-with-lease`; divergence alone never
    /// forces (that would drop remote-only commits).
    #[allow(clippy::too_many_arguments)]
    async fn sync_existing_workspace_pr(
        &self,
        ws: &Workspace,
        proj: &Project,
        existing: &Pr,
        mgr: &recur_git::Manager,
        base: &str,
        title: &str,
        body: &str,
        force: bool,
        draft: bool,
    ) -> Result<(String, String)> {
        let mut branch = self.sync_workspace_branch(&ws.id).await;
        if branch.is_empty() {
            branch = ws.branch.clone();
        }
        let mut forge_fail = String::new();
        let local = existing.source == PrSource::Local;
        let forge = self.pr_forge(existing).is_some();
        if local && proj.pr_on_github {
            match self
                .promote_local_pr_to_forge(
                    ws, proj, existing, mgr, base, &branch, title, body, draft,
                )
                .await
            {
                Ok(id) => return Ok((id, String::new())),
                Err(fail) => forge_fail = fail,
            }
        }
        if local {
            self.apply_local_pr_details(&existing.id, title, body);
        }
        if forge {
            let head = if existing.branch.is_empty() {
                branch.clone()
            } else {
                existing.branch.clone()
            };
            if let Err((label, e)) = self
                .push_forge_pr_head(&ws.worktree_path, &proj.repo_path, existing, &head, force)
                .await
            {
                self.emit_workspace_event(
                    &ws.id,
                    "AR",
                    "You",
                    &format!("Could not push to {label}: {}", html_escape(e.trim())),
                );
                return Err(EngineError(format!("push to {label}: {e}")));
            }
            branch = head;
        }
        self.refresh_workspace_pr_snapshot(ws, existing, mgr, base)
            .await?;
        if !forge {
            self.store
                .update_pr(&existing.id, |p| p.branch = branch.clone());
        }
        let updated = self
            .store
            .pr(&existing.id)
            .ok_or_else(|| EngineError(format!("update PR {}", existing.id)))?;
        let kind = self.pr_source_label(&existing.source);
        let verb = match (forge, force) {
            (false, _) => "Updated",
            (true, true) => "Force-pushed",
            (true, false) => "Pushed",
        };
        self.emit_workspace_event(
            &ws.id,
            "AR",
            "You",
            &format!(
                "{verb} updates to {kind} PR <code>#{}</code> (+{} −{} across {}).",
                updated.number,
                updated.diff.add,
                updated.diff.del,
                plural(updated.file_count, "file", "files")
            ),
        );
        self.publish_pr_updated(&updated.id);
        Ok((updated.id, forge_fail))
    }

    /// Retries the forge create after a Local fallback (offline, missing
    /// auth). On success the Recur row becomes the forge PR and keeps its
    /// id, thread and inbox row. `Err` is the failure reason.
    #[allow(clippy::too_many_arguments)]
    async fn promote_local_pr_to_forge(
        &self,
        ws: &Workspace,
        proj: &Project,
        existing: &Pr,
        mgr: &recur_git::Manager,
        base: &str,
        branch: &str,
        title: &str,
        body: &str,
        draft: bool,
    ) -> std::result::Result<String, String> {
        let draft = draft || existing.draft;
        let mut title = single_line(title).trim().to_owned();
        if title.is_empty() {
            title = existing.title.clone();
        }
        title = if title.is_empty() {
            let name = ws.goal_label().trim().to_owned();
            pr_title(if name.is_empty() {
                "Workspace changes"
            } else {
                &name
            })
        } else {
            hard_truncate_runes(&title, 80).to_owned()
        };
        let mut pr_body = cap_github_pr_body(body.trim());
        if pr_body.is_empty() {
            pr_body = pr_description(existing);
        }
        if pr_body.is_empty() {
            let diff = mgr.diff_pr(&ws.worktree_path, base).await.ok();
            let commits = mgr
                .log_commits(&ws.worktree_path, base)
                .await
                .unwrap_or_default();
            let (add, del, files) =
                diff.map_or((0, 0, 0), |d| (d.add, d.del, d.files.len() as i64));
            pr_body = pr_fallback_markdown(
                &pr_fallback_intro(&ws.goal, &ws.goal_display),
                &commits,
                files,
                add,
                del,
                &[],
            );
        }
        let created = match self
            .create_forge_pr(
                proj,
                &ws.worktree_path,
                branch,
                base,
                &title,
                &pr_body,
                draft,
            )
            .await
        {
            None => return Err(String::new()),
            Some(Err((label, reason))) => {
                self.emit_workspace_event(
                    &ws.id,
                    "AR",
                    "You",
                    &format!(
                        "{label} PR still not opened ({}): keeping the Local PR.",
                        html_escape(&reason)
                    ),
                );
                return Err(reason);
            }
            Some(Ok(c)) => c,
        };
        let updated = self.store.update_pr(&existing.id, |p| {
            p.source = created.source.clone();
            p.number = created.number;
            p.branch = branch.to_owned();
            p.title = title.clone();
            p.draft = draft;
            upsert_description_comment(p, &pr_body);
        });
        let Some(updated) = updated else {
            return Err(String::new());
        };
        let label = self.pr_source_label(&created.source);
        let opened = if draft {
            format!("{label} draft PR")
        } else {
            format!("{label} PR")
        };
        let note = match self
            .refresh_workspace_pr_snapshot(ws, &updated, mgr, base)
            .await
        {
            Ok(()) => format!("Opened {opened} #{}.", created.number),
            Err(_) => format!(
                "Opened {opened} #{} (snapshot refresh failed).",
                created.number
            ),
        };
        self.emit_workspace_event(&ws.id, "AR", "You", &note);
        self.publish_pr_updated(&updated.id);
        self.drop_unmerged_sibling_prs(&ws.id, &updated.id);
        Ok(updated.id)
    }

    /// A new agent title/body on a Local PR (no-op for a forge PR).
    fn apply_local_pr_details(&self, pr_id: &str, title: &str, body: &str) {
        let title = single_line(title).trim().to_owned();
        let body = cap_github_pr_body(body.trim());
        if title.is_empty() && body.is_empty() {
            return;
        }
        self.store.update_pr(pr_id, |p| {
            if is_forge_source(&p.source) {
                return;
            }
            if !title.is_empty() {
                p.title = hard_truncate_runes(&title, 80).to_owned();
            }
            if !body.is_empty() {
                upsert_description_comment(p, &body);
            }
            touch_pr_updated(p);
        });
    }

    /// Reloads files, commits and summary from the worktree without
    /// touching title, body, comments or the frozen forge head.
    async fn refresh_workspace_pr_snapshot(
        &self,
        ws: &Workspace,
        existing: &Pr,
        mgr: &recur_git::Manager,
        base: &str,
    ) -> Result<()> {
        let diff = mgr
            .diff_pr(&ws.worktree_path, base)
            .await
            .map_err(|e| EngineError(format!("compute diff: {e}")))?;
        let mut commits = mgr
            .log_commits(&ws.worktree_path, base)
            .await
            .unwrap_or_default();
        fill_commit_files(mgr, &ws.worktree_path, &mut commits, true).await;
        let name = ws.goal_label().trim().to_owned();
        let file_count = diff.files.len() as i64;
        let commit_count = commits.len() as i64;
        self.store
            .update_pr(&existing.id, |p| {
                p.summary = Some(workspace_pr_summary(
                    &name,
                    file_count,
                    diff.add,
                    diff.del,
                    commit_count,
                ));
                p.focus = engine_focus(&diff.files, &[], None);
                p.diff = DiffStat {
                    add: diff.add,
                    del: diff.del,
                };
                p.file_count = file_count;
                p.files = none_if_empty(diff.files);
                p.commits = none_if_empty(commits);
                p.when = "now".into();
                touch_pr_updated(p);
            })
            .map(|_| ())
            .ok_or_else(|| EngineError(format!("update PR {}", existing.id)))
    }

    /// The actionable "ready for review" inbox row of a workspace PR (plain
    /// wording: no review or verify ran).
    fn emit_workspace_pr_inbox(&self, pr: &Pr, name: &str) {
        let mut sub = html_escape(&truncate(name.trim(), 56));
        if sub.is_empty() {
            sub = "Workspace changes".into();
        }
        self.store.add_inbox_item(InboxItem {
            id: format!("ib-pr-{}", pr.id),
            group: "ready".into(),
            unread: true,
            kind: "pr".into(),
            color: "var(--emerald)".into(),
            title: format!("<b>#{}</b> is ready for review", pr.number),
            sub,
            time: "now".into(),
            action: "Review".into(),
            action_class: "go".into(),
            target: format!("pr:{}", pr.id),
            workspace_id: pr.workspace_id.clone(),
        });
        self.publish_inbox_updated();
        self.publish_notify(
            &format!("PR #{} is ready for review", pr.number),
            &truncate(name.trim(), 120),
            &format!("pr:{}", pr.id),
        );
    }
}

/// The `open_pr` tool result: what opened or got pushed, and why a
/// requested forge PR fell back to Local (the forge's command output only).
pub fn format_open_pr_tool_result(
    pr: &Pr,
    forge_label: &str,
    want_forge: bool,
    had_open: bool,
    was_local: bool,
    fail_diag: &str,
) -> String {
    if is_forge_source(&pr.source) {
        let kind = if pr.draft {
            format!("{forge_label} draft PR")
        } else {
            format!("{forge_label} PR")
        };
        if was_local {
            return format!("Opened {kind} #{} (promoted from Local).", pr.number);
        }
        if had_open {
            return format!(
                "Pushed new commits to {forge_label} PR #{} (description unchanged).",
                pr.number
            );
        }
        return format!("Opened {kind} #{}.", pr.number);
    }
    let fail = hard_truncate_runes(fail_diag.trim(), 240);
    let n = pr.number;
    match (had_open, want_forge, fail.is_empty()) {
        (true, true, false) => {
            format!(
                "PR #{n} is already open (Local). {forge_label} create failed ({fail}); retry Open PR."
            )
        }
        (true, true, true) => {
            format!("PR #{n} is already open (Local). {forge_label} create failed; retry Open PR.")
        }
        (true, false, _) => format!("PR #{n} is already open (Local)."),
        (false, true, false) => {
            format!("Opened Local PR #{n} ({forge_label} create failed: {fail}; retry Open PR).")
        }
        (false, true, true) => {
            format!("Opened Local PR #{n} ({forge_label} create failed; retry Open PR).")
        }
        (false, false, _) => format!("Opened Local PR #{n}."),
    }
}

impl Engine {
    /// The agent's `open_pr` tool: opens or syncs the workspace PR and
    /// reports it in the tool result.
    pub async fn open_pr_from_agent(
        &self,
        workspace_id: &str,
        title: &str,
        body: &str,
        draft: bool,
    ) -> String {
        let existing = self.store.pr_by_workspace(workspace_id);
        let had_open = existing
            .as_ref()
            .is_some_and(|p| p.state != "Merged" && p.group != "merged");
        let was_local = had_open
            && existing
                .as_ref()
                .is_some_and(|p| p.source == PrSource::Local);
        let (id, fail) = match self
            .open_workspace_pr_inner(workspace_id, title, body, draft, false)
            .await
        {
            Ok(r) => r,
            Err(e) => return format!("open_pr: {e}"),
        };
        let Some(pr) = self.store.pr(&id) else {
            return format!("Opened PR {id}.");
        };
        let proj = self
            .store
            .workspace(workspace_id)
            .and_then(|ws| self.store.project(&ws.project_id));
        let want_forge = proj.as_ref().is_some_and(|p| p.pr_on_github);
        let repo = proj.map(|p| p.repo_path).unwrap_or_default();
        let forge = match self.pr_forge(&pr) {
            Some(f) => Some(f),
            None => self.project_forge(&repo).await,
        };
        let label = forge.as_ref().map_or("", |f| f.label()).to_owned();
        let diag = match (&forge, fail.trim().is_empty()) {
            (Some(f), false) => f.failure_diag(fail.trim()),
            _ => fail.trim().to_owned(),
        };
        format_open_pr_tool_result(&pr, &label, want_forge, had_open, was_local, &diag)
    }
}
