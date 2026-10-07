package engine

import (
	"testing"
	"time"

	"recur/internal/domain"
)

// inboxByID returns the inbox row with the given id, if present.
func inboxByID(eng *Engine, id string) (domain.InboxItem, bool) {
	for _, it := range eng.Store.Inbox() {
		if it.ID == id {
			return it, true
		}
	}
	return domain.InboxItem{}, false
}

// TestInboxDerivedFromRealPR is the M8-C test: a real converged PR (not seeded)
// produces an actionable "ready for you" inbox row derived from real state, and
// MERGING it clears the row. Progress stays in the loop feed; only the actionable
// PR-ready item earns an inbox row (PRD §21.5 / D5).
func TestInboxDerivedFromRealPR(t *testing.T) {
	repo := initRepo(t)
	// Converge on the first review iteration (0 findings) → straight to pr_open.
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner) // rail #1 (verify) disabled by the helper
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Add OAuth provider config", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 5*time.Second, "loop to reach pr_open", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})

	// The engine opened a real PR linked to the loop.
	pr, ok := st.PRByWorkspace(id)
	if !ok {
		t.Fatal("expected a PR for the converged loop")
	}

	// A real, actionable "ready for you" inbox row was derived (id keyed to the PR).
	row, ok := inboxByID(eng, "ib-pr-"+pr.ID)
	if !ok {
		t.Fatalf("expected an actionable inbox row ib-pr-%s for the ready PR", pr.ID)
	}
	if row.Group != "ready" || row.Action != "Review" || row.Target != "pr:"+pr.ID {
		t.Errorf("inbox row shape wrong: %+v", row)
	}
	if !row.Unread {
		t.Error("a freshly-ready PR inbox row should be unread")
	}

	// Merging the PR clears the actionable row (the human acted — no longer "needs me").
	if _, ok := eng.MergePR(pr.ID, ""); !ok {
		t.Fatal("MergePR should succeed")
	}
	if _, stillThere := inboxByID(eng, "ib-pr-"+pr.ID); stillThere {
		t.Error("the PR-ready inbox row should be cleared after merge")
	}
}

// TestStatusBarItersCountReal is the M8-F test: the machine-global "iters today"
// counter reflects REAL completed review iterations (it is the seeded baseline plus
// the governor's real iteration count), updated live via resource.updated.
func TestStatusBarItersCountReal(t *testing.T) {
	repo := initRepo(t)
	// Two review iterations: 1 finding, then 0 (converges on iteration 2).
	runner := &convergeRunner{findingsPerIter: []int{1, 0}}
	eng, st, sink, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	baseIters := eng.itersBase // captured at construction from the seeded resource

	id, err := eng.StartWorkspace(projID, "two-iteration task", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 6*time.Second, "loop to reach pr_open", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})

	// Two review iterations completed → the governor counted two iters.
	if got := eng.Governor().Iters(); got != 2 {
		t.Errorf("governor iters: got %d, want 2", got)
	}
	// The status-bar resource reflects baseline + real iters, and was published live.
	if got := st.Resource().ItersToday; got != baseIters+2 {
		t.Errorf("resource ItersToday: got %d, want %d (base %d + 2)", got, baseIters+2, baseIters)
	}
	if !sink.has("resource.updated") {
		t.Error("expected resource.updated events for the live status-bar counter")
	}
}

// TestSendBackClearsThenReAddsInbox: a ready PR has an inbox row; Send Back
// clears it (the human already acted). The row is not re-added until a later
// ensure_pr.
func TestSendBackClearsThenReAddsInbox(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0, 0}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "send-back task", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 5*time.Second, "loop to reach pr_open", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})
	pr, ok := st.PRByWorkspace(id)
	if !ok {
		t.Fatal("expected a PR")
	}
	if _, ok := inboxByID(eng, "ib-pr-"+pr.ID); !ok {
		t.Fatal("expected a ready inbox row before send-back")
	}

	if _, ok := eng.SendBackToLoop(pr.ID, []string{"please rename the config key"}); !ok {
		t.Fatal("SendBackToLoop should succeed")
	}
	if _, ok := inboxByID(eng, "ib-pr-"+pr.ID); ok {
		t.Error("SendBackToLoop() should clear the ready inbox row")
	}
	if p, _ := st.PR(pr.ID); p.State != "In review" {
		t.Errorf("SendBackToLoop() state = %q, want In review", p.State)
	}
}
