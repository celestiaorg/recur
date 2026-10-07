package engine

import (
	"context"
	"strings"
	"testing"
)

type textResultRunner struct{ text string }

func (r textResultRunner) Run(_ context.Context, _ AgentSpec, _ func(ParsedEvent)) (Result, error) {
	return Result{Subtype: "success", Text: r.text}, nil
}

func TestGeneratePRDescription(t *testing.T) {
	reply := "Fix the overlay\n\nKeep the sidebar from covering the composer on short viewports."
	eng, _, _, _ := newTestEngine(t, textResultRunner{text: reply})
	title, body, err := eng.generatePRDescription(t.Context(), "ws", t.TempDir(), "diff --git a/x b/x", []string{"fix overlay"})
	if err != nil {
		t.Fatalf("generatePRDescription() err = %v, want nil", err)
	}
	if title != "Fix the overlay" {
		t.Errorf("generatePRDescription() title = %q, want %q", title, "Fix the overlay")
	}
	if body != "Keep the sidebar from covering the composer on short viewports." {
		t.Errorf("generatePRDescription() body = %q, want the model body", body)
	}

	_, _, err = eng.generatePRDescription(t.Context(), "ws", t.TempDir(), "  \n", nil)
	if err == nil {
		t.Error("generatePRDescription(empty diff) err = nil, want an error")
	}
}

func TestSanitizeCommitMessage(t *testing.T) {
	cases := []struct {
		in   string
		want string
	}{
		{"", ""},
		{"  fix: add sparkle button  ", "fix: add sparkle button"},
		{"\"feat: scm AI message\"", "feat: scm AI message"},
		{"```\nfix: fences\n```", "fix: fences"},
		{"```text\nfeat: tagged fence\n\nBody line.\n```", "feat: tagged fence\n\nBody line."},
		{
			"fix: subject\n\nExplain why in a short body.",
			"fix: subject\n\nExplain why in a short body.",
		},
	}
	for _, tc := range cases {
		got := sanitizeCommitMessage(tc.in, commitMessageMaxLen)
		if got != tc.want {
			t.Errorf("sanitizeCommitMessage(%q) = %q, want %q", tc.in, got, tc.want)
		}
	}
}

func TestBuildCommitMessagePromptCapsDiff(t *testing.T) {
	big := strings.Repeat("x", commitMessageDiffCap+100)
	prompt := buildCommitMessagePrompt(big, []string{"feat: prior"})
	for _, part := range []string{"Recent commit subjects", "feat: prior", "Staged diff:", "[diff truncated]"} {
		if !strings.Contains(prompt, part) {
			t.Fatalf("prompt missing %q:\n%s", part, prompt[:min(400, len(prompt))])
		}
	}
	if len(prompt) > commitMessageDiffCap+400 {
		t.Fatalf("prompt still too large: %d", len(prompt))
	}
}

func TestBuildPRDescriptionPromptCoversEveryFile(t *testing.T) {
	lock := "diff --git a/package-lock.json b/package-lock.json\n" + strings.Repeat("x", commitMessageDiffCap)
	src := "diff --git a/src/fix.go b/src/fix.go\n+fixed the overlay\n"
	prompt := buildPRDescriptionPrompt(lock+"\n"+src, []string{"squash: misc"})
	for _, part := range []string{"package-lock.json", "src/fix.go", "+fixed the overlay"} {
		if !strings.Contains(prompt, part) {
			t.Errorf("buildPRDescriptionPrompt() missing %q in a multi-file diff over the cutoff", part)
		}
	}
	if !strings.Contains(prompt, "[truncated]") {
		t.Errorf("buildPRDescriptionPrompt() = %q, want a per-file truncation marker", prompt[len(prompt)-80:])
	}
}

func TestLoopCommitMessage(t *testing.T) {
	linearGoal := "Work on Linear issue ACME-114 in this workspace. Implement what the issue asks; do not expand scope.\n\n" +
		"Identifier: ACME-114\n" +
		"Title: PR-C: client-side acme.dev API prechecks and SVG validation\n\n" +
		"## Description\nCatch predictable API rejections inside Apollo before spending a doomed round-trip.\n" +
		strings.Repeat("more ticket prose. ", 80)

	cases := []struct {
		name         string
		done, goal   string
		files        []string
		wantContains []string
		wantNot      []string
	}{
		{
			name:         "done summary wins",
			done:         "Add holdings precheck before sell_token",
			goal:         "ACME-114 API prechecks",
			files:        []string{"apollo/src/api.ts"},
			wantContains: []string{"Add holdings precheck before sell_token"},
			wantNot:      []string{"api.ts", "ACME-114 API prechecks", "recur(", "iteration"},
		},
		{
			name:         "done is first line only",
			done:         "Wire SVG validation\n\nAlso touched three other files and restated the ticket.",
			goal:         linearGoal,
			wantContains: []string{"Wire SVG validation"},
			wantNot:      []string{"Also touched", "Description", "recur(", "iteration"},
		},
		{
			name:         "linear dump becomes title plus files",
			goal:         linearGoal,
			files:        []string{"apollo/src/api.ts", "apollo/src/svg.ts"},
			wantContains: []string{"ACME-114:", "api.ts", "svg.ts"},
			wantNot:      []string{"Work on Linear", "Description", "doomed round-trip", "recur(", "iteration"},
		},
		{
			name:         "short ticket plus files",
			goal:         "ACME-114 API prechecks",
			files:        []string{"src/a.ts", "src/b.ts", "src/c.ts", "src/d.ts"},
			wantContains: []string{"ACME-114 API prechecks (a.ts, b.ts, c.ts +1)"},
			wantNot:      []string{"recur(", "iteration"},
		},
		{
			name:         "long hint drops instead of chopping the headline",
			goal:         "The human already wrote this change. Review the current worktree, then fix it.",
			files:        []string{"ChatTab.tsx", "Composer.tsx", "mission-agent-chat.test.tsx", "mission-home.test.tsx"},
			wantContains: []string{"The human already wrote this change"},
			wantNot:      []string{"ChatTab.tsx"},
		},
		{
			name:         "plain short goal",
			goal:         "add dark mode",
			wantContains: []string{"add dark mode"},
			wantNot:      []string{"recur(", "iteration"},
		},
		{
			name:         "empty falls back",
			wantContains: []string{"changes"},
			wantNot:      []string{"recur(", "iteration"},
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := loopCommitMessage(tc.done, tc.goal, tc.files)
			if len(got) > 80 {
				t.Errorf("loopCommitMessage(%q) length = %d, want a scannable subject; got %q", tc.name, len(got), got)
			}
			if strings.Contains(got, "\n") {
				t.Errorf("loopCommitMessage(%q) = %q, want a single line", tc.name, got)
			}
			for _, part := range tc.wantContains {
				if !strings.Contains(got, part) {
					t.Errorf("loopCommitMessage(%q) = %q, want it to contain %q", tc.name, got, part)
				}
			}
			for _, part := range tc.wantNot {
				if strings.Contains(got, part) {
					t.Errorf("loopCommitMessage(%q) = %q, must not contain %q", tc.name, got, part)
				}
			}
		})
	}
}
