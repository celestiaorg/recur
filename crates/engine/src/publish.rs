//! Store write-through and bus helpers: workspace state transitions, the
//! activity feed, the durable event log, agent records, and the status-bar
//! rollup. Every slice publishes through these.

use recur_agent::display::FEED_INPUT_LIMIT;
use recur_domain::{
    Agent, AgentStep, Checkpoint, FEED_KIND_DEVIATION, FEED_KIND_OUTCOME, FEED_KIND_TOOL, FeedItem,
    Group, Plan, StoredEvent, WorkspaceState,
};

use crate::Engine;
use crate::bus::{
    AgentStepEvent, AgentUpdated, EngineEvent, InboxUpdated, Notify, PrUpdated, WorkspaceEvent,
    WorkspaceUpdated,
};
use crate::codes::MAESTRO_AGENT_CODE;
use crate::text::{rfc3339, truncate, unix_millis, unix_nanos};

/// The human label for a state.
pub fn status_label(s: &WorkspaceState) -> String {
    match s {
        WorkspaceState::Queued => "Queued",
        WorkspaceState::Planning => "Planning",
        WorkspaceState::Coding => "Coding",
        WorkspaceState::Reviewing => "Reviewing",
        WorkspaceState::Verifying => "Verifying",
        WorkspaceState::PrOpen => "Ready",
        WorkspaceState::Paused => "Paused",
        WorkspaceState::Stopped => "Stopped",
        WorkspaceState::Failed => "Failed",
        other => return other.as_str().to_owned(),
    }
    .to_owned()
}

/// The coarse progress ring fill for a state.
pub fn progress_for(s: &WorkspaceState) -> f64 {
    match s {
        WorkspaceState::Planning => 0.15,
        WorkspaceState::Coding => 0.5,
        WorkspaceState::Reviewing => 0.7,
        WorkspaceState::Verifying => 0.85,
        WorkspaceState::PrOpen => 1.0,
        _ => 0.0,
    }
}

/// Real progress off structured plan steps while coding or reviewing.
pub fn run_progress(state: &WorkspaceState, p: Option<&Plan>) -> Option<f64> {
    let p = p?;
    if !matches!(state, WorkspaceState::Coding | WorkspaceState::Reviewing) {
        return None;
    }
    p.progress().map(|frac| 0.15 + 0.7 * frac)
}

/// An in-flight state that has no task after a restart.
pub fn interrupted(s: &WorkspaceState) -> bool {
    matches!(
        s,
        WorkspaceState::Planning
            | WorkspaceState::Coding
            | WorkspaceState::Reviewing
            | WorkspaceState::Verifying
    )
}

/// A settled outcome a late control command must not overwrite.
pub fn terminal(s: &WorkspaceState) -> bool {
    matches!(
        s,
        WorkspaceState::PrOpen
            | WorkspaceState::Merged
            | WorkspaceState::Failed
            | WorkspaceState::Stopped
    )
}

impl Engine {
    /// Sets state + phase (with label, progress, elapsed label and group
    /// rules), checkpoints every in-flight transition, persists, and
    /// publishes `workspace.updated`.
    pub fn set_workspace_state(&self, id: &str, state: WorkspaceState, phase: &str) {
        let prev = self.store.workspace(id).unwrap_or_default();
        let at = rfc3339(chrono::Local::now());
        let Some(l) = self.store.update_workspace(id, |l| {
            l.state = state.clone();
            l.phase = phase.to_owned();
            l.status_label = status_label(&state);
            l.progress = progress_for(&state);
            if let Some(p) = run_progress(&state, l.plan.as_ref()) {
                l.progress = p;
            }
            if interrupted(&state) {
                let (prev_id, prev_resume) = l
                    .checkpoint
                    .as_ref()
                    .map(|c| (c.phase_id.clone(), c.resume_phase_id.clone()))
                    .unwrap_or_default();
                l.checkpoint = Some(Checkpoint {
                    state: state.clone(),
                    iteration: l.iteration,
                    at: at.clone(),
                    phase_id: prev_id,
                    resume_phase_id: prev_resume,
                });
            }
            match state {
                WorkspaceState::Stopped | WorkspaceState::Failed => {
                    l.last = state.as_str().to_owned();
                    l.elapsed_label = state.as_str().to_owned();
                    // Leaving "needs you" unless it failed retryably.
                    if state == WorkspaceState::Failed && l.retryable {
                        l.group = Group::Needs;
                    } else if l.group == Group::Needs {
                        l.group = Group::Active;
                    }
                    // A stopped loop drops its resume position; a failed one
                    // keeps it for Retry (D56).
                    if state == WorkspaceState::Stopped {
                        l.checkpoint = None;
                    }
                }
                WorkspaceState::PrOpen | WorkspaceState::Merged => {
                    l.elapsed_label = state.as_str().to_owned();
                    l.checkpoint = None;
                }
                WorkspaceState::Paused | WorkspaceState::Escalated => {
                    l.elapsed_label = "paused".into();
                }
                WorkspaceState::Queued => l.elapsed_label = "queued".into(),
                _ => l.elapsed_label = "running".into(),
            }
        }) else {
            return;
        };
        self.publish_loop_updated(id, l.state.clone(), &l.phase, l.iteration);
        if prev.state != l.state || prev.phase != l.phase {
            let mut sum = l.last.trim().to_owned();
            if sum.is_empty() {
                sum = l.state.as_str().to_owned();
            }
            self.touch_workspace_activity(id, &sum, self.now());
        }
        if matches!(
            state,
            WorkspaceState::Stopped
                | WorkspaceState::Failed
                | WorkspaceState::Paused
                | WorkspaceState::Escalated
                | WorkspaceState::PrOpen
                | WorkspaceState::Merged
        ) {
            self.publish_resource();
        }
    }

    /// Moves a workspace between list buckets and publishes.
    pub fn mark_group(&self, id: &str, g: Group) {
        if let Some(l) = self.store.update_workspace(id, |l| l.group = g) {
            self.publish_loop_updated(id, l.state.clone(), &l.phase, l.iteration);
        }
    }

    /// Refreshes the sidebar DiffStat from git after a commit, falling back
    /// to accumulating the commit's numstat when the worktree cannot be
    /// measured.
    pub async fn update_loop_diff(&self, id: &str, add: i64, del: i64) {
        if self.sync_workspace_diff(id, true).await {
            return;
        }
        if let Some(l) = self.store.update_workspace(id, |ws| {
            ws.diff.add += add;
            ws.diff.del += del;
        }) {
            self.publish_loop_updated(id, l.state.clone(), &l.phase, l.iteration);
        }
    }

    /// Prepends an engine-authored feed item, persists it and publishes
    /// `workspace.event`.
    pub fn emit_workspace_event(
        &self,
        workspace_id: &str,
        code: &str,
        name: &str,
        text_html: &str,
    ) {
        self.emit_workspace_event_item(
            workspace_id,
            FeedItem {
                id: format!("ev-{}", unix_nanos()),
                agent_code: code.to_owned(),
                name: name.to_owned(),
                text: text_html.to_owned(),
                time: "now".into(),
                ..FeedItem::default()
            },
        );
    }

    /// Prepends a fully formed feed item (tool rows carry ToolUseID so a
    /// later result can patch them). The typed kind is stamped here.
    pub fn emit_workspace_event_item(&self, workspace_id: &str, mut item: FeedItem) {
        if item.id.is_empty() {
            item.id = format!("ev-{}", unix_nanos());
        }
        if item.time.is_empty() {
            item.time = "now".into();
        }
        if item.kind.is_empty() {
            item.kind = if item.is_deviation {
                FEED_KIND_DEVIATION
            } else if !item.tool_use_id.is_empty() {
                FEED_KIND_TOOL
            } else {
                FEED_KIND_OUTCOME
            }
            .to_owned();
        }
        if item.click_agent.is_empty()
            && !item.agent_code.is_empty()
            && item.agent_code != MAESTRO_AGENT_CODE
        {
            item.click_agent = item.agent_code.clone();
        }
        self.stamp_feed_chat_code(workspace_id, &mut item);
        self.store.prepend_feed(workspace_id, item.clone());
        self.publish_workspace_event(workspace_id, item);
    }

    /// Publishes a typed cockpit card under a stable id: the first emit
    /// prepends, later ones replace.
    pub fn emit_workspace_card(&self, workspace_id: &str, mut item: FeedItem) {
        if item.id.is_empty() || item.card.is_empty() {
            return;
        }
        if item.time.is_empty() {
            item.time = "now".into();
        }
        if item.kind.is_empty() {
            item.kind = FEED_KIND_OUTCOME.into();
        }
        self.stamp_feed_chat_code(workspace_id, &mut item);
        self.store.upsert_feed(workspace_id, item.clone());
        self.publish_workspace_event(workspace_id, item);
    }

    fn publish_workspace_event(&self, workspace_id: &str, item: FeedItem) {
        let ev = EngineEvent::WorkspaceEvent(WorkspaceEvent {
            event: item,
            workspace_id: workspace_id.to_owned(),
        });
        self.persist_event(workspace_id, "", &ev);
        self.bus.publish(ev);
    }

    /// Fills ChatCode from the one running walk with a conductor, or the
    /// workspace conductor when none runs, so PA2/PA3 tabs own their cards.
    pub fn stamp_feed_chat_code(&self, workspace_id: &str, item: &mut FeedItem) {
        if !item.chat_code.is_empty() {
            return;
        }
        let mut owner = String::new();
        let mut n = 0;
        for rl in self.running_loops_on(workspace_id) {
            let code = rl.handle.chat_code();
            let code = code.trim();
            if code.is_empty() {
                continue;
            }
            n += 1;
            owner = code.to_owned();
        }
        match n {
            1 => item.chat_code = owner,
            0 => {
                if let Some(ws) = self.store.workspace(workspace_id) {
                    item.chat_code = ws.loop_chat_code.trim().to_owned();
                }
            }
            _ => {}
        }
    }

    /// Patches the feed row for `tool_use_id` with its result and re-emits.
    pub fn attach_tool_result(
        &self,
        workspace_id: &str,
        tool_use_id: &str,
        result: &str,
        is_error: bool,
    ) {
        if tool_use_id.is_empty() {
            return;
        }
        let Some(item) = self
            .store
            .patch_feed_by_tool_use_id(workspace_id, tool_use_id, |it| {
                it.tool_result = truncate(result, FEED_INPUT_LIMIT);
                it.is_tool_error = is_error;
            })
        else {
            return;
        };
        self.publish_workspace_event(workspace_id, item);
    }

    /// Appends a DEVIATION item (PRD §7.4: rule, departure, why).
    pub fn emit_deviation(&self, workspace_id: &str, text_html: &str, rule: &str, why: &str) {
        let item = FeedItem {
            id: format!("ev-{}", unix_nanos()),
            name: "Maestro".into(),
            text: text_html.to_owned(),
            time: "now".into(),
            kind: FEED_KIND_DEVIATION.into(),
            is_deviation: true,
            rule: rule.to_owned(),
            why: why.to_owned(),
            ..FeedItem::default()
        };
        self.store.prepend_feed(workspace_id, item.clone());
        self.publish_workspace_event(workspace_id, item);
    }

    /// Persists and publishes the loop shape of `workspace.updated`.
    pub fn publish_loop_updated(
        &self,
        workspace_id: &str,
        state: WorkspaceState,
        phase: &str,
        iter: i64,
    ) {
        let w = self.store.workspace(workspace_id).unwrap_or_default();
        let ev = EngineEvent::WorkspaceUpdated(WorkspaceUpdated::from_workspace(
            workspace_id,
            state,
            phase,
            iter,
            &w,
        ));
        self.persist_event(workspace_id, "", &ev);
        self.bus.publish(ev);
    }

    /// Appends an event to the durable log (D13). Feed reconstruction reads
    /// these payloads back, so they are a storage format (R10).
    pub fn persist_event(&self, workspace_id: &str, agent_id: &str, ev: &EngineEvent) {
        let Ok(raw) = serde_json::to_string(ev) else {
            return;
        };
        let _ = self.store.append_event(&StoredEvent {
            id: 0,
            workspace_id: workspace_id.to_owned(),
            agent_id: agent_id.to_owned(),
            r#type: ev.name().to_owned(),
            payload_json: raw,
            ts: unix_millis(),
        });
    }

    /// Writes an agent record. MA is update-only (creating it is the
    /// Maestro's open/dispatch path), so a planner upsert cannot resurrect a
    /// closed tab.
    pub fn upsert_agent(&self, workspace_id: &str, a: Agent) {
        if a.code == MAESTRO_AGENT_CODE {
            let mut wrote = false;
            let next = a.clone();
            self.store.update_agent(workspace_id, &a.code, |ag| {
                *ag = next;
                wrote = true;
            });
            if !wrote {
                return;
            }
        } else {
            self.store.upsert_agent(workspace_id, a.clone());
        }
        self.finish_agent_write(workspace_id, &a);
    }

    /// Writes an agent record unconditionally (creates MA too).
    pub fn put_agent(&self, workspace_id: &str, a: Agent) {
        self.store.upsert_agent(workspace_id, a.clone());
        self.finish_agent_write(workspace_id, &a);
    }

    fn finish_agent_write(&self, workspace_id: &str, a: &Agent) {
        self.store.update_workspace(workspace_id, |l| {
            let codes = l.agent_codes.get_or_insert_with(Vec::new);
            if !codes.iter().any(|c| c == &a.code) {
                codes.push(a.code.clone());
            }
        });
        self.bus.publish(EngineEvent::AgentUpdated(AgentUpdated {
            agent_id: a.id.clone(),
            code: None,
            status: a.status.clone(),
            surface: None,
            workspace_id: workspace_id.to_owned(),
        }));
    }

    /// Records the agent's provider session id (no-op when empty).
    pub fn set_agent_session(&self, workspace_id: &str, code: &str, session_id: &str) {
        if session_id.is_empty() {
            return;
        }
        self.store
            .update_agent(workspace_id, code, |a| a.session_id = session_id.to_owned());
    }

    pub fn clear_agent_session(&self, workspace_id: &str, code: &str) {
        self.store
            .update_agent(workspace_id, code, |a| a.session_id.clear());
    }

    /// Appends one "see into an agent" step and publishes `agent.step`.
    pub fn append_agent_step(&self, workspace_id: &str, code: &str, step: AgentStep) {
        let s = step.clone();
        self.store.update_agent(workspace_id, code, |a| {
            a.steps.get_or_insert_with(Vec::new).push(s);
        });
        self.bus.publish(EngineEvent::AgentStep(AgentStepEvent {
            agent_id: format!("{workspace_id}-{code}"),
            step,
            workspace_id: workspace_id.to_owned(),
        }));
    }

    /// Recomputes the status-bar rollup and publishes `resource.updated`.
    /// The git warning is published but never persisted.
    pub fn publish_resource(&self) {
        let mut r = self.store.resource();
        r.loops_running = self.gov.loops_running();
        r.needs_you = i64::try_from(self.store.count_group(&Group::Needs)).unwrap_or(i64::MAX);
        r.iters_today = self.iters_base + self.gov.iters();
        r.git_warning.clear();
        self.store.set_resource(r.clone());
        r.git_warning = self.git_warning.clone();
        self.bus.publish(EngineEvent::ResourceUpdated(r));
    }

    /// `inbox.updated` with the live unread count.
    pub fn publish_inbox_updated(&self) {
        let unread = i64::try_from(self.store.unread_count()).unwrap_or(i64::MAX);
        self.bus
            .publish(EngineEvent::InboxUpdated(InboxUpdated { unread }));
    }

    /// An OS notification; a click routes to `target` ("" = none).
    pub fn publish_notify(&self, title: &str, body: &str, target: &str) {
        self.bus.publish(EngineEvent::Notify(Notify {
            body: body.to_owned(),
            target: (!target.is_empty()).then(|| target.to_owned()),
            title: title.to_owned(),
        }));
    }

    /// `pr.updated` for a PR row.
    pub fn publish_pr_updated(&self, pr_id: &str) {
        self.emit_pr_updated(pr_id, false);
    }

    /// `pr.updated`; `pending_only` marks a local draft-comment or thread
    /// write the UI merges without refetching.
    pub fn emit_pr_updated(&self, pr_id: &str, pending_only: bool) {
        let Some(pr) = self.store.pr(pr_id) else {
            return;
        };
        self.bus.publish(EngineEvent::PrUpdated(PrUpdated::from_pr(
            pr_id,
            &pr,
            pending_only,
        )));
    }
}

#[cfg(test)]
#[path = "publish_tests.rs"]
mod tests;
