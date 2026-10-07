package engine

import (
	"context"
	"encoding/json"

	"recur/internal/domain"
)

// AgentRunner runs one agent turn and streams its parsed events out while it
// works, returning the final Result when the turn ends (PRD §6.1, §8.1).
//
// Implementations:
//   - ClaudeRunner — spawns `claude -p` (claude.go)
//   - CursorRunner / CodexRunner — their own agent loops
//   - HarnessRunner — Recur-owned loop over POST /v1/responses
//   - ScriptedRunner — test double (scripted.go); never the production default
//
// emit is called synchronously for each parsed event as it arrives. It must not
// block for long (the runner drives it inline off the agent's output); callers
// push onto a buffered channel / the Bus. emit may be nil (events discarded).
type AgentRunner interface {
	Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error)
}

// AgentSpec is everything needed to run one agent turn (PRD §8). The runner
// builds the `claude -p` argv from these fields (see ClaudeRunner.args).
type AgentSpec struct {
	WorkspaceID string // owning workspace (for event attribution)
	// ChatChild is a workspace with a parent. It cannot nest children.
	ChatChild bool
	// ChatCode is the conductor tab (PA*, MA) for a loop specialist turn.
	ChatCode string
	AgentID  string           // stable per-agent id (e.g. "<workspaceID>-CO")
	Role     domain.AgentRole // coder/reviewer/… selects the system prompt + tools
	Model    string           // BARE provider model id (opus/sonnet/composer-2.5); "" → provider default
	// Provider is the resolved provider slug for this turn, set by Engine.runAgent
	// from the qualified model ref. Runners ignore it — they already know who they
	// are — but the session mirror needs it, and it cannot recover it from Model
	// because runAgent hands runners the BARE id.
	Provider     string
	Flags        map[string]string // provider-owned dims (effort/context/fast/…); "" keys ignored
	SystemPrompt string            // role / Recur overlay; Claude appends this, does not replace
	// AppendSystemPrompt is extra overlay (chat-mode notes, simplify). Claude
	// concatenates it after SystemPrompt onto --append-system-prompt so Claude
	// Code's default system prompt stays. Codex uses SystemPrompt as
	// developerInstructions; Cursor inlines it on the first Send.
	AppendSystemPrompt string
	// TurnAppendPrompt is per-turn context (which surface / PR the operator is
	// looking at). Cursor and Codex put it on every user message, including
	// resumes; mode notes stay in AppendSystemPrompt and are skipped on resume.
	TurnAppendPrompt string
	// RefreshStandingInstructions forces Cursor/Codex to re-attach role and
	// append notes in the user message even when ResumeSession is set. Mode-switch
	// remounts and human dock-mode changes need this; ordinary follow-ups must
	// not, or those providers re-bill standing context they already have in-session.
	RefreshStandingInstructions bool

	AllowedTools []string // scoped tool list (--allowedTools); e.g. Read,Edit,Write,Bash,Grep
	// Prompt is the full wire sent to the model AND persisted as content_json text
	// (may include @file /skill wraps). Provider rebuild reads this field only.
	Prompt string
	// DisplayText is an optional UI label persisted as content_json display_text.
	// Empty when Prompt is already the human-facing wire (no wraps / prompt-action
	// label). ask_user resume may set this to the short Answer line while Prompt
	// keeps the full resume prose for provider replay.
	DisplayText string
	// SkipRecordUser omits the user row in the session mirror. Chat Retry
	// and switch-mode remounts continue the last prompt without a second user
	// bubble. App-restart AutoResume does the same only when that prompt is
	// still an open turn (start checkpoint, no DurationMS footer).
	SkipRecordUser bool
	// UserMessageID pre-assigns the durable id of the user row so a worktree
	// checkpoint taken before the turn can be keyed to it. Empty → generated.
	UserMessageID string
	// Images are user-pasted chat attachments. Prompt may contain
	// ⟦attachment:<id>⟧ markers (UI appends them after the text in chip order);
	// Claude receives interleaved content blocks, Cursor receives text +
	// images[] in marker order.
	Images   []UserImage
	Worktree string // cmd.Dir — the loop's isolated worktree (PRD §11)

	// PermissionMode is the Claude Code --permission-mode value. It is
	// machine-global: callers set it from Settings.PermissionMode() on every
	// turn, never from the role or the tool list. Empty fails closed to
	// "dontAsk". See permissionArgs for how it reaches the CLI.
	PermissionMode string

	// MCPURL, if set, is this agent's per-token Recur MCP endpoint
	// (http://127.0.0.1:<port>/<token>/mcp). The runner passes it via
	// --mcp-config + --strict-mcp-config so the agent can call mcp__recur__*
	// tools, which the engine turns into typed callbacks (PRD §8.3, D7).
	MCPURL string

	// Playwright, when true, additionally wires a Playwright browser MCP server into
	// --mcp-config (alongside Recur) so the QA/Browser agent can drive the running
	// app (PRD §8.2, §10). Only the QA role sets this; the package is launched on
	// demand via npx (see playwrightMCPServer).
	Playwright bool

	// RecurSessionID is the Recur transcript session this turn mirrors into.
	// Native harnesses rebuild Responses input from it. Empty when the store
	// has no durable session (tests without a DB).
	RecurSessionID string
	// HistoryAfterID drops Recur transcript messages through this id (inclusive)
	// when rebuilding Responses input. Set from a fresh_session cutoff so a
	// reused tab does not replay the prior conversation. Empty = full history.
	HistoryAfterID string

	// UserMCP is extra MCP servers from Settings → MCP (Linear preset, custom
	// HTTP/stdio). Wired alongside Recur in --mcp-config. Do not log Headers/Env.
	UserMCP []MCPAttach

	// ResumeSession, if set, continues a prior `claude -p` session via --resume
	// <session_id> (headless ref §8.1, PRD §8.5) — the multi-turn continuity for the
	// Maestro chat. "" starts a fresh session. The runner returns the session id on
	// the Result so the engine can persist it and resume next turn.
	ResumeSession string

	// ForkSession, with ResumeSession, asks Claude Code for --fork-session.
	// Recur chat forks do not set this — they inject the copied transcript
	// into a fresh provider session. The flag remains so the Claude CLI
	// wiring stays testable if a caller ever needs a provider-side branch.
	ForkSession bool

	// PromptOnStdin puts Prompt on stream-json stdin instead of `-p <prompt>`
	// so a large fork transcript cannot blow ARG_MAX.
	PromptOnStdin bool

	// Route optionally overrides how this turn is keyed in the session store
	// and on the bus. When Kind is set, the turn may persist with a NULL
	// workspace_id (app-scoped Ask Recur).
	Route SessionRoute

	// IncludePartials asks the CLI for --include-partial-messages so stream_event
	// text/thinking deltas reach the chat (pair + loop cockpit). runAgent sets this
	// unless SkipThinking (cheap one-shots). Cursor/Codex gate KindThinkingDelta on it.
	IncludePartials bool

	// Tools, when non-nil, sets Claude Code's --tools flag (the complete built-in
	// toolset the model can see). Pass a pointer to "" to disable all tools — used
	// for one-shot completion turns (e.g. workspace auto-title). Nil leaves the CLI
	// default toolbox alone. Distinct from AllowedTools, which only auto-approves.
	Tools *string

	// MaxTurns, when > 0, sets --max-turns so a completion can't wander into a
	// multi-step tool loop. Zero leaves the CLI default (unlimited).
	MaxTurns int

	// SkipThinking omits --thinking / --thinking-display. Cheap one-shot turns
	// (title generation) don't need reasoning tokens.
	SkipThinking bool

	// Mirror, when set, receives this turn's raw provider lines and parsed events
	// so the session store can persist a lossless transcript alongside the live
	// stream. It is best-effort and additive: leaving it
	// nil (the default, and every ScriptedRunner path that doesn't opt in) changes
	// nothing about a turn's behavior. The runner calls Raw for every stdout line —
	// including line types the parser drops — so unknown provider vocabulary is
	// captured verbatim rather than lost.
	Mirror TurnMirror

	// Inject, when non-nil, is a mid-turn steer channel (D49). Claude opens
	// `--input-format stream-json` stdin and writes each steer as an additional
	// user message on the next tool_result; Codex calls turn/steer; Cursor
	// tries SteerRun (SDK run.steer) and falls back to CancelRun plus a
	// follow-up Send when the turn does not take the message. Newly spawned
	// agents still get the steer in Prompt instead; this channel is for the
	// already-running turn.
	Inject <-chan string
	// InjectAck, when non-nil, receives one bool per Inject payload after
	// the runner commits it (true) or drops it (false).
	InjectAck chan bool
	// InjectPlain, when true, sends Inject payloads as the user message with no
	// "Maestro steer" prefix. Workspace coding-agent follow-ups use this;
	// loop specialists leave it false so D49 wrapping still applies.
	InjectPlain bool
}

// MCPAttach is one extra MCP server for this turn (name is the mcp config key).
type MCPAttach struct {
	Name    string
	URL     string
	Headers map[string]string
	Command string
	Args    []string
	Env     map[string]string
}

// SessionRoute keys a turn in the session store and on session.* bus events.
// Empty WorkspaceID means an app-scoped session (workspace_id NULL).
type SessionRoute struct {
	WorkspaceID string
	Kind        string
	Code        string
	Name        string
}

// standingInUserPrompt reports whether role / AppendSystemPrompt notes should
// ride in the user message. Claude uses CLI flags instead. Cursor has no
// system-prompt API; Codex uses developerInstructions for SystemPrompt but
// still needs AppendSystemPrompt in-band on a fresh thread. TurnAppendPrompt
// is independent — always in-band on Cursor/Codex.
//
// resumed is true only when this turn actually continued a live provider
// session (ResumeAgent / thread/resume succeeded). A failed resume that fell
// back to a new agent/thread must re-attach standing notes.
func standingInUserPrompt(spec AgentSpec, resumed bool) bool {
	if spec.RefreshStandingInstructions {
		return true
	}
	return !resumed
}

// ParsedEvent carries a provider's RAW tool vocabulary — Tool, ToolInputRaw and
// a tool result's Text are verbatim provider data, because the session store is
// the lossless record of what the provider actually did. Rendering them with the
// common cross-provider names happens at the display boundary (toolview.go).
//
// ParsedEvent is one typed event parsed out of an agent's stream-json output
// (headless ref §3). Kind selects which fields are meaningful; unknown stream-json
// line types are dropped by the parser (never surface as a ParsedEvent).
type ParsedEvent struct {
	Kind ParsedKind

	// Provider is the slug of the provider that produced this event, so display
	// code can translate its raw tool vocabulary without threading the spec
	// through every consumer. Empty means Claude Code.
	Provider string

	// Init (system/init line).
	SessionID  string
	Model      string
	MCPServers []MCPServerStatus

	// Step (assistant tool_use block).
	Tool         string          // tool name: Read/Edit/Write/Bash/Grep, or mcp__recur__*
	ToolInput    string          // a short human summary of the tool input (file path, command, …)
	ToolInputRaw json.RawMessage // the verbatim tool input JSON (for lossless session mirroring)
	ToolUseID    string
	// ParentToolUseID is set when this event belongs to a Claude Code Task
	// subagent multiplexed onto the parent stream-json (parent_tool_use_id).
	// Empty for top-level parent events. Nested result/init must not be
	// treated as the parent turn; nested text/tools render inside the Task card.
	ParentToolUseID string

	// Text (assistant text block) / Result final text / Thinking (thinking block text).
	Text string
	// DisplayText is an optional UI label for KindUser (wrapped composer /
	// prompt-action). Empty means Text is already human-facing.
	DisplayText string
	// LiveSteer is set on KindUser when SteerRun delivered into the current
	// run. Cursor follow-up Send leaves this false so tool-id maps reset.
	LiveSteer bool

	// Signature is the opaque signature on a thinking block (Anthropic round-trip).
	Signature string

	// Tokens is the cumulative estimated thinking tokens (KindThinkingTokens) —
	// the only visibility headless Claude Code gives into REDACTED thinking.
	Tokens int64

	// ToolResult (user tool_result block).
	IsError bool

	// Truncated marks a result the PROVIDER clipped for size. Without it a
	// half-shown file or command output reads as the whole thing.
	Truncated bool

	// CompactStatus is started|done on KindCompact. CompactTrigger is auto|manual
	// when the provider reports it. Text carries an optional summary body.
	CompactStatus  string
	CompactTrigger string

	// FromModel / ToModel are set on KindFallback: the declining model and the
	// one that continued (Claude Fable → Opus classifier fallback).
	FromModel string
	ToModel   string
	// UUID is the provider frame id (Claude Code stream-json `uuid`). RetractedUUIDs
	// are frames the provider tombstoned — from assistant `supersedes` (immediate)
	// or system/model_refusal_fallback.retracted_message_uuids (audit).
	UUID           string
	RetractedUUIDs []string

	// Result (final result line) — carried in Result, but the terminal ParsedEvent
	// of KindResult mirrors the headline fields for emit consumers.
	Result *Result
}

// nested reports a multiplexed Claude Code subagent event (parent_tool_use_id).
func (ev ParsedEvent) nested() bool { return ev.ParentToolUseID != "" }

// ParsedKind tags a ParsedEvent.
type ParsedKind string

const (
	KindInit           ParsedKind = "init"            // system/init: session_id, model, mcp_servers
	KindText           ParsedKind = "text"            // assistant text content (complete block)
	KindTextDelta      ParsedKind = "text_delta"      // partial text from stream_event (IncludePartials)
	KindThinking       ParsedKind = "thinking"        // assistant thinking content (complete block) — mirrored
	KindThinkingDelta  ParsedKind = "thinking_delta"  // partial thinking text from stream_event (live only)
	KindThinkingTokens ParsedKind = "thinking_tokens" // thinking-progress token estimate (headless REDACTS the text)
	KindStep           ParsedKind = "step"            // assistant tool_use: a Read/Edit/Write/Bash/Grep step
	KindToolResult     ParsedKind = "tool_result"     // user tool_result
	KindUsage          ParsedKind = "usage"           // mid-turn token usage (live meter; not mirrored)
	KindNotice         ParsedKind = "notice"          // Recur status (drain, retry); Activity only, not transcript
	KindCompact        ParsedKind = "compact"         // provider auto/manual context compaction
	KindUser           ParsedKind = "user"            // mid-turn user message (Cursor D49 steer)
	KindFallback       ParsedKind = "fallback"        // model handoff (Fable classifier → Opus)
	KindResult         ParsedKind = "result"          // final result line
)

// TurnMirror receives a turn's parsed events so a session store can persist a
// lossless normalized transcript. Event is called
// synchronously from the runner's read loop, in stream order, so an implementation
// must not block for long and must not panic. Implementations live in the engine
// (the production one maps events onto store.Store session writes; see
// sessionMirror).
type TurnMirror interface {
	// Event records one parsed event (the normalized projection of the stream).
	Event(ev ParsedEvent)
}

// MCPServerStatus mirrors a system/init mcp_servers[] entry (headless ref §3).
type MCPServerStatus struct {
	Name   string `json:"name"`
	Status string `json:"status"` // connected | failed | pending
}

// Result is the final outcome of an agent turn, captured from the stream-json
// `result` line (headless ref §3). Error subtypes carry no final text.
type Result struct {
	SessionID  string `json:"sessionId"`
	Subtype    string `json:"subtype"` // success | error_max_turns | error_during_execution | …
	IsError    bool   `json:"isError"`
	NumTurns   int    `json:"numTurns"`
	DurationMS int    `json:"durationMs"`
	Text       string `json:"text"` // final assistant text (success only)

	// TurnEndEmitted is set by mirroringRunner after inner.Run when this turn
	// published session.turn_end (KindResult with DurationMS > 0). It is not
	// a provider field — Claude/Cursor return copies distinct from the emit
	// payload. finalizeChatReply uses it so persistChatFailure does not stamp
	// a second footer, and so a Retry that fails before any new events does
	// not mistake the previous turn's footer for this run.
	TurnEndEmitted bool `json:"-"`

	// Token usage for the chat context meter: the latest per-inference prompt
	// footprint (Claude assistant.message.usage / Cursor mid-stream usage), not
	// cumulative result.usage billing which can exceed the window.
	InputTokens         int64 `json:"inputTokens,omitempty"`
	OutputTokens        int64 `json:"outputTokens,omitempty"`
	CacheCreationTokens int64 `json:"cacheCreationTokens,omitempty"`
	CacheReadTokens     int64 `json:"cacheReadTokens,omitempty"`
	// ContextWindow is the model limit in tokens when the provider reports it
	// (Claude modelUsage.contextWindow) or when resolved from Claude flags.
	ContextWindow int64 `json:"contextWindow,omitempty"`

	// Stderr is the tail of what the CLI wrote to stderr. Usage/spend-limit refusals,
	// auth failures and provider outages are reported there rather than in the
	// stream-json result, so this is what lets the engine tell a TRANSIENT failure
	// (retryable from the checkpoint) from a genuine one (D56).
	Stderr string `json:"stderr,omitempty"`
}
