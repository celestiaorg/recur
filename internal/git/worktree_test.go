package git

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

// initRepo creates a git repo with one commit on branch "main" and returns its path.
func initRepo(t *testing.T) string {
	t.Helper()
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	dir := t.TempDir()
	run := func(args ...string) {
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
	run("init", "-b", "main")
	run("config", "user.name", "test")
	run("config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(dir, "README.md"), []byte("hello\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "initial")
	return dir
}

func TestIsRepo(t *testing.T) {
	if IsRepo("") || IsRepo(t.TempDir()) {
		t.Fatal("empty / plain folder must not report as a git repo")
	}
	repo := initRepo(t)
	if !IsRepo(repo) {
		t.Fatalf("initRepo path %q should be a git repo", repo)
	}
	// Nested folders sit inside a work tree but are not workspace hosts — Create
	// would allocate a worktree of the enclosing repo, not of the project folder.
	sub := filepath.Join(repo, "nested")
	if err := os.Mkdir(sub, 0o755); err != nil {
		t.Fatal(err)
	}
	if IsRepo(sub) {
		t.Fatalf("subdir %q must not count as a git project root", sub)
	}
}

func TestRepoRoot(t *testing.T) {
	if _, ok := RepoRoot(t.TempDir()); ok {
		t.Fatal("RepoRoot(plain dir) ok = true, want false")
	}
	repo := initRepo(t)
	top, ok := RepoRoot(repo)
	if !ok {
		t.Fatal("RepoRoot(repo) ok = false, want true")
	}
	want, _ := filepath.EvalSymlinks(repo)
	if top != want && top != filepath.Clean(repo) {
		t.Fatalf("RepoRoot(%s) = %q, want %q", repo, top, want)
	}
	sub := filepath.Join(repo, "nested")
	if err := os.Mkdir(sub, 0o755); err != nil {
		t.Fatal(err)
	}
	file := filepath.Join(sub, "a.txt")
	if err := os.WriteFile(file, []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	fromFile, ok := RepoRoot(file)
	if !ok {
		t.Fatal("RepoRoot(nested file) ok = false, want true")
	}
	if fromFile != top {
		t.Fatalf("RepoRoot(%s) = %q, want toplevel %q", file, fromFile, top)
	}
}

func TestPathHasPrefix(t *testing.T) {
	if !PathHasPrefix("/foo/bar", "/foo") {
		t.Fatal("PathHasPrefix(/foo/bar, /foo) = false, want true")
	}
	if PathHasPrefix("/foobar", "/foo") {
		t.Fatal("PathHasPrefix(/foobar, /foo) = true, want false")
	}
	if !PathHasPrefix("/foo", "/foo") {
		t.Fatal("PathHasPrefix(/foo, /foo) = false, want true")
	}
	dir := t.TempDir()
	alt := flipWorktreePathCase(dir)
	if alt == dir {
		return
	}
	if _, err := os.Stat(alt); err != nil {
		return // case-sensitive volume: fold must not invent a match
	}
	child := filepath.Join(alt, "bar")
	if err := os.Mkdir(filepath.Join(dir, "bar"), 0o755); err != nil {
		t.Fatal(err)
	}
	if !PathHasPrefix(child, dir) {
		t.Fatalf("PathHasPrefix(%s, %s) = false, want true on case-insensitive volume", child, dir)
	}
	if !SamePath(alt, dir) {
		t.Fatalf("SamePath(%s, %s) = false, want true on case-insensitive volume", alt, dir)
	}
	aligned := AlignUnderRoot(child, dir)
	want := filepath.Join(dir, "bar")
	if aligned != want {
		t.Fatalf("AlignUnderRoot(%s, %s) = %q, want %q", child, dir, aligned, want)
	}
}

func flipWorktreePathCase(p string) string {
	b := []byte(p)
	for i := len(b) - 1; i >= 0; i-- {
		c := b[i]
		if c >= 'a' && c <= 'z' {
			b[i] = c - 'a' + 'A'
			return string(b)
		}
		if c >= 'A' && c <= 'Z' {
			b[i] = c - 'A' + 'a'
			return string(b)
		}
	}
	return p
}

func TestAddListRemove(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()

	wt, _, err := m.Add(ctx, "LOOP-1", "rate-limit", "main")
	if err != nil {
		t.Fatalf("Add: %v", err)
	}
	if wt.Branch != "recur/rate-limit" {
		t.Errorf("branch: got %q, want recur/rate-limit", wt.Branch)
	}
	if _, err := os.Stat(wt.Path); err != nil {
		t.Errorf("worktree dir not created: %v", err)
	}

	list, err := m.List(ctx)
	if err != nil {
		t.Fatalf("List: %v", err)
	}
	found := false
	for _, w := range list {
		if cleanPath(w.Path) == cleanPath(wt.Path) {
			found = true
			if w.Branch != "recur/rate-limit" {
				t.Errorf("listed branch: got %q", w.Branch)
			}
		}
	}
	if !found {
		t.Errorf("added worktree not in list: %+v", list)
	}

	if err := m.Remove(ctx, wt.Path); err != nil {
		t.Fatalf("Remove: %v", err)
	}
	if _, err := os.Stat(wt.Path); !os.IsNotExist(err) {
		t.Errorf("worktree dir still present after remove: %v", err)
	}
}

func TestWorktreeRelPath(t *testing.T) {
	cases := []struct {
		id, want string
	}{
		{"recur.flaky-ramen", filepath.Join("recur", "flaky-ramen")},
		{"flaky-ramen", "flaky-ramen"},
		{"LOOP-1", "LOOP-1"},
		{"my-app.bubbly-pretzel-2", filepath.Join("my-app", "bubbly-pretzel-2")},
		{"", ""},
	}
	for _, tc := range cases {
		if got := WorktreeRelPath(tc.id); got != tc.want {
			t.Errorf("WorktreeRelPath(%q) = %q, want %q", tc.id, got, tc.want)
		}
	}
	if slug := WorkspaceSlug("recur.flaky-ramen"); slug != "flaky-ramen" {
		t.Errorf("WorkspaceSlug dotted = %q", slug)
	}
	if slug := WorkspaceSlug("flaky-ramen"); slug != "flaky-ramen" {
		t.Errorf("WorkspaceSlug legacy = %q", slug)
	}
}

func TestAddNestedProjectLayout(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()

	wt, _, err := m.Add(ctx, "kitchen.bubbly-pretzel", "bubbly-pretzel", "main")
	if err != nil {
		t.Fatalf("Add: %v", err)
	}
	want := filepath.Join(root, "kitchen", "bubbly-pretzel")
	if wt.Path != want {
		t.Fatalf("Path = %q, want %q", wt.Path, want)
	}
	if wt.Branch != "recur/bubbly-pretzel" {
		t.Fatalf("Branch = %q, want recur/bubbly-pretzel", wt.Branch)
	}
	if _, err := os.Stat(want); err != nil {
		t.Fatalf("nested dir missing: %v", err)
	}

	if err := m.Remove(ctx, wt.Path); err != nil {
		t.Fatalf("Remove: %v", err)
	}
	if _, err := os.Stat(want); !os.IsNotExist(err) {
		t.Errorf("nested worktree still present: %v", err)
	}
	if _, err := os.Stat(filepath.Join(root, "kitchen")); !os.IsNotExist(err) {
		t.Errorf("empty project dir not pruned: %v", err)
	}
}

func TestValidateManagedWorkspaceID(t *testing.T) {
	ok := []string{"flaky-ramen", "kitchen.bubbly-pretzel", "LOOP-1", "my-app.x-2"}
	for _, id := range ok {
		if err := validateManagedWorkspaceID(id); err != nil {
			t.Errorf("validateManagedWorkspaceID(%q) unexpected: %v", id, err)
		}
	}
	bad := []string{"", ".", "..", "../escape", "foo/bar", `foo\bar`}
	for _, id := range bad {
		if err := validateManagedWorkspaceID(id); err == nil {
			t.Errorf("validateManagedWorkspaceID(%q) want error", id)
		}
	}
}

func TestCurrentBranch(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()

	wt, _, err := m.Add(ctx, "LOOP-CB", "cb", "main")
	if err != nil {
		t.Fatalf("Add: %v", err)
	}
	got, err := CurrentBranch(ctx, wt.Path)
	if err != nil {
		t.Fatalf("CurrentBranch: %v", err)
	}
	if got != "recur/cb" {
		t.Fatalf("CurrentBranch = %q, want recur/cb", got)
	}

	cmd := exec.Command("git", "-C", wt.Path, "branch", "-m", "renamed/feature")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("branch -m: %v\n%s", err, out)
	}
	got, err = CurrentBranch(ctx, wt.Path)
	if err != nil {
		t.Fatalf("CurrentBranch after rename: %v", err)
	}
	if got != "renamed/feature" {
		t.Fatalf("CurrentBranch after rename = %q, want renamed/feature", got)
	}

	if _, err := CurrentBranch(ctx, ""); err == nil {
		t.Error("CurrentBranch empty worktree should error")
	}
}

func TestSanitizeBranchName(t *testing.T) {
	cases := []struct {
		in, want string
	}{
		{"add-dark-mode", "recur/add-dark-mode"},
		{"Add Dark Mode", "recur/add-dark-mode"},
		{"recur/already-prefixed", "recur/already-prefixed"},
		{"feature/foo", "feature/foo"},
		{"Fix bug.", "recur/fix-bug"},
		{".net support", "recur/net-support"},
		{"fix db.lock", "recur/fix-db"},
	}
	for _, tc := range cases {
		got, err := SanitizeBranchName(tc.in)
		if err != nil {
			t.Errorf("SanitizeBranchName(%q): %v", tc.in, err)
			continue
		}
		if got != tc.want {
			t.Errorf("SanitizeBranchName(%q)=%q, want %q", tc.in, got, tc.want)
		}
	}
	if _, err := SanitizeBranchName("   "); err == nil {
		t.Error("empty name should error")
	}
}

func TestPrefixedBranch(t *testing.T) {
	cases := []struct {
		prefix, slug, want string
	}{
		{"recur", "rate-limit", "recur/rate-limit"},
		{"", "rate-limit", "rate-limit"},
		{"josh", "rate-limit", "josh/rate-limit"},
		{"/josh/", "/rate-limit/", "josh/rate-limit"},
	}
	for _, tc := range cases {
		got := PrefixedBranch(tc.prefix, tc.slug)
		if got != tc.want {
			t.Errorf("PrefixedBranch(%q, %q) = %q, want %q", tc.prefix, tc.slug, got, tc.want)
		}
	}
}

func TestNormalizeBranchPrefix(t *testing.T) {
	cases := []struct {
		in, want string
	}{
		{"", ""},
		{"  ", ""},
		{"Recur", "recur"},
		{"My User", "my-user"},
		{"foo/bar", "foo/bar"},
	}
	for _, tc := range cases {
		got := NormalizeBranchPrefix(tc.in)
		if got != tc.want {
			t.Errorf("NormalizeBranchPrefix(%q) = %q, want %q", tc.in, got, tc.want)
		}
	}
}

func TestSanitizeBranchNameWithPrefix(t *testing.T) {
	cases := []struct {
		prefix, in, want string
	}{
		{"josh", "add-dark-mode", "josh/add-dark-mode"},
		{"", "add-dark-mode", "add-dark-mode"},
		{"josh", "feature/foo", "feature/foo"},
		{"MyUser", "Add Dark Mode", "myuser/add-dark-mode"},
	}
	for _, tc := range cases {
		got, err := SanitizeBranchNameWithPrefix(tc.prefix, tc.in)
		if err != nil {
			t.Errorf("SanitizeBranchNameWithPrefix(%q, %q): %v", tc.prefix, tc.in, err)
			continue
		}
		if got != tc.want {
			t.Errorf("SanitizeBranchNameWithPrefix(%q, %q) = %q, want %q", tc.prefix, tc.in, got, tc.want)
		}
	}
}

func TestAddUsesBranchPrefix(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	ctx := context.Background()

	custom, _, err := NewManager(repo, root).WithBranchPrefix("josh").Add(ctx, "p.custom-ws", "custom-ws", "main")
	if err != nil {
		t.Fatalf("Add custom prefix: %v", err)
	}
	if custom.Branch != "josh/custom-ws" {
		t.Errorf("Add(custom) branch = %q, want josh/custom-ws", custom.Branch)
	}

	none, _, err := NewManager(repo, root).WithBranchPrefix("").Add(ctx, "p.none-ws", "none-ws", "main")
	if err != nil {
		t.Fatalf("Add none prefix: %v", err)
	}
	if none.Branch != "none-ws" {
		t.Errorf("Add(none) branch = %q, want none-ws", none.Branch)
	}
}

func TestAddRejectsPrefixThatExistsAsBranch(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	ctx := context.Background()
	if _, err := NewManager(repo, root).run(ctx, "branch", "josh"); err != nil {
		t.Fatalf("git branch josh: %v", err)
	}

	_, _, err := NewManager(repo, root).WithBranchPrefix("josh").Add(ctx, "p.collide-ws", "collide-ws", "main")
	if err == nil {
		t.Fatal("Add with prefix that exists as a branch succeeded, want conflict error")
	}
	if !strings.Contains(err.Error(), "josh") || !strings.Contains(err.Error(), "blocks") {
		t.Errorf("Add conflict error = %v, want it to name the blocking branch", err)
	}
}

func TestRenameBranch(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()

	wt, _, err := m.Add(ctx, "LOOP-RB", "rb", "main")
	if err != nil {
		t.Fatalf("Add: %v", err)
	}
	got, err := RenameBranch(ctx, wt.Path, "add-dark-mode")
	if err != nil {
		t.Fatalf("RenameBranch: %v", err)
	}
	if got != "recur/add-dark-mode" {
		t.Fatalf("RenameBranch = %q, want recur/add-dark-mode", got)
	}
	live, err := CurrentBranch(ctx, wt.Path)
	if err != nil || live != "recur/add-dark-mode" {
		t.Fatalf("live = %q err=%v", live, err)
	}
	again, err := RenameBranch(ctx, wt.Path, "add-dark-mode")
	if err != nil || again != "recur/add-dark-mode" {
		t.Fatalf("idempotent rename: %q %v", again, err)
	}
	cmd := exec.Command("git", "-C", wt.Path, "branch", "recur/taken")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("create taken: %v\n%s", err, out)
	}
	if _, err := RenameBranch(ctx, wt.Path, "taken"); err == nil {
		t.Fatal("rename onto existing recur/taken should fail")
	}
}

func TestListLocalBranches(t *testing.T) {
	repo := initRepo(t)
	m := NewManager(repo, t.TempDir())
	ctx := context.Background()

	// Create a second local branch so the list isn't just "main".
	if _, err := m.run(ctx, "branch", "feature/x"); err != nil {
		t.Fatalf("create branch: %v", err)
	}
	branches, err := m.ListLocalBranches(ctx)
	if err != nil {
		t.Fatalf("ListLocalBranches: %v", err)
	}
	seen := map[string]bool{}
	for _, b := range branches {
		seen[b] = true
	}
	if !seen["main"] || !seen["feature/x"] {
		t.Fatalf("expected main + feature/x, got %v", branches)
	}
}

func TestAddEmptyRepoCreatesInitialCommit(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	dir := t.TempDir()
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = dir
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-b", "main")
	run("config", "user.name", "test")
	run("config", "user.email", "test@example.com")
	// Leave an untracked file — the initial commit must NOT scoop it up.
	if err := os.WriteFile(filepath.Join(dir, "scratch.txt"), []byte("leave me\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := NewManager(dir, t.TempDir())
	ctx := context.Background()

	wt, _, err := m.Add(ctx, "LOOP-EMPTY", "first-task", "main")
	if err != nil {
		t.Fatalf("Add on empty repo: %v", err)
	}
	if wt.Branch != "recur/first-task" {
		t.Errorf("branch: got %q", wt.Branch)
	}
	if !m.commitExists(ctx, "main") {
		t.Fatal("expected an initial commit on main")
	}
	author, aerr := m.run(ctx, "log", "-1", "--format=%an <%ae>")
	if aerr != nil {
		t.Fatalf("log author: %v", aerr)
	}
	if got, want := strings.TrimSpace(author), "test <test@example.com>"; got != want {
		t.Errorf("ensureInitialCommit author = %q, want %q", got, want)
	}
	// Untracked file must still be untracked in the source checkout.
	st, err := m.run(ctx, "status", "--porcelain")
	if err != nil {
		t.Fatalf("status: %v", err)
	}
	if !strings.Contains(st, "scratch.txt") {
		t.Fatalf("expected scratch.txt to remain untracked, status=%q", st)
	}
	// Worktree should be based on the empty initial commit (no scratch.txt tracked).
	tracked, _ := m.run(ctx, "-C", wt.Path, "ls-files")
	if strings.Contains(tracked, "scratch.txt") {
		t.Fatal("initial commit must not include the user's untracked files")
	}
}

func TestAddEmptyRepoRequiresGitIdentity(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	dir := t.TempDir()
	cmd := exec.Command("git", "init", "-b", "main")
	cmd.Dir = dir
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git init: %v\n%s", err, out)
	}
	cmd = exec.Command("git", "config", "user.useConfigOnly", "true")
	cmd.Dir = dir
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git config: %v\n%s", err, out)
	}
	isolateGitConfig(t)

	m := NewManager(dir, t.TempDir())
	_, _, err := m.Add(context.Background(), "LOOP-EMPTY", "first-task", "main")
	if err == nil {
		t.Fatal("Add() error = nil, want git identity error")
	}
	if !strings.Contains(err.Error(), "git identity") {
		t.Errorf("Add() error = %v, want git identity", err)
	}
	if m.commitExists(context.Background(), "main") {
		t.Fatal("Add() must not create an initial commit without a git identity")
	}
}

func TestAddIsIdempotent(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()

	wt1, created1, err := m.Add(ctx, "LOOP-2", "feature-x", "main")
	if err != nil {
		t.Fatalf("first Add: %v", err)
	}
	if !created1 {
		t.Fatal("first Add should report created=true")
	}
	// Calling Add again for the same loop must return the existing worktree, not error.
	wt2, created2, err := m.Add(ctx, "LOOP-2", "feature-x", "main")
	if err != nil {
		t.Fatalf("second Add (idempotent): %v", err)
	}
	if created2 {
		t.Fatal("second Add should report created=false")
	}
	if cleanPath(wt1.Path) != cleanPath(wt2.Path) {
		t.Errorf("idempotent Add returned different paths: %q vs %q", wt1.Path, wt2.Path)
	}
}

func TestAddDoesNotReuseLeftoverBranch(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()

	wt1, _, err := m.Add(ctx, "first-ws", "feature-x", "main")
	if err != nil {
		t.Fatalf("first Add: %v", err)
	}
	stale := filepath.Join(wt1.Path, "stale.txt")
	if err := os.WriteFile(stale, []byte("old work\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := m.Commit(ctx, wt1.Path, "stale work", "test", "test@example.com"); err != nil {
		t.Fatalf("Commit: %v", err)
	}
	staleHead := gitRevParse(t, repo, "recur/feature-x")
	mainHead := gitRevParse(t, repo, "main")
	if staleHead == mainHead {
		t.Fatal("setup: leftover branch should differ from main")
	}

	if err := m.Remove(ctx, wt1.Path); err != nil {
		t.Fatalf("Remove: %v", err)
	}

	wt2, created, err := m.Add(ctx, "second-ws", "feature-x", "main")
	if err != nil {
		t.Fatalf("Add with leftover branch: %v", err)
	}
	if !created {
		t.Fatal("Add over leftover branch should allocate a new worktree")
	}
	if wt2.Branch != "recur/feature-x-2" {
		t.Errorf("Add leftover branch = %q, want recur/feature-x-2", wt2.Branch)
	}
	if got := gitRevParse(t, repo, wt2.Branch); got != mainHead {
		t.Errorf("Add leftover HEAD = %s, want main %s (not leftover %s)", got, mainHead, staleHead)
	}
	if _, err := os.Stat(filepath.Join(wt2.Path, "stale.txt")); !os.IsNotExist(err) {
		t.Fatal("new worktree must not carry leftover stale.txt")
	}
}

func TestAddSkipsTakenNumericSuffixes(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()
	for _, b := range []string{"recur/feature-x", "recur/feature-x-2"} {
		if _, err := m.run(ctx, "branch", b); err != nil {
			t.Fatalf("git branch %s: %v", b, err)
		}
	}

	wt, _, err := m.Add(ctx, "third-ws", "feature-x", "main")
	if err != nil {
		t.Fatalf("Add: %v", err)
	}
	if wt.Branch != "recur/feature-x-3" {
		t.Errorf("Add(third-ws, feature-x) branch = %q, want recur/feature-x-3", wt.Branch)
	}
}

func TestRestoreReusesExistingBranch(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()

	wt1, _, err := m.Add(ctx, "archived-ws", "archived-ws", "main")
	if err != nil {
		t.Fatalf("Add: %v", err)
	}
	stale := filepath.Join(wt1.Path, "kept.txt")
	if err := os.WriteFile(stale, []byte("keep me\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := m.Commit(ctx, wt1.Path, "keep", "test", "test@example.com"); err != nil {
		t.Fatalf("Commit: %v", err)
	}
	wantHead := gitRevParse(t, repo, "recur/archived-ws")

	if err := m.Remove(ctx, wt1.Path); err != nil {
		t.Fatalf("Remove: %v", err)
	}

	wt2, created, err := m.Restore(ctx, "archived-ws", "recur/archived-ws", "main")
	if err != nil {
		t.Fatalf("Restore: %v", err)
	}
	if !created {
		t.Fatal("Restore after Remove should allocate a worktree")
	}
	if wt2.Branch != "recur/archived-ws" {
		t.Errorf("Restore branch = %q, want recur/archived-ws", wt2.Branch)
	}
	if got := gitRevParse(t, repo, wt2.Branch); got != wantHead {
		t.Errorf("Restore HEAD = %s, want leftover %s", got, wantHead)
	}
	if _, err := os.Stat(filepath.Join(wt2.Path, "kept.txt")); err != nil {
		t.Fatalf("Restore should keep leftover file: %v", err)
	}
}

func TestRestoreEmptyBranchUsesDefaultPrefix(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	ctx := context.Background()

	wt1, _, err := NewManager(repo, root).Add(ctx, "legacy-ws", "legacy-ws", "main")
	if err != nil {
		t.Fatalf("Add: %v", err)
	}
	stale := filepath.Join(wt1.Path, "kept.txt")
	if err := os.WriteFile(stale, []byte("keep me\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := NewManager(repo, root).Commit(ctx, wt1.Path, "keep", "test", "test@example.com"); err != nil {
		t.Fatalf("Commit: %v", err)
	}
	wantHead := gitRevParse(t, repo, "recur/legacy-ws")
	if err := NewManager(repo, root).Remove(ctx, wt1.Path); err != nil {
		t.Fatalf("Remove: %v", err)
	}

	wt2, _, err := NewManager(repo, root).WithBranchPrefix("josh").Restore(ctx, "legacy-ws", "", "main")
	if err != nil {
		t.Fatalf("Restore: %v", err)
	}
	if wt2.Branch != "recur/legacy-ws" {
		t.Errorf("Restore empty branch = %q, want recur/legacy-ws (not the live josh/ prefix)", wt2.Branch)
	}
	if got := gitRevParse(t, repo, wt2.Branch); got != wantHead {
		t.Errorf("Restore HEAD = %s, want leftover %s", got, wantHead)
	}
}

func TestRestoreUsesRecordedSuffixedBranch(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()
	if _, err := m.run(ctx, "branch", "recur/archived-ws"); err != nil {
		t.Fatalf("git branch leftover: %v", err)
	}

	wt1, _, err := m.Add(ctx, "archived-ws", "archived-ws", "main")
	if err != nil {
		t.Fatalf("Add: %v", err)
	}
	if wt1.Branch != "recur/archived-ws-2" {
		t.Fatalf("Add() branch = %q, want recur/archived-ws-2", wt1.Branch)
	}
	kept := filepath.Join(wt1.Path, "kept.txt")
	if err := os.WriteFile(kept, []byte("keep me\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := m.Commit(ctx, wt1.Path, "keep", "test", "test@example.com"); err != nil {
		t.Fatalf("Commit: %v", err)
	}
	wantHead := gitRevParse(t, repo, "recur/archived-ws-2")

	if err := m.Remove(ctx, wt1.Path); err != nil {
		t.Fatalf("Remove: %v", err)
	}

	wt2, _, err := m.Restore(ctx, "archived-ws", "recur/archived-ws-2", "main")
	if err != nil {
		t.Fatalf("Restore: %v", err)
	}
	if wt2.Branch != "recur/archived-ws-2" {
		t.Errorf("Restore recorded branch = %q, want recur/archived-ws-2", wt2.Branch)
	}
	if got := gitRevParse(t, repo, wt2.Branch); got != wantHead {
		t.Errorf("Restore HEAD = %s, want recorded %s", got, wantHead)
	}
	if _, err := os.Stat(filepath.Join(wt2.Path, "kept.txt")); err != nil {
		t.Fatalf("Restore should keep recorded-branch file: %v", err)
	}
}

func gitRevParse(t *testing.T, repo, ref string) string {
	t.Helper()
	cmd := exec.Command("git", "-C", repo, "rev-parse", ref)
	out, err := cmd.Output()
	if err != nil {
		t.Fatalf("git rev-parse %s: %v", ref, err)
	}
	return strings.TrimSpace(string(out))
}

func TestAddAtRefFastForwardsAncestorBranch(t *testing.T) {
	repo := initRepo(t)
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = repo
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("branch", "feat/oauth")
	if err := os.WriteFile(filepath.Join(repo, "fresh.txt"), []byte("new\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "fresh")
	fresh := gitRevParse(t, repo, "HEAD")
	behind := gitRevParse(t, repo, "feat/oauth")
	if behind == fresh {
		t.Fatal("setup: feat/oauth should be behind HEAD")
	}

	m := NewManager(repo, t.TempDir())
	wt, _, err := m.AddAtRef(context.Background(), "from-pr", "from-pr", "feat/oauth", fresh)
	if err != nil {
		t.Fatalf("AddAtRef: %v", err)
	}
	if wt.Branch != "feat/oauth" {
		t.Errorf("AddAtRef branch = %q, want feat/oauth", wt.Branch)
	}
	got := gitRevParse(t, wt.Path, "HEAD")
	if got != fresh {
		t.Errorf("AddAtRef(feat/oauth, startPoint=%s) HEAD = %q, want %q (behind was %q)", fresh, got, fresh, behind)
	}
}

func TestAddAtRefFallsBackWhenLocalDiverged(t *testing.T) {
	repo := initRepo(t)
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = repo
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("checkout", "-b", "feat/oauth")
	if err := os.WriteFile(filepath.Join(repo, "stale.txt"), []byte("old\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "stale")
	stale := gitRevParse(t, repo, "HEAD")
	run("checkout", "main")
	if err := os.WriteFile(filepath.Join(repo, "fresh.txt"), []byte("new\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "fresh")
	fresh := gitRevParse(t, repo, "HEAD")
	if stale == fresh {
		t.Fatal("setup: stale feat/oauth and fresh main should differ")
	}

	m := NewManager(repo, t.TempDir())
	wt, _, err := m.AddAtRef(context.Background(), "from-pr", "from-pr", "feat/oauth", fresh)
	if err != nil {
		t.Fatalf("AddAtRef: %v", err)
	}
	if wt.Branch == "feat/oauth" {
		t.Errorf("AddAtRef branch = %q, want unique fallback not feat/oauth", wt.Branch)
	}
	got := gitRevParse(t, wt.Path, "HEAD")
	if got != fresh {
		t.Errorf("AddAtRef fallback HEAD = %q, want %q", got, fresh)
	}
	kept := gitRevParse(t, repo, "feat/oauth")
	if kept != stale {
		t.Errorf("AddAtRef left feat/oauth = %q, want %q (diverged commits must stay reachable)", kept, stale)
	}
}

func TestAddAtRefFallsBackWhenBranchCheckedOut(t *testing.T) {
	repo := initRepo(t)
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = repo
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("checkout", "-b", "feat/oauth")
	if err := os.WriteFile(filepath.Join(repo, "head.txt"), []byte("pr\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "pr head")
	run("checkout", "main")

	m := NewManager(repo, t.TempDir())
	ctx := context.Background()
	if _, _, err := m.AddAtRef(ctx, "one", "one", "feat/oauth", "feat/oauth"); err != nil {
		t.Fatalf("AddAtRef first: %v", err)
	}
	wt2, _, err := m.AddAtRef(ctx, "two", "two", "feat/oauth", "feat/oauth")
	if err != nil {
		t.Fatalf("AddAtRef second: %v", err)
	}
	if wt2.Branch == "feat/oauth" {
		t.Errorf("AddAtRef second branch = %q, want unique fallback not feat/oauth", wt2.Branch)
	}
}

func TestAddRepairsMovedWorktree(t *testing.T) {
	repo := initRepo(t)
	oldRoot := t.TempDir()
	newRoot := t.TempDir()
	ctx := context.Background()

	oldMgr := NewManager(repo, oldRoot)
	wt, _, err := oldMgr.Add(ctx, "LOOP-moved", "moved-feat", "main")
	if err != nil {
		t.Fatalf("first Add: %v", err)
	}
	newPath := filepath.Join(newRoot, "LOOP-moved")
	if err := os.MkdirAll(newRoot, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Rename(wt.Path, newPath); err != nil {
		t.Fatalf("move worktree dir: %v", err)
	}

	newMgr := NewManager(repo, newRoot)
	reused, _, err := newMgr.Add(ctx, "LOOP-moved", "moved-feat", "main")
	if err != nil {
		t.Fatalf("Add after move: %v", err)
	}
	if cleanPath(reused.Path) != cleanPath(newPath) {
		t.Errorf("repaired path: got %q want %q", reused.Path, newPath)
	}
	if reused.Branch != "recur/moved-feat" {
		t.Errorf("branch: got %q want recur/moved-feat", reused.Branch)
	}
}

func TestRemoveMissingIsNoOp(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	if err := m.Remove(context.Background(), filepath.Join(root, "nope")); err != nil {
		t.Errorf("Remove of missing worktree should be a no-op, got: %v", err)
	}
}

func TestDeleteBranchIfAtBase(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()

	wt, _, err := m.Add(ctx, "empty-ws", "empty-ws", "main")
	if err != nil {
		t.Fatalf("Add: %v", err)
	}
	if err := m.Remove(ctx, wt.Path); err != nil {
		t.Fatalf("Remove: %v", err)
	}
	if err := m.DeleteBranchIfAtBase(ctx, wt.Branch, "main"); err != nil {
		t.Fatalf("DeleteBranchIfAtBase: %v", err)
	}
	if m.CommitExists(ctx, "recur/empty-ws") {
		t.Error("DeleteBranchIfAtBase(recur/empty-ws) left the branch, want it deleted")
	}

	wt2, _, err := m.Add(ctx, "kept-ws", "kept-ws", "main")
	if err != nil {
		t.Fatalf("Add kept: %v", err)
	}
	if err := os.WriteFile(filepath.Join(wt2.Path, "work.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := m.Commit(ctx, wt2.Path, "work", "test", "test@example.com"); err != nil {
		t.Fatalf("Commit: %v", err)
	}
	if err := m.Remove(ctx, wt2.Path); err != nil {
		t.Fatalf("Remove kept: %v", err)
	}
	if err := m.DeleteBranchIfAtBase(ctx, "recur/kept-ws", "main"); err != nil {
		t.Fatalf("DeleteBranchIfAtBase kept: %v", err)
	}
	if !m.CommitExists(ctx, "recur/kept-ws") {
		t.Error("DeleteBranchIfAtBase(recur/kept-ws) deleted a branch with unique commits")
	}
}

func TestDeleteLocalBranch(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()

	wt, _, err := m.Add(ctx, "del-ws", "del-ws", "main")
	if err != nil {
		t.Fatalf("Add: %v", err)
	}
	if err := os.WriteFile(filepath.Join(wt.Path, "work.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := m.Commit(ctx, wt.Path, "work", "test", "test@example.com"); err != nil {
		t.Fatalf("Commit: %v", err)
	}
	branch := wt.Branch
	if err := m.Remove(ctx, wt.Path); err != nil {
		t.Fatalf("Remove: %v", err)
	}
	if err := m.DeleteLocalBranch(ctx, branch); err != nil {
		t.Fatalf("DeleteLocalBranch: %v", err)
	}
	if m.CommitExists(ctx, branch) {
		t.Errorf("DeleteLocalBranch(%q) left the branch", branch)
	}
	if err := m.DeleteLocalBranch(ctx, "no-such-branch"); err != nil {
		t.Fatalf("DeleteLocalBranch missing: %v", err)
	}
}

func TestRemoveWipesOrphanDirectory(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()

	orphan := filepath.Join(root, "herbed-ciabatta")
	if err := os.MkdirAll(orphan, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(orphan, "stale.txt"), []byte("old\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	// Not a registered worktree — previously Remove returned nil and left the dir.
	if err := m.Remove(ctx, orphan); err != nil {
		t.Fatalf("Remove orphan: %v", err)
	}
	if _, err := os.Stat(orphan); !os.IsNotExist(err) {
		t.Fatalf("orphan dir still present after Remove: %v", err)
	}
}

func TestRemoveRefusesPathOutsideManagedRoot(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()

	outside := t.TempDir()
	marker := filepath.Join(outside, "keep.txt")
	if err := os.WriteFile(marker, []byte("safe\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := m.Remove(ctx, outside); err == nil {
		t.Fatal("Remove outside managed root must error")
	}
	if _, err := os.Stat(marker); err != nil {
		t.Fatalf("Remove must not touch paths outside root: %v", err)
	}
	if err := m.Remove(ctx, root); err == nil {
		t.Fatal("Remove of managed root itself must error")
	}
}

func TestRemoveLockedWorktree(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()

	wt, _, err := m.Add(ctx, "locked-ws", "locked-ws", "main")
	if err != nil {
		t.Fatalf("Add: %v", err)
	}
	lock := exec.Command("git", "-C", repo, "worktree", "lock", wt.Path)
	if out, err := lock.CombinedOutput(); err != nil {
		t.Fatalf("worktree lock: %v\n%s", err, out)
	}
	if err := m.Remove(ctx, wt.Path); err != nil {
		t.Fatalf("Remove locked worktree: %v", err)
	}
	if _, err := os.Stat(wt.Path); !os.IsNotExist(err) {
		t.Fatalf("locked worktree dir still present: %v", err)
	}
	if _, ok, err := m.worktreeAt(ctx, wt.Path); err != nil || ok {
		t.Fatalf("locked worktree still registered: ok=%v err=%v", ok, err)
	}

	// Recycled id must be able to allocate a fresh worktree.
	wt2, created, err := m.Add(ctx, "locked-ws", "locked-ws", "main")
	if err != nil {
		t.Fatalf("Add after locked remove: %v", err)
	}
	if !created {
		t.Fatal("expected fresh worktree after locked remove")
	}
	if _, err := os.Stat(filepath.Join(wt2.Path, "README.md")); err != nil {
		t.Fatalf("fresh worktree incomplete: %v", err)
	}
}

func TestAddWipesOrphanThenCreatesFresh(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()

	orphan := filepath.Join(root, "herbed-ciabatta")
	if err := os.MkdirAll(orphan, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(orphan, "stale.txt"), []byte("old\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	wt, created, err := m.Add(ctx, "herbed-ciabatta", "herbed-ciabatta", "main")
	if err != nil {
		t.Fatalf("Add over orphan: %v", err)
	}
	if !created {
		t.Fatal("Add over orphan should create a fresh worktree")
	}
	if _, err := os.Stat(filepath.Join(wt.Path, "stale.txt")); !os.IsNotExist(err) {
		t.Fatal("stale leftover file must not survive Add")
	}
	if _, err := os.Stat(filepath.Join(wt.Path, "README.md")); err != nil {
		t.Fatalf("fresh worktree missing README.md: %v", err)
	}
}

func TestDiscardRiskSummary(t *testing.T) {
	cases := []struct {
		r    DiscardRisk
		want string
	}{
		{DiscardRisk{}, ""},
		{DiscardRisk{DirtyCount: 1}, "1 uncommitted change"},
		{DiscardRisk{DirtyCount: 3}, "3 uncommitted changes"},
		{DiscardRisk{Unmerged: true}, "unmerged changes"},
		{DiscardRisk{Unmerged: true, DirtyCount: 2}, "unmerged + dirty"},
	}
	for _, tc := range cases {
		if got := tc.r.Summary(); got != tc.want {
			t.Errorf("%+v Summary=%q, want %q", tc.r, got, tc.want)
		}
	}
}

func TestInspectDiscardRisk(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()

	wt, _, err := m.Add(ctx, "risk-ws", "risk-ws", "main")
	if err != nil {
		t.Fatalf("Add: %v", err)
	}
	if got := InspectDiscardRisk(ctx, wt.Path).Summary(); got != "" {
		t.Fatalf("clean worktree Summary=%q, want empty", got)
	}

	if err := os.WriteFile(filepath.Join(wt.Path, "dirty.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	risk := InspectDiscardRisk(ctx, wt.Path)
	if risk.DirtyCount < 1 {
		t.Fatalf("DirtyCount=%d, want >= 1", risk.DirtyCount)
	}
	if risk.Summary() == "" {
		t.Fatal("expected non-empty Summary for dirty worktree")
	}
	if got := InspectDiscardRisk(ctx, filepath.Join(root, "missing")).Summary(); got != "" {
		t.Fatalf("missing path Summary=%q, want empty", got)
	}
}

func TestCommitStagesAndCountsDiff(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()

	wt, _, err := m.Add(ctx, "LOOP-C", "commit-test", "main")
	if err != nil {
		t.Fatalf("Add: %v", err)
	}

	// Nothing changed yet → Commit is a no-op.
	res, err := m.Commit(ctx, wt.Path, "noop", "Coder", "coder@recur.local")
	if err != nil {
		t.Fatalf("Commit (no changes): %v", err)
	}
	if res.Committed {
		t.Errorf("expected no commit when worktree is clean, got %+v", res)
	}

	// Write a new file, then commit — should land with a real hash + diff stat.
	if err := os.WriteFile(filepath.Join(wt.Path, "new.txt"), []byte("a\nb\nc\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	res, err = m.Commit(ctx, wt.Path, "recur(Coder): iteration 1 — add file", "Coder", "coder@recur.local")
	if err != nil {
		t.Fatalf("Commit: %v", err)
	}
	if !res.Committed || res.Hash == "" {
		t.Fatalf("expected a real commit, got %+v", res)
	}
	if res.Additions != 3 {
		t.Errorf("additions: got %d, want 3", res.Additions)
	}

	// Verify the commit exists with the encoded author + message.
	out, err := m.runIn(ctx, wt.Path, "log", "-1", "--pretty=%an|%s")
	if err != nil {
		t.Fatalf("git log: %v", err)
	}
	got := strings.TrimSpace(out)
	if want := "Coder|recur(Coder): iteration 1 — add file"; got != want {
		t.Errorf("commit metadata: got %q, want %q", got, want)
	}
}

func TestExpandHome(t *testing.T) {
	home, err := os.UserHomeDir()
	if err != nil {
		t.Skip("no home dir")
	}
	if got := ExpandHome("~/code/x"); got != filepath.Join(home, "code/x") {
		t.Errorf("ExpandHome(~/code/x) = %q", got)
	}
	if got := ExpandHome("/abs/path"); got != "/abs/path" {
		t.Errorf("ExpandHome should leave absolute paths: %q", got)
	}
}

func TestCommitsBehind(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := NewManager(repo, root)
	ctx := context.Background()

	wt, _, err := m.Add(ctx, "LOOP-BEHIND", "feature-behind", "main")
	if err != nil {
		t.Fatalf("Add: %v", err)
	}
	// Fresh off main — should be 0 behind and 0 ahead (HEAD is the merge-base).
	n, err := m.CommitsBehind(ctx, wt.Path, "main")
	if err != nil {
		t.Fatalf("CommitsBehind (fresh): %v", err)
	}
	if n != 0 {
		t.Errorf("CommitsBehind(fresh, main) = %d, want 0", n)
	}
	ahead, err := m.CommitsAhead(ctx, wt.Path, "main")
	if err != nil {
		t.Fatalf("CommitsAhead (fresh): %v", err)
	}
	if ahead != 0 {
		t.Errorf("CommitsAhead(fresh, main) = %d, want 0", ahead)
	}

	// Advance main with a new commit while the worktree stays put.
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = repo
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	if err := os.WriteFile(filepath.Join(repo, "extra.txt"), []byte("base moved\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", "extra.txt")
	run("commit", "-m", "advance main")

	n, err = m.CommitsBehind(ctx, wt.Path, "main")
	if err != nil {
		t.Fatalf("CommitsBehind (stale): %v", err)
	}
	if n != 1 {
		t.Errorf("CommitsBehind(stale, main) = %d, want 1", n)
	}
	ahead, err = m.CommitsAhead(ctx, wt.Path, "main")
	if err != nil {
		t.Fatalf("CommitsAhead (stale): %v", err)
	}
	if ahead != 0 {
		t.Errorf("CommitsAhead(stale, main) = %d, want 0", ahead)
	}

	if err := os.WriteFile(filepath.Join(wt.Path, "feature.txt"), []byte("own commit\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	res, err := m.Commit(ctx, wt.Path, "add feature", "Coder", "coder@recur.local")
	if err != nil {
		t.Fatalf("Commit: %v", err)
	}
	if !res.Committed {
		t.Fatalf("Commit() = %+v, want committed", res)
	}
	ahead, err = m.CommitsAhead(ctx, wt.Path, "main")
	if err != nil {
		t.Fatalf("CommitsAhead (own commit): %v", err)
	}
	if ahead != 1 {
		t.Errorf("CommitsAhead(own commit, main) = %d, want 1", ahead)
	}
}
