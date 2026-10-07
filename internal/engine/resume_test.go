package engine

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"sync"
	"testing"
	"time"

	"recur/internal/cursor"
	"recur/internal/domain"
	"recur/internal/store"
)

// interruptedLoop persists the loop record an app CRASH leaves behind: an in-flight
// state with a checkpoint and no goroutine. Building the record directly is the honest
// simulation — a force-quit kills the process, and all that survives is the DB — and it
// is what Recover is specified to reconcile (D23, D55).
func interruptedLoop(t *testing.T, st *store.Store, projID string,
	cpState domain.WorkspaceState, iter int) string {
	t.Helper()
	id := "LOOP-RESUME-" + string(cpState)
	rec := domain.Workspace{
		ID:           id,
		ProjectID:    projID,
		Group:        domain.GroupActive,
		Goal:         "Add a resumable feature",
		Short:        "Add a resumable feature",
		State:        cpState,
		StatusLabel:  statusLabel(cpState),
		Phase:        fmt.Sprintf("iteration %d", iter),
		Iteration:    iter,
		Autonomy:     domain.AutonomyBalanced,
		BaseBranch:   "main",
		AgentCodes:   []string{"MA", "CO", "RV"},
		ElapsedLabel: "running",
		Checkpoint: &domain.Checkpoint{
			State: cpState, Iteration: iter, At: time.Now().Format(time.RFC3339),
		},
	}
	if _, err := st.AddWorkspace(rec); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	return id
}

// waitForState polls until the loop reaches one of want, then returns the record.
func waitForState(t *testing.T, st *store.Store, id string, what string,
	want ...domain.WorkspaceState) domain.Workspace {
	t.Helper()
	waitFor(t, 10*time.Second, what, func() bool {
		l, ok := st.Workspace(id)
		if !ok {
			return false
		}
		for _, w := range want {
			if l.State == w {
				return true
			}
		}
		return false
	})
	l, _ := st.Workspace(id)
	return l
}

// TestResumeContinuesAtCheckpointIteration is the headline D55 test: a loop that died
// while CODING iteration 3 must resume at iteration 3, not replay the loop from
// planning at iteration 1. The scripted reviewer converges immediately, so exactly ONE
// coding turn should run — proof the first two iterations were not redone.
func TestResumeContinuesAtCheckpointIteration(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id := interruptedLoop(t, st, projID, domain.StateCoding, 3)

	// Restart: reconcile, then auto-resume.
	eng.Recover()
	parked, _ := st.Workspace(id)
	if parked.State != domain.StatePaused || parked.Phase != phaseInterrupted {
		t.Fatalf("Recover should park an in-flight loop as paused/interrupted, got %s/%s",
			parked.State, parked.Phase)
	}
	if parked.Checkpoint == nil || parked.Checkpoint.Iteration != 3 {
		t.Fatalf("Recover must preserve the resume checkpoint, got %+v", parked.Checkpoint)
	}

	eng.AutoResume()
	final := waitForState(t, st, id, "resumed loop to settle",
		domain.StatePROpen, domain.StateFailed, domain.StateStopped)
	if final.State != domain.StatePROpen {
		t.Fatalf("resumed loop should converge to pr_open, got %s (%s)", final.State, final.LastError)
	}

	runner.mu.Lock()
	coderRuns, reviewRuns := runner.coderRuns, runner.reviewRuns
	runner.mu.Unlock()

	// One coding turn only: iterations 1 and 2 were NOT replayed.
	if coderRuns != 1 {
		t.Errorf("resume replayed the loop: want 1 coding turn, got %d", coderRuns)
	}
	if reviewRuns != 1 {
		t.Errorf("want 1 review turn, got %d", reviewRuns)
	}
	if final.Iteration != 3 {
		t.Errorf("resumed loop should keep iteration 3, got %d", final.Iteration)
	}
	// The convergence chart must record iteration 3, not a fresh iteration 1.
	if len(final.Convergence) != 1 || final.Convergence[0].Iteration != 3 {
		t.Errorf("convergence should be recorded against iteration 3, got %+v", final.Convergence)
	}
}

// TestResumeSkipsCoderWhenInterruptedAfterCommit covers the reviewing/verifying
// checkpoints: the coding turn for that iteration already committed, so resume must
// re-review the existing diff rather than run the Coder again over finished work.
func TestResumeSkipsCoderWhenInterruptedAfterCommit(t *testing.T) {
	for _, cp := range []domain.WorkspaceState{domain.StateReviewing, domain.StateVerifying} {
		t.Run(string(cp), func(t *testing.T) {
			repo := initRepo(t)
			runner := &convergeRunner{findingsPerIter: []int{0}}
			eng, st, _, _ := newTestEngine(t, runner)
			projID := addProject(t, st, repo)

			id := interruptedLoop(t, st, projID, cp, 2)
			eng.Recover()
			eng.AutoResume()

			final := waitForState(t, st, id, "resumed loop to settle",
				domain.StatePROpen, domain.StateFailed, domain.StateStopped)
			if final.State != domain.StatePROpen {
				t.Fatalf("want pr_open, got %s (%s)", final.State, final.LastError)
			}

			runner.mu.Lock()
			coderRuns, reviewRuns := runner.coderRuns, runner.reviewRuns
			runner.mu.Unlock()

			if coderRuns != 0 {
				t.Errorf("a loop interrupted at %s already committed its code: "+
					"want 0 coding turns, got %d", cp, coderRuns)
			}
			if reviewRuns != 1 {
				t.Errorf("want 1 review turn, got %d", reviewRuns)
			}
			if final.Iteration != 2 {
				t.Errorf("want iteration 2 preserved, got %d", final.Iteration)
			}
		})
	}
}

// sessionSpyRunner records the ResumeSession the engine asked for on each coder turn.
type sessionSpyRunner struct {
	mu           sync.Mutex
	coderResumes []string
}

func (r *sessionSpyRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if spec.Role == domain.RoleCoder {
		r.mu.Lock()
		r.coderResumes = append(r.coderResumes, spec.ResumeSession)
		r.mu.Unlock()
		sr := &ScriptedRunner{Edits: map[string]string{"work.txt": "edit\n"}}
		if err := sr.applyEdits(spec.Worktree); err != nil {
			return Result{}, err
		}
	}
	return Result{Subtype: "success", SessionID: "new-session"}, nil
}

// TestResumeRestoresCoderSession is the "agents lost all memory" fix (D55): the Coder's
// FIRST turn after a resume continues the claude session it was killed in (--resume),
// so it still knows what it was doing. Later turns start fresh, as they always did.
func TestResumeRestoresCoderSession(t *testing.T) {
	repo := initRepo(t)
	runner := &sessionSpyRunner{}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id := interruptedLoop(t, st, projID, domain.StateCoding, 2)
	// The Coder had a live session when the app died.
	st.UpsertAgent(id, domain.Agent{
		ID: id + "-CO", WorkspaceID: id, Code: "CO", Name: "Coder", SessionID: "sess-killed-mid-edit",
	})

	eng.Recover()
	eng.AutoResume()
	waitForState(t, st, id, "resumed loop to settle",
		domain.StatePROpen, domain.StateFailed, domain.StateStopped)

	runner.mu.Lock()
	got := append([]string(nil), runner.coderResumes...)
	runner.mu.Unlock()

	if len(got) == 0 {
		t.Fatal("the Coder never ran")
	}
	if got[0] != "sess-killed-mid-edit" {
		t.Errorf("first resumed Coder turn must continue the killed session, got %q", got[0])
	}
	for i, s := range got[1:] {
		if s != "" {
			t.Errorf("coder turn %d should start a fresh session, got %q", i+2, s)
		}
	}
}

// promptSpyRunner records the prompt the engine gave each coder turn.
type promptSpyRunner struct {
	mu           sync.Mutex
	coderPrompts []string
}

func (r *promptSpyRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if spec.Role == domain.RoleCoder {
		r.mu.Lock()
		r.coderPrompts = append(r.coderPrompts, spec.Prompt)
		r.mu.Unlock()
		sr := &ScriptedRunner{Edits: map[string]string{"work.txt": "edit\n"}}
		if err := sr.applyEdits(spec.Worktree); err != nil {
			return Result{}, err
		}
	}
	return Result{Subtype: "success", SessionID: "s"}, nil
}

// TestAutoResumeIgnoresHumanPausedLoops pins the boundary that makes auto-resume safe:
// restarting the app continues loops the CRASH interrupted, but must never override a
// pause the human chose before quitting.
func TestAutoResumeIgnoresHumanPausedLoops(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	rec := domain.Workspace{
		ID: "LOOP-HUMAN-PAUSED", ProjectID: projID, Goal: "Paused by hand",
		State: domain.StatePaused, Phase: "paused by you", Iteration: 2,
		Autonomy: domain.AutonomyBalanced, BaseBranch: "main",
		Checkpoint: &domain.Checkpoint{State: domain.StateCoding, Iteration: 2},
	}
	if _, err := st.AddWorkspace(rec); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}

	eng.Recover()
	eng.AutoResume()

	// Give a wrongly-dispatched loop time to show itself.
	time.Sleep(200 * time.Millisecond)
	got, _ := st.Workspace(rec.ID)
	if got.State != domain.StatePaused {
		t.Errorf("auto-resume must not restart a human-paused loop, state=%s", got.State)
	}
	runner.mu.Lock()
	runs := runner.coderRuns
	runner.mu.Unlock()
	if runs != 0 {
		t.Errorf("no agent should have run, got %d coder turns", runs)
	}
}

// TestResumeSurvivesStoreReopen is the closest thing to the real bug report: the app is
// CLOSED mid-loop and reopened. The store is genuinely closed and reopened from disk and
// a FRESH Engine is built over it (as main does), proving the checkpoint and the agent's
// session id are durable across a process boundary — not just live in memory.
func TestResumeSurvivesStoreReopen(t *testing.T) {
	repo := initRepo(t)
	runner := &sessionSpyRunner{}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id := interruptedLoop(t, st, projID, domain.StateCoding, 4)
	st.UpsertAgent(id, domain.Agent{
		ID: id + "-CO", WorkspaceID: id, Code: "CO", Name: "Coder", SessionID: "sess-before-quit",
	})
	_ = eng // the first "process" never ran the loop; it died mid-flight

	// ---- the app quits ----
	if err := st.Close(); err != nil {
		t.Fatalf("close store: %v", err)
	}

	// ---- the app reopens: new store off the same data dir, new engine ----
	st2, err := store.New() // same $RECUR_DATA_DIR (set by newTestEngine)
	if err != nil {
		t.Fatalf("reopen store: %v", err)
	}
	t.Cleanup(func() { st2.Close() })

	reloaded, ok := st2.Workspace(id)
	if !ok {
		t.Fatal("loop missing after reopen")
	}
	if reloaded.Checkpoint == nil || reloaded.Checkpoint.Iteration != 4 {
		t.Fatalf("the resume checkpoint must be durable, got %+v", reloaded.Checkpoint)
	}

	eng2 := New(st2)
	eng2.SetRunner(runner)
	eng2.SetWorktreeRoot(eng.worktreeRoot)
	// Rail #1 stays off across the restart on its own: newTestEngine disabled it in the
	// PERSISTED policy, which is exactly the durability this test is about.
	t.Cleanup(func() { eng2.Shutdown() })

	eng2.Recover()
	eng2.AutoResume()

	final := waitForState(t, st2, id, "reopened loop to settle",
		domain.StatePROpen, domain.StateFailed, domain.StateStopped)
	if final.State != domain.StatePROpen {
		t.Fatalf("a loop resumed after an app restart should finish, got %s (%s)",
			final.State, final.LastError)
	}
	if final.Iteration != 4 {
		t.Errorf("want iteration 4 preserved across the restart, got %d", final.Iteration)
	}

	runner.mu.Lock()
	got := append([]string(nil), runner.coderResumes...)
	runner.mu.Unlock()
	if len(got) == 0 || got[0] != "sess-before-quit" {
		t.Errorf("the Coder should continue its pre-quit session, got %v", got)
	}
}

// TestAutoResumeStartsQueuedLoops covers the loop that was still waiting for a slot
// when the app exited. It has no checkpoint (nothing ran), and the queued drain only
// fires when another loop exits — so without auto-resume it would sit queued forever
// after a restart.
func TestAutoResumeStartsQueuedLoops(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	rec := domain.Workspace{
		ID: "LOOP-QUEUED", ProjectID: projID, Goal: "Queued when the app closed",
		State: domain.StateQueued, Phase: "waiting for a slot",
		Autonomy: domain.AutonomyBalanced, BaseBranch: "main",
	}
	if _, err := st.AddWorkspace(rec); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}

	eng.Recover()
	eng.AutoResume()

	final := waitForState(t, st, rec.ID, "queued loop to run after restart",
		domain.StatePROpen, domain.StateFailed, domain.StateStopped)
	if final.State != domain.StatePROpen {
		t.Fatalf("a queued loop should start and converge after a restart, got %s (%s)",
			final.State, final.LastError)
	}
	// It never ran before, so it starts from the beginning: iteration 1.
	if final.Iteration != 1 {
		t.Errorf("a never-started loop should begin at iteration 1, got %d", final.Iteration)
	}
}

// limitRunner fails the coder turn the way a real spend-limit refusal arrives: the CLI
// writes the provider's message to stderr and exits without a usable result.
type limitRunner struct {
	mu    sync.Mutex
	turns int
	msg   string
	// failUntil fails the first N coder turns, then succeeds — so a Retry can land.
	failUntil int
}

func (r *limitRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if spec.Role != domain.RoleCoder {
		return Result{Subtype: "success"}, nil
	}
	r.mu.Lock()
	r.turns++
	n := r.turns
	r.mu.Unlock()
	if n <= r.failUntil {
		res := Result{Subtype: "error_during_execution", IsError: true, Stderr: r.msg}
		return res, errors.New("claude exited without a result: exit status 1: " + r.msg)
	}
	sr := &ScriptedRunner{Edits: map[string]string{"work.txt": "edit\n"}}
	if err := sr.applyEdits(spec.Worktree); err != nil {
		return Result{}, err
	}
	return Result{Subtype: "success", SessionID: "s"}, nil
}

const spendLimitMsg = "You've hit your org's monthly spend limit · run /usage-credits to ask your admin for a higher limit"

// TestTransientLLMFailureIsRetryable is the headline D56 test: the exact spend-limit
// error a user hits must leave the loop RETRYABLE — carrying the provider's own message,
// keeping its checkpoint, and sitting in "needs you" — not silently terminal with
// "error_during_execution".
func TestTransientLLMFailureIsRetryable(t *testing.T) {
	repo := initRepo(t)
	runner := &limitRunner{msg: spendLimitMsg, failUntil: 1}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Task that hits the spend limit", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	failed := waitForState(t, st, id, "loop to fail on the spend limit", domain.StateFailed)

	if !failed.Retryable {
		t.Error("a spend-limit failure must be marked retryable")
	}
	if !strings.Contains(failed.LastError, "monthly spend limit") {
		t.Errorf("LastError should carry the provider's message, got %q", failed.LastError)
	}
	if failed.Checkpoint == nil {
		t.Fatal("a retryable failure must keep its checkpoint so Retry can re-enter")
	}
	if failed.Group != domain.GroupNeeds {
		t.Errorf("a retryable failure needs the human, group=%s", failed.Group)
	}
	if !resumable(failed) {
		t.Error("a retryable failed loop must be resumable")
	}

	// Retry: the runner now succeeds, so the loop must finish from the checkpoint.
	eng.Retry(id)
	final := waitForState(t, st, id, "retried loop to settle",
		domain.StatePROpen, domain.StateStopped)
	if final.State != domain.StatePROpen {
		t.Fatalf("retried loop should converge, got %s (%s)", final.State, final.LastError)
	}
	if final.Retryable || final.LastError != "" {
		t.Errorf("a successful retry must clear the failure marks, got retryable=%v err=%q",
			final.Retryable, final.LastError)
	}
}

// genericFailRunner fails the coder with a real defect, not a transient error.
type genericFailRunner struct{}

func (genericFailRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if spec.Role != domain.RoleCoder {
		return Result{Subtype: "success"}, nil
	}
	return Result{Subtype: "error_during_execution", IsError: true},
		errors.New("the agent could not parse the file")
}

// TestGenuineFailureIsNotRetryable is the other half of D56: a real failure stays
// terminal and offers no Retry — re-running it would just fail again.
func TestGenuineFailureIsNotRetryable(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, genericFailRunner{})
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Task that genuinely fails", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	failed := waitForState(t, st, id, "loop to fail", domain.StateFailed)

	if failed.Retryable {
		t.Error("a genuine failure must not be marked retryable")
	}
	if resumable(failed) {
		t.Error("a non-retryable failed loop must stay terminal")
	}

	// Retry is a no-op: no goroutine, still failed.
	eng.Retry(id)
	time.Sleep(200 * time.Millisecond)
	after, _ := st.Workspace(id)
	if after.State != domain.StateFailed {
		t.Errorf("Retry on a non-retryable loop must be a no-op, state=%s", after.State)
	}
}

// plannerFailRunner fails the planning (Maestro) turn, then succeeds so Retry
// can finish. Coder/reviewer succeed whenever they run — a transient planner
// failure must not reach them.
type plannerFailRunner struct {
	mu           sync.Mutex
	plannerTurns int
	coderTurns   int
	failUntil    int
	msg          string
	genuine      bool
}

func (r *plannerFailRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if spec.Role == domain.RoleMaestro {
		r.mu.Lock()
		r.plannerTurns++
		n := r.plannerTurns
		r.mu.Unlock()
		if n <= r.failUntil {
			if r.genuine {
				return Result{Subtype: "error_during_execution", IsError: true},
					errors.New("the agent could not parse the file")
			}
			res := Result{Subtype: "error_during_execution", IsError: true, Text: r.msg}
			return res, errors.New("claude exited without a result: " + r.msg)
		}
		if emit != nil {
			emit(ParsedEvent{Kind: KindText, Text: "1. Do the thing"})
		}
		return Result{Subtype: "success", SessionID: spec.AgentID, Text: "1. Do the thing"}, nil
	}
	if spec.Role == domain.RoleCoder {
		r.mu.Lock()
		r.coderTurns++
		r.mu.Unlock()
		sr := &ScriptedRunner{Edits: map[string]string{"work.txt": "edit\n"}}
		if err := sr.applyEdits(spec.Worktree); err != nil {
			return Result{}, err
		}
		return Result{Subtype: "success", SessionID: "s"}, nil
	}
	return Result{Subtype: "success"}, nil
}

func (r *plannerFailRunner) counts() (planner, coder int) {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.plannerTurns, r.coderTurns
}

// TestPlannerTransientFailureIsRetryable: an overloaded planner must park the
// loop at planning (Retryable) instead of coding with no plan. Retry re-enters
// the planning turn and can succeed.
func TestPlannerTransientFailureIsRetryable(t *testing.T) {
	repo := initRepo(t)
	runner := &plannerFailRunner{msg: "overloaded_error: the model is overloaded", failUntil: 1}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Task whose planner is overloaded", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	failed := waitForState(t, st, id, "loop to fail on the overloaded planner", domain.StateFailed)

	if !failed.Retryable {
		t.Error("an overloaded planner must be marked retryable")
	}
	if !strings.Contains(failed.LastError, "overloaded") {
		t.Errorf("LastError = %q, want it to contain overloaded", failed.LastError)
	}
	if failed.Checkpoint == nil || failed.Checkpoint.PhaseID != "planning" {
		t.Errorf("checkpoint = %+v, want PhaseID=planning so Retry re-enters planning", failed.Checkpoint)
	}
	planner, coder := runner.counts()
	if planner < 1 {
		t.Error("planner turn never ran")
	}
	if coder != 0 {
		t.Errorf("coderTurns = %d, want 0 — must not code without a plan after a retryable planner failure", coder)
	}

	eng.Retry(id)
	final := waitForState(t, st, id, "retried loop to settle after planner recovers",
		domain.StatePROpen, domain.StateStopped)
	if final.State != domain.StatePROpen {
		t.Fatalf("retried loop should converge, got %s (%s)", final.State, final.LastError)
	}
	_, coder = runner.counts()
	if coder == 0 {
		t.Error("coder must run after a successful planner retry")
	}
}

// TestPlannerGenuineFailureStillDegrades: a non-transient planner defect still
// proceeds to coding with the bare task (best-effort), unlike overload.
func TestPlannerGenuineFailureStillDegrades(t *testing.T) {
	repo := initRepo(t)
	runner := &plannerFailRunner{genuine: true, failUntil: 1}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Task whose planner genuinely fails", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	final := waitForState(t, st, id, "loop to settle after genuine planner failure",
		domain.StatePROpen, domain.StateFailed, domain.StateStopped)
	if final.State != domain.StatePROpen {
		t.Fatalf("genuine planner failure should still code, got %s (%s)", final.State, final.LastError)
	}
	if final.Retryable {
		t.Error("a genuine planner failure that degraded must not leave the loop retryable")
	}
	_, coder := runner.counts()
	if coder == 0 {
		t.Error("coderTurns = 0, want the loop to proceed to coding after a genuine planner failure")
	}
}

// TestClassifyAgentError pins the transient/genuine split, including the exact message
// from the user report and the near-miss cases that must NOT be retried.
func TestClassifyAgentError(t *testing.T) {
	cases := []struct {
		name      string
		err       error
		res       Result
		transient bool
	}{
		{"org monthly spend limit (stderr)", errors.New("claude exited without a result: exit status 1"),
			Result{Stderr: spendLimitMsg}, true},
		{"usage limit in result text", nil,
			Result{IsError: true, Subtype: "error_during_execution", Text: "Usage limit reached"}, true},
		{"429 rate limit", errors.New("api error 429: rate_limit_error"), Result{}, true},
		{"overloaded", nil, Result{IsError: true, Text: "overloaded_error"}, true},
		{"503", errors.New("unexpected status 503 Service Unavailable"), Result{}, true},
		{"connection reset", errors.New("read tcp: connection reset by peer"), Result{}, true},
		{"cursor stream died mid-turn", errors.New("cursor run ended without a result (bridge log: /tmp/x.log)"), Result{}, true},
		{"credit balance", nil, Result{IsError: true, Stderr: "Your credit balance is too low"}, true},
		{"cursor bridge [internal]", fmt.Errorf("cursor run: %w", &cursor.Error{Code: "internal", Message: "internal error"}), Result{}, true},
		{"genuine parse failure", errors.New("could not parse the file"), Result{}, false},
		{"max turns", nil, Result{IsError: true, Subtype: "error_max_turns"}, false},
		{"engine commit failure", errors.New("engine commit: nothing to commit"), Result{}, false},
		{"codex refresh token", nil, Result{Stderr: "Failed to refresh token: 401 Unauthorized {"}, true},
		{"codex_login 401 without refresh phrase", nil, Result{Stderr: "ERROR codex_login::auth::manager - 401 Unauthorized"}, true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			reason, transient := classifyAgentError(tc.err, tc.res)
			if transient != tc.transient {
				t.Errorf("transient=%v, want %v (reason %q)", transient, tc.transient, reason)
			}
			if reason == "" {
				t.Error("a failure must always produce a human-readable reason")
			}
		})
	}
}

// TestResumePointForMapsCheckpoints pins the checkpoint → re-entry mapping:
// no checkpoint means start fresh; in-flight phases resume; a landed PR is
// not resumable.
func TestResumePointForMapsCheckpoints(t *testing.T) {
	cases := []struct {
		name      string
		rec       domain.Workspace
		wantNil   bool
		wantState domain.WorkspaceState
		wantIter  int
		wantSkip  bool
	}{
		{name: "no checkpoint starts fresh", rec: domain.Workspace{}, wantNil: true},
		{
			name:      "planning",
			rec:       domain.Workspace{Checkpoint: &domain.Checkpoint{State: domain.StatePlanning, Iteration: 0}},
			wantState: domain.StatePlanning, wantIter: 1,
		},
		{
			name:      "coding re-runs the coder",
			rec:       domain.Workspace{Checkpoint: &domain.Checkpoint{State: domain.StateCoding, Iteration: 4}},
			wantState: domain.StateCoding, wantIter: 4,
		},
		{
			name:      "reviewing skips the coder",
			rec:       domain.Workspace{Checkpoint: &domain.Checkpoint{State: domain.StateReviewing, Iteration: 2}},
			wantState: domain.StateCoding, wantIter: 2, wantSkip: true,
		},
		{
			name:      "verifying skips the coder",
			rec:       domain.Workspace{Checkpoint: &domain.Checkpoint{State: domain.StateVerifying, Iteration: 3}},
			wantState: domain.StateCoding, wantIter: 3, wantSkip: true,
		},
		{
			name:    "terminal checkpoint is not resumable",
			rec:     domain.Workspace{Checkpoint: &domain.Checkpoint{State: domain.StatePROpen, Iteration: 1}},
			wantNil: true,
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := resumePointFor(tc.rec, nil)
			if tc.wantNil {
				if got != nil {
					t.Fatalf("want nil resume point, got %+v", got)
				}
				return
			}
			if got == nil {
				t.Fatal("want a resume point, got nil")
			}
			if got.State != tc.wantState || got.Iteration != tc.wantIter || got.SkipCoder != tc.wantSkip {
				t.Errorf("got {%s iter=%d skip=%v}, want {%s iter=%d skip=%v}",
					got.State, got.Iteration, got.SkipCoder, tc.wantState, tc.wantIter, tc.wantSkip)
			}
		})
	}
}

// TestSetWorkspaceStateWritesCheckpoint pins the write-through: every in-flight transition
// records a resume position, and a genuinely terminal one clears it.
func TestSetWorkspaceStateWritesCheckpoint(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, repo)
	rec := domain.Workspace{ID: "LOOP-CP", ProjectID: projID, Goal: "g", State: domain.StateQueued}
	if _, err := st.AddWorkspace(rec); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}

	eng.setWorkspaceState(rec.ID, domain.StateReviewing, "reviewing iteration 2")
	got, _ := st.Workspace(rec.ID)
	if got.Checkpoint == nil || got.Checkpoint.State != domain.StateReviewing {
		t.Fatalf("in-flight transition must checkpoint, got %+v", got.Checkpoint)
	}

	eng.setWorkspaceState(rec.ID, domain.StatePROpen, "done")
	got, _ = st.Workspace(rec.ID)
	if got.Checkpoint != nil {
		t.Errorf("a landed loop must not keep a resume checkpoint, got %+v", got.Checkpoint)
	}
}

func TestSetWorkspaceStatePreservesPhaseID(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, repo)
	rec := domain.Workspace{
		ID: "LOOP-PHASE", ProjectID: projID, Goal: "g", State: domain.StateQueued,
		Checkpoint: &domain.Checkpoint{PhaseID: "converge", ResumePhaseID: "pr"},
	}
	if _, err := st.AddWorkspace(rec); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}

	eng.setWorkspaceState(rec.ID, domain.StateCoding, "iteration 1")
	got, _ := st.Workspace(rec.ID)
	if got.Checkpoint == nil {
		t.Fatal("in-flight transition must keep a checkpoint")
	}
	if got.Checkpoint.PhaseID != "converge" || got.Checkpoint.ResumePhaseID != "pr" {
		t.Fatalf("PhaseID/ResumePhaseID wiped: %+v", got.Checkpoint)
	}
	if got.Checkpoint.State != domain.StateCoding {
		t.Fatalf("State=%s, want coding", got.Checkpoint.State)
	}
}
