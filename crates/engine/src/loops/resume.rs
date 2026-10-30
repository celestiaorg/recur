//! Resume-from-checkpoint (D55, PRD §6.5): the durable checkpoint decides where
//! a re-dispatched walk re-enters, and each role's prior provider session
//! continues on its first post-resume turn. Also the failure record (D56),
//! Retry, Restart, idle re-dispatch, and AutoResume.
//!
//! What is deliberately NOT resumed: the killed turn itself. Its phase is
//! re-run with the session resumed, and the engine re-counts findings and
//! re-declares convergence, so correctness never trusts pre-restart state.

use std::collections::{HashMap, HashSet};

use recur_domain::{InboxItem, Workspace, WorkspaceState};

use crate::Engine;
use crate::lifecycle::PHASE_INTERRUPTED;
use crate::loops::Walk;
use crate::text::{html_escape, truncate};

/// A decoded checkpoint: the state a re-dispatched walk re-enters, the
/// convergence iteration, and whether that iteration's coding turn already
/// committed (so resume reviews the commit instead of re-coding it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumePoint {
    pub state: WorkspaceState,
    pub iteration: i64,
    pub skip_coder: bool,
    /// Role code → the session it ran when interrupted; its FIRST
    /// post-resume turn continues it.
    pub sessions: HashMap<String, String>,
    /// Roles whose first post-resume turn already happened.
    pub consumed: HashSet<String>,
    /// Set once the first converge read iteration / skip_coder, so a later
    /// re-entry does not restart numbering or re-skip the coder.
    pub start_consumed: bool,
}

/// Where a re-dispatched walk starts, or `None` for a fresh start (no
/// checkpoint, or one outside the re-enterable states). Reviewing and
/// verifying collapse onto converge with the coder skipped.
pub fn resume_point_for(rec: &Workspace, sessions: HashMap<String, String>) -> Option<ResumePoint> {
    let cp = rec.checkpoint.as_ref()?;
    let iteration = cp.iteration.max(1);
    let (state, skip_coder) = match cp.state {
        WorkspaceState::Planning => (WorkspaceState::Planning, false),
        WorkspaceState::Coding => (WorkspaceState::Coding, false),
        WorkspaceState::Reviewing | WorkspaceState::Verifying => (WorkspaceState::Coding, true),
        _ => return None,
    };
    Some(ResumePoint {
        state,
        iteration,
        skip_coder,
        sessions,
        ..ResumePoint::default()
    })
}

impl Walk {
    /// Whether this is the FIRST turn `code` takes after a resume, and the
    /// session to continue ("" when none was captured). One-shot per role.
    pub(crate) fn resumed_role(&mut self, code: &str) -> (String, bool) {
        let Some(r) = self.resume.as_mut() else {
            return (String::new(), false);
        };
        if !r.consumed.insert(code.to_owned()) {
            return (String::new(), false);
        }
        (r.sessions.get(code).cloned().unwrap_or_default(), true)
    }
}

impl Engine {
    /// Each role code's most recent provider session for a loop.
    pub fn agent_sessions(&self, workspace_id: &str) -> HashMap<String, String> {
        self.store
            .agents(workspace_id)
            .into_iter()
            .filter(|a| !a.session_id.is_empty())
            .map(|a| (a.code, a.session_id))
            .collect()
    }

    /// Persists why a loop failed and whether Retry applies (D56). Call
    /// before setting StateFailed.
    pub fn record_failure(&self, id: &str, reason: &str, retryable: bool) {
        self.store.update_workspace(id, |l| {
            l.last_error = reason.to_owned();
            l.retryable = retryable;
        });
    }

    /// Parks a loop that hit a transient provider failure: failed, but
    /// keeping its checkpoint, in Needs you with an inbox row and a
    /// notification so the human can clear it and Retry.
    pub fn fail_retryable(&self, id: &str, reason: &str) {
        self.record_failure(id, reason, true);
        self.set_workspace_state(id, WorkspaceState::Failed, &format!("stopped: {reason}"));
        self.emit_workspace_event(
            id,
            "MA",
            "Maestro",
            &format!(
                "Loop stopped on a transient error. Retry when it clears: {}",
                html_escape(reason)
            ),
        );
        let Some(rec) = self.store.workspace(id) else {
            return;
        };
        self.store.add_inbox_item(InboxItem {
            id: format!("ib-retry-{id}"),
            group: "needs".into(),
            unread: true,
            kind: "pause".into(),
            color: "var(--coral)".into(),
            title: format!("<b>{}</b> stopped: retry when it clears", html_escape(id)),
            sub: format!(
                "{} · {}",
                html_escape(&truncate(&rec.short, 64)),
                html_escape(&truncate(reason, 64))
            ),
            time: "now".into(),
            action: "Retry".into(),
            action_class: "urgent".into(),
            target: format!("workspace:{id}"),
            workspace_id: id.to_owned(),
        });
        self.publish_inbox_updated();
        self.publish_notify(
            "A loop stopped: retry available",
            &truncate(reason, 120),
            &format!("workspace:{id}"),
        );
    }

    /// Re-runs a deliberately stopped (or terminally failed) loop over its
    /// worktree, fresh from the current tree and steered by the recorded
    /// plan. Anything else is refused with the reason named.
    pub async fn restart(&self, id: &str) -> String {
        if self.running_loop_by_id(id).is_some() {
            return "Refused: the loop is already running.".into();
        }
        let Some(rec) = self.store.workspace(id) else {
            return "Refused: that workspace no longer exists.".into();
        };
        if !rec.has_run() {
            return "Refused: this workspace has no loop to restart; engage Loop mode in the chat first.".into();
        }
        match rec.state {
            WorkspaceState::Stopped | WorkspaceState::Failed => {}
            WorkspaceState::PrOpen => {
                return "Refused: the loop settled with an open PR; Send Back on the PR (talks to Maestro), or reenter_phase.".into();
            }
            WorkspaceState::Merged => {
                return "Refused: this loop's PR is merged; start new work instead.".into();
            }
            other => {
                return format!(
                    "Refused: the loop is {}; Resume covers that state.",
                    other.as_str()
                );
            }
        }
        self.dispatch_idle_walk(id).await
    }

    /// Why a settled walk cannot be re-dispatched ("" when it can).
    /// reenter_phase / run_phase are legal at idle.
    pub fn idle_walk_refuse(&self, id: &str) -> String {
        if self.running_loop_by_id(id).is_some() {
            return "Refused: the loop is already running.".into();
        }
        let Some(rec) = self.store.workspace(id) else {
            return "Refused: that workspace no longer exists.".into();
        };
        if !rec.has_run() {
            return "Refused: this workspace has no loop to restart; engage Loop mode in the chat first.".into();
        }
        match rec.state {
            WorkspaceState::Stopped
            | WorkspaceState::Failed
            | WorkspaceState::Paused
            | WorkspaceState::PrOpen => String::new(),
            WorkspaceState::Merged => {
                "Refused: this loop's PR is merged; start new work instead.".into()
            }
            other => format!(
                "Refused: the loop is {}; wait until it settles.",
                other.as_str()
            ),
        }
    }

    pub(crate) async fn dispatch_idle_walk(&self, id: &str) -> String {
        let d = self.dispatch_mu.lock().await;
        self.dispatch_idle_walk_locked(&d, id).await
    }

    /// Re-dispatches a settled loop from its checkpoint. Caller holds
    /// `dispatch_mu`.
    pub(crate) async fn dispatch_idle_walk_locked(
        &self,
        held: &tokio::sync::MutexGuard<'_, ()>,
        id: &str,
    ) -> String {
        let reason = self.idle_walk_refuse(id);
        if !reason.is_empty() {
            return reason;
        }
        self.store.update_workspace(id, |l| {
            l.last_error.clear();
            l.retryable = false;
        });
        // Park in the resting state Resume re-dispatches.
        self.set_workspace_state(id, WorkspaceState::Paused, "restarting");
        self.store
            .remove_inbox_by_target(&format!("workspace:{id}"));
        self.emit_workspace_event(
            id,
            "MA",
            "Maestro",
            "Restarting the loop over the current worktree.",
        );
        self.resume_not_running_locked(held, id, "").await;
        self.publish_inbox_updated();
        "Loop restarting: it continues from the current worktree, steered by the recorded plan."
            .into()
    }

    /// Re-runs a loop that failed on a transient error from its last
    /// checkpoint (D56). A non-retryable failure is a no-op.
    pub async fn retry(&self, id: &str) {
        let Some(rec) = self.store.workspace(id) else {
            return;
        };
        if rec.state == WorkspaceState::Failed && !rec.retryable {
            return;
        }
        self.store.update_workspace(id, |l| {
            l.last_error.clear();
            l.retryable = false;
        });
        // Out of failed before Resume re-reads under dispatch_mu; the
        // checkpoint survives, so the re-dispatch re-enters where it stopped.
        self.set_workspace_state(
            id,
            WorkspaceState::Paused,
            "retrying from the last checkpoint",
        );
        self.store
            .remove_inbox_by_target(&format!("workspace:{id}"));
        self.emit_workspace_event(id, "MA", "Maestro", "Retrying from the last checkpoint.");
        self.resume(id).await;
        self.publish_inbox_updated();
    }

    /// Re-dispatches every loop Recover parked as interrupted (and loops
    /// still queued at exit), continuing interrupted chats around them.
    /// Loops paused for a reason the human owns are never touched. Call once
    /// at startup, right after Recover.
    pub async fn auto_resume(&self) {
        self.sync_chat_inbox();
        // Chats on a workspace this pass resumes wait until after its
        // Resume: an agent-mode send would edit the tree with the Coder.
        let hold = self.workspaces_pending_loop_auto_resume();
        self.launch_interrupted_chats_except(&hold).await;
        for l in self.store.workspaces() {
            if l.archived {
                continue;
            }
            if l.state == WorkspaceState::Paused && l.phase == PHASE_INTERRUPTED {
                let Some(cp) = &l.checkpoint else {
                    continue;
                };
                tracing::info!(
                    "recur: auto-resuming loop {} from {} (iteration {})",
                    l.id,
                    cp.state.as_str(),
                    cp.iteration
                );
                self.emit_workspace_event(
                    &l.id,
                    "MA",
                    "Maestro",
                    "Reloop restarted: resuming from the last checkpoint.",
                );
            } else if l.state == WorkspaceState::Queued {
                // Nothing ran; without this it would sit queued forever (the
                // drain only fires when another loop exits).
                tracing::info!("recur: starting queued loop {} after restart", l.id);
            } else {
                continue;
            }
            self.resume(&l.id).await;
            self.launch_interrupted_chats_for(&l.id).await;
        }
        self.launch_remaining_interrupted_chats().await;
        // Subagent watchers re-arm only after interrupted chats launched.
        self.rearm_chat_children().await;
    }

    /// The workspaces AutoResume will call Resume on.
    pub fn workspaces_pending_loop_auto_resume(&self) -> HashSet<String> {
        self.store
            .workspaces()
            .into_iter()
            .filter(|l| {
                !l.archived
                    && ((l.state == WorkspaceState::Paused
                        && l.phase == PHASE_INTERRUPTED
                        && l.checkpoint.is_some())
                        || l.state == WorkspaceState::Queued)
            })
            .map(|l| l.id)
            .collect()
    }
}
