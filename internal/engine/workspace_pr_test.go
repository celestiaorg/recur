package engine

import (
	"testing"

	"recur/internal/domain"
)

func TestDropUnmergedSiblingPRsKeepsMergedAndKeepID(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	local, err := st.AddPR(domain.PR{
		ID: "4", WorkspaceID: "caramel", Number: 4, Source: domain.SourceLocal,
		State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR(local) err=%v", err)
	}
	merged, err := st.AddPR(domain.PR{
		ID: "gh-old", WorkspaceID: "caramel", Number: 100, Source: domain.SourceGitHub,
		State: "Merged", Group: "merged",
	})
	if err != nil {
		t.Fatalf("AddPR(merged) err=%v", err)
	}
	gh, err := st.AddPR(domain.PR{
		ID: "gh-156", WorkspaceID: "caramel", Number: 156, Source: domain.SourceGitHub,
		State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR(gh) err=%v", err)
	}

	eng.dropUnmergedSiblingPRs("caramel", gh.ID)

	if _, ok := st.PR(local.ID); ok {
		t.Errorf("PR(%q) still present, want leftover Local dropped", local.ID)
	}
	if _, ok := st.PR(gh.ID); !ok {
		t.Errorf("PR(%q) missing, want keepID retained", gh.ID)
	}
	if _, ok := st.PR(merged.ID); !ok {
		t.Errorf("PR(%q) missing, want merged sibling retained", merged.ID)
	}
	if !sink.has("pr.removed") {
		t.Error("dropUnmergedSiblingPRs() pr.removed missing, want event for the Local PR")
	}
	removed := sink.byName("pr.removed")
	if len(removed) != 1 {
		t.Fatalf("pr.removed count = %d, want 1", len(removed))
	}
	payload, _ := removed[0].Data.(map[string]any)
	if got, _ := payload["prId"].(string); got != local.ID {
		t.Errorf("pr.removed prId = %q, want %q", got, local.ID)
	}
}
