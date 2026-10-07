package domain

import (
	"strings"
	"testing"
)

func TestPlanTextPrefersBodyAndIncludesStepsAndAttachments(t *testing.T) {
	p := Plan{
		Summary: "Headline only",
		Body:    "Touch `internal/engine/plan.go` and keep the checklist honest.",
		Steps: []PlanStep{
			{ID: "S1", Title: "Add Body to Plan", Status: StepDone},
			{ID: "S2", Title: "Wire submit_plan", Status: StepPending},
		},
		Attachments: []PlanAttachment{
			{ID: "a1", Kind: "md", Title: "Sequence diagram", Path: "workspaces/w/artifacts/a1.md"},
			{ID: "img", Kind: "png", Title: "", Path: "workspaces/w/attachments/img.png"},
		},
	}
	got := p.Text()
	if !strings.Contains(got, "Touch `internal/engine/plan.go`") {
		t.Errorf("Text() should use Body, got %q", got)
	}
	if strings.Contains(got, "Headline only") {
		t.Errorf("Text() should not prepend Summary when Body is set, got %q", got)
	}
	if !strings.Contains(got, "1. Add Body to Plan (done)") {
		t.Errorf("Text() missing live step status, got %q", got)
	}
	if !strings.Contains(got, "2. Wire submit_plan") {
		t.Errorf("Text() missing pending step, got %q", got)
	}
	if !strings.Contains(got, "`workspaces/w/artifacts/a1.md` - Sequence diagram") {
		t.Errorf("Text() missing titled attachment, got %q", got)
	}
	if !strings.Contains(got, "`workspaces/w/attachments/img.png`") {
		t.Errorf("Text() missing path-only attachment, got %q", got)
	}
}

func TestPlanBoardIncludesDoingFilesDepsAndChild(t *testing.T) {
	p := Plan{
		Body: "Importer",
		Steps: []PlanStep{
			{ID: "S1", Title: "Parser", Status: StepDoing, Files: []string{"parser.go"}, Workspace: "child-1"},
			{ID: "S2", Title: "Writer", Status: StepPending, Files: []string{"writer.go"}, DependsOn: []string{"S1"}, Note: "waiting"},
		},
	}
	got := p.Board()
	if !strings.Contains(got, "S1 [doing] Parser files=parser.go child=child-1") {
		t.Errorf("Board() missing doing/files/child, got %q", got)
	}
	if !strings.Contains(got, "S2 [pending] Writer files=writer.go after=S1 note=waiting") {
		t.Errorf("Board() missing deps/note, got %q", got)
	}
	if strings.Contains(p.Text(), "files=") || strings.Contains(p.Text(), "[doing]") {
		t.Errorf("Text() must stay prose without board metadata, got %q", p.Text())
	}
}

func TestPlanTextFallsBackToSummary(t *testing.T) {
	p := Plan{Summary: "Add a dark-mode toggle", Steps: []PlanStep{
		{Title: "Add the toggle to Settings", Status: StepPending},
	}}
	got := p.Text()
	if !strings.Contains(got, "Add a dark-mode toggle") || !strings.Contains(got, "1. Add the toggle to Settings") {
		t.Errorf("legacy summary-only Text() = %q", got)
	}
}

func TestPlanTextBlankLineBeforeAttachmentsWithoutSteps(t *testing.T) {
	p := Plan{
		Body: "Rerun the tests.",
		Attachments: []PlanAttachment{
			{ID: "img", Kind: "png", Path: "workspaces/w/attachments/img.png"},
		},
	}
	got := p.Text()
	if !strings.Contains(got, "Rerun the tests.\n\nAttachments:\n") {
		t.Errorf("Text() = %q, want blank line before Attachments", got)
	}
}
