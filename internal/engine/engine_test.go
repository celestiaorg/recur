package engine

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"recur/internal/cursor"
	"recur/internal/domain"
	"recur/internal/store"
)

// initRepo creates a git repo with one commit on branch "main" and returns its path.
func initRepo(t testing.TB) string {
	t.Helper()
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	dir := t.TempDir()
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = dir
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-b", "main")
	run("config", "user.name", "test")
	run("config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(dir, "README.md"), []byte("hello\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "initial")
	return dir
}

// recordingSink captures every event the bus publishes, thread-safely.
type recordingSink struct {
	mu     sync.Mutex
	events []Event
}

func (r *recordingSink) sink(ev Event) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.events = append(r.events, ev)
}

func (r *recordingSink) byName(name string) []Event {
	r.mu.Lock()
	defer r.mu.Unlock()
	var out []Event
	for _, e := range r.events {
		if e.Name == name {
			out = append(out, e)
		}
	}
	return out
}

func (r *recordingSink) has(name string) bool { return len(r.byName(name)) > 0 }

func addSoloWorkspace(t *testing.T, st *store.Store, id string) {
	t.Helper()
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: id, ProjectID: "p1", Type: domain.WorkspaceSolo, Short: "hand",
	}); err != nil {
		t.Fatal(err)
	}
}

// newTestEngine wires a SQLite-backed store under a temp data dir, an engine with
// a ScriptedRunner, and a recording sink. The repo's worktrees live under the same
// temp data dir (engine reads $RECUR_DATA_DIR via store.DataDir).
func newTestEngine(t *testing.T, runner AgentRunner) (*Engine, *store.Store, *recordingSink, string) {
	t.Helper()
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)

	st, err := store.New()
	if err != nil {
		t.Fatalf("store.New: %v", err)
	}
	t.Cleanup(func() { st.Close() })

	eng := New(st)
	eng.SetRunner(runner)
	oldSyncIV := githubPRSyncMinInterval
	githubPRSyncMinInterval = 0
	t.Cleanup(func() { githubPRSyncMinInterval = oldSyncIV })
	oldGH := ghCLIAvailableFn
	ghCLIAvailableFn = func() bool { return true }
	t.Cleanup(func() { ghCLIAvailableFn = oldGH })
	// Point the worktree root explicitly at the temp dir (defensive; New already
	// derives it from $RECUR_DATA_DIR).
	wtRoot := filepath.Join(dataDir, "worktrees")
	_ = os.MkdirAll(wtRoot, 0o755)
	eng.SetWorktreeRoot(wtRoot)

	// Join any loop goroutines before the test's temp dirs are removed. t.Cleanup is
	// LIFO and t.TempDir registered its removal first, so this runs before both the
	// store close and the directory removal. Without it a loop still running at test
	// end kept writing git objects into the repo/worktree while TempDir was deleting
	// them, which surfaced intermittently as "TempDir RemoveAll cleanup: directory not
	// empty" in whichever test happened to lose the race.
	t.Cleanup(eng.Shutdown)

	sink := &recordingSink{}
	eng.Bus.Subscribe(sink.sink)
	return eng, st, sink, dataDir
}

// installTestPack writes a bundle.toml under the test data Bundles dir so
// DiscoverCatalog can see it (user ~/.recur/bundles is not used).
func installTestPack(t *testing.T, name, toml string) {
	t.Helper()
	dir, err := store.BundlesDir()
	if err != nil {
		t.Fatalf("BundlesDir: %v", err)
	}
	pack := filepath.Join(dir, name)
	if err := os.MkdirAll(pack, 0o755); err != nil {
		t.Fatalf("mkdir pack: %v", err)
	}
	if err := os.WriteFile(filepath.Join(pack, "bundle.toml"), []byte(toml), 0o644); err != nil {
		t.Fatalf("write bundle.toml: %v", err)
	}
}

func setDefaultPolicy(t *testing.T, st *store.Store, id string) {
	t.Helper()
	set := st.Settings()
	set.DefaultPolicy = id
	st.SetSettings(set)
}

// addProject registers a project pointing at repo and returns its id.
func addProject(t *testing.T, st *store.Store, repo string) string {
	t.Helper()
	p := domain.Project{ID: "proj", Name: "proj", RepoPath: repo, BaseBranch: "main"}
	if _, err := st.AddProject(p); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	return p.ID
}

// waitFor polls until cond() is true or the deadline passes.
func waitFor(t *testing.T, d time.Duration, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(d)
	for time.Now().Before(deadline) {
		if cond() {
			return
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatalf("timed out waiting for %s", what)
}

// TestEngineEndToEndScripted is the M2 end-to-end test: a loop runs via a
// ScriptedRunner against a real git repo, transitions states, emits events, the
// ENGINE commit lands a real diff in the worktree, events persist, and a reopen
// reconstructs the feed. No real claude.
func TestEngineEndToEndScripted(t *testing.T) {
	repo := initRepo(t)

	runner := &ScriptedRunner{
		Edits: map[string]string{"feature.txt": "added by the coder\n"},
		Lines: fixtureLines("sess-e2e", "feature.txt", "Implemented the feature.", 0.25),
	}

	eng, st, sink, dataDir := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Add a feature file", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	if id == "" {
		t.Fatal("StartWorkspace returned empty id")
	}

	// The loop should run to a terminal pr_open (ready) state and exit.
	waitFor(t, 5*time.Second, "loop to reach pr_open", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})

	l, _ := st.Workspace(id)
	if l.Group != domain.GroupReady {
		t.Errorf("loop group: got %q, want ready", l.Group)
	}
	if l.Diff.Add == 0 {
		t.Errorf("loop diff additions should be > 0 after commit, got %+v", l.Diff)
	}

	// The engine commit must have landed a real commit in the worktree.
	wt := l.WorktreePath
	if wt == "" {
		t.Fatal("loop has no worktree path")
	}
	logOut, err := gitOut(repo, wt, "log", "--oneline")
	if err != nil {
		t.Fatalf("git log in worktree: %v", err)
	}
	if !containsLine(logOut, "Add a feature file") {
		t.Errorf("engine commit not found in worktree log:\n%s", logOut)
	}
	author, err := gitOut(repo, wt, "log", "-1", "--format=%an <%ae>")
	if err != nil {
		t.Fatalf("git log author: %v", err)
	}
	if got, want := strings.TrimSpace(author), "test <test@example.com>"; got != want {
		t.Errorf("engine commit author = %q, want %q", got, want)
	}
	// The scripted edit file is committed.
	if _, err := os.Stat(filepath.Join(wt, "feature.txt")); err != nil {
		t.Errorf("scripted edit file missing from worktree: %v", err)
	}

	// Events were emitted: loop.updated, loop.event, agent.step, resource.updated.
	for _, name := range []string{"workspace.updated", "workspace.event", "agent.step", "resource.updated"} {
		if !sink.has(name) {
			t.Errorf("expected at least one %q event", name)
		}
	}

	// State transitions are visible across loop.updated events.
	var states []string
	for _, ev := range sink.byName("workspace.updated") {
		m := ev.Data.(map[string]any)
		states = append(states, string(m["state"].(domain.WorkspaceState)))
	}
	if !sawSequence(states, domain.StatePlanning, domain.StateCoding, domain.StatePROpen) {
		t.Errorf("did not observe planning→coding→pr_open in: %v", states)
	}

	// Durable events persisted: reconstruct the feed from the events table.
	evs, err := st.EventsForLoop(id)
	if err != nil {
		t.Fatalf("EventsForLoop: %v", err)
	}
	if len(evs) == 0 {
		t.Fatal("no durable events persisted")
	}
	feed := eng.ReconstructFeed(id)
	if len(feed) == 0 {
		t.Fatal("ReconstructFeed returned nothing despite persisted events")
	}

	// Reopen the store from disk and confirm history survives (PRD §6.5).
	st.Close()
	st2, err := store.New() // same $RECUR_DATA_DIR
	if err != nil {
		t.Fatalf("reopen store: %v", err)
	}
	defer st2.Close()
	_ = dataDir

	l2, ok := st2.Workspace(id)
	if !ok {
		t.Fatal("loop missing after reopen")
	}
	if l2.State != domain.StatePROpen {
		t.Errorf("reopened loop state: got %q, want pr_open", l2.State)
	}
	eng2 := New(st2)
	eng2.SetRunner(&ScriptedRunner{})
	reFeed := eng2.ReconstructFeed(id)
	if len(reFeed) == 0 {
		t.Error("feed not reconstructable from disk after reopen")
	}
}

func isolateEngineGitConfig(t *testing.T) {
	t.Helper()
	home := t.TempDir()
	empty := filepath.Join(home, "empty.gitconfig")
	if err := os.WriteFile(empty, nil, 0o644); err != nil {
		t.Fatal(err)
	}
	t.Setenv("HOME", home)
	t.Setenv("XDG_CONFIG_HOME", home)
	t.Setenv("GIT_CONFIG_GLOBAL", empty)
	t.Setenv("GIT_CONFIG_SYSTEM", empty)
	t.Setenv("GIT_CONFIG_NOSYSTEM", "1")
	for _, k := range []string{
		"GIT_AUTHOR_NAME", "GIT_AUTHOR_EMAIL",
		"GIT_COMMITTER_NAME", "GIT_COMMITTER_EMAIL",
		"EMAIL",
	} {
		t.Setenv(k, "")
		os.Unsetenv(k)
	}
}

func TestEngineCommitRequiresGitIdentity(t *testing.T) {
	repo := initRepo(t)
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = repo
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("config", "user.useConfigOnly", "true")
	run("config", "--unset", "user.name")
	run("config", "--unset", "user.email")
	isolateEngineGitConfig(t)

	runner := &ScriptedRunner{
		Edits: map[string]string{"feature.txt": "added by the coder\n"},
		Lines: fixtureLines("sess-noid", "feature.txt", "Implemented the feature.", 0.25),
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Add a feature file", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}

	waitFor(t, 5*time.Second, "loop to fail without git identity", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateFailed
	})
	l, _ := st.Workspace(id)
	if !strings.Contains(l.LastError, "engine commit identity") {
		t.Errorf("LastError = %q, want engine commit identity", l.LastError)
	}
	logOut, lerr := gitOut(repo, repo, "log", "--oneline")
	if lerr != nil {
		t.Fatalf("git log: %v\n%s", lerr, logOut)
	}
	if containsLine(logOut, "Add a feature file") {
		t.Errorf("engine committed despite missing identity:\n%s", logOut)
	}
}

// TestEngineGovernorQueuesThirdLoop asserts a 3rd loop queues when maxLoops=2.
func TestEngineGovernorQueuesThirdLoop(t *testing.T) {
	repo := initRepo(t)

	// A runner that blocks until released, so the first two loops stay "running"
	// and hold their loop-slots while we start a third.
	gate := make(chan struct{})
	runner := &blockingRunner{gate: gate}

	eng, st, _, _ := newTestEngine(t, runner)
	// Force the governor to max 2 loops.
	set := st.Settings()
	set.MaxLoops = 2
	st.SetSettings(set)
	eng.gov = NewGovernor(2, 8)

	projID := addProject(t, st, repo)

	id1, err := eng.StartWorkspace(projID, "task one", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace 1: %v", err)
	}
	id2, err := eng.StartWorkspace(projID, "task two", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace 2: %v", err)
	}

	// Wait until both loops are actually running (have acquired their agent and are
	// blocked in the runner) so their loop-slots are held.
	waitFor(t, 3*time.Second, "two loops running", func() bool {
		return eng.Governor().LoopsRunning() == 2
	})

	// The third must be persisted as queued (no slot).
	id3, err := eng.StartWorkspace(projID, "task three", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace 3: %v", err)
	}
	l3, ok := st.Workspace(id3)
	if !ok || l3.State != domain.StateQueued {
		t.Errorf("third loop should be queued, got state=%q ok=%v", l3.State, ok)
	}

	// Release the blocked runners so loops 1 and 2 can finish; then shut down.
	close(gate)
	waitFor(t, 5*time.Second, "loops 1 and 2 to finish", func() bool {
		return eng.Governor().LoopsRunning() == 0
	})
	_ = id1
	_ = id2
}

// TestApplyGovernorCapsDrainsQueuedLoops: raising MaxLoops must start queued
// work immediately (multiple queued loops if the new cap allows), without a restart.
func TestApplyGovernorCapsDrainsQueuedLoops(t *testing.T) {
	repo := initRepo(t)
	gate := make(chan struct{})
	runner := &blockingRunner{gate: gate}

	eng, st, _, _ := newTestEngine(t, runner)
	eng.gov = NewGovernor(1, 8)
	projID := addProject(t, st, repo)

	if _, err := eng.StartWorkspace(projID, "loop A", "", ""); err != nil {
		t.Fatalf("StartWorkspace A: %v", err)
	}
	waitFor(t, 3*time.Second, "loop A running", func() bool {
		return eng.Governor().LoopsRunning() == 1
	})

	idB, err := eng.StartWorkspace(projID, "loop B", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace B: %v", err)
	}
	idC, err := eng.StartWorkspace(projID, "loop C", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace C: %v", err)
	}
	for _, id := range []string{idB, idC} {
		l, ok := st.Workspace(id)
		if !ok || l.State != domain.StateQueued {
			t.Fatalf("loop %s should be queued, got state=%q ok=%v", id, l.State, ok)
		}
	}

	eng.ApplyGovernorCaps(3, 8)
	waitFor(t, 3*time.Second, "queued loops to drain after raising cap", func() bool {
		return eng.Governor().LoopsRunning() == 3
	})

	close(gate)
	waitFor(t, 5*time.Second, "all loops to finish", func() bool {
		return eng.Governor().LoopsRunning() == 0
	})
}

func TestEngineNewTreatsZeroCapsAsUnlimited(t *testing.T) {
	st := store.NewInMemory()
	set := st.Settings()
	set.MaxLoops = 0
	set.MaxAgents = 0
	st.SetSettings(set)

	eng := New(st)
	for i := 0; i < domain.DefaultMaxLoops+1; i++ {
		if !eng.Governor().TryAcquireLoop() {
			t.Fatalf("loop slot %d should be free when MaxLoops=0 (unlimited)", i)
		}
	}
}

// TestEnginePauseResumeStop exercises the control channel: pause blocks the loop
// at a checkpoint, resume continues it, and stop cancels promptly.
func TestEnginePauseResumeStop(t *testing.T) {
	repo := initRepo(t)

	// Stop test: a runner that blocks until ctx is cancelled, so Stop must unblock
	// it by cancelling the loop context (which kills the agent).
	t.Run("stop cancels promptly", func(t *testing.T) {
		blockTillCancel := &ctxRunner{}
		eng, st, sink, _ := newTestEngine(t, blockTillCancel)
		projID := addProject(t, st, repo)

		id, err := eng.StartWorkspace(projID, "long task", "", "")
		if err != nil {
			t.Fatalf("StartWorkspace: %v", err)
		}
		waitFor(t, 3*time.Second, "agent running", func() bool {
			return eng.Governor().AgentsRunning() == 1
		})

		start := time.Now()
		eng.Stop(id)

		waitFor(t, 3*time.Second, "loop to stop", func() bool {
			l, ok := st.Workspace(id)
			return ok && l.State == domain.StateStopped
		})
		if elapsed := time.Since(start); elapsed > 2*time.Second {
			t.Errorf("stop took too long: %v", elapsed)
		}
		_ = sink
		// Agent slot released after stop.
		waitFor(t, 2*time.Second, "agent slot released", func() bool {
			return eng.Governor().AgentsRunning() == 0
		})
	})

	t.Run("pause then resume completes", func(t *testing.T) {
		// A runner that signals when it has started and waits on a gate, so we can
		// pause while it's mid-run, then resume.
		gate := make(chan struct{})
		runner := &blockingRunner{gate: gate, edits: map[string]string{"p.txt": "x\n"}}
		eng, st, _, _ := newTestEngine(t, runner)
		projID := addProject(t, st, repo)

		id, err := eng.StartWorkspace(projID, "pausable", "", "")
		if err != nil {
			t.Fatalf("StartWorkspace: %v", err)
		}
		waitFor(t, 3*time.Second, "agent running", func() bool {
			return eng.Governor().AgentsRunning() == 1
		})

		// Pause is delivered; it takes effect at the next checkpoint (after coding).
		eng.Pause(id)
		// Let the agent finish its (blocked) work.
		close(gate)

		// The loop should reach paused (at the post-coding checkpoint) — it must NOT
		// race straight to pr_open while paused.
		waitFor(t, 4*time.Second, "loop paused", func() bool {
			l, ok := st.Workspace(id)
			return ok && l.State == domain.StatePaused
		})

		// Resume → loop proceeds to terminal pr_open.
		eng.Resume(id)
		waitFor(t, 4*time.Second, "loop resumes to pr_open", func() bool {
			l, ok := st.Workspace(id)
			return ok && l.State == domain.StatePROpen
		})
	})
}

// TestRealClaudeSmoke is an OPT-IN end-to-end smoke against the real ClaudeRunner
// (the production default). It is SKIPPED unless RECUR_E2E=1 so the default test
// run never invokes claude (no API budget, no auth assumptions). When enabled it
// runs a trivial Coder turn in a temp repo and asserts the engine commit landed.
func TestRealClaudeSmoke(t *testing.T) {
	if os.Getenv("RECUR_E2E") != "1" {
		t.Skip("set RECUR_E2E=1 to run the real claude smoke test")
	}
	if _, err := exec.LookPath("claude"); err != nil {
		t.Skip("claude not on PATH")
	}
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, NewClaudeRunner()) // production runner
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Create a file named HELLO.txt containing the word hi", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 5*time.Minute, "real loop to finish", func() bool {
		l, ok := st.Workspace(id)
		return ok && (l.State == domain.StatePROpen || l.State == domain.StateFailed)
	})
	l, _ := st.Workspace(id)
	if l.State != domain.StatePROpen {
		t.Fatalf("real loop did not reach pr_open: state=%q", l.State)
	}
	out, err := gitOut(repo, l.WorktreePath, "log", "--oneline")
	if err != nil || !containsLine(out, "HELLO.txt") {
		t.Errorf("engine commit not found after real run:\n%s (err=%v)", out, err)
	}
}

// ---- test runners ----

// blockingRunner applies optional edits then blocks until gate is closed (or ctx
// is cancelled), then returns a success result. Lets a test hold loop/agent slots.
type blockingRunner struct {
	gate  chan struct{}
	edits map[string]string
}

func (r *blockingRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if len(r.edits) > 0 {
		sr := &ScriptedRunner{Edits: r.edits}
		if err := sr.applyEdits(spec.Worktree); err != nil {
			return Result{}, err
		}
	}
	if emit != nil {
		emit(ParsedEvent{Kind: KindStep, Tool: "Bash", ToolInput: "working"})
	}
	select {
	case <-r.gate:
	case <-ctx.Done():
		return Result{}, ctx.Err()
	}
	return Result{Subtype: "success", SessionID: spec.AgentID}, nil
}

// ctxRunner blocks until ctx is cancelled, simulating a long-running agent that
// Stop must terminate via context cancellation.
type ctxRunner struct{}

func (r *ctxRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if emit != nil {
		emit(ParsedEvent{Kind: KindStep, Tool: "Bash", ToolInput: "long op"})
	}
	<-ctx.Done()
	return Result{}, ctx.Err()
}

// durationCancelRunner is the Codex cancel contract: return wall-clock DurationMS
// so Stop can stamp one footer, but do not emit KindResult (that would close the
// mirrored turn before finalizeChatReply classifies Stop vs park).
type durationCancelRunner struct{}

func (r *durationCancelRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if emit != nil {
		emit(ParsedEvent{Kind: KindText, Text: "working on it"})
	}
	<-ctx.Done()
	return Result{DurationMS: 1500, SessionID: "thread-1"}, ctx.Err()
}

// durationErrorRunner is the Codex failure contract: emit KindResult with
// usage and DurationMS 0 (persist tokens without session.turn_end), and
// return DurationMS so the synthesized Agent-error note owns the turn footer.
type durationErrorRunner struct{}

func (r *durationErrorRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if emit != nil {
		emit(ParsedEvent{Kind: KindText, Text: "partial…"})
	}
	res := Result{
		DurationMS:      1500,
		IsError:         true,
		SessionID:       "thread-1",
		InputTokens:     200,
		OutputTokens:    50,
		CacheReadTokens: 15000,
		ContextWindow:   200000,
	}
	if emit != nil {
		usage := res
		usage.DurationMS = 0
		emit(ParsedEvent{Kind: KindResult, Result: &usage, Text: res.Text, IsError: true})
	}
	return res, fmt.Errorf("codex app-server exited before turn completed")
}

// sequentialChatRunner succeeds once with a duration-bearing KindResult, then
// waits on ctx so a Retry/Stop can cancel before any new events.
type sequentialChatRunner struct {
	n int
}

func (r *sequentialChatRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	r.n++
	if r.n == 1 {
		res := Result{DurationMS: 800, SessionID: "thread-1"}
		if emit != nil {
			emit(ParsedEvent{Kind: KindText, Text: "first reply"})
			emit(ParsedEvent{Kind: KindResult, Result: &res, Text: res.Text})
		}
		return res, nil
	}
	<-ctx.Done()
	return Result{DurationMS: 400, SessionID: "thread-1"}, ctx.Err()
}

// kindResultErrorRunner models Claude/Cursor: KindResult already stamped the
// turn duration before Run returns an error. The returned Result is a copy
// distinct from the emit payload (Claude copies before emit; Cursor rebuilds
// via result()), so TurnEndEmitted must come from the mirroring wrapper, not
// pointer mutation. The error note must not add a second footer.
type kindResultErrorRunner struct{}

func (r *kindResultErrorRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if emit != nil {
		emit(ParsedEvent{Kind: KindText, Text: "partial…"})
	}
	res := Result{DurationMS: 1500, IsError: true, SessionID: "thread-1"}
	if emit != nil {
		emitted := res
		emit(ParsedEvent{Kind: KindResult, Result: &emitted, Text: res.Text, IsError: true})
	}
	return res, fmt.Errorf("cursor run: boom")
}

// ---- small helpers ----

func gitOut(repo, worktree string, args ...string) (string, error) {
	cmd := exec.Command("git", append([]string{"-C", worktree}, args...)...)
	out, err := cmd.CombinedOutput()
	return string(out), err
}

func containsLine(s, sub string) bool {
	for _, line := range splitLines(s) {
		if indexOf(line, sub) >= 0 {
			return true
		}
	}
	return false
}

func splitLines(s string) []string {
	var out []string
	cur := ""
	for _, r := range s {
		if r == '\n' {
			out = append(out, cur)
			cur = ""
			continue
		}
		cur += string(r)
	}
	if cur != "" {
		out = append(out, cur)
	}
	return out
}

// TestStartWorkspaceStampsAutonomy asserts each loop keeps its own autonomy: an
// explicit override wins, empty falls back to the policy default, and later
// dial changes do not rewrite loops that already started.
func TestStartWorkspaceStampsAutonomy(t *testing.T) {
	repo := initRepo(t)
	runner := &ScriptedRunner{
		Edits: map[string]string{"feature.txt": "ok\n"},
		Lines: fixtureLines("sess-autonomy", "feature.txt", "done", 0.01),
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	idDefault, err := eng.StartWorkspace(projID, "policy default", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace default: %v", err)
	}
	l, _ := st.Workspace(idDefault)
	if l.Autonomy != domain.AutonomyBalanced {
		t.Fatalf("empty autonomy: got %q, want balanced (policy default)", l.Autonomy)
	}

	idCautious, err := eng.StartWorkspace(projID, "cautious override", "", domain.AutonomyCautious)
	if err != nil {
		t.Fatalf("StartWorkspace cautious: %v", err)
	}
	l, _ = st.Workspace(idCautious)
	if l.Autonomy != domain.AutonomyCautious {
		t.Fatalf("override: got %q, want cautious", l.Autonomy)
	}

	p := st.Policy()
	p.Autonomy = domain.AutonomyAggressive
	st.SetPolicy(p)

	idAggressive, err := eng.StartWorkspace(projID, "new policy default", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace after dial move: %v", err)
	}
	l, _ = st.Workspace(idAggressive)
	if l.Autonomy != domain.AutonomyAggressive {
		t.Fatalf("new default: got %q, want aggressive", l.Autonomy)
	}
	l, _ = st.Workspace(idDefault)
	if l.Autonomy != domain.AutonomyBalanced {
		t.Fatalf("earlier loop rewritten after dial move: got %q", l.Autonomy)
	}
	l, _ = st.Workspace(idCautious)
	if l.Autonomy != domain.AutonomyCautious {
		t.Fatalf("override loop rewritten after dial move: got %q", l.Autonomy)
	}
}

func indexOf(s, sub string) int {
	for i := 0; i+len(sub) <= len(s); i++ {
		if s[i:i+len(sub)] == sub {
			return i
		}
	}
	return -1
}

func sawSequence(states []string, want ...domain.WorkspaceState) bool {
	i := 0
	for _, s := range states {
		if i < len(want) && s == string(want[i]) {
			i++
		}
	}
	return i == len(want)
}

func TestCheckGitPublishesNotifyWhenTooOld(t *testing.T) {
	eng, _, sink, _ := newTestEngine(t, nil)
	eng.gitWarning = "Recur needs Git 2.31 or newer (found 2.24)."
	eng.CheckGit()
	evs := sink.byName("notify")
	if len(evs) == 0 {
		t.Fatal("CheckGit() notify count = 0, want 1")
	}
	data, _ := evs[len(evs)-1].Data.(map[string]any)
	if data["body"] != eng.gitWarning {
		t.Errorf("CheckGit() notify body = %v, want %q", data["body"], eng.gitWarning)
	}
	if data["title"] != "Git is too old" {
		t.Errorf("CheckGit() notify title = %v, want %q", data["title"], "Git is too old")
	}
	res := sink.byName("resource.updated")
	if len(res) == 0 {
		t.Fatal("CheckGit() resource.updated count = 0, want ≥1")
	}
	r, ok := res[len(res)-1].Data.(domain.Resource)
	if !ok {
		t.Fatalf("CheckGit() resource.updated data type = %T, want domain.Resource", res[len(res)-1].Data)
	}
	if r.GitWarning != eng.gitWarning {
		t.Errorf("CheckGit() Resource.GitWarning = %q, want %q", r.GitWarning, eng.gitWarning)
	}
}

func TestCheckGitNotifyTitleWhenMissing(t *testing.T) {
	eng, _, sink, _ := newTestEngine(t, nil)
	eng.gitWarningTitle = "Git was not found"
	eng.gitWarning = "Recur needs Git 2.31 or newer on PATH. Git was not found."
	eng.CheckGit()
	evs := sink.byName("notify")
	if len(evs) == 0 {
		t.Fatal("CheckGit() notify count = 0, want 1")
	}
	data, _ := evs[len(evs)-1].Data.(map[string]any)
	if data["title"] != "Git was not found" {
		t.Errorf("CheckGit() notify title = %v, want %q", data["title"], "Git was not found")
	}
}

// fakeCursorBridge installs a stub bridge (launcher + manifest) under a fresh
// RECUR_DATA_DIR so Locate/SDKVersion see sdkVersion.
func fakeCursorBridge(t *testing.T, sdkVersion string) {
	t.Helper()
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)
	root := filepath.Join(dataDir, "cursor-bridge")
	if err := os.MkdirAll(filepath.Join(root, "bin"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "bin", "cursor-sdk-bridge"), []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "manifest.json"), []byte(`{"sdkVersion":"`+sdkVersion+`"}`), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestCheckCursorBridgeNotifiesWhenBehindPin(t *testing.T) {
	eng, _, sink, _ := newTestEngine(t, nil)
	fakeCursorBridge(t, "0.0.1")
	eng.CheckCursorBridge()
	evs := sink.byName("notify")
	if len(evs) == 0 {
		t.Fatal("CheckCursorBridge() notify count = 0, want 1")
	}
	data, _ := evs[len(evs)-1].Data.(map[string]any)
	if data["title"] != "Cursor bridge update available" {
		t.Errorf("CheckCursorBridge() notify title = %v", data["title"])
	}
}

func TestCheckCursorBridgeSilentWhenPinnedOrMissing(t *testing.T) {
	eng, _, sink, _ := newTestEngine(t, nil)
	t.Setenv("RECUR_DATA_DIR", t.TempDir()) // no bridge installed
	eng.CheckCursorBridge()
	fakeCursorBridge(t, cursor.PinnedSDKVersion())
	eng.CheckCursorBridge()
	if sink.has("notify") {
		t.Errorf("CheckCursorBridge() published notify = %+v, want none", sink.byName("notify"))
	}
}

func TestCheckGitSilentWhenCurrent(t *testing.T) {
	eng, _, sink, _ := newTestEngine(t, nil)
	eng.gitWarning = ""
	eng.CheckGit()
	if sink.has("notify") {
		t.Errorf("CheckGit() published notify = %+v, want none", sink.byName("notify"))
	}
}
