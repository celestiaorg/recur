// Package domain defines Recur's core types — named per the PRD §2 glossary
// (Loop, Maestro, Specialist, Convergence, Deviation, PR, Escalation, Worktree).
// These serialize to the frontend via Wails bindings and to SQLite for persistence.
package domain

import (
	"encoding/json"
	"fmt"
	"strings"
	"time"
)

// AutonomyLevel is the single dial that sets the Maestro's latitude within the
// hard rails (PRD §9). It changes thresholds, never whether a rail holds.
type AutonomyLevel string

const (
	AutonomyCautious   AutonomyLevel = "cautious"
	AutonomyBalanced   AutonomyLevel = "balanced"
	AutonomyAggressive AutonomyLevel = "aggressive"
)

// WorkspaceState is the coarse lifecycle state (PRD §7.1).
type WorkspaceState string

const (
	StateQueued    WorkspaceState = "queued"
	StatePlanning  WorkspaceState = "planning"
	StateCoding    WorkspaceState = "coding"
	StateReviewing WorkspaceState = "reviewing"
	StateVerifying WorkspaceState = "verifying"
	StatePROpen    WorkspaceState = "pr_open"
	StateMerged    WorkspaceState = "merged"
	StatePaused    WorkspaceState = "paused"
	StateEscalated WorkspaceState = "escalated"
	StateFailed    WorkspaceState = "failed"
	StateStopped   WorkspaceState = "stopped"
)

// Group buckets a loop for the grouped list views (PRD §16: Needs you / Working / Ready).
type Group string

const (
	GroupNeeds  Group = "needs"  // needs you (escalated / paused for a decision)
	GroupActive Group = "active" // working
	GroupReady  Group = "ready"  // ready for you (converged, PR awaiting review)
)

// ProjectGroup is a named, collapsible section in the sidebar Projects tree.
// Projects reference it via Project.GroupID; empty GroupID means ungrouped.
type ProjectGroup struct {
	ID        string `json:"id"`
	Name      string `json:"name"`
	Open      bool   `json:"open"`      // expanded in the sidebar (shows member projects)
	CreatedAt string `json:"createdAt"` // RFC3339
	UpdatedAt string `json:"updatedAt"` // RFC3339
}

// BranchChoice is one entry in the new-workspace base-branch picker.
// Group is a remote name (origin, upstream, …) or git.LocalBranchGroup ("Local").
type BranchChoice struct {
	Name  string `json:"name"`
	Group string `json:"group"`
}

// Project is a local folder Recur can open — typically a git repo (PRD §14, §18).
// Non-git folders are allowed as projects with a working copy only; worktree-based
// workspaces require IsGit.
type Project struct {
	ID         string `json:"id"`
	Name       string `json:"name"`
	Pic        string `json:"pic"`   // letter, lucide:name, or data:image… avatar
	Color      string `json:"color"` // CSS color var or custom hex
	Open       bool   `json:"open"`  // expanded in the sidebar tree
	Idle       bool   `json:"idle"`
	RepoPath   string `json:"repoPath"`
	BaseBranch string `json:"baseBranch"`
	// SetupCmd/TeardownCmd/AppURL/Env/RunCommands are portable run-config from
	// <repo>/.recur/project.toml (computed on read; not stored in Recur's DB).
	// Settings save writes that file (creates if needed). Other Project
	// fields stay machine-local in Recur.
	SetupCmd    string `json:"setupCmd"`    // runs once when a worktree is created
	TeardownCmd string `json:"teardownCmd"` // runs once on remove/delete (after worktree removal prep)
	AppURL      string `json:"appURL"`
	// RunCommands are named commands (dev, test, …) from project.toml [[run]].
	// PinnedRun (below) is the machine-local default for the Run button.
	RunCommands []RunCommand `json:"runCommands,omitempty"`
	// PinnedRun is the name of the Run button default. Empty = first command.
	// Stored in Recur's DB, not project.toml.
	PinnedRun string `json:"pinnedRun,omitempty"`
	// GroupID is the sidebar project group this project belongs to.
	// Empty means ungrouped (shown after all groups).
	GroupID string `json:"groupId,omitempty"`
	// Hidden leaves the project out of the sidebar tree unless
	// Settings.ShowHiddenProjects is on.
	Hidden bool `json:"hidden,omitempty"`
	// Env is dotenv-format text (KEY=value lines, # comments) injected into
	// setup/teardown/named runs and Observatory terminals. Values may reference
	// workspace vars ($RECUR_PORT, $RECUR_WORKSPACE_ID, $RECUR_WORKTREE,
	// $RECUR_PROJECT_ID, $RECUR_BRANCH, $PORT), host vars like $HOME, and
	// keys from .env / .env.local / .recur/.env / .recur/.env.local
	// (secrets; later files win).
	Env string `json:"env,omitempty"`
	// CopyFiles is newline-separated glob patterns copied from the project
	// checkout into each new workspace, before setup_cmd. Patterns without a
	// slash match at any depth (.env* → apps/web/.env.local). Gitignored files
	// are included — that is the point (secrets, local dotenv). Portable via
	// project.toml.
	CopyFiles string `json:"copyFiles,omitempty"`
	// HasProjectFile is true when <repo>/.recur/project.toml exists. Computed
	// on read; not persisted.
	HasProjectFile bool `json:"hasProjectFile,omitempty"`
	// ProjectFileError is set when the file exists but cannot be parsed.
	// Computed on read; not persisted. Settings save replaces the file.
	ProjectFileError string `json:"projectFileError,omitempty"`
	// WriteProjectFile, when set on create/update from project settings,
	// writes .recur/project.toml (creates or replaces). Not persisted.
	WriteProjectFile bool `json:"writeProjectFile,omitempty"`
	// PROnGitHub: when true, loops open REAL GitHub PRs (push + `gh pr create`) instead
	// of Local PRs (PRD §12.1). Requires the gh CLI installed+authenticated and an
	// `origin` remote; the engine falls back to a Local PR (with a feed note) if not.
	PROnGitHub bool `json:"prOnGithub"`
	// DeleteBranchOnRemove, when true, deletes the workspace's local git branch
	// after Remove tears down the worktree. Default false. Remote branches are
	// never deleted. Machine-local (extra_json), not project.toml.
	DeleteBranchOnRemove bool `json:"deleteBranchOnRemove,omitempty"`
	// LinearProjects is a comma-separated list of Linear project names or team
	// keys whose assigned issues belong to this project. GitHub issues match by
	// git remote instead. Machine-local (extra_json).
	LinearProjects string `json:"linearProjects,omitempty"`
	// MergeMethod overrides the global default merge strategy for this project.
	// Empty = inherit Settings.DefaultMergeMethod (D53).
	MergeMethod MergeMethod `json:"mergeMethod,omitempty"`
	// UpdateBaseMethod overrides the global default for "Update with base" (D54).
	// Empty = inherit Settings.DefaultUpdateBaseMethod. Independent of MergeMethod.
	UpdateBaseMethod UpdateBaseMethod `json:"updateBaseMethod,omitempty"`
	// SquashCommitMessage overrides the global squash-merge commit message
	// format (D65). Empty = inherit Settings.DefaultSquashCommitMessage.
	SquashCommitMessage SquashCommitMessage `json:"squashCommitMessage,omitempty"`
	// MergeCommitMessage overrides the global merge-commit message format (D65).
	// Empty = inherit Settings.DefaultMergeCommitMessage.
	MergeCommitMessage MergeCommitMessage `json:"mergeCommitMessage,omitempty"`
	// DisabledSkills / EnabledSkills are per-project overrides over
	// Settings.DisabledSkills (Project → Skills). DisabledSkills force off;
	// EnabledSkills force on (including skills disabled globally). Names in
	// neither list inherit the global default. See EffectiveSkillEnabled.
	DisabledSkills []string `json:"disabledSkills,omitempty"`
	EnabledSkills  []string `json:"enabledSkills,omitempty"`
	// DisabledMCP / EnabledMCP are per-project overrides over Settings.MCPServers
	// (Project → MCP). Same EffectiveSkillEnabled rule as skills: disable wins,
	// else enable, else inherit the machine default. Machine-local extra_json.
	DisabledMCP []string `json:"disabledMCP,omitempty"`
	EnabledMCP  []string `json:"enabledMCP,omitempty"`
	// IsGit is true when RepoPath is inside a git work tree. Computed on read
	// (List/Get); not persisted. Non-git projects keep a working copy but cannot
	// create worktree-based workspaces.
	IsGit bool `json:"isGit"`
}

// CopyFilesPreview is the settings-page scan of files matching CopyFiles globs
// in the project's checkout. Files is capped; Count is the full match total.
type CopyFilesPreview struct {
	Root      string   `json:"root"`
	Files     []string `json:"files"`
	Count     int      `json:"count"`
	Truncated bool     `json:"truncated,omitempty"`
	Error     string   `json:"error,omitempty"`
}

// FilesProjectID is the singleton project that hosts homeless files (paths
// that are not inside any Recur project or git checkout). Its working copy is
// an empty Recur-managed folder; the real files are absolute-path editor tabs.
const FilesProjectID = "files"

// OpenPathResult is the outcome of opening a file or folder (File → Open,
// `recur .`, drag-drop). The UI focuses Workspace and optionally jumps to File.
type OpenPathResult struct {
	Path        string     `json:"path"`
	ProjectID   string     `json:"projectId,omitempty"`
	WorkspaceID string     `json:"workspaceId,omitempty"`
	File        string     `json:"file,omitempty"`   // workspace-relative, or absolute when Loose
	Reveal      string     `json:"reveal,omitempty"` // directory to highlight in the explorer
	Created     bool       `json:"created,omitempty"`
	Loose       bool       `json:"loose,omitempty"`
	Project     *Project   `json:"project,omitempty"`
	Workspace   *Workspace `json:"workspace,omitempty"`
}

// EffectiveSkillEnabled applies project overrides over the global default.
// Project disable wins over project enable if both somehow list the same name.
func EffectiveSkillEnabled(name string, globalDisabled, projectDisabled, projectEnabled []string) bool {
	name = strings.TrimSpace(name)
	if name == "" {
		return true
	}
	if skillNameListed(projectDisabled, name) {
		return false
	}
	if skillNameListed(projectEnabled, name) {
		return true
	}
	return !skillNameListed(globalDisabled, name)
}

// DiffStat is an additions/deletions pair, rendered as +N −M throughout the UI.
// On Workspace it is the live branch-vs-base rollup (incl. uncommitted tracked
// changes and untracked text files), refreshed from git for the sidebar.
type DiffStat struct {
	Add int `json:"add"`
	Del int `json:"del"`
}

// Workspace type markers. DEPRECATED: the Solo/Loop
// workspace-type split is collapsed — a workspace is just a worktree + the
// surfaces around it, and "loop" is an agent MODE on a chat session. Type
// survives on records only as a creation-origin marker (old records and old
// builds read it); behavior branches on Workspace.HasRun instead.
const (
	WorkspaceLoop = "loop"
	WorkspaceSolo = "solo"
	WorkspacePair = "pair"
	// WorkspaceMain marks the singleton "main working copy" workspace whose
	// WorktreePath is the project's real checkout (not a managed worktree).
	// Behavior branches on Workspace.IsMain; the marker exists so older builds
	// reading the database can tell it apart from a solo worktree.
	WorkspaceMain = "main"
)

// WorkspaceType returns w.Type, defaulting an empty (pre-Type) record to "loop".
// DEPRECATED — see the type markers above; use Workspace.HasRun.
func WorkspaceType(t string) string {
	if t == "" {
		return WorkspaceLoop
	}
	return t
}

// RunActive reports whether the autopilot actively OWNS the workspace's
// worktree (the run-lock): any in-flight phase,
// including queued (it may start at any moment). Paused/escalated/settled runs
// sit at a checkpoint with no specialist running, so the human may edit.
func RunActive(s WorkspaceState) bool {
	switch s {
	case StateQueued, StatePlanning, StateCoding, StateReviewing, StateVerifying:
		return true
	}
	return false
}

// IssueRef points at a tracker issue. ID is the tracker's stable id (Linear
// issue UUID, GitHub issue node id) so team moves, repo renames, and
// transfers keep the link; Key is the human ref shown in the UI.
type IssueRef struct {
	Source string `json:"source"` // "github" | "linear"
	ID     string `json:"id"`
	Key    string `json:"key,omitempty"` // BOTFUN-152, owner/repo#12
}

// Workspace is the single unit of work (PRD §2): a git worktree + branch plus the surfaces
// around it (feed, Observatory, PRs, chat tabs). There are no workspace types —
// what varies is each chat session's MODE (ask/plan/agent/review/loop), and
// whether the autopilot has been engaged (HasRun). One noun.
type Workspace struct {
	// ScheduledPromptPending prevents replay of an interrupted scheduled turn.
	ScheduledPromptPending bool `json:"scheduledPromptPending,omitempty"`
	// ScheduleID is set when this workspace was created by a scheduled prompt
	// occurrence. Sidebar groups these under Scheduled; empty for human-created
	// workspaces and create_workspace fan-out children.
	ScheduleID string `json:"scheduleId,omitempty"`

	ID        string `json:"id"`
	ProjectID string `json:"projectId"`
	// Type is the DEPRECATED creation-origin marker (see WorkspaceType). New
	// records still stamp it so older builds can read the database; nothing
	// should branch on it — use HasRun.
	Type string `json:"type,omitempty"`
	// RunStarted marks that the autopilot has been engaged on this workspace
	// (StartWorkspace, or Loop mode on an existing workspace). Legacy loop
	// records predate the field — HasRun folds their Type marker in.
	RunStarted bool `json:"runStarted,omitempty"`
	// IsMain marks the project's singleton "main working copy" workspace: its
	// WorktreePath is the project's real checkout rather than a managed git
	// worktree. It never engages the autopilot, opens PRs, or has its worktree
	// removed — lifecycle actions that touch a worktree are guarded so they can
	// never delete the user's actual repo (docs/main-working-copy: no Loop, no
	// PRs; full Observatory + Ask/Plan/Agent/Review chat).
	IsMain bool `json:"isMain,omitempty"`
	// HostPRID is set while this workspace exists only to host agent chat for
	// an imported GitHub PR that has no Recur worktree yet. It is hidden from
	// the sidebar, must not be treated as the PR's linked workspace, and uses
	// the project's checkout as cwd (Ask/Plan/Review only). CreateWorkspaceFromPR
	// promotes the same row — HostPRID is cleared once a real worktree lands.
	HostPRID string `json:"hostPrId,omitempty"`
	// Issue is the tracker issue this workspace works on, set when it is
	// created from the Issues view or a prompt (/linear) reports one.
	Issue *IssueRef `json:"issue,omitempty"`
	Group Group     `json:"group"`
	Goal  string    `json:"goal"`
	// GoalDisplay is the short UI label when Goal is expanded wire (a prompt
	// action such as "Linear ENG-123 · title"). Empty means Goal is already
	// human-facing. The model still receives Goal.
	GoalDisplay      string         `json:"goalDisplay,omitempty"`
	Short            string         `json:"short"`
	DefinitionOfDone string         `json:"definitionOfDone"`
	State            WorkspaceState `json:"state"`
	StatusLabel      string         `json:"statusLabel"` // human label, e.g. "Verifying"
	Phase            string         `json:"phase"`       // sub-label, e.g. "browsing the app"
	Iteration        int            `json:"iteration"`
	Progress         float64        `json:"progress"` // 0..1 ring fill
	Autonomy         AutonomyLevel  `json:"autonomy"`
	// LoopID is the engaged [[loop]] id (empty on old records → recur.loops.pr).
	LoopID string `json:"loopId,omitempty"`
	// PolicyID is the engaged [[policy]] id (empty → map Autonomy onto a
	// shipped policy).
	PolicyID string `json:"policyId,omitempty"`
	// LoopChatCode is the chat tab that owns this workspace's run (one chat
	// = one loop). Empty on legacy records that engaged as MA.
	LoopChatCode string `json:"loopChatCode,omitempty"`
	Branch       string `json:"branch"`
	// BaseBranch is the branch this workspace's diffs, PRs, and "update with
	// base" compare against. Set at creation (defaults to the project's
	// BaseBranch) and changeable afterwards — retargeting does not rebase the
	// worktree.
	BaseBranch string `json:"baseBranch,omitempty"`
	// BehindBase is how many commits the base branch is ahead of this worktree
	// (D51-style; no remote fetch). Drives the titlebar Update affordance.
	BehindBase   int    `json:"behindBase"`
	WorktreePath string `json:"worktreePath"`
	// Port is the free TCP port allocated once when the workspace's worktree is
	// created. Injected as $RECUR_PORT / $PORT for setup, teardown, named runs,
	// verify, and Observatory terminals. Zero means not yet allocated.
	Port       int      `json:"port,omitempty"`
	Diff       DiffStat `json:"diff"`
	AgentCodes []string `json:"agentCodes"` // e.g. ["MA","CO","QA","RV"]
	Last       string   `json:"last"`       // last activity one-liner
	// LastActivityAt is when meaningful work last happened (unix ms). Zero on
	// legacy rows — Briefing falls back to session/event/PR timestamps.
	LastActivityAt int64 `json:"lastActivityAt,omitempty"`
	// LastViewedAt is when the human last had this workspace as the committed
	// visible shell (unix ms). Used for Pick-up recency, not the unread badge.
	LastViewedAt int64 `json:"lastViewedAt,omitempty"`
	// SeenActivityAt is the LastActivityAt value the visible UI has already
	// shown. A later concurrent activity event stays "changed since viewed".
	SeenActivityAt int64       `json:"seenActivityAt,omitempty"`
	ElapsedLabel   string      `json:"elapsedLabel"`
	Convergence    []ConvPoint `json:"convergence"`

	// ParentID / ParentStepIDs mark a child workspace that works a slice of
	// its parent's plan (fan-out). The child branches off the parent's branch
	// and is merged back into it when it settles.
	ParentID      string   `json:"parentId,omitempty"`
	ParentStepIDs []string `json:"parentStepIds,omitempty"`
	// FanOut is stamped on a child by the parent's board: which wave started
	// it, what had just landed to unblock that wave, and — once the parent
	// merged it — the outcome the wave card shows. The card is rebuilt from
	// these rows, so it survives restarts without a copy of its own.
	FanOut *ChildFanOut `json:"fanOut,omitempty"`
	// ParentPhaseID is the parent's converge phase the child was cut from: the
	// child's loop is derived from it (same roles, prompts, overrides).
	ParentPhaseID string `json:"parentPhaseId,omitempty"`

	// FanOutWaves counts the board's fan-out waves so far (the cockpit card's
	// number). Persisted so a resumed parent continues the sequence instead of
	// restarting at 1 and overwriting an earlier wave's card.
	FanOutWaves int `json:"fanOutWaves,omitempty"`

	// Handoff is the implementer's latest done-notes (deviations from the plan,
	// concerns, unfinished work). Empty when the last turn reported nothing.
	Handoff string `json:"handoff,omitempty"`

	// Verify is the loop's browser-verification evidence (PRD §10): the QA agent's
	// per-step results from the most recent verify run, surfaced on the eventual PR.
	// VerifyState records the verify outcome the guard reads to gate pr_open (rail
	// #1): "" = not run, "passed" = all steps passed, "failed" = a step failed.
	Verify      []VerifyStep `json:"verify,omitempty"`
	VerifyState string       `json:"verifyState,omitempty"`

	// Archived marks a removed workspace: hidden from default lists while history
	// (feed, PRs, agents) is kept. The worktree is torn down; the JSON field name
	// stays "archived" for store compatibility. User-facing copy says "removed".
	Archived bool `json:"archived"`

	// TestResult is the last Tester report_result for this loop (ADE checks).
	TestResult *TestResult `json:"testResult,omitempty"`

	// Checkpoint is the last durable "known good" position of the loop's state
	// machine (D55). It is written on every in-flight phase transition so a loop
	// interrupted by an app quit/crash — or stopped by a transient LLM error — can be
	// re-dispatched from where it left off instead of replaying from planning.
	Checkpoint *Checkpoint `json:"checkpoint,omitempty"`

	// Plan is the structured plan artifact. When
	// set, DefinitionOfDone mirrors Plan.Text() so every prose consumer (prompt
	// builders, the plan editor fallback) stays coherent with the body and steps.
	Plan *Plan `json:"plan,omitempty"`

	// LastError is the human-readable reason the loop last failed (empty when it did
	// not). Retryable marks that reason as a TRANSIENT LLM/API failure (spend or rate
	// limit, overload, dropped connection) rather than a genuine defect, so the UI can
	// offer Retry and Resume will re-dispatch from Checkpoint (D56).
	LastError string `json:"lastError,omitempty"`
	Retryable bool   `json:"retryable,omitempty"`

	// ChatTabOrder is the Agent-panel tab strip order (agent codes). Empty means
	// the default: Loop (MA) first when a run is engaged, then PA, PA2, ….
	ChatTabOrder []string `json:"chatTabOrder,omitempty"`
}

// HasRun reports whether the autopilot has (ever) been engaged on this
// workspace — the collapsed-world replacement for `WorkspaceType(...) == loop`
// branches. It folds in legacy records: pre-mode
// loop records carry Type ""/"loop" but no RunStarted flag.
func (w Workspace) HasRun() bool {
	return w.RunStarted || WorkspaceType(w.Type) == WorkspaceLoop
}

// GoalLabel is the short UI string for this workspace's goal. Empty
// GoalDisplay (plain-text goals) returns Goal.
func (w Workspace) GoalLabel() string {
	if s := strings.TrimSpace(w.GoalDisplay); s != "" {
		return s
	}
	return w.Goal
}

// ReplaceGoal sets Goal and its prompt-action label together so a label
// cannot outlive (or be dropped from) the wire it describes.
func (w *Workspace) ReplaceGoal(goal, display string) {
	if w == nil {
		return
	}
	g := strings.TrimSpace(goal)
	if g == "" {
		return
	}
	w.GoalDisplay = strings.TrimSpace(display)
	w.Goal = g
}

// IsPRChatHost reports whether this row exists only to host agent chat for an
// imported PR (no Recur worktree yet). Callers that attach PRs, paint the
// sidebar, or tear down worktrees must skip these — WorktreePath is the
// project's real checkout.
func (w Workspace) IsPRChatHost() bool {
	return strings.TrimSpace(w.HostPRID) != ""
}

// Plan step statuses.
const (
	StepPending = "pending"
	StepDoing   = "doing"
	StepDone    = "done"
	StepSkipped = "skipped"
)

// Plan is the structured, living plan artifact:
// written by Plan-mode chats and the loop planner via mcp__recur__submit_plan
// (structured at the source, never parsed from prose), rendered and edited in
// the Observatory Plan panel, and — once steps exist — the source of real run
// progress (steps done / total instead of hardcoded per-state constants).
// Body is the durable detailed brief other agents execute from; Summary is the
// short headline for lists and the panel lead-in.
type Plan struct {
	Summary string `json:"summary"`
	// Body is the full markdown plan (files, approach, edge cases, definition
	// of done). Empty on legacy summary-only records.
	Body        string           `json:"body,omitempty"`
	Steps       []PlanStep       `json:"steps,omitempty"`
	Attachments []PlanAttachment `json:"attachments,omitempty"`
	// Source records who wrote the current plan: "plan-chat" (a Plan-mode chat
	// tab), "loop-planner" (the run's planning turn), or "human" (panel edit).
	Source string `json:"source,omitempty"`
	// ChatCode is the pair-chat tab that filed this plan (PA, PA2, …). Empty
	// for the loop planner. Survives a later human panel edit so Run as loop
	// can inherit that chat's Simplify chip.
	ChatCode string `json:"chatCode,omitempty"`
	// Approved marks the human's sign-off (the Plan panel's Approve action, and
	// the plan-approval gate's answer for loops).
	Approved bool `json:"approved,omitempty"`
}

// PlanAttachment is a file that belongs to the plan: an agent artifact
// (create_artifact) or a user-pasted chat image. Path is relative to the
// managed data root.
type PlanAttachment struct {
	ID    string `json:"id"`
	Kind  string `json:"kind"` // artifact kind (html, md, png, …) or image ext
	Title string `json:"title,omitempty"`
	Path  string `json:"path"`
}

// Text renders the plan as prose — the bridge to every existing consumer of the
// free-text plan (DefinitionOfDone prompts, the plan editor fallback). Other
// agents execute from this string, so it prefers the detailed Body over the
// headline Summary and includes live step status plus attachment paths.
func (p Plan) Text() string {
	var b strings.Builder
	body := strings.TrimSpace(p.Body)
	if body != "" {
		b.WriteString(body)
	} else {
		b.WriteString(strings.TrimSpace(p.Summary))
	}
	if len(p.Steps) > 0 {
		b.WriteString("\n\nSteps:\n")
		for i, s := range p.Steps {
			b.WriteString(fmt.Sprintf("%d. %s", i+1, s.Title))
			if s.Status == StepDone || s.Status == StepSkipped {
				b.WriteString(" (" + s.Status + ")")
			}
			b.WriteString("\n")
		}
	}
	if len(p.Attachments) > 0 {
		b.WriteString("\n\nAttachments:\n")
		for _, a := range p.Attachments {
			title := strings.TrimSpace(a.Title)
			path := strings.TrimSpace(a.Path)
			switch {
			case path != "" && title != "" && title != path:
				b.WriteString("- `")
				b.WriteString(path)
				b.WriteString("` - ")
				b.WriteString(title)
			case path != "":
				b.WriteString("- `")
				b.WriteString(path)
				b.WriteString("`")
			default:
				b.WriteString("- ")
				b.WriteString(title)
			}
			b.WriteString("\n")
		}
	}
	return strings.TrimSpace(b.String())
}

// Board serializes the live execution plan for a replan turn: ids, every
// status (including doing), files, depends_on, child ownership, and notes.
// Text() omits those so it stays a prose definition of done.
func (p Plan) Board() string {
	var b strings.Builder
	body := strings.TrimSpace(p.Body)
	if body != "" {
		b.WriteString(body)
	} else {
		b.WriteString(strings.TrimSpace(p.Summary))
	}
	if len(p.Steps) > 0 {
		b.WriteString("\n\nSteps:\n")
		for _, s := range p.Steps {
			st := s.Status
			if st == "" {
				st = StepPending
			}
			fmt.Fprintf(&b, "%s [%s] %s", s.ID, st, s.Title)
			if len(s.Files) > 0 {
				b.WriteString(" files=" + strings.Join(s.Files, ","))
			}
			if len(s.DependsOn) > 0 {
				b.WriteString(" after=" + strings.Join(s.DependsOn, ","))
			}
			if ws := strings.TrimSpace(s.Workspace); ws != "" {
				b.WriteString(" child=" + ws)
			}
			if n := strings.TrimSpace(s.Note); n != "" {
				b.WriteString(" note=" + n)
			}
			b.WriteString("\n")
		}
	}
	return strings.TrimSpace(b.String())
}

// Progress is the fraction of steps completed (done or skipped), and ok=false
// when the plan has no steps to measure.
func (p Plan) Progress() (frac float64, ok bool) {
	if len(p.Steps) == 0 {
		return 0, false
	}
	done := 0
	for _, s := range p.Steps {
		if s.Status == StepDone || s.Status == StepSkipped {
			done++
		}
	}
	return float64(done) / float64(len(p.Steps)), true
}

// PlanStep is one concrete, ordered action in a Plan. The engine assigns IDs
// and owns status transitions (agents request them via update_plan_step).
type PlanStep struct {
	ID     string `json:"id"`
	Title  string `json:"title"`
	Status string `json:"status"` // pending | doing | done | skipped
	Note   string `json:"note,omitempty"`
	// Files the step touches (repo-relative). Steps with disjoint files and
	// satisfied DependsOn may run in parallel as child workspaces.
	Files []string `json:"files,omitempty"`
	// DependsOn lists step ids that must be done or skipped first.
	DependsOn []string `json:"dependsOn,omitempty"`
	// Workspace is the child workspace working this step, when fanned out.
	Workspace string `json:"workspace,omitempty"`
}

// Ready reports whether a pending step's dependencies are all done/skipped.
func (p Plan) Ready(s PlanStep) bool {
	if s.Status != StepPending {
		return false
	}
	byID := make(map[string]PlanStep, len(p.Steps))
	for _, st := range p.Steps {
		byID[st.ID] = st
	}
	for _, dep := range s.DependsOn {
		d, ok := byID[dep]
		if !ok {
			continue // unknown id: never block on a typo
		}
		if d.Status != StepDone && d.Status != StepSkipped {
			return false
		}
	}
	return true
}

// ChildFanOut is the parent board's record on a child workspace (see
// Workspace.FanOut).
type ChildFanOut struct {
	Wave    int      `json:"wave"`
	After   []string `json:"after,omitempty"`   // parent step ids whose landing started the wave
	Outcome string   `json:"outcome,omitempty"` // merged | failed | conflict: <files>
	Note    string   `json:"note,omitempty"`    // the child's handoff as merged
	// Chat is the parent chat tab that created a chat child (create_workspace);
	// its report turn and card go back to that tab. Empty on loop fan-out.
	Chat string `json:"chat,omitempty"`
	// Mode is the chat child's first-tab chat mode (agent, plan, loop…).
	Mode string `json:"mode,omitempty"`
}

// Checkpoint is a resumable position in a loop's state machine (D55): the phase to
// re-enter plus the convergence iteration it belongs to. Only in-flight phases are
// checkpointed — a terminal loop has nothing to resume.
// At is an RFC3339 timestamp rather than a time.Time: every other type in this
// package keeps to JSON/Wails-friendly primitives, and a time.Time here drags a
// generated `time` package into the frontend bindings.
type Checkpoint struct {
	State     WorkspaceState `json:"state"`
	Iteration int            `json:"iteration"`
	At        string         `json:"at"`
	// PhaseID is the [[loop.phase]] running now (empty on pre-walker records).
	PhaseID string `json:"phaseId,omitempty"`
	// ResumePhaseID is the parked graph continuation after an on-demand phase.
	ResumePhaseID string `json:"resumePhaseId,omitempty"`
}

// ConvPoint is one iteration's open-finding count for the convergence chart (PRD §7.3).
type ConvPoint struct {
	Iteration int    `json:"iteration"`
	Findings  int    `json:"findings"`
	Color     string `json:"color"`
}

// AgentRole is an app-owned role with a scoped tool set (PRD §8.2).
type AgentRole string

const (
	RoleMaestro  AgentRole = "maestro"
	RoleCoder    AgentRole = "coder"
	RoleReviewer AgentRole = "reviewer"
	RoleTester   AgentRole = "tester"
	RoleQA       AgentRole = "qa"
)

// Agent is a spawned specialist or the Maestro. Inspection is read-only (PRD §8.5).
type Agent struct {
	ID          string      `json:"id"`
	WorkspaceID string      `json:"workspaceId"`
	Code        string      `json:"code"` // 2-letter avatar: MA/CO/QA/RV/TE/PR
	Name        string      `json:"name"`
	Role        string      `json:"role"`      // descriptive subtitle
	Model       string      `json:"model"`     // opus/sonnet
	Status      string      `json:"status"`    // directing/working/idle
	SessionID   string      `json:"sessionId"` // claude session id (PRD §8.1, for resume/inspection)
	Color       string      `json:"color"`
	Task        string      `json:"task"`
	Cur         int         `json:"cur"` // index of the current (in-progress) step, -1 if none
	Steps       []AgentStep `json:"steps"`
	// Unseen is true when a chat-facing agent (PA*/MA) finished a turn while the
	// human has not yet viewed that tab. Cleared by MarkWorkspaceChatSeen.
	// Specialist agents (CO/RV/…) never set this — they are not chat tabs.
	Unseen bool `json:"unseen,omitempty"`
}

// AgentStep is one line in the "see into an agent" stream, backed by stream-json events.
type AgentStep struct {
	Kind  string    `json:"kind"` // plan/spawn/watch/read/edit/run/note/done/open/click/check/ok/cur
	Text  string    `json:"text"`
	Diff  *DiffStat `json:"diff,omitempty"`
	State string    `json:"state,omitempty"` // run/edit/read/ok/cur tint
}

// Feed item kinds (the typed classification the frontend used to regex out of
// rendered HTML). Stamped by the engine at the source so the UI
// reads a field instead of screen-scraping its own English outcome phrases.
const (
	FeedKindNote      = "note"      // agent narration / free text
	FeedKindTool      = "tool"      // a tool call (carries ToolUseID)
	FeedKindOutcome   = "outcome"   // an engine-authored milestone (committed, converged, opened PR…)
	FeedKindDeviation = "deviation" // a logged departure from a default (IsDeviation)
)

// FeedItem is an activity-feed entry (PRD §15.2 loop.event). Deviations render specially.
type FeedItem struct {
	ID        string `json:"id"`
	AgentCode string `json:"agentCode"`
	Name      string `json:"name"`
	Text      string `json:"text"` // may contain inline <code> (seed data is trusted)
	Time      string `json:"time"`
	// Kind is the typed classification (note|tool|outcome|deviation). Empty on
	// legacy/pre-typed rows and seed data, where the frontend falls back to its
	// heuristic. New engine-emitted items always set it.
	Kind        string `json:"kind,omitempty"`
	ClickAgent  string `json:"clickAgent,omitempty"`
	IsDeviation bool   `json:"isDeviation"`
	Rule        string `json:"rule,omitempty"` // for deviations: the default departed from
	Why         string `json:"why,omitempty"`
	// Optional tool-call correlation: set on KindStep feed rows, patched when the
	// matching tool_result arrives so Activity can show input + result on expand.
	ToolUseID   string `json:"toolUseId,omitempty"`
	ToolResult  string `json:"toolResult,omitempty"`
	IsToolError bool   `json:"isToolError,omitempty"`
	// Ts is the real event timestamp (unix ms), set on rows read back from the
	// durable events table — the Loop chat cockpit interleaves facts between
	// Maestro turns and specialist cards by it. Live-emitted rows may leave it 0 ("now").
	Ts int64 `json:"ts,omitempty"`
	// ChatCode is the conductor tab this fact belongs to (one chat = one loop).
	ChatCode string `json:"chatCode,omitempty"`
	// Card names a typed cockpit card ("fanout") rendered from Data (JSON); Text
	// stays the plain fallback for feeds that do not know the card. A card is
	// re-emitted under the same ID as it progresses; consumers replace by ID.
	Card string `json:"card,omitempty"`
	Data string `json:"data,omitempty"`
}

// Typed cockpit cards (FeedItem.Card).
const (
	// FeedCardFanOut: children of one board round and their outcomes (engine fanout.go).
	FeedCardFanOut = "fanout"
	// FeedCardPlan: a filed structured plan — summary + steps (engine plan.go).
	FeedCardPlan = "plan"
)

// StoredEvent is one row of the append-only durable event log (PRD §6.5, §14;
// DECISIONS D13). It is the persisted form of an engine event: the raw payload is
// kept as JSON so the feed (and, later, agent inspection) can be reconstructed on
// reload. Not exposed over the Wails bindings directly — the feed is derived from it.
type StoredEvent struct {
	ID          int64  `json:"id"`
	WorkspaceID string `json:"workspaceId"`
	AgentID     string `json:"agentId,omitempty"`
	Type        string `json:"type"` // event name, e.g. "workspace.event" / "workspace.updated"
	PayloadJSON string `json:"payloadJson"`
	TS          int64  `json:"ts"` // unix milliseconds
}

// Severity of a reviewer finding.
type Severity string

const (
	SevInfo     Severity = "info"
	SevLow      Severity = "low"
	SevMedium   Severity = "medium"
	SevHigh     Severity = "high"
	SevCritical Severity = "critical"
)

// FindingStatus is the lifecycle of a reviewer finding.
type FindingStatus string

const (
	FindingOpen     FindingStatus = "open"     // active; counts toward convergence
	FindingResolved FindingStatus = "resolved" // coder (or engine) marked addressed
	FindingIgnored  FindingStatus = "ignored"  // human: out of scope / not worth fixing
	FindingInvalid  FindingStatus = "invalid"  // coder or human: finding is wrong / disputed
)

// FindingActor is who last changed a finding's status (attribution).
const (
	FindingActorHuman    = "human"
	FindingActorCoder    = "coder"
	FindingActorReviewer = "reviewer"
	FindingActorEngine   = "engine"
	FindingSourceUser    = "user"
)

// ValidFindingStatus reports whether s is a known finding status.
func ValidFindingStatus(s string) bool {
	switch FindingStatus(s) {
	case FindingOpen, FindingResolved, FindingIgnored, FindingInvalid:
		return true
	default:
		return false
	}
}

// FindingNote is an append-only operator comment on a finding (humans write;
// agents read via list_findings). Independent of statusReason.
type FindingNote struct {
	ID        string `json:"id"`     // UUID
	Author    string `json:"author"` // FindingActor* (human for v1)
	Body      string `json:"body"`
	CreatedAt string `json:"createdAt"` // RFC3339
}

// Finding is a structured reviewer report (PRD §8.3 report_finding) driving convergence.
type Finding struct {
	ID           string        `json:"id"`      // UUID — internal storage key
	ShortID      string        `json:"shortId"` // find_… — agents and humans; unique per workspace
	WorkspaceID  string        `json:"workspaceId"`
	Iteration    int           `json:"iteration"`
	File         string        `json:"file"`
	Line         int           `json:"line"`              // 1-based start (0 if not line-specific)
	EndLine      int           `json:"endLine,omitempty"` // inclusive end; 0 or == Line → single line
	Severity     Severity      `json:"severity"`
	Message      string        `json:"message"`
	Summary      string        `json:"summary,omitempty"`      // optional user-facing digest (markdown)
	Status       string        `json:"status"`                 // open/resolved/ignored/invalid
	StatusReason string        `json:"statusReason,omitempty"` // required when coder marks invalid
	CreatedAt    string        `json:"createdAt,omitempty"`    // RFC3339 — stamped on first persist
	UpdatedBy    string        `json:"updatedBy,omitempty"`    // human|coder|reviewer|engine
	UpdatedAt    string        `json:"updatedAt,omitempty"`    // RFC3339
	Notes        []FindingNote `json:"notes,omitempty"`
	// Source is "user" for operator notes from the Observatory review surface;
	// empty means a reviewer/agent finding.
	Source string `json:"source,omitempty"`
}

// PublicID is the identifier agents use (short id). Falls back to ID for
// legacy rows that have not been backfilled.
func (f Finding) PublicID() string {
	if f.ShortID != "" {
		return f.ShortID
	}
	return f.ID
}

// MatchesRef reports whether ref is this finding's UUID or public short id.
func (f Finding) MatchesRef(ref string) bool {
	return ref != "" && (f.ID == ref || f.ShortID == ref)
}

// FindingLoc formats file[:line] or file:start-end for display/prompts.
func FindingLoc(file string, line, endLine int) string {
	if file == "" {
		return ""
	}
	if line <= 0 {
		return file
	}
	if endLine > line {
		return fmt.Sprintf("%s:%d-%d", file, line, endLine)
	}
	return fmt.Sprintf("%s:%d", file, line)
}

// CoversLine reports whether a 1-based line falls in this finding's span.
func (f Finding) CoversLine(lineNo int) bool {
	if f.Line <= 0 || lineNo <= 0 {
		return false
	}
	if f.EndLine > f.Line {
		return lineNo >= f.Line && lineNo <= f.EndLine
	}
	return lineNo == f.Line
}

// MergeMethod is the strategy used to integrate a PR branch into its base,
// matching GitHub's three options and gh pr merge flags (D53).
type MergeMethod string

const (
	MergeMethodMerge  MergeMethod = "merge"  // create a merge commit (--no-ff / --merge)
	MergeMethodSquash MergeMethod = "squash" // squash and merge (--squash)
	MergeMethodRebase MergeMethod = "rebase" // rebase and merge (--rebase)
)

// NormalizeMergeMethod returns a valid MergeMethod, falling back to MergeMethodMerge.
func NormalizeMergeMethod(s string) MergeMethod {
	switch MergeMethod(s) {
	case MergeMethodMerge, MergeMethodSquash, MergeMethodRebase:
		return MergeMethod(s)
	default:
		return MergeMethodMerge
	}
}

// EffectiveMergeMethod resolves the merge strategy: project override wins (if
// valid), then global default (if valid), then the hardcoded fallback (merge
// commit). Invalid values are silently skipped so they don't shadow a valid
// setting at a lower tier.
func EffectiveMergeMethod(proj Project, settings Settings) MergeMethod {
	if isValidMergeMethod(proj.MergeMethod) {
		return proj.MergeMethod
	}
	if isValidMergeMethod(settings.DefaultMergeMethod) {
		return settings.DefaultMergeMethod
	}
	return MergeMethodMerge
}

func isValidMergeMethod(m MergeMethod) bool {
	switch m {
	case MergeMethodMerge, MergeMethodSquash, MergeMethodRebase:
		return true
	default:
		return false
	}
}

// MergeMethodPermitted reports whether method is allowed on this repo.
// An empty list means unknown (not fetched) — every method is permitted.
func MergeMethodPermitted(allowed []string, m MergeMethod) bool {
	if len(allowed) == 0 {
		return true
	}
	for _, a := range allowed {
		if MergeMethod(a) == m && isValidMergeMethod(m) {
			return true
		}
	}
	return false
}

// MergeMethodLabel returns a human-readable label for a merge method.
func MergeMethodLabel(m MergeMethod) string {
	switch m {
	case MergeMethodSquash:
		return "Squash and merge"
	case MergeMethodRebase:
		return "Rebase and merge"
	default:
		return "Merge"
	}
}

// UpdateBaseMethod is how a PR branch catches up with its base (D54).
// Independent of MergeMethod (how a PR lands onto the base).
type UpdateBaseMethod string

const (
	UpdateBaseMerge  UpdateBaseMethod = "merge"  // git merge <base> into the worktree
	UpdateBaseRebase UpdateBaseMethod = "rebase" // git rebase <base> in the worktree
)

// NormalizeUpdateBaseMethod returns a valid UpdateBaseMethod, falling back to merge.
func NormalizeUpdateBaseMethod(s string) UpdateBaseMethod {
	switch UpdateBaseMethod(s) {
	case UpdateBaseMerge, UpdateBaseRebase:
		return UpdateBaseMethod(s)
	default:
		return UpdateBaseMerge
	}
}

// EffectiveUpdateBaseMethod resolves the update-with-base strategy: project
// override wins (if valid), then global default (if valid), then "merge".
func EffectiveUpdateBaseMethod(proj Project, settings Settings) UpdateBaseMethod {
	if isValidUpdateBaseMethod(proj.UpdateBaseMethod) {
		return proj.UpdateBaseMethod
	}
	if isValidUpdateBaseMethod(settings.DefaultUpdateBaseMethod) {
		return settings.DefaultUpdateBaseMethod
	}
	return UpdateBaseMerge
}

func isValidUpdateBaseMethod(m UpdateBaseMethod) bool {
	switch m {
	case UpdateBaseMerge, UpdateBaseRebase:
		return true
	default:
		return false
	}
}

// UpdateBaseMethodLabel returns a short human-readable label.
func UpdateBaseMethodLabel(m UpdateBaseMethod) string {
	switch m {
	case UpdateBaseRebase:
		return "rebase"
	default:
		return "merge"
	}
}

// SquashCommitMessage is the squash-merge commit message format (D65), matching
// GitHub's repo setting minus "Default message".
type SquashCommitMessage string

const (
	SquashCommitTitle               SquashCommitMessage = "title"
	SquashCommitTitleAndCommits     SquashCommitMessage = "title-and-commits"
	SquashCommitTitleAndDescription SquashCommitMessage = "title-and-description"
)

// NormalizeSquashCommitMessage returns a valid squash message format, falling
// back to title-and-commits (the Recur default).
func NormalizeSquashCommitMessage(s string) SquashCommitMessage {
	if isValidSquashCommitMessage(SquashCommitMessage(s)) {
		return SquashCommitMessage(s)
	}
	return SquashCommitTitleAndCommits
}

// EffectiveSquashCommitMessage resolves squash commit message format: project
// override wins (if valid), then global default (if valid), then title-and-commits.
func EffectiveSquashCommitMessage(proj Project, settings Settings) SquashCommitMessage {
	if isValidSquashCommitMessage(proj.SquashCommitMessage) {
		return proj.SquashCommitMessage
	}
	if isValidSquashCommitMessage(settings.DefaultSquashCommitMessage) {
		return settings.DefaultSquashCommitMessage
	}
	return SquashCommitTitleAndCommits
}

func isValidSquashCommitMessage(m SquashCommitMessage) bool {
	switch m {
	case SquashCommitTitle, SquashCommitTitleAndCommits, SquashCommitTitleAndDescription:
		return true
	default:
		return false
	}
}

// MergeCommitMessage is the merge-commit message format (D65), matching GitHub's
// repo setting minus "Default message".
type MergeCommitMessage string

const (
	MergeCommitTitle               MergeCommitMessage = "title"
	MergeCommitTitleAndDescription MergeCommitMessage = "title-and-description"
)

// NormalizeMergeCommitMessage returns a valid merge-commit message format,
// falling back to title (the Recur default).
func NormalizeMergeCommitMessage(s string) MergeCommitMessage {
	if isValidMergeCommitMessage(MergeCommitMessage(s)) {
		return MergeCommitMessage(s)
	}
	return MergeCommitTitle
}

// EffectiveMergeCommitMessage resolves merge-commit message format: project
// override wins (if valid), then global default (if valid), then title.
func EffectiveMergeCommitMessage(proj Project, settings Settings) MergeCommitMessage {
	if isValidMergeCommitMessage(proj.MergeCommitMessage) {
		return proj.MergeCommitMessage
	}
	if isValidMergeCommitMessage(settings.DefaultMergeCommitMessage) {
		return settings.DefaultMergeCommitMessage
	}
	return MergeCommitTitle
}

func isValidMergeCommitMessage(m MergeCommitMessage) bool {
	switch m {
	case MergeCommitTitle, MergeCommitTitleAndDescription:
		return true
	default:
		return false
	}
}

// Workspace land-state for the hand-driven titlebar (Commit / Update / Merge).
const (
	UpdateWorkspaceUpdated   = "updated"
	UpdateWorkspaceConflict  = "conflict"
	UpdateWorkspaceNoop      = "noop"
	UpdateWorkspaceAutostash = "autostash" // merge/rebase ok; leftover edits left in git stash
)

// WorkspaceLandState is the titlebar's Commit/Update/Merge (or Push) switch:
// dirty uncommitted work, commits behind/ahead of the merge base, or an
// in-progress merge/rebase (conflict markers). AheadBase is unique commits on
// the workspace branch; Merge is hidden when it is 0 (HEAD is the merge-base).
// On the main working copy, Base/BehindBase/AheadBase are versus the tracking
// remote (origin/main) and Remote is the remote name for Push; both are empty
// when the branch has no upstream. When an open PR exists, PRID/PRNumber/
// PRSource drive View PR vs Open PR; AheadRemote drives Push updates;
// BehindRemote drives Pull updates.
type WorkspaceLandState struct {
	BehindBase int    `json:"behindBase"`
	AheadBase  int    `json:"aheadBase"`
	Dirty      bool   `json:"dirty"`
	Unmerged   bool   `json:"unmerged"`
	Base       string `json:"base"`
	Remote     string `json:"remote,omitempty"`
	// RequirePR is the last-known GitHub branch-rules answer (whether that
	// remote refuses a land without a pull request). Land state never waits
	// on GitHub after the first probe; a stale value is returned immediately
	// and refreshed in the background. True when unknown or the probe failed.
	RequirePR bool `json:"requirePR"`
	// Open (non-merged) PR for this workspace, if any. AheadRemote / BehindRemote
	// are unique commits versus the GitHub PR's remote head (0 for Local PRs).
	PRID         string `json:"prId,omitempty"`
	PRNumber     int    `json:"prNumber,omitempty"`
	PRSource     string `json:"prSource,omitempty"`
	PRDraft      bool   `json:"prDraft,omitempty"`
	AheadRemote  int    `json:"aheadRemote,omitempty"`
	BehindRemote int    `json:"behindRemote,omitempty"`
}

// UpdateWorkspaceResult is the outcome of UpdateWorkspaceWithBase /
// UpdateWorkingCopyFromRemote. Status is updated (clean git), conflict
// (markers left — invoke /update), noop (already up to date), or autostash
// (merge/rebase succeeded but leftover dirty edits could not be reapplied
// and were left in git stash).
type UpdateWorkspaceResult struct {
	Status      string    `json:"status"`
	Workspace   Workspace `json:"workspace"`
	Base        string    `json:"base"`
	PushedPR    int       `json:"pushedPR,omitempty"`
	ForcePushed bool      `json:"forcePushed,omitempty"`
}

// PRSource distinguishes a GitHub PR from a Local (worktree-diff) PR (PRD §12).
type PRSource string

const (
	SourceGitHub PRSource = "gh"
	SourceLocal  PRSource = "local"
)

// Attention reason codes on PR.AttentionReasons. The dashboard and PR page
// map these to labels so "Needs your attention" is never a mystery bucket.
const (
	AttentionReviewRequested  = "review_requested"
	AttentionAssigned         = "assigned"
	AttentionChangesRequested = "changes_requested"
	AttentionChecksFailing    = "checks_failing"
	// AttentionAwaitingReview is a legacy alias of AttentionReviewRequested
	// (local PRs that need the viewer). The UI treats both as "Needs your review".
	AttentionAwaitingReview = "awaiting_review"
)

// PR is the human review boundary — one review UI for both sources (PRD §12).
type PR struct {
	ID          string `json:"id"`
	WorkspaceID string `json:"workspaceId"`
	// ProjectID is the Recur project whose GitHub repo this PR belongs to.
	// Set even when WorkspaceID is empty (a GitHub PR Recur imported, not opened).
	ProjectID string   `json:"projectId,omitempty"`
	Group     string   `json:"group"` // you/loop/open/merged for the PR list
	Number    int      `json:"number"`
	Source    PRSource `json:"source"`
	// URL is the GitHub pull request page (https://github.com/org/repo/pull/N).
	// Set from `gh pr view` for source=gh; empty for local PRs.
	URL string `json:"url,omitempty"`
	// HeadSHA is the GitHub PR head commit (headRefOid); empty for local PRs.
	HeadSHA string `json:"headSha,omitempty"`
	// Draft is a GitHub draft PR (gh pr create --draft). Stored on Local
	// fallbacks so a later retry keeps the same intent.
	Draft     bool          `json:"draft,omitempty"`
	Branch    string        `json:"branch"`
	Base      string        `json:"base"`
	Title     string        `json:"title"`
	Summary   []SummaryPara `json:"summary"`
	Focus     FocusNote     `json:"focus"`
	State     string        `json:"state"` // "In review" / "Merged"
	Diff      DiffStat      `json:"diff"`
	FileCount int           `json:"fileCount"`
	ChecksOK  bool          `json:"checksOk"`
	// HasChecks is true when GitHub (or Recur) reported at least one check run.
	// Distinct from ChecksOK, which is true when nothing has failed: checks
	// passed, are still running, or have not been reported yet.
	HasChecks bool   `json:"hasChecks,omitempty"`
	When      string `json:"when"`
	// CreatedAt is when the PR was opened: GitHub `createdAt` for source=gh,
	// Recur persist time for Local PRs. RFC3339. Dashboard newest/oldest sort.
	CreatedAt string `json:"createdAt,omitempty"`
	// UpdatedAt is the last activity time: GitHub `updatedAt` for source=gh,
	// Recur persist time for Local PRs. RFC3339. Dashboard recently-updated sort.
	UpdatedAt string      `json:"updatedAt,omitempty"`
	Commits   []Commit    `json:"commits"`
	Files     []FileDiff  `json:"files"`
	Comments  []PRComment `json:"comments"`
	// PendingComments are line notes not yet submitted with a review (GitHub-style).
	PendingComments []PendingLineComment `json:"pendingComments,omitempty"`
	Verify          []VerifyStep         `json:"verify"`
	Checks          []Check              `json:"checks"`
	// BehindBase is how many commits the base branch is ahead of the PR branch
	// (0 = up to date). Refreshed on GetPR from the live worktree; not a historical
	// record. Drives the "out of date with base" banner + Update-with-base CTA.
	BehindBase int `json:"behindBase,omitempty"`
	// UpdatingBase is the in-flight Update-with-base phase (D54 UX).
	// "" = idle, "git" = merge/rebase running, "coder" = conflict Coder turn.
	UpdatingBase string `json:"updatingBase,omitempty"`
	// Impact is the observatory blast-radius for this PR (APIs/symbols/tests touched).
	Impact []ImpactItem `json:"impact,omitempty"`
	// AuthorLogin is the GitHub user who opened the PR (source=gh).
	AuthorLogin string `json:"authorLogin,omitempty"`
	// Participants are people on this PR: author, assignees, reviewers, and
	// commenters. The dashboard stacks their avatars. Deduped; author first.
	Participants []PRParticipant `json:"participants,omitempty"`
	// Reviewers are people asked to review, or who already reviewed.
	// State is pending, approved, changes_requested, or commented.
	Reviewers []PRReviewer `json:"reviewers,omitempty"`
	// ViewerLogin is the authenticated gh user looking at this PR.
	ViewerLogin string `json:"viewerLogin,omitempty"`
	// ViewerCanWrite is true when the viewer has push/maintain/admin on the repo.
	// Approve and Request changes need this; a plain conversation comment does not.
	ViewerCanWrite bool `json:"viewerCanWrite,omitempty"`
	// NodeID is the GitHub GraphQL id for this pull request (used to react on
	// the description).
	NodeID string `json:"nodeId,omitempty"`
	// Mergeable is GitHub's conflict bit: MERGEABLE, CONFLICTING, or UNKNOWN.
	// Empty on Local PRs and when gh has not returned a value yet.
	Mergeable string `json:"mergeable,omitempty"`
	// MergeStateStatus is GitHub's mergeability bag: CLEAN, DIRTY, BLOCKED,
	// BEHIND, UNSTABLE, HAS_HOOKS, UNKNOWN. Empty on Local PRs.
	MergeStateStatus string `json:"mergeStateStatus,omitempty"`
	// ReviewDecision is GitHub's review gate: APPROVED, CHANGES_REQUESTED,
	// REVIEW_REQUIRED, or empty when reviews are not required / unknown.
	ReviewDecision string `json:"reviewDecision,omitempty"`
	// ReviewRequested is true when GitHub has an outstanding review request
	// for the authenticated user. Distinct from ReviewDecision, which is the
	// repo's review gate (someone must review), not "you were asked."
	ReviewRequested bool `json:"reviewRequested,omitempty"`
	// AttentionReasons is why this PR is in the "Needs your attention" list.
	// Codes: review_requested (legacy awaiting_review means the same), assigned,
	// changes_requested, checks_failing. Empty when the PR does not need the viewer.
	AttentionReasons []string `json:"attentionReasons,omitempty"`
	// IsCrossRepository is true when the PR head lives in a fork, not this repo.
	IsCrossRepository bool `json:"isCrossRepository,omitempty"`
	// AllowedMergeMethods is the repo's enabled GitHub merge types
	// (merge / squash / rebase). Empty means unknown — show all three.
	AllowedMergeMethods []string `json:"allowedMergeMethods,omitempty"`
	// MergeQueueEnabled is GitHub's isMergeQueueEnabled: the base branch
	// requires a merge queue, so Merge enqueues instead of merging.
	MergeQueueEnabled bool `json:"mergeQueueEnabled,omitempty"`
	// MergeQueued is true while the PR sits in GitHub's merge queue or waits
	// on auto-merge to join it.
	MergeQueued bool `json:"mergeQueued,omitempty"`
}

// ViewerIsAuthor reports whether the authenticated gh user opened this PR.
func (p PR) ViewerIsAuthor() bool {
	return p.ViewerLogin != "" && strings.EqualFold(p.ViewerLogin, p.AuthorLogin)
}

// CanEditDetails reports whether the viewer can change the title and body.
// Local PRs are always editable while open. GitHub matches hosted GH: author
// or write access. Merged and closed PRs are locked.
func (p PR) CanEditDetails() bool {
	if p.ConversationLocked() {
		return false
	}
	if p.Source != SourceGitHub {
		return true
	}
	return p.ViewerIsAuthor() || p.ViewerCanWrite
}

// ConversationLocked is true for merged or closed PRs. Recur does not allow
// new comments or reviews on a settled pull request.
func (p PR) ConversationLocked() bool {
	s := strings.ToLower(strings.TrimSpace(p.State))
	return p.Group == "merged" || strings.Contains(s, "merged") || strings.Contains(s, "closed")
}

// CanApproveReview reports whether Approve / Request changes should be offered.
// GitHub forbids reviewing your own pull request (no org setting overrides this).
// Merged/closed PRs are locked. Local PRs and an unknown viewer still offer it;
// write access is required otherwise.
func (p PR) CanApproveReview() bool {
	if p.ConversationLocked() {
		return false
	}
	if p.Source != SourceGitHub {
		return true
	}
	if p.ViewerIsAuthor() {
		return false
	}
	if p.ViewerLogin == "" {
		return true
	}
	return p.ViewerCanWrite
}

// CanEditComment reports whether the viewer can edit this thread row.
func (p PR) CanEditComment(c PRComment) bool {
	if c.Kind == "action" {
		return false
	}
	if p.Source != SourceGitHub {
		return c.Author == "You"
	}
	return p.ViewerLogin != "" && strings.EqualFold(p.ViewerLogin, c.AuthorLogin)
}

// CanDeleteComment reports whether the viewer can delete this thread row.
// Line notes and standalone conversation comments are deletable (own row, or
// any row with write access). Submitted reviews are not — GitHub's conversation
// UI and API only delete unpublished reviews.
func (p PR) CanDeleteComment(c PRComment) bool {
	if c.Kind == "action" || c.Kind == "description" || c.Kind == "hidden" {
		return false
	}
	if c.IsReviewCard() {
		return false
	}
	if p.CanEditComment(c) {
		return true
	}
	return p.Source == SourceGitHub && p.ViewerCanWrite
}

// CanResolveThread reports whether the viewer can resolve a line-note thread
// (GitHub: PR author or write access; local: always).
func (p PR) CanResolveThread() bool {
	if p.Source != SourceGitHub {
		return true
	}
	if p.ViewerCanWrite {
		return true
	}
	return p.ViewerLogin != "" && strings.EqualFold(p.ViewerLogin, p.AuthorLogin)
}

// CanReactComment reports whether the viewer can add or remove a reaction on
// this row. GitHub: any authenticated viewer on a reactable conversation item
// (description, comments, reviews, line notes). Local: the same shapes.
func (p PR) CanReactComment(c PRComment) bool {
	if c.Kind == "hidden" || strings.TrimSpace(c.CommitSHA) != "" {
		return false
	}
	if c.Kind == "action" && !c.IsReviewCard() {
		return false
	}
	if p.Source != SourceGitHub {
		return true
	}
	return p.ViewerLogin != ""
}

// SummaryPara is one block of the Maestro-written PR summary.
type SummaryPara struct {
	Kind  string   `json:"kind"` // p / bullets / decision
	HTML  string   `json:"html,omitempty"`
	Items []string `json:"items,omitempty"`
}

// FocusNote is the Maestro's "where to focus your review" callout (PRD §12.2).
type FocusNote struct {
	Title string `json:"title"`
	Body  string `json:"body"` // may contain <code>
}

// Commit is an agent-authored commit; messages encode iteration/role (PRD §11).
type Commit struct {
	Hash        string     `json:"hash"`
	SHA         string     `json:"sha,omitempty"` // full hash for copy; Hash stays short
	Message     string     `json:"message"`
	Body        string     `json:"body,omitempty"`   // message body after the subject
	Author      string     `json:"author,omitempty"` // git author or GitHub login
	AuthorRole  string     `json:"authorRole"`       // descriptive, e.g. "Coder"
	AgentCode   string     `json:"agentCode"`
	Color       string     `json:"color"`
	IterLabel   string     `json:"iterLabel"`
	Diff        DiffStat   `json:"diff"`
	When        string     `json:"when"`
	Files       []FileDiff `json:"files"`
	AvatarURL   string     `json:"avatarUrl,omitempty"`
	Verified    bool       `json:"verified,omitempty"`
	CheckStatus string     `json:"checkStatus,omitempty"` // pass, fail, pending
}

// FileDiff is one changed file with its diff lines (PRD §12.2 Files changed).
type FileDiff struct {
	Path    string     `json:"path"`
	OldPath string     `json:"oldPath,omitempty"` // set on renames (status r)
	Status  string     `json:"status"`            // m/a/d/r
	Diff    DiffStat   `json:"diff"`
	Lines   []DiffLine `json:"lines"`
}

// FileDiffResult is a files-changed list. Truncated means untracked paths past
// the scan cap were omitted — it is not itself a file row.
type FileDiffResult struct {
	Files     []FileDiff `json:"files"`
	Truncated bool       `json:"truncated,omitempty"`
}

// FileSearchHit is one ranked path from CodeService.SearchFiles (@-mentions / Quick Open).
type FileSearchHit struct {
	Path  string `json:"path"`
	Score int    `json:"score"`
	Dir   bool   `json:"dir"`
}

// OpenInTarget is one destination in the workspace "Open In" picker (Finder,
// Cursor, Terminal, …). Kind is "app"; the frontend appends local actions
// such as copy-path. Icon is a PNG data URL of the OS app icon when available.
type OpenInTarget struct {
	ID   string `json:"id"`
	Name string `json:"name"`
	Kind string `json:"kind"`
	Icon string `json:"icon,omitempty"`
}

// FileSearchResult is SearchFiles output: top hits plus index readiness so the
// UI can show "Loading…" / refresh while the first walk is still streaming.
type FileSearchResult struct {
	Hits     []FileSearchHit `json:"hits"`
	Building bool            `json:"building"`
	Ready    bool            `json:"ready"`
}

// DiffLine is one rendered diff row. CodeHTML/ExtraHTML hold pre-highlighted
// markup (seed data is hand-tagged; real diffs are escaped + highlighted in git).
type DiffLine struct {
	Old       string `json:"old"`
	New       string `json:"new"`
	Type      string `json:"type"` // ctx/add/del/hunk
	CodeHTML  string `json:"codeHtml"`
	ExtraHTML string `json:"extraHtml,omitempty"` // inline comment / risk flag block
}

// PendingLineComment is one unsaved inline note. It posts with SubmitReview
// (or SendBackToLoop), not when the reviewer types it.
type PendingLineComment struct {
	ID        string `json:"id"`
	File      string `json:"file"`
	Line      int    `json:"line"`
	Side      string `json:"side,omitempty"` // LEFT or RIGHT
	StartLine int    `json:"startLine,omitempty"`
	StartSide string `json:"startSide,omitempty"`
	Body      string `json:"body"`
	// FindingID is the observatory finding this draft was queued from, if any.
	FindingID string `json:"findingId,omitempty"`
}

// Review states on PRReviewer.State.
const (
	ReviewStatePending          = "pending"
	ReviewStateApproved         = "approved"
	ReviewStateChangesRequested = "changes_requested"
	ReviewStateCommented        = "commented"
)

// PRParticipant is one person on a pull request (dashboard avatar stack).
type PRParticipant struct {
	Login     string `json:"login,omitempty"`
	Name      string `json:"name,omitempty"`
	AvatarURL string `json:"avatarUrl,omitempty"`
	AvatarCSS string `json:"avatarCss,omitempty"`
	Code      string `json:"code,omitempty"`
}

// PRReviewer is someone asked to review this PR, or who already left a review.
type PRReviewer struct {
	Login     string `json:"login,omitempty"`
	Name      string `json:"name,omitempty"`
	AvatarURL string `json:"avatarUrl,omitempty"`
	AvatarCSS string `json:"avatarCss,omitempty"`
	Code      string `json:"code,omitempty"`
	// State is pending, approved, changes_requested, or commented.
	State string `json:"state"`
}

// PRComment is a review thread entry (human or agent) (PRD §14 pr_comments).
type PRComment struct {
	ID     string `json:"id"`
	Code   string `json:"code"` // avatar
	Author string `json:"author"`
	// AuthorLogin is the GitHub username when this row came from GH.
	AuthorLogin string `json:"authorLogin,omitempty"`
	AvatarCSS   string `json:"avatarCss"`
	// AvatarURL is a GitHub profile image when this row came from GH.
	AvatarURL string `json:"avatarUrl,omitempty"`
	Time      string `json:"time"`
	BodyHTML  string `json:"bodyHtml"`
	ReactHTML string `json:"reactHtml,omitempty"`
	// Kind is the GitHub timeline shape: comment (default), action, or description.
	Kind string `json:"kind,omitempty"`
	// Verb is the header action on a comment card: "commented", "approved",
	// "requested changes". Empty on description/action rows.
	Verb string `json:"verb,omitempty"`
	// BodyMD is source markdown; the UI renders it when set (GH comments).
	BodyMD string `json:"bodyMd,omitempty"`
	// At is the source timestamp (RFC3339) for ordering the GH timeline.
	At string `json:"at,omitempty"`
	// CommitSHA / Verified / CheckStatus describe a GH commit timeline row.
	CommitSHA   string `json:"commitSha,omitempty"`
	Verified    bool   `json:"verified,omitempty"`
	CheckStatus string `json:"checkStatus,omitempty"` // pass, fail, pending
	// URL is the GitHub permalink for this row (PR, comment, review, or commit).
	URL string `json:"url,omitempty"`
	// ParentID nests this row under another conversation card (a review and its
	// line comments render as one unit).
	ParentID string `json:"parentId,omitempty"`
	// ReviewID is the GitHub pull_request_review_id for an inline comment.
	ReviewID string `json:"reviewId,omitempty"`
	// Path / Line / Side / DiffHunk are the file context for a line note.
	Path     string `json:"path,omitempty"`
	Line     int    `json:"line,omitempty"`
	Side     string `json:"side,omitempty"`
	DiffHunk string `json:"diffHunk,omitempty"`
	// StartLine / StartSide are the first line of a GitHub multiline comment.
	// Empty when the note is a single line (StartLine == 0 or == Line).
	StartLine int    `json:"startLine,omitempty"`
	StartSide string `json:"startSide,omitempty"`
	// Outdated is true when GitHub nulled `line` (the commented rows are no
	// longer in the current diff). Line/StartLine then hold original_line for
	// the conversation label only — do not anchor to the live Files-changed rows.
	Outdated bool `json:"outdated,omitempty"`
	// InReplyToID is the line-note thread this reply belongs to.
	InReplyToID string `json:"inReplyToId,omitempty"`
	// ThreadID is the GitHub review-thread node (PRRT_…) or the local root id.
	ThreadID string `json:"threadId,omitempty"`
	// Resolved is true when this line-note conversation is marked done.
	Resolved bool `json:"resolved,omitempty"`
	// NodeID is the GitHub GraphQL id for addReaction/removeReaction.
	NodeID string `json:"nodeId,omitempty"`
	// Reactions are the GitHub emoji rollups on this row (count > 0).
	Reactions []PRReaction `json:"reactions,omitempty"`
}

// PRReaction is one GitHub reaction emoji and its rollup on a conversation item.
type PRReaction struct {
	// Content is the GitHub reaction name: +1, -1, laugh, hooray, confused,
	// heart, rocket, eyes.
	Content string `json:"content"`
	Count   int    `json:"count"`
	Mine    bool   `json:"mine,omitempty"`
}

// IsLineNote reports a file-anchored review comment (or a reply on one).
func (c PRComment) IsLineNote() bool {
	if strings.HasPrefix(c.ID, "gh-inline-") {
		return true
	}
	return strings.TrimSpace(c.Path) != "" || strings.TrimSpace(c.InReplyToID) != "" || strings.TrimSpace(c.DiffHunk) != ""
}

// IsReviewCard reports a submitted pull-request review (body or action row).
func (c PRComment) IsReviewCard() bool {
	return strings.HasPrefix(c.ID, "gh-review-")
}

// VerifyStep is one browser-verification evidence row (PRD §10, attach_evidence).
// Screenshot, when set, is the on-disk path to the screenshot the QA agent captured
// for this step (rendered as a thumbnail in the "Verified in browser" card).
type VerifyStep struct {
	HTML       string `json:"html"`
	Passed     bool   `json:"passed"`
	Screenshot string `json:"screenshot,omitempty"`
}

// Check is one entry in the checks rollup.
// Local PRs use Recur review/verify/typecheck rows; GitHub PRs use the GitHub
// Checks tab rollup (name/detail/passed/status).
type Check struct {
	Name   string `json:"name"`
	Detail string `json:"detail"`
	Passed bool   `json:"passed"`
	// Status is the GitHub check bucket when this row came from GH:
	// pass, fail, pending, skipping, cancel. Empty for Local Recur checks.
	Status string `json:"status,omitempty"`
	// URL is the GitHub check-run page when this row came from GitHub.
	// Pending rows prefer the PR Checks tab (`/pull/N/checks` or `/runs/ID`)
	// over Actions job URLs, which omit queued/expected checks.
	URL string `json:"url,omitempty"`
	// StartedAt is the GitHub check-run start (RFC3339) when known. The UI
	// ticks pending elapsed time from this until the next refresh; completed
	// rows keep Detail as the final duration.
	StartedAt string `json:"startedAt,omitempty"`
}

// EscalationOption is one reasoned choice the Maestro offers (PRD §13).
type EscalationOption struct {
	Text string `json:"text"`
	Tag  string `json:"tag"` // simplest / smoothest / recommended
	Desc string `json:"desc"`
}

// Escalation is a scoped question the loop pauses on (PRD §13). The most important
// behavior for adoption — the quality of the question is the product.
type Escalation struct {
	ID          string             `json:"id"`
	WorkspaceID string             `json:"workspaceId"`
	AgentCode   string             `json:"agentCode,omitempty"` // owning chat/walk; empty = workspace-scoped
	Question    string             `json:"question"`
	Context     string             `json:"context"`
	Options     []EscalationOption `json:"options"`
	Answer      string             `json:"answer"`
	// Choice is the structured option text when the human picked one.
	// Empty when they only typed free text. Distinct from Answer, which is the
	// single-line resolved value (free text wins, else the choice).
	Choice string `json:"choice,omitempty"`
	// FreeText is the freeform reply. Empty when they only picked an option.
	FreeText string `json:"freeText,omitempty"`
	Status   string `json:"status"` // open/answered
	// Kind is engine-stamped provenance for restart dispatch (agent/cap/
	// ensure_pr/plan). Agents cannot set this; option text is not used to
	// infer it once Kind is present. Empty on rows raised before this field.
	Kind string `json:"kind,omitempty"`
	// Applied is set when a loop goroutine has consumed the answer. An answered
	// but not-yet-applied escalation is the durable pending-answer state for a
	// delayed dispatch (no governor slot, Recover heal, queued drain).
	Applied  bool   `json:"applied"`
	AskedAgo string `json:"askedAgo"`
}

// UnmarshalJSON treats a missing applied field on an answered row as already
// consumed. Pre-Applied JSON has no field, and those answers were consumed
// in-process; replaying them after upgrade would re-steer or re-grant.
func (e *Escalation) UnmarshalJSON(data []byte) error {
	type alias Escalation
	var v alias
	if err := json.Unmarshal(data, &v); err != nil {
		return err
	}
	*e = Escalation(v)
	var probe struct {
		Applied *bool `json:"applied"`
	}
	if err := json.Unmarshal(data, &probe); err != nil {
		return err
	}
	if probe.Applied == nil && e.Status == "answered" {
		e.Applied = true
	}
	return nil
}

// UserQuestion is one parked ask_user prompt from an interactive agent. The
// calling chat turn ends so the tab is idle; answering starts a new resumed
// turn with the answer as user input. Not durable across app restarts. Unlike
// Escalation it does NOT put the workspace into StateEscalated.
type UserQuestion struct {
	ID          string             `json:"id"`
	WorkspaceID string             `json:"workspaceId"`
	AgentCode   string             `json:"agentCode"`
	SessionID   string             `json:"sessionId,omitempty"`
	Question    string             `json:"question"`
	Context     string             `json:"context,omitempty"`
	Options     []EscalationOption `json:"options,omitempty"`
	AskedAgo    string             `json:"askedAgo"`
}

// UserQuestionAnswer is the human's reply to a UserQuestion.
type UserQuestionAnswer struct {
	// Choice is the selected option text (or id) when the human picked one.
	Choice string `json:"choice,omitempty"`
	// FreeText is the freeform answer (card input or chat composer send).
	FreeText string `json:"freeText,omitempty"`
}

// ArtifactKind is a supported agent-created artifact type.
type ArtifactKind string

const (
	ArtifactHTML    ArtifactKind = "html"
	ArtifactSVG     ArtifactKind = "svg"
	ArtifactMermaid ArtifactKind = "mermaid"
	ArtifactPNG     ArtifactKind = "png"
	ArtifactJPEG    ArtifactKind = "jpeg"
	ArtifactCSV     ArtifactKind = "csv"
	ArtifactJSON    ArtifactKind = "json"
	ArtifactMD      ArtifactKind = "md"
)

// Artifact is one agent-created file under workspaces/<id>/artifacts/.
type Artifact struct {
	ID          string       `json:"id"`
	WorkspaceID string       `json:"workspaceId"`
	Title       string       `json:"title"`
	Kind        ArtifactKind `json:"kind"`
	// Path is relative to the managed data root (workspaces/<id>/artifacts/…).
	Path      string `json:"path"`
	CreatedAt string `json:"createdAt"` // RFC3339
	CreatedBy string `json:"createdBy,omitempty"`
}

// PermissionRequest is one live "may I run this tool?" question from an agent,
// raised when Settings.BypassPermissions is off. Unlike an Escalation it is not
// persisted: the agent subprocess is holding an open MCP request while it waits,
// so a request cannot outlive the turn that raised it. Answering it resolves
// that request; the app restarting abandons it along with the turn.
type PermissionRequest struct {
	ID          string `json:"id"`
	WorkspaceID string `json:"workspaceId"`
	AgentCode   string `json:"agentCode"`
	// Surface tells the UI how to ask: "loop" for unattended autopilot agents
	// (also raises an inbox row), versus "chat" for an in-flight PA*/MA chat
	// turn — those render inline in the transcript when that tab is focused.
	Surface string `json:"surface"`
	// Tool is the Claude Code tool being requested, e.g. "Bash" or "Write".
	Tool string `json:"tool"`
	// Input is the tool's arguments as raw JSON, rendered for the human so the
	// decision is informed (the command line, the file path, …).
	Input string `json:"input"`
	// Summary is a one-line human rendering of Tool+Input for compact surfaces.
	Summary  string `json:"summary"`
	AskedAgo string `json:"askedAgo"`
}

// PermissionDecision is the human's answer to a PermissionRequest.
type PermissionDecision struct {
	// Allow is the human's verdict. A denial carries Reason back to the model so
	// it can adapt rather than retry blindly.
	Allow  bool   `json:"allow"`
	Reason string `json:"reason,omitempty"`
	// Remember, on an allow, adds the tool to the turn's approved set so the
	// same tool does not re-prompt for the rest of that turn.
	Remember bool `json:"remember,omitempty"`
}

// Rail is one hard guardrail (PRD §9). Toggleable in Policy; enforced by the engine.
type Rail struct {
	Key   string `json:"key"`
	Title string `json:"title"`
	Desc  string `json:"desc"`
	On    bool   `json:"on"`
}

// Policy is the single global control surface (PRD §9, §18) — one row in v1.
type Policy struct {
	Autonomy AutonomyLevel `json:"autonomy"`
	Rails    []Rail        `json:"rails"`
}

// InboxItem is an actionable "what needs me" row (PRD §16.11, D5 actionable-only).
type InboxItem struct {
	ID          string `json:"id"`
	Group       string `json:"group"` // needs/ready/upd
	Kind        string `json:"kind"`  // pause/pr/loop/merge (icon)
	Color       string `json:"color"`
	Title       string `json:"title"` // may contain <b>
	Sub         string `json:"sub"`
	Time        string `json:"time"`
	Action      string `json:"action"`      // Answer/Review/View
	ActionClass string `json:"actionClass"` // urgent/go/""
	Unread      bool   `json:"unread"`
	Target      string `json:"target"` // route target, e.g. "loop:LOOP-2218" / "pr:482" / "view:pr"
	// WorkspaceID joins the row onto its Briefing workspace card ("" = app-level).
	WorkspaceID string `json:"workspaceId,omitempty"`
}

// Settings holds machine-global caps and agent/provider config (PRD §18).
type Settings struct {
	// Per-mode models are fully-qualified ModelRefs (`provider/id`), e.g.
	// `claude-code/opus` or `cursor/composer-2.5`. One slot per chat mode
	// (ask/plan/agent/review/loop) so dock defaults are 1:1 with agent modes.
	// The provider prefix routes the turn — there is no separate per-role
	// provider setting. Settings.DisabledProviders can hide a provider from
	// pickers without clearing credentials. Empty ModelReview falls back to ModelLoop. Loop pipeline
	// specialists reuse these slots: Maestro←Loop, Coder←Agent, Reviewer←Review.
	ModelAsk    string `json:"modelAsk"`
	ModelPlan   string `json:"modelPlan"`
	ModelAgent  string `json:"modelAgent"`
	ModelReview string `json:"modelReview"`
	ModelLoop   string `json:"modelLoop"`
	// ModelWorkspaceTitle is the cheap one-shot model for AI commit messages
	// (and similar titles). Empty → claude-code/claude-haiku-4-5-20251001. Cursor-only installs
	// should point this at a cheap Cursor model (Claude's Tools:"" path is
	// unavailable there).
	ModelWorkspaceTitle string `json:"modelWorkspaceTitle,omitempty"`
	// Per-mode model flags (dim → value), e.g. {"effort":"high","context":"1m"}
	// for Claude or {"fast":"true"} for Cursor. Empty map → provider defaults.
	FlagsAsk            map[string]string `json:"flagsAsk,omitempty"`
	FlagsPlan           map[string]string `json:"flagsPlan,omitempty"`
	FlagsAgent          map[string]string `json:"flagsAgent,omitempty"`
	FlagsReview         map[string]string `json:"flagsReview,omitempty"`
	FlagsLoop           map[string]string `json:"flagsLoop,omitempty"`
	FlagsWorkspaceTitle map[string]string `json:"flagsWorkspaceTitle,omitempty"`
	// ChildWorkspaceModel is how create_workspace picks the spawned tab's model and flags.
	// Empty or "parent" (default): copy the calling chat. "mode": use Settings → Models for the child's chat mode.
	ChildWorkspaceModel string `json:"childWorkspaceModel,omitempty"`
	// BypassPermissions is the machine-global permission switch, and it applies
	// to EVERY agent turn regardless of role or chat mode. Off (the default),
	// Claude turns run with --permission-mode default plus --permission-prompt-tool,
	// and Codex turns run with approval_policy=on-request so sandbox exceptions
	// reach the human via the same permission prompt UI. On, Claude uses
	// bypassPermissions and Codex uses danger-full-access + approval never —
	// nothing is ever asked about. Defaulting to off is the safe ship: the worst
	// case is an interruption, not an unreviewed command.
	//
	// Read-only intent for roles like the Reviewer is best-effort on top of this:
	// a narrow --allowedTools, --disallowedTools for the edit builtins, and the
	// role prompt. It is deliberately NOT expressed through the permission mode.
	BypassPermissions bool `json:"bypassPermissions"`
	// MaxLoops / MaxAgents are machine-global concurrency caps (PRD §6.4).
	// Zero → unlimited. Negative → defaults (4 loops, 8 agents).
	MaxLoops  int `json:"maxLoops"`
	MaxAgents int `json:"maxAgents"`
	// DefaultMergeMethod is the machine-global default merge strategy (D53).
	// Empty or unset → "merge" (create a merge commit).
	DefaultMergeMethod MergeMethod `json:"defaultMergeMethod,omitempty"`
	// DefaultUpdateBaseMethod is the machine-global default for "Update with base"
	// (D54). Empty or unset → "merge". Independent of DefaultMergeMethod.
	DefaultUpdateBaseMethod UpdateBaseMethod `json:"defaultUpdateBaseMethod,omitempty"`
	// DefaultSquashCommitMessage is the machine-global squash-merge commit
	// message format (D65). Empty or unset → "title-and-commits".
	DefaultSquashCommitMessage SquashCommitMessage `json:"defaultSquashCommitMessage,omitempty"`
	// DefaultMergeCommitMessage is the machine-global merge-commit message
	// format (D65). Empty or unset → "title".
	DefaultMergeCommitMessage MergeCommitMessage `json:"defaultMergeCommitMessage,omitempty"`

	// BranchNamePrefix is prepended to new workspace branches as prefix/slug.
	// nil/unset (legacy blobs) falls back to EffectiveBranchNamePrefix.
	// Pointer-to-empty means no prefix (the branch is just the slug).
	BranchNamePrefix *string `json:"branchNamePrefix,omitempty"`
	// BranchPrefixMode and BranchPrefixCustom are the pre-string schema.
	// EffectiveBranchNamePrefix still reads them when BranchNamePrefix is nil.
	// New writes store BranchNamePrefix; empty prefix also keeps mode "none"
	// so a dropped empty pointer cannot silently revert to recur.
	BranchPrefixMode   BranchPrefixMode `json:"branchPrefixMode,omitempty"`
	BranchPrefixCustom string           `json:"branchPrefixCustom,omitempty"`

	// --- Cursor provider ---

	// CursorAPIKey authenticates Cursor runs (SDK). Together with ProviderEnabled
	// it is what offers Cursor in pickers. Recur Connect mints this key; there
	// is no Settings field to paste one. Disconnect, turning Cursor off, and a
	// missing Connect file clear it.
	// Stored in the local settings blob, so it is only as protected as the rest
	// of Recur's data dir.
	CursorAPIKey string `json:"cursorApiKey,omitempty"`

	// CursorBridgePath is the launcher path last written by Install. Locate
	// always uses <dataDir>/cursor-bridge; this field is display/history only.
	CursorBridgePath string `json:"cursorBridgePath,omitempty"`

	// CursorStrictConfig suppresses the user's ambient Cursor configuration
	// (~/.cursor plus the repo's own .cursor/rules and AGENTS.md) for agent
	// runs. Default false, matching how the Claude provider lets a repo's
	// CLAUDE.md load — a repo's own rules are usually wanted context.
	CursorStrictConfig bool `json:"cursorStrictConfig,omitempty"`

	// CursorAttributeCommits, when true, lets Cursor agents append the
	// "Made with Cursor" trailer to git commits they create. Default false —
	// Recur patches the bridge so attribution stays off unless opted in.
	CursorAttributeCommits bool `json:"cursorAttributeCommits,omitempty"`

	// CursorAttributePRs, when true, lets Cursor agents append attribution to
	// pull requests they open via gh. Default false.
	CursorAttributePRs bool `json:"cursorAttributePRs,omitempty"`

	// BundleValues stores [[setting]] values keyed by setting id (user pack
	// with the same id wins the schema; the stored value follows the id).
	// Injected into that bundle's shebang scripts as RECUR_SETTING_<name>.
	BundleValues map[string]string `json:"bundleValues,omitempty"`

	// LinearAPIKey / LinearTeamKey are deprecated aliases migrated into
	// BundleValues (SettingLinearAPIKey / SettingLinearTeamKey) on read/write.
	LinearAPIKey  string `json:"linearApiKey,omitempty"`
	LinearTeamKey string `json:"linearTeamKey,omitempty"`

	// PreventSleepWhileAgentsRun, when true, holds a macOS `caffeinate -i`
	// assertion whenever any agent subprocess slot is in use, so the machine
	// does not idle-sleep mid-turn. Default false. No-op on non-darwin.
	PreventSleepWhileAgentsRun bool `json:"preventSleepWhileAgentsRun,omitempty"`

	// ShowHiddenProjects reveals Project.Hidden projects in the sidebar tree.
	ShowHiddenProjects bool `json:"showHiddenProjects,omitempty"`

	// QueueFollowUps, when true, makes Enter queue a follow-up sent while an
	// agent is running until the turn ends; ⌘/Ctrl+Enter steers instead.
	// Default false: Enter steers into the live turn, ⌘/Ctrl+Enter queues.
	QueueFollowUps bool `json:"queueFollowUps,omitempty"`

	// AgentTurnTimeoutMinutes is the wall-clock ceiling for one interactive
	// agent chat turn (Ask/Plan/Agent/Review tabs, and ad-hoc dispatched
	// specialist turns). Zero/unset → DefaultAgentTurnTimeoutMinutes (4h).
	// Clamped to [MinAgentTurnTimeoutMinutes, MaxAgentTurnTimeoutMinutes] when
	// positive. Last-resort kill for a wedged CLI — not a "task should be done
	// by now" budget. Does not bound Loop pipeline specialists or Maestro chat
	// (Maestro keeps its own short budget). User Stop cancels immediately.
	AgentTurnTimeoutMinutes int `json:"agentTurnTimeoutMinutes,omitempty"`

	// ScheduledWorkspaceKeep is how many live workspaces to keep per schedule.
	// Older settled runs are removed automatically. Zero (the default) keeps all.
	ScheduledWorkspaceKeep int `json:"scheduledWorkspaceKeep,omitempty"`

	// --- Codex provider (direct app-server JSON-RPC) ---

	// CodexAPIKey authenticates Codex runs via CODEX_API_KEY. Together with
	// CodexChatGPTAuth and ProviderEnabled it offers Codex in pickers: with
	// neither credential, or with Codex turned off, Codex models do not appear.
	// Turning Codex off keeps this key. Stored in the local settings blob.
	CodexAPIKey string `json:"codexApiKey,omitempty"`

	// CodexChatGPTAuth, when true, lets Codex use a ChatGPT login (auth.json
	// under Recur's single CODEX_HOME, seeded from ~/.codex when present).
	// Either this or CodexAPIKey enables the provider.
	CodexChatGPTAuth bool `json:"codexChatGPTAuth,omitempty"`

	// CodexBinPath points at a `codex` CLI binary. Empty means Locate's default
	// resolution ($RECUR_CODEX, PATH, then common Homebrew paths).
	CodexBinPath string `json:"codexBinPath,omitempty"`

	// CodexNetworkAccess, when true, lets Codex Agent shell commands use the
	// network inside the workspace-write sandbox (local test servers, npm,
	// curl) without prompting. Default false. Does not grant writes outside
	// the worktree. Ignored when BypassPermissions is on (danger-full-access).
	// Ask/Plan/Review are read-only on disk and always get network so `gh`
	// can fetch PR diffs. When bypass is off, Codex can still ask for other
	// sandbox exceptions.
	CodexNetworkAccess bool `json:"codexNetworkAccess,omitempty"`

	// ChatGPTAccountID identifies a saved ChatGPT account. Tokens are kept
	// separately in chatgpt-auth.json and are never returned to the frontend.
	ChatGPTAccountID string            `json:"chatGPTAccountId,omitempty"`
	ChatGPTModels    []CompatibleModel `json:"chatGPTModels,omitempty"`

	// DisabledBundleCommands lists Recur Bundle command ids the user turned
	// off in Settings → Bundles. Disabled commands are omitted from the Run
	// Command palette, keybindings, and willSave. Empty/nil → all enabled.
	DisabledBundleCommands []string `json:"disabledBundleCommands,omitempty"`

	// DisabledBundlePrompts lists [[prompt]] action ids turned off in
	// Settings → Bundles. Disabled prompts leave the composer `/` menu and
	// first-party invoke paths. Empty/nil → all enabled.
	DisabledBundlePrompts []string `json:"disabledBundlePrompts,omitempty"`

	// DisabledBundleGrammars lists [[grammar]] ids turned off in Settings →
	// Bundles. Disabled grammars are not used for path highlighting. Empty/nil
	// → all enabled.
	DisabledBundleGrammars []string `json:"disabledBundleGrammars,omitempty"`

	// DisabledBundleThemes lists [[theme]] ids turned off in Settings →
	// Bundles. Disabled themes leave the Appearance picker and View menu.
	// Empty/nil → all enabled.
	DisabledBundleThemes []string `json:"disabledBundleThemes,omitempty"`

	// DisabledBundleSnippets lists [[snippet]] ids turned off in Settings →
	// Bundles. Disabled snippets do not expand on Tab or keyEquivalent
	// and are omitted from the editor autocomplete menu.
	// Empty/nil → all enabled.
	DisabledBundleSnippets []string `json:"disabledBundleSnippets,omitempty"`

	// DisabledLanguageServers lists language server ids (e.g. "gopls") the user
	// turned off in Settings → Bundles. Disabled servers soft-degrade in the
	// Observatory editor. Empty/nil → all enabled.
	DisabledLanguageServers []string `json:"disabledLanguageServers,omitempty"`
	// DefaultLoop is the dock / New Task loop when none is chosen.
	// Empty → recur.loops.pr.
	DefaultLoop string `json:"defaultLoop,omitempty"`
	// DefaultPolicy is the dock / New Task policy when none is chosen.
	// Empty → recur.policies.balanced.
	DefaultPolicy string `json:"defaultPolicy,omitempty"`
	// DisabledBundleLoops / Roles / Policies omit those ids from pickers
	// and refuse engage. Empty/nil → all enabled.
	DisabledBundleLoops    []string `json:"disabledBundleLoops,omitempty"`
	DisabledBundleRoles    []string `json:"disabledBundleRoles,omitempty"`
	DisabledBundlePolicies []string `json:"disabledBundlePolicies,omitempty"`

	// DisabledSkills lists Agent Skill names the user turned off in Settings →
	// Skills (Recur + user/home skills). Combined with Project.DisabledSkills
	// at runtime. Empty/nil → none globally disabled.
	DisabledSkills []string `json:"disabledSkills,omitempty"`

	// MCPServers is the machine catalog of extra MCP servers (Settings → MCP).
	// Recur MCP and Playwright are engine-owned and are not stored here.
	// Linear is a shipped preset (id=linear); it is off until Enabled is true.
	MCPServers []MCPServer `json:"mcpServers,omitempty"`

	// Simplify is Recur's first-class "write less" mode (YAGNI, stdlib first,
	// shortest working diff). nil/omitted → on. Settings → General is the
	// default for new chats; each session's composer chip can override.
	// Injected on Agent/Plan chats and loop Coder turns (a loop that's on
	// stays on for agents it starts).
	Simplify *bool `json:"simplify,omitempty"`

	// UseNativeTitleBar keeps the OS title bar and menu bar on Linux. Off (the
	// default) hides them and the app draws a themed header like macOS does.
	// Read at startup, so a change applies after a restart. Ignored elsewhere.
	UseNativeTitleBar bool `json:"useNativeTitleBar,omitempty"`

	// DisabledProviders lists provider slugs turned off in Settings → Providers
	// (claude-code, cursor, codex, and user-defined compatible slugs).
	// Credentials stay saved; the provider is omitted from model pickers and
	// the engine will not start it. Empty/nil → every configured provider is
	// on. Picking a model is still what selects the provider for a role — this
	// is only an on/off switch.
	DisabledProviders []string `json:"disabledProviders,omitempty"`

	// CompatibleProviders are user-defined OpenAI-compatible HTTP endpoints
	// Recur drives via POST {baseURL}/v1/responses. Each has its own slug
	// (the ModelRef provider prefix), models, and flag dimensions.
	CompatibleProviders []CompatibleProvider `json:"compatibleProviders,omitempty"`
}

// Shipped Linear bundle [[setting]] ids. Shebang /linear, [[search]], and
// [[expand]] read these via RECUR_SETTING_* (LINEAR_API_KEY process-env fallback).
const (
	SettingLinearAPIKey  = "recur.linear.api-key"
	SettingLinearTeamKey = "recur.linear.team-key"
)

// MigrateBundleValues copies deprecated LinearAPIKey / LinearTeamKey into
// BundleValues and clears the old fields.
func (s Settings) MigrateBundleValues() Settings {
	out := s
	n := 0
	if out.BundleValues != nil {
		n = len(out.BundleValues)
	}
	cp := make(map[string]string, n+2)
	for k, v := range out.BundleValues {
		cp[k] = v
	}
	out.BundleValues = cp
	if k := strings.TrimSpace(out.LinearAPIKey); k != "" {
		if strings.TrimSpace(out.BundleValues[SettingLinearAPIKey]) == "" {
			out.BundleValues[SettingLinearAPIKey] = k
		}
		out.LinearAPIKey = ""
	}
	if k := strings.TrimSpace(out.LinearTeamKey); k != "" {
		if strings.TrimSpace(out.BundleValues[SettingLinearTeamKey]) == "" {
			out.BundleValues[SettingLinearTeamKey] = k
		}
		out.LinearTeamKey = ""
	}
	if len(out.BundleValues) == 0 {
		out.BundleValues = nil
	}
	return out
}

// BundleValue returns the stored [[setting]] value for id after Linear migration.
func (s Settings) BundleValue(id string) string {
	s = s.MigrateBundleValues()
	if s.BundleValues == nil {
		return ""
	}
	return strings.TrimSpace(s.BundleValues[id])
}

// Interactive agent chat turn deadline (Settings → General). Shipped default is
// generous enough for long Claude Code turns; the max lets power users go much
// longer without us shipping a multi-day default.
const (
	DefaultAgentTurnTimeoutMinutes = 4 * 60      // 4 hours
	MinAgentTurnTimeoutMinutes     = 5           // avoid accidental 1-minute kills
	MaxAgentTurnTimeoutMinutes     = 7 * 24 * 60 // 7 days
)

// AgentTurnTimeout returns the wall-clock limit for one interactive agent chat
// turn. Zero/unset settings use the default; positive values are clamped to the
// min/max.
func (s Settings) AgentTurnTimeout() time.Duration {
	return time.Duration(EffectiveAgentTurnTimeoutMinutes(s.AgentTurnTimeoutMinutes)) * time.Minute
}

// EffectiveAgentTurnTimeoutMinutes normalizes a minutes value for turn deadlines
// and Settings UI. Non-positive → default. Otherwise clamped to [min, max].
func EffectiveAgentTurnTimeoutMinutes(minutes int) int {
	if minutes <= 0 {
		return DefaultAgentTurnTimeoutMinutes
	}
	if minutes < MinAgentTurnTimeoutMinutes {
		return MinAgentTurnTimeoutMinutes
	}
	if minutes > MaxAgentTurnTimeoutMinutes {
		return MaxAgentTurnTimeoutMinutes
	}
	return minutes
}

const (
	DefaultMaxLoops  = 4
	DefaultMaxAgents = 8
)

// EffectiveMaxLoops / EffectiveMaxAgents normalize concurrency caps for Settings.
// Negative → default. Zero is unlimited (matches NewGovernor). Positive values
// pass through with no upper bound.
func EffectiveMaxLoops(n int) int  { return normalizeGovernorCap(n, DefaultMaxLoops) }
func EffectiveMaxAgents(n int) int { return normalizeGovernorCap(n, DefaultMaxAgents) }

func normalizeGovernorCap(n, fallback int) int {
	if n < 0 {
		return fallback
	}
	return n
}

// SkillEnabled reports whether name is not in DisabledSkills.
func (s Settings) SkillEnabled(name string) bool {
	return skillNameEnabled(name, s.DisabledSkills)
}

// SimplifyEnabled reports whether Simplify mode is on. nil/omitted → on.
func (s Settings) SimplifyEnabled() bool {
	if s.Simplify == nil {
		return true
	}
	return *s.Simplify
}

// skillNameEnabled is false when name appears in disabled.
func skillNameEnabled(name string, disabled []string) bool {
	name = strings.TrimSpace(name)
	if name == "" {
		return true
	}
	return !skillNameListed(disabled, name)
}

func skillNameListed(list []string, name string) bool {
	if name == "" || len(list) == 0 {
		return false
	}
	for _, d := range list {
		if d == name {
			return true
		}
	}
	return false
}

// CursorConfigured reports whether Cursor credentials are present. Connect
// mints the API key before Cursor models can be offered; Settings.ProviderEnabled
// is the on/off switch that can hide Cursor without clearing the key.
func (s Settings) CursorConfigured() bool { return strings.TrimSpace(s.CursorAPIKey) != "" }

// CodexConfigured reports whether Codex credentials are present. An API key or
// an explicit ChatGPT-auth opt-in is required before Codex models can be
// offered — the binary is resolved separately.
func (s Settings) CodexConfigured() bool {
	return strings.TrimSpace(s.CodexAPIKey) != "" || s.CodexChatGPTAuth
}

// UsesProvider reports whether any mode (or workspace-title) model is pointed
// at the given provider — i.e. whether the engine may need that provider's
// runner this session.
func (s Settings) UsesProvider(provider string) bool {
	for _, m := range []ModelRef{
		s.AskModel(), s.PlanModel(), s.AgentModel(), s.ReviewModel(),
		s.LoopModel(), s.WorkspaceTitleModel(),
	} {
		if m.Provider == provider {
			return true
		}
	}
	return false
}

// PermissionMode is the Claude Code --permission-mode value for every agent
// turn. Off maps to "default" (not "dontAsk") because default is the mode that
// actually raises a permission request; the runner pairs it with
// --permission-prompt-tool so the request reaches the human.
func (s Settings) PermissionMode() string {
	if s.BypassPermissions {
		return PermissionModeBypass
	}
	return PermissionModeAsk
}

// Claude Code --permission-mode values Recur uses. Ask is paired with
// --permission-prompt-tool; Deny is the fallback for turns that have no MCP
// endpoint to route a prompt through, where asking is impossible.
const (
	PermissionModeBypass = "bypassPermissions"
	PermissionModeAsk    = "default"
	PermissionModeDeny   = "dontAsk"
)

// AskModel returns Ask mode's resolved model ref.
func (s Settings) AskModel() ModelRef { return ParseModelRef(s.ModelAsk) }

// PlanModel returns Plan mode's resolved model ref.
func (s Settings) PlanModel() ModelRef { return ParseModelRef(s.ModelPlan) }

// AgentModel returns Agent mode's resolved model ref.
func (s Settings) AgentModel() ModelRef { return ParseModelRef(s.ModelAgent) }

// ReviewModel returns Review mode's resolved model ref, falling back to Loop
// when unset so a half-configured Settings pane still runs.
func (s Settings) ReviewModel() ModelRef {
	if strings.TrimSpace(s.ModelReview) != "" {
		return ParseModelRef(s.ModelReview)
	}
	return s.LoopModel()
}

// LoopModel returns Loop mode's resolved model ref.
func (s Settings) LoopModel() ModelRef { return ParseModelRef(s.ModelLoop) }

// MaestroModel is the loop orchestrator's model — aliases Loop mode.
func (s Settings) MaestroModel() ModelRef { return s.LoopModel() }

// CoderModel is the loop coder's model — aliases Agent mode.
func (s Settings) CoderModel() ModelRef { return s.AgentModel() }

// ReviewerModel is the loop reviewer's model — aliases Review mode (with Loop fallback).
func (s Settings) ReviewerModel() ModelRef { return s.ReviewModel() }

// WorkspaceTitleModel returns the cheap completion model used for AI commit
// messages (and similar one-shot titles). Empty setting → Claude Haiku.
func (s Settings) WorkspaceTitleModel() ModelRef {
	if strings.TrimSpace(s.ModelWorkspaceTitle) != "" {
		return ParseModelRef(s.ModelWorkspaceTitle)
	}
	return ParseModelRef("claude-code/claude-haiku-4-5-20251001")
}

// cloneFlags returns a shallow copy of m (nil-safe). Callers may mutate the result.
func cloneFlags(m map[string]string) map[string]string {
	if len(m) == 0 {
		return nil
	}
	out := make(map[string]string, len(m))
	for k, v := range m {
		out[k] = v
	}
	return out
}

func (s Settings) AskFlags() map[string]string            { return cloneFlags(s.FlagsAsk) }
func (s Settings) PlanFlags() map[string]string           { return cloneFlags(s.FlagsPlan) }
func (s Settings) AgentFlags() map[string]string          { return cloneFlags(s.FlagsAgent) }
func (s Settings) ReviewFlags() map[string]string         { return cloneFlags(s.FlagsReview) }
func (s Settings) LoopFlags() map[string]string           { return cloneFlags(s.FlagsLoop) }
func (s Settings) WorkspaceTitleFlags() map[string]string { return cloneFlags(s.FlagsWorkspaceTitle) }

// MaestroFlags / CoderFlags / ReviewerFlags alias the loop specialists' mode slots.
func (s Settings) MaestroFlags() map[string]string  { return s.LoopFlags() }
func (s Settings) CoderFlags() map[string]string    { return s.AgentFlags() }
func (s Settings) ReviewerFlags() map[string]string { return s.ReviewFlags() }

// ModelForChatMode returns the Settings default model + flags for a chat mode
// when the dock has not overridden them — one Settings slot per mode.
func (s Settings) ModelForChatMode(mode string) (string, map[string]string) {
	switch NormalizeChatMode(mode) {
	case ChatModeAsk:
		return s.AskModel().String(), s.AskFlags()
	case ChatModePlan:
		return s.PlanModel().String(), s.PlanFlags()
	case ChatModeReview:
		return s.ReviewModel().String(), s.ReviewFlags()
	case ChatModeLoop:
		return s.LoopModel().String(), s.LoopFlags()
	default: // agent (and unknown)
		return s.AgentModel().String(), s.AgentFlags()
	}
}

// How create_workspace picks the spawned tab's model and flags.
const (
	ChildWorkspaceModelParent = "parent"
	ChildWorkspaceModelMode   = "mode"
)

// EffectiveChildWorkspaceModel reports how create_workspace picks the spawned
// tab's model and flags. Empty or unknown → parent (copy the calling chat).
func (s Settings) EffectiveChildWorkspaceModel() string {
	if strings.TrimSpace(s.ChildWorkspaceModel) == ChildWorkspaceModelMode {
		return ChildWorkspaceModelMode
	}
	return ChildWorkspaceModelParent
}

// Resource is the machine-global rollup (PRD §15.2 resource.updated, D6).
// ItersToday remains computed (D43) even though chrome no longer renders it (D-app-footer).
type Resource struct {
	LoopsRunning int `json:"loopsRunning"`
	NeedsYou     int `json:"needsYou"`
	ItersToday   int `json:"itersToday"`
	// GitWarning is set at boot when git is missing or older than 2.31. Not persisted.
	GitWarning string `json:"gitWarning,omitempty"`
}
