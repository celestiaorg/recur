package engine

import (
	"encoding/json"
	"fmt"
	"strings"
	"unicode/utf8"

	"recur/internal/domain"
)

// parseStreamLine parses one NDJSON line of `claude -p --output-format stream-json`
// output (headless ref §3) into zero or more typed ParsedEvents. It switches on
// the top-level `type` and IGNORES unknown types (the union has ~30 variants);
// unknown/blank lines yield (nil, nil). A line may produce several events (e.g.
// an assistant message with multiple tool_use blocks).
//
// The shapes match the literal schemas in docs/claude-headless-reference.md §3:
//   - system/init  → session_id, model, mcp_servers[]
//   - assistant    → message.content[] with tool_use blocks
//   - user         → message.content[] with tool_result blocks
//   - result       → success carries result/total_cost_usd/…; error subtypes have NO result field
func parseStreamLine(line []byte) ([]ParsedEvent, error) {
	line = trimSpace(line)
	if len(line) == 0 {
		return nil, nil
	}

	var head struct {
		Type            string          `json:"type"`
		Subtype         string          `json:"subtype"`
		ParentToolUseID json.RawMessage `json:"parent_tool_use_id"`
	}
	if err := json.Unmarshal(line, &head); err != nil {
		return nil, fmt.Errorf("stream-json: %w", err)
	}

	parentID := nestedParentToolUseID(head.ParentToolUseID)
	if parentID != "" {
		// Nested Task subagent. Keep tools, text, thinking, and live deltas so
		// they render inside the parent Task card. Drop nested result/init/
		// compact/usage — those must not close or speak as the parent turn.
		switch head.Type {
		case "assistant":
			evs, err := parseAssistant(line)
			return stampNestedParent(evs, parentID), err
		case "user":
			evs, err := parseUser(line)
			return stampNestedParent(evs, parentID), err
		case "stream_event":
			evs, err := parseStreamEvent(line)
			return stampNestedParent(evs, parentID), err
		case "system":
			if head.Subtype == "thinking_tokens" {
				var msg struct {
					EstimatedTokens int64 `json:"estimated_tokens"`
				}
				if err := json.Unmarshal(line, &msg); err != nil || msg.EstimatedTokens <= 0 {
					return nil, nil //nolint:nilerr // malformed system line — ignore like other subtypes
				}
				return stampNestedParent([]ParsedEvent{{Kind: KindThinkingTokens, Tokens: msg.EstimatedTokens}}, parentID), nil
			}
			return nil, nil
		default:
			return nil, nil
		}
	}

	switch head.Type {
	case "system":
		switch head.Subtype {
		case "init":
			return parseInit(line)
		case "thinking_tokens":
			// Headless Claude Code REDACTS thinking text (empty string + signature)
			// and reports progress as cumulative token estimates instead.
			// The envelope uuid is per-event (uuid:l()), not the assistant frame
			// id used for fallback retraction — leave it off so live Thought
			// rows are not split on every token tick.
			var msg struct {
				EstimatedTokens int64 `json:"estimated_tokens"`
			}
			if err := json.Unmarshal(line, &msg); err != nil || msg.EstimatedTokens <= 0 {
				return nil, nil //nolint:nilerr // malformed system line — ignore like other subtypes
			}
			return []ParsedEvent{{Kind: KindThinkingTokens, Tokens: msg.EstimatedTokens}}, nil
		case "compact_boundary", "compact_started":
			return parseCompactSystem(line, head.Subtype)
		case "status":
			return parseCompactStatus(line)
		case "model_refusal_fallback":
			return parseRefusalFallback(line)
		case "api_retry":
			return parseAPIRetry(line)
		}
		return nil, nil // other system subtypes (plugin_install) ignored
	case "assistant":
		return parseAssistant(line)
	case "user":
		return parseUser(line)
	case "result":
		return parseResult(line)
	case "stream_event":
		return parseStreamEvent(line)
	default:
		// Unknown types ignored (§3).
		return nil, nil
	}
}

// parseStreamEvent extracts streaming deltas from --include-partial-messages
// stream_event lines (headless ref §3): text_delta → KindTextDelta,
// thinking_delta → KindThinkingDelta, and content_block_start type=fallback →
// KindFallback (live handoff; the complete assistant line persists the block).
// Other delta kinds are ignored.
func parseStreamEvent(line []byte) ([]ParsedEvent, error) {
	var msg struct {
		UUID  string `json:"uuid"`
		Event struct {
			Type         string `json:"type"`
			ContentBlock *struct {
				Type string          `json:"type"`
				From json.RawMessage `json:"from"`
				To   json.RawMessage `json:"to"`
			} `json:"content_block"`
			Delta struct {
				Type            string `json:"type"`
				Text            string `json:"text"`
				Thinking        string `json:"thinking"`
				EstimatedTokens int64  `json:"estimated_tokens"`
			} `json:"delta"`
		} `json:"event"`
	}
	if err := json.Unmarshal(line, &msg); err != nil {
		return nil, fmt.Errorf("stream-json stream_event: %w", err)
	}
	if msg.Event.Type == "content_block_start" && msg.Event.ContentBlock != nil && msg.Event.ContentBlock.Type == "fallback" {
		return stampUUID(fallbackEvents(msg.Event.ContentBlock.From, msg.Event.ContentBlock.To), msg.UUID), nil
	}
	if msg.Event.Type != "content_block_delta" {
		return nil, nil
	}
	switch msg.Event.Delta.Type {
	case "text_delta":
		if msg.Event.Delta.Text == "" {
			return nil, nil
		}
		// Do not stamp the stream_event envelope uuid: Claude Code mints a
		// unique uuid per NDJSON line (uuid:l()), not per assistant frame.
		// Forwarding it made the live UI open a new Thought/text row per chunk.
		return []ParsedEvent{{Kind: KindTextDelta, Text: msg.Event.Delta.Text}}, nil
	case "thinking_delta":
		if msg.Event.Delta.Thinking != "" {
			return []ParsedEvent{{Kind: KindThinkingDelta, Text: msg.Event.Delta.Thinking}}, nil
		}
		// Redacted thinking: text withheld, only a cumulative token estimate.
		if msg.Event.Delta.EstimatedTokens > 0 {
			return []ParsedEvent{{Kind: KindThinkingTokens, Tokens: msg.Event.Delta.EstimatedTokens}}, nil
		}
		return nil, nil
	default:
		return nil, nil
	}
}

func parseInit(line []byte) ([]ParsedEvent, error) {
	var msg struct {
		SessionID  string            `json:"session_id"`
		Model      string            `json:"model"`
		MCPServers []MCPServerStatus `json:"mcp_servers"`
	}
	if err := json.Unmarshal(line, &msg); err != nil {
		return nil, fmt.Errorf("stream-json init: %w", err)
	}
	return []ParsedEvent{{
		Kind:       KindInit,
		SessionID:  msg.SessionID,
		Model:      msg.Model,
		MCPServers: msg.MCPServers,
	}}, nil
}

// parseCompactSystem handles Claude Code compact_boundary (after compact) and
// compact_started (before the silent window, when the CLI emits it).
func parseCompactSystem(line []byte, subtype string) ([]ParsedEvent, error) {
	var msg struct {
		CompactMetadata *struct {
			Trigger   string `json:"trigger"`
			PreTokens int64  `json:"preTokens"`
			PreSnake  int64  `json:"pre_tokens"`
		} `json:"compactMetadata"`
		CompactSnake *struct {
			Trigger   string `json:"trigger"`
			PreTokens int64  `json:"pre_tokens"`
			PreCamel  int64  `json:"preTokens"`
		} `json:"compact_metadata"`
	}
	if err := json.Unmarshal(line, &msg); err != nil {
		return nil, nil //nolint:nilerr // ignore malformed compact lines like other subtypes
	}
	status := domain.CompactDone
	if subtype == "compact_started" {
		status = domain.CompactStarted
	}
	ev := ParsedEvent{Kind: KindCompact, CompactStatus: status}
	meta := msg.CompactMetadata
	if meta == nil && msg.CompactSnake != nil {
		ev.CompactTrigger = msg.CompactSnake.Trigger
		ev.Tokens = msg.CompactSnake.PreTokens
		if ev.Tokens <= 0 {
			ev.Tokens = msg.CompactSnake.PreCamel
		}
		return []ParsedEvent{ev}, nil
	}
	if meta != nil {
		ev.CompactTrigger = meta.Trigger
		ev.Tokens = meta.PreTokens
		if ev.Tokens <= 0 {
			ev.Tokens = meta.PreSnake
		}
	}
	return []ParsedEvent{ev}, nil
}

// parseCompactStatus handles system/status{status:"compacting"} emitted during
// manual /compact. Other status values are ignored.
func parseCompactStatus(line []byte) ([]ParsedEvent, error) {
	var msg struct {
		Status string `json:"status"`
	}
	if err := json.Unmarshal(line, &msg); err != nil {
		return nil, nil //nolint:nilerr
	}
	if !strings.EqualFold(msg.Status, "compacting") {
		return nil, nil
	}
	return []ParsedEvent{{Kind: KindCompact, CompactStatus: domain.CompactStarted, CompactTrigger: "manual"}}, nil
}

// parseAPIRetry surfaces system/api_retry as a notice so a turn stuck in
// Claude Code's request retries does not look like a silent hang.
func parseAPIRetry(line []byte) ([]ParsedEvent, error) {
	var msg struct {
		Attempt     int    `json:"attempt"`
		MaxRetries  int    `json:"max_retries"`
		ErrorStatus *int   `json:"error_status"`
		Error       string `json:"error"`
	}
	if err := json.Unmarshal(line, &msg); err != nil || msg.Attempt <= 0 {
		return nil, nil //nolint:nilerr
	}
	reason := msg.Error
	if reason == "" {
		reason = "error"
	}
	if msg.ErrorStatus != nil {
		reason = fmt.Sprintf("%s %d", reason, *msg.ErrorStatus)
	}
	return []ParsedEvent{{Kind: KindNotice, Text: fmt.Sprintf("API request failed (%s), retrying %d/%d…", reason, msg.Attempt, msg.MaxRetries)}}, nil
}

// contentBlock is one entry of an Anthropic message.content[] array. Only the
// fields Recur reads are declared; input is kept raw for summarizing.
type contentBlock struct {
	Type string `json:"type"` // text | tool_use | tool_result | thinking | …

	// text block
	Text string `json:"text"`

	// thinking block (extended thinking)
	Thinking  string `json:"thinking"`
	Signature string `json:"signature"`

	// tool_use block
	ID    string          `json:"id"`
	Name  string          `json:"name"`
	Input json.RawMessage `json:"input"`

	// tool_result block
	ToolUseID string          `json:"tool_use_id"`
	IsError   bool            `json:"is_error"`
	Content   json.RawMessage `json:"content"` // string OR content-block array

	// fallback block: from/to are {model: "…"} objects (Anthropic fallback protocol).
	From json.RawMessage `json:"from"`
	To   json.RawMessage `json:"to"`
}

type assistantUsage struct {
	InputTokens              int64 `json:"input_tokens"`
	OutputTokens             int64 `json:"output_tokens"`
	CacheCreationInputTokens int64 `json:"cache_creation_input_tokens"`
	CacheReadInputTokens     int64 `json:"cache_read_input_tokens"`
	Iterations               []struct {
		Type  string `json:"type"`
		Model string `json:"model"`
	} `json:"iterations"`
}

func parseAssistant(line []byte) ([]ParsedEvent, error) {
	var msg struct {
		UUID       string   `json:"uuid"`
		Supersedes []string `json:"supersedes"`
		Message    struct {
			Model   string          `json:"model"`
			Content []contentBlock  `json:"content"`
			Usage   *assistantUsage `json:"usage"`
		} `json:"message"`
	}
	if err := json.Unmarshal(line, &msg); err != nil {
		return nil, fmt.Errorf("stream-json assistant: %w", err)
	}
	var out []ParsedEvent
	var contentFrom, contentTo string
	// Per-inference usage drives the composer context meter. result.usage is
	// cumulative billing across the turn and can exceed the window — emit
	// KindUsage here so runners can keep the latest footprint.
	if u := msg.Message.Usage; u != nil {
		partial := Result{
			InputTokens:         u.InputTokens,
			OutputTokens:        u.OutputTokens,
			CacheCreationTokens: u.CacheCreationInputTokens,
			CacheReadTokens:     u.CacheReadInputTokens,
		}
		if contextUsedTokens(&partial) > 0 {
			out = append(out, ParsedEvent{Kind: KindUsage, Result: &partial})
		}
	}
	for _, b := range msg.Message.Content {
		switch b.Type {
		case "text":
			if strings.TrimSpace(b.Text) != "" {
				out = append(out, ParsedEvent{Kind: KindText, Text: b.Text})
			}
		case "thinking":
			// Emit even when the text is REDACTED (empty + signature) — the mirror
			// persists the block with its running token estimate so a reload can
			// still show that (and roughly how much) the agent thought.
			if strings.TrimSpace(b.Thinking) != "" || b.Signature != "" {
				out = append(out, ParsedEvent{Kind: KindThinking, Text: b.Thinking, Signature: b.Signature})
			}
		case "tool_use":
			out = append(out, ParsedEvent{
				Kind:         KindStep,
				Tool:         b.Name,
				ToolUseID:    b.ID,
				ToolInput:    summarizeToolInput(b.Name, b.Input),
				ToolInputRaw: b.Input,
			})
		case "fallback":
			if evs := fallbackEvents(b.From, b.To); len(evs) > 0 {
				contentFrom, contentTo = evs[0].FromModel, evs[0].ToModel
			}
		}
	}
	from, to := fallbackFromIterations(msg.Message.Usage)
	if contentTo != "" {
		from, to = contentFrom, contentTo
	}
	supersedes := trimUUIDs(msg.Supersedes)
	// Overlay message.model only on a superseding replacement frame — every
	// ordinary Claude assistant line also carries message.model, which is not
	// a fallback signal by itself.
	if serving := strings.TrimSpace(msg.Message.Model); serving != "" && len(supersedes) > 0 {
		to = serving
	}
	if to != "" || len(supersedes) > 0 {
		out = insertAfterUsage(out, ParsedEvent{
			Kind: KindFallback, FromModel: from, ToModel: to, RetractedUUIDs: supersedes,
		})
	}
	return stampUUID(out, msg.UUID), nil
}

func insertAfterUsage(out []ParsedEvent, ev ParsedEvent) []ParsedEvent {
	i := 0
	for i < len(out) && out[i].Kind == KindUsage {
		i++
	}
	return append(out[:i], append([]ParsedEvent{ev}, out[i:]...)...)
}

func trimUUIDs(ids []string) []string {
	var out []string
	for _, id := range ids {
		if s := strings.TrimSpace(id); s != "" {
			out = append(out, s)
		}
	}
	return out
}

func fallbackEvents(fromRaw, toRaw json.RawMessage) []ParsedEvent {
	from, to := fallbackModelID(fromRaw), fallbackModelID(toRaw)
	if to == "" {
		return nil
	}
	return []ParsedEvent{{Kind: KindFallback, FromModel: from, ToModel: to}}
}

func fallbackModelID(raw json.RawMessage) string {
	if len(raw) == 0 {
		return ""
	}
	var obj struct {
		Model string `json:"model"`
	}
	if json.Unmarshal(raw, &obj) == nil && strings.TrimSpace(obj.Model) != "" {
		return strings.TrimSpace(obj.Model)
	}
	var s string
	if json.Unmarshal(raw, &s) == nil {
		return strings.TrimSpace(s)
	}
	return ""
}

func fallbackFromIterations(u *assistantUsage) (from, to string) {
	if u == nil {
		return "", ""
	}
	for _, it := range u.Iterations {
		switch it.Type {
		case "fallback_message":
			if m := strings.TrimSpace(it.Model); m != "" {
				to = m
			}
		default:
			if from == "" && strings.TrimSpace(it.Model) != "" {
				from = strings.TrimSpace(it.Model)
			}
		}
	}
	return from, to
}

func stampUUID(evs []ParsedEvent, id string) []ParsedEvent {
	id = strings.TrimSpace(id)
	if id == "" {
		return evs
	}
	for i := range evs {
		if evs[i].UUID == "" {
			evs[i].UUID = id
		}
	}
	return evs
}

// parseRefusalFallback handles system/model_refusal_fallback: Claude Code
// tombstoned the refusing leg and retried on another model. Retracted UUIDs
// name the frames to drop; original/fallback models feed the same notice as a
// content-block hop (snake_case and camelCase both appear on the wire).
func parseRefusalFallback(line []byte) ([]ParsedEvent, error) {
	var msg struct {
		UUID           string   `json:"uuid"`
		OriginalModel  string   `json:"original_model"`
		OriginalCamel  string   `json:"originalModel"`
		FallbackModel  string   `json:"fallback_model"`
		FallbackCamel  string   `json:"fallbackModel"`
		Retracted      []string `json:"retracted_message_uuids"`
		RetractedCamel []string `json:"retractedMessageUuids"`
	}
	if err := json.Unmarshal(line, &msg); err != nil {
		return nil, nil //nolint:nilerr // ignore malformed system lines like other subtypes
	}
	from := strings.TrimSpace(msg.OriginalModel)
	if from == "" {
		from = strings.TrimSpace(msg.OriginalCamel)
	}
	to := strings.TrimSpace(msg.FallbackModel)
	if to == "" {
		to = strings.TrimSpace(msg.FallbackCamel)
	}
	retracted := msg.Retracted
	if len(retracted) == 0 {
		retracted = msg.RetractedCamel
	}
	if to == "" && len(retracted) == 0 {
		return nil, nil
	}
	return []ParsedEvent{{
		Kind:           KindFallback,
		UUID:           strings.TrimSpace(msg.UUID),
		FromModel:      from,
		ToModel:        to,
		RetractedUUIDs: retracted,
	}}, nil
}

func parseUser(line []byte) ([]ParsedEvent, error) {
	// message.content may be a string OR a content-block array (headless ref §4);
	// when it's a string there are no tool_result blocks to surface.
	var msg struct {
		UUID    string `json:"uuid"`
		Message struct {
			Content json.RawMessage `json:"content"`
		} `json:"message"`
	}
	if err := json.Unmarshal(line, &msg); err != nil {
		return nil, fmt.Errorf("stream-json user: %w", err)
	}
	var blocks []contentBlock
	if err := json.Unmarshal(msg.Message.Content, &blocks); err != nil {
		// Content was a plain string (or otherwise not an array) — nothing to emit.
		return nil, nil //nolint:nilerr // a string content carries no tool_result
	}
	var out []ParsedEvent
	for _, b := range blocks {
		if b.Type == "tool_result" {
			text := toolResultText(b.Content)
			if isSubagentLaunchMeta(text) {
				// Background Task acks with agentId/output_file plumbing the
				// model is told not to show. Keep the parent Task card pending
				// until a real result (or turn end) arrives.
				continue
			}
			out = append(out, ParsedEvent{
				Kind:      KindToolResult,
				ToolUseID: b.ToolUseID,
				IsError:   b.IsError,
				Text:      text,
			})
		}
	}
	return stampUUID(out, msg.UUID), nil
}

// nestedParentToolUseID returns the parent Task tool_use id when this stream-json
// line belongs to a multiplexed subagent, or "" for a top-level parent event.
func nestedParentToolUseID(raw json.RawMessage) string {
	if len(raw) == 0 || string(raw) == "null" {
		return ""
	}
	var s string
	if err := json.Unmarshal(raw, &s); err != nil {
		return ""
	}
	return strings.TrimSpace(s)
}

// stampNestedParent tags subagent events with the parent Task id. Nested
// result/init/compact/usage are dropped so they cannot close or speak as the
// parent turn; text, thinking, and tools stay for the Task card.
func stampNestedParent(evs []ParsedEvent, parentID string) []ParsedEvent {
	if parentID == "" || len(evs) == 0 {
		return evs
	}
	out := evs[:0]
	for _, ev := range evs {
		switch ev.Kind {
		case KindStep, KindToolResult, KindText, KindThinking, KindThinkingTokens, KindTextDelta, KindThinkingDelta:
			ev.ParentToolUseID = parentID
			out = append(out, ev)
		}
	}
	return out
}

// isSubagentLaunchMeta reports Claude Code's background-Task ack. Anchor to the
// ack prefix so a Read/Grep of a file that merely mentions the phrase (this
// source, frontend helpers) is not treated as a launch ack.
func isSubagentLaunchMeta(s string) bool {
	return strings.HasPrefix(strings.TrimSpace(s), "Async agent launched successfully")
}

// toolResultText normalizes a tool_result content payload (string or text blocks)
// into a single plain-text string for the Activity expand view.
func toolResultText(raw json.RawMessage) string {
	if len(raw) == 0 {
		return ""
	}
	var s string
	if err := json.Unmarshal(raw, &s); err == nil {
		return s
	}
	var blocks []struct {
		Type string `json:"type"`
		Text string `json:"text"`
	}
	if err := json.Unmarshal(raw, &blocks); err == nil {
		var b strings.Builder
		for _, bl := range blocks {
			if strings.TrimSpace(bl.Text) == "" {
				continue
			}
			if b.Len() > 0 {
				b.WriteByte('\n')
			}
			b.WriteString(bl.Text)
		}
		return b.String()
	}
	return strings.TrimSpace(string(raw))
}

func parseResult(line []byte) ([]ParsedEvent, error) {
	// Branch on subtype BEFORE reading `result`: error variants have no result
	// field (headless ref §3). All other fields are present on both variants.
	var msg struct {
		Subtype    string `json:"subtype"`
		SessionID  string `json:"session_id"`
		IsError    bool   `json:"is_error"`
		NumTurns   int    `json:"num_turns"`
		DurationMS int    `json:"duration_ms"`
		Result     string `json:"result"`
		Error      string `json:"error"`
		Usage      *struct {
			InputTokens              int64 `json:"input_tokens"`
			OutputTokens             int64 `json:"output_tokens"`
			CacheCreationInputTokens int64 `json:"cache_creation_input_tokens"`
			CacheReadInputTokens     int64 `json:"cache_read_input_tokens"`
		} `json:"usage"`
		// modelUsage is keyed by model id; camelCase fields (unlike usage).
		ModelUsage map[string]struct {
			ContextWindow            int64 `json:"contextWindow"`
			InputTokens              int64 `json:"inputTokens"`
			OutputTokens             int64 `json:"outputTokens"`
			CacheCreationInputTokens int64 `json:"cacheCreationInputTokens"`
			CacheReadInputTokens     int64 `json:"cacheReadInputTokens"`
		} `json:"modelUsage"`
		StopReason      string          `json:"stop_reason"`
		TerminalReason  string          `json:"terminal_reason"`
		QueuedTurnCount int             `json:"queued_turn_count"`
		DeferredToolUse json.RawMessage `json:"deferred_tool_use"`
		Origin          struct {
			Kind string `json:"kind"`
		} `json:"origin"`
	}
	if err := json.Unmarshal(line, &msg); err != nil {
		return nil, fmt.Errorf("stream-json result: %w", err)
	}
	if resultIsNonTerminal(msg.StopReason, msg.TerminalReason, msg.QueuedTurnCount, msg.DeferredToolUse) {
		return nil, nil
	}
	// A --resume replays a background task killed with the previous process
	// as a model-less result before the prompt's own turn. Treating it as
	// terminal closes the turn in ~30ms and hides the real reply until resync.
	if msg.Origin.Kind == "task-notification" && msg.NumTurns == 0 && !msg.IsError {
		return nil, nil
	}
	r := Result{
		SessionID:  msg.SessionID,
		Subtype:    msg.Subtype,
		IsError:    msg.IsError,
		NumTurns:   msg.NumTurns,
		DurationMS: msg.DurationMS,
	}
	if msg.Usage != nil {
		r.InputTokens = msg.Usage.InputTokens
		r.OutputTokens = msg.Usage.OutputTokens
		r.CacheCreationTokens = msg.Usage.CacheCreationInputTokens
		r.CacheReadTokens = msg.Usage.CacheReadInputTokens
	}
	for _, u := range msg.ModelUsage {
		if u.ContextWindow > r.ContextWindow {
			r.ContextWindow = u.ContextWindow
		}
	}
	if msg.Subtype == "success" {
		r.Text = msg.Result
	} else {
		// Defensive: any non-success subtype is treated as an error even if the
		// is_error flag is absent, so the engine never advances on a failed turn.
		r.IsError = true
		// Keep whatever message the error variant DID carry. The reference says error
		// results have no `result` field, but some carry `error` (and some builds do fill
		// `result`) — without it a spend-limit failure reaches the user as a bare subtype
		// with no way to classify it as transient (D56).
		if msg.Error != "" {
			r.Text = msg.Error
		} else if msg.Result != "" {
			r.Text = msg.Result
		}
	}
	rc := r
	return []ParsedEvent{{Kind: KindResult, Text: r.Text, IsError: r.IsError, Result: &rc}}, nil
}

// resultIsNonTerminal reports Claude 2.1+ mid-run type:result frames. Those must
// not become KindResult — they would publish session.turn_end and close
// InjectPlain stdin while tools or queued user turns are still live.
func resultIsNonTerminal(stopReason, terminalReason string, queued int, deferred json.RawMessage) bool {
	if queued > 0 {
		return true
	}
	if stopReason == "tool_deferred" || terminalReason == "tool_deferred" || terminalReason == "background_requested" {
		return true
	}
	return len(deferred) > 0 && string(deferred) != "null"
}

// feedInputLimit is generous so Activity expand can show the real command /
// summary. Collapsed rows clip in the UI; do not bake "…" into stored feed text.
const feedInputLimit = 8000

// chatDetailLimit bounds a chat tool-call's full input JSON / result text (the
// expanded detail card). Larger than feedInputLimit because an Edit's old/new
// strings or a diff-sized tool result are exactly what the card shows.
const chatDetailLimit = 16000

// chatToolInput bounds a tool call's input JSON for the live/history detail card.
// A byte-slice truncate mid-JSON breaks the frontend parser (parsedInput returns
// null and CreatePlan renders nothing), so CreatePlan is re-serialized with a
// cleanly truncated plan body up to planMaxPersistChars — the same bound used
// for DefinitionOfDone — instead of slicing the envelope.
func chatToolInput(tool string, input json.RawMessage) string {
	if len(input) == 0 {
		return ""
	}
	if isCreatePlanTool(tool) {
		body := createPlanBody(input)
		if body == "" {
			return truncate(string(input), chatDetailLimit)
		}
		body = truncate(body, planMaxPersistChars)
		b, err := json.Marshal(map[string]string{"plan": body})
		if err != nil {
			return truncate(string(input), chatDetailLimit)
		}
		return string(b)
	}
	if isTaskTool(tool) {
		return chatTaskInput(input)
	}
	return truncate(string(input), chatDetailLimit)
}

func isTaskTool(name string) bool {
	switch name {
	case "Task", "task", "Agent":
		return true
	default:
		return false
	}
}

// chatTaskInput keeps a parseable Task envelope so the card can show the prompt
// as prose. Byte-slicing a long Explore brief mid-JSON made parsedInput fail
// and the card dumped the truncated {description,prompt,subagent_type} blob.
func chatTaskInput(input json.RawMessage) string {
	if len(input) <= chatDetailLimit && json.Valid(input) {
		return string(input)
	}
	var in map[string]any
	if err := json.Unmarshal(input, &in); err != nil {
		return truncate(string(input), chatDetailLimit)
	}
	if p, ok := in["prompt"].(string); ok && p != "" {
		in["prompt"] = truncate(p, planMaxPersistChars)
	}
	b, err := json.Marshal(in)
	if err != nil {
		return truncate(string(input), chatDetailLimit)
	}
	return string(b)
}

func isListFindingsTool(name string) bool {
	return name == "list_findings" || strings.HasSuffix(name, "__list_findings")
}

func isListArtifactsTool(name string) bool {
	return name == "list_artifacts" || strings.HasSuffix(name, "__list_artifacts")
}

func isListWorkspaceBriefsTool(name string) bool {
	return name == "list_workspace_briefs" || strings.HasSuffix(name, "__list_workspace_briefs")
}

func isGetWorkspaceContextTool(name string) bool {
	return name == "get_workspace_context" || strings.HasSuffix(name, "__get_workspace_context")
}

func isPresentWorkspaceTool(name string) bool {
	return name == "present_workspace" || strings.HasSuffix(name, "__present_workspace")
}

// chatToolResult bounds a tool result for the live/history detail card.
// list_findings / list_artifacts / briefing tools return JSON the frontend
// accordion parses; a mid-JSON byte slice made the card dump the minified blob.
// Re-serialize a compact, still-parseable envelope instead of slicing through a string.
func chatToolResult(tool, result string) string {
	if result == "" {
		return result
	}
	if isTaskTool(tool) && isSubagentLaunchMeta(result) {
		return ""
	}
	if isListFindingsTool(tool) {
		if compact, ok := compactListFindingsResult(result); ok {
			return compact
		}
	}
	if isListArtifactsTool(tool) {
		if compact, ok := compactListArtifactsResult(result); ok {
			return compact
		}
	}
	if isListWorkspaceBriefsTool(tool) {
		if compact, ok := compactListWorkspaceBriefsResult(result); ok {
			return compact
		}
	}
	if isGetWorkspaceContextTool(tool) {
		if compact, ok := compactWorkspaceContextResult(result); ok {
			return compact
		}
	}
	if isPresentWorkspaceTool(tool) {
		if compact, ok := compactPresentWorkspaceResult(result); ok {
			return compact
		}
	}
	return truncate(result, chatDetailLimit)
}

type listedFindingNote struct {
	ID        string `json:"id,omitempty"`
	Author    string `json:"author,omitempty"`
	Body      string `json:"body"`
	CreatedAt string `json:"createdAt,omitempty"`
}

type listedFindingRow struct {
	ID           string              `json:"id"`
	File         string              `json:"file"`
	Line         int                 `json:"line,omitempty"`
	EndLine      int                 `json:"endLine,omitempty"`
	Severity     string              `json:"severity"`
	Message      string              `json:"message"`
	Summary      string              `json:"summary,omitempty"`
	Status       string              `json:"status"`
	StatusReason string              `json:"statusReason,omitempty"`
	UpdatedBy    string              `json:"updatedBy,omitempty"`
	Notes        []listedFindingNote `json:"notes,omitempty"`
}

func compactListFindingsResult(result string) (string, bool) {
	raw := []byte(result)
	if len(raw) <= chatDetailLimit && json.Valid(raw) {
		return result, true
	}
	var in struct {
		Status   string            `json:"status"`
		Count    int               `json:"count"`
		Findings []json.RawMessage `json:"findings"`
	}
	if err := json.Unmarshal(raw, &in); err != nil {
		return "", false
	}
	// Missing findings (nil) means this is not the list_findings envelope —
	// an MCP wrapper or error object. Empty [] is a real empty list.
	if in.Findings == nil {
		return "", false
	}
	if in.Count == 0 {
		in.Count = len(in.Findings)
	}
	rows := make([]listedFindingRow, 0, len(in.Findings))
	for _, f := range in.Findings {
		var row listedFindingRow
		if json.Unmarshal(f, &row) != nil {
			continue
		}
		rows = append(rows, row)
	}
	// 800 keeps notes + longer prose; 120 drops notes and tightens text.
	// Both steps truncate every variable field so the ladder is monotonic.
	msgBudget := 800
	for {
		out := struct {
			Status   string             `json:"status"`
			Count    int                `json:"count"`
			Findings []listedFindingRow `json:"findings"`
		}{
			Status:   in.Status,
			Count:    in.Count,
			Findings: shrinkListedFindingMessages(rows, msgBudget),
		}
		b, err := json.Marshal(out)
		if err != nil {
			return "", false
		}
		if len(b) <= chatDetailLimit {
			return string(b), true
		}
		switch {
		case msgBudget > 120:
			msgBudget = 120
		case len(rows) > 0:
			rows = rows[:len(rows)-1]
		default:
			return string(b), true
		}
	}
}

func shrinkListedFindingMessages(rows []listedFindingRow, budget int) []listedFindingRow {
	out := make([]listedFindingRow, len(rows))
	keepNotes := budget > 120
	for i := range rows {
		row := rows[i]
		if keepNotes && len(row.Notes) > 0 {
			notes := make([]listedFindingNote, len(row.Notes))
			copy(notes, row.Notes)
			for j := range notes {
				if len(notes[j].Body) > budget {
					notes[j].Body = truncate(notes[j].Body, budget)
				}
			}
			row.Notes = notes
		} else {
			row.Notes = nil
		}
		if len(row.Message) > budget {
			row.Message = truncate(row.Message, budget)
		}
		if len(row.Summary) > budget {
			row.Summary = truncate(row.Summary, budget)
		}
		if len(row.StatusReason) > budget {
			row.StatusReason = truncate(row.StatusReason, budget)
		}
		out[i] = row
	}
	return out
}

type listedArtifactRow struct {
	ID          string `json:"id"`
	WorkspaceID string `json:"workspaceId,omitempty"`
	Title       string `json:"title"`
	Kind        string `json:"kind"`
	Path        string `json:"path"`
	CreatedAt   string `json:"createdAt,omitempty"`
	CreatedBy   string `json:"createdBy,omitempty"`
}

func compactListArtifactsResult(result string) (string, bool) {
	raw := []byte(result)
	if len(raw) <= chatDetailLimit && json.Valid(raw) {
		return result, true
	}
	var in struct {
		Count     int               `json:"count"`
		Artifacts []json.RawMessage `json:"artifacts"`
	}
	if err := json.Unmarshal(raw, &in); err != nil {
		return "", false
	}
	// Missing artifacts (nil) means this is not the list_artifacts envelope —
	// an MCP wrapper or error object. Empty [] is a real empty list.
	if in.Artifacts == nil {
		return "", false
	}
	if in.Count == 0 {
		in.Count = len(in.Artifacts)
	}
	rows := make([]listedArtifactRow, 0, len(in.Artifacts))
	for _, a := range in.Artifacts {
		var row listedArtifactRow
		if json.Unmarshal(a, &row) != nil {
			continue
		}
		rows = append(rows, row)
	}
	// 800 keeps a readable title; 120 is the last shrink before dropping rows
	// so one oversized title cannot empty the list while count stays N.
	titleBudget := 800
	for {
		out := struct {
			Count     int                 `json:"count"`
			Artifacts []listedArtifactRow `json:"artifacts"`
		}{
			Count:     in.Count,
			Artifacts: shrinkListedArtifactTitles(rows, titleBudget),
		}
		b, err := json.Marshal(out)
		if err != nil {
			return "", false
		}
		if len(b) <= chatDetailLimit {
			return string(b), true
		}
		switch {
		case titleBudget > 120:
			titleBudget = 120
		case len(rows) > 0:
			rows = rows[:len(rows)-1]
		default:
			return string(b), true
		}
	}
}

func shrinkListedArtifactTitles(rows []listedArtifactRow, budget int) []listedArtifactRow {
	out := make([]listedArtifactRow, len(rows))
	for i := range rows {
		row := rows[i]
		if len(row.Title) > budget {
			row.Title = truncate(row.Title, budget)
		}
		out[i] = row
	}
	return out
}

func compactListWorkspaceBriefsResult(result string) (string, bool) {
	raw := []byte(result)
	if len(raw) <= chatDetailLimit && json.Valid(raw) {
		return result, true
	}
	var in domain.BriefingSnapshot
	if err := json.Unmarshal(raw, &in); err != nil {
		return "", false
	}
	// Missing workspaces (nil) is not the list envelope — an MCP wrapper or error.
	if in.Workspaces == nil {
		return "", false
	}
	rows := stripBriefsForChat(in.Workspaces)
	for {
		out := domain.BriefingSnapshot{GeneratedAt: in.GeneratedAt, Counts: in.Counts, Workspaces: rows}
		b, err := json.Marshal(out)
		if err != nil {
			return "", false
		}
		if len(b) <= chatDetailLimit {
			return string(b), true
		}
		if len(rows) == 0 {
			return string(b), true
		}
		rows = rows[:len(rows)-1]
	}
}

func stripBriefsForChat(rows []domain.WorkspaceBrief) []domain.WorkspaceBrief {
	out := make([]domain.WorkspaceBrief, len(rows))
	for i, b := range rows {
		b.PlanSummary = ""
		b.LastSummary = ""
		b.ActiveAgentTask = ""
		b.Secondary = nil
		out[i] = b
	}
	return out
}

func compactWorkspaceContextResult(result string) (string, bool) {
	raw := []byte(result)
	if len(raw) <= chatDetailLimit && json.Valid(raw) {
		return result, true
	}
	var in domain.WorkspaceContext
	if err := json.Unmarshal(raw, &in); err != nil {
		return "", false
	}
	if strings.TrimSpace(in.Brief.WorkspaceID) == "" && strings.TrimSpace(in.Brief.Title) == "" {
		return "", false
	}
	in.Conversation = nil
	if b, ok := marshalIfUnder(in); ok {
		return b, true
	}
	for i := range in.PlanSteps {
		in.PlanSteps[i].Note = ""
	}
	if b, ok := marshalIfUnder(in); ok {
		return b, true
	}
	in.Brief.PlanSummary = ""
	in.Brief.LastSummary = ""
	in.Brief.ActiveAgentTask = ""
	in.Brief.Secondary = nil
	if b, ok := marshalIfUnder(in); ok {
		return b, true
	}
	in.PlanSteps = nil
	in.Agents = nil
	in.RecentChats = nil
	in.OpenAsks = nil
	in.PR = nil
	if b, ok := marshalIfUnder(in); ok {
		return b, true
	}
	b, err := json.Marshal(in)
	if err != nil {
		return "", false
	}
	return string(b), true
}

func compactPresentWorkspaceResult(result string) (string, bool) {
	raw := []byte(result)
	if len(raw) <= chatDetailLimit && json.Valid(raw) {
		return result, true
	}
	var in domain.PresentWorkspaceResult
	if err := json.Unmarshal(raw, &in); err != nil {
		return "", false
	}
	if !in.OK && in.Target == "" && in.Error == "" {
		return "", false
	}
	if in.Brief != nil {
		in.Brief.PlanSummary = ""
		in.Brief.LastSummary = ""
		in.Brief.ActiveAgentTask = ""
		in.Brief.Secondary = nil
	}
	if b, ok := marshalIfUnder(in); ok {
		return b, true
	}
	b, err := json.Marshal(in)
	if err != nil {
		return "", false
	}
	return string(b), true
}

func marshalIfUnder(v any) (string, bool) {
	b, err := json.Marshal(v)
	if err != nil || len(b) > chatDetailLimit {
		return "", false
	}
	return string(b), true
}

// toolDescription extracts the agent's own one-line description of a tool call
// (Claude Code sets input.description on Bash/Task calls). Empty when absent.
//
// TaskCreate / TaskUpdate also have a "description" field, but that is the task
// BODY — not a row title. Feeding it into ChatItem.Description made the
// transcript header a truncated paragraph plus a JSON dump of the same args.
func toolDescription(tool string, raw json.RawMessage) string {
	if len(raw) == 0 {
		return ""
	}
	switch tool {
	case "TaskCreate", "TaskUpdate", "TaskGet", "TaskList":
		return ""
	}
	var in struct {
		Description string `json:"description"`
	}
	if json.Unmarshal(raw, &in) != nil {
		return ""
	}
	return strings.TrimSpace(in.Description)
}

// summarizeTodos renders a plan update as "2/5 · <the in-progress item>", which
// is what a collapsed row can usefully show.
func summarizeTodos(in map[string]any) string {
	items, _ := in["todos"].([]any)
	if len(items) == 0 {
		return ""
	}
	done, active := 0, ""
	for _, it := range items {
		m, _ := it.(map[string]any)
		switch m["status"] {
		case "completed":
			done++
		case "in_progress":
			if s, ok := m["content"].(string); ok && active == "" {
				active = s
			}
		}
	}
	head := fmt.Sprintf("%d/%d", done, len(items))
	if active != "" {
		return truncate(head+" · "+active, feedInputLimit)
	}
	return head
}

// summarizeToolInput renders a human-readable tool input for the activity feed /
// agent step stream. It pulls the salient field per tool (file_path, command,
// summary) so MCP rows are not raw truncated JSON.
func summarizeToolInput(tool string, raw json.RawMessage) string {
	if len(raw) == 0 {
		return ""
	}
	var in map[string]any
	if err := json.Unmarshal(raw, &in); err != nil {
		return truncate(strings.TrimSpace(string(raw)), feedInputLimit)
	}
	str := func(k string) string {
		if v, ok := in[k].(string); ok {
			return v
		}
		return ""
	}
	switch tool {
	case "Read", "Edit", "Write", "Delete":
		// Delete is Cursor + Codex (Claude Code has no builtin; it shells out).
		// Without this the feed/header dump truncated {"file_path":…} JSON.
		if p := str("file_path"); p != "" {
			return p
		}
	case "Bash":
		if c := str("command"); c != "" {
			return truncate(c, feedInputLimit)
		}
	case "Glob":
		if p := str("pattern"); p != "" {
			return p
		}
		if p := str("glob_pattern"); p != "" {
			return p
		}
	case "Grep":
		if p := str("pattern"); p != "" {
			if path := str("path"); path != "" {
				return fmt.Sprintf("%q in %s", p, path)
			}
			return fmt.Sprintf("%q", p)
		}
	case "WebFetch":
		// Chat card shows a URL chip; feed/header must not dump {"url","prompt"} JSON.
		if u := str("url"); u != "" {
			return truncate(u, feedInputLimit)
		}
	case "WebSearch":
		if q := str("query"); q != "" {
			return truncate(q, feedInputLimit)
		}
		if q := str("search_term"); q != "" {
			return truncate(q, feedInputLimit)
		}
	case "ToolSearch":
		if q := str("query"); q != "" {
			return q
		}
	case "TodoWrite":
		// A raw todo array is unreadable in a one-line row; show progress plus
		// whatever is currently in flight.
		return summarizeTodos(in)
	case "CreatePlan":
		// Cursor plan-mode brief — first line (or a short prefix) for the feed row.
		if p := str("plan"); p != "" {
			if i := strings.IndexByte(p, '\n'); i >= 0 {
				p = p[:i]
			}
			return truncate(strings.TrimSpace(p), feedInputLimit)
		}
	case "Skill":
		if s := str("skill"); s != "" {
			if a := str("args"); a != "" {
				return s + " " + truncate(a, 120)
			}
			return s
		}
	case "Task":
		// Description is the row title (toolDescription); meta should not
		// repeat it or dump the full prompt JSON. Prefer the subagent type,
		// then a short prompt preview.
		if t := str("subagent_type"); t != "" {
			return t
		}
		if p := str("prompt"); p != "" {
			return truncate(p, 120)
		}
	case "SendMessage":
		// Follow-up to a background subagent. Never dump {to,prompt} JSON or
		// the internal agent id.
		if d := str("description"); d != "" {
			return truncate(d, feedInputLimit)
		}
		if s := str("summary"); s != "" {
			return truncate(s, feedInputLimit)
		}
		if p := str("prompt"); p != "" {
			return truncate(p, 120)
		}
		if p := str("message"); p != "" {
			return truncate(p, 120)
		}
	case "TaskCreate":
		// Subject is the one-line title; description is the body (card, not feed).
		if s := str("subject"); s != "" {
			return truncate(s, feedInputLimit)
		}
	case "TaskUpdate":
		id := str("taskId")
		if id == "" {
			id = str("task_id")
		}
		st := str("status")
		sub := str("subject")
		parts := make([]string, 0, 2)
		switch {
		case id != "" && st != "":
			parts = append(parts, "#"+strings.TrimPrefix(id, "#")+" → "+st)
		case id != "":
			parts = append(parts, "#"+strings.TrimPrefix(id, "#"))
		case st != "":
			parts = append(parts, st)
		}
		if sub != "" {
			parts = append(parts, sub)
		}
		if len(parts) > 0 {
			return truncate(strings.Join(parts, " · "), feedInputLimit)
		}
	case "TaskGet":
		id := str("taskId")
		if id == "" {
			id = str("task_id")
		}
		if id != "" {
			return "#" + strings.TrimPrefix(id, "#")
		}
	case "TaskList":
		// Empty args `{}` must not become a JSON stub in the feed row.
		return ""
	}
	if strings.HasPrefix(tool, "mcp__recur__") {
		switch {
		case strings.HasSuffix(tool, "__done"), strings.HasSuffix(tool, "__plan"):
			if s := str("summary"); s != "" {
				return truncate(s, feedInputLimit)
			}
		case strings.HasSuffix(tool, "__submit_plan"):
			// Chat card shows the checklist; feed meta is the summary only.
			if s := str("summary"); s != "" {
				return truncate(s, feedInputLimit)
			}
		case strings.HasSuffix(tool, "__edit_plan"):
			// Definition-of-done rewrite — first line, never {"plan":…} JSON.
			if p := str("plan"); p != "" {
				if i := strings.IndexByte(p, '\n'); i >= 0 {
					p = p[:i]
				}
				return truncate(strings.TrimSpace(p), feedInputLimit)
			}
		case strings.HasSuffix(tool, "__update_plan_step"):
			// Compact "S2 → done" — never raw {step_id,status} JSON in the row.
			id := str("step_id")
			st := str("status")
			note := str("note")
			parts := make([]string, 0, 2)
			switch {
			case id != "" && st != "":
				parts = append(parts, id+" → "+st)
			case id != "":
				parts = append(parts, id)
			case st != "":
				parts = append(parts, st)
			}
			if note != "" {
				parts = append(parts, note)
			}
			if len(parts) > 0 {
				return truncate(strings.Join(parts, " · "), feedInputLimit)
			}
		case strings.HasSuffix(tool, "__report_finding"):
			msg := str("message")
			file := str("file")
			sev := str("severity")
			start, end := 0, 0
			if n, ok := in["line"].(float64); ok && n > 0 {
				start = int(n)
			}
			if n, ok := in["end_line"].(float64); ok && n > 0 {
				end = int(n)
			}
			loc := domain.FindingLoc(file, start, end)
			parts := make([]string, 0, 3)
			if sev != "" && sev != "info" {
				parts = append(parts, sev)
			}
			if loc != "" {
				parts = append(parts, loc)
			}
			if msg != "" {
				parts = append(parts, msg)
			}
			if len(parts) > 0 {
				return truncate(strings.Join(parts, " · "), feedInputLimit)
			}
		case strings.HasSuffix(tool, "__list_findings"):
			if s := str("status"); s != "" && s != "open" {
				return s
			}
			return "open"
		case strings.HasSuffix(tool, "__list_skills"):
			return "skills"
		case strings.HasSuffix(tool, "__list_terminals"):
			return "terminals"
		case strings.HasSuffix(tool, "__read_terminal"):
			if n := str("id"); n != "" {
				return truncate(n, feedInputLimit)
			}
			return "terminal"
		case strings.HasSuffix(tool, "__read_artifact"):
			if n := str("id"); n != "" {
				return truncate(n, feedInputLimit)
			}
			return "artifact"
		case strings.HasSuffix(tool, "__activate_skill"):
			if n := str("name"); n != "" {
				return truncate("/"+n, feedInputLimit)
			}
		case strings.HasSuffix(tool, "__update_finding"):
			id := str("finding_id")
			st := str("status")
			if id != "" && st != "" {
				return truncate(id+" → "+st, feedInputLimit)
			}
			if id != "" {
				return id
			}
		case strings.HasSuffix(tool, "__report_result"):
			if s := str("summary"); s != "" {
				return truncate(s, feedInputLimit)
			}
		case strings.HasSuffix(tool, "__escalate"), strings.HasSuffix(tool, "__ask_user"):
			if q := str("question"); q != "" {
				return truncate(q, feedInputLimit)
			}
		case strings.HasSuffix(tool, "__switch_mode"):
			if m := str("mode"); m != "" {
				return m
			}
		case strings.HasSuffix(tool, "__open_pr"):
			if t := str("title"); t != "" {
				if d, ok := in["draft"].(bool); ok && d {
					return truncate("draft · "+t, feedInputLimit)
				}
				return truncate(t, feedInputLimit)
			}
		case strings.HasSuffix(tool, "__attach_evidence"):
			if s := str("step"); s != "" {
				return truncate(s, feedInputLimit)
			}
		}
		if s := str("summary"); s != "" {
			return truncate(s, feedInputLimit)
		}
	}
	if isLinearMCPTool(tool) {
		for _, k := range []string{"query", "id", "issue", "team", "name", "title", "urlOrId", "project", "user"} {
			if s := str(k); s != "" {
				return truncate(s, feedInputLimit)
			}
		}
		return ""
	}
	// Unrecognized: keep compact JSON, but don't clip to an unreadable stub.
	return truncate(strings.TrimSpace(string(raw)), feedInputLimit)
}

func isLinearMCPTool(tool string) bool {
	if !strings.HasPrefix(tool, "mcp__") {
		return false
	}
	rest := strings.TrimPrefix(tool, "mcp__")
	server, _, ok := strings.Cut(rest, "__")
	if !ok {
		return false
	}
	s := strings.ToLower(server)
	return s == "linear" || strings.Contains(s, "linear")
}

// truncate cuts s to at most n bytes (plus an ellipsis), rune-safe: a plan or
// other prose containing multi-byte UTF-8 (em-dashes, curly quotes, arrows —
// exactly what a thorough plan-mode document tends to contain) must not be cut
// mid-rune, or the result is invalid UTF-8 wherever it is later stored/rendered
// (e.g. DefinitionOfDone, feed text). We back up from the n-1
// byte cutoff to the start of the rune straddling it, if any.
func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	cut := n - 1
	if cut < 0 {
		cut = 0
	}
	for cut > 0 && !utf8.RuneStart(s[cut]) {
		cut--
	}
	return s[:cut] + "…"
}

// hardTruncateRunes caps s to at most n runes with no ellipsis (stored field limits).
func hardTruncateRunes(s string, n int) string {
	if n <= 0 {
		return ""
	}
	i := 0
	for pos := range s {
		if i == n {
			return s[:pos]
		}
		i++
	}
	return s
}

// trimSpace trims ASCII whitespace from a byte slice without allocating.
func trimSpace(b []byte) []byte {
	for len(b) > 0 && (b[0] == ' ' || b[0] == '\t' || b[0] == '\r' || b[0] == '\n') {
		b = b[1:]
	}
	for len(b) > 0 {
		c := b[len(b)-1]
		if c == ' ' || c == '\t' || c == '\r' || c == '\n' {
			b = b[:len(b)-1]
			continue
		}
		break
	}
	return b
}
