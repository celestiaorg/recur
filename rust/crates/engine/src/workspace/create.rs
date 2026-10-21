//! Workspace creation (Go `workspace_create.go`, E1 half): persist the row,
//! allocate the git worktree (+ port, copied files, setup_cmd) in the
//! background, retry a failed setup, and roll a failed create back. The loop
//! start paths (`BeginStartWorkspace*`, `queueStartWorkspace`, …) are E3.

use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

use recur_domain::{Group, Project, Workspace, WorkspaceState};
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::core::{EngineError, Result};
use crate::text::truncate;
use crate::workspace::hooks::{HookKind, PROJECT_HOOK_TIMEOUT};

/// A new hand-driven workspace's Goal/Short before its chat names it.
pub const DEFAULT_WORKSPACE_TITLE: &str = "New Workspace";
pub const PHASE_CREATING: &str = "creating";
pub const STATUS_CREATING: &str = "Creating";
/// How long Remove/Delete wait for an in-flight create to exit.
pub const ALLOC_WAIT: Duration = Duration::from_secs(30);
/// The `git worktree add` budget.
pub const WORKTREE_ADD_TIMEOUT: Duration = Duration::from_secs(60);

/// One in-flight allocation: `cancel` stops git/setup; `done` fires once the
/// task fully exited.
pub struct CreatingAlloc {
    pub cancel: CancellationToken,
    pub done: CancellationToken,
}

/// The PR whose head a creating workspace checks out instead of the base
/// (E5's `create_workspace_from_pr` stores it before allocating).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrHeadCheckoutSpec {
    pub number: i64,
    pub branch: String,
    pub pr_id: String,
    pub cross_repo: bool,
}

/// `refs/heads/` stripped, trimmed (Go `normalizePRBranch`).
pub fn normalize_pr_branch(b: &str) -> String {
    let b = b.trim();
    b.strip_prefix("refs/heads/").unwrap_or(b).to_owned()
}

fn canceled() -> EngineError {
    EngineError::new("context canceled")
}

/// Waits for an allocation to exit, bounded by [`ALLOC_WAIT`].
pub async fn wait_alloc(alloc: Option<&Arc<CreatingAlloc>>) {
    if let Some(a) = alloc {
        let _ = tokio::time::timeout(ALLOC_WAIT, a.done.cancelled()).await;
    }
}

/// Overlays `src_root`'s dirty and untracked files (and deletions) onto
/// `dest_root`; tracked commits already travelled with the branch (Go
/// `copyWorktreeDirty`). Errors are joined with newlines.
pub async fn copy_worktree_dirty(src_root: &str, dest_root: &str) -> Result<(), String> {
    if src_root.is_empty() || dest_root.is_empty() || src_root == dest_root {
        return Ok(());
    }
    let st = recur_git::status_map(src_root).await.unwrap_or_default();
    let (src_root, dest_root) = (src_root.to_owned(), dest_root.to_owned());
    tokio::task::spawn_blocking(move || copy_dirty_files(&src_root, &dest_root, &st))
        .await
        .map_err(|e| e.to_string())?
}

fn copy_dirty_files(
    src_root: &str,
    dest_root: &str,
    st: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    use recur_bundles::go::path_err;
    let mut errs: Vec<String> = Vec::new();
    for (rel, letter) in st {
        let dst = match recur_code::safe_join(std::path::Path::new(dest_root), rel) {
            Ok(p) => p,
            Err(e) => {
                errs.push(e.to_string());
                continue;
            }
        };
        let dst_s = dst.to_string_lossy().into_owned();
        if letter == "d" {
            let res = match std::fs::symlink_metadata(&dst) {
                Ok(m) if m.is_dir() => std::fs::remove_dir_all(&dst),
                Ok(_) => std::fs::remove_file(&dst),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            };
            if let Err(e) = res
                && e.kind() != std::io::ErrorKind::NotFound
            {
                errs.push(path_err("unlinkat", &dst_s, &e));
            }
            continue;
        }
        let src = match recur_code::safe_join(std::path::Path::new(src_root), rel) {
            Ok(p) => p,
            Err(e) => {
                errs.push(e.to_string());
                continue;
            }
        };
        let src_s = src.to_string_lossy().into_owned();
        let info = match std::fs::symlink_metadata(&src) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                errs.push(path_err("lstat", &src_s, &e));
                continue;
            }
        };
        if info.file_type().is_symlink() || info.is_dir() {
            continue;
        }
        if let Some(parent) = dst.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            errs.push(path_err("mkdir", &parent.to_string_lossy(), &e));
            continue;
        }
        if let Err(e) = copy_file_overwrite(&src, &dst, &info) {
            errs.push(e);
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("\n"))
    }
}

fn copy_file_overwrite(
    from: &std::path::Path,
    to: &std::path::Path,
    info: &std::fs::Metadata,
) -> Result<(), String> {
    use recur_bundles::go::path_err;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    let from_s = from.to_string_lossy().into_owned();
    let to_s = to.to_string_lossy().into_owned();
    let mut input = std::fs::File::open(from).map_err(|e| path_err("open", &from_s, &e))?;
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(info.permissions().mode() & 0o777)
        .open(to)
        .map_err(|e| path_err("open", &to_s, &e))?;
    std::io::copy(&mut input, &mut out).map_err(|e| path_err("write", &to_s, &e))?;
    Ok(())
}

impl Engine {
    /// Creates a hand-driven workspace (no autopilot) and returns it once
    /// the worktree and setup_cmd finished. A failed allocate rolls the row
    /// back. An empty name is "New Workspace".
    pub async fn create_workspace(
        &self,
        project_id: &str,
        name: &str,
        base_branch: &str,
    ) -> Result<Workspace> {
        let rec = self
            .persist_creating_workspace(project_id, name, base_branch)
            .await?;
        let cancel = CancellationToken::new();
        let res = self.allocate_workspace_worktree(&cancel, &rec).await;
        cancel.cancel();
        if let Err(e) = res {
            self.rollback_creating(&rec.id).await;
            return Err(e);
        }
        self.maybe_dispatch_after_allocate(&rec.id).await;
        self.store
            .workspace(&rec.id)
            .ok_or_else(|| EngineError(format!("workspace {:?} missing after create", rec.id)))
    }

    /// Persists the workspace and allocates its worktree + setup in the
    /// background. The returned row has no worktree and phase "creating";
    /// `workspace.updated` carries the path as soon as `git worktree add`
    /// lands and again when setup finishes or fails.
    pub async fn begin_create_workspace(
        &self,
        project_id: &str,
        name: &str,
        base_branch: &str,
    ) -> Result<Workspace> {
        let rec = self
            .persist_creating_workspace(project_id, name, base_branch)
            .await?;
        self.start_allocate(rec.clone());
        Ok(rec)
    }

    pub(crate) async fn persist_creating_workspace(
        &self,
        project_id: &str,
        name: &str,
        base_branch: &str,
    ) -> Result<Workspace> {
        let proj = self
            .store
            .project(project_id)
            .ok_or_else(|| EngineError(format!("project {project_id:?} not found")))?;
        if proj.repo_path.is_empty() {
            return Err(EngineError(format!(
                "project {project_id:?} has no repo path"
            )));
        }
        if !recur_git::is_repo(&proj.repo_path).await {
            return Err(EngineError(format!(
                "project {project_id:?} is not a git repository — workspaces need a git checkout"
            )));
        }
        let mut base = base_branch.to_owned();
        if base.is_empty() {
            base = proj.base_branch.clone();
        }
        if base.is_empty() {
            base = "main".into();
        }
        let mut name = name.trim().to_owned();
        if name.is_empty() {
            name = DEFAULT_WORKSPACE_TITLE.into();
        }
        let mgr = self.git_manager(&proj.repo_path);
        let saved = {
            let _id = self.id_mu.lock().await;
            let id = self.mint_workspace_id(project_id, Some(&mgr)).await;
            let rec = Workspace {
                id: id.clone(),
                project_id: project_id.to_owned(),
                r#type: recur_domain::WORKSPACE_SOLO.into(),
                group: Group::Active,
                goal: name.clone(),
                short: truncate(&name, 48),
                status_label: STATUS_CREATING.into(),
                phase: PHASE_CREATING.into(),
                branch: self.workspace_branch(&recur_git::workspace_slug(&id)),
                base_branch: base,
                last: "creating".into(),
                elapsed_label: "just now".into(),
                ..Workspace::default()
            };
            self.store
                .add_workspace(rec)
                .map_err(|e| EngineError(e.to_string()))?
        };
        self.publish_loop_updated(
            &saved.id,
            saved.state.clone(),
            &saved.phase,
            saved.iteration,
        );
        Ok(saved)
    }

    pub(crate) fn start_allocate(&self, rec: Workspace) {
        let alloc = Arc::new(CreatingAlloc {
            cancel: CancellationToken::new(),
            done: CancellationToken::new(),
        });
        self.creating
            .lock()
            .insert(rec.id.clone(), Arc::clone(&alloc));
        let e = self.arc();
        tokio::spawn(async move {
            let res = e.allocate_workspace_worktree(&alloc.cancel, &rec).await;
            match res {
                Err(err) => {
                    if alloc.cancel.is_cancelled() {
                        e.cleanup_partial_worktree(&rec).await;
                    } else {
                        e.fail_creating(&rec.id, &err.0);
                    }
                }
                Ok(()) => {
                    // Drop the in-flight mark before dispatching so a run
                    // stamped during this tail dispatches itself.
                    e.forget_creating(&rec.id, &alloc);
                    e.maybe_dispatch_after_allocate(&rec.id).await;
                }
            }
            alloc.cancel.cancel();
            e.forget_creating(&rec.id, &alloc);
            alloc.done.cancel();
        });
    }

    pub(crate) fn forget_creating(&self, id: &str, alloc: &Arc<CreatingAlloc>) {
        let mut creating = self.creating.lock();
        if creating.get(id).is_some_and(|a| Arc::ptr_eq(a, alloc)) {
            creating.remove(id);
        }
    }

    /// Whether a worktree allocation is in flight for the workspace.
    pub fn creating_in_flight(&self, id: &str) -> bool {
        self.creating.lock().contains_key(id)
    }

    /// Re-runs setup_cmd on a hand workspace whose create failed after the
    /// worktree landed (the tree is kept; an edited setup_cmd is picked up).
    pub async fn retry_workspace_setup(&self, id: &str) -> Result<()> {
        let ws = self
            .store
            .workspace(id)
            .ok_or_else(|| EngineError(format!("workspace {id:?} not found")))?;
        if ws.is_main || ws.is_pr_chat_host() {
            return Err("retry setup only applies to a hand workspace".into());
        }
        if ws.has_run() {
            return Err("retry setup only applies to a workspace that never started a loop".into());
        }
        if ws.phase != "failed" && ws.status_label != "Couldn't create" {
            return Err("this workspace did not fail setup".into());
        }
        // Copy-files failures keep the tree too; retry does not re-copy.
        if !ws.last_error.starts_with("setup command failed") {
            return Err("this workspace did not fail setup".into());
        }
        if ws.worktree_path.is_empty()
            || !std::fs::metadata(&ws.worktree_path).is_ok_and(|m| m.is_dir())
        {
            return Err(
                "the worktree was removed — remove this workspace and create it again".into(),
            );
        }
        let proj = self
            .store
            .project(&ws.project_id)
            .ok_or_else(|| EngineError(format!("project {:?} not found", ws.project_id)))?;

        let alloc = Arc::new(CreatingAlloc {
            cancel: CancellationToken::new(),
            done: CancellationToken::new(),
        });
        {
            let mut creating = self.creating.lock();
            if creating.contains_key(id) {
                return Err("workspace setup is already running".into());
            }
            creating.insert(id.to_owned(), Arc::clone(&alloc));
        }
        // Installed first so Remove/Delete can wait on it; a reservation may
        // have landed between the read and the insert.
        let release = |e: &Engine| {
            alloc.cancel.cancel();
            e.forget_creating(id, &alloc);
            alloc.done.cancel();
        };
        if self.is_deleting(id) {
            release(self);
            return Err(EngineError(format!("workspace {id:?} is being removed")));
        }
        let Some(reset) = self.store.update_workspace(id, |w| {
            w.phase = PHASE_CREATING.into();
            w.status_label = STATUS_CREATING.into();
            w.last = "creating".into();
            w.last_error.clear();
        }) else {
            release(self);
            return Err(EngineError(format!("workspace {id:?} not found")));
        };
        self.publish_loop_updated(
            &reset.id,
            reset.state.clone(),
            &reset.phase,
            reset.iteration,
        );

        let e = self.arc();
        let id = id.to_owned();
        tokio::spawn(async move {
            e.retry_setup_task(&alloc, &id, &reset, &proj).await;
            alloc.cancel.cancel();
            e.forget_creating(&id, &alloc);
            alloc.done.cancel();
        });
        Ok(())
    }

    async fn retry_setup_task(
        &self,
        alloc: &Arc<CreatingAlloc>,
        id: &str,
        reset: &Workspace,
        proj: &Project,
    ) {
        let cancel = &alloc.cancel;
        if cancel.is_cancelled() || !self.creating_still_wanted(id) {
            return;
        }
        let setup = self
            .run_project_hook_with(
                cancel,
                Some(Instant::now() + PROJECT_HOOK_TIMEOUT),
                &proj.setup_cmd,
                reset,
                proj,
                HookKind::Setup,
            )
            .await;
        if let Err(err) = setup {
            if cancel.is_cancelled() {
                return;
            }
            self.fail_creating(id, &format!("setup command failed: {err}"));
            return;
        }
        if cancel.is_cancelled() || !self.creating_still_wanted(id) {
            return;
        }
        let mut merge_base = String::new();
        let overlay = self.fork_overlay.lock().remove(id);
        if let Some(src) = overlay {
            if !src.worktree_path.is_empty()
                && !reset.worktree_path.is_empty()
                && let Err(err) =
                    copy_worktree_dirty(&src.worktree_path, &reset.worktree_path).await
            {
                if cancel.is_cancelled() {
                    return;
                }
                self.fail_creating(id, &format!("copy source changes: {err}"));
                return;
            }
            merge_base = src.base_branch.trim().to_owned();
        }
        if cancel.is_cancelled() || !self.creating_still_wanted(id) {
            return;
        }
        let mut wrote = false;
        let ready = self.store.update_workspace(id, |w| {
            if w.archived {
                return;
            }
            wrote = true;
            w.status_label = "Idle".into();
            w.phase = "editing".into();
            w.last = "created".into();
            w.last_error.clear();
            if !merge_base.is_empty() {
                w.base_branch = merge_base.clone();
            }
        });
        let Some(ready) = ready.filter(|_| wrote) else {
            return;
        };
        self.publish_loop_updated(
            &ready.id,
            ready.state.clone(),
            &ready.phase,
            ready.iteration,
        );
        self.forget_creating(id, alloc);
        self.maybe_dispatch_after_allocate(id).await;
    }

    /// Cancels an in-flight create and returns it so the caller can wait.
    pub fn cancel_creating(&self, id: &str) -> Option<Arc<CreatingAlloc>> {
        let alloc = self.creating.lock().get(id).cloned()?;
        alloc.cancel.cancel();
        Some(alloc)
    }

    async fn git_cancellable<T>(
        &self,
        cancel: &CancellationToken,
        budget: Option<Duration>,
        fut: impl std::future::Future<Output = recur_git::Result<T>>,
    ) -> Result<T, String> {
        let fut = async {
            match budget {
                Some(d) => tokio::time::timeout(d, fut).await.unwrap_or_else(|_| {
                    Err(recur_git::Error::from(std::io::Error::other(
                        "context deadline exceeded",
                    )))
                }),
                None => fut.await,
            }
        };
        tokio::select! {
            r = fut => r.map_err(|e| e.to_string()),
            _ = cancel.cancelled() => Err("context canceled".to_owned()),
        }
    }

    /// Allocates the worktree, publishes the path, copies configured files
    /// and runs setup_cmd. A failed setup keeps the published tree (the
    /// editor or terminal may already be in use).
    pub(crate) async fn allocate_workspace_worktree(
        &self,
        cancel: &CancellationToken,
        rec: &Workspace,
    ) -> Result<()> {
        let res = self.allocate_inner(cancel, rec).await;
        // Keep the spec visible until this allocate returns so a second
        // "create from PR" click reuses the in-flight workspace.
        self.pr_head_checkout.lock().remove(&rec.id);
        res
    }

    async fn allocate_inner(&self, cancel: &CancellationToken, rec: &Workspace) -> Result<()> {
        let proj = self
            .store
            .project(&rec.project_id)
            .ok_or_else(|| EngineError(format!("project {:?} not found", rec.project_id)))?;
        if !self.creating_still_wanted(&rec.id) {
            return Err(canceled());
        }
        let mut base = rec.base_branch.clone();
        if base.is_empty() {
            base = proj.base_branch.clone();
        }
        if base.is_empty() {
            base = "main".into();
        }
        let mgr = self.git_manager(&proj.repo_path);
        let mut add_base = base.clone();
        let head_spec = self
            .pr_head_checkout
            .lock()
            .get(&rec.id)
            .filter(|s| s.number > 0)
            .cloned()
            .unwrap_or_default();
        if head_spec.number > 0 {
            let r = self
                .git_cancellable(cancel, None, mgr.fetch_pr_head(head_spec.number))
                .await
                .map_err(|e| EngineError(format!("fetch pull request head: {e}")))?;
            if !r.is_empty() {
                add_base = r;
            }
        }
        let slug = recur_git::workspace_slug(&rec.id);
        let use_named_head =
            head_spec.number > 0 && !head_spec.branch.is_empty() && !head_spec.cross_repo;
        let added = if use_named_head {
            self.git_cancellable(
                cancel,
                Some(WORKTREE_ADD_TIMEOUT),
                mgr.add_at_ref(&rec.id, &slug, &head_spec.branch, &add_base),
            )
            .await
        } else {
            self.git_cancellable(
                cancel,
                Some(WORKTREE_ADD_TIMEOUT),
                mgr.add(&rec.id, &slug, &add_base),
            )
            .await
        };
        let (wt, created) = added.map_err(|e| EngineError(format!("allocate worktree: {e}")))?;
        if use_named_head
            && !wt.path.is_empty()
            && normalize_pr_branch(&wt.branch) == normalize_pr_branch(&head_spec.branch)
        {
            tokio::select! {
                _ = mgr.try_set_upstream(&wt.path, &head_spec.branch) => {}
                _ = cancel.cancelled() => {}
            }
        }

        let mut port = rec.port;
        if created && port <= 0 {
            match recur_project::runconfig::free_port() {
                Ok(p) => port = i64::from(p),
                Err(e) => {
                    let _ = mgr.remove(&wt.path).await;
                    return Err(EngineError(format!("allocate port: {e}")));
                }
            }
        }
        if cancel.is_cancelled() {
            let _ = mgr.remove(&wt.path).await;
            return Err(canceled());
        }
        if !self.creating_still_wanted(&rec.id) {
            let _ = mgr.remove(&wt.path).await;
            return Err(canceled());
        }

        let mut wrote = false;
        let updated = self.store.update_workspace(&rec.id, |w| {
            // Remove may have archived the row meanwhile: never resurrect
            // path/port.
            if w.archived {
                return;
            }
            wrote = true;
            w.worktree_path = wt.path.clone();
            w.branch = wt.branch.clone();
            w.base_branch = base.clone();
            w.port = port;
            w.host_pr_id.clear();
        });
        let updated = match updated {
            Some(u) if wrote => u,
            gone => {
                let _ = mgr.remove(&wt.path).await;
                if gone.is_none() {
                    return Err(EngineError(format!(
                        "workspace {:?} gone during allocate",
                        rec.id
                    )));
                }
                return Err(canceled());
            }
        };
        // Paint the Observatory as soon as the worktree exists; chat still
        // waits because the phase stays "creating" through setup.
        self.publish_loop_updated(
            &updated.id,
            updated.state.clone(),
            &updated.phase,
            updated.iteration,
        );
        self.link_workspace_to_matching_prs(&updated).await;
        if !head_spec.pr_id.is_empty() {
            let ws_id = updated.id.clone();
            let ws_proj = updated.project_id.clone();
            self.store.update_pr(&head_spec.pr_id, |p| {
                p.workspace_id = ws_id;
                if p.project_id.trim().is_empty() {
                    p.project_id = ws_proj;
                }
            });
            self.publish_pr_updated(&head_spec.pr_id);
            self.archive_pr_chat_hosts(&head_spec.pr_id, &updated.id)
                .await;
        }

        if created {
            self.copy_project_files(cancel, &proj, &updated.worktree_path)
                .await
                .map_err(|e| EngineError(format!("copy files: {e}")))?;
            let setup = self
                .run_project_hook_with(
                    cancel,
                    Some(Instant::now() + PROJECT_HOOK_TIMEOUT),
                    &proj.setup_cmd,
                    &updated,
                    &proj,
                    HookKind::Setup,
                )
                .await;
            if let Err(err) = setup {
                self.run_teardown_hook(&updated, &proj).await;
                return Err(EngineError(format!("setup command failed: {err}")));
            }
            if cancel.is_cancelled() {
                self.run_teardown_hook(&updated, &proj).await;
                let _ = mgr.remove(&wt.path).await;
                return Err(canceled());
            }
        }

        let mut merge_base = String::new();
        let overlay = self.fork_overlay.lock().remove(&rec.id);
        if let Some(src) = overlay {
            if !src.worktree_path.is_empty() && !updated.worktree_path.is_empty() {
                copy_worktree_dirty(&src.worktree_path, &updated.worktree_path)
                    .await
                    .map_err(|e| EngineError(format!("copy source changes: {e}")))?;
            }
            merge_base = src.base_branch.trim().to_owned();
        }

        let Some(ready) = self.store.update_workspace(&rec.id, |w| {
            w.status_label = "Idle".into();
            w.phase = "editing".into();
            w.last = "created".into();
            w.last_error.clear();
            if !merge_base.is_empty() {
                w.base_branch = merge_base.clone();
            }
        }) else {
            let _ = mgr.remove(&wt.path).await;
            return Err(EngineError(format!(
                "workspace {:?} gone after allocate",
                rec.id
            )));
        };
        self.publish_loop_updated(
            &ready.id,
            ready.state.clone(),
            &ready.phase,
            ready.iteration,
        );
        Ok(())
    }

    /// Marks a create failed (keeping any worktree path so Delete can remove
    /// it) and fails a stamped run.
    pub fn fail_creating(&self, id: &str, msg: &str) {
        self.pr_head_checkout.lock().remove(id);
        let Some(updated) = self.store.update_workspace(id, |w| {
            w.status_label = "Couldn't create".into();
            w.phase = "failed".into();
            w.last = "create failed".into();
            w.last_error = msg.to_owned();
            if w.has_run() {
                w.state = WorkspaceState::Failed;
            }
        }) else {
            return;
        };
        self.publish_loop_updated(
            &updated.id,
            updated.state.clone(),
            &updated.phase,
            updated.iteration,
        );
    }

    /// The workspace still wants its allocation (not being removed or
    /// archived).
    pub fn creating_still_wanted(&self, id: &str) -> bool {
        if self.is_deleting(id) {
            return false;
        }
        self.store.workspace(id).is_some_and(|ws| !ws.archived)
    }

    /// Undoes a failed synchronous create: teardown, partial worktree,
    /// the still-at-base branch, and the row.
    pub(crate) async fn rollback_creating(&self, id: &str) {
        self.fork_overlay.lock().remove(id);
        self.pr_head_checkout.lock().remove(id);
        let Some(ws) = self.store.workspace(id) else {
            return;
        };
        let proj = self.store.project(&ws.project_id);
        if let Some(p) = &proj {
            self.run_teardown_hook(&ws, p).await;
        }
        self.cleanup_partial_worktree(&ws).await;
        if let Some(p) = &proj
            && !ws.branch.is_empty()
        {
            let mut base = ws.base_branch.clone();
            if base.is_empty() {
                base = p.base_branch.clone();
            }
            let mgr = self.plain_git_manager(&p.repo_path);
            let _ = mgr.delete_branch_if_at_base(&ws.branch, &base).await;
        }
        let _ = self.store.delete_workspace(id);
    }

    /// Removes a worktree a create may have left (the stored path, else the
    /// managed path for the id).
    pub(crate) async fn cleanup_partial_worktree(&self, ws: &Workspace) {
        let mut path = ws.worktree_path.clone();
        if path.is_empty() && !self.worktree_root.is_empty() && !ws.id.is_empty() {
            path = format!(
                "{}/{}",
                self.worktree_root.trim_end_matches('/'),
                recur_git::worktree_rel_path(&ws.id)
            );
        }
        if path.is_empty() {
            return;
        }
        let mut ws = ws.clone();
        ws.worktree_path = path;
        self.remove_loop_worktree(&ws).await;
    }

    /// Overlays glob-matched files from the checkout onto a new worktree
    /// before setup_cmd (bounded by the hook timeout and `cancel`).
    pub(crate) async fn copy_project_files(
        &self,
        cancel: &CancellationToken,
        proj: &Project,
        dest: &str,
    ) -> Result<(), String> {
        let (cancel, src, dest_s, patterns) = (
            cancel.clone(),
            proj.repo_path.clone(),
            dest.to_owned(),
            proj.copy_files.clone(),
        );
        let deadline = Some(std::time::Instant::now() + PROJECT_HOOK_TIMEOUT);
        let n = tokio::task::spawn_blocking(move || {
            recur_code::copyfiles::copy(&cancel, deadline, &src, &dest_s, &patterns)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        if n > 0 {
            tracing::info!("recur: copied {n} file(s) into {dest}");
        }
        Ok(())
    }
}
