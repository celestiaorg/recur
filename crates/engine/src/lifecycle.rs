//! Engine-wide control: Pause and Stop of a workspace's walks, Shutdown, and
//! restart recovery of loop states.

use std::time::Duration;

use recur_domain::{Checkpoint, Group, WorkspaceState};

use crate::Engine;
use crate::loop_handle::Command;
use crate::loop_registry::RunningLoop;
use crate::loops::escalation::esc_inbox_id;
use crate::loops::resume::retry_inbox_id;
use crate::publish::{interrupted, status_label, terminal};
use crate::text::rfc3339;

/// The resting phase Recover writes on a loop that was in flight when the
/// app last exited. AutoResume keys on exactly this.
pub const PHASE_INTERRUPTED: &str = "interrupted";
/// The phase of an interrupted scheduled run.
pub const PHASE_SCHEDULED_INTERRUPTED: &str = "scheduled run interrupted";
/// The phase of an escalation answered before the walk could take it.
pub const PHASE_ANSWERED_WAITING: &str = "answered: waiting to resume";

impl Engine {
    /// Pauses the workspace's running walks (at their next checkpoint). A
    /// workspace with no walk is left alone. Serialized with Shutdown's
    /// snapshot: once quit started, the pause is persisted here.
    pub fn pause(&self, id: &str) {
        let running = self.running_loops_on(id);
        let t = self.teardown.lock();
        for rl in &running {
            rl.handle.send(Command::Pause);
        }
        if *t {
            self.persist_pause(id);
        }
    }

    /// Stops the workspace's walks (cancelling their agents). With no walk,
    /// a non-terminal workspace becomes stopped (serialized with Resume
    /// under `dispatch_mu`, so a racing Resume cannot resurrect it).
    pub async fn stop(&self, id: &str) {
        let running = self.running_loops_on(id);
        if !running.is_empty() {
            self.stop_running(id, &running);
            return;
        }
        let _d = self.dispatch_mu.lock().await;
        let again = self.running_loops_on(id);
        if !again.is_empty() {
            self.stop_running(id, &again);
            return;
        }
        self.persist_stop(id);
    }

    fn stop_running(&self, id: &str, running: &[RunningLoop]) {
        // Atomic with Shutdown's snapshot: either Stop marks the walk first
        // or the snapshot preserved it and the stop is persisted here.
        let t = self.teardown.lock();
        for rl in running {
            rl.handle.send(Command::Stop);
            rl.cancel.cancel();
        }
        if *t {
            self.persist_stop(id);
        }
    }

    /// Records StateStopped when the workspace is still non-terminal, and
    /// retires escalations no walk will unwind from.
    pub fn persist_stop(&self, id: &str) {
        if let Some(rec) = self.store.workspace(id)
            && !terminal(&rec.state)
        {
            self.set_workspace_state(id, WorkspaceState::Stopped, "stopped");
            self.retire_open_escalations(id);
        }
    }

    /// Records StatePaused when Shutdown already took its snapshot and the
    /// workspace is in flight (escalated/queued/terminal stay as they are).
    pub fn persist_pause(&self, id: &str) {
        if let Some(rec) = self.store.workspace(id)
            && interrupted(&rec.state)
        {
            self.set_workspace_state(id, WorkspaceState::Paused, "paused");
        }
    }

    /// App quit: stop schedules, the watcher, in-flight creates, walks and
    /// chats; join them for up to 5 s; then close the MCP server, providers
    /// and keep-awake. Chats cancelled here keep their streamed text.
    pub async fn shutdown(&self) {
        *self.teardown.lock() = true;
        let ticker = {
            let mut sc = self.schedule.lock().await;
            if let Some(stop) = &sc.stop {
                stop.cancel();
            }
            sc.ticker.take()
        };
        if let Some(t) = ticker {
            let _ = t.await;
        }
        self.watch_workspace("");
        for alloc in self.creating.lock().values() {
            alloc.cancel.cancel();
        }
        for rl in self.all_running_loops() {
            rl.cancel.cancel();
        }
        let mut ended = Vec::new();
        for (_, turn) in self.live_chat_turns() {
            turn.cancel_token().cancel();
            ended.push(turn.ended());
        }
        self.tasks.schedules.close();
        self.tasks.loops.close();
        self.tasks.teardown.close();
        self.tasks.chat_turns.close();
        let join = async {
            self.tasks.schedules.wait().await;
            self.tasks.loops.wait().await;
            self.tasks.teardown.wait().await;
            for e in &ended {
                e.cancelled().await;
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(5), join).await;
        if *self.mcp_started.lock().await {
            let _ = self.mcp.close().await;
        }
        for p in self.providers.lifecycle() {
            p.shutdown().await;
        }
        // Close, not stop: a late release from an unwinding agent must not
        // respawn caffeinate.
        self.keep_awake.close();
    }

    /// Reconciles loop states persisted live at the last exit (crash,
    /// force-quit): in-flight runs become paused "interrupted" in Needs you
    /// (AutoResume picks them up), stopped/failed runs leave Needs you, and
    /// an escalation answered before its walk took it rests paused. Then
    /// interrupted chats are healed and queued. Call once at startup.
    pub fn recover(&self) {
        let now = rfc3339(chrono::Local::now());
        for l in self.store.workspaces() {
            if interrupted(&l.state)
                || (l.scheduled_prompt_pending && l.state == WorkspaceState::Queued)
            {
                let scheduled = l.scheduled_prompt_pending;
                let Some(updated) = self.store.update_workspace(&l.id, |lp| {
                    // Keep where it was: that phase is the resume position
                    // (D55). Older builds may not have checkpointed.
                    if lp.checkpoint.is_none() {
                        lp.checkpoint = Some(Checkpoint {
                            state: lp.state.clone(),
                            iteration: lp.iteration,
                            at: now.clone(),
                            ..Checkpoint::default()
                        });
                    }
                    lp.state = WorkspaceState::Paused;
                    lp.phase = if scheduled {
                        PHASE_SCHEDULED_INTERRUPTED
                    } else {
                        PHASE_INTERRUPTED
                    }
                    .to_owned();
                    lp.group = Group::Needs;
                    lp.status_label = status_label(&WorkspaceState::Paused);
                    lp.last = "interrupted".into();
                }) else {
                    continue;
                };
                self.publish_loop_updated(
                    &updated.id,
                    updated.state.clone(),
                    &updated.phase,
                    updated.iteration,
                );
                continue;
            }
            if l.state != WorkspaceState::Escalated {
                // Terminal runs left in Needs you: clear the stale badge.
                if matches!(l.state, WorkspaceState::Stopped | WorkspaceState::Failed)
                    && l.group == Group::Needs
                    && let Some(updated) = self.store.update_workspace(&l.id, |lp| {
                        lp.group = Group::Active;
                        if lp.elapsed_label == "running" {
                            lp.elapsed_label = lp.state.as_str().to_owned();
                        }
                    })
                {
                    self.store
                        .remove_inbox_by_target(&format!("workspace:{}", l.id));
                    self.store.remove_inbox_item(&retry_inbox_id(&l.id));
                    self.publish_loop_updated(
                        &updated.id,
                        updated.state.clone(),
                        &updated.phase,
                        updated.iteration,
                    );
                }
                continue;
            }
            if self.loop_key_running(&l.id) {
                continue;
            }
            // Still waiting on the human: leave it.
            if let Some(esc) = self.store.escalation(&l.id)
                && esc.status != "answered"
            {
                continue;
            }
            let Some(updated) = self.store.update_workspace(&l.id, |lp| {
                lp.state = WorkspaceState::Paused;
                lp.phase = PHASE_ANSWERED_WAITING.into();
                lp.group = Group::Active;
                lp.status_label = status_label(&WorkspaceState::Paused);
                lp.last = "answered".into();
            }) else {
                continue;
            };
            self.store
                .remove_inbox_by_target(&format!("workspace:{}", l.id));
            if let Some(esc) = self.store.escalation(&l.id) {
                self.store
                    .remove_inbox_item(&esc_inbox_id(&l.id, &esc.agent_code));
            }
            self.publish_loop_updated(
                &updated.id,
                updated.state.clone(),
                &updated.phase,
                updated.iteration,
            );
        }
        self.recover_interrupted_chats();
        self.publish_inbox_updated();
    }
}

#[cfg(test)]
mod tests;
