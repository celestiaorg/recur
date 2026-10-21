//! Workspaces: the single unit of work (a worktree + branch plus the
//! surfaces around it), their lifecycle state and land state.

use serde::{Deserialize, Serialize};

use crate::serde_util::{is_false, is_zero};
use crate::{Plan, TestResult, VerifyStep};

string_enum! {
    /// The single dial that sets the Maestro's latitude within the hard rails
    /// (PRD §9). It changes thresholds, never whether a rail holds.
    AutonomyLevel {
        Cautious = "cautious",
        Balanced = "balanced",
        Aggressive = "aggressive",
    }
}

string_enum! {
    /// The coarse lifecycle state (PRD §7.1).
    WorkspaceState {
        Queued = "queued",
        Planning = "planning",
        Coding = "coding",
        Reviewing = "reviewing",
        Verifying = "verifying",
        PrOpen = "pr_open",
        Merged = "merged",
        Paused = "paused",
        Escalated = "escalated",
        Failed = "failed",
        Stopped = "stopped",
    }
}

string_enum! {
    /// Buckets a loop for the grouped list views (PRD §16).
    Group {
        /// Needs you (escalated / paused for a decision).
        Needs = "needs",
        /// Working.
        Active = "active",
        /// Ready for you (converged, PR awaiting review).
        Ready = "ready",
    }
}

/// Reports whether the autopilot actively OWNS the workspace's worktree (the
/// run-lock): any in-flight phase, including queued. Paused/escalated/settled
/// runs sit at a checkpoint with no specialist running, so the human may edit.
pub fn run_active(s: &WorkspaceState) -> bool {
    matches!(
        s,
        WorkspaceState::Queued
            | WorkspaceState::Planning
            | WorkspaceState::Coding
            | WorkspaceState::Reviewing
            | WorkspaceState::Verifying
    )
}

/// An additions/deletions pair, rendered as +N −M throughout the UI. On
/// Workspace it is the live branch-vs-base rollup (including uncommitted
/// tracked changes and untracked text files).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiffStat {
    pub add: i64,
    pub del: i64,
}

// Workspace type markers. DEPRECATED: a workspace is just a worktree + the
// surfaces around it, and "loop" is an agent MODE on a chat session. Type
// survives on records only as a creation-origin marker; behavior branches on
// `Workspace::has_run` instead.
pub const WORKSPACE_LOOP: &str = "loop";
pub const WORKSPACE_SOLO: &str = "solo";
pub const WORKSPACE_PAIR: &str = "pair";
/// Marks the singleton "main working copy" workspace whose worktree path is
/// the project's real checkout. Behavior branches on `Workspace::is_main`.
pub const WORKSPACE_MAIN: &str = "main";

/// The type marker, defaulting an empty (pre-Type) record to "loop".
/// DEPRECATED: use `Workspace::has_run`.
pub fn workspace_type(t: &str) -> &str {
    if t.is_empty() { WORKSPACE_LOOP } else { t }
}

/// Points at a tracker issue. ID is the tracker's stable id (Linear issue
/// UUID, GitHub issue node id) so team moves, repo renames, and transfers keep
/// the link; Key is the human ref shown in the UI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IssueRef {
    /// "github" | "linear".
    pub source: String,
    pub id: String,
    /// BOTFUN-152, owner/repo#12.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub key: String,
}

/// The single unit of work (PRD §2): a git worktree + branch plus the
/// surfaces around it (feed, Observatory, PRs, chat tabs). What varies is each
/// chat session's mode and whether the autopilot has been engaged (HasRun).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Workspace {
    /// Prevents replay of an interrupted scheduled turn.
    #[serde(skip_serializing_if = "is_false")]
    pub scheduled_prompt_pending: bool,
    /// Set when a scheduled prompt occurrence created this workspace.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub schedule_id: String,

    pub id: String,
    pub project_id: String,
    /// DEPRECATED creation-origin marker (see [`workspace_type`]). New records
    /// still stamp it so older builds can read the database.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub r#type: String,
    /// The autopilot has been engaged on this workspace.
    #[serde(skip_serializing_if = "is_false")]
    pub run_started: bool,
    /// The project's singleton "main working copy" workspace: its worktree
    /// path is the real checkout. It never engages the autopilot, opens PRs,
    /// or has its worktree removed.
    #[serde(skip_serializing_if = "is_false")]
    pub is_main: bool,
    /// Set while this workspace only hosts agent chat for an imported GitHub
    /// PR with no Recur worktree yet. Cleared once a real worktree lands.
    #[serde(rename = "hostPrId", skip_serializing_if = "String::is_empty")]
    pub host_pr_id: String,
    /// The tracker issue this workspace works on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue: Option<IssueRef>,
    pub group: Group,
    pub goal: String,
    /// The short UI label when Goal is expanded wire (a prompt action such as
    /// "Linear ENG-123 · title"). Empty means Goal is already human-facing.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub goal_display: String,
    pub short: String,
    pub definition_of_done: String,
    pub state: WorkspaceState,
    /// Human label, e.g. "Verifying".
    pub status_label: String,
    /// Sub-label, e.g. "browsing the app".
    pub phase: String,
    pub iteration: i64,
    /// 0..1 ring fill.
    #[serde(with = "crate::json::float")]
    pub progress: f64,
    pub autonomy: AutonomyLevel,
    /// The engaged `[[loop]]` id (empty on old records → recur.loops.pr).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub loop_id: String,
    /// The engaged `[[policy]]` id (empty → map Autonomy onto a shipped policy).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub policy_id: String,
    /// The chat tab that owns this workspace's run (one chat = one loop).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub loop_chat_code: String,
    pub branch: String,
    /// The branch diffs, PRs, and "update with base" compare against.
    /// Retargeting does not rebase the worktree.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub base_branch: String,
    /// How many commits the base branch is ahead of this worktree.
    pub behind_base: i64,
    pub worktree_path: String,
    /// The TCP port allocated when the worktree was created ($RECUR_PORT /
    /// $PORT). Zero means not yet allocated.
    #[serde(skip_serializing_if = "is_zero")]
    pub port: i64,
    pub diff: DiffStat,
    /// e.g. ["MA","CO","QA","RV"]. `None` is Go's nil slice (`null`).
    pub agent_codes: Option<Vec<String>>,
    /// Last activity one-liner.
    pub last: String,
    /// When meaningful work last happened (unix ms). Zero on legacy rows.
    #[serde(skip_serializing_if = "is_zero")]
    pub last_activity_at: i64,
    /// When the human last had this workspace as the visible shell (unix ms).
    #[serde(skip_serializing_if = "is_zero")]
    pub last_viewed_at: i64,
    /// The LastActivityAt value the visible UI has already shown.
    #[serde(skip_serializing_if = "is_zero")]
    pub seen_activity_at: i64,
    pub elapsed_label: String,
    /// `None` is Go's nil slice (`null`).
    pub convergence: Option<Vec<ConvPoint>>,

    /// Marks a child workspace working a slice of its parent's plan
    /// (fan-out). The child branches off the parent's branch and is merged
    /// back into it when it settles.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub parent_id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parent_step_ids: Vec<String>,
    /// Stamped on a child by the parent's board.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fan_out: Option<ChildFanOut>,
    /// The parent's converge phase the child was cut from.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub parent_phase_id: String,

    /// Counts the board's fan-out waves so far, so a resumed parent continues
    /// the sequence.
    #[serde(skip_serializing_if = "is_zero")]
    pub fan_out_waves: i64,

    /// The implementer's latest done-notes. Empty when the last turn
    /// reported nothing.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub handoff: String,

    /// The loop's browser-verification evidence (PRD §10).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub verify: Vec<VerifyStep>,
    /// "" = not run, "passed", or "failed". Gates pr_open (rail #1).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub verify_state: String,

    /// A removed workspace: hidden from default lists while history is kept.
    /// User-facing copy says "removed".
    pub archived: bool,

    /// The last Tester report_result for this loop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_result: Option<TestResult>,

    /// The last durable "known good" loop position (D55), so an interrupted
    /// loop can be re-dispatched from where it left off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<Checkpoint>,

    /// The structured plan artifact. When set, DefinitionOfDone mirrors
    /// `Plan::text`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<Plan>,

    /// Why the loop last failed. Retryable marks a transient LLM/API failure,
    /// so Resume re-dispatches from Checkpoint (D56).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub last_error: String,
    #[serde(skip_serializing_if = "is_false")]
    pub retryable: bool,

    /// The Agent-panel tab strip order (agent codes). Empty means the default.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub chat_tab_order: Vec<String>,
}

impl Workspace {
    /// Whether the autopilot has (ever) been engaged. Folds in legacy loop
    /// records that carry Type ""/"loop" but no RunStarted flag.
    pub fn has_run(&self) -> bool {
        self.run_started || workspace_type(&self.r#type) == WORKSPACE_LOOP
    }

    /// The short UI string for this workspace's goal.
    pub fn goal_label(&self) -> &str {
        match self.goal_display.trim() {
            "" => &self.goal,
            s => s,
        }
    }

    /// Sets Goal and its prompt-action label together so a label cannot
    /// outlive (or be dropped from) the wire it describes. A blank goal is
    /// ignored.
    pub fn replace_goal(&mut self, goal: &str, display: &str) {
        let g = goal.trim();
        if g.is_empty() {
            return;
        }
        self.goal_display = display.trim().to_owned();
        self.goal = g.to_owned();
    }

    /// Whether this row only hosts agent chat for an imported PR (its
    /// worktree path is the project's real checkout).
    pub fn is_pr_chat_host(&self) -> bool {
        !self.host_pr_id.trim().is_empty()
    }
}

/// The parent board's record on a child workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChildFanOut {
    pub wave: i64,
    /// Parent step ids whose landing started the wave.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    /// merged | failed | conflict: <files>.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub outcome: String,
    /// The child's handoff as merged.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The parent chat tab that created a chat child (create_workspace).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub chat: String,
    /// The chat child's first-tab chat mode.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
}

/// A resumable position in a loop's state machine (D55). At is an RFC3339
/// string.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Checkpoint {
    pub state: WorkspaceState,
    pub iteration: i64,
    pub at: String,
    /// The `[[loop.phase]]` running now (empty on pre-walker records).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub phase_id: String,
    /// The parked graph continuation after an on-demand phase.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub resume_phase_id: String,
}

/// One iteration's open-finding count for the convergence chart (PRD §7.3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConvPoint {
    pub iteration: i64,
    pub findings: i64,
    pub color: String,
}

// Workspace land-state for the hand-driven titlebar (Commit / Update / Merge).
pub const UPDATE_WORKSPACE_UPDATED: &str = "updated";
pub const UPDATE_WORKSPACE_CONFLICT: &str = "conflict";
pub const UPDATE_WORKSPACE_NOOP: &str = "noop";
/// Merge/rebase ok; leftover edits left in git stash.
pub const UPDATE_WORKSPACE_AUTOSTASH: &str = "autostash";

/// The titlebar's Commit/Update/Merge (or Push) switch. On the main working
/// copy, Base/BehindBase/AheadBase are versus the tracking remote and Remote
/// names it; both are empty without an upstream.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceLandState {
    pub behind_base: i64,
    /// Unique commits on the workspace branch; Merge is hidden at 0.
    pub ahead_base: i64,
    pub dirty: bool,
    pub unmerged: bool,
    pub base: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remote: String,
    /// Last-known GitHub branch-rules answer. True when unknown or the probe
    /// failed.
    #[serde(rename = "requirePR")]
    pub require_pr: bool,
    /// The open (non-merged) PR for this workspace, if any.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pr_id: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub pr_number: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pr_source: String,
    #[serde(skip_serializing_if = "is_false")]
    pub pr_draft: bool,
    /// Unique commits versus the GitHub PR's remote head (0 for Local PRs).
    #[serde(skip_serializing_if = "is_zero")]
    pub ahead_remote: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub behind_remote: i64,
}

/// The outcome of UpdateWorkspaceWithBase / UpdateWorkingCopyFromRemote.
/// Status is one of the `UPDATE_WORKSPACE_*` values.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdateWorkspaceResult {
    pub status: String,
    pub workspace: Workspace,
    pub base: String,
    #[serde(rename = "pushedPR", skip_serializing_if = "is_zero")]
    pub pushed_pr: i64,
    #[serde(skip_serializing_if = "is_false")]
    pub force_pushed: bool,
}

/// The machine-global rollup (PRD §15.2 resource.updated, D6).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Resource {
    pub loops_running: i64,
    pub needs_you: i64,
    pub iters_today: i64,
    /// Set at boot when git is missing or older than 2.31. Not persisted.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub git_warning: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_run_folds_legacy_type() {
        assert!(
            Workspace::default().has_run(),
            "empty type is a legacy loop"
        );
        let solo = Workspace {
            r#type: WORKSPACE_SOLO.into(),
            ..Default::default()
        };
        assert!(!solo.has_run());
        let started = Workspace {
            r#type: WORKSPACE_MAIN.into(),
            run_started: true,
            ..Default::default()
        };
        assert!(started.has_run());
        assert_eq!(workspace_type(""), WORKSPACE_LOOP);
        assert_eq!(workspace_type(WORKSPACE_PAIR), WORKSPACE_PAIR);
    }

    #[test]
    fn goal_label_and_replace_goal() {
        let mut w = Workspace {
            goal: "wire".into(),
            ..Default::default()
        };
        assert_eq!(w.goal_label(), "wire");
        w.goal_display = "  label ".into();
        assert_eq!(w.goal_label(), "label");
        w.replace_goal("  ", "x");
        assert_eq!(
            (w.goal.as_str(), w.goal_display.as_str()),
            ("wire", "  label ")
        );
        w.replace_goal(" next ", " shown ");
        assert_eq!(
            (w.goal.as_str(), w.goal_display.as_str()),
            ("next", "shown")
        );
    }

    #[test]
    fn pr_chat_host_and_run_active() {
        assert!(!Workspace::default().is_pr_chat_host());
        let host = Workspace {
            host_pr_id: " pr-1 ".into(),
            ..Default::default()
        };
        assert!(host.is_pr_chat_host());
        assert!(run_active(&WorkspaceState::Queued));
        assert!(run_active(&WorkspaceState::Verifying));
        assert!(!run_active(&WorkspaceState::Paused));
        assert!(!run_active(&WorkspaceState::default()));
    }

    #[test]
    fn zero_workspace_json_matches_go() {
        let got = serde_json::to_value(Workspace::default()).unwrap();
        let want = serde_json::json!({
            "id": "", "projectId": "", "group": "", "goal": "", "short": "",
            "definitionOfDone": "", "state": "", "statusLabel": "", "phase": "",
            "iteration": 0, "progress": 0, "autonomy": "", "branch": "",
            "behindBase": 0, "worktreePath": "", "diff": {"add": 0, "del": 0},
            "agentCodes": null, "last": "", "elapsedLabel": "", "convergence": null,
            "archived": false
        });
        assert_eq!(got, want);
    }
}
