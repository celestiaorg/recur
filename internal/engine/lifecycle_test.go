package engine

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"recur/internal/cursor"
	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

// runToPROpen drives a loop with a converge-immediately ScriptedRunner (0 findings
// on the first review) to pr_open and returns the settled loop record. Shared setup
// for the Remove/Delete lifecycle tests.
func runToPROpen(t *testing.T, eng *Engine, st interface {
	Workspace(string) (domain.Workspace, bool)
}, id string) domain.Workspace {
	t.Helper()
	waitFor(t, 5*time.Second, "loop to reach pr_open", func() bool {
		l, ok := st.Workspace(id)
		return ok && (l.State == domain.StatePROpen || l.State == domain.StateFailed)
	})
	l, ok := st.Workspace(id)
	if !ok {
		t.Fatal("loop not found")
	}
	if l.State != domain.StatePROpen {
		t.Fatalf("loop did not converge: state=%q", l.State)
	}
	// The terminal state is persisted from INSIDE the loop goroutine, which is then
	// removed from the engine's running map by its exit defer (loopExited). Remove and
	// Delete refuse a loop that is still in that map, so waiting only on the state left
	// a race the callers lost intermittently. Wait for the goroutine to actually exit.
	waitFor(t, 5*time.Second, "loop goroutine to exit", func() bool {
		_, running := eng.loops.Load(id)
		return !running
	})
	return l
}

// TestRemoveWorkspaceRemovesWorktree drives a real loop (real temp git repo, real
// worktrees, ScriptedRunner — no claude) to pr_open, then archives it: the
// worktree directory must be gone from disk, `git worktree list` must no longer
// carry it, and the loop record must be Archived with WorktreePath cleared while
// keeping its history (feed/PRs untouched).
func TestRemoveWorkspaceRemovesWorktree(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Add a converging feature", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	before := runToPROpen(t, eng, st, id)
	if before.WorktreePath == "" {
		t.Fatal("expected a real worktree path before removing")
	}
	if _, err := os.Stat(before.WorktreePath); err != nil {
		t.Fatalf("worktree should exist before remove: %v", err)
	}
	home := t.TempDir()
	t.Setenv("HOME", home)
	if err := cursor.EnsureCursorProjectDirs(before.WorktreePath); err != nil {
		t.Fatalf("EnsureCursorProjectDirs: %v", err)
	}
	cursorRoot := filepath.Join(home, ".cursor", "projects", cursor.CursorProjectSlug(before.WorktreePath))
	if _, err := os.Stat(cursorRoot); err != nil {
		t.Fatalf("cursor project dir should exist before remove: %v", err)
	}

	removed, err := eng.RemoveWorkspace(id)
	if err != nil {
		t.Fatalf("RemoveWorkspace: %v", err)
	}
	if !removed.Archived {
		t.Error("expected Archived=true")
	}
	if removed.WorktreePath != "" {
		t.Errorf("expected WorktreePath cleared, got %q", removed.WorktreePath)
	}
	if removed.Port != 0 {
		t.Errorf("expected Port released, got %d", removed.Port)
	}
	waitFor(t, 5*time.Second, "worktree removed", func() bool {
		_, err := os.Stat(before.WorktreePath)
		return os.IsNotExist(err)
	})
	waitFor(t, 5*time.Second, "cursor project dir removed", func() bool {
		_, err := os.Stat(cursorRoot)
		return os.IsNotExist(err)
	})

	proj, _ := st.Project(projID)
	list, err := gitList(t, proj.RepoPath)
	if err != nil {
		t.Fatalf("git worktree list: %v", err)
	}
	for _, w := range list {
		if w == before.WorktreePath {
			t.Errorf("git worktree list still carries the removed worktree: %v", list)
		}
	}

	// History survives removing.
	if len(st.Feed(id)) == 0 {
		t.Error("removing must not wipe the loop's feed history")
	}
	if _, ok := st.PRByWorkspace(id); !ok {
		t.Error("removing must not remove the loop's PR")
	}
}

// TestRemoveWorkspaceDeletesBranchWhenConfigured asserts DeleteBranchOnRemove
// force-deletes the local workspace branch after the worktree is gone when no
// open PR depends on it; default (off) and open-PR cases leave the branch.
func TestRemoveWorkspaceDeletesBranchWhenConfigured(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Feature with a unique commit", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	before := runToPROpen(t, eng, st, id)
	branch := before.Branch
	if branch == "" {
		t.Fatal("expected a branch before remove")
	}

	// Default off: branch survives.
	if _, err := eng.RemoveWorkspace(id); err != nil {
		t.Fatalf("RemoveWorkspace: %v", err)
	}
	mgr := gitpkg.NewManager(repo, eng.worktreeRoot)
	ctx := context.Background()
	waitFor(t, 5*time.Second, "default remove finished disk teardown", func() bool {
		_, busy := eng.deleting.Load(id)
		return !busy
	})
	if !mgr.CommitExists(ctx, branch) {
		t.Fatalf("default Remove deleted branch %q", branch)
	}

	// Setting on + open PR: branch kept so mergeLocal can still run.
	id2, err := eng.StartWorkspace(projID, "Another feature", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace 2: %v", err)
	}
	before2 := runToPROpen(t, eng, st, id2)
	branch2 := before2.Branch
	if branch2 == "" || branch2 == branch {
		t.Fatalf("expected a distinct branch, got %q (first %q)", branch2, branch)
	}
	proj, _ := st.Project(projID)
	proj.DeleteBranchOnRemove = true
	if _, err := st.UpdateProjectRunConfig(proj); err != nil {
		t.Fatalf("UpdateProjectRunConfig: %v", err)
	}
	if _, err := eng.RemoveWorkspace(id2); err != nil {
		t.Fatalf("RemoveWorkspace with DeleteBranchOnRemove + open PR: %v", err)
	}
	waitFor(t, 5*time.Second, "open-PR remove finished disk teardown", func() bool {
		_, busy := eng.deleting.Load(id2)
		return !busy
	})
	if !mgr.CommitExists(ctx, branch2) {
		t.Fatalf("Remove with open PR deleted branch %q", branch2)
	}

	// Setting on + merged PR: force-delete proceeds.
	id3, err := eng.StartWorkspace(projID, "Merged then remove", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace 3: %v", err)
	}
	before3 := runToPROpen(t, eng, st, id3)
	branch3 := before3.Branch
	if branch3 == "" || branch3 == branch2 {
		t.Fatalf("expected a distinct branch for third loop, got %q", branch3)
	}
	pr3, ok := st.PRByWorkspace(id3)
	if !ok {
		t.Fatal("expected PR before merge")
	}
	st.UpdatePR(pr3.ID, func(p *domain.PR) {
		p.State = "Merged"
		p.Group = "merged"
	})
	if _, err := eng.RemoveWorkspace(id3); err != nil {
		t.Fatalf("RemoveWorkspace after merge: %v", err)
	}
	waitFor(t, 5*time.Second, "branch deleted on remove after merge", func() bool {
		return !mgr.CommitExists(ctx, branch3)
	})
}

// TestDeleteWorkspaceRemovesEverything drives a loop to pr_open then deletes it: the
// store must forget it entirely and a loop.deleted event must fire on the bus.
func TestDeleteWorkspaceRemovesEverything(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, sink, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Add another converging feature", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	settled := runToPROpen(t, eng, st, id)
	pr, ok := st.PRByWorkspace(id)
	if !ok {
		t.Fatal("expected a PR before delete")
	}
	// The run mirrored agent turns into the session store; those rows are scoped to
	// the workspace and must cascade away with it.
	if got := st.SessionsForLoop(id); len(got) == 0 {
		t.Fatal("expected mirrored sessions before delete")
	}

	if err := eng.DeleteWorkspace(id); err != nil {
		t.Fatalf("DeleteWorkspace: %v", err)
	}
	if _, ok := st.Workspace(id); ok {
		t.Error("loop should be gone from the store")
	}
	if _, ok := st.PR(pr.ID); ok {
		t.Error("the loop's PR should be gone too")
	}
	if got := st.SessionsForLoop(id); len(got) != 0 {
		t.Errorf("sessions should cascade on delete, %d left", len(got))
	}
	if !sink.has("workspace.deleted") {
		t.Error("expected a workspace.deleted event on delete")
	}
	waitFor(t, 5*time.Second, "worktree directory removed", func() bool {
		_, err := os.Stat(settled.WorktreePath)
		return os.IsNotExist(err)
	})
}

// TestDeleteWorkspaceWhileRunningErrors asserts the not-running precondition: deleting a
// loop that is still actively running is refused, never yanking a live worktree out
// from under its goroutine.
func TestDeleteWorkspaceWhileRunningErrors(t *testing.T) {
	repo := initRepo(t)
	// blockingRunner blocks the Coder turn until the test releases it, keeping the
	// loop's goroutine alive (and its map entry present) for the duration.
	release := make(chan struct{})
	runner := &blockingCoderRunner{release: release}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "A loop that stays running", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 2*time.Second, "loop to start coding", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateCoding
	})

	if err := eng.DeleteWorkspace(id); err == nil {
		t.Error("expected DeleteWorkspace to be refused while the loop is running")
	}

	close(release)
	eng.Stop(id)
}

// blockingCoderRunner blocks the Coder role's turn on a channel so a test can
// assert behavior while a loop's goroutine is definitely still alive.
type blockingCoderRunner struct {
	release chan struct{}
}

func (r *blockingCoderRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if spec.Role == domain.RoleCoder {
		select {
		case <-r.release:
		case <-ctx.Done():
			return Result{}, ctx.Err()
		}
	}
	return Result{Subtype: "success"}, nil
}

// gitList shells out to `git -C <repo> worktree list --porcelain` and returns every
// worktree path (test helper; avoids importing the internal git package's private
// parser).
func gitList(t *testing.T, repoPath string) ([]string, error) {
	t.Helper()
	out, err := runGit(context.Background(), repoPath, "worktree", "list", "--porcelain")
	if err != nil {
		return nil, err
	}
	var paths []string
	for _, line := range splitLines(out) {
		if after, ok := strings.CutPrefix(line, "worktree "); ok {
			paths = append(paths, after)
		}
	}
	return paths, nil
}
