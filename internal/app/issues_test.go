package app

import (
	"testing"

	"recur/internal/domain"
)

func TestLinkIssueProjects(t *testing.T) {
	projects := []domain.Project{
		{ID: "houston", LinearProjects: "Houston"},
		{ID: "botfun", LinearProjects: " BOTFUN , bot.fun"},
	}
	repos := map[string]string{"botfun/eden": "botfun"}
	issues := []AssignedIssue{
		{Source: "github", Project: "BotFun/Eden"},
		{Source: "github", Project: "other/repo"},
		{Source: "linear", LinearProject: "houston", LinearTeam: "BOTFUN"},
		{Source: "linear", LinearProject: "Unmapped", LinearTeam: "BOTFUN"},
		{Source: "linear", LinearTeam: "ENG"},
	}
	linkIssueProjects(issues, projects, repos)
	want := []string{"botfun", "", "houston", "botfun", ""}
	for i, w := range want {
		if issues[i].ProjectID != w {
			t.Errorf("issue %d: ProjectID = %q, want %q", i, issues[i].ProjectID, w)
		}
	}
}

func TestLinearAppURL(t *testing.T) {
	got, ok := linearAppURL("https://linear.app/botfun/issue/BOTFUN-152/sold-coins?x=1")
	if !ok || got != "linear://botfun/issue/BOTFUN-152/sold-coins?x=1" {
		t.Errorf("got %q ok=%v", got, ok)
	}
	for _, bad := range []string{"https://github.com/a/b/issues/1", "http://linear.app/x", "linear://x", ""} {
		if _, ok := linearAppURL(bad); ok {
			t.Errorf("linearAppURL(%q) ok, want false", bad)
		}
	}
}
