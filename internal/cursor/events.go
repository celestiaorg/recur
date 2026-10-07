package cursor

import (
	"encoding/json"
	"strconv"
)

// RunStreamEvent is one frame from SdkAgentService/Send (or ObserveRun). The
// bridge encodes a oneof, so exactly one field is set — the event kind is
// "whichever field is non-nil", which is why these are pointers rather than a
// discriminated `kind` string.
type RunStreamEvent struct {
	Offset string `json:"offset,omitempty"`

	// SDKMessage is the normalized conversation event (status/thinking/
	// assistant/tool_call/usage). This is what Recur drives its activity feed
	// from.
	SDKMessage *SDKMessage `json:"sdkMessage,omitempty"`

	// Result is the terminal envelope; Result.Result holds the RunResult.
	Result *RunStreamResult `json:"result,omitempty"`

	// InteractionUpdate carries raw onDelta frames when Send is called with
	// enableDeltas. Recur ignores token/text spam and only reads summary and
	// turn-ended usage from it.
	InteractionUpdate json.RawMessage `json:"interactionUpdate,omitempty"`

	// Step is a conversation-step view of the same information as SDKMessage.
	// Decoded but unused, so a step frame is not mistaken for an unknown event.
	Step json.RawMessage `json:"step,omitempty"`

	// Done marks graceful stream completion. It arrives AFTER the result frame.
	Done json.RawMessage `json:"done,omitempty"`
}

// RunStreamResult is the `result` envelope wrapping the terminal RunResult.
type RunStreamResult struct {
	AgentID string     `json:"agentId,omitempty"`
	RunID   string     `json:"runId,omitempty"`
	Status  string     `json:"status,omitempty"` // RUN_LIFECYCLE_STATUS_*
	Result  *RunResult `json:"result,omitempty"`
}

// Message types seen on the wire.
const (
	MsgStatus    = "status"
	MsgThinking  = "thinking"
	MsgAssistant = "assistant"
	MsgUser      = "user"
	MsgToolCall  = "tool_call"
	MsgUsage     = "usage"
	MsgSystem    = "system"
	// MsgTask is SDKTaskMessage: task/subagent status on the SDKMessage stream
	// ({type:"task", status?, text?}). It is not compaction — summary events
	// arrive as InteractionUpdate summary / summary-started / summary-completed.
	MsgTask    = "task"
	MsgRequest = "request"
)

// tool_call statuses, per the SDK's documented SDKToolUseMessage:
// status is "running" | "completed" | "error".
//
// Two caveats, both observed on live traffic:
//
//   - The in-flight value is "running", NOT "started" as the cursor-agent CLI's
//     stream-json uses — the bridge and the CLI disagree here.
//   - "error" is documented but not reliably sent. A read of a missing file
//     emits "running" and then NOTHING: no terminal frame at all, and the run
//     still finishes successfully. So a call cannot be assumed to terminate,
//     and the runner closes any that do not (see cursorTurn.closeOpenCalls).
//
// The runner treats ANY non-running status as terminal rather than matching
// this list, so a status added later still closes its call.
const (
	ToolRunning   = "running"
	ToolCompleted = "completed"
	ToolError     = "error"
)

// status-message values (the run's own lifecycle).
const (
	StatusRunning  = "RUNNING"
	StatusFinished = "FINISHED"
	StatusError    = "ERROR"
)

// ToolNameMCP is the `name` on a tool_call frame for ANY MCP tool. The server
// and tool it actually targets live in Args (providerIdentifier / toolName), so
// callers wanting a specific name must use SDKMessage.ToolLabel.
const ToolNameMCP = "mcp"

// SDKMessage is one normalized conversation event.
//
// The wire shape nests the real payload one level down — the frame is
// `{"type":"assistant","message":{"type":"assistant","agent_id":…,"message":{…}}}` —
// and the payload uses snake_case keys (`agent_id`, `call_id`) while nested
// structures like `usage` use camelCase. UnmarshalJSON flattens both levels and
// accepts either spelling, so callers see one clean struct.
type SDKMessage struct {
	Type    string
	AgentID string
	RunID   string

	// status: RUNNING | FINISHED | ERROR. tool_call: started | completed.
	Status string
	// status: SDKStatusMessage.message — on ERROR this is the provider's own
	// reason ("Unable to reach the model provider…"). The bridge's proto
	// RunResult has no error field, so this frame is the ONLY place the
	// reason crosses the wire.
	Message string

	// system
	Subtype string

	// thinking. ThinkingDurationMS is set only on the FINAL thinking frame, whose
	// Text is empty — that is how a completed reasoning block is signalled.
	Text               string
	ThinkingDurationMS int

	// assistant / user body
	Content *MessageContent

	// tool_call
	CallID string
	Name   string
	Args   json.RawMessage
	Result json.RawMessage
	// Truncated reports whether the SDK cut `args` or `result` because the
	// payload was too large. Without surfacing it a clipped result reads as the
	// whole thing — a half-shown file or command output with no indication.
	Truncated Truncation

	// usage
	Usage *TokenUsage
}

// sdkMessagePayload mirrors the inner payload, accepting both key spellings.
type sdkMessagePayload struct {
	Type     string `json:"type"`
	AgentID  string `json:"agent_id"`
	AgentID2 string `json:"agentId"`
	RunID    string `json:"run_id"`
	RunID2   string `json:"runId"`

	Subtype string `json:"subtype"`
	Status  string `json:"status"`
	Text    string `json:"text"`

	ThinkingDurationMS  int `json:"thinking_duration_ms"`
	ThinkingDurationMS2 int `json:"thinkingDurationMs"`

	CallID  string `json:"call_id"`
	CallID2 string `json:"callId"`
	// `id` is raw so a numeric JSON-RPC id cannot fail the whole frame.
	// tool_call_id / toolCallId are accepted when call_id is absent.
	ID          json.RawMessage `json:"id"`
	ToolCallID  string          `json:"tool_call_id"`
	ToolCallID2 string          `json:"toolCallId"`
	Name        string          `json:"name"`

	Args      json.RawMessage `json:"args"`
	Result    json.RawMessage `json:"result"`
	Truncated Truncation      `json:"truncated"`

	// For assistant/user this is the {role, content[]} body; for the OUTER
	// envelope it is the nested payload. Which one it is depends on nesting
	// depth, resolved in UnmarshalJSON.
	Message json.RawMessage `json:"message"`

	Usage *TokenUsage `json:"usage"`
}

func (m *SDKMessage) UnmarshalJSON(b []byte) error {
	var outer sdkMessagePayload
	if err := json.Unmarshal(b, &outer); err != nil {
		return err
	}

	// If `message` is itself a typed payload, that is the real event body and
	// the outer object is just an envelope. Otherwise the outer object already
	// IS the payload (older/flatter frames).
	payload := outer
	if len(outer.Message) > 0 {
		var inner sdkMessagePayload
		if err := json.Unmarshal(outer.Message, &inner); err == nil && inner.Type != "" {
			payload = inner
		}
	}

	m.Type = firstNonEmpty(payload.Type, outer.Type)
	m.AgentID = firstNonEmpty(payload.AgentID, payload.AgentID2)
	m.RunID = firstNonEmpty(payload.RunID, payload.RunID2)
	m.Subtype = payload.Subtype
	m.Status = payload.Status
	m.Text = payload.Text
	m.ThinkingDurationMS = payload.ThinkingDurationMS
	if m.ThinkingDurationMS == 0 {
		m.ThinkingDurationMS = payload.ThinkingDurationMS2
	}
	m.CallID = firstNonEmpty(payload.CallID, payload.CallID2, jsonString(payload.ID), payload.ToolCallID, payload.ToolCallID2)
	m.Name = payload.Name
	m.Args = payload.Args
	m.Result = payload.Result
	m.Truncated = payload.Truncated
	m.Usage = payload.Usage

	// assistant/user carry their body one level deeper still; status carries a
	// plain string.
	if len(payload.Message) > 0 {
		var content MessageContent
		if err := json.Unmarshal(payload.Message, &content); err == nil && content.Role != "" {
			m.Content = &content
		}
		_ = json.Unmarshal(payload.Message, &m.Message)
	}
	return nil
}

// Truncation flags which parts of a tool_call frame the SDK clipped for size.
type Truncation struct {
	Args   bool `json:"args"`
	Result bool `json:"result"`
}

// Any reports whether anything was clipped.
func (t Truncation) Any() bool { return t.Args || t.Result }

// ToolCallArgs is the decoded `args` of a tool_call frame. Only the MCP routing
// fields are modelled; the tool's own parameters stay in Params.
type ToolCallArgs struct {
	ProviderIdentifier string          `json:"providerIdentifier"`
	ToolName           string          `json:"toolName"`
	Params             json.RawMessage `json:"args"`
}

// ToolLabel returns a stable, human- and allowlist-friendly tool name.
//
// Built-in tools report their own name ("read", "shell", …). MCP tools all
// report "mcp", so this rewrites them to Claude Code's `mcp__<server>__<tool>`
// spelling — which is what Recur's role tables and activity feed already speak,
// so the rest of the engine does not need to care which provider ran the turn.
func (m *SDKMessage) ToolLabel() string {
	if m == nil {
		return ""
	}
	if m.Name != ToolNameMCP {
		return m.Name
	}
	a, err := m.ToolArgs()
	if err != nil || a.ToolName == "" {
		return m.Name
	}
	if a.ProviderIdentifier == "" {
		return "mcp__" + a.ToolName
	}
	return "mcp__" + a.ProviderIdentifier + "__" + a.ToolName
}

// ToolArgs decodes the tool_call args envelope.
func (m *SDKMessage) ToolArgs() (ToolCallArgs, error) {
	var a ToolCallArgs
	if m == nil || len(m.Args) == 0 {
		return a, nil
	}
	err := json.Unmarshal(m.Args, &a)
	return a, err
}

// ToolResultIsError reports whether a completed tool_call failed. The result
// envelope is `{"status":"success"|"error","value":{…,"isError":bool}}`, and MCP
// tools can report a protocol-level success carrying an application-level
// isError, so both are checked.
func (m *SDKMessage) ToolResultIsError() bool {
	if m == nil || len(m.Result) == 0 {
		return false
	}
	var r struct {
		Status string `json:"status"`
		Value  struct {
			IsError bool `json:"isError"`
		} `json:"value"`
	}
	if err := json.Unmarshal(m.Result, &r); err != nil {
		return false
	}
	return r.Value.IsError || (r.Status != "" && r.Status != "success")
}

// MessageContent is the `{role, content[]}` body on assistant/user messages.
type MessageContent struct {
	Role    string         `json:"role"`
	Content []ContentBlock `json:"content"`
}

// ContentBlock is one block of message content. Only text blocks matter to
// Recur; other block types decode with an empty Text.
type ContentBlock struct {
	Type string `json:"type"`
	Text string `json:"text,omitempty"`
}

// TextContent concatenates the text blocks of a message.
func (m *MessageContent) TextContent() string {
	if m == nil {
		return ""
	}
	var out string
	for _, b := range m.Content {
		if b.Type == "text" {
			out += b.Text
		}
	}
	return out
}

// TokenUsage is per-run token accounting.
//
// There is deliberately NO dollar cost here: the bridge's GetUsage is cloud-only
// and errors for local agents. Recur keeps token counts for the context meter
// and does not convert them to USD.
//
// Counts arrive as JSON numbers on streaming `usage` messages but as JSON
// strings inside the terminal result (proto int64 → string in proto3 JSON),
// hence flexInt.
type TokenUsage struct {
	InputTokens      flexInt `json:"inputTokens"`
	OutputTokens     flexInt `json:"outputTokens"`
	CacheReadTokens  flexInt `json:"cacheReadTokens"`
	CacheWriteTokens flexInt `json:"cacheWriteTokens"`
	TotalTokens      flexInt `json:"totalTokens"`
	ReasoningTokens  flexInt `json:"reasoningTokens,omitempty"`
}

// SubUsage returns total minus sub, clamped at zero per field. Used to strip
// pre-compaction cumulative billing from a later usage snapshot.
func SubUsage(total, sub TokenUsage) TokenUsage {
	clamp := func(a, b flexInt) flexInt {
		if a < b {
			return 0
		}
		return a - b
	}
	return TokenUsage{
		InputTokens:      clamp(total.InputTokens, sub.InputTokens),
		OutputTokens:     clamp(total.OutputTokens, sub.OutputTokens),
		CacheReadTokens:  clamp(total.CacheReadTokens, sub.CacheReadTokens),
		CacheWriteTokens: clamp(total.CacheWriteTokens, sub.CacheWriteTokens),
		TotalTokens:      clamp(total.TotalTokens, sub.TotalTokens),
		ReasoningTokens:  clamp(total.ReasoningTokens, sub.ReasoningTokens),
	}
}

// PromptTokens is the prompt-side fill (input + cache read + cache write).
func (u TokenUsage) PromptTokens() int {
	return u.InputTokens.Int() + u.CacheReadTokens.Int() + u.CacheWriteTokens.Int()
}

// Interaction kinds Recur reads from enableDeltas frames. Other types
// (token-delta, text-delta, …) are ignored.
const (
	DeltaSummaryStarted   = "summary-started"
	DeltaSummaryCompleted = "summary-completed"
	DeltaSummary          = "summary"
	DeltaTurnEnded        = "turn-ended"
)

// ParseInteractionType returns the InteractionUpdate type string, accepting both
// the SDK's {type:"summary-started"} shape and protobuf-JSON oneof wrappers.
func ParseInteractionType(raw json.RawMessage) (typ string, body json.RawMessage) {
	if len(raw) == 0 {
		return "", nil
	}
	var tagged struct {
		Type   string          `json:"type"`
		Update json.RawMessage `json:"update"`
	}
	if json.Unmarshal(raw, &tagged) == nil {
		if tagged.Type != "" {
			// Bridge wraps SDK InteractionUpdate as {type, update: Struct}.
			// Summary text lives on the inner object, not the wrapper.
			body := tagged.Update
			if len(body) == 0 {
				body = raw
			}
			return tagged.Type, body
		}
		if len(tagged.Update) > 0 {
			return ParseInteractionType(tagged.Update)
		}
	}
	var oneof struct {
		SummaryStarted   json.RawMessage `json:"summaryStarted"`
		SummaryCompleted json.RawMessage `json:"summaryCompleted"`
		Summary          json.RawMessage `json:"summary"`
		TurnEnded        json.RawMessage `json:"turnEnded"`
	}
	if json.Unmarshal(raw, &oneof) != nil {
		return "", nil
	}
	switch {
	case len(oneof.SummaryStarted) > 0:
		return DeltaSummaryStarted, oneof.SummaryStarted
	case len(oneof.SummaryCompleted) > 0:
		return DeltaSummaryCompleted, oneof.SummaryCompleted
	case len(oneof.Summary) > 0:
		return DeltaSummary, oneof.Summary
	case len(oneof.TurnEnded) > 0:
		return DeltaTurnEnded, oneof.TurnEnded
	}
	return "", nil
}

// RunResult is the terminal outcome of a run.
type RunResult struct {
	RunID      string          `json:"runId"`
	AgentID    string          `json:"agentId"`
	RequestID  string          `json:"requestId,omitempty"`
	Status     string          `json:"status"` // RUN_LIFECYCLE_STATUS_*
	Result     string          `json:"result,omitempty"`
	Error      *RunError       `json:"error,omitempty"`
	Model      *ModelSelection `json:"model,omitempty"`
	DurationMS flexInt         `json:"durationMs,omitempty"`
	Usage      *TokenUsage     `json:"usage,omitempty"`
}

// RunError is the terminal failure detail.
type RunError struct {
	Message string `json:"message"`
	Code    string `json:"code,omitempty"`
}

// Run lifecycle statuses on the terminal result / ListRuns snapshots.
const (
	RunCreating  = "RUN_LIFECYCLE_STATUS_CREATING"
	RunRunning   = "RUN_LIFECYCLE_STATUS_RUNNING"
	RunFinished  = "RUN_LIFECYCLE_STATUS_FINISHED"
	RunError_    = "RUN_LIFECYCLE_STATUS_ERROR"
	RunCancelled = "RUN_LIFECYCLE_STATUS_CANCELLED"
	RunExpired   = "RUN_LIFECYCLE_STATUS_EXPIRED"
)

// RunStatusTerminal reports whether status is a terminal lifecycle value.
// Matches the SDK store's "may start a follow-up run" check: non-terminal
// statuses (creating/running/unspecified) block Send with "already has active run".
func RunStatusTerminal(status string) bool {
	switch status {
	case RunFinished, RunError_, RunCancelled, RunExpired:
		return true
	default:
		return false
	}
}

// IsError reports whether the run ended badly. A nil result counts as an error:
// a stream that ends without one means the run did not complete.
func (r *RunResult) IsError() bool {
	if r == nil {
		return true
	}
	if r.Error != nil {
		return true
	}
	return r.Status != "" && r.Status != RunFinished
}

// flexInt decodes a proto numeric that may arrive as a JSON number or, for
// int64 fields, as a JSON string.
type flexInt int64

func (f *flexInt) UnmarshalJSON(b []byte) error {
	if len(b) == 0 || string(b) == "null" {
		return nil
	}
	if b[0] == '"' {
		var s string
		if err := json.Unmarshal(b, &s); err != nil {
			return err
		}
		if s == "" {
			return nil
		}
		n, err := strconv.ParseInt(s, 10, 64)
		if err != nil {
			return err
		}
		*f = flexInt(n)
		return nil
	}
	var n int64
	if err := json.Unmarshal(b, &n); err != nil {
		return err
	}
	*f = flexInt(n)
	return nil
}

// Int returns the value as a plain int.
func (f flexInt) Int() int { return int(f) }

func firstNonEmpty(vals ...string) string {
	for _, v := range vals {
		if v != "" {
			return v
		}
	}
	return ""
}

// jsonString returns a JSON string's contents, or empty for numbers/objects/null.
func jsonString(b json.RawMessage) string {
	if len(b) == 0 || b[0] != '"' {
		return ""
	}
	var s string
	if err := json.Unmarshal(b, &s); err != nil {
		return ""
	}
	return s
}
