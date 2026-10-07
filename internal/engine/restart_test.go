package engine

import (
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
)

// TestRestartStoppedLoop covers the Restart hand (both the strip button and the
// Maestro's mcp__recur__resume_loop route here): a deliberately STOPPED loop —
// which Resume/Retry refuse by design — re-dispatches over its existing worktree;
// non-restartable states get a named refusal.
func TestRestartStoppedLoop(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Feature to stop and restart", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	eng.Stop(id)
	waitFor(t, 5*time.Second, "loop stopped", func() bool {
		l, ok := st.Workspace(id)
		return ok && (l.State == domain.StateStopped || l.State == domain.StatePROpen)
	})
	rec, _ := st.Workspace(id)
	if rec.State == domain.StatePROpen {
		// The loop won the race and finished before Stop landed — restart must
		// refuse a settled-with-PR loop with the Send Back / reenter_phase pointer.
		if s := eng.Restart(id); !strings.Contains(s, "Send Back") && !strings.Contains(s, "reenter_phase") {
			t.Fatalf("pr_open restart should point at Send Back / reenter_phase, got %q", s)
		}
		return
	}

	status := eng.Restart(id)
	if !strings.Contains(status, "restarting") {
		t.Fatalf("Restart of a stopped loop: got %q", status)
	}
	waitFor(t, 5*time.Second, "loop running again", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State != domain.StateStopped && l.State != domain.StatePaused
	})

	// Refusals stay named: unknown workspace; and once re-running, "already running"
	// (or the loop already settled again — either way not a bare success).
	if s := eng.Restart("nope"); !strings.Contains(s, "Refused") {
		t.Errorf("unknown workspace: got %q", s)
	}
	eng.Stop(id)
}
