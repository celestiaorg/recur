package engine

import (
	"context"
	"fmt"
	"strings"
	"sync"
	"testing"
	"time"

	"recur/internal/domain"
)

func TestSwitchModeAllowed(t *testing.T) {
	cases := []struct {
		from, to string
		ok       bool
	}{
		{domain.ChatModeAgent, domain.ChatModeAsk, true},
		{domain.ChatModeAgent, domain.ChatModePlan, true},
		{domain.ChatModeAgent, domain.ChatModeReview, true},
		{domain.ChatModeAsk, domain.ChatModePlan, true},
		{domain.ChatModePlan, domain.ChatModeReview, true},
		{domain.ChatModeReview, domain.ChatModeAsk, true},
		{domain.ChatModeAsk, domain.ChatModeAgent, false},
		{domain.ChatModePlan, domain.ChatModeAgent, false},
		{domain.ChatModeReview, domain.ChatModeAgent, false},
		{domain.ChatModeAsk, domain.ChatModeLoop, false},
	}
	for _, tc := range cases {
		if got := switchModeAllowed(tc.from, tc.to); got != tc.ok {
			t.Errorf("switchModeAllowed(%s→%s)=%v, want %v", tc.from, tc.to, got, tc.ok)
		}
	}
}

func TestSwitchChatModeSameModeNoRemount(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "same-mode", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "hi", "sonnet", nil, nil, domain.ChatModeAgent, "")

	got := eng.SwitchChatMode(ws.ID, "PA", domain.ChatModeAgent)
	if !strings.Contains(got, "Already in agent") {
		t.Fatalf("same-mode result = %q, want Already in agent", got)
	}
	if strings.Contains(got, "remounting") {
		t.Fatalf("same-mode must not remount: %q", got)
	}
}

func TestSwitchChatModeNoLiveTurnPersistsOnly(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "idle-switch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "hi", "sonnet", nil, nil, domain.ChatModeAgent, "")

	got := eng.SwitchChatMode(ws.ID, "PA", domain.ChatModePlan)
	if !strings.Contains(got, "Switched chat mode from agent to plan") {
		t.Fatalf("result = %q", got)
	}
	if strings.Contains(got, "remounting") {
		t.Fatalf("idle switch must not remount: %q", got)
	}
	if _, _, mode := eng.WorkspaceChatSelection(ws.ID, "PA"); mode != domain.ChatModePlan {
		t.Fatalf("persisted mode = %q, want plan", mode)
	}
	if !sink.has("chat.mode_changed") {
		t.Fatal("expected chat.mode_changed")
	}
}

func TestSwitchChatModeUpgradeRefused(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "no-upgrade", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "hi", "sonnet", nil, nil, domain.ChatModePlan, "")

	got := eng.SwitchChatMode(ws.ID, "PA", domain.ChatModeAgent)
	if !strings.Contains(got, "Cannot switch from plan to agent") {
		t.Fatalf("upgrade result = %q", got)
	}
	if _, _, mode := eng.WorkspaceChatSelection(ws.ID, "PA"); mode != domain.ChatModePlan {
		t.Fatalf("refused upgrade must keep plan, got %q", mode)
	}
}

func TestFormatModeSwitchResumePrompt(t *testing.T) {
	got := formatModeSwitchResumePrompt(domain.ChatModeAgent, domain.ChatModePlan)
	if !strings.Contains(got, "from agent to plan") {
		t.Fatalf("missing transition: %q", got)
	}
	if !strings.Contains(got, "mcp__recur__submit_plan") {
		t.Fatalf("plan remount should name submit_plan: %q", got)
	}
	if !strings.Contains(got, "Do not ask them to switch") {
		t.Fatalf("missing do-not-ask: %q", got)
	}
	if got := formatModeSwitchDisplay(domain.ChatModePlan); got != "Switched to Plan: continuing" {
		t.Fatalf("display = %q", got)
	}

	ask := formatModeSwitchResumePrompt(domain.ChatModeAgent, domain.ChatModeAsk)
	if !strings.Contains(ask, "mcp__recur__create_artifact") {
		t.Fatalf("ask remount should name create_artifact: %q", ask)
	}
	if !strings.Contains(ask, "do not edit repository files") {
		t.Fatalf("ask remount should keep the worktree read-only: %q", ask)
	}

	for _, to := range []string{domain.ChatModeAsk, domain.ChatModePlan, domain.ChatModeReview} {
		got := formatModeSwitchResumePrompt(domain.ChatModeAgent, to)
		if !strings.Contains(got, "git worktree") {
			t.Errorf("formatModeSwitchResumePrompt(agent, %q) missing git worktree, got %q", to, got)
		}
		if !strings.Contains(got, "mcp__recur__create_artifact") {
			t.Errorf("formatModeSwitchResumePrompt(agent, %q) missing create_artifact, got %q", to, got)
		}
	}
	plan := formatModeSwitchResumePrompt(domain.ChatModeAsk, domain.ChatModePlan)
	if !strings.Contains(plan, "never instead of, submit_plan") {
		t.Errorf("formatModeSwitchResumePrompt(ask, plan) should keep submit_plan primary, got %q", plan)
	}
	review := formatModeSwitchResumePrompt(domain.ChatModeAsk, domain.ChatModeReview)
	if !strings.Contains(review, "never instead of, report_finding") {
		t.Errorf("formatModeSwitchResumePrompt(ask, review) should keep report_finding primary, got %q", review)
	}
	if !strings.Contains(review, "update_finding") {
		t.Errorf("formatModeSwitchResumePrompt(ask, review) should mention update_finding, got %q", review)
	}
}

// holdFirstRunner records specs like captureRunner; the first Run blocks until
// ctx is cancelled (mid-turn switch_mode park). Later runs replay a success.
type holdFirstRunner struct {
	captureRunner
	started chan struct{}
}

func (r *holdFirstRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	r.mu.Lock()
	call := len(r.specs)
	r.specs = append(r.specs, spec)
	r.mu.Unlock()

	if call == 0 {
		if r.started != nil {
			close(r.started)
		}
		<-ctx.Done()
		return Result{}, ctx.Err()
	}
	sr := &ScriptedRunner{Lines: maestroFixture(fmt.Sprintf("mode-cont-%d", call), "continued")}
	return sr.Run(ctx, spec, emit)
}

func waitCoderTurns(t *testing.T, r *holdFirstRunner, n int) []AgentSpec {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		specs := r.specsWithRole(domain.RoleCoder)
		if len(specs) >= n {
			return specs
		}
		time.Sleep(15 * time.Millisecond)
	}
	t.Fatalf("timed out waiting for %d coder turns, have %d", n, len(r.specsWithRole(domain.RoleCoder)))
	return nil
}

func hasTool(spec AgentSpec, tool string) bool {
	for _, tl := range spec.AllowedTools {
		if tl == tool {
			return true
		}
	}
	return false
}

// TestSwitchChatModeRemountsWithNewTools: Agent→Plan mid-turn must park the
// current process and start a continuation whose allowlist is Plan (submit_plan,
// no Edit). The bug was persisting mode while this turn kept Agent tools.
func TestSwitchChatModeRemountsWithNewTools(t *testing.T) {
	runner := &holdFirstRunner{started: make(chan struct{})}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "remount", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	firstDone := make(chan string, 1)
	go func() {
		firstDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "build this feature", "sonnet", nil, nil, domain.ChatModeAgent, "")
	}()
	select {
	case <-runner.started:
	case <-time.After(2 * time.Second):
		t.Fatal("first turn did not start")
	}

	msg := eng.SwitchChatMode(ws.ID, "PA", domain.ChatModePlan)
	if !strings.Contains(msg, "Switched chat mode from agent to plan") {
		t.Fatalf("switch result = %q", msg)
	}
	if !strings.Contains(msg, "remounting") {
		t.Fatalf("live switch should remount: %q", msg)
	}

	select {
	case <-firstDone:
	case <-time.After(2 * time.Second):
		t.Fatal("first turn did not end after switch_mode park")
	}

	specs := waitCoderTurns(t, runner, 2)
	first, cont := specs[0], specs[1]
	if !hasTool(first, "Edit") {
		t.Fatalf("first turn should be Agent (Edit), tools=%v", first.AllowedTools)
	}
	if hasTool(first, mcpSubmitPlan) {
		t.Fatalf("first turn should not have submit_plan yet, tools=%v", first.AllowedTools)
	}
	if !hasTool(cont, mcpSubmitPlan) {
		t.Fatalf("continuation must allowlist submit_plan, tools=%v", cont.AllowedTools)
	}
	if hasTool(cont, "Edit") || hasTool(cont, "Write") {
		t.Fatalf("Plan continuation must be read-only, tools=%v", cont.AllowedTools)
	}
	if !strings.Contains(cont.Prompt, "from agent to plan") {
		t.Errorf("continuation prompt missing remount note: %q", cont.Prompt)
	}
	if !strings.Contains(cont.AppendSystemPrompt, "PLAN mode") {
		t.Errorf("continuation should be Plan-shaped, append=%q", cont.AppendSystemPrompt)
	}
	if cont.DisplayText != "Switched to Plan: continuing" {
		t.Errorf("continuation display = %q", cont.DisplayText)
	}
	if !cont.RefreshStandingInstructions {
		t.Error("mode remount must re-attach standing instructions on Cursor/Codex resume")
	}
	if !cont.SkipRecordUser {
		t.Error("remount continuation must skip the user bubble")
	}
	if _, _, mode := eng.WorkspaceChatSelection(ws.ID, "PA"); mode != domain.ChatModePlan {
		t.Errorf("persisted mode = %q, want plan", mode)
	}
}

func TestSwitchChatModeUserStopDoesNotRemount(t *testing.T) {
	runner := &holdFirstRunner{started: make(chan struct{})}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "stop-no-remount", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	firstDone := make(chan string, 1)
	go func() {
		firstDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "build this", "sonnet", nil, nil, domain.ChatModeAgent, "")
	}()
	select {
	case <-runner.started:
	case <-time.After(2 * time.Second):
		t.Fatal("first turn did not start")
	}

	_ = eng.SwitchChatMode(ws.ID, "PA", domain.ChatModePlan)
	// Stop before the park delay: cancel without parked → drop remount.
	if !eng.CancelChat(ws.ID, "PA") {
		t.Fatal("CancelChat returned false")
	}

	select {
	case <-firstDone:
	case <-time.After(2 * time.Second):
		t.Fatal("first turn did not end after Stop")
	}

	time.Sleep(askUserParkDelay + 200*time.Millisecond)
	if n := len(runner.specsWithRole(domain.RoleCoder)); n != 1 {
		t.Fatalf("user Stop must not remount, coder turns=%d", n)
	}
}

func TestParkChatTurnIfSkipsStoppedTurn(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	addSoloWorkspace(t, st, "W1")
	cancelled := 0
	turn := eng.registerChatTurn("W1", "PA", func() { cancelled++ })
	if turn == nil {
		t.Fatal("registerChatTurn returned nil")
	}
	if !eng.CancelChat("W1", "PA") {
		t.Fatal("CancelChat returned false")
	}
	if cancelled != 1 {
		t.Fatalf("CancelChat cancel count = %d, want 1", cancelled)
	}
	if eng.parkChatTurnIf("W1", "PA", turn) {
		t.Fatal("parkChatTurnIf must not mark a user-stopped turn parked")
	}
	if turn.isParked() {
		t.Fatal("stopped turn must not become parked")
	}
	if cancelled != 1 {
		t.Fatalf("park must not cancel again, count=%d", cancelled)
	}
}

// delayAfterCancelRunner holds the first turn after ctx cancel to simulate a
// real CLI teardown longer than askUserParkDelay.
type delayAfterCancelRunner struct {
	captureRunner
	started chan struct{}
	delay   time.Duration
}

func (r *delayAfterCancelRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	r.mu.Lock()
	call := len(r.specs)
	r.specs = append(r.specs, spec)
	r.mu.Unlock()
	if call == 0 {
		if r.started != nil {
			close(r.started)
		}
		<-ctx.Done()
		time.Sleep(r.delay)
		return Result{}, ctx.Err()
	}
	sr := &ScriptedRunner{Lines: maestroFixture(fmt.Sprintf("slow-cont-%d", call), "continued")}
	return sr.Run(ctx, spec, emit)
}

func TestSwitchChatModeUserStopDuringTeardownDoesNotRemount(t *testing.T) {
	runner := &delayAfterCancelRunner{
		started: make(chan struct{}),
		delay:   askUserParkDelay + 80*time.Millisecond,
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "stop-teardown", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	firstDone := make(chan string, 1)
	go func() {
		firstDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "build this", "sonnet", nil, nil, domain.ChatModeAgent, "")
	}()
	select {
	case <-runner.started:
	case <-time.After(2 * time.Second):
		t.Fatal("first turn did not start")
	}

	_ = eng.SwitchChatMode(ws.ID, "PA", domain.ChatModePlan)
	if !eng.CancelChat(ws.ID, "PA") {
		t.Fatal("CancelChat returned false")
	}

	select {
	case reply := <-firstDone:
		if reply == "" || strings.Contains(reply, "continued") {
			t.Fatalf("stopped turn reply = %q, want Stopped by user (not a remount)", reply)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("first turn did not end after teardown delay")
	}

	time.Sleep(200 * time.Millisecond)
	if n := len(runner.specsWithRole(domain.RoleCoder)); n != 1 {
		t.Fatalf("Stop during teardown must not remount, coder turns=%d", n)
	}
}

func TestSwitchChatModeRemountCap(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "remount-cap", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "hi", "sonnet", nil, nil, domain.ChatModeAgent, "")

	turn := eng.registerChatTurn(ws.ID, "PA", func() {})
	if turn == nil {
		t.Fatal("registerChatTurn returned nil")
	}
	seq := []string{domain.ChatModePlan, domain.ChatModeAsk, domain.ChatModeReview, domain.ChatModePlan}
	for i, to := range seq {
		msg := eng.SwitchChatMode(ws.ID, "PA", to)
		if i < maxModeRemounts {
			if !strings.Contains(msg, "remounting") {
				t.Fatalf("switch %d (%s) should remount: %q", i, to, msg)
			}
		} else if strings.Contains(msg, "remounting") {
			t.Fatalf("switch %d must hit remount cap: %q", i, msg)
		} else if !strings.Contains(msg, "Subsequent turns") {
			t.Fatalf("capped switch result = %q", msg)
		}
	}

	eng.unregisterChatTurn(ws.ID, "PA", turn)
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "go on", "sonnet", nil, nil, "", "")
	if eng.registerChatTurn(ws.ID, "PA", func() {}) == nil {
		t.Fatal("registerChatTurn after reset returned nil")
	}
	msg := eng.SwitchChatMode(ws.ID, "PA", domain.ChatModeAsk)
	if !strings.Contains(msg, "remounting") {
		t.Fatalf("user send should reset remount cap: %q", msg)
	}
}

func TestSwitchChatModeSkipsRemountWhenAskUserPending(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("ask-skip-%d", call), "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "ask-owns-park", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "hi", "sonnet", nil, nil, domain.ChatModeAgent, "")
	before := len(runner.specsWithRole(domain.RoleCoder))

	turn := eng.registerChatTurn(ws.ID, "PA", func() {})
	_ = eng.askUsers.park(domain.UserQuestion{
		WorkspaceID: ws.ID, AgentCode: "PA", Question: "which?",
	}, turn)
	turn.markParked()
	eng.noteModeContinue(ws.ID, "PA", domain.ChatModeAgent, domain.ChatModePlan, turn)
	eng.maybeContinueAfterModeSwitch(ws.ID, "PA", turn)
	time.Sleep(200 * time.Millisecond)

	if n := len(runner.specsWithRole(domain.RoleCoder)); n != before {
		t.Fatalf("pending ask_user must own the tab, coder turns=%d want %d", n, before)
	}
}

func TestEmitChatInboxSkipsPendingModeContinue(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "inbox-remount", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	turn := eng.registerChatTurn(ws.ID, pairAgentCode, func() {})
	eng.noteModeContinue(ws.ID, pairAgentCode, domain.ChatModeAgent, domain.ChatModePlan, turn)
	st.UpdateAgent(ws.ID, pairAgentCode, func(a *domain.Agent) { a.Unseen = true })
	if eng.emitChatInbox(ws.ID, pairAgentCode, false) {
		t.Fatal("pending remount must not emit a finished-reply inbox row")
	}
	if _, ok := inboxByID(eng, chatInboxID(ws.ID, pairAgentCode)); ok {
		t.Fatal("pending remount left a chat inbox row")
	}
}

func TestPersistChatFailureRecordsModeSwitchDeadEnd(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "dead-end", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "hi", "sonnet", nil, nil, domain.ChatModeAgent, "")
	prompt := formatModeSwitchResumePrompt(domain.ChatModeAgent, domain.ChatModePlan)
	display := formatModeSwitchDisplay(domain.ChatModePlan)
	note := "I can't run right now: mcp bind failed"
	eng.persistChatFailure(ws.ID, "PA", prompt, display, note, 0, Result{}, false, "")
	items := contentItems(eng.WorkspaceChatItems(ws.ID, "PA"))
	found := false
	for _, it := range items {
		if it.Kind == "text" && it.Text == note {
			found = true
		}
		if it.Role == "user" && isModeSwitchContinueDisplay(it.Text) {
			t.Fatalf("remount dead-end must not persist a continue user bubble: %+v", it)
		}
	}
	if !found {
		t.Fatalf("remount dead-end note missing, items=%+v", items)
	}
}

func TestIsModeSwitchResumeDeadEnd(t *testing.T) {
	if !isModeSwitchResumeDeadEnd("I'm at capacity right now — every agent slot is busy. Try again in a moment.") {
		t.Fatal("capacity reply should be a remount dead end")
	}
	if isModeSwitchResumeDeadEnd("continued") || isModeSwitchResumeDeadEnd("") {
		t.Fatal("normal/empty replies are not remount dead ends")
	}
}

func TestMaybeContinueRemountsWhenModelStopsBeforePark(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("pre-park-%d", call), "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "pre-park", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "hi", "sonnet", nil, nil, domain.ChatModeAgent, "")
	before := len(runner.specsWithRole(domain.RoleCoder))

	turn := &chatTurn{cancel: func() {}}
	eng.noteModeContinue(ws.ID, "PA", domain.ChatModeAgent, domain.ChatModePlan, turn)
	eng.maybeContinueAfterModeSwitch(ws.ID, "PA", turn)
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		if len(runner.specsWithRole(domain.RoleCoder)) > before {
			return
		}
		time.Sleep(15 * time.Millisecond)
	}
	t.Fatal("model finishing before park should still remount")
}

func TestFlagChatUnseenRaisesInboxWhenUnfocused(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "dead-end-unseen", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	_ = eng.SendWorkspaceMessage(ws.ID, pairAgentCode, "hi", "sonnet", nil, nil, domain.ChatModeAgent, "")
	eng.SetWorkspaceChatFocus(ws.ID, pairAgentCode)
	eng.SetAppFocused(false)
	eng.persistChatFailure(ws.ID, pairAgentCode, "wire", formatModeSwitchDisplay(domain.ChatModePlan),
		"I can't run right now: mcp bind failed", 0, Result{}, false, "")
	eng.flagChatUnseen(ws.ID, pairAgentCode, false)
	if a, ok := st.Agent(ws.ID, pairAgentCode); !ok || !a.Unseen {
		t.Fatalf("dead-end remount should mark unseen, ok=%v unseen=%v", ok, a.Unseen)
	}
	if _, ok := inboxByID(eng, chatInboxID(ws.ID, pairAgentCode)); !ok {
		t.Fatal("dead-end remount should emit a chat inbox row")
	}
}

func TestFlagChatUnseenLastEventMatchesStore(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "unseen-race", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	for i := 0; i < 50; i++ {
		eng.SetWorkspaceChatFocus("", "")
		start := make(chan struct{})
		var wg sync.WaitGroup
		wg.Add(2)
		go func() {
			defer wg.Done()
			<-start
			eng.flagChatUnseen(ws.ID, pairAgentCode, false)
		}()
		go func() {
			defer wg.Done()
			<-start
			eng.SetWorkspaceChatFocus(ws.ID, pairAgentCode)
		}()
		close(start)
		wg.Wait()
		a, ok := st.Agent(ws.ID, pairAgentCode)
		if !ok {
			t.Fatal("agent missing")
		}
		if a.Unseen {
			t.Fatalf("iter %d: focused tab left unseen", i)
		}
		var last *bool
		for _, ev := range sink.byName("attention.updated") {
			d, _ := ev.Data.(map[string]any)
			if d["code"] != pairAgentCode || d["workspaceId"] != ws.ID {
				continue
			}
			u, _ := d["tabUnseen"].(bool)
			last = &u
		}
		if last != nil && *last {
			t.Fatalf("iter %d: last tabUnseen=true after mark-seen, store Unseen=%v", i, a.Unseen)
		}
	}
}

func TestFlagChatUnseenFocusRecheckPublishesClear(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "unseen-focus-clear", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	for i := 0; i < 50; i++ {
		eng.SetWorkspaceChatFocus("", "")
		eng.flagChatUnseen(ws.ID, pairAgentCode, false)
		if a, ok := st.Agent(ws.ID, pairAgentCode); !ok || !a.Unseen {
			t.Fatal("need a prior unread tab")
		}
		if _, ok := inboxByID(eng, chatInboxID(ws.ID, pairAgentCode)); !ok {
			t.Fatal("need a prior inbox row")
		}
		attBefore := len(sink.byName("attention.updated"))
		inBefore := len(sink.byName("inbox.updated"))
		start := make(chan struct{})
		var wg sync.WaitGroup
		wg.Add(2)
		go func() {
			defer wg.Done()
			<-start
			eng.flagChatUnseen(ws.ID, pairAgentCode, true)
		}()
		go func() {
			defer wg.Done()
			<-start
			eng.SetWorkspaceChatFocus(ws.ID, pairAgentCode)
		}()
		close(start)
		wg.Wait()
		a, ok := st.Agent(ws.ID, pairAgentCode)
		if !ok {
			t.Fatal("agent missing")
		}
		if a.Unseen {
			t.Fatalf("iter %d: focused tab left unseen", i)
		}
		if _, ok := inboxByID(eng, chatInboxID(ws.ID, pairAgentCode)); ok {
			t.Fatalf("iter %d: focused tab left an inbox row", i)
		}
		var last *bool
		for _, ev := range sink.byName("attention.updated")[attBefore:] {
			d, _ := ev.Data.(map[string]any)
			if d["code"] != pairAgentCode || d["workspaceId"] != ws.ID {
				continue
			}
			u, _ := d["tabUnseen"].(bool)
			last = &u
		}
		if last == nil || *last {
			t.Fatalf("iter %d: store clear but no attention.updated(false)", i)
		}
		if len(sink.byName("inbox.updated")) == inBefore {
			t.Fatalf("iter %d: store inbox gone but no inbox.updated", i)
		}
	}
}

func TestModeSwitchContinueDoesNotDisplaceLiveTurn(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("disp-%d", call), "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "no-displace", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "hi", "sonnet", nil, nil, domain.ChatModeAgent, "")
	before := len(runner.specsWithRole(domain.RoleCoder))
	cancelled := false
	if eng.registerChatTurn(ws.ID, "PA", func() { cancelled = true }) == nil {
		t.Fatal("registerChatTurn returned nil")
	}
	reply := eng.SendWorkspaceMessage(ws.ID, "PA", formatModeSwitchResumePrompt(domain.ChatModeAgent, domain.ChatModePlan),
		"sonnet", nil, nil, domain.ChatModePlan, formatModeSwitchDisplay(domain.ChatModePlan))
	if cancelled {
		t.Fatal("remount send must not cancel a live user turn")
	}
	if reply != "" {
		t.Fatalf("displaced remount reply = %q, want empty", reply)
	}
	if n := len(runner.specsWithRole(domain.RoleCoder)); n != before {
		t.Fatalf("remount send ran over a live turn, coder turns=%d want %d", n, before)
	}
}

func TestResumeAfterModeSwitchBailsWhenTurnLive(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("live-%d", call), "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "handoff-clobber", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "hi", "sonnet", nil, nil, domain.ChatModeAgent, "")
	before := len(runner.specsWithRole(domain.RoleCoder))
	cancelled := false
	if eng.registerChatTurn(ws.ID, "PA", func() { cancelled = true }) == nil {
		t.Fatal("registerChatTurn returned nil")
	}
	eng.resumeAfterModeSwitch(ws.ID, "PA", domain.ChatModeAgent, domain.ChatModePlan, 0)
	if cancelled {
		t.Fatal("remount must not cancel a live user turn")
	}
	if n := len(runner.specsWithRole(domain.RoleCoder)); n != before {
		t.Fatalf("remount ran over a live turn, coder turns=%d want %d", n, before)
	}
}

func TestResumeAfterModeSwitchUsesDockModeForPrompt(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("dock-%d", call), "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "dock-flip", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "hi", "sonnet", nil, nil, domain.ChatModeAgent, "")
	eng.RememberWorkspaceChatSelection(ws.ID, "PA", "sonnet", nil, domain.ChatModeAsk, nil)
	gen := eng.allocContinueGen(ws.ID, "PA")
	eng.resumeAfterModeSwitch(ws.ID, "PA", domain.ChatModeAgent, domain.ChatModePlan, gen)
	specs := runner.specsWithRole(domain.RoleCoder)
	got := specs[len(specs)-1]
	if !strings.Contains(got.Prompt, "to ask") {
		t.Errorf("continuation prompt should follow dock mode, got %q", got.Prompt)
	}
	if !strings.Contains(got.AppendSystemPrompt, "ASK mode") {
		t.Errorf("continuation shape should be Ask, append=%q", got.AppendSystemPrompt)
	}
	if hasTool(got, mcpSubmitPlan) {
		t.Errorf("Ask continuation must not allowlist submit_plan, tools=%v", got.AllowedTools)
	}
}

func TestSwitchChatModeDroppedRemountDoesNotBurnBudget(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "uncount", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "hi", "sonnet", nil, nil, domain.ChatModeAgent, "")
	turn := eng.registerChatTurn(ws.ID, "PA", func() {})
	seq := []string{domain.ChatModePlan, domain.ChatModeAsk, domain.ChatModeReview}
	for _, to := range seq {
		msg := eng.SwitchChatMode(ws.ID, "PA", to)
		if !strings.Contains(msg, "remounting") {
			t.Fatalf("dropped switch to %s should still schedule a remount: %q", to, msg)
		}
		if !eng.CancelChat(ws.ID, "PA") {
			t.Fatal("CancelChat returned false")
		}
		eng.maybeContinueAfterModeSwitch(ws.ID, "PA", turn)
		turn = eng.registerChatTurn(ws.ID, "PA", func() {})
	}
	msg := eng.SwitchChatMode(ws.ID, "PA", domain.ChatModePlan)
	if !strings.Contains(msg, "remounting") {
		t.Fatalf("dropped remounts must not consume the cap: %q", msg)
	}
}

func TestClearModeContinuesForWorkspace(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "prune", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "hi", "sonnet", nil, nil, domain.ChatModeAgent, "")
	turn := eng.registerChatTurn(ws.ID, "PA", func() {})
	eng.noteModeContinue(ws.ID, "PA", domain.ChatModeAgent, domain.ChatModePlan, turn)
	if eng.bumpModeRemounts(ws.ID, "PA") != 1 {
		t.Fatal("expected depth 1")
	}
	eng.clearModeContinuesForWorkspace(ws.ID)
	if eng.pendingModeContinueFor(ws.ID, "PA") {
		t.Fatal("pending remount survived workspace clear")
	}
	if got := eng.bumpModeRemounts(ws.ID, "PA"); got != 1 {
		t.Fatalf("depth after clear+bump = %d, want 1", got)
	}
}

func TestMaybeContinueDoesNotStealNewerPending(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, &captureRunner{})
	old := &chatTurn{cancel: func() {}}
	newer := &chatTurn{cancel: func() {}}
	eng.noteModeContinue("ws-steal", "PA", domain.ChatModeAgent, domain.ChatModePlan, old)
	eng.noteModeContinue("ws-steal", "PA", domain.ChatModeAgent, domain.ChatModeAsk, newer)
	var gen uint64
	eng.modeContinueMu.Lock()
	if p, ok := eng.modeContinues[modeContinueKey("ws-steal", "PA")]; ok {
		gen = p.epoch
	}
	eng.modeContinueMu.Unlock()
	if gen == 0 {
		t.Fatal("newer remount should have a live generation")
	}
	eng.maybeContinueAfterModeSwitch("ws-steal", "PA", old)
	if !eng.pendingModeContinueFor("ws-steal", "PA") {
		t.Fatal("older turn consumed the newer remount")
	}
	if !eng.continueGenLive(gen) {
		t.Fatal("older turn dropped the newer remount generation")
	}
	p, ok := eng.takeModeContinueFor("ws-steal", "PA", newer)
	if !ok || p.turn != newer {
		t.Fatal("matching turn should still own the pending remount")
	}
	if !eng.continueGenLive(gen) {
		t.Fatal("taken remount generation should stay live until resume")
	}
	eng.dropContinueGen(gen)
}
