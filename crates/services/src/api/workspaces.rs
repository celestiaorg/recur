use std::collections::BTreeMap;

use async_trait::async_trait;
use recur_domain::{
    AttentionCounts, ChatAttachment, ChatItem, Commit, FeedItem, IssueRef, OpenInTarget,
    OpenPathResult, SessionView, UpdateWorkspaceResult, Workspace, WorkspaceChatTab,
    WorkspaceLandState,
};

use super::WorkspaceChatSelection;
use crate::Result;

#[async_trait]
#[allow(clippy::too_many_arguments)]
pub trait WorkspacesService: Send + Sync {
    /// ApplyProvisionalChatTitles names an untitled workspace and chat tab from a
    /// message queued while the worktree is still being set up.
    async fn apply_provisional_chat_titles(
        &self,
        workspace_id: &str,
        code: &str,
        text: &str,
    ) -> Result<()>;

    /// ApprovePlan marks the workspace's structured plan approved (the Plan panel's
    /// Approve action). Returns a status string.
    async fn approve_plan(&self, workspace_id: &str) -> Result<String>;

    /// AttentionCounts returns unseen chat + inbox unread rollups for sidebar and
    /// dock badges (dock = unseen + non-chat inbox unread; unseen chats are inbox rows).
    async fn attention_counts(&self) -> Result<AttentionCounts>;

    /// CancelWorkspaceMessage interrupts this tab's in-flight chat turn and, on a
    /// loop-engaged tab, parks that chat's walk idle so the conductor can continue.
    async fn cancel_workspace_message(&self, workspace_id: &str, code: &str) -> Result<()>;

    /// ClearWorkspaceIssue removes a workspace's tracker issue link.
    async fn clear_workspace_issue(&self, workspace_id: &str) -> Result<Workspace>;

    /// CloseWorkspaceChat closes a coding-agent chat tab. History stays in the session
    /// store (reopen via ReopenWorkspaceChat / recents). Closing the last tab resets
    /// to a fresh empty chat. Returns remaining tabs.
    async fn close_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> Result<Vec<WorkspaceChatTab>>;

    /// CreateWorkspace is the single creation path:
    /// one workspace noun, the MODE decides what starts inside it. mode="loop"
    /// engages the autopilot immediately: prompt is the run's task (required) and
    /// autonomy overrides the global default. Any other mode creates the workspace
    /// and lets the chat dock drive (the UI seeds prompt as the first chat
    /// message in that mode). simplify is the New Workspace chip for mode=loop
    /// (nil → Settings / newest pair-chat inherit); other modes ignore it and seed
    /// the first chat after create. Returns the stored record immediately; the git
    /// worktree (and setup command) finish in the background and paint via
    /// workspace.updated so the UI is not blocked on `git worktree add`.
    async fn create_workspace(
        &self,
        project_id: &str,
        name: &str,
        base_branch: &str,
        mode: &str,
        prompt: &str,
        autonomy: &str,
        simplify: Option<bool>,
    ) -> Result<Workspace>;

    /// CreateWorkspaceChat opens a new coding-agent chat tab on a solo/pair workspace.
    async fn create_workspace_chat(&self, workspace_id: &str) -> Result<WorkspaceChatTab>;

    /// CreateWorkspaceWith is CreateWorkspace plus the dock's loop + policy ids.
    /// Empty loopID / policyID resolve to Settings defaults (or shipped To a PR /
    /// Balanced). Refuses a missing or disabled id. simplify is the New Workspace
    /// chip for mode=loop (nil → Settings / inherit). For mode=loop, name is the
    /// short GoalDisplay label when prompt is expanded prompt-action wire.
    /// attachments are opening image pastes (mode=loop) so the planner can see them.
    async fn create_workspace_with(
        &self,
        project_id: &str,
        name: &str,
        base_branch: &str,
        mode: &str,
        prompt: &str,
        autonomy: &str,
        loop_id: &str,
        policy_id: &str,
        simplify: Option<bool>,
        attachments: &[ChatAttachment],
    ) -> Result<Workspace>;

    /// DeleteClosedWorkspaceChat permanently removes a closed chat's transcript.
    /// Open tabs and in-flight turns are ignored (the latter returns an error).
    /// Returns the remaining closed chats.
    async fn delete_closed_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> Result<Vec<WorkspaceChatTab>>;

    /// DeleteWorkspace permanently removes a settled loop and everything under it (PRs,
    /// agents, feed, findings, escalation, inbox rows, on-disk artifacts + worktree).
    /// Returns after the store record is gone and workspace.deleted has fired; disk
    /// teardown continues in the background. Used for NewTask cancel / failed create:
    /// not the titlebar Remove action.
    async fn delete_workspace(&self, id: &str) -> Result<()>;

    /// EngageLoop engages the autopilot on an EXISTING workspace with goal as the
    /// run's task: the Plan panel's "Run as loop" handoff and the chat dock's Loop
    /// mode share this path. Returns a status string.
    async fn engage_loop(&self, workspace_id: &str, goal: &str) -> Result<String>;

    /// EngageLoopWith engages the autopilot with explicit loop + policy ids.
    /// Empty ids fall back to the workspace stamp, then Settings / shipped defaults.
    async fn engage_loop_with(
        &self,
        workspace_id: &str,
        goal: &str,
        loop_id: &str,
        policy_id: &str,
    ) -> Result<String>;

    /// FetchWorkingCopyUpstream refreshes the remote tip the titlebar compares
    /// against: the main working copy's tracking branch, a workspace's merge base,
    /// and an open GitHub PR head. Best-effort: never an error.
    async fn fetch_working_copy_upstream(&self, id: &str) -> Result<WorkspaceLandState>;

    /// ForkWorkspaceChat branches a chat tab at the given turn's message id and
    /// opens a new PA* tab with the copied transcript. Empty tab if the fork point
    /// is unknown.
    async fn fork_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
        message_id: &str,
    ) -> Result<WorkspaceChatTab>;

    /// ForkWorkspaceChatToWorkspace creates a new workspace in the same project and
    /// copies this chat through the given turn onto its default tab.
    async fn fork_workspace_chat_to_workspace(
        &self,
        workspace_id: &str,
        code: &str,
        message_id: &str,
    ) -> Result<Workspace>;

    async fn get_workspace(&self, id: &str) -> Result<Workspace>;

    /// GetWorkspaceFeed returns the loop's activity feed (newest first), backing the
    /// Activity tab (PRD §16.4). Live items arrive via the loop.event stream. For a
    /// loop that ran in a prior session, the in-memory feed is reconstructed from the
    /// durable append-only events table on first access so a reload shows history
    /// (PRD §6.5, D13). The M0 seeded feeds are returned as-is.
    async fn get_workspace_feed(&self, workspace_id: &str) -> Result<Vec<FeedItem>>;

    /// LastAssistantText returns the last assistant text block in this workspace's
    /// latest session (newest message, then newest text content). Prefers
    /// display_text over wire text. Empty when that session has no assistant text yet.
    async fn last_assistant_text(&self, workspace_id: &str) -> Result<String>;

    /// ListClosedWorkspaceChats returns pair-chat tabs that were closed but still
    /// have a durable transcript (the Agent tab-strip recents menu).
    async fn list_closed_workspace_chats(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<WorkspaceChatTab>>;

    /// ListOpenInTargets returns installed apps that can open a workspace folder.
    async fn list_open_in_targets(&self) -> Result<Vec<OpenInTarget>>;

    /// ListWorkspaceChats returns open coding-agent chat tabs for a solo/pair workspace.
    async fn list_workspace_chats(&self, workspace_id: &str) -> Result<Vec<WorkspaceChatTab>>;

    /// ListWorkspaces returns every workspace immediately (cached DiffStat). A
    /// background single-flight refresh recomputes live git +N/−M and publishes
    /// workspace.updated when counts change so the sidebar paints without blocking
    /// boot / OverviewPanel / onLoopEngaged on sequential git shells.
    ///
    /// Order is newest-first for the sidebar and Go to palette. The store itself
    /// stays oldest-first so queued-loop drain still picks the oldest waiting run.
    async fn list_workspaces(&self) -> Result<Vec<Workspace>>;

    /// MarkWorkspaceChatSeen clears the unseen badge for a chat tab the human just
    /// viewed (selected / focused after a background turn finished).
    async fn mark_workspace_chat_seen(&self, workspace_id: &str, code: &str) -> Result<()>;

    /// MergeWorkspace lands a hand-driven workspace onto its merge target. An
    /// open GitHub PR is refused (merge from the PR page). When the merge
    /// target has a remote-tracking counterpart (origin/main, including a
    /// local "main" that tracks it), the land is pushed; a purely local base
    /// stays on disk. method is "merge"/"squash"/"rebase"; empty =
    /// project/settings prefs. Returns the updated workspace (state=merged).
    async fn merge_workspace(&self, workspace_id: &str, method: &str) -> Result<Workspace>;

    /// OpenMainWorkspace returns the project's singleton "main working copy"
    /// workspace (the Observatory scoped to the project's real checkout, not a
    /// managed worktree), creating it on first open. It has no Loop mode and no PRs;
    /// edits and commits land directly in the user's repo. Returns the stored record.
    async fn open_main_workspace(&self, project_id: &str) -> Result<Workspace>;

    /// OpenPath opens a file or folder in Recur: find-or-create a project, focus
    /// its working copy (or the matching workspace), and optionally jump to a file.
    /// Homeless files (not inside any project or git repo) become absolute-path
    /// tabs on the singleton Files project: they do not add ~/Downloads as a project.
    async fn open_path(&self, path: &str) -> Result<OpenPathResult>;

    /// OpenWorkspaceIn opens the workspace's on-disk folder in the named app
    /// (finder, cursor, terminal, …).
    async fn open_workspace_in(&self, workspace_id: &str, target_id: &str) -> Result<()>;

    /// OpenWorkspacePR opens a Local (or GitHub, when the project opts in) PR from a
    /// hand-driven workspace's committed changes. Title/body may be agent-written
    /// via mcp__recur__open_pr (OpenWorkspacePRWith); the UI path generates a
    /// workspace-wide summary versus the base (same job as /open-pr). If a PR is already open, this pushes
    /// new commits and refreshes the snapshot (titlebar Push updates). force is
    /// --force-with-lease (titlebar Force push). Merge is the shared
    /// PRsService.MergePR (a PR is owned by its WorkspaceID either way).
    /// Returns the PR id. Hand-driven workspaces tracking a local branch use
    /// MergeWorkspace instead; this remains for GitHub-opt-in projects. The
    /// titlebar Open PR button invokes /open-pr rather than this method.
    async fn open_workspace_pr(&self, workspace_id: &str, force: bool) -> Result<String>;

    /// PauseLoop / ResumeLoop / StopLoop deliver control commands to the running loop
    /// via the engine's command channel (PRD §6.2). Stop cancels the loop's context,
    /// killing its agents (PRD §6.1).
    async fn pause_loop(&self, id: &str) -> Result<()>;

    /// PullWorkspacePR fetches the open GitHub PR's remote head into the worktree
    /// when GitHub is ahead of local HEAD (titlebar Pull updates).
    async fn pull_workspace_pr(&self, workspace_id: &str) -> Result<UpdateWorkspaceResult>;

    /// PushWorkingCopy pushes the main working copy's HEAD to its tracking remote.
    async fn push_working_copy(&self, workspace_id: &str) -> Result<Workspace>;

    /// RememberChatLoopPolicy stores a tab's [[loop]] + [[policy]] before that chat engages.
    async fn remember_chat_loop_policy(
        &self,
        workspace_id: &str,
        code: &str,
        loop_id: &str,
        policy_id: &str,
    ) -> Result<()>;

    /// RememberWorkspaceChatPromptGroup stamps a [[prompt]] tab_group on this chat tab.
    async fn remember_workspace_chat_prompt_group(
        &self,
        workspace_id: &str,
        code: &str,
        group: &str,
    ) -> Result<()>;

    /// RememberWorkspaceChatSelection persists the dock's model + flags + mode + simplify for a
    /// chat tab without sending a turn: so switching to Review (etc.) survives
    /// reload even before the first message. simplify nil freezes Settings on first write.
    async fn remember_workspace_chat_selection(
        &self,
        workspace_id: &str,
        code: &str,
        model: &str,
        flags: &BTreeMap<String, String>,
        mode: &str,
        simplify: Option<bool>,
    ) -> Result<()>;

    /// RememberWorkspaceChatSimplify persists the composer Simplify chip for a chat
    /// tab (PA* or Loop/MA) without sending a turn. Settings is only the default for
    /// new sessions; this is the per-session override.
    async fn remember_workspace_chat_simplify(
        &self,
        workspace_id: &str,
        code: &str,
        on: bool,
    ) -> Result<()>;

    /// RemoveWorkspace hides a settled workspace from the default lists while keeping
    /// its history. Returns after the store is marked archived; worktree teardown
    /// continues in the background.
    async fn remove_workspace(&self, id: &str) -> Result<Workspace>;

    /// ReopenWorkspaceChat restores a closed pair-chat tab onto the strip. Returns
    /// the open tabs after reopen.
    async fn reopen_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> Result<Vec<WorkspaceChatTab>>;

    /// ReorderWorkspaceChats persists a new Agent-panel tab strip order. codes must
    /// be a permutation of the currently open tab codes.
    async fn reorder_workspace_chats(&self, workspace_id: &str, codes: &[String]) -> Result<()>;

    /// ResetWorkspaceChat drops every message after the turn closed by messageID
    /// and, when a later turn changed files, restores the worktree to that turn's end.
    async fn reset_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
        message_id: &str,
    ) -> Result<()>;

    /// RestartLoop re-runs a stopped or failed loop over its existing worktree: the
    /// human's version of the Maestro's mcp__recur__resume_loop hand. Returns the
    /// engine's status string ("Loop restarting…" or a named refusal).
    async fn restart_loop(&self, id: &str) -> Result<String>;

    async fn resume_loop(&self, id: &str) -> Result<()>;

    /// RetryLoop re-runs a loop that stopped on a TRANSIENT provider failure (a spend or
    /// rate limit, an overload, a dropped connection) from its last durable checkpoint
    /// (D56). It is a no-op on a loop that failed for any other reason: that would just
    /// fail again. The loop's Retryable flag says which case it is.
    async fn retry_loop(&self, id: &str) -> Result<()>;

    /// RetryWorkspaceSetup re-runs setup_cmd on a hand workspace whose setup failed
    /// after the worktree landed. Loop-start failures that deleted the tree must be
    /// removed and created again.
    async fn retry_workspace_setup(&self, id: &str) -> Result<()>;

    /// SendWorkspaceMessage runs one turn of a workspace coding-agent chat tab: an
    /// interactive Claude Code agent shaped by the tab's chat MODE: agent (default)
    /// can edit the worktree; ask/plan/review are read-only tool scopes (no loop, no
    /// Maestro system prompt). code selects the tab (PA, PA2, …; empty → PA). model
    /// is a qualified ref ("" = that mode's Settings → Models default); flags are the
    /// dock's per-model dimensions (nil/empty = that mode's defaults); attachments
    /// are images pasted into the composer (nil/empty = text-only); mode is the
    /// dock's mode ("" = the tab's persisted mode, else agent); displayText is an
    /// optional UI label for the user bubble ("" for normal composer sends).
    /// freshSession, when true, starts a new provider session on this tab instead
    /// of resuming (prompt actions). Tokens also stream on the bus
    /// (session.delta, tagged with the chat code); returns the final reply.
    async fn send_workspace_message(
        &self,
        workspace_id: &str,
        code: &str,
        text: &str,
        model: &str,
        flags: &BTreeMap<String, String>,
        attachments: &[ChatAttachment],
        mode: &str,
        display_text: &str,
        fresh_session: bool,
    ) -> Result<String>;

    /// SetChatUIOccluded tells the engine a modal/dialog is covering the chat
    /// (New workspace, palettes, permission prompts). While occluded, a finish on
    /// the selected tab still raises unseen / dock attention (D-attention).
    async fn set_chat_ui_occluded(&self, occluded: bool) -> Result<()>;

    /// SetWorkspaceBaseBranch changes the branch this workspace's diffs, PRs, and
    /// "update with base" compare against. It does not rebase the worktree. The main
    /// working copy is refused (it has no merge base).
    async fn set_workspace_base_branch(
        &self,
        workspace_id: &str,
        base_branch: &str,
    ) -> Result<Workspace>;

    /// SetWorkspaceChatFocus records the chat tab the human is viewing and clears
    /// its unseen badge. Empty workspaceID clears focus (navigated away).
    async fn set_workspace_chat_focus(&self, workspace_id: &str, code: &str) -> Result<()>;

    /// SetWorkspaceIssue links a workspace to a tracker issue (replacing any link)
    /// and assigns the issue to the user when nobody owns it yet. An issue belongs
    /// to one workspace, so the link moves off any other workspace that had it. The
    /// claim runs before returning so the caller can refresh the issue list afterwards.
    async fn set_workspace_issue(&self, workspace_id: &str, issue: &IssueRef) -> Result<Workspace>;

    /// SetWorkspaceLoopPolicy stamps the dock pickers onto a workspace that has
    /// not engaged yet. After HasRun() the ids are pinned: switching is a restart.
    async fn set_workspace_loop_policy(
        &self,
        workspace_id: &str,
        loop_id: &str,
        policy_id: &str,
    ) -> Result<Workspace>;

    /// StartWorkspace creates and starts a real loop: a persisted Loop record, a worktree +
    /// branch off baseBranch (or the project default when empty), and a loop goroutine
    /// running a Coder agent (PRD §15.1, §11, §19 M2). Returns the new loop id, or ""
    /// on error (the UI keeps the user on the new-task screen).
    ///
    /// autonomy is stamped onto this loop (cautious|balanced|aggressive). Empty uses
    /// the global policy default; running loops keep their own value if the dial moves.
    async fn start_workspace(
        &self,
        project_id: &str,
        task: &str,
        base_branch: &str,
        autonomy: &str,
    ) -> Result<String>;

    async fn stop_loop(&self, id: &str) -> Result<()>;

    /// SubmitPlan writes a structured plan from the Plan panel (source=human):
    /// headline summary, optional detailed body, and checklist steps. stepIDs is
    /// aligned with steps (blank for a new row) so a retitle keeps status/note.
    /// Same engine path Plan-mode chats and the loop planner use. Returns a status string.
    async fn submit_plan(
        &self,
        workspace_id: &str,
        summary: &str,
        body: &str,
        steps: &[String],
        step_ids: &[String],
    ) -> Result<String>;

    /// UpdatePlan rewrites a loop's plan (DefinitionOfDone) from the plan editor:
    /// the same engine path as the Maestro's edit_plan tool: logged
    /// as a plan-change deviation and steered into subsequent specialist turns.
    /// Returns the engine's status string ("Plan updated…" or a named refusal).
    async fn update_plan(&self, workspace_id: &str, plan: &str) -> Result<String>;

    /// UpdatePlanStep changes one plan step's status from the Plan panel (the
    /// human's version of the agent's update_plan_step hand). Returns a status string.
    async fn update_plan_step(
        &self,
        workspace_id: &str,
        step_id: &str,
        status: &str,
    ) -> Result<String>;

    /// UpdateWorkingCopyFromRemote fetches the main working copy's upstream and
    /// merges or rebases it into HEAD. method is "merge" or "rebase"; empty uses
    /// project/settings UpdateBaseMethod prefs. On conflict the checkout is aborted
    /// back to its previous state.
    async fn update_working_copy_from_remote(
        &self,
        workspace_id: &str,
        method: &str,
    ) -> Result<UpdateWorkspaceResult>;

    /// UpdateWorkspaceWithBase brings a hand-driven workspace up to date with its
    /// merge base. Clean git merge/rebase (no LLM); Status=conflict means markers
    /// were left and the UI should invoke /update. Empty method uses project/settings
    /// UpdateBaseMethod prefs.
    async fn update_workspace_with_base(
        &self,
        workspace_id: &str,
        method: &str,
    ) -> Result<UpdateWorkspaceResult>;

    /// WatchWorkspace makes workspaceID the single watched workspace: its worktree is
    /// watched via FSEvents for on-disk changes (from agents, editors, terminals:
    /// anything) and workspace.fs_changed fires on change so the Observatory tracks
    /// the disk live. Empty id stops watching (call on leaving the workspace).
    async fn watch_workspace(&self, workspace_id: &str) -> Result<()>;

    /// UnwatchWorkspace stops watching workspaceID, unless another workspace has
    /// taken the watch since. Leaving a workspace calls this rather than
    /// WatchWorkspace(""), which could land after the next workspace's watch.
    async fn unwatch_workspace(&self, workspace_id: &str) -> Result<()>;

    /// WorkspaceChatItems returns the FULL agent-chat transcript for one chat tab
    /// (text, reasoning, and the agent's tool calls, in order) from the session store
    /// (the single source of truth). code selects the tab (PA, PA2, …; empty → PA).
    async fn workspace_chat_items(&self, workspace_id: &str, code: &str) -> Result<Vec<ChatItem>>;

    /// WorkspaceChatModels returns the Claude Code model aliases offered in the
    /// workspace chat's model switcher, with the default first.
    async fn workspace_chat_models(&self) -> Result<Vec<String>>;

    /// WorkspaceChatSelection returns the last dock selection for a chat tab
    /// (code PA, PA2, …; empty → PA).
    async fn workspace_chat_selection(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> Result<WorkspaceChatSelection>;

    /// WorkspaceChatTurnDiff is the worktree change the agent made during the turn
    /// closed by messageID (its start vs end checkpoint), shaped as a Commit for
    /// the Observatory review surface.
    async fn workspace_chat_turn_diff(
        &self,
        workspace_id: &str,
        code: &str,
        message_id: &str,
    ) -> Result<Commit>;

    /// WorkspaceDiscardRisk returns a short warning when Remove would throw away
    /// uncommitted or unmerged work (empty when the worktree is clean/missing).
    async fn workspace_discard_risk(&self, id: &str) -> Result<String>;

    /// WorkspaceFacts returns loop-level facts (outcomes, deviations) with real
    /// timestamps, interleaved into the Loop chat cockpit by the UI.
    async fn workspace_facts(&self, workspace_id: &str) -> Result<Vec<FeedItem>>;

    /// WorkspaceLandState is the titlebar Commit / Update / Merge (or Push) switch.
    async fn workspace_land_state(&self, id: &str) -> Result<WorkspaceLandState>;

    /// WorkspaceSessions returns a chat's specialist transcripts. An empty agentCode
    /// loads all its specialists; live completion refreshes only the affected code.
    async fn workspace_sessions(
        &self,
        workspace_id: &str,
        owner_code: &str,
        agent_code: &str,
    ) -> Result<Vec<SessionView>>;
}
