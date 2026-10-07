package engine

import (
	"context"
	"encoding/json"
	"strings"
	"sync"
	"testing"
	"time"

	"recur/internal/domain"
)

func TestSettingsPermissionModeIsGlobal(t *testing.T) {
	if got := (domain.Settings{BypassPermissions: true}).PermissionMode(); got != "bypassPermissions" {
		t.Errorf("bypass on: got %q", got)
	}
	// Off must be "default", not "dontAsk": default is the mode that raises a
	// permission request, and dontAsk would silently auto-deny instead of asking.
	if got := (domain.Settings{BypassPermissions: false}).PermissionMode(); got != "default" {
		t.Errorf("bypass off: got %q", got)
	}
}

func TestPermissionArgs(t *testing.T) {
	join := func(spec AgentSpec) string { return strings.Join(permissionArgs(spec), " ") }

	if got := join(AgentSpec{PermissionMode: "bypassPermissions", MCPURL: "http://x/mcp"}); got != "--permission-mode bypassPermissions" {
		t.Errorf("bypass must not wire a prompt tool, got %q", got)
	}
	want := "--permission-mode default --permission-prompt-tool " + permissionPromptTool
	if got := join(AgentSpec{PermissionMode: "default", MCPURL: "http://x/mcp"}); got != want {
		t.Errorf("ask mode: got %q, want %q", got, want)
	}
	// No MCP endpoint means nowhere to ask. Denying is the safe fallback;
	// leaving it on "default" would hang the turn on an unanswerable prompt.
	if got := join(AgentSpec{PermissionMode: "default"}); got != "--permission-mode dontAsk" {
		t.Errorf("ask mode without MCP: got %q", got)
	}
	if got := join(AgentSpec{}); got != "--permission-mode dontAsk" {
		t.Errorf("unset mode must fail closed, got %q", got)
	}
}

func TestDisallowedEditTools(t *testing.T) {
	if got := disallowedEditTools(nil); got != nil {
		t.Errorf("empty allowlist: got %v", got)
	}
	got := disallowedEditTools([]string{"Read", "Bash", "mcp__recur__report_finding"})
	for _, want := range editBuiltins {
		if !containsStr(got, want) {
			t.Errorf("missing disallowed %q in %v", want, got)
		}
	}
	if got := disallowedEditTools([]string{"Read", "Edit", "Write", "Bash", "NotebookEdit"}); len(got) != 0 {
		t.Errorf("full edit allowlist should disallow nothing, got %v", got)
	}
}

func TestDisallowedClaudeTools(t *testing.T) {
	// Empty allowlist still denies AskUserQuestion + Skill (unlike edit builtins).
	got := disallowedClaudeTools(nil)
	want := append([]string{askUserBuiltin, skillBuiltin}, subagentBuiltins...)
	if strings.Join(got, ",") != strings.Join(want, ",") {
		t.Errorf("empty allowlist: got %v, want %v", got, want)
	}
	ro := disallowedClaudeTools([]string{"Read", "Bash", "mcp__recur__ask_user"})
	for _, want := range []string{"Edit", "Write", "NotebookEdit", askUserBuiltin, skillBuiltin} {
		if !containsStr(ro, want) {
			t.Errorf("missing disallowed %q in %v", want, ro)
		}
	}
	coder := disallowedClaudeTools([]string{"Read", "Edit", "Write", "Bash", "NotebookEdit"})
	if strings.Join(coder, ",") != strings.Join(want, ",") {
		t.Errorf("full edit allowlist should only deny AskUserQuestion+Skill+subagents, got %v", coder)
	}
}

// ask starts a request on its own goroutine and returns a channel carrying the
// outcome, so a test can assert the call is still blocked before answering.
func ask(b *permissionBroker, ctx context.Context, req domain.PermissionRequest) chan struct {
	d   domain.PermissionDecision
	err error
} {
	out := make(chan struct {
		d   domain.PermissionDecision
		err error
	}, 1)
	go func() {
		d, err := b.request(ctx, req)
		out <- struct {
			d   domain.PermissionDecision
			err error
		}{d, err}
	}()
	return out
}

func waitForPending(t *testing.T, b *permissionBroker) domain.PermissionRequest {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		if p := b.Pending(); len(p) > 0 {
			return p[0]
		}
		time.Sleep(time.Millisecond)
	}
	t.Fatal("no permission request became pending")
	return domain.PermissionRequest{}
}

func TestPermissionBrokerBlocksUntilAnswered(t *testing.T) {
	b := newPermissionBroker()
	var notified []domain.PermissionRequest
	var mu sync.Mutex
	b.notify = func(r domain.PermissionRequest) {
		mu.Lock()
		defer mu.Unlock()
		notified = append(notified, r)
	}

	out := ask(b, context.Background(), domain.PermissionRequest{
		WorkspaceID: "W1", AgentCode: "C", Tool: "Bash",
	})
	req := waitForPending(t, b)

	select {
	case <-out:
		t.Fatal("request returned before it was answered")
	case <-time.After(20 * time.Millisecond):
	}

	if !b.Answer(req.ID, domain.PermissionDecision{Allow: true}) {
		t.Fatal("Answer reported the request as unknown")
	}
	got := <-out
	if got.err != nil || !got.d.Allow {
		t.Fatalf("got %+v, err %v; want allow", got.d, got.err)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(notified) != 1 || notified[0].Tool != "Bash" {
		t.Errorf("notify got %+v", notified)
	}
}

func TestPermissionBrokerAnswerIsIdempotent(t *testing.T) {
	b := newPermissionBroker()
	out := ask(b, context.Background(), domain.PermissionRequest{WorkspaceID: "W1", AgentCode: "C", Tool: "Bash"})
	req := waitForPending(t, b)

	if !b.Answer(req.ID, domain.PermissionDecision{Allow: true}) {
		t.Fatal("first answer should land")
	}
	<-out
	// A second click, or a second window, must not panic or block on the
	// already-drained channel.
	if b.Answer(req.ID, domain.PermissionDecision{Allow: false}) {
		t.Error("answering twice should report the request as gone")
	}
	if b.Answer("perm-nope", domain.PermissionDecision{}) {
		t.Error("unknown id should report false")
	}
}

func TestPermissionBrokerTimesOutAsDenial(t *testing.T) {
	b := newPermissionBroker()
	b.timeout = 10 * time.Millisecond
	d, err := b.request(context.Background(), domain.PermissionRequest{WorkspaceID: "W1", AgentCode: "C", Tool: "Bash"})
	if err != nil {
		t.Fatalf("timeout should be a verdict, not an error: %v", err)
	}
	if d.Allow {
		t.Error("timeout must deny")
	}
	if !strings.Contains(d.Reason, "no answer") {
		t.Errorf("timeout reason should tell the model why: %q", d.Reason)
	}
	if len(b.Pending()) != 0 {
		t.Error("timed-out request should leave the queue")
	}
}

func TestPermissionBrokerReleasesOnDeadAgent(t *testing.T) {
	b := newPermissionBroker()
	var resolved []string
	var mu sync.Mutex
	b.resolved = func(id string, _ domain.PermissionDecision) {
		mu.Lock()
		defer mu.Unlock()
		resolved = append(resolved, id)
	}

	ctx, cancel := context.WithCancel(context.Background())
	out := ask(b, ctx, domain.PermissionRequest{WorkspaceID: "W1", AgentCode: "C", Tool: "Bash"})
	req := waitForPending(t, b)

	cancel() // the agent subprocess went away mid-wait
	got := <-out
	if got.err == nil {
		t.Fatal("a cancelled request should report abandonment")
	}
	if len(b.Pending()) != 0 {
		t.Error("abandoned request should leave the queue")
	}
	// The prompt must be retracted too, or the human is left answering a
	// question no agent is waiting on.
	mu.Lock()
	defer mu.Unlock()
	if len(resolved) != 1 || resolved[0] != req.ID {
		t.Errorf("abandoning should publish a resolution, got %v", resolved)
	}
}

func TestPermissionBrokerEndTurnReleasesWaiters(t *testing.T) {
	b := newPermissionBroker()
	out := ask(b, context.Background(), domain.PermissionRequest{WorkspaceID: "W1", AgentCode: "C", Tool: "Bash"})
	waitForPending(t, b)

	b.endTurn("W1", "C")
	got := <-out
	if got.err != nil || got.d.Allow {
		t.Fatalf("ending a turn should deny anything still pending, got %+v err %v", got.d, got.err)
	}
	if len(b.Pending()) != 0 {
		t.Error("pending queue should be empty after the turn ended")
	}
}

func TestPermissionBrokerRemembersForTheTurn(t *testing.T) {
	b := newPermissionBroker()
	out := ask(b, context.Background(), domain.PermissionRequest{WorkspaceID: "W1", AgentCode: "C", Tool: "Bash"})
	req := waitForPending(t, b)
	b.Answer(req.ID, domain.PermissionDecision{Allow: true, Remember: true})
	<-out

	// Same tool, same turn: answered from the remembered set without asking again.
	d, err := b.request(context.Background(), domain.PermissionRequest{WorkspaceID: "W1", AgentCode: "C", Tool: "Bash"})
	if err != nil || !d.Allow {
		t.Fatalf("remembered tool should auto-allow, got %+v err %v", d, err)
	}

	// A different tool still asks.
	other := ask(b, context.Background(), domain.PermissionRequest{WorkspaceID: "W1", AgentCode: "C", Tool: "Write"})
	got := waitForPending(t, b)
	if got.Tool != "Write" {
		t.Errorf("expected Write to still prompt, pending = %q", got.Tool)
	}
	b.Answer(got.ID, domain.PermissionDecision{Allow: true})
	<-other

	// The memory is turn-scoped, so the next turn starts clean.
	b.endTurn("W1", "C")
	next := ask(b, context.Background(), domain.PermissionRequest{WorkspaceID: "W1", AgentCode: "C", Tool: "Bash"})
	if p := waitForPending(t, b); p.Tool != "Bash" {
		t.Errorf("after the turn ended, Bash should prompt again, got %q", p.Tool)
	}
	b.endTurn("W1", "C")
	<-next
}

func TestPermissionSurfaceFor(t *testing.T) {
	e := &Engine{}
	if got := e.permissionSurfaceFor("W", "PA"); got != permissionSurfaceChat {
		t.Errorf("solo chat: got %q, want chat", got)
	}

	e.loops.Store("W", &runningLoop{loop: &Loop{ID: "W"}})
	if got := e.permissionSurfaceFor("W", "CD"); got != permissionSurfaceLoop {
		t.Errorf("unattended loop agent: got %q, want loop", got)
	}
	// A PA/MA tab mid-turn stays chat even while the workspace loop is live —
	// that is the human-watched transcript, not autopilot.
	e.registerChatTurn("W", "PA", func() {})
	if got := e.permissionSurfaceFor("W", "PA"); got != permissionSurfaceChat {
		t.Errorf("PA chat during loop: got %q, want chat", got)
	}
	e.registerChatTurn("W", "MA", func() {})
	if got := e.permissionSurfaceFor("W", "MA"); got != permissionSurfaceChat {
		t.Errorf("Maestro chat during loop: got %q, want chat", got)
	}
}

func TestPermissionToolLabel(t *testing.T) {
	cases := map[string]string{
		"Bash":                       "Bash",
		"mcp__recur__list_findings":  "List findings",
		"mcp__recur__report_finding": "Report finding",
		"mcp__recur__activate_skill": "Activate skill",
		"mcp__recur__message_child":  "Message child workspace",
		"mcp__recur__unknown_widget": "Unknown widget",
		"mcp__other__do_thing":       "Do thing",
	}
	for tool, want := range cases {
		if got := permissionToolLabel(tool); got != want {
			t.Errorf("label(%s) = %q, want %q", tool, got, want)
		}
	}
}

func TestPermissionSummaryShowsTheDecidingDetail(t *testing.T) {
	cases := []struct {
		tool, input, want string
	}{
		{"Bash", `{"command":"rm -rf build","description":"clean"}`, "Bash: rm -rf build"},
		{"Write", `{"file_path":"/tmp/x.go","content":"package x"}`, "Write: /tmp/x.go"},
		{"WebFetch", `{"url":"https://example.com"}`, "WebFetch: https://example.com"},
		{"Weird", `{"zeta":1,"alpha":2}`, "Weird (alpha, zeta)"},
		{"Broken", `not json`, "Broken"},
		{"Empty", `{}`, "Empty"},
		{"mcp__recur__list_findings", `{"status":"open"}`, "List findings: open"},
		{"mcp__recur__update_finding", `{"finding_id":"F1","status":"resolved"}`, "Update finding: resolved"},
		{"mcp__recur__done", `{"summary":"wired the toggle"}`, "Complete turn: wired the toggle"},
		{"mcp__recur__open_pr", `{"title":"Ship the gateway","body":"long"}`, "Open PR: Ship the gateway"},
		{"mcp__recur__create_workspace", `{"title":"ACME-12: Fix login","prompt":"long"}`, "Create workspace: ACME-12: Fix login"},
	}
	for _, c := range cases {
		if got := permissionSummary(c.tool, json.RawMessage(c.input)); got != c.want {
			t.Errorf("summary(%s, %s) = %q, want %q", c.tool, c.input, got, c.want)
		}
	}
	// A multi-line command must stay one line so it fits a compact prompt.
	got := permissionSummary("Bash", json.RawMessage(`{"command":"line one\nline two"}`))
	if strings.Contains(got, "\n") {
		t.Errorf("summary should collapse to one line, got %q", got)
	}
}
