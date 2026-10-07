package engine

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

// TestSyncWorkspaceBranchAfterRename covers the live-branch source of truth:
// after `git branch -m` in the worktree, syncWorkspaceBranch updates the
// cached Workspace.Branch, an open Local PR.Branch, and emits workspace.updated
// with the new name — while leaving a GitHub PR.Branch frozen.
func TestSyncWorkspaceBranchAfterRename(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "rename-me", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	old := ws.Branch
	if old == "" {
		t.Fatal("expected a branch at create")
	}

	localPR, err := st.AddPR(domain.PR{
		ID: "1", WorkspaceID: ws.ID, Number: 1, Source: domain.SourceLocal,
		Branch: old, Base: "main", Title: "local", State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR local: %v", err)
	}

	ghWS, err := eng.CreateWorkspace(projID, "gh-frozen", "")
	if err != nil {
		t.Fatalf("CreateWorkspace gh: %v", err)
	}
	ghPR, err := st.AddPR(domain.PR{
		ID: "gh-9", WorkspaceID: ghWS.ID, Number: 9, Source: domain.SourceGitHub,
		Branch: "recur/original-gh-head", Base: "main", Title: "gh", State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR gh: %v", err)
	}

	cmd := exec.Command("git", "-C", ws.WorktreePath, "branch", "-m", "human/renamed")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("branch -m: %v\n%s", err, out)
	}

	got := eng.syncWorkspaceBranch(ws.ID)
	if got != "human/renamed" {
		t.Fatalf("syncWorkspaceBranch = %q, want human/renamed", got)
	}
	ws2, _ := st.Workspace(ws.ID)
	if ws2.Branch != "human/renamed" {
		t.Fatalf("Workspace.Branch = %q, want human/renamed", ws2.Branch)
	}
	local2, _ := st.PR(localPR.ID)
	if local2.Branch != "human/renamed" {
		t.Fatalf("Local PR.Branch = %q, want human/renamed", local2.Branch)
	}
	gh2, _ := st.PR(ghPR.ID)
	if gh2.Branch != "recur/original-gh-head" {
		t.Fatalf("GitHub PR.Branch changed to %q; must stay frozen", gh2.Branch)
	}

	// Rename the GH workspace's worktree too — sync must NOT overwrite PR.Branch.
	cmd = exec.Command("git", "-C", ghWS.WorktreePath, "branch", "-m", "agent/also-renamed")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("gh branch -m: %v\n%s", err, out)
	}
	if got := eng.syncWorkspaceBranch(ghWS.ID); got != "agent/also-renamed" {
		t.Fatalf("gh sync = %q", got)
	}
	gh3, _ := st.PR(ghPR.ID)
	if gh3.Branch != "recur/original-gh-head" {
		t.Fatalf("GitHub PR.Branch after workspace rename = %q, want frozen head", gh3.Branch)
	}
	ghWS2, _ := st.Workspace(ghWS.ID)
	if ghWS2.Branch != "agent/also-renamed" {
		t.Fatalf("GH workspace Branch = %q", ghWS2.Branch)
	}

	foundBranch := false
	for _, ev := range sink.byName("workspace.updated") {
		d, _ := ev.Data.(map[string]any)
		if d["workspaceId"] == ws.ID && d["branch"] == "human/renamed" {
			foundBranch = true
		}
	}
	if !foundBranch {
		t.Error("workspace.updated did not carry the renamed branch")
	}
}

// TestOpenWorkspacePRAfterRename asserts Open PR uses the live branch name after
// a mid-session rename (the bug that fell back to Local with a missing refspec).
func TestOpenWorkspacePRAfterRename(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	repo := initRepo(t)
	projID := addProject(t, st, repo)
	ws, err := eng.CreateWorkspace(projID, "open-after-rename", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	if err := os.WriteFile(filepath.Join(ws.WorktreePath, "feat.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGitIn := func(dir string, args ...string) {
		t.Helper()
		cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	runGitIn(ws.WorktreePath, "add", "-A")
	runGitIn(ws.WorktreePath, "commit", "-m", "feat")
	runGitIn(ws.WorktreePath, "branch", "-m", "mycodecrafting/screening-cache")

	prID, err := eng.OpenWorkspacePR(ws.ID, false)
	if err != nil {
		t.Fatalf("OpenWorkspacePR: %v", err)
	}
	pr, ok := st.PR(prID)
	if !ok {
		t.Fatal("PR missing")
	}
	if pr.Branch != "mycodecrafting/screening-cache" {
		t.Fatalf("PR.Branch = %q, want mycodecrafting/screening-cache (live)", pr.Branch)
	}
	ws2, _ := st.Workspace(ws.ID)
	if ws2.Branch != "mycodecrafting/screening-cache" {
		t.Fatalf("Workspace.Branch = %q after open", ws2.Branch)
	}
}

// TestFingerprintIncludesBranch ensures a rename alone changes the watch fingerprint
// (FSEvents ignores .git/, so the backstop must see branch renames).
func TestFingerprintIncludesBranch(t *testing.T) {
	repo := initRepo(t)
	root := t.TempDir()
	m := gitpkg.NewManager(repo, root)
	ctx := context.Background()
	wt, _, err := m.Add(ctx, "LOOP-FP", "fp", "main")
	if err != nil {
		t.Fatalf("Add: %v", err)
	}

	a, ok := worktreeFingerprint(ctx, wt.Path, "")
	if !ok {
		t.Fatal("fingerprint failed")
	}
	cmd := exec.Command("git", "-C", wt.Path, "branch", "-m", "renamed-fp")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("branch -m: %v\n%s", err, out)
	}
	b, ok := worktreeFingerprint(ctx, wt.Path, "")
	if !ok {
		t.Fatal("fingerprint after rename failed")
	}
	if a == b {
		t.Fatal("fingerprint unchanged after branch rename")
	}
}
