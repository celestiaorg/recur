package engine

import (
	"context"
	"strings"
	"sync"
	"testing"
	"time"

	"recur/internal/bundles"
	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

func TestCoderDoneStoresHandoffNotes(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	if _, err := st.AddWorkspace(domain.Workspace{ID: "L-hn", ProjectID: "P1", Goal: "t", State: domain.StateCoding}); err != nil {
		t.Fatal(err)
	}
	lp := &Loop{ID: "L-hn"}
	eng.loops.Store("L-hn", &runningLoop{loop: lp})
	t.Cleanup(func() { eng.loops.Delete("L-hn") })

	rep := engineReporter{eng: eng}
	if err := rep.Done("L-hn", "CO", "Add precheck", "  S2 needs splitting  "); err != nil {
		t.Fatalf("Done = %v, want nil", err)
	}
	if got := lp.takeDoneSummary(); got != "Add precheck" {
		t.Errorf("takeDoneSummary() = %q", got)
	}
	if got := lp.takeDoneNotes(); got != "S2 needs splitting" {
		t.Errorf("takeDoneNotes() = %q, want trimmed notes", got)
	}
	if got := lp.takeDoneNotes(); got != "" {
		t.Errorf("second takeDoneNotes() = %q, want empty", got)
	}
}

func TestHandoffSection(t *testing.T) {
	if got := handoffSection("   "); got != "" {
		t.Fatalf("empty handoff must add nothing, got %q", got)
	}
	got := handoffSection("Skipped the migration")
	if !strings.Contains(got, "Skipped the migration") || !strings.Contains(got, "is a finding") {
		t.Fatalf("handoff section should carry the notes and the judging rule, got %q", got)
	}
}

func TestAccumulateHandoff(t *testing.T) {
	if got := accumulateHandoff("S1: split parser", ""); got != "S1: split parser" {
		t.Errorf("accumulateHandoff keeps prev on empty next = %q", got)
	}
	if got := accumulateHandoff("S1: a", "S2: b"); got != "S1: a\nS2: b" {
		t.Errorf("accumulateHandoff append = %q", got)
	}
}

func TestPlanExecutionChangedDetectsFilesAndDeps(t *testing.T) {
	before := domain.Workspace{
		DefinitionOfDone: "Importer\n\nSteps:\n1. Parser\n2. Writer\n",
		Plan: &domain.Plan{
			Body: "Importer",
			Steps: []domain.PlanStep{
				{ID: "S1", Title: "Parser", Files: []string{"a.go"}},
				{ID: "S2", Title: "Writer", DependsOn: []string{"S1"}},
			},
		},
	}
	after := before
	after.Plan = &domain.Plan{
		Body: "Importer",
		Steps: []domain.PlanStep{
			{ID: "S1", Title: "Parser", Files: []string{"parser.go"}},
			{ID: "S2", Title: "Writer", DependsOn: []string{"S1"}},
		},
	}
	if !planExecutionChanged(before, after) {
		t.Fatal("planExecutionChanged(files-only) = false, want true")
	}
	after.Plan.Steps[0].Files = []string{"a.go"}
	if planExecutionChanged(before, after) {
		t.Fatal("planExecutionChanged(identical shape) = true, want false")
	}
}

// handoffRunner drives one shipped To-a-PR run through the real MCP path: the
// Coder files done WITH notes, the Maestro replan turn re-files the plan with
// an extra step, and the Reviewer records the briefing it was handed.
type handoffRunner struct {
	mu            sync.Mutex
	reviewerBrief string
	replanRuns    int
}

func (r *handoffRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	switch spec.Role {
	case domain.RoleCoder:
		sr := &ScriptedRunner{Edits: map[string]string{"work.txt": "edit\n"}}
		if err := sr.applyEdits(spec.Worktree); err != nil {
			return Result{}, err
		}
		if err := closeOpenFindingsMCP(spec.MCPURL); err != nil {
			return Result{}, err
		}
		if err := postMCPTool(spec.MCPURL, "done", map[string]any{
			"summary": "Wire the precheck", "notes": "S2 is too big: split the parser from the writer",
		}); err != nil {
			return Result{}, err
		}
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil
	case domain.RoleMaestro:
		if !strings.Contains(spec.SystemPrompt, "Implementer handoff:") {
			return Result{Subtype: "success"}, nil
		}
		r.mu.Lock()
		r.replanRuns++
		r.mu.Unlock()
		if err := postMCPTool(spec.MCPURL, "submit_plan", map[string]any{
			"summary": "Precheck", "body": "Split per handoff.",
			"steps": []string{"Add the precheck", "Split: parser", "Split: writer"},
		}); err != nil {
			return Result{}, err
		}
		return Result{Subtype: "success"}, nil
	case domain.RoleReviewer:
		r.mu.Lock()
		r.reviewerBrief = spec.SystemPrompt
		r.mu.Unlock()
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil
	}
	return Result{Subtype: "success"}, nil
}

func TestHandoffBriefsReviewerAndReplans(t *testing.T) {
	repo := initRepo(t)
	runner := &handoffRunner{}
	eng, st, _, _ := newTestEngine(t, runner)
	installShippedLoopPack(t)
	projID := addProject(t, st, repo)

	rec, acquired, err := eng.queueStartWorkspace(projID, "Add a precheck", "", "", "", bundles.DefaultLoopID, "recur.policies.aggressive", nil)
	if err != nil || !acquired {
		t.Fatalf("queueStartWorkspace: %v acquired=%v", err, acquired)
	}
	plan := domain.Plan{
		Summary: "Precheck", Body: "Add the precheck.", Approved: true, Source: "human",
		Steps: []domain.PlanStep{{ID: "S1", Title: "Add the precheck", Status: domain.StepPending}, {ID: "S2", Title: "Wire it", Status: domain.StepPending}},
	}
	rec.Plan = &plan
	rec.DefinitionOfDone = plan.Text()
	if err := eng.dispatch(rec); err != nil {
		t.Fatalf("dispatch: %v", err)
	}
	waitFor(t, 20*time.Second, "loop to open a PR", func() bool {
		l, ok := st.Workspace(rec.ID)
		return ok && (l.State == domain.StatePROpen || l.State == domain.StateFailed || l.State == domain.StateStopped)
	})
	ws, _ := st.Workspace(rec.ID)
	if ws.State != domain.StatePROpen {
		t.Fatalf("state = %s (%s), want pr_open", ws.State, ws.Phase)
	}
	if !strings.Contains(ws.Handoff, "S2 is too big") {
		t.Errorf("Workspace.Handoff = %q, want the coder's notes", ws.Handoff)
	}
	runner.mu.Lock()
	brief, replans := runner.reviewerBrief, runner.replanRuns
	runner.mu.Unlock()
	if !strings.Contains(brief, "S2 is too big") {
		t.Errorf("reviewer briefing should carry the handoff, got %q", truncate(brief, 400))
	}
	// Uniform fan-out: the two no-files steps ran as one child; its handoff
	// triggered a replan that added steps, and the revised board was
	// re-evaluated — the new steps ran as a SECOND child (not a parent turn)
	// and handed off again. Every handoff gets a replan turn.
	if replans < 1 {
		t.Errorf("replan turns = %d, want at least 1", replans)
	}
	if ws.Plan == nil || len(ws.Plan.Steps) != 3 {
		t.Fatalf("plan should have been revised to 3 steps, got %+v", ws.Plan)
	}
	children := 0
	for _, w := range st.Workspaces() {
		if w.ParentID == rec.ID {
			children++
		}
	}
	if children != 2 {
		t.Errorf("children = %d, want 2 (the replanned steps fan out too)", children)
	}
	for _, s := range ws.Plan.Steps {
		if s.Status != domain.StepDone {
			t.Errorf("step %s = %s, want done (merged from its child)", s.ID, s.Status)
		}
	}
	if got := maestroSystemPrompt(eng.maestroContext(ws)); !strings.Contains(got, "S2 is too big") {
		t.Errorf("Maestro context should carry the last handoff")
	}
}

func TestRecordHandoffAccumulatesWithinBoardThenResets(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddWorkspace(domain.Workspace{ID: "L-ho", ProjectID: "P1", Goal: "t", State: domain.StateCoding}); err != nil {
		t.Fatal(err)
	}
	lp := &Loop{ID: "L-ho", eng: eng}
	rs := roleSpec{code: "CO", name: "Coder"}
	lp.recordHandoff(rs, "S1: unfinished parser")
	lp.recordHandoff(rs, "S2: split writer")
	if got := lp.reviewHandoff(); got != "S1: unfinished parser\nS2: split writer" {
		t.Errorf("reviewHandoff() = %q, want accumulated child notes", got)
	}
	lp.recordHandoff(rs, "")
	if got := lp.reviewHandoff(); got != "S1: unfinished parser\nS2: split writer" {
		t.Errorf("reviewHandoff() after empty parent fallback = %q, want child notes kept", got)
	}

	lp.resetBoardHandoff()
	lp.recordHandoff(rs, "")
	if got := lp.reviewHandoff(); got != "" {
		t.Errorf("reviewHandoff() after new coding round = %q, want empty", got)
	}
	ws, _ := st.Workspace("L-ho")
	if ws.Handoff != "" {
		t.Errorf("Workspace.Handoff = %q, want cleared for the new round", ws.Handoff)
	}
}

func TestRunBoardResumeKeepsLandedHandoff(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, nil)
	ctx := context.Background()
	m := gitpkg.NewManager(repo, eng.worktreeRoot)
	parentWT, _, err := m.Add(ctx, "WS-ho-parent", "ho-parent", "main")
	if err != nil {
		t.Fatal(err)
	}
	s2WT, _, err := m.Add(ctx, "WS-ho-s2", "ho-s2", parentWT.Branch)
	if err != nil {
		t.Fatal(err)
	}
	commitWorktreeFile(t, m, s2WT.Path, "s2.txt", "from s2\n", "s2 work")

	parentID, s1ID, s2ID := "WS-ho-parent", "WS-ho-s1", "WS-ho-s2"
	note := "S1: concern about parser"
	plan := domain.Plan{Steps: []domain.PlanStep{
		{ID: "S1", Title: "parser", Status: domain.StepDone, Files: []string{"s1.txt"}, Note: note},
		{ID: "S2", Title: "writer", Status: domain.StepDoing, Files: []string{"s2.txt"}, Workspace: s2ID},
	}}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: parentID, Goal: "parent", State: domain.StateCoding, Plan: &plan,
		WorktreePath: parentWT.Path, Branch: parentWT.Branch, Handoff: note, FanOutWaves: 1,
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: s1ID, ParentID: parentID, Goal: "parser", State: domain.StateMerged, Archived: true,
		ParentStepIDs: []string{"S1"}, FanOut: &domain.ChildFanOut{Wave: 1, Outcome: "merged", Note: "concern about parser"},
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: s2ID, ParentID: parentID, Goal: "writer", State: domain.StatePROpen,
		Branch: s2WT.Branch, WorktreePath: s2WT.Path, ParentStepIDs: []string{"S2"},
		FanOut: &domain.ChildFanOut{Wave: 1},
	}); err != nil {
		t.Fatal(err)
	}
	lp := &Loop{
		ID: parentID, eng: eng, git: m, Worktree: parentWT.Path,
		planStructured: true, boardHandoff: note, lastHandoff: note,
	}
	ran, stop, err := lp.runBoard(ctx, 1)
	if err != nil || stop != "" {
		t.Fatalf("runBoard() ran=%v stop=%q err=%v", ran, stop, err)
	}
	if got := lp.reviewHandoff(); !strings.Contains(got, "concern about parser") {
		t.Errorf("reviewHandoff() = %q, want landed S1 notes kept after empty S2 merge", got)
	}
	parent, _ := st.Workspace(parentID)
	if parent.Plan.Steps[1].Status != domain.StepDone {
		t.Errorf("runBoard() S2 status = %s, want done", parent.Plan.Steps[1].Status)
	}
}
