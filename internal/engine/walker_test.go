package engine

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"recur/internal/bundles"
	"recur/internal/domain"
)

func TestInboundGraphPhase(t *testing.T) {
	l := &Loop{guard: resolvedGuard{Loop: bundles.LoopDef{
		Phases: []bundles.LoopPhase{
			{ID: "planning", Next: "converge"},
			{ID: "converge", Next: "pr"},
			{ID: "pr", Uses: bundles.VerbEnsurePR},
			{ID: "security", Dispatch: "on_demand"},
		},
	}}}
	if got := l.inboundGraphPhase("pr"); got != "converge" {
		t.Fatalf("inbound(pr)=%q, want converge", got)
	}
	if got := l.inboundGraphPhase("converge"); got != "planning" {
		t.Fatalf("inbound(converge)=%q, want planning", got)
	}
	if got := l.inboundGraphPhase("missing"); got != "" {
		t.Fatalf("inbound(missing)=%q, want empty", got)
	}
	if got := l.ensurePRPhaseID(); got != "pr" {
		t.Errorf("ensurePRPhaseID() = %q, want pr", got)
	}
}

func TestSeedFixFindings(t *testing.T) {
	open := []domain.Finding{{ID: "f1", Status: "open"}}
	if got := seedFixFindings(5, false, open); len(got) != 1 {
		t.Errorf("seedFixFindings(5, false, open) len = %d, want 1", len(got))
	}
	if got := seedFixFindings(5, true, open); got != nil {
		t.Errorf("seedFixFindings(5, true, open) = %v, want nil (start=review)", got)
	}
	if got := seedFixFindings(1, false, open); got != nil {
		t.Errorf("seedFixFindings(1, false, open) = %v, want nil (first entry)", got)
	}
	if got := seedFixFindings(5, false, nil); got != nil {
		t.Errorf("seedFixFindings(5, false, nil) = %v, want nil", got)
	}
}

func TestConvergeRoundWindow(t *testing.T) {
	tests := []struct {
		start, max, want int
	}{
		{1, 4, 4},
		{3, 4, 6},
		{7, 6, 12},
		{5, 4, 8},
		{0, 4, 4},
		{1, 0, 1},
	}
	for _, tc := range tests {
		got := convergeRoundWindow(tc.start, tc.max)
		if got != tc.want {
			t.Errorf("convergeRoundWindow(%d, %d) = %d, want %d", tc.start, tc.max, got, tc.want)
		}
	}
}

func TestPhaseRetries(t *testing.T) {
	n := 2
	l := &Loop{guard: resolvedGuard{Policy: bundles.Policy{Retries: 3}}}
	if got := l.phaseRetries(bundles.LoopPhase{Retries: &n}); got != 2 {
		t.Fatalf("phase override retries=%d, want 2", got)
	}
	if got := l.phaseRetries(bundles.LoopPhase{}); got != 3 {
		t.Fatalf("policy retries=%d, want 3", got)
	}
}

func TestRunLoopPhaseCancelledWritesStopped(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	id := "WS-walk-stop"
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: id, Goal: "x", State: domain.StateCoding, Phase: "implementing",
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	l := &Loop{ID: id, eng: eng}
	_, done := l.runLoopPhase(ctx, bundles.LoopPhase{ID: "x", Uses: "unknown"})
	if !done {
		t.Fatal("runLoopPhase(cancelled) done = false, want true")
	}
	ws, ok := st.Workspace(id)
	if !ok {
		t.Fatal("workspace missing after runLoopPhase")
	}
	if ws.State != domain.StateStopped {
		t.Errorf("runLoopPhase(cancelled) state = %s, want %s", ws.State, domain.StateStopped)
	}
}

func TestRunLoopPhaseCancelledIdlesWhenRequested(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	id := "WS-walk-idle"
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: id, Goal: "x", State: domain.StateCoding, Phase: "implementing",
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	l := &Loop{ID: id, eng: eng}
	l.requestIdle()
	_, done := l.runLoopPhase(ctx, bundles.LoopPhase{ID: "x", Uses: "unknown"})
	if !done {
		t.Fatal("runLoopPhase(cancelled idle) done = false, want true")
	}
	ws, ok := st.Workspace(id)
	if !ok {
		t.Fatal("workspace missing after runLoopPhase")
	}
	if ws.State != domain.StatePaused || ws.Phase != "idle" {
		t.Errorf("runLoopPhase(cancelled idle) state = %s phase = %q, want paused/idle", ws.State, ws.Phase)
	}
}

func TestTakeOnFailPreservesTurnError(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	id := "WS-onfail"
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: id, Goal: "x", State: domain.StateCoding, Phase: "implementing",
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	l := &Loop{
		ID: id, eng: eng,
		guard: resolvedGuard{Policy: bundles.Policy{OnFail: "fail"}},
	}
	te := newTurnError("Coder", errors.New("provider overloaded"), Result{IsError: true})
	_, done := l.takeOnFail(context.Background(), bundles.LoopPhase{ID: "converge"}, te)
	if !done {
		t.Fatal("takeOnFail(fail) done = false, want true")
	}
	ws, ok := st.Workspace(id)
	if !ok {
		t.Fatal("workspace missing after takeOnFail")
	}
	if !ws.Retryable {
		t.Errorf("takeOnFail(turnError) Retryable = false, want true")
	}
	if !strings.Contains(ws.LastError, "overloaded") {
		t.Errorf("takeOnFail LastError = %q, want it to contain overloaded", ws.LastError)
	}
	if ws.LastError == errPhaseFailed.Error() {
		t.Errorf("takeOnFail LastError = %q, must not use synthetic errPhaseFailed", ws.LastError)
	}
}

func TestTurnContextUsesPhaseTimeout(t *testing.T) {
	l := &Loop{
		currentPhase: bundles.LoopPhase{Timeout: "50ms"},
		guard:        resolvedGuard{Policy: bundles.Policy{Timeout: "4h"}},
	}
	ctx, cancel := l.turnContext(context.Background())
	defer cancel()
	dl, ok := ctx.Deadline()
	if !ok {
		t.Fatal("turnContext() missing deadline, want 50ms phase timeout")
	}
	remain := time.Until(dl)
	if remain > 200*time.Millisecond || remain < 0 {
		t.Errorf("turnContext() deadline remain = %v, want ~50ms", remain)
	}
}

func TestFinishStoppedIfInFlightWritesStopped(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	id := "WS-inflight"
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: id, Goal: "x", State: domain.StateCoding, Phase: "iteration 1",
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	l := &Loop{ID: id, eng: eng}
	l.finishStoppedIfInFlight()
	ws, ok := st.Workspace(id)
	if !ok {
		t.Fatal("workspace missing")
	}
	if ws.State != domain.StateStopped {
		t.Errorf("finishStoppedIfInFlight() state = %s, want %s", ws.State, domain.StateStopped)
	}
}

func TestFinishStoppedIfInFlightLeavesPROpen(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	id := "WS-propen"
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: id, Goal: "x", State: domain.StatePROpen, Phase: "PR open — ready for your review",
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	l := &Loop{ID: id, eng: eng}
	l.finishStoppedIfInFlight()
	ws, ok := st.Workspace(id)
	if !ok {
		t.Fatal("workspace missing")
	}
	if ws.State != domain.StatePROpen {
		t.Errorf("finishStoppedIfInFlight() state = %s, want %s", ws.State, domain.StatePROpen)
	}
}

func TestFinishStoppedIfInFlightUsesWalkExit(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	id := "WS-walk-exit"
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: id, Goal: "x", State: domain.StateCoding, Phase: "implementing",
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	l := &Loop{
		ID: id, eng: eng,
		exitState: domain.StatePROpen, exitPhase: "PR open — ready for your review",
	}
	l.finishStoppedIfInFlight()
	if l.exitState != domain.StatePROpen {
		t.Errorf("exitState = %s, want %s", l.exitState, domain.StatePROpen)
	}
	ws, ok := st.Workspace(id)
	if !ok {
		t.Fatal("workspace missing")
	}
	if ws.State != domain.StateCoding {
		t.Errorf("workspace state = %s, want %s (sibling in-flight)", ws.State, domain.StateCoding)
	}
}

func TestPhaseThresholdRejectsLooserOverride(t *testing.T) {
	g := resolvedGuard{Policy: bundles.Policy{
		ID:                "recur.policies.balanced",
		FindingsThreshold: bundles.FindingsThreshold{Levels: map[string]int{"medium": 0, "high": 0, "critical": 0}},
	}}
	looser := bundles.FindingsThreshold{Levels: map[string]int{"high": 0, "critical": 0}}
	th := g.phaseThreshold(bundles.LoopPhase{ID: "pr", Findings: &looser})
	if th.Levels["medium"] != 0 {
		t.Errorf("phaseThreshold(looser) = %+v, want policy medium=0", th.Levels)
	}
}

func TestQARuntimeExtraKeepsURL(t *testing.T) {
	extra := qaRuntimeExtra("http://127.0.0.1:4173", "/tmp/shots")
	if !strings.Contains(extra, "http://127.0.0.1:4173") {
		t.Errorf("qaRuntimeExtra() = %q, want app URL", extra)
	}
	if !strings.Contains(extra, "/tmp/shots") {
		t.Errorf("qaRuntimeExtra() = %q, want screenshot dir", extra)
	}
	if !strings.Contains(extra, "attach_evidence") {
		t.Errorf("qaRuntimeExtra() = %q, want attach_evidence handshake", extra)
	}
}

func TestRunPhaseAttemptBoardCapIsTerminal(t *testing.T) {
	old := boardMaxRounds
	boardMaxRounds = 0
	t.Cleanup(func() { boardMaxRounds = old })

	eng, st, _, _ := newTestEngine(t, nil)
	id := "WS-board-cap-walk"
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: id, Goal: "x", State: domain.StateCoding, Phase: "implementing",
		Plan: &domain.Plan{Steps: []domain.PlanStep{{ID: "S1", Title: "do", Status: domain.StepPending}}},
	}); err != nil {
		t.Fatal(err)
	}
	l := &Loop{
		ID: id, eng: eng, planStructured: true,
		guard: resolvedGuard{Policy: bundles.Policy{MaxRounds: 4}},
	}
	next, done, err := l.runPhaseAttempt(context.Background(), bundles.LoopPhase{
		ID: "converge", Uses: bundles.VerbConverge, Next: "pr",
	})
	if err != nil {
		t.Fatalf("runPhaseAttempt: %v", err)
	}
	if !done || next != string(domain.StatePaused) {
		t.Fatalf("runPhaseAttempt(board cap) next=%q done=%v, want paused/true", next, done)
	}
	ws, _ := st.Workspace(id)
	if ws.State != domain.StatePaused || ws.Phase != boardCapPhase {
		t.Errorf("runPhaseAttempt(board cap) state=%s phase=%q, want paused/%s", ws.State, ws.Phase, boardCapPhase)
	}
	l.finishStoppedIfInFlight()
	ws, _ = st.Workspace(id)
	if ws.State != domain.StatePaused || ws.Phase != boardCapPhase {
		t.Errorf("finishStoppedIfInFlight() state=%s phase=%q, want paused/%s", ws.State, ws.Phase, boardCapPhase)
	}
}
