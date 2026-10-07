package engine

import (
	"cmp"
	"context"
	"encoding/json"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"recur/internal/cursor"
	"recur/internal/domain"
)

// TestCursorToolRenderingSmoke drives a REAL Cursor run that exercises the
// built-in tools, and asserts the events carry everything the activity feed and
// tool cards need: canonical names, canonical inputs, and non-empty results.
//
// The first Cursor build emitted bare tool events — right name-ish, but no
// ToolInputRaw and no result text — so every tool card rendered as an empty row
// and the transcript stored calls with no arguments or output.
func TestCursorToolRenderingSmoke(t *testing.T) {
	bridgePath := os.Getenv("RECUR_CURSOR_BRIDGE")
	key := os.Getenv("CURSOR_API_KEY")
	if bridgePath == "" || key == "" {
		t.Skip("RECUR_CURSOR_BRIDGE and CURSOR_API_KEY required")
	}
	bin := cursor.ResolveLauncher(bridgePath)
	if bin == "" {
		t.Fatalf("not a cursor-sdk-bridge: %s", bridgePath)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
	defer cancel()

	worktree := t.TempDir()
	if err := os.WriteFile(filepath.Join(worktree, "hello.txt"), []byte("alpha\nbravo\ncharlie\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	b := &cursor.Bridge{Bin: bin, APIKey: key, Workspace: worktree}
	if err := b.Start(ctx); err != nil {
		t.Fatalf("start bridge: %v", err)
	}
	defer func() { _ = b.Close() }()

	steps := map[string]*toolStep{} // by ToolUseID
	stepCount := map[string]int{}   // by ToolUseID — duplicates orphan the result

	_, err := NewCursorRunner(b, key).Run(ctx, AgentSpec{
		WorkspaceID: "LOOP-TOOLS",
		AgentID:     "LOOP-TOOLS-CO",
		Role:        domain.RoleCoder,
		Model:       cmp.Or(os.Getenv("RECUR_CURSOR_TEST_MODEL"), "composer-2.5"),
		Worktree:    worktree,
		Prompt: "Do exactly these, each as its own tool call: read hello.txt; " +
			"replace 'bravo' with 'BRAVO' in hello.txt; run `echo hi` in the shell. Then reply DONE.",
	}, func(ev ParsedEvent) {
		switch ev.Kind {
		case KindStep:
			stepCount[ev.ToolUseID]++
			d := displayToolFor(ev)
			steps[ev.ToolUseID] = &toolStep{
				raw: ev.Tool, rawInput: string(ev.ToolInputRaw),
				tool: d.Name, inputRaw: string(d.Input), summary: ev.ToolInput,
			}
		case KindToolResult:
			if s, ok := steps[ev.ToolUseID]; ok {
				s.rawResult = ev.Text
				s.result = displayToolResult(ev.Provider, ev.Tool, ev.Text)
				s.isErr = ev.IsError
			}
		}
	})
	if err != nil {
		t.Fatalf("Run: %v", err)
	}

	byTool := map[string]*toolStep{}
	for _, s := range steps {
		byTool[s.tool] = s
		t.Logf("raw=%-8s → %-6s | rawArgs=%s | input=%s | result=%.40q",
			s.raw, s.tool, s.rawInput, s.inputRaw, s.result)
	}

	// Cursor's lowercase names must have been mapped to Recur's vocabulary, or
	// the cards fall through to a generic row with a plug icon.
	for _, want := range []string{"Read", "Edit", "Bash"} {
		if byTool[want] == nil {
			t.Errorf("no %s step seen (tools: %v)", want, keysOf(byTool))
		}
	}
	// Exactly one step per call: Cursor re-reports progress for some tools, and a
	// duplicate step mints a second tool_use block that the result never reaches.
	if len(stepCount) != len(steps) {
		t.Errorf("step counts %v — every call must produce exactly one step", stepCount)
	}
	for id, n := range stepCount {
		if n != 1 {
			t.Errorf("call %s produced %d steps, want 1", id, n)
		}
	}
	// Every call must end with a result, or its card renders as a bare input.
	for id, s := range steps {
		if s.rawResult == "" {
			t.Errorf("call %s (%s) got no result — its card would show only the input", id, s.raw)
		}
	}

	// The provider's own vocabulary must survive on the event, since that is what
	// the session store records as the source of truth.
	for _, s := range steps {
		if s.raw == "" || s.raw == s.tool {
			t.Errorf("event lost the provider's raw tool name: %+v", s)
		}
		if s.rawResult != "" && !strings.Contains(s.rawResult, `"status"`) {
			t.Errorf("event lost Cursor's raw result envelope: %.80q", s.rawResult)
		}
	}

	if s := byTool["Read"]; s != nil {
		if !strings.Contains(s.inputRaw, "file_path") {
			t.Errorf("Read input = %s, want a canonical file_path", s.inputRaw)
		}
		if s.result == "" {
			t.Error("Read result is empty — the card would show no file content")
		}
	}
	if s := byTool["Bash"]; s != nil {
		if !strings.Contains(s.inputRaw, "command") {
			t.Errorf("Bash input = %s, want a canonical command", s.inputRaw)
		}
		if s.summary == "" {
			t.Error("Bash has no input summary — the feed row would be blank")
		}
		if !strings.Contains(s.result, "hi") {
			t.Errorf("Bash result = %q, want the command stdout", s.result)
		}
	}
	if s := byTool["Edit"]; s != nil {
		if !strings.Contains(s.inputRaw, "file_path") {
			t.Errorf("Edit input = %s, want a canonical file_path", s.inputRaw)
		}
		// Cursor puts the change itself in the RESULT; without it the Edit card
		// has no diff to render at all.
		if !strings.Contains(s.result, "@@") {
			t.Errorf("Edit result = %q, want a unified diff", s.result)
		}
	}
}

// toolStep is one tool call as the activity feed would receive it.
type toolStep struct {
	raw       string // the provider's own tool name (what gets stored)
	rawInput  string // the provider's verbatim args
	rawResult string // the provider's verbatim result envelope
	tool      string // the common name (what the UI shows)
	inputRaw  string // the common input shape
	summary   string
	result    string
	isErr     bool
}

func keysOf(m map[string]*toolStep) []string {
	out := make([]string, 0, len(m))
	for k := range m {
		out = append(out, k)
	}
	return out
}

// TestCursorStreamingTranscriptSmoke drives a REAL Cursor run whose reply is
// long enough to arrive in multiple assistant frames, and asserts the stored
// transcript reassembles it into coherent blocks.
//
// Cursor streams a reply as a run of CHUNKS, not one complete message. Storing a
// block per frame split replies mid-sentence and mid-markdown — a fenced code
// block opened in one block and closed in the next — so the reloaded transcript
// rendered broken fragments that looked nothing like the live stream.
func TestCursorStreamingTranscriptSmoke(t *testing.T) {
	bridgePath := os.Getenv("RECUR_CURSOR_BRIDGE")
	key := os.Getenv("CURSOR_API_KEY")
	if bridgePath == "" || key == "" {
		t.Skip("RECUR_CURSOR_BRIDGE and CURSOR_API_KEY required")
	}
	bin := cursor.ResolveLauncher(bridgePath)
	if bin == "" {
		t.Fatalf("not a cursor-sdk-bridge: %s", bridgePath)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
	defer cancel()

	worktree := t.TempDir()
	b := &cursor.Bridge{Bin: bin, APIKey: key, Workspace: worktree}
	if err := b.Start(ctx); err != nil {
		t.Fatalf("start bridge: %v", err)
	}
	defer func() { _ = b.Close() }()

	var (
		blocks []string
		deltas int
		order  []ParsedKind
	)
	res, err := NewCursorRunner(b, key).Run(ctx, AgentSpec{
		WorkspaceID: "LOOP-STREAM",
		AgentID:     "LOOP-STREAM-CO",
		Role:        domain.RoleCoder,
		Model:       cmp.Or(os.Getenv("RECUR_CURSOR_TEST_MODEL"), "composer-2.5"),
		Worktree:    worktree,
		Prompt: "Do not use any tools. Reply with a short guide that contains " +
			"a fenced bash code block running `go run .`, a bold **Controls:** line, " +
			"and a closing sentence.",
	}, func(ev ParsedEvent) {
		switch ev.Kind {
		case KindText:
			blocks = append(blocks, ev.Text)
			order = append(order, ev.Kind)
		case KindTextDelta:
			deltas++
		case KindStep, KindToolResult, KindThinking:
			order = append(order, ev.Kind)
		}
	})
	if err != nil {
		t.Fatalf("Run: %v", err)
	}

	joined := strings.Join(blocks, "")
	t.Logf("blocks=%d deltas=%d order=%v", len(blocks), deltas, order)
	t.Logf("reply:\n%s", joined)

	if len(blocks) == 0 {
		t.Fatal("no text block stored — the reply would vanish from the transcript")
	}
	// Chunks must still stream, or the reply only appears once the turn ends.
	if deltas == 0 {
		t.Error("no text deltas emitted — nothing would stream live")
	}
	// Every code fence must open AND close inside ONE block. A fence split across
	// blocks is exactly what produced the broken markdown on reload.
	for i, b := range blocks {
		if strings.Count(b, "```")%2 != 0 {
			t.Errorf("block %d has an unbalanced code fence — markdown will break on reload:\n%s", i, b)
		}
	}
	// The turn's Result must carry the whole reply, not the last fragment.
	if res.Text == "" {
		t.Error("Result.Text is empty")
	}
	if len(res.Text) < len(joined)/2 {
		t.Errorf("Result.Text (%d chars) is much shorter than the streamed reply (%d) — likely a fragment",
			len(res.Text), len(joined))
	}
}

// TestCursorTodoRenderingSmoke drives a REAL Cursor run that maintains a todo
// list, and asserts it reaches the UI as a checklist rather than a raw JSON
// dump ({"todos":[{"content":"Create tetromino pieces and board lo…).
func TestCursorTodoRenderingSmoke(t *testing.T) {
	bridgePath := os.Getenv("RECUR_CURSOR_BRIDGE")
	key := os.Getenv("CURSOR_API_KEY")
	if bridgePath == "" || key == "" {
		t.Skip("RECUR_CURSOR_BRIDGE and CURSOR_API_KEY required")
	}
	bin := cursor.ResolveLauncher(bridgePath)
	if bin == "" {
		t.Fatalf("not a cursor-sdk-bridge: %s", bridgePath)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
	defer cancel()

	worktree := t.TempDir()
	b := &cursor.Bridge{Bin: bin, APIKey: key, Workspace: worktree}
	if err := b.Start(ctx); err != nil {
		t.Fatalf("start bridge: %v", err)
	}
	defer func() { _ = b.Close() }()

	var todoSteps []ParsedEvent
	_, err := NewCursorRunner(b, key).Run(ctx, AgentSpec{
		WorkspaceID: "LOOP-PLAN",
		AgentID:     "LOOP-PLAN-CO",
		Role:        domain.RoleCoder,
		Model:       cmp.Or(os.Getenv("RECUR_CURSOR_TEST_MODEL"), "composer-2.5"),
		Worktree:    worktree,
		Prompt: "Create a todo list of 3 steps for building a CLI app, then update it " +
			"marking the first completed and the second in progress. Create no files. Reply DONE.",
	}, func(ev ParsedEvent) {
		if ev.Kind == KindStep && displayToolFor(ev).Name == "TodoWrite" {
			todoSteps = append(todoSteps, ev)
		}
	})
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if len(todoSteps) == 0 {
		t.Skip("model did not use the todo tool this run")
	}

	last := todoSteps[len(todoSteps)-1]
	d := displayToolFor(last)
	t.Logf("input=%s", d.Input)
	t.Logf("row summary=%q", last.ToolInput)

	var got struct {
		Todos []struct {
			Content string `json:"content"`
			Status  string `json:"status"`
		} `json:"todos"`
	}
	if err := json.Unmarshal(d.Input, &got); err != nil {
		t.Fatalf("todo input is not the checklist shape: %s", d.Input)
	}
	if len(got.Todos) == 0 {
		t.Fatal("todo list has no items — the card would fall back to a JSON dump")
	}
	for i, td := range got.Todos {
		if td.Content == "" {
			t.Errorf("todo %d has no content", i)
		}
		// Cursor's camelCase must be gone, or the card reads running items as
		// pending.
		switch td.Status {
		case "pending", "in_progress", "completed":
		default:
			t.Errorf("todo %d status = %q, want the common vocabulary", i, td.Status)
		}
	}
	// The collapsed row must be readable, not a truncated JSON blob.
	if strings.Contains(last.ToolInput, "{") || strings.Contains(last.ToolInput, "\"") {
		t.Errorf("row summary is raw JSON: %q", last.ToolInput)
	}
	if last.ToolInput == "" {
		t.Error("row summary is empty")
	}
}

// TestCursorFailedToolCallSmoke drives a REAL Cursor run whose tool calls FAIL —
// reading files that do not exist — and asserts every call still ends with a
// result.
//
// Cursor sends `running` and then simply stops: no terminal frame, no error,
// and the run finishes successfully. The failure appears only in the model's
// prose. Without closing these client-side the transcript keeps a tool_use with
// no tool_result, and the card falls back to dumping {"file_path": …}.
func TestCursorFailedToolCallSmoke(t *testing.T) {
	bridgePath := os.Getenv("RECUR_CURSOR_BRIDGE")
	key := os.Getenv("CURSOR_API_KEY")
	if bridgePath == "" || key == "" {
		t.Skip("RECUR_CURSOR_BRIDGE and CURSOR_API_KEY required")
	}
	bin := cursor.ResolveLauncher(bridgePath)
	if bin == "" {
		t.Fatalf("not a cursor-sdk-bridge: %s", bridgePath)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
	defer cancel()

	// Deliberately EMPTY: every read below must fail.
	worktree := t.TempDir()
	b := &cursor.Bridge{Bin: bin, APIKey: key, Workspace: worktree}
	if err := b.Start(ctx); err != nil {
		t.Fatalf("start bridge: %v", err)
	}
	defer func() { _ = b.Close() }()

	steps := map[string]string{}   // call id -> tool
	results := map[string]string{} // call id -> result text
	errs := map[string]bool{}
	open := map[string]bool{} // calls awaiting a result
	n := 0
	firstUnresolvedAfter := -1

	_, err := NewCursorRunner(b, key).Run(ctx, AgentSpec{
		WorkspaceID: "LOOP-FAILREAD",
		AgentID:     "LOOP-FAILREAD-CO",
		Role:        domain.RoleCoder,
		Model:       cmp.Or(os.Getenv("RECUR_CURSOR_TEST_MODEL"), "composer-2.5"),
		Worktree:    worktree,
		Prompt: "First read go.mod (it may not exist). Then list the directory with a " +
			"glob. Then run `echo done` in the shell. Then say what you found. Create nothing.",
	}, func(ev ParsedEvent) {
		n++
		switch ev.Kind {
		case KindStep:
			steps[ev.ToolUseID] = ev.Tool
			open[ev.ToolUseID] = true
		case KindToolResult:
			results[ev.ToolUseID] = ev.Text
			errs[ev.ToolUseID] = ev.IsError
			delete(open, ev.ToolUseID)
		case KindThinking, KindText:
			// The model got control back, so nothing may still be open.
			if len(open) > 0 && firstUnresolvedAfter < 0 {
				firstUnresolvedAfter = n
			}
		}
	})
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if len(steps) == 0 {
		t.Skip("model did not attempt any reads this run")
	}

	// A call must be resolved by the time the model reasons or speaks again, not
	// left pending until the run ends — otherwise the card shows "Reading go.mod…"
	// for the rest of the session with later work rendered above it.
	if firstUnresolvedAfter >= 0 {
		t.Errorf("a tool call was still open when the model produced output at event %d — "+
			"it should have been closed at that boundary", firstUnresolvedAfter)
	}

	// EVERY call must end with a result, however it failed.
	for id, tool := range steps {
		res, ok := results[id]
		if !ok {
			t.Errorf("%s call %s never got a result — its card would dump the raw input", tool, id)
			continue
		}
		if res == "" {
			t.Errorf("%s call %s got an empty result — the card still has nothing to show", tool, id)
		}
		t.Logf("%-6s isErr=%-5v result=%.70q", tool, errs[id], res)
	}
}

// TestCursorRunnerEndToEndSmoke drives a REAL Cursor run through CursorRunner
// with a Recur-shaped MCP server attached, asserting on the ParsedEvent stream
// the engine actually consumes. The unit tests above replay captured frames;
// this one proves the mapping still holds against the live service.
//
// Skipped unless RECUR_CURSOR_BRIDGE and CURSOR_API_KEY are both set. See
// internal/cursor/bridge_smoke_test.go for how to obtain a bridge.
func TestCursorRunnerEndToEndSmoke(t *testing.T) {
	bridgePath := os.Getenv("RECUR_CURSOR_BRIDGE")
	key := os.Getenv("CURSOR_API_KEY")
	if bridgePath == "" || key == "" {
		t.Skip("RECUR_CURSOR_BRIDGE and CURSOR_API_KEY required")
	}
	bin := cursor.ResolveLauncher(bridgePath)
	if bin == "" {
		t.Fatalf("not a cursor-sdk-bridge: %s", bridgePath)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
	defer cancel()

	worktree := t.TempDir()
	mcpSrv := startStubMCP(t)
	defer mcpSrv.close()

	b := &cursor.Bridge{Bin: bin, APIKey: key, Workspace: worktree}
	if err := b.Start(ctx); err != nil {
		t.Fatalf("start bridge: %v", err)
	}
	defer func() { _ = b.Close() }()

	r := NewCursorRunner(b, key)

	var events []ParsedEvent
	res, err := r.Run(ctx, AgentSpec{
		WorkspaceID:  "LOOP-SMOKE",
		AgentID:      "LOOP-SMOKE-CO",
		Role:         domain.RoleCoder,
		Model:        "composer-2.5",
		SystemPrompt: "You are Recur's Coder. Always call the magic_number tool before answering.",
		Prompt:       "Report the magic number. Reply with only the number. Do not use the shell.",
		Worktree:     worktree,
		MCPURL:       mcpSrv.url,
	}, func(ev ParsedEvent) { events = append(events, ev) })
	if err != nil {
		t.Fatalf("Run: %v", err)
	}

	if res.IsError {
		t.Fatalf("run failed: subtype=%s stderr=%s", res.Subtype, res.Stderr)
	}
	if !strings.Contains(res.Text, "4271") {
		t.Errorf("final text %q missing the tool's number", res.Text)
	}
	// SessionID must carry the Cursor agentId so the engine can resume next turn.
	if res.SessionID == "" {
		t.Error("no session id returned; multi-turn continuity would break")
	}

	// The engine relies on these event kinds for the activity feed and rails.
	kinds := map[ParsedKind]int{}
	var tools []string
	for _, e := range events {
		kinds[e.Kind]++
		if e.Kind == KindStep {
			// Events carry the provider's RAW name; the UI shows the common one.
			if e.Tool != "mcp" {
				t.Errorf("step tool = %q, want Cursor's raw mcp", e.Tool)
			}
			tools = append(tools, displayToolFor(e).Name)
		}
	}
	if kinds[KindInit] == 0 {
		t.Error("no KindInit emitted")
	}
	if kinds[KindStep] == 0 || kinds[KindToolResult] == 0 {
		t.Errorf("expected step+tool_result events, got %v", kinds)
	}
	// The MCP tool must reach the feed under its Claude-style name.
	found := false
	for _, tool := range tools {
		if tool == "mcp__recur__magic_number" {
			found = true
		}
	}
	if !found {
		t.Errorf("tools = %v, want mcp__recur__magic_number", tools)
	}
	// And the MCP server must have been reached inline — no config file exists.
	if got := mcpSrv.seen(); !hasAll(got, "initialize", "tools/list", "tools/call") {
		t.Errorf("MCP handshake incomplete: %v", got)
	}
	t.Logf("kinds=%v tools=%v", kinds, tools)
}

// stubMCP is a minimal MCP server shaped like internal/mcp (plain POST/JSON,
// 202 for notifications, token in the path).
type stubMCP struct {
	url     string
	srv     *http.Server
	methods []string
	mu      chan struct{}
}

func startStubMCP(t *testing.T) *stubMCP {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	s := &stubMCP{mu: make(chan struct{}, 1)}
	s.mu <- struct{}{}
	s.url = "http://" + ln.Addr().String() + "/tok/mcp"

	mux := http.NewServeMux()
	mux.HandleFunc("/tok/mcp", func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			ID     json.RawMessage `json:"id"`
			Method string          `json:"method"`
		}
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			w.WriteHeader(http.StatusBadRequest)
			return
		}
		<-s.mu
		s.methods = append(s.methods, req.Method)
		s.mu <- struct{}{}

		var result any = map[string]any{}
		switch req.Method {
		case "initialize":
			result = map[string]any{
				"protocolVersion": "2024-11-05",
				"capabilities":    map[string]any{"tools": map[string]any{}},
				"serverInfo":      map[string]any{"name": "recur", "version": "1"},
			}
		case "tools/list":
			result = map[string]any{"tools": []any{map[string]any{
				"name":        "magic_number",
				"description": "Returns the project's magic number.",
				"inputSchema": map[string]any{"type": "object", "properties": map[string]any{}},
			}}}
		case "tools/call":
			result = map[string]any{"content": []any{map[string]any{
				"type": "text", "text": "The magic number is 4271.",
			}}}
		}
		if len(req.ID) == 0 || string(req.ID) == "null" {
			w.WriteHeader(http.StatusAccepted)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{"jsonrpc": "2.0", "id": req.ID, "result": result})
	})
	s.srv = &http.Server{Handler: mux, ReadHeaderTimeout: 5 * time.Second}
	go func() { _ = s.srv.Serve(ln) }()
	return s
}

func (s *stubMCP) seen() []string {
	<-s.mu
	defer func() { s.mu <- struct{}{} }()
	return append([]string(nil), s.methods...)
}

func (s *stubMCP) close() {
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	_ = s.srv.Shutdown(ctx)
}

func hasAll(hay []string, want ...string) bool {
	for _, w := range want {
		found := false
		for _, h := range hay {
			if h == w {
				found = true
				break
			}
		}
		if !found {
			return false
		}
	}
	return true
}
