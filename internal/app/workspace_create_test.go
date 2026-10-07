package app

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"recur/internal/domain"
	"recur/internal/engine"
	gitpkg "recur/internal/git"
	"recur/internal/store"
)

// TestCreateSoloWorkspace covers the unified model: a Solo workspace is a Workspace
// of Type=solo with a worktree but NO autopilot — it appears in the workspace list
// and its Observatory is writable (edit + commit) through the same CodeService the
// loop worktrees use, with zero agents.
func TestCreateSoloWorkspace(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "tidy the header", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	if ws.Type != domain.WorkspaceSolo || ws.Branch == "" {
		t.Fatalf("solo workspace fields wrong: %+v", ws)
	}
	if ws.WorktreePath != "" {
		t.Fatal("CreateWorkspace must return before the worktree exists")
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if ws.WorktreePath == "" {
		t.Fatal("expected worktree path after allocate")
	}
	// It shows up in the workspace list (so the sidebar renders it).
	found := false
	for _, w := range svc.Workspaces.ListWorkspaces() {
		if w.ID == ws.ID {
			found = true
		}
	}
	if !found {
		t.Error("solo workspace missing from ListWorkspaces (sidebar would not show it)")
	}

	// Newest-first: a later create must lead the list (sidebar / Go to palette).
	newer, err := svc.Workspaces.CreateWorkspace(projectID, "after", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace 2: %v", err)
	}
	list := svc.Workspaces.ListWorkspaces()
	if len(list) < 2 || list[0].ID != newer.ID {
		t.Fatalf("ListWorkspaces newest-first: first=%q, want %q", list[0].ID, newer.ID)
	}

	// Its Observatory is writable via the shared CodeService (workspace id in the
	// scope slot) — the bare project checkout is not.
	if err := svc.Code.WriteFile(projectID, "", "solo.txt", "hi\n"); err == nil {
		t.Error("bare project checkout must not be writable")
	}
	if err := svc.Code.WriteFile(projectID, ws.ID, "solo.txt", "hi\n"); err != nil {
		t.Fatalf("WriteFile into solo workspace: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "solo: add file"); err != nil || !res.Committed {
		t.Fatalf("commit in solo workspace: res=%+v err=%v", res, err)
	}
}

func TestGenerateCommitMessageRequiresStaged(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "scm ai", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if _, err := svc.Code.GenerateCommitMessage(t.Context(), projectID, ws.ID); err == nil {
		t.Fatal("expected error when nothing is staged")
	}
	if err := svc.Code.WriteFile(projectID, ws.ID, "msg.txt", "x\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	// Unstaged only — still refused.
	if _, err := svc.Code.GenerateCommitMessage(t.Context(), projectID, ws.ID); err == nil {
		t.Fatal("expected error when changes are unstaged")
	}
}

// TestCreateWorkspaceRefusesNonGit: plain-folder projects can open a working copy
// but cannot allocate worktrees (git worktree add would fail with a git error).
func TestCreateWorkspaceRefusesNonGit(t *testing.T) {
	dir := t.TempDir()
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	p, err := eng.Store.AddProject(domain.Project{
		ID: "plain", Name: "plain", RepoPath: dir, BaseBranch: "main",
	})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := svc.Workspaces.CreateWorkspace(p.ID, "nope", "main", "agent", "", "", nil); err == nil {
		t.Fatal("CreateWorkspace must refuse a non-git project")
	}
	if _, err := svc.Workspaces.CreateWorkspace(p.ID, "nope", "main", "loop", "do a thing", "balanced", nil); err == nil {
		t.Fatal("StartWorkspace (via mode=loop) must refuse a non-git project")
	}
	branches, err := svc.Projects.ListBranches(p.ID)
	if err != nil {
		t.Fatalf("ListBranches: %v", err)
	}
	if len(branches) != 0 {
		t.Fatalf("ListBranches for non-git = %v, want empty", branches)
	}
}

// TestListBranchesForPathNoPhantomPreferred: empty/stale preferred must not
// invent a selectable "main" (or prior-repo origin/main) that doesn't exist.
func TestListBranchesForPathNoPhantomPreferred(t *testing.T) {
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
	run("init", "-b", "master")
	if err := os.WriteFile(filepath.Join(dir, "README.md"), []byte("hi\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "initial")

	svc := NewServices(engine.New(store.NewInMemory()))
	for _, preferred := range []string{"", "main", "origin/main"} {
		choices, err := svc.Projects.ListBranchesForPath(dir, preferred)
		if err != nil {
			t.Fatalf("ListBranchesForPath(%q): %v", preferred, err)
		}
		for _, c := range choices {
			if c.Name == "main" || c.Name == "origin/main" {
				t.Fatalf("preferred %q injected phantom %q into %v", preferred, c.Name, choices)
			}
		}
		if len(choices) == 0 || choices[0].Name != "master" {
			t.Fatalf("preferred %q: want master listed, got %v", preferred, choices)
		}
	}
}

// TestListBranchesForPathWorksWithoutProjectID: new-project form can list
// remotes/locals before AddProject.
func TestListBranchesForPathWorksWithoutProjectID(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	bare := filepath.Join(t.TempDir(), "upstream.git")
	seed := t.TempDir()
	run := func(dir string, args ...string) {
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
	run(seed, "init", "-b", "main")
	if err := os.WriteFile(filepath.Join(seed, "README.md"), []byte("hi\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(seed, "add", ".")
	run(seed, "commit", "-m", "initial")
	run(seed, "branch", "develop")
	run(seed, "clone", "--bare", seed, bare)
	clone := filepath.Join(t.TempDir(), "work")
	run(t.TempDir(), "clone", bare, clone)

	svc := NewServices(engine.New(store.NewInMemory()))
	choices, err := svc.Projects.ListBranchesForPath(clone, "origin/main")
	if err != nil {
		t.Fatalf("ListBranchesForPath: %v", err)
	}
	var sawDevelop, sawOrigin, sawLocalMain bool
	var lastGroup string
	for _, c := range choices {
		if lastGroup != "" && c.Group != lastGroup && lastGroup == gitpkg.LocalBranchGroup {
			t.Fatalf("Local group must be last; got %v after Local", c)
		}
		lastGroup = c.Group
		if c.Name == "origin/develop" || c.Name == "develop" {
			sawDevelop = true
		}
		if c.Name == "origin/main" {
			sawOrigin = true
		}
		if c.Name == "main" && c.Group == gitpkg.LocalBranchGroup {
			sawLocalMain = true
		}
	}
	if !sawDevelop || !sawOrigin || !sawLocalMain {
		t.Fatalf("expected origin/develop + origin/main + local main, got %v", choices)
	}
}

// TestListBranchesMissingDefaultStaysInGroup: a preferred name that isn't a
// real ref is folded into Local without prepending a second Local heading.
func TestListBranchesMissingDefaultStaysInGroup(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	p, err := eng.Store.AddProject(domain.Project{
		ID: "missing-def", Name: "missing-def", RepoPath: repo, BaseBranch: "does-not-exist",
	})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	choices, err := svc.Projects.ListBranches(p.ID)
	if err != nil {
		t.Fatalf("ListBranches: %v", err)
	}
	var localRuns int
	prev := ""
	for _, c := range choices {
		if c.Group == gitpkg.LocalBranchGroup && prev != gitpkg.LocalBranchGroup {
			localRuns++
		}
		prev = c.Group
	}
	if localRuns != 1 {
		t.Fatalf("want one contiguous Local group, got %d runs in %v", localRuns, choices)
	}
	if choices[0].Name == "does-not-exist" && choices[0].Group == gitpkg.LocalBranchGroup {
		// OK if Local is the only group and preferred is first within it
	} else {
		found := false
		for _, c := range choices {
			if c.Name == "does-not-exist" && c.Group == gitpkg.LocalBranchGroup {
				found = true
				break
			}
		}
		if !found {
			t.Fatalf("missing default not in Local group: %v", choices)
		}
	}
}

// TestAddProjectSuggestsOriginMain: empty BaseBranch on create prefers origin/main.
func TestAddProjectSuggestsOriginMain(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	bare := filepath.Join(t.TempDir(), "upstream.git")
	seed := t.TempDir()
	run := func(dir string, args ...string) {
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
	run(seed, "init", "-b", "main")
	if err := os.WriteFile(filepath.Join(seed, "README.md"), []byte("hi\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(seed, "add", ".")
	run(seed, "commit", "-m", "initial")
	run(seed, "clone", "--bare", seed, bare)
	clone := filepath.Join(t.TempDir(), "work")
	run(t.TempDir(), "clone", bare, clone)

	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	p, err := svc.Projects.AddProject(domain.Project{
		Name: "suggest-base", RepoPath: clone, // BaseBranch intentionally empty
	})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if p.BaseBranch != "origin/main" {
		t.Fatalf("BaseBranch = %q, want origin/main", p.BaseBranch)
	}
}

// TestListBranchesGroupsRemotesFirst: picker lists remote-tracking refs (grouped
// by remote name) before local heads.
func TestListBranchesGroupsRemotesFirst(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	bare := filepath.Join(t.TempDir(), "upstream.git")
	seed := t.TempDir()
	run := func(dir string, args ...string) {
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
	run(seed, "init", "-b", "main")
	if err := os.WriteFile(filepath.Join(seed, "README.md"), []byte("hi\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(seed, "add", ".")
	run(seed, "commit", "-m", "initial")
	run(seed, "branch", "feature/x")
	run(seed, "clone", "--bare", seed, bare)
	clone := filepath.Join(t.TempDir(), "work")
	run(t.TempDir(), "clone", bare, clone)

	eng := engine.New(store.NewInMemory())
	svc := NewServices(eng)
	p, err := eng.Store.AddProject(domain.Project{
		ID: "remote-base", Name: "remote-base", RepoPath: clone, BaseBranch: "origin/main",
	})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	choices, err := svc.Projects.ListBranches(p.ID)
	if err != nil {
		t.Fatalf("ListBranches: %v", err)
	}
	if len(choices) < 2 {
		t.Fatalf("expected remote + local choices, got %v", choices)
	}
	if choices[0].Name != "origin/main" || choices[0].Group != "origin" {
		t.Fatalf("first choice = %+v, want origin/main in group origin", choices[0])
	}
	var sawLocal bool
	localIdx := -1
	for i, c := range choices {
		if c.Group == gitpkg.LocalBranchGroup {
			sawLocal = true
			if localIdx < 0 {
				localIdx = i
			}
		}
		if sawLocal && c.Group != gitpkg.LocalBranchGroup {
			t.Fatalf("remote group %q appeared after Local at index %d: %v", c.Group, i, choices)
		}
	}
	if !sawLocal || localIdx == 0 {
		t.Fatalf("expected Local group after remotes, got %v", choices)
	}
}

// TestCreateSoloWorkspaceDefaultTitle: an empty name becomes "New Workspace" so the
// sidebar has a placeholder until the agent names it via MCP.
func TestCreateSoloWorkspaceDefaultTitle(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	if ws.Goal != "New Workspace" || ws.Short != "New Workspace" {
		t.Fatalf("default title: goal=%q short=%q, want New Workspace", ws.Goal, ws.Short)
	}
	if ws.HasRun() {
		t.Fatal("a hand-driven workspace must not report an engaged run")
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if ws.StatusLabel != "Idle" {
		t.Fatalf("statusLabel=%q, want Idle (no run engaged; not the title)", ws.StatusLabel)
	}
}
