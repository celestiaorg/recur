package engine

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

func TestCreateWorkspaceRunsSetupOnce(t *testing.T) {
	eng, st, _, dataDir := newTestEngine(t, nil)
	repo := initRepo(t)

	marker := filepath.Join(dataDir, "setup.ok")
	proj := addRunProject(t, st, domain.Project{
		ID: "p1", Name: "p1", RepoPath: repo, BaseBranch: "main",
		SetupCmd: "echo hi > '" + marker + "'",
	})

	ws, err := eng.CreateWorkspace(proj.ID, "feature", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	if ws.Port <= 0 {
		t.Fatalf("expected port allocated, got %d", ws.Port)
	}
	if ws.WorktreePath == "" {
		t.Fatal("expected worktree path")
	}
	if _, err := os.Stat(marker); err != nil {
		t.Fatalf("setup did not run: %v", err)
	}

	marker2 := filepath.Join(dataDir, "setup2.ok")
	proj.SetupCmd = "echo hi > '" + marker2 + "'"
	proj.WriteProjectFile = true
	if _, err := st.UpdateProjectRunConfig(proj); err != nil {
		t.Fatal(err)
	}
	ws2, err := eng.CreateWorkspace(proj.ID, "other", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace 2: %v", err)
	}
	if ws2.Port == ws.Port {
		t.Fatalf("expected distinct ports, both %d", ws.Port)
	}
	if _, err := os.Stat(marker2); err != nil {
		t.Fatalf("second setup did not run: %v", err)
	}
}

func TestRemoveRunsTeardown(t *testing.T) {
	eng, st, _, dataDir := newTestEngine(t, nil)
	repo := initRepo(t)

	torn := filepath.Join(dataDir, "torn.down")
	proj := addRunProject(t, st, domain.Project{
		ID: "p1", Name: "p1", RepoPath: repo, BaseBranch: "main",
		TeardownCmd: "echo bye > '" + torn + "'",
	})
	ws, err := eng.CreateWorkspace(proj.ID, "feature", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	if _, err := eng.RemoveWorkspace(ws.ID); err != nil {
		t.Fatalf("RemoveWorkspace: %v", err)
	}
	deadline := time.Now().Add(2 * time.Second)
	for {
		if _, err := os.Stat(torn); err == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("teardown did not run")
		}
		time.Sleep(20 * time.Millisecond)
	}
}

func TestRemoveWorkspaceReturnsBeforeDiskTeardown(t *testing.T) {
	eng, st, _, dataDir := newTestEngine(t, nil)
	repo := initRepo(t)

	gate := filepath.Join(dataDir, "teardown.gate")
	torn := filepath.Join(dataDir, "torn.down")
	proj := addRunProject(t, st, domain.Project{
		ID: "p1", Name: "p1", RepoPath: repo, BaseBranch: "main",
		TeardownCmd: "while [ ! -f '" + gate + "' ]; do sleep 0.01; done && echo bye > '" + torn + "'",
	})
	ws, err := eng.CreateWorkspace(proj.ID, "feature", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	removed, err := eng.RemoveWorkspace(ws.ID)
	if err != nil {
		t.Fatalf("RemoveWorkspace: %v", err)
	}
	t.Cleanup(func() { _ = os.WriteFile(gate, []byte("go\n"), 0o644) })
	if !removed.Archived || removed.WorktreePath != "" {
		t.Fatalf("store should be archived with path cleared before disk teardown, got archived=%v path=%q", removed.Archived, removed.WorktreePath)
	}
	if _, err := os.Stat(ws.WorktreePath); os.IsNotExist(err) {
		t.Fatal("worktree should still exist until teardown is released")
	}
	if _, err := os.Stat(torn); err == nil {
		t.Fatal("teardown_cmd should still be waiting on the gate")
	}

	if err := os.WriteFile(gate, []byte("go\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	waitFor(t, 5*time.Second, "teardown command", func() bool {
		_, err := os.Stat(torn)
		return err == nil
	})
	waitFor(t, 5*time.Second, "worktree removed", func() bool {
		_, err := os.Stat(ws.WorktreePath)
		return os.IsNotExist(err)
	})
}

func TestDeleteRunsTeardown(t *testing.T) {
	eng, st, _, dataDir := newTestEngine(t, nil)
	repo := initRepo(t)

	torn := filepath.Join(dataDir, "deleted.down")
	proj := addRunProject(t, st, domain.Project{
		ID: "p1", Name: "p1", RepoPath: repo, BaseBranch: "main",
		TeardownCmd: "echo bye > '" + torn + "'",
	})
	ws, err := eng.CreateWorkspace(proj.ID, "feature", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	if err := eng.DeleteWorkspace(ws.ID); err != nil {
		t.Fatalf("DeleteWorkspace: %v", err)
	}
	waitFor(t, 5*time.Second, "teardown command", func() bool {
		_, err := os.Stat(torn)
		return err == nil
	})
}

func TestDeleteWorkspaceReturnsBeforeDiskTeardown(t *testing.T) {
	eng, st, sink, dataDir := newTestEngine(t, nil)
	repo := initRepo(t)

	gate := filepath.Join(dataDir, "teardown.gate")
	torn := filepath.Join(dataDir, "torn.down")
	proj := addRunProject(t, st, domain.Project{
		ID: "p1", Name: "p1", RepoPath: repo, BaseBranch: "main",
		TeardownCmd: "while [ ! -f '" + gate + "' ]; do sleep 0.01; done && echo bye > '" + torn + "'",
	})
	ws, err := eng.CreateWorkspace(proj.ID, "feature", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	if err := eng.DeleteWorkspace(ws.ID); err != nil {
		t.Fatalf("DeleteWorkspace: %v", err)
	}
	t.Cleanup(func() { _ = os.WriteFile(gate, []byte("go\n"), 0o644) })
	if _, ok := st.Workspace(ws.ID); ok {
		t.Fatal("store record should be gone before disk teardown")
	}
	if !sink.has("workspace.deleted") {
		t.Fatal("workspace.deleted should fire before disk teardown")
	}
	if _, err := os.Stat(ws.WorktreePath); os.IsNotExist(err) {
		t.Fatal("worktree should still exist until teardown is released")
	}
	if _, err := os.Stat(torn); err == nil {
		t.Fatal("teardown_cmd should still be waiting on the gate")
	}

	if err := os.WriteFile(gate, []byte("go\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	waitFor(t, 5*time.Second, "teardown command", func() bool {
		_, err := os.Stat(torn)
		return err == nil
	})
	waitFor(t, 5*time.Second, "worktree removed", func() bool {
		_, err := os.Stat(ws.WorktreePath)
		return os.IsNotExist(err)
	})
}

func TestCreateWorkspaceFailedSetupDeletesEmptyBranch(t *testing.T) {
	pinKitchenPair(t, "bubbly", "pretzel")
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	proj := addRunProject(t, st, domain.Project{
		ID: "p-setup-fail", Name: "p-setup-fail", RepoPath: repo, BaseBranch: "main",
		SetupCmd: "exit 1",
	})
	if _, err := eng.CreateWorkspace(proj.ID, "fail", "main"); err == nil {
		t.Fatal("CreateWorkspace must fail when setup exits 1")
	}
	mgr := gitpkg.NewManager(repo, t.TempDir())
	if mgr.CommitExists(context.Background(), "recur/bubbly-pretzel") {
		t.Fatal("failed setup left recur/bubbly-pretzel, want the empty branch deleted")
	}
	proj.SetupCmd = ""
	proj.WriteProjectFile = true
	if _, err := st.UpdateProjectRunConfig(proj); err != nil {
		t.Fatal(err)
	}
	ws, err := eng.CreateWorkspace(proj.ID, "retry", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace retry: %v", err)
	}
	if ws.ID != "p-setup-fail.bubbly-pretzel" {
		t.Errorf("CreateWorkspace retry ID = %q, want p-setup-fail.bubbly-pretzel", ws.ID)
	}
	if ws.Branch != "recur/bubbly-pretzel" {
		t.Errorf("CreateWorkspace retry Branch = %q, want recur/bubbly-pretzel", ws.Branch)
	}
}

func TestFailedSetupLastErrorIncludesStderr(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	proj := addRunProject(t, st, domain.Project{
		ID: "p-setup-fail-out", Name: "p-setup-fail-out", RepoPath: repo, BaseBranch: "main",
		SetupCmd: "echo 'missing tool: foo' >&2; exit 23",
	})
	ws, err := eng.BeginCreateWorkspace(proj.ID, "fail", "main")
	if err != nil {
		t.Fatalf("BeginCreateWorkspace: %v", err)
	}
	waitFor(t, 5*time.Second, "create failure", func() bool {
		got, ok := st.Workspace(ws.ID)
		return ok && got.Phase == "failed"
	})
	got, ok := st.Workspace(ws.ID)
	if !ok {
		t.Fatal("workspace missing after failed setup")
	}
	if !strings.Contains(got.LastError, "exit status 23") {
		t.Errorf("LastError = %q, want it to contain exit status 23", got.LastError)
	}
	if !strings.Contains(got.LastError, "missing tool: foo") {
		t.Errorf("LastError = %q, want it to contain stderr text", got.LastError)
	}

	found := false
	for _, it := range st.Feed(ws.ID) {
		if strings.Contains(it.Text, "Setup command failed") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("feed missing Setup command failed item, got %+v", st.Feed(ws.ID))
	}
}

func TestDeleteFailedBeginCreatePrunesEmptyBranch(t *testing.T) {
	pinKitchenPair(t, "bubbly", "pretzel")
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	proj := addRunProject(t, st, domain.Project{
		ID: "p-setup-fail-ui", Name: "p-setup-fail-ui", RepoPath: repo, BaseBranch: "main",
		SetupCmd: "exit 1",
	})
	ws, err := eng.BeginCreateWorkspace(proj.ID, "fail", "main")
	if err != nil {
		t.Fatalf("BeginCreateWorkspace: %v", err)
	}
	waitFor(t, 5*time.Second, "create failure", func() bool {
		got, ok := st.Workspace(ws.ID)
		return ok && got.Phase == "failed"
	})
	got, ok := st.Workspace(ws.ID)
	if !ok || got.Branch == "" {
		t.Fatal("failed create left Branch empty")
	}
	branch := got.Branch
	if err := eng.DeleteWorkspace(ws.ID); err != nil {
		t.Fatalf("DeleteWorkspace: %v", err)
	}
	mgr := gitpkg.NewManager(repo, t.TempDir())
	waitFor(t, 5*time.Second, "empty branch pruned", func() bool {
		return !mgr.CommitExists(context.Background(), branch)
	})
}
