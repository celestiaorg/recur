package engine

import (
	"strings"
	"testing"

	"recur/internal/domain"
)

// TestParseStreamLineFixtures feeds canned NDJSON lines matching the headless
// reference §3 schemas and asserts the parsed events/Result. No real claude.
func TestParseStreamLineFixtures(t *testing.T) {
	t.Run("system/init", func(t *testing.T) {
		line := []byte(`{"type":"system","subtype":"init","session_id":"sess-abc","uuid":"u1","model":"claude-sonnet-4-6","tools":["Read","Edit"],"mcp_servers":[{"name":"recur","status":"connected"}],"cwd":"/tmp/wt","permissionMode":"dontAsk"}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse init: %v", err)
		}
		if len(evs) != 1 || evs[0].Kind != KindInit {
			t.Fatalf("want 1 init event, got %+v", evs)
		}
		if evs[0].SessionID != "sess-abc" || evs[0].Model != "claude-sonnet-4-6" {
			t.Errorf("init fields wrong: %+v", evs[0])
		}
		if len(evs[0].MCPServers) != 1 || evs[0].MCPServers[0].Status != "connected" {
			t.Errorf("mcp_servers not parsed: %+v", evs[0].MCPServers)
		}
	})

	t.Run("assistant tool_use (Edit) + text", func(t *testing.T) {
		line := []byte(`{"type":"assistant","uuid":"u2","session_id":"sess-abc","message":{"role":"assistant","content":[{"type":"text","text":"I'll edit the file."},{"type":"tool_use","id":"toolu_1","name":"Edit","input":{"file_path":"src/app.go","old_string":"a","new_string":"b"}}]}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse assistant: %v", err)
		}
		if len(evs) != 2 {
			t.Fatalf("want 2 events (text+step), got %d: %+v", len(evs), evs)
		}
		if evs[0].Kind != KindText || evs[0].Text != "I'll edit the file." {
			t.Errorf("text event wrong: %+v", evs[0])
		}
		if evs[1].Kind != KindStep || evs[1].Tool != "Edit" || evs[1].ToolUseID != "toolu_1" {
			t.Errorf("step event wrong: %+v", evs[1])
		}
		if evs[1].ToolInput != "src/app.go" {
			t.Errorf("Edit input summary: got %q, want file path", evs[1].ToolInput)
		}
	})

	t.Run("assistant thinking block", func(t *testing.T) {
		line := []byte(`{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"weigh options","signature":"sig-1"},{"type":"text","text":"Here's the plan."}]}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse assistant thinking: %v", err)
		}
		if len(evs) != 2 {
			t.Fatalf("want 2 events (thinking+text), got %d: %+v", len(evs), evs)
		}
		if evs[0].Kind != KindThinking || evs[0].Text != "weigh options" || evs[0].Signature != "sig-1" {
			t.Errorf("thinking event wrong: %+v", evs[0])
		}
		if evs[1].Kind != KindText {
			t.Errorf("text event should follow thinking: %+v", evs[1])
		}
	})

	t.Run("redacted thinking (headless): empty text + signature still emits", func(t *testing.T) {
		// Real shape from claude 2.1.198 without --thinking-display: text withheld.
		line := []byte(`{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"","signature":"EtoCC"}]}}`)
		evs, _ := parseStreamLine(line)
		if len(evs) != 1 || evs[0].Kind != KindThinking || evs[0].Text != "" || evs[0].Signature != "EtoCC" {
			t.Errorf("redacted thinking block wrong: %+v", evs)
		}
	})

	t.Run("system thinking_tokens -> KindThinkingTokens", func(t *testing.T) {
		line := []byte(`{"type":"system","subtype":"thinking_tokens","estimated_tokens":88,"estimated_tokens_delta":38}`)
		evs, _ := parseStreamLine(line)
		if len(evs) != 1 || evs[0].Kind != KindThinkingTokens || evs[0].Tokens != 88 {
			t.Errorf("thinking_tokens wrong: %+v", evs)
		}
		if evs[0].UUID != "" {
			t.Errorf("thinking_tokens UUID = %q, want empty when omitted", evs[0].UUID)
		}
	})

	t.Run("system thinking_tokens drops envelope uuid", func(t *testing.T) {
		// Claude Code mints uuid:l() per thinking_tokens line — not the
		// assistant frame id used for fallback retraction.
		line := []byte(`{"type":"system","subtype":"thinking_tokens","uuid":"evt-tick","estimated_tokens":88}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parseStreamLine(thinking_tokens uuid): %v", err)
		}
		if len(evs) != 1 || evs[0].Kind != KindThinkingTokens || evs[0].Tokens != 88 || evs[0].UUID != "" {
			t.Errorf("parseStreamLine(thinking_tokens uuid) = %+v, want tokens=88 uuid empty", evs)
		}
	})

	t.Run("empty thinking_delta with estimate -> tokens event", func(t *testing.T) {
		line := []byte(`{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"","estimated_tokens":50}}}`)
		evs, _ := parseStreamLine(line)
		if len(evs) != 1 || evs[0].Kind != KindThinkingTokens || evs[0].Tokens != 50 {
			t.Errorf("empty thinking_delta wrong: %+v", evs)
		}
		// estimated_tokens:null (the --thinking-display summarized shape) parses as text.
		line2 := []byte(`{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"This","estimated_tokens":null}}}`)
		evs2, err := parseStreamLine(line2)
		if err != nil || len(evs2) != 1 || evs2[0].Kind != KindThinkingDelta || evs2[0].Text != "This" {
			t.Errorf("summarized thinking_delta wrong: %+v (err %v)", evs2, err)
		}
	})

	t.Run("stream_event thinking_delta", func(t *testing.T) {
		line := []byte(`{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse thinking_delta: %v", err)
		}
		if len(evs) != 1 || evs[0].Kind != KindThinkingDelta || evs[0].Text != "hmm" {
			t.Errorf("thinking_delta wrong: %+v", evs)
		}
	})

	t.Run("stream_event deltas drop envelope uuid", func(t *testing.T) {
		think := []byte(`{"type":"stream_event","uuid":"evt-1","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Act"}}}`)
		evs, err := parseStreamLine(think)
		if err != nil {
			t.Fatalf("parse thinking_delta uuid: %v", err)
		}
		if len(evs) != 1 || evs[0].Kind != KindThinkingDelta || evs[0].Text != "Act" || evs[0].UUID != "" {
			t.Errorf("thinking_delta must not carry envelope uuid: %+v", evs)
		}
		text := []byte(`{"type":"stream_event","uuid":"evt-2","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}}`)
		evs, err = parseStreamLine(text)
		if err != nil {
			t.Fatalf("parse text_delta uuid: %v", err)
		}
		if len(evs) != 1 || evs[0].Kind != KindTextDelta || evs[0].Text != "Hi" || evs[0].UUID != "" {
			t.Errorf("text_delta must not carry envelope uuid: %+v", evs)
		}
	})

	t.Run("assistant tool_use (Bash) summary", func(t *testing.T) {
		line := []byte(`{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"go test ./..."}}]}}`)
		evs, _ := parseStreamLine(line)
		if len(evs) != 1 || evs[0].Tool != "Bash" || evs[0].ToolInput != "go test ./..." {
			t.Errorf("bash step wrong: %+v", evs)
		}
	})

	t.Run("user tool_result (array content)", func(t *testing.T) {
		line := []byte(`{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"ok"}],"is_error":false}]}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse user: %v", err)
		}
		if len(evs) != 1 || evs[0].Kind != KindToolResult || evs[0].ToolUseID != "toolu_1" || evs[0].IsError {
			t.Errorf("tool_result event wrong: %+v", evs)
		}
		if evs[0].Text != "ok" {
			t.Errorf("tool_result text: got %q, want ok", evs[0].Text)
		}
	})

	t.Run("mcp done summarizes summary field, not truncated JSON", func(t *testing.T) {
		line := []byte(`{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t9","name":"mcp__recur__done","input":{"summary":"Created a full-featured terminal Tetris game (tetris.py) using Python's curses module, with all 7 tetromino types."}}]}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse: %v", err)
		}
		if len(evs) != 1 || evs[0].Tool != "mcp__recur__done" {
			t.Fatalf("step wrong: %+v", evs)
		}
		if strings.Contains(evs[0].ToolInput, `{"summary"`) {
			t.Errorf("want plain summary, got JSON: %q", evs[0].ToolInput)
		}
		if !strings.Contains(evs[0].ToolInput, "full-featured terminal Tetris") {
			t.Errorf("summary missing: %q", evs[0].ToolInput)
		}
	})

	t.Run("user string content (no tool_result)", func(t *testing.T) {
		line := []byte(`{"type":"user","message":{"role":"user","content":"hello"}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse user string: %v", err)
		}
		if len(evs) != 0 {
			t.Errorf("string content should yield no events, got %+v", evs)
		}
	})

	t.Run("result success", func(t *testing.T) {
		line := []byte(`{"type":"result","subtype":"success","session_id":"sess-abc","duration_ms":4200,"duration_api_ms":3900,"is_error":false,"num_turns":3,"result":"Done editing.","total_cost_usd":0.0123,"stop_reason":"end_turn"}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse result: %v", err)
		}
		if len(evs) != 1 || evs[0].Kind != KindResult || evs[0].Result == nil {
			t.Fatalf("want 1 result event, got %+v", evs)
		}
		r := evs[0].Result
		if r.Subtype != "success" || r.IsError {
			t.Errorf("success result flags wrong: %+v", r)
		}
		if r.SessionID != "sess-abc" || r.NumTurns != 3 || r.DurationMS != 4200 {
			t.Errorf("result fields wrong: %+v", r)
		}
		if r.Text != "Done editing." {
			t.Errorf("result text wrong: %+v", r)
		}
	})

	t.Run("non-terminal result is not KindResult", func(t *testing.T) {
		for _, tc := range []struct {
			name string
			line string
			want bool
		}{
			{
				name: "tool_deferred 1ms",
				line: `{"type":"result","subtype":"success","session_id":"s","duration_ms":1,"is_error":false,"num_turns":1,"result":"","stop_reason":"tool_deferred","deferred_tool_use":{"id":"toolu_bash"}}`,
			},
			{
				name: "queued follow-up",
				line: `{"type":"result","subtype":"success","session_id":"s","duration_ms":1,"is_error":false,"num_turns":1,"result":"","queued_turn_count":1}`,
			},
			{
				name: "background_requested",
				line: `{"type":"result","subtype":"success","session_id":"s","duration_ms":12,"is_error":false,"num_turns":1,"result":"","terminal_reason":"background_requested"}`,
			},
			{
				name: "resumed task-notification replay",
				line: `{"type":"result","subtype":"success","session_id":"s","duration_ms":31,"is_error":false,"num_turns":0,"result":"","stop_reason":null,"origin":{"kind":"task-notification"},"queued_turn_count":0}`,
			},
			{
				name: "task-notification follow-up turn",
				line: `{"type":"result","subtype":"success","session_id":"s","duration_ms":4200,"is_error":false,"num_turns":1,"result":"done","stop_reason":"end_turn","origin":{"kind":"task-notification"}}`,
				want: true,
			},
			{
				name: "tiny terminal still counts",
				line: `{"type":"result","subtype":"success","session_id":"s","duration_ms":1,"is_error":false,"num_turns":1,"result":"ok"}`,
				want: true,
			},
			{
				name: "real completed",
				line: `{"type":"result","subtype":"success","session_id":"s","duration_ms":62095,"is_error":false,"num_turns":2,"result":"done","stop_reason":"end_turn"}`,
				want: true,
			},
		} {
			t.Run(tc.name, func(t *testing.T) {
				evs, err := parseStreamLine([]byte(tc.line))
				if err != nil {
					t.Fatalf("parse: %v", err)
				}
				got := len(evs) == 1 && evs[0].Kind == KindResult
				if got != tc.want {
					t.Fatalf("KindResult=%v want %v, events=%+v", got, tc.want, evs)
				}
			})
		}
	})

	t.Run("result usage and contextWindow", func(t *testing.T) {
		line := []byte(`{"type":"result","subtype":"success","session_id":"sess-u","duration_ms":100,"is_error":false,"num_turns":1,"result":"ok","total_cost_usd":0.01,"usage":{"input_tokens":120,"output_tokens":40,"cache_creation_input_tokens":500,"cache_read_input_tokens":18000},"modelUsage":{"claude-sonnet-4-6":{"inputTokens":120,"outputTokens":40,"cacheCreationInputTokens":500,"cacheReadInputTokens":18000,"contextWindow":200000}}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse: %v", err)
		}
		r := evs[0].Result
		if r.InputTokens != 120 || r.OutputTokens != 40 || r.CacheCreationTokens != 500 || r.CacheReadTokens != 18000 {
			t.Errorf("usage = %+v", r)
		}
		if r.ContextWindow != 200000 {
			t.Errorf("contextWindow = %d, want 200000", r.ContextWindow)
		}
		if got := contextUsedTokens(r); got != 18620 {
			t.Errorf("contextUsedTokens = %d, want 18620", got)
		}
	})

	t.Run("system compact_boundary", func(t *testing.T) {
		line := []byte(`{"type":"system","subtype":"compact_boundary","compactMetadata":{"trigger":"auto","preTokens":167219}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse compact_boundary: %v", err)
		}
		if len(evs) != 1 || evs[0].Kind != KindCompact {
			t.Fatalf("parseStreamLine(compact_boundary) = %+v, want KindCompact", evs)
		}
		if evs[0].CompactStatus != domain.CompactDone || evs[0].CompactTrigger != "auto" {
			t.Errorf("compact status/trigger = %q/%q, want done/auto", evs[0].CompactStatus, evs[0].CompactTrigger)
		}
		if evs[0].Tokens != 167219 {
			t.Errorf("compact preTokens = %d, want 167219", evs[0].Tokens)
		}
	})

	t.Run("system compact_metadata snake_case", func(t *testing.T) {
		line := []byte(`{"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"manual","pre_tokens":18875}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse compact_metadata: %v", err)
		}
		if len(evs) != 1 || evs[0].CompactTrigger != "manual" || evs[0].Tokens != 18875 {
			t.Errorf("parseStreamLine(compact_metadata) = %+v, want trigger=manual preTokens=18875", evs)
		}
	})

	t.Run("system status compacting", func(t *testing.T) {
		line := []byte(`{"type":"system","subtype":"status","status":"compacting"}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse compacting status: %v", err)
		}
		if len(evs) != 1 || evs[0].Kind != KindCompact || evs[0].CompactStatus != domain.CompactStarted {
			t.Errorf("parseStreamLine(status compacting) = %+v, want KindCompact started", evs)
		}
	})

	t.Run("result error_max_turns (no result field)", func(t *testing.T) {
		line := []byte(`{"type":"result","subtype":"error_max_turns","session_id":"sess-xyz","duration_ms":9000,"is_error":true,"num_turns":12,"total_cost_usd":0.5}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse error result: %v", err)
		}
		if len(evs) != 1 || evs[0].Result == nil {
			t.Fatalf("want 1 result event, got %+v", evs)
		}
		r := evs[0].Result
		if !r.IsError || r.Subtype != "error_max_turns" {
			t.Errorf("error result flags wrong: %+v", r)
		}
		if r.Text != "" {
			t.Errorf("error result must have no final text, got %q", r.Text)
		}
	})

	t.Run("assistant fallback content block", func(t *testing.T) {
		line := []byte(`{"type":"assistant","message":{"model":"claude-opus-4-8","content":[{"type":"fallback","from":{"model":"claude-fable-5"},"to":{"model":"claude-opus-4-8"}},{"type":"text","text":"continuing on Opus."}]}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parseStreamLine(fallback block): %v", err)
		}
		if len(evs) != 2 {
			t.Fatalf("parseStreamLine(fallback block) len = %d, want 2: %+v", len(evs), evs)
		}
		if evs[0].Kind != KindFallback || evs[0].FromModel != "claude-fable-5" || evs[0].ToModel != "claude-opus-4-8" {
			t.Errorf("parseStreamLine(fallback block)[0] = %+v, want KindFallback fable→opus", evs[0])
		}
		if evs[1].Kind != KindText || evs[1].Text != "continuing on Opus." {
			t.Errorf("parseStreamLine(fallback block)[1] = %+v, want KindText", evs[1])
		}
	})

	t.Run("ordinary assistant message.model is not a fallback", func(t *testing.T) {
		line := []byte(`{"type":"assistant","uuid":"asst-1","message":{"role":"assistant","model":"claude-sonnet-4-6","content":[{"type":"text","text":"hello"}]}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parseStreamLine(ordinary model): %v", err)
		}
		for _, ev := range evs {
			if ev.Kind == KindFallback {
				t.Errorf("parseStreamLine(ordinary model) emitted KindFallback = %+v, want text only", ev)
			}
		}
		if len(evs) != 1 || evs[0].Kind != KindText || evs[0].Text != "hello" {
			t.Errorf("parseStreamLine(ordinary model) = %+v, want KindText hello", evs)
		}
	})

	t.Run("assistant supersedes merges content-block fallback", func(t *testing.T) {
		line := []byte(`{"type":"assistant","uuid":"opus-frame","supersedes":["fable-frame"],"message":{"model":"claude-opus-4-8","content":[{"type":"fallback","from":{"model":"claude-fable-5"},"to":{"model":"claude-opus-4-8"}},{"type":"text","text":"replacement"}]}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parseStreamLine(supersedes+block): %v", err)
		}
		nFallback := 0
		var fb *ParsedEvent
		for i := range evs {
			if evs[i].Kind == KindFallback {
				nFallback++
				fb = &evs[i]
			}
		}
		if nFallback != 1 {
			t.Errorf("parseStreamLine(supersedes+block) fallback count = %d, want 1", nFallback)
		}
		if fb == nil || fb.FromModel != "claude-fable-5" || fb.ToModel != "claude-opus-4-8" {
			t.Errorf("parseStreamLine(supersedes+block) fallback = %+v, want fable→opus", fb)
		}
		if fb == nil || len(fb.RetractedUUIDs) != 1 || fb.RetractedUUIDs[0] != "fable-frame" {
			t.Errorf("parseStreamLine(supersedes+block) RetractedUUIDs = %v, want [fable-frame]", fb)
		}
	})

	t.Run("assistant supersedes retracts before replacement content", func(t *testing.T) {
		line := []byte(`{"type":"assistant","uuid":"opus-frame","supersedes":["fable-frame"],"message":{"model":"claude-opus-4-8","content":[{"type":"text","text":"replacement"}],"usage":{"iterations":[{"type":"message","model":"claude-fable-5"},{"type":"fallback_message","model":"claude-opus-4-8"}]}}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parseStreamLine(supersedes): %v", err)
		}
		var fb, text *ParsedEvent
		nFallback := 0
		for i := range evs {
			switch evs[i].Kind {
			case KindFallback:
				nFallback++
				fb = &evs[i]
			case KindText:
				text = &evs[i]
			}
		}
		if nFallback != 1 {
			t.Errorf("parseStreamLine(supersedes) fallback count = %d, want 1", nFallback)
		}
		if fb == nil || fb.FromModel != "claude-fable-5" || fb.ToModel != "claude-opus-4-8" {
			t.Errorf("parseStreamLine(supersedes) fallback = %+v, want fable→opus", fb)
		}
		if fb == nil || len(fb.RetractedUUIDs) != 1 || fb.RetractedUUIDs[0] != "fable-frame" {
			t.Errorf("parseStreamLine(supersedes) RetractedUUIDs = %v, want [fable-frame]", fb)
		}
		if text == nil || text.Text != "replacement" {
			t.Errorf("parseStreamLine(supersedes) text = %+v, want replacement", text)
		}
		if fb != nil && text != nil {
			fi, ti := -1, -1
			for i := range evs {
				if evs[i].Kind == KindFallback && fi < 0 {
					fi = i
				}
				if evs[i].Kind == KindText && ti < 0 {
					ti = i
				}
			}
			if fi < 0 || ti < 0 || fi >= ti {
				t.Errorf("parseStreamLine(supersedes) order fallback@%d text@%d, want fallback before text", fi, ti)
			}
		}
	})

	t.Run("last fallback_message iteration is the serving model", func(t *testing.T) {
		line := []byte(`{"type":"assistant","uuid":"asst-1","message":{"content":[{"type":"text","text":"ok"}],"usage":{"iterations":[{"type":"message","model":"claude-fable-5"},{"type":"fallback_message","model":"claude-opus-4-8"},{"type":"fallback_message","model":"claude-opus-4-7"}]}}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parseStreamLine(cascade iterations): %v", err)
		}
		var fb *ParsedEvent
		for i := range evs {
			if evs[i].Kind == KindFallback {
				fb = &evs[i]
			}
		}
		if fb == nil || fb.FromModel != "claude-fable-5" || fb.ToModel != "claude-opus-4-7" {
			t.Errorf("parseStreamLine(cascade iterations) fallback = %+v, want fable→opus-4-7", fb)
		}
		if fb != nil && fb.UUID != "asst-1" {
			t.Errorf("parseStreamLine(cascade iterations) UUID = %q, want asst-1", fb.UUID)
		}
	})

	t.Run("system model_refusal_fallback retracts frames", func(t *testing.T) {
		line := []byte(`{"type":"system","subtype":"model_refusal_fallback","uuid":"sys-1","original_model":"claude-fable-5","fallback_model":"claude-opus-4-8","retracted_message_uuids":["fable-frame"]}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parseStreamLine(model_refusal_fallback): %v", err)
		}
		if len(evs) != 1 || evs[0].Kind != KindFallback {
			t.Fatalf("parseStreamLine(model_refusal_fallback) = %+v, want KindFallback", evs)
		}
		if evs[0].FromModel != "claude-fable-5" || evs[0].ToModel != "claude-opus-4-8" {
			t.Errorf("parseStreamLine(model_refusal_fallback) models = %+v, want fable→opus", evs[0])
		}
		if len(evs[0].RetractedUUIDs) != 1 || evs[0].RetractedUUIDs[0] != "fable-frame" {
			t.Errorf("parseStreamLine(model_refusal_fallback) RetractedUUIDs = %v, want [fable-frame]", evs[0].RetractedUUIDs)
		}
	})

	t.Run("system model_refusal_fallback camelCase", func(t *testing.T) {
		line := []byte(`{"type":"system","subtype":"model_refusal_fallback","originalModel":"claude-fable-5","fallbackModel":"claude-opus-4-8","retractedMessageUuids":["a"]}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parseStreamLine(camelCase refusal): %v", err)
		}
		if len(evs) != 1 || evs[0].FromModel != "claude-fable-5" || evs[0].ToModel != "claude-opus-4-8" || len(evs[0].RetractedUUIDs) != 1 {
			t.Errorf("parseStreamLine(camelCase refusal) = %+v, want fable→opus retracted [a]", evs)
		}
	})

	t.Run("stream_event content_block_start fallback", func(t *testing.T) {
		line := []byte(`{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"fallback","from":{"model":"claude-fable-5"},"to":{"model":"claude-opus-4-8"}}}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parseStreamLine(fallback start): %v", err)
		}
		if len(evs) != 1 || evs[0].Kind != KindFallback || evs[0].FromModel != "claude-fable-5" || evs[0].ToModel != "claude-opus-4-8" {
			t.Errorf("parseStreamLine(fallback start) = %+v, want KindFallback fable→opus", evs)
		}
	})

	t.Run("sticky fallback from usage.iterations", func(t *testing.T) {
		line := []byte(`{"type":"assistant","message":{"model":"claude-opus-4-8","content":[{"type":"text","text":"served by Opus."}],"usage":{"input_tokens":10,"output_tokens":4,"iterations":[{"type":"message","model":"claude-fable-5"},{"type":"fallback_message","model":"claude-opus-4-8"}]}}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parseStreamLine(sticky iterations): %v", err)
		}
		var fb, text *ParsedEvent
		for i := range evs {
			switch evs[i].Kind {
			case KindFallback:
				fb = &evs[i]
			case KindText:
				text = &evs[i]
			}
		}
		if fb == nil || fb.FromModel != "claude-fable-5" || fb.ToModel != "claude-opus-4-8" {
			t.Errorf("parseStreamLine(sticky iterations) fallback = %+v, want fable→opus", fb)
		}
		if text == nil || text.Text != "served by Opus." {
			t.Errorf("parseStreamLine(sticky iterations) text = %+v, want served by Opus.", text)
		}
		if fb != nil && text != nil {
			fi, ti := -1, -1
			for i, ev := range evs {
				if ev.Kind == KindFallback {
					fi = i
				}
				if ev.Kind == KindText {
					ti = i
				}
			}
			if fi > ti {
				t.Errorf("parseStreamLine(sticky iterations) fallback index %d after text %d, want before", fi, ti)
			}
		}
	})

	t.Run("fallback block wins over iterations (no duplicate)", func(t *testing.T) {
		line := []byte(`{"type":"assistant","message":{"content":[{"type":"fallback","from":{"model":"claude-fable-5"},"to":{"model":"claude-opus-4-8"}},{"type":"text","text":"ok"}],"usage":{"iterations":[{"type":"fallback_message","model":"claude-opus-4-8"}]}}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parseStreamLine(block+iterations): %v", err)
		}
		n := 0
		for _, ev := range evs {
			if ev.Kind == KindFallback {
				n++
			}
		}
		if n != 1 {
			t.Errorf("parseStreamLine(block+iterations) KindFallback count = %d, want 1; evs = %+v", n, evs)
		}
	})

	t.Run("api_retry surfaces notice", func(t *testing.T) {
		evs, err := parseStreamLine([]byte(`{"type":"system","subtype":"api_retry","attempt":3,"max_retries":10,"retry_delay_ms":2000,"error_status":null,"error":"unknown"}`))
		if err != nil || len(evs) != 1 || evs[0].Kind != KindNotice || evs[0].Text != "API request failed (unknown), retrying 3/10…" {
			t.Fatalf("api_retry → %+v, %v", evs, err)
		}
		evs, _ = parseStreamLine([]byte(`{"type":"system","subtype":"api_retry","attempt":1,"max_retries":10,"error_status":529,"error":"server_error"}`))
		if len(evs) != 1 || evs[0].Text != "API request failed (server_error 529), retrying 1/10…" {
			t.Fatalf("api_retry with status → %+v", evs)
		}
	})

	t.Run("unknown type ignored", func(t *testing.T) {
		for _, line := range [][]byte{
			[]byte(`{"type":"stream_event","event":{}}`),
			[]byte(`{"type":"totally_unknown"}`),
			[]byte(``),
			[]byte(`   `),
		} {
			evs, err := parseStreamLine(line)
			if err != nil {
				t.Errorf("unknown/blank line should not error: %q → %v", line, err)
			}
			if len(evs) != 0 {
				t.Errorf("unknown/blank line should yield no events: %q → %+v", line, evs)
			}
		}
	})

	t.Run("malformed json errors", func(t *testing.T) {
		if _, err := parseStreamLine([]byte(`{not json`)); err == nil {
			t.Errorf("expected error on malformed json")
		}
	})

	t.Run("stream_event text_delta", func(t *testing.T) {
		line := []byte(`{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse stream_event: %v", err)
		}
		if len(evs) != 1 || evs[0].Kind != KindTextDelta || evs[0].Text != "Hel" {
			t.Fatalf("want text_delta Hel, got %+v", evs)
		}
	})

	t.Run("stream_event unknown delta ignored", func(t *testing.T) {
		line := []byte(`{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{}"}}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse: %v", err)
		}
		if len(evs) != 0 {
			t.Fatalf("want no events, got %+v", evs)
		}
	})

	t.Run("parent_tool_use_id keeps nested tools and text, drops nested result", func(t *testing.T) {
		parent := "toolu_task_1"
		asst, err := parseStreamLine([]byte(`{"type":"assistant","parent_tool_use_id":"` + parent + `","message":{"content":[{"type":"text","text":"I'll grep the UI."},{"type":"tool_use","id":"t_nested","name":"Grep","input":{"pattern":"PRDetail"}}]}}`))
		if err != nil {
			t.Fatalf("parse nested assistant: %v", err)
		}
		if len(asst) != 2 {
			t.Fatalf("want nested text + Grep, got %+v", asst)
		}
		if asst[0].Kind != KindText || asst[0].Text != "I'll grep the UI." || asst[0].ParentToolUseID != parent {
			t.Fatalf("want nested text, got %+v", asst[0])
		}
		if asst[1].Kind != KindStep || asst[1].Tool != "Grep" || asst[1].ToolUseID != "t_nested" || asst[1].ParentToolUseID != parent {
			t.Fatalf("want nested Grep step, got %+v", asst[1])
		}

		usr, err := parseStreamLine([]byte(`{"type":"user","parent_tool_use_id":"` + parent + `","message":{"content":[{"type":"tool_result","tool_use_id":"t_nested","content":[{"type":"text","text":"src/PR.svelte"}],"is_error":false}]}}`))
		if err != nil {
			t.Fatalf("parse nested user: %v", err)
		}
		if len(usr) != 1 || usr[0].Kind != KindToolResult || usr[0].ToolUseID != "t_nested" || usr[0].ParentToolUseID != parent {
			t.Fatalf("want nested tool_result, got %+v", usr)
		}

		delta, err := parseStreamLine([]byte(`{"type":"stream_event","parent_tool_use_id":"` + parent + `","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"inside"}}}`))
		if err != nil {
			t.Fatalf("parse nested stream_event: %v", err)
		}
		if len(delta) != 1 || delta[0].Kind != KindTextDelta || delta[0].Text != "inside" || delta[0].ParentToolUseID != parent {
			t.Fatalf("want nested text_delta, got %+v", delta)
		}

		for _, line := range [][]byte{
			[]byte(`{"type":"result","parent_tool_use_id":"` + parent + `","subtype":"success","session_id":"sub","is_error":false,"result":"subagent done","total_cost_usd":0.1}`),
			[]byte(`{"type":"system","subtype":"init","parent_tool_use_id":"` + parent + `","session_id":"sub","model":"sonnet","mcp_servers":[]}`),
		} {
			evs, err := parseStreamLine(line)
			if err != nil {
				t.Fatalf("parse nested: %v\n%s", err, line)
			}
			if len(evs) != 0 {
				t.Errorf("nested line leaked %d events: %+v\n%s", len(evs), evs, line)
			}
		}
	})

	t.Run("parent Task tool_use without parent_tool_use_id is kept", func(t *testing.T) {
		line := []byte(`{"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"toolu_task_1","name":"Task","input":{"description":"Map PR detail UI structure","subagent_type":"Explore","prompt":"Search the Svelte frontend."}}]}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse parent Task: %v", err)
		}
		if len(evs) != 1 || evs[0].Kind != KindStep || evs[0].Tool != "Task" || evs[0].ToolUseID != "toolu_task_1" {
			t.Fatalf("want parent Task step, got %+v", evs)
		}
	})

	t.Run("async Task launch metadata is not a tool_result", func(t *testing.T) {
		ack := `Async agent launched successfully.\n(This tool result is internal metadata – never quote or paste any part of it, including the agentId below, into a user-facing reply.)\n\nagentId: a2a1c3c855409bb10\n`
		line := []byte(`{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_task_1","content":[{"type":"text","text":"` + ack + `"}],"is_error":false}]}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse launch ack: %v", err)
		}
		if len(evs) != 0 {
			t.Fatalf("launch metadata leaked: %+v", evs)
		}
	})

	t.Run("tool_result mentioning launch metadata is kept", func(t *testing.T) {
		text := "see Async agent launched successfully plus agentId and internal metadata in comments"
		line := []byte(`{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t_read","content":[{"type":"text","text":"` + text + `"}],"is_error":false}]}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse: %v", err)
		}
		if len(evs) != 1 || evs[0].Kind != KindToolResult || evs[0].Text != text {
			t.Fatalf("want kept Read-like result, got %+v", evs)
		}
	})

	t.Run("ordinary tool_result is kept", func(t *testing.T) {
		line := []byte(`{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"ok"}],"is_error":false}]}}`)
		evs, err := parseStreamLine(line)
		if err != nil {
			t.Fatalf("parse: %v", err)
		}
		if len(evs) != 1 || evs[0].Kind != KindToolResult || evs[0].Text != "ok" {
			t.Fatalf("want ordinary tool_result, got %+v", evs)
		}
	})
}

// fixtureLines returns a canonical happy-path stream-json transcript (init →
// assistant-with-tool_use → user-with-tool_result → success result) for the
// ScriptedRunner-based engine tests.
func fixtureLines(sessionID, filePath, finalText string, cost float64) [][]byte {
	return [][]byte{
		[]byte(`{"type":"system","subtype":"init","session_id":"` + sessionID + `","model":"claude-sonnet-4-6","mcp_servers":[]}`),
		[]byte(`{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Write","input":{"file_path":"` + filePath + `"}}]}}`),
		[]byte(`{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":false}]}}`),
		[]byte(`{"type":"result","subtype":"success","session_id":"` + sessionID + `","duration_ms":1000,"is_error":false,"num_turns":1,"result":"` + finalText + `","total_cost_usd":` + ftoa(cost) + `}`),
	}
}

func ftoa(f float64) string {
	// small helper to avoid importing strconv in the fixture for one value
	switch f {
	case 0.05:
		return "0.05"
	case 0.10:
		return "0.10"
	case 0.25:
		return "0.25"
	default:
		return "0"
	}
}

// sanity: domain import used (keeps the file honest if fixtures evolve).
var _ = domain.AgentStep{}
