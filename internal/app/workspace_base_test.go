package app

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"recur/internal/engine"
	"recur/internal/store"
)

func gitIn(t *testing.T, dir string, args ...string) {
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

func TestSetWorkspaceBaseBranch(t *testing.T) {
	repo := initGitRepo(t)
	gitIn(t, repo, "branch", "develop")
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "retarget me", "main", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if ws.BaseBranch != "main" {
		t.Fatalf("CreateWorkspace BaseBranch = %q, want main", ws.BaseBranch)
	}

	got, err := svc.Workspaces.SetWorkspaceBaseBranch(ws.ID, "develop")
	if err != nil {
		t.Fatalf("SetWorkspaceBaseBranch(develop): %v", err)
	}
	if got.BaseBranch != "develop" {
		t.Fatalf("SetWorkspaceBaseBranch(develop) BaseBranch = %q, want develop", got.BaseBranch)
	}
	fresh, ok := eng.Store.Workspace(ws.ID)
	if !ok || fresh.BaseBranch != "develop" {
		t.Fatalf("stored BaseBranch = %q, want develop", fresh.BaseBranch)
	}

	// Same branch is a no-op success.
	again, err := svc.Workspaces.SetWorkspaceBaseBranch(ws.ID, "develop")
	if err != nil {
		t.Fatalf("SetWorkspaceBaseBranch(same): %v", err)
	}
	if again.BaseBranch != "develop" {
		t.Fatalf("SetWorkspaceBaseBranch(same) BaseBranch = %q, want develop", again.BaseBranch)
	}
}

func TestSetWorkspaceBaseBranchRefuses(t *testing.T) {
	repo := initGitRepo(t)
	gitIn(t, repo, "branch", "develop")
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "refusals", "main", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)

	cases := []struct {
		name, id, base string
	}{
		{"empty", ws.ID, ""},
		{"whitespace", ws.ID, "  "},
		{"missing workspace", "no-such", "develop"},
		{"unknown branch", ws.ID, "does-not-exist"},
		{"own branch", ws.ID, ws.Branch},
		{"HEAD", ws.ID, "HEAD"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, err := svc.Workspaces.SetWorkspaceBaseBranch(tc.id, tc.base)
			if err == nil {
				t.Fatalf("SetWorkspaceBaseBranch(%q, %q) error = nil, want error", tc.id, tc.base)
			}
		})
	}

	mainWS, err := svc.Workspaces.OpenMainWorkspace(projectID)
	if err != nil {
		t.Fatalf("OpenMainWorkspace: %v", err)
	}
	if _, err := svc.Workspaces.SetWorkspaceBaseBranch(mainWS.ID, "develop"); err == nil {
		t.Fatal("SetWorkspaceBaseBranch(main workspace) error = nil, want error")
	}

	gitIn(t, repo, "tag", "v1.0")
	if _, err := svc.Workspaces.SetWorkspaceBaseBranch(ws.ID, "v1.0"); err == nil {
		t.Fatal("SetWorkspaceBaseBranch(tag) error = nil, want error")
	}
	shaCmd := exec.Command("git", "rev-parse", "HEAD")
	shaCmd.Dir = repo
	shaOut, err := shaCmd.Output()
	if err != nil {
		t.Fatalf("rev-parse HEAD: %v", err)
	}
	sha := strings.TrimSpace(string(shaOut))
	if _, err := svc.Workspaces.SetWorkspaceBaseBranch(ws.ID, sha); err == nil {
		t.Fatalf("SetWorkspaceBaseBranch(SHA %s) error = nil, want error", sha)
	}
}

func TestSetWorkspaceBaseBranchRetargetsOpenPR(t *testing.T) {
	repo := initGitRepo(t)
	gitIn(t, repo, "branch", "develop")
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "open a pr", "main", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if err := svc.Code.WriteFile(projectID, ws.ID, "feature.txt", "the feature\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}
	prID, err := svc.Workspaces.OpenWorkspacePR(ws.ID, false)
	if err != nil {
		t.Fatalf("OpenWorkspacePR: %v", err)
	}
	pr, err := svc.PRs.GetPR(prID)
	if err != nil {
		t.Fatalf("GetPR: %v", err)
	}
	if pr.Base != "main" {
		t.Fatalf("PR.Base = %q, want main", pr.Base)
	}

	if _, err := svc.Workspaces.SetWorkspaceBaseBranch(ws.ID, "develop"); err != nil {
		t.Fatalf("SetWorkspaceBaseBranch: %v", err)
	}
	pr, err = svc.PRs.GetPR(prID)
	if err != nil {
		t.Fatalf("GetPR after retarget: %v", err)
	}
	if pr.Base != "develop" {
		t.Fatalf("PR.Base after retarget = %q, want develop", pr.Base)
	}
}

func TestSetWorkspaceBaseBranchLeavesMergedPR(t *testing.T) {
	repo := initGitRepo(t)
	gitIn(t, repo, "branch", "develop")
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "merged pr", "main", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	if err := svc.Code.WriteFile(projectID, ws.ID, "feature.txt", "the feature\n"); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if res, err := svc.Code.Commit(t.Context(), projectID, ws.ID, "solo: add feature"); err != nil || !res.Committed {
		t.Fatalf("Commit: res=%+v err=%v", res, err)
	}
	prID, err := svc.Workspaces.OpenWorkspacePR(ws.ID, false)
	if err != nil {
		t.Fatalf("OpenWorkspacePR: %v", err)
	}
	if _, err := svc.PRs.MergePR(prID, ""); err != nil {
		t.Fatalf("MergePR: %v", err)
	}

	if _, err := svc.Workspaces.SetWorkspaceBaseBranch(ws.ID, "develop"); err != nil {
		t.Fatalf("SetWorkspaceBaseBranch after merge: %v", err)
	}
	pr, err := svc.PRs.GetPR(prID)
	if err != nil {
		t.Fatalf("GetPR: %v", err)
	}
	if pr.Base != "main" {
		t.Fatalf("merged PR.Base = %q, want main (historical)", pr.Base)
	}
	fresh, _ := eng.Store.Workspace(ws.ID)
	if fresh.BaseBranch != "develop" {
		t.Fatalf("workspace BaseBranch = %q, want develop", fresh.BaseBranch)
	}
}

func TestSetWorkspaceBaseBranchTrims(t *testing.T) {
	repo := initGitRepo(t)
	gitIn(t, repo, "branch", "develop")
	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, repo)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "trim", "main", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	got, err := svc.Workspaces.SetWorkspaceBaseBranch(ws.ID, "  develop  ")
	if err != nil {
		t.Fatalf("SetWorkspaceBaseBranch(padded): %v", err)
	}
	if got.BaseBranch != "develop" {
		t.Fatalf("SetWorkspaceBaseBranch(padded) BaseBranch = %q, want develop", got.BaseBranch)
	}
	if strings.TrimSpace(got.BaseBranch) != got.BaseBranch {
		t.Fatalf("BaseBranch still has surrounding space: %q", got.BaseBranch)
	}
}

func TestSetWorkspaceBaseBranchAcceptsRemoteTracking(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	bare := filepath.Join(t.TempDir(), "upstream.git")
	seed := t.TempDir()
	gitIn(t, seed, "init", "-b", "main")
	if err := os.WriteFile(filepath.Join(seed, "README.md"), []byte("hi\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitIn(t, seed, "add", ".")
	gitIn(t, seed, "commit", "-m", "initial")
	gitIn(t, seed, "clone", "--bare", seed, bare)
	clone := filepath.Join(t.TempDir(), "work")
	gitIn(t, t.TempDir(), "clone", bare, clone)

	eng := engine.New(store.NewInMemory())
	eng.SetWorktreeRoot(t.TempDir())
	eng.SetRunner(&engine.ScriptedRunner{})
	svc := NewServices(eng)
	projectID := svcAddProject(t, eng, clone)

	ws, err := svc.Workspaces.CreateWorkspace(projectID, "remote base", "main", "agent", "", "", nil)
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	ws = waitWorkspaceReady(t, eng, ws.ID)
	got, err := svc.Workspaces.SetWorkspaceBaseBranch(ws.ID, "origin/main")
	if err != nil {
		t.Fatalf("SetWorkspaceBaseBranch(origin/main): %v", err)
	}
	if got.BaseBranch != "origin/main" {
		t.Fatalf("BaseBranch = %q, want origin/main", got.BaseBranch)
	}
}
