package app

import (
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"recur/internal/code"
	"recur/internal/domain"
	"recur/internal/engine"
	"recur/internal/store"
)

// TestOpenMainWorkspace covers the "main working copy" noun: a singleton
// workspace scoped to the project's REAL checkout (not a managed worktree). Its
// Observatory is writable and commits land in the checkout, but it never engages
// the autopilot, opens a PR, or has its worktree removed on delete.
func TestOpenMainWorkspace(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.OpenMainWorkspace(projectID)
	if err != nil {
		t.Fatalf("OpenMainWorkspace: %v", err)
	}
	if !ws.IsMain || ws.Type != domain.WorkspaceMain {
		t.Fatalf("main workspace markers wrong: isMain=%v type=%q", ws.IsMain, ws.Type)
	}
	if ws.HasRun() {
		t.Fatal("the main working copy must never report an engaged run")
	}
	// The worktree IS the project's real checkout (no managed worktree created).
	wantRoot, _ := filepath.Abs(repo)
	if got, _ := filepath.Abs(ws.WorktreePath); got != wantRoot {
		t.Fatalf("WorktreePath=%q, want project checkout %q", ws.WorktreePath, wantRoot)
	}
	if ws.BaseBranch != "" {
		t.Fatalf("BaseBranch=%q, want empty (uncommitted-vs-HEAD baseline)", ws.BaseBranch)
	}

	// Idempotent: a second open returns the SAME record (singleton per project).
	ws2, err := svc.Workspaces.OpenMainWorkspace(projectID)
	if err != nil {
		t.Fatalf("OpenMainWorkspace (2nd): %v", err)
	}
	if ws2.ID != ws.ID {
		t.Fatalf("second open created a new record: %q vs %q", ws2.ID, ws.ID)
	}
	mains := 0
	for _, w := range svc.Workspaces.ListWorkspaces() {
		if w.IsMain && w.ProjectID == projectID {
			mains++
		}
	}
	if mains != 1 {
		t.Fatalf("want exactly one main workspace, got %d", mains)
	}

	// Editing + committing works against the real checkout through the shared
	// CodeService (writable because WorktreePath resolves to a directory).
	if err := svc.Code.WriteFile(projectID, ws.ID, "main.txt", "hi\n"); err != nil {
		t.Fatalf("WriteFile into main working copy: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "main: add file"); err != nil || !res.Committed {
		t.Fatalf("commit in main working copy: res=%+v err=%v", res, err)
	}
}

// TestOpenMainWorkspaceConcurrentSingleton: concurrent opens (a sidebar
// double-click fires two RPCs on separate goroutines) must still yield exactly ONE
// main record — the find-or-create is serialized so the reuse scan can't race the
// add. A duplicate would be unrecoverable from the UI.
func TestOpenMainWorkspaceConcurrentSingleton(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	const n = 8
	ids := make([]string, n)
	errs := make([]error, n)
	var wg sync.WaitGroup
	wg.Add(n)
	for i := 0; i < n; i++ {
		go func(i int) {
			defer wg.Done()
			ws, err := svc.Workspaces.OpenMainWorkspace(projectID)
			ids[i], errs[i] = ws.ID, err
		}(i)
	}
	wg.Wait()

	for i, err := range errs {
		if err != nil {
			t.Fatalf("concurrent OpenMainWorkspace[%d]: %v", i, err)
		}
		if ids[i] != ids[0] {
			t.Fatalf("concurrent opens returned different records: %q vs %q", ids[i], ids[0])
		}
	}
	mains := 0
	for _, w := range svc.Workspaces.ListWorkspaces() {
		if w.IsMain && w.ProjectID == projectID {
			mains++
		}
	}
	if mains != 1 {
		t.Fatalf("want exactly one main workspace after concurrent opens, got %d", mains)
	}
}

// TestOpenMainWorkspaceRepairsMovedRepo: RepoPath is editable after creation, so a
// re-pointed project must not keep the main workspace targeting the OLD checkout.
// On reuse, OpenMainWorkspace repairs WorktreePath (and refreshes the branch) to
// the project's current checkout — otherwise edits/commits land in the wrong repo.
func TestOpenMainWorkspaceRepairsMovedRepo(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.OpenMainWorkspace(projectID)
	if err != nil {
		t.Fatalf("OpenMainWorkspace: %v", err)
	}

	// Re-point the project at a different checkout (as project settings would).
	repo2 := initGitRepo(t)
	proj, _ := eng.Store.Project(projectID)
	proj.RepoPath = repo2
	if _, err := eng.Store.UpdateProjectRunConfig(proj); err != nil {
		t.Fatalf("UpdateProjectRunConfig: %v", err)
	}

	ws2, err := svc.Workspaces.OpenMainWorkspace(projectID)
	if err != nil {
		t.Fatalf("OpenMainWorkspace (repointed): %v", err)
	}
	if ws2.ID != ws.ID {
		t.Fatalf("repair created a new record: %q vs %q", ws2.ID, ws.ID)
	}
	wantRoot, _ := filepath.Abs(repo2)
	if got, _ := filepath.Abs(ws2.WorktreePath); got != wantRoot {
		t.Fatalf("WorktreePath=%q, want repointed checkout %q", ws2.WorktreePath, wantRoot)
	}
	// The persisted record was repaired, not just the returned copy.
	if stored, _ := eng.Store.Workspace(ws.ID); func() bool {
		g, _ := filepath.Abs(stored.WorktreePath)
		return g != wantRoot
	}() {
		t.Fatalf("stored WorktreePath not repaired: %q", ws2.WorktreePath)
	}
}

// TestMainWorkspaceRefusesLoopAndPR: the autopilot and PR paths are refused on
// the main working copy (they would run against the user's real checkout).
func TestMainWorkspaceRefusesLoopAndPR(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.OpenMainWorkspace(projectID)
	if err != nil {
		t.Fatalf("OpenMainWorkspace: %v", err)
	}

	if _, err := eng.StartRunOnWorkspace(ws.ID, "do a thing"); err == nil {
		t.Error("StartRunOnWorkspace must be refused on the main working copy")
	}
	if _, err := svc.Workspaces.OpenWorkspacePR(ws.ID, false); err == nil {
		t.Error("OpenWorkspacePR must be refused on the main working copy")
	}
	if _, err := svc.Workspaces.MergeWorkspace(ws.ID, ""); err == nil {
		t.Error("MergeWorkspace must be refused on the main working copy")
	}
	if _, err := svc.Workspaces.UpdateWorkspaceWithBase(ws.ID, ""); err == nil {
		t.Error("UpdateWorkspaceWithBase must be refused on the main working copy")
	}
	if _, err := svc.Workspaces.RemoveWorkspace(ws.ID); err == nil {
		t.Error("RemoveWorkspace must be refused on the main working copy")
	}
}

// TestStagedFilesNonGitWorkingCopy: Observatory live-refresh calls StagedFiles
// after every save. On a plain-folder working copy that must return empty
// (no error) — never the `git diff --cached` / --no-index usage blast that
// used to toast as "Staged files failed".
func TestStagedFilesNonGitWorkingCopy(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "note.txt"), []byte("hi\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	p, err := eng.Store.AddProject(domain.Project{
		ID: "plain-scm", Name: "plain-scm", RepoPath: dir,
	})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := svc.Workspaces.OpenMainWorkspace(p.ID)
	if err != nil {
		t.Fatalf("OpenMainWorkspace: %v", err)
	}
	paths, err := svc.Code.StagedFiles(t.Context(), p.ID, ws.ID)
	if err != nil {
		t.Fatalf("StagedFiles: %v", err)
	}
	if len(paths) != 0 {
		t.Fatalf("StagedFiles = %v, want empty", paths)
	}
	st, err := svc.Code.WorkingStatus(t.Context(), p.ID, ws.ID)
	if err != nil {
		t.Fatalf("WorkingStatus: %v", err)
	}
	if len(st) != 0 {
		t.Fatalf("WorkingStatus = %v, want empty", st)
	}
}

// TestStagedFilesNonWritableRoot: switching workspaces (or landing on an
// eager-create row before git worktree add finishes) makes Observatory
// live-refresh call StagedFiles against a root that is not a loop worktree.
// That must return empty — never "staging is only allowed in a loop worktree",
// which Wails surfaces as a 422 RuntimeError toast.
func TestStagedFilesNonWritableRoot(t *testing.T) {
	repo := initGitRepo(t)
	// Stage something in the project checkout so a fallback read would be
	// visible — empty-list assertions must not pass by accident on a clean index.
	if err := os.WriteFile(filepath.Join(repo, "staged.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	add := exec.Command("git", "add", "staged.txt")
	add.Dir = repo
	if out, err := add.CombinedOutput(); err != nil {
		t.Fatalf("git add staged.txt: %v\n%s", err, out)
	}

	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	assertStagedEmpty := func(workspaceID string) {
		t.Helper()
		paths, err := svc.Code.StagedFiles(t.Context(), projectID, workspaceID)
		if err != nil {
			t.Fatalf("StagedFiles(%q, %q) error = %v, want nil", projectID, workspaceID, err)
		}
		if len(paths) != 0 {
			t.Fatalf("StagedFiles(%q, %q) = %v, want empty (must not read the project checkout index)", projectID, workspaceID, paths)
		}
	}

	// Sanity: the checkout really has a staged file, so empty answers below
	// are load-bearing (a fallback read would surface staged.txt).
	if gs, err := svc.Code.GitStatus(t.Context(), projectID, ""); err != nil {
		t.Fatalf("GitStatus(%q, %q) error = %v, want nil", projectID, "", err)
	} else if _, ok := gs["staged.txt"]; !ok {
		t.Fatalf("GitStatus(%q, %q) = %v, want staged.txt (setup)", projectID, "", gs)
	}

	// Project checkout with no workspace id — ResolveRootInfo reports !writable.
	assertStagedEmpty("")

	// Eager create persists the workspace before `git worktree add` finishes.
	const pendingID = "pending-wt"
	if _, err := eng.Store.AddWorkspace(domain.Workspace{
		ID: pendingID, ProjectID: projectID,
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	assertStagedEmpty(pendingID)

	// Recorded path, directory gone — must not fall back to the checkout.
	const staleID = "stale-wt"
	if _, err := eng.Store.AddWorkspace(domain.Workspace{
		ID: staleID, ProjectID: projectID,
		WorktreePath: filepath.Join(t.TempDir(), "gone"),
	}); err != nil {
		t.Fatalf("AddWorkspace stale: %v", err)
	}
	assertStagedEmpty(staleID)
	st, err := svc.Code.WorkingStatus(t.Context(), projectID, staleID)
	if err != nil {
		t.Fatalf("WorkingStatus(%q, %q) error = %v, want nil", projectID, staleID, err)
	}
	if len(st) != 0 {
		t.Fatalf("WorkingStatus(%q, %q) = %v, want empty", projectID, staleID, st)
	}
	wd, err := svc.Code.DiffWorking(t.Context(), projectID, staleID)
	if err != nil {
		t.Fatalf("DiffWorking(%q, %q) error = %v, want nil", projectID, staleID, err)
	}
	if len(wd.Files) != 0 {
		t.Fatalf("DiffWorking(%q, %q) = %v, want empty", projectID, staleID, wd)
	}
	gs, err := svc.Code.GitStatus(t.Context(), projectID, staleID)
	if err != nil {
		t.Fatalf("GitStatus(%q, %q) error = %v, want nil", projectID, staleID, err)
	}
	if len(gs) != 0 {
		t.Fatalf("GitStatus(%q, %q) = %v, want empty", projectID, staleID, gs)
	}
	if _, err := svc.Code.ListTree(t.Context(), projectID, staleID, ""); !errors.Is(err, code.ErrWorktreeMissing) {
		t.Fatalf("ListTree(%q, %q) error = %v, want ErrWorktreeMissing", projectID, staleID, err)
	}

	// Mutations still refuse the same root — only the read must be quiet.
	if err := svc.Code.Stage(t.Context(), projectID, pendingID, []string{"README.md"}); err == nil {
		t.Fatal("Stage on a non-writable root must still be refused")
	}
}

// TestAddProjectSeedsWorkingCopyFromCheckout: creating a project with a remote
// default (origin/main) must still label the working copy with HEAD (main).
func TestAddProjectSeedsWorkingCopyFromCheckout(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	p, err := svc.Projects.AddProject(domain.Project{
		Name: "app", RepoPath: repo, BaseBranch: "origin/main",
	})
	if err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	var main domain.Workspace
	for _, w := range eng.Store.Workspaces() {
		if w.IsMain && w.ProjectID == p.ID {
			main = w
			break
		}
	}
	if main.ID == "" {
		t.Fatal("expected a main working copy after AddProject")
	}
	if main.Branch != "main" {
		t.Fatalf("working copy Branch=%q, want checkout branch main (not project base %q)", main.Branch, p.BaseBranch)
	}
}

// TestOpenMainWorkspaceNonGitClearsBranch: a plain-folder project still gets a
// working copy (the directory itself), but must not surface a fake branch like
// "main" — CurrentBranch fails and Branch stays empty.
func TestOpenMainWorkspaceNonGitClearsBranch(t *testing.T) {
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
	got, err := svc.Projects.GetProject(p.ID)
	if err != nil {
		t.Fatalf("GetProject: %v", err)
	}
	if got.IsGit {
		t.Fatalf("plain folder project reported IsGit=true: %+v", got)
	}

	ws, err := svc.Workspaces.OpenMainWorkspace(p.ID)
	if err != nil {
		t.Fatalf("OpenMainWorkspace: %v", err)
	}
	if ws.Branch != "" {
		t.Fatalf("Branch=%q, want empty for non-git working copy", ws.Branch)
	}
	// Stale branch from an earlier mistaken write must clear on reopen.
	if _, ok := eng.Store.UpdateWorkspace(ws.ID, func(m *domain.Workspace) {
		m.Branch = "main"
	}); !ok {
		t.Fatal("UpdateWorkspace failed")
	}
	ws2, err := svc.Workspaces.OpenMainWorkspace(p.ID)
	if err != nil {
		t.Fatalf("OpenMainWorkspace (reopen): %v", err)
	}
	if ws2.Branch != "" {
		t.Fatalf("Branch=%q after reopen, want cleared", ws2.Branch)
	}
}

// TestDeleteMainWorkspaceKeepsCheckout: deleting the main workspace record must
// NOT remove the user's real checkout from disk (only the record/history go).
func TestDeleteMainWorkspaceKeepsCheckout(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.OpenMainWorkspace(projectID)
	if err != nil {
		t.Fatalf("OpenMainWorkspace: %v", err)
	}
	if err := svc.Workspaces.DeleteWorkspace(ws.ID); err != nil {
		t.Fatalf("DeleteWorkspace: %v", err)
	}
	// The checkout and its README survive (the repo dir was never touched).
	if _, err := os.ReadFile(filepath.Join(repo, "README.md")); err != nil {
		t.Fatalf("real checkout was damaged by deleting the main workspace: %v", err)
	}
	// A fresh open recreates the singleton.
	if _, err := svc.Workspaces.OpenMainWorkspace(projectID); err != nil {
		t.Fatalf("re-open after delete: %v", err)
	}
}

// TestMainWorkingCopyLandAndRemote covers titlebar Commit / Update / Push on
// the main working copy: dirty vs tracking remote, fetch+merge when origin
// advances, and push/pull of committed work even when leftover files are dirty.
func TestMainWorkingCopyLandAndRemote(t *testing.T) {
	clone, bare := initGitRepoWithRemote(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, clone)

	ws, err := svc.Workspaces.OpenMainWorkspace(projectID)
	if err != nil {
		t.Fatalf("OpenMainWorkspace: %v", err)
	}

	land := svc.Workspaces.WorkspaceLandState(ws.ID)
	if land.Dirty || land.Unmerged || land.AheadBase != 0 || land.BehindBase != 0 {
		t.Fatalf("clean tracking checkout: %+v", land)
	}
	if land.Remote != "origin" || land.Base != "origin/main" {
		t.Fatalf("tracking = remote=%q base=%q, want origin / origin/main", land.Remote, land.Base)
	}

	if err := os.WriteFile(filepath.Join(clone, "dirty.txt"), []byte("wip\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	land = svc.Workspaces.WorkspaceLandState(ws.ID)
	if !land.Dirty {
		t.Fatalf("WorkspaceLandState().Dirty = false, want true after uncommitted write")
	}
	if _, err := svc.Workspaces.PushWorkingCopy(ws.ID); err == nil {
		t.Error("PushWorkingCopy with nothing ahead should error")
	}

	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "main: dirty"); err != nil || !res.Committed {
		t.Fatalf("commit dirty.txt: res=%+v err=%v", res, err)
	}
	// Leftover untracked work must not block pushing the commits already on HEAD.
	if err := os.WriteFile(filepath.Join(clone, "leftover.txt"), []byte("still here\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	land = svc.Workspaces.WorkspaceLandState(ws.ID)
	if !land.Dirty || land.AheadBase < 1 {
		t.Fatalf("after selective commit: %+v, want dirty and ahead", land)
	}

	pushed, err := svc.Workspaces.PushWorkingCopy(ws.ID)
	if err != nil {
		t.Fatalf("PushWorkingCopy: %v", err)
	}
	if !pushed.IsMain {
		t.Fatalf("PushWorkingCopy returned non-main workspace")
	}
	land = svc.Workspaces.WorkspaceLandState(ws.ID)
	if land.AheadBase != 0 {
		t.Fatalf("ahead after push = %d, want 0", land.AheadBase)
	}
	if !land.Dirty {
		t.Fatal("leftover untracked file should still mark the checkout dirty after push")
	}
	if _, err := os.ReadFile(filepath.Join(clone, "leftover.txt")); err != nil {
		t.Fatalf("leftover.txt should remain in the working copy: %v", err)
	}
	out, gerr := exec.Command("git", "-C", bare, "show", "HEAD:dirty.txt").CombinedOutput()
	if gerr != nil || strings.TrimSpace(string(out)) != "wip" {
		t.Fatalf("bare missing pushed file: %v (%s)", gerr, out)
	}

	// Advance origin; FetchWorkingCopyUpstream should make behind visible.
	pusher := filepath.Join(t.TempDir(), "pusher")
	runGit(t, t.TempDir(), "clone", bare, pusher)
	runGit(t, pusher, "config", "user.name", "test")
	runGit(t, pusher, "config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(pusher, "from-origin.txt"), []byte("theirs\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit(t, pusher, "add", "from-origin.txt")
	runGit(t, pusher, "commit", "-m", "origin advance")
	runGit(t, pusher, "push", "origin", "main")

	// Local leftover work: untracked file + modified tracked file. Pull must
	// still land origin's commit and leave both dirty files in the tree.
	if err := os.WriteFile(filepath.Join(clone, "README.md"), []byte("hello\nlocal edit\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	land = svc.Workspaces.FetchWorkingCopyUpstream(ws.ID)
	if land.BehindBase < 1 {
		t.Fatalf("FetchWorkingCopyUpstream behind = %d, want >= 1", land.BehindBase)
	}
	if !land.Dirty {
		t.Fatal("want dirty checkout before update-from-remote")
	}

	res, err := svc.Workspaces.UpdateWorkingCopyFromRemote(ws.ID, "")
	if err != nil {
		t.Fatalf("UpdateWorkingCopyFromRemote: %v", err)
	}
	if res.Status != domain.UpdateWorkspaceUpdated {
		t.Fatalf("update status = %q, want %q", res.Status, domain.UpdateWorkspaceUpdated)
	}
	body, err := os.ReadFile(filepath.Join(clone, "from-origin.txt"))
	if err != nil || strings.TrimSpace(string(body)) != "theirs" {
		t.Fatalf("checkout missing origin file: %v %q", err, body)
	}
	readme, err := os.ReadFile(filepath.Join(clone, "README.md"))
	if err != nil || strings.TrimSpace(string(readme)) != "hello\nlocal edit" {
		t.Fatalf("local README edit not preserved: %v %q", err, readme)
	}
	if _, err := os.ReadFile(filepath.Join(clone, "leftover.txt")); err != nil {
		t.Fatalf("leftover.txt should remain after update: %v", err)
	}
	land = svc.Workspaces.WorkspaceLandState(ws.ID)
	if land.BehindBase != 0 {
		t.Fatalf("behind after update = %d, want 0", land.BehindBase)
	}
	if !land.Dirty {
		t.Fatal("leftover dirty files should still mark the checkout dirty after update")
	}

	noop, err := svc.Workspaces.UpdateWorkingCopyFromRemote(ws.ID, "")
	if err != nil {
		t.Fatalf("update noop: %v", err)
	}
	if noop.Status != domain.UpdateWorkspaceNoop {
		t.Errorf("update noop status = %q, want %q", noop.Status, domain.UpdateWorkspaceNoop)
	}
}

func TestMainWorkingCopyUpdateConflictAborts(t *testing.T) {
	clone, bare := initGitRepoWithRemote(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, clone)

	ws, err := svc.Workspaces.OpenMainWorkspace(projectID)
	if err != nil {
		t.Fatalf("OpenMainWorkspace: %v", err)
	}

	if err := os.WriteFile(filepath.Join(clone, "README.md"), []byte("ours\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "local readme"); err != nil || !res.Committed {
		t.Fatalf("commit local readme: res=%+v err=%v", res, err)
	}
	if err := os.WriteFile(filepath.Join(clone, "notes.txt"), []byte("wip notes\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	pusher := filepath.Join(t.TempDir(), "pusher")
	runGit(t, t.TempDir(), "clone", bare, pusher)
	runGit(t, pusher, "config", "user.name", "test")
	runGit(t, pusher, "config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(pusher, "README.md"), []byte("theirs\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit(t, pusher, "add", "README.md")
	runGit(t, pusher, "commit", "-m", "origin readme")
	runGit(t, pusher, "push", "origin", "main")

	_, err = svc.Workspaces.UpdateWorkingCopyFromRemote(ws.ID, "")
	if err == nil {
		t.Fatal("UpdateWorkingCopyFromRemote err=nil, want conflict")
	}
	if !strings.Contains(err.Error(), "left unchanged") {
		t.Errorf("error = %q, want leftover-unchanged message", err)
	}

	readme, rerr := os.ReadFile(filepath.Join(clone, "README.md"))
	if rerr != nil || strings.TrimSpace(string(readme)) != "ours" {
		t.Fatalf("README after conflict = %q (%v), want ours (pre-update)", readme, rerr)
	}
	notes, nerr := os.ReadFile(filepath.Join(clone, "notes.txt"))
	if nerr != nil || strings.TrimSpace(string(notes)) != "wip notes" {
		t.Fatalf("notes.txt after conflict = %q (%v), want leftover dirty file", notes, nerr)
	}
	land := svc.Workspaces.WorkspaceLandState(ws.ID)
	if land.Unmerged {
		t.Fatal("checkout left in a merge — want abort back to the previous state")
	}
}

func TestMainWorkingCopyUpdateRefusesInProgress(t *testing.T) {
	clone, bare := initGitRepoWithRemote(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, clone)

	ws, err := svc.Workspaces.OpenMainWorkspace(projectID)
	if err != nil {
		t.Fatalf("OpenMainWorkspace: %v", err)
	}

	if err := os.WriteFile(filepath.Join(clone, "README.md"), []byte("ours\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "local readme"); err != nil || !res.Committed {
		t.Fatalf("commit local readme: res=%+v err=%v", res, err)
	}

	pusher := filepath.Join(t.TempDir(), "pusher")
	runGit(t, t.TempDir(), "clone", bare, pusher)
	runGit(t, pusher, "config", "user.name", "test")
	runGit(t, pusher, "config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(pusher, "README.md"), []byte("theirs\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit(t, pusher, "add", "README.md")
	runGit(t, pusher, "commit", "-m", "origin readme")
	runGit(t, pusher, "push", "origin", "main")

	runGit(t, clone, "fetch", "origin")
	cmd := exec.Command("git", "merge", "origin/main")
	cmd.Dir = clone
	cmd.Env = append(os.Environ(),
		"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
		"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
	)
	if out, merr := cmd.CombinedOutput(); merr == nil {
		t.Fatalf("expected a conflicting git merge, got success: %s", out)
	}
	if err := os.WriteFile(filepath.Join(clone, "README.md"), []byte("partial resolution\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	_, err = svc.Workspaces.UpdateWorkingCopyFromRemote(ws.ID, "")
	if err == nil {
		t.Fatal("UpdateWorkingCopyFromRemote err=nil, want in-progress refusal")
	}
	if !strings.Contains(err.Error(), "already in progress") {
		t.Errorf("error = %q, want already-in-progress message", err)
	}

	readme, rerr := os.ReadFile(filepath.Join(clone, "README.md"))
	if rerr != nil || strings.TrimSpace(string(readme)) != "partial resolution" {
		t.Fatalf("README after refused update = %q (%v), want preserved partial resolution", readme, rerr)
	}
	land := svc.Workspaces.WorkspaceLandState(ws.ID)
	if !land.Unmerged {
		t.Fatal("in-progress merge should still be unmerged after refuse")
	}
}

func TestMainWorkingCopyAutostashLeftInStash(t *testing.T) {
	clone, bare := initGitRepoWithRemote(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, clone)

	ws, err := svc.Workspaces.OpenMainWorkspace(projectID)
	if err != nil {
		t.Fatalf("OpenMainWorkspace: %v", err)
	}

	if err := os.WriteFile(filepath.Join(clone, "README.md"), []byte("hello\nlocal\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	pusher := filepath.Join(t.TempDir(), "pusher")
	runGit(t, t.TempDir(), "clone", bare, pusher)
	runGit(t, pusher, "config", "user.name", "test")
	runGit(t, pusher, "config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(pusher, "README.md"), []byte("hello\nremote\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit(t, pusher, "add", "README.md")
	runGit(t, pusher, "commit", "-m", "origin readme")
	runGit(t, pusher, "push", "origin", "main")

	res, err := svc.Workspaces.UpdateWorkingCopyFromRemote(ws.ID, "")
	if err != nil {
		t.Fatalf("UpdateWorkingCopyFromRemote: %v", err)
	}
	if res.Status != domain.UpdateWorkspaceAutostash {
		t.Fatalf("update status = %q, want %q", res.Status, domain.UpdateWorkspaceAutostash)
	}
	readme, rerr := os.ReadFile(filepath.Join(clone, "README.md"))
	if rerr != nil || strings.TrimSpace(string(readme)) != "hello\nremote" {
		t.Fatalf("README after autostash update = %q (%v), want origin version (no stash-apply markers)", readme, rerr)
	}
	stash, serr := exec.Command("git", "-C", clone, "stash", "list").CombinedOutput()
	if serr != nil || !strings.Contains(strings.ToLower(string(stash)), "autostash") {
		t.Fatalf("stash list = %q (%v), want an autostash entry", stash, serr)
	}
	land := svc.Workspaces.WorkspaceLandState(ws.ID)
	if land.Unmerged {
		t.Fatal("stash-apply conflict markers should have been reset off the working tree")
	}
}

func TestMainWorkingCopyRemoteRefusedOnLoop(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "not main", "", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	waitWorkspaceReady(t, eng, ws.ID)
	if _, err := svc.Workspaces.UpdateWorkingCopyFromRemote(ws.ID, ""); err == nil {
		t.Error("UpdateWorkingCopyFromRemote on a non-main workspace should error")
	}
	if _, err := svc.Workspaces.PushWorkingCopy(ws.ID); err == nil {
		t.Error("PushWorkingCopy on a non-main workspace should error")
	}
}

func TestMainWorkingCopyLandNoRemote(t *testing.T) {
	repo := initGitRepo(t)
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.OpenMainWorkspace(projectID)
	if err != nil {
		t.Fatalf("OpenMainWorkspace: %v", err)
	}
	land := svc.Workspaces.WorkspaceLandState(ws.ID)
	if land.Remote != "" || land.Base != "" {
		t.Fatalf("no-remote land = %+v, want empty remote/base", land)
	}
	if land.Dirty {
		t.Fatalf("fresh repo should be clean: %+v", land)
	}
}

func initGitRepoWithRemote(t *testing.T) (clone, bare string) {
	t.Helper()
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	bare = filepath.Join(t.TempDir(), "upstream.git")
	seed := t.TempDir()
	runGit(t, seed, "init", "-b", "main")
	runGit(t, seed, "config", "user.name", "test")
	runGit(t, seed, "config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(seed, "README.md"), []byte("hello\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit(t, seed, "add", ".")
	runGit(t, seed, "commit", "-m", "initial")
	runGit(t, seed, "clone", "--bare", seed, bare)
	clone = filepath.Join(t.TempDir(), "work")
	runGit(t, t.TempDir(), "clone", bare, clone)
	runGit(t, clone, "config", "user.name", "test")
	runGit(t, clone, "config", "user.email", "test@example.com")
	return clone, bare
}

func runGit(t *testing.T, dir string, args ...string) {
	t.Helper()
	cmd := exec.Command("git", args...)
	cmd.Dir = dir
	cmd.Env = append(os.Environ(),
		"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
		"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
	)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git %v (in %s): %v\n%s", args, dir, err, out)
	}
}
