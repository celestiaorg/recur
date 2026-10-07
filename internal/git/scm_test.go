package git

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// isolateGitConfig hides global/system git config and author env so Identity
// tests see only the repo under test.
func isolateGitConfig(t *testing.T) {
	t.Helper()
	home := t.TempDir()
	empty := filepath.Join(home, "empty.gitconfig")
	if err := os.WriteFile(empty, nil, 0o644); err != nil {
		t.Fatal(err)
	}
	t.Setenv("HOME", home)
	t.Setenv("XDG_CONFIG_HOME", home)
	t.Setenv("GIT_CONFIG_GLOBAL", empty)
	t.Setenv("GIT_CONFIG_SYSTEM", empty)
	t.Setenv("GIT_CONFIG_NOSYSTEM", "1")
	for _, k := range []string{
		"GIT_AUTHOR_NAME", "GIT_AUTHOR_EMAIL",
		"GIT_COMMITTER_NAME", "GIT_COMMITTER_EMAIL",
		"EMAIL",
	} {
		t.Setenv(k, "")
		os.Unsetenv(k)
	}
}

func TestIdentity(t *testing.T) {
	repo := initRepo(t)
	isolateGitConfig(t)
	ctx := context.Background()

	name, email, err := Identity(ctx, repo)
	if err != nil {
		t.Fatalf("Identity(%q) error = %v, want nil", repo, err)
	}
	if name != "test" || email != "test@example.com" {
		t.Errorf("Identity(%q) = %q, %q, want %q, %q", repo, name, email, "test", "test@example.com")
	}

	if _, err := runGit(ctx, repo, "config", "user.useConfigOnly", "true"); err != nil {
		t.Fatalf("git config user.useConfigOnly: %v", err)
	}
	if _, err := runGit(ctx, repo, "config", "--unset", "user.name"); err != nil {
		t.Fatalf("git config --unset user.name: %v", err)
	}
	if _, err := runGit(ctx, repo, "config", "--unset", "user.email"); err != nil {
		t.Fatalf("git config --unset user.email: %v", err)
	}
	name, email, err = Identity(ctx, repo)
	if err == nil {
		t.Fatalf("Identity(%q) = %q, %q, nil, want error", repo, name, email)
	}
	if name != "" || email != "" {
		t.Errorf("Identity(%q) = %q, %q, want empty on error", repo, name, email)
	}
}

func TestStagedPathsNonGit(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "a.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	paths, err := StagedPaths(context.Background(), dir)
	if err != nil {
		t.Fatalf("StagedPaths on non-git dir: %v", err)
	}
	if len(paths) != 0 {
		t.Fatalf("StagedPaths = %v, want empty", paths)
	}
	diff, err := StagedDiff(context.Background(), dir)
	if err != nil {
		t.Fatalf("StagedDiff on non-git dir: %v", err)
	}
	if diff != "" {
		t.Fatalf("StagedDiff = %q, want empty", diff)
	}
	if err := Stage(context.Background(), dir, []string{"a.txt"}); err == nil || !strings.Contains(err.Error(), "not a git repository") {
		t.Fatalf("Stage error = %v, want not a git repository", err)
	}
}

func TestDiscardAndHumanCommit(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()

	wt, _, err := m.Add(ctx, "LOOP-SCM", "scm-test", "main")
	if err != nil {
		t.Fatalf("Add: %v", err)
	}

	path := filepath.Join(wt.Path, "edit.txt")
	if err := os.WriteFile(path, []byte("hello\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	st := WorkingStatus(ctx, wt.Path)
	if st["edit.txt"] != "u" {
		t.Fatalf("expected untracked edit.txt, got %v", st)
	}

	if err := DiscardFile(ctx, wt.Path, "edit.txt"); err != nil {
		t.Fatalf("DiscardFile untracked: %v", err)
	}
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Fatalf("expected untracked file removed, stat=%v", err)
	}

	// Tracked modify + discard
	tracked := filepath.Join(wt.Path, "README.md")
	_ = os.WriteFile(tracked, []byte("base\n"), 0o644)
	if _, err := m.Commit(ctx, wt.Path, "seed", "Coder", "coder@recur.local"); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(tracked, []byte("changed\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := DiscardFile(ctx, wt.Path, "README.md"); err != nil {
		t.Fatalf("DiscardFile tracked: %v", err)
	}
	data, _ := os.ReadFile(tracked)
	if string(data) != "base\n" {
		t.Fatalf("discard did not restore: %q", data)
	}

	// Human commit with identity
	if err := os.WriteFile(filepath.Join(wt.Path, "yours.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	name, email, err := Identity(ctx, wt.Path)
	if err != nil || name == "" || email == "" {
		t.Fatalf("Identity: name=%q email=%q err=%v", name, email, err)
	}
	res, err := m.Commit(ctx, wt.Path, "my quick fix", name, email)
	if err != nil || !res.Committed {
		t.Fatalf("Commit: %+v err=%v", res, err)
	}

	// Stage selective + CommitStaged
	if err := os.WriteFile(filepath.Join(wt.Path, "a.txt"), []byte("a\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(wt.Path, "b.txt"), []byte("b\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := Stage(ctx, wt.Path, []string{"a.txt"}); err != nil {
		t.Fatal(err)
	}
	staged, err := StagedPaths(ctx, wt.Path)
	if err != nil {
		t.Fatal(err)
	}
	if len(staged) != 1 || staged[0] != "a.txt" {
		t.Fatalf("staged=%v", staged)
	}
	res, err = m.CommitStaged(ctx, wt.Path, "only a", name, email)
	if err != nil || !res.Committed {
		t.Fatalf("CommitStaged: %+v err=%v", res, err)
	}
	st = WorkingStatus(ctx, wt.Path)
	if st["b.txt"] != "u" {
		t.Fatalf("b.txt should remain untracked, got %v", st)
	}
	if err := DiscardAll(ctx, wt.Path); err != nil {
		t.Fatal(err)
	}
	st = WorkingStatus(ctx, wt.Path)
	if len(st) != 0 {
		t.Fatalf("DiscardAll left status %v", st)
	}
}

func TestStagedDiffAndRecentSubjects(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()
	wt, _, err := m.Add(ctx, "LOOP-DIFF", "staged-diff", "main")
	if err != nil {
		t.Fatal(err)
	}

	empty, err := StagedDiff(ctx, wt.Path)
	if err != nil {
		t.Fatalf("StagedDiff empty: %v", err)
	}
	if strings.TrimSpace(empty) != "" {
		t.Fatalf("expected empty staged diff, got %q", empty)
	}

	if err := os.WriteFile(filepath.Join(wt.Path, "new.txt"), []byte("hello\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := Stage(ctx, wt.Path, []string{"new.txt"}); err != nil {
		t.Fatal(err)
	}
	diff, err := StagedDiff(ctx, wt.Path)
	if err != nil {
		t.Fatalf("StagedDiff: %v", err)
	}
	if !strings.Contains(diff, "new.txt") || !strings.Contains(diff, "+hello") {
		t.Fatalf("staged diff missing expected content: %q", diff)
	}

	name, email, err := Identity(ctx, wt.Path)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := m.CommitStaged(ctx, wt.Path, "add new.txt helper", name, email); err != nil {
		t.Fatal(err)
	}
	subjects := RecentCommitSubjects(ctx, wt.Path, 5)
	if len(subjects) == 0 || subjects[0] != "add new.txt helper" {
		t.Fatalf("RecentCommitSubjects=%v", subjects)
	}
}

func TestUnstage(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()
	wt, _, err := m.Add(ctx, "LOOP-US", "unstage", "main")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(wt.Path, "s.txt"), []byte("s\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := Stage(ctx, wt.Path, []string{"s.txt"}); err != nil {
		t.Fatal(err)
	}
	if err := Unstage(ctx, wt.Path, []string{"s.txt"}); err != nil {
		t.Fatal(err)
	}
	staged, _ := StagedPaths(ctx, wt.Path)
	if len(staged) != 0 {
		t.Fatalf("expected empty staged, got %v", staged)
	}
	// file still on disk untracked
	if _, err := os.Stat(filepath.Join(wt.Path, "s.txt")); err != nil {
		t.Fatal(err)
	}
	_ = strings.TrimSpace
}
