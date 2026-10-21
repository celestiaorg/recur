//! Deferred call sites (plan §5, rule 1): E1 functions whose Go version
//! calls into code a later engine slice owns. Each hook is the exact Go call
//! site, named after the Go function it stands for; its body is what that
//! call does while the subsystem it reaches does not exist yet (no pending
//! questions, no remounts, no PR chat hosts, …). The owning slice replaces
//! the body with the real call when its module lands and deletes the hook's
//! entry from the plan's deferred list.

use std::sync::Arc;

use chrono::{DateTime, Local};
use recur_domain::Workspace;

use crate::Engine;
use crate::chatlife::ChatTurn;
use crate::loop_handle::LoopHandle;

impl Engine {
    // ---- E2: turn pipeline (ask_user broker)

    /// Go `askUsers.pendingFor`: a parked or resuming ask_user question on
    /// this tab. E2 (`ask_user` broker).
    pub(crate) fn ask_user_pending_for(&self, _workspace_id: &str, _code: &str) -> bool {
        false
    }

    /// Go `clearAskUsersForWorkspace`: drops parked questions, their inbox
    /// rows, and publishes `question.answered` / `inbox.updated`. E2.
    pub(crate) fn clear_ask_users_for_workspace(&self, _workspace_id: &str) {}

    // ---- E3: loops

    /// Go `drainQueued`: promote the oldest queued loop into a free slot.
    /// E3 (`loops::lifecycle`).
    pub(crate) async fn drain_queued(&self) {}

    /// Go `Loop.emitSettledWrapUp`: the Maestro's wrap-up for a settled
    /// walk. E3.
    pub(crate) fn emit_settled_wrap_up(&self, _handle: &Arc<LoopHandle>) {}

    /// Go `retireOpenEscalations`: answer-close a stopped run's open
    /// escalations. E3 (`loops::escalation`).
    pub(crate) fn retire_open_escalations(&self, _workspace_id: &str) {}

    /// Go `maybeDispatchAfterAllocate`: start a run stamped while the
    /// worktree was still allocating. E3 (`loops::lifecycle`).
    pub(crate) async fn maybe_dispatch_after_allocate(&self, _workspace_id: &str) {}

    // ---- E4: chat and coordination

    /// Go `pendingModeContinueFor`: a switch_mode remount is about to
    /// continue this tab. E4 (`chat::switch_mode`).
    pub(crate) fn pending_mode_continue_for(&self, _workspace_id: &str, _code: &str) -> bool {
        false
    }

    /// Go CancelChat's `takeModeContinue` + `dropContinueGen(p.epoch)` +
    /// `uncountModeRemount`: Stop drops a pending remount. E4.
    pub(crate) fn cancel_mode_continue(&self, _workspace_id: &str, _code: &str) {}

    /// Go `clearModeContinuesForWorkspace`. E4.
    pub(crate) fn clear_mode_continues_for_workspace(&self, _workspace_id: &str) {}

    /// Go `healZombieChat`: idle a tab persisted live with no turn. E4
    /// (`chat::recover`).
    pub(crate) async fn heal_zombie_chat(&self, _workspace_id: &str, _code: &str) -> bool {
        false
    }

    /// Go `chatResuming`: an interrupted chat AutoResume relaunched has not
    /// finished its resumed turn. E4 (`chat::recover`).
    pub(crate) fn chat_resuming(&self, _workspace_id: &str) -> bool {
        false
    }

    /// Go `recoverInterruptedChats`: heal live-but-dead chat agents at boot
    /// and queue PA*/MA tabs for AutoResume. E4 (`chat::recover`).
    pub(crate) fn recover_interrupted_chats(&self) {}

    /// Go `MarkAppChatSeen`. E4 (`app_chat`).
    pub(crate) fn mark_app_chat_seen(&self, _code: &str) {}

    /// Go `closeChatChildOnRemove`: a removed chat child reports closed to
    /// its parent. E4 (`chat::children`).
    pub(crate) fn close_chat_child_on_remove(&self, _ws: &Workspace) {}

    /// Go `childWaves.Delete(turn)`: forget the subagent wave a finished
    /// parent turn was creating. E4 (`chat::children`).
    pub(crate) fn forget_child_wave(&self, _turn: &Arc<ChatTurn>) {}

    /// Go `touchWorkspaceActivity`: the Briefing activity watermark. E4
    /// (`briefing`).
    pub(crate) fn touch_workspace_activity(
        &self,
        _workspace_id: &str,
        _summary: &str,
        _at: DateTime<Local>,
    ) {
    }

    // ---- E5: pull requests

    /// Go `maybeArchiveLinkedPRChatHost`: archive a PR chat host once its
    /// chats moved to a real worktree. E5 (`hoist`).
    pub(crate) fn maybe_archive_linked_pr_chat_host(&self, _workspace_id: &str) {}

    /// Go `linkWorkspaceToMatchingPRs`: attach imported PRs whose head is
    /// this workspace's branch. E5 (`prs::linking`).
    pub(crate) async fn link_workspace_to_matching_prs(&self, _ws: &Workspace) {}

    /// Go `archivePRChatHosts`: hoist a PR's chat hosts onto the workspace
    /// that now checks it out. E5 (`hoist`).
    pub(crate) async fn archive_pr_chat_hosts(&self, _pr_id: &str, _keep_id: &str) {}

    /// Go `fetchOpenPRRemote`: refresh an open forge PR's head ref. E5.
    pub(crate) async fn fetch_open_pr_remote(&self, _ws: &Workspace) {}
}
