package engine

import (
	"context"
	"os"
	"os/exec"
	"strings"
	"sync"
	"testing"
	"time"

	"recur/internal/domain"
)

// reworkRunner is a TEST DOUBLE that drives the PR lifecycle without real
// claude: the Coder edits a file each coding turn (so the engine commit lands a
// diff) and the Reviewer files findings per a schedule via the REAL MCP path.
type reworkRunner struct {
	findingsPerIter []int

	mu          sync.Mutex
	coderRuns   int
	reviewIter  int
	editCounter int
}

func (r *reworkRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	switch spec.Role {
	case domain.RoleCoder:
		r.mu.Lock()
		r.coderRuns++
		r.editCounter++
		n := r.editCounter
		r.mu.Unlock()
		sr := &ScriptedRunner{Edits: map[string]string{"work.txt": strings.Repeat("edit\n", n)}}
		if err := sr.applyEdits(spec.Worktree); err != nil {
			return Result{}, err
		}
		if emit != nil {
			emit(ParsedEvent{Kind: KindStep, Tool: "Edit", ToolInput: "work.txt"})
		}
		if spec.MCPURL != "" {
			if err := closeOpenFindingsMCP(spec.MCPURL); err != nil {
				return Result{}, err
			}
		}
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil

	case domain.RoleReviewer:
		r.mu.Lock()
		r.reviewIter++
		iter := r.reviewIter
		r.mu.Unlock()
		n := 0
		if iter-1 < len(r.findingsPerIter) {
			n = r.findingsPerIter[iter-1]
		}
		for i := 0; i < n; i++ {
			if err := postFinding(spec.MCPURL, "work.txt", i+1, "medium", "issue"); err != nil {
				return Result{}, err
			}
		}
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil

	default:
		return Result{Subtype: "success"}, nil
	}
}

// TestSendBackToLoopIsConductorTurn: Send Back flushes pending line notes, posts
// the human notes, and starts a Maestro turn with the thread in context.
func TestSendBackToLoopIsConductorTurn(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	ws := domain.Workspace{
		ID: "L-sendback", ProjectID: "P", Goal: "Add OAuth",
		State: domain.StatePROpen, RunStarted: true, LoopID: "recur.loops.pr",
	}
	if _, err := st.AddWorkspace(ws); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "pr-sb", WorkspaceID: ws.ID, Number: 12, Title: "Add OAuth",
		State: "In review", Group: "you",
		Comments: []domain.PRComment{
			{Author: "ada", BodyMD: "please rename the config key"},
			{Author: "ada", Path: "oauth.ts", Line: 40, BodyMD: "client secret read"},
		},
		PendingComments: []domain.PendingLineComment{
			{File: "auth.go", Line: 8, Body: "nit: unwrap this error"},
		},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	updated, ok := eng.SendBackToLoop(pr.ID, []string{"Resolve the client-secret read in oauth.ts"})
	if !ok {
		t.Fatal("SendBackToLoop() ok = false, want true")
	}
	if updated.State != "In review" {
		t.Errorf("SendBackToLoop() state=%q, want In review", updated.State)
	}
	if l, _ := st.Workspace(ws.ID); l.State != domain.StatePROpen {
		t.Errorf("Workspace.State = %q, want pr_open", l.State)
	}
	got, ok := st.PR(pr.ID)
	if !ok {
		t.Fatal("PR missing after SendBackToLoop")
	}
	if len(got.PendingComments) != 0 {
		t.Errorf("SendBackToLoop() pending = %d, want flushed", len(got.PendingComments))
	}
	ctx := eng.maestroContext(ws)
	prompt := maestroSystemPrompt(ctx)
	for _, want := range []string{
		"please rename the config key",
		"client secret read",
		"unwrap this error",
		"Resolve the client-secret read in oauth.ts",
		"reenter_phase",
	} {
		if !strings.Contains(prompt, want) && !strings.Contains(ctx.PRThread, want) {
			t.Errorf("conductor context missing %q: PRThread=%q prompt=%q", want, ctx.PRThread, prompt)
		}
	}
}

// TestRefreshPRBehindAndUpdateWithBase covers D51/D54: behind-base count +
// lightweight 1-click update (direct git merge).
func TestRefreshPRBehindAndUpdateWithBase(t *testing.T) {
	repo := initRepo(t)
	runner := &reworkRunner{findingsPerIter: []int{0, 0}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Feature before base moves", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 10*time.Second, "first run to open a PR", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})
	pr, ok := st.PRByWorkspace(id)
	if !ok {
		t.Fatal("no PR")
	}
	loop, _ := st.Workspace(id)

	// Advance base while the loop branch stays put (non-conflicting empty commit).
	cmd := exec.Command("git", "commit", "--allow-empty", "-m", "base moved ahead")
	cmd.Dir = repo
	cmd.Env = append(os.Environ(),
		"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
		"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
	)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("advance base: %v\n%s", err, out)
	}

	eng.RefreshPRBehind(pr.ID)
	p, _ := st.PR(pr.ID)
	if p.BehindBase < 1 {
		t.Fatalf("RefreshPRBehind: BehindBase=%d, want >=1 (worktree=%s)", p.BehindBase, loop.WorktreePath)
	}

	updated, ok := eng.UpdatePRWithBase(pr.ID, "")
	if !ok {
		t.Fatal("UpdatePRWithBase refused")
	}
	if updated.BehindBase != 0 {
		t.Errorf("BehindBase=%d after clean update, want 0", updated.BehindBase)
	}
	if updated.UpdatingBase != "" {
		t.Errorf("UpdatingBase=%q after clean update, want empty", updated.UpdatingBase)
	}
	if updated.Group != "you" || updated.State != "In review" {
		t.Errorf("PR should stay In review/you, got state=%q group=%q", updated.State, updated.Group)
	}

	cmd = exec.Command("git", "commit", "--allow-empty", "-m", "base moved again")
	cmd.Dir = repo
	cmd.Env = append(os.Environ(),
		"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
		"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
	)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("advance base again: %v\n%s", err, out)
	}
	eng.RefreshPRBehind(pr.ID)
	rebased, ok := eng.UpdatePRWithBase(pr.ID, "rebase")
	if !ok {
		t.Fatal("UpdatePRWithBase(rebase) refused")
	}
	if rebased.BehindBase != 0 {
		t.Errorf("BehindBase=%d after rebase, want 0", rebased.BehindBase)
	}
	foundRebase := false
	for _, c := range rebased.Comments {
		if strings.Contains(c.BodyHTML, "rebase") || strings.Contains(c.BodyMD, "rebase") {
			foundRebase = true
			break
		}
	}
	if !foundRebase {
		t.Error("UpdatePRWithBase(\"rebase\") should record a rebase note even when project default is merge")
	}
}

// TestUpdatePRWithBaseConflict resolves via a single Coder turn (D54).
func TestUpdatePRWithBaseConflict(t *testing.T) {
	repo := initRepo(t)
	runner := &reworkRunner{findingsPerIter: []int{0, 0}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Feature that will conflict with base", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 10*time.Second, "first run to open a PR", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})
	pr, ok := st.PRByWorkspace(id)
	if !ok {
		t.Fatal("no PR")
	}
	loop, _ := st.Workspace(id)

	// Advance base by editing the same file the Coder wrote (work.txt) → conflict.
	workPath := loop.WorktreePath + "/work.txt"
	if _, err := os.Stat(workPath); err != nil {
		// Ensure the conflicted file exists on the branch even if the scripted
		// coder used a different path in some setups.
		t.Fatalf("expected work.txt in worktree from scripted coder: %v", err)
	}
	baseFile := repo + "/work.txt"
	if err := os.WriteFile(baseFile, []byte("base side — conflicting change\n"), 0o644); err != nil {
		t.Fatalf("write base work.txt: %v", err)
	}
	cmd := exec.Command("git", "add", "work.txt")
	cmd.Dir = repo
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git add: %v\n%s", err, out)
	}
	cmd = exec.Command("git", "commit", "-m", "base conflicts with branch")
	cmd.Dir = repo
	cmd.Env = append(os.Environ(),
		"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
		"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
	)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("advance base with conflict: %v\n%s", err, out)
	}

	eng.RefreshPRBehind(pr.ID)
	p, _ := st.PR(pr.ID)
	if p.BehindBase < 1 {
		t.Fatalf("BehindBase=%d, want >=1", p.BehindBase)
	}

	updated, ok := eng.UpdatePRWithBase(pr.ID, "")
	if !ok {
		t.Fatal("UpdatePRWithBase refused")
	}
	if updated.UpdatingBase != "coder" {
		t.Errorf("UpdatingBase=%q immediately after conflict, want coder", updated.UpdatingBase)
	}

	waitFor(t, 15*time.Second, "conflict Coder turn to settle BehindBase=0", func() bool {
		p, ok := st.PR(pr.ID)
		return ok && p.BehindBase == 0 && p.UpdatingBase == ""
	})
	final, _ := st.PR(pr.ID)
	if final.Group != "you" {
		t.Errorf("PR group=%q, want you", final.Group)
	}
	if final.UpdatingBase != "" {
		t.Errorf("UpdatingBase=%q after settle, want empty", final.UpdatingBase)
	}
}
