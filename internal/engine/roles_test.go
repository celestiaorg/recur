package engine

import (
	"strings"
	"testing"

	"recur/internal/domain"
)

func TestCoderToolsIncludeExploreAndEdit(t *testing.T) {
	tools := roles[domain.RoleCoder].allowedTools
	for _, want := range []string{
		"Read", "Glob", "Grep", "LSP", "WebFetch", "WebSearch", "Edit", "Write", "Bash", "NotebookEdit",
		mcpDone, mcpListFindings, mcpUpdateFinding, mcpListSkills, mcpActivateSkill,
	} {
		if !containsStr(tools, want) {
			t.Errorf("Coder missing tool %q; tools=%v", want, tools)
		}
	}
	if containsStr(tools, mcpReportFinding) {
		t.Errorf("Coder must NOT have report_finding; tools=%v", tools)
	}
}

func TestReviewerIsReadOnly(t *testing.T) {
	// Reviewer: bare Bash for lint/test under bypass, plus scoped git rules so
	// `git diff` still works when BypassPermissions is off (dontAsk denies bare
	// Bash). Edit/Write stay off the allowlist (+ --disallowedTools).
	tools := roles[domain.RoleReviewer].allowedTools
	for _, want := range []string{"Read", "Glob", "Grep", "LSP", "WebFetch", "WebSearch", "Bash", mcpReportFinding, mcpListFindings, mcpUpdateFinding, mcpListSkills, mcpActivateSkill} {
		if !containsStr(tools, want) {
			t.Errorf("Reviewer missing tool %q; tools=%v", want, tools)
		}
	}
	for _, banned := range []string{"Edit", "Write", "NotebookEdit"} {
		if containsStr(tools, banned) {
			t.Errorf("Reviewer must NOT have %q; tools=%v", banned, tools)
		}
	}
	for _, want := range gitReadTools {
		if !containsStr(tools, want) {
			t.Errorf("Reviewer missing scoped git Bash %q (needed under dontAsk opt-out)", want)
		}
	}
	deny := disallowedEditTools(tools)
	for _, want := range editBuiltins {
		if !containsStr(deny, want) {
			t.Errorf("Reviewer allowlist should disallow %q, got %v", want, deny)
		}
	}
	// Sanity: at least one Bash( rule uses the expected prefix form.
	hasScoped := false
	for _, tl := range tools {
		if strings.HasPrefix(tl, "Bash(") {
			hasScoped = true
			break
		}
	}
	if !hasScoped {
		t.Error("Reviewer must keep scoped Bash(git…) rules alongside bare Bash")
	}
}

func TestTesterToolsIncludeExploreAndEdit(t *testing.T) {
	tools := roles[domain.RoleTester].allowedTools
	for _, want := range []string{"Read", "Glob", "Grep", "WebFetch", "WebSearch", "Edit", "Write", "Bash", mcpReportResult, mcpListFindings, mcpListSkills, mcpActivateSkill} {
		if !containsStr(tools, want) {
			t.Errorf("Tester missing tool %q; tools=%v", want, tools)
		}
	}
}

func TestCoderDispatchDisplay(t *testing.T) {
	findings := []domain.Finding{{ID: "F1", Severity: "medium", File: "a.go", Line: 1, Message: "off-by-one"}}
	if got := coderDispatchDisplay(coderFixPrompt("pong", findings)); got != coderFixDisplay {
		t.Errorf("fix prompt display = %q, want %q", got, coderFixDisplay)
	}
	if got := coderDispatchDisplay(coderOpenFindingsNudgePrompt("pong", findings)); got != coderFixDisplay {
		t.Errorf("nudge prompt display = %q, want %q", got, coderFixDisplay)
	}
	if got := coderDispatchDisplay(coderResumePrompt("pong", 2, true)); got != coderResumeDisplay {
		t.Errorf("resume (restored) display = %q, want %q", got, coderResumeDisplay)
	}
	if got := coderDispatchDisplay(coderResumePrompt("pong", 2, false)); got != coderResumeDisplay {
		t.Errorf("resume (fresh) display = %q, want %q", got, coderResumeDisplay)
	}
	if got := coderDispatchDisplay("create a cli game of pong"); got != "" {
		t.Errorf("bare goal display = %q, want empty (show the task)", got)
	}
}

func TestQACanRead(t *testing.T) {
	tools := roles[domain.RoleQA].allowedTools
	for _, want := range []string{"Read", mcpAttachEvidence, mcpPlaywrightAll, mcpReportResult, mcpListSkills, mcpActivateSkill} {
		if !containsStr(tools, want) {
			t.Errorf("QA missing %q; tools=%v", want, tools)
		}
	}
	for _, banned := range []string{"Edit", "Write", "NotebookEdit"} {
		if containsStr(tools, banned) {
			t.Errorf("QA must NOT have %q; tools=%v", banned, tools)
		}
	}
}
