package engine

import (
	"context"
	"encoding/json"
	"strings"
	"sync"
	"testing"
	"time"

	"recur/internal/domain"
	"recur/internal/store"
)

// headlineRunner is the capstone TEST DOUBLE that drives the FULL PRD §20 headline
// flow without real claude/browser/gh. It combines every role behavior the headline
// needs and routes structured signals through the REAL Recur MCP HTTP path (the
// exact HTTP → JSON-RPC → tool handler → engineReporter seam the production agent
// uses) rather than poking the engine directly:
//
//   - Coder: makes a real worktree edit each coding turn (so the engine commit lands
//     a diff).
//   - Reviewer: files findings via report_finding per a schedule. The convergence
//     follows reviewFindings (e.g. [2,0] → converges on iter 2).
//   - QA: files PASSING attach_evidence steps via the MCP on every verify run,
//     satisfying rail #1.
//
// All counters are mutex-guarded so the test reads them race-free.
type headlineRunner struct {
	reviewFindings []int          // initial-converge review iteration i+1 → findings[i]
	evidence       []stepEvidence // passing QA evidence filed on each verify run

	mu             sync.Mutex
	coderRuns      int
	reviewerRuns   int
	qaRuns         int
	reviewIter     int      // 1-based review iteration
	sawCoder       []string // agent ids seen as the Coder
	sawReviewer    []string // agent ids seen as the Reviewer
	qaPlaywright   bool
	qaAllowedTools []string
}

func (r *headlineRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	switch spec.Role {
	case domain.RoleCoder:
		r.mu.Lock()
		r.coderRuns++
		r.sawCoder = append(r.sawCoder, spec.AgentID)
		r.mu.Unlock()
		// A real edit each coding turn so the engine commit has a diff to commit.
		sr := &ScriptedRunner{Edits: map[string]string{"feature.txt": "the implemented feature\n"}}
		if err := sr.applyEdits(spec.Worktree); err != nil {
			return Result{}, err
		}
		if emit != nil {
			// A faithful tool event (id + raw input, as the real parser produces) so the
			// session mirror reconstructs the same feed row the live stream shows.
			emit(ParsedEvent{
				Kind: KindStep, Tool: "Edit", ToolUseID: "tu-edit-" + spec.AgentID,
				ToolInput: "feature.txt", ToolInputRaw: json.RawMessage(`{"file_path":"feature.txt"}`),
			})
		}
		if spec.MCPURL != "" {
			if err := closeOpenFindingsMCP(spec.MCPURL); err != nil {
				return Result{}, err
			}
		}
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil

	case domain.RoleReviewer:
		r.mu.Lock()
		r.reviewerRuns++
		r.sawReviewer = append(r.sawReviewer, spec.AgentID)
		r.reviewIter++
		iter := r.reviewIter
		var n int
		if iter-1 < len(r.reviewFindings) {
			n = r.reviewFindings[iter-1]
		}
		r.mu.Unlock()
		for i := 0; i < n; i++ {
			if err := postFinding(spec.MCPURL, "feature.txt", i+1, "medium", "tighten this"); err != nil {
				return Result{}, err
			}
		}
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil

	case domain.RoleQA:
		r.mu.Lock()
		r.qaRuns++
		r.qaPlaywright = spec.Playwright
		r.qaAllowedTools = append([]string(nil), spec.AllowedTools...)
		r.mu.Unlock()
		// File passing evidence through the REAL MCP path on every verify run
		// so rail #1 is satisfied.
		allOK := len(r.evidence) > 0
		for _, s := range r.evidence {
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
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil
	}
}

// TestHeadlineE2E is the FINAL-ACCEPTANCE capstone for PRD §20 (the headline E2E).
// It drives the WHOLE flow end-to-end against a REAL temp git repo with a REAL
// bootable dev server and the REAL Recur MCP — no real claude/browser/gh — and
// asserts each transition of the headline sentence:
//
//	fresh project → typed task → loop plans/codes/iterates code↔review to 0 findings
//	→ verifies in a browser with captured evidence → opens a PR → human leaves an
//	inline comment + sends it back to the conductor → human merges; everything
//	survives an app reload (history intact).
//
// The escalation pause→answer→resume half of the headline is proven separately and
// rigorously by TestEscalationRoundTripAtCap (escalation_test.go) — cited in the
// audit; this capstone focuses on the converge→verify→PR→send-back→merge→reload
// spine so it stays hermetic and fast.
//
// Run under -race; every waitFor has a HARD deadline that acts as a hang guard.
func TestHeadlineE2E(t *testing.T) {
	repo := initRepo(t)

	runner := &headlineRunner{
		// Convergence series [2,0]: 2 findings on review iter 1, 0 on iter 2 → the
		// ENGINE declares convergence on iteration 2.
		reviewFindings: []int{2, 0},
		evidence: []stepEvidence{
			{step: "Booted the app and opened /", shot: "/shots/01.png", passed: true},
			{step: "Exercised the new feature end to end", shot: "/shots/02.png", passed: true},
			{step: "Reloaded — state persisted", shot: "/shots/03.png", passed: true},
		},
	}

	eng, st, sink, dataDir := newTestEngine(t, runner)
	installFixturePack(t)
	projID := verifyDevProject(t, st, repo)

	// ---- 1. Fresh project → typed task → StartWorkspaceWith (verify graph). ----
	id, err := eng.StartWorkspaceWith(projID, "Add a verifiable, reviewable feature", "", "", "test.loops.verify", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	if id == "" {
		t.Fatal("StartWorkspace returned empty loop id")
	}

	// ---- 2 + 3 + 4. Converge code↔review to 0 findings, verify in a browser, open a
	// PR. The loop reaches pr_open only after convergence AND a passing verify (rail
	// #1) AND zero open findings (rail #3). ----
	waitFor(t, 30*time.Second, "loop to converge, verify, and open a PR", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})

	loop, _ := st.Workspace(id)
	if loop.Group != domain.GroupReady {
		t.Errorf("converged loop group: got %q, want ready", loop.Group)
	}

	// ---- 2. Convergence series is exactly [2,0]; the Reviewer is a SEPARATE agent
	// from the Coder (rail #2); the engine — not the Coder — declared convergence. ----
	var series []int
	for _, c := range loop.Convergence {
		series = append(series, c.Findings)
	}
	if !equalInts(series, []int{2, 0}) {
		t.Errorf("convergence series: got %v, want [2 0]", series)
	}
	runner.mu.Lock()
	coderRuns, reviewerRuns := runner.coderRuns, runner.reviewerRuns
	sawCoder := append([]string(nil), runner.sawCoder...)
	sawReviewer := append([]string(nil), runner.sawReviewer...)
	runner.mu.Unlock()
	// Two convergence iterations on the initial run (coder+reviewer each twice).
	if coderRuns < 2 || reviewerRuns < 2 {
		t.Errorf("initial convergence should run 2 code↔review iterations, got coder=%d reviewer=%d", coderRuns, reviewerRuns)
	}
	// RAIL #2: the Reviewer is never the same agent that wrote the code.
	for _, cid := range sawCoder {
		for _, rid := range sawReviewer {
			if cid == rid {
				t.Errorf("rail #2 violated: coder and reviewer share agent id %q", cid)
			}
		}
	}
	if roles[domain.RoleReviewer].code == roles[domain.RoleCoder].code {
		t.Error("rail #2: reviewer and coder must have structurally distinct agent codes")
	}
	// The ENGINE (Maestro) authored the "Converged" feed line — the Coder never opens a
	// PR or declares convergence itself (PRD §7.3).
	sawConverged := false
	for _, ev := range sink.byName("workspace.event") {
		m := ev.Data.(map[string]any)
		item, _ := m["event"].(domain.FeedItem)
		if item.Name == "Maestro" && contains(item.Text, "Converged") {
			sawConverged = true
		}
	}
	if !sawConverged {
		t.Error("expected an engine/Maestro-authored 'Converged' event (engine decides convergence, PRD §7.3)")
	}
	// Zero open findings at convergence (rail #3 precondition).
	if open := st.OpenFindingsCount(id); open != 0 {
		t.Errorf("converged loop must have 0 open findings, got %d", open)
	}
	// Both findings from iteration 1 were persisted (PRD §14).
	if got := len(st.Findings(id)); got != 2 {
		t.Errorf("expected 2 persisted findings (2+0), got %d", got)
	}

	// ---- 3. Verify-in-browser passed with captured evidence (RAIL #1 satisfied). ----
	if loop.VerifyState != "passed" {
		t.Errorf("loop VerifyState: got %q, want passed (rail #1 must be satisfied before pr_open)", loop.VerifyState)
	}
	if len(loop.Verify) != 3 {
		t.Fatalf("expected 3 captured verify evidence steps, got %d", len(loop.Verify))
	}
	for i, v := range loop.Verify {
		if !v.Passed {
			t.Errorf("verify step %d should be passed", i)
		}
		if v.Screenshot == "" {
			t.Errorf("verify step %d lost its screenshot path", i)
		}
	}
	runner.mu.Lock()
	qaRuns, qaPlaywright := runner.qaRuns, runner.qaPlaywright
	qaTools := append([]string(nil), runner.qaAllowedTools...)
	runner.mu.Unlock()
	if qaRuns < 1 {
		t.Error("the QA browser agent must run at least once to verify (rail #1)")
	}
	if !qaPlaywright {
		t.Error("the QA agent must be wired with the Playwright browser MCP (spec.Playwright)")
	}
	// QA is browser-only — it must NOT be able to edit source (PRD §8.2).
	for _, banned := range []string{"Edit", "Write", "NotebookEdit"} {
		if containsStr(qaTools, banned) {
			t.Errorf("QA must NOT have source tool %q; tools=%v", banned, qaTools)
		}
	}
	// A verifying state was observed in the live pipeline.
	sawVerifying := false
	for _, ev := range sink.byName("workspace.updated") {
		m := ev.Data.(map[string]any)
		if m["state"].(domain.WorkspaceState) == domain.StateVerifying {
			sawVerifying = true
		}
	}
	if !sawVerifying {
		t.Error("expected a verifying state in the live pipeline (loop.updated)")
	}

	// ---- 4. A Local PR opened with a REAL diff + commits + 0 open findings. ----
	pr, ok := st.PRByWorkspace(id)
	if !ok {
		t.Fatal("no PR created for the converged+verified loop")
	}
	if pr.Source != domain.SourceLocal {
		t.Errorf("PR source: got %q, want local", pr.Source)
	}
	if pr.State != "In review" || pr.Group != "you" {
		t.Errorf("fresh PR state wrong: state=%q group=%q", pr.State, pr.Group)
	}
	if pr.Branch == "" || pr.Base != "main" {
		t.Errorf("PR branch/base wrong: branch=%q base=%q", pr.Branch, pr.Base)
	}
	if pr.FileCount == 0 || len(pr.Files) == 0 || pr.Diff.Add == 0 {
		t.Errorf("PR must carry the real diff: fileCount=%d files=%d diff=%+v", pr.FileCount, len(pr.Files), pr.Diff)
	}
	if len(pr.Commits) == 0 {
		t.Fatal("PR has no commits")
	}
	if msg := singleLine(pr.Commits[0].Message); msg == "" {
		t.Errorf("PR commit message is empty: %+v", pr.Commits[0])
	}
	// The PR includes the verify evidence as a passing check (rail #1 surfaced in the
	// review UI) and a passing independent-review check (rail #2).
	hasReviewCheck, hasVerifyEvidence := false, false
	for _, c := range pr.Checks {
		if c.Name == "Independent review" && c.Passed {
			hasReviewCheck = true
		}
	}
	for _, c := range pr.Checks {
		if contains(strings.ToLower(c.Name), "verif") && c.Passed {
			hasVerifyEvidence = true
		}
	}
	if !hasReviewCheck {
		t.Errorf("PR checks must include a passing independent review, got %+v", pr.Checks)
	}
	if !hasVerifyEvidence {
		t.Errorf("PR checks must surface the passing browser verify, got %+v", pr.Checks)
	}
	prNumBefore := pr.Number

	// ---- 5. Human leaves an INLINE note, then SENDS IT BACK → conductor turn. ----
	inline, ok := eng.AddPRComment(pr.ID, "feature.txt", 1, "This needs the client-secret read removed", 0, "")
	if !ok || inline.BodyHTML == "" {
		t.Error("AddPRComment returned an empty inline comment")
	}
	updated, ok := eng.SendBackToLoop(pr.ID, []string{"Resolve the client-secret read in feature.txt"})
	if !ok {
		t.Fatal("SendBackToLoop returned not-ok")
	}
	if updated.State != "In review" || updated.Group != "you" {
		t.Errorf("SendBackToLoop() state=%q group=%q, want In review/you", updated.State, updated.Group)
	}
	if l, _ := st.Workspace(id); l.State != domain.StatePROpen {
		t.Errorf("workspace state = %q, want pr_open", l.State)
	}
	afterSend, _ := st.PR(pr.ID)
	if afterSend.Number != prNumBefore {
		t.Errorf("SendBackToLoop() PR number = %d, want %d", afterSend.Number, prNumBefore)
	}
	ws, _ := st.Workspace(id)
	ctx := eng.maestroContext(ws)
	for _, want := range []string{"client-secret", "feature.txt", "reenter_phase"} {
		if !strings.Contains(ctx.PRThread, want) && !strings.Contains(maestroSystemPrompt(ctx), want) {
			t.Errorf("conductor context missing %q: PRThread=%q", want, ctx.PRThread)
		}
	}
	if !sink.has("pr.updated") || !sink.has("pr.comment") {
		t.Error("expected live pr.updated + pr.comment events on send-back")
	}

	// ---- 6. Human MERGES → PR + loop merged; the base ref advances ON DISK. ----
	baseBefore := refHash(t, repo, "main")
	merged, mok := eng.MergePR(pr.ID, "")
	if !mok {
		t.Fatal("merge should succeed after send-back")
	}
	if merged.State != "Merged" || merged.Group != "merged" {
		t.Errorf("merged PR state: state=%q group=%q", merged.State, merged.Group)
	}
	if l, _ := st.Workspace(id); l.State != domain.StateMerged {
		t.Errorf("loop state after merge: got %q, want merged", l.State)
	}
	baseAfter := refHash(t, repo, "main")
	if baseAfter == baseBefore {
		t.Errorf("base ref did not advance on disk after the Local merge: %s == %s", baseAfter, baseBefore)
	}

	// ---- 7. SURVIVES AN APP RELOAD: reopen the store over the SAME data dir and
	// confirm the loop / PR / feed / findings are intact (PRD §6.5, §20). ----
	st.Close()
	st2, err := store.New() // same $RECUR_DATA_DIR
	if err != nil {
		t.Fatalf("reopen store: %v", err)
	}
	defer st2.Close()
	_ = dataDir

	l2, ok := st2.Workspace(id)
	if !ok {
		t.Fatal("loop missing after reload")
	}
	if l2.State != domain.StateMerged {
		t.Errorf("reloaded loop state: got %q, want merged", l2.State)
	}
	if len(l2.Convergence) != 2 {
		t.Errorf("reloaded loop lost its convergence series: got %d points, want 2", len(l2.Convergence))
	}
	if l2.VerifyState != "passed" || len(l2.Verify) != 3 {
		t.Errorf("reloaded loop lost verify evidence: state=%q steps=%d", l2.VerifyState, len(l2.Verify))
	}
	pr2, ok := st2.PRByWorkspace(id)
	if !ok {
		t.Fatal("PR missing after reload")
	}
	if pr2.State != "Merged" {
		t.Errorf("reloaded PR state: got %q, want Merged", pr2.State)
	}
	if pr2.FileCount != afterSend.FileCount || len(pr2.Files) == 0 {
		t.Errorf("reloaded PR lost its diff: fileCount=%d files=%d", pr2.FileCount, len(pr2.Files))
	}
	if len(pr2.Comments) < len(afterSend.Comments) {
		t.Errorf("reloaded PR lost its thread: got %d comments, want >= %d", len(pr2.Comments), len(afterSend.Comments))
	}
	if got := len(st2.Findings(id)); got != 2 {
		t.Errorf("reloaded loop lost its findings history: got %d, want 2", got)
	}
	// The feed is reconstructable from disk (history intact, §20).
	eng2 := New(st2)
	eng2.SetRunner(&ScriptedRunner{})
	if len(eng2.ReconstructFeed(id)) == 0 {
		t.Error("feed not reconstructable from disk after reload (history must survive)")
	}
}
