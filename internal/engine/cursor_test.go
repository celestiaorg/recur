package engine

import (
	"context"
	"encoding/json"
	"strings"
	"testing"
	"time"

	"recur/internal/cursor"
	"recur/internal/domain"
)

// --- spec → AgentOptions -----------------------------------------------------

func TestCursorAgentOptionsWiresInlineMCP(t *testing.T) {
	r := &CursorRunner{APIKey: "sk-test"}
	opts := r.agentOptions(AgentSpec{
		AgentID:  "LOOP-1-CO",
		Model:    "composer-2.5",
		Flags:    map[string]string{"fast": "true"},
		Worktree: "/tmp/wt",
		MCPURL:   "http://127.0.0.1:5555/tok/mcp",
	})

	// The whole reason for the bridge: MCP goes over the wire, never to disk.
	got, ok := opts.MCPServers["recur"]
	if !ok {
		t.Fatalf("no recur MCP server configured: %+v", opts.MCPServers)
	}
	if got.HTTP == nil || got.HTTP.URL != "http://127.0.0.1:5555/tok/mcp" {
		t.Errorf("mcp entry = %+v", got.HTTP)
	}
	if opts.Local == nil || len(opts.Local.Cwd) != 1 || opts.Local.Cwd[0] != "/tmp/wt" {
		t.Errorf("cwd = %+v, want [/tmp/wt]", opts.Local)
	}
	if opts.Local == nil || opts.Local.Sandbox == nil || opts.Local.Sandbox.Enabled {
		t.Errorf("sandbox = %+v, want explicit enabled:false", opts.Local.Sandbox)
	}
	if opts.Model == nil || opts.Model.ID != "composer-2.5" {
		t.Errorf("model = %+v", opts.Model)
	}
	if opts.Model == nil || len(opts.Model.Params) != 1 || opts.Model.Params[0].ID != "fast" || opts.Model.Params[0].Value != "true" {
		t.Errorf("model params = %+v", opts.Model)
	}
	if opts.APIKey != "sk-test" {
		t.Errorf("apiKey not forwarded")
	}
}

func TestCursorAgentOptionsOmitsMCPWhenUnset(t *testing.T) {
	r := &CursorRunner{}
	opts := r.agentOptions(AgentSpec{Worktree: "/tmp/wt"})
	if len(opts.MCPServers) != 0 {
		t.Errorf("expected no MCP servers, got %+v", opts.MCPServers)
	}
}

func TestCursorAgentOptionsAddsPlaywright(t *testing.T) {
	r := &CursorRunner{}
	opts := r.agentOptions(AgentSpec{MCPURL: "http://x/tok/mcp", Playwright: true})
	pw, ok := opts.MCPServers["playwright"]
	if !ok || pw.Stdio == nil {
		t.Fatalf("playwright not wired as stdio: %+v", opts.MCPServers)
	}
	if pw.Stdio.Command != "npx" {
		t.Errorf("command = %q, want npx", pw.Stdio.Command)
	}
	if _, ok := opts.MCPServers["recur"]; !ok {
		t.Error("playwright must not displace the recur MCP server")
	}
}

func TestCursorAgentOptionsAddsUserMCP(t *testing.T) {
	r := &CursorRunner{}
	opts := r.agentOptions(AgentSpec{
		MCPURL: "http://x/tok/mcp",
		UserMCP: []MCPAttach{{
			Name:    "linear",
			URL:     "https://mcp.linear.app/mcp/readonly",
			Headers: map[string]string{"Authorization": "Bearer lin_api_x"},
		}},
	})
	lin, ok := opts.MCPServers["linear"]
	if !ok || lin.HTTP == nil {
		t.Fatalf("linear not wired as http: %+v", opts.MCPServers)
	}
	if lin.HTTP.Headers["Authorization"] != "Bearer lin_api_x" {
		t.Errorf("headers=%v", lin.HTTP.Headers)
	}
	st := mcpStatusFor(AgentSpec{MCPURL: "http://x", UserMCP: []MCPAttach{{Name: "linear"}}})
	if len(st) != 2 || st[1].Name != "linear" {
		t.Fatalf("status=%+v", st)
	}

	stdio := r.agentOptions(AgentSpec{
		UserMCP: []MCPAttach{{
			Name:    "fs",
			Command: "npx",
			Args:    []string{"-y", "mcp-fs"},
			Env:     map[string]string{"FOO": "bar"},
		}},
	})
	fs, ok := stdio.MCPServers["fs"]
	if !ok || fs.Stdio == nil {
		t.Fatalf("stdio not wired: %+v", stdio.MCPServers)
	}
	if fs.Stdio.Env["FOO"] != "bar" {
		t.Errorf("stdio env=%v", fs.Stdio.Env)
	}
}

func TestCursorStrictConfigOptIn(t *testing.T) {
	// Default: the repo's own .cursor/rules and AGENTS.md load, matching how
	// ClaudeRunner lets CLAUDE.md load.
	loose := (&CursorRunner{}).agentOptions(AgentSpec{Worktree: "/tmp/wt"})
	if len(loose.Local.SettingSources) != 0 {
		t.Errorf("default should not restrict setting sources, got %v", loose.Local.SettingSources)
	}

	strict := (&CursorRunner{StrictConfig: true}).agentOptions(AgentSpec{Worktree: "/tmp/wt"})
	if len(strict.Local.SettingSources) == 0 {
		t.Fatal("strict mode must NAME a layer — an empty slice reads as 'defaults' to the bridge")
	}
	for _, s := range strict.Local.SettingSources {
		if s == cursor.SettingSourceProject || s == cursor.SettingSourceUser {
			t.Errorf("strict mode still loads %q", s)
		}
	}
}

// TestCursorStrictConfigMarshalsNonEmpty guards the specific trap: the bridge
// maps `settingSources.length === 0` to undefined (SDK defaults), so serialising
// an empty list would silently do the OPPOSITE of what strict mode promises.
func TestCursorStrictConfigMarshalsNonEmpty(t *testing.T) {
	opts := (&CursorRunner{StrictConfig: true}).agentOptions(AgentSpec{Worktree: "/tmp/wt"})
	b, err := json.Marshal(opts.Local)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	if !strings.Contains(string(b), "settingSources") {
		t.Errorf("settingSources missing from payload: %s", b)
	}
	if strings.Contains(string(b), `"settingSources":[]`) {
		t.Errorf("empty settingSources means 'defaults' to the bridge: %s", b)
	}
}

func TestCursorReadOnlyRoleUsesPlanMode(t *testing.T) {
	ro := (&CursorRunner{}).agentOptions(AgentSpec{
		AllowedTools: []string{"Read", "Glob", "Grep"},
	})
	if ro.Mode != "plan" {
		t.Errorf("read-only role mode = %q, want plan", ro.Mode)
	}

	// Reviewer: bare Bash for lint must NOT drop plan mode.
	rev := (&CursorRunner{}).agentOptions(AgentSpec{
		AllowedTools: []string{"Read", "Bash", "mcp__recur__report_finding"},
	})
	if rev.Mode != "plan" {
		t.Errorf("Bash-without-Edit reviewer mode = %q, want plan", rev.Mode)
	}

	rw := (&CursorRunner{}).agentOptions(AgentSpec{
		AllowedTools: []string{"Read", "Edit", "Write", "Bash"},
	})
	if rw.Mode == "plan" {
		t.Error("a role that edits must not run in plan mode")
	}

	// No allow-list at all means "unscoped", not "read-only".
	none := (&CursorRunner{}).agentOptions(AgentSpec{})
	if none.Mode == "plan" {
		t.Error("empty AllowedTools should not imply plan mode")
	}
}

// --- prompt composition ------------------------------------------------------

func TestCursorPromptCarriesRoleInstructions(t *testing.T) {
	r := &CursorRunner{}
	got := r.prompt(AgentSpec{
		SystemPrompt: "You are the Reviewer.",
		Prompt:       "Review the diff.",
	})
	// Cursor accepts no system prompt, so the role prompt MUST survive in-band
	// or every role would behave identically.
	if !strings.Contains(got, "You are the Reviewer.") {
		t.Errorf("role prompt dropped: %q", got)
	}
	if !strings.Contains(got, "Review the diff.") {
		t.Errorf("task prompt dropped: %q", got)
	}
	if strings.Index(got, "You are the Reviewer.") > strings.Index(got, "Review the diff.") {
		t.Error("role instructions should precede the task")
	}
}

func TestCursorPromptOmitsStandingOnResume(t *testing.T) {
	r := &CursorRunner{}
	got := r.prompt(AgentSpec{
		SystemPrompt:       "You are the Reviewer.",
		AppendSystemPrompt: "This chat is in AGENT mode.",
		Prompt:             "fix the flake",
		ResumeSession:      "agent-1",
	})
	if got != "fix the flake" {
		t.Errorf("prompt(resume) = %q, want task only (session already has standing notes)", got)
	}
}

func TestCursorPromptKeepsTurnSubjectOnResume(t *testing.T) {
	r := &CursorRunner{}
	got := r.prompt(AgentSpec{
		AppendSystemPrompt: "This chat is in AGENT mode.",
		TurnAppendPrompt:   "The operator is looking at GitHub pull request #3.",
		Prompt:             "what changed?",
		ResumeSession:      "agent-1",
	})
	if strings.Contains(got, "AGENT mode") {
		t.Errorf("prompt(resume) must omit standing notes, got %q", got)
	}
	if !strings.Contains(got, "GitHub pull request #3") || !strings.Contains(got, "what changed?") {
		t.Errorf("prompt(resume) must keep per-turn subject, got %q", got)
	}
}

func TestCursorPromptRefreshStandingOnResume(t *testing.T) {
	r := &CursorRunner{}
	got := r.prompt(AgentSpec{
		SystemPrompt:                "You are the Coder.",
		AppendSystemPrompt:          "This chat is in PLAN mode.",
		Prompt:                      "continue",
		ResumeSession:               "agent-1",
		RefreshStandingInstructions: true,
	})
	if !strings.Contains(got, "You are the Coder.") || !strings.Contains(got, "PLAN mode") {
		t.Errorf("mode remount must re-attach standing notes: %q", got)
	}
	if !strings.Contains(got, "continue") {
		t.Errorf("task prompt dropped: %q", got)
	}
}

func TestCursorPromptUnchangedWithoutSystemPrompt(t *testing.T) {
	if got := (&CursorRunner{}).prompt(AgentSpec{Prompt: "Just do it."}); got != "Just do it." {
		t.Errorf("prompt = %q, want unchanged", got)
	}
}

func TestCursorPromptMapsAttachmentMarkersToImageOrdinals(t *testing.T) {
	r := &CursorRunner{}
	marker := AttachmentMarker("img_msgze3vv_f1gekn")
	body := "what is this image:\n\n" + marker + "\n\ndo you know?"
	got := r.prompt(AgentSpec{
		Prompt: body,
		Images: []UserImage{{ID: "img_msgze3vv_f1gekn", Mime: "image/png", Data: "AAAA"}},
	})
	if !strings.Contains(got, marker) {
		t.Fatalf("marker should stay for in-text location: %q", got)
	}
	if !strings.Contains(got, marker+" is image 1 (first attached image)") {
		t.Fatalf("missing ordinal mapping: %q", got)
	}
	if !strings.Contains(got, "NOT files to open or search") {
		t.Fatalf("missing anti-Glob guidance: %q", got)
	}
	if !strings.Contains(got, "what is this image:") || !strings.Contains(got, "do you know?") {
		t.Fatalf("surrounding text lost: %q", got)
	}
	// Without Images, no image instruction — markers still fine in body.
	kept := r.prompt(AgentSpec{Prompt: "see " + marker})
	if kept != "see "+marker {
		t.Fatalf("unexpected wrap without images: %q", kept)
	}
}

// --- event stream → ParsedEvent ---------------------------------------------

// feed replays frames through a turn. The returned slice is a snapshot taken
// when feed returns; use turn.emitted() after calling turn.finish() to see
// everything, since a turn buffers text and reasoning until a boundary.
func feed(t *testing.T, spec AgentSpec, frames ...string) (*cursorTurn, []ParsedEvent) {
	t.Helper()
	var got []ParsedEvent
	turn := &cursorTurn{spec: spec, agentID: "agent-1", emit: func(e ParsedEvent) { got = append(got, e) }}
	turn.seen = &got
	for _, f := range frames {
		var ev cursor.RunStreamEvent
		if err := json.Unmarshal([]byte(f), &ev); err != nil {
			t.Fatalf("decode %s: %v", f, err)
		}
		if err := turn.onEvent(ev); err != nil {
			t.Fatalf("onEvent: %v", err)
		}
	}
	return turn, got
}

const (
	fAssistant       = `{"sdkMessage":{"type":"assistant","message":{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}}}`
	fToolRun         = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c1","name":"mcp","status":"running","args":{"providerIdentifier":"recur","toolName":"done","args":{"summary":"finished the work"}}}}}`
	fToolOK          = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c1","name":"mcp","status":"completed","args":{"providerIdentifier":"recur","toolName":"done"},"result":{"status":"success","value":{"isError":false}}}}}`
	fToolBad         = `{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c2","name":"shell","status":"completed","args":{"command":"go test ./..."},"result":{"status":"success","value":{"isError":true}}}}}`
	fThinking        = `{"sdkMessage":{"type":"thinking","message":{"type":"thinking","text":"hmm"}}}`
	fUsage           = `{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":1000,"outputTokens":500,"cacheReadTokens":2000,"cacheWriteTokens":0,"totalTokens":3500}}}}`
	fResultOK        = `{"result":{"status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"all good","durationMs":"1500","usage":{"inputTokens":"1000","outputTokens":"500","cacheReadTokens":"2000","totalTokens":"3500"}}}}`
	fResultErr       = `{"result":{"status":"RUN_LIFECYCLE_STATUS_ERROR","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_ERROR","error":{"message":"model overloaded"}}}}`
	fResultCancelled = `{"result":{"status":"RUN_LIFECYCLE_STATUS_CANCELLED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_CANCELLED"}}}`
	fTaskSummary     = `{"sdkMessage":{"type":"task","message":{"type":"task","text":"older turns folded into a short brief"}}}`
)

func TestCursorTaskMessageIsNotCompact(t *testing.T) {
	// SDKTaskMessage is task/subagent status. cursor-sdk also maps summary
	// InteractionUpdates onto type "task" as a side channel; Recur must not
	// treat that as compaction — summaries arrive on onDelta after the minify patch.
	_, got := feed(t, AgentSpec{Model: "composer-2.5"}, fUsage, fTaskSummary, fResultOK)
	for _, ev := range got {
		if ev.Kind == KindCompact {
			t.Fatalf("SDKTaskMessage must not emit KindCompact: %+v", ev)
		}
	}
}

func TestCursorInteractionSummaryIsCompact(t *testing.T) {
	const (
		start = `{"interactionUpdate":{"type":"summary-started"}}`
		text  = `{"interactionUpdate":{"type":"summary","update":{"type":"summary","summary":"older turns folded into a short brief"}}}`
		done  = `{"interactionUpdate":{"type":"summary-completed"}}`
	)
	_, got := feed(t, AgentSpec{Model: "composer-2.5"}, fUsage, start, text, done, fResultOK)
	var doneEv ParsedEvent
	nStart, nDone := 0, 0
	for _, ev := range got {
		if ev.Kind != KindCompact {
			continue
		}
		switch ev.CompactStatus {
		case domain.CompactStarted:
			nStart++
		case domain.CompactDone:
			nDone++
			doneEv = ev
		}
	}
	if nStart != 1 || nDone != 1 {
		t.Fatalf("KindCompact started=%d done=%d, want 1 each", nStart, nDone)
	}
	if doneEv.Text != "older turns folded into a short brief" {
		t.Errorf("KindCompact text = %q, want older turns folded into a short brief", doneEv.Text)
	}
}

func TestCursorSecondCompactClearsPriorSummary(t *testing.T) {
	const (
		start1 = `{"interactionUpdate":{"type":"summary-started"}}`
		text1  = `{"interactionUpdate":{"type":"summary","update":{"type":"summary","summary":"first fold"}}}`
		done1  = `{"interactionUpdate":{"type":"summary-completed"}}`
		start2 = `{"interactionUpdate":{"type":"summary-started"}}`
		done2  = `{"interactionUpdate":{"type":"summary-completed"}}`
	)
	_, got := feed(t, AgentSpec{Model: "composer-2.5"}, fUsage, start1, text1, done1, start2, done2, fResultOK)
	var summaries []string
	for _, ev := range got {
		if ev.Kind == KindCompact && ev.CompactStatus == domain.CompactDone {
			summaries = append(summaries, ev.Text)
		}
	}
	if len(summaries) != 2 {
		t.Fatalf("KindCompact done count = %d, want 2; texts=%q", len(summaries), summaries)
	}
	if summaries[0] != "first fold" {
		t.Errorf("first compact text = %q, want first fold", summaries[0])
	}
	if summaries[1] != "" {
		t.Errorf("second compact text = %q, want empty (prior summary must not leak)", summaries[1])
	}
}

func TestCursorTurnCollectsRunIDsForCancel(t *testing.T) {
	// Status frames carry run_id immediately; Task sub-agents may add more.
	// CancelRun needs every id we saw, not only the terminal result's.
	const (
		statusParent = `{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-1","run_id":"run-parent","status":"RUNNING"}}}`
		statusChild  = `{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-1","run_id":"run-child","status":"RUNNING"}}}`
	)
	turn, _ := feed(t, AgentSpec{}, statusParent, statusChild, fResultOK)
	got := turn.allRunIDs()
	want := []string{"run-1", "run-child", "run-parent"}
	if len(got) != len(want) {
		t.Fatalf("runIDs=%v, want %v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("runIDs=%v, want %v", got, want)
		}
	}
}

func TestCursorTurnWaitRunID(t *testing.T) {
	turn := &cursorTurn{gotRunID: make(chan string, 1)}
	go func() {
		time.Sleep(20 * time.Millisecond)
		turn.noteRunID("run-late")
	}()
	if got := turn.waitRunID(time.Second); got != "run-late" {
		t.Fatalf("waitRunID=%q, want run-late", got)
	}
}

func TestCursorWaitRunIDCtxAndPrepareSteerFollowUpNoRace(t *testing.T) {
	turn := &cursorTurn{gotRunID: make(chan string, 1)}
	done := make(chan struct{})
	go func() {
		defer close(done)
		_ = turn.waitRunIDCtx(context.Background(), 100*time.Millisecond)
	}()
	time.Sleep(10 * time.Millisecond)
	turn.prepareSteerFollowUp()
	turn.noteRunID("run-follow")
	<-done
}

func TestCursorWaitRunIDAfterPrepareSteerFollowUp(t *testing.T) {
	turn := &cursorTurn{gotRunID: make(chan string, 1)}
	turn.noteRunID("run-old")
	turn.prepareSteerFollowUp()
	go func() {
		time.Sleep(20 * time.Millisecond)
		turn.noteRunID("run-follow")
	}()
	if got := turn.waitRunID(time.Second); got != "run-follow" {
		t.Fatalf("waitRunID=%q, want run-follow after drain", got)
	}
}

func TestCursorTurnEmitsStepAndToolResult(t *testing.T) {
	_, got := feed(t, AgentSpec{}, fToolRun, fToolOK)
	if len(got) != 2 {
		t.Fatalf("got %d events, want 2: %+v", len(got), got)
	}
	if got[0].Kind != KindStep {
		t.Errorf("first = %q, want %q", got[0].Kind, KindStep)
	}
	// The engine's role scoping and feed both key off mcp__server__tool.
	// Raw on the event (source of truth), common at the display boundary.
	if got[0].Tool != "mcp" {
		t.Errorf("stored tool = %q, want Cursor's raw mcp", got[0].Tool)
	}
	if d := displayToolFor(got[0]); d.Name != "mcp__recur__done" {
		t.Errorf("display name = %q, want mcp__recur__done", d.Name)
	}
	if got[0].ToolInput != "finished the work" {
		t.Errorf("tool input = %q, want the summary", got[0].ToolInput)
	}
	if got[0].ToolUseID != "c1" {
		t.Errorf("toolUseID = %q, want c1", got[0].ToolUseID)
	}
	if got[1].Kind != KindToolResult || got[1].IsError {
		t.Errorf("second = %+v, want a clean tool result", got[1])
	}
}

func TestCursorTurnFlagsFailedTool(t *testing.T) {
	// A lone terminal frame yields the call AND its result, so the transcript
	// never holds a result pointing at a tool_use block that was never written.
	_, got := feed(t, AgentSpec{}, fToolBad)
	if len(got) != 2 || got[0].Kind != KindStep || got[1].Kind != KindToolResult {
		t.Fatalf("got %+v, want a step followed by its result", got)
	}
	res := got[1]
	// isError lives under a protocol-level "success", so a naive read misses it.
	if !res.IsError {
		t.Error("tool result with isError:true not flagged")
	}
	// The event keeps Cursor's own name; the card shows the common one.
	if res.Tool != "shell" {
		t.Errorf("stored tool = %q, want Cursor's raw shell", res.Tool)
	}
	if d := displayToolFor(got[0]); d.Name != "Bash" {
		t.Errorf("display name = %q, want Bash", d.Name)
	}
}

func TestCursorTurnEmitsAssistantText(t *testing.T) {
	// Cursor streams a reply in chunks, so text buffers until a boundary; the
	// end of the turn is one.
	turn, _ := feed(t, AgentSpec{}, fAssistant)
	turn.finish()
	var blocks []ParsedEvent
	for _, e := range turn.emitted() {
		if e.Kind == KindText {
			blocks = append(blocks, e)
		}
	}
	if len(blocks) != 1 || blocks[0].Text != "done" {
		t.Errorf("got %+v, want one KindText 'done'", blocks)
	}
}

func TestCursorTurnThinkingOnlyWithPartials(t *testing.T) {
	// Token-level chatter must not flood the activity feed for normal roles.
	if _, got := feed(t, AgentSpec{}, fThinking); len(got) != 0 {
		t.Errorf("thinking leaked into the feed: %+v", got)
	}
	// The Maestro chat opts in — as THINKING deltas, so reasoning streams to the
	// thinking pane instead of being spliced into the assistant's reply.
	_, got := feed(t, AgentSpec{IncludePartials: true}, fThinking)
	if len(got) != 1 || got[0].Kind != KindThinkingDelta {
		t.Errorf("Maestro chat should see thinking deltas, got %+v", got)
	}
}

func TestCursorTurnEmitsMidTurnUsage(t *testing.T) {
	_, got := feed(t, AgentSpec{}, fUsage)
	if len(got) != 1 || got[0].Kind != KindUsage || got[0].Result == nil {
		t.Fatalf("want KindUsage, got %+v", got)
	}
	r := got[0].Result
	if r.InputTokens != 1000 || r.CacheReadTokens != 2000 {
		t.Errorf("usage tokens = in=%d cacheRead=%d", r.InputTokens, r.CacheReadTokens)
	}
}

func TestCursorTurnResultSuccess(t *testing.T) {
	turn, _ := feed(t, AgentSpec{Model: "composer-2.5"}, fAssistant, fUsage, fResultOK)
	res := turn.result()
	if res.IsError || res.Subtype != "success" {
		t.Errorf("result = %+v, want success", res)
	}
	if res.Text != "all good" {
		t.Errorf("text = %q, want 'all good'", res.Text)
	}
	if res.DurationMS != 1500 {
		t.Errorf("durationMs = %d, want 1500 (string-encoded int64)", res.DurationMS)
	}
	// SessionID carries the agentId so the next turn can ResumeAgent.
	if res.SessionID != "agent-1" {
		t.Errorf("sessionID = %q, want agent-1", res.SessionID)
	}
	// Token fields power the context meter; the window is filled by the
	// mirroringRunner from Cursor flags (default 200k) before stamp/broadcast.
	if res.InputTokens != 1000 || res.OutputTokens != 500 || res.CacheReadTokens != 2000 {
		t.Errorf("tokens = in=%d out=%d cacheRead=%d", res.InputTokens, res.OutputTokens, res.CacheReadTokens)
	}
}

func TestCursorTurnSuppressesCancelledResultWhenSteering(t *testing.T) {
	var got []ParsedEvent
	turn := &cursorTurn{
		spec:    AgentSpec{},
		agentID: "agent-1",
		emit:    func(e ParsedEvent) { got = append(got, e) },
	}
	turn.setSuppressResult(true)
	var ev cursor.RunStreamEvent
	if err := json.Unmarshal([]byte(fResultCancelled), &ev); err != nil {
		t.Fatalf("decode fResultCancelled: %v", err)
	}
	if err := turn.onEvent(ev); err != nil {
		t.Fatalf("onEvent: %v", err)
	}
	for _, e := range got {
		if e.Kind == KindResult {
			t.Fatalf("onEvent(cancelled) emitted KindResult during steer, got %+v", e)
		}
	}
	if turn.haveResult {
		t.Fatal("onEvent(cancelled) left haveResult true, want false so Run can follow up")
	}
}

func TestCursorTurnEmitsFinishedResultEvenWhenSteerPending(t *testing.T) {
	var got []ParsedEvent
	turn := &cursorTurn{
		spec:    AgentSpec{Model: "composer-2.5"},
		agentID: "agent-1",
		emit:    func(e ParsedEvent) { got = append(got, e) },
	}
	turn.setSuppressResult(true)
	var ev cursor.RunStreamEvent
	if err := json.Unmarshal([]byte(fResultOK), &ev); err != nil {
		t.Fatalf("decode fResultOK: %v", err)
	}
	if err := turn.onEvent(ev); err != nil {
		t.Fatalf("onEvent: %v", err)
	}
	var saw bool
	for _, e := range got {
		if e.Kind == KindResult {
			saw = true
		}
	}
	if !saw {
		t.Fatal("onEvent(FINISHED) omitted KindResult while suppressResult was set")
	}
	if !turn.haveResult {
		t.Fatal("haveResult = false after FINISHED, want true (late Inject must not follow up)")
	}
}

func TestCursorTurnResultError(t *testing.T) {
	turn, _ := feed(t, AgentSpec{}, fResultErr)
	res := turn.result()
	if !res.IsError {
		t.Error("errored run not flagged")
	}
	if res.Subtype != "error_during_execution" {
		t.Errorf("subtype = %q", res.Subtype)
	}
	// The engine distinguishes transient from genuine failures via Stderr.
	if !strings.Contains(res.Stderr, "model overloaded") {
		t.Errorf("stderr = %q, want the provider message", res.Stderr)
	}
}

// The real bridge (cursor-sdk-bridge 1.0.0 / sdk ≤1.0.37) never puts `error`
// on the wire RunResult — the proto has no such field. The provider's reason
// arrives earlier, as SDKStatusMessage{status:"ERROR", message}, and the
// terminal frame is a bare ERROR. Recur must stitch the two together or the
// chat shows only "error_during_execution".
func TestCursorTurnStatusErrorFillsBareResult(t *testing.T) {
	// Verbatim: Cursor's backend sent resource_exhausted with no detail text.
	const fStatusErr = `{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-1","run_id":"run-1","status":"ERROR","message":"[resource_exhausted] Error"}}}`
	const fResultBareErr = `{"result":{"status":"RUN_LIFECYCLE_STATUS_ERROR","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_ERROR"}}}`
	turn, _ := feed(t, AgentSpec{}, fStatusErr, fResultBareErr)
	res := turn.result()
	if !res.IsError || res.Subtype != "error_during_execution" {
		t.Fatalf("got %+v, want error_during_execution", res)
	}
	if !strings.HasPrefix(res.Stderr, "[resource_exhausted] Error. ") || !strings.Contains(res.Stderr, "rate-limited") {
		t.Errorf("stderr = %q, want the provider's words plus the rate-limit hint", res.Stderr)
	}
	if reason, transient := classifyAgentError(nil, res); !transient || reason != res.Stderr {
		t.Errorf("classify = (%q, %v), want (stderr, transient)", reason, transient)
	}
}

func TestCursorTurnMissingResultIsError(t *testing.T) {
	turn, _ := feed(t, AgentSpec{}, fAssistant)
	// A stream that dies mid-reply still flushes, so the partial text survives.
	turn.finish()
	res := turn.result()
	if !res.IsError || res.Subtype != "error_during_execution" {
		t.Errorf("a stream with no result must be an error, got %+v", res)
	}
	// Text seen before the stream died is still worth reporting.
	if res.Text != "done" {
		t.Errorf("text = %q, want the last assistant text", res.Text)
	}
}

func TestCursorTurnUsesMidStreamUsageWhenResultMissing(t *testing.T) {
	turn, _ := feed(t, AgentSpec{Model: "composer-2.5"}, fUsage)
	res := turn.result()
	if res.InputTokens == 0 && res.CacheReadTokens == 0 {
		t.Error("a run that died after reporting usage should still meter tokens")
	}
}

// --- runner contract ---------------------------------------------------------

func TestCursorRunnerImplementsAgentRunner(t *testing.T) {
	var _ AgentRunner = (*CursorRunner)(nil)
}

func TestCursorRunnerWithoutBridgeFailsClearly(t *testing.T) {
	_, err := (&CursorRunner{}).Run(t.Context(), AgentSpec{Role: domain.RoleCoder}, nil)
	if err == nil || !strings.Contains(err.Error(), "bridge not started") {
		t.Errorf("err = %v, want a clear 'bridge not started'", err)
	}
}

func TestCursorNoResultMessage(t *testing.T) {
	const hint = " (bridge log: /tmp/bridge.log)"
	startup := "recur: starting cursor-sdk-bridge bin=/tmp/cursor-sdk-bridge workspace=/tmp/wt\n" +
		`cursor-sdk-bridge ready {"authTokenFile":"/tmp/auth-token","host":"127.0.0.1","pid":20136,"port":54632,"protocol":"connect"}` + "\n" +
		"(node:20136) ExperimentalWarning: SQLite is an experimental feature\n" +
		"cursor-sdk-bridge SIGTERM received, shutting down\n" +
		"(bridge log: /tmp/bridge.log)"

	got := cursorNoResultMessage(startup, hint)
	if strings.Contains(got, "authTokenFile") || strings.Contains(got, "starting cursor-sdk-bridge") {
		t.Fatalf("cursorNoResultMessage dumped startup stderr: %q", got)
	}
	if !strings.Contains(got, "stopped mid-turn") || !strings.Contains(got, "/tmp/bridge.log") {
		t.Fatalf("cursorNoResultMessage(%q, %q) = %q, want mid-turn stop and log path", startup, hint, got)
	}

	pathOnly := cursorNoResultMessage("", hint)
	if pathOnly != "cursor run ended without a result"+hint {
		t.Fatalf("cursorNoResultMessage(\"\", hint) = %q, want generic + log path", pathOnly)
	}

	crash := "Error: spawn /bin/zsh ENOENT\n    at ChildProcess._handle.onexit"
	got = cursorNoResultMessage(crash, hint)
	if !strings.Contains(got, "Shell spawn ENOENT") {
		t.Fatalf("cursorNoResultMessage(ENOENT) = %q, want Shell spawn ENOENT", got)
	}
	if strings.Contains(got, "ChildProcess") {
		t.Fatalf("cursorNoResultMessage(ENOENT) = %q, want no stack dump", got)
	}
}

func TestBridgeTailLooksLikeShellSpawnCrash(t *testing.T) {
	crash := "Error: spawn /bin/zsh ENOENT\n    at ChildProcess._handle.onexit"
	if !bridgeTailLooksLikeShellSpawnCrash(crash) {
		t.Fatal("expected crash signature to match")
	}
	// Non-fatal Playwright launch noise must not trip the Shell-crash hint,
	// even when ENOENT appears elsewhere in the ring buffer.
	stale := "spawn npx ENOENT\nsome later unrelated line\ncode: 'ENOENT'"
	if bridgeTailLooksLikeShellSpawnCrash(stale) {
		t.Fatal("spawn npx ENOENT across lines must not match")
	}
	if bridgeTailLooksLikeShellSpawnCrash("spawn /bin/zsh\nENOENT elsewhere") {
		t.Fatal("tokens on different lines must not match")
	}
}

func TestCursorPrepareSteerFollowUpResetsPerRunState(t *testing.T) {
	turn, _ := feed(t, AgentSpec{Model: "composer-2.5"}, fToolRun)
	turn.toolWaves = 6
	turn.waveOpen = true
	turn.compactBaseline = &cursor.TokenUsage{InputTokens: 9000}
	turn.compactPreFill = 42
	turn.compactSummary = "old"
	turn.usage = &cursor.TokenUsage{InputTokens: 1000}
	turn.usageCumulative = &cursor.TokenUsage{InputTokens: 1000}
	turn.haveResult = true
	turn.res = cursor.RunResult{Result: "partial"}
	turn.noteRunID("run-old")
	if turn.modelRounds() != 7 {
		t.Fatalf("modelRounds before reset = %d, want 7", turn.modelRounds())
	}

	turn.prepareSteerFollowUp()

	if turn.toolWaves != 0 || turn.waveOpen {
		t.Errorf("waves after follow-up: toolWaves=%d waveOpen=%v", turn.toolWaves, turn.waveOpen)
	}
	if turn.modelRounds() != 1 {
		t.Errorf("modelRounds after reset = %d, want 1 (follow-up must not inherit run 1 waves)", turn.modelRounds())
	}
	if turn.compactBaseline != nil || turn.compactPreFill != 0 || turn.compactSummary != "" {
		t.Errorf("compact state leaked: baseline=%v preFill=%d summary=%q", turn.compactBaseline, turn.compactPreFill, turn.compactSummary)
	}
	if turn.steppedCalls != nil || turn.createPlanArgs != nil {
		t.Errorf("call dedupe leaked: stepped=%v createPlan=%v", turn.steppedCalls, turn.createPlanArgs)
	}
	if len(turn.openCalls) != 0 || turn.openTool != nil {
		t.Errorf("open tools leaked: calls=%v tool=%v", turn.openCalls, turn.openTool)
	}
	if turn.usage != nil || turn.usageCumulative != nil || turn.haveResult {
		t.Errorf("usage/result leaked: usage=%v cumulative=%v haveResult=%v", turn.usage, turn.usageCumulative, turn.haveResult)
	}
	if ids := turn.allRunIDs(); len(ids) != 0 {
		t.Errorf("runIDs leaked: %v", ids)
	}

	// Same Cursor call_id on the follow-up run must mint a new KindStep.
	before := len(turn.emitted())
	var ev cursor.RunStreamEvent
	if err := json.Unmarshal([]byte(fToolRun), &ev); err != nil {
		t.Fatalf("decode: %v", err)
	}
	if err := turn.onEvent(ev); err != nil {
		t.Fatalf("onEvent: %v", err)
	}
	var sawStep bool
	for _, e := range turn.emitted()[before:] {
		if e.Kind == KindStep && e.ToolUseID == "c1" {
			sawStep = true
		}
	}
	if !sawStep {
		t.Fatal("follow-up tool with reused call_id was deduped; steppedCalls must reset")
	}
}

func TestCursorPrepareSteerFollowUpKeepsSuppressResult(t *testing.T) {
	turn := &cursorTurn{spec: AgentSpec{}, emit: func(ParsedEvent) {}}
	turn.setSuppressResult(true)
	turn.prepareSteerFollowUp()
	if !turn.suppressingResult() {
		t.Fatal("prepareSteerFollowUp cleared suppressResult; a steer in the take/follow-up window would emit error_cancelled")
	}
}

func TestCursorPrepareSteerFollowUpBanksDurationWithoutCancelledFrame(t *testing.T) {
	turn := &cursorTurn{spec: AgentSpec{Model: "composer-2.5"}, emit: func(ParsedEvent) {}}
	turn.usage = &cursor.TokenUsage{InputTokens: 1000, OutputTokens: 500, CacheReadTokens: 2000}
	turn.res.DurationMS = 1500
	turn.prepareSteerFollowUp()
	if turn.usage != nil {
		t.Fatal("prepareSteerFollowUp left usage set")
	}
	if turn.priorDurationMS != 1500 {
		t.Errorf("priorDurationMS = %d, want 1500", turn.priorDurationMS)
	}
	if got := turn.result().DurationMS; got != 1500 {
		t.Errorf("result() DurationMS = %d, want 1500", got)
	}
}

func TestCursorBankSuppressedDurationDoesNotDoubleCount(t *testing.T) {
	turn := &cursorTurn{spec: AgentSpec{Model: "composer-2.5"}}
	turn.usage = &cursor.TokenUsage{InputTokens: 1000, OutputTokens: 500, CacheReadTokens: 2000}
	turn.res.DurationMS = 1500
	turn.bankSuppressedDuration()
	if turn.priorDurationMS != 1500 {
		t.Errorf("first bank duration = %d, want 1500", turn.priorDurationMS)
	}
	if turn.usage != nil {
		t.Fatal("bankSuppressedDuration left usage set")
	}

	// Second steer: follow-up cancelled before any usage frame (and no res.Usage).
	turn.res = cursor.RunResult{}
	turn.bankSuppressedDuration()
	if turn.priorDurationMS != 1500 {
		t.Errorf("second bank re-added duration: priorDurationMS=%d, want 1500", turn.priorDurationMS)
	}

	if got := turn.result().DurationMS; got != 1500 {
		t.Errorf("result() DurationMS = %d, want 1500 (pre-steer wall time kept)", got)
	}
}

func TestCursorTurnDoesNotSuppressProviderError(t *testing.T) {
	var got []ParsedEvent
	turn := &cursorTurn{
		spec:    AgentSpec{},
		agentID: "agent-1",
		emit:    func(e ParsedEvent) { got = append(got, e) },
	}
	turn.setSuppressResult(true)
	var ev cursor.RunStreamEvent
	if err := json.Unmarshal([]byte(fResultErr), &ev); err != nil {
		t.Fatalf("decode fResultErr: %v", err)
	}
	if err := turn.onEvent(ev); err != nil {
		t.Fatalf("onEvent: %v", err)
	}
	var saw bool
	for _, e := range got {
		if e.Kind == KindResult {
			saw = true
			if e.Result == nil || !strings.Contains(e.Result.Stderr, "model overloaded") {
				t.Errorf("KindResult dropped the provider error: %+v", e)
			}
		}
	}
	if !saw {
		t.Fatal("onEvent(ERROR) omitted KindResult while suppressResult was set")
	}
	if !turn.haveResult {
		t.Fatal("haveResult = false after ERROR; Run would start a follow-up instead of surfacing it")
	}
}

func TestCursorPrepareSteerFollowUpClosesOpenCallsAsCancelled(t *testing.T) {
	turn, _ := feed(t, AgentSpec{}, fToolRun)
	turn.prepareSteerFollowUp()
	var saw bool
	for _, e := range turn.emitted() {
		if e.Kind != KindToolResult || e.ToolUseID != "c1" {
			continue
		}
		saw = true
		if e.IsError {
			t.Error("steer-cancelled tool result has IsError; cancelled is not a failure")
		}
		if e.Text != steerCancelledCallText {
			t.Errorf("tool result text = %q, want steer-cancelled copy", e.Text)
		}
		if e.Text == unreportedResultText {
			t.Error("steer cancel used unexplained-failure copy")
		}
	}
	if !saw {
		t.Fatal("prepareSteerFollowUp did not emit KindToolResult for the open call")
	}
}

func TestCursorWirePromptAddsSubagentOverlayOnlyWithCreateWorkspace(t *testing.T) {
	r := &CursorRunner{}
	agent := r.wirePrompt(AgentSpec{Prompt: "hi", AllowedTools: []string{"Read", mcpCreateWorkspace}}, true)
	if !strings.Contains(agent, cursorSubagentPrompt) || !strings.Contains(agent, "<role-instructions>") {
		t.Fatalf("agent turn missing subagent overlay:\n%s", agent)
	}
	if strings.Contains(agent, cursorChildSubagentPrompt) {
		t.Fatalf("parent turn must not carry the child execute-locally overlay:\n%s", agent)
	}
	child := r.wirePrompt(AgentSpec{Prompt: "hi", ChatChild: true, AllowedTools: []string{"Read"}}, true)
	if !strings.Contains(child, cursorChildSubagentPrompt) || strings.Contains(child, "beyond a very simple task") {
		t.Fatalf("child turn missing execute-locally overlay:\n%s", child)
	}
	readonly := r.wirePrompt(AgentSpec{Prompt: "hi", AllowedTools: []string{"Read"}}, true)
	if strings.Contains(readonly, cursorSubagentPrompt) || strings.Contains(readonly, cursorChildSubagentPrompt) {
		t.Fatalf("read-only turn must not carry the subagent overlay:\n%s", readonly)
	}
	if follow := r.wirePrompt(AgentSpec{Prompt: "hi", AllowedTools: []string{mcpCreateWorkspace}}, false); follow != "hi" {
		t.Fatalf("follow-up = %q, want bare body", follow)
	}
}
