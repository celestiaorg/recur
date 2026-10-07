package engine

import (
	"context"
	"testing"

	"recur/internal/domain"
)

// TestAssistantUsageEmitsKindUsage pins the live context-meter signal: each
// assistant.message.usage is a per-inference footprint, not session billing.
func TestAssistantUsageEmitsKindUsage(t *testing.T) {
	line := []byte(`{"type":"assistant","message":{"role":"assistant","model":"claude-sonnet-5","usage":{"input_tokens":100,"output_tokens":20,"cache_creation_input_tokens":5,"cache_read_input_tokens":80000},"content":[{"type":"text","text":"hi"}]}}`)
	evs, err := parseStreamLine(line)
	if err != nil {
		t.Fatalf("parse: %v", err)
	}
	var usage *Result
	var sawText bool
	for i := range evs {
		switch evs[i].Kind {
		case KindUsage:
			usage = evs[i].Result
		case KindText:
			sawText = true
		}
	}
	if !sawText {
		t.Fatal("expected KindText")
	}
	if usage == nil {
		t.Fatal("expected KindUsage from assistant.message.usage")
	}
	if usage.InputTokens != 100 || usage.CacheReadTokens != 80000 || usage.CacheCreationTokens != 5 {
		t.Errorf("usage = %+v", usage)
	}
	if got := contextUsedTokens(usage); got != 80105 {
		t.Errorf("contextUsedTokens = %d, want 80105", got)
	}
}

// TestLastAssistantUsageWinsOverCumulativeResult is the meter accuracy guard:
// result.usage sums every inference (billing) and can exceed the window;
// the meter must stamp the last assistant footprint instead.
func TestLastAssistantUsageWinsOverCumulativeResult(t *testing.T) {
	lines := [][]byte{
		[]byte(`{"type":"system","subtype":"init","session_id":"sess-ctx","model":"claude-sonnet-5","mcp_servers":[]}`),
		[]byte(`{"type":"assistant","message":{"usage":{"input_tokens":50,"output_tokens":10,"cache_read_input_tokens":10000,"cache_creation_input_tokens":0},"content":[{"type":"text","text":"step1"},{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"a.go"}}]}}`),
		[]byte(`{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"ok"}]}}`),
		// Last inference: ~90k context fill.
		[]byte(`{"type":"assistant","message":{"usage":{"input_tokens":200,"output_tokens":40,"cache_read_input_tokens":90000,"cache_creation_input_tokens":0},"content":[{"type":"text","text":"done"}]}}`),
		// Cumulative billing across both inferences (~100k+) — must NOT drive the meter.
		[]byte(`{"type":"result","subtype":"success","session_id":"sess-ctx","duration_ms":800,"is_error":false,"num_turns":2,"result":"done","total_cost_usd":0.12,"usage":{"input_tokens":250,"output_tokens":50,"cache_creation_input_tokens":0,"cache_read_input_tokens":100000},"modelUsage":{"claude-sonnet-5":{"contextWindow":1000000,"inputTokens":250,"outputTokens":50,"cacheReadInputTokens":100000,"cacheCreationInputTokens":0}}}`),
	}

	runner := &ScriptedRunner{Lines: lines}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	workspaceID := startedLoop(t, eng, projID)

	res, err := eng.runAgent(context.Background(), AgentSpec{
		WorkspaceID: workspaceID,
		AgentID:     workspaceID + "-CO",
		Role:        domain.RoleCoder,
		Model:       "claude-code/sonnet",
		Prompt:      "go",
	}, nil)
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if res.InputTokens != 200 || res.CacheReadTokens != 90000 {
		t.Fatalf("result meter tokens in=%d cacheRead=%d; want last assistant 200/90000 (not cumulative 250/100000)",
			res.InputTokens, res.CacheReadTokens)
	}
	if got := contextUsedTokens(&res); got != 90200 {
		t.Errorf("contextUsedTokens = %d, want 90200", got)
	}

	sess, ok := st.SessionForWorkspaceKind(workspaceID, string(domain.RoleCoder))
	if !ok {
		t.Fatal("no session")
	}
	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		t.Fatalf("transcript: %v", err)
	}
	var stamped domain.Message
	for _, m := range msgs {
		if m.DurationMS > 0 {
			stamped = m
		}
	}
	if stamped.ID == "" {
		t.Fatal("no turn-final message with duration")
	}
	if stamped.InputTokens != 200 || stamped.CacheReadTokens != 90000 {
		t.Errorf("mirrored meter tokens in=%d cacheRead=%d; want 200/90000",
			stamped.InputTokens, stamped.CacheReadTokens)
	}

	// Reload path: turn footer projected from the stamped message.
	turn := turnChatItem(stamped, "claude-code/sonnet", nil)
	if turn.InputTokens != 200 || turn.CacheReadTokens != 90000 {
		t.Errorf("turn footer meter in=%d cacheRead=%d; want 200/90000",
			turn.InputTokens, turn.CacheReadTokens)
	}
	// Always-1M Sonnet 5 window from dock/model resolution.
	if turn.ContextWindow != 1_000_000 {
		t.Errorf("ContextWindow = %d, want 1000000", turn.ContextWindow)
	}
}

// TestCursorResultPrefersMidStreamUsageForMeter: terminal result usage can be
// cumulative run billing; mid-stream usage frames are the live footprint when
// they still fit in the model window (single-call / already-per-inference).
func TestCursorResultPrefersMidStreamUsageForMeter(t *testing.T) {
	const (
		mid = `{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":3000,"outputTokens":100,"cacheReadTokens":70000,"cacheWriteTokens":0,"totalTokens":73100}}}}`
		// Cumulative-looking terminal totals (larger than last mid-stream frame).
		fin = `{"result":{"status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"ok","durationMs":"900","usage":{"inputTokens":"9000","outputTokens":"400","cacheReadTokens":"200000","totalTokens":"209400"}}}}`
	)
	turn, _ := feed(t, AgentSpec{Model: "composer-2.5"}, mid, fin)
	res := turn.result()
	if res.InputTokens != 3000 || res.CacheReadTokens != 70000 {
		t.Fatalf("meter tokens in=%d cacheRead=%d; want mid-stream 3000/70000 not cumulative 9000/200000",
			res.InputTokens, res.CacheReadTokens)
	}
}

// TestCursorMeterScalesCumulativeTurnUsage: Cursor's turn-ended usage sums every
// model call in the tool loop and routinely exceeds the window (live DB: millions
// of tokens on grok-4.5). The composer meter must estimate the last-call footprint
// from tool-wave count, not paint a pegged-full ring.
func TestCursorMeterScalesCumulativeTurnUsage(t *testing.T) {
	const (
		// Two tool waves separated by model reasoning → 3 model calls.
		tool1Run = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c1","name":"shell","status":"running","args":{"command":"ls"}}}}`
		tool1OK  = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c1","name":"shell","status":"completed","args":{"command":"ls"},"result":{"status":"success","value":{"isError":false}}}}}`
		// Reasoning boundary closes wave 1 before wave 2 starts.
		think    = `{"sdkMessage":{"type":"thinking","message":{"type":"thinking","text":"next"}}}`
		thinkEnd = `{"sdkMessage":{"type":"thinking","message":{"type":"thinking","text":""}}}`
		tool2Run = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c2","name":"shell","status":"running","args":{"command":"pwd"}}}}`
		tool2OK  = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c2","name":"shell","status":"completed","args":{"command":"pwd"},"result":{"status":"success","value":{"isError":false}}}}}`
		// Cumulative billing across 3 calls: 150k+150k = 300k > composer 200k window.
		usage = `{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":150000,"outputTokens":900,"cacheReadTokens":150000,"cacheWriteTokens":0,"totalTokens":300900}}}}`
		fin   = `{"result":{"status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"ok","durationMs":"900","usage":{"inputTokens":"150000","outputTokens":"900","cacheReadTokens":"150000","totalTokens":"300900"}}}}`
	)
	turn, _ := feed(t, AgentSpec{Model: "composer-2.5"},
		tool1Run, tool1OK, think, thinkEnd, tool2Run, tool2OK, usage, fin)
	res := turn.result()
	// Arithmetic-series last-call estimate: used * 2 / (rounds+1) with rounds=3
	// → 300000 * 2 / 4 = 150000, split across input/cacheRead.
	if got := contextUsedTokens(&res); got != 150000 {
		t.Fatalf("meter used = %d, want 150000 (scaled last-call estimate); raw was 300000", got)
	}
	if res.InputTokens != 75000 || res.CacheReadTokens != 75000 {
		t.Errorf("scaled fields in=%d cacheRead=%d; want 75000/75000", res.InputTokens, res.CacheReadTokens)
	}
}

// TestCursorMeterSerialSiblingToolsShareOneWave: Cursor often streams tools from
// one model call as running→completed pairs. Those must not each open a new wave
// (that under-counts fill after scaling).
func TestCursorMeterSerialSiblingToolsShareOneWave(t *testing.T) {
	const (
		tool1Run = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c1","name":"shell","status":"running","args":{"command":"ls"}}}}`
		tool1OK  = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c1","name":"shell","status":"completed","args":{"command":"ls"},"result":{"status":"success","value":{"isError":false}}}}}`
		tool2Run = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c2","name":"shell","status":"running","args":{"command":"pwd"}}}}`
		tool2OK  = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c2","name":"shell","status":"completed","args":{"command":"pwd"},"result":{"status":"success","value":{"isError":false}}}}}`
		usage    = `{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":150000,"outputTokens":900,"cacheReadTokens":150000,"cacheWriteTokens":0,"totalTokens":300900}}}}`
		fin      = `{"result":{"status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"ok","durationMs":"900","usage":{"inputTokens":"150000","outputTokens":"900","cacheReadTokens":"150000","totalTokens":"300900"}}}}`
	)
	turn, _ := feed(t, AgentSpec{Model: "composer-2.5"}, tool1Run, tool1OK, tool2Run, tool2OK, usage, fin)
	if turn.toolWaves != 1 {
		t.Fatalf("toolWaves = %d, want 1 (serial siblings share a wave)", turn.toolWaves)
	}
	res := turn.result()
	// One wave → rounds=2 → 300000 * 2 / 3 = 200000 (clamped to composer window).
	if got := contextUsedTokens(&res); got != 200000 {
		t.Fatalf("meter used = %d, want 200000 (rounds=2 estimate clamped to window)", got)
	}
}

// TestCursorMeterSubtractsUsageAtSummary: Cursor usage is run-cumulative, so a
// post-compact frame still includes the discarded window. Snapshot at
// summary-started and meter the delta.
func TestCursorMeterSubtractsUsageAtSummary(t *testing.T) {
	const (
		pre   = `{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":150000,"outputTokens":200,"cacheReadTokens":150000,"cacheWriteTokens":0,"totalTokens":300200}}}}`
		start = `{"interactionUpdate":{"type":"summary-started"}}`
		done  = `{"interactionUpdate":{"type":"summary-completed"}}`
		post  = `{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":165000,"outputTokens":260,"cacheReadTokens":165000,"cacheWriteTokens":0,"totalTokens":330260}}}}`
		fin   = `{"result":{"status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"ok","durationMs":"900","usage":{"inputTokens":"165000","outputTokens":"260","cacheReadTokens":"165000","totalTokens":"330260"}}}}`
	)
	turn, got := feed(t, AgentSpec{Model: "composer-2.5"}, pre, start, done, post, fin)
	var sawStart, sawDone bool
	for _, ev := range got {
		if ev.Kind != KindCompact {
			continue
		}
		switch ev.CompactStatus {
		case domain.CompactStarted:
			sawStart = true
		case domain.CompactDone:
			sawDone = true
		}
	}
	if !sawStart || !sawDone {
		t.Fatalf("compact events start=%v done=%v; want both (got kinds %+v)", sawStart, sawDone, kindsOf(got))
	}
	for _, ev := range got {
		if ev.Kind == KindCompact && ev.Tokens != 200000 {
			t.Fatalf("compact card tokens = %d, want 200000 (300k cumulative clamped to window)", ev.Tokens)
		}
	}
	res := turn.result()
	// Post-compact delta: (165k+165k) − (150k+150k) = 30k.
	if got := contextUsedTokens(&res); got != 30000 {
		t.Fatalf("meter used = %d, want 30000 (post-summary delta); raw cumulative was 330000", got)
	}
}

// TestCursorMeterScalesCumulativeUsageUnderWindow is the Composer 2.5 hello
// case: a few tool waves whose run-cumulative billing (114k) still fits the
// 200k window. scaleCursorMeterUsage used to keep that total as if it were a
// single prompt footprint. Recur estimates
// last ≈ used * 2 / (rounds+1) (arithmetic series), not equal-split.
func TestCursorMeterScalesCumulativeUsageUnderWindow(t *testing.T) {
	const (
		tool1Run  = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c1","name":"glob","status":"running","args":{"glob_pattern":"*"}}}}`
		tool1OK   = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c1","name":"glob","status":"completed","args":{"glob_pattern":"*"},"result":{"status":"success","value":{"isError":false}}}}}`
		think     = `{"sdkMessage":{"type":"thinking","message":{"type":"thinking","text":"next"}}}`
		thinkEnd  = `{"sdkMessage":{"type":"thinking","message":{"type":"thinking","text":""}}}`
		tool2Run  = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c2","name":"read","status":"running","args":{"path":"main.go"}}}}`
		tool2OK   = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c2","name":"read","status":"completed","args":{"path":"main.go"},"result":{"status":"success","value":{"isError":false}}}}}`
		tool3Run  = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c3","name":"glob","status":"running","args":{"glob_pattern":"README*"}}}}`
		tool3OK   = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c3","name":"glob","status":"completed","args":{"glob_pattern":"README*"},"result":{"status":"success","value":{"isError":false}}}}}`
		think2    = `{"sdkMessage":{"type":"thinking","message":{"type":"thinking","text":"rename"}}}`
		think2End = `{"sdkMessage":{"type":"thinking","message":{"type":"thinking","text":""}}}`
		tool4Run  = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c4","name":"mcp","status":"running","args":{"toolName":"rename_workspace"}}}}`
		tool4OK   = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c4","name":"mcp","status":"completed","args":{"toolName":"rename_workspace"},"result":{"status":"success","value":{"isError":false}}}}}`
		tool5Run  = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c5","name":"mcp","status":"running","args":{"toolName":"rename_branch"}}}}`
		tool5OK   = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c5","name":"mcp","status":"completed","args":{"toolName":"rename_branch"},"result":{"status":"success","value":{"isError":false}}}}}`
		tool6Run  = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c6","name":"mcp","status":"running","args":{"toolName":"rename_session"}}}}`
		tool6OK   = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c6","name":"mcp","status":"completed","args":{"toolName":"rename_session"},"result":{"status":"success","value":{"isError":false}}}}}`
		// Live savory-parfait "hello" on Composer 2.5: 61155+53344 = 114499 < 200k.
		usage = `{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":61155,"outputTokens":763,"cacheReadTokens":53344,"cacheWriteTokens":0,"totalTokens":115262}}}}`
		fin   = `{"result":{"status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"ok","durationMs":"7725","usage":{"inputTokens":"61155","outputTokens":"763","cacheReadTokens":"53344","totalTokens":"115262"}}}}`
	)
	turn, _ := feed(t, AgentSpec{Model: "composer-2.5"},
		tool1Run, tool1OK, think, thinkEnd,
		tool2Run, tool2OK, tool3Run, tool3OK, think2, think2End,
		tool4Run, tool4OK, tool5Run, tool5OK, tool6Run, tool6OK,
		usage, fin)
	if turn.toolWaves != 3 {
		t.Fatalf("toolWaves = %d, want 3 (glob | read+glob | three mcp)", turn.toolWaves)
	}
	res := turn.result()
	// Arithmetic last-call: 114499 * 2 / 5 rounds-term = 45799.
	const wantUsed int64 = 45799
	if got := contextUsedTokens(&res); got != wantUsed {
		t.Fatalf("contextUsedTokens = %d, want %d (last-call estimate, not raw cumulative 114499)", got, wantUsed)
	}
	if res.InputTokens+res.CacheReadTokens != wantUsed {
		t.Errorf("scaled fields in=%d cacheRead=%d sum=%d, want %d",
			res.InputTokens, res.CacheReadTokens, res.InputTokens+res.CacheReadTokens, wantUsed)
	}
}

// TestCursorTurnEndedFootprintNotDividedByWaves: a small turn-ended usage is
// already the last prompt. Scaling it by tool-wave count would under-fill.
func TestCursorTurnEndedFootprintNotDividedByWaves(t *testing.T) {
	const (
		tool1Run = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c1","name":"glob","status":"running","args":{"glob_pattern":"*"}}}}`
		tool1OK  = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c1","name":"glob","status":"completed","args":{"glob_pattern":"*"},"result":{"status":"success","value":{"isError":false}}}}}`
		think    = `{"sdkMessage":{"type":"thinking","message":{"type":"thinking","text":"next"}}}`
		thinkEnd = `{"sdkMessage":{"type":"thinking","message":{"type":"thinking","text":""}}}`
		tool2Run = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c2","name":"read","status":"running","args":{"path":"main.go"}}}}`
		tool2OK  = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c2","name":"read","status":"completed","args":{"path":"main.go"},"result":{"status":"success","value":{"isError":false}}}}}`
		cumul    = `{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":61155,"outputTokens":763,"cacheReadTokens":53344,"totalTokens":115262}}}}`
		ended    = `{"interactionUpdate":{"type":"turn-ended","usage":{"inputTokens":12000,"outputTokens":400,"cacheReadTokens":8000,"cacheWriteTokens":0}}}`
		fin      = `{"result":{"status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"ok","durationMs":"7725","usage":{"inputTokens":"61155","outputTokens":"763","cacheReadTokens":"53344","totalTokens":"115262"}}}}`
	)
	turn, _ := feed(t, AgentSpec{Model: "composer-2.5"},
		tool1Run, tool1OK, think, thinkEnd, tool2Run, tool2OK, cumul, ended, fin)
	res := turn.result()
	if got := contextUsedTokens(&res); got != 20000 {
		t.Fatalf("contextUsedTokens = %d, want 20000 from turn-ended footprint (not 20000/rounds or 114499)", got)
	}
}

// TestCursorTurnEndedUsageIsPerCallFootprint: a turn-ended usage that fits the
// model window is the last conversation-turn prompt, not run-cumulative billing.
func TestCursorTurnEndedUsageIsPerCallFootprint(t *testing.T) {
	const (
		cumul = `{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":150000,"outputTokens":900,"cacheReadTokens":150000,"totalTokens":300900}}}}`
		ended = `{"interactionUpdate":{"type":"turn-ended","usage":{"inputTokens":12000,"outputTokens":400,"cacheReadTokens":8000,"cacheWriteTokens":0}}}`
		fin   = `{"result":{"status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"ok","durationMs":"400","usage":{"inputTokens":"150000","outputTokens":"900","cacheReadTokens":"150000","totalTokens":"300900"}}}}`
	)
	turn, _ := feed(t, AgentSpec{Model: "composer-2.5"}, cumul, ended, fin)
	res := turn.result()
	if got := contextUsedTokens(&res); got != 20000 {
		t.Fatalf("meter used = %d, want 20000 from turn-ended footprint (not scaled 300900)", got)
	}
}

// TestCursorCompactBaselineSkipsPerCallFootprint: a turn-ended frame may
// overwrite t.usage with a small footprint. Compact must snapshot the last
// cumulative total, or post-summary subtraction barely reduces fill.
func TestCursorCompactBaselineSkipsPerCallFootprint(t *testing.T) {
	const (
		pre   = `{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":150000,"outputTokens":200,"cacheReadTokens":150000,"cacheWriteTokens":0,"totalTokens":300200}}}}`
		ended = `{"interactionUpdate":{"type":"turn-ended","usage":{"inputTokens":12000,"outputTokens":400,"cacheReadTokens":8000,"cacheWriteTokens":0}}}`
		start = `{"interactionUpdate":{"type":"summary-started"}}`
		done  = `{"interactionUpdate":{"type":"summary-completed"}}`
		post  = `{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":165000,"outputTokens":260,"cacheReadTokens":165000,"cacheWriteTokens":0,"totalTokens":330260}}}}`
		fin   = `{"result":{"status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"ok","durationMs":"900","usage":{"inputTokens":"165000","outputTokens":"260","cacheReadTokens":"165000","totalTokens":"330260"}}}}`
	)
	turn, _ := feed(t, AgentSpec{Model: "composer-2.5"}, pre, ended, start, done, post, fin)
	res := turn.result()
	if got := contextUsedTokens(&res); got != 30000 {
		t.Fatalf("meter used = %d, want 30000 (cumulative−cumulative); snapshotting the 20k footprint would leave ~200k", got)
	}
}

// TestCursorCompactClearsPerCallMeter: compact with no later usage frame must
// not stamp the pre-summary turn-ended footprint. That prompt was discarded.
func TestCursorCompactClearsPerCallMeter(t *testing.T) {
	const (
		pre   = `{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":150000,"outputTokens":200,"cacheReadTokens":150000,"cacheWriteTokens":0,"totalTokens":300200}}}}`
		ended = `{"interactionUpdate":{"type":"turn-ended","usage":{"inputTokens":12000,"outputTokens":400,"cacheReadTokens":8000,"cacheWriteTokens":0}}}`
		start = `{"interactionUpdate":{"type":"summary-started"}}`
		done  = `{"interactionUpdate":{"type":"summary-completed"}}`
		fin   = `{"result":{"status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"ok","durationMs":"900","usage":{"inputTokens":"150000","outputTokens":"200","cacheReadTokens":"150000","totalTokens":"300200"}}}}`
	)
	turn, got := feed(t, AgentSpec{Model: "composer-2.5"}, pre, ended, start, done, fin)
	if turn.usagePerCall {
		t.Fatal("usagePerCall still set after compact; pre-summary footprint would stamp the meter")
	}
	res := turn.result()
	if got := contextUsedTokens(&res); got != 0 {
		t.Fatalf("meter used = %d, want 0 (no post-compact usage yet; not the discarded 20k footprint)", got)
	}
	var sawUsageAfterDone bool
	seenDone := false
	for _, ev := range got {
		if ev.Kind == KindCompact && ev.CompactStatus == domain.CompactDone {
			seenDone = true
			continue
		}
		if seenDone && ev.Kind == KindUsage {
			sawUsageAfterDone = true
		}
	}
	if sawUsageAfterDone {
		t.Fatal("KindUsage after compact-done with a 0 delta; live UI resets on session.compact")
	}
}

// TestCursorCompactFlushesTextFirst: auto-compaction runs after the last
// assistant chunk. The persisted compact marker must follow that text.
func TestCursorCompactFlushesTextFirst(t *testing.T) {
	const start = `{"interactionUpdate":{"type":"summary-started"}}`
	_, got := feed(t, AgentSpec{Model: "composer-2.5"}, fAssistant, start)
	var kinds []ParsedKind
	for _, ev := range got {
		switch ev.Kind {
		case KindText, KindCompact:
			kinds = append(kinds, ev.Kind)
		}
	}
	if len(kinds) < 2 || kinds[0] != KindText || kinds[1] != KindCompact {
		t.Fatalf("kinds = %v, want KindText then KindCompact", kinds)
	}
}

func kindsOf(evs []ParsedEvent) []ParsedKind {
	out := make([]ParsedKind, len(evs))
	for i, ev := range evs {
		out[i] = ev.Kind
	}
	return out
}
