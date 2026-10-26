package codex

import (
	"encoding/json"
	"strings"
)

// Thread is a durable Codex conversation.
type Thread struct {
	ID  string `json:"id"`
	CWD string `json:"cwd,omitempty"`
}

// Turn is one unit of agent work.
type Turn struct {
	ID     string `json:"id"`
	Status string `json:"status,omitempty"`
}

// TokenUsage is the cumulative / last footprint from thread/tokenUsage/updated.
type TokenUsage struct {
	TotalTokens           int64 `json:"totalTokens"`
	InputTokens           int64 `json:"inputTokens"`
	CachedInputTokens     int64 `json:"cachedInputTokens"`
	CacheWriteInputTokens int64 `json:"cacheWriteInputTokens"`
	OutputTokens          int64 `json:"outputTokens"`
	ReasoningOutputTokens int64 `json:"reasoningOutputTokens"`
}

// Model is one entry from model/list.
type Model struct {
	ID                       string            `json:"id"`
	Model                    string            `json:"model"`
	DisplayName              string            `json:"displayName"`
	Description              string            `json:"description"`
	Hidden                   bool              `json:"hidden"`
	IsDefault                bool              `json:"isDefault"`
	DefaultReasoningEffort   string            `json:"defaultReasoningEffort"`
	SupportedReasoningEffort []ReasoningEffort `json:"supportedReasoningEfforts"`
	InputModalities          []string          `json:"inputModalities"`
	AdditionalSpeedTiers     []string          `json:"additionalSpeedTiers"`
	ServiceTiers             []ServiceTier     `json:"serviceTiers"`
	// DefaultServiceTier is the API field name (plural) for the default tier id.
	DefaultServiceTier string `json:"defaultServiceTier"`
}

// ReasoningEffort is one selectable effort level on a model.
type ReasoningEffort struct {
	ReasoningEffort string `json:"reasoningEffort"`
	Description     string `json:"description"`
}

// ServiceTier is one speed / priority tier from model/list.
type ServiceTier struct {
	ID          string `json:"id"`
	Name        string `json:"name"`
	Description string `json:"description"`
}

// Account is the result of account/read.
type Account struct {
	Type     string `json:"type"`
	Email    string `json:"email,omitempty"`
	PlanType string `json:"planType,omitempty"`
}

// AccountReadResult wraps account/read.
type AccountReadResult struct {
	Account            *Account `json:"account"`
	RequiresOpenaiAuth bool     `json:"requiresOpenaiAuth"`
}

// LoginStartResult is the managed ChatGPT browser login returned by Codex.
type LoginStartResult struct {
	LoginID string `json:"loginId"`
	AuthURL string `json:"authUrl"`
}

// LoginCompleted is the account/login/completed notification.
type LoginCompleted struct {
	LoginID string `json:"loginId"`
	Success bool   `json:"success"`
	Error   string `json:"error"`
}

// ThreadStartParams for thread/start.
type ThreadStartParams struct {
	Model                 string `json:"model,omitempty"`
	CWD                   string `json:"cwd,omitempty"`
	ApprovalPolicy        string `json:"approvalPolicy,omitempty"`
	Sandbox               string `json:"sandbox,omitempty"`
	DeveloperInstructions string `json:"developerInstructions,omitempty"`
	BaseInstructions      string `json:"baseInstructions,omitempty"`
	ServiceName           string `json:"serviceName,omitempty"`
	Ephemeral             *bool  `json:"ephemeral,omitempty"`
}

// UserInput is one turn/start input item.
type UserInput struct {
	Type string `json:"type"`
	Text string `json:"text,omitempty"`
	URL  string `json:"url,omitempty"`
	Path string `json:"path,omitempty"`
}

// SandboxPolicy for turn/start overrides.
type SandboxPolicy struct {
	Type string `json:"type"`
	// NetworkAccess is Codex's sandbox network flag (read-only and
	// workspace-write). omitempty keeps the default (blocked) off the wire
	// unless Recur opts in.
	NetworkAccess bool `json:"networkAccess,omitempty"`
}

// TurnStartParams for turn/start.
type TurnStartParams struct {
	ThreadID       string          `json:"threadId"`
	Input          []UserInput     `json:"input"`
	Model          string          `json:"model,omitempty"`
	CWD            string          `json:"cwd,omitempty"`
	ApprovalPolicy string          `json:"approvalPolicy,omitempty"`
	SandboxPolicy  *SandboxPolicy  `json:"sandboxPolicy,omitempty"`
	Effort         string          `json:"effort,omitempty"`
	ServiceTier    string          `json:"serviceTier,omitempty"`
	Collaboration  json.RawMessage `json:"collaborationMode,omitempty"`
}

// TurnSteerParams for turn/steer.
type TurnSteerParams struct {
	ThreadID string      `json:"threadId"`
	Input    []UserInput `json:"input"`
}

// TurnInterruptParams for turn/interrupt.
type TurnInterruptParams struct {
	ThreadID string `json:"threadId"`
	TurnID   string `json:"turnId"`
}

// Notification is a decoded server notification of interest to the runner.
type Notification struct {
	Method string
	Params json.RawMessage
}

// FormatPlan maps Codex planType / ChatGPT plan_type keys to CLI display names.
func FormatPlan(raw string) string {
	switch strings.ToLower(strings.TrimSpace(raw)) {
	case "":
		return ""
	case "free":
		return "Free"
	case "go":
		return "Go"
	case "plus":
		return "Plus"
	case "pro":
		return "Pro"
	case "prolite":
		return "Pro Lite"
	case "team":
		return "Team"
	case "self_serve_business_prolite":
		return "Self Serve Business ProLite"
	case "self_serve_business_usage_based":
		return "Self Serve Business Usage Based"
	case "business":
		return "Business"
	case "ent26", "enterprise", "hc":
		return "Enterprise"
	case "enterprise_cbp_automation":
		return "Enterprise (Automation)"
	case "enterprise_cbp_usage_based":
		return "Enterprise CBP Usage Based"
	case "education", "edu":
		return "Edu"
	case "edu_plus":
		return "Edu Plus"
	case "edu_pro":
		return "Edu Pro"
	default:
		return strings.TrimSpace(raw)
	}
}
