package engine

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"sync"
	"testing"
	"time"

	"recur/internal/bundles"
	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

func TestReadyStepsAndGrouping(t *testing.T) {
	p := domain.Plan{Steps: []domain.PlanStep{
		{ID: "S1", Title: "parser", Status: domain.StepPending, Files: []string{"a.go"}},
		{ID: "S2", Title: "writer", Status: domain.StepPending, Files: []string{"b.go"}},
		{ID: "S3", Title: "shares a.go", Status: domain.StepPending, Files: []string{"a.go", "c.go"}},
		{ID: "S4", Title: "needs S1", Status: domain.StepPending, Files: []string{"d.go"}, DependsOn: []string{"S1"}},
		{ID: "S5", Title: "no files", Status: domain.StepPending},
		{ID: "S6", Title: "already done", Status: domain.StepDone, Files: []string{"e.go"}},
		{ID: "S7", Title: "dep done", Status: domain.StepPending, Files: []string{"f.go"}, DependsOn: []string{"S6", "S99"}},
		{ID: "S8", Title: "no files either", Status: domain.StepPending},
		{ID: "S9", Title: "excluded", Status: domain.StepPending, Files: []string{"g.go"}},
	}}
	ready := readySteps(p, map[string]bool{"S9": true})
	if got := stepIDs(ready); got != "S1, S2, S3, S5, S7, S8" {
		t.Fatalf("readySteps = %q (S4 blocked, S6 done, S9 excluded)", got)
	}
	groups := groupByFiles(ready)
	var got []string
	for _, g := range groups {
		got = append(got, stepIDs(g))
	}
	// S1+S3 share a.go; S5+S8 have no files and share one child; S2, S7 alone.
	if strings.Join(got, " | ") != "S1, S3 | S2 | S5, S8 | S7" {
		t.Fatalf("groupByFiles = %q", strings.Join(got, " | "))
	}
	p.Steps[0].Status, p.Steps[0].Workspace = domain.StepDoing, "child-1"
	p.Steps[2].Status, p.Steps[2].Workspace = domain.StepDoing, "child-1"
	ids, owned := inFlightChildren(p)
	if len(ids) != 1 || ids[0] != "child-1" || stepIDs(owned["child-1"]) != "S1, S3" {
		t.Fatalf("inFlightChildren = %v %v", ids, owned)
	}

	p.Steps[1].Workspace = "child-fail"
	ready = readySteps(p, map[string]bool{"child-fail": true})
	if got := stepIDs(ready); strings.Contains(got, "S2") {
		t.Fatalf("readySteps after workspace exclude = %q, S2 should stay excluded across id remaps", got)
	}

	// Replan inserts a step: failed S2 is now S3. Exclude the child id only —
	// a stale S2 key would suppress the new unrelated S2.
	inserted := domain.Plan{Steps: []domain.PlanStep{
		{ID: "S1", Title: "new first", Status: domain.StepPending, Files: []string{"z.go"}},
		{ID: "S2", Title: "unrelated", Status: domain.StepPending, Files: []string{"b.go"}},
		{ID: "S3", Title: "writer", Status: domain.StepPending, Files: []string{"w.go"}, Workspace: "child-fail"},
	}}
	ready = readySteps(inserted, map[string]bool{"child-fail": true})
	if got := stepIDs(ready); got != "S1, S2" {
		t.Fatalf("readySteps after insert reused S2 = %q, want S1, S2", got)
	}
}

func TestChildIncomplete(t *testing.T) {
	if !childIncomplete(domain.Workspace{State: domain.StateFailed}) {
		t.Error("childIncomplete(failed) = false, want true")
	}
	if !childIncomplete(domain.Workspace{State: domain.StatePaused, Phase: "loop refs missing"}) {
		t.Error("childIncomplete(paused loop refs missing) = false, want true")
	}
	if !childIncomplete(domain.Workspace{State: domain.StatePaused, Phase: "phase missing"}) {
		t.Error("childIncomplete(paused phase missing) = false, want true")
	}
	if childIncomplete(domain.Workspace{State: domain.StatePaused, Phase: "idle"}) {
		t.Error("childIncomplete(paused idle) = true, want false")
	}
	if childIncomplete(domain.Workspace{State: domain.StatePROpen}) {
		t.Error("childIncomplete(pr_open) = true, want false")
	}
}

// fanOutRunner: every Coder edits a file named after its workspace so
// children touch disjoint paths. It records which worktrees ran a coder turn.
type fanOutRunner struct {
	mu        sync.Mutex
	coderWork []string // worktree paths that ran a coder turn
	parentID  string   // set by the test: turns on other workspaces are children
	failOn    string   // a CHILD coder turn whose prompt mentions this fails (parent turns run)
	failed    int
	slowOn    string // a coder turn whose prompt mentions this sleeps slowDelay first
	slowDelay time.Duration
	started   map[string]time.Time // step title → coder turn start (titles in slowOn/markOn)
	finished  map[string]time.Time
	markOn    []string // titles whose turn timings are recorded
}

func (r *fanOutRunner) stamp(prompt string, m *map[string]time.Time) {
	for _, t := range r.markOn {
		if strings.Contains(prompt, t) {
			if *m == nil {
				*m = map[string]time.Time{}
			}
			if _, seen := (*m)[t]; !seen {
				(*m)[t] = time.Now()
			}
		}
	}
}

func (r *fanOutRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	switch spec.Role {
	case domain.RoleCoder:
		r.mu.Lock()
		r.coderWork = append(r.coderWork, spec.Worktree)
		child := r.parentID != "" && spec.WorkspaceID != r.parentID
		fail := r.failOn != "" && child && strings.Contains(spec.SystemPrompt+spec.Prompt, r.failOn)
		if fail {
			r.failed++
		}
		r.mu.Unlock()
		if fail {
			return Result{Subtype: "error", IsError: true, SessionID: spec.AgentID}, nil
		}
		text := spec.SystemPrompt + " " + spec.Prompt
		r.mu.Lock()
		r.stamp(text, &r.started)
		slow := r.slowOn != "" && strings.Contains(text, r.slowOn)
		r.mu.Unlock()
		if slow {
			time.Sleep(r.slowDelay)
		}
		defer func() {
			r.mu.Lock()
			r.stamp(text, &r.finished)
			r.mu.Unlock()
		}()
		name := filepath.Base(spec.Worktree) + ".txt"
		sr := &ScriptedRunner{Edits: map[string]string{name: "done by " + spec.WorkspaceID + "\n"}}
		if err := sr.applyEdits(spec.Worktree); err != nil {
			return Result{}, err
		}
		if spec.MCPURL != "" {
			if err := closeOpenFindingsMCP(spec.MCPURL); err != nil {
				return Result{}, err
			}
			// A child's plan steps are S1..Sn; the parent closes its own steps on
			// merge. A parent fallback turn carries the plan in the system prompt
			// and closes the steps it finishes itself (unknown ids are refused).
			ids := stepIDsIn(spec.SystemPrompt + " " + spec.Prompt)
			if len(ids) == 0 {
				ids = []string{"S1"}
			}
			for _, id := range ids {
				_ = postMCPTool(spec.MCPURL, "update_plan_step", map[string]any{"step_id": id, "status": "done"})
			}
			_ = postMCPTool(spec.MCPURL, "done", map[string]any{"summary": "Step done", "notes": "no deviations for " + spec.WorkspaceID})
		}
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil
	}
	return Result{Subtype: "success", SessionID: spec.AgentID}, nil
}

func TestFanOutRunsIndependentStepsInChildWorkspaces(t *testing.T) {
	repo := initRepo(t)
	runner := &fanOutRunner{}
	eng, st, _, _ := newTestEngine(t, runner)
	eng.fanOutPoll = 50 * time.Millisecond
	installShippedLoopPack(t)
	projID := addProject(t, st, repo)

	rec, acquired, err := eng.queueStartWorkspace(projID, "Build the importer", "", "", "", bundles.DefaultLoopID, "recur.policies.aggressive", nil)
	if err != nil || !acquired {
		t.Fatalf("queueStartWorkspace: %v acquired=%v", err, acquired)
	}
	plan := domain.Plan{
		Summary: "Importer", Body: "Parser and writer are independent.", Approved: true, Source: "human",
		Steps: []domain.PlanStep{
			{ID: "S1", Title: "Write the parser", Status: domain.StepPending, Files: []string{"parser.go"}},
			{ID: "S2", Title: "Write the writer", Status: domain.StepPending, Files: []string{"writer.go"}},
		},
	}
	rec.Plan = &plan
	rec.DefinitionOfDone = plan.Text()
	if err := eng.dispatch(rec); err != nil {
		t.Fatalf("dispatch: %v", err)
	}
	waitFor(t, 30*time.Second, "parent to open a PR", func() bool {
		l, ok := st.Workspace(rec.ID)
		return ok && (l.State == domain.StatePROpen || l.State == domain.StateFailed || l.State == domain.StateStopped)
	})
	parent, _ := st.Workspace(rec.ID)
	if parent.State != domain.StatePROpen {
		t.Fatalf("parent state = %s (%s), want pr_open", parent.State, parent.Phase)
	}

	// Two children, both linked to the parent and their step, now merged+archived.
	var children []domain.Workspace
	for _, w := range st.Workspaces() {
		if w.ParentID == rec.ID {
			children = append(children, w)
		}
	}
	if len(children) != 2 {
		t.Fatalf("children = %d, want 2", len(children))
	}
	for _, c := range children {
		if c.State != domain.StateMerged || !c.Archived {
			t.Errorf("child %s state=%s archived=%v, want merged+archived", c.ID, c.State, c.Archived)
		}
		if len(c.ParentStepIDs) != 1 || (c.ParentStepIDs[0] != "S1" && c.ParentStepIDs[0] != "S2") {
			t.Errorf("child %s ParentStepIDs = %v", c.ID, c.ParentStepIDs)
		}
		if c.BaseBranch != parent.Branch {
			t.Errorf("child %s base = %q, want parent branch %q", c.ID, c.BaseBranch, parent.Branch)
		}
		if c.LoopID != bundles.StepLoopID {
			t.Errorf("child %s loop = %q", c.ID, c.LoopID)
		}
	}

	// Both steps done, notes carry the child handoff, child id recorded.
	if parent.Plan == nil || len(parent.Plan.Steps) != 2 {
		t.Fatalf("parent plan = %+v", parent.Plan)
	}
	for _, s := range parent.Plan.Steps {
		if s.Status != domain.StepDone {
			t.Errorf("step %s status = %s, want done (note %q)", s.ID, s.Status, s.Note)
		}
		if !strings.Contains(s.Note, "no deviations") || s.Workspace == "" {
			t.Errorf("step %s should carry the child handoff and id, got note=%q ws=%q", s.ID, s.Note, s.Workspace)
		}
	}

	// One fan-out card for the wave, not a line per child; final state merged.
	cards := 0
	for _, f := range st.Feed(rec.ID) {
		if f.Card != domain.FeedCardFanOut {
			if strings.Contains(f.Text, "child workspace <code>") {
				t.Errorf("per-child feed line should be replaced by the wave card: %q", f.Text)
			}
			continue
		}
		cards++
		var w fanOutWave
		if err := json.Unmarshal([]byte(f.Data), &w); err != nil || w.Wave != 1 || len(w.Children) != 2 || !w.Done {
			t.Errorf("wave card = %+v (%v), want wave 1, 2 children, done", w, err)
		}
		for _, c := range w.Children {
			if c.State != "merged" || len(c.Steps) != 1 {
				t.Errorf("card child %+v, want merged with one step", c)
			}
		}
	}
	if cards != 1 {
		t.Errorf("fan-out cards in the feed = %d, want exactly 1 (re-emitted under one id)", cards)
	}

	// Both children's files landed in the parent worktree via merge commits.
	entries, err := os.ReadDir(parent.WorktreePath)
	if err != nil {
		t.Fatal(err)
	}
	txt := 0
	for _, e := range entries {
		if strings.HasSuffix(e.Name(), ".txt") {
			txt++
		}
	}
	if txt != 2 {
		t.Errorf("parent worktree has %d step files, want 2", txt)
	}

	// The coder never ran in the parent worktree: every step landed from a child.
	runner.mu.Lock()
	defer runner.mu.Unlock()
	for _, wt := range runner.coderWork {
		if wt == parent.WorktreePath {
			t.Errorf("coder ran in the parent worktree; fan-out should have covered every step")
		}
	}
	if len(runner.coderWork) != 2 {
		t.Errorf("coder turns = %d, want 2 (one per child)", len(runner.coderWork))
	}
}

// plannedFanOutRunner is fanOutRunner plus a real planner turn: the shipped
// Planner role (PL) files a structured plan with files through MCP. This is
// the path the dock takes — no pre-seeded plan — and it must still fan out.
type plannedFanOutRunner struct {
	fanOutRunner
	planned int
}

func (r *plannedFanOutRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if spec.Role == domain.RoleMaestro && strings.Contains(strings.Join(spec.AllowedTools, ","), "submit_plan") &&
		!strings.Contains(spec.SystemPrompt, "Implementer handoff:") {
		r.mu.Lock()
		r.planned++
		r.mu.Unlock()
		if err := postMCPTool(spec.MCPURL, "submit_plan", map[string]any{
			"summary": "Importer", "body": "Parser and writer are independent.",
			"steps": []any{
				map[string]any{"title": "Write the parser", "files": []string{"parser.go"}},
				map[string]any{"title": "Write the writer", "files": []string{"writer.go"}},
			},
		}); err != nil {
			return Result{}, err
		}
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil
	}
	return r.fanOutRunner.Run(ctx, spec, emit)
}

// A step whose child fails is excluded from the very next wave — the parent
// finishes it in one fallback turn — while its independent sibling's child
// merges normally. No second child is ever spawned for the failed step.
func TestFailedChildIsNotRespawned(t *testing.T) {
	repo := initRepo(t)
	runner := &fanOutRunner{failOn: "Write the writer"}
	eng, st, _, _ := newTestEngine(t, runner)
	eng.fanOutPoll = 50 * time.Millisecond
	installShippedLoopPack(t)
	projID := addProject(t, st, repo)

	rec, acquired, err := eng.queueStartWorkspace(projID, "Build the importer", "", "", "", bundles.DefaultLoopID, "recur.policies.aggressive", nil)
	if err != nil || !acquired {
		t.Fatalf("queueStartWorkspace: %v acquired=%v", err, acquired)
	}
	plan := domain.Plan{
		Summary: "Importer", Body: "Parser and writer are independent.", Approved: true, Source: "human",
		Steps: []domain.PlanStep{
			{ID: "S1", Title: "Write the parser", Status: domain.StepPending, Files: []string{"parser.go"}},
			{ID: "S2", Title: "Write the writer", Status: domain.StepPending, Files: []string{"writer.go"}},
		},
	}
	rec.Plan = &plan
	rec.DefinitionOfDone = plan.Text()
	runner.mu.Lock()
	runner.parentID = rec.ID
	runner.mu.Unlock()
	if err := eng.dispatch(rec); err != nil {
		t.Fatalf("dispatch: %v", err)
	}
	waitFor(t, 30*time.Second, "parent to open a PR", func() bool {
		l, ok := st.Workspace(rec.ID)
		return ok && (l.State == domain.StatePROpen || l.State == domain.StateFailed || l.State == domain.StateStopped)
	})
	parent, _ := st.Workspace(rec.ID)
	if parent.State != domain.StatePROpen {
		t.Fatalf("parent state = %s (%s), want pr_open", parent.State, parent.Phase)
	}
	writerChildren := 0
	for _, w := range st.Workspaces() {
		if w.ParentID == rec.ID && len(w.ParentStepIDs) == 1 && w.ParentStepIDs[0] == "S2" {
			writerChildren++
		}
	}
	runner.mu.Lock()
	failed := runner.failed
	parentTurns := 0
	for _, wt := range runner.coderWork {
		if wt == parent.WorktreePath {
			parentTurns++
		}
	}
	runner.mu.Unlock()
	if writerChildren != 1 || failed != 1 {
		t.Errorf("writer step: children=%d failed turns=%d, want exactly one of each", writerChildren, failed)
	}
	if parentTurns != 1 {
		t.Errorf("parent coder turns = %d, want 1 fallback turn for the failed step", parentTurns)
	}
	for _, s := range parent.Plan.Steps {
		if s.Status != domain.StepDone {
			t.Errorf("step %s = %s, want done", s.ID, s.Status)
		}
	}
}

// No wave barrier: a slow child does not hold back its fast sibling's
// dependents. S1 and S2 start together; S3 (after S1) starts as soon as S1
// lands, while S2 is still running. The second wave records what started it.
func TestFastChildUnblocksDependentWhileSiblingRuns(t *testing.T) {
	repo := initRepo(t)
	runner := &fanOutRunner{slowOn: "Write the slow writer", slowDelay: 2 * time.Second,
		markOn: []string{"Write the slow writer", "Wire the parser"}}
	eng, st, _, _ := newTestEngine(t, runner)
	eng.fanOutPoll = 50 * time.Millisecond
	installShippedLoopPack(t)
	projID := addProject(t, st, repo)

	rec, acquired, err := eng.queueStartWorkspace(projID, "Build the importer", "", "", "", bundles.DefaultLoopID, "recur.policies.aggressive", nil)
	if err != nil || !acquired {
		t.Fatalf("queueStartWorkspace: %v acquired=%v", err, acquired)
	}
	plan := domain.Plan{
		Summary: "Importer", Body: "Parser first, then wiring; the writer is independent.", Approved: true, Source: "human",
		Steps: []domain.PlanStep{
			{ID: "S1", Title: "Write the parser", Status: domain.StepPending, Files: []string{"parser.go"}},
			{ID: "S2", Title: "Write the slow writer", Status: domain.StepPending, Files: []string{"writer.go"}},
			{ID: "S3", Title: "Wire the parser", Status: domain.StepPending, Files: []string{"main.go"}, DependsOn: []string{"S1"}},
		},
	}
	rec.Plan = &plan
	rec.DefinitionOfDone = plan.Text()
	runner.mu.Lock()
	runner.parentID = rec.ID
	runner.mu.Unlock()
	if err := eng.dispatch(rec); err != nil {
		t.Fatalf("dispatch: %v", err)
	}
	waitFor(t, 40*time.Second, "parent to open a PR", func() bool {
		l, ok := st.Workspace(rec.ID)
		return ok && (l.State == domain.StatePROpen || l.State == domain.StateFailed || l.State == domain.StateStopped)
	})
	parent, _ := st.Workspace(rec.ID)
	if parent.State != domain.StatePROpen {
		t.Fatalf("parent state = %s (%s), want pr_open", parent.State, parent.Phase)
	}
	runner.mu.Lock()
	s3Start, s2End := runner.started["Wire the parser"], runner.finished["Write the slow writer"]
	runner.mu.Unlock()
	if s3Start.IsZero() || s2End.IsZero() {
		t.Fatalf("timings missing: s3 start=%v s2 end=%v", s3Start, s2End)
	}
	if !s3Start.Before(s2End) {
		t.Errorf("S3's child started at %v, after the slow S2 child finished at %v — the wave barrier is back", s3Start, s2End)
	}
	if parent.FanOutWaves != 2 {
		t.Errorf("FanOutWaves = %d, want 2 (S1+S2, then S3 after S1)", parent.FanOutWaves)
	}
	for _, w := range st.Workspaces() {
		if w.ParentID != rec.ID || len(w.ParentStepIDs) != 1 || w.ParentStepIDs[0] != "S3" {
			continue
		}
		if w.FanOut == nil || w.FanOut.Wave != 2 || len(w.FanOut.After) != 1 || w.FanOut.After[0] != "S1" {
			t.Errorf("S3 child FanOut = %+v, want wave 2 after S1", w.FanOut)
		}
		if w.FanOut != nil && w.FanOut.Outcome != "merged" {
			t.Errorf("S3 child outcome = %q, want merged", w.FanOut.Outcome)
		}
	}
	for _, s := range parent.Plan.Steps {
		if s.Status != domain.StepDone {
			t.Errorf("step %s = %s, want done", s.ID, s.Status)
		}
	}
}

func TestPlannerFiledPlanFansOut(t *testing.T) {
	repo := initRepo(t)
	runner := &plannedFanOutRunner{}
	eng, st, _, _ := newTestEngine(t, runner)
	eng.fanOutPoll = 50 * time.Millisecond
	installShippedLoopPack(t)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspaceWith(projID, "Build the importer", "", "", bundles.DefaultLoopID, "recur.policies.aggressive")
	if err != nil {
		t.Fatalf("StartWorkspaceWith: %v", err)
	}
	waitFor(t, 30*time.Second, "parent to open a PR", func() bool {
		l, ok := st.Workspace(id)
		return ok && (l.State == domain.StatePROpen || l.State == domain.StateFailed || l.State == domain.StateStopped)
	})
	parent, _ := st.Workspace(id)
	if parent.State != domain.StatePROpen {
		t.Fatalf("parent state = %s (%s), want pr_open", parent.State, parent.Phase)
	}
	if parent.Plan == nil || parent.Plan.Source != "loop-planner" || len(parent.Plan.Steps) != 2 {
		t.Fatalf("planner-filed plan should be adopted as loop-planner with 2 steps, got %+v", parent.Plan)
	}
	planCards := 0
	for _, f := range st.Feed(id) {
		if strings.Contains(f.Text, "definition-of-done:") {
			t.Errorf("a structured filing must not also dump the plan body into the feed: %q", truncate(f.Text, 80))
		}
		if f.Card == domain.FeedCardPlan {
			planCards++
			var d planCardData
			if err := json.Unmarshal([]byte(f.Data), &d); err != nil || len(d.Steps) != 2 || d.Steps[0].Files[0] != "parser.go" {
				t.Errorf("plan card data = %+v (%v), want 2 steps with files", d, err)
			}
		}
	}
	if planCards != 1 {
		t.Errorf("plan cards = %d, want 1", planCards)
	}
	children := 0
	for _, w := range st.Workspaces() {
		if w.ParentID == id {
			children++
		}
	}
	if children != 2 {
		t.Errorf("children = %d, want 2 — a plan filed by the PL planner turn must fan out", children)
	}
	runner.mu.Lock()
	defer runner.mu.Unlock()
	if runner.planned != 1 {
		t.Errorf("planner turns = %d, want 1", runner.planned)
	}
	for _, wt := range runner.coderWork {
		if wt == parent.WorktreePath {
			t.Errorf("coder ran inline in the parent; steps should have fanned out")
		}
	}
}

// TestDocsLoopFansOutWithWriters: children are cut from the PARENT's converge
// phase, so a Docs plan fans out into Writer/Editor children (never the
// shipped Coder/Reviewer) and the parent still ends without a PR.
func TestDocsLoopFansOutWithWriters(t *testing.T) {
	repo := initRepo(t)
	runner := &fanOutRunner{}
	eng, st, _, _ := newTestEngine(t, runner)
	eng.fanOutPoll = 50 * time.Millisecond
	installShippedLoopPack(t)
	projID := addProject(t, st, repo)

	rec, acquired, err := eng.queueStartWorkspace(projID, "Write the docs", "", "", "", bundles.DocsLoopID, "recur.policies.aggressive", nil)
	if err != nil || !acquired {
		t.Fatalf("queueStartWorkspace: %v acquired=%v", err, acquired)
	}
	plan := domain.Plan{
		Summary: "Docs", Body: "Two independent pages.", Approved: true, Source: "human",
		Steps: []domain.PlanStep{
			{ID: "S1", Title: "Write install.md", Status: domain.StepPending, Files: []string{"docs/install.md"}},
			{ID: "S2", Title: "Write usage.md", Status: domain.StepPending, Files: []string{"docs/usage.md"}},
		},
	}
	rec.Plan = &plan
	rec.DefinitionOfDone = plan.Text()
	if err := eng.dispatch(rec); err != nil {
		t.Fatalf("dispatch: %v", err)
	}
	waitFor(t, 30*time.Second, "docs parent to settle", func() bool {
		l, ok := st.Workspace(rec.ID)
		return ok && (loopSettled(l.State, l.Phase) || l.State == domain.StateFailed || l.State == domain.StateStopped)
	})
	parent, _ := st.Workspace(rec.ID)
	if parent.State == domain.StatePROpen || parent.State == domain.StateFailed || parent.State == domain.StateStopped {
		t.Fatalf("docs parent state = %s (%s), want idle without a PR", parent.State, parent.Phase)
	}
	children := 0
	for _, w := range st.Workspaces() {
		if w.ParentID != rec.ID {
			continue
		}
		children++
		if w.ParentPhaseID != "converge" || w.LoopID != bundles.StepLoopID {
			t.Errorf("child %s phase=%q loop=%q, want cut from the parent's converge phase", w.ID, w.ParentPhaseID, w.LoopID)
		}
		codes := map[string]bool{}
		for _, a := range st.Agents(w.ID) {
			codes[a.Code] = true
		}
		if !codes["WR"] || !codes["ED"] {
			t.Errorf("child %s should run the parent's Writer+Editor, agents=%v", w.ID, codes)
		}
		if codes["CO"] || codes["RV"] {
			t.Errorf("child %s must not run the shipped Coder/Reviewer, agents=%v", w.ID, codes)
		}
	}
	if children != 2 {
		t.Errorf("children = %d, want 2", children)
	}
	for _, s := range parent.Plan.Steps {
		if s.Status != domain.StepDone {
			t.Errorf("step %s = %s, want done", s.ID, s.Status)
		}
	}
}

func TestChildLoopDefMirrorsParentConverge(t *testing.T) {
	two := 2
	parent := bundles.LoopDef{
		ID: "acme.loops.custom", Name: "Custom", Entry: "plan",
		Roles: map[string]string{"conductor": "acme.roles.boss"},
		Phases: []bundles.LoopPhase{
			{ID: "plan", Uses: bundles.VerbPlan, Next: "work"},
			{ID: "work", Uses: bundles.VerbConverge, Next: "pr", Gate: "plan_approval", Start: "review",
				MaxRounds: &two, OnMaxRounds: "proceed", Timeout: "10m", OnFail: "skip",
				Roles:   map[string]string{"implementer": "acme.roles.dev", "reviewer": "acme.roles.qa"},
				Prompts: map[string]string{"review": "/acme-review", "fix": "/acme-fix"}},
			{ID: "pr", Uses: bundles.VerbEnsurePR},
		},
	}
	ph, _ := parent.PhaseByID("work")
	child := childLoopDef(parent, ph)
	if child.ID != bundles.StepLoopID || child.Entry != "converge" || len(child.Phases) != 2 {
		t.Fatalf("child loop shape = %+v", child)
	}
	conv := child.Phases[0]
	if conv.Uses != bundles.VerbConverge || conv.Next != "done" || conv.Start != "" || conv.Gate != "" {
		t.Errorf("converge phase should always implement first and lead to done: %+v", conv)
	}
	if conv.Roles["implementer"] != "acme.roles.dev" || conv.Roles["reviewer"] != "acme.roles.qa" ||
		conv.Prompts["review"] != "/acme-review" || conv.Prompts["fix"] != "/acme-fix" {
		t.Errorf("child must keep the parent's role and prompt slots: %+v", conv)
	}
	if conv.MaxRounds == nil || *conv.MaxRounds != 2 || conv.OnMaxRounds != "proceed" || conv.Timeout != "10m" || conv.OnFail != "skip" {
		t.Errorf("child must keep the parent's phase overrides: %+v", conv)
	}
	if child.Phases[1].Uses != bundles.VerbArtifact || child.Roles["conductor"] != "acme.roles.boss" {
		t.Errorf("child ends at artifact under the parent's conductor: %+v", child)
	}
}

var stepIDRe = regexp.MustCompile(`\bS[0-9]+\b`)

func stepIDsIn(prompt string) []string {
	seen := map[string]bool{}
	var out []string
	for _, m := range stepIDRe.FindAllString(prompt, -1) {
		if !seen[m] {
			seen[m] = true
			out = append(out, m)
		}
	}
	return out
}

// TestBoardBootstrapThenFanOutThenTail is the shape a real planner produces:
// one bootstrap step everything depends on, independent steps behind it, and
// a verification tail that needs them all. Uniform fan-out: S1 runs as a
// child of one, S2+S3 fan out together, S4 runs as a child once they merge,
// then the parent reviews — the parent never codes.
func TestBoardBootstrapThenFanOutThenTail(t *testing.T) {
	repo := initRepo(t)
	runner := &fanOutRunner{}
	eng, st, _, _ := newTestEngine(t, runner)
	eng.fanOutPoll = 50 * time.Millisecond
	installShippedLoopPack(t)
	projID := addProject(t, st, repo)

	rec, acquired, err := eng.queueStartWorkspace(projID, "CSV to JSON", "", "", "", bundles.DefaultLoopID, "recur.policies.aggressive", nil)
	if err != nil || !acquired {
		t.Fatalf("queueStartWorkspace: %v acquired=%v", err, acquired)
	}
	plan := domain.Plan{
		Summary: "csv2json", Body: "Bootstrap, then parser and writer, then verify.", Approved: true, Source: "human",
		Steps: []domain.PlanStep{
			{ID: "S1", Title: "Create go.mod", Status: domain.StepPending, Files: []string{"go.mod"}},
			{ID: "S2", Title: "Write parser.go", Status: domain.StepPending, Files: []string{"parser.go"}, DependsOn: []string{"S1"}},
			{ID: "S3", Title: "Write writer.go", Status: domain.StepPending, Files: []string{"writer.go"}, DependsOn: []string{"S1"}},
			{ID: "S4", Title: "Verify end to end", Status: domain.StepPending, Files: []string{"parser.go", "writer.go"}, DependsOn: []string{"S2", "S3"}},
		},
	}
	rec.Plan = &plan
	rec.DefinitionOfDone = plan.Text()
	if err := eng.dispatch(rec); err != nil {
		t.Fatalf("dispatch: %v", err)
	}
	waitFor(t, 30*time.Second, "parent to open a PR", func() bool {
		l, ok := st.Workspace(rec.ID)
		return ok && (l.State == domain.StatePROpen || l.State == domain.StateFailed || l.State == domain.StateStopped)
	})
	parent, _ := st.Workspace(rec.ID)
	if parent.State != domain.StatePROpen {
		t.Fatalf("parent state = %s (%s), want pr_open", parent.State, parent.Phase)
	}
	children := 0
	for _, w := range st.Workspaces() {
		if w.ParentID == rec.ID {
			children++
			if len(w.ParentStepIDs) != 1 {
				t.Errorf("child %s owns %v, want one step each (disjoint files)", w.ID, w.ParentStepIDs)
			}
		}
	}
	if children != 4 {
		t.Errorf("children = %d, want 4 (S1; S2+S3 in parallel; S4)", children)
	}
	for _, s := range parent.Plan.Steps {
		if s.Status != domain.StepDone {
			t.Errorf("step %s = %s, want done (note %q)", s.ID, s.Status, s.Note)
		}
	}
	runner.mu.Lock()
	defer runner.mu.Unlock()
	inParent := 0
	for _, wt := range runner.coderWork {
		if wt == parent.WorktreePath {
			inParent++
		}
	}
	if inParent != 0 {
		t.Errorf("parent coder turns = %d, want 0 (the parent integrates, it does not code)", inParent)
	}
	waves := map[int]int{}
	for _, f := range st.Feed(rec.ID) {
		if f.Card != domain.FeedCardFanOut {
			continue
		}
		var w fanOutWave
		if err := json.Unmarshal([]byte(f.Data), &w); err == nil {
			waves[w.Wave] = len(w.Children)
		}
	}
	if len(waves) != 3 || waves[1] != 1 || waves[2] != 2 || waves[3] != 1 {
		t.Errorf("wave cards = %v, want 1:{S1} 2:{S2,S3} 3:{S4}", waves)
	}
	if parent.FanOutWaves != 3 {
		t.Errorf("FanOutWaves = %d, want 3 (persisted for resume)", parent.FanOutWaves)
	}
	if len(runner.coderWork) != 4 {
		t.Errorf("coder turns = %d, want 4 (one per child)", len(runner.coderWork))
	}
}

func TestFanOutSkipsChildrenButNotSingleSteps(t *testing.T) {
	l := &Loop{planStructured: true, parentID: "parent-1"}
	if l.fanOutEligible() {
		t.Fatal("a child workspace must never fan out")
	}
	l.parentID = ""
	if !l.fanOutEligible() {
		t.Fatal("structured bundle loop should be eligible")
	}
	p := domain.Plan{Steps: []domain.PlanStep{{ID: "S1", Status: domain.StepPending, Files: []string{"a"}}}}
	if got := groupByFiles(readySteps(p, nil)); len(got) != 1 || len(got[0]) != 1 {
		t.Fatalf("a lone ready step is still one child, got %v", got)
	}
}

func TestMarkConflictsResolvedUsesCurrentWorkspaceOwnership(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	parentID := "WS-conflict-parent"
	childID := "WS-conflict-child"
	plan := domain.Plan{Steps: []domain.PlanStep{
		{ID: "S1", Title: "inserted bootstrap", Status: domain.StepPending, Files: []string{"boot.go"}},
		{ID: "S2", Title: "conflicted work", Status: domain.StepDoing, Files: []string{"a.go"}, Workspace: childID},
	}}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: parentID, Goal: "parent", State: domain.StateCoding, Plan: &plan,
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: childID, ParentID: parentID, Goal: "conflicted work", State: domain.StatePROpen,
		ParentStepIDs: []string{"S1"},
		FanOut:        &domain.ChildFanOut{Wave: 1, Outcome: "conflict: a.go", Note: "landed dirty"},
	}); err != nil {
		t.Fatal(err)
	}
	l := &Loop{ID: parentID, eng: eng}
	l.markConflictsResolved()
	got, _ := st.Workspace(parentID)
	if got.Plan.Steps[0].Status != domain.StepPending {
		t.Errorf("markConflictsResolved() S1 status = %s, want pending (inserted step)", got.Plan.Steps[0].Status)
	}
	if got.Plan.Steps[1].Status != domain.StepDone {
		t.Errorf("markConflictsResolved() S2 status = %s, want done (conflicted step after replan)", got.Plan.Steps[1].Status)
	}
}

func TestPreserveChildWorkFailureKeepsWorktree(t *testing.T) {
	repo := initRepo(t)
	hook := filepath.Join(repo, ".git", "hooks", "pre-commit")
	if err := os.WriteFile(hook, []byte("#!/bin/sh\nexit 1\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	dirty := filepath.Join(repo, "wip.txt")
	if err := os.WriteFile(dirty, []byte("unfinished\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	eng, st, _, _ := newTestEngine(t, nil)
	parentID := "WS-preserve-parent"
	childID := "WS-preserve-child"
	plan := domain.Plan{Steps: []domain.PlanStep{
		{ID: "S1", Title: "work", Status: domain.StepDoing, Workspace: childID},
	}}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: parentID, Goal: "parent", State: domain.StateCoding, Plan: &plan, WorktreePath: repo,
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: childID, ParentID: parentID, Goal: "work", State: domain.StateFailed,
		WorktreePath: repo, Branch: "recur/child",
	}); err != nil {
		t.Fatal(err)
	}
	l := &Loop{ID: parentID, eng: eng, git: gitpkg.NewManager(repo, t.TempDir()), Worktree: repo}
	conflicts, merged, _, failed := l.mergeChild(context.Background(), childID, plan.Steps)
	if conflicts != nil || merged || !failed {
		t.Fatalf("mergeChild(preserve fail) conflicts=%v merged=%v failed=%v, want no merge and failed", conflicts, merged, failed)
	}
	child, ok := st.Workspace(childID)
	if !ok || child.Archived {
		t.Fatalf("mergeChild(preserve fail) archived child, want worktree retained")
	}
	if child.WorktreePath == "" {
		t.Fatal("mergeChild(preserve fail) cleared WorktreePath, want it kept")
	}
	if _, err := os.Stat(dirty); err != nil {
		t.Fatalf("mergeChild(preserve fail) lost dirty file: %v", err)
	}
	parent, _ := st.Workspace(parentID)
	if parent.Plan.Steps[0].Status != domain.StepPending {
		t.Errorf("mergeChild(preserve fail) step status = %s, want pending", parent.Plan.Steps[0].Status)
	}
}

func commitWorktreeFile(t *testing.T, m *gitpkg.Manager, dir, rel, content, msg string) {
	t.Helper()
	p := filepath.Join(dir, rel)
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(p, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := m.Commit(context.Background(), dir, msg, "test", "test@example.com"); err != nil {
		t.Fatalf("Commit(%s): %v", msg, err)
	}
}

// Resume while S2's merge is still in progress and S1 has since settled: recover
// S2 first and do not stamp S1 as conflicted from S2's unmerged paths.
func TestFanOutResumePrefersInProgressSiblingMerge(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, nil)
	ctx := context.Background()
	m := gitpkg.NewManager(repo, eng.worktreeRoot)
	parentWT, _, err := m.Add(ctx, "WS-resume-parent", "resume-parent", "main")
	if err != nil {
		t.Fatal(err)
	}
	s1WT, _, err := m.Add(ctx, "WS-resume-s1", "resume-s1", parentWT.Branch)
	if err != nil {
		t.Fatal(err)
	}
	s2WT, _, err := m.Add(ctx, "WS-resume-s2", "resume-s2", parentWT.Branch)
	if err != nil {
		t.Fatal(err)
	}
	commitWorktreeFile(t, m, s1WT.Path, "s1.txt", "from s1\n", "s1 work")
	commitWorktreeFile(t, m, s2WT.Path, "README.md", "s2 says hi\n", "s2 work")
	commitWorktreeFile(t, m, parentWT.Path, "README.md", "parent says hi\n", "parent edit")
	conflicts, err := m.MergeInto(ctx, parentWT.Path, s2WT.Branch, "Merge S2", "test", "test@example.com")
	if err != nil {
		t.Fatal(err)
	}
	if len(conflicts) == 0 {
		t.Fatal("MergeInto(S2) conflicts empty, want README conflict left in progress")
	}

	parentID, s1ID, s2ID := "WS-resume-parent", "WS-resume-s1", "WS-resume-s2"
	plan := domain.Plan{Steps: []domain.PlanStep{
		{ID: "S1", Title: "earlier sibling", Status: domain.StepDoing, Files: []string{"s1.txt"}, Workspace: s1ID},
		{ID: "S2", Title: "conflicted sibling", Status: domain.StepDoing, Files: []string{"README.md"}, Workspace: s2ID},
	}}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: parentID, Goal: "parent", State: domain.StateCoding, Plan: &plan, WorktreePath: parentWT.Path, Branch: parentWT.Branch,
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: s1ID, ParentID: parentID, Goal: "earlier sibling", State: domain.StatePROpen,
		Branch: s1WT.Branch, WorktreePath: s1WT.Path, ParentStepIDs: []string{"S1"},
		FanOut: &domain.ChildFanOut{Wave: 1},
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: s2ID, ParentID: parentID, Goal: "conflicted sibling", State: domain.StatePROpen,
		Branch: s2WT.Branch, WorktreePath: s2WT.Path, ParentStepIDs: []string{"S2"},
		FanOut: &domain.ChildFanOut{Wave: 1, Outcome: "conflict: README.md"},
	}); err != nil {
		t.Fatal(err)
	}
	l := &Loop{ID: parentID, eng: eng, git: m, Worktree: parentWT.Path, planStructured: true}

	res := l.fanOutSteps(ctx, map[string]bool{})
	if len(res.conflicts) == 0 {
		t.Fatal("fanOutSteps() conflicts empty, want the in-progress S2 merge")
	}
	s1, _ := st.Workspace(s1ID)
	if s1.FanOut != nil && strings.HasPrefix(s1.FanOut.Outcome, "conflict") {
		t.Errorf("fanOutSteps() S1 outcome = %q, want no conflict (S2's merge is in progress)", s1.FanOut.Outcome)
	}
	s2, _ := st.Workspace(s2ID)
	if s2.FanOut == nil || !strings.HasPrefix(s2.FanOut.Outcome, "conflict") {
		t.Errorf("fanOutSteps() S2 outcome = %+v, want conflict kept", s2.FanOut)
	}
	parent, _ := st.Workspace(parentID)
	if parent.Plan.Steps[0].Status != domain.StepDoing {
		t.Errorf("fanOutSteps() S1 status = %s, want doing (not closed as conflicted)", parent.Plan.Steps[0].Status)
	}
}

func TestMergeChildDoesNotClaimSiblingMerge(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, nil)
	ctx := context.Background()
	m := gitpkg.NewManager(repo, eng.worktreeRoot)
	parentWT, _, err := m.Add(ctx, "WS-claim-parent", "claim-parent", "main")
	if err != nil {
		t.Fatal(err)
	}
	s1WT, _, err := m.Add(ctx, "WS-claim-s1", "claim-s1", parentWT.Branch)
	if err != nil {
		t.Fatal(err)
	}
	s2WT, _, err := m.Add(ctx, "WS-claim-s2", "claim-s2", parentWT.Branch)
	if err != nil {
		t.Fatal(err)
	}
	commitWorktreeFile(t, m, s1WT.Path, "s1.txt", "from s1\n", "s1 work")
	commitWorktreeFile(t, m, s2WT.Path, "README.md", "s2 says hi\n", "s2 work")
	commitWorktreeFile(t, m, parentWT.Path, "README.md", "parent says hi\n", "parent edit")
	if _, err := m.MergeInto(ctx, parentWT.Path, s2WT.Branch, "Merge S2", "test", "test@example.com"); err != nil {
		t.Fatal(err)
	}

	parentID, s1ID, s2ID := "WS-claim-parent", "WS-claim-s1", "WS-claim-s2"
	s1Step := domain.PlanStep{ID: "S1", Title: "earlier sibling", Status: domain.StepDoing, Workspace: s1ID}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: parentID, Goal: "parent", State: domain.StateCoding, WorktreePath: parentWT.Path,
		Plan: &domain.Plan{Steps: []domain.PlanStep{s1Step, {ID: "S2", Title: "conflicted", Status: domain.StepDoing, Workspace: s2ID}}},
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: s1ID, ParentID: parentID, Goal: "earlier sibling", State: domain.StatePROpen,
		Branch: s1WT.Branch, WorktreePath: s1WT.Path, FanOut: &domain.ChildFanOut{Wave: 1},
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: s2ID, ParentID: parentID, Goal: "conflicted", State: domain.StatePROpen,
		Branch: s2WT.Branch, FanOut: &domain.ChildFanOut{Wave: 1, Outcome: "conflict: README.md"},
	}); err != nil {
		t.Fatal(err)
	}
	l := &Loop{ID: parentID, eng: eng, git: m, Worktree: parentWT.Path}
	conflicts, merged, _, failed := l.mergeChild(ctx, s1ID, []domain.PlanStep{s1Step})
	if len(conflicts) != 0 || merged || failed {
		t.Fatalf("mergeChild(S1 during S2 merge) conflicts=%v merged=%v failed=%v, want skip", conflicts, merged, failed)
	}
	s1, _ := st.Workspace(s1ID)
	if s1.FanOut != nil && strings.HasPrefix(s1.FanOut.Outcome, "conflict") {
		t.Errorf("mergeChild(S1) outcome = %q, want no conflict claimed from sibling MERGE_HEAD", s1.FanOut.Outcome)
	}
}

// TestStartStepChildDrainDuringInitKeepsParentMetadata: queueStartWorkspace
// inserts a queued row before ParentID/Plan are written. A sibling's
// loopExited → drainQueued in that window used to dispatch the incomplete
// row (ordinary Coder/Reviewer step loop, then overwrite child metadata and
// reset the parent's steps). dispatchMu now covers insert, init, and
// dispatch; this test drains in the old window.
func TestStartStepChildDrainDuringInitKeepsParentMetadata(t *testing.T) {
	repo := initRepo(t)
	gate := make(chan struct{})
	t.Cleanup(func() { close(gate) })
	eng, st, _, _ := newTestEngine(t, &blockingRunner{gate: gate})
	installShippedLoopPack(t)
	projID := addProject(t, st, repo)

	parentID := "WS-drain-parent"
	plan := domain.Plan{
		Summary: "Docs", Approved: true, Source: "human",
		Steps: []domain.PlanStep{
			{ID: "S1", Title: "Write install.md", Status: domain.StepPending, Files: []string{"docs/install.md"}},
		},
	}
	parentWS := domain.Workspace{
		ID: parentID, ProjectID: projID, Goal: "Write the docs",
		State: domain.StateCoding, Plan: &plan, Branch: "main",
		LoopID: bundles.DocsLoopID, PolicyID: "recur.policies.aggressive",
		Autonomy: domain.AutonomyAggressive,
	}
	if _, err := st.AddWorkspace(parentWS); err != nil {
		t.Fatal(err)
	}
	parent := &Loop{
		ID: parentID, ProjectID: projID, Branch: "main", eng: eng,
		Autonomy: domain.AutonomyAggressive, PolicyID: "recur.policies.aggressive",
		currentPhase: bundles.LoopPhase{ID: "converge", Uses: bundles.VerbConverge},
	}

	var drainWG sync.WaitGroup
	eng.childQueueHook = func() {
		drainWG.Add(1)
		go func() {
			defer drainWG.Done()
			eng.drainQueued()
		}()
		time.Sleep(40 * time.Millisecond)
	}

	id, err := eng.startStepChild(parent, parentWS, plan.Steps, 1, nil)
	if err != nil {
		t.Fatalf("startStepChild: %v", err)
	}
	drainWG.Wait()

	child, ok := st.Workspace(id)
	if !ok {
		t.Fatal("child workspace missing")
	}
	if child.ParentID != parentID {
		t.Errorf("startStepChild() child.ParentID = %q, want %s", child.ParentID, parentID)
	}
	if child.ParentPhaseID != "converge" {
		t.Errorf("startStepChild() child.ParentPhaseID = %q, want converge", child.ParentPhaseID)
	}
	if child.Plan == nil || len(child.Plan.Steps) == 0 {
		t.Errorf("startStepChild() child.Plan = %+v, want slice plan", child.Plan)
	}
	gotParent, _ := st.Workspace(parentID)
	if gotParent.Plan.Steps[0].Status != domain.StepDoing || gotParent.Plan.Steps[0].Workspace != id {
		t.Errorf("startStepChild() parent step = %+v, want doing linked to %s", gotParent.Plan.Steps[0], id)
	}

	lp := eng.runningLoopByID(id)
	if lp == nil {
		t.Fatal("child loop not running after drain during init")
	}
	if lp.parentID != parentID {
		t.Errorf("newLoop parentID = %q, want %s (drain dispatched before init)", lp.parentID, parentID)
	}
	ph, ok := lp.guard.Loop.PhaseByID("converge")
	if !ok || ph.Roles["implementer"] != "recur.roles.writer" || ph.Roles["reviewer"] != "recur.roles.editor" {
		t.Errorf("child converge roles = %v, want writer/editor from docs parent", ph.Roles)
	}
}

// TestStopInFlightChildrenStopsQueuedChild: a child linked to a doing parent
// step can sit queued with no loop (slot stolen between FreeLoops and
// queueStartWorkspace). Parent stop must retire it so drainQueued cannot
// start agent work after the parent board is gone.
func TestStopInFlightChildrenStopsQueuedChild(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	installShippedLoopPack(t)
	projID := addProject(t, st, repo)

	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	t.Cleanup(func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	})

	parentID := "WS-stop-queued-parent"
	plan := domain.Plan{
		Summary: "Docs", Approved: true, Source: "human",
		Steps: []domain.PlanStep{
			{ID: "S1", Title: "Write install.md", Status: domain.StepPending, Files: []string{"docs/install.md"}},
		},
	}
	parentWS := domain.Workspace{
		ID: parentID, ProjectID: projID, Goal: "Write the docs",
		State: domain.StateCoding, Plan: &plan, Branch: "main",
		LoopID: bundles.DocsLoopID, PolicyID: "recur.policies.aggressive",
		Autonomy: domain.AutonomyAggressive,
	}
	if _, err := st.AddWorkspace(parentWS); err != nil {
		t.Fatal(err)
	}
	parent := &Loop{
		ID: parentID, ProjectID: projID, Branch: "main", eng: eng,
		Autonomy: domain.AutonomyAggressive, PolicyID: "recur.policies.aggressive",
		currentPhase: bundles.LoopPhase{ID: "converge", Uses: bundles.VerbConverge},
	}

	id, err := eng.startStepChild(parent, parentWS, plan.Steps, 1, nil)
	if err != nil {
		t.Fatalf("startStepChild: %v", err)
	}
	child, ok := st.Workspace(id)
	if !ok || child.State != domain.StateQueued {
		t.Fatalf("startStepChild() child.State = %q ok=%v, want queued", child.State, ok)
	}
	if eng.runningLoopByID(id) != nil {
		t.Fatal("queued child should have no running loop")
	}

	parent.stopInFlightChildren()

	child, _ = st.Workspace(id)
	if child.State != domain.StateStopped {
		t.Errorf("stopInFlightChildren() child.State = %q, want stopped", child.State)
	}

	if slots > 0 {
		eng.gov.ReleaseLoop()
		slots--
	}
	eng.drainQueued()
	if eng.runningLoopByID(id) != nil {
		t.Error("drainQueued started a child stopped with its parent")
	}
	child, _ = st.Workspace(id)
	if child.State != domain.StateStopped {
		t.Errorf("drainQueued() child.State = %q, want stopped", child.State)
	}
}

// App shutdown cancels the parent's context too; that must NOT stop its
// children — they persist interrupted and the parent's resume picks them up.
func TestStopChildrenSkippedDuringShutdown(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	installShippedLoopPack(t)
	projID := addProject(t, st, repo)
	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	t.Cleanup(func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	})
	parentID := "WS-shutdown-parent"
	plan := domain.Plan{Summary: "Docs", Approved: true, Source: "human",
		Steps: []domain.PlanStep{{ID: "S1", Title: "Write install.md", Status: domain.StepPending, Files: []string{"docs/install.md"}}}}
	parentWS := domain.Workspace{ID: parentID, ProjectID: projID, Goal: "Write the docs", State: domain.StateCoding, Plan: &plan, Branch: "main",
		LoopID: bundles.DocsLoopID, PolicyID: "recur.policies.aggressive", Autonomy: domain.AutonomyAggressive}
	if _, err := st.AddWorkspace(parentWS); err != nil {
		t.Fatal(err)
	}
	parent := &Loop{ID: parentID, ProjectID: projID, Branch: "main", eng: eng, Autonomy: domain.AutonomyAggressive,
		PolicyID: "recur.policies.aggressive", currentPhase: bundles.LoopPhase{ID: "converge", Uses: bundles.VerbConverge}}
	id, err := eng.startStepChild(parent, parentWS, plan.Steps, 1, nil)
	if err != nil {
		t.Fatalf("startStepChild: %v", err)
	}
	eng.teardownMu.Lock()
	eng.shuttingDown = true
	eng.teardownMu.Unlock()
	parent.stopInFlightChildren()
	if child, _ := st.Workspace(id); child.State != domain.StateQueued {
		t.Errorf("child.State = %q after a shutdown-time stop, want queued (untouched)", child.State)
	}
}

// A parent resumed after a restart re-dispatches the children Recover left
// paused/interrupted; a child the human paused is left alone.
func TestBoardResumeRedispatchesInterruptedChildren(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	installShippedLoopPack(t)
	projID := addProject(t, st, repo)
	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	t.Cleanup(func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	})
	parentID := "WS-resume-parent"
	plan := domain.Plan{Summary: "Docs", Approved: true, Source: "human",
		Steps: []domain.PlanStep{
			{ID: "S1", Title: "Write install.md", Status: domain.StepPending, Files: []string{"docs/install.md"}},
			{ID: "S2", Title: "Write usage.md", Status: domain.StepPending, Files: []string{"docs/usage.md"}},
		}}
	parentWS := domain.Workspace{ID: parentID, ProjectID: projID, Goal: "Write the docs", State: domain.StateCoding, Plan: &plan, Branch: "main",
		LoopID: bundles.DocsLoopID, PolicyID: "recur.policies.aggressive", Autonomy: domain.AutonomyAggressive}
	if _, err := st.AddWorkspace(parentWS); err != nil {
		t.Fatal(err)
	}
	parent := &Loop{ID: parentID, ProjectID: projID, Branch: "main", eng: eng, Autonomy: domain.AutonomyAggressive,
		PolicyID: "recur.policies.aggressive", currentPhase: bundles.LoopPhase{ID: "converge", Uses: bundles.VerbConverge}}
	interruptedID, err := eng.startStepChild(parent, parentWS, plan.Steps[:1], 1, nil)
	if err != nil {
		t.Fatalf("startStepChild: %v", err)
	}
	pausedID, err := eng.startStepChild(parent, parentWS, plan.Steps[1:], 1, nil)
	if err != nil {
		t.Fatalf("startStepChild: %v", err)
	}
	// What Recover leaves after a restart mid-run, and what a human Pause leaves.
	st.UpdateWorkspace(interruptedID, func(w *domain.Workspace) { w.State = domain.StatePaused; w.Phase = phaseInterrupted })
	st.UpdateWorkspace(pausedID, func(w *domain.Workspace) { w.State = domain.StatePaused; w.Phase = "paused" })
	if slots > 0 {
		eng.gov.ReleaseLoop()
		slots--
	}
	parent.resumeInterruptedChildren([]string{interruptedID, pausedID})
	if c, _ := st.Workspace(interruptedID); c.State == domain.StatePaused && c.Phase == phaseInterrupted {
		t.Errorf("interrupted child still %s/%s, want re-dispatched", c.State, c.Phase)
	}
	if c, _ := st.Workspace(pausedID); c.State != domain.StatePaused || c.Phase != "paused" {
		t.Errorf("human-paused child = %s/%s, want left alone", c.State, c.Phase)
	}
	eng.Stop(interruptedID)
}

// A conflict-resolve turn that errors must stop a sibling still running; the
// parent context stays live and the parent is not Stop'd, so the old defer
// skipped cleanup and the sibling's result was never integrated.
func TestRunBoardStopsSiblingWhenConflictResolverFails(t *testing.T) {
	repo := initRepo(t)
	eng, st, _, _ := newTestEngine(t, failAgentRunner{})
	ctx := context.Background()
	m := gitpkg.NewManager(repo, eng.worktreeRoot)
	parentWT, _, err := m.Add(ctx, "WS-fail-parent", "fail-parent", "main")
	if err != nil {
		t.Fatal(err)
	}
	s1WT, _, err := m.Add(ctx, "WS-fail-s1", "fail-s1", parentWT.Branch)
	if err != nil {
		t.Fatal(err)
	}
	s2WT, _, err := m.Add(ctx, "WS-fail-s2", "fail-s2", parentWT.Branch)
	if err != nil {
		t.Fatal(err)
	}
	commitWorktreeFile(t, m, s1WT.Path, "s1.txt", "from s1\n", "s1 work")
	commitWorktreeFile(t, m, s2WT.Path, "README.md", "s2 says hi\n", "s2 work")
	commitWorktreeFile(t, m, parentWT.Path, "README.md", "parent says hi\n", "parent edit")
	if _, err := m.MergeInto(ctx, parentWT.Path, s2WT.Branch, "Merge S2", "test", "test@example.com"); err != nil {
		t.Fatal(err)
	}

	parentID, s1ID, s2ID := "WS-fail-parent", "WS-fail-s1", "WS-fail-s2"
	plan := domain.Plan{Steps: []domain.PlanStep{
		{ID: "S1", Title: "slow sibling", Status: domain.StepDoing, Files: []string{"s1.txt"}, Workspace: s1ID},
		{ID: "S2", Title: "conflicted sibling", Status: domain.StepDoing, Files: []string{"README.md"}, Workspace: s2ID},
	}}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: parentID, Goal: "parent", State: domain.StateCoding, Plan: &plan,
		WorktreePath: parentWT.Path, Branch: parentWT.Branch,
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: s1ID, ParentID: parentID, Goal: "slow sibling", State: domain.StateCoding,
		Branch: s1WT.Branch, WorktreePath: s1WT.Path, ParentStepIDs: []string{"S1"},
		FanOut: &domain.ChildFanOut{Wave: 1},
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: s2ID, ParentID: parentID, Goal: "conflicted sibling", State: domain.StatePROpen,
		Branch: s2WT.Branch, WorktreePath: s2WT.Path, ParentStepIDs: []string{"S2"},
		FanOut: &domain.ChildFanOut{Wave: 1, Outcome: "conflict: README.md"},
	}); err != nil {
		t.Fatal(err)
	}

	childLoop := &Loop{ID: s1ID, eng: eng, cmds: make(chan Command, 8)}
	childLoop.cond = sync.NewCond(&childLoop.mu)
	canceled := false
	eng.loops.Store(s1ID, &runningLoop{loop: childLoop, cancel: func() { canceled = true }})
	t.Cleanup(func() { eng.loops.Delete(s1ID) })

	lp := &Loop{ID: parentID, eng: eng, git: m, Worktree: parentWT.Path, planStructured: true}
	_, stop, err := lp.runBoard(ctx, 1)
	if err == nil {
		t.Fatal("runBoard() err = nil, want conflict-resolve failure")
	}
	if stop != domain.StateFailed {
		t.Errorf("runBoard() stop = %q, want failed", stop)
	}
	if !childLoop.isStopped() {
		t.Error("slow sibling loop was not stopped after the resolve turn failed")
	}
	if !canceled {
		t.Error("slow sibling dispatch was not cancelled after the resolve turn failed")
	}
}

type failAgentRunner struct{}

func (failAgentRunner) Run(context.Context, AgentSpec, func(ParsedEvent)) (Result, error) {
	return Result{Subtype: "error_during_execution", IsError: true, Text: "provider timeout"}, nil
}
