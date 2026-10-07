package engine

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	"recur/internal/domain"
	"recur/internal/store"
)

func TestScheduleRefreshAllWorkspaceDiffsSingleFlight(t *testing.T) {
	st := store.NewInMemory()
	eng := New(st)

	eng.ScheduleRefreshAllWorkspaceDiffs()
	eng.ScheduleRefreshAllWorkspaceDiffs()
	eng.ScheduleRefreshAllWorkspaceDiffs()

	// Wait for the background loop to drain (no worktrees → instant).
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		eng.diffRefreshAllMu.Lock()
		busy := eng.diffRefreshAllBusy
		pending := eng.diffRefreshAllPending
		eng.diffRefreshAllMu.Unlock()
		if !busy && !pending {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("refresh-all single-flight did not settle")
}

func TestSyncWorkspaceDiffFromGit(t *testing.T) {
	repo := initRepo(t)
	st := store.NewInMemory()
	eng := New(st)

	p, err := st.AddProject(domain.Project{
		ID: "p-diff", Name: "p-diff", RepoPath: repo, BaseBranch: "main",
	})
	if err != nil {
		t.Fatal(err)
	}

	// Feature branch with a committed file + dirty working tree.
	gitDiffSync(t, repo, "checkout", "-b", "recur/diff-sync")
	if err := os.WriteFile(filepath.Join(repo, "side.txt"), []byte("a\nb\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitDiffSync(t, repo, "add", "side.txt")
	gitDiffSync(t, repo, "commit", "-m", "add side")
	if err := os.WriteFile(filepath.Join(repo, "side.txt"), []byte("a\nb\nc\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	ws, err := st.AddWorkspace(domain.Workspace{
		ID: "LOOP-DIFF", ProjectID: p.ID, Goal: "diff sync", Short: "diff sync",
		Branch: "recur/diff-sync", BaseBranch: "main", WorktreePath: repo,
		State: domain.StateQueued, Group: domain.GroupActive,
	})
	if err != nil {
		t.Fatal(err)
	}
	if ws.Diff.Add != 0 {
		t.Fatalf("fixture Diff should start at 0, got %+v", ws.Diff)
	}

	if !eng.syncWorkspaceDiff(ws.ID, false) {
		t.Fatal("syncWorkspaceDiff returned false")
	}
	got, ok := st.Workspace(ws.ID)
	if !ok {
		t.Fatal("workspace missing")
	}
	if got.Diff.Add < 3 {
		t.Fatalf("expected live numstat ≥ +3 (2 committed + 1 dirty), got %+v", got.Diff)
	}
}

func gitDiffSync(t *testing.T, dir string, args ...string) {
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
