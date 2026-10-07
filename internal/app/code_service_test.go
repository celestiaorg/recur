package app

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"recur/internal/domain"
	"recur/internal/engine"
	gitpkg "recur/internal/git"
	"recur/internal/store"
)

// TestListChangedFilesLiveWorktree pins the Observatory realtime-refresh fix:
// ListChangedFiles must reflect the loop's worktree AS IT SITS ON DISK right
// now — including a coding agent's uncommitted edits and brand-new untracked
// files — not just what made it into a commit or PR. Before this, the only
// source was the last PR snapshot, which doesn't exist until the loop
// converges, so the "Changed in loop" list (and the file tree/open-file
// live-refresh that key off it) stayed frozen for the entire time an agent
// was actively editing.
func TestListChangedFilesLiveWorktree(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	root := t.TempDir()
	mgr := gitpkg.NewManager(repo, root)
	wt, _, err := mgr.Add(context.Background(), "loop-live", "live-refresh", "main")
	if err != nil {
		t.Fatalf("Add worktree: %v", err)
	}

	if _, err := eng.Store.AddWorkspace(domain.Workspace{
		ID:           "loop-live",
		ProjectID:    projectID,
		WorktreePath: wt.Path,
		BaseBranch:   "main",
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}

	if got := svc.Code.ListChangedFiles(t.Context(), "loop-live"); len(got) != 0 {
		t.Fatalf("expected no changed files on a fresh worktree, got %v", got)
	}

	// A coding agent edits a tracked file and creates a new one — neither is
	// committed. Both must show up immediately.
	if err := os.WriteFile(filepath.Join(wt.Path, "README.md"), []byte("hi\nedited\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(wt.Path, "new.go"), []byte("package main\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	got := svc.Code.ListChangedFiles(t.Context(), "loop-live")
	want := map[string]bool{"README.md": true, "new.go": true}
	if len(got) != len(want) {
		t.Fatalf("ListChangedFiles = %v, want %v", got, want)
	}
	for _, p := range got {
		if !want[p] {
			t.Errorf("unexpected path %q in %v", p, got)
		}
	}
}

// TestReadFileAndListTreeReportNotFound asserts a missing path (the agent
// deleted it out from under an open tab / expanded tree dir) comes back as a
// stable "not found: <path>" error, which is what the frontend's live-refresh
// polling sniffs for to distinguish "genuinely gone" from a transient I/O
// blip (see Observatory.svelte's isNotFound).
func TestReadFileAndListTreeReportNotFound(t *testing.T) {
	repo := initGitRepo(t)
	svc, _ := newTestServices(t)
	projectID := svcAddProject(t, svc.Projects.eng, repo)

	_, err := svc.Code.ReadFile(projectID, "", "does-not-exist.txt")
	if err == nil {
		t.Fatal("expected an error reading a missing file")
	}
	if err.Error() != "not found: does-not-exist.txt" {
		t.Errorf("ReadFile error = %q, want stable not-found form", err.Error())
	}

	_, err = svc.Code.ListTree(t.Context(), projectID, "", "no/such/dir")
	if err == nil {
		t.Fatal("expected an error listing a missing dir")
	}
	if err.Error() != "not found: no/such/dir" {
		t.Errorf("ListTree error = %q, want stable not-found form", err.Error())
	}
}

func commitIn(t *testing.T, dir, msg string) {
	t.Helper()
	for _, args := range [][]string{{"add", "-A"}, {"commit", "-m", msg}} {
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
}

func addWorktreeWorkspace(t *testing.T, svc *Services, repo, id, branch string) (projectID, path string) {
	t.Helper()
	eng := svc.Code.eng
	projectID = svcAddProject(t, eng, repo)
	root := t.TempDir()
	mgr := gitpkg.NewManager(repo, root)
	wt, _, err := mgr.Add(context.Background(), id, branch, "main")
	if err != nil {
		t.Fatalf("Add worktree: %v", err)
	}
	if _, err := eng.Store.AddWorkspace(domain.Workspace{
		ID:           id,
		ProjectID:    projectID,
		WorktreePath: wt.Path,
		BaseBranch:   "main",
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	return projectID, wt.Path
}

func TestDiffVsBaseAndCommits(t *testing.T) {
	repo := initGitRepo(t)
	svc, _ := newTestServices(t)
	projectID, wt := addWorktreeWorkspace(t, svc, repo, "ws-review", "review")

	if err := os.WriteFile(filepath.Join(wt, "feat.txt"), []byte("one\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	commitIn(t, wt, "add feat")
	if err := os.WriteFile(filepath.Join(wt, "clean.txt"), []byte("ok\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	commitIn(t, wt, "add clean")
	if err := os.WriteFile(filepath.Join(wt, "feat.txt"), []byte("one\ntwo\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(wt, "new.txt"), []byte("untracked\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	files, err := svc.Code.DiffVsBase(t.Context(), projectID, "ws-review")
	if err != nil {
		t.Fatalf("DiffVsBase: %v", err)
	}
	byPath := map[string]bool{}
	for _, f := range files.Files {
		byPath[f.Path] = true
	}
	if !byPath["feat.txt"] || !byPath["new.txt"] || !byPath["clean.txt"] {
		t.Errorf("DiffVsBase = %+v, want feat.txt + new.txt + clean.txt", byPath)
	}

	working, err := svc.Code.DiffWorking(t.Context(), projectID, "ws-review")
	if err != nil {
		t.Fatalf("DiffWorking: %v", err)
	}
	workPath := map[string]bool{}
	for _, f := range working.Files {
		workPath[f.Path] = true
	}
	if !workPath["feat.txt"] || !workPath["new.txt"] {
		t.Errorf("DiffWorking = %+v, want feat.txt + new.txt", workPath)
	}
	if workPath["clean.txt"] {
		t.Errorf("DiffWorking includes clean.txt; that file is committed and clean vs HEAD")
	}

	commits, err := svc.Code.ListCommits(t.Context(), projectID, "ws-review")
	if err != nil {
		t.Fatalf("ListCommits: %v", err)
	}
	if len(commits) != 2 {
		t.Fatalf("ListCommits len = %d, want 2", len(commits))
	}
	var featHash string
	for _, c := range commits {
		if c.Message == "add feat" {
			featHash = c.Hash
		}
	}
	if featHash == "" {
		t.Fatalf("ListCommits messages = %v, want add feat", commits)
	}

	diff, err := svc.Code.CommitDiff(t.Context(), projectID, "ws-review", featHash)
	if err != nil {
		t.Fatalf("CommitDiff: %v", err)
	}
	found := false
	for _, f := range diff {
		if f.Path == "feat.txt" {
			found = true
		}
	}
	if !found {
		t.Errorf("CommitDiff files = %+v, want feat.txt", diff)
	}
}

func TestListCommitsMainWorkingCopy(t *testing.T) {
	repo := initGitRepo(t)
	svc, _ := newTestServices(t)
	eng := svc.Code.eng
	projectID := svcAddProject(t, eng, repo)
	if _, err := eng.Store.AddWorkspace(domain.Workspace{
		ID:           "main-wc",
		ProjectID:    projectID,
		WorktreePath: repo,
		IsMain:       true,
		BaseBranch:   "",
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	commits, err := svc.Code.ListCommits(t.Context(), projectID, "main-wc")
	if err != nil {
		t.Fatalf("ListCommits: %v", err)
	}
	if len(commits) != 1 {
		t.Fatalf("ListCommits on main working copy len = %d, want 1", len(commits))
	}
	if commits[0].Message != "initial" {
		t.Errorf("ListCommits[0].Message = %q, want initial", commits[0].Message)
	}
}

// TestDiffWorkingUnbornHEAD: git init with no commits still has WorkingStatus
// files; DiffWorking must not fail on `git diff HEAD` (exit 128) and should
// show staged + untracked paths as additions versus the empty tree.
func TestDiffWorkingUnbornHEAD(t *testing.T) {
	dir := t.TempDir()
	init := exec.Command("git", "init", "-b", "main")
	init.Dir = dir
	if out, err := init.CombinedOutput(); err != nil {
		t.Fatalf("git init: %v\n%s", err, out)
	}
	if err := os.WriteFile(filepath.Join(dir, "staged.txt"), []byte("in index\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	add := exec.Command("git", "add", "staged.txt")
	add.Dir = dir
	if out, err := add.CombinedOutput(); err != nil {
		t.Fatalf("git add: %v\n%s", err, out)
	}
	if err := os.WriteFile(filepath.Join(dir, "loose.txt"), []byte("untracked\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	svc, _ := newTestServices(t)
	eng := svc.Code.eng
	projectID := svcAddProject(t, eng, dir)
	if _, err := eng.Store.AddWorkspace(domain.Workspace{
		ID:           "unborn",
		ProjectID:    projectID,
		WorktreePath: dir,
		IsMain:       true,
		BaseBranch:   "",
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}

	got, err := svc.Code.DiffWorking(t.Context(), projectID, "unborn")
	if err != nil {
		t.Fatalf("DiffWorking: %v", err)
	}
	byPath := map[string]bool{}
	for _, f := range got.Files {
		byPath[f.Path] = true
	}
	if !byPath["staged.txt"] || !byPath["loose.txt"] {
		t.Fatalf("DiffWorking unborn = %+v, want staged.txt + loose.txt", byPath)
	}
}
