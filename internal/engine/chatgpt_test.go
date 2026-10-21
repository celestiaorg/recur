package engine

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"

	"recur/internal/domain"
	"recur/internal/responses"
	"recur/internal/store"
)

func TestChatGPTUsesNativeHarnessWithoutCLIOrMCP(t *testing.T) {
	t.Setenv("PATH", t.TempDir())
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	st := store.NewInMemory()
	defer st.Close()
	st.SetSettings(domain.Settings{ChatGPTAccountID: "registration", ChatGPTModels: []domain.CompatibleModel{{ID: "gpt-test", Label: "GPT Test"}}})
	eng := New(st)
	r, err := eng.resolveRunner(t.Context(), domain.ProviderChatGPT, t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	native, ok := r.(*HarnessRunner)
	if !ok || !native.Client.ChatGPT || native.Client.Authorize == nil {
		t.Fatalf("ChatGPT runner = %T", r)
	}
	_, endpoint, err := eng.registerMCP("workspace", "CO", "CO", []string{"Read"}, "chatgpt/gpt-test")
	if err != nil || endpoint != "" || eng.mcpStarted {
		t.Fatal("direct ChatGPT started an MCP server")
	}
	if native.Client.Endpoint() != "https://chatgpt.com/backend-api/codex/responses" {
		t.Fatal("ChatGPT is not using the Codex subscription endpoint")
	}
}

func TestChatGPTStreamsToolRoundAndReplaysReasoningWithoutCodex(t *testing.T) {
	t.Setenv("PATH", t.TempDir())
	worktree := t.TempDir()
	if err := os.WriteFile(filepath.Join(worktree, "hello.txt"), []byte("hello from a real file"), 0o600); err != nil {
		t.Fatal(err)
	}
	var rounds, authCalls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/backend-api/codex/responses" || r.Header.Get("Authorization") != "Bearer direct-oauth-token" || r.Header.Get("ChatGPT-Account-Id") != "workspace-one" || r.Header.Get("Originator") != "recur" {
			t.Error("wrong endpoint or OAuth authorization")
		}
		var body map[string]any
		if json.NewDecoder(r.Body).Decode(&body) != nil {
			t.Error("invalid Responses request")
		}
		if body["stream"] != true || body["store"] != false {
			t.Error("ChatGPT request was not stateless streaming")
		}
		include, _ := body["include"].([]any)
		if len(include) != 1 || include[0] != "reasoning.encrypted_content" {
			t.Error("stateless tool rounds did not request encrypted reasoning")
		}
		if _, exists := body["temperature"]; exists {
			t.Error("unsupported ChatGPT request field")
		}
		tools, ok := body["tools"].([]any)
		if !ok || len(tools) != 1 {
			t.Error("missing function tools")
			return
		}
		tool := tools[0].(map[string]any)
		if tool["type"] != "function" || tool["name"] != "Read" {
			t.Error("Codex request changed the tool schema")
		}
		w.Header().Set("Content-Type", "text/event-stream")
		if rounds.Add(1) == 1 {
			_, _ = fmt.Fprint(w, `data: {"type":"response.output_item.done","item":{"id":"fc_read","type":"function_call","call_id":"call_read","name":"Read","arguments":"{\"file_path\":\"hello.txt\"}"}}

data: {"type":"response.completed","response":{"id":"resp_one","status":"completed","output":[{"id":"reasoning_one","type":"reasoning","summary":[],"encrypted_content":"encrypted-reasoning"},{"id":"fc_read","type":"function_call","call_id":"call_read","name":"Read","arguments":"{\"file_path\":\"hello.txt\"}"}]}}

`)
			return
		}
		input, _ := json.Marshal(body["input"])
		if !strings.Contains(string(input), "encrypted-reasoning") || !strings.Contains(string(input), "hello from a real file") || strings.Contains(string(input), `"namespace":"recur"`) {
			t.Errorf("next round dropped reasoning/tool output or rewrote the function call: %s", input)
		}
		_, _ = fmt.Fprint(w, `data: {"type":"response.output_text.delta","delta":"Read the file"}

data: {"type":"response.completed","response":{"id":"resp_two","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Read the file"}]}]}}

`)
	}))
	defer server.Close()
	p := domain.CompatibleProvider{ID: domain.ProviderChatGPT, Name: "ChatGPT", BaseURL: server.URL + "/backend-api/codex", Models: []domain.CompatibleModel{{ID: "gpt-test"}}}
	c := responses.NewClient(p)
	c.ChatGPT = true
	c.Authorize = func(req *http.Request) error {
		authCalls.Add(1)
		req.Header.Set("Authorization", "Bearer direct-oauth-token")
		req.Header.Set("ChatGPT-Account-Id", "workspace-one")
		req.Header.Set("Originator", "recur")
		return nil
	}
	r := &HarnessRunner{Provider: p, Client: c}
	var streamed strings.Builder
	result, err := r.Run(t.Context(), AgentSpec{Provider: domain.ProviderChatGPT, Model: "gpt-test", Prompt: "Read hello.txt", Worktree: worktree, AllowedTools: []string{"Read"}, IncludePartials: true}, func(ev ParsedEvent) {
		if ev.Kind == KindTextDelta {
			streamed.WriteString(ev.Text)
		}
	})
	if err != nil || result.Text != "Read the file" || streamed.String() != result.Text || rounds.Load() != 2 || authCalls.Load() != 2 {
		t.Fatalf("direct tool turn: %+v, %v, %d rounds", result, err, rounds.Load())
	}
}
