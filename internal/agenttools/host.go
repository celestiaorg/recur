package agenttools

import (
	"context"
	"encoding/json"

	"recur/internal/domain"
)

// Host is the typed callback seam (PRD §8.3): every Recur host tool call is turned
// into one of these method calls on the engine, attributed to a specific
// loop+agent. This is the SINGLE path structured signals re-enter the engine —
// the convergence test drives findings through the exact same interface the live
// MCP handler calls, so the test exercises the real convergence logic.
//
// Implementations MUST be safe for concurrent use: the MCP HTTP handler is hit by
// agent subprocesses concurrently with the loop goroutine.
type Host interface {
	// ReportFinding records one reviewer finding for the loop's current iteration
	// (PRD §8.3 report_finding → drives convergence). The MCP server assigns
	// nothing — the engine owns Finding identity/iteration. Returns the stored
	// finding (with id) so the tool result can expose it to the UI digest.
	ReportFinding(workspaceID, agentCode string, f Finding) domain.Finding

	// Done signals a Coder/Tester turn finished (PRD §8.3 done). summary is the
	// agent's own wrap-up; notes is the optional handoff (deviations from the
	// plan, concerns, work left undone) the engine threads to the Reviewer and
	// the conductor. Returns a non-nil error when the turn must continue
	// (e.g. open findings remain) — surfaced as a tool-level IsError so the
	// model self-corrects in the same turn. The ENGINE still decides convergence
	// after an independent re-review (PRD §7.3).
	Done(workspaceID, agentCode, summary, notes string) error

	// ListFindings returns persisted findings for the workspace, optionally
	// filtered by status (open|resolved|ignored|invalid|all; default open).
	ListFindings(workspaceID, agentCode, status string) ([]domain.Finding, error)

	// UpdateFinding sets one finding's status from the Coder (resolved|invalid).
	// invalid requires a non-empty reason. Returns the updated finding.
	UpdateFinding(workspaceID, agentCode, findingID, status, reason string) (domain.Finding, error)

	// ReportResult records a Tester's pass/fail result (PRD §8.3 report_result).
	ReportResult(workspaceID, agentCode string, passed bool, summary string)

	// Plan records a Maestro/agent plan summary (PRD §8.3 plan, minimal).
	Plan(workspaceID, agentCode, summary string)

	// Escalate raises an escalation: the loop pauses and asks the human a scoped
	// question with reasoned options + context (PRD §8.3 escalate, §13). The engine
	// persists the escalation, moves the loop to "needs you", emits escalation.raised,
	// and BLOCKS the loop at a safe checkpoint until the human answers — answering
	// resumes from the checkpoint. The MCP handler returns immediately (the block is
	// on the loop goroutine, not the HTTP request). Non-interactive loop agents only.
	Escalate(workspaceID, agentCode string, e EscalationRequest)

	// NoteToolArgs records the verbatim tools/call arguments for transcript
	// backfill. Providers like Codex often omit args on mcpToolCall item
	// frames (stored as {}); the MCP HTTP path still receives the real payload.
	// No-op when args are empty. Must not block.
	NoteToolArgs(workspaceID, agentCode, toolName string, args json.RawMessage)

	// AskUser parks a clarifying question for the human and returns immediately
	// (interactive agents only: Maestro chat, PA*). Does not escalate the
	// workspace. The calling turn is stopped so the chat goes idle; answering
	// starts a new resumed turn with the answer as user input. Always raises
	// an inbox row. Must not block the MCP HTTP handler — Cursor MCP clients
	// time out blocked tools/call POSTs in ~60s–5m.
	AskUser(workspaceID, agentCode string, req AskUserRequest) string

	// SwitchMode changes the current interactive chat's mode (privilege-safe).
	SwitchMode(workspaceID, agentCode, mode string) string

	// CreateArtifact writes an agent-created artifact under the workspace data dir.
	CreateArtifact(workspaceID, agentCode string, req CreateArtifactRequest) (domain.Artifact, error)

	// ListArtifacts returns artifacts for the workspace.
	ListArtifacts(workspaceID, agentCode string) ([]domain.Artifact, error)

	// ReadArtifact returns one workspace artifact's metadata and text body.
	// PNG/JPEG are flagged binary and are not inlined.
	ReadArtifact(workspaceID, agentCode, artifactID string) (string, error)

	// PresentArtifact opens/focuses one artifact in the UI.
	PresentArtifact(workspaceID, agentCode, artifactID string) string

	// RenameSession sets the current chat/session title. ownerChat is the
	// calling turn's conductor tab when known (loop ChatCode); empty means
	// infer from the running walk.
	RenameSession(workspaceID, agentCode, ownerChat, title string) string

	// RenameWorkspace sets the workspace sidebar title (Goal/Short).
	// Allowed during loops; refused on the main working copy.
	RenameWorkspace(workspaceID, agentCode, title string) string

	// RenameBranch renames the workspace's checked-out git branch.
	// Allowed during planning; refused while a loop is coding, reviewing, or verifying.
	RenameBranch(workspaceID, agentCode, name string) string

	// OpenPR opens a Local or GitHub PR from this workspace with the given
	// title and markdown body. draft is gh pr create --draft. Refused on
	// the main working copy, loop workspaces, and dirty worktrees.
	OpenPR(workspaceID, agentCode, title, body string, draft bool) string

	// CreateWorkspace starts a fresh workspace running prompt on its first chat
	// tab (or as a loop), on behalf of an Agent chat. Returns a human summary;
	// a same-titled live workspace in the project is reported, not duplicated.
	CreateWorkspace(workspaceID, agentCode string, req CreateWorkspaceRequest) string

	// MergeChild merges a settled child (create_workspace) into the calling
	// workspace's worktree now. ReadChild returns the child's report; ArchiveChild
	// closes a settled child without merging. MessageChild injects a mid-turn
	// instruction into a running child. ReadChildArtifact is read_artifact
	// scoped to one of the caller's children.
	MergeChild(workspaceID, agentCode, childID string) string
	ReadChild(workspaceID, agentCode, childID string) string
	ArchiveChild(workspaceID, agentCode, childID string) string
	MessageChild(workspaceID, agentCode, childID, message string) string
	ReadChildArtifact(workspaceID, agentCode, childID, artifactID string) (string, error)

	// AttachEvidence records one browser-verification evidence step from the QA
	// agent (PRD §8.3 attach_evidence, §10): a short step description, an optional
	// screenshot path on disk, and pass/fail. The engine collects these on the
	// loop's current verify run; rail #1 reads them to decide whether the loop may
	// open a PR. The MCP handler returns immediately; the loop goroutine reads the
	// accumulated verify result after the QA turn ends.
	AttachEvidence(workspaceID, agentCode string, e Evidence)

	// RequestPermission asks the human whether this agent may run one tool, and
	// BLOCKS until they answer. It backs --permission-prompt-tool, which the
	// runner wires whenever Settings.BypassPermissions is off.
	//
	// This is the one Reporter method that does not return promptly: the agent
	// subprocess is holding an open MCP request for the duration, which is what
	// makes the tool call actually wait on the human rather than proceed. The
	// implementation MUST therefore honor ctx (the HTTP request's context, so a
	// dead agent releases the waiter) and MUST impose its own timeout. Return an
	// error only when no verdict could be reached; a refusal is a verdict.
	RequestPermission(ctx context.Context, workspaceID, agentCode string, ask PermissionAsk) (PermissionVerdict, error)

	// SendBack re-enters an open PR's loop for rework (PRD §12.3), with the given
	// instruction as the send-back payload. Used by the Maestro chat when the loop
	// has already settled at pr_open and the human wants more work — plan-steering
	// alone cannot restart a finished loop. Returns a short status string for the
	// tool result (success or why it was refused).
	SendBack(workspaceID, agentCode, instruction string) string

	// ---- Maestro hands: the agent REQUESTS, the engine
	// validates and executes within the rails. Each returns a status string for
	// the tool result — success, or the NAMED reason the engine refused. ----

	// EditPlan rewrites the loop's persisted plan (DefinitionOfDone). Logged as a
	// plan-change deviation and steered into subsequent specialist turns.
	EditPlan(workspaceID, agentCode, plan string) string

	// RerunReview dispatches a fresh Reviewer pass over the loop's current
	// worktree; findings land in the loop's findings like any review.
	RerunReview(workspaceID, agentCode string) string

	// DispatchSpecialist runs one bounded specialist turn (reviewer|security) over
	// the loop's worktree with the given instruction, governed like any agent turn.
	DispatchSpecialist(workspaceID, agentCode, role, instruction string) string

	// ExtendIterations grants the running loop extra fix iterations, clamped by
	// the autonomy level's extension rules (rail #4 arithmetic stays the engine's).
	ExtendIterations(workspaceID, agentCode string, n int) string

	// ExtendRounds is the human “keep iterating” grant at the converge cap.
	ExtendRounds(workspaceID, agentCode string, n int) string

	// ReenterPhase starts a new graph walk from phaseID (idle only).
	ReenterPhase(workspaceID, agentCode, phaseID string) string

	// RunPhase runs an on-demand phase, then resumes the parked graph walk.
	RunPhase(workspaceID, agentCode, phaseID string) string

	// ResumeLoop RESTARTS a stopped/failed loop over its existing worktree.
	ResumeLoop(workspaceID, agentCode string) string

	// SubmitPlan files a STRUCTURED plan: a
	// headline summary, the full detailed body, ordered step titles, and
	// optional attachment ids. Plan-mode chats and the loop planner use it so
	// the plan is structured at the source, never parsed from prose. The engine
	// assigns step IDs and owns statuses. Returns a status string.
	SubmitPlan(workspaceID, agentCode string, p PlanSubmission) string

	// UpdatePlanStep requests a plan-step status change (pending|doing|done|
	// skipped) by step id. The engine validates the id and the transition, logs
	// the change, and recomputes run progress. Returns a status string.
	UpdatePlanStep(workspaceID, agentCode, stepID, status, note string) string

	// ListSkills returns Agent Skills available for this agent (name + description
	// catalog). Skills with disable-model-invocation are omitted.
	ListSkills(workspaceID, agentCode string) ([]domain.AgentSkill, error)

	// ActivateSkill loads one skill's SKILL.md body for model-driven activation.
	// Returns structured skill_content text, or an error if unknown / disabled.
	ActivateSkill(workspaceID, agentCode, name string) (string, error)

	// ListTerminals returns live Observatory shells for the workspace.
	ListTerminals(workspaceID, agentCode string) []TerminalInfo

	// ReadTerminal returns ANSI-stripped recent output. Empty id reads the
	// focused tab (or the only session). tailLines 0 means the default tail.
	ReadTerminal(workspaceID, agentCode, id string, tailLines int) (string, error)

	// ListWorkspaceBriefs returns the current Briefing overview. App coordinator only.
	ListWorkspaceBriefs() domain.BriefingSnapshot

	// WorkspaceContext returns bounded detail for one workspace. App coordinator only.
	WorkspaceContext(id string) (domain.WorkspaceContext, error)

	// PresentWorkspace validates a target and returns a JSON card payload for
	// the Ask Recur transcript. It does not switch the visible shell.
	PresentWorkspace(target string) string
}

// TerminalInfo is one Observatory session as advertised to the agent.
type TerminalInfo struct {
	ID     string `json:"id"`
	Title  string `json:"title"`
	Cwd    string `json:"cwd"`
	Shell  string `json:"shell,omitempty"`
	Active bool   `json:"active"`
	Exited bool   `json:"exited,omitempty"`
	// ExitCode is set when Exited is true, including 0 (clean exit).
	ExitCode *int `json:"exitCode,omitempty"`
}

// PlanSubmission is the payload of a submit_plan tool call: the plan's headline,
// full detailed body, ordered step titles, and optional attachment ids. MCP
// callers omit StepIDs (the engine assigns S1…Sn). The Plan panel may pass
// existing step ids aligned with Steps so a retitle keeps status and note.
type PlanSubmission struct {
	Summary       string
	Body          string
	Steps         []string
	StepIDs       []string
	AttachmentIDs []string
	// StepMeta is aligned with Steps (nil entries allowed): files touched and
	// dependency ids, when the planner declared them.
	StepMeta []PlanStepMeta
}

// PlanStepMeta is the optional per-step detail a planner may file.
type PlanStepMeta struct {
	Files     []string
	DependsOn []string
	// Explicit marks the object form ({title, files, depends_on}): its lists
	// are authoritative even when empty. A bare string title leaves them unset.
	Explicit bool
}

// CreateWorkspaceRequest is the create_workspace tool input. Empty ProjectID
// means the caller's project; empty BaseBranch means the project default;
// empty Mode means agent.
type CreateWorkspaceRequest struct {
	ProjectID, Title, Prompt, BaseBranch, Mode, LoopID, PolicyID string
	// Standalone requests an independent workspace. Nil means default: a child
	// of the caller (branched off its branch; call merge_child to land it)
	// unless the caller is a scheduled run, mode is loop, or BaseBranch is set.
	Standalone *bool
}

// EscalationRequest is the payload of an escalate tool call (PRD §8.3, §13): a
// scoped question, reasoned options (each with a short trade-off), and one-read
// context (what was built, what's ambiguous, what was considered). The engine
// stamps id/status/timing — the agent supplies only the human-facing content.
type EscalationRequest struct {
	Question string
	Options  []EscalationOption
	Context  string
}

// AskUserRequest is the payload of an ask_user tool call.
type AskUserRequest struct {
	Question string
	Options  []EscalationOption
	Context  string
}

// CreateArtifactRequest is the payload of create_artifact.
type CreateArtifactRequest struct {
	Title   string
	Kind    string
	Content string
}

// EscalationOption is one reasoned choice the agent offers (PRD §13). Tag is a
// short label (e.g. simplest / smoothest / recommended); Desc is the trade-off.
type EscalationOption struct {
	Text string
	Tag  string
	Desc string
}

// Finding is the payload of a report_finding tool call — a structured reviewer
// report (PRD §8.3). It carries no engine-assigned identity/iteration; the engine
// stamps those when it records the finding (so an agent can't forge them).
type Finding struct {
	File     string
	Line     int // 1-based start; 0 if not line-specific
	EndLine  int // inclusive end; 0 or == Line → single line
	Severity domain.Severity
	Message  string
	Summary  string // optional user-facing digest (markdown); empty → use Message
}

// Evidence is the payload of an attach_evidence tool call — one browser-
// verification step (PRD §8.3, §10). It carries no engine-assigned identity; the
// engine stamps timing/ordering when it records the step (so an agent can't forge
// them). ScreenshotPath is the on-disk path to a captured screenshot (may be empty).
type Evidence struct {
	Step           string
	ScreenshotPath string
	Passed         bool
}

// PermissionAsk is the payload of a permission_request tool call: the tool
// Claude Code wants to run, that tool's arguments as raw JSON (rendered for the
// human, echoed back untouched on approval), and the tool_use_id correlating it
// with the assistant turn.
type PermissionAsk struct {
	Tool      string
	Input     json.RawMessage
	ToolUseID string
}

// PermissionVerdict is the answer to a PermissionAsk. Reason is carried back to
// the model on a refusal so it can adapt instead of retrying blindly.
type PermissionVerdict struct {
	Allow  bool
	Reason string
}
