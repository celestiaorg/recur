//! Deferred call sites (plan §5, rule 1): E1 functions whose Go version
//! calls into code a later engine slice owns. Each hook is the exact Go call
//! site, named after the Go function it stands for; its body is what that
//! call does while the subsystem it reaches does not exist yet (no pending
//! questions, no remounts, no PR chat hosts, …). The owning slice replaces
//! the body with the real call when its module lands and deletes the hook's
//! entry from the plan's deferred list.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{DateTime, Local};
use recur_domain::{
    ChatAttachment, Check, FeedItem, Pr, PrSource, Project, UserQuestion, Workspace,
};

use crate::Engine;
use crate::chatlife::ChatTurn;

impl Engine {
    // ---- E4: chat and coordination

    /// Go `resumeAfterAskUser`'s send: the answer as a new turn on the
    /// asking tab (`SendMaestroMessage` for MA, else `SendWorkspaceMessage`
    /// with the tab's dock selection). E4 (`chat::ask_resume`).
    pub(crate) async fn send_ask_user_resume(
        &self,
        _q: &UserQuestion,
        _prompt: &str,
        _display: &str,
        _attachments: Vec<ChatAttachment>,
    ) {
    }

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

    /// Go `ReconstructFeed`: the reload-path activity feed from sessions and
    /// the events table, read when the live feed is empty (the Maestro's
    /// recent activity). E4 (`feed`).
    pub(crate) fn reconstruct_feed(&self, _workspace_id: &str) -> Vec<FeedItem> {
        Vec::new()
    }

    /// Go AutoResume's `launchInterruptedChatsExcept(hold)`: continue the
    /// interrupted chats `recover_interrupted_chats` queued, except those on
    /// workspaces whose loop AutoResume resumes first. E4 (`chat::recover`).
    pub(crate) async fn launch_interrupted_chats_except(&self, _hold: &HashSet<String>) {}

    /// Go `launchInterruptedChatsFor(id)`: the held chats of a workspace,
    /// right after its loop's Resume returned. E4 (`chat::recover`).
    pub(crate) async fn launch_interrupted_chats_for(&self, _workspace_id: &str) {}

    /// Go `launchInterruptedChats(e.interruptedChats)` + clearing the queue:
    /// whatever is still queued after the loop pass. E4 (`chat::recover`).
    pub(crate) async fn launch_remaining_interrupted_chats(&self) {}

    /// Go `rearmChatChildren`: re-arm subagent watchers after interrupted
    /// chats launched. E4 (`chat::children`).
    pub(crate) async fn rearm_chat_children(&self) {}

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

    /// Go `CreateGitHubPR` (+ `githubPRBase`, `githubImportedPRID`): push the
    /// branch and open the PR on the project's forge for a project that
    /// opts in. `None` while no forge PR glue exists, so the walk stays on a
    /// Local PR; `Err((forge label, reason))` is the fallback note. E5
    /// (`prs::ops`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_forge_pr(
        &self,
        _proj: &Project,
        _worktree: &str,
        _branch: &str,
        _base: &str,
        _title: &str,
        _body: &str,
        _draft: bool,
    ) -> Option<Result<ForgePrCreated, (String, String)>> {
        None
    }

    /// Go `pushGitHubHead`: push a rework onto an open forge PR's remote
    /// head. `Err((forge label, reason))`. Only reached for a forge PR, which
    /// E5 creates. E5 (`prs::ops`).
    pub(crate) async fn push_forge_pr_head(
        &self,
        _worktree: &str,
        _repo_path: &str,
        _pr: &Pr,
        _head: &str,
        _force: bool,
    ) -> Result<(), (String, String)> {
        Ok(())
    }

    /// Go `githubPRChecks`: the forge's check rollup at PR open; a fetch
    /// error is an empty rollup that counts as passing (unknown is not a
    /// failure), which is also the answer with no forge glue. E5.
    pub(crate) async fn forge_pr_checks(
        &self,
        _source: &PrSource,
        _repo_path: &str,
        _number: i64,
    ) -> (Vec<Check>, bool) {
        (Vec::new(), true)
    }
}

/// A PR the forge opened (`create_forge_pr`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForgePrCreated {
    pub source: PrSource,
    pub number: i64,
    pub pr_id: String,
}
