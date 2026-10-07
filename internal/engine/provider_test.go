package engine

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"recur/internal/cursor"
	"recur/internal/domain"
)

func TestProviderNeedsPermissionPromptTool(t *testing.T) {
	if !(*ClaudeRunner)(nil).NeedsPermissionPromptTool() {
		t.Error("ClaudeRunner must require permission_request on MCP bindings")
	}
	if (*CursorRunner)(nil).NeedsPermissionPromptTool() {
		t.Error("CursorRunner must not require permission_request on MCP bindings")
	}
	if !needsPermissionPromptTool(domain.ProviderClaudeCode) || !needsPermissionPromptTool("") {
		t.Error("needsPermissionPromptTool(claude/empty) = false, want true")
	}
	if needsPermissionPromptTool(domain.ProviderCursor) {
		t.Error("needsPermissionPromptTool(cursor) = true, want false")
	}
	if needsPermissionPromptTool(domain.ProviderCodex) {
		t.Error("needsPermissionPromptTool(codex) = true, want false (app-server approvals, not MCP)")
	}
	if (*CodexRunner)(nil).NeedsPermissionPromptTool() {
		t.Error("CodexRunner must not require permission_request on MCP bindings")
	}
}

func TestRunnerForClaudeCode(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, nil)
	// Clear the injected-nil marker so this exercises production routing.
	eng.runnerOverrideSet = false
	eng.runner = NewClaudeRunner()

	r, err := eng.resolveRunner(t.Context(), domain.ProviderClaudeCode, "")
	if err != nil {
		t.Fatalf("resolveRunner: %v", err)
	}
	if _, ok := r.(*ClaudeRunner); !ok {
		t.Errorf("runner = %T, want *ClaudeRunner", r)
	}
}

// An empty provider means Claude Code — that is what an unqualified model ref
// parses to, and it is the provider every install has.
func TestRunnerForEmptyProviderIsClaudeCode(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, nil)
	// Clear the injected-nil marker so this exercises production routing.
	eng.runnerOverrideSet = false
	eng.runner = NewClaudeRunner()

	r, err := eng.resolveRunner(t.Context(), "", "")
	if err != nil {
		t.Fatalf("resolveRunner: %v", err)
	}
	if _, ok := r.(*ClaudeRunner); !ok {
		t.Errorf("runner = %T, want *ClaudeRunner", r)
	}
}

// An unrecognised provider must fail loudly. Falling back to Claude Code would
// run — and bill — a model the user did not choose.
func TestRunnerForUnknownProviderFails(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, nil)
	// Clear the injected-nil marker so this exercises production routing.
	eng.runnerOverrideSet = false
	eng.runner = NewClaudeRunner()

	r, err := eng.runnerFor(t.Context(), "openai", "")
	if err == nil {
		t.Fatalf("want an error, got runner %T", r)
	}
	if r != nil {
		t.Errorf("runner = %T, want nil on failure", r)
	}
	if !strings.Contains(err.Error(), "openai") {
		t.Errorf("err = %v, want it to name the provider", err)
	}
}

// TestInjectedRunnerBeatsProvider keeps the suite hermetic: a settings blob
// naming Cursor must NOT redirect turns away from an injected runner. Asserted
// on DELIVERY rather than pointer identity, because SetRunner wraps what it is
// given in the session mirror — the injected runner is reached through a
// mirroringRunner, not returned as-is.
func TestInjectedRunnerBeatsProvider(t *testing.T) {
	var got int
	spy := &spyRunner{onRun: func(AgentSpec) { got++ }}
	eng, st, _, _ := newTestEngine(t, spy)

	set := st.Settings()
	set.ModelAgent = "cursor/composer-2.5"
	set.CursorAPIKey = "sk-should-never-be-used"
	st.SetSettings(set)

	// A Cursor-qualified model with no bridge installed would fail here if the
	// injected runner were not winning.
	if _, err := eng.runAgent(t.Context(), AgentSpec{Model: "cursor/composer-2.5"}, nil); err != nil {
		t.Fatalf("runAgent: %v", err)
	}
	if got != 1 {
		t.Errorf("injected runner ran %d turns, want 1", got)
	}
}

func TestRunAgentStreamsPartialsUnlessSkipThinking(t *testing.T) {
	var specs []AgentSpec
	eng, _, _, _ := newTestEngine(t, &spyRunner{onRun: func(s AgentSpec) { specs = append(specs, s) }})
	if _, err := eng.runAgent(t.Context(), AgentSpec{Role: domain.RoleReviewer}, nil); err != nil {
		t.Fatalf("reviewer: %v", err)
	}
	if _, err := eng.runAgent(t.Context(), AgentSpec{SkipThinking: true}, nil); err != nil {
		t.Fatalf("skip: %v", err)
	}
	if len(specs) != 2 {
		t.Fatalf("ran %d turns, want 2", len(specs))
	}
	if !specs[0].IncludePartials {
		t.Error("reviewer should stream thinking/text deltas into the cockpit")
	}
	if specs[1].IncludePartials {
		t.Error("SkipThinking one-shots must not request partials")
	}
}

func TestRunAgentPublishesUserDisplay(t *testing.T) {
	eng, _, sink, _ := newTestEngine(t, &spyRunner{onRun: func(AgentSpec) {}})
	display := "Review the current diff for iteration 1 of: pong"
	spec := AgentSpec{
		WorkspaceID: "W1", AgentID: "W1-RV", Role: domain.RoleReviewer,
		Prompt: "WIRE do not show this in the bubble", DisplayText: display,
	}
	if _, err := eng.runAgent(t.Context(), spec, nil); err != nil {
		t.Fatalf("runAgent: %v", err)
	}
	users := sink.byName("session.user")
	if len(users) != 1 {
		t.Fatalf("session.user events = %d, want 1", len(users))
	}
	d, _ := users[0].Data.(map[string]any)
	if d["text"] != display || d["code"] != "RV" {
		t.Errorf("session.user = %+v, want text=%q code=RV", d, display)
	}
}

func TestCursorProviderWithoutKeyFails(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	eng.runnerOverrideSet = false
	eng.runner = NewClaudeRunner()

	set := st.Settings()
	set.ModelAgent = "cursor/composer-2.5"
	set.CursorAPIKey = ""
	st.SetSettings(set)

	wt := t.TempDir()
	r, err := eng.runnerFor(t.Context(), domain.ProviderCursor, wt)
	if err == nil {
		t.Fatalf("want an error, got runner %T", r)
	}
	if !strings.Contains(err.Error(), "Connect") {
		t.Errorf("err = %v, want it to name Connect", err)
	}
	if r != nil {
		t.Errorf("runner = %T, want nil on failure", r)
	}
}

func TestCursorProviderDisabledFails(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	eng.runnerOverrideSet = false
	eng.runner = NewClaudeRunner()

	set := st.Settings()
	set.ModelAgent = "cursor/composer-2.5"
	set.CursorAPIKey = "sk-test"
	set.DisabledProviders = []string{domain.ProviderCursor}
	st.SetSettings(set)

	r, err := eng.runnerFor(t.Context(), domain.ProviderCursor, t.TempDir())
	if err == nil {
		t.Fatalf("want an error, got runner %T", r)
	}
	if !strings.Contains(err.Error(), "turned off") {
		t.Errorf("err = %v, want it to say Cursor is turned off", err)
	}
}

func TestClaudeProviderDisabledFails(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	eng.runnerOverrideSet = false
	eng.runner = NewClaudeRunner()

	set := st.Settings()
	set.DisabledProviders = []string{domain.ProviderClaudeCode}
	set.CursorAPIKey = "sk-test" // so Normalize isn't involved; store writes as-is
	st.SetSettings(set)

	r, err := eng.runnerFor(t.Context(), domain.ProviderClaudeCode, "")
	if err == nil {
		t.Fatalf("want an error, got runner %T", r)
	}
	if !strings.Contains(err.Error(), "turned off") {
		t.Errorf("err = %v, want it to say Claude Code is turned off", err)
	}
}

func TestCursorProviderMissingBridgeFails(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	eng.runnerOverrideSet = false
	eng.runner = NewClaudeRunner()

	set := st.Settings()
	set.ModelAgent = "cursor/composer-2.5"
	set.CursorAPIKey = "sk-test"
	st.SetSettings(set)

	if _, err := eng.runnerFor(t.Context(), domain.ProviderCursor, t.TempDir()); err == nil {
		t.Fatal("want an error when no bridge is installed")
	}
}

func TestCursorProviderRejectsUnpatchableSDK(t *testing.T) {
	eng, st, _, dataDir := newTestEngine(t, nil)
	writeFakeCursorBridge(t, dataDir)
	if err := os.RemoveAll(filepath.Join(dataDir, cursor.BridgeDirName, "node_modules")); err != nil {
		t.Fatal(err)
	}
	eng.runnerOverrideSet = false
	eng.runner = NewClaudeRunner()
	t.Cleanup(eng.cursor.Close)

	set := st.Settings()
	set.CursorAPIKey = "sk-test"
	st.SetSettings(set)

	_, err := eng.cursor.ensureRunner(t.Context(), st.Settings(), t.TempDir())
	if err == nil {
		t.Fatal("ensureRunner() succeeded on unpatchable SDK, want reinstall error")
	}
	if !strings.Contains(err.Error(), "reinstall") {
		t.Fatalf("err = %v, want reinstall", err)
	}
}

func TestCursorProviderRequiresWorktree(t *testing.T) {
	eng, st, _, dataDir := newTestEngine(t, nil)
	writeFakeCursorBridge(t, dataDir)
	eng.runnerOverrideSet = false
	eng.runner = NewClaudeRunner()
	t.Cleanup(eng.cursor.Close)

	set := st.Settings()
	set.CursorAPIKey = "sk-test"
	st.SetSettings(set)

	if _, err := eng.cursor.ensureRunner(t.Context(), st.Settings(), ""); err == nil {
		t.Fatal("ensureRunner(\"\") succeeded, want worktree required")
	}
	if _, err := eng.cursor.ensureRunner(t.Context(), st.Settings(), "/"); err == nil {
		t.Fatal("ensureRunner(\"/\") succeeded, want refusal")
	}
}

// TestCursorProviderRestartsDeadBridge is the self-heal guarantee: if the Node
// bridge process dies, ensureRunner must start a fresh one instead of handing
// out a client that dials the dead port (connection refused on CreateAgent).
func TestCursorDiedBeforeTurn(t *testing.T) {
	cases := []struct {
		err  error
		want bool
	}{
		{nil, false},
		{fmt.Errorf("cursor create agent: dial tcp: connection refused"), true},
		{fmt.Errorf("cursor run: dial tcp: connection refused"), false},
		{fmt.Errorf("cursor auth failed: connection refused"), false},
		{fmt.Errorf("some other error"), false},
	}
	for _, tc := range cases {
		if got := cursorDiedBeforeTurn(tc.err); got != tc.want {
			t.Errorf("cursorDiedBeforeTurn(%v) = %v, want %v", tc.err, got, tc.want)
		}
	}
}

func TestCursorProviderRestartsDeadBridge(t *testing.T) {
	eng, st, _, dataDir := newTestEngine(t, nil)
	writeFakeCursorBridge(t, dataDir)
	eng.runnerOverrideSet = false
	eng.runner = NewClaudeRunner()
	t.Cleanup(eng.cursor.Close)

	set := st.Settings()
	set.CursorAPIKey = "sk-test"
	st.SetSettings(set)

	wt := t.TempDir()
	r1, err := eng.cursor.ensureRunner(t.Context(), st.Settings(), wt)
	if err != nil {
		t.Fatalf("first ensureRunner: %v", err)
	}
	if !r1.Bridge.Alive() {
		t.Fatal("fresh bridge is not alive")
	}
	oldPort := r1.Bridge.Discovery().Port
	oldPID := r1.Bridge.Discovery().PID
	if oldPID == 0 {
		t.Fatal("fake bridge did not advertise a pid")
	}

	proc, err := os.FindProcess(oldPID)
	if err != nil {
		t.Fatalf("FindProcess: %v", err)
	}
	if err := proc.Kill(); err != nil {
		t.Fatalf("kill bridge: %v", err)
	}
	waitFor(t, 5*time.Second, "bridge process to be reaped", func() bool {
		return !r1.Bridge.Alive()
	})

	r2, err := eng.cursor.ensureRunner(t.Context(), st.Settings(), wt)
	if err != nil {
		t.Fatalf("ensureRunner after kill: %v", err)
	}
	if !r2.Bridge.Alive() {
		t.Fatal("self-healed bridge is not alive")
	}
	if r2.Bridge.Discovery().PID == oldPID {
		t.Fatal("ensureRunner reused the dead process instead of starting a new one")
	}
	if r2.Bridge.Discovery().Port == oldPort && r2.Bridge == r1.Bridge {
		t.Fatal("ensureRunner returned the stale bridge pointer")
	}
}

// TestCursorProviderPoolsBridgesByWorktree is the multi-worktree guarantee:
// two worktrees get two bridges (each with Workspace bound), and a second
// ensureRunner for the same worktree reuses the warm process.
func TestCursorProviderPoolsBridgesByWorktree(t *testing.T) {
	eng, st, _, dataDir := newTestEngine(t, nil)
	writeFakeCursorBridge(t, dataDir)
	eng.runnerOverrideSet = false
	eng.runner = NewClaudeRunner()
	t.Cleanup(eng.cursor.Close)

	set := st.Settings()
	set.CursorAPIKey = "sk-test"
	st.SetSettings(set)

	wt1 := t.TempDir()
	wt2 := t.TempDir()
	r1, err := eng.cursor.ensureRunner(t.Context(), st.Settings(), wt1)
	if err != nil {
		t.Fatalf("ensureRunner(wt1): %v", err)
	}
	r2, err := eng.cursor.ensureRunner(t.Context(), st.Settings(), wt2)
	if err != nil {
		t.Fatalf("ensureRunner(wt2): %v", err)
	}
	if r1.Bridge == r2.Bridge {
		t.Fatal("different worktrees must not share one bridge")
	}
	if got := r1.Bridge.Workspace; got != wt1 {
		t.Errorf("bridge1.Workspace = %q, want %q", got, wt1)
	}
	if got := r2.Bridge.Workspace; got != wt2 {
		t.Errorf("bridge2.Workspace = %q, want %q", got, wt2)
	}
	r1b, err := eng.cursor.ensureRunner(t.Context(), st.Settings(), wt1)
	if err != nil {
		t.Fatalf("ensureRunner(wt1 reuse): %v", err)
	}
	if r1b.Bridge != r1.Bridge {
		t.Fatal("same worktree must reuse the warm bridge")
	}
}

func TestEvictLockedSkipsInUseBridges(t *testing.T) {
	busy := &CursorRunner{}
	busy.inUse.Add(1)
	p := &cursorProvider{entries: map[string]*bridgeEntry{
		"/busy": {worktree: "/busy", lastUsed: time.Unix(1, 0), runner: busy},
		"/idle": {worktree: "/idle", lastUsed: time.Unix(2, 0), runner: &CursorRunner{}},
	}}
	p.evictLocked(1)
	if p.entries["/busy"] == nil {
		t.Fatal("evictLocked evicted the in-use bridge")
	}
	if p.entries["/idle"] != nil {
		t.Fatal("evictLocked kept the idle LRU entry, want it closed")
	}
}

func TestEvictLockedKeepsAllBusyOverCap(t *testing.T) {
	a, b := &CursorRunner{}, &CursorRunner{}
	a.inUse.Add(1)
	b.inUse.Add(1)
	p := &cursorProvider{entries: map[string]*bridgeEntry{
		"/a": {worktree: "/a", lastUsed: time.Unix(1, 0), runner: a},
		"/b": {worktree: "/b", lastUsed: time.Unix(2, 0), runner: b},
	}}
	p.evictLocked(1)
	if len(p.entries) != 2 {
		t.Fatalf("evictLocked kept %d entries, want 2 busy over cap", len(p.entries))
	}
}

func TestReservedHandoutSurvivesColdStartEviction(t *testing.T) {
	fill := func(handed *CursorRunner) *cursorProvider {
		t.Helper()
		p := &cursorProvider{entries: map[string]*bridgeEntry{
			"/handed": {worktree: "/handed", lastUsed: time.Unix(0, 0), runner: handed},
		}}
		for i := 1; i < maxCursorBridges; i++ {
			busy := &CursorRunner{}
			busy.inUse.Add(1)
			wt := fmt.Sprintf("/%d", i)
			p.entries[wt] = &bridgeEntry{
				worktree: wt,
				lastUsed: time.Unix(int64(i), 0),
				runner:   busy,
			}
		}
		return p
	}

	reserved := &CursorRunner{}
	reserved.inUse.Add(1)
	p := fill(reserved)
	p.evictLocked(maxCursorBridges - 1)
	if p.entries["/handed"] == nil {
		t.Fatal("evictLocked(maxCursorBridges-1) evicted the reserved handout")
	}

	idle := &CursorRunner{}
	p2 := fill(idle)
	p2.evictLocked(maxCursorBridges - 1)
	if p2.entries["/handed"] != nil {
		t.Fatal("evictLocked(maxCursorBridges-1) kept an unreserved handout, want it chosen as the idle victim")
	}
}

func TestEnsureRunnerReservesAgainstEviction(t *testing.T) {
	eng, st, _, dataDir := newTestEngine(t, nil)
	writeFakeCursorBridge(t, dataDir)
	eng.runnerOverrideSet = false
	eng.runner = NewClaudeRunner()
	t.Cleanup(eng.cursor.Close)

	set := st.Settings()
	set.CursorAPIKey = "sk-test"
	st.SetSettings(set)

	wt := t.TempDir()
	r, err := eng.cursor.ensureRunner(t.Context(), st.Settings(), wt)
	if err != nil {
		t.Fatalf("ensureRunner(%q) error = %v, want nil", wt, err)
	}
	if got := r.inUse.Load(); got != 1 {
		t.Fatalf("ensureRunner inUse = %d, want 1 (reserved before unlock)", got)
	}
	t.Cleanup(func() { eng.cursor.releaseRunner(r) })

	eng.cursor.mu.Lock()
	for i := 0; i < maxCursorBridges-1; i++ {
		busy := &CursorRunner{}
		busy.inUse.Add(1)
		key := fmt.Sprintf("/busy-%d", i)
		eng.cursor.entries[key] = &bridgeEntry{
			worktree: key,
			lastUsed: time.Unix(int64(i+1), 0),
			runner:   busy,
		}
	}
	eng.cursor.mu.Unlock()

	wt2 := t.TempDir()
	r2, err := eng.cursor.ensureRunner(t.Context(), st.Settings(), wt2)
	if err != nil {
		t.Fatalf("ensureRunner(%q) error = %v, want nil", wt2, err)
	}
	t.Cleanup(func() { eng.cursor.releaseRunner(r2) })
	if !r.Bridge.Alive() {
		t.Fatal("ensureRunner reserved runner was evicted by a later cold start")
	}
	eng.cursor.mu.Lock()
	_, kept := eng.cursor.entries[wt]
	eng.cursor.mu.Unlock()
	if !kept {
		t.Fatalf("ensureRunner pool entry %q missing after cold start, want reserved runner kept", wt)
	}
}

func TestRunAgentReleasesCursorReservation(t *testing.T) {
	r := &CursorRunner{}
	r.inUse.Add(1)
	eng, _, _, _ := newTestEngine(t, r)
	_, _ = eng.runAgent(t.Context(), AgentSpec{Model: "cursor/composer-2.5"}, nil)
	if got := r.inUse.Load(); got != 0 {
		t.Fatalf("runAgent left inUse = %d, want 0", got)
	}
}

func TestReleaseRunnerTrimsOverflowWhenIdle(t *testing.T) {
	p := &cursorProvider{entries: make(map[string]*bridgeEntry)}
	n := maxCursorBridges + 4
	runners := make([]*CursorRunner, n)
	for i := 0; i < n; i++ {
		r := &CursorRunner{}
		r.inUse.Add(1)
		runners[i] = r
		wt := fmt.Sprintf("/%d", i)
		p.entries[wt] = &bridgeEntry{
			worktree: wt,
			lastUsed: time.Unix(int64(i+1), 0),
			runner:   r,
		}
	}
	p.releaseRunner(runners[0])
	if got := len(p.entries); got != n-1 {
		t.Fatalf("releaseRunner() while others busy: %d entries, want %d (idle overflow trimmed)", got, n-1)
	}
	for _, r := range runners[1:] {
		p.releaseRunner(r)
	}
	if got := len(p.entries); got > maxCursorBridges {
		t.Fatalf("releaseRunner() after all idle: %d entries, want <= %d", got, maxCursorBridges)
	}
}

// writeFakeCursorBridge installs a tiny Python stand-in for cursor-sdk-bridge:
// it prints a valid discovery line, serves GetVersion/Ping/Shutdown, and stays
// up until killed — enough for ensureRunner's start + Alive self-heal path.
func writeFakeCursorBridge(t *testing.T, dataDir string) string {
	t.Helper()
	bin := filepath.Join(dataDir, cursor.BridgeDirName, "bin", "cursor-sdk-bridge")
	if err := os.MkdirAll(filepath.Dir(bin), 0o755); err != nil {
		t.Fatalf("mkdir fake bridge: %v", err)
	}
	const script = `#!/usr/bin/env python3
import json, os, sys
from http.server import BaseHTTPRequestHandler
from socketserver import TCPServer

class H(BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        if n:
            self.rfile.read(n)
        if self.path.endswith("/GetVersion"):
            body = b'{"bridgeVersion":"fake","protocolVersion":"sdk.v1","capabilities":["agent.create","agent.send"]}'
        else:
            body = b"{}"
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *args):
        pass

httpd = TCPServer(("127.0.0.1", 0), H)
port = httpd.server_address[1]
ws = os.getcwd()
for i, a in enumerate(sys.argv):
    if a == "--workspace" and i + 1 < len(sys.argv):
        ws = sys.argv[i + 1]
disc = {
    "schemaVersion": 1,
    "serverVersion": "fake",
    "pid": os.getpid(),
    "transport": "tcp",
    "protocol": "connect",
    "host": "127.0.0.1",
    "port": port,
    "url": f"http://127.0.0.1:{port}",
    "authToken": "test-token",
    "workspaceRef": ws,
}
print("cursor-sdk-bridge ready " + json.dumps(disc), file=sys.stderr, flush=True)
httpd.serve_forever()
`
	if err := os.WriteFile(bin, []byte(script), 0o755); err != nil {
		t.Fatalf("write fake bridge: %v", err)
	}
	// Owned installs run EnsureAttributionPatch on the ESM entrypoint.
	entry := filepath.Join(dataDir, cursor.BridgeDirName, "dist", "bin", "cursor-sdk-bridge.js")
	if err := os.MkdirAll(filepath.Dir(entry), 0o755); err != nil {
		t.Fatalf("mkdir fake entrypoint: %v", err)
	}
	if err := os.WriteFile(entry, []byte("console.log(1)\n"), 0o644); err != nil {
		t.Fatalf("write fake entrypoint: %v", err)
	}
	// Minify fixtures so EnsureAttributionPatch can stub getAgentSkills.
	// Without them ensureRunner fail-closes (unrecognized SDK).
	root := filepath.Join(dataDir, cursor.BridgeDirName)
	cjs := filepath.Join(root, "node_modules", "@cursor", "sdk", "dist", "cjs", "616.js")
	esm := filepath.Join(root, "node_modules", "@cursor", "sdk", "dist", "esm", "357.js")
	for _, path := range []string{cjs, esm} {
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatalf("mkdir %s: %v", path, err)
		}
	}
	if err := os.WriteFile(cjs, []byte("prefix;getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new Pb(n):new yx(n,m);suffix"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(esm, []byte("prefix;getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===b?new n.DvK(o):new l(o,b);suffix"), 0o644); err != nil {
		t.Fatal(err)
	}
	skip := `function(e){return"summary"===e.type||"summary-started"===e.type||"summary-completed"===e.type}(e)||(yield null===(n=(t=this.options).onDelta)||void 0===n?void 0:n.call(t,{update:e}))`
	for _, dir := range []string{filepath.Dir(cjs), filepath.Dir(esm)} {
		if err := os.WriteFile(filepath.Join(dir, "index.js"), []byte("prefix;"+skip+";suffix"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return bin
}

// --- model refs --------------------------------------------------------------

func TestParseModelRef(t *testing.T) {
	cases := []struct {
		in       string
		provider string
		id       string
	}{
		{"cursor/composer-2.5", domain.ProviderCursor, "composer-2.5"},
		{"codex/gpt-5.6-luna", domain.ProviderCodex, "gpt-5.6-luna"},
		{"claude-code/opus", domain.ProviderClaudeCode, "opus"},
		{"claude-code/claude-opus-4-8[1m]", domain.ProviderClaudeCode, "claude-opus-4-8[1m]"},
		// Unqualified is Claude Code — the default provider, not a fallback hack.
		{"opus", domain.ProviderClaudeCode, "opus"},
		{"", domain.ProviderClaudeCode, ""},
		{"  ", domain.ProviderClaudeCode, ""},
	}
	for _, tc := range cases {
		got := domain.ParseModelRef(tc.in)
		if got.Provider != tc.provider || got.ID != tc.id {
			t.Errorf("ParseModelRef(%q) = %+v, want {%s %s}", tc.in, got, tc.provider, tc.id)
		}
	}
}

func TestModelRefRoundTrip(t *testing.T) {
	for _, s := range []string{"cursor/composer-2.5", "claude-code/opus"} {
		if got := domain.ParseModelRef(s).String(); got != s {
			t.Errorf("round-trip %q → %q", s, got)
		}
	}
	// An empty selection stays empty rather than becoming "claude-code/".
	if got := domain.ParseModelRef("").String(); got != "" {
		t.Errorf("empty ref rendered as %q", got)
	}
}

// runAgent hands the runner a BARE model id — providers know nothing about refs.
func TestRunAgentStripsProviderPrefix(t *testing.T) {
	var seen string
	spy := &spyRunner{onRun: func(spec AgentSpec) { seen = spec.Model }}
	eng, _, _, _ := newTestEngine(t, spy)

	if _, err := eng.runAgent(t.Context(), AgentSpec{Model: "cursor/composer-2.5"}, nil); err != nil {
		t.Fatalf("runAgent: %v", err)
	}
	if seen != "composer-2.5" {
		t.Errorf("runner saw model %q, want the bare id", seen)
	}
}

// Modes can name different providers within one loop, since the model ref is
// what routes each turn. Loop specialists alias Loop/Agent/Review.
func TestSettingsPerModeProviders(t *testing.T) {
	set := domain.Settings{
		ModelLoop:   "claude-code/opus",
		ModelAgent:  "cursor/composer-2.5",
		ModelReview: "",
	}
	if got := set.MaestroModel(); got.Provider != domain.ProviderClaudeCode || got.ID != "opus" {
		t.Errorf("maestro/loop = %+v", got)
	}
	if got := set.CoderModel(); got.Provider != domain.ProviderCursor || got.ID != "composer-2.5" {
		t.Errorf("coder/agent = %+v", got)
	}
	// An unset review mode inherits Loop, provider and all.
	if got := set.ReviewerModel(); got.Provider != domain.ProviderClaudeCode || got.ID != "opus" {
		t.Errorf("reviewer = %+v, want the loop model's ref", got)
	}

	if !set.UsesProvider(domain.ProviderCursor) {
		t.Error("UsesProvider(cursor) = false with a Cursor agent model")
	}
	if !set.UsesProvider(domain.ProviderClaudeCode) {
		t.Error("UsesProvider(claude-code) = false with a Claude loop model")
	}
	if set.UsesProvider("openai") {
		t.Error("UsesProvider reported a provider no mode names")
	}
}

func TestSettingsCursorConfigured(t *testing.T) {
	if (domain.Settings{}).CursorConfigured() {
		t.Error("no key must mean not configured")
	}
	if (domain.Settings{CursorAPIKey: "   "}).CursorConfigured() {
		t.Error("whitespace-only key must mean not configured")
	}
	if !(domain.Settings{CursorAPIKey: "sk-x"}).CursorConfigured() {
		t.Error("a key must mean Cursor is configured")
	}
}

func TestSettingsCodexConfigured(t *testing.T) {
	if (domain.Settings{}).CodexConfigured() {
		t.Error("empty settings must not enable Codex")
	}
	if !(domain.Settings{CodexAPIKey: "sk-x"}).CodexConfigured() {
		t.Error("API key must enable Codex")
	}
	if !(domain.Settings{CodexChatGPTAuth: true}).CodexConfigured() {
		t.Error("ChatGPT auth opt-in must enable Codex")
	}
}

func TestCodexProviderWithoutConfigFails(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	eng.runnerOverrideSet = false
	eng.runner = NewClaudeRunner()
	set := st.Settings()
	set.CodexAPIKey = ""
	set.CodexChatGPTAuth = false
	st.SetSettings(set)
	if _, err := eng.runnerFor(t.Context(), domain.ProviderCodex, t.TempDir()); err == nil {
		t.Fatal("want error when Codex is not configured")
	}
}

func TestCodexProviderDisabledFails(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	eng.runnerOverrideSet = false
	eng.runner = NewClaudeRunner()
	set := st.Settings()
	set.CodexAPIKey = "sk-x"
	set.DisabledProviders = []string{domain.ProviderCodex}
	st.SetSettings(set)
	r, err := eng.runnerFor(t.Context(), domain.ProviderCodex, t.TempDir())
	if err == nil {
		t.Fatalf("want error, got runner %T", r)
	}
	if !strings.Contains(err.Error(), "turned off") {
		t.Errorf("err = %v, want it to say Codex is turned off", err)
	}
}

// spyRunner records the spec it was handed.
type spyRunner struct{ onRun func(AgentSpec) }

func (s *spyRunner) Run(_ context.Context, spec AgentSpec, _ func(ParsedEvent)) (Result, error) {
	s.onRun(spec)
	return Result{Subtype: "success"}, nil
}

type failRunner struct{ err error }

func (f failRunner) Run(context.Context, AgentSpec, func(ParsedEvent)) (Result, error) {
	return Result{}, f.err
}

func TestRunAgentPreservesCancelWhenCursorOff(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, failRunner{err: context.Canceled})
	set := st.Settings()
	set.CursorAPIKey = "sk-x"
	set.DisabledProviders = []string{domain.ProviderCursor}
	st.SetSettings(set)
	_, err := eng.runAgent(t.Context(), AgentSpec{Model: "cursor/composer-2.5"}, nil)
	if !errors.Is(err, context.Canceled) {
		t.Errorf("runAgent() error = %v, want context.Canceled", err)
	}
}

func TestRunAgentWrapsCursorOffError(t *testing.T) {
	orig := fmt.Errorf("connection refused")
	eng, st, _, _ := newTestEngine(t, failRunner{err: orig})
	set := st.Settings()
	set.CursorAPIKey = "sk-x"
	set.DisabledProviders = []string{domain.ProviderCursor}
	st.SetSettings(set)
	_, err := eng.runAgent(t.Context(), AgentSpec{Model: "cursor/composer-2.5"}, nil)
	if err == nil {
		t.Fatal("runAgent() error = nil, want wrapped turned-off error")
	}
	if !strings.Contains(err.Error(), "turned off") {
		t.Errorf("runAgent() error = %v, want it to say Cursor is turned off", err)
	}
	if !errors.Is(err, orig) {
		t.Errorf("runAgent() error = %v, want errors.Is match for %v", err, orig)
	}
}

// TestNoDirectRunnerDispatch is a static guard with teeth: every agent turn must
// go through Engine.runAgent, which resolves the provider from the spec's model
// ref AND is the seam SetRunner overrides.
//
// A call site that reaches for e.runner directly bypasses both. In production it
// silently pins the turn to Claude Code regardless of the model chosen; in a
// TEST it ignores the injected ScriptedRunner and makes a real, billed API call
// against the live service. Both happened during the Cursor provider rebase —
// once in workspace_chat.go, once in mirror_test.go — so this covers test files
// too, which is where the failure mode is most expensive and least visible.
func TestNoDirectRunnerDispatch(t *testing.T) {
	entries, err := os.ReadDir(".")
	if err != nil {
		t.Fatalf("read package dir: %v", err)
	}
	const needle = ".runner" + ".Run("
	for _, e := range entries {
		name := e.Name()
		if e.IsDir() || !strings.HasSuffix(name, ".go") {
			continue
		}
		// provider.go holds the dispatch seam itself; this file spells the needle.
		if name == "provider.go" || name == "provider_test.go" {
			continue
		}
		src, err := os.ReadFile(name)
		if err != nil {
			t.Fatalf("read %s: %v", name, err)
		}
		if strings.Contains(string(src), needle) {
			t.Errorf("%s dispatches a turn via %s — use e.runAgent so provider routing and SetRunner apply", name, needle)
		}
	}
}

// SetRunner(nil) must not degrade into "use the production runner". Several
// tests build an engine with no runner because they never dispatch; if that fell
// through to ClaudeRunner, any accidental dispatch would hit the live API.
func TestNilInjectedRunnerFailsClosed(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, nil)

	r, err := eng.runnerFor(t.Context(), domain.ProviderClaudeCode, "")
	if err == nil {
		t.Fatalf("want an error, got runner %T", r)
	}
	if r != nil {
		t.Errorf("runner = %T, want nil", r)
	}
	if _, ok := r.(*ClaudeRunner); ok {
		t.Error("a nil injected runner must never fall back to the production runner")
	}
}

// TestRunnerForAlwaysMirrors pins the invariant that makes the "Cursor runs show
// no streaming or tool calls" bug impossible: whatever a provider branch
// resolves, runnerFor wraps it in the session mirror.
//
// The original bug was a branch (Cursor) that returned its runner UNWRAPPED, so
// its turns never reached the mirror — no session, no transcript, no tool_use
// blocks, no session.delta. The run succeeded; the UI just had nothing to show.
//
// The runner here is deliberately RAW and installed on the non-override path,
// because that is the Cursor branch's exact shape. Using SetRunner would prove
// nothing: it pre-wraps what it stores, so runnerFor could drop its wrap
// entirely and the assertion would still pass.
func TestRunnerForAlwaysMirrors(t *testing.T) {
	for _, provider := range []string{domain.ProviderClaudeCode, ""} {
		eng, _, _, _ := newTestEngine(t, nil)
		eng.runnerOverrideSet = false
		eng.runner = &ScriptedRunner{} // raw, exactly as the Cursor branch returns

		raw, err := eng.resolveRunner(t.Context(), provider, "")
		if err != nil {
			t.Fatalf("resolveRunner(%q): %v", provider, err)
		}
		if _, wrapped := raw.(mirroringRunner); wrapped {
			t.Fatalf("test setup is wrong: resolveRunner(%q) already returns a mirroringRunner, so this proves nothing", provider)
		}

		r, err := eng.runnerFor(t.Context(), provider, "")
		if err != nil {
			t.Fatalf("runnerFor(%q): %v", provider, err)
		}
		if _, ok := r.(mirroringRunner); !ok {
			t.Errorf("runnerFor(%q) = %T, want a mirroringRunner", provider, r)
		}
	}
}

// And the raw resolution stays unwrapped, so the wrap in runnerFor is the ONLY
// thing standing between a provider and a missing transcript. If this ever
// returns a mirroringRunner, the invariant above has moved and the guard above
// stops proving anything.
func TestResolveRunnerIsRaw(t *testing.T) {
	scripted := &ScriptedRunner{}
	eng, _, _, _ := newTestEngine(t, nil)
	eng.runnerOverrideSet = false
	eng.runner = scripted

	r, err := eng.resolveRunner(t.Context(), domain.ProviderClaudeCode, "")
	if err != nil {
		t.Fatalf("resolveRunner: %v", err)
	}
	if r != AgentRunner(scripted) {
		t.Errorf("resolveRunner returned %T, want the raw runner verbatim", r)
	}
}

func TestCloseCursorBridgesDefersWhenAgentsRunning(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, nil)
	eng.cursor.mu.Lock()
	eng.cursor.entries = map[string]*bridgeEntry{"/tmp/wt": {worktree: "/tmp/wt"}}
	eng.cursor.mu.Unlock()

	done := make(chan struct{})
	rel, ok := eng.acquireAgent(done)
	if !ok {
		t.Fatal("acquireAgent() ok = false, want slot")
	}

	eng.CloseCursorBridges()
	eng.cursor.mu.Lock()
	_, still := eng.cursor.entries["/tmp/wt"]
	flag := eng.cursor.closeWhenIdle
	eng.cursor.mu.Unlock()
	if !still {
		t.Fatal("CloseCursorBridges() dropped the pool while an agent is running")
	}
	if !flag {
		t.Fatal("CloseCursorBridges() closeWhenIdle = false, want true while agents run")
	}

	rel()
	eng.cursor.mu.Lock()
	_, still = eng.cursor.entries["/tmp/wt"]
	flag = eng.cursor.closeWhenIdle
	eng.cursor.mu.Unlock()
	if still {
		t.Fatal("release did not close Cursor bridges after the last agent finished")
	}
	if flag {
		t.Fatal("closeWhenIdle = true after idle close, want false")
	}
}

func TestCloseCursorBridgesImmediateWhenIdle(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, nil)
	eng.cursor.mu.Lock()
	eng.cursor.entries = map[string]*bridgeEntry{"/tmp/wt": {worktree: "/tmp/wt"}}
	eng.cursor.mu.Unlock()
	eng.CloseCursorBridges()
	eng.cursor.mu.Lock()
	n := len(eng.cursor.entries)
	eng.cursor.mu.Unlock()
	if n != 0 {
		t.Errorf("CloseCursorBridges() entries = %d, want 0 when no agents run", n)
	}
}

func TestCancelDeferredCursorClose(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, nil)
	eng.cursor.mu.Lock()
	eng.cursor.entries = map[string]*bridgeEntry{"/tmp/wt": {worktree: "/tmp/wt"}}
	eng.cursor.mu.Unlock()
	done := make(chan struct{})
	rel, ok := eng.acquireAgent(done)
	if !ok {
		t.Fatal("acquireAgent() ok = false, want slot")
	}
	eng.CloseCursorBridges()
	eng.CancelDeferredCursorClose()
	rel()
	eng.cursor.mu.Lock()
	_, still := eng.cursor.entries["/tmp/wt"]
	eng.cursor.mu.Unlock()
	if !still {
		t.Fatal("CancelDeferredCursorClose() still tore the pool down on agent release")
	}
}
