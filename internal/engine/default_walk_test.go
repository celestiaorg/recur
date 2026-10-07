package engine

import (
	"testing"
	"time"

	"recur/internal/bundles"
	"recur/internal/domain"
)

// TestEngageWithoutPickWalksDefaultGraph: a run started with no explicit
// loop/policy (the dock's untouched defaults) must still walk the shipped
// To a PR graph — never a hidden pre-bundle path — and the resolved ids
// must land on the row so the dock and the record agree. The planner runs
// as the shipped Planner role (PL), so it has an agent card and a
// transcript on the cockpit.
func TestEngageWithoutPickWalksDefaultGraph(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	installShippedLoopPack(t)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Default-pick run", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 20*time.Second, "default-pick run to open a PR", func() bool {
		l, ok := st.Workspace(id)
		return ok && (l.State == domain.StatePROpen || l.State == domain.StateFailed || l.State == domain.StateStopped)
	})
	ws, _ := st.Workspace(id)
	if ws.State != domain.StatePROpen {
		t.Fatalf("state = %s (%s), want pr_open via the graph walk", ws.State, ws.Phase)
	}
	if ws.LoopID != bundles.DefaultLoopID {
		t.Errorf("row LoopID = %q, want the resolved default %q", ws.LoopID, bundles.DefaultLoopID)
	}
	if ws.PolicyID == "" {
		t.Error("row PolicyID should be stamped with the resolved default policy")
	}
	codes := map[string]bool{}
	for _, a := range st.Agents(id) {
		codes[a.Code] = true
	}
	if !codes["PL"] {
		t.Errorf("planner should run as the shipped Planner role (PL) so it is visible, agents=%v", codes)
	}
}

// TestUnresolvableGuardIdlesWithReason: a loop id that does not exist cannot
// fall back to anything — the run idles and says why.
func TestUnresolvableGuardIdlesWithReason(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, repo)

	rec, acquired, err := eng.queueStartWorkspace(projID, "bad loop", "", "", "", "", "", nil)
	if err != nil || !acquired {
		t.Fatalf("queueStartWorkspace: %v acquired=%v", err, acquired)
	}
	rec.LoopID = "nobody.loops.missing"
	if err := eng.dispatch(rec); err != nil {
		t.Fatalf("dispatch: %v", err)
	}
	waitFor(t, 10*time.Second, "unresolvable loop to idle", func() bool {
		l, ok := st.Workspace(rec.ID)
		return ok && l.State == domain.StatePaused && l.Phase == "loop refs missing"
	})
}
