package domain

// Briefing is the deterministic cross-workspace Now/Next projection.
// Copy, reason codes, and routes are derived in Go so the Overview UI and
// Ask Recur cannot disagree.

// Overview buckets. A workspace appears in at most one.
const (
	BriefBucketNeeds   = "needs"
	BriefBucketWorking = "working"
	BriefBucketPickup  = "pickup"
)

// Reason codes, highest priority first. The primary Now/Next pair uses the
// first matching code; the rest are secondary.
const (
	BriefReasonPermission       = "permission"
	BriefReasonQuestion         = "question"
	BriefReasonEscalation       = "escalation"
	BriefReasonRetryableFailure = "retryable_failure"
	BriefReasonFailure          = "failure"
	BriefReasonPlanApproval     = "plan_approval"
	BriefReasonFindings         = "findings"
	BriefReasonPRAttention      = "pr_attention"
	BriefReasonUnseenChat       = "unseen_chat"
	BriefReasonActiveAgent      = "active_agent"
	BriefReasonUnfinishedPlan   = "unfinished_plan"
	BriefReasonDirtyIdle        = "dirty_idle"
	BriefReasonRecentChat       = "recent_chat"
)

// BriefActionKind is the semantic kind of a BriefAction (navigation, not a
// duplicated mutation). The first release routes to existing surfaces.
const (
	BriefActionOpen     = "open"
	BriefActionAnswer   = "answer"
	BriefActionReview   = "review"
	BriefActionContinue = "continue"
	BriefActionApprove  = "approve"
)

// SessionKindApp is the durable kind of the app-scoped Ask Recur session
// (workspace_id NULL).
const SessionKindApp = "app"

// AppAgentCode is the session config_json.code / bus agent code for Ask Recur.
const AppAgentCode = "APP"

// BriefingSnapshot is one projection of current work across workspaces.
type BriefingSnapshot struct {
	GeneratedAt int64            `json:"generatedAt"`
	Counts      BriefingCounts   `json:"counts"`
	Workspaces  []WorkspaceBrief `json:"workspaces"`
}

// BriefingCounts is the Overview section tally. Independent of inbox unread.
type BriefingCounts struct {
	Needs   int `json:"needs"`
	Working int `json:"working"`
	Pickup  int `json:"pickup"`
}

// WorkspaceBrief is one Overview card: identity, Now/Next, and routes.
type WorkspaceBrief struct {
	WorkspaceID        string        `json:"workspaceId"`
	ProjectID          string        `json:"projectId,omitempty"`
	ProjectName        string        `json:"projectName,omitempty"`
	Title              string        `json:"title"`
	Branch             string        `json:"branch,omitempty"`
	Bucket             string        `json:"bucket"`
	Reasons            []string      `json:"reasons"`
	Now                string        `json:"now"`
	Next               string        `json:"next"`
	LastActivityAt     int64         `json:"lastActivityAt,omitempty"`
	LastViewedAt       int64         `json:"lastViewedAt,omitempty"`
	SeenActivityAt     int64         `json:"seenActivityAt,omitempty"`
	ChangedSinceViewed bool          `json:"changedSinceViewed,omitempty"`
	QuietAge           string        `json:"quietAge,omitempty"`
	LastSummary        string        `json:"lastSummary,omitempty"`
	State              string        `json:"state,omitempty"`
	Phase              string        `json:"phase,omitempty"`
	PlanSummary        string        `json:"planSummary,omitempty"`
	PlanDone           int           `json:"planDone,omitempty"`
	PlanTotal          int           `json:"planTotal,omitempty"`
	ActiveAgents       int           `json:"activeAgents,omitempty"`
	ActiveAgentTask    string        `json:"activeAgentTask,omitempty"`
	LatestChatCode     string        `json:"latestChatCode,omitempty"`
	LatestChatTitle    string        `json:"latestChatTitle,omitempty"`
	DiffAdd            int           `json:"diffAdd,omitempty"`
	DiffDel            int           `json:"diffDel,omitempty"`
	PRID               string        `json:"prId,omitempty"`
	PRNumber           int           `json:"prNumber,omitempty"`
	IsPRHost           bool          `json:"isPrHost,omitempty"`
	Primary            BriefAction   `json:"primary"`
	Secondary          []BriefAction `json:"secondary,omitempty"`
}

// BriefAction is a navigation target on an existing surface.
type BriefAction struct {
	Label  string `json:"label"`
	Target string `json:"target"`
	Kind   string `json:"kind,omitempty"`
}

// PresentWorkspaceResult is the coordinator's show-in-chat payload for
// present_workspace. It does not switch the visible shell — the transcript
// renders a card the human opens.
type PresentWorkspaceResult struct {
	OK     bool            `json:"ok"`
	Target string          `json:"target"`
	Kind   string          `json:"kind,omitempty"`
	Brief  *WorkspaceBrief `json:"brief,omitempty"`
	Error  string          `json:"error,omitempty"`
}

// WorkspaceContext is the bounded detail shape returned only to the
// coordinator (`get_workspace_context`). Not a full transcript dump.
type WorkspaceContext struct {
	Brief        WorkspaceBrief     `json:"brief"`
	PlanSteps    []PlanStep         `json:"planSteps,omitempty"`
	Agents       []AgentSummary     `json:"agents,omitempty"`
	OpenAsks     []OpenAsk          `json:"openAsks,omitempty"`
	RecentChats  []ChatHeader       `json:"recentChats,omitempty"`
	PR           *PRSummary         `json:"pr,omitempty"`
	Conversation []ConversationLine `json:"conversation,omitempty"`
}

// AgentSummary is one live or recent agent on a workspace, for context.
type AgentSummary struct {
	Code   string `json:"code"`
	Name   string `json:"name"`
	Status string `json:"status"`
	Task   string `json:"task,omitempty"`
	Unseen bool   `json:"unseen,omitempty"`
}

// OpenAsk is a parked question, permission, or escalation the human still owes.
type OpenAsk struct {
	Kind      string `json:"kind"` // permission | question | escalation
	ID        string `json:"id"`
	AgentCode string `json:"agentCode,omitempty"`
	Summary   string `json:"summary"`
}

// ChatHeader is a session header without transcript (title, mode, recency).
type ChatHeader struct {
	Code      string `json:"code"`
	Title     string `json:"title"`
	Mode      string `json:"mode,omitempty"`
	UpdatedAt int64  `json:"updatedAt"`
	Unseen    bool   `json:"unseen,omitempty"`
}

// PRSummary is the PR state the coordinator may cite.
type PRSummary struct {
	ID             string `json:"id"`
	Number         int    `json:"number"`
	Title          string `json:"title"`
	State          string `json:"state,omitempty"`
	ReviewDecision string `json:"reviewDecision,omitempty"`
}

// ConversationLine is one human-visible excerpt line (user/assistant text).
type ConversationLine struct {
	Role string `json:"role"`
	Text string `json:"text"`
	At   int64  `json:"at,omitempty"`
}
