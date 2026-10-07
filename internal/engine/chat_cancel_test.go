package engine

import (
	"context"
	"sync/atomic"
	"testing"
	"time"

	"recur/internal/domain"
)

// TestCancelWorkspaceMessage interrupts an in-flight coding-agent chat turn:
// CancelChat cancels the turn context, the runner unwinds, status returns to
// idle, and the reply is the calm "Stopped by user" note (not an agent-error scare).
func TestCancelWorkspaceMessage(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ctxRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	done := make(chan string, 1)
	go func() {
		done <- eng.SendWorkspaceMessage(ws.ID, "PA", "long task", "sonnet", nil, nil, "", "")
	}()

	waitFor(t, 3*time.Second, "chat agent working", func() bool {
		a, ok := st.Agent(ws.ID, "PA")
		return ok && a.Status == "working"
	})

	if !eng.CancelChat(ws.ID, "PA") {
		t.Fatal("CancelChat returned false; no turn was registered")
	}

	select {
	case reply := <-done:
		if reply != chatStoppedReply {
			t.Fatalf("reply=%q, want %q", reply, chatStoppedReply)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("SendWorkspaceMessage did not return after CancelChat")
	}

	waitFor(t, 2*time.Second, "chat agent idle", func() bool {
		a, ok := st.Agent(ws.ID, "PA")
		return ok && a.Status == "idle"
	})

	items := eng.WorkspaceChatItems(ws.ID, "PA")
	var sawStop bool
	var footers int
	for _, it := range items {
		if it.Kind == "text" && it.Text == chatStoppedReply {
			sawStop = true
		}
		if it.Kind == "turn" && it.DurationMS > 0 {
			footers++
		}
	}
	if !sawStop || footers != 1 {
		t.Fatalf("transcript=%+v, want stop note + exactly 1 turn footer", items)
	}
}

func TestCancelChatIdlesLoopWalk(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ctxRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	reply := eng.SendWorkspaceMessage(ws.ID, "PA", "review and fix", "", nil, nil, "loop", "Review & fix")
	if reply != "" {
		t.Fatalf("engage reply = %q, want empty", reply)
	}

	waitFor(t, 3*time.Second, "specialist running", func() bool {
		return eng.Governor().AgentsRunning() >= 1
	})

	if !eng.CancelChat(ws.ID, "PA") {
		t.Fatal("CancelChat returned false; walk should idle")
	}

	waitFor(t, 3*time.Second, "walk idle", func() bool {
		got, ok := st.Workspace(ws.ID)
		return ok && got.State == domain.StatePaused && got.Phase == "idle"
	})
	got, _ := st.Workspace(ws.ID)
	if got.State == domain.StateStopped {
		t.Fatal("composer Stop parked StateStopped; want idle so the conductor can reenter")
	}
	if eng.runningLoopByID(ws.ID) != nil {
		t.Fatal("walk still registered after composer Stop")
	}
	waitFor(t, 2*time.Second, "agent slot released", func() bool {
		return eng.Governor().AgentsRunning() == 0
	})
}

func TestCancelChatIdlesQueuedLoop(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	defer func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	}()

	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "review and fix", "", nil, nil, "loop", "Review & fix")
	got, _ := st.Workspace(ws.ID)
	if got.State != domain.StateQueued {
		t.Fatalf("state = %q, want queued", got.State)
	}
	if !eng.CancelChat(ws.ID, "PA") {
		t.Fatal("CancelChat returned false for queued walk")
	}
	got, _ = st.Workspace(ws.ID)
	if got.State != domain.StatePaused || got.Phase != "idle" {
		t.Fatalf("state=%s phase=%q, want paused/idle", got.State, got.Phase)
	}
}

// TestCancelMaestroMessage interrupts an in-flight Maestro chat turn the same way.
func TestCancelMaestroMessage(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ctxRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	done := make(chan string, 1)
	go func() { done <- eng.SendMaestroMessage(ws.ID, "status?", nil) }()

	waitFor(t, 3*time.Second, "maestro directing", func() bool {
		a, ok := st.Agent(ws.ID, maestroAgentCode)
		return ok && a.Status == "directing"
	})

	if !eng.CancelChat(ws.ID, maestroAgentCode) {
		t.Fatal("CancelChat returned false for Maestro turn")
	}

	select {
	case reply := <-done:
		if reply != chatStoppedReply {
			t.Fatalf("reply=%q, want %q", reply, chatStoppedReply)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("SendMaestroMessage did not return after CancelChat")
	}

	waitFor(t, 2*time.Second, "maestro idle", func() bool {
		a, ok := st.Agent(ws.ID, maestroAgentCode)
		return ok && a.Status == "idle"
	})
}

func TestCancelChatNoopWhenIdle(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, &ScriptedRunner{})
	if eng.CancelChat("missing", "PA") {
		t.Fatal("expected false when nothing is running")
	}
}

func TestFinalizeChatReplyCanceled(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	code := pairAgentCode
	eng.ensurePairAgent(ws.ID, code, "sonnet")

	started := time.Now().Add(-1500 * time.Millisecond)
	out := eng.finalizeChatReply(ws.ID, code, "hi", "", "partial…", Result{}, context.Canceled, "Done.", started, false, "")
	if out != chatStoppedReply {
		t.Fatalf("out=%q, want %q", out, chatStoppedReply)
	}
	items := eng.WorkspaceChatItems(ws.ID, code)
	var dur int64
	for _, it := range items {
		if it.Kind == "turn" {
			dur = it.DurationMS
		}
	}
	if dur < 1000 {
		t.Fatalf("stopped turn duration=%d, want >= 1000ms (items=%+v)", dur, items)
	}
}

func TestCancelChatOneFooterFromReturnedDuration(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &durationCancelRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	done := make(chan string, 1)
	go func() {
		done <- eng.SendWorkspaceMessage(ws.ID, "PA", "long task", "sonnet", nil, nil, "", "")
	}()

	waitFor(t, 3*time.Second, "chat agent working", func() bool {
		a, ok := st.Agent(ws.ID, "PA")
		return ok && a.Status == "working"
	})

	if !eng.CancelChat(ws.ID, "PA") {
		t.Fatal("CancelChat returned false; no turn was registered")
	}

	select {
	case reply := <-done:
		if reply != chatStoppedReply {
			t.Fatalf("reply=%q, want %q", reply, chatStoppedReply)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("SendWorkspaceMessage did not return after CancelChat")
	}

	items := eng.WorkspaceChatItems(ws.ID, "PA")
	var footers int
	var dur int64
	for _, it := range items {
		if it.Kind == "turn" {
			footers++
			dur = it.DurationMS
		}
	}
	if footers != 1 {
		t.Fatalf("flattenTranscript turn footers = %d, want 1 (items=%+v)", footers, items)
	}
	if dur != 1500 {
		t.Errorf("turn footer DurationMS = %d, want 1500 from the cancelled Result (not a second KindResult stamp)", dur)
	}
}

func TestParkedChatHasNoTurnFooter(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &durationCancelRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	done := make(chan string, 1)
	go func() {
		done <- eng.SendWorkspaceMessage(ws.ID, "PA", "ask something", "sonnet", nil, nil, "", "")
	}()

	waitFor(t, 3*time.Second, "chat agent working", func() bool {
		a, ok := st.Agent(ws.ID, "PA")
		return ok && a.Status == "working"
	})

	if !eng.ParkChatTurn(ws.ID, "PA") {
		t.Fatal("ParkChatTurn returned false; no turn was registered")
	}

	select {
	case reply := <-done:
		if reply != "working on it" {
			t.Fatalf("parked reply=%q, want streamed text (quiet)", reply)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("SendWorkspaceMessage did not return after ParkChatTurn")
	}

	items := eng.WorkspaceChatItems(ws.ID, "PA")
	for _, it := range items {
		if it.Kind == "turn" {
			t.Fatalf("parked transcript has turn footer %+v; KindResult on cancel must not close the turn", items)
		}
	}
}

func TestChatTurnTryInjectRefusesAfterStop(t *testing.T) {
	turn := &chatTurn{cancel: func() {}}
	if turn.armChatInject() == nil {
		t.Fatal("armChatInject returned nil")
	}
	turn.markStopped()
	if turn.tryInject("steer") {
		t.Fatal("tryInject after Stop must return false")
	}
}

func TestChatTurnTryInjectRefusesAfterPark(t *testing.T) {
	turn := &chatTurn{cancel: func() {}}
	if turn.armChatInject() == nil {
		t.Fatal("armChatInject returned nil")
	}
	turn.markParked()
	if turn.tryInject("steer") {
		t.Fatal("tryInject after park must return false")
	}
}

func TestChatTurnEndedWaitsWhenStopped(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ctxRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "stop wait", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	turn := eng.registerChatTurn(ws.ID, "PA", func() {})
	if turn == nil {
		t.Fatal("registerChatTurn returned nil")
	}
	turn.markStopped()
	ch := eng.chatTurnEnded(ws.ID, "PA")
	if ch == nil {
		t.Fatal("stopped turn must expose ended so a post-Stop send waits for unregister")
	}
}

func TestCancelChatUnblocksEndedWaitWithoutOwner(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "leaked wait", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	turn := eng.registerChatTurn(ws.ID, "PA", func() {})
	if turn == nil {
		t.Fatal("registerChatTurn returned nil")
	}
	turn.dropOwner()
	done := make(chan string, 1)
	go func() {
		done <- eng.SendWorkspaceMessage(ws.ID, "PA", "go on", "sonnet", nil, nil, "", "")
	}()
	select {
	case got := <-done:
		t.Fatalf("send returned %q while leaked handle still registered — must wait on ended", got)
	case <-time.After(80 * time.Millisecond):
	}
	if !eng.CancelChat(ws.ID, "PA") {
		t.Fatal("CancelChat returned false")
	}
	select {
	case got := <-done:
		if got != chatStoppedReply {
			t.Fatalf("reply=%q, want %q", got, chatStoppedReply)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("SendWorkspaceMessage still blocked after CancelChat on a handle with no owning RPC")
	}
}

func TestChatTurnEndedWaitsWhenHealthy(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ctxRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "healthy wait", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	turn := eng.registerChatTurn(ws.ID, "PA", func() {})
	if turn == nil {
		t.Fatal("registerChatTurn returned nil")
	}
	ch := eng.chatTurnEnded(ws.ID, "PA")
	if ch == nil {
		t.Fatal("healthy live turn must expose ended so a non-inject follow-up waits instead of Swap-cancel")
	}
}

func TestChatTurnTryInjectBlocksUntilStop(t *testing.T) {
	turn := &chatTurn{cancel: func() {}}
	if turn.armChatInject() == nil {
		t.Fatal("armChatInject returned nil")
	}
	done := make(chan bool, 1)
	go func() { done <- turn.tryInject("steer") }()
	select {
	case ok := <-done:
		t.Fatalf("tryInject returned %v with no receiver; must block until claimed or stopped", ok)
	case <-time.After(40 * time.Millisecond):
	}
	turn.stopChatInject()
	select {
	case ok := <-done:
		if ok {
			t.Fatal("tryInject succeeded after inject was disabled")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("tryInject still blocked after stopChatInject")
	}
}

func TestChatTurnTryInjectUnblocksAfterSendWhenAckNeverArrives(t *testing.T) {
	turn := &chatTurn{cancel: func() {}}
	ch := turn.armChatInject()
	got := make(chan string, 1)
	go func() { got <- <-ch }()
	done := make(chan bool, 1)
	go func() { done <- turn.tryInject("steer") }()
	select {
	case <-got:
	case <-time.After(2 * time.Second):
		t.Fatal("inject was never received")
	}
	select {
	case ok := <-done:
		t.Fatalf("tryInject returned %v before ack or stop; must wait", ok)
	case <-time.After(40 * time.Millisecond):
	}
	turn.stopChatInject()
	select {
	case ok := <-done:
		if ok {
			t.Fatal("tryInject succeeded without an ack")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("tryInject still blocked after stopChatInject with no ack")
	}
}

func TestCancelChatStopsTicketedSendBeforeRegister(t *testing.T) {
	runner := &countRunner{}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "prologue stop", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	blocked := make(chan struct{})
	proceed := make(chan struct{})
	testBeforeChatClaim = func() {
		close(blocked)
		<-proceed
	}
	t.Cleanup(func() { testBeforeChatClaim = nil })

	done := make(chan string, 1)
	go func() {
		done <- eng.SendWorkspaceMessage(ws.ID, "PA", "start me", "sonnet", nil, nil, "", "")
	}()
	select {
	case <-blocked:
	case <-time.After(3 * time.Second):
		t.Fatal("send never entered prologue")
	}
	if !eng.CancelChat(ws.ID, "PA") {
		t.Fatal("CancelChat returned false; ticketed prologue must record Stop")
	}
	close(proceed)
	select {
	case got := <-done:
		if got != chatStoppedReply {
			t.Fatalf("reply=%q, want %q", got, chatStoppedReply)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("SendWorkspaceMessage did not return after prologue Stop")
	}
	if n := runner.n.Load(); n != 0 {
		t.Fatalf("Run called %d times, want 0", n)
	}
}

func TestCancelChatStopsTicketedSendAfterForkCheck(t *testing.T) {
	runner := &countRunner{}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fork stop", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	blocked := make(chan struct{})
	proceed := make(chan struct{})
	testBeforeApplyChatFork = func() {
		close(blocked)
		<-proceed
	}
	t.Cleanup(func() { testBeforeApplyChatFork = nil })

	done := make(chan string, 1)
	go func() {
		done <- eng.SendWorkspaceMessage(ws.ID, "PA", "start me", "sonnet", nil, nil, "", "")
	}()
	select {
	case <-blocked:
	case <-time.After(3 * time.Second):
		t.Fatal("send never reached applyChatFork")
	}
	if !eng.CancelChat(ws.ID, "PA") {
		t.Fatal("CancelChat returned false; ticketed fork window must record Stop")
	}
	close(proceed)
	select {
	case got := <-done:
		if got != chatStoppedReply {
			t.Fatalf("reply=%q, want %q", got, chatStoppedReply)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("SendWorkspaceMessage did not return after fork-window Stop")
	}
	if n := runner.n.Load(); n != 0 {
		t.Fatalf("Run called %d times, want 0", n)
	}
}

func TestCancelChatLeavesOwnedEndedOpen(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "owned ended", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	turn := eng.registerChatTurn(ws.ID, "PA", func() {})
	if turn == nil {
		t.Fatal("registerChatTurn returned nil")
	}
	if !eng.CancelChat(ws.ID, "PA") {
		t.Fatal("CancelChat returned false")
	}
	select {
	case <-turn.ended:
		t.Fatal("owned turn ended closed before unregister")
	case <-time.After(30 * time.Millisecond):
	}
	eng.unregisterChatTurn(ws.ID, "PA", turn)
	select {
	case <-turn.ended:
	case <-time.After(time.Second):
		t.Fatal("ended should close on unregister")
	}
}

func TestSendWorkspaceMessageAfterStopWaitsForUnregister(t *testing.T) {
	started := make(chan struct{})
	unwind := make(chan struct{})
	eng, st, _, _ := newTestEngine(t, &slowUnwindRunner{started: started, unwind: unwind})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "stop wait owner", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	firstDone := make(chan string, 1)
	go func() {
		firstDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "long task", "sonnet", nil, nil, "", "")
	}()
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("first turn did not start")
	}
	if !eng.CancelChat(ws.ID, "PA") {
		t.Fatal("CancelChat returned false")
	}
	secondDone := make(chan string, 1)
	go func() {
		secondDone <- eng.SendWorkspaceMessage(ws.ID, "PA", "go on", "sonnet", nil, nil, "", "")
	}()
	select {
	case got := <-secondDone:
		t.Fatalf("second send returned %q before first unregistered", got)
	case <-time.After(80 * time.Millisecond):
	}
	close(unwind)
	select {
	case <-firstDone:
	case <-time.After(3 * time.Second):
		t.Fatal("first send did not finish after unwind")
	}
	select {
	case <-secondDone:
	case <-time.After(3 * time.Second):
		t.Fatal("second send still blocked after unregister")
	}
}

type countRunner struct{ n atomic.Int32 }

func (r *countRunner) Run(context.Context, AgentSpec, func(ParsedEvent)) (Result, error) {
	r.n.Add(1)
	return Result{Subtype: "success", Text: "ran"}, nil
}

type slowUnwindRunner struct {
	started chan struct{}
	unwind  chan struct{}
	n       atomic.Int32
}

func (r *slowUnwindRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if r.n.Add(1) == 1 {
		close(r.started)
		<-ctx.Done()
		<-r.unwind
		return Result{}, ctx.Err()
	}
	return Result{Subtype: "success", SessionID: "next", Text: "ok"}, nil
}
