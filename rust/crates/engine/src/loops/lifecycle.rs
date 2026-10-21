//! Loop lifecycle (Go `engine.go` start/dispatch/resume/drain and
//! `workspace_create.go` queued starts): persisting a run, taking a governor
//! slot (or queueing), the single dispatch path that allocates the worktree
//! and launches the walk task, Resume of live and not-running walks, the
//! drain that promotes the oldest queued loop, and the walk task itself with
//! its panic guard.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use recur_agent::attach::strip_markers;
use recur_domain::{AutonomyLevel, ChatAttachment, FeedItem, Group, Workspace, WorkspaceState};
use tokio_util::sync::CancellationToken;

use crate::bus::{EngineEvent, EscalationResolved};
use crate::chat_shared::{is_unnamed_workspace, provisional_workspace_title};
use crate::codes::MAESTRO_AGENT_CODE;
use crate::loop_handle::{Command, EscalationAnswer, LoopHandle};
use crate::loop_registry::{LOOP_MAP_SEP, RunningLoop};
use crate::loops::Walk;
use crate::loops::escalation::restore_escalation_answer;
use crate::publish::status_label;
use crate::text::{html_escape, truncate};
use crate::workspace::create::{PHASE_CREATING, STATUS_CREATING, WORKTREE_ADD_TIMEOUT};
use crate::workspace::hooks::HookKind;
use crate::{Engine, EngineError, Result};

/// A concurrent dispatch already owns the walk (D26). Benign.
pub const ERR_ALREADY_DISPATCHED: &str = "loop already dispatched";
/// The conductor tab closed before the walk registered. Benign.
pub const ERR_LOOP_OWNER_GONE: &str = "loop owner closed";

/// A dispatch failure that leaves the loop alone (release the slot only).
pub fn benign_dispatch_err(e: &EngineError) -> bool {
    e.0 == ERR_ALREADY_DISPATCHED || e.0 == ERR_LOOP_OWNER_GONE
}

/// Whether a not-running loop can be (re)started by Resume: queued, paused
/// (incl. interrupted), escalated, or failed retryably (D56).
pub fn resumable(rec: &Workspace) -> bool {
    match rec.state {
        WorkspaceState::Queued | WorkspaceState::Paused | WorkspaceState::Escalated => true,
        WorkspaceState::Failed => rec.retryable,
        _ => false,
    }
}

fn quote(s: &str) -> String {
    recur_bundles::go::quote(s)
}

fn valid_autonomy(a: AutonomyLevel) -> AutonomyLevel {
    match a {
        AutonomyLevel::Cautious | AutonomyLevel::Balanced | AutonomyLevel::Aggressive => a,
        _ => AutonomyLevel::Balanced,
    }
}

type WalkFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

impl Walk {
    /// Walks the graph to a terminal or idle state. A guard that cannot
    /// resolve idles with the reason; a persisted answer is applied first.
    pub(crate) async fn run(mut self, cancel: CancellationToken) -> Walk {
        if !self.missing_guard.is_empty() {
            self.emit_feed(
                "MA",
                "Maestro",
                &format!("{} — idling.", html_escape(&self.missing_guard)),
            );
            self.set_state(WorkspaceState::Paused, "loop refs missing");
            return self;
        }
        if self.apply_answered_cap(&cancel).await {
            return self;
        }
        self.walk_loop(&cancel).await;
        self
    }
}

impl Engine {
    /// Launches the walk task: the walk runs in its own task so a panic is
    /// converted to a failed loop BEFORE the slot is released (D25), then
    /// `loop_exited` releases the slot and drains the queue.
    fn spawn_walk(&self, walk: Walk, cancel: CancellationToken) {
        let eng = self.arc();
        let h = Arc::clone(&walk.h);
        let fut: WalkFuture = Box::pin(async move {
            let inner = tokio::spawn(walk.run(cancel));
            let wrap = match inner.await {
                Ok(w) => Some(w.wrap_up_inputs()),
                Err(e) if e.is_panic() => {
                    let p = e.into_panic();
                    let msg = p
                        .downcast_ref::<&str>()
                        .map(|s| (*s).to_owned())
                        .or_else(|| p.downcast_ref::<String>().cloned())
                        .unwrap_or_default();
                    tracing::error!("recur: loop {} panicked: {msg}", h.id);
                    eng.recover_walk_panic(&h, &msg);
                    None
                }
                Err(_) => None,
            };
            eng.loop_exited(&h, wrap).await;
        });
        self.tasks.loops.spawn(fut);
    }

    /// Go `Loop.recoverPanic`: one walk's panic is a failed loop, never a
    /// crashed app.
    fn recover_walk_panic(&self, h: &Arc<LoopHandle>, msg: &str) {
        {
            let mut s = h.lock();
            s.walk_state = WorkspaceState::Failed;
            s.exit_state = WorkspaceState::Failed;
            s.exit_phase = "failed".into();
        }
        self.set_workspace_state(&h.id, WorkspaceState::Failed, "failed");
        self.emit_workspace_event_item(
            &h.id,
            FeedItem {
                agent_code: "MA".into(),
                name: "Maestro".into(),
                text: format!("Loop failed (internal error): {}", html_escape(msg)),
                chat_code: h.chat_code(),
                ..FeedItem::default()
            },
        );
    }

    /// Creates a loop record, allocates its worktree off `base_branch` (the
    /// project's when empty) and starts the walk; with no free loop slot it
    /// is persisted queued and drained later (D23). Returns the id.
    pub async fn start_workspace(
        &self,
        project_id: &str,
        task: &str,
        base_branch: &str,
        autonomy: AutonomyLevel,
    ) -> Result<String> {
        self.start_workspace_with(project_id, task, base_branch, autonomy, "", "")
            .await
    }

    /// [`start_workspace`](Self::start_workspace) with an explicit loop and
    /// policy.
    pub async fn start_workspace_with(
        &self,
        project_id: &str,
        task: &str,
        base_branch: &str,
        autonomy: AutonomyLevel,
        loop_id: &str,
        policy_id: &str,
    ) -> Result<String> {
        let (rec, acquired) = self
            .queue_start_workspace(
                project_id,
                task,
                "",
                base_branch,
                &autonomy,
                loop_id,
                policy_id,
                None,
            )
            .await?;
        if !acquired {
            return Ok(rec.id);
        }
        let id = rec.id.clone();
        if let Err(e) = self.dispatch(rec).await {
            self.gov.release_loop();
            return Err(e);
        }
        Ok(id)
    }

    /// Persists the run (and takes a slot when free) then dispatches in the
    /// background so the New-workspace modal can close at once. `simplify`
    /// is the New Workspace chip (`None` = Settings / inherit).
    pub async fn begin_start_workspace(
        &self,
        project_id: &str,
        task: &str,
        base_branch: &str,
        autonomy: AutonomyLevel,
        simplify: Option<bool>,
    ) -> Result<Workspace> {
        self.begin_start_workspace_with(
            project_id,
            task,
            "",
            base_branch,
            autonomy,
            "",
            "",
            simplify,
            &[],
        )
        .await
    }

    /// [`begin_start_workspace`](Self::begin_start_workspace) plus the
    /// bundle loop and policy, the goal's display label, and the opening
    /// image pastes (so the planner and specialists see them).
    #[allow(clippy::too_many_arguments)]
    pub async fn begin_start_workspace_with(
        &self,
        project_id: &str,
        task: &str,
        goal_display: &str,
        base_branch: &str,
        autonomy: AutonomyLevel,
        loop_id: &str,
        policy_id: &str,
        simplify: Option<bool>,
        attachments: &[ChatAttachment],
    ) -> Result<Workspace> {
        if !loop_id.trim().is_empty() || !policy_id.trim().is_empty() {
            self.resolve_guard(loop_id, policy_id, &autonomy)
                .map_err(EngineError)?;
        }
        let (rec, acquired) = self
            .queue_start_workspace(
                project_id,
                task,
                goal_display,
                base_branch,
                &autonomy,
                loop_id,
                policy_id,
                simplify,
            )
            .await?;
        let mut imgs = Vec::new();
        if !attachments.is_empty() {
            match crate::attachments::prepare_chat_attachments(
                &self.data_dir,
                &rec.id,
                task,
                attachments,
            ) {
                Ok((_, i)) => imgs = i,
                Err(_) => {
                    self.abort_queued_start(&rec, acquired).await;
                    return Err(EngineError::new(
                        "I couldn't save that image attachment. Try a smaller PNG/JPEG/GIF/WebP.",
                    ));
                }
            }
        }
        self.stamp_loop_goal_images(&rec.id, MAESTRO_AGENT_CODE, &imgs);
        if !acquired {
            return Ok(rec);
        }
        let eng = self.arc();
        let r = rec.clone();
        tokio::spawn(async move {
            let id = r.id.clone();
            if let Err(e) = eng.dispatch(r).await {
                eng.gov.release_loop();
                if !benign_dispatch_err(&e) {
                    eng.fail_creating(&id, &e.0);
                }
            }
        });
        Ok(rec)
    }

    /// Validates and persists a new queued run row (Go `queueStartWorkspace`)
    /// and takes a loop slot when one is free. True = the slot is held and
    /// the caller dispatches.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn queue_start_workspace(
        &self,
        project_id: &str,
        task: &str,
        goal_display: &str,
        base_branch: &str,
        autonomy: &AutonomyLevel,
        loop_id: &str,
        policy_id: &str,
        simplify: Option<bool>,
    ) -> Result<(Workspace, bool)> {
        let Some(proj) = self.store.project(project_id) else {
            return Err(EngineError(format!(
                "project {} not found",
                quote(project_id)
            )));
        };
        if !recur_git::is_repo(&proj.repo_path).await {
            return Err(EngineError(format!(
                "project {} is not a git repository — workspaces need a git checkout",
                quote(project_id)
            )));
        }
        let mut base_branch = base_branch.to_owned();
        if base_branch.is_empty() {
            base_branch = proj.base_branch.clone();
        }
        if base_branch.is_empty() {
            base_branch = "main".into();
        }
        let mut autonomy = autonomy.clone();
        if autonomy.is_empty() {
            autonomy = self.store.policy().autonomy;
        }
        let autonomy = valid_autonomy(autonomy);
        self.resolve_guard(loop_id, policy_id, &autonomy)
            .map_err(EngineError)?;

        let mgr = self.git_manager(&proj.repo_path);
        let id_lock = self.id_mu.lock().await;
        let id = self.mint_workspace_id(project_id, Some(&mgr)).await;
        let raw_task = task.trim();
        let mut task = strip_markers(raw_task);
        if task.is_empty() && !raw_task.is_empty() {
            task = "image attachment".into();
        }
        let mut display = strip_markers(goal_display.trim());
        if display == task.trim() {
            display.clear();
        }
        let short_src = if display.is_empty() { &task } else { &display };
        let mut rec = Workspace {
            id: id.clone(),
            project_id: project_id.to_owned(),
            run_started: true,
            group: Group::Active,
            short: truncate(short_src, 48),
            goal: task.clone(),
            goal_display: display.clone(),
            state: WorkspaceState::Queued,
            status_label: "Queued".into(),
            phase: "waiting for a slot".into(),
            iteration: 0,
            progress: 0.0,
            autonomy,
            loop_id: loop_id.trim().to_owned(),
            policy_id: policy_id.trim().to_owned(),
            loop_chat_code: MAESTRO_AGENT_CODE.into(),
            branch: self.workspace_branch(&recur_git::workspace_slug(&id)),
            base_branch,
            agent_codes: Some(vec![MAESTRO_AGENT_CODE.into()]),
            last: "queued".into(),
            elapsed_label: "just now".into(),
            ..Workspace::default()
        };
        let acquired = self.gov.try_acquire_loop();
        if acquired {
            // The slot is ours; git/setup is the blocker now.
            rec.phase = PHASE_CREATING.into();
            rec.status_label = STATUS_CREATING.into();
            rec.last = "creating".into();
        }
        if let Err(e) = self.store.add_workspace(rec.clone()) {
            drop(id_lock);
            if acquired {
                self.gov.release_loop();
            }
            return Err(EngineError(e.to_string()));
        }
        drop(id_lock);
        self.stamp_maestro_simplify_if_unset(&rec.id);
        if let Some(on) = simplify {
            self.remember_chat_simplify(&rec.id, MAESTRO_AGENT_CODE, on);
        }
        if !acquired {
            self.publish_loop_updated(&rec.id, rec.state.clone(), &rec.phase, rec.iteration);
            self.publish_resource();
            return Ok((rec, false));
        }
        Ok((rec, true))
    }

    /// Rolls a queued start back after a failure before dispatch.
    pub(crate) async fn abort_queued_start(&self, rec: &Workspace, acquired: bool) {
        self.rollback_creating(&rec.id).await;
        if acquired {
            self.gov.release_loop();
        }
        if !rec.id.is_empty() {
            let _ = std::fs::remove_dir_all(self.data_dir.join("workspaces").join(&rec.id));
        }
    }

    /// Engages the autopilot on an EXISTING workspace (the chat dock's Loop
    /// mode) with its stamped loop/policy. True = waiting for a slot.
    pub async fn start_run_on_workspace(&self, workspace_id: &str, goal: &str) -> Result<bool> {
        let (loop_id, policy_id) = self
            .store
            .workspace(workspace_id)
            .map(|r| (r.loop_id, r.policy_id))
            .unwrap_or_default();
        self.start_run_on_workspace_with(workspace_id, goal, &loop_id, &policy_id)
            .await
    }

    /// Stamps a run on an existing workspace (reusing its worktree) and
    /// dispatches it, or queues it. Refused on the main working copy, a PR
    /// chat host, a workspace that already has a loop, or one that never
    /// finished creating.
    pub async fn start_run_on_workspace_with(
        &self,
        workspace_id: &str,
        goal: &str,
        loop_id: &str,
        policy_id: &str,
    ) -> Result<bool> {
        let goal = goal.trim();
        if goal.is_empty() {
            return Err("the loop needs a goal — describe the task".into());
        }
        let Some(rec) = self.store.workspace(workspace_id) else {
            return Err(EngineError(format!(
                "workspace {} not found",
                quote(workspace_id)
            )));
        };
        if rec.is_main {
            return Err(
                "the main working copy can't run the autopilot — create a workspace instead".into(),
            );
        }
        if rec.is_pr_chat_host() {
            return Err("create a workspace from this pull request to run the autopilot".into());
        }
        if rec.has_run() {
            return Err("this workspace already has a loop — resume or restart it instead".into());
        }
        if rec.phase == "failed" || rec.status_label == "Couldn't create" {
            return Err("this workspace never finished creating — discard it and try again".into());
        }
        // An empty path is only OK while the worktree is still allocating.
        if rec.worktree_path.is_empty() && !self.creating_in_flight(workspace_id) {
            return Err("this workspace never finished creating — discard it and try again".into());
        }
        let autonomy = valid_autonomy(self.store.policy().autonomy);
        if !loop_id.trim().is_empty() || !policy_id.trim().is_empty() {
            self.resolve_guard(loop_id, policy_id, &autonomy)
                .map_err(EngineError)?;
        }
        let (lid, pid) = (loop_id.trim().to_owned(), policy_id.trim().to_owned());
        let Some(updated) = self.store.update_workspace(workspace_id, |l| {
            // A failCreating that landed meanwhile must not become a queued
            // run nothing dispatches.
            if l.phase == "failed" || !l.last_error.is_empty() {
                return;
            }
            l.run_started = true;
            l.group = Group::Active;
            if is_unnamed_workspace(l) {
                let t = provisional_workspace_title(goal);
                let t = if t.is_empty() { goal.to_owned() } else { t };
                l.short = truncate(&t, 48);
                l.goal = t;
            }
            l.state = WorkspaceState::Queued;
            l.status_label = "Queued".into();
            l.phase = "waiting for a slot".into();
            l.iteration = 0;
            l.progress = 0.0;
            l.autonomy = autonomy.clone();
            l.loop_id = lid.clone();
            l.policy_id = pid.clone();
            l.last = "queued".into();
            let codes = l.agent_codes.get_or_insert_with(Vec::new);
            if !codes.iter().any(|c| c == MAESTRO_AGENT_CODE) {
                codes.push(MAESTRO_AGENT_CODE.into());
            }
        }) else {
            return Err(EngineError(format!(
                "workspace {} not found",
                quote(workspace_id)
            )));
        };
        if !updated.has_run() {
            return Err("this workspace never finished creating — discard it and try again".into());
        }
        self.publish_loop_updated(
            &updated.id,
            updated.state.clone(),
            &updated.phase,
            updated.iteration,
        );
        self.stamp_maestro_simplify_if_unset(&updated.id);
        if updated.worktree_path.is_empty() || self.creating_in_flight(workspace_id) {
            // Still creating (or setup_cmd running): allocation dispatches.
            return Ok(true);
        }
        if !self.gov.try_acquire_loop() {
            self.publish_resource();
            return Ok(true);
        }
        if let Err(e) = self.dispatch(updated).await {
            self.gov.release_loop();
            if benign_dispatch_err(&e) {
                return Ok(false);
            }
            return Err(e);
        }
        self.publish_resource();
        Ok(false)
    }

    /// Starts a walk from a durable record (D23); the caller holds a loop
    /// slot. Reserves the registry entry before any side effect (D26),
    /// allocates or restores the worktree, persists the running state and
    /// launches the walk. Backs every start path (StartWorkspace, the queued
    /// drain, Resume), so "running" always means a real walk.
    pub(crate) async fn dispatch(&self, mut rec: Workspace) -> Result<()> {
        let Some(proj) = self.store.project(&rec.project_id) else {
            return Err(EngineError(format!(
                "project {} not found",
                quote(&rec.project_id)
            )));
        };
        let mgr = self.git_manager(&proj.repo_path);
        self.stamp_maestro_simplify_if_unset(&rec.id);
        // CloseWorkspaceChat can clear LoopChatCode after a snapshot; trust
        // the live row. A named conductor that is now empty is a close.
        let want = rec.loop_chat_code.trim().to_owned();
        if let Some(live) = self.store.workspace(&rec.id) {
            rec.loop_chat_code = live.loop_chat_code;
        }
        let arc = self.arc();
        let mut walk = Walk::new(&arc, &rec);
        if walk.missing_guard.is_empty() {
            rec.loop_id = walk.loop_id.clone();
            rec.policy_id = walk.policy_id.clone();
        }
        let code = rec.loop_chat_code.trim().to_owned();
        if !want.is_empty() && code.is_empty() {
            return Err(ERR_LOOP_OWNER_GONE.into());
        }
        let mut life = None;
        if !code.is_empty() {
            // Held through registration so a close cannot miss an
            // in-progress dispatch.
            let (g, held) = self.lock_chat_life(&rec.id, &code).await;
            if !held {
                return Err(ERR_LOOP_OWNER_GONE.into());
            }
            if let Some(live) = self.store.workspace(&rec.id) {
                rec.loop_chat_code = live.loop_chat_code;
                walk.h.lock().chat_code = rec.loop_chat_code.trim().to_owned();
            }
            if walk.chat_code() != code {
                return Err(ERR_LOOP_OWNER_GONE.into());
            }
            // MA is created on first start; pair tabs must exist or this is a
            // close that raced the agent's deletion.
            if code != MAESTRO_AGENT_CODE {
                if self.store.agent(&rec.id, &code).is_none() {
                    return Err(ERR_LOOP_OWNER_GONE.into());
                }
            } else {
                self.ensure_maestro_agent_with_status(&rec.id, rec.goal_label(), "idle");
            }
            life = Some(g);
        }
        walk.git = Some(mgr.clone());
        walk.rekey(&self.loop_map_key(&rec));
        let cancel = CancellationToken::new();
        if !self.register_loop(RunningLoop {
            handle: Arc::clone(&walk.h),
            cancel: cancel.clone(),
        }) {
            drop(life);
            return Err(ERR_ALREADY_DISPATCHED.into());
        }
        drop(life);

        // From here a registry entry exists; any failure removes it (the
        // caller releases the slot) so a later Resume can retry.
        let handle = Arc::clone(&walk.h);
        let cleanup = |e: &Engine| {
            e.remove_loop(&handle);
            cancel.cancel();
        };

        // The slot is held and git worktree add is next.
        if rec.worktree_path.is_empty()
            && let Some(updated) = self.store.update_workspace(&rec.id, |w| {
                if !w.worktree_path.is_empty() || w.phase == "failed" || !w.last_error.is_empty() {
                    return;
                }
                w.phase = PHASE_CREATING.into();
                w.status_label = STATUS_CREATING.into();
                w.last = "creating".into();
            })
        {
            self.publish_loop_updated(
                &updated.id,
                updated.state.clone(),
                &updated.phase,
                updated.iteration,
            );
        }

        // Fresh runs Add (a leftover branch is never checked out); resume,
        // archive and an existing worktree Restore the recorded branch.
        let mut base = rec.base_branch.clone();
        if base.is_empty() {
            base = proj.base_branch.clone();
        }
        if base.is_empty() {
            base = "main".into();
        }
        let added = if rec.archived || !rec.worktree_path.is_empty() || walk.resume.is_some() {
            tokio::time::timeout(
                WORKTREE_ADD_TIMEOUT,
                mgr.restore(&rec.id, &rec.branch, &base),
            )
            .await
        } else {
            tokio::time::timeout(
                WORKTREE_ADD_TIMEOUT,
                mgr.add(&rec.id, &recur_git::workspace_slug(&rec.id), &base),
            )
            .await
        };
        let (wt, created) = match added {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                cleanup(self);
                return Err(EngineError(format!("allocate worktree: {e}")));
            }
            Err(_) => {
                cleanup(self);
                return Err(EngineError(format!(
                    "allocate worktree: {}",
                    crate::loops::DEADLINE_EXCEEDED
                )));
            }
        };
        rec.worktree_path = wt.path.clone();
        rec.branch = wt.branch.clone();
        rec.base_branch = base.clone();
        if created && rec.port <= 0 {
            match recur_project::runconfig::free_port() {
                Ok(p) => rec.port = i64::from(p),
                Err(e) => {
                    cleanup(self);
                    let _ = mgr.remove(&wt.path).await;
                    return Err(EngineError(format!("allocate port: {e}")));
                }
            }
        }
        // A checkpointed loop RESUMES at its phase and iteration (D55); only
        // a fresh one starts at planning.
        if let Some(r) = &walk.resume {
            rec.state = r.state.clone();
            rec.status_label = status_label(&r.state);
            rec.phase = format!("resuming at iteration {}", r.iteration);
            rec.iteration = r.iteration;
        } else {
            rec.state = WorkspaceState::Planning;
            rec.status_label = "Planning".into();
            rec.phase = "starting".into();
        }
        rec.group = Group::Active;
        // Keep the live conductor (a close during worktree add) and issue
        // link.
        let r = rec.clone();
        if let Some(updated) = self.store.update_workspace(&rec.id, |w| {
            let keep = std::mem::take(&mut w.loop_chat_code);
            let issue = w.issue.take();
            *w = r;
            w.loop_chat_code = keep;
            w.issue = issue;
        }) {
            rec.loop_chat_code = updated.loop_chat_code;
            rec.issue = updated.issue;
        } else if let Err(e) = self.store.add_workspace(rec.clone()) {
            cleanup(self);
            return Err(EngineError(e.to_string()));
        }
        if created {
            if let Err(e) = self
                .copy_project_files(&cancel, &proj, &rec.worktree_path)
                .await
            {
                let _ = mgr.remove(&wt.path).await;
                let _ = mgr.delete_branch_if_at_base(&wt.branch, &base).await;
                self.fail_setup(&rec.id, "copy files failed", &e);
                cleanup(self);
                return Err(EngineError(format!("copy files: {e}")));
            }
            if let Err(e) = self
                .run_project_hook(&proj.setup_cmd, &rec, &proj, HookKind::Setup)
                .await
            {
                self.run_teardown_hook(&rec, &proj).await;
                let _ = mgr.remove(&wt.path).await;
                let _ = mgr.delete_branch_if_at_base(&wt.branch, &base).await;
                self.fail_setup(&rec.id, "setup command failed", &e);
                cleanup(self);
                return Err(EngineError(format!("setup command failed: {e}")));
            }
        }
        walk.worktree = wt.path.clone();
        {
            let mut s = walk.h.lock();
            s.worktree = wt.path.clone();
            s.branch = wt.branch.clone();
        }
        self.publish_loop_updated(&rec.id, rec.state.clone(), &rec.phase, rec.iteration);
        self.publish_resource();
        self.spawn_walk(walk, cancel);
        Ok(())
    }

    fn fail_setup(&self, id: &str, last: &str, err: &str) {
        self.store.update_workspace(id, |lp| {
            lp.worktree_path.clear();
            lp.state = WorkspaceState::Failed;
            lp.status_label = "Failed".into();
            lp.phase = "setup failed".into();
            lp.last = last.into();
            lp.last_error = err.to_owned();
            lp.group = Group::Needs;
        });
    }

    /// Starts a second walk on a workspace whose sibling still runs: it
    /// shares the worktree but neither resets the sibling's state nor
    /// resumes its checkpoint.
    pub async fn dispatch_additional_chat_run(&self, mut rec: Workspace) -> Result<()> {
        let code = rec.loop_chat_code.trim().to_owned();
        if code.is_empty() {
            return Err("additional chat run needs a loop chat code".into());
        }
        let (_life, held) = self.lock_chat_life(&rec.id, &code).await;
        if !held {
            return Err(ERR_LOOP_OWNER_GONE.into());
        }
        if self.store.agent(&rec.id, &code).is_none() {
            return Err(ERR_LOOP_OWNER_GONE.into());
        }
        if rec.worktree_path.is_empty() {
            return Err(EngineError(format!(
                "workspace {} has no worktree",
                quote(&rec.id)
            )));
        }
        let Some(proj) = self.store.project(&rec.project_id) else {
            return Err(EngineError(format!(
                "project {} not found",
                quote(&rec.project_id)
            )));
        };
        rec.iteration = 0;
        let arc = self.arc();
        let mut walk = Walk::new(&arc, &rec);
        walk.git = Some(self.git_manager(&proj.repo_path));
        walk.worktree = rec.worktree_path.clone();
        {
            let mut s = walk.h.lock();
            s.worktree = rec.worktree_path.clone();
            s.branch = rec.branch.clone();
            s.chat_code = code.clone();
        }
        walk.rekey(&format!("{}{LOOP_MAP_SEP}{code}", rec.id));
        walk.fresh_walk = true;
        walk.resume = None;
        let cancel = CancellationToken::new();
        if !self.register_loop(RunningLoop {
            handle: Arc::clone(&walk.h),
            cancel: cancel.clone(),
        }) {
            return Err(ERR_ALREADY_DISPATCHED.into());
        }
        self.spawn_walk(walk, cancel);
        Ok(())
    }

    /// Starts this chat's walk while a sibling keeps running.
    pub(crate) async fn start_additional_chat_walk(&self, rec: Workspace) -> String {
        if rec.loop_chat_code.trim().is_empty() {
            return self.dispatch_idle_walk(&rec.id).await;
        }
        if !self.gov.try_acquire_loop() {
            return "Refused: every loop slot is busy.".into();
        }
        let id = rec.id.clone();
        if let Err(e) = self.dispatch_additional_chat_run(rec).await {
            self.gov.release_loop();
            return format!("Refused: {e}");
        }
        self.publish_resource();
        self.emit_workspace_event(
            &id,
            "MA",
            "Maestro",
            "Restarting the loop over the current worktree.",
        );
        "Loop restarting — it continues from the current worktree, steered by the recorded plan."
            .into()
    }

    /// Continues a loop: a live walk gets Resume; a not-running resumable
    /// one is re-dispatched (queued when no slot is free). Terminal loops are
    /// left alone.
    pub async fn resume(&self, id: &str) {
        self.resume_with_owner(id, "").await;
    }

    pub(crate) async fn resume_with_owner(&self, id: &str, want_owner: &str) {
        let running = self.running_loops_on(id);
        if !running.is_empty() {
            for rl in running {
                self.resume_live_loop(id, &rl.handle);
            }
            return;
        }
        // Under dispatch_mu: two Resumes promote once, and a racing Stop
        // (same lock) cannot be resurrected (re-read below).
        let d = self.dispatch_mu.lock().await;
        self.resume_not_running_locked(&d, id, want_owner).await;
    }

    /// Re-dispatches `id` from its durable record. Caller holds
    /// `dispatch_mu`.
    pub(crate) async fn resume_not_running_locked(
        &self,
        _held: &tokio::sync::MutexGuard<'_, ()>,
        id: &str,
        want_owner: &str,
    ) {
        if self.running_loop_by_id(id).is_some() {
            return;
        }
        let Some(mut rec) = self.store.workspace(id) else {
            return;
        };
        if !resumable(&rec) {
            return;
        }
        let want = want_owner.trim();
        if !want.is_empty() {
            rec.loop_chat_code = want.to_owned();
        }
        self.run_promote_hook();
        let mut owner = None;
        if !want.is_empty() {
            let (g, held) = self.lock_chat_life(id, want).await;
            if !held || !self.expected_owner_live(id, want) {
                return;
            }
            owner = Some(g);
        }
        if !self.gov.try_acquire_loop() {
            // Queued for the drain; the life lock stops a close from leaving
            // it as an empty-owner walk meanwhile.
            self.set_workspace_state(id, WorkspaceState::Queued, "waiting for a slot");
            self.mark_group(id, Group::Active);
            drop(owner);
            return;
        }
        drop(owner);
        if let Err(e) = self.dispatch(rec).await {
            self.gov.release_loop();
            if !benign_dispatch_err(&e) {
                self.set_workspace_state(
                    id,
                    WorkspaceState::Failed,
                    &format!("resume failed: {e}"),
                );
            }
        }
    }

    /// A live walk still blocked on an escalation whose answer is already
    /// persisted gets that answer re-delivered; otherwise Resume.
    fn resume_live_loop(&self, id: &str, lp: &Arc<LoopHandle>) {
        if let Some(esc) = self.store.escalation(id)
            && esc.status == "answered"
            && lp.lock().escalated
        {
            self.mark_group(id, Group::Active);
            let ans = restore_escalation_answer(&esc).unwrap_or_else(|| EscalationAnswer {
                choice: esc.answer.clone(),
                resolved: esc.answer.clone(),
                ..EscalationAnswer::default()
            });
            lp.send(Command::AnswerEscalation(ans));
            if self
                .store
                .workspace(id)
                .is_some_and(|r| r.state == WorkspaceState::Escalated)
            {
                self.set_workspace_state(id, WorkspaceState::Coding, "resuming");
            }
            self.bus
                .publish(EngineEvent::EscalationResolved(EscalationResolved {
                    agent_code: None,
                    answer: esc.answer,
                    open: None,
                    workspace_id: id.to_owned(),
                }));
            return;
        }
        lp.send(Command::Resume);
    }

    /// Whether `want` still owns the workspace (caller holds its life lock).
    fn expected_owner_live(&self, id: &str, want: &str) -> bool {
        let want = want.trim();
        if want.is_empty() {
            return true;
        }
        let Some(live) = self.store.workspace(id) else {
            return false;
        };
        if live.loop_chat_code.trim() != want {
            return false;
        }
        want == MAESTRO_AGENT_CODE || self.store.agent(id, want).is_some()
    }

    /// Promotes the oldest queued loop into a free slot (D23). A closed
    /// conductor is skipped so the rest still drains; otherwise at most one
    /// is promoted per call (each promoted loop drains the next on exit).
    /// The whole pick → acquire → register section runs under
    /// `dispatch_mu` (D26).
    pub(crate) async fn drain_queued(&self) {
        let _d = self.dispatch_mu.lock().await;
        let mut seen = std::collections::HashSet::new();
        loop {
            // Insertion order: the oldest queued wins (R4).
            let Some(next) = self.store.workspaces().into_iter().find(|l| {
                l.state == WorkspaceState::Queued
                    && !self.loop_key_running(&l.id)
                    && !seen.contains(&l.id)
            }) else {
                return;
            };
            seen.insert(next.id.clone());
            self.run_promote_hook();
            if !self.gov.try_acquire_loop() {
                return;
            }
            let id = next.id.clone();
            match self.dispatch(next).await {
                Ok(()) => return,
                Err(e) => {
                    self.gov.release_loop();
                    if e.0 == ERR_ALREADY_DISPATCHED {
                        return;
                    }
                    if e.0 == ERR_LOOP_OWNER_GONE {
                        if self
                            .store
                            .workspace(&id)
                            .is_some_and(|r| r.state == WorkspaceState::Queued)
                        {
                            self.set_workspace_state(&id, WorkspaceState::Paused, "idle");
                        }
                        continue;
                    }
                    self.set_workspace_state(
                        &id,
                        WorkspaceState::Failed,
                        &format!("drain failed: {e}"),
                    );
                    return;
                }
            }
        }
    }

    /// Starts a run stamped while the worktree was still allocating.
    pub(crate) async fn maybe_dispatch_after_allocate(&self, id: &str) {
        let Some(rec) = self.store.workspace(id) else {
            return;
        };
        if !rec.has_run() || rec.worktree_path.is_empty() {
            return;
        }
        if self.loop_key_running(id) {
            return;
        }
        if !self.gov.try_acquire_loop() {
            self.publish_resource();
            return;
        }
        if let Err(e) = self.dispatch(rec).await {
            self.gov.release_loop();
            if !benign_dispatch_err(&e) {
                self.fail_creating(id, &e.0);
            }
        }
    }
}
