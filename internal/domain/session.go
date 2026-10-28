package domain

import (
	"encoding/json"
	"strings"
)

// The session model is Recur's lossless, provider-neutral
// transcript store: a Session owns an ordered set of Messages, each Message owns
// typed ContentBlocks. It replaces the v1 world where conversation state lived
// only in Claude Code's own session files and the app kept nothing but a resume
// pointer and lossy rendered feed strings.

// Context modes. In provider mode the LLM provider owns context
// assembly and Recur only mirrors; in app mode the store IS the context.
const (
	ContextModeProvider = "provider"
	ContextModeApp      = "app"
)

// Provider identifiers stored on session/message rows. Neutral names, mapped to
// real API strings inside each provider implementation.
const (
	ProviderClaudeCode = "claude-code"
	ProviderCursor     = "cursor"
	ProviderCodex      = "codex"
	ProviderChatGPT    = "chatgpt"
	ProviderAnthropic  = "anthropic"
	ProviderOpenAI     = "openai"
	ProviderGemini     = "gemini"
	ProviderOllama     = "ollama"
)

// Stream status of a message. A message created at turn start is
// pending/streaming; it becomes complete once its final block is written, or
// error if the turn failed.
const (
	StreamPending   = "pending"
	StreamStreaming = "streaming"
	StreamComplete  = "complete"
	StreamError     = "error"
)

// Content-block types. The set is OPEN — a stored type the UI
// doesn't recognize must round-trip and render as a fallback, never error — so
// this is a documented list, not an exhaustive enum.
const (
	BlockText       = "text"
	BlockThinking   = "thinking"
	BlockToolUse    = "tool_use"
	BlockToolResult = "tool_result"
	BlockImage      = "image"
	BlockCompact    = "compact"
	BlockFallback   = "fallback"
)

// Session is one agent conversation: a role/kind, a provider, a config, and an
// ordered transcript. It belongs to a workspace (a workspace of
// any type — loop, pair, task…).
type Session struct {
	ID                 string `json:"id"`
	WorkspaceID        string `json:"workspaceId,omitempty"`
	Kind               string `json:"kind"` // maestro | coder | reviewer | qa | pair | task | …
	ParentSessionID    string `json:"parentSessionId,omitempty"`
	ForkMessageID      string `json:"forkMessageId,omitempty"`
	ContextMode        string `json:"contextMode"` // provider | app
	Provider           string `json:"provider"`
	ProviderSessionRef string `json:"providerSessionRef,omitempty"` // e.g. claude session id for --resume
	ConfigJSON         string `json:"configJson,omitempty"`         // system prompt, tools, default model/effort
	ActiveContextID    string `json:"activeContextId,omitempty"`    // compaction pointer (app mode)
	Status             string `json:"status"`                       // idle | streaming | error
	CreatedAt          int64  `json:"createdAt"`
	UpdatedAt          int64  `json:"updatedAt"`
}

// SessionContext is one compaction watermark. Full history
// stays on session_message; this row is the summary + last compacted message
// used for fork inject and app-mode request assembly. session.active_context_id
// points at the current row; re-compaction inserts a new row and repoints.
type SessionContext struct {
	ID                     string `json:"id"`
	SessionID              string `json:"sessionId"`
	Summary                string `json:"summary"`
	LastCompactedMessageID string `json:"lastCompactedMessageId"`
	CreatedAt              int64  `json:"createdAt"`
}

// Message is one turn element (user/assistant/system), stamped with the model and
// provider that produced it so history is self-describing across mid-session model
// changes. Content is loaded from message_content in block order.
type Message struct {
	ID                  string `json:"id"`
	OriginSessionID     string `json:"originSessionId"` // provenance across forks
	Role                string `json:"role"`            // user | assistant | system
	ModelID             string `json:"modelId,omitempty"`
	Provider            string `json:"provider,omitempty"`
	StreamStatus        string `json:"streamStatus"`
	StartedAt           int64  `json:"startedAt,omitempty"`
	CompletedAt         int64  `json:"completedAt,omitempty"`
	InputTokens         int64  `json:"inputTokens,omitempty"`
	OutputTokens        int64  `json:"outputTokens,omitempty"`
	CacheCreationTokens int64  `json:"cacheCreationTokens,omitempty"`
	CacheReadTokens     int64  `json:"cacheReadTokens,omitempty"`
	// DurationMS is the wall time of the TURN this message ended, as reported by
	// the provider. Stamped on the last message of a turn, so it is zero on
	// every other message.
	DurationMS int64 `json:"durationMs,omitempty"`
	// ContextWindow is the model's context limit for the turn that stamped this
	// message (tokens). Used with input/cache tokens for the chat context meter.
	ContextWindow int64 `json:"contextWindow,omitempty"`
	CreatedAt     int64 `json:"createdAt"`

	Content []ContentBlock `json:"content"`
}

// ContentBlock is one typed part of a message. Payload is stored
// as normalized JSON in content_json; the typed constructors below build the
// canonical shapes. Type is an open set (unknown types render as a fallback).
type ContentBlock struct {
	ID          string          `json:"id"`
	Type        string          `json:"type"`
	ContentJSON json.RawMessage `json:"content"`
	// CreatedAt is when the block was mirrored (unix ms). Loaded on read; it is the
	// per-block timestamp the activity-feed projection merges on.
	CreatedAt int64 `json:"createdAt,omitempty"`
}

// ---- typed block payloads ----

// TextPayload is a `text` block.
type TextPayload struct {
	Text        string `json:"text"`                   // full wire (provider replay)
	DisplayText string `json:"display_text,omitempty"` // UI-only; providers ignore
}

// ThinkingPayload is a `thinking` block; the signature is preserved for the
// Anthropic round-trip. EstimatedTokens is Recur's own annotation for REDACTED
// thinking: headless Claude Code withholds the reasoning text (empty string +
// signature) and reports only cumulative token estimates — kept here so the UI
// can still render "Thought ~N tokens".
type ThinkingPayload struct {
	Thinking        string `json:"thinking"`
	Signature       string `json:"signature,omitempty"`
	EstimatedTokens int64  `json:"estimated_tokens,omitempty"`
}

// ToolUsePayload is a `tool_use` block. It carries BOTH an internal id and the
// provider's tool-use id (the dual-ID trick that makes cross-provider replay
// tractable — OpenAI needs the mapping, Gemini has no tool ids at all).
type ToolUsePayload struct {
	ID                string `json:"id"`
	ProviderToolUseID string `json:"provider_tool_use_id,omitempty"`
	// ParentToolUseID is the provider tool_use id of a parent Task when this
	// call was made by a multiplexed Claude Code subagent. Empty for top-level.
	ParentToolUseID string          `json:"parent_tool_use_id,omitempty"`
	Name            string          `json:"name"`
	Input           json.RawMessage `json:"input,omitempty"`
}

// ToolResultPayload is a `tool_result` block. tool_use_id is the INTERNAL id it
// answers (the json_extract '$.tool_use_id' the one-result-per-call unique index
// keys on).
type ToolResultPayload struct {
	ToolUseID string `json:"tool_use_id"`
	Result    string `json:"result"`
	IsError   bool   `json:"is_error,omitempty"`
	// Truncated marks a result the PROVIDER clipped for size (Cursor's SDK sets
	// a `truncated` flag on the tool_call frame). Stored because it is provider
	// fact, and because a clipped result otherwise reads as the whole thing.
	Truncated bool `json:"truncated,omitempty"`
}

// ImagePayload is an `image` block for a user-pasted chat attachment. Path is
// relative to the managed data root (e.g. workspaces/<ws>/attachments/<id>.png);
// bytes live on disk, not in content_json.
type ImagePayload struct {
	ID   string `json:"id"`
	Name string `json:"name,omitempty"`
	Mime string `json:"mime"`
	Path string `json:"path"`
}

// ChatAttachment is an image the user pasted into agent chat (send path). Data is
// raw base64 (no data: URL prefix).
type ChatAttachment struct {
	ID   string `json:"id"`
	Name string `json:"name,omitempty"`
	Mime string `json:"mime"`
	Data string `json:"data"`
}

// ChatImageRef is a UI/transcript pointer to a persisted chat attachment (no bytes).
type ChatImageRef struct {
	ID   string `json:"id"`
	Name string `json:"name,omitempty"`
	Mime string `json:"mime,omitempty"`
	URL  string `json:"url,omitempty"` // /workspaces/<ws>/attachments/<file>
}

// TextBlock builds a normalized text content block.
func TextBlock(text string) ContentBlock {
	return TextBlockDisplay(text, "")
}

// TextBlockDisplay builds a text content block with optional UI-only display_text.
// Providers and session rebuild read Text; the chat flatten uses DisplayText when set.
func TextBlockDisplay(text, display string) ContentBlock {
	return block(BlockText, TextPayload{Text: text, DisplayText: display})
}

// ThinkingBlock builds a normalized thinking content block. tokens is the running
// estimate for redacted thinking (0 when the text itself is present).
func ThinkingBlock(thinking, signature string, tokens int64) ContentBlock {
	return block(BlockThinking, ThinkingPayload{Thinking: thinking, Signature: signature, EstimatedTokens: tokens})
}

// ToolUseBlock builds a normalized tool_use content block.
func ToolUseBlock(internalID, providerID, name string, input json.RawMessage) ContentBlock {
	return ToolUseBlockNested(internalID, providerID, "", name, input)
}

// ToolUseBlockNested is ToolUseBlock with an optional parent Task provider id.
func ToolUseBlockNested(internalID, providerID, parentProviderID, name string, input json.RawMessage) ContentBlock {
	return block(BlockToolUse, ToolUsePayload{
		ID: internalID, ProviderToolUseID: providerID, ParentToolUseID: parentProviderID, Name: name, Input: input,
	})
}

// ToolResultBlock builds a normalized tool_result content block. internalID is the
// internal tool_use id this result answers.
func ToolResultBlock(internalID, result string, isError bool) ContentBlock {
	return block(BlockToolResult, ToolResultPayload{
		ToolUseID: internalID, Result: result, IsError: isError,
	})
}

// TruncatedToolResultBlock is ToolResultBlock for a result the provider clipped.
func TruncatedToolResultBlock(internalID, result string, isError, truncated bool) ContentBlock {
	return block(BlockToolResult, ToolResultPayload{
		ToolUseID: internalID, Result: result, IsError: isError, Truncated: truncated,
	})
}

// ImageBlock builds a normalized image content block for a chat attachment.
func ImageBlock(id, name, mime, path string) ContentBlock {
	return block(BlockImage, ImagePayload{ID: id, Name: name, Mime: mime, Path: path})
}

// CompactStatus values stored on a compact block / ChatItem.
const (
	CompactStarted = "started"
	CompactDone    = "done"
)

// CompactPayload is the inline "compacted here" chip on the transcript.
// The compaction pointer itself is SessionContext /
// session.active_context_id — this block is UI, not the store.
type CompactPayload struct {
	Status    string `json:"status"`              // started | done
	Trigger   string `json:"trigger,omitempty"`   // auto | manual
	Summary   string `json:"summary,omitempty"`   // provider summary text when present
	PreTokens int64  `json:"preTokens,omitempty"` // tokens before compact, when reported
}

// CompactBlock builds a normalized compact content block.
func CompactBlock(status, trigger, summary string, preTokens int64) ContentBlock {
	return block(BlockCompact, CompactPayload{
		Status: status, Trigger: trigger, Summary: summary, PreTokens: preTokens,
	})
}

// FallbackPayload is a `fallback` block: Claude Fable 5 (and similar) declined
// and another model continued the turn (typically Opus).
type FallbackPayload struct {
	From string `json:"from"` // requested / declining model id
	To   string `json:"to"`   // model that continued
}

// FallbackBlock builds a normalized model-fallback content block.
func FallbackBlock(from, to string) ContentBlock {
	return block(BlockFallback, FallbackPayload{From: from, To: to})
}

// RawBlock stores an unknown/opaque block type verbatim so it round-trips
// (forward compatibility).
func RawBlock(typ string, payload json.RawMessage) ContentBlock {
	return ContentBlock{Type: typ, ContentJSON: payload}
}

func block(typ string, payload any) ContentBlock {
	b, err := json.Marshal(payload)
	if err != nil {
		// The payloads above are all plain structs; marshal cannot fail in practice.
		b = []byte(`{}`)
	}
	return ContentBlock{Type: typ, ContentJSON: b}
}

// AsText decodes a text block's payload; ok is false for other types.
func (c ContentBlock) AsText() (TextPayload, bool) {
	if c.Type != BlockText {
		return TextPayload{}, false
	}
	var p TextPayload
	return p, json.Unmarshal(c.ContentJSON, &p) == nil
}

// AsThinking decodes a thinking block's payload; ok is false for other types.
func (c ContentBlock) AsThinking() (ThinkingPayload, bool) {
	if c.Type != BlockThinking {
		return ThinkingPayload{}, false
	}
	var p ThinkingPayload
	return p, json.Unmarshal(c.ContentJSON, &p) == nil
}

// AsToolUse decodes a tool_use block's payload; ok is false for other types.
func (c ContentBlock) AsToolUse() (ToolUsePayload, bool) {
	if c.Type != BlockToolUse {
		return ToolUsePayload{}, false
	}
	var p ToolUsePayload
	return p, json.Unmarshal(c.ContentJSON, &p) == nil
}

// AsToolResult decodes a tool_result block's payload; ok is false for other types.
func (c ContentBlock) AsToolResult() (ToolResultPayload, bool) {
	if c.Type != BlockToolResult {
		return ToolResultPayload{}, false
	}
	var p ToolResultPayload
	return p, json.Unmarshal(c.ContentJSON, &p) == nil
}

// AsImage decodes an image block's payload; ok is false for other types.
func (c ContentBlock) AsImage() (ImagePayload, bool) {
	if c.Type != BlockImage {
		return ImagePayload{}, false
	}
	var p ImagePayload
	return p, json.Unmarshal(c.ContentJSON, &p) == nil
}

// AsCompact decodes a compact block's payload; ok is false for other types.
func (c ContentBlock) AsCompact() (CompactPayload, bool) {
	if c.Type != BlockCompact {
		return CompactPayload{}, false
	}
	var p CompactPayload
	return p, json.Unmarshal(c.ContentJSON, &p) == nil
}

// ParentToolUseID is the provider tool-use id of the Agent/Task call this
// block belongs to. Empty means the block is from the parent agent.
func (c ContentBlock) ParentToolUseID() string {
	var head struct {
		Parent string `json:"parent_tool_use_id"`
	}
	if json.Unmarshal(c.ContentJSON, &head) != nil {
		return ""
	}
	return strings.TrimSpace(head.Parent)
}

// WithParent stamps parent_tool_use_id onto the block payload so nested
// subagent output can be reconstructed without a schema migration.
func (c ContentBlock) WithParent(parent string) ContentBlock {
	parent = strings.TrimSpace(parent)
	if parent == "" {
		return c
	}
	var m map[string]json.RawMessage
	if len(c.ContentJSON) == 0 || json.Unmarshal(c.ContentJSON, &m) != nil {
		return c
	}
	b, err := json.Marshal(parent)
	if err != nil {
		return c
	}
	m["parent_tool_use_id"] = b
	raw, err := json.Marshal(m)
	if err != nil {
		return c
	}
	c.ContentJSON = raw
	return c
}

// AsFallback decodes a fallback block's payload; ok is false for other types.
func (c ContentBlock) AsFallback() (FallbackPayload, bool) {
	if c.Type != BlockFallback {
		return FallbackPayload{}, false
	}
	var p FallbackPayload
	return p, json.Unmarshal(c.ContentJSON, &p) == nil
}

// Chat modes: how a chat session's agent may act.
// The mode is a property of the CHAT SESSION, not the workspace — one workspace
// can host an Ask chat and an Agent chat side by side. Enforcement is structural
// (tool scoping in Go, the Reviewer's proven mechanism), never prompt text.
const (
	ChatModeAsk    = "ask"    // read-only Q&A over the worktree
	ChatModePlan   = "plan"   // read-only exploration → a structured, approvable plan
	ChatModeAgent  = "agent"  // the default coding chat: read + edit
	ChatModeReview = "review" // read-only review pass; files structured findings
	ChatModeLoop   = "loop"   // the autopilot; the chat is the Maestro thread
)

// NormalizeChatMode maps empty/unknown modes to "agent" (the default coding
// chat). Unknown values normalize rather than error so a stored mode from a
// newer build degrades to a safe default instead of wedging the chat.
func NormalizeChatMode(m string) string {
	switch m {
	case ChatModeAsk, ChatModePlan, ChatModeAgent, ChatModeReview, ChatModeLoop:
		return m
	}
	return ChatModeAgent
}

// WorkspaceChatTab is one interactive coding-agent chat tab in a solo/pair
// workspace Agent panel (codes PA, PA2, …).
type WorkspaceChatTab struct {
	Code      string `json:"code"`
	Title     string `json:"title"`
	UpdatedAt int64  `json:"updatedAt"`
	// Mode is the tab's chat mode (ask/plan/agent/review/loop); empty means the
	// tab has never had a mode persisted (the dock then defaults to agent).
	Mode string `json:"mode,omitempty"`
	// Unseen is true when this tab finished a turn the human has not viewed yet.
	Unseen bool `json:"unseen,omitempty"`
	// LoopID / PolicyID are this tab's [[loop]] + [[policy]] (one chat = one loop).
	LoopID   string `json:"loopId,omitempty"`
	PolicyID string `json:"policyId,omitempty"`
	// LoopStarted is true after this tab has engaged its own run.
	LoopStarted bool `json:"loopStarted,omitempty"`
	// PromptGroup is a shared slot for fresh_session agent flagships so
	// /commit and /open-pr reuse one tab instead of the user's working chat.
	PromptGroup string `json:"promptGroup,omitempty"`
}

// AttentionCounts is the rollup for unseen chat tabs + inbox unread — drives
// sidebar workspace/project badges and the macOS dock icon badge (D-attention).
type AttentionCounts struct {
	Unseen      int            `json:"unseen"`
	InboxUnread int            `json:"inboxUnread"`
	Dock        int            `json:"dock"` // unseen + non-chat inbox unread + app unseen
	ByWorkspace map[string]int `json:"byWorkspace"`
	ByProject   map[string]int `json:"byProject"`
	// AppUnseen is 1 when the persistent Ask Recur session finished a turn the
	// human has not viewed. Counted once; not Resource.NeedsYou.
	AppUnseen int `json:"appUnseen,omitempty"`
}

// SessionView is one agent session summarized for the Plan panel (todos) and the
// Loop chat cockpit (specialist cards): identity, lifecycle, and the flattened
// transcript. Live turns also stream via session.* events.
type SessionView struct {
	ID        string     `json:"id"`
	Code      string     `json:"code"`
	Name      string     `json:"name"`
	Kind      string     `json:"kind"`
	Status    string     `json:"status"`
	OwnerCode string     `json:"ownerCode,omitempty"`
	CreatedAt int64      `json:"createdAt"`
	UpdatedAt int64      `json:"updatedAt"`
	Items     []ChatItem `json:"items"`
}

// ChatItem is one rendered line of an agent chat transcript — a flattened, UI-ready
// view of the session's messages/content blocks (a user/assistant text turn, the
// agent's reasoning, or a tool call with its result). Tool calls carry enough for
// both the collapsed row (Description/ToolInput) and the expanded detail card
// (Input/Result), so the chat can render the whole turn without re-parsing blocks.
type ChatItem struct {
	Role      string `json:"role"`                // user | assistant
	Kind      string `json:"kind"`                // text | tool | thinking | turn | compact | fallback
	Text      string `json:"text,omitempty"`      // kind=text | thinking | compact summary | fallback to-label
	Tool      string `json:"tool,omitempty"`      // kind=tool: tool name (Bash, Read, …)
	ToolInput string `json:"toolInput,omitempty"` // kind=tool: short human summary of the input
	Tokens    int64  `json:"tokens,omitempty"`    // kind=thinking: redacted estimate; kind=compact: pre-compact tokens

	// Description is the agent's own one-line label for a tool call (Claude Code
	// sets input.description on Bash/Task calls) — the collapsed row's title.
	// kind=compact: trigger (auto|manual). kind=fallback: from-model label.
	Description string `json:"description,omitempty"`
	// Input is the tool call's full input JSON for the expanded detail card
	// (command, old/new strings, file content, …). Bounded for most tools;
	// CreatePlan keeps a valid {"plan":…} up to the plan persist size.
	Input string `json:"input,omitempty"`
	// Result is the tool call's result text (truncated); ResultError marks a failed call.
	Result      string `json:"result,omitempty"`
	ResultError bool   `json:"resultError,omitempty"`
	// ResultTruncated marks a result the PROVIDER clipped for size, so the card
	// can say so rather than presenting a fragment as the whole output.
	ResultTruncated bool `json:"resultTruncated,omitempty"`
	// ResultPending marks a call that has not reported a result yet — it is still
	// running. An empty Result is NOT the same thing: delete and TodoWrite both
	// legitimately return nothing, so the card needs an explicit signal to tell
	// "in flight" from "finished with no output".
	ResultPending bool `json:"resultPending,omitempty"`

	// DurationMS is set on kind="turn" — a footer closing a finished agent turn,
	// carrying how long the provider says it took. Emitted per TURN, not per
	// message: one turn spans several messages as tool calls interleave.
	DurationMS int64 `json:"durationMs,omitempty"`

	// MessageID is the durable message that closed this turn (kind="turn").
	// Fork points at this id — never a timestamp.
	MessageID string `json:"messageId,omitempty"`
	// Checkpoint marks a turn whose start→end snapshot changed files, so
	// "diff this turn" and "reset to here" are available (engine/checkpoint.go).
	Checkpoint bool `json:"checkpoint,omitempty"`

	// Context usage on kind="turn" — last-turn prompt fill vs model window, so
	// the chat context meter survives reload without a separate API.
	InputTokens         int64 `json:"inputTokens,omitempty"`
	CacheCreationTokens int64 `json:"cacheCreationTokens,omitempty"`
	CacheReadTokens     int64 `json:"cacheReadTokens,omitempty"`
	ContextWindow       int64 `json:"contextWindow,omitempty"`

	// Attachments are images pasted into a user message (kind=text). Text may
	// contain ⟦attachment:<id>⟧ markers at the caret positions where they were
	// pasted; the UI replaces those markers with thumbnails.
	Attachments []ChatImageRef `json:"attachments,omitempty"`

	// Ts is when this row happened (unix ms), from the message/block CreatedAt.
	// The Loop chat cockpit merges Maestro turns with specialist sessions and
	// engine facts by this stamp.
	Ts int64 `json:"ts,omitempty"`

	// ToolID is the provider tool-use id (toolu_…) for kind=tool. Live nested
	// subagent events key off this to attach inside the parent card.
	ToolID string `json:"toolId,omitempty"`

	// Children are nested subagent output (Grep/Read/text/thinking) that belongs
	// under this row (the parent Task), not as sibling transcript items.
	Children []ChatItem `json:"children,omitempty"`
}
