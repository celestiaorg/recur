package engine

import (
	"context"
	"strings"
	"testing"
	"time"

	"recur/internal/bundles"
	"recur/internal/domain"
)

func TestBundleSystemPromptOmitsWalkRecipe(t *testing.T) {
	mission := "The human already wrote this change. Review the current worktree, then fix and re-review until the policy bar passes."
	l := &Loop{
		guard: resolvedGuard{
			Loop: bundles.LoopDef{Prompts: map[string]string{"mission": mission}},
		},
		catalog: bundles.Catalog{
			Roles: []bundles.Role{{
				ID:     "recur.roles.reviewer",
				Prompt: "You are an independent Reviewer. Read the diff. Do not edit.",
			}},
		},
	}
	got := l.bundleSystemPrompt("recur.roles.reviewer", "FALLBACK", mission, "")
	if strings.Contains(got, "fix and re-review") {
		t.Errorf("reviewer system prompt must not carry the loop walk recipe, got %q", got)
	}
	if !strings.Contains(got, "Do not edit") {
		t.Errorf("reviewer role body missing, got %q", got)
	}
	if strings.Contains(got, "FALLBACK") {
		t.Errorf("catalog role should win, got %q", got)
	}

	got = l.bundleSystemPrompt("recur.roles.reviewer", "FALLBACK", "add dark mode", "")
	if !strings.Contains(got, "Task: add dark mode") {
		t.Errorf("a real product task should remain, got %q", got)
	}
}

func TestLoopSpecialistPromptActionUsesDisplay(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, dataDir := newTestEngine(t, runner)
	body := "#!/bin/sh\nprintf '%s\\n' '{\"prompt\":\"WIRE Review the changes in this worktree and file every real bug\",\"display\":\"Please review the changes in this worktree and file any real issues\"}'\n"
	writePromptBundle(t, dataDir, "review", body, "recur.chattest.review")
	installTestPack(t, "TestLoops", `
[bundle]
name = "Test Loops"
id = "test.loops"
version = "1.0.0"

[[loop]]
id = "test.loops.prompt-display"
name = "Prompt display"
entry = "converge"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "converge"
uses = "converge"
start = "review"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"
[loop.phase.prompts]
review = "/review"
`)
	projID := addProject(t, st, initRepo(t))

	id, err := eng.StartWorkspaceWith(projID, "Review the current diff", "", "", "test.loops.prompt-display", "")
	if err != nil {
		t.Fatalf("StartWorkspaceWith: %v", err)
	}
	waitFor(t, 15*time.Second, "prompt-display loop to idle", func() bool {
		l, ok := st.Workspace(id)
		return ok && (l.State == domain.StatePaused || l.State == domain.StateStopped)
	})

	var review AgentSpec
	for _, s := range runner.capturedSpecs() {
		if s.Role == domain.RoleReviewer {
			review = s
			break
		}
	}
	if review.Role == "" {
		t.Fatal("StartWorkspaceWith(test.loops.prompt-display): no reviewer spec")
	}
	wantDisplay := "Please review the changes in this worktree and file any real issues"
	if review.DisplayText != wantDisplay {
		t.Errorf("runReviewer DisplayText = %q, want %q", review.DisplayText, wantDisplay)
	}
	if !strings.Contains(review.Prompt, "WIRE Review the changes") {
		t.Errorf("runReviewer Prompt = %q, want the action wire", review.Prompt)
	}

	items := contentItems(eng.WorkspaceChatItems(id, "RV"))
	var user string
	for _, it := range items {
		if it.Role == "user" && it.Kind == "text" {
			user = it.Text
			break
		}
	}
	if user != wantDisplay {
		t.Errorf("WorkspaceChatItems(RV) user bubble = %q, want display %q", user, wantDisplay)
	}
}

func TestLoopGoalLabel(t *testing.T) {
	tests := []struct {
		name, goal, display, want string
	}{
		{name: "plain", goal: "add dark mode", want: "add dark mode"},
		{name: "labeled", goal: "full Linear body", display: "Linear ENG-123 · title", want: "Linear ENG-123 · title"},
		{name: "whitespace display", goal: "task", display: "  ", want: "task"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			l := &Loop{Goal: tt.goal, GoalDisplay: tt.display}
			if got := l.goalLabel(); got != tt.want {
				t.Errorf("goalLabel() goal=%q display=%q = %q, want %q", tt.goal, tt.display, got, tt.want)
			}
		})
	}
}

func TestLoopGoalDisplayIsUserBubble(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	installTestPack(t, "TestLoops", `
[bundle]
name = "Test Loops"
id = "test.loops"
version = "1.0.0"

[[loop]]
id = "test.loops.goal-display"
name = "Goal display"
entry = "converge"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "converge"
uses = "converge"
start = "implement"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"
`)
	projID := addProject(t, st, initRepo(t))
	wire := "Linear ENG-123\n\nFull issue body that must reach the model."
	label := "Linear ENG-123 · title"
	rec, acquired, err := eng.queueStartWorkspace(projID, wire, label, "", "", "test.loops.goal-display", "", nil)
	if err != nil {
		t.Fatalf("queueStartWorkspace: %v", err)
	}
	if rec.Goal != wire {
		t.Errorf("queueStartWorkspace Goal = %q, want wire %q", rec.Goal, wire)
	}
	if rec.GoalDisplay != label {
		t.Errorf("queueStartWorkspace GoalDisplay = %q, want %q", rec.GoalDisplay, label)
	}
	if rec.Short != label {
		t.Errorf("queueStartWorkspace Short = %q, want %q", rec.Short, label)
	}
	if !acquired {
		t.Fatal("queueStartWorkspace: expected a loop slot")
	}
	if err := eng.dispatch(rec); err != nil {
		eng.gov.ReleaseLoop()
		t.Fatalf("dispatch: %v", err)
	}
	waitFor(t, 15*time.Second, "goal-display loop to idle", func() bool {
		l, ok := st.Workspace(rec.ID)
		return ok && (l.State == domain.StatePaused || l.State == domain.StateStopped)
	})

	var coder, review AgentSpec
	for _, s := range runner.capturedSpecs() {
		switch s.Role {
		case domain.RoleCoder:
			if coder.Role == "" {
				coder = s
			}
		case domain.RoleReviewer:
			if review.Role == "" {
				review = s
			}
		}
	}
	if coder.Role == "" {
		t.Fatal("queueStartWorkspace(test.loops.goal-display): no coder spec")
	}
	if coder.DisplayText != label {
		t.Errorf("runCoder DisplayText = %q, want %q", coder.DisplayText, label)
	}
	if !strings.Contains(coder.Prompt, wire) {
		t.Errorf("runCoder Prompt = %q, want the goal wire", coder.Prompt)
	}
	if review.Role == "" {
		t.Fatal("queueStartWorkspace(test.loops.goal-display): no reviewer spec")
	}
	if strings.Contains(review.DisplayText, "Full issue body") {
		t.Errorf("runReviewer DisplayText = %q, want the short label not the wire", review.DisplayText)
	}
	if !strings.Contains(review.DisplayText, label) {
		t.Errorf("runReviewer DisplayText = %q, want %q", review.DisplayText, label)
	}
	if !strings.Contains(review.Prompt, wire) {
		t.Errorf("runReviewer Prompt = %q, want the goal wire", review.Prompt)
	}

	if a, ok := st.Agent(rec.ID, "CO"); !ok {
		t.Fatal("Coder agent missing")
	} else if a.Task != label {
		t.Errorf("Coder Task = %q, want %q", a.Task, label)
	}

	items := contentItems(eng.WorkspaceChatItems(rec.ID, "CO"))
	var user string
	for _, it := range items {
		if it.Role == "user" && it.Kind == "text" {
			user = it.Text
			break
		}
	}
	if user != label {
		t.Errorf("WorkspaceChatItems(CO) user bubble = %q, want %q", user, label)
	}
}

func TestLoopImplementProseKeepsPromptDisplay(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	implement := "Ship the change by editing the worktree."
	installTestPack(t, "TestLoops", `
[bundle]
name = "Test Loops"
id = "test.loops"
version = "1.0.0"

[[loop]]
id = "test.loops.implement-prose"
name = "Implement prose"
entry = "converge"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "converge"
uses = "converge"
start = "implement"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"
[loop.phase.prompts]
implement = "Ship the change by editing the worktree."
`)
	projID := addProject(t, st, initRepo(t))
	goal := "add dark mode"
	id, err := eng.StartWorkspaceWith(projID, goal, "", "", "test.loops.implement-prose", "")
	if err != nil {
		t.Fatalf("StartWorkspaceWith: %v", err)
	}
	waitFor(t, 15*time.Second, "implement-prose loop to idle", func() bool {
		l, ok := st.Workspace(id)
		return ok && (l.State == domain.StatePaused || l.State == domain.StateStopped)
	})

	var coder AgentSpec
	for _, s := range runner.capturedSpecs() {
		if s.Role == domain.RoleCoder {
			coder = s
			break
		}
	}
	if coder.Role == "" {
		t.Fatal("StartWorkspaceWith(test.loops.implement-prose): no coder spec")
	}
	if !strings.Contains(coder.Prompt, implement) {
		t.Errorf("runCoder Prompt = %q, want the implement instruction", coder.Prompt)
	}
	if coder.DisplayText == goal {
		t.Errorf("runCoder DisplayText = %q, want the implement instruction not the goal", coder.DisplayText)
	}

	items := contentItems(eng.WorkspaceChatItems(id, "CO"))
	var user string
	for _, it := range items {
		if it.Role == "user" && it.Kind == "text" {
			user = it.Text
			break
		}
	}
	if user != implement {
		t.Errorf("WorkspaceChatItems(CO) user bubble = %q, want %q", user, implement)
	}
}

// linearIssueWire matches bundles/Linear/src/linear.sh: a short instruction
// plus a nonced raw region around the issue body.
func linearIssueWire(ident, title, body string) string {
	return strings.Join([]string{
		"Work on Linear issue " + ident + " in this workspace. Implement what the issue asks; do not expand scope.",
		"",
		"⟦raw:lin⟧",
		"Identifier: " + ident,
		"Title: " + title,
		"",
		"## Description",
		body,
		"⟦/raw:lin⟧",
		"",
	}, "\n")
}

func TestLoopGoalDisplaySurvivesLinearRawExpand(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	installTestPack(t, "TestLoops", `
[bundle]
name = "Test Loops"
id = "test.loops"
version = "1.0.0"

[[loop]]
id = "test.loops.raw-label"
name = "Raw label"
entry = "planning"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "planning"
uses = "plan"
role = "recur.roles.planner"
next = "converge"

[[loop.phase]]
id = "converge"
uses = "converge"
start = "implement"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"
`)
	wire := linearIssueWire("ENG-1", "Fix the thing", "Full issue body that must reach the model.")
	label := "Linear ENG-1 · Fix the thing"
	composer, err := eng.ExpandComposer(context.Background(), wire, expandOpts{})
	if err != nil {
		t.Fatalf("ExpandComposer: %v", err)
	}
	if composer.Display == "" {
		t.Fatal("ExpandComposer Display empty; unwrap must set it so the fallback path is real")
	}
	l := &Loop{
		eng: eng, Goal: wire, GoalDisplay: label,
		currentPhase: bundles.LoopPhase{ID: "planning", Uses: bundles.VerbPlan},
	}

	planPrompt, planDisp, err := l.expandPhasePrompt(context.Background(), "plan", wire)
	if err != nil {
		t.Fatalf("expandPhasePrompt(plan) err = %v", err)
	}
	if planDisp != "" {
		t.Errorf("expandPhasePrompt(plan) display = %q, want empty so GoalDisplay is kept", planDisp)
	}
	if strings.Contains(planPrompt, "⟦raw:lin⟧") || strings.Contains(planPrompt, "⟦/raw:lin⟧") {
		t.Errorf("expandPhasePrompt(plan) prompt still has raw markers: %q", planPrompt)
	}
	if !strings.Contains(planPrompt, "Full issue body that must reach the model.") {
		t.Errorf("expandPhasePrompt(plan) prompt = %q, want the issue body", planPrompt)
	}

	l.currentPhase = bundles.LoopPhase{ID: "verify", Uses: bundles.VerbVerify}
	verifyFallback := "Verify in the browser: " + wire
	qaPrompt, qaDisp, err := l.expandPhasePrompt(context.Background(), "verify", verifyFallback)
	if err != nil {
		t.Fatalf("expandPhasePrompt(verify) err = %v", err)
	}
	if qaDisp != "" {
		t.Errorf("expandPhasePrompt(verify) display = %q, want empty so GoalDisplay is kept", qaDisp)
	}
	if strings.Contains(qaPrompt, "⟦raw:lin⟧") || strings.Contains(qaPrompt, "⟦/raw:lin⟧") {
		t.Errorf("expandPhasePrompt(verify) prompt still has raw markers: %q", qaPrompt)
	}
	if !strings.Contains(qaPrompt, "Full issue body that must reach the model.") {
		t.Errorf("expandPhasePrompt(verify) prompt = %q, want the issue body", qaPrompt)
	}

	blank := &Loop{eng: eng, Goal: wire}
	_, blankDisp, err := blank.expandPhasePrompt(context.Background(), "plan", wire)
	if err != nil {
		t.Fatalf("expandPhasePrompt(plan, no label) err = %v", err)
	}
	if blankDisp != "" {
		t.Errorf("expandPhasePrompt(plan, no label) display = %q, want empty", blankDisp)
	}

	projID := addProject(t, st, initRepo(t))
	rec, acquired, err := eng.queueStartWorkspace(projID, wire, label, "", "", "test.loops.raw-label", "", nil)
	if err != nil {
		t.Fatalf("queueStartWorkspace: %v", err)
	}
	if !acquired {
		t.Fatal("queueStartWorkspace: expected a loop slot")
	}
	if err := eng.dispatch(rec); err != nil {
		eng.gov.ReleaseLoop()
		t.Fatalf("dispatch: %v", err)
	}
	waitFor(t, 15*time.Second, "raw-label loop to idle", func() bool {
		ws, ok := st.Workspace(rec.ID)
		return ok && (ws.State == domain.StatePaused || ws.State == domain.StateStopped || ws.State == domain.StatePROpen)
	})

	var planner AgentSpec
	for _, s := range runner.capturedSpecs() {
		if s.Role == domain.RoleMaestro {
			planner = s
			break
		}
	}
	if planner.Role == "" {
		t.Fatal("queueStartWorkspace(test.loops.raw-label): no planner spec")
	}
	if planner.DisplayText != label {
		t.Errorf("runPlanner DisplayText = %q, want %q", planner.DisplayText, label)
	}
	if strings.Contains(planner.DisplayText, "Full issue body") {
		t.Errorf("runPlanner DisplayText = %q, want the short label not the wire", planner.DisplayText)
	}
	if strings.Contains(planner.Prompt, "⟦raw:lin⟧") {
		t.Errorf("runPlanner Prompt = %q, want raw markers stripped", planner.Prompt)
	}
	if !strings.Contains(planner.Prompt, "Full issue body that must reach the model.") {
		t.Errorf("runPlanner Prompt = %q, want the issue body", planner.Prompt)
	}

	items := contentItems(eng.WorkspaceChatItems(rec.ID, "MA"))
	var user string
	for _, it := range items {
		if it.Role == "user" && it.Kind == "text" {
			user = it.Text
			break
		}
	}
	if user != label {
		t.Errorf("WorkspaceChatItems(MA) user bubble = %q, want %q", user, label)
	}
}
