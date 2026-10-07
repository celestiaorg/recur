package engine

import (
	"path/filepath"
	"testing"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
	"recur/internal/store"
)

func TestBaseBranchForReadsStoreAfterRetarget(t *testing.T) {
	eng := New(store.NewInMemory())
	if _, err := eng.Store.AddProject(domain.Project{ID: "p", Name: "p", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws := domain.Workspace{ID: "w1", ProjectID: "p", BaseBranch: "main", Branch: "recur/w1"}
	if _, err := eng.Store.AddWorkspace(ws); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	lp := newLoop(eng, ws)
	if got := baseBranchFor(lp); got != "main" {
		t.Fatalf("baseBranchFor at dispatch = %q, want main", got)
	}
	if _, ok := eng.Store.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
		w.BaseBranch = "develop"
	}); !ok {
		t.Fatal("UpdateWorkspace failed")
	}
	if lp.BaseBranch != "main" {
		t.Fatalf("Loop.BaseBranch mutated to %q; must stay at dispatch value", lp.BaseBranch)
	}
	if got := baseBranchFor(lp); got != "develop" {
		t.Fatalf("baseBranchFor after store retarget = %q, want develop", got)
	}
}

func TestRetargetOpenPRKeepsSnapshotOnGitError(t *testing.T) {
	eng := New(store.NewInMemory())
	repo := initRepo(t)
	if _, err := eng.Store.AddProject(domain.Project{ID: "p", Name: "p", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws := domain.Workspace{
		ID: "w1", ProjectID: "p", BaseBranch: "main", Branch: "recur/w1",
		WorktreePath: filepath.Join(t.TempDir(), "missing-worktree"),
	}
	if _, err := eng.Store.AddWorkspace(ws); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	pr, err := eng.Store.AddPR(domain.PR{
		ID: "pr1", WorkspaceID: ws.ID, Base: "main", State: "In review",
		CreatedAt: "2026-08-01T12:00:00Z", UpdatedAt: "2026-08-01T12:00:00Z",
		Files:     []domain.FileDiff{{Path: "feature.txt", Status: "a"}},
		FileCount: 1,
		Diff:      domain.DiffStat{Add: 3, Del: 0},
		Commits:   []domain.Commit{{Hash: "abc1234", Message: "solo: add feature"}},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	mgr := gitpkg.NewManager(repo, t.TempDir())
	eng.retargetOpenPR(mgr, ws, pr.ID, "develop")

	got, ok := eng.Store.PR(pr.ID)
	if !ok {
		t.Fatal("PR missing")
	}
	if got.Base != "develop" {
		t.Fatalf("PR.Base = %q, want develop", got.Base)
	}
	if len(got.Files) != 1 || got.Files[0].Path != "feature.txt" {
		t.Fatalf("Files wiped: %+v", got.Files)
	}
	if got.Diff.Add != 3 || got.FileCount != 1 {
		t.Fatalf("Diff/FileCount wiped: diff=%+v count=%d", got.Diff, got.FileCount)
	}
	if len(got.Commits) != 1 || got.Commits[0].Hash != "abc1234" {
		t.Fatalf("Commits wiped: %+v", got.Commits)
	}
	if got.UpdatedAt == "2026-08-01T12:00:00Z" {
		t.Fatal("retargetOpenPR did not bump UpdatedAt")
	}
}
