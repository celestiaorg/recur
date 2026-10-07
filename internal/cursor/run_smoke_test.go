package cursor

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"os"
	"strings"
	"sync"
	"testing"
	"time"
)

// TestRunWithInlineMCPSmoke is the load-bearing test for the whole Cursor
// provider: it proves that an agent created through the bridge picks up an
// MCP server passed INLINE (no .cursor/mcp.json anywhere on disk) and actually
// calls a Recur-shaped tool over it.
//
// This is the capability the cursor-agent CLI cannot offer — its MCP config
// paths are hardcoded to <projectRoot>/.cursor/mcp.json and ~/.cursor/mcp.json —
// and it is the reason Recur takes the bridge route.
//
// Costs a real Cursor run; skipped unless RECUR_CURSOR_BRIDGE and CURSOR_API_KEY
// are both set.
func TestRunWithInlineMCPSmoke(t *testing.T) {
	bin := requireBridge(t)
	key := os.Getenv("CURSOR_API_KEY")
	if key == "" {
		t.Skip("CURSOR_API_KEY not set")
	}

	srv := startFakeMCP(t)
	defer srv.close()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
	defer cancel()

	workdir := t.TempDir()
	b := &Bridge{Bin: bin, APIKey: key, Workspace: workdir}
	if err := b.Start(ctx); err != nil {
		t.Fatalf("start bridge: %v", err)
	}
	defer func() { _ = b.Close() }()

	agentID, err := b.Client().CreateAgent(ctx, AgentOptions{
		Model:  &ModelSelection{ID: "composer-2.5"},
		APIKey: key,
		Name:   "recur-smoke",
		Local: &LocalOptions{
			Cwd: []string{workdir},
			// Narrowest ambient config, so the run cannot pick up the developer's
			// own ~/.cursor or any stray project config while under test.
			SettingSources: SettingSourcesMinimal,
		},
		MCPServers: map[string]MCPServer{"recur": NewHTTPMCPServer(srv.url)},
	})
	if err != nil {
		t.Fatalf("CreateAgent: %v", err)
	}
	defer func() {
		cctx, ccancel := context.WithTimeout(context.Background(), 10*time.Second)
		_ = b.Client().CloseAgent(cctx, agentID)
		ccancel()
	}()

	var (
		toolCalls  []string
		final      *RunResult
		sawRunning bool
	)
	err = b.Client().Send(ctx,
		agentID,
		"Call the magic_number tool and reply with only the number it returns. Do not use the shell.",
		nil,
		func(ev RunStreamEvent) error {
			if r := ev.Result; r != nil && r.Result != nil {
				final = r.Result
			}
			m := ev.SDKMessage
			if m == nil {
				return nil
			}
			switch m.Type {
			case MsgStatus:
				if m.Status == StatusRunning {
					sawRunning = true
				}
			case MsgToolCall:
				if m.Status == ToolRunning {
					toolCalls = append(toolCalls, m.ToolLabel())
				}
			}
			return nil
		})
	if err != nil {
		t.Fatalf("Send: %v", err)
	}

	if !sawRunning {
		t.Error("never saw a status/RUNNING event")
	}
	if final == nil {
		t.Fatal("run ended without a result")
	}
	if final.IsError() {
		t.Fatalf("run failed: %+v", final.Error)
	}
	if !strings.Contains(final.Result, "4271") {
		t.Errorf("final text %q does not contain the tool's magic number", final.Result)
	}

	// The point of the test: the MCP server was discovered and invoked from an
	// inline config. A model that shells out to curl instead would satisfy the
	// text assertion above while leaving these empty — so assert on the wire.
	methods := srv.seen()
	if !contains(methods, "initialize") || !contains(methods, "tools/list") {
		t.Errorf("MCP server never handshook (saw %v) — inline mcpServers did not take effect", methods)
	}
	if !contains(methods, "tools/call") {
		t.Errorf("MCP tool was never invoked (saw %v)", methods)
	}
	// The engine keys role scoping and the activity feed off mcp__server__tool,
	// so the label rewrite has to survive a real run.
	if !contains(toolCalls, "mcp__recur__magic_number") {
		t.Errorf("tool label = %v, want mcp__recur__magic_number", toolCalls)
	}
	t.Logf("tool calls: %v; mcp methods: %v; usage: %+v", toolCalls, methods, final.Usage)
}

// fakeMCP is a minimal streamable-HTTP MCP server shaped like internal/mcp:
// plain POST/JSON, 202 for notifications, token in the URL path.
type fakeMCP struct {
	url string
	srv *http.Server
	ln  net.Listener

	mu      sync.Mutex
	methods []string
}

func startFakeMCP(t *testing.T) *fakeMCP {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	f := &fakeMCP{ln: ln}
	f.url = fmt.Sprintf("http://127.0.0.1:%d/tok/mcp", ln.Addr().(*net.TCPAddr).Port)

	mux := http.NewServeMux()
	mux.HandleFunc("/tok/mcp", f.handle)
	f.srv = &http.Server{Handler: mux, ReadHeaderTimeout: 5 * time.Second}
	go func() { _ = f.srv.Serve(ln) }()
	return f
}

func (f *fakeMCP) handle(w http.ResponseWriter, r *http.Request) {
	var req struct {
		ID     json.RawMessage `json:"id"`
		Method string          `json:"method"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		w.WriteHeader(http.StatusBadRequest)
		return
	}
	f.mu.Lock()
	f.methods = append(f.methods, req.Method)
	f.mu.Unlock()

	var result any
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
	default:
		result = map[string]any{}
	}

	// Notifications carry no id and get a bare 202, matching internal/mcp.
	if len(req.ID) == 0 || string(req.ID) == "null" {
		w.WriteHeader(http.StatusAccepted)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]any{
		"jsonrpc": "2.0", "id": req.ID, "result": result,
	})
}

func (f *fakeMCP) seen() []string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]string(nil), f.methods...)
}

func (f *fakeMCP) close() {
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	_ = f.srv.Shutdown(ctx)
}

func contains(hay []string, needle string) bool {
	for _, h := range hay {
		if h == needle {
			return true
		}
	}
	return false
}
