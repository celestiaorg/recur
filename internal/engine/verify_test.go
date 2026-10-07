package engine

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"os/exec"
	"sync"
	"testing"
	"time"

	"recur/internal/domain"
	"recur/internal/store"
)

// verifyRunner is a TEST DOUBLE that drives the convergence→verify→pr_open path
// through the REAL MCP path. As the Coder it makes a tiny edit (so the engine
// commit lands a diff); as the Reviewer it files findings per findingsPerIter
// (default 0 → converges on iteration 1); as the QA agent it POSTs attach_evidence
// tool calls to the loop's live MCP URL — the exact HTTP → JSON-RPC → tool handler
// → engineReporter → collectEvidence path the production claude agent uses. The
// evidence schedule per verify run is evidencePerRun[runIndex].
type verifyRunner struct {
	findingsPerIter []int            // review iteration i+1 files findingsPerIter[i] findings
	evidencePerRun  [][]stepEvidence // verify run i+1 files these evidence steps

	mu           sync.Mutex
	coderRuns    int
	reviewRuns   int
	qaRuns       int
	reviewIter   int
	verifyRun    int
	qaPlaywright bool   // whether the QA spec requested the Playwright MCP
	qaMCPCfg     string // the full --mcp-config arg the runner WOULD pass for the QA agent
	qaTools      []string
}

type stepEvidence struct {
	step   string
	shot   string
	passed bool
}

func (r *verifyRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	switch spec.Role {
	case domain.RoleCoder:
		r.mu.Lock()
		r.coderRuns++
		r.mu.Unlock()
		sr := &ScriptedRunner{Edits: map[string]string{"work.txt": "edit\n"}}
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
		r.reviewRuns++
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

	case domain.RoleQA:
		r.mu.Lock()
		r.qaRuns++
		run := r.verifyRun
		r.verifyRun++
		// Record what the engine asked of the QA agent (tool scoping + Playwright).
		r.qaPlaywright = spec.Playwright
		r.qaTools = append([]string(nil), spec.AllowedTools...)
		r.qaMCPCfg = mcpConfigJSON(spec)
		r.mu.Unlock()

		var steps []stepEvidence
		if run < len(r.evidencePerRun) {
			steps = r.evidencePerRun[run]
		}
		allOK := len(steps) > 0
		for _, s := range steps {
			if err := postEvidence(spec.MCPURL, s.step, s.shot, s.passed); err != nil {
				return Result{}, err
			}
			if !s.passed {
				allOK = false
			}
		}
		if spec.MCPURL != "" {
			if err := postReportResult(spec.MCPURL, allOK); err != nil {
				return Result{}, err
			}
		}
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil

	default:
		return Result{Subtype: "success"}, nil
	}
}

// postEvidence POSTs an attach_evidence tools/call to the MCP URL and waits for the
// response, so by the time the QA turn returns the engine has recorded the step.
func postEvidence(mcpURL, step, shot string, passed bool) error {
	args, _ := json.Marshal(map[string]any{
		"step": step, "screenshot_path": shot, "passed": passed,
	})
	params, _ := json.Marshal(map[string]any{"name": "attach_evidence", "arguments": json.RawMessage(args)})
	body, _ := json.Marshal(map[string]any{
		"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": json.RawMessage(params),
	})
	resp, err := http.Post(mcpURL, "application/json", bytes.NewReader(body))
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	return nil
}

func postReportResult(mcpURL string, passed bool) error {
	args, _ := json.Marshal(map[string]any{"passed": passed, "summary": "verify"})
	params, _ := json.Marshal(map[string]any{"name": "report_result", "arguments": json.RawMessage(args)})
	body, _ := json.Marshal(map[string]any{
		"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": json.RawMessage(params),
	})
	resp, err := http.Post(mcpURL, "application/json", bytes.NewReader(body))
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	return nil
}

// verifyDevProject registers a project whose dev_cmd serves HTTP on $RECUR_PORT so
// the verify phase can actually boot a server (PRD §10). Skips if python3 is absent.
func verifyDevProject(t *testing.T, st *store.Store, repo string) string {
	t.Helper()
	if _, err := exec.LookPath("python3"); err != nil {
		t.Skip("python3 not available; verify phase needs a bootable dev server")
	}
	// Not `python3 -m http.server`: its getfqdn reverse lookup stalls 30s+ on macOS CI.
	p := domain.Project{
		ID: "proj", Name: "proj", RepoPath: repo, BaseBranch: "main",
		RunCommands: []domain.RunCommand{{Name: "dev", Cmd: `python3 -c 'import http.server as h, socketserver as s, sys; s.TCPServer(("", int(sys.argv[1])), h.SimpleHTTPRequestHandler).serve_forever()' $RECUR_PORT`}},
		AppURL:      "http://localhost:$RECUR_PORT",
	}
	saved := addRunProject(t, st, p)
	return saved.ID
}

// TestVerifyPassAdvancesToPROpen is the headline M5 verify test (PRD §10, §19 M5):
// a converged loop (0 findings) with rail #1 ON boots the dev server, the scripted
// QA agent files 3 PASSING evidence steps through the real MCP path → rail #1 is
// satisfied → the loop advances to pr_open. The evidence is persisted on the loop.
func TestVerifyPassAdvancesToPROpen(t *testing.T) {
	repo := initRepo(t)
	runner := &verifyRunner{
		findingsPerIter: []int{0}, // converges on iteration 1
		evidencePerRun: [][]stepEvidence{{
			{step: "Booted the app and opened /checkout", shot: "/shots/01.png", passed: true},
			{step: "Signed in with Apple → valid session", shot: "/shots/02.png", passed: true},
			{step: "Reloaded — session persisted", shot: "/shots/03.png", passed: true},
		}},
	}
	eng, st, sink, _ := newTestEngine(t, runner)
	installFixturePack(t)
	enableLinearMCP(st)
	projID := verifyDevProject(t, st, repo)

	id, err := eng.StartWorkspaceWith(projID, "Add a verifiable feature", "", "", "test.loops.verify", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}

	waitFor(t, 25*time.Second, "loop to verify and reach pr_open", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})

	runner.mu.Lock()
	qaRuns, plw := runner.qaRuns, runner.qaPlaywright
	tools := append([]string(nil), runner.qaTools...)
	cfg := runner.qaMCPCfg
	runner.mu.Unlock()

	if qaRuns != 1 {
		t.Errorf("QA agent should run exactly once, ran %d", qaRuns)
	}
	// The QA agent was wired with the Playwright MCP + scoped to browser+evidence only.
	if !plw {
		t.Error("QA spec should request the Playwright MCP (spec.Playwright)")
	}
	if !containsStr(tools, mcpAttachEvidence) || !containsStr(tools, mcpPlaywrightAll) || !containsStr(tools, mcpReportResult) {
		t.Errorf("QA tools should include evidence + playwright + report_result, got %v", tools)
	}
	if !containsStr(tools, "Read") {
		t.Errorf("QA is Ask-mode and must be able to Read, tools=%v", tools)
	}
	for _, banned := range []string{"Edit", "Write", "NotebookEdit"} {
		if containsStr(tools, banned) {
			t.Errorf("QA must NOT have %q; tools=%v", banned, tools)
		}
	}
	// The --mcp-config the runner builds for QA lists BOTH recur AND playwright.
	if !contains(cfg, `"recur"`) || !contains(cfg, `"playwright"`) || !contains(cfg, "@playwright/mcp") {
		t.Errorf("QA mcp-config should list recur + playwright (@playwright/mcp): %s", cfg)
	}
	if !contains(cfg, domain.LinearMCPReadOnlyURL) {
		t.Errorf("QA mcp-config should attach Linear readonly: %s", cfg)
	}

	// Evidence persisted on the loop, all passed, with screenshot paths.
	l, _ := st.Workspace(id)
	if l.VerifyState != "passed" {
		t.Errorf("loop VerifyState: got %q, want passed", l.VerifyState)
	}
	if len(l.Verify) != 3 {
		t.Fatalf("expected 3 verify steps persisted, got %d", len(l.Verify))
	}
	for i, v := range l.Verify {
		if !v.Passed {
			t.Errorf("verify step %d should be passed", i)
		}
	}
	if l.Verify[0].Screenshot != "/shots/01.png" {
		t.Errorf("verify step screenshot path lost: %q", l.Verify[0].Screenshot)
	}

	// A verifying state was observed in the pipeline.
	sawVerifying := false
	for _, ev := range sink.byName("workspace.updated") {
		m := ev.Data.(map[string]any)
		if m["state"].(domain.WorkspaceState) == domain.StateVerifying {
			sawVerifying = true
		}
	}
	if !sawVerifying {
		t.Error("expected a verifying state in the pipeline")
	}
}

// TestVerifyFailBlocksPROpen: rail #1 ON, the scripted QA files a FAILING evidence
// step → rail #1 BLOCKS pr_open. The loop re-enters coding (it does NOT reach
// pr_open) and, since every verify keeps failing, ends NON-pr_open (stopped or
// escalated). We turn rail #4 OFF so the cap stops honestly (deterministic).
func TestVerifyFailBlocksPROpen(t *testing.T) {
	repo := initRepo(t)
	// Always converges (0 findings) but EVERY verify run has a failing step.
	failRun := []stepEvidence{
		{step: "Opened /checkout", shot: "/shots/a.png", passed: true},
		{step: "Sign in with Google returned a 500", shot: "/shots/b.png", passed: false},
	}
	runner := &verifyRunner{
		findingsPerIter: []int{0, 0, 0, 0, 0, 0},
		evidencePerRun:  [][]stepEvidence{failRun, failRun, failRun, failRun, failRun, failRun},
	}
	eng, st, _, _ := newTestEngine(t, runner)
	installFixturePack(t)
	projID := verifyDevProject(t, st, repo)

	id, err := eng.StartWorkspaceWith(projID, "feature that never verifies", "", "", "test.loops.verify", "test.policies.fail")
	if err != nil {
		t.Fatalf("StartWorkspaceWith: %v", err)
	}

	// Omit on_reject → verify fail is the verb's fail (not a PR).
	waitFor(t, 40*time.Second, "loop to fail without opening a PR", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateFailed
	})

	l, _ := st.Workspace(id)
	if l.State == domain.StatePROpen {
		t.Fatal("a loop whose browser verify never passes must NOT open a PR")
	}
	if l.VerifyState != "failed" {
		t.Errorf("loop VerifyState: got %q, want failed", l.VerifyState)
	}
	runner.mu.Lock()
	qaRuns := runner.qaRuns
	runner.mu.Unlock()
	if qaRuns < 1 {
		t.Errorf("QA should run at least once, ran %d", qaRuns)
	}
}

// TestVerifyRailOffNoVerifyRequired: with rail #1 OFF, a converged loop reaches
// pr_open WITHOUT running the verify phase at all (no QA agent, no dev server boot).
func TestVerifyRailOffNoVerifyRequired(t *testing.T) {
	repo := initRepo(t)
	runner := &verifyRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	// The default To a PR graph has no verify phase: QA never runs.
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "no-verify feature", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}

	waitFor(t, 10*time.Second, "loop to reach pr_open without verify", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})

	runner.mu.Lock()
	qaRuns := runner.qaRuns
	runner.mu.Unlock()
	if qaRuns != 0 {
		t.Errorf("no verify phase must skip the QA agent entirely, ran %d", qaRuns)
	}
	l, _ := st.Workspace(id)
	if l.VerifyState != "" {
		t.Errorf("no verify phase: loop should have no verify state, got %q", l.VerifyState)
	}
}

// TestVerifyAbsentRunConfigBlocksPR: rail #1 ON but the project has NO dev_cmd → the
// loop can't verify → rail #1 blocks pr_open. With rail #4 off it stops honestly.
func TestVerifyAbsentRunConfigBlocksPR(t *testing.T) {
	repo := initRepo(t)
	runner := &verifyRunner{findingsPerIter: []int{0, 0, 0, 0, 0, 0}}
	eng, st, _, _ := newTestEngine(t, runner)
	installFixturePack(t)
	projID := addProject(t, st, repo) // NO dev_cmd → can't verify

	id, err := eng.StartWorkspaceWith(projID, "feature with no run config", "", "", "test.loops.verify", "test.policies.fail")
	if err != nil {
		t.Fatalf("StartWorkspaceWith: %v", err)
	}

	waitFor(t, 20*time.Second, "loop to fail (no run config to verify)", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateFailed
	})
	l, _ := st.Workspace(id)
	if l.State == domain.StatePROpen {
		t.Fatal("no run config → can't verify → must not open a PR")
	}
	if l.VerifyState != "failed" {
		t.Errorf("absent verify should be recorded as failed, got %q", l.VerifyState)
	}
	// The QA agent never ran (no server to drive).
	runner.mu.Lock()
	qaRuns := runner.qaRuns
	runner.mu.Unlock()
	if qaRuns != 0 {
		t.Errorf("QA must not run when there is no dev server to drive, ran %d", qaRuns)
	}
}

func containsStr(xs []string, s string) bool {
	for _, x := range xs {
		if x == s {
			return true
		}
	}
	return false
}
