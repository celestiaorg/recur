package mcp

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"strings"
	"sync"
	"time"
)

// maxBody caps a single JSON-RPC request body (defense against a runaway client).
// Sized for create_artifact image payloads (base64 png/jpeg) over localhost MCP.
const maxBody = 32 << 20 // 32 MiB

// Server is Recur's MCP HTTP server (PRD §8.3, D7). It listens on 127.0.0.1, and
// each agent connects with a per-token URL (/<token>/mcp). A token is attributed
// to a (workspace, agent) so a report_finding is stamped to the right loop+agent
// and the engine can scope tool access — the server never trusts loop/agent ids
// supplied in the request body.
//
// Sequential turns reuse one token per (workspace, agent) so --mcp-config URLs
// stay cache-friendly; Unregister deactivates it (404) until the next Register.
// A concurrent overlapping turn with the same identity mints a distinct token
// so allowlists cannot clobber each other. Drop / DropAgent / DropWorkspace
// revoke.
//
// Concurrency: the token registry is mutex-guarded; the HTTP handler (hit by
// agent subprocesses concurrently with the loop goroutine) only reads the
// immutable tool table + the registry under the lock, then calls the Reporter
// (which must be concurrency-safe). -race clean.
type Server struct {
	rep   Reporter
	tools []toolSpec

	// Addr overrides the listen address. Empty (production) means 127.0.0.1:0 —
	// a free ephemeral port. Tests set it to force a bind failure.
	Addr string

	httpSrv  *http.Server
	listener net.Listener

	mu      sync.RWMutex
	tokens  map[string]binding // token -> loop+agent it is bound to
	byAgent map[string]string  // workspaceID\x00agentCode -> stable token (sequential reuse)
}

// binding records which loop+agent a URL token is attributed to, and optionally
// which bare tool names tools/list may advertise. allowed == nil means
// unrestricted; allowed non-nil (even empty) means restricted to that set.
type binding struct {
	workspaceID string
	agentCode   string
	chatCode    string          // conductor tab for loop specialist turns; empty = AgentCode is the tab
	allowed     map[string]bool // nil = all tools; empty map = no tools
	active      bool            // false after Unregister until the next Register
	stable      bool            // true = canonical byAgent token; false = overlapping extra
}

// allows reports whether this binding may see/call the bare tool name.
func (b binding) allows(name string) bool {
	if b.allowed == nil {
		return true
	}
	return b.allowed[name]
}

// New builds a Server backed by rep (the engine's typed callback seam). rep must
// be safe for concurrent use; a nil rep is replaced with a no-op so the server
// never nil-dereferences.
func New(rep Reporter) *Server {
	if rep == nil {
		rep = noopReporter{}
	}
	return &Server{
		rep:     rep,
		tools:   allTools(),
		tokens:  map[string]binding{},
		byAgent: map[string]string{},
	}
}

// Start binds a free 127.0.0.1 port and serves until Close. It returns once the
// listener is open (so callers can immediately read URL/Port). Safe to call once.
func (s *Server) Start() error {
	addr := s.Addr
	if addr == "" {
		addr = "127.0.0.1:0"
	}
	ln, err := net.Listen("tcp", addr)
	if err != nil {
		return fmt.Errorf("mcp listen: %w", err)
	}
	s.listener = ln

	mux := http.NewServeMux()
	mux.HandleFunc("/", s.handle)
	s.httpSrv = &http.Server{
		Handler:           mux,
		ReadHeaderTimeout: 10 * time.Second,
	}
	go func() { _ = s.httpSrv.Serve(ln) }()
	return nil
}

// Close shuts the server down with a bounded deadline.
func (s *Server) Close() error {
	if s.httpSrv == nil {
		return nil
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	return s.httpSrv.Shutdown(ctx)
}

// Port returns the bound TCP port (0 if not started).
func (s *Server) Port() int {
	if s.listener == nil {
		return 0
	}
	return s.listener.Addr().(*net.TCPAddr).Port
}

// Register returns the MCP URL for this loop+agent. Sequential turns reuse the
// same token after Unregister so --mcp-config URLs do not rotate. allowed is
// replaced on reuse (mode switch). An overlapping Register while the canonical
// token is still active mints a distinct extra token so the two turns do not
// share an allowlist.
//
// allowed distinguishes three cases (nil vs empty matters):
//   - nil: unrestricted — advertise/allow every tool (legacy tests / e2e probes)
//   - non-nil (even empty): restricted to those bare names; an empty slice means
//     no Recur tools at all (tools/list empty, tools/call refuses). Use
//     NamesFromAllowedTools for the runner allowlist → bare-name conversion.
func (s *Server) Register(workspaceID, agentCode string, allowed []string) (token, url string) {
	return s.RegisterOwned(workspaceID, agentCode, "", allowed)
}

// RegisterOwned is Register with the calling turn's conductor tab. rename_session
// uses chatCode to retarget a specialist (PL, …) onto that tab when two walks
// share the same role code.
func (s *Server) RegisterOwned(workspaceID, agentCode, chatCode string, allowed []string) (token, url string) {
	b := binding{workspaceID: workspaceID, agentCode: agentCode, chatCode: strings.TrimSpace(chatCode), active: true}
	if allowed != nil {
		b.allowed = make(map[string]bool, len(allowed))
		for _, name := range allowed {
			if name != "" {
				b.allowed[name] = true
			}
		}
	}
	key := agentKey(workspaceID, agentCode)
	s.mu.Lock()
	defer s.mu.Unlock()
	if tok, ok := s.byAgent[key]; ok {
		if existing, live := s.tokens[tok]; live && existing.active {
			b.stable = false
			token = newToken()
			s.tokens[token] = b
			return token, s.URLFor(token)
		}
		b.stable = true
		s.tokens[tok] = b
		s.byAgent[key] = tok
		return tok, s.URLFor(tok)
	}
	token = newToken()
	b.stable = true
	s.tokens[token] = b
	s.byAgent[key] = token
	return token, s.URLFor(token)
}

// Unregister signals that an agent turn ended. Permission waits still unblock
// via the returned loop+agent. The canonical token is deactivated (404) so a
// leaked URL cannot call tools until the next Register; an overlapping extra
// token is revoked. ok is false for an unknown token.
func (s *Server) Unregister(token string) (workspaceID, agentCode string, ok bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	b, ok := s.tokens[token]
	if !ok {
		return "", "", false
	}
	if !b.stable {
		s.dropLocked(token)
		return b.workspaceID, b.agentCode, true
	}
	b.active = false
	s.tokens[token] = b
	return b.workspaceID, b.agentCode, true
}

// Drop revokes a token so it 404s. Used when the agent or workspace is gone.
func (s *Server) Drop(token string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.dropLocked(token)
}

// DropAgent revokes every token for one workspace agent (chat close), including
// overlapping extras that are not in byAgent.
func (s *Server) DropAgent(workspaceID, agentCode string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var toks []string
	for tok, b := range s.tokens {
		if b.workspaceID == workspaceID && b.agentCode == agentCode {
			toks = append(toks, tok)
		}
	}
	for _, tok := range toks {
		s.dropLocked(tok)
	}
}

// DropWorkspace revokes every token for a workspace (workspace delete).
func (s *Server) DropWorkspace(workspaceID string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var toks []string
	for tok, b := range s.tokens {
		if b.workspaceID == workspaceID {
			toks = append(toks, tok)
		}
	}
	for _, tok := range toks {
		s.dropLocked(tok)
	}
}

func (s *Server) dropLocked(token string) {
	b, ok := s.tokens[token]
	if !ok {
		return
	}
	delete(s.tokens, token)
	if b.stable {
		delete(s.byAgent, agentKey(b.workspaceID, b.agentCode))
	}
}

func agentKey(workspaceID, agentCode string) string {
	return workspaceID + "\x00" + agentCode
}

// URLFor builds the MCP endpoint URL for a token.
func (s *Server) URLFor(token string) string {
	return fmt.Sprintf("http://127.0.0.1:%d/%s/mcp", s.Port(), token)
}

// lookup resolves a token to its binding under the read lock.
func (s *Server) lookup(token string) (binding, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	b, ok := s.tokens[token]
	return b, ok
}

// handle is the single HTTP entry point. Path shape: /<token>/mcp. Only POST
// carries JSON-RPC; GET (the optional SSE stream of the streamable transport) is
// answered 405 since Recur's tools are request/response only — claude falls back
// to plain POST/JSON, which is all we need (headless ref §5).
func (s *Server) handle(w http.ResponseWriter, r *http.Request) {
	token := tokenFromPath(r.URL.Path)
	if token == "" {
		http.NotFound(w, r)
		return
	}
	b, ok := s.lookup(token)
	if !ok || !b.active {
		http.Error(w, "unknown mcp token", http.StatusNotFound)
		return
	}

	switch r.Method {
	case http.MethodPost:
		s.handlePost(w, r, b)
	case http.MethodGet, http.MethodDelete:
		// No server-initiated stream / session teardown needed for Recur's tools.
		w.Header().Set("Allow", "POST")
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	default:
		w.Header().Set("Allow", "POST")
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

// handlePost reads one JSON-RPC request and dispatches it. A notification (no id)
// gets a 202 with no body; a request gets a single JSON-RPC response object.
func (s *Server) handlePost(w http.ResponseWriter, r *http.Request, b binding) {
	body, err := io.ReadAll(io.LimitReader(r.Body, maxBody))
	if err != nil {
		writeJSON(w, http.StatusOK, newError(nil, codeParse, "read body: "+err.Error()))
		return
	}

	var req rpcRequest
	if err := json.Unmarshal(body, &req); err != nil {
		writeJSON(w, http.StatusOK, newError(nil, codeParse, "parse error"))
		return
	}
	if req.JSONRPC != jsonrpcVersion {
		writeJSON(w, http.StatusOK, newError(req.ID, codeInvalidRequest, "jsonrpc must be \"2.0\""))
		return
	}

	resp, isNotification := s.dispatch(r.Context(), req, b)
	if isNotification {
		// JSON-RPC notifications get no response body (e.g. notifications/initialized).
		w.WriteHeader(http.StatusAccepted)
		return
	}
	writeJSON(w, http.StatusOK, resp)
}

// dispatch routes one JSON-RPC method to its handler and returns the response (and
// whether the request was a notification, for which no body is written). Exported
// indirectly via tests so the protocol framing is unit-tested without HTTP.
//
// ctx is the HTTP request's context. It matters for permission_request, which
// blocks on a human: if the agent subprocess dies mid-wait, cancelling ctx is
// what releases the waiting handler.
func (s *Server) dispatch(ctx context.Context, req rpcRequest, b binding) (rpcResponse, bool) {
	switch req.Method {
	case "initialize":
		return s.onInitialize(req), false
	case "notifications/initialized", "initialized":
		return rpcResponse{}, true // notification: no response
	case "tools/list":
		return s.onListTools(req, b), false
	case "tools/call":
		return s.onCallTool(ctx, req, b), false
	case "ping":
		return newResult(req.ID, map[string]any{}), false
	default:
		if req.isNotification() {
			return rpcResponse{}, true // ignore unknown notifications
		}
		return newError(req.ID, codeMethodNotFound, "method not found: "+req.Method), false
	}
}

func (s *Server) onInitialize(req rpcRequest) rpcResponse {
	var p initializeParams
	_ = json.Unmarshal(req.Params, &p) // tolerate missing params
	version := protocolVersion
	if p.ProtocolVersion != "" {
		// Echo the client's requested version so negotiation succeeds; the wire
		// shapes Recur uses are stable across the supported revisions.
		version = p.ProtocolVersion
	}
	return newResult(req.ID, initializeResult{
		ProtocolVersion: version,
		Capabilities:    serverCapabilities{Tools: &toolsCapability{ListChanged: false}},
		ServerInfo:      implementation{Name: serverName, Version: serverVersion},
		Instructions:    "Use these tools to report structured signals back to the Recur orchestrator.",
	})
}

func (s *Server) onListTools(req rpcRequest, b binding) rpcResponse {
	out := make([]tool, 0, len(s.tools))
	for _, t := range s.tools {
		if !b.allows(t.name) {
			continue
		}
		out = append(out, tool{Name: t.name, Description: t.description, InputSchema: t.schema})
	}
	return newResult(req.ID, listToolsResult{Tools: out})
}

func (s *Server) onCallTool(ctx context.Context, req rpcRequest, b binding) rpcResponse {
	var p callToolParams
	if err := json.Unmarshal(req.Params, &p); err != nil {
		return newError(req.ID, codeInvalidParams, "invalid tools/call params")
	}
	spec, ok := s.toolByName(p.Name)
	if !ok {
		// Unknown tool name is a protocol-level error (not a tool result).
		return newError(req.ID, codeMethodNotFound, "unknown tool: "+p.Name)
	}
	if !b.allows(p.Name) {
		return newResult(req.ID, toolError("tool not available on this agent: "+p.Name))
	}
	// Backfill transcript tool_use args from the real tools/call payload before
	// the handler runs (Codex item frames often arrive as {}).
	s.rep.NoteToolArgs(b.workspaceID, b.agentCode, p.Name, p.Arguments)
	text, err := spec.handle(ctx, s.rep, b, p.Arguments)
	if err != nil {
		// Bad arguments → a tool-level IsError result (so the model self-corrects),
		// per MCP convention — NOT a JSON-RPC error.
		return newResult(req.ID, toolError(err.Error()))
	}
	return newResult(req.ID, toolText(text))
}

func (s *Server) toolByName(name string) (toolSpec, bool) {
	for _, t := range s.tools {
		if t.name == name {
			return t, true
		}
	}
	return toolSpec{}, false
}

// tokenFromPath extracts the <token> from a /<token>/mcp path. Returns "" if the
// path does not match.
func tokenFromPath(p string) string {
	p = strings.Trim(p, "/")
	parts := strings.Split(p, "/")
	if len(parts) != 2 || parts[1] != "mcp" || parts[0] == "" {
		return ""
	}
	return parts[0]
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}
