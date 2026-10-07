package engine

import (
	"context"
	"encoding/json"
	"strings"
	"sync"
	"testing"
	"time"
	"unicode/utf8"

	"recur/internal/domain"
)

// TestMaestroPlanToolsScope pins the planning-turn tool scope (PRD §7.2): to write
// a thorough plan mode-style brief the Maestro must first INVESTIGATE the repo
// (Read/Grep/Glob), but it must never gain Edit/Write/Bash — the planning turn
// does judgment, never edits (§8.2), and a regression widening maestroPlanTools
// to include an edit tool would let a "planning" turn silently mutate the
// worktree before the Coder ever runs. The existing tool-scope assertion in
// maestro_test.go only covers the CHAT role (maestroTools); this covers the
// separate planning-turn tool set (maestroPlanTools).
func TestMaestroPlanToolsScope(t *testing.T) {
	got := map[string]bool{}
	for _, tool := range maestroPlanTools {
		got[tool] = true
		if tool == "Edit" || tool == "Write" || tool == "Bash" {
			t.Errorf("maestroPlanTools must not include %q (planning investigates, never edits)", tool)
		}
	}
	for _, want := range []string{"Read", "Grep", "Glob", mcpListSkills, mcpActivateSkill} {
		if !got[want] {
			t.Errorf("maestroPlanTools missing %q (needed to investigate the repo like plan mode)", want)
		}
	}
	if !got[mcpEscalate] {
		t.Errorf("maestroPlanTools missing mcpEscalate (a genuinely ambiguous task must still be escalatable)")
	}
}

// TestPlanSectionDegradesToTaskOnly asserts planSection/coderSystemPrompt degrade
// cleanly to a task-only prompt when planning produced nothing (best-effort
// planning, PRD §7.2) — and that a real plan IS attached when present.
func TestPlanSectionDegradesToTaskOnly(t *testing.T) {
	if got := planSection(""); got != "" {
		t.Errorf("planSection(\"\") = %q, want empty", got)
	}
	if got := planSection("   \n\t "); got != "" {
		t.Errorf("planSection(whitespace-only) = %q, want empty", got)
	}

	taskOnly := coderSystemPrompt("do the thing", "", false)
	if strings.Contains(taskOnly, "investigated the repo") {
		t.Errorf("coderSystemPrompt with no plan should not mention a plan, got: %q", taskOnly)
	}
	if !strings.Contains(taskOnly, "do the thing") {
		t.Errorf("coderSystemPrompt must still contain the task, got: %q", taskOnly)
	}

	withPlan := coderSystemPrompt("do the thing", "Step 1: touch foo.go", false)
	if !strings.Contains(withPlan, "Step 1: touch foo.go") {
		t.Errorf("coderSystemPrompt with a plan must attach it, got: %q", withPlan)
	}
	if !strings.Contains(withPlan, "Follow it on the first coding pass") {
		t.Errorf("coderSystemPrompt should tell the first pass to follow the plan, got: %q", withPlan)
	}
	if !strings.Contains(withPlan, "take precedence") {
		t.Errorf("coderSystemPrompt should say findings/rework take precedence, got: %q", withPlan)
	}

	// Same bytes on a "would-be fix" turn — rewriting the prefix used to bust
	// --resume prompt cache.
	again := coderSystemPrompt("do the thing", "Step 1: touch foo.go", false)
	if again != withPlan {
		t.Errorf("coderSystemPrompt must be stable across iterations")
	}
}

// planBriefRunner is a TEST DOUBLE that scripts a full loop: the Maestro planning
// turn emits a fixed plan text, the Coder edits a file each turn (recording every
// spec it was called with), and the Reviewer files exactly one finding on the
// first iteration (forcing a second, fix, Coder turn) then converges.
//
// When planViaCreatePlan is set, the Maestro turn mimics Cursor plan mode: the
// brief lands in a createPlan tool call and assistant text is only a short ack
// that must NOT win over the tool args when persisting DefinitionOfDone.
type planBriefRunner struct {
	planText          string
	planViaCreatePlan bool

	mu          sync.Mutex
	coderCalls  []AgentSpec
	reviewCalls []AgentSpec
	planSpec    *AgentSpec
	reviewIter  int
}

func (r *planBriefRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	switch spec.Role {
	case domain.RoleMaestro:
		r.mu.Lock()
		s := spec
		r.planSpec = &s
		r.mu.Unlock()
		if emit != nil {
			if r.planViaCreatePlan {
				args, _ := json.Marshal(map[string]string{"plan": r.planText})
				emit(ParsedEvent{
					Kind: KindStep, Tool: "createPlan", ToolInputRaw: args,
					Provider: domain.ProviderCursor,
				})
				emit(ParsedEvent{Kind: KindText, Text: "I've written up the plan."})
				return Result{
					Subtype: "success", SessionID: spec.AgentID,
					Text: "I've written up the plan.",
				}, nil
			}
			emit(ParsedEvent{Kind: KindText, Text: r.planText})
		}
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil

	case domain.RoleCoder:
		r.mu.Lock()
		r.coderCalls = append(r.coderCalls, spec)
		n := len(r.coderCalls)
		r.mu.Unlock()
		sr := &ScriptedRunner{Edits: map[string]string{"work.txt": strings.Repeat("x\n", n)}}
		if err := sr.applyEdits(spec.Worktree); err != nil {
			return Result{}, err
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
		r.reviewCalls = append(r.reviewCalls, spec)
		r.mu.Unlock()
		if iter == 1 {
			if err := postFinding(spec.MCPURL, "work.txt", 1, "medium", "needs a fix"); err != nil {
				return Result{}, err
			}
		}
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil

	default:
		return Result{Subtype: "success"}, nil
	}
}

// TestPlannerPlanReachesCoderExactlyOnce is the engine-level handoff test for this
// task: the Maestro's planning-turn text (a) uses the read-only investigation
// tools, (b) lands in the loop's DefinitionOfDone, and (c) is threaded into EVERY
// Coder turn's system prompt (not just the first) — but is NOT duplicated into the
// per-iteration user/fix prompt, since coderSystemPrompt already carries it (fix
// for the "plan sent twice" issue).
func TestPlannerPlanReachesCoderExactlyOnce(t *testing.T) {
	repo := initRepo(t)
	const planNeedle = "Steps:\n1. Touch work.txt and add the marker line"
	planText := "Summary: add a marker to work.txt.\n\nRelevant files:\n- work.txt — target file\n\n" +
		planNeedle + "\n\nEdge cases / gotchas: none identified\n\nDefinition of done: work.txt exists with the marker."

	runner := &planBriefRunner{planText: planText}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Add a marker to work.txt", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 10*time.Second, "loop to reach pr_open", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})

	// The planning turn used the read-only investigation tool scope (no Edit/Write/Bash).
	runner.mu.Lock()
	planSpec := runner.planSpec
	runner.mu.Unlock()
	if planSpec == nil {
		t.Fatal("Maestro planning turn never ran")
	}
	for _, tool := range planSpec.AllowedTools {
		if tool == "Edit" || tool == "Write" || tool == "Bash" {
			t.Errorf("planning turn spec.AllowedTools contained %q, want read-only", tool)
		}
	}

	// The plan text persisted to DefinitionOfDone (PRD §7.2; also asserts the
	// planMaxPersistChars bound doesn't truncate a plan well under that size).
	l, _ := st.Workspace(id)
	if !strings.Contains(l.DefinitionOfDone, planNeedle) {
		t.Errorf("planner's plan text did not persist to DefinitionOfDone: %q", l.DefinitionOfDone)
	}

	runner.mu.Lock()
	calls := append([]AgentSpec(nil), runner.coderCalls...)
	runner.mu.Unlock()
	if len(calls) < 2 {
		t.Fatalf("expected at least 2 Coder turns (initial + fix), got %d", len(calls))
	}

	// Iteration 1: the plan is attached in the system prompt.
	if !strings.Contains(calls[0].SystemPrompt, planNeedle) {
		t.Errorf("iter1 Coder system prompt missing the Maestro's plan")
	}

	// Iteration 2 (the fix turn): every Coder turn is still briefed via the system
	// prompt, but the plan must NOT also be duplicated into the user/fix prompt —
	// that would double its token cost on every convergence iteration.
	if !strings.Contains(calls[1].SystemPrompt, planNeedle) {
		t.Errorf("iter2 Coder system prompt missing the Maestro's plan")
	}
	if strings.Contains(calls[1].Prompt, planNeedle) {
		t.Errorf("iter2 Coder USER prompt repeats the plan (should be system-prompt-only now), got: %q", calls[1].Prompt)
	}
	if !strings.Contains(calls[1].Prompt, "needs a fix") {
		t.Errorf("iter2 Coder user prompt should still contain the reviewer's finding, got: %q", calls[1].Prompt)
	}
	if calls[0].SystemPrompt != calls[1].SystemPrompt {
		t.Errorf("Coder system prompt changed across iterations (busts resume cache)")
	}
}

// TestPlannerCreatePlanHarvestedOverNarration asserts that when a Cursor-backed
// Maestro planning turn records its brief via createPlan (with only a short ack
// in assistant text), runPlanner persists the tool's plan body — not the
// narration — as DefinitionOfDone and briefs the Coder with it.
func TestPlannerCreatePlanHarvestedOverNarration(t *testing.T) {
	repo := initRepo(t)
	const planNeedle = "Steps:\n1. Touch work.txt and add the marker line"
	planText := "Summary: add a marker to work.txt.\n\nRelevant files:\n- work.txt — target file\n\n" +
		planNeedle + "\n\nEdge cases / gotchas: none identified\n\nDefinition of done: work.txt exists with the marker."

	runner := &planBriefRunner{planText: planText, planViaCreatePlan: true}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Add a marker to work.txt", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 10*time.Second, "loop to reach pr_open", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})

	l, _ := st.Workspace(id)
	if !strings.Contains(l.DefinitionOfDone, planNeedle) {
		t.Errorf("createPlan body not persisted to DefinitionOfDone: %q", l.DefinitionOfDone)
	}
	if strings.Contains(l.DefinitionOfDone, "I've written up the plan") {
		t.Errorf("assistant narration won over createPlan: %q", l.DefinitionOfDone)
	}
	if l.Plan == nil || !strings.Contains(l.Plan.Body, planNeedle) {
		t.Errorf("createPlan body not filed as Plan artifact: %+v", l.Plan)
	}
	if l.Plan != nil && len(l.Plan.Steps) != 0 {
		t.Errorf("createPlan harvest should have no structured steps, got %d steps", len(l.Plan.Steps))
	}
	if l.Plan != nil && l.Plan.Source != "loop-planner" {
		t.Errorf("source = %q, want loop-planner", l.Plan.Source)
	}

	runner.mu.Lock()
	calls := append([]AgentSpec(nil), runner.coderCalls...)
	runner.mu.Unlock()
	if len(calls) == 0 {
		t.Fatal("Coder never ran")
	}
	if !strings.Contains(calls[0].SystemPrompt, planNeedle) {
		t.Errorf("Coder system prompt missing harvested createPlan brief")
	}
}

// TestReviewerReceivesMaestroPlan is the regression test for threading the
// Maestro's planning-turn brief into the Reviewer's system prompt: convergence
// (PRD §7.3) is decided off the Reviewer's findings, so a Reviewer that never sees
// the plan's DEFINITION OF DONE can only judge the diff against the bare one-line
// task string — it has no way to know what "done" concretely means for this task,
// or which files/edge-cases were actually in scope. This drives a full loop (via
// planBriefRunner, which now records every Reviewer spec) and asserts EVERY
// Reviewer turn's system prompt carries the plan text, mirroring the existing
// Coder-side assertion in TestPlannerPlanReachesCoderExactlyOnce.
func TestReviewerReceivesMaestroPlan(t *testing.T) {
	repo := initRepo(t)
	const planNeedle = "Definition of done: work.txt exists with the marker."
	planText := "Summary: add a marker to work.txt.\n\nRelevant files:\n- work.txt — target file\n\n" +
		"Steps:\n1. Touch work.txt and add the marker line\n\nEdge cases / gotchas: none identified\n\n" +
		planNeedle

	runner := &planBriefRunner{planText: planText}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Add a marker to work.txt", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 10*time.Second, "loop to reach pr_open", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})

	runner.mu.Lock()
	reviews := append([]AgentSpec(nil), runner.reviewCalls...)
	runner.mu.Unlock()
	if len(reviews) < 2 {
		t.Fatalf("expected at least 2 Reviewer turns (initial + re-review), got %d", len(reviews))
	}
	for i, spec := range reviews {
		if !strings.Contains(spec.SystemPrompt, planNeedle) {
			t.Errorf("review iteration %d system prompt missing the Maestro's plan/definition-of-done: %q", i+1, spec.SystemPrompt)
		}
	}
}

// TestTruncateRuneSafe asserts truncate() never splits a multi-byte UTF-8 rune,
// which the old byte-slice implementation could do — and easily would, now that
// plan-mode prose (em dashes, curly quotes, arrows) routinely crosses the
// planMaxPersistChars DefinitionOfDone bound. A mid-rune cut stores invalid UTF-8.
func TestTruncateRuneSafe(t *testing.T) {
	// Build a string whose n-1 byte cutoff can land in the middle of a 3-byte rune
	// (an em dash, U+2014, encodes as 0xE2 0x80 0x94) for several values of n.
	s := strings.Repeat("a", 9) + "—" + strings.Repeat("b", 20)
	for n := 8; n <= 13; n++ {
		got := truncate(s, n)
		if !utf8.ValidString(got) {
			t.Errorf("truncate(%q, %d) = %q is not valid UTF-8", s, n, got)
		}
		if len(s) > n && !strings.HasSuffix(got, "…") {
			t.Errorf("truncate(%q, %d) = %q, want an ellipsis suffix", s, n, got)
		}
	}

	// A longer, still-multi-byte-laden string at the production DefinitionOfDone
	// bound (planMaxPersistChars) must also stay valid UTF-8 end to end.
	long := strings.Repeat("plan step — with an em dash and “curly quotes” → arrow. ", 2000)
	got := truncate(long, planMaxPersistChars)
	if !utf8.ValidString(got) {
		t.Errorf("truncate(long, planMaxPersistChars) produced invalid UTF-8")
	}
	if len(got) > planMaxPersistChars+3 { // +3 for the ellipsis rune's bytes, minus what was trimmed
		t.Errorf("truncate(long, planMaxPersistChars) = %d bytes, want roughly <= %d", len(got), planMaxPersistChars)
	}
}

// TestSendBackKeepsPersistedPlan: after a run opens a PR, Send Back is a
// conductor turn. The Maestro's plan stays on DefinitionOfDone.
func TestSendBackKeepsPersistedPlan(t *testing.T) {
	repo := initRepo(t)
	const planNeedle = "Steps:\n1. Touch work.txt and add the marker line"
	planText := "Summary: add a marker to work.txt.\n\nRelevant files:\n- work.txt — target file\n\n" +
		planNeedle + "\n\nEdge cases / gotchas: none identified\n\nDefinition of done: work.txt exists with the marker."

	runner := &planBriefRunner{planText: planText}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Add a marker to work.txt", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 10*time.Second, "first run to open a PR", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})
	pr, ok := st.PRByWorkspace(id)
	if !ok {
		t.Fatal("no PR created on the first run")
	}

	if _, ok := eng.SendBackToLoop(pr.ID, []string{"please add one more marker"}); !ok {
		t.Fatal("SendBackToLoop returned not-ok")
	}
	p, _ := st.PR(pr.ID)
	if p.State != "In review" {
		t.Errorf("SendBackToLoop() state = %q, want In review", p.State)
	}
	l, _ := st.Workspace(id)
	if l.State != domain.StatePROpen {
		t.Errorf("workspace state = %q, want pr_open", l.State)
	}
	if !strings.Contains(l.DefinitionOfDone, planNeedle) {
		t.Errorf("DefinitionOfDone after send-back = %q, want plan needle", l.DefinitionOfDone)
	}
	runner.mu.Lock()
	calls := append([]AgentSpec(nil), runner.coderCalls...)
	runner.mu.Unlock()
	for i := range calls {
		if strings.Contains(calls[i].Prompt, "Maestro-distilled") || strings.Contains(calls[i].Prompt, "sent it back") {
			t.Fatalf("SendBackToLoop started a Coder; prompt=%q", calls[i].Prompt)
		}
	}
}
