package engine

import (
	"fmt"
	"strings"
	"testing"

	"recur/internal/domain"
)

func TestFormatOpenPRToolResultLocalFallbackIncludesReason(t *testing.T) {
	pr := domain.PR{Number: 164, Source: domain.SourceLocal}
	got := formatOpenPRToolResult(pr, true, false, false, "gh pr create failed: HTTP 401")
	if !strings.Contains(got, "Opened Local PR #164") {
		t.Errorf("got %q, want Local PR #164", got)
	}
	if !strings.Contains(got, "HTTP 401") {
		t.Errorf("got %q, want the GitHub fail reason", got)
	}
	if strings.Contains(got, "Opened GitHub PR") {
		t.Errorf("got %q, must not claim a GitHub PR opened", got)
	}

	retry := formatOpenPRToolResult(pr, true, true, true, "git push failed: permission denied")
	if !strings.Contains(retry, "already open (Local)") {
		t.Errorf("retry %q, want already open Local", retry)
	}
	if !strings.Contains(retry, "permission denied") {
		t.Errorf("retry %q, want the GitHub fail reason", retry)
	}
}

func TestFormatOpenPRToolResultKeepsCreateErrorWhenBodyIsLong(t *testing.T) {
	body := strings.Repeat("word ", 80)
	wrapped := fmt.Sprintf(
		"gh pr create failed: gh %s: GraphQL: Resource not accessible by integration",
		strings.Join(ghCreatePRArgs("feat", "main", "Add feature", body, false), " "),
	)
	pr := domain.PR{Number: 164, Source: domain.SourceLocal}
	got := formatOpenPRToolResult(pr, true, false, false, wrapped)
	if !strings.Contains(got, "Resource not accessible") {
		t.Errorf("formatOpenPRToolResult(wrapped create err) = %q, want the gh error output", got)
	}
	if strings.Contains(got, body[:40]) {
		t.Errorf("formatOpenPRToolResult(wrapped create err) = %q, truncated the --body instead of the error", got)
	}
}
