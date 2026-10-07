package engine

import (
	"context"
	"strconv"
	"sync"
	"testing"
	"time"

	"recur/internal/domain"
	"recur/internal/store"
)

// panicRunner panics on Run, simulating a fault inside an agent turn. Used to prove
// a loop goroutine panic degrades to a failed loop instead of crashing the app (#6).
type panicRunner struct{}

func (panicRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	panic("boom in the runner")
}

// TestEngineLoopPanicRecovers covers fix #6: a panic inside a loop goroutine is
// recovered, the loop ends "failed", the engine/process survives, the governor
// loop-slot is released, and a SUBSEQUENT loop runs to completion unaffected.
func TestEngineLoopPanicRecovers(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, panicRunner{})
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "this loop panics", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}

	// The panicking loop must settle as failed (not crash the test binary).
	waitFor(t, 5*time.Second, "panicking loop to fail", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateFailed
	})

	// The loop-slot must have been released by loopExited despite the panic.
	waitFor(t, 3*time.Second, "loop slot released after panic", func() bool {
		return eng.Governor().LoopsRunning() == 0
	})
	if _, running := eng.loops.Load(id); running {
		t.Error("panicked loop still present in the running-loop map")
	}

	// The engine survives: a fresh, healthy loop runs to completion.
	eng.SetRunner(&ScriptedRunner{
		Edits: map[string]string{"ok.txt": "fine\n"},
		Lines: fixtureLines("sess-after-panic", "ok.txt", "done", 0.05),
	})
	id2, err := eng.StartWorkspace(projID, "this loop is healthy", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace after panic: %v", err)
	}
	waitFor(t, 5*time.Second, "healthy loop after panic to reach pr_open", func() bool {
		l, ok := st.Workspace(id2)
		return ok && l.State == domain.StatePROpen
	})
}

// TestEngineRecoverInterruptedLoops covers fix #2 (PRD §6.5): a loop persisted in a
// running state (no goroutine) is reconciled to paused/interrupted/needs by a fresh
// engine's Recover() over the same store.
func TestEngineRecoverInterruptedLoops(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, repo)

	// Persist a loop in a mid-run state directly, as if the app crashed while coding.
	rec := domain.Workspace{
		ID:          "LOOP-CRASHED",
		ProjectID:   projID,
		Group:       domain.GroupActive,
		Goal:        "interrupted work",
		State:       domain.StateCoding,
		StatusLabel: "Coding",
		Phase:       "iteration 1",
	}
	if _, err := st.AddWorkspace(rec); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}

	// A brand-new engine over the same store has no goroutine for this loop.
	eng2 := New(st)
	var got domain.Workspace
	sink := &recordingSink{}
	eng2.Bus.Subscribe(sink.sink)
	eng2.Recover()

	l, ok := st.Workspace(rec.ID)
	if !ok {
		t.Fatal("loop missing after Recover")
	}
	got = l
	if got.State != domain.StatePaused {
		t.Errorf("recovered loop state: got %q, want paused", got.State)
	}
	if got.Phase != "interrupted" {
		t.Errorf("recovered loop phase: got %q, want interrupted", got.Phase)
	}
	if got.Group != domain.GroupNeeds {
		t.Errorf("recovered loop group: got %q, want needs", got.Group)
	}
	if !sink.has("workspace.updated") {
		t.Error("Recover should emit loop.updated for the reconciled loop")
	}
	_ = eng
}

// TestEngineRecoverHealsAnsweredStuckEscalation covers the pre-fix stuck state:
// escalation status=answered but loop still state=escalated with no goroutine.
// Recover must leave an honest paused resting state Resume can pick up.
func TestEngineRecoverHealsAnsweredStuckEscalation(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, repo)

	rec := domain.Workspace{
		ID:          "LOOP-STUCK-ESC",
		ProjectID:   projID,
		Group:       domain.GroupNeeds,
		Goal:        "answered but never resumed",
		State:       domain.StateEscalated,
		StatusLabel: "Needs you",
		Phase:       "asking a question",
	}
	if _, err := st.AddWorkspace(rec); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	st.AddEscalation(domain.Escalation{
		WorkspaceID: rec.ID,
		Question:    "A or B?",
		Status:      "answered",
		Answer:      "Approach A",
	})
	st.AddInboxItem(domain.InboxItem{
		ID: "ib-stuck", Target: "workspace:" + rec.ID, Kind: "escalation", Unread: true,
	})

	eng2 := New(st)
	eng2.Recover()

	got, ok := st.Workspace(rec.ID)
	if !ok {
		t.Fatal("loop missing after Recover")
	}
	if got.State != domain.StatePaused {
		t.Errorf("healed state: got %q, want paused", got.State)
	}
	if got.Group != domain.GroupActive {
		t.Errorf("healed group: got %q, want active", got.Group)
	}
	if got.Phase != "answered — waiting to resume" {
		t.Errorf("healed phase: got %q", got.Phase)
	}
	for _, ib := range st.Inbox() {
		if ib.Target == "workspace:"+rec.ID {
			t.Errorf("inbox row for answered stuck escalation should be cleared")
		}
	}
	_ = eng
}

// TestEngineQueuedLoopDrains covers fix #4: with MaxLoops=1, loop A runs while B is
// queued; when A finishes, the governor slot frees and B auto-dispatches and runs
// to completion.
func TestEngineQueuedLoopDrains(t *testing.T) {
	repo := initRepo(t)

	// A blocks until we release the gate, holding the only loop-slot. After A is
	// released and finishes, the engine must drain B (which uses the same runner —
	// the gate is already closed by then, so B runs straight through).
	gate := make(chan struct{})
	runner := &blockingRunner{gate: gate, edits: map[string]string{"drain.txt": "x\n"}}

	eng, st, _, _ := newTestEngine(t, runner)
	eng.gov = NewGovernor(1, 8) // cap loops at 1
	projID := addProject(t, st, repo)

	idA, err := eng.StartWorkspace(projID, "loop A", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace A: %v", err)
	}
	waitFor(t, 3*time.Second, "loop A running", func() bool {
		return eng.Governor().LoopsRunning() == 1
	})

	idB, err := eng.StartWorkspace(projID, "loop B", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace B: %v", err)
	}
	// B has no slot → queued.
	lB, ok := st.Workspace(idB)
	if !ok || lB.State != domain.StateQueued {
		t.Fatalf("loop B should be queued, got state=%q ok=%v", lB.State, ok)
	}

	// Release A; it finishes and loopExited should drain B.
	close(gate)
	waitFor(t, 5*time.Second, "loop A to finish", func() bool {
		l, _ := st.Workspace(idA)
		return l.State == domain.StatePROpen
	})
	waitFor(t, 5*time.Second, "queued loop B to auto-start and finish", func() bool {
		l, ok := st.Workspace(idB)
		return ok && l.State == domain.StatePROpen
	})
}

// TestEngineConcurrentStartWorkspaceRaceClean covers fix #1: several StartWorkspaces fired
// from multiple goroutines must be race-clean now that there is no shared mutable
// engine.git — each loop owns its manager. Run under -race to detect a regression.
func TestEngineConcurrentStartWorkspaceRaceClean(t *testing.T) {
	repo := initRepo(t)
	runner := &ScriptedRunner{
		Edits: map[string]string{"c.txt": "concurrent\n"},
		Lines: fixtureLines("sess-conc", "c.txt", "ok", 0.05),
	}
	eng, st, _, _ := newTestEngine(t, runner)
	eng.gov = NewGovernor(0, 0) // unlimited slots so all start immediately
	projID := addProject(t, st, repo)

	const n = 8
	var wg sync.WaitGroup
	ids := make([]string, n)
	wg.Add(n)
	for i := 0; i < n; i++ {
		go func(i int) {
			defer wg.Done()
			id, err := eng.StartWorkspace(projID, "concurrent task", "", "")
			if err != nil {
				t.Errorf("StartWorkspace %d: %v", i, err)
				return
			}
			ids[i] = id
		}(i)
	}
	wg.Wait()

	// Every loop should reach a terminal pr_open without a data race or panic.
	for _, id := range ids {
		if id == "" {
			continue
		}
		id := id
		waitFor(t, 10*time.Second, "concurrent loop "+id+" to finish", func() bool {
			l, ok := st.Workspace(id)
			return ok && l.State == domain.StatePROpen
		})
	}
}

// TestEngineResumeNotRunningRedispatches covers fix #3: Resume on a not-running
// loop that is queued/paused(interrupted) starts a REAL goroutine and runs it to
// completion (no fabricated state).
func TestEngineResumeNotRunningRedispatches(t *testing.T) {
	repo := initRepo(t)
	runner := &ScriptedRunner{
		Edits: map[string]string{"resume.txt": "resumed\n"},
		Lines: fixtureLines("sess-resume", "resume.txt", "ok", 0.05),
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	// Persist a paused/interrupted loop with no goroutine (as Recover would leave it).
	rec := domain.Workspace{
		ID:          "LOOP-PAUSED",
		ProjectID:   projID,
		Group:       domain.GroupNeeds,
		Goal:        "resume me",
		State:       domain.StatePaused,
		StatusLabel: "Paused",
		Phase:       "interrupted",
	}
	if _, err := st.AddWorkspace(rec); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}

	eng.Resume(rec.ID)

	// A real goroutine should now drive the loop to a terminal pr_open.
	waitFor(t, 5*time.Second, "resumed loop to reach pr_open", func() bool {
		l, ok := st.Workspace(rec.ID)
		return ok && l.State == domain.StatePROpen
	})
	l, _ := st.Workspace(rec.ID)
	if l.WorktreePath == "" {
		t.Error("resumed loop should have a worktree (created on re-dispatch)")
	}
	if l.Diff.Add == 0 {
		t.Errorf("resumed loop should have committed a diff, got %+v", l.Diff)
	}
}

// TestEnginePauseNotRunningNoop covers fix #3: Pause on a not-running loop must NOT
// fabricate a paused state (it is a no-op).
func TestEnginePauseNotRunningNoop(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, repo)

	rec := domain.Workspace{
		ID: "LOOP-QUEUED", ProjectID: projID, Goal: "queued",
		State: domain.StateQueued, Group: domain.GroupActive, Phase: "waiting for a slot",
	}
	if _, err := st.AddWorkspace(rec); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}

	eng.Pause(rec.ID)

	l, _ := st.Workspace(rec.ID)
	if l.State != domain.StateQueued {
		t.Errorf("Pause on a not-running queued loop must be a no-op; got state=%q", l.State)
	}
}

// TestEngineStopDoesNotDowngradeTerminal covers fix #7: a late Stop on a loop that
// already reached a terminal state must not overwrite it with "stopped".
func TestEngineStopDoesNotDowngradeTerminal(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, repo)

	for _, term := range []domain.WorkspaceState{
		domain.StatePROpen, domain.StateMerged, domain.StateFailed, domain.StateStopped,
	} {
		id := "LOOP-TERM-" + string(term)
		rec := domain.Workspace{ID: id, ProjectID: projID, Goal: "done", State: term}
		if _, err := st.AddWorkspace(rec); err != nil {
			t.Fatalf("AddWorkspace %q: %v", id, err)
		}
		eng.Stop(id) // not running

		l, _ := st.Workspace(id)
		if l.State != term {
			t.Errorf("late Stop downgraded terminal %q to %q", term, l.State)
		}
	}

	// A non-terminal not-running loop (queued) DOES transition to stopped.
	rec := domain.Workspace{ID: "LOOP-QUEUED-STOP", ProjectID: projID, Goal: "q", State: domain.StateQueued}
	if _, err := st.AddWorkspace(rec); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	eng.Stop(rec.ID)
	l, _ := st.Workspace(rec.ID)
	if l.State != domain.StateStopped {
		t.Errorf("Stop on a queued loop should mark it stopped, got %q", l.State)
	}
}

// TestEngineShutdownJoins covers fix #5: Shutdown cancels running loops AND blocks
// until their goroutines have exited (not just fire-and-forget). We use a runner
// that holds until ctx cancel; after Shutdown returns, no loop goroutine remains.
func TestEngineShutdownJoins(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, &ctxRunner{})
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "long running", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 3*time.Second, "agent running", func() bool {
		return eng.Governor().AgentsRunning() == 1
	})

	done := make(chan struct{})
	go func() { eng.Shutdown(); close(done) }()
	select {
	case <-done:
	case <-time.After(6 * time.Second):
		t.Fatal("Shutdown did not return within the bounded deadline")
	}

	// After a joined Shutdown the goroutine is gone: the slot is released and the
	// loop is no longer in the running map.
	if eng.Governor().LoopsRunning() != 0 {
		t.Errorf("loop slot not released after Shutdown join: %d running", eng.Governor().LoopsRunning())
	}
	if _, running := eng.loops.Load(id); running {
		t.Error("loop still in running map after Shutdown join")
	}
}

// countingRunner records, per loop id, how many times Run was invoked, so a test
// can detect a SINGLE loop being dispatched onto two goroutines (double-dispatch).
// It applies a tiny edit so the engine commit has something to commit, then returns
// success. Honors ctx cancellation between phases like the production parser would.
type countingRunner struct {
	mu     sync.Mutex
	counts map[string]int // workspaceID -> number of Run invocations
	gate   chan struct{}  // if non-nil, Run blocks on it (or ctx) before returning
}

func newCountingRunner() *countingRunner {
	return &countingRunner{counts: map[string]int{}}
}

func (r *countingRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	// Count CODER turns only as the per-loop dispatch marker: a single dispatch
	// produces exactly one Coder turn (then a Reviewer turn that files no findings →
	// the loop converges in iteration 1). A DOUBLE-dispatch would run the whole
	// sequence — including the Coder — twice, which is the symptom these tests catch.
	// The Reviewer turn (RoleReviewer) is intentionally NOT counted so adding the M3
	// review pass doesn't inflate the dispatch count. The Reviewer files no findings,
	// so each counting-runner loop converges on its first iteration.
	gate := r.gate
	if spec.Role == domain.RoleCoder {
		r.mu.Lock()
		r.counts[spec.WorkspaceID]++
		r.mu.Unlock()

		// A small edit so the engine commit lands a real diff (the worktree is shared
		// per loop id, so two goroutines on the same loop would race here — the symptom).
		sr := &ScriptedRunner{Edits: map[string]string{"work.txt": "done by " + spec.WorkspaceID + "\n"}}
		if err := sr.applyEdits(spec.Worktree); err != nil {
			return Result{}, err
		}
	}
	if emit != nil {
		emit(ParsedEvent{Kind: KindStep, Tool: "Write", ToolInput: "work.txt"})
	}
	if gate != nil && spec.Role == domain.RoleCoder {
		select {
		case <-gate:
		case <-ctx.Done():
			return Result{}, ctx.Err()
		}
	}
	return Result{Subtype: "success", SessionID: spec.AgentID}, nil
}

func (r *countingRunner) maxCount() (id string, n int) {
	r.mu.Lock()
	defer r.mu.Unlock()
	for k, v := range r.counts {
		if v > n {
			id, n = k, v
		}
	}
	return id, n
}

// windowWidener returns a promoteHook that holds the dispatchMu-protected promotion
// critical section open for d, and records the MAX number of goroutines observed
// inside it at once. With the fix, dispatchMu serializes the section so that max is
// 1; without the fix, concurrent drains pile in and the max exceeds 1 — which is the
// double-dispatch window this test depends on.
func windowWidener(d time.Duration) (hook func(), maxConcurrent func() int) {
	var mu sync.Mutex
	var cur, max int
	hook = func() {
		mu.Lock()
		cur++
		if cur > max {
			max = cur
		}
		mu.Unlock()

		time.Sleep(d) // hold the window open so other drains can collide here

		mu.Lock()
		cur--
		mu.Unlock()
	}
	maxConcurrent = func() int {
		mu.Lock()
		defer mu.Unlock()
		return max
	}
	return hook, maxConcurrent
}

// TestEngineConcurrentDrainNoDoubleDispatch is the NEW-1 regression test. It forces
// the double-dispatch window deterministically: a pool of QUEUED loops, then many
// goroutines fire drainQueued at once, each of which — after selecting the oldest
// queued loop and confirming it is not already running — enters a test hook
// (promoteHook) that holds the promotion critical section open for a fixed delay.
//
// Without the fix, drainQueued has no serialization, so several drains sit in that
// window simultaneously, all having selected the SAME oldest queued loop, all then
// acquire a slot and dispatch it → that loop's goroutine runs MORE THAN ONCE
// (count>1), two `git worktree add` race the same path, and ReleaseLoop is called
// twice. With the fix, dispatchMu holds across the section so only ONE drain is ever
// in the window (maxConcurrent==1), each queued loop is dispatched EXACTLY once, and
// the slot accounting stays balanced. Run under -race.
func TestEngineConcurrentDrainNoDoubleDispatch(t *testing.T) {
	repo := initRepo(t)
	// A loop cap large enough that several concurrent drains can ALL acquire a slot
	// at once — the bug is about colliding on the same target, not exceeding the cap.
	const maxLoops = 8
	gate := make(chan struct{})
	runner := newCountingRunner()
	runner.gate = gate // dispatched loops BLOCK in Run so the oldest stays selectable
	eng, st, _, _ := newTestEngine(t, runner)
	eng.gov = NewGovernor(maxLoops, 64)

	hook, maxConcurrent := windowWidener(40 * time.Millisecond)
	eng.promoteHook = hook

	projID := addProject(t, st, repo)

	// Seed a pool of QUEUED loops directly (no goroutine yet) — the backed-up queue.
	const queued = 12
	ids := make([]string, queued)
	for i := 0; i < queued; i++ {
		ids[i] = newQueuedLoop(t, st, projID, i)
	}

	// Fire many drainQueued goroutines released simultaneously by a barrier.
	const hammer = 48
	barrier := make(chan struct{})
	var wg sync.WaitGroup
	wg.Add(hammer)
	for i := 0; i < hammer; i++ {
		go func() {
			defer wg.Done()
			<-barrier
			eng.drainQueued()
		}()
	}
	close(barrier) // release them all at once
	wg.Wait()

	// The first wave dispatched up to maxLoops loops (they are blocked on the gate,
	// holding slots). Release the gate so they finish and the loopExited cascade
	// drains the rest of the queue.
	waitFor(t, 5*time.Second, "first wave of loops to occupy slots", func() bool {
		return eng.Governor().LoopsRunning() >= 1
	})
	close(gate)

	// All queued loops must eventually run to pr_open (the cascade empties the queue).
	for _, id := range ids {
		id := id
		waitFor(t, 20*time.Second, "queued loop "+id+" to finish", func() bool {
			l, ok := st.Workspace(id)
			return ok && l.State == domain.StatePROpen
		})
	}

	// The test actually drove concurrency into the promotion window: with the fix the
	// dispatchMu serializes it so AT MOST one drain is ever inside at a time. Without
	// the fix this is the hammer count (48) — this assertion is the deterministic
	// proof the guard holds.
	if mc := maxConcurrent(); mc != 1 {
		t.Errorf("promotion critical section was not serialized: max concurrent = %d, want 1", mc)
	}

	// CORE ASSERTION: no loop was ever dispatched twice (count==1 per loop). This is
	// the symptom the bug produces in the window where two dispatches both win the
	// `git worktree add` race.
	if id, n := runner.maxCount(); n > 1 {
		t.Fatalf("double-dispatch detected: loop %s ran %d times (want exactly 1 per loop)", id, n)
	}
	// Every queued loop ran exactly once.
	for _, id := range ids {
		runner.mu.Lock()
		n := runner.counts[id]
		runner.mu.Unlock()
		if n != 1 {
			t.Errorf("loop %s dispatched %d times, want exactly 1", id, n)
		}
	}

	// Slot accounting must be balanced: no leak, no negative.
	waitFor(t, 5*time.Second, "all loop slots released", func() bool {
		return eng.Governor().LoopsRunning() == 0
	})
	if got := eng.Governor().LoopsRunning(); got != 0 {
		t.Errorf("governor LoopsRunning after drain storm: got %d, want 0", got)
	}
	// No leftover entries in the running map.
	eng.loops.Range(func(k, _ any) bool {
		t.Errorf("running map not empty after drain: leftover loop %v", k)
		return true
	})
}

// TestEngineConcurrentResumeSingleDispatch covers the NEW-1 window for two
// concurrent Resume(sameId) calls: a single not-running paused loop, Resumed from
// many goroutines at once, must be promoted to a goroutine EXACTLY once (no
// double-dispatch, no double slot acquire).
func TestEngineConcurrentResumeSingleDispatch(t *testing.T) {
	repo := initRepo(t)
	runner := newCountingRunner()
	eng, st, _, _ := newTestEngine(t, runner)

	// Widen the re-dispatch window so the concurrent Resume calls deterministically
	// collide in it (the same dispatchMu section drainQueued uses).
	hook, maxConcurrent := windowWidener(40 * time.Millisecond)
	eng.promoteHook = hook

	projID := addProject(t, st, repo)

	rec := domain.Workspace{
		ID:          "LOOP-RESUME-RACE",
		ProjectID:   projID,
		Group:       domain.GroupNeeds,
		Goal:        "resume me concurrently",
		State:       domain.StatePaused,
		StatusLabel: "Paused",
		Phase:       "interrupted",
	}
	if _, err := st.AddWorkspace(rec); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}

	const callers = 32
	barrier := make(chan struct{})
	var wg sync.WaitGroup
	wg.Add(callers)
	for i := 0; i < callers; i++ {
		go func() {
			defer wg.Done()
			<-barrier
			eng.Resume(rec.ID)
		}()
	}
	close(barrier) // release all Resume callers at once
	wg.Wait()

	// dispatchMu must have serialized the re-dispatch: only one caller in the window.
	if mc := maxConcurrent(); mc != 1 {
		t.Errorf("Resume re-dispatch not serialized: max concurrent = %d, want 1", mc)
	}

	waitFor(t, 5*time.Second, "resumed loop to finish", func() bool {
		l, ok := st.Workspace(rec.ID)
		return ok && l.State == domain.StatePROpen
	})

	// Exactly one dispatch despite 32 concurrent Resume calls.
	runner.mu.Lock()
	n := runner.counts[rec.ID]
	runner.mu.Unlock()
	if n != 1 {
		t.Fatalf("concurrent Resume(sameId) dispatched the loop %d times, want exactly 1", n)
	}

	waitFor(t, 5*time.Second, "loop slot released after resume", func() bool {
		return eng.Governor().LoopsRunning() == 0
	})
}

// TestEngineResumeDoesNotResurrectStopped covers NEW-2: a Resume racing a Stop on a
// not-running loop must not resurrect a loop a concurrent Stop just stopped. Both
// commands take dispatchMu for the not-running case, so the outcome is consistent:
// either it runs to pr_open (Resume won) or it is stopped (Stop won) — never a
// stopped loop with a live goroutine. Run many times under -race.
func TestEngineResumeDoesNotResurrectStopped(t *testing.T) {
	repo := initRepo(t)

	for iter := 0; iter < 20; iter++ {
		runner := newCountingRunner()
		eng, st, _, _ := newTestEngine(t, runner)
		projID := addProject(t, st, repo)

		rec := domain.Workspace{
			ID: "LOOP-STOP-RESUME", ProjectID: projID, Goal: "race",
			State: domain.StatePaused, Group: domain.GroupNeeds, Phase: "interrupted",
		}
		if _, err := st.AddWorkspace(rec); err != nil {
			t.Fatalf("AddWorkspace: %v", err)
		}

		var wg sync.WaitGroup
		wg.Add(2)
		go func() { defer wg.Done(); eng.Stop(rec.ID) }()
		go func() { defer wg.Done(); eng.Resume(rec.ID) }()
		wg.Wait()

		// Let any dispatched goroutine settle. The race has several legitimate
		// terminal outcomes depending on who wins and when the cancel lands relative
		// to the goroutine's phases: pr_open (Resume won, ran to completion), stopped
		// (Stop won outright, or Stop cancelled the running loop), or failed (cancel
		// landed mid-commit). The invariant we assert is NOT the exact outcome but
		// that there is no double-dispatch and no slot leak / resurrected goroutine.
		waitFor(t, 8*time.Second, "loop settles after stop/resume race", func() bool {
			l, ok := st.Workspace(rec.ID)
			if !ok {
				return false
			}
			return terminal(l.State)
		})

		// Whatever the outcome, the loop must not be dispatched more than once and the
		// slot must end balanced (a stopped loop must not be left holding a slot).
		_, maxN := runner.maxCount()
		if maxN > 1 {
			t.Fatalf("iter %d: stop/resume race double-dispatched (count=%d)", iter, maxN)
		}
		waitFor(t, 5*time.Second, "slot released after stop/resume race", func() bool {
			return eng.Governor().LoopsRunning() == 0
		})

		// If Stop won, the loop is stopped and there must be NO live goroutine for it.
		l, _ := st.Workspace(rec.ID)
		if l.State == domain.StateStopped {
			if _, running := eng.loops.Load(rec.ID); running {
				t.Fatalf("iter %d: stopped loop still has a live goroutine (resurrected)", iter)
			}
		}
	}
}

func TestDrainQueuedPromotesNextAfterClosedOwner(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	eng.gov = NewGovernor(1, 64)
	projID := addProject(t, st, repo)

	idB := newQueuedLoop(t, st, projID, 0)
	idC := newQueuedLoop(t, st, projID, 1)
	st.UpdateWorkspace(idB, func(w *domain.Workspace) {
		w.RunStarted = true
		w.LoopChatCode = pairAgentCode
	})
	eng.ensurePairAgent(idB, pairAgentCode, "")

	eng.promoteHook = func() {
		_ = eng.CloseWorkspaceChat(idB, pairAgentCode)
	}
	eng.drainQueued()

	if rec, ok := st.Workspace(idB); !ok || rec.State == domain.StateQueued {
		t.Fatalf("closed owner B state = %+v, want not queued", rec)
	}
	waitFor(t, 10*time.Second, "C to leave the queue after B closed during drain", func() bool {
		l, ok := st.Workspace(idC)
		return ok && l.State != domain.StateQueued
	})
}

// newQueuedLoop persists a fresh queued loop record for the drain tests.
func newQueuedLoop(t *testing.T, st *store.Store, projID string, i int) string {
	t.Helper()
	id := "LOOP-Q-" + string(rune('A'+i%26)) + "-" + strconv.Itoa(i)
	rec := domain.Workspace{
		ID:          id,
		ProjectID:   projID,
		Group:       domain.GroupActive,
		Goal:        "queued work " + strconv.Itoa(i),
		State:       domain.StateQueued,
		StatusLabel: "Queued",
		Phase:       "waiting for a slot",
	}
	if _, err := st.AddWorkspace(rec); err != nil {
		t.Fatalf("AddWorkspace queued %d: %v", i, err)
	}
	return id
}
