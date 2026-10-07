package engine

import (
	"strings"
	"testing"

	"recur/internal/domain"
)

func TestPeelChatSurface(t *testing.T) {
	wire := `⟦recur-surface⟧{"surface":"github_pr","prId":"gh-1","prNumber":9}⟦/recur-surface⟧
please review`
	rest, surf, ok := peelChatSurface(wire)
	if !ok {
		t.Fatal("peelChatSurface(github marker) ok=false, want true")
	}
	if rest != "please review" {
		t.Errorf("peelChatSurface rest = %q, want %q", rest, "please review")
	}
	if surf.Surface != ChatSurfaceGitHubPR || surf.PRNumber != 9 || surf.PRID != "gh-1" {
		t.Errorf("peelChatSurface surf = %+v, want github_pr #9 gh-1", surf)
	}

	plain := "hello"
	rest, surf, ok = peelChatSurface(plain)
	if ok || rest != plain || surf.Surface != "" {
		t.Errorf("peelChatSurface(%q) = rest %q ok=%v surf=%+v, want unchanged", plain, rest, ok, surf)
	}
}

func TestSubjectAppendPromptGitHubHost(t *testing.T) {
	got := subjectAppendPrompt(chatSurface{
		Surface: ChatSurfaceGitHubPR, PRNumber: 9, PRURL: "https://github.com/acme/r/pull/9",
		PRTitle: "Imported", PRBranch: "feat", PRBase: "main", Host: true,
	})
	for _, want := range []string{
		"GitHub pull request #9 (https://github.com/acme/r/pull/9)",
		`titled "Imported"`,
		"gh pr diff",
		"working copy",
	} {
		if !strings.Contains(got, want) {
			t.Errorf("githubPRSubjectPrompt missing %q, got %q", want, got)
		}
	}
	if strings.Contains(got, "workspace for that PR") {
		t.Errorf("host prompt should not claim a Recur worktree, got %q", got)
	}
}

func TestSubjectAppendPromptWorkspaceMentionsLinkedPR(t *testing.T) {
	got := subjectAppendPrompt(chatSurface{
		Surface: ChatSurfaceWorkspace, PRNumber: 9, PRSource: string(domain.SourceGitHub),
		PRURL: "https://github.com/acme/r/pull/9",
	})
	if !strings.Contains(got, "this Recur workspace") {
		t.Errorf("workspaceSubjectPrompt missing workspace looking-at, got %q", got)
	}
	if !strings.Contains(got, "different surface") {
		t.Errorf("workspaceSubjectPrompt missing linked-PR caveat, got %q", got)
	}
}

func TestSubjectAppendPromptLocalPR(t *testing.T) {
	got := subjectAppendPrompt(chatSurface{
		Surface: ChatSurfaceLocalPR, PRNumber: 7, PRTitle: "WIP",
	})
	if !strings.Contains(got, "Local PR #7") || !strings.Contains(got, "not a GitHub") {
		t.Errorf("localPRSubjectPrompt = %q, want Local PR #7 and not-GitHub", got)
	}
}
