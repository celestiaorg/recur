package engine

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
)

// TestRealHeadlineE2E is the ONE real-agent end-to-end test (PRD §20 headline with
// NO test doubles): real `claude -p` agents (Sonnet 5 via the `sonnet` alias), the
// real Recur MCP, the real Playwright MCP driving a real dev server, real git.
//
//	plan → code → independent review → VERIFY-IN-BROWSER (rail #1 ON) → Local PR
//	→ human send-back → real rework → re-request → merge (base advances on disk).
//
// It is deliberately LEAN (one tiny task, cautious autonomy = 2-iteration cap, one
// send-back) so a full run costs a handful of Sonnet turns, and is gated behind
// RECUR_E2E=1 (plus claude/python3/Chrome present) so default test runs never
// spend credit. Total spend is printed at the end.
func TestRealHeadlineE2E(t *testing.T) {
	if os.Getenv("RECUR_E2E") != "1" {
		t.Skip("set RECUR_E2E=1 to run the real-agent headline E2E (spends credit)")
	}
	for _, bin := range []string{"claude", "python3", "npx", "git"} {
		if _, err := exec.LookPath(bin); err != nil {
			t.Skipf("%s not on PATH", bin)
		}
	}

	// A tiny REAL web project: index.html served by python3 http.server.
	repo := initRepo(t)
	html := "<!doctype html>\n<html>\n<head><title>demo</title></head>\n<body>\n</body>\n</html>\n"
	if err := os.WriteFile(filepath.Join(repo, "index.html"), []byte(html), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, repo, "add", ".")
	gitRun(t, repo, "commit", "-m", "add index.html")

	eng, st, _, _ := newTestEngine(t, NewClaudeRunner()) // PRODUCTION runner
	projID := verifyDevProject(t, st, repo)              // dev_cmd: python3 http.server $RECUR_PORT

	installFixturePack(t)
	p := st.Policy()
	p.Autonomy = domain.AutonomyCautious
	st.SetPolicy(p)

	// ---- 1) The loop runs the full pipeline with real agents. ----
	id, err := eng.StartWorkspaceWith(projID, "In index.html, add an <h1> element with the exact text 'Hello Recur' inside <body>. Change nothing else.", "", "", "test.loops.verify", "recur.policies.cautious")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 12*time.Minute, "real loop to reach pr_open (plan→code→review→verify→PR)", func() bool {
		l, ok := st.Workspace(id)
		return ok && (l.State == domain.StatePROpen || l.State == domain.StateFailed ||
			l.State == domain.StateStopped || l.State == domain.StateEscalated)
	})
	l, _ := st.Workspace(id)
	if l.State != domain.StatePROpen {
		t.Fatalf("loop did not reach pr_open: state=%q phase=%q last=%q", l.State, l.Phase, l.Last)
	}
	if l.VerifyState != "passed" {
		t.Fatalf("rail #1: verify-in-browser did not pass: %q (evidence: %d steps)", l.VerifyState, len(l.Verify))
	}
	if len(l.Verify) == 0 {
		t.Fatal("no QA evidence recorded")
	}
	t.Logf("verify passed with %d evidence step(s); first: %q", len(l.Verify), l.Verify[0].HTML)

	pr, ok := st.PRByWorkspace(id)
	if !ok {
		t.Fatal("no PR opened")
	}
	if pr.Diff.Add == 0 {
		t.Errorf("PR has an empty diff")
	}
	if n := st.OpenFindingsCount(id); n != 0 {
		t.Errorf("PR opened with %d open findings (rail #3)", n)
	}

	// The change is really in the worktree branch.
	if out, err := gitOut(repo, l.WorktreePath, "grep", "Hello Recur", "--", "index.html"); err != nil || !strings.Contains(out, "Hello Recur") {
		t.Errorf("expected 'Hello Recur' in the worktree index.html (err=%v out=%q)", err, out)
	}

	// ---- 2) Human send-back → conductor turn with the review thread. ----
	if _, ok := eng.SendBackToLoop(pr.ID, []string{"Also change the <title> to exactly 'Recur Demo'."}); !ok {
		t.Fatal("SendBackToLoop refused")
	}
	p2, _ := st.PR(pr.ID)
	lp2, _ := st.Workspace(id)
	if p2.State != "In review" {
		dumpLoopDiagnostics(t, st, repo, id, pr.ID)
		t.Fatalf("SendBackToLoop() state=%q, want In review", p2.State)
	}
	if lp2.State != domain.StatePROpen {
		t.Errorf("workspace state = %q, want pr_open", lp2.State)
	}

	// ---- 3) Merge: the base branch advances ON DISK. ----
	baseBefore, _ := gitOut(repo, repo, "rev-parse", "main")
	merged, ok := eng.MergePR(pr.ID, "")
	if !ok || merged.State != "Merged" {
		t.Fatalf("merge failed: ok=%v state=%q", ok, merged.State)
	}
	baseAfter, _ := gitOut(repo, repo, "rev-parse", "main")
	if baseBefore == baseAfter {
		t.Errorf("base ref did not advance on merge")
	}
	if out, _ := gitOut(repo, repo, "show", "main:index.html"); !strings.Contains(out, "Hello Recur") {
		t.Errorf("merged main/index.html missing the change:\n%s", out)
	}

	fmt.Printf("\n=== REAL HEADLINE E2E PASSED ===\n\n")
}

// dumpLoopDiagnostics prints the loop's activity feed, the PR thread, and the
// worktree git log so a failed real-agent run is diagnosable from the test output.
func dumpLoopDiagnostics(t *testing.T, st interface {
	Feed(string) []domain.FeedItem
	PR(string) (domain.PR, bool)
	Workspace(string) (domain.Workspace, bool)
}, repo, workspaceID, prID string) {
	t.Helper()
	t.Log("---------- LOOP FEED (newest first) ----------")
	for _, f := range st.Feed(workspaceID) {
		t.Logf("  [%s] %s: %s", f.AgentCode, f.Name, f.Text)
	}
	if pr, ok := st.PR(prID); ok {
		t.Log("---------- PR THREAD ----------")
		for _, c := range pr.Comments {
			t.Logf("  [%s] %s: %s", c.Code, c.Author, c.BodyHTML)
		}
	}
	if l, ok := st.Workspace(workspaceID); ok && l.WorktreePath != "" {
		if out, err := gitOut(repo, l.WorktreePath, "log", "--oneline", "-8"); err == nil {
			t.Logf("---------- WORKTREE LOG ----------\n%s", out)
		}
	}
}

// gitRun runs git in dir, failing the test on error (thin local helper).
func gitRun(t *testing.T, dir string, args ...string) {
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
