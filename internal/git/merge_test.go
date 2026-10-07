package git

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// addBranchCommit creates a worktree, writes a file, and commits it; returns
// the worktree info and the branch head hash.
func addBranchCommit(t *testing.T, m *Manager, ctx context.Context, workspaceID, branch, file, content string) (Worktree, string) {
	t.Helper()
	wt, _, err := m.Add(ctx, workspaceID, branch, "main")
	if err != nil {
		t.Fatalf("Add worktree: %v", err)
	}
	write(t, wt.Path, file, content)
	res, err := m.Commit(ctx, wt.Path, "recur(Coder): "+file, "Coder", "coder@recur.local")
	if err != nil {
		t.Fatalf("Commit: %v", err)
	}
	if !res.Committed {
		t.Fatal("expected a commit")
	}
	return wt, m.RefHash(ctx, wt.Branch)
}

// commitCount returns how many commits exist from ref back to the root.
func commitCount(t *testing.T, m *Manager, ctx context.Context, ref string) int {
	t.Helper()
	out, err := m.run(ctx, "rev-list", "--count", ref)
	if err != nil {
		t.Fatalf("rev-list --count %s: %v", ref, err)
	}
	n := 0
	for _, c := range strings.TrimSpace(out) {
		n = n*10 + int(c-'0')
	}
	return n
}

// TestMergeCommit tests the "merge" method (--no-ff): always creates a merge
// commit even when the branch is linear.
func TestMergeCommit(t *testing.T) {
	repo := initRepo(t)
	ctx := context.Background()
	m := NewManager(repo, t.TempDir())

	baseBefore := m.RefHash(ctx, "main")
	wt, branchHead := addBranchCommit(t, m, ctx, "LOOP-mc", "merge-feature", "mc.txt", "merge commit\n")

	mr, err := m.Merge(ctx, wt.Branch, "main", "Merge "+wt.Branch, "merge")
	if err != nil {
		t.Fatalf("Merge: %v", err)
	}

	baseAfter := m.RefHash(ctx, "main")
	if baseAfter == baseBefore {
		t.Error("base ref did not advance")
	}
	if mr.NewHead != baseAfter {
		t.Errorf("NewHead %q != base head %q", mr.NewHead, baseAfter)
	}
	if mr.FastForward {
		t.Error("expected --no-ff merge commit, got fast-forward")
	}
	if baseAfter == branchHead {
		t.Error("--no-ff merge commit should differ from branch head")
	}
	author, aerr := m.run(ctx, "log", "-1", "--format=%an <%ae>")
	if aerr != nil {
		t.Fatalf("log author: %v", aerr)
	}
	if got, want := strings.TrimSpace(author), "test <test@example.com>"; got != want {
		t.Errorf("Merge() author = %q, want %q (user git config, not Recur)", got, want)
	}
}

// TestSquashMerge tests the "squash" method: all branch commits land as a
// single commit on the base (the branch topology is lost).
func TestSquashMerge(t *testing.T) {
	repo := initRepo(t)
	ctx := context.Background()
	m := NewManager(repo, t.TempDir())

	countBefore := commitCount(t, m, ctx, "main")
	wt, _ := addBranchCommit(t, m, ctx, "LOOP-sq", "squash-feature", "sq.txt", "squash\n")

	// Add a second commit on the branch to make the squash non-trivial.
	write(t, wt.Path, "sq2.txt", "second\n")
	if _, err := m.Commit(ctx, wt.Path, "recur(Coder): sq2.txt", "Coder", "coder@recur.local"); err != nil {
		t.Fatalf("second commit: %v", err)
	}

	mr, err := m.Merge(ctx, wt.Branch, "main", "Squash "+wt.Branch, "squash")
	if err != nil {
		t.Fatalf("Squash merge: %v", err)
	}
	if mr.NewHead == "" {
		t.Fatal("expected a new head")
	}

	// Squash produces exactly one extra commit on base (not two).
	countAfter := commitCount(t, m, ctx, "main")
	if countAfter != countBefore+1 {
		t.Errorf("expected %d commits on main after squash, got %d", countBefore+1, countAfter)
	}
}

// TestSquashMergeDirtyBaseCheckout is the "Could not merge" toast: Recur lands
// onto the project folder (main checkout), not the workspace worktree. Uncommitted
// edits there — even if the workspace is clean — make git refuse the squash.
func TestSquashMergeDirtyBaseCheckout(t *testing.T) {
	repo := initRepo(t)
	ctx := context.Background()
	m := NewManager(repo, t.TempDir())

	wt, _ := addBranchCommit(t, m, ctx, "LOOP-sqd", "squash-dirty", "feat.txt", "feat\n")
	write(t, wt.Path, "README.md", "from branch\n")
	if _, err := m.Commit(ctx, wt.Path, "recur(Coder): README.md", "Coder", "coder@recur.local"); err != nil {
		t.Fatalf("branch README commit: %v", err)
	}
	write(t, repo, "README.md", "local dirty\n")

	_, err := m.Merge(ctx, wt.Branch, "main", "Squash "+wt.Branch, "squash")
	if err == nil {
		t.Fatal("Merge(squash) err=nil, want DirtyBaseError")
	}
	var dirty DirtyBaseError
	if !errors.As(err, &dirty) {
		t.Fatalf("Merge(squash) err=%v, want DirtyBaseError", err)
	}
	if dirty.Base != "main" {
		t.Errorf("DirtyBaseError.Base = %q, want %q", dirty.Base, "main")
	}
	found := false
	for _, f := range dirty.Files {
		if f == "README.md" {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("DirtyBaseError.Files = %v, want README.md", dirty.Files)
	}
	msg := err.Error()
	if strings.Contains(msg, "git merge --squash") || strings.Contains(msg, "exit status") {
		t.Errorf("Merge(squash) error = %q, want a user-facing message without git command dump", msg)
	}
	if !strings.Contains(msg, "project folder") || !strings.Contains(msg, "README.md") {
		t.Errorf("Merge(squash) error = %q, want project folder + README.md", msg)
	}
	got, rerr := os.ReadFile(filepath.Join(repo, "README.md"))
	if rerr != nil {
		t.Fatalf("read README.md: %v", rerr)
	}
	if strings.TrimSpace(string(got)) != "local dirty" {
		t.Errorf("README.md after failed squash = %q, want %q", strings.TrimSpace(string(got)), "local dirty")
	}
}

// TestRebaseMergeDirtyBaseCheckout: cherry-pick refuses when the base checkout
// has overlapping uncommitted edits. reset --hard must not run — that would
// destroy the dirty work DirtyBaseError then tells the user to stash.
func TestRebaseMergeDirtyBaseCheckout(t *testing.T) {
	repo := initRepo(t)
	ctx := context.Background()
	m := NewManager(repo, t.TempDir())

	wt, _ := addBranchCommit(t, m, ctx, "LOOP-rbd", "rebase-dirty", "feat.txt", "feat\n")
	write(t, wt.Path, "README.md", "from branch\n")
	if _, err := m.Commit(ctx, wt.Path, "recur(Coder): README.md", "Coder", "coder@recur.local"); err != nil {
		t.Fatalf("branch README commit: %v", err)
	}
	write(t, repo, "README.md", "local dirty\n")
	baseBefore := m.RefHash(ctx, "main")

	_, err := m.Merge(ctx, wt.Branch, "main", "", "rebase")
	if err == nil {
		t.Fatal("Merge(rebase) err=nil, want DirtyBaseError")
	}
	var dirty DirtyBaseError
	if !errors.As(err, &dirty) {
		t.Fatalf("Merge(rebase) err=%v, want DirtyBaseError", err)
	}
	found := false
	for _, f := range dirty.Files {
		if f == "README.md" {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("DirtyBaseError.Files = %v, want README.md", dirty.Files)
	}
	got, rerr := os.ReadFile(filepath.Join(repo, "README.md"))
	if rerr != nil {
		t.Fatalf("read README.md: %v", rerr)
	}
	if strings.TrimSpace(string(got)) != "local dirty" {
		t.Errorf("README.md after failed rebase = %q, want %q (uncommitted work preserved)", strings.TrimSpace(string(got)), "local dirty")
	}
	if after := m.RefHash(ctx, "main"); after != baseBefore {
		t.Errorf("Merge(rebase) dirty refusal moved main HEAD %s → %s, want unchanged", baseBefore, after)
	}
}

// TestRebaseMergeConflictPreservesUnrelatedDirty: a cherry-pick conflict on
// README.md must not reset --hard the base checkout — that would wipe
// uncommitted edits to files the pick did not touch.
func TestRebaseMergeConflictPreservesUnrelatedDirty(t *testing.T) {
	repo := initRepo(t)
	ctx := context.Background()
	m := NewManager(repo, t.TempDir())

	write(t, repo, "notes.txt", "committed notes\n")
	if _, err := m.run(ctx, "add", "notes.txt"); err != nil {
		t.Fatalf("git add notes.txt: %v", err)
	}
	if _, err := m.run(ctx, "commit", "-m", "notes"); err != nil {
		t.Fatalf("git commit notes: %v", err)
	}

	wt, _ := addBranchCommit(t, m, ctx, "LOOP-rbc", "rebase-conflict", "feat.txt", "feat\n")
	write(t, wt.Path, "README.md", "from branch\n")
	if _, err := m.Commit(ctx, wt.Path, "recur(Coder): README.md", "Coder", "coder@recur.local"); err != nil {
		t.Fatalf("branch README commit: %v", err)
	}

	write(t, repo, "README.md", "from main\n")
	if _, err := m.run(ctx, "add", "README.md"); err != nil {
		t.Fatalf("git add README.md: %v", err)
	}
	if _, err := m.run(ctx, "commit", "-m", "main README"); err != nil {
		t.Fatalf("git commit README: %v", err)
	}
	baseBefore := m.RefHash(ctx, "main")
	write(t, repo, "notes.txt", "UNRELATED DIRTY WORK\n")

	_, err := m.Merge(ctx, wt.Branch, "main", "", "rebase")
	if err == nil {
		t.Fatal("Merge(rebase) err=nil, want conflict")
	}
	var dirty DirtyBaseError
	if errors.As(err, &dirty) {
		t.Fatalf("Merge(rebase) err=%v, want a conflict not DirtyBaseError", err)
	}
	if after := m.RefHash(ctx, "main"); after != baseBefore {
		t.Errorf("Merge(rebase) conflict moved main HEAD %s → %s, want unchanged", baseBefore, after)
	}
	got, rerr := os.ReadFile(filepath.Join(repo, "notes.txt"))
	if rerr != nil {
		t.Fatalf("read notes.txt: %v", rerr)
	}
	if strings.TrimSpace(string(got)) != "UNRELATED DIRTY WORK" {
		t.Errorf("notes.txt after failed rebase = %q, want %q (unrelated dirty work preserved)", strings.TrimSpace(string(got)), "UNRELATED DIRTY WORK")
	}
}

func TestDirtyBaseErrorError(t *testing.T) {
	err := DirtyBaseError{Base: "main", Files: []string{"frontend/bindings/foo.ts"}}
	got := err.Error()
	if !strings.Contains(got, "project folder") || !strings.Contains(got, "main") || !strings.Contains(got, "foo.ts") {
		t.Errorf("DirtyBaseError.Error() = %q, want project folder, main, and foo.ts", got)
	}
	if strings.Contains(got, "git merge") {
		t.Errorf("DirtyBaseError.Error() = %q, must not mention git merge", got)
	}
}

func TestOverwrittenPaths(t *testing.T) {
	tests := []struct {
		name string
		msg  string
		want []string
	}{
		{
			name: "tracked local changes",
			msg: "error: Your local changes to the following files would be overwritten by merge:\n" +
				"\tfrontend/bindings/recur/internal/app/workspacesservice.ts\n" +
				"Please commit your changes or stash them before you merge.\n" +
				"Aborting\nMerge with strategy ort failed.",
			want: []string{"frontend/bindings/recur/internal/app/workspacesservice.ts"},
		},
		{
			name: "untracked working tree files",
			msg: "error: The following untracked working tree files would be overwritten by merge:\n" +
				"\tuntracked.txt\n" +
				"Please move or remove them before you merge.\n" +
				"Aborting",
			want: []string{"untracked.txt"},
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := overwrittenPaths(tt.msg)
			if len(got) != len(tt.want) {
				t.Fatalf("overwrittenPaths(%s) = %v, want %v", tt.name, got, tt.want)
			}
			for i := range tt.want {
				if got[i] != tt.want[i] {
					t.Errorf("overwrittenPaths(%s)[%d] = %q, want %q", tt.name, i, got[i], tt.want[i])
				}
			}
		})
	}
}

// TestUpdateWorktreeWithBaseMerge advances the worktree via git merge when the
// base tip moves ahead with a non-conflicting change.
func TestUpdateWorktreeWithBaseMerge(t *testing.T) {
	repo := initRepo(t)
	ctx := context.Background()
	m := NewManager(repo, t.TempDir())

	wt, _ := addBranchCommit(t, m, ctx, "LOOP-ubm", "update-merge", "feat.txt", "feature\n")

	// Advance base with a non-conflicting file.
	write(t, repo, "base.txt", "from base\n")
	if _, err := m.run(ctx, "-c", "user.name=test", "-c", "user.email=test@example.com",
		"add", "base.txt"); err != nil {
		t.Fatalf("git add: %v", err)
	}
	if _, err := m.run(ctx, "-c", "user.name=test", "-c", "user.email=test@example.com",
		"commit", "-m", "base moved"); err != nil {
		t.Fatalf("git commit base: %v", err)
	}
	baseHead := m.RefHash(ctx, "main")

	if err := m.UpdateWorktreeWithBase(ctx, wt.Path, "main", "merge"); err != nil {
		t.Fatalf("UpdateWorktreeWithBase merge: %v", err)
	}
	// Worktree should now contain the base tip as an ancestor.
	if _, err := m.runIn(ctx, wt.Path, "merge-base", "--is-ancestor", baseHead, "HEAD"); err != nil {
		t.Error("expected base tip to be an ancestor of worktree HEAD after merge")
	}
	behind, err := m.CommitsBehind(ctx, wt.Path, "main")
	if err != nil {
		t.Fatalf("CommitsBehind: %v", err)
	}
	if behind != 0 {
		t.Errorf("CommitsBehind(%q, main) = %d, want 0", wt.Path, behind)
	}
	author, aerr := m.runIn(ctx, wt.Path, "log", "-1", "--format=%an <%ae>")
	if aerr != nil {
		t.Fatalf("log author: %v", aerr)
	}
	if got, want := strings.TrimSpace(author), "test <test@example.com>"; got != want {
		t.Errorf("UpdateWorktreeWithBase merge author = %q, want %q (user git config, not Recur)", got, want)
	}
}

// TestUpdateWorktreeWithBaseRebase replays worktree commits onto a moved base.
func TestUpdateWorktreeWithBaseRebase(t *testing.T) {
	repo := initRepo(t)
	ctx := context.Background()
	m := NewManager(repo, t.TempDir())

	wt, _ := addBranchCommit(t, m, ctx, "LOOP-ubr", "update-rebase", "feat.txt", "feature\n")

	write(t, repo, "base.txt", "from base\n")
	if _, err := m.run(ctx, "-c", "user.name=test", "-c", "user.email=test@example.com",
		"add", "base.txt"); err != nil {
		t.Fatalf("git add: %v", err)
	}
	if _, err := m.run(ctx, "-c", "user.name=test", "-c", "user.email=test@example.com",
		"commit", "-m", "base moved"); err != nil {
		t.Fatalf("git commit base: %v", err)
	}
	baseHead := m.RefHash(ctx, "main")

	if err := m.UpdateWorktreeWithBase(ctx, wt.Path, "main", "rebase"); err != nil {
		t.Fatalf("UpdateWorktreeWithBase rebase: %v", err)
	}
	if _, err := m.runIn(ctx, wt.Path, "merge-base", "--is-ancestor", baseHead, "HEAD"); err != nil {
		t.Error("expected base tip to be an ancestor of worktree HEAD after rebase")
	}
	// Rebase should leave linear history (no merge commit parents > 1).
	parents, err := m.runIn(ctx, wt.Path, "rev-list", "--parents", "-n", "1", "HEAD")
	if err != nil {
		t.Fatalf("rev-list: %v", err)
	}
	if len(strings.Fields(strings.TrimSpace(parents))) > 2 {
		t.Errorf("expected linear history after rebase, got parents line %q", parents)
	}
}

// TestUpdateWorktreeWithBaseConflict aborts and leaves the worktree clean.
func TestUpdateWorktreeWithBaseConflict(t *testing.T) {
	repo := initRepo(t)
	ctx := context.Background()
	m := NewManager(repo, t.TempDir())

	wt, _ := addBranchCommit(t, m, ctx, "LOOP-ubc", "update-conflict", "same.txt", "branch side\n")

	// Conflict: base edits the same file differently.
	write(t, repo, "same.txt", "base side\n")
	if _, err := m.run(ctx, "-c", "user.name=test", "-c", "user.email=test@example.com",
		"add", "same.txt"); err != nil {
		t.Fatalf("git add: %v", err)
	}
	if _, err := m.run(ctx, "-c", "user.name=test", "-c", "user.email=test@example.com",
		"commit", "-m", "base conflict"); err != nil {
		t.Fatalf("git commit base: %v", err)
	}

	if err := m.UpdateWorktreeWithBase(ctx, wt.Path, "main", "merge"); err == nil {
		t.Fatal("expected conflict error")
	}
	clean, err := m.isClean(ctx, wt.Path)
	if err != nil {
		t.Fatalf("isClean: %v", err)
	}
	if !clean {
		t.Error("worktree should be clean after conflict abort")
	}
}

// TestRebaseMerge tests the "rebase" method: branch commits are replayed
// onto base, then the base is fast-forwarded — the result is linear history.
func TestRebaseMerge(t *testing.T) {
	repo := initRepo(t)
	ctx := context.Background()
	m := NewManager(repo, t.TempDir())

	countBefore := commitCount(t, m, ctx, "main")
	wt, _ := addBranchCommit(t, m, ctx, "LOOP-rb", "rebase-feature", "rb.txt", "rebase\n")

	mr, err := m.Merge(ctx, wt.Branch, "main", "", "rebase")
	if err != nil {
		t.Fatalf("Rebase merge: %v", err)
	}

	if !mr.FastForward {
		t.Error("rebase merge should result in a fast-forward")
	}

	// Rebase adds the branch commit(s) linearly — no merge commit.
	countAfter := commitCount(t, m, ctx, "main")
	if countAfter != countBefore+1 {
		t.Errorf("expected %d commits after rebase, got %d", countBefore+1, countAfter)
	}
}

// TestMergeIntoWorktree covers the fan-out merge-up: a child branch merges
// into the parent's WORKTREE (not the shared checkout) with a --no-ff commit,
// and a content conflict is left in progress with the conflicted paths named.
func TestMergeIntoWorktree(t *testing.T) {
	repo := initRepo(t)
	ctx := context.Background()
	m := NewManager(repo, t.TempDir())

	parent, _, err := m.Add(ctx, "LOOP-parent", "parent", "main")
	if err != nil {
		t.Fatalf("Add parent: %v", err)
	}
	// Child A branches off the parent branch and adds its own file.
	childA, _, err := m.Add(ctx, "LOOP-childa", "child-a", parent.Branch)
	if err != nil {
		t.Fatalf("Add child A: %v", err)
	}
	write(t, childA.Path, "a.txt", "from a\n")
	if _, err := m.Commit(ctx, childA.Path, "step A", "test", "test@example.com"); err != nil {
		t.Fatalf("Commit A: %v", err)
	}
	conflicts, err := m.MergeInto(ctx, parent.Path, childA.Branch, "Merge step A", "test", "test@example.com")
	if err != nil || len(conflicts) != 0 {
		t.Fatalf("MergeInto A: conflicts=%v err=%v", conflicts, err)
	}
	if _, err := os.Stat(filepath.Join(parent.Path, "a.txt")); err != nil {
		t.Fatalf("a.txt should be in the parent worktree after merge: %v", err)
	}
	subject, _ := m.runIn(ctx, parent.Path, "log", "-1", "--format=%s")
	if strings.TrimSpace(subject) != "Merge step A" {
		t.Errorf("merge commit subject = %q", strings.TrimSpace(subject))
	}

	// Child B (also off the parent, before A landed) edits README the same way
	// the parent does afterwards → content conflict, left in progress.
	childB, _, err := m.Add(ctx, "LOOP-childb", "child-b", parent.Branch)
	if err != nil {
		t.Fatalf("Add child B: %v", err)
	}
	write(t, childB.Path, "README.md", "child B says hi\n")
	if _, err := m.Commit(ctx, childB.Path, "step B", "test", "test@example.com"); err != nil {
		t.Fatalf("Commit B: %v", err)
	}
	write(t, parent.Path, "README.md", "parent says hi\n")
	if _, err := m.Commit(ctx, parent.Path, "parent edit", "test", "test@example.com"); err != nil {
		t.Fatalf("Commit parent: %v", err)
	}
	conflicts, err = m.MergeInto(ctx, parent.Path, childB.Branch, "Merge step B", "test", "test@example.com")
	if err != nil {
		t.Fatalf("MergeInto B: %v", err)
	}
	if len(conflicts) != 1 || conflicts[0] != "README.md" {
		t.Fatalf("conflicts = %v, want [README.md]", conflicts)
	}
	if _, err := os.Stat(filepath.Join(parent.Path, ".git")); err != nil {
		t.Fatal(err)
	}
	if out, _ := m.runIn(ctx, parent.Path, "rev-parse", "-q", "--verify", "MERGE_HEAD"); strings.TrimSpace(out) == "" {
		t.Fatal("merge should be left in progress (MERGE_HEAD set) for the coder to resolve")
	}
	// Resolving and committing through the engine's Commit path concludes the merge.
	write(t, parent.Path, "README.md", "both say hi\n")
	res, err := m.Commit(ctx, parent.Path, "resolve", "test", "test@example.com")
	if err != nil || !res.Committed {
		t.Fatalf("Commit after resolve: %+v %v", res, err)
	}
	if out, _ := m.runIn(ctx, parent.Path, "rev-parse", "-q", "--verify", "MERGE_HEAD"); strings.TrimSpace(out) != "" {
		t.Fatal("MERGE_HEAD should be gone after the resolving commit")
	}
	parents, _ := m.runIn(ctx, parent.Path, "log", "-1", "--format=%P")
	if len(strings.Fields(parents)) != 2 {
		t.Errorf("resolving commit should be a merge commit, parents=%q", parents)
	}
}

func TestCommitConcludesOursOnlyMerge(t *testing.T) {
	repo := initRepo(t)
	ctx := context.Background()
	m := NewManager(repo, t.TempDir())

	parent, _, err := m.Add(ctx, "LOOP-ours", "parent-ours", "main")
	if err != nil {
		t.Fatalf("Add parent: %v", err)
	}
	child, _, err := m.Add(ctx, "LOOP-ourschild", "child-ours", parent.Branch)
	if err != nil {
		t.Fatalf("Add child: %v", err)
	}
	write(t, child.Path, "README.md", "child says hi\n")
	if _, err := m.Commit(ctx, child.Path, "child edit", "test", "test@example.com"); err != nil {
		t.Fatalf("Commit child: %v", err)
	}
	write(t, parent.Path, "README.md", "parent says hi\n")
	if _, err := m.Commit(ctx, parent.Path, "parent edit", "test", "test@example.com"); err != nil {
		t.Fatalf("Commit parent: %v", err)
	}
	conflicts, err := m.MergeInto(ctx, parent.Path, child.Branch, "Merge child", "test", "test@example.com")
	if err != nil {
		t.Fatalf("MergeInto: %v", err)
	}
	if len(conflicts) != 1 || conflicts[0] != "README.md" {
		t.Fatalf("conflicts = %v, want [README.md]", conflicts)
	}
	write(t, parent.Path, "README.md", "parent says hi\n")
	res, err := m.Commit(ctx, parent.Path, "keep ours", "test", "test@example.com")
	if err != nil || !res.Committed {
		t.Fatalf("Commit(ours-only merge) = %+v %v, want Committed", res, err)
	}
	if m.MergeHead(ctx, parent.Path) {
		t.Fatal("MERGE_HEAD should be gone after the ours-only resolving commit")
	}
	parents, _ := m.runIn(ctx, parent.Path, "log", "-1", "--format=%P")
	if len(strings.Fields(parents)) != 2 {
		t.Errorf("ours-only resolving commit should be a merge commit, parents=%q", parents)
	}
}
