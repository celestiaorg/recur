package app

import (
	"testing"

	"recur/internal/domain"
	"recur/internal/engine"
	"recur/internal/store"
)

func TestSetWorkspaceIssueMovesLinkAndClears(t *testing.T) {
	t.Setenv("LINEAR_API_KEY", "")
	eng := engine.New(store.NewInMemory())
	for _, id := range []string{"A", "B"} {
		if _, err := eng.Store.AddWorkspace(domain.Workspace{ID: id, ProjectID: "p"}); err != nil {
			t.Fatal(err)
		}
	}
	svc := &WorkspacesService{base: base{eng: eng}}
	issue := domain.IssueRef{Source: "linear", ID: "uuid-157", Key: "BOTFUN-157"}

	if _, err := svc.SetWorkspaceIssue("A", issue); err != nil {
		t.Fatal(err)
	}
	if _, err := svc.SetWorkspaceIssue("B", issue); err != nil {
		t.Fatal(err)
	}
	if a, _ := eng.Store.Workspace("A"); a.Issue != nil {
		t.Fatalf("A still linked: %+v", a.Issue)
	}
	if b, _ := eng.Store.Workspace("B"); b.Issue == nil || b.Issue.ID != issue.ID {
		t.Fatalf("B not linked: %+v", b.Issue)
	}

	if _, err := svc.ClearWorkspaceIssue("B"); err != nil {
		t.Fatal(err)
	}
	if b, _ := eng.Store.Workspace("B"); b.Issue != nil {
		t.Fatalf("B still linked after clear: %+v", b.Issue)
	}
	if _, err := svc.ClearWorkspaceIssue("missing"); err == nil {
		t.Fatal("expected error for unknown workspace")
	}
}
