package app

import (
	"runtime"
	"testing"

	"recur/internal/domain"
	"recur/internal/engine"
	"recur/internal/store"
)

func TestOpenWorkspaceIn(t *testing.T) {
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	repo := initGitRepo(t)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "open-in", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if ws.WorktreePath == "" {
		t.Fatal("expected a worktree path")
	}

	targets := svc.Workspaces.ListOpenInTargets()
	if runtime.GOOS == "darwin" {
		if len(targets) == 0 || targets[0].ID != "finder" {
			t.Fatalf("ListOpenInTargets() = %+v, want finder first", targets)
		}
	}

	if err := svc.Workspaces.OpenWorkspaceIn("nope", "finder"); err == nil {
		t.Fatal("missing workspace should fail")
	}
	if _, err := eng.Store.AddWorkspace(domain.Workspace{ID: "empty-wt", ProjectID: projectID}); err != nil {
		t.Fatal(err)
	}
	if err := svc.Workspaces.OpenWorkspaceIn("empty-wt", "finder"); err == nil {
		t.Fatal("empty worktree should fail")
	}
	if err := svc.Workspaces.OpenWorkspaceIn(ws.ID, "nope"); err == nil {
		t.Fatal("unknown target should fail")
	}
}
