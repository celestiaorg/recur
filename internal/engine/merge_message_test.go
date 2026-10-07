package engine

import (
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
)

func TestLocalMergeCommitMessage(t *testing.T) {
	pr := domain.PR{
		Number: 42,
		Branch: "recur/pepper-stew",
		Title:  "Fix squash commit messages",
		Commits: []domain.Commit{
			{Message: "recur(Reviewer): nits"},
			{Message: "recur(Coder): use PR title for squash\n\nbody"},
			{Message: ""},
		},
		Comments: []domain.PRComment{
			{Kind: "description", BodyMD: "Prefer GitHub-style squash messages."},
		},
	}
	defSq := domain.SquashCommitTitleAndCommits
	defMg := domain.MergeCommitTitle

	t.Run("squash uses title and commit subjects", func(t *testing.T) {
		got := localMergeCommitMessage(pr, domain.MergeMethodSquash, defSq, defMg)
		want := "Fix squash commit messages\n\n" +
			"* recur(Coder): use PR title for squash\n" +
			"* recur(Reviewer): nits\n" +
			"\nRecur Local PR #42"
		if got != want {
			t.Errorf("localMergeCommitMessage(squash) = %q, want %q", got, want)
		}
		if strings.Contains(got, "pepper-stew") {
			t.Errorf("localMergeCommitMessage(squash) must not include food-name branch, got %q", got)
		}
	})

	t.Run("squash title only", func(t *testing.T) {
		got := localMergeCommitMessage(pr, domain.MergeMethodSquash, domain.SquashCommitTitle, defMg)
		want := "Fix squash commit messages\n\nRecur Local PR #42"
		if got != want {
			t.Errorf("localMergeCommitMessage(squash title) = %q, want %q", got, want)
		}
		if strings.Contains(got, "recur(Coder)") {
			t.Errorf("title-only squash must not list commits, got %q", got)
		}
	})

	t.Run("squash title and description", func(t *testing.T) {
		got := localMergeCommitMessage(pr, domain.MergeMethodSquash, domain.SquashCommitTitleAndDescription, defMg)
		want := "Fix squash commit messages\n\n" +
			"Prefer GitHub-style squash messages.\n" +
			"\nRecur Local PR #42"
		if got != want {
			t.Errorf("localMergeCommitMessage(squash description) = %q, want %q", got, want)
		}
		if strings.Contains(got, "recur(Coder)") {
			t.Errorf("description squash must not list commits, got %q", got)
		}
	})

	t.Run("merge commit includes title", func(t *testing.T) {
		got := localMergeCommitMessage(pr, domain.MergeMethodMerge, defSq, defMg)
		want := "Merge Local PR #42: Fix squash commit messages"
		if got != want {
			t.Errorf("localMergeCommitMessage(merge) = %q, want %q", got, want)
		}
	})

	t.Run("merge title and description", func(t *testing.T) {
		got := localMergeCommitMessage(pr, domain.MergeMethodMerge, defSq, domain.MergeCommitTitleAndDescription)
		want := "Merge Local PR #42: Fix squash commit messages\n\nPrefer GitHub-style squash messages."
		if got != want {
			t.Errorf("localMergeCommitMessage(merge description) = %q, want %q", got, want)
		}
	})

	t.Run("empty title falls back", func(t *testing.T) {
		empty := domain.PR{Number: 7, Branch: "recur/mystery"}
		got := localMergeCommitMessage(empty, domain.MergeMethodSquash, defSq, defMg)
		if !strings.HasPrefix(got, "Loop changes") {
			t.Errorf("localMergeCommitMessage(empty title) = %q, want prefix %q", got, "Loop changes")
		}
		if strings.Contains(got, "mystery") {
			t.Errorf("localMergeCommitMessage(empty title) must not use branch name, got %q", got)
		}
	})

	t.Run("multiline title collapses to subject", func(t *testing.T) {
		multi := domain.PR{Number: 1, Title: "Ship it\n\nmore detail"}
		got := localMergeCommitMessage(multi, domain.MergeMethodMerge, defSq, defMg)
		want := "Merge Local PR #1: Ship it"
		if got != want {
			t.Errorf("localMergeCommitMessage(multiline) = %q, want %q", got, want)
		}
	})

	t.Run("github squash subject includes number and omits local trailer", func(t *testing.T) {
		gh := pr
		gh.Source = domain.SourceGitHub
		got := formatPRMergeMessage(gh, domain.MergeMethodSquash, defSq, defMg)
		if got.Subject != "Fix squash commit messages (#42)" {
			t.Errorf("github squash subject = %q, want title (#N)", got.Subject)
		}
		if strings.Contains(got.Combined(), "Recur Local PR") {
			t.Errorf("github squash must not use Local PR trailer, got %q", got.Combined())
		}
		wantBody := "* recur(Coder): use PR title for squash\n* recur(Reviewer): nits"
		if got.Body != wantBody {
			t.Errorf("github squash body = %q, want oldest-first %q", got.Body, wantBody)
		}
		if got.OmitBody {
			t.Error("github squash with commits must pass --body")
		}
	})

	t.Run("github squash without commits omits body so GitHub can fill it", func(t *testing.T) {
		gh := domain.PR{Number: 9, Source: domain.SourceGitHub, Title: "Live title"}
		got := formatPRMergeMessage(gh, domain.MergeMethodSquash, defSq, defMg)
		if got.Subject != "Live title (#9)" {
			t.Errorf("subject = %q, want Live title (#9)", got.Subject)
		}
		if !got.OmitBody {
			t.Error("empty commit list must omit --body")
		}
	})

	t.Run("github empty title omits subject", func(t *testing.T) {
		gh := domain.PR{Number: 3, Source: domain.SourceGitHub}
		got := formatPRMergeMessage(gh, domain.MergeMethodSquash, defSq, defMg)
		if got.Subject != "" {
			t.Errorf("empty github title subject = %q, want empty so gh uses its default", got.Subject)
		}
	})

	t.Run("github merge title includes number", func(t *testing.T) {
		gh := pr
		gh.Source = domain.SourceGitHub
		got := formatPRMergeMessage(gh, domain.MergeMethodMerge, defSq, defMg)
		if got.Subject != "Fix squash commit messages (#42)" {
			t.Errorf("github merge subject = %q, want title (#N)", got.Subject)
		}
		if got.Body != "" {
			t.Errorf("github merge title-only body = %q, want empty", got.Body)
		}
	})

	t.Run("unknown method keeps merge-commit fail-safe", func(t *testing.T) {
		got := localMergeCommitMessage(pr, domain.MergeMethod("nope"), defSq, defMg)
		want := "Merge Local PR #42: Fix squash commit messages"
		if got != want {
			t.Errorf("localMergeCommitMessage(unknown) = %q, want %q", got, want)
		}
	})
}

func TestWorkspaceMergeCommitMessage(t *testing.T) {
	ws := domain.Workspace{Goal: "Ship dark mode"}
	commits := []domain.Commit{
		{Message: "wire settings toggle"},
		{Message: "add theme tokens"},
	}

	t.Run("squash default lists commits oldest-first", func(t *testing.T) {
		got := workspaceMergeCommitMessage(ws, "main", domain.MergeMethodSquash, commits, domain.SquashCommitTitleAndCommits)
		want := "Ship dark mode\n\n* add theme tokens\n* wire settings toggle"
		if got != want {
			t.Errorf("workspace squash = %q, want %q", got, want)
		}
	})

	t.Run("squash title only", func(t *testing.T) {
		got := workspaceMergeCommitMessage(ws, "main", domain.MergeMethodSquash, commits, domain.SquashCommitTitle)
		if got != "Ship dark mode" {
			t.Errorf("workspace squash title = %q, want %q", got, "Ship dark mode")
		}
	})

	t.Run("merge keeps Merge prefix", func(t *testing.T) {
		got := workspaceMergeCommitMessage(ws, "main", domain.MergeMethodMerge, commits, domain.SquashCommitTitleAndCommits)
		want := "Merge Ship dark mode into main"
		if got != want {
			t.Errorf("workspace merge = %q, want %q", got, want)
		}
	})
}

// TestSquashMergeCommitMessageOnDisk asserts a squash Local merge writes the PR
// title as the commit subject on the base branch (not the food-name branch).
func TestSquashMergeCommitMessageOnDisk(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	goal := "Informative squash history"
	id, err := eng.StartWorkspace(projID, goal, "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 10*time.Second, "loop to reach pr_open", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})
	pr, ok := st.PRByWorkspace(id)
	if !ok {
		t.Fatal("no PR created")
	}

	if _, mok := eng.MergePR(pr.ID, domain.MergeMethodSquash); !mok {
		t.Fatal("MergePR(squash) should succeed")
	}

	subj, err := gitOut(repo, repo, "log", "-1", "--pretty=%s", "main")
	if err != nil {
		t.Fatalf("git log subject: %v", err)
	}
	subj = strings.TrimSpace(subj)
	if subj != goal {
		t.Errorf("squash commit subject = %q, want %q", subj, goal)
	}
	if strings.Contains(subj, "recur/") || strings.Contains(subj, "Merge ") {
		t.Errorf("squash commit subject still looks like a merge/branch message: %q", subj)
	}

	body, err := gitOut(repo, repo, "log", "-1", "--pretty=%b", "main")
	if err != nil {
		t.Fatalf("git log body: %v", err)
	}
	if !strings.Contains(body, "Recur Local PR #") {
		t.Errorf("squash commit body missing Local PR trailer, got %q", body)
	}
}
