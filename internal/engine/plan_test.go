package engine

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
	"recur/internal/mcp"
	"recur/internal/store"
)

// TestSubmitPlanPersistsStructuredArtifact: a Plan-mode chat's submit_plan call
// becomes the workspace's plan artifact — engine-assigned step IDs, a prose
// mirror in DefinitionOfDone, and Approve flips the human sign-off.
func TestSubmitPlanPersistsStructuredArtifact(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("plan-sess", "OK.") },
	}
	eng, st, sink, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "planner", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	rep := engineReporter{eng: eng}
	status := rep.SubmitPlan(ws.ID, "PA", mcp.PlanSubmission{
		Summary: "Add a dark-mode toggle",
		Body:    "Touch Settings.svelte. Persist the preference in localStorage so it survives reload.",
		Steps:   []string{"Add the toggle to Settings", "Persist the preference"},
	})
	if !strings.Contains(status, "2 steps") {
		t.Fatalf("submit status = %q", status)
	}

	got, _ := st.Workspace(ws.ID)
	if got.Plan == nil || len(got.Plan.Steps) != 2 {
		t.Fatalf("plan not persisted: %+v", got.Plan)
	}
	// The live event must CARRY the plan — the open Plan panel renders from the
	// frontend store, which is patched from workspace.updated (a payload without
	// the plan leaves the panel stale until a full refetch).
	sawPlan := false
	for _, ev := range sink.byName("workspace.updated") {
		d, _ := ev.Data.(map[string]any)
		if d["workspaceId"] != ws.ID {
			continue
		}
		if p, ok := d["plan"].(*domain.Plan); ok && p != nil && len(p.Steps) == 2 {
			sawPlan = true
		}
	}
	if !sawPlan {
		t.Fatal("workspace.updated should carry the structured plan")
	}
	if got.Plan.Steps[0].ID != "S1" || got.Plan.Steps[0].Status != domain.StepPending {
		t.Errorf("step identity is the engine's: %+v", got.Plan.Steps[0])
	}
	if got.Plan.Source != "plan-chat" {
		t.Errorf("source = %q, want plan-chat", got.Plan.Source)
	}
	if got.Plan.Approved {
		t.Error("a fresh plan must not be pre-approved")
	}
	if got.Plan.Body != "Touch Settings.svelte. Persist the preference in localStorage so it survives reload." {
		t.Errorf("body = %q", got.Plan.Body)
	}
	// Prose mirror keeps every existing DefinitionOfDone consumer coherent —
	// and prefers the detailed body over the headline.
	if !strings.Contains(got.DefinitionOfDone, "Touch Settings.svelte") ||
		!strings.Contains(got.DefinitionOfDone, "1. Add the toggle to Settings") {
		t.Errorf("DefinitionOfDone mirror: %q", got.DefinitionOfDone)
	}
	if strings.Contains(got.DefinitionOfDone, "Add a dark-mode toggle") &&
		!strings.Contains(got.DefinitionOfDone, "Touch Settings.svelte") {
		t.Errorf("mirror should prefer body, got %q", got.DefinitionOfDone)
	}

	// Step updates: engine validates the id and applies the status.
	if s := rep.UpdatePlanStep(ws.ID, "PA", "S2", "done", "landed with tests"); !strings.Contains(s, "done") {
		t.Fatalf("update status = %q", s)
	}
	if s := rep.UpdatePlanStep(ws.ID, "PA", "S9", "done", ""); !strings.Contains(s, "Refused") {
		t.Fatalf("unknown step id must be refused, got %q", s)
	}
	got, _ = st.Workspace(ws.ID)
	if got.Plan.Steps[1].Status != domain.StepDone || got.Plan.Steps[1].Note == "" {
		t.Errorf("step 2 after update: %+v", got.Plan.Steps[1])
	}
	if !strings.Contains(got.DefinitionOfDone, "(done)") {
		t.Errorf("mirror should show completion: %q", got.DefinitionOfDone)
	}

	// The Maestro filing a plan marks the loop-planner source.
	_ = rep.SubmitPlan(ws.ID, "MA", mcp.PlanSubmission{Summary: "re-plan", Steps: []string{"one"}})
	got, _ = st.Workspace(ws.ID)
	if got.Plan.Source != "loop-planner" {
		t.Errorf("MA-filed plan source = %q, want loop-planner", got.Plan.Source)
	}

	if s := eng.ApprovePlan(ws.ID); s != "Plan approved." {
		t.Fatalf("ApprovePlan = %q", s)
	}
	got, _ = st.Workspace(ws.ID)
	if !got.Plan.Approved {
		t.Fatal("plan not marked approved")
	}
	if s := eng.ApprovePlan(ws.ID); !strings.Contains(s, "already approved") {
		t.Fatalf("second approve = %q", s)
	}
}

// TestAdoptProsePlanFromCreatePlan: Cursor createPlan prose becomes a
// summary-only Plan artifact (no steps) without clobbering a structured
// submit_plan that already has checklist steps.
func TestAdoptProsePlanFromCreatePlan(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("prose", "OK.") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "createplan", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	body := "Summary: touch work.txt.\n\nSteps:\n1. Write the marker\n\nDefinition of done: file exists."
	if !eng.adoptProsePlan(ws.ID, "PA", body) {
		t.Fatal("adoptProsePlan returned false")
	}
	got, _ := st.Workspace(ws.ID)
	if got.Plan == nil {
		t.Fatal("plan artifact missing")
	}
	if got.Plan.Body != body {
		t.Errorf("body = %q, want full createPlan body", got.Plan.Body)
	}
	if got.Plan.Summary != "Summary: touch work.txt." {
		t.Errorf("summary = %q, want first paragraph", got.Plan.Summary)
	}
	if len(got.Plan.Steps) != 0 {
		t.Errorf("prose plan should have no steps, got %d", len(got.Plan.Steps))
	}
	if got.Plan.Source != "plan-chat" {
		t.Errorf("source = %q, want plan-chat", got.Plan.Source)
	}
	if got.DefinitionOfDone != body {
		t.Errorf("DefinitionOfDone mirror = %q", got.DefinitionOfDone)
	}
	saw := false
	for _, ev := range sink.byName("workspace.updated") {
		d, _ := ev.Data.(map[string]any)
		if d["workspaceId"] != ws.ID {
			continue
		}
		if p, ok := d["plan"].(*domain.Plan); ok && p != nil && p.Body == body {
			saw = true
		}
	}
	if !saw {
		t.Fatal("workspace.updated should carry the prose plan")
	}

	// Structured submit_plan wins; createPlan must not clobber steps.
	rep := engineReporter{eng: eng}
	_ = rep.SubmitPlan(ws.ID, "PA", mcp.PlanSubmission{
		Summary: "Real structured plan",
		Steps:   []string{"Do the thing"},
	})
	if eng.adoptProsePlan(ws.ID, "PA", "should not replace") {
		t.Fatal("adoptProsePlan must refuse to overwrite a stepped plan")
	}
	got, _ = st.Workspace(ws.ID)
	if got.Plan == nil || len(got.Plan.Steps) != 1 || got.Plan.Summary != "Real structured plan" {
		t.Fatalf("structured plan clobbered: %+v", got.Plan)
	}
}

// TestPlanApprovalGate: a fresh run with an UNAPPROVED structured plan pauses
// after planning for the human's sign-off (Cautious/Balanced; decision #4),
// resumes on the answer, and the Coder is briefed with the plan + the
// update_plan_step hand. Aggressive runs skip the gate. Pre-existing plans
// (Plan-mode chat / Run-as-loop handoff) are adopted — no re-planning turn.
func TestPlanApprovalGate(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("gate-%d", call), "OK.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))

	ws, err := eng.CreateWorkspace(projID, "gated", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	rep := engineReporter{eng: eng}
	_ = rep.SubmitPlan(ws.ID, "PA", mcp.PlanSubmission{
		Summary: "Wire the toggle",
		Steps:   []string{"Edit settings", "Add persistence"},
	})

	if queued, err := eng.StartRunOnWorkspace(ws.ID, "ship the toggle"); err != nil || queued {
		t.Fatalf("StartRunOnWorkspace: queued=%v err=%v", queued, err)
	}
	waitFor(t, 5*time.Second, "plan-approval escalation", func() bool {
		got, _ := st.Workspace(ws.ID)
		return got.State == domain.StateEscalated
	})
	esc, ok := st.Escalation(ws.ID)
	if !ok || !strings.Contains(esc.Question, "plan") {
		t.Fatalf("escalation: ok=%v %+v", ok, esc)
	}
	// The planner was NOT re-run: the existing plan was adopted (source intact).
	got, _ := st.Workspace(ws.ID)
	if got.Plan == nil || got.Plan.Source != "plan-chat" {
		t.Fatalf("existing plan should be adopted, got %+v", got.Plan)
	}

	if _, ok := eng.AnswerEscalation(ws.ID, "Approve and proceed", ""); !ok {
		t.Fatal("AnswerEscalation refused")
	}
	waitFor(t, 10*time.Second, "run to settle at pr_open", func() bool {
		got, _ := st.Workspace(ws.ID)
		return got.State == domain.StatePROpen
	})
	got, _ = st.Workspace(ws.ID)
	if got.Plan == nil || !got.Plan.Approved {
		t.Fatal("answering the gate must mark the plan approved")
	}

	// The Coder was briefed with the plan and the update_plan_step hand.
	coder, ok := runner.specWithRole(domain.RoleCoder)
	if !ok {
		t.Fatal("no coder turn ran")
	}
	if !strings.Contains(coder.SystemPrompt, "Wire the toggle") {
		t.Errorf("coder brief should carry the plan, got %q", truncate(coder.SystemPrompt, 200))
	}
	if !strings.Contains(coder.SystemPrompt, "update_plan_step") {
		t.Errorf("structured plan should add the step hand to the coder prompt")
	}
	if !strings.Contains(coder.AppendSystemPrompt, "SIMPLIFY MODE") {
		t.Errorf("loop coder should inject simplify rails, got %q", coder.AppendSystemPrompt)
	}
	if !strings.Contains(coder.AppendSystemPrompt, commentHygieneNeedle) {
		t.Errorf("loop coder should inject comment hygiene, got %q", coder.AppendSystemPrompt)
	}
	hasStepTool := false
	for _, tl := range coder.AllowedTools {
		if tl == "mcp__recur__update_plan_step" {
			hasStepTool = true
		}
	}
	if !hasStepTool {
		t.Errorf("coder allowlist should include update_plan_step, got %v", coder.AllowedTools)
	}

	// Aggressive: same setup, no gate — straight through to pr_open.
	pol := st.Policy()
	pol.Autonomy = domain.AutonomyAggressive
	st.SetPolicy(pol)
	ws2, err := eng.CreateWorkspace(projID, "ungated", "")
	if err != nil {
		t.Fatalf("CreateWorkspace 2: %v", err)
	}
	_ = rep.SubmitPlan(ws2.ID, "PA", mcp.PlanSubmission{Summary: "Small fix", Steps: []string{"one"}})
	if queued, err := eng.StartRunOnWorkspace(ws2.ID, "small fix"); err != nil || queued {
		t.Fatalf("StartRunOnWorkspace 2: queued=%v err=%v", queued, err)
	}
	waitFor(t, 10*time.Second, "aggressive run settles without a gate", func() bool {
		got, _ := st.Workspace(ws2.ID)
		return got.State == domain.StatePROpen
	})
	if _, ok := st.Escalation(ws2.ID); ok {
		t.Fatal("aggressive run must not raise the plan-approval gate")
	}
}

// TestSubmitPlanResolvesAttachments: artifact ids and persisted chat images
// become Plan.Attachments; unknown ids are named in the tool result.
func TestSubmitPlanResolvesAttachments(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("att", "OK.") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "atts", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	art, err := eng.CreateArtifact(ws.ID, "PA", mcp.CreateArtifactRequest{
		Title: "Flow", Kind: "md", Content: "# flow\n",
	})
	if err != nil {
		t.Fatal(err)
	}
	root, err := store.DataDir()
	if err != nil {
		t.Fatal(err)
	}
	dir, err := store.AttachmentsDir(root, ws.ID)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "shot.png"), []byte("png"), 0o644); err != nil {
		t.Fatal(err)
	}

	rep := engineReporter{eng: eng}
	status := rep.SubmitPlan(ws.ID, "PA", mcp.PlanSubmission{
		Summary:       "Use the diagram",
		Body:          "See the attached flow.",
		Steps:         []string{"Follow the diagram"},
		AttachmentIDs: []string{art.ID, "shot", "missing-id"},
	})
	if !strings.Contains(status, "2 attachment") {
		t.Fatalf("status = %q", status)
	}
	if !strings.Contains(status, "missing-id") {
		t.Fatalf("unknown id not named: %q", status)
	}
	got, _ := st.Workspace(ws.ID)
	if got.Plan == nil || len(got.Plan.Attachments) != 2 {
		t.Fatalf("attachments: %+v", got.Plan)
	}
	if !strings.Contains(got.DefinitionOfDone, "See the attached flow.") {
		t.Errorf("body missing from mirror: %q", got.DefinitionOfDone)
	}
	if !strings.Contains(got.DefinitionOfDone, art.Path) {
		t.Errorf("artifact path missing from mirror: %q", got.DefinitionOfDone)
	}
}

func TestSubmitPlanPreservesStepStatusOnResubmit(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("keep", "OK.") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "keep-steps", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	rep := engineReporter{eng: eng}
	_ = rep.SubmitPlan(ws.ID, "PA", mcp.PlanSubmission{
		Summary: "Ship the toggle",
		Body:    "Original brief.",
		Steps:   []string{"Add the toggle", "Persist the preference"},
	})
	if s := rep.UpdatePlanStep(ws.ID, "PA", "S1", "done", "landed with tests"); !strings.Contains(s, "done") {
		t.Fatalf("UpdatePlanStep = %q", s)
	}
	st.UpdateWorkspace(ws.ID, func(l *domain.Workspace) {
		l.State = domain.StateCoding
		l.Progress = 0.99
	})

	status := eng.SubmitHumanPlan(ws.ID, "Ship the toggle", "Reworded brief.", []string{
		"Add the toggle", "Persist the preference",
	}, nil)
	if strings.HasPrefix(status, "Refused") {
		t.Fatalf("SubmitHumanPlan = %q", status)
	}
	got, _ := st.Workspace(ws.ID)
	if got.Plan == nil || len(got.Plan.Steps) != 2 {
		t.Fatalf("plan steps: %+v", got.Plan)
	}
	s1 := got.Plan.Steps[0]
	if s1.ID != "S1" || s1.Status != domain.StepDone || s1.Note == "" {
		t.Errorf("mergePlanSteps preserved S1 = %+v, want id=S1 status=done with note", s1)
	}
	if got.Plan.Steps[1].ID != "S2" || got.Plan.Steps[1].Status != domain.StepPending {
		t.Errorf("S2 = %+v, want pending", got.Plan.Steps[1])
	}
	if got.Plan.Body != "Reworded brief." {
		t.Errorf("Body = %q, want reworded brief", got.Plan.Body)
	}
	wantProg := 0.15 + 0.7*0.5
	if got.Progress != wantProg {
		t.Errorf("Progress = %v, want %v (recomputed from preserved steps)", got.Progress, wantProg)
	}

	// A full rewrite (no titles in common) still resets to pending S1…Sn.
	_ = eng.SubmitHumanPlan(ws.ID, "Start over", "New brief.", []string{"Do something else"}, nil)
	got, _ = st.Workspace(ws.ID)
	if got.Plan == nil || len(got.Plan.Steps) != 1 {
		t.Fatalf("rewritten steps: %+v", got.Plan)
	}
	if got.Plan.Steps[0].ID != "S1" || got.Plan.Steps[0].Status != domain.StepPending {
		t.Errorf("full rewrite step = %+v, want S1 pending", got.Plan.Steps[0])
	}

	// Insert-at-front: status follows the title; ids stay S1…Sn in list order
	// so the Coder's "1. / S1" mapping still holds.
	_ = rep.SubmitPlan(ws.ID, "PA", mcp.PlanSubmission{
		Summary: "Ship the toggle",
		Body:    "Brief.",
		Steps:   []string{"Add the toggle", "Persist the preference"},
	})
	if s := rep.UpdatePlanStep(ws.ID, "PA", "S1", "done", "toggle landed"); !strings.Contains(s, "done") {
		t.Fatalf("UpdatePlanStep = %q", s)
	}
	_ = eng.SubmitHumanPlan(ws.ID, "Ship the toggle", "Brief.", []string{
		"Add migration", "Add the toggle", "Persist the preference",
	}, nil)
	got, _ = st.Workspace(ws.ID)
	if got.Plan == nil || len(got.Plan.Steps) != 3 {
		t.Fatalf("inserted steps: %+v", got.Plan)
	}
	if got.Plan.Steps[0].ID != "S1" || got.Plan.Steps[0].Title != "Add migration" || got.Plan.Steps[0].Status != domain.StepPending {
		t.Errorf("new first step = %+v, want S1 pending Add migration", got.Plan.Steps[0])
	}
	if got.Plan.Steps[1].ID != "S2" || got.Plan.Steps[1].Title != "Add the toggle" || got.Plan.Steps[1].Status != domain.StepDone {
		t.Errorf("shifted toggle = %+v, want S2 done (status followed the title)", got.Plan.Steps[1])
	}
	if !strings.Contains(got.DefinitionOfDone, "1. Add migration") || !strings.Contains(got.DefinitionOfDone, "2. Add the toggle (done)") {
		t.Errorf("DefinitionOfDone ordinals should match ids, got %q", got.DefinitionOfDone)
	}
}

func TestMergePlanStepsRetitleKeepsStatus(t *testing.T) {
	prev := []domain.PlanStep{
		{ID: "S1", Title: "Add the toggle", Status: domain.StepDone, Note: "toggle landed"},
		{ID: "S2", Title: "Persist the preferene", Status: domain.StepDone, Note: "landed with tests"},
		{ID: "S3", Title: "Ship it", Status: domain.StepPending},
	}
	got := mergePlanSteps(prev, []string{
		"Add the toggle", "Persist the preference", "Ship it",
	}, []string{"S1", "S2", "S3"}, nil)
	if len(got) != 3 {
		t.Fatalf("mergePlanSteps len = %d, want 3", len(got))
	}
	s2 := got[1]
	if s2.ID != "S2" || s2.Title != "Persist the preference" || s2.Status != domain.StepDone || s2.Note != "landed with tests" {
		t.Errorf("mergePlanSteps retitle S2 = %+v, want id=S2 title=Persist the preference status=done note=landed with tests", s2)
	}
	if got[0].Status != domain.StepDone || got[2].Status != domain.StepPending {
		t.Errorf("mergePlanSteps neighbors = %+v %+v, want S1 done S3 pending", got[0], got[2])
	}

	rewritten := mergePlanSteps(prev, []string{"Do something else"}, nil, nil)
	if len(rewritten) != 1 || rewritten[0].ID != "S1" || rewritten[0].Status != domain.StepPending {
		t.Errorf("mergePlanSteps full rewrite = %+v, want S1 pending", rewritten)
	}
}

func TestMergePlanStepsRemapsDependenciesOnRenumber(t *testing.T) {
	prev := []domain.PlanStep{
		{ID: "S1", Title: "API", Status: domain.StepDone, Files: []string{"api.go"}},
		{ID: "S2", Title: "UI", Status: domain.StepPending, Files: []string{"ui.go"}, DependsOn: []string{"S1"}},
		{ID: "S3", Title: "Docs", Status: domain.StepPending, DependsOn: []string{"S2"}},
	}
	// Reorder: UI filed first must still depend on API (now S2), not on itself.
	got := mergePlanSteps(prev, []string{"UI", "API", "Docs"}, nil, nil)
	if got[0].ID != "S1" || got[0].Title != "UI" || len(got[0].DependsOn) != 1 || got[0].DependsOn[0] != "S2" {
		t.Errorf("reorder: UI = %+v, want S1 depending on S2 (API)", got[0])
	}
	if got[2].DependsOn[0] != "S1" {
		t.Errorf("reorder: Docs = %+v, want depends on S1 (UI)", got[2])
	}
	// Drop: an edge to a removed step goes with it; insertion shifts the rest.
	got = mergePlanSteps(prev, []string{"API", "Tests", "Docs"}, nil, nil)
	if got[2].Title != "Docs" || len(got[2].DependsOn) != 0 {
		t.Errorf("drop: Docs = %+v, want no dependency (UI was dropped)", got[2])
	}
	if got[1].Title != "Tests" || got[1].Status != domain.StepPending || got[1].DependsOn != nil {
		t.Errorf("insert: Tests = %+v, want fresh pending step", got[1])
	}
	// Object form is authoritative, including empty lists.
	got = mergePlanSteps(prev, []string{"API", "UI", "Docs"}, nil, []mcp.PlanStepMeta{
		{Explicit: true}, {Explicit: true, DependsOn: []string{"S1"}}, {},
	})
	if len(got[0].Files) != 0 {
		t.Errorf("explicit empty files should clear, got %+v", got[0])
	}
	if len(got[1].Files) != 0 || len(got[1].DependsOn) != 1 || got[1].DependsOn[0] != "S1" {
		t.Errorf("explicit meta should replace both lists, got %+v", got[1])
	}
	if len(got[2].DependsOn) != 1 || got[2].DependsOn[0] != "S2" {
		t.Errorf("bare title keeps (remapped) deps, got %+v", got[2])
	}
}

func TestSubmitPlanKeepsStepsInMirrorWhenBodyIsCapped(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("cap", "OK.") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "cap-body", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	body := strings.Repeat("x", planMaxPersistChars+800)
	rep := engineReporter{eng: eng}
	status := rep.SubmitPlan(ws.ID, "PA", mcp.PlanSubmission{
		Summary: "Long brief",
		Body:    body,
		Steps:   []string{"Keep the checklist"},
	})
	if strings.HasPrefix(status, "Refused") {
		t.Fatalf("SubmitPlan = %q", status)
	}
	got, _ := st.Workspace(ws.ID)
	if got.Plan == nil {
		t.Fatal("plan missing")
	}
	if len(got.Plan.Body) >= planMaxPersistChars {
		t.Errorf("Plan.Body len = %d, want below planMaxPersistChars so the suffix fits", len(got.Plan.Body))
	}
	if !strings.Contains(got.DefinitionOfDone, "Steps:") || !strings.Contains(got.DefinitionOfDone, "1. Keep the checklist") {
		t.Errorf("DefinitionOfDone lost the checklist at the cap: len=%d ending %q",
			len(got.DefinitionOfDone), tail(got.DefinitionOfDone, 80))
	}
}

func tail(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[len(s)-n:]
}

func TestFirstParagraphStripsMarkdownHeadings(t *testing.T) {
	got := firstParagraph("# Add dark mode\n\nTouch Settings.svelte.")
	if got != "Add dark mode" {
		t.Errorf("firstParagraph(# heading) = %q, want %q", got, "Add dark mode")
	}
	got = firstParagraph("##\n\nDo the work.")
	if got != "Do the work." {
		t.Errorf("firstParagraph(empty heading) = %q, want next paragraph %q", got, "Do the work.")
	}
	got = firstParagraph("- ship the toggle\n\nDetails.")
	if got != "ship the toggle" {
		t.Errorf("firstParagraph(bullet) = %q, want %q", got, "ship the toggle")
	}
}
