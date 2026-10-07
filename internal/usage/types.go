// Package usage fetches subscription-quota snapshots for Claude, Codex, and Cursor.
package usage

const (
	ProviderClaude = "claude-code"
	ProviderCursor = "cursor"
	ProviderCodex  = "codex"

	SourceClaudeLogin   = "Claude Code login"
	SourceCodexChatGPT  = "ChatGPT login"
	SourceCodexAPIKey   = "API key"
	SourceCursorConnect = "Recur Connect"

	SourceIDClaudeLogin   = "claude_login"
	SourceIDCodexChatGPT  = "chatgpt"
	SourceIDCodexAPIKey   = "api_key"
	SourceIDCursorConnect = "cursor_connect"

	WindowFiveHour = "five_hour"
	WindowSevenDay = "seven_day"
	WindowExtra    = "extra"
	WindowOpus     = "seven_day_opus"
	WindowSonnet   = "seven_day_sonnet"
	WindowFable    = "seven_day_fable"
	WindowBilling  = "billing"

	BucketCursor = "cursor"
	BucketOther  = "other"
)

// Snapshot is the UI payload for the sidebar footer. Tokens never appear here.
type Snapshot struct {
	FetchedAt int64           `json:"fetchedAt"`
	Stale     bool            `json:"stale"`
	Providers []ProviderQuota `json:"providers"`
}

// ProviderQuota is one provider's plan windows (and optional Cursor model rows).
type ProviderQuota struct {
	ID         string     `json:"id"`
	Name       string     `json:"name"`
	Email      string     `json:"email,omitempty"`
	Source     string     `json:"source,omitempty"`
	SourceID   string     `json:"sourceId,omitempty"`
	Plan       string     `json:"plan,omitempty"`
	Error      string     `json:"error,omitempty"`
	Hint       string     `json:"hint,omitempty"`
	Connecting bool       `json:"connecting,omitempty"`
	Windows    []Window   `json:"windows,omitempty"`
	Models     []ModelRow `json:"models,omitempty"`
}

// Window is one quota bar (5-hour, 7-day, billing cycle, …).
type Window struct {
	ID       string  `json:"id"`
	Label    string  `json:"label"`
	UsedPct  float64 `json:"usedPct"`
	ResetsAt int64   `json:"resetsAt,omitempty"`
	Detail   string  `json:"detail,omitempty"`
}

// ModelRow is one Cursor included-usage model line.
type ModelRow struct {
	Name    string  `json:"name"`
	Tokens  int64   `json:"tokens"`
	UsedPct float64 `json:"usedPct,omitempty"`
	// Bucket is "cursor" (Composer / Cursor-native) or "other" (API models).
	Bucket string `json:"bucket,omitempty"`
}

// CursorUsageConnect is the return of ConnectCursorUsage.
type CursorUsageConnect struct {
	LoginURL string `json:"loginUrl"`
	Pending  bool   `json:"pending"`
	Error    string `json:"error,omitempty"`
}

// EmptySnapshot is a zero-value snapshot for first paint.
func EmptySnapshot() Snapshot {
	return Snapshot{Providers: []ProviderQuota{}}
}

func setSource(q *ProviderQuota, display string) {
	q.Source = display
	switch display {
	case SourceClaudeLogin:
		q.SourceID = SourceIDClaudeLogin
	case SourceCodexChatGPT:
		q.SourceID = SourceIDCodexChatGPT
	case SourceCodexAPIKey:
		q.SourceID = SourceIDCodexAPIKey
	case SourceCursorConnect:
		q.SourceID = SourceIDCursorConnect
	default:
		q.SourceID = ""
	}
}

func clampPct(n float64) float64 {
	if n < 0 {
		return 0
	}
	if n > 100 {
		return 100
	}
	return n
}
