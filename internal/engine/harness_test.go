package engine

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/google/uuid"

	"recur/internal/agenttools"
	"recur/internal/domain"
	"recur/internal/responses"
)

func testCompatibleProvider(baseURL string) domain.CompatibleProvider {
	return domain.CompatibleProvider{
		ID:      "openai",
		Name:    "OpenAI",
		BaseURL: baseURL,
		APIKey:  "sk-test",
		Models: []domain.CompatibleModel{{
			ID:    "gpt-5",
			Label: "GPT-5",
			Flags: domain.CompatibleFlagPreset("openai"),
		}},
	}
}

func TestHarnessInstructionsAlwaysIncludeCodingPrompt(t *testing.T) {
	got := harnessInstructions(AgentSpec{})
	if !strings.Contains(got, "You are Recur's coding agent") {
		t.Errorf("harnessInstructions(empty) = %q, want coding prompt", got)
	}
	got = harnessInstructions(AgentSpec{SystemPrompt: "role identity", TurnAppendPrompt: "on the billing PR"})
	if !strings.Contains(got, "You are Recur's coding agent") || !strings.Contains(got, "role identity") {
		t.Errorf("harnessInstructions(system) = %q, want coding prompt + system", got)
	}
	if !strings.Contains(got, "on the billing PR") {
		t.Errorf("harnessInstructions() missing TurnAppendPrompt: %q", got)
	}
}

func TestHarnessAskModeOmitsEdit(t *testing.T) {
	r := &HarnessRunner{Provider: testCompatibleProvider("http://example.invalid")}
	reg := agenttools.NewRegistry(append(agenttools.HostTools(), agenttools.CodingTools()...)...)
	tools := r.responsesTools(reg, []string{"Read", "Glob", "Grep", "mcp__recur__ask_user"})
	names := map[string]bool{}
	for _, tl := range tools {
		names[tl.Name] = true
	}
	if names["Edit"] || names["Write"] || names["Bash"] {
		t.Errorf("Ask allowlist advertised %v, want no Edit/Write/Bash", names)
	}
	if !names["Read"] {
		t.Errorf("Ask allowlist advertised %v, want Read", names)
	}
	if !names["mcp__recur__ask_user"] && !names["ask_user"] {
		t.Errorf("Ask allowlist advertised %v, want ask_user", names)
	}
}

func TestHarnessWriteThenText(t *testing.T) {
	wt := t.TempDir()
	var n atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/responses" {
			http.NotFound(w, r)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		round := n.Add(1)
		if round == 1 {
			_ = json.NewEncoder(w).Encode(map[string]any{
				"id":     "resp-1",
				"status": "completed",
				"output": []any{map[string]any{
					"type":      "function_call",
					"id":        "fc_1",
					"call_id":   "call_1",
					"name":      "Write",
					"arguments": `{"file_path":"hello.txt","content":"hi"}`,
				}},
			})
			return
		}
		_ = json.NewEncoder(w).Encode(map[string]any{
			"id":     "resp-2",
			"status": "completed",
			"output": []any{map[string]any{
				"type": "message",
				"content": []any{
					map[string]string{"type": "output_text", "text": "Wrote hello.txt"},
				},
			}},
			"usage": map[string]any{"input_tokens": 10, "output_tokens": 4},
		})
	}))
	t.Cleanup(srv.Close)

	p := testCompatibleProvider(srv.URL)
	r := &HarnessRunner{
		Provider: p,
		Client:   responses.NewClient(p),
		Todos:    &agenttools.TodoStore{},
	}
	var steps []string
	res, err := r.Run(t.Context(), AgentSpec{
		Provider:     p.ID,
		Model:        "gpt-5",
		Prompt:       "write hello",
		Worktree:     wt,
		AllowedTools: []string{"Read", "Edit", "Write", "Glob", "Grep", "Bash"},
	}, func(ev ParsedEvent) {
		if ev.Kind == KindStep {
			steps = append(steps, ev.Tool)
		}
	})
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if res.Text != "Wrote hello.txt" {
		t.Errorf("Run text = %q, want Wrote hello.txt", res.Text)
	}
	if len(steps) != 1 || steps[0] != "Write" {
		t.Errorf("steps = %v, want [Write]", steps)
	}
	raw, err := os.ReadFile(filepath.Join(wt, "hello.txt"))
	if err != nil {
		t.Fatalf("worktree hello.txt: %v", err)
	}
	if string(raw) != "hi" {
		t.Errorf("hello.txt = %q, want hi", raw)
	}
}

func TestHarnessPlainSteerDecodesDisplay(t *testing.T) {
	const display = "user text"
	prompt := "The operator is looking at this workspace.\n\n<system_instruction>\nattached\n</system_instruction>\n\n" + display
	var n atomic.Int32
	var second []byte
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		if n.Add(1) == 1 {
			_ = json.NewEncoder(w).Encode(map[string]any{
				"id":     "resp-1",
				"status": "completed",
				"output": []any{map[string]any{
					"type":      "function_call",
					"id":        "fc_1",
					"call_id":   "call_1",
					"name":      "Glob",
					"arguments": `{"pattern":"*"}`,
				}},
			})
			return
		}
		second, _ = io.ReadAll(r.Body)
		_ = json.NewEncoder(w).Encode(map[string]any{
			"id":     "resp-2",
			"status": "completed",
			"output": []any{map[string]any{
				"type":    "message",
				"content": []any{map[string]string{"type": "output_text", "text": "ok"}},
			}},
		})
	}))
	t.Cleanup(srv.Close)

	inject := make(chan string, 1)
	inject <- encodeInjectPlain(prompt, display)
	p := testCompatibleProvider(srv.URL)
	r := &HarnessRunner{Provider: p, Client: responses.NewClient(p), Todos: &agenttools.TodoStore{}}
	var users []ParsedEvent
	if _, err := r.Run(t.Context(), AgentSpec{
		Provider:     p.ID,
		Model:        "gpt-5",
		Prompt:       "go",
		Worktree:     t.TempDir(),
		AllowedTools: []string{"Glob"},
		Inject:       inject,
		InjectPlain:  true,
	}, func(ev ParsedEvent) {
		if ev.Kind == KindUser {
			users = append(users, ev)
		}
	}); err != nil {
		t.Fatalf("Run: %v", err)
	}
	if len(users) != 1 || users[0].Text != prompt || users[0].DisplayText != display {
		t.Fatalf("KindUser = %+v, want Text=prompt DisplayText=%q", users, display)
	}
	var body struct {
		Input []json.RawMessage `json:"input"`
	}
	if err := json.Unmarshal(second, &body); err != nil || len(body.Input) == 0 {
		t.Fatalf("second request = %s (%v)", second, err)
	}
	var last responses.InputItem
	if err := json.Unmarshal(body.Input[len(body.Input)-1], &last); err != nil {
		t.Fatalf("last input: %v", err)
	}
	if last.Role != "user" || len(last.Content) != 1 || last.Content[0].Text != prompt {
		t.Fatalf("steer input = %+v, want decoded prompt", last)
	}
	if strings.Contains(string(second), injectPlainSep) {
		t.Fatalf("second request leaked separator: %s", second)
	}
}

func TestHarnessAskUserHalts(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, nil)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{
			"id":     "resp-ask",
			"status": "completed",
			"output": []any{map[string]any{
				"type":      "function_call",
				"id":        "fc_ask",
				"call_id":   "call_ask",
				"name":      "ask_user",
				"arguments": `{"question":"Which file?"}`,
			}},
		})
	}))
	t.Cleanup(srv.Close)

	p := testCompatibleProvider(srv.URL)
	r := &HarnessRunner{
		Provider: p,
		Client:   responses.NewClient(p),
		Reporter: engineReporter{eng: eng},
		Todos:    &eng.todos,
	}
	var n int
	res, err := r.Run(t.Context(), AgentSpec{
		Provider:     p.ID,
		Model:        "gpt-5",
		Prompt:       "ask me",
		Worktree:     t.TempDir(),
		AllowedTools: []string{"mcp__recur__ask_user", "Read"},
		WorkspaceID:  "W1",
		AgentID:      "W1-PA",
	}, func(ParsedEvent) { n++ })
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if res.IsError {
		t.Errorf("Run result is error subtype=%q text=%q", res.Subtype, res.Text)
	}
	if n == 0 {
		t.Error("Run emitted no events")
	}
	pending := eng.PendingAskUsers()
	if len(pending) == 0 {
		t.Fatal("PendingAskUsers() = empty, want parked ask_user")
	}
}

func TestRegisterMCPSkipsCompatible(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	set := st.Settings()
	set.CompatibleProviders = []domain.CompatibleProvider{testCompatibleProvider("https://api.openai.com")}
	st.SetSettings(set.NormalizeProviders())

	token, url, err := eng.registerMCP("W1", "PA", "PA", []string{"Read", "mcp__recur__ask_user"}, "openai/gpt-5")
	if err != nil {
		t.Fatalf("registerMCP: %v", err)
	}
	if url != "" {
		t.Errorf("registerMCP(compatible) url = %q, want empty", url)
	}
	if _, _, ok := parseCompatTurnToken(token); !ok {
		t.Errorf("registerMCP(compatible) token = %q, want compat turn token", token)
	}
}

func TestResolveRunnerCompatible(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	eng.runnerOverrideSet = false
	set := st.Settings()
	set.CompatibleProviders = []domain.CompatibleProvider{testCompatibleProvider("https://api.openai.com")}
	st.SetSettings(set.NormalizeProviders())

	r, err := eng.resolveRunner(t.Context(), "openai", t.TempDir())
	if err != nil {
		t.Fatalf("resolveRunner: %v", err)
	}
	h, ok := r.(*HarnessRunner)
	if !ok {
		t.Fatalf("runner = %T, want *HarnessRunner", r)
	}
	if h.Provider.ID != "openai" {
		t.Errorf("Provider.ID = %q, want openai", h.Provider.ID)
	}
}

func TestToolAllowedOnTurn(t *testing.T) {
	empty := ""
	if toolAllowedOnTurn("Write", AgentSpec{Tools: &empty, AllowedTools: []string{"Write"}}) {
		t.Fatal("Tools=\"\" must disable Write even if allowlisted")
	}
	if toolAllowedOnTurn("Write", AgentSpec{AllowedTools: []string{"Read"}}) {
		t.Fatal("Write must be rejected when not in AllowedTools")
	}
	if !toolAllowedOnTurn("Read", AgentSpec{AllowedTools: []string{"Read", "Glob"}}) {
		t.Fatal("Read should be allowed")
	}
	if toolAllowedOnTurn("permission_request", AgentSpec{}) {
		t.Fatal("permission_request must never be allowed on the native turn")
	}
}

func TestHarnessHistorySkipsCurrentUserAndKeepsCallIDs(t *testing.T) {
	msgs := []domain.Message{
		{ID: "u1", Role: "user", Content: []domain.ContentBlock{domain.TextBlock("hello")}},
		{ID: "a1", Role: "assistant", Content: []domain.ContentBlock{
			domain.ToolUseBlock("internal-1", "call_abc", "Read", json.RawMessage(`{"file_path":"a.go"}`)),
		}},
		{ID: "r1", Role: "user", Content: []domain.ContentBlock{
			domain.ToolResultBlock("internal-1", "ok", false),
		}},
		{ID: "u2", Role: "user", Content: []domain.ContentBlock{domain.TextBlock("again")}},
	}
	got := harnessHistory(msgs, AgentSpec{Prompt: "again", UserMessageID: "u2"})
	raw, _ := json.Marshal(got)
	s := string(raw)
	if strings.Count(s, `"again"`) != 1 {
		t.Fatalf("current user prompt duplicated: %s", s)
	}
	if !strings.Contains(s, `"call_abc"`) {
		t.Fatalf("provider call_id not replayed: %s", s)
	}
	if strings.Contains(s, "internal-1") {
		t.Fatalf("internal tool id leaked into Responses input: %s", s)
	}
}

func TestCapHistoryKeepsCallOutputPair(t *testing.T) {
	items := []responses.InputItem{
		responses.FunctionCallItem("c1", "Read", strings.Repeat("x", 100)),
		responses.FunctionOutputItem("c1", strings.Repeat("y", 100)),
		responses.UserText("latest"),
	}
	if nextHistoryGroup(items[:2]) != 2 {
		t.Fatalf("nextHistoryGroup(call+output) = %d, want 2", nextHistoryGroup(items[:2]))
	}
	if nextHistoryGroup(items[1:2]) != 1 {
		t.Fatalf("nextHistoryGroup(orphan output) = %d, want 1", nextHistoryGroup(items[1:2]))
	}
}

func TestCapHistoryKeepsParallelCallBatch(t *testing.T) {
	const mark = "latest-user-keep"
	user := responses.UserText(mark)
	mk := func(n int) []responses.InputItem {
		pad := strings.Repeat("x", n)
		return []responses.InputItem{
			responses.FunctionCallItem("c1", "Read", `{}`),
			responses.FunctionCallItem("c2", "Glob", `{}`),
			responses.FunctionOutputItem("c1", pad),
			responses.FunctionOutputItem("c2", pad),
			user,
		}
	}
	small := mk(8)
	if got := nextHistoryGroup(small[:4]); got != 4 {
		t.Fatalf("nextHistoryGroup(parallel batch) = %d, want 4", got)
	}

	skel, err := json.Marshal(mk(0))
	if err != nil {
		t.Fatal(err)
	}
	// Two output strings grow by n each; pick n so the full JSON is just over
	// the cap while dropping only call(c1) would still fit (the old split).
	n := (maxHistoryBytes-len(skel))/2 + 1
	if n < 1 {
		n = 1
	}
	items := mk(n)
	full, err := json.Marshal(items)
	if err != nil {
		t.Fatal(err)
	}
	rest, err := json.Marshal(items[1:])
	if err != nil {
		t.Fatal(err)
	}
	if len(full) <= maxHistoryBytes {
		t.Fatalf("setup: full size %d, want > %d", len(full), maxHistoryBytes)
	}
	if len(rest) > maxHistoryBytes {
		t.Fatalf("setup: rest without call(c1) is %d, want <= %d", len(rest), maxHistoryBytes)
	}

	got := capHistory(items)
	var calls, outs int
	var hasUser bool
	for _, it := range got {
		switch {
		case it.Type == "function_call" && (it.CallID == "c1" || it.CallID == "c2"):
			calls++
		case it.Type == "function_call_output" && (it.CallID == "c1" || it.CallID == "c2"):
			outs++
		case it.Role == "user":
			hasUser = true
		}
	}
	if !hasUser {
		t.Fatal("capHistory dropped the latest user item")
	}
	if calls != outs || (calls != 0 && calls != 2) {
		t.Fatalf("split parallel batch: calls=%d outputs=%d (want 0/0 or 2/2)", calls, outs)
	}
}

func TestResponsesFreshInputSubtractsCache(t *testing.T) {
	if got := responsesFreshInput(responses.Usage{InputTokens: 100, CacheReadTokens: 90}); got != 10 {
		t.Errorf("responsesFreshInput = %d, want 10", got)
	}
}

func TestHarnessRejectsFailedResponse(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{
			"id":     "resp-fail",
			"status": "failed",
			"error":  map[string]string{"message": "overloaded"},
			"output": []any{map[string]any{
				"type":      "function_call",
				"call_id":   "call_1",
				"name":      "Write",
				"arguments": `{"file_path":"x","content":"y"}`,
			}},
		})
	}))
	t.Cleanup(srv.Close)
	p := testCompatibleProvider(srv.URL)
	r := &HarnessRunner{Provider: p, Client: responses.NewClient(p), Todos: &agenttools.TodoStore{}}
	wt := t.TempDir()
	_, err := r.Run(t.Context(), AgentSpec{
		Provider: p.ID, Model: "gpt-5", Prompt: "go", Worktree: wt,
		AllowedTools: []string{"Write"},
	}, nil)
	if err == nil || !strings.Contains(err.Error(), "overloaded") {
		t.Fatalf("Run err = %v, want overloaded", err)
	}
	if _, statErr := os.Stat(filepath.Join(wt, "x")); statErr == nil {
		t.Fatal("failed response must not execute Write")
	}
}

func TestHarnessRejectsIncompleteResponse(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{
			"id":     "resp-incomplete",
			"status": "incomplete",
			"output": []any{map[string]any{
				"type":      "function_call",
				"call_id":   "call_1",
				"name":      "Write",
				"arguments": `{"file_path":"x","content":"y"}`,
			}},
		})
	}))
	t.Cleanup(srv.Close)
	p := testCompatibleProvider(srv.URL)
	r := &HarnessRunner{Provider: p, Client: responses.NewClient(p), Todos: &agenttools.TodoStore{}}
	wt := t.TempDir()
	_, err := r.Run(t.Context(), AgentSpec{
		Provider: p.ID, Model: "gpt-5", Prompt: "go", Worktree: wt,
		AllowedTools: []string{"Write"},
	}, nil)
	if err == nil || !strings.Contains(err.Error(), "incomplete") {
		t.Fatalf("Run err = %v, want incomplete", err)
	}
	if _, statErr := os.Stat(filepath.Join(wt, "x")); statErr == nil {
		t.Fatal("incomplete response must not execute Write")
	}
}

func TestHarnessRejectsEmptyJSON(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{}`))
	}))
	t.Cleanup(srv.Close)
	p := testCompatibleProvider(srv.URL)
	r := &HarnessRunner{Provider: p, Client: responses.NewClient(p), Todos: &agenttools.TodoStore{}}
	_, err := r.Run(t.Context(), AgentSpec{
		Provider: p.ID, Model: "gpt-5", Prompt: "go", Worktree: t.TempDir(),
	}, nil)
	if err == nil {
		t.Fatal("Run({}) err = nil, want decode error")
	}
}

func TestHarnessRejectsPrematureStream(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = w.Write([]byte("data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"Write\",\"arguments\":\"\"}}\n\n"))
		_, _ = w.Write([]byte("data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"{\\\"file_path\\\":\\\"x\\\",\\\"content\\\":\\\"y\\\"}\"}\n\n"))
	}))
	t.Cleanup(srv.Close)
	p := testCompatibleProvider(srv.URL)
	r := &HarnessRunner{Provider: p, Client: responses.NewClient(p), Todos: &agenttools.TodoStore{}}
	wt := t.TempDir()
	_, err := r.Run(t.Context(), AgentSpec{
		Provider: p.ID, Model: "gpt-5", Prompt: "go", Worktree: wt,
		AllowedTools: []string{"Write"},
	}, nil)
	if err == nil || !strings.Contains(err.Error(), "before completion") {
		t.Fatalf("Run err = %v, want stream ended before completion", err)
	}
	if _, statErr := os.Stat(filepath.Join(wt, "x")); statErr == nil {
		t.Fatal("truncated stream must not execute Write")
	}
}

func TestHarnessReasoningDoneNotDuplicated(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		frames := []string{
			`{"type":"response.reasoning_summary_text.delta","item_id":"rs_1","delta":"think"}`,
			`{"type":"response.reasoning_summary_text.done","item_id":"rs_1","text":"think"}`,
			`{"type":"response.output_text.delta","item_id":"msg_1","delta":"hi"}`,
			`{"type":"response.completed","response":{"id":"r1","status":"completed"}}`,
		}
		for _, f := range frames {
			_, _ = w.Write([]byte("data: " + f + "\n\n"))
		}
	}))
	t.Cleanup(srv.Close)
	p := testCompatibleProvider(srv.URL)
	r := &HarnessRunner{Provider: p, Client: responses.NewClient(p), Todos: &agenttools.TodoStore{}}
	var thinking string
	res, err := r.Run(t.Context(), AgentSpec{
		Provider: p.ID, Model: "gpt-5", Prompt: "go", Worktree: t.TempDir(),
	}, func(ev ParsedEvent) {
		if ev.Kind == KindThinking {
			thinking = ev.Text
		}
	})
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if thinking != "think" {
		t.Errorf("KindThinking = %q, want think (done must not append after deltas)", thinking)
	}
	if res.Text != "hi" {
		t.Errorf("Run text = %q, want hi", res.Text)
	}
}

func TestHarnessKeepsEveryFinalMessage(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		frames := []string{
			`{"type":"response.output_item.done","item":{"id":"msg_1","type":"message","content":[{"type":"output_text","text":"hello"}]}}`,
			`{"type":"response.output_item.done","item":{"id":"msg_2","type":"message","content":[{"type":"output_text","text":"world"}]}}`,
			`{"type":"response.completed","response":{"id":"r1","status":"completed"}}`,
		}
		for _, f := range frames {
			_, _ = w.Write([]byte("data: " + f + "\n\n"))
		}
	}))
	t.Cleanup(srv.Close)
	p := testCompatibleProvider(srv.URL)
	r := &HarnessRunner{Provider: p, Client: responses.NewClient(p), Todos: &agenttools.TodoStore{}}
	res, err := r.Run(t.Context(), AgentSpec{
		Provider: p.ID, Model: "gpt-5", Prompt: "go", Worktree: t.TempDir(),
	}, nil)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if res.Text != "helloworld" {
		t.Errorf("Run text = %q, want helloworld", res.Text)
	}
}

func TestHarnessMixedStreamAndFinalMessages(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		frames := []string{
			`{"type":"response.output_text.delta","item_id":"msg_1","delta":"hello"}`,
			`{"type":"response.output_item.done","item":{"id":"msg_1","type":"message","content":[{"type":"output_text","text":"hello"}]}}`,
			`{"type":"response.output_item.done","item":{"id":"msg_2","type":"message","content":[{"type":"output_text","text":"world"}]}}`,
			`{"type":"response.completed","response":{"id":"r1","status":"completed"}}`,
		}
		for _, f := range frames {
			_, _ = w.Write([]byte("data: " + f + "\n\n"))
		}
	}))
	t.Cleanup(srv.Close)
	p := testCompatibleProvider(srv.URL)
	r := &HarnessRunner{Provider: p, Client: responses.NewClient(p), Todos: &agenttools.TodoStore{}}
	res, err := r.Run(t.Context(), AgentSpec{
		Provider: p.ID, Model: "gpt-5", Prompt: "go", Worktree: t.TempDir(),
	}, nil)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if res.Text != "helloworld" {
		t.Errorf("Run text = %q, want helloworld", res.Text)
	}
}

func TestHarnessOmitsToolsWhenDisabled(t *testing.T) {
	r := &HarnessRunner{Provider: testCompatibleProvider("http://example.invalid")}
	reg := agenttools.NewRegistry(append(agenttools.HostTools(), agenttools.CodingTools()...)...)
	empty := ""
	if tools := r.responsesTools(reg, nil); len(tools) == 0 {
		t.Fatal("nil allowlist should advertise tools")
	}
	spec := AgentSpec{Tools: &empty}
	if !toolsDisabled(spec) {
		t.Fatal("Tools=\"\" should disable tools")
	}
}

func TestHarnessDispatchEnforcesAllowlist(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{
			"id":     "resp-1",
			"status": "completed",
			"output": []any{map[string]any{
				"type":      "function_call",
				"call_id":   "call_1",
				"name":      "Write",
				"arguments": `{"file_path":"x.txt","content":"nope"}`,
			}},
		})
	}))
	t.Cleanup(srv.Close)
	p := testCompatibleProvider(srv.URL)
	r := &HarnessRunner{Provider: p, Client: responses.NewClient(p), Todos: &agenttools.TodoStore{}}
	wt := t.TempDir()
	_, err := r.Run(t.Context(), AgentSpec{
		Provider: p.ID, Model: "gpt-5", Prompt: "write", Worktree: wt,
		AllowedTools: []string{"Read"},
		MaxTurns:     1,
	}, nil)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if _, statErr := os.Stat(filepath.Join(wt, "x.txt")); statErr == nil {
		t.Fatal("Write must not run when not in AllowedTools")
	}
}

func TestMirroringRunnerAssignsUserMessageIDForHistory(t *testing.T) {
	cap := &captureRunner{linesFor: func(int) [][]byte { return maestroToolFixture("sess-hist") }}
	eng, st, _, _ := newTestEngine(t, cap)
	projID := addProject(t, st, initRepo(t))
	workspaceID := startedLoop(t, eng, projID)
	prompt := "a long briefing that must not duplicate"
	_, err := eng.runAgent(t.Context(), AgentSpec{
		WorkspaceID: workspaceID,
		AgentID:     workspaceID + "-CO",
		Role:        domain.RoleCoder,
		Model:       "claude-sonnet-5",
		Prompt:      prompt,
	}, nil)
	if err != nil {
		t.Fatalf("runAgent: %v", err)
	}
	got, ok := cap.specAt(0)
	if !ok {
		t.Fatal("inner runner received no spec")
	}
	if got.UserMessageID == "" {
		t.Fatal("mirroringRunner UserMessageID = empty, want recorded row id")
	}
	msgs, err := st.SessionTranscript(got.RecurSessionID)
	if err != nil {
		t.Fatalf("SessionTranscript: %v", err)
	}
	raw, _ := json.Marshal(harnessHistory(msgs, got))
	if n := strings.Count(string(raw), prompt); n != 1 {
		t.Errorf("harnessHistory prompt count = %d, want 1: %s", n, raw)
	}
}

func TestMirroringRunnerRetryResolvesUserMessageID(t *testing.T) {
	cap := &captureRunner{linesFor: func(int) [][]byte { return maestroToolFixture("sess-retry") }}
	eng, st, _, _ := newTestEngine(t, cap)
	projID := addProject(t, st, initRepo(t))
	workspaceID := startedLoop(t, eng, projID)
	prompt := "retry this briefing"
	base := AgentSpec{
		WorkspaceID: workspaceID,
		AgentID:     workspaceID + "-CO",
		Role:        domain.RoleCoder,
		Model:       "claude-sonnet-5",
		Prompt:      prompt,
	}
	if _, err := eng.runAgent(t.Context(), base, nil); err != nil {
		t.Fatalf("runAgent first: %v", err)
	}
	first, ok := cap.specAt(0)
	if !ok || first.UserMessageID == "" {
		t.Fatal("first turn UserMessageID empty")
	}
	retry := base
	retry.SkipRecordUser = true
	retry.UserMessageID = uuid.NewString()
	if _, err := eng.runAgent(t.Context(), retry, nil); err != nil {
		t.Fatalf("runAgent retry: %v", err)
	}
	got, ok := cap.specAt(1)
	if !ok {
		t.Fatal("retry runner received no spec")
	}
	if got.UserMessageID != first.UserMessageID {
		t.Errorf("retry UserMessageID = %q, want recorded %q", got.UserMessageID, first.UserMessageID)
	}
	msgs, err := st.SessionTranscript(got.RecurSessionID)
	if err != nil {
		t.Fatalf("SessionTranscript: %v", err)
	}
	raw, _ := json.Marshal(harnessHistory(msgs, got))
	if n := strings.Count(string(raw), prompt); n != 1 {
		t.Errorf("retry harnessHistory prompt count = %d, want 1: %s", n, raw)
	}
}

func TestHarnessHistoryKeepsOriginalOnHiddenContinue(t *testing.T) {
	orig := "plan this screenshot"
	msgs := []domain.Message{
		{ID: "u1", Role: "user", Content: []domain.ContentBlock{domain.TextBlock(orig)}},
		{ID: "a1", Role: "assistant", Content: []domain.ContentBlock{domain.TextBlock("switching")}},
	}
	cont := formatModeSwitchResumePrompt(domain.ChatModeAsk, domain.ChatModePlan)
	got := harnessHistory(msgs, AgentSpec{
		Prompt:         cont,
		DisplayText:    formatModeSwitchDisplay(domain.ChatModePlan),
		UserMessageID:  "u1",
		SkipRecordUser: true,
	})
	raw, _ := json.Marshal(got)
	s := string(raw)
	if !strings.Contains(s, orig) {
		t.Errorf("harnessHistory(mode remount) missing original request: %s", s)
	}
	if !strings.Contains(s, "from ask to plan") {
		t.Errorf("harnessHistory(mode remount) missing continuation: %s", s)
	}

	got = harnessHistory(msgs, AgentSpec{
		Prompt:         chatResumePrompt,
		DisplayText:    chatResumeDisplay,
		UserMessageID:  "u1",
		SkipRecordUser: true,
	})
	raw, _ = json.Marshal(got)
	s = string(raw)
	if !strings.Contains(s, orig) {
		t.Errorf("harnessHistory(restart) missing original request: %s", s)
	}
	if !strings.Contains(s, "Recur restarted") {
		t.Errorf("harnessHistory(restart) missing continuation: %s", s)
	}
}

func TestHarnessHistoryRespectsFreshSessionCutoff(t *testing.T) {
	msgs := []domain.Message{
		{ID: "old-u", Role: "user", Content: []domain.ContentBlock{domain.TextBlock("hello from last week")}},
		{ID: "old-a", Role: "assistant", Content: []domain.ContentBlock{domain.TextBlock("ok")}},
		{ID: "cut", Role: "assistant", Content: []domain.ContentBlock{domain.TextBlock("end of prior session")}},
		{ID: "new-u", Role: "user", Content: []domain.ContentBlock{domain.TextBlock("WIRE")}},
	}
	got := harnessHistory(msgs, AgentSpec{Prompt: "WIRE", UserMessageID: "new-u", HistoryAfterID: "cut"})
	raw, _ := json.Marshal(got)
	s := string(raw)
	if strings.Contains(s, "hello from last week") || strings.Contains(s, "end of prior session") {
		t.Errorf("harnessHistory(cutoff) leaked prior turns: %s", s)
	}
	if strings.Count(s, "WIRE") != 1 {
		t.Errorf("harnessHistory(cutoff) WIRE count = %d, want 1: %s", strings.Count(s, "WIRE"), s)
	}
}

func TestMirroringRunnerModeSwitchKeepsOriginalRequest(t *testing.T) {
	runner := &holdFirstRunner{started: make(chan struct{})}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "remount-hist", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	png := base64.StdEncoding.EncodeToString([]byte{
		0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
		0x00, 0x00, 0x00, 0x00,
	})
	orig := "plan this screenshot"
	firstDone := make(chan string, 1)
	go func() {
		firstDone <- eng.SendWorkspaceMessage(ws.ID, "PA", orig+" "+AttachmentMarker("shot1"), "sonnet", nil, []domain.ChatAttachment{{
			ID: "shot1", Name: "shot.png", Mime: "image/png", Data: png,
		}}, domain.ChatModeAsk, "")
	}()
	select {
	case <-runner.started:
	case <-time.After(2 * time.Second):
		t.Fatal("first turn did not start")
	}

	msg := eng.SwitchChatMode(ws.ID, "PA", domain.ChatModePlan)
	if !strings.Contains(msg, "remounting") {
		t.Fatalf("live switch should remount: %q", msg)
	}
	select {
	case <-firstDone:
	case <-time.After(2 * time.Second):
		t.Fatal("first turn did not end after switch_mode park")
	}

	specs := waitCoderTurns(t, runner, 2)
	cont := specs[1]
	if !cont.SkipRecordUser {
		t.Fatal("remount SkipRecordUser = false, want true")
	}
	if cont.RecurSessionID == "" {
		t.Fatal("remount RecurSessionID empty")
	}
	msgs, err := st.SessionTranscript(cont.RecurSessionID)
	if err != nil {
		t.Fatalf("SessionTranscript: %v", err)
	}
	raw, _ := json.Marshal(harnessHistory(msgs, cont))
	s := string(raw)
	if !strings.Contains(s, orig) {
		t.Errorf("remount harnessHistory missing original request: %s", s)
	}
	if !strings.Contains(s, "input_image") {
		t.Errorf("remount harnessHistory missing original image: %s", s)
	}
	if !strings.Contains(s, "from ask to plan") {
		t.Errorf("remount harnessHistory missing continuation: %s", s)
	}
	if strings.Count(s, orig) != 1 {
		t.Errorf("remount harnessHistory original count = %d, want 1: %s", strings.Count(s, orig), s)
	}
}

func TestHarnessStopDuringBashDoesNotWrite(t *testing.T) {
	wt := t.TempDir()
	started := filepath.Join(wt, "started")
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{
			"id":     "resp-1",
			"status": "completed",
			"output": []any{
				map[string]any{
					"type":      "function_call",
					"call_id":   "call_bash",
					"name":      "Bash",
					"arguments": `{"command":"touch started; sleep 30"}`,
				},
				map[string]any{
					"type":      "function_call",
					"call_id":   "call_write",
					"name":      "Write",
					"arguments": `{"file_path":"after.txt","content":"nope"}`,
				},
			},
		})
	}))
	t.Cleanup(srv.Close)
	p := testCompatibleProvider(srv.URL)
	r := &HarnessRunner{Provider: p, Client: responses.NewClient(p), Todos: &agenttools.TodoStore{}}
	ctx, cancel := context.WithCancel(t.Context())
	go func() {
		deadline := time.Now().Add(3 * time.Second)
		for time.Now().Before(deadline) {
			if _, err := os.Stat(started); err == nil {
				cancel()
				return
			}
			time.Sleep(10 * time.Millisecond)
		}
		cancel()
	}()
	_, err := r.Run(ctx, AgentSpec{
		Provider:       p.ID,
		Model:          "gpt-5",
		Prompt:         "go",
		Worktree:       wt,
		AllowedTools:   []string{"Bash", "Write"},
		PermissionMode: domain.PermissionModeBypass,
	}, nil)
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("Run err = %v, want context.Canceled", err)
	}
	if _, statErr := os.Stat(filepath.Join(wt, "after.txt")); statErr == nil {
		t.Fatal("canceled Bash must not execute queued Write")
	}
}

type gateTool struct {
	name            string
	active, maxSeen *atomic.Int32
	started         chan struct{}
	release         <-chan struct{}
}

func (g *gateTool) Name() string            { return g.name }
func (g *gateTool) Description() string     { return g.name }
func (g *gateTool) Schema() json.RawMessage { return json.RawMessage(`{"type":"object"}`) }
func (g *gateTool) Execute(c agenttools.Call) (string, error) {
	n := g.active.Add(1)
	for {
		old := g.maxSeen.Load()
		if n <= old || g.maxSeen.CompareAndSwap(old, n) {
			break
		}
	}
	select {
	case g.started <- struct{}{}:
	default:
	}
	select {
	case <-c.Ctx.Done():
	case <-g.release:
	}
	g.active.Add(-1)
	return "ok", nil
}

func TestHarnessParallelToolsCap(t *testing.T) {
	const n = 16
	var active, maxSeen atomic.Int32
	started := make(chan struct{}, n)
	release := make(chan struct{})
	reg := agenttools.NewRegistry(&gateTool{
		name: "Read", active: &active, maxSeen: &maxSeen, started: started, release: release,
	})
	r := &HarnessRunner{Provider: testCompatibleProvider("http://example.invalid")}
	calls := make([]responses.FunctionCall, n)
	for i := range calls {
		calls[i] = responses.FunctionCall{CallID: fmt.Sprintf("c%d", i), Name: "Read", Arguments: "{}"}
	}
	done := make(chan []callResult, 1)
	go func() {
		done <- r.execCalls(t.Context(), AgentSpec{Provider: "openai"}, reg, calls, func(ParsedEvent) {})
	}()
	for i := 0; i < harnessParallelLimit; i++ {
		select {
		case <-started:
		case <-time.After(2 * time.Second):
			t.Fatalf("execCalls started %d parallel Reads, want %d active", i, harnessParallelLimit)
		}
	}
	select {
	case <-started:
		t.Fatal("execCalls started a Read beyond harnessParallelLimit before a slot freed")
	case <-time.After(50 * time.Millisecond):
	}
	if got := int(maxSeen.Load()); got != harnessParallelLimit {
		t.Errorf("execCalls max active = %d, want %d", got, harnessParallelLimit)
	}
	if got := int(active.Load()); got != harnessParallelLimit {
		t.Errorf("execCalls held active = %d, want %d", got, harnessParallelLimit)
	}
	close(release)
	select {
	case results := <-done:
		if len(results) != n {
			t.Errorf("execCalls results = %d, want %d", len(results), n)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("execCalls did not return after release")
	}
	if got := int(maxSeen.Load()); got > harnessParallelLimit {
		t.Errorf("execCalls max active = %d, want <= %d", got, harnessParallelLimit)
	}
}
