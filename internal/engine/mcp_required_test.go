package engine

import (
	"context"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"recur/internal/domain"
)

// deadMCPAddr is an address net.Listen always refuses, so ensureMCP fails the
// way a real bind failure would.
const deadMCPAddr = "127.0.0.1:-1"

// realisticRunner behaves the way a REAL claude agent does when it has no MCP
// endpoint: it does NOT error, it simply cannot call report_finding, so it
// finishes its review having filed nothing and reports success. That difference
// matters — convergeRunner POSTs unconditionally and so fails loudly on an empty
// URL, which would hide the very bug this file is about.
type realisticRunner struct {
	mu    sync.Mutex
	turns int
}

func (r *realisticRunner) ran() int {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.turns
}

func (r *realisticRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	r.mu.Lock()
	r.turns++
	r.mu.Unlock()

	switch spec.Role {
	case domain.RoleCoder:
		sr := &ScriptedRunner{Edits: map[string]string{"work.txt": "edit\n"}}
		if err := sr.applyEdits(spec.Worktree); err != nil {
			return Result{}, err
		}
		if spec.MCPURL != "" {
			if err := closeOpenFindingsMCP(spec.MCPURL); err != nil {
				return Result{}, err
			}
		}
	case domain.RoleReviewer:
		// With an endpoint this review is damning; without one it is silent.
		if spec.MCPURL != "" {
			if err := postFinding(spec.MCPURL, "work.txt", 1, "high", "must not ship"); err != nil {
				return Result{}, err
			}
		}
	}
	return Result{Subtype: "success", SessionID: spec.AgentID}, nil
}

// TestDeadMCPFailsTheLoopInsteadOfConverging is the regression that motivated
// making a missing MCP fatal. Convergence is defined as "the Reviewer filed zero
// findings" — and report_finding is an MCP tool. An agent with no MCP endpoint
// therefore files nothing, which is indistinguishable from a clean review: the
// loop sails to pr_open on its first iteration and opens a PR on code that was
// never reviewed. Failing loudly is the only safe reading of "no MCP".
func TestDeadMCPFailsTheLoopInsteadOfConverging(t *testing.T) {
	repo := initRepo(t)
	// This runner files a blocking finding if it is ever given an endpoint, and
	// stays quiet if it isn't. It must never get the chance to do either.
	runner := &realisticRunner{}
	eng, st, _, _ := newTestEngine(t, runner)
	eng.MCP().Addr = deadMCPAddr
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Build the feature", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}

	// Wait for ANY terminal state, then assert which one, so a regression reports
	// what the loop actually did rather than just timing out.
	waitFor(t, 10*time.Second, "loop to reach a terminal state", func() bool {
		l, ok := st.Workspace(id)
		return ok && (l.State == domain.StateFailed ||
			l.State == domain.StatePROpen || l.State == domain.StateStopped)
	})

	l, _ := st.Workspace(id)
	if l.State == domain.StatePROpen {
		t.Fatal("a loop with no MCP opened a PR: the Reviewer could not file findings, " +
			"so zero findings was read as convergence and unreviewed code shipped")
	}
	if l.State != domain.StateFailed {
		t.Fatalf("loop ended in %q, want failed", l.State)
	}

	// The failure has to say what went wrong, or this is just a different kind
	// of silence.
	if !strings.Contains(strings.ToLower(l.LastError), "mcp") {
		t.Errorf("failure reason %q does not mention the MCP", l.LastError)
	}
	// A bind failure clears, and ensureMCP retries it — so this must land as
	// retryable ("needs you" + a Retry that can succeed), not a dead end.
	if !l.Retryable {
		t.Error("MCP failure is not marked retryable, so Retry is unavailable")
	}
	if l.Group != domain.GroupNeeds {
		t.Errorf("loop is in group %q, want %q so the human sees it", l.Group, domain.GroupNeeds)
	}

	// No turn should have burned tokens: the check happens before the agent runs.
	if n := runner.ran(); n != 0 {
		t.Errorf("%d agent turn(s) ran without an MCP endpoint, want 0", n)
	}
}

// TestEnsureMCPRetriesAfterFailure guards the sync.Once trap: caching the first
// failure would mean a transient bind error bricks the MCP for the whole
// process, so the Retry button the user reaches for could never work.
func TestEnsureMCPRetriesAfterFailure(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, &ScriptedRunner{})
	eng.MCP().Addr = deadMCPAddr

	if err := eng.ensureMCP(); err == nil {
		t.Fatal("ensureMCP succeeded on an unbindable address")
	}

	eng.MCP().Addr = "" // the condition clears
	if err := eng.ensureMCP(); err != nil {
		t.Fatalf("ensureMCP did not retry after a failure: %v", err)
	}
	if eng.MCP().Port() == 0 {
		t.Error("MCP reports no bound port after a successful retry")
	}

	// Still idempotent once started: a second call must not rebind.
	port := eng.MCP().Port()
	if err := eng.ensureMCP(); err != nil {
		t.Fatalf("ensureMCP on an already-started server: %v", err)
	}
	if eng.MCP().Port() != port {
		t.Errorf("ensureMCP rebound the server: port %d → %d", port, eng.MCP().Port())
	}
}

// TestDeadMCPFailsChatTurn covers the chat path, where the damage is quieter but
// the same shape: a Plan-mode turn without submit_plan produces a confident
// reply about a plan that was never filed.
func TestDeadMCPFailsChatTurn(t *testing.T) {
	repo := initRepo(t)
	var ran int32
	runner := &ScriptedRunner{OnRun: func(AgentSpec) {
		atomic.AddInt32(&ran, 1)
	}}
	eng, st, _, _ := newTestEngine(t, runner)
	eng.MCP().Addr = deadMCPAddr
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "chat only", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}

	reply := eng.SendWorkspaceMessage(id, "CH", "make me a plan", "", nil, nil, "plan", "")
	if !strings.Contains(strings.ToLower(reply), "mcp") {
		t.Errorf("chat reply %q hides the MCP failure", reply)
	}
	if n := atomic.LoadInt32(&ran); n != 0 {
		t.Errorf("the turn ran %d time(s) text-only instead of refusing", n)
	}
}
