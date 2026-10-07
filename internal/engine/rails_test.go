package engine

import (
	"testing"

	"recur/internal/domain"
)

// TestCanOpenPRPolicyBar: 0 findings open; high blocks Balanced; resolve opens;
// leftover low is allowed under Balanced (medium+ bar).
func TestCanOpenPRPolicyBar(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})

	const workspaceID = "L-rail3"

	if ok, reason := eng.CanOpenPR(workspaceID); !ok {
		t.Errorf("CanOpenPR with 0 findings: got false (%q), want true", reason)
	}

	st.AddFinding(domain.Finding{ID: "f1", WorkspaceID: workspaceID, Iteration: 1, File: "a.go", Severity: domain.SevHigh, Message: "bug", Status: "open"})
	ok, reason := eng.CanOpenPR(workspaceID)
	if ok {
		t.Errorf("CanOpenPR with 1 high finding: got true, want false")
	}
	if reason == "" {
		t.Error("CanOpenPR false must carry a human reason")
	}

	st.ResolveFindings(workspaceID)
	if ok, _ := eng.CanOpenPR(workspaceID); !ok {
		t.Error("CanOpenPR after resolving findings: got false, want true")
	}

	st.AddFinding(domain.Finding{ID: "f2", WorkspaceID: workspaceID, Iteration: 2, File: "b.go", Severity: domain.SevLow, Message: "nit", Status: "open"})
	if ok, reason := eng.CanOpenPR(workspaceID); !ok {
		t.Errorf("CanOpenPR with leftover low under Balanced: got false (%q), want true", reason)
	}
}
