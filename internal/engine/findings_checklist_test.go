package engine

import (
	"strings"
	"testing"

	"recur/internal/bundles"
	"recur/internal/domain"
)

// TestCoderDoneRefusesOpenFindings covers the continue-turn nudge: done fails
// while findings are open, succeeds after update_finding resolves them.
func TestCoderDoneRefusesOpenFindings(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	ws := domain.Workspace{ID: "L-fc", ProjectID: "P1", Goal: "t", State: domain.StateCoding}
	if _, err := st.AddWorkspace(ws); err != nil {
		t.Fatal(err)
	}

	rep := engineReporter{eng: eng}
	st.AddFinding(domain.Finding{
		ID: "F1", WorkspaceID: "L-fc", Iteration: 1,
		File: "a.go", Line: 1, Severity: domain.SevHigh, Message: "bug",
		Status: string(domain.FindingOpen),
	})

	err := rep.Done("L-fc", "CO", "all good", "")
	if err == nil {
		t.Fatal("done should refuse while findings are open")
	}
	if !strings.Contains(err.Error(), "F1") {
		t.Errorf("done error should list open finding ids, got %v", err)
	}

	if _, err := rep.UpdateFinding("L-fc", "CO", "F1", "resolved", ""); err != nil {
		t.Fatalf("update_finding resolved: %v", err)
	}
	got := st.Findings("L-fc")[0]
	if got.Status != string(domain.FindingResolved) || got.UpdatedBy != domain.FindingActorCoder {
		t.Fatalf("expected coder-resolved finding, got %+v", got)
	}
	if err := rep.Done("L-fc", "CO", "fixed", ""); err != nil {
		t.Fatalf("done after resolve: %v", err)
	}
}

func TestCoderDoneStoresCommitSummary(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	if _, err := st.AddWorkspace(domain.Workspace{ID: "L-cs", ProjectID: "P1", Goal: "t", State: domain.StateCoding}); err != nil {
		t.Fatal(err)
	}
	lp := &Loop{ID: "L-cs"}
	eng.loops.Store("L-cs", &runningLoop{loop: lp})
	t.Cleanup(func() { eng.loops.Delete("L-cs") })

	rep := engineReporter{eng: eng}
	if err := rep.Done("L-cs", "CO", "Add holdings precheck", ""); err != nil {
		t.Fatalf("Done(L-cs, CO, ...) = %v, want nil", err)
	}
	if got := lp.takeDoneSummary(); got != "Add holdings precheck" {
		t.Errorf("takeDoneSummary() = %q, want %q", got, "Add holdings precheck")
	}
	if got := lp.takeDoneSummary(); got != "" {
		t.Errorf("second takeDoneSummary() = %q, want empty", got)
	}
}

func TestWriterDoneStoresCommitSummary(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	if _, err := st.AddWorkspace(domain.Workspace{ID: "L-wr", ProjectID: "P1", Goal: "t", State: domain.StateCoding}); err != nil {
		t.Fatal(err)
	}
	lp := &Loop{
		ID:  "L-wr",
		eng: eng,
		currentPhase: bundles.LoopPhase{
			ID:    "converge",
			Roles: map[string]string{"implementer": "recur.roles.writer"},
		},
		catalog: bundles.Catalog{
			Roles: []bundles.Role{{
				ID:   "recur.roles.writer",
				Code: "WR",
				Name: "Writer",
				Mode: bundles.RoleModeAgent,
			}},
		},
	}
	eng.loops.Store("L-wr", &runningLoop{loop: lp})
	t.Cleanup(func() { eng.loops.Delete("L-wr") })

	rep := engineReporter{eng: eng}
	st.AddFinding(domain.Finding{
		ID: "F-open", WorkspaceID: "L-wr", Iteration: 1,
		File: "a.go", Line: 1, Severity: domain.SevHigh, Message: "bug",
		Status: string(domain.FindingOpen),
	})
	if err := rep.Done("L-wr", "WR", "Draft the README outline", ""); err != nil {
		t.Fatalf("Done(L-wr, WR, ...) = %v, want nil", err)
	}
	if got := lp.takeDoneSummary(); got != "Draft the README outline" {
		t.Errorf("takeDoneSummary() = %q, want %q", got, "Draft the README outline")
	}
}

func TestDoneSummaryRoutesToOwningLoop(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	if _, err := st.AddWorkspace(domain.Workspace{ID: "L-ml", ProjectID: "P1", Goal: "t", State: domain.StateCoding}); err != nil {
		t.Fatal(err)
	}
	loopA := &Loop{ID: "L-ml", ChatCode: "PA"}
	loopB := &Loop{ID: "L-ml", ChatCode: "PA2"}
	eng.loops.Store("L-ml", &runningLoop{loop: loopA})
	eng.loops.Store("L-ml"+loopMapSep+"PA2", &runningLoop{loop: loopB})
	t.Cleanup(func() {
		eng.loops.Delete("L-ml")
		eng.loops.Delete("L-ml" + loopMapSep + "PA2")
	})
	_, clear := loopB.bindInject("CO")
	t.Cleanup(clear)

	rep := engineReporter{eng: eng}
	if err := rep.Done("L-ml", "CO", "B's holdings precheck", ""); err != nil {
		t.Fatalf("Done(L-ml, CO, ...) = %v, want nil", err)
	}
	if got := loopA.takeDoneSummary(); got != "" {
		t.Errorf("loopA.takeDoneSummary() = %q, want empty (sibling must not receive B's summary)", got)
	}
	if got := loopB.takeDoneSummary(); got != "B's holdings precheck" {
		t.Errorf("loopB.takeDoneSummary() = %q, want %q", got, "B's holdings precheck")
	}
}

func TestDoneSummarySkipsAmbiguousOwner(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	if _, err := st.AddWorkspace(domain.Workspace{ID: "L-amb", ProjectID: "P1", Goal: "t", State: domain.StateCoding}); err != nil {
		t.Fatal(err)
	}
	loopA := &Loop{ID: "L-amb", ChatCode: "PA"}
	loopB := &Loop{ID: "L-amb", ChatCode: "PA2"}
	eng.loops.Store("L-amb", &runningLoop{loop: loopA})
	eng.loops.Store("L-amb"+loopMapSep+"PA2", &runningLoop{loop: loopB})
	t.Cleanup(func() {
		eng.loops.Delete("L-amb")
		eng.loops.Delete("L-amb" + loopMapSep + "PA2")
	})
	_, clearA := loopA.bindInject("CO")
	_, clearB := loopB.bindInject("CO")
	t.Cleanup(clearA)
	t.Cleanup(clearB)

	rep := engineReporter{eng: eng}
	if err := rep.Done("L-amb", "CO", "A's holdings precheck", ""); err != nil {
		t.Fatalf("Done(L-amb, CO, ...) = %v, want nil", err)
	}
	if got := loopA.takeDoneSummary(); got != "" {
		t.Errorf("loopA.takeDoneSummary() = %q, want empty (ambiguous CO owner)", got)
	}
	if got := loopB.takeDoneSummary(); got != "" {
		t.Errorf("loopB.takeDoneSummary() = %q, want empty (ambiguous CO owner)", got)
	}
}

func TestFindingUpdateActor(t *testing.T) {
	tests := []struct {
		code string
		want string
	}{
		{"CO", domain.FindingActorCoder},
		{"RV", domain.FindingActorReviewer},
		{"SR", domain.FindingActorReviewer},
		{"PA", domain.FindingActorCoder},
		{"", domain.FindingActorCoder},
	}
	for _, tt := range tests {
		got := findingUpdateActor(nil, "L", tt.code)
		if got != tt.want {
			t.Errorf("findingUpdateActor(nil, L, %q) = %q, want %q", tt.code, got, tt.want)
		}
	}
}

func TestReviewerUpdateFindingPreservesAttribution(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	if _, err := st.AddWorkspace(domain.Workspace{ID: "L-rv", ProjectID: "P1", Goal: "t"}); err != nil {
		t.Fatal(err)
	}
	rep := engineReporter{eng: eng}
	st.AddFinding(domain.Finding{
		ID: "F-ok", WorkspaceID: "L-rv", Iteration: 1,
		File: "a.go", Line: 1, Severity: domain.SevHigh, Message: "bug",
		Status: string(domain.FindingOpen),
	})
	st.AddFinding(domain.Finding{
		ID: "F-no", WorkspaceID: "L-rv", Iteration: 1,
		File: "b.go", Line: 2, Severity: domain.SevLow, Message: "nit",
		Status: string(domain.FindingOpen),
	})

	got, err := rep.UpdateFinding("L-rv", "RV", "F-ok", "resolved", "")
	if err != nil {
		t.Fatalf("UpdateFinding(L-rv, RV, F-ok, resolved) = %v, want nil", err)
	}
	if got.UpdatedBy != domain.FindingActorReviewer {
		t.Errorf("UpdateFinding(L-rv, RV, F-ok, resolved) UpdatedBy = %q, want %q", got.UpdatedBy, domain.FindingActorReviewer)
	}

	got, err = rep.UpdateFinding("L-rv", "RV", "F-no", "invalid", "already addressed")
	if err != nil {
		t.Fatalf("UpdateFinding(L-rv, RV, F-no, invalid) = %v, want nil", err)
	}
	if got.Status != string(domain.FindingInvalid) || got.UpdatedBy != domain.FindingActorReviewer {
		t.Errorf("UpdateFinding(L-rv, RV, F-no, invalid) = %+v, want invalid by reviewer", got)
	}

	prompt := reviewerPromptFor(&Loop{ID: "L-rv", Goal: "t", eng: eng}, 2)
	if strings.Contains(prompt, "Coder disputed") {
		t.Fatalf("reviewer invalidation must not appear as a coder dispute, got:\n%s", prompt)
	}

	st.UpsertAgent("L-rv", domain.Agent{ID: "L-rv-PA", WorkspaceID: "L-rv", Code: "PA", Name: "Agent", Model: "sonnet"})
	eng.RememberWorkspaceChatSelection("L-rv", "PA", "sonnet", nil, domain.ChatModeReview, nil)
	st.AddFinding(domain.Finding{
		ID: "F-rm", WorkspaceID: "L-rv", Iteration: 1,
		File: "c.go", Line: 3, Severity: domain.SevMedium, Message: "regress",
		Status: string(domain.FindingOpen),
	})
	got, err = rep.UpdateFinding("L-rv", "PA", "F-rm", "resolved", "")
	if err != nil {
		t.Fatalf("UpdateFinding(L-rv, PA, F-rm, resolved) = %v, want nil", err)
	}
	if got.UpdatedBy != domain.FindingActorReviewer {
		t.Errorf("UpdateFinding(L-rv, PA, F-rm, resolved) UpdatedBy = %q, want %q", got.UpdatedBy, domain.FindingActorReviewer)
	}
}

func TestCoderUpdateFindingInvalidRequiresReason(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	if _, err := st.AddWorkspace(domain.Workspace{ID: "L-inv", ProjectID: "P1", Goal: "t"}); err != nil {
		t.Fatal(err)
	}
	rep := engineReporter{eng: eng}
	st.AddFinding(domain.Finding{
		ID: "F2", WorkspaceID: "L-inv", Iteration: 1,
		File: "b.go", Line: 2, Severity: domain.SevLow, Message: "nit",
		Status: string(domain.FindingOpen),
	})

	if _, err := rep.UpdateFinding("L-inv", "CO", "F2", "invalid", ""); err == nil {
		t.Fatal("invalid without reason should fail")
	}
	f, err := rep.UpdateFinding("L-inv", "CO", "F2", "invalid", "out of scope for this task")
	if err != nil {
		t.Fatalf("invalid with reason: %v", err)
	}
	if f.StatusReason != "out of scope for this task" || f.UpdatedBy != domain.FindingActorCoder {
		t.Fatalf("got %+v", f)
	}
}

func TestCoderCannotTouchHumanIgnored(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	if _, err := st.AddWorkspace(domain.Workspace{ID: "L-ig", ProjectID: "P1", Goal: "t"}); err != nil {
		t.Fatal(err)
	}
	rep := engineReporter{eng: eng}
	st.AddFinding(domain.Finding{
		ID: "F3", WorkspaceID: "L-ig", Iteration: 1,
		File: "c.go", Line: 3, Severity: domain.SevMedium, Message: "x",
		Status: string(domain.FindingOpen),
	})
	if _, err := st.SetFindingStatus("L-ig", "F3", string(domain.FindingIgnored)); err != nil {
		t.Fatal(err)
	}
	if _, err := rep.UpdateFinding("L-ig", "CO", "F3", "resolved", ""); err == nil {
		t.Fatal("coder must not change human-ignored findings")
	}
}

func TestReviewerPromptIncludesCoderDisputedFindings(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	ws := domain.Workspace{ID: "L-dp", ProjectID: "P1", Goal: "ship feature", State: domain.StateReviewing}
	if _, err := st.AddWorkspace(ws); err != nil {
		t.Fatal(err)
	}
	st.AddFinding(domain.Finding{
		ID: "F4", WorkspaceID: "L-dp", Iteration: 1,
		File: "d.go", Line: 4, Severity: domain.SevHigh, Message: "missing nil check",
		Status: string(domain.FindingInvalid), StatusReason: "nil is impossible here",
		UpdatedBy: domain.FindingActorCoder,
	})
	// Dispute from another iteration must not leak into this review's prompt.
	st.AddFinding(domain.Finding{
		ID: "F5", WorkspaceID: "L-dp", Iteration: 3,
		File: "e.go", Line: 5, Severity: domain.SevMedium, Message: "later dispute",
		Status: string(domain.FindingInvalid), StatusReason: "from a future cycle",
		UpdatedBy: domain.FindingActorCoder,
	})
	lp := &Loop{ID: "L-dp", Goal: "ship feature", eng: eng}
	prompt := reviewerPromptFor(lp, 2) // previous review iteration = 1 → F4 only
	if !strings.Contains(prompt, "Coder disputed") {
		t.Fatalf("expected disputed-findings section, got:\n%s", prompt)
	}
	if !strings.Contains(prompt, "nil is impossible here") {
		t.Fatalf("expected coder reason in prompt, got:\n%s", prompt)
	}
	if strings.Contains(prompt, "from a future cycle") {
		t.Fatalf("must not inject disputes from other iterations, got:\n%s", prompt)
	}
	// First review of a run/rework: no prior coding disputes to re-judge.
	if got := reviewerPromptFor(lp, 1); strings.Contains(got, "Coder disputed") {
		t.Fatalf("iter 1 must not inject disputed findings, got:\n%s", got)
	}
}

func TestAgentDoneAllowsOpenFindings(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	if _, err := st.AddWorkspace(domain.Workspace{ID: "L-pa", ProjectID: "P1", Goal: "t"}); err != nil {
		t.Fatal(err)
	}
	rep := engineReporter{eng: eng}
	st.AddFinding(domain.Finding{
		ID: "F-open", WorkspaceID: "L-pa", Iteration: 1,
		File: "a.go", Line: 1, Severity: domain.SevHigh, Message: "bug",
		Status: string(domain.FindingOpen),
	})
	// Interactive Agent chat (PA) must not be blocked by leftover opens.
	if err := rep.Done("L-pa", "PA", "unrelated work done", ""); err != nil {
		t.Fatalf("agent done should allow open findings, got %v", err)
	}
	// Loop Coder still refused.
	if err := rep.Done("L-pa", "CO", "all good", ""); err == nil {
		t.Fatal("coder done should refuse while findings are open")
	}
}
