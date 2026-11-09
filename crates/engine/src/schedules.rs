//! Scheduled prompts: saved local times that start an independent workspace and
//! run a prompt on its first tab (or engage its loop). One ticker task checks
//! once a second under `schedule`, which also serializes configuration changes,
//! occurrence claims and Shutdown. Startup never replays overdue occurrences:
//! anything a minute late is missed; busy targets are skipped, not queued.
//!
//! `launch_prompt_workspace` is shared with the create_workspace tool.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use parking_lot::Mutex;
use recur_agent::Flags;
use recur_domain::{
    CHAT_MODE_AGENT, CHAT_MODE_ASK, CHAT_MODE_LOOP, CHAT_MODE_PLAN, CHAT_MODE_REVIEW,
    ScheduledPrompt, ScheduledPromptRun, WorkspaceState, next_scheduled_time, parse_model_ref,
    run_active,
};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use crate::attention::chat_inbox::scheduled_pause_settled;
use crate::codes::PAIR_AGENT_CODE;
use crate::core::ScheduleControl;
use crate::turn::finalize::CHAT_STOPPED_REPLY;
use crate::workspace::create::CreatingAlloc;
use crate::{Engine, EngineError, Result};

/// One engine-initiated workspace: allocate a fresh worktree, then run the
/// prompt on its first tab (or engage the loop).
#[derive(Debug, Clone, Default)]
pub struct PromptLaunch {
    pub project_id: String,
    pub name: String,
    pub base_branch: String,
    pub prompt: String,
    pub mode: String,
    pub model: String,
    pub loop_id: String,
    pub policy_id: String,
    pub schedule_id: String,
    pub flags: Flags,
    pub simplify: Option<bool>,
}

/// Called with the new id before any side effect; an error rolls it back.
pub(crate) type LinkFn<'a> = Box<dyn FnOnce(&str) -> Result<()> + Send + 'a>;
/// Receives the outcome (finished/failed/stopped/waiting) once the first
/// turn settles.
pub(crate) type DoneFn = Arc<dyn Fn(&str, &str) + Send + Sync>;

fn scheduled_millis<T: chrono::TimeZone>(t: Option<DateTime<T>>) -> i64 {
    t.map_or(0, |t| t.timestamp_millis())
}

#[cfg(test)]
mod tests;

fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

impl Engine {
    fn now_utc(&self) -> DateTime<Utc> {
        self.now().with_timezone(&Utc)
    }

    /// Validates and saves a schedule with its next occurrence.
    pub async fn save_scheduled_prompt(&self, mut p: ScheduledPrompt) -> Result<ScheduledPrompt> {
        let _g = self.schedule.lock().await;
        p.name = p.name.trim().to_owned();
        p.prompt = p.prompt.trim().to_owned();
        p.cron = p.cron.trim().to_owned();
        if p.mode == CHAT_MODE_LOOP {
            let guard = self
                .resolve_guard(&p.loop_id, &p.policy_id, &self.store.policy().autonomy)
                .map_err(EngineError)?;
            p.loop_id = guard.loop_def.id;
            p.policy_id = guard.policy.id;
        }
        if p.name.is_empty() || p.prompt.is_empty() {
            return Err(EngineError::new("name and prompt are required"));
        }
        self.validate_scheduled_prompt(&p).await?;
        if p.id.is_empty() {
            p.id = new_id();
        } else {
            let old = self.scheduled_prompt(&p.id)?;
            if old.project_id != p.project_id {
                return Err(EngineError::new("a schedule cannot change projects"));
            }
        }
        let next =
            next_scheduled_time(&p, self.now_utc()).map_err(|e| EngineError(e.to_string()))?;
        if next.is_none() && !p.on_startup() {
            return Err(EngineError::new(
                "choose a future date and time that exists in this time zone",
            ));
        }
        p.next_run_at = scheduled_millis(next);
        if p.simplify.is_none() {
            p.simplify = Some(self.settings().simplify_enabled());
        }
        p.last_run = None;
        self.store
            .save_scheduled_prompt(&p)
            .map_err(|e| EngineError(e.to_string()))?;
        Ok(p)
    }

    async fn validate_scheduled_prompt(&self, p: &ScheduledPrompt) -> Result<()> {
        let proj = self
            .store
            .project(&p.project_id)
            .ok_or_else(|| EngineError::new("project not found"))?;
        match p.mode.as_str() {
            CHAT_MODE_ASK | CHAT_MODE_PLAN | CHAT_MODE_AGENT | CHAT_MODE_REVIEW => {}
            CHAT_MODE_LOOP => {
                self.resolve_guard(&p.loop_id, &p.policy_id, &self.store.policy().autonomy)
                    .map_err(EngineError)?;
            }
            _ => {
                return Err(EngineError::new(
                    "choose Ask, Plan, Agent, Review, or Loop mode",
                ));
            }
        }
        let r = parse_model_ref(&p.model);
        if r.id.is_empty()
            || !p.model.contains('/')
            || !self.store.settings().model_ref_offers_models(&p.model)
        {
            return Err(EngineError::new("choose an available model"));
        }
        let exists = tokio::time::timeout(
            Duration::from_secs(5),
            self.git_manager(&proj.repo_path)
                .base_branch_exists(&p.base_branch),
        )
        .await
        .unwrap_or(false);
        if !exists {
            return Err(EngineError::new("base branch no longer exists"));
        }
        Ok(())
    }

    fn scheduled_prompt(&self, id: &str) -> Result<ScheduledPrompt> {
        self.store
            .scheduled_prompts()
            .map_err(|e| EngineError(e.to_string()))?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| EngineError::new("scheduled prompt not found"))
    }

    /// Pauses or resumes a schedule. History stays; resuming skips the
    /// times that elapsed meanwhile.
    pub async fn pause_scheduled_prompt(&self, id: &str, paused: bool) -> Result<()> {
        let _g = self.schedule.lock().await;
        let mut p = self.scheduled_prompt(id)?;
        p.paused = paused;
        if !(paused || p.cadence == "once" && p.next_run_at == 0) {
            let next =
                next_scheduled_time(&p, self.now_utc()).map_err(|e| EngineError(e.to_string()))?;
            p.next_run_at = scheduled_millis(next);
        }
        self.store
            .save_scheduled_prompt(&p)
            .map_err(|e| EngineError(e.to_string()))
    }

    /// Stops future occurrences without stopping a running chat.
    pub async fn delete_scheduled_prompt(&self, id: &str) -> Result<()> {
        let _g = self.schedule.lock().await;
        self.store
            .delete_scheduled_prompt(id)
            .map_err(|e| EngineError(e.to_string()))
    }

    /// Claims a manual occurrence and returns its new workspace. A one-time
    /// schedule is consumed; recurring ones keep their next due time.
    pub async fn run_scheduled_prompt_now(&self, id: &str) -> Result<String> {
        let _g = self.schedule.lock().await;
        if self.is_shutting_down() {
            return Err(EngineError::new("Recur is closing"));
        }
        let mut p = self.scheduled_prompt(id)?;
        let runs = self
            .store
            .active_scheduled_prompt_runs()
            .map_err(|e| EngineError(e.to_string()))?;
        if self.schedule_busy(&p.id, &runs) {
            return Err(EngineError::new(
                "the previous run is still active or needs your input",
            ));
        }
        self.validate_scheduled_prompt(&p).await?;
        let now = self.now_utc();
        if p.cadence == "once" {
            p.next_run_at = 0;
        } else if p.next_run_at > 0 && p.next_run_at <= now.timestamp_millis() {
            let next = next_scheduled_time(&p, now).map_err(|e| EngineError(e.to_string()))?;
            p.next_run_at = scheduled_millis(next);
        }
        let r = ScheduledPromptRun {
            id: new_id(),
            schedule_id: id.to_owned(),
            due_at: now.timestamp_millis(),
            status: "running".into(),
            ..ScheduledPromptRun::default()
        };
        let claimed = self
            .store
            .claim_scheduled_prompt(&p, &r)
            .map_err(|e| EngineError(e.to_string()))?;
        if !claimed {
            return Err(EngineError::new("this occurrence was already claimed"));
        }
        self.launch_scheduled_prompt(p, r).await
    }

    fn schedule_busy(&self, id: &str, runs: &[ScheduledPromptRun]) -> bool {
        runs.iter().filter(|r| r.schedule_id == id).any(|r| {
            r.status == "running"
                || (!r.workspace_id.is_empty() && self.scheduled_chat_active(&r.workspace_id))
        })
    }

    /// Starts the app-owned ticker (once, after the bus is wired). Startup
    /// and wake never replay overdue occurrences.
    pub async fn start_scheduled_prompts(&self) -> Result<()> {
        let mut sc = self.schedule.lock().await;
        if sc.stop.is_some() || self.is_shutting_down() {
            return Ok(());
        }
        self.tick_scheduled_prompts(self.now(), true).await?;
        let stop = CancellationToken::new();
        let (eng, s) = (self.arc(), stop.clone());
        let ticker = tokio::spawn(async move {
            let start = tokio::time::Instant::now() + Duration::from_secs(1);
            let mut every = tokio::time::interval_at(start, Duration::from_secs(1));
            every.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = s.cancelled() => return,
                    _ = every.tick() => {}
                }
                let _g = eng.schedule.lock().await;
                if !eng.is_shutting_down()
                    && let Err(e) = eng.tick_scheduled_prompts(eng.now(), false).await
                {
                    tracing::warn!("scheduled prompts: {e}");
                }
            }
        });
        *sc = ScheduleControl {
            stop: Some(stop),
            ticker: Some(ticker),
        };
        drop(sc);
        let eng = self.arc();
        tokio::spawn(async move { eng.reconcile_scheduled_workspaces().await });
        Ok(())
    }

    /// Copies schedule ids from occurrence rows onto workspaces created
    /// before the workspace carried it.
    fn stamp_scheduled_workspace_origins(&self) {
        let runs = match self.store.scheduled_prompt_runs() {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("scheduled workspace origins: {e}");
                return;
            }
        };
        for r in runs {
            if r.workspace_id.is_empty() || r.schedule_id.is_empty() {
                continue;
            }
            self.store.update_workspace(&r.workspace_id, |w| {
                if w.schedule_id.is_empty() {
                    w.schedule_id = r.schedule_id.clone();
                }
            });
        }
    }

    /// Removes settled occurrence workspaces beyond Keep last N per schedule
    /// (empty id: every schedule). Running, waiting, parked, dirty, open-PR
    /// and parent workspaces stay.
    pub async fn prune_scheduled_workspaces(&self, schedule_id: &str) {
        let keep = self.settings().scheduled_workspace_keep;
        if keep <= 0 {
            return;
        }
        let keep = usize::try_from(keep).unwrap_or(usize::MAX);
        let mut by: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut parents: HashSet<String> = HashSet::new();
        for w in self.store.workspaces() {
            if w.archived || w.is_main || w.is_pr_chat_host() {
                continue;
            }
            let pid = w.parent_id.trim();
            if !pid.is_empty() {
                parents.insert(pid.to_owned());
            }
            if w.schedule_id.is_empty() || (!schedule_id.is_empty() && w.schedule_id != schedule_id)
            {
                continue;
            }
            by.entry(w.schedule_id.clone())
                .or_default()
                .push(w.id.clone());
        }
        for ids in by.values() {
            if ids.len() <= keep {
                continue;
            }
            for id in &ids[..ids.len() - keep] {
                if parents.contains(id) || self.scheduled_occurrence_protected(id).await {
                    continue;
                }
                if let Err(e) = self.remove_workspace(id).await {
                    tracing::warn!("scheduled workspace prune {id}: {e}");
                }
            }
        }
    }

    async fn scheduled_occurrence_protected(&self, id: &str) -> bool {
        let Some(ws) = self.store.workspace(id) else {
            return true;
        };
        if ws.state == WorkspaceState::PrOpen || self.store.open_pr_by_workspace(id).is_some() {
            return true;
        }
        if self.scheduled_chat_active(id) {
            return true;
        }
        !self.workspace_discard_risk(id).await.is_empty()
    }

    async fn reconcile_scheduled_workspaces(&self) {
        self.stamp_scheduled_workspace_origins();
        self.prune_scheduled_workspaces("").await;
    }

    /// One scheduler pass; the caller holds `schedule`. Tests supply `now`.
    pub(crate) async fn tick_scheduled_prompts(
        &self,
        now: DateTime<Local>,
        startup: bool,
    ) -> Result<()> {
        let now = now.with_timezone(&Utc);
        let err = |e: recur_store::Error| EngineError(e.to_string());
        let all = self
            .store
            .due_scheduled_prompts(now, startup)
            .map_err(err)?;
        let mut runs = self.store.active_scheduled_prompt_runs().map_err(err)?;
        for r in &mut runs {
            if startup && (r.status == "running" || r.status == "waiting") {
                r.status = "interrupted".into();
                r.detail =
                    "Recur closed during this run. Inspect its workspace before retrying.".into();
                self.store.update_scheduled_prompt_run(r).map_err(err)?;
            }
            if r.status == "waiting" && !self.scheduled_chat_active(&r.workspace_id) {
                let (status, detail) = self.scheduled_chat_outcome(&r.workspace_id, "");
                r.status = status;
                r.detail = detail;
                self.store.update_scheduled_prompt_run(r).map_err(err)?;
                self.clear_scheduled_prompt_pending(&r.workspace_id);
            }
        }
        self.settle_schedule_wait();
        let now_ms = now.timestamp_millis();
        for mut p in all {
            let on_startup = p.on_startup();
            if p.paused
                || (on_startup && !startup)
                || (!on_startup && (p.next_run_at == 0 || p.next_run_at > now_ms))
            {
                continue;
            }
            let mut r = ScheduledPromptRun {
                id: new_id(),
                schedule_id: p.id.clone(),
                due_at: if on_startup { now_ms } else { p.next_run_at },
                status: "running".into(),
                ..ScheduledPromptRun::default()
            };
            let next = next_scheduled_time(&p, now).map_err(|e| EngineError(e.to_string()))?;
            p.next_run_at = scheduled_millis(next);
            // A one-minute grace tolerates timer jitter without replaying a
            // backlog; later delivery is a missed run.
            if !on_startup && (startup || now_ms - r.due_at >= 60_000) {
                r.status = "missed".into();
                r.detail =
                    "Recur was unavailable at the scheduled time. Use Run now to run once.".into();
            } else if self.schedule_busy(&p.id, &runs) {
                r.status = "skipped".into();
                r.detail = "The previous run is still active or needs your input.".into();
            }
            let claimed = self.store.claim_scheduled_prompt(&p, &r).map_err(err)?;
            if claimed
                && r.status == "running"
                && let Err(e) = self.launch_scheduled_prompt(p.clone(), r).await
            {
                tracing::warn!("scheduled prompt {}: {e}", p.id);
            }
        }
        Ok(())
    }

    fn finish_scheduled_prompt(&self, mut r: ScheduledPromptRun, status: &str, detail: &str) {
        r.status = status.to_owned();
        r.detail = detail.to_owned();
        if let Err(e) = self.store.update_scheduled_prompt_run(&r) {
            tracing::warn!("scheduled prompt outcome: {e}");
        }
    }

    /// Persists the occurrence → workspace link before allocating or
    /// sending; the worker uses the usual allocation cancel and chat Stop.
    async fn launch_scheduled_prompt(
        &self,
        p: ScheduledPrompt,
        r: ScheduledPromptRun,
    ) -> Result<String> {
        if let Err(e) = self.validate_scheduled_prompt(&p).await {
            self.finish_scheduled_prompt(r, "failed", &e.0);
            return Err(e);
        }
        let l = PromptLaunch {
            project_id: p.project_id.clone(),
            name: p.name.clone(),
            base_branch: p.base_branch.clone(),
            prompt: p.prompt.clone(),
            mode: p.mode.clone(),
            model: p.model.clone(),
            loop_id: p.loop_id.clone(),
            policy_id: p.policy_id.clone(),
            schedule_id: p.id.clone(),
            flags: p.flags.clone(),
            simplify: p.simplify,
        };
        let run = Arc::new(Mutex::new(r));
        let (link_run, done_run) = (Arc::clone(&run), Arc::clone(&run));
        let store = Arc::clone(&self.store);
        let eng = self.arc();
        let res = self
            .launch_prompt_workspace(
                l,
                Some(Box::new(move |ws_id: &str| {
                    let id = {
                        let mut r = link_run.lock();
                        r.workspace_id = ws_id.to_owned();
                        r.id.clone()
                    };
                    store
                        .link_scheduled_prompt_run(&id, ws_id)
                        .map_err(|e| EngineError(e.to_string()))
                })),
                Arc::new(move |status: &str, detail: &str| {
                    let r = done_run.lock().clone();
                    eng.finish_scheduled_prompt(r, status, detail);
                }),
            )
            .await;
        if let Err(e) = &res {
            let r = run.lock().clone();
            self.finish_scheduled_prompt(r, "failed", &e.0);
        }
        res
    }

    /// Creates the record, links it before any side effect, and runs the
    /// prompt in the background (tracked with the schedules so Shutdown
    /// joins it). `on_done` gets the outcome once the first turn settles.
    pub(crate) async fn launch_prompt_workspace(
        &self,
        l: PromptLaunch,
        link: Option<LinkFn<'_>>,
        on_done: DoneFn,
    ) -> Result<String> {
        let ws = self
            .persist_creating_workspace(&l.project_id, &l.name, &l.base_branch)
            .await?;
        if let Some(link) = link
            && let Err(e) = link(&ws.id)
        {
            self.rollback_creating(&ws.id).await;
            return Err(e);
        }
        self.store.update_workspace(&ws.id, |w| {
            w.scheduled_prompt_pending = true;
            w.schedule_id = l.schedule_id.clone();
        });
        let alloc = Arc::new(CreatingAlloc {
            cancel: CancellationToken::new(),
            done: CancellationToken::new(),
        });
        self.creating
            .lock()
            .insert(ws.id.clone(), Arc::clone(&alloc));
        let eng = self.arc();
        let id = ws.id.clone();
        self.tasks.schedules.spawn(async move {
            let res = eng.allocate_workspace_worktree(&alloc.cancel, &ws).await;
            alloc.cancel.cancel();
            eng.creating.lock().remove(&ws.id);
            alloc.done.cancel();
            if eng.is_shutting_down() {
                return;
            }
            match res {
                Err(e) => {
                    eng.fail_creating(&ws.id, &e.0);
                    on_done("failed", &e.0);
                }
                Ok(()) => {
                    eng.ensure_pair_agent(&ws.id, PAIR_AGENT_CODE, &l.model);
                    eng.remember_workspace_chat_selection(
                        &ws.id,
                        PAIR_AGENT_CODE,
                        &l.model,
                        Some(&l.flags),
                        &l.mode,
                        l.simplify,
                    )
                    .await;
                    let send = || {
                        eng.send_workspace_message(
                            &ws.id,
                            PAIR_AGENT_CODE,
                            &l.prompt,
                            &l.model,
                            Some(&l.flags),
                            &[],
                            &l.mode,
                            "",
                        )
                    };
                    let reply = if l.mode == CHAT_MODE_LOOP {
                        // Loop kickoff returns after registering its walk:
                        // serialize that with Shutdown's snapshot.
                        let _g = eng.schedule.lock().await;
                        if eng.is_shutting_down() {
                            return;
                        }
                        eng.remember_chat_loop_policy(
                            &ws.id,
                            PAIR_AGENT_CODE,
                            &l.loop_id,
                            &l.policy_id,
                        )
                        .await;
                        send().await
                    } else {
                        send().await
                    };
                    if eng.is_shutting_down() {
                        return;
                    }
                    let (status, detail) = eng.scheduled_chat_outcome(&ws.id, &reply);
                    on_done(&status, &detail);
                    if status == "waiting" {
                        eng.schedule_wait.lock().insert(ws.id.clone());
                        if !l.schedule_id.is_empty() {
                            eng.prune_scheduled_workspaces(&l.schedule_id).await;
                        }
                        return;
                    }
                }
            }
            eng.clear_scheduled_prompt_pending(&ws.id);
            if !l.schedule_id.is_empty() {
                eng.prune_scheduled_workspaces(&l.schedule_id).await;
            }
        });
        Ok(id)
    }

    fn scheduled_chat_outcome(&self, workspace_id: &str, reply: &str) -> (String, String) {
        let pair = |s: &str, d: &str| (s.to_owned(), d.to_owned());
        if self.scheduled_chat_active(workspace_id) {
            return pair(
                "waiting",
                "Run is continuing or waiting for input in the workspace.",
            );
        }
        if let Some(ws) = self.store.workspace(workspace_id)
            && ws.has_run()
        {
            return match ws.state {
                WorkspaceState::Failed => pair("failed", &ws.last_error),
                WorkspaceState::Stopped => pair("stopped", "Loop stopped."),
                _ => pair("finished", "Open the workspace to inspect the loop result."),
            };
        }
        if reply == CHAT_STOPPED_REPLY {
            return pair("stopped", reply);
        }
        let reply = if reply.trim().is_empty() {
            "Open the workspace for details."
        } else {
            reply
        };
        if let Some(sess) = self
            .store
            .session_for_loop_agent(workspace_id, PAIR_AGENT_CODE)
        {
            let messages = match self.store.session_transcript(&sess.id) {
                Ok(m) => m.unwrap_or_default(),
                Err(e) => return pair("failed", &e.to_string()),
            };
            if let Some(m) = messages.iter().rev().find(|m| m.role == "assistant") {
                if m.stream_status == recur_domain::STREAM_ERROR {
                    return pair("failed", reply);
                }
                return pair("finished", "Open the workspace to inspect the result.");
            }
        }
        pair("failed", reply)
    }

    /// Includes the gap between a mode switch and its remount, and parked
    /// questions with no live provider process.
    pub(crate) fn scheduled_chat_active(&self, workspace_id: &str) -> bool {
        if self.running_loop_by_id(workspace_id).is_some() {
            return true;
        }
        if let Some(ws) = self.store.workspace(workspace_id)
            && ws.has_run()
            && (run_active(&ws.state)
                || ws.state == WorkspaceState::Escalated
                || (ws.state == WorkspaceState::Paused && !scheduled_pause_settled(&ws.phase)))
        {
            return true;
        }
        self.workspace_has_live_chat(workspace_id)
            || self.ask_user_workspace_busy(workspace_id)
            || self.workspace_has_live_continue(workspace_id)
    }

    fn clear_scheduled_prompt_pending(&self, workspace_id: &str) {
        if workspace_id.is_empty() {
            return;
        }
        self.schedule_wait.lock().remove(workspace_id);
        self.store
            .update_workspace(workspace_id, |w| w.scheduled_prompt_pending = false);
    }

    fn settle_schedule_wait(&self) {
        let ids: Vec<String> = self.schedule_wait.lock().iter().cloned().collect();
        for id in ids {
            if id.is_empty() || self.scheduled_chat_active(&id) {
                continue;
            }
            self.clear_scheduled_prompt_pending(&id);
        }
    }
}
