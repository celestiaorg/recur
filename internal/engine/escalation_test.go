package engine

import (
	"context"
	"encoding/json"
	"errors"
	"strings"
	"sync"
	"testing"
	"time"

	"recur/internal/bundles"
	"recur/internal/domain"
	"recur/internal/mcp"
)

// mcpEscReq builds a minimal escalate request for the agent-tool escalation test.
func mcpEscReq() mcp.EscalationRequest {
	return mcp.EscalationRequest{
		Question: "Should I use approach A or B here?",
		Context:  "Both are viable; the choice changes the public contract, so I won't guess.",
		Options: []mcp.EscalationOption{
			{Text: "Approach A", Tag: "simplest", Desc: "Less code, slightly less flexible."},
			{Text: "Approach B", Tag: "recommended", Desc: "More robust, a bit more work."},
		},
	}
}

// TestEscalationRoundTripAtCap is the headline M4 test (PRD §13, §19 M4: "a loop
// pauses, asks, resumes on your answer"). A loop whose scripted reviewer NEVER
// converges hits the autonomy cap → rail #4 RAISES an escalation. We assert:
//   - the loop reaches state=escalated, group=needs;
//   - the escalation is persisted + escalation.raised is emitted;
//   - the loop GOROUTINE is BLOCKED (not exited) — its loop-slot is still held;
//   - AnswerEscalation delivers the answer; the loop resumes from the checkpoint
//     and reaches a terminal state; escalation status=answered + escalation.resolved
//     emitted.
//
// Run under -race; the waitFor deadlines act as a hard hang guard.
func TestEscalationRoundTripAtCap(t *testing.T) {
	repo := initRepo(t)
	// Never converges (always 2 findings) → hits the Balanced cap (4) → rail #4.
	runner := &convergeRunner{findingsPerIter: []int{2, 2, 2, 2, 2, 2, 2, 2}}
	eng, st, sink, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "never converges, must escalate", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}

	// The loop escalates at the cap.
	waitFor(t, 15*time.Second, "loop to escalate", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateEscalated
	})

	l, _ := st.Workspace(id)
	if l.Group != domain.GroupNeeds {
		t.Errorf("escalated loop group: got %q, want needs", l.Group)
	}

	// The escalation is persisted with reasoned options + open status.
	esc, ok := st.Escalation(id)
	if !ok {
		t.Fatal("no escalation persisted for the escalated loop")
	}
	if esc.Status != "open" {
		t.Errorf("escalation status: got %q, want open", esc.Status)
	}
	if esc.Question == "" || len(esc.Options) == 0 {
		t.Errorf("escalation must carry a question + reasoned options: %+v", esc)
	}

	// escalation.raised was emitted.
	if !sink.has("escalation.raised") {
		t.Error("expected an escalation.raised event")
	}

	// The loop GOROUTINE is still alive (blocked on the escalation), NOT exited: its
	// loop-slot is still held by the governor.
	if got := eng.Governor().LoopsRunning(); got != 1 {
		t.Errorf("escalated loop must still hold its slot (goroutine blocked): LoopsRunning=%d, want 1", got)
	}
	// Give it a moment and re-check it has NOT progressed past escalated on its own
	// (a blocked loop does not busy-spin to a terminal state).
	time.Sleep(100 * time.Millisecond)
	if l2, _ := st.Workspace(id); l2.State != domain.StateEscalated {
		t.Errorf("escalated loop changed state without an answer: %q", l2.State)
	}

	// Now ANSWER: choose "Stop" so the resumed loop reaches a terminal state quickly
	// and deterministically (no further reviewer rounds needed).
	answered, ok := eng.AnswerEscalation(id, "Stop", "")
	if !ok {
		t.Fatal("AnswerEscalation returned not-ok")
	}
	if answered.Status != "answered" {
		t.Errorf("answered escalation status: got %q, want answered", answered.Status)
	}

	// The loop resumes from the checkpoint and reaches a terminal state (stopped, per
	// the "Stop" choice) — it does NOT hang.
	waitFor(t, 10*time.Second, "answered loop to reach a terminal state", func() bool {
		l, ok := st.Workspace(id)
		return ok && terminal(l.State)
	})
	final, _ := st.Workspace(id)
	if final.State != domain.StateStopped {
		t.Errorf("loop after 'Stop' answer: got %q, want stopped", final.State)
	}

	// The loop goroutine exited (slot released).
	waitFor(t, 5*time.Second, "loop slot released after answer", func() bool {
		return eng.Governor().LoopsRunning() == 0
	})

	// escalation.resolved emitted + persisted answered.
	if !sink.has("escalation.resolved") {
		t.Error("expected an escalation.resolved event")
	}
	escFinal, _ := st.Escalation(id)
	if escFinal.Status != "answered" {
		t.Errorf("persisted escalation status: got %q, want answered", escFinal.Status)
	}

	// Instrumentation hook (PRD §13): raised/answered counters moved.
	raised, ans := eng.EscalationStats()
	if raised < 1 || ans < 1 {
		t.Errorf("escalation precision counters not tracked: raised=%d answered=%d", raised, ans)
	}
}

// TestEscalationKeepIteratingThenConverges asserts the "keep iterating" answer
// extends the cap and the loop then converges to pr_open (rail #4 → apply answer).
func TestEscalationKeepIteratingThenConverges(t *testing.T) {
	repo := initRepo(t)
	// 4 non-converging iterations (hits the Balanced cap), then 0 → converges after
	// the human says "keep iterating".
	runner := &convergeRunner{findingsPerIter: []int{2, 2, 2, 2, 0}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "converges after keep-iterating", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}

	waitFor(t, 15*time.Second, "loop to escalate at cap", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateEscalated
	})

	if _, ok := eng.AnswerEscalation(id, "Keep iterating", ""); !ok {
		t.Fatal("AnswerEscalation not-ok")
	}

	waitFor(t, 15*time.Second, "loop to converge after keep-iterating", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})
}

// TestExtendLoopRoundsUnblocksCap is the Maestro-chat path for the same grant:
// extend_rounds must answer the live round-cap wait (not only credit grantedExtra).
func TestExtendLoopRoundsUnblocksCap(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{2, 2, 2, 2, 0}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "extend_rounds unblocks cap", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}

	waitFor(t, 15*time.Second, "loop to escalate at cap", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateEscalated
	})

	got := eng.ExtendLoopRounds(id, "", 0)
	if !strings.Contains(got, "Granted") {
		t.Fatalf("ExtendLoopRounds = %q, want Granted", got)
	}
	if _, ok := eng.AnswerEscalation(id, "Keep iterating", ""); ok {
		t.Fatal("AnswerEscalation after extend_rounds must be a no-op")
	}

	waitFor(t, 15*time.Second, "loop to converge after extend_rounds", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})
}

// TestExtendLoopRoundsRefusesAgentEscalation: a coder-raised question is not a
// round-cap wait, so extend_rounds must refuse and leave the loop blocked.
func TestExtendLoopRoundsRefusesAgentEscalation(t *testing.T) {
	repo := initRepo(t)
	runner := &escalatingRunner{}
	eng, st, sink, _ := newTestEngine(t, runner)
	runner.eng = eng
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "agent question is not a round cap", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 10*time.Second, "agent-tool escalation.raised", func() bool {
		return sink.has("escalation.raised")
	})
	waitFor(t, 5*time.Second, "loop to reach escalated", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateEscalated
	})

	got := eng.ExtendLoopRounds(id, "", 0)
	if !strings.Contains(got, "Refused") {
		t.Fatalf("ExtendLoopRounds = %q, want Refused", got)
	}
	l, _ := st.Workspace(id)
	if l.State != domain.StateEscalated {
		t.Errorf("state after refused extend_rounds = %q, want escalated", l.State)
	}
	if _, ok := eng.AnswerEscalation(id, "Stop", ""); !ok {
		t.Fatal("AnswerEscalation not-ok")
	}
	waitFor(t, 10*time.Second, "loop to finish after answering tool escalation", func() bool {
		l, ok := st.Workspace(id)
		return ok && terminal(l.State)
	})
}

// TestStopWhileEscalated asserts a Stop CANCELS a loop that is blocked on an
// escalation (PRD §6.2: a Stop must still cancel a blocked-on-escalation loop; no
// busy-wait, no hang). The blocked loop must wake on the ctx-cancel and finish
// stopped, releasing its slot.
func TestStopWhileEscalated(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{2, 2, 2, 2, 2, 2}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "stop while escalated", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}

	waitFor(t, 15*time.Second, "loop to escalate", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateEscalated
	})

	start := time.Now()
	eng.Stop(id)

	waitFor(t, 5*time.Second, "blocked-on-escalation loop to stop", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateStopped
	})
	if elapsed := time.Since(start); elapsed > 3*time.Second {
		t.Errorf("Stop on an escalated loop took too long: %v", elapsed)
	}
	// Slot released → the goroutine actually exited (no leak/hang).
	waitFor(t, 5*time.Second, "loop slot released after stop", func() bool {
		return eng.Governor().LoopsRunning() == 0
	})
}

// TestStopAtCapClosesQuestionForRestart: workspace Stop at the round-cap
// question must close that row so a later restart request is not consumed as
// an answer to the stale card.
func TestStopAtCapClosesQuestionForRestart(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{2, 2, 2, 2, 2, 2}}
	eng, st, sink, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "stop at cap then restart", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}

	waitFor(t, 15*time.Second, "loop to escalate", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateEscalated
	})

	eng.Stop(id)
	waitFor(t, 5*time.Second, "blocked-on-escalation loop to stop", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateStopped
	})
	waitFor(t, 5*time.Second, "loop slot released after stop", func() bool {
		return eng.Governor().LoopsRunning() == 0
	})

	esc, ok := st.Escalation(id)
	if ok && esc.Status == "open" {
		t.Fatalf("stop left the cap question open: %+v", esc)
	}
	if !sink.has("escalation.resolved") {
		t.Error("expected escalation.resolved after stop at cap")
	}
	if _, answered := eng.AnswerEscalation(id, "", "keep going"); answered {
		t.Fatal("stale cap question still answerable after stop")
	}

	status := eng.Restart(id)
	if !strings.Contains(status, "restarting") {
		t.Fatalf("Restart after stop-at-cap: got %q", status)
	}
}

// TestStopRecoveredEscalationClosesQuestionForRestart: Recover leaves an
// unanswered cap question with no goroutine. Explicit Stop must retire it
// (and any sibling chat row) so a later restart request is not consumed as an
// answer to the stale card.
func TestStopRecoveredEscalationClosesQuestionForRestart(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, sink, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	ws, err := eng.CreateWorkspace(projID, "recovered cap", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpdateWorkspace(ws.ID, func(l *domain.Workspace) {
		l.Type = domain.WorkspaceLoop
		l.RunStarted = true
		l.State = domain.StateEscalated
		l.Group = domain.GroupNeeds
		l.Phase = "asking a question"
	})
	st.AddEscalation(domain.Escalation{
		ID: "esc-recovered", WorkspaceID: ws.ID, Status: "open", Question: "round cap",
	})
	st.AddEscalation(domain.Escalation{
		ID: "esc-pa", WorkspaceID: ws.ID, AgentCode: "PA", Status: "open", Question: "A cap",
	})

	eng.Recover()
	got, _ := st.Workspace(ws.ID)
	if got.State != domain.StateEscalated {
		t.Fatalf("Recover() state = %s, want escalated (open question preserved)", got.State)
	}
	if esc, ok := st.Escalation(ws.ID); !ok || esc.Status != "open" {
		t.Fatalf("Recover() Escalation() = ok=%v %+v, want open", ok, esc)
	}

	eng.Stop(ws.ID)
	got, _ = st.Workspace(ws.ID)
	if got.State != domain.StateStopped {
		t.Fatalf("Stop() state = %s, want stopped", got.State)
	}
	if st.HasOpenEscalation(ws.ID) {
		t.Fatal("Stop left a recovered question open")
	}
	if !sink.has("escalation.resolved") {
		t.Error("expected escalation.resolved after Stop of recovered escalation")
	}
	if _, answered := eng.AnswerEscalation(ws.ID, "", "keep going"); answered {
		t.Fatal("stale recovered question still answerable after Stop")
	}
	if _, ok := st.AnswerEscalationFor(ws.ID, "PA", "keep going", ""); ok {
		t.Fatal("sibling PA question still answerable after Stop")
	}

	status := eng.Restart(ws.ID)
	if !strings.Contains(status, "restarting") {
		t.Fatalf("Restart after recovered-escalation Stop: got %q", status)
	}
}

func TestRetireWalkEscalationLeavesSiblingOpen(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "retire-sib", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.AddEscalation(domain.Escalation{ID: "esc-a", WorkspaceID: ws.ID, AgentCode: "PA", Status: "open", Question: "A cap"})
	st.AddEscalation(domain.Escalation{ID: "esc-b", WorkspaceID: ws.ID, AgentCode: "PA2", Status: "open", Question: "B cap"})

	eng.retireWalkEscalation(ws.ID, "PA")
	if !sink.has("escalation.resolved") {
		t.Error("expected escalation.resolved")
	}
	if _, ok := st.AnswerEscalationFor(ws.ID, "PA", "later", ""); ok {
		t.Fatal("PA question should already be closed")
	}
	got, ok := st.AnswerEscalationFor(ws.ID, "PA2", "keep", "")
	if !ok || got.Question != "B cap" {
		t.Fatalf("PA2 AnswerEscalationFor = ok=%v %+v, want still-open question B", ok, got)
	}
}

// TestShutdownWakesEscalatedLoop asserts engine Shutdown (app quit) wakes a loop
// blocked on an escalation via ctx cancellation, so quit doesn't hang (D24 + §6.2).
// The unanswered question must survive the restart: the loop stays escalated
// (not stopped), Recover/AutoResume leave it waiting, and Answer still works.
func TestShutdownWakesEscalatedLoop(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{2, 2, 2, 2, 2, 2}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "shutdown while escalated", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}

	waitFor(t, 15*time.Second, "loop to escalate", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateEscalated
	})

	done := make(chan struct{})
	go func() { eng.Shutdown(); close(done) }()
	select {
	case <-done:
	case <-time.After(8 * time.Second):
		t.Fatal("Shutdown hung on a loop blocked on an escalation")
	}

	got, ok := st.Workspace(id)
	if !ok {
		t.Fatal("workspace missing after Shutdown")
	}
	if got.State != domain.StateEscalated {
		t.Errorf("Shutdown while waiting for an answer: state = %s, want escalated", got.State)
	}
	esc, ok := st.Escalation(id)
	if !ok {
		t.Fatal("escalation missing after Shutdown")
	}
	if esc.Status != "open" {
		t.Errorf("Shutdown while waiting for an answer: escalation status = %q, want open", esc.Status)
	}

	eng2 := New(st)
	eng2.SetRunner(&convergeRunner{findingsPerIter: []int{0}})
	t.Cleanup(eng2.Shutdown)
	eng2.Recover()
	eng2.AutoResume()
	time.Sleep(200 * time.Millisecond)

	got, _ = st.Workspace(id)
	if got.State != domain.StateEscalated {
		t.Errorf("after restart Recover/AutoResume: state = %s, want escalated", got.State)
	}
	if n := eng2.Governor().LoopsRunning(); n != 0 {
		t.Errorf("after restart Recover/AutoResume: LoopsRunning = %d, want 0 (still waiting)", n)
	}

	// Zero-finding runner would open a PR if the answer were ignored and the
	// checkpoint re-ran. "Stop" must actually stop.
	if _, ok := eng2.AnswerEscalation(id, "Stop", ""); !ok {
		t.Fatal("AnswerEscalation after restart returned not-ok")
	}
	waitFor(t, 10*time.Second, "post-restart Stop answer to stop the loop", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateStopped
	})
	waitFor(t, 5*time.Second, "loop slot released after Stop answer", func() bool {
		return eng2.Governor().LoopsRunning() == 0
	})
}

// TestStopDuringShutdownPersistsStopped: Stop must persist StateStopped itself so
// a concurrent Shutdown that skips finishStopped cannot drop an accepted Stop.
func TestStopDuringShutdownPersistsStopped(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{2, 2, 2, 2, 2, 2}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "stop during shutdown", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 15*time.Second, "loop to escalate", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateEscalated
	})

	eng.teardownMu.Lock()
	eng.shuttingDown = true
	eng.teardownMu.Unlock()

	eng.Stop(id)
	got, _ := st.Workspace(id)
	if got.State != domain.StateStopped {
		t.Errorf("Stop during shutdown: state = %s, want stopped", got.State)
	}
}

// TestPauseDuringShutdownPreservesPause: Pause must persist StatePaused itself so
// a concurrent Shutdown that snapshots paused==false cannot drop an accepted pause.
func TestPauseDuringShutdownPreservesPause(t *testing.T) {
	repo := initRepo(t)
	gate := make(chan struct{})
	runner := &blockingRunner{gate: gate, edits: map[string]string{"p.txt": "x\n"}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "pause during shutdown", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 3*time.Second, "agent running", func() bool {
		return eng.Governor().AgentsRunning() == 1
	})

	eng.teardownMu.Lock()
	eng.shuttingDown = true
	eng.teardownMu.Unlock()

	eng.Pause(id)
	got, _ := st.Workspace(id)
	if got.State != domain.StatePaused {
		t.Errorf("Pause during shutdown: state = %s, want paused", got.State)
	}
	if got.Phase == phaseInterrupted {
		t.Errorf("Pause during shutdown: phase = %q, must not be interrupted", got.Phase)
	}
}

// TestEscalateMCPToolRaisesEscalation drives the escalate MCP tool end-to-end
// through the reporter: an agent's escalate tool call on a RUNNING loop records a
// pending escalation, the loop consumes it at its next checkpoint, blocks, and ends
// up escalated. This proves the escalate tool → engineReporter.Escalate → loop
// block path (the MCP handler path is unit-tested separately in mcp/server_test).
func TestEscalateMCPToolRaisesEscalation(t *testing.T) {
	repo := initRepo(t)
	// escalatingRunner: the Coder makes an edit and, on its turn, calls the engine
	// reporter's Escalate (simulating the escalate MCP tool firing mid-turn).
	runner := &escalatingRunner{}
	eng, st, sink, _ := newTestEngine(t, runner)
	runner.eng = eng
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "agent escalates via the tool", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}

	// Key off the escalation.raised event so the assertions don't race the emit
	// (the event is published right after the state transition).
	waitFor(t, 10*time.Second, "agent-tool escalation.raised", func() bool {
		return sink.has("escalation.raised")
	})

	waitFor(t, 5*time.Second, "loop to reach escalated", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateEscalated
	})

	esc, ok := st.Escalation(id)
	if !ok || esc.Question == "" {
		t.Fatalf("escalate tool did not raise a persisted escalation: %+v", esc)
	}

	// Answer it so the test cleans up (loop resumes and stops).
	if _, ok := eng.AnswerEscalation(id, "Stop", ""); !ok {
		t.Fatal("AnswerEscalation not-ok")
	}
	waitFor(t, 10*time.Second, "loop to finish after answering tool escalation", func() bool {
		l, ok := st.Workspace(id)
		return ok && terminal(l.State)
	})
}

// escalatingRunner is a test runner whose Coder turn fires an escalate "tool call"
// by invoking the engine reporter directly (the same typed seam the live MCP
// handler calls). The reviewer files no findings, so without the escalation the
// loop would converge; the escalation makes it pause instead. It uses spec.WorkspaceID
// (set by the engine) rather than a shared field, so there is no test-race on the id.
type escalatingRunner struct {
	eng *Engine
}

func (r *escalatingRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	switch spec.Role {
	case domain.RoleCoder:
		sr := &ScriptedRunner{Edits: map[string]string{"work.txt": "edit\n"}}
		if err := sr.applyEdits(spec.Worktree); err != nil {
			return Result{}, err
		}
		// Fire the escalate tool callback (as the MCP handler would) on the running
		// loop, BEFORE the turn ends. The loop consumes the pending escalation at its
		// next checkpoint and blocks.
		engineReporter{eng: r.eng}.Escalate(spec.WorkspaceID, "MA", mcpEscReq())
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil
	default:
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil
	}
}

// TestAnswerEscalationIdempotentNoResurrect is the M4 regression for the
// "AnswerEscalation can resurrect a terminal loop on a second answer" bug. A loop
// escalates at the cap and is answered with "Stop" so it settles TERMINAL and its
// goroutine exits (LoopsRunning back to 0). We then assert two independent things:
//
//  1. A SECOND AnswerEscalation (a late/duplicate answer) is a NO-OP — it does NOT
//     resurrect the loop: no new goroutine, LoopsRunning stays 0, the loop's state
//     stays terminal, and the governor slot is not re-acquired.
//  2. A CONCURRENT double-answer (two AnswerEscalation calls racing) results in at
//     most one dispatch/delivery (exactly one returns ok=true; the other is the
//     idempotent no-op at the store root).
//
// Run under -race; the waitFor deadlines act as a hard hang guard.
func TestAnswerEscalationIdempotentNoResurrect(t *testing.T) {
	repo := initRepo(t)
	// Never converges → hits the Balanced cap (4) → rail #4 escalates.
	runner := &convergeRunner{findingsPerIter: []int{2, 2, 2, 2, 2, 2, 2, 2}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "idempotent answer must not resurrect", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}

	waitFor(t, 15*time.Second, "loop to escalate at cap", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateEscalated
	})
	// The goroutine is blocked on the escalation — it still holds its slot.
	if got := eng.Governor().LoopsRunning(); got != 1 {
		t.Fatalf("escalated loop must still hold its slot: LoopsRunning=%d, want 1", got)
	}

	// First answer: "Stop" → the loop resumes from the checkpoint and settles stopped.
	answered, ok := eng.AnswerEscalation(id, "Stop", "")
	if !ok {
		t.Fatal("first AnswerEscalation returned not-ok")
	}
	if answered.Status != "answered" {
		t.Errorf("first answer status: got %q, want answered", answered.Status)
	}

	// The loop reaches a terminal state and the goroutine EXITS (slot released).
	waitFor(t, 10*time.Second, "answered loop to settle terminal", func() bool {
		l, ok := st.Workspace(id)
		return ok && terminal(l.State)
	})
	waitFor(t, 5*time.Second, "loop slot released after answer", func() bool {
		return eng.Governor().LoopsRunning() == 0
	})

	settled, _ := st.Workspace(id)
	if settled.State != domain.StateStopped {
		t.Fatalf("loop after 'Stop' answer: got %q, want stopped", settled.State)
	}

	// ---- the bug: a SECOND answer must NOT resurrect the now-terminal loop ----
	raisedBefore, ansBefore := eng.EscalationStats()

	second, ok2 := eng.AnswerEscalation(id, "Keep iterating", "")
	if ok2 {
		t.Errorf("second answer must be a no-op (already answered), got ok=true: %+v", second)
	}

	// Give any (erroneously) spawned goroutine a chance to register a slot, then
	// assert nothing was resurrected.
	time.Sleep(150 * time.Millisecond)

	if got := eng.Governor().LoopsRunning(); got != 0 {
		t.Errorf("second answer resurrected the loop: LoopsRunning=%d, want 0", got)
	}
	if l, _ := st.Workspace(id); l.State != domain.StateStopped {
		t.Errorf("second answer changed a terminal loop's state: got %q, want stopped", l.State)
	}
	// The governor slot was not re-acquired: a fresh StartWorkspace can still take all caps.
	if !eng.Governor().TryAcquireLoop() {
		t.Errorf("a loop-slot was leaked by the second answer (slot re-acquired but not released)")
	} else {
		eng.Governor().ReleaseLoop()
	}
	// The escalation answered-counter did not move on the no-op (no second delivery).
	raisedAfter, ansAfter := eng.EscalationStats()
	if ansAfter != ansBefore {
		t.Errorf("idempotent no-op bumped the answered counter: before=%d after=%d", ansBefore, ansAfter)
	}
	_ = raisedBefore
	_ = raisedAfter

	// ---- concurrent double-answer: at most one dispatch/delivery ----
	// A fresh loop that escalates; two AnswerEscalation calls race. The store-root
	// idempotency guard guarantees EXACTLY ONE returns ok=true (one dispatch), the
	// other is a no-op — never two deliveries against one escalation.
	id2, err := eng.StartWorkspace(projID, "concurrent double answer", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace #2: %v", err)
	}
	waitFor(t, 15*time.Second, "second loop to escalate at cap", func() bool {
		l, ok := st.Workspace(id2)
		return ok && l.State == domain.StateEscalated
	})

	var wg sync.WaitGroup
	var mu sync.Mutex
	okCount := 0
	for i := 0; i < 2; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if _, ok := eng.AnswerEscalation(id2, "Stop", ""); ok {
				mu.Lock()
				okCount++
				mu.Unlock()
			}
		}()
	}
	wg.Wait()

	if okCount != 1 {
		t.Errorf("concurrent double-answer: %d calls returned ok=true, want exactly 1", okCount)
	}

	// The loop settles terminal exactly once and its slot is released (no leak from
	// a second dispatch).
	waitFor(t, 10*time.Second, "concurrently-answered loop to settle terminal", func() bool {
		l, ok := st.Workspace(id2)
		return ok && terminal(l.State)
	})
	waitFor(t, 5*time.Second, "second loop slot released", func() bool {
		return eng.Governor().LoopsRunning() == 0
	})
}

// TestAnswerChatOwnedEscalationNoLiveLoop is crash recovery: a persisted PA
// escalation with no goroutine must re-dispatch on answer (D37), and must not
// resurrect a terminal workspace.
func TestAnswerChatOwnedEscalationNoLiveLoop(t *testing.T) {
	t.Run("redispatches", func(t *testing.T) {
		repo := initRepo(t)
		runner := &ScriptedRunner{
			Edits: map[string]string{"resume.txt": "resumed\n"},
			Lines: fixtureLines("sess-esc-chat", "resume.txt", "ok", 0.05),
		}
		eng, st, _, _ := newTestEngine(t, runner)
		projID := addProject(t, st, repo)
		rec := domain.Workspace{
			ID:           "LOOP-CHAT-ESC",
			ProjectID:    projID,
			Group:        domain.GroupNeeds,
			Goal:         "chat-owned escalation after crash",
			State:        domain.StateEscalated,
			StatusLabel:  "Needs you",
			Phase:        "asking a question",
			LoopChatCode: "PA",
		}
		if _, err := st.AddWorkspace(rec); err != nil {
			t.Fatalf("AddWorkspace: %v", err)
		}
		st.UpsertAgent(rec.ID, domain.Agent{ID: rec.ID + "-PA", WorkspaceID: rec.ID, Code: "PA", Name: "Agent"})
		st.AddEscalation(domain.Escalation{
			ID: "esc-pa", WorkspaceID: rec.ID, AgentCode: "PA", Status: "open", Question: "A or B?",
		})

		answered, ok := eng.AnswerEscalationFor(rec.ID, "PA", "Stop", "")
		if !ok || answered.Question != "A or B?" {
			t.Fatalf("AnswerEscalationFor = ok=%v %+v, want question answered", ok, answered)
		}
		waitFor(t, 8*time.Second, "redispatched chat-owned walk to settle", func() bool {
			l, found := st.Workspace(rec.ID)
			return found && terminal(l.State)
		})
		waitFor(t, 5*time.Second, "loop slot released", func() bool {
			return eng.Governor().LoopsRunning() == 0
		})
	})

	t.Run("redispatches answering chat", func(t *testing.T) {
		repo := initRepo(t)
		runner := &ScriptedRunner{
			Edits: map[string]string{"resume.txt": "resumed\n"},
			Lines: fixtureLines("sess-esc-chat-pa2", "resume.txt", "ok", 0.05),
		}
		eng, st, _, _ := newTestEngine(t, runner)
		installFixturePack(t)
		projID := addProject(t, st, repo)
		rec := domain.Workspace{
			ID:           "LOOP-CHAT-ESC-PA2",
			ProjectID:    projID,
			Group:        domain.GroupNeeds,
			Goal:         "owner goal",
			State:        domain.StateEscalated,
			StatusLabel:  "Needs you",
			Phase:        "asking a question",
			LoopChatCode: "PA",
			LoopID:       "recur.loops.pr",
			Checkpoint:   &domain.Checkpoint{PhaseID: "planning"},
		}
		if _, err := st.AddWorkspace(rec); err != nil {
			t.Fatalf("AddWorkspace: %v", err)
		}
		eng.stampChatLoop(rec.ID, "PA2", "test.loops.verify", "", "chat goal", "", true)
		st.AddEscalation(domain.Escalation{
			ID: "esc-pa2", WorkspaceID: rec.ID, AgentCode: "PA2", Status: "open", Question: "A or B?",
		})

		answered, ok := eng.AnswerEscalationFor(rec.ID, "PA2", "Stop", "")
		if !ok || answered.Question != "A or B?" {
			t.Fatalf("AnswerEscalationFor(PA2) = ok=%v %+v, want question answered", ok, answered)
		}
		got, _ := st.Workspace(rec.ID)
		if got.LoopChatCode != "PA2" {
			t.Fatalf("LoopChatCode = %q, want PA2 (answering chat, not workspace owner PA)", got.LoopChatCode)
		}
		if got.Goal != "chat goal" {
			t.Fatalf("Goal = %q, want chat goal", got.Goal)
		}
		if got.LoopID != "test.loops.verify" {
			t.Fatalf("LoopID = %q, want test.loops.verify", got.LoopID)
		}
		if got.Checkpoint != nil && got.Checkpoint.PhaseID == "planning" {
			t.Fatalf("Checkpoint = %+v, want not sibling planning (unowned / off-graph)", got.Checkpoint)
		}
		if lp := eng.runningLoopForHand(rec.ID, "PA2"); lp != nil && lp.startPhase == "planning" {
			t.Fatalf("startPhase = %q, want graph entry not sibling checkpoint", lp.startPhase)
		}
		waitFor(t, 8*time.Second, "redispatched answering-chat walk to settle", func() bool {
			l, found := st.Workspace(rec.ID)
			return found && (terminal(l.State) || l.Phase == "phase missing")
		})
		got, _ = st.Workspace(rec.ID)
		if got.Phase == "phase missing" {
			t.Fatal("answering chat adopted sibling checkpoint and idled")
		}
		waitFor(t, 5*time.Second, "loop slot released", func() bool {
			return eng.Governor().LoopsRunning() == 0
		})
	})

	t.Run("defers answering chat when slots full", func(t *testing.T) {
		repo := initRepo(t)
		runner := &ScriptedRunner{
			Edits: map[string]string{"resume.txt": "resumed\n"},
			Lines: fixtureLines("sess-esc-chat-pa2-slot", "resume.txt", "ok", 0.05),
		}
		eng, st, _, _ := newTestEngine(t, runner)
		installFixturePack(t)
		projID := addProject(t, st, repo)
		rec := domain.Workspace{
			ID:           "LOOP-CHAT-ESC-PA2-SLOT",
			ProjectID:    projID,
			Group:        domain.GroupNeeds,
			Goal:         "owner goal",
			State:        domain.StateEscalated,
			StatusLabel:  "Needs you",
			Phase:        "asking a question",
			LoopChatCode: "PA",
			LoopID:       "recur.loops.pr",
			Checkpoint:   &domain.Checkpoint{PhaseID: "planning"},
		}
		if _, err := st.AddWorkspace(rec); err != nil {
			t.Fatalf("AddWorkspace: %v", err)
		}
		eng.stampChatLoop(rec.ID, "PA2", "test.loops.verify", "", "chat goal", "", true)
		st.AddEscalation(domain.Escalation{
			ID: "esc-pa2-slot", WorkspaceID: rec.ID, AgentCode: "PA2", Status: "open", Question: "A or B?",
		})

		slots := 0
		for eng.gov.TryAcquireLoop() {
			slots++
		}
		defer func() {
			for i := 0; i < slots; i++ {
				eng.gov.ReleaseLoop()
			}
		}()

		answered, ok := eng.AnswerEscalationFor(rec.ID, "PA2", "Stop", "")
		if !ok || answered.Question != "A or B?" {
			t.Fatalf("AnswerEscalationFor(PA2) = ok=%v %+v, want question answered", ok, answered)
		}
		got, _ := st.Workspace(rec.ID)
		if got.LoopChatCode != "PA2" {
			t.Fatalf("LoopChatCode = %q, want PA2 after slot-exhausted answer", got.LoopChatCode)
		}
		if got.Goal != "chat goal" {
			t.Fatalf("Goal = %q, want chat goal", got.Goal)
		}
		if got.LoopID != "test.loops.verify" {
			t.Fatalf("LoopID = %q, want test.loops.verify", got.LoopID)
		}
		if got.Checkpoint != nil && got.Checkpoint.PhaseID == "planning" {
			t.Fatalf("Checkpoint = %+v, want not sibling planning after no-slot persist", got.Checkpoint)
		}
		if got.State != domain.StatePaused {
			t.Fatalf("State = %q, want paused (deferred resume)", got.State)
		}

		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
		slots = 0
		eng.Resume(rec.ID)
		got, _ = st.Workspace(rec.ID)
		if got.LoopChatCode != "PA2" {
			t.Fatalf("LoopChatCode after Resume = %q, want PA2", got.LoopChatCode)
		}
		if got.Goal != "chat goal" {
			t.Fatalf("Goal after Resume = %q, want chat goal", got.Goal)
		}
		lp := eng.runningLoopForHand(rec.ID, "PA2")
		if lp == nil {
			t.Fatal("Resume must dispatch PA2, not the persisted owner PA")
		}
		if lp.startPhase == "planning" {
			t.Fatalf("startPhase = %q, want graph entry not sibling checkpoint", lp.startPhase)
		}
	})

	t.Run("skips terminal", func(t *testing.T) {
		eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
		projID := addProject(t, st, initRepo(t))
		rec := domain.Workspace{
			ID: "LOOP-CHAT-ESC-TERM", ProjectID: projID, State: domain.StateStopped,
			LoopChatCode: "PA",
		}
		if _, err := st.AddWorkspace(rec); err != nil {
			t.Fatalf("AddWorkspace: %v", err)
		}
		st.AddEscalation(domain.Escalation{
			ID: "esc-term", WorkspaceID: rec.ID, AgentCode: "PA", Status: "open", Question: "stale",
		})
		if _, ok := eng.AnswerEscalationFor(rec.ID, "PA", "Stop", ""); !ok {
			t.Fatal("AnswerEscalationFor = false, want the open row answered")
		}
		time.Sleep(150 * time.Millisecond)
		if got := eng.Governor().LoopsRunning(); got != 0 {
			t.Errorf("terminal answer resurrected the walk: LoopsRunning=%d", got)
		}
		if l, _ := st.Workspace(rec.ID); l.State != domain.StateStopped {
			t.Errorf("state = %q, want stopped", l.State)
		}
	})
}

// TestRaiseEscalationPublishesNotify is the Phase 3.6 regression test: raising an
// escalation publishes a generic "notify" bus event (main.go turns this into a
// native OS notification) alongside the existing inbox/feed side effects.
func TestRaiseEscalationPublishesNotify(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	eng.raiseEscalation("LOOP-X", "", mcpEscReq(), escKindAgent)
	if !sink.has("notify") {
		t.Error("expected a notify event on escalation raise")
	}
	esc, ok := st.Escalation("LOOP-X")
	if !ok || esc.Kind != escKindAgent {
		t.Errorf("Kind = %q ok=%v, want agent", esc.Kind, ok)
	}
}

func TestRestoreEscalationAnswerPreservesShape(t *testing.T) {
	capOpts := []domain.EscalationOption{{Text: "Keep iterating"}, {Text: "Stop"}}
	tests := []struct {
		name string
		esc  domain.Escalation
		want capAnswer
	}{
		{
			name: "new free text",
			esc:  domain.Escalation{Answer: "use a mutex", FreeText: "use a mutex", Options: capOpts},
			want: capGuidance,
		},
		{
			name: "new keep iterating",
			esc:  domain.Escalation{Answer: "Keep iterating", Choice: "Keep iterating", Options: capOpts},
			want: capKeepIterating,
		},
		{
			name: "old free text (Answer only)",
			esc:  domain.Escalation{Answer: "use a mutex", Options: capOpts},
			want: capGuidance,
		},
		{
			name: "old stop (Answer only)",
			esc:  domain.Escalation{Answer: "Stop", Options: capOpts},
			want: capStop,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			ans := restoreEscalationAnswer(tt.esc)
			if ans == nil {
				t.Fatalf("restoreEscalationAnswer(%s) = nil, want an answer", tt.name)
			}
			if got := classifyCapAnswer(*ans); got != tt.want {
				t.Errorf("classifyCapAnswer(restoreEscalationAnswer(%s)) = %v, want %v", tt.name, got, tt.want)
			}
		})
	}
}

// TestRestartFreeTextCapGuidance: a free-text cap answer after restart must keep
// iterating (capGuidance), not fall through to capStop.
func TestRestartFreeTextCapGuidance(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{2, 2, 2, 2, 2, 2}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "free-text cap after restart", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 15*time.Second, "loop to escalate", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateEscalated
	})

	done := make(chan struct{})
	go func() { eng.Shutdown(); close(done) }()
	select {
	case <-done:
	case <-time.After(8 * time.Second):
		t.Fatal("Shutdown hung on a loop blocked on an escalation")
	}

	eng2 := New(st)
	eng2.SetRunner(&convergeRunner{findingsPerIter: []int{0}})
	t.Cleanup(eng2.Shutdown)
	if _, ok := eng2.AnswerEscalation(id, "", "try a mutex instead"); !ok {
		t.Fatal("AnswerEscalation(free text) after restart returned not-ok")
	}
	waitFor(t, 10*time.Second, "free-text cap guidance to continue (not stop)", func() bool {
		l, ok := st.Workspace(id)
		return ok && (l.State == domain.StatePROpen || l.State == domain.StateStopped)
	})
	got, _ := st.Workspace(id)
	if got.State != domain.StatePROpen {
		t.Errorf("classifyCapAnswer after restart: state = %s, want pr_open (free text is guidance, not Stop)", got.State)
	}
}

// TestRestartPlanApprovalNotes: free-text plan notes must fold into the plan
// after a restart, not be discarded by restoring Choice=resolved.
func TestRestartPlanApprovalNotes(t *testing.T) {
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "plan-notes-restart", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	rep := engineReporter{eng: eng}
	_ = rep.SubmitPlan(ws.ID, "PA", mcp.PlanSubmission{
		Summary: "Wire the toggle",
		Steps:   []string{"Edit settings"},
	})
	if queued, err := eng.StartRunOnWorkspace(ws.ID, "ship the toggle"); err != nil || queued {
		t.Fatalf("StartRunOnWorkspace: queued=%v err=%v", queued, err)
	}
	waitFor(t, 5*time.Second, "plan-approval escalation", func() bool {
		got, _ := st.Workspace(ws.ID)
		return got.State == domain.StateEscalated
	})

	done := make(chan struct{})
	go func() { eng.Shutdown(); close(done) }()
	select {
	case <-done:
	case <-time.After(8 * time.Second):
		t.Fatal("Shutdown hung on plan-approval escalation")
	}

	notes := "also persist in localStorage"
	eng2 := New(st)
	eng2.SetRunner(&convergeRunner{findingsPerIter: []int{0}})
	t.Cleanup(eng2.Shutdown)
	if _, ok := eng2.AnswerEscalation(ws.ID, "Proceed with my notes", notes); !ok {
		t.Fatal("AnswerEscalation(plan notes) after restart returned not-ok")
	}
	waitFor(t, 10*time.Second, "plan-approval run to settle", func() bool {
		got, _ := st.Workspace(ws.ID)
		return got.State == domain.StatePROpen || got.State == domain.StateStopped
	})
	got, _ := st.Workspace(ws.ID)
	if got.State != domain.StatePROpen {
		t.Errorf("plan-approval after restart: state = %s, want pr_open", got.State)
	}
	if !strings.Contains(got.DefinitionOfDone, notes) {
		t.Errorf("plan notes missing after restart: DefinitionOfDone = %q, want substring %q", got.DefinitionOfDone, notes)
	}
}

// TestAnsweredEscalationSurvivesDelayedDispatch: answering with no governor slot
// must keep the answer unapplied so a later Resume/drain still applies it.
func TestAnsweredEscalationSurvivesDelayedDispatch(t *testing.T) {
	repo := initRepo(t)
	gate := make(chan struct{})
	runner := &blockingRunner{gate: gate, edits: map[string]string{"hold.txt": "x\n"}}
	eng, st, _, _ := newTestEngine(t, runner)
	eng.gov = NewGovernor(1, 8)
	projID := addProject(t, st, repo)

	idA, err := eng.StartWorkspace(projID, "holder", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace holder: %v", err)
	}
	waitFor(t, 3*time.Second, "holder running", func() bool {
		return eng.Governor().LoopsRunning() == 1
	})

	rec := domain.Workspace{
		ID: "LOOP-DELAY-ANS", ProjectID: projID, Goal: "answered while capped",
		State: domain.StateEscalated, Phase: "asking a question",
		Autonomy: domain.AutonomyBalanced, BaseBranch: "main",
		Checkpoint: &domain.Checkpoint{State: domain.StateCoding, Iteration: 4},
	}
	if _, err := st.AddWorkspace(rec); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	st.AddEscalation(domain.Escalation{
		WorkspaceID: rec.ID,
		Question:    "I've run 4 iteration(s) without converging",
		Options: []domain.EscalationOption{
			{Text: "Keep iterating"}, {Text: "Stop"},
		},
		Status: "open",
	})

	if _, ok := eng.AnswerEscalation(rec.ID, "Stop", ""); !ok {
		t.Fatal("AnswerEscalation returned not-ok")
	}
	got, _ := st.Workspace(rec.ID)
	if got.State != domain.StatePaused {
		t.Errorf("AnswerEscalation with no slot: state = %s, want paused", got.State)
	}
	esc, _ := st.Escalation(rec.ID)
	if esc.Applied {
		t.Error("AnswerEscalation with no slot must leave Applied=false")
	}

	close(gate)
	waitFor(t, 5*time.Second, "holder to finish", func() bool {
		l, _ := st.Workspace(idA)
		return l.State == domain.StatePROpen
	})
	waitFor(t, 3*time.Second, "holder slot released", func() bool {
		return eng.Governor().LoopsRunning() == 0
	})

	eng.Resume(rec.ID)
	waitFor(t, 10*time.Second, "delayed Stop answer to apply", func() bool {
		l, ok := st.Workspace(rec.ID)
		return ok && l.State == domain.StateStopped
	})
}

// TestRestartAgentEscalationDoesNotSwallowNext: after restart, an answered
// agent-raised escalation must not short-circuit the next escalate() call.
func TestRestartAgentEscalationDoesNotSwallowNext(t *testing.T) {
	repo := initRepo(t)
	runner := &escalatingRunner{}
	eng, st, sink, _ := newTestEngine(t, runner)
	runner.eng = eng
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "agent esc then another", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 10*time.Second, "agent-tool escalation.raised", func() bool {
		return sink.has("escalation.raised")
	})
	waitFor(t, 5*time.Second, "loop to reach escalated", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateEscalated
	})

	done := make(chan struct{})
	go func() { eng.Shutdown(); close(done) }()
	select {
	case <-done:
	case <-time.After(8 * time.Second):
		t.Fatal("Shutdown hung on a loop blocked on an agent escalation")
	}

	eng2 := New(st)
	runner2 := &escalatingRunner{eng: eng2}
	eng2.SetRunner(runner2)
	t.Cleanup(eng2.Shutdown)
	if _, ok := eng2.AnswerEscalation(id, "Approach A", ""); !ok {
		t.Fatal("AnswerEscalation after restart returned not-ok")
	}
	waitFor(t, 10*time.Second, "second agent escalate after restart", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateEscalated
	})
	got, _ := st.Workspace(id)
	if got.State != domain.StateEscalated {
		t.Errorf("restarted agent escalation swallowed the next escalate(): state = %s, want escalated", got.State)
	}
	esc, ok := st.Escalation(id)
	if !ok || esc.Status != "open" {
		t.Errorf("second escalation status = %q ok=%v, want open", esc.Status, ok)
	}
}

// TestPauseThenImmediateShutdownPreservesPause: Pause is accepted immediately
// but StatePaused is only written at checkpoint. Quit before that checkpoint
// must still persist the pause so AutoResume does not restart the loop.
func TestPauseThenImmediateShutdownPreservesPause(t *testing.T) {
	repo := initRepo(t)
	gate := make(chan struct{})
	runner := &blockingRunner{gate: gate, edits: map[string]string{"p.txt": "x\n"}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "pause then quit", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 3*time.Second, "agent running", func() bool {
		return eng.Governor().AgentsRunning() == 1
	})
	eng.Pause(id)

	done := make(chan struct{})
	go func() { eng.Shutdown(); close(done) }()
	select {
	case <-done:
	case <-time.After(8 * time.Second):
		t.Fatal("Shutdown hung after Pause")
	}

	got, ok := st.Workspace(id)
	if !ok {
		t.Fatal("workspace missing after Pause+Shutdown")
	}
	if got.State != domain.StatePaused {
		t.Errorf("Pause then Shutdown: state = %s, want paused", got.State)
	}
	if got.Phase == phaseInterrupted {
		t.Errorf("Pause then Shutdown: phase = %q, must not be interrupted", got.Phase)
	}

	eng2 := New(st)
	eng2.SetRunner(&convergeRunner{findingsPerIter: []int{0}})
	t.Cleanup(eng2.Shutdown)
	eng2.Recover()
	eng2.AutoResume()
	time.Sleep(200 * time.Millisecond)
	got, _ = st.Workspace(id)
	if got.State != domain.StatePaused {
		t.Errorf("AutoResume restarted a pause-then-quit loop: state = %s, want paused", got.State)
	}
}

func TestEnsurePRDoorEscalation(t *testing.T) {
	if !ensurePRDoorEscalation(domain.Escalation{Question: "Policy bar blocked the PR: 1 countable finding. How should I proceed?"}) {
		t.Error("question prefix should be the ensure_pr door")
	}
	if !ensurePRDoorEscalation(domain.Escalation{Options: []domain.EscalationOption{
		{Text: "Ship with caveats", Desc: "Open the PR anyway, noting remaining findings."},
	}}) {
		t.Error("anyway-ship option should be the ensure_pr door")
	}
	if ensurePRDoorEscalation(domain.Escalation{
		Question: "I've run 4 iteration(s) without converging — 1 open finding remain. How should I proceed?",
		Options:  []domain.EscalationOption{{Text: "Ship with caveats", Desc: "Open the PR noting remaining risk (rails currently allow it)."}},
	}) {
		t.Error("round-cap question must not look like the ensure_pr door")
	}
}

func TestApplyAnsweredCapEnsurePRAfterRestart(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	guard := resolvedGuard{
		Loop: bundles.LoopDef{
			Phases: []bundles.LoopPhase{
				{ID: "converge", Uses: bundles.VerbConverge, Next: "pr"},
				{ID: "pr", Uses: bundles.VerbEnsurePR},
			},
		},
		Policy: bundles.Policy{ExtendRounds: 2},
	}
	doorOpts := []domain.EscalationOption{
		{Text: "Keep iterating"},
		{Text: "Change approach"},
		{Text: "Ship with caveats", Desc: "Open the PR anyway, noting remaining findings."},
		{Text: "Stop"},
	}
	seed := func(id, choice, free, question string, opts []domain.EscalationOption) *Loop {
		t.Helper()
		if _, err := st.AddWorkspace(domain.Workspace{
			ID: id, Goal: "ensure-pr restart", State: domain.StatePaused,
			Autonomy:   domain.AutonomyBalanced,
			Checkpoint: &domain.Checkpoint{PhaseID: "pr", Iteration: 4, State: domain.StateEscalated},
		}); err != nil {
			t.Fatalf("AddWorkspace: %v", err)
		}
		st.AddFinding(domain.Finding{
			ID: "f-" + id, WorkspaceID: id, Severity: domain.SevHigh, Status: "open",
			Message: "still open", File: "a.go",
		})
		kind := escKindCap
		if strings.HasPrefix(question, "Policy bar blocked the PR:") {
			kind = escKindEnsurePR
		}
		st.AddEscalation(domain.Escalation{
			WorkspaceID: id, Question: question, Options: opts,
			Status: "answered", Choice: choice, FreeText: free, Answer: strings.TrimSpace(free + " " + choice),
			Kind: kind,
		})
		rec, _ := st.Workspace(id)
		l := newLoop(eng, rec)
		l.guard = guard
		return l
	}
	doorQ := "Policy bar blocked the PR: 1 countable finding — policy bar blocks opening a PR. How should I proceed?"
	capQ := "I've run 4 iteration(s) without converging — 1 open finding remain. How should I proceed?"
	capOpts := []domain.EscalationOption{
		{Text: "Keep iterating"},
		{Text: "Ship with caveats", Desc: "Open the PR noting remaining risk (rails currently allow it)."},
		{Text: "Stop"},
	}

	t.Run("ship opens PR despite open findings", func(t *testing.T) {
		l := seed("WS-ens-ship", "Ship with caveats", "", doorQ, doorOpts)
		if !l.applyAnsweredCap(context.Background()) {
			t.Fatal("ship should finish the loop")
		}
		ws, _ := st.Workspace(l.ID)
		if ws.State != domain.StatePROpen {
			t.Errorf("state = %s, want pr_open (ensure_pr shipAnyway)", ws.State)
		}
	})
	t.Run("keep iterating returns inbound", func(t *testing.T) {
		l := seed("WS-ens-ki", "Keep iterating", "", doorQ, doorOpts)
		if l.applyAnsweredCap(context.Background()) {
			t.Fatal("keep iterating should not finish the loop")
		}
		ws, _ := st.Workspace(l.ID)
		if ws.Checkpoint == nil || ws.Checkpoint.PhaseID != "converge" {
			t.Errorf("PhaseID = %v, want converge", ws.Checkpoint)
		}
		if l.grantedExtra != 0 {
			t.Errorf("grantedExtra = %d, want 0 (inbound, not cap grant)", l.grantedExtra)
		}
	})
	t.Run("guidance returns inbound", func(t *testing.T) {
		l := seed("WS-ens-guide", "", "fix the timeout first", doorQ, doorOpts)
		if l.applyAnsweredCap(context.Background()) {
			t.Fatal("guidance should not finish the loop")
		}
		ws, _ := st.Workspace(l.ID)
		if ws.Checkpoint == nil || ws.Checkpoint.PhaseID != "converge" {
			t.Errorf("PhaseID = %v, want converge", ws.Checkpoint)
		}
	})
	t.Run("round-cap ship still respects the bar", func(t *testing.T) {
		l := seed("WS-cap-ship", "Ship with caveats", "", capQ, capOpts)
		if !l.applyAnsweredCap(context.Background()) {
			t.Fatal("blocked round-cap ship should finish (stop)")
		}
		ws, _ := st.Workspace(l.ID)
		if ws.State != domain.StateStopped {
			t.Errorf("state = %s, want stopped (round-cap ship blocked by findings)", ws.State)
		}
	})
	t.Run("ship follows ensure_pr next", func(t *testing.T) {
		l := seed("WS-ens-ship-next", "Ship with caveats", "", doorQ, doorOpts)
		l.guard.Loop.Phases = []bundles.LoopPhase{
			{ID: "converge", Uses: bundles.VerbConverge, Next: "pr"},
			{ID: "pr", Uses: bundles.VerbEnsurePR, Next: "docs"},
			{ID: "docs", Uses: bundles.VerbArtifact},
		}
		if l.applyAnsweredCap(context.Background()) {
			t.Fatal("ship with next should not finish the loop")
		}
		ws, _ := st.Workspace(l.ID)
		if ws.State != domain.StatePROpen {
			t.Errorf("state = %s, want pr_open", ws.State)
		}
		if ws.Checkpoint == nil || ws.Checkpoint.PhaseID != "docs" {
			t.Errorf("PhaseID = %v, want docs", ws.Checkpoint)
		}
	})
}

func TestApplyAnsweredCapBundleCapShipFollowsNext(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	capQ := "I've run 4 iteration(s) without converging — 1 open finding remain. How should I proceed?"
	capOpts := []domain.EscalationOption{
		{Text: "Keep iterating"},
		{Text: "Ship with caveats", Desc: "Open the PR noting remaining risk (rails currently allow it)."},
		{Text: "Stop"},
	}
	seed := func(id string, phases []bundles.LoopPhase) *Loop {
		t.Helper()
		if _, err := st.AddWorkspace(domain.Workspace{
			ID: id, Goal: "docs cap ship", State: domain.StatePaused,
			Autonomy:   domain.AutonomyBalanced,
			Checkpoint: &domain.Checkpoint{PhaseID: "converge", Iteration: 4, State: domain.StateEscalated},
		}); err != nil {
			t.Fatalf("AddWorkspace: %v", err)
		}
		st.AddFinding(domain.Finding{
			ID: "f-" + id, WorkspaceID: id, Severity: domain.SevLow, Status: "open",
			Message: "typo", File: "README.md",
		})
		st.AddEscalation(domain.Escalation{
			WorkspaceID: id, Question: capQ, Options: capOpts,
			Status: "answered", Choice: "Ship with caveats", Answer: "Ship with caveats",
			Kind: escKindCap,
		})
		rec, _ := st.Workspace(id)
		l := newLoop(eng, rec)
		l.guard = resolvedGuard{Loop: bundles.LoopDef{Phases: phases}}
		return l
	}

	t.Run("follows non-PR next", func(t *testing.T) {
		l := seed("WS-docs-cap-next", []bundles.LoopPhase{
			{ID: "converge", Uses: bundles.VerbConverge, Next: "done"},
			{ID: "done", Uses: bundles.VerbArtifact},
		})
		if l.applyAnsweredCap(context.Background()) {
			t.Fatal("bundle cap ship with next should not finish the loop")
		}
		ws, _ := st.Workspace(l.ID)
		if ws.State == domain.StatePROpen {
			t.Fatal("opened a PR; want converge next (done)")
		}
		if ws.Checkpoint == nil || ws.Checkpoint.PhaseID != "done" {
			t.Errorf("PhaseID = %v, want done", ws.Checkpoint)
		}
	})
	t.Run("no next idles", func(t *testing.T) {
		l := seed("WS-docs-cap-idle", []bundles.LoopPhase{
			{ID: "converge", Uses: bundles.VerbConverge},
		})
		if !l.applyAnsweredCap(context.Background()) {
			t.Fatal("bundle cap ship with no next should finish (idle)")
		}
		ws, _ := st.Workspace(l.ID)
		if ws.State == domain.StatePROpen {
			t.Fatal("opened a PR; want idle")
		}
		if ws.State != domain.StatePaused {
			t.Errorf("state = %s, want paused (idle)", ws.State)
		}
	})
}

func TestApplyAnsweredCapUsesPersistedKindNotOptionText(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	seed := func(id, kind, choice string, opts []domain.EscalationOption) *Loop {
		t.Helper()
		if _, err := st.AddWorkspace(domain.Workspace{
			ID: id, Goal: "kind collision", State: domain.StatePaused,
			Autonomy: domain.AutonomyBalanced,
		}); err != nil {
			t.Fatalf("AddWorkspace: %v", err)
		}
		st.AddEscalation(domain.Escalation{
			WorkspaceID: id, Question: "Which path?", Options: opts,
			Status: "answered", Choice: choice, Answer: choice, Kind: kind,
		})
		rec, _ := st.Workspace(id)
		return newLoop(eng, rec)
	}

	t.Run("agent Keep iterating collision steers", func(t *testing.T) {
		l := seed("WS-kind-cap", escKindAgent, "Refactor first", []domain.EscalationOption{
			{Text: "Keep iterating"}, {Text: "Refactor first"},
		})
		if l.applyAnsweredCap(context.Background()) {
			t.Fatal("agent answer must not finish the loop")
		}
		ws, _ := st.Workspace(l.ID)
		if ws.State == domain.StateStopped {
			t.Fatal("colliding cap option text stopped an agent escalation")
		}
		if l.pendingSteer != "Refactor first" {
			t.Errorf("pendingSteer = %q, want Refactor first", l.pendingSteer)
		}
	})
	t.Run("agent Approve and proceed collision steers", func(t *testing.T) {
		l := seed("WS-kind-plan", escKindAgent, "Keep iterating", []domain.EscalationOption{
			{Text: "Approve and proceed"}, {Text: "Keep iterating"},
		})
		if l.applyAnsweredCap(context.Background()) {
			t.Fatal("agent answer must not finish the loop")
		}
		if l.pendingSteer != "Keep iterating" {
			t.Errorf("pendingSteer = %q, want Keep iterating", l.pendingSteer)
		}
		if l.grantedExtra != 0 {
			t.Errorf("grantedExtra = %d, want 0 (not a cap grant)", l.grantedExtra)
		}
	})
}

func TestNewLoopDoesNotReplayLegacyAnsweredEscalation(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: "WS-legacy-applied", Goal: "already consumed", State: domain.StatePaused,
		Phase: phaseInterrupted, Autonomy: domain.AutonomyBalanced,
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	var esc domain.Escalation
	raw := `{"workspaceId":"WS-legacy-applied","status":"answered","answer":"Keep iterating","options":[{"text":"Keep iterating"},{"text":"Stop"}]}`
	if err := json.Unmarshal([]byte(raw), &esc); err != nil {
		t.Fatalf("Unmarshal: %v", err)
	}
	if !esc.Applied {
		t.Fatal("legacy answered JSON must decode as Applied")
	}
	st.AddEscalation(esc)
	rec, _ := st.Workspace("WS-legacy-applied")
	l := newLoop(eng, rec)
	if l.answer != nil {
		t.Fatalf("newLoop seeded a legacy consumed answer: %+v", l.answer)
	}
}

func TestNewLoopSeedsLegacyStuckAnsweredEscalation(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	raw := `{"workspaceId":"WS-legacy-stuck","status":"answered","answer":"Approach A"}`
	var esc domain.Escalation
	if err := json.Unmarshal([]byte(raw), &esc); err != nil {
		t.Fatalf("Unmarshal: %v", err)
	}
	if !esc.Applied {
		t.Fatal("legacy answered JSON must decode as Applied")
	}

	cases := []domain.Workspace{
		{ID: "WS-legacy-stuck-esc", Goal: "pending", State: domain.StateEscalated, Phase: "asking a question", Autonomy: domain.AutonomyBalanced},
		{ID: "WS-legacy-stuck-wait", Goal: "pending", State: domain.StatePaused, Phase: "answered — waiting to resume", Autonomy: domain.AutonomyBalanced},
	}
	for _, ws := range cases {
		if _, err := st.AddWorkspace(ws); err != nil {
			t.Fatalf("AddWorkspace(%s): %v", ws.ID, err)
		}
		esc.WorkspaceID = ws.ID
		st.AddEscalation(esc)
		rec, _ := st.Workspace(ws.ID)
		l := newLoop(eng, rec)
		if l.answer == nil || l.answer.Resolved != "Approach A" {
			t.Fatalf("newLoop(%s) must seed legacy stuck answer, got %+v", ws.ID, l.answer)
		}
	}
}

func TestApplyAnsweredCapAgentSteerStaysPendingUntilPrompt(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	id := "WS-steer-pending"
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: id, Goal: "steer", State: domain.StatePaused, Autonomy: domain.AutonomyBalanced,
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	st.AddEscalation(domain.Escalation{
		WorkspaceID: id, Question: "Which path?", Status: "answered",
		Choice: "Refactor first", Answer: "Refactor first", Kind: escKindAgent,
	})
	rec, _ := st.Workspace(id)
	l := newLoop(eng, rec)
	if l.applyAnsweredCap(context.Background()) {
		t.Fatal("agent answer must not finish the loop")
	}
	esc, _ := st.Escalation(id)
	if esc.Applied {
		t.Fatal("Applied must stay false until the steer is in a prompt")
	}
	got := l.applyPendingSteer("do the work")
	if !strings.Contains(got, "Refactor first") {
		t.Errorf("applyPendingSteer(%q) missing steer, got %q", "do the work", got)
	}
	esc, _ = st.Escalation(id)
	if !esc.Applied {
		t.Fatal("applyPendingSteer must mark the answer applied")
	}
}

// TestCapAnswerAppliedAfterDurableOutcome: Applied must not flip true until
// Stop is persisted, or keep-iterating/ship has recorded its continuation.
// A blocked bus sink used to sit between MarkEscalationApplied and setState,
// so a quit could drop the human's Stop; keep-iterating used to mark Applied
// before the walker recorded the inbound phase.
func TestCapAnswerAppliedAfterDurableOutcome(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	opts := []domain.EscalationOption{
		{Text: "Keep iterating"}, {Text: "Ship with caveats"}, {Text: "Stop"},
	}
	seed := func(id, choice string) *Loop {
		t.Helper()
		if _, err := st.AddWorkspace(domain.Workspace{
			ID: id, Goal: "cap apply order", State: domain.StateCoding,
			Autonomy: domain.AutonomyBalanced,
		}); err != nil {
			t.Fatalf("AddWorkspace: %v", err)
		}
		st.AddEscalation(domain.Escalation{
			WorkspaceID: id, Question: "How should I proceed?", Options: opts,
			Status: "answered", Choice: choice, Answer: choice, Kind: escKindCap,
		})
		rec, _ := st.Workspace(id)
		return newLoop(eng, rec)
	}

	t.Run("stop persists before the feed", func(t *testing.T) {
		l := seed("WS-cap-stop-order", "Stop")
		var stateAtFeed domain.WorkspaceState
		eng.Bus.Subscribe(func(ev Event) {
			if ev.Name != "workspace.event" {
				return
			}
			data, _ := ev.Data.(map[string]any)
			item, _ := data["event"].(domain.FeedItem)
			if !strings.Contains(item.Text, "Stopping per your decision") {
				return
			}
			ws, _ := st.Workspace(l.ID)
			stateAtFeed = ws.State
		})
		stOut, more := l.escalateAtCapApplyMsg(context.Background(), 4, 1, "", false)
		if stOut != domain.StateStopped || more != 0 {
			t.Fatalf("got %s/%d, want stopped/0", stOut, more)
		}
		if stateAtFeed != domain.StateStopped {
			t.Errorf("feed published while state=%s, want stopped already", stateAtFeed)
		}
		esc, _ := st.Escalation(l.ID)
		if !esc.Applied {
			t.Fatal("Stop must mark Applied after StateStopped")
		}
	})

	t.Run("keep iterating stays pending after helper", func(t *testing.T) {
		l := seed("WS-cap-ki-order", "Keep iterating")
		appliedAtFeed := true
		eng.Bus.Subscribe(func(ev Event) {
			if ev.Name != "workspace.event" {
				return
			}
			data, _ := ev.Data.(map[string]any)
			item, _ := data["event"].(domain.FeedItem)
			if !strings.Contains(item.Text, "keep iterating") {
				return
			}
			esc, _ := st.Escalation(l.ID)
			appliedAtFeed = esc.Applied
		})
		_, more := l.escalateAtCapApplyMsg(context.Background(), 4, 1, "", false)
		if more <= 0 {
			t.Fatal("keep iterating must return a grant")
		}
		if appliedAtFeed {
			t.Error("Applied during keep-iterating feed; grant not yet returned")
		}
		esc, _ := st.Escalation(l.ID)
		if esc.Applied {
			t.Fatal("keep iterating must stay pending until the caller records continuation")
		}
	})

	t.Run("ship stays pending after helper", func(t *testing.T) {
		l := seed("WS-cap-ship-order", "Ship with caveats")
		stOut, more := l.escalateAtCapApplyMsg(context.Background(), 4, 1, "", true)
		if stOut != domain.StatePROpen || more != 0 {
			t.Fatalf("got %s/%d, want pr_open/0", stOut, more)
		}
		esc, _ := st.Escalation(l.ID)
		if esc.Applied {
			t.Fatal("ship must stay pending until the caller follows the next edge")
		}
	})
}

func TestEnsurePRKeepIteratingSurvivesShutdownDuringFeed(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	guard := resolvedGuard{
		Loop: bundles.LoopDef{
			Phases: []bundles.LoopPhase{
				{ID: "converge", Uses: bundles.VerbConverge, Next: "pr"},
				{ID: "pr", Uses: bundles.VerbEnsurePR},
			},
		},
		Policy: bundles.Policy{
			ExtendRounds:      2,
			FindingsThreshold: bundles.FindingsThreshold{Levels: map[string]int{"high": 0, "critical": 0, "medium": 0}},
		},
	}
	id := "WS-ens-ki-quit"
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: id, Goal: "ensure-pr quit", State: domain.StateCoding,
		Autonomy:   domain.AutonomyBalanced,
		Checkpoint: &domain.Checkpoint{PhaseID: "pr", Iteration: 4, State: domain.StateCoding},
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	st.AddFinding(domain.Finding{
		ID: "f-" + id, WorkspaceID: id, Severity: domain.SevHigh, Status: "open",
		Message: "still open", File: "a.go",
	})
	st.AddEscalation(domain.Escalation{
		WorkspaceID: id,
		Question:    "Policy bar blocked the PR: 1 countable finding. How should I proceed?",
		Options: []domain.EscalationOption{
			{Text: "Keep iterating"}, {Text: "Change approach"},
			{Text: "Ship with caveats", Desc: "Open the PR anyway, noting remaining findings."},
			{Text: "Stop"},
		},
		Status: "answered", Choice: "Keep iterating", Answer: "Keep iterating",
		Kind: escKindEnsurePR,
	})
	rec, _ := st.Workspace(id)
	l := newLoop(eng, rec)
	l.guard = guard

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	eng.Bus.Subscribe(func(ev Event) {
		if ev.Name != "workspace.event" {
			return
		}
		data, _ := ev.Data.(map[string]any)
		item, _ := data["event"].(domain.FeedItem)
		if !strings.Contains(strings.ToLower(item.Text), "keep iterating") {
			return
		}
		esc, _ := st.Escalation(l.ID)
		if esc.Applied {
			t.Error("Applied during keep-iterating feed; inbound phase not yet recorded")
		}
		cancel()
	})
	eng.teardownMu.Lock()
	eng.shuttingDown = true
	eng.teardownMu.Unlock()

	_, done := l.runLoopPhase(ctx, bundles.LoopPhase{ID: "pr", Uses: bundles.VerbEnsurePR})
	if !done {
		t.Fatal("runLoopPhase(cancelled) done = false, want true")
	}
	ws, _ := st.Workspace(l.ID)
	if ws.Checkpoint == nil || ws.Checkpoint.PhaseID != "converge" {
		t.Fatalf("after cancel PhaseID = %v, want converge already recorded", ws.Checkpoint)
	}
	if ws.State == domain.StateStopped {
		t.Fatal("shutdown cancel must not persist Stopped")
	}

	eng2 := New(st)
	eng2.SetRunner(&ScriptedRunner{})
	t.Cleanup(eng2.Shutdown)
	eng2.Recover()
	rec2, _ := st.Workspace(id)
	l2 := newLoop(eng2, rec2)
	l2.guard = guard
	if l2.answer != nil {
		t.Fatal("Applied continuation must not re-seed Keep iterating")
	}
	if l2.applyAnsweredCap(context.Background()) {
		t.Fatal("applyAnsweredCap with no seeded answer must not finish the loop")
	}
	ws2, _ := st.Workspace(id)
	if ws2.Checkpoint == nil || ws2.Checkpoint.PhaseID != "converge" {
		t.Errorf("after restart PhaseID = %v, want converge (not ensure_pr)", ws2.Checkpoint)
	}
}

func TestRoundCapKeepIteratingSurvivesCancelAndSecondRestart(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	guard := resolvedGuard{
		Loop: bundles.LoopDef{
			Phases: []bundles.LoopPhase{
				{ID: "converge", Uses: bundles.VerbConverge, Next: "done"},
				{ID: "done", Uses: bundles.VerbArtifact},
			},
		},
		Policy: bundles.Policy{ExtendRounds: 2, MaxRounds: 4},
	}
	id := "WS-cap-ki-quit"
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: id, Goal: "round-cap quit", State: domain.StateCoding,
		Autonomy:   domain.AutonomyBalanced,
		Checkpoint: &domain.Checkpoint{PhaseID: "converge", Iteration: 4, State: domain.StateCoding},
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	st.AddFinding(domain.Finding{
		ID: "f-" + id, WorkspaceID: id, Severity: domain.SevLow, Status: "open",
		Message: "still open", File: "a.go",
	})
	st.AddEscalation(domain.Escalation{
		WorkspaceID: id,
		Question:    "I've run 4 iteration(s) without converging — 1 open finding remain. How should I proceed?",
		Options: []domain.EscalationOption{
			{Text: "Keep iterating"}, {Text: "Ship with caveats"}, {Text: "Stop"},
		},
		Status: "answered", Choice: "Keep iterating", Answer: "Keep iterating",
		Kind: escKindCap,
	})
	rec, _ := st.Workspace(id)
	l := newLoop(eng, rec)
	l.guard = guard

	if l.applyAnsweredCap(context.Background()) {
		t.Fatal("keep iterating must not finish the loop")
	}
	esc, _ := st.Escalation(id)
	if esc.Applied {
		t.Fatal("round-cap grant must stay pending until a turn consumes it")
	}
	if l.grantedExtra <= 0 {
		t.Fatal("applyAnsweredCap must credit grantedExtra")
	}

	eng.ApplyGovernorCaps(domain.DefaultMaxLoops, 1)
	held, ok := eng.acquireAgent(make(chan struct{}))
	if !ok {
		t.Fatal("acquireAgent() ok = false, want occupied slot")
	}
	defer held()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	finished := make(chan struct{})
	var gotErr error
	go func() {
		defer close(finished)
		_, gotErr = l.converge(ctx)
	}()
	waitFor(t, 5*time.Second, "converge blocked on occupied agent slot", func() bool {
		return eng.Governor().AgentsWaiting() > 0
	})
	esc, _ = st.Escalation(id)
	if esc.Applied {
		t.Fatal("grant must stay pending while waiting for an agent slot")
	}
	cancel()
	select {
	case <-finished:
	case <-time.After(5 * time.Second):
		t.Fatal("converge did not return after cancel during agent-slot wait")
	}
	if !errors.Is(gotErr, context.Canceled) {
		t.Errorf("converge() error = %v, want %v", gotErr, context.Canceled)
	}
	esc, _ = st.Escalation(id)
	if esc.Applied {
		t.Fatal("cancel during agent-slot wait must leave Applied=false")
	}

	restart := func(engN *Engine) *Loop {
		t.Helper()
		engN.Recover()
		recN, _ := st.Workspace(id)
		ln := newLoop(engN, recN)
		ln.guard = guard
		if ln.answer == nil {
			t.Fatal("pending grant must re-seed Keep iterating")
		}
		if ln.applyAnsweredCap(context.Background()) {
			t.Fatal("keep iterating must not finish the loop")
		}
		escN, _ := st.Escalation(id)
		if escN.Applied {
			t.Fatal("grant must stay pending until a turn consumes it")
		}
		if ln.grantedExtra <= 0 {
			t.Fatal("replay must credit grantedExtra")
		}
		return ln
	}

	eng2 := New(st)
	eng2.SetRunner(&ScriptedRunner{})
	t.Cleanup(eng2.Shutdown)
	restart(eng2)

	eng3 := New(st)
	eng3.SetRunner(&ScriptedRunner{})
	t.Cleanup(eng3.Shutdown)
	restart(eng3)
}

func TestStructuredCapAnswerAppliedAfterEarlierSteer(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	id := "WS-docs-steer-then-ship"
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: id, Goal: "docs ship after steer", State: domain.StatePaused,
		Autonomy:   domain.AutonomyBalanced,
		Checkpoint: &domain.Checkpoint{PhaseID: "converge", Iteration: 4, State: domain.StateEscalated},
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	st.AddFinding(domain.Finding{
		ID: "f-" + id, WorkspaceID: id, Severity: domain.SevLow, Status: "open",
		Message: "typo", File: "README.md",
	})
	st.AddEscalation(domain.Escalation{
		WorkspaceID: id,
		Question:    "I've run 4 iteration(s) without converging — 1 open finding remain. How should I proceed?",
		Options: []domain.EscalationOption{
			{Text: "Keep iterating"},
			{Text: "Ship with caveats", Desc: "Open the PR noting remaining risk (rails currently allow it)."},
			{Text: "Stop"},
		},
		Status: "answered", Choice: "Ship with caveats", Answer: "Ship with caveats",
		Kind: escKindCap,
	})
	rec, _ := st.Workspace(id)
	l := newLoop(eng, rec)
	l.guard = resolvedGuard{Loop: bundles.LoopDef{Phases: []bundles.LoopPhase{
		{ID: "converge", Uses: bundles.VerbConverge, Next: "done"},
		{ID: "done", Uses: bundles.VerbArtifact},
	}}}
	l.queueSteer("Fix the heading first")

	if l.applyAnsweredCap(context.Background()) {
		t.Fatal("bundle cap ship with next should not finish the loop")
	}
	esc, _ := st.Escalation(id)
	if !esc.Applied {
		t.Fatal("structured ship must mark Applied despite leftover steer")
	}
	ws, _ := st.Workspace(id)
	if ws.Checkpoint == nil || ws.Checkpoint.PhaseID != "done" {
		t.Errorf("PhaseID = %v, want done", ws.Checkpoint)
	}

	eng2 := New(st)
	eng2.SetRunner(&ScriptedRunner{})
	t.Cleanup(eng2.Shutdown)
	eng2.Recover()
	rec2, _ := st.Workspace(id)
	l2 := newLoop(eng2, rec2)
	if l2.answer != nil {
		t.Fatal("applied ship must not re-seed at the done checkpoint")
	}
}
