package engine

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"recur/internal/domain"
	"recur/internal/store"
)

// TestGHAvailabilityDetection asserts the gh-availability gate is conservative: it
// is FALSE when gh is absent. This runs by default (no gh calls when gh is missing)
// and proves the GitHub path degrades gracefully (M6 brief F).
func TestGHAvailabilityDetection(t *testing.T) {
	if _, err := exec.LookPath("gh"); err == nil {
		t.Skip("gh is installed; availability depends on auth — covered by the gated test")
	}
	resetGHAvailCache()
	if ghCLIAvailable() {
		t.Error("ghCLIAvailable must be false when gh is not installed")
	}
}

func TestGHCLIAvailableCachesProbe(t *testing.T) {
	orig := probeGHCLIFn
	t.Cleanup(func() {
		probeGHCLIFn = orig
		resetGHAvailCache()
	})
	n := 0
	probeGHCLIFn = func() bool {
		n++
		return true
	}
	resetGHAvailCache()
	if !ghCLIAvailable() || !ghCLIAvailable() {
		t.Fatal("ghCLIAvailable() = false, want cached true")
	}
	if n != 1 {
		t.Errorf("probes = %d, want 1 (second call should hit the cache)", n)
	}
}

// TestGHMergeArgsUsesNumber asserts GitHub merge is keyed by PR number, not the
// (possibly renamed) local branch name.
func TestGHCreatePRArgsDraft(t *testing.T) {
	got := ghCreatePRArgs("feat", "main", "Title", "Body", false)
	for _, flag := range got {
		if flag == "--draft" {
			t.Errorf("ghCreatePRArgs(draft=false) = %v, must not include --draft", got)
		}
	}
	got = ghCreatePRArgs("feat", "main", "Title", "Body", true)
	found := false
	for _, flag := range got {
		if flag == "--draft" {
			found = true
		}
	}
	if !found {
		t.Errorf("ghCreatePRArgs(draft=true) = %v, want --draft", got)
	}
}

func TestGithubPRBaseStripsRemoteTracking(t *testing.T) {
	clone := initCloneTrackingRemoteBranch(t, "main", "main")
	ctx := context.Background()
	if got := githubPRBase(ctx, clone, "origin/main"); got != "main" {
		t.Fatalf("githubPRBase(origin/main) = %q, want main", got)
	}
	if got := githubPRBase(ctx, clone, "main"); got != "main" {
		t.Fatalf("githubPRBase(main) = %q, want main", got)
	}
	if got := githubPRBase(ctx, clone, "  origin/main  "); got != "main" {
		t.Fatalf("githubPRBase(padded origin/main) = %q, want main", got)
	}
}

func TestGithubPRBaseSlashyTrackingRef(t *testing.T) {
	clone := initCloneTrackingRemoteBranch(t, "origin/release", "origin/release")
	ctx := context.Background()
	if got := githubPRBase(ctx, clone, "origin/origin/release"); got != "origin/release" {
		t.Fatalf("githubPRBase(origin/origin/release) = %q, want origin/release", got)
	}
}

func TestGHMergeArgsUsesNumber(t *testing.T) {
	got := ghMergeArgs(482, "--squash", mergeCommitText{})
	want := []string{"pr", "merge", "482", "--squash"}
	if len(got) != len(want) {
		t.Fatalf("ghMergeArgs len = %d, want %d (%v)", len(got), len(want), got)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("ghMergeArgs = %v, want %v", got, want)
		}
	}
}

func TestGHMergeRESTArgs(t *testing.T) {
	got := ghMergeRESTArgs("acme/app", 12, "squash")
	want := []string{"api", "-X", "PUT", "repos/acme/app/pulls/12/merge", "-f", "merge_method=squash"}
	if len(got) != len(want) {
		t.Fatalf("ghMergeRESTArgs len = %d, want %d (%v)", len(got), len(want), got)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("ghMergeRESTArgs = %v, want %v", got, want)
		}
	}
}

func TestPlanGitHubMergePrefersRESTFromURL(t *testing.T) {
	got := planGitHubMerge(domain.PR{
		Number: 12, URL: "https://github.com/acme/app/pull/12", NodeID: "PR_1",
	}, domain.MergeMethodSquash)
	if got.kind != githubMergeREST {
		t.Fatalf("kind = %q, want rest", got.kind)
	}
	want := ghMergeRESTArgs("acme/app", 12, "squash")
	if len(got.restArgs) != len(want) {
		t.Fatalf("restArgs = %v, want %v", got.restArgs, want)
	}
	for i := range want {
		if got.restArgs[i] != want[i] {
			t.Fatalf("restArgs = %v, want %v", got.restArgs, want)
		}
	}
}

func TestPlanGitHubMergeGraphQLWhenNoURL(t *testing.T) {
	got := planGitHubMerge(domain.PR{Number: 12, NodeID: "PR_kwDO"}, domain.MergeMethodMerge)
	if got.kind != githubMergeGraphQL || got.nodeID != "PR_kwDO" {
		t.Fatalf("plan = %+v, want graphql PR_kwDO", got)
	}
	if githubGraphQLMergeMethod(domain.MergeMethodSquash) != "SQUASH" ||
		githubGraphQLMergeMethod(domain.MergeMethodRebase) != "REBASE" ||
		githubGraphQLMergeMethod(domain.MergeMethodMerge) != "MERGE" {
		t.Fatalf("graphql merge methods: squash=%s rebase=%s merge=%s",
			githubGraphQLMergeMethod(domain.MergeMethodSquash),
			githubGraphQLMergeMethod(domain.MergeMethodRebase),
			githubGraphQLMergeMethod(domain.MergeMethodMerge))
	}
}

func TestPlanGitHubMergeFallsBackToCLI(t *testing.T) {
	got := planGitHubMerge(domain.PR{Number: 482}, domain.MergeMethodRebase)
	if got.kind != githubMergeCLI {
		t.Fatalf("kind = %q, want cli", got.kind)
	}
	want := []string{"pr", "merge", "482", "--rebase"}
	if len(got.cliArgs) != len(want) {
		t.Fatalf("cliArgs = %v, want %v", got.cliArgs, want)
	}
	for i := range want {
		if got.cliArgs[i] != want[i] {
			t.Fatalf("cliArgs = %v, want %v", got.cliArgs, want)
		}
	}
}

func TestParseGitHubMergeFields(t *testing.T) {
	raw := `{
		"title": "Live PR title",
		"body": "The real description.",
		"commits": [
			{"oid": "aaa1111", "messageHeadline": "oldest commit", "messageBody": "detail"},
			{"oid": "bbb2222", "messageHeadline": "newest commit"}
		]
	}`
	got, err := parseGitHubMergeFields(raw)
	if err != nil {
		t.Fatalf("parseGitHubMergeFields: %v", err)
	}
	if got.Title != "Live PR title" || got.Body != "The real description." {
		t.Errorf("title/body = %q / %q", got.Title, got.Body)
	}
	if len(got.Commits) != 2 {
		t.Fatalf("commits len = %d, want 2", len(got.Commits))
	}
	if got.Commits[0].Message != "newest commit" || got.Commits[1].Message != "oldest commit" {
		t.Errorf("commits newest-first = %q then %q", got.Commits[0].Message, got.Commits[1].Message)
	}

	overlay := overlayGitHubMergeFields(domain.PR{
		Title:    "stale",
		Commits:  []domain.Commit{{Message: "stale commit"}},
		Comments: []domain.PRComment{{Kind: "description", BodyMD: "stale body"}},
	}, got)
	if overlay.Title != "Live PR title" {
		t.Errorf("overlay title = %q, want live", overlay.Title)
	}
	if prDescription(overlay) != "The real description." {
		t.Errorf("overlay description = %q, want live body", prDescription(overlay))
	}
	if len(overlay.Commits) != 2 || overlay.Commits[0].Message != "newest commit" {
		t.Errorf("overlay commits = %+v, want live newest-first", overlay.Commits)
	}
}

func TestWithGitHubMergeFieldsFetchErrorOmitsFlags(t *testing.T) {
	prev := fetchGitHubMergeFieldsFn
	t.Cleanup(func() { fetchGitHubMergeFieldsFn = prev })
	fetchGitHubMergeFieldsFn = func(context.Context, string, int) (githubMergeFields, error) {
		return githubMergeFields{}, fmt.Errorf("network blip")
	}

	orig := domain.PR{
		Number:   12,
		Source:   domain.SourceGitHub,
		Title:    "stale title",
		Commits:  []domain.Commit{{Message: "stale commit"}},
		Comments: []domain.PRComment{{Kind: "description", BodyMD: "stale body"}},
	}
	got, err := withGitHubMergeFields(context.Background(), "", orig)
	if err == nil {
		t.Fatal("withGitHubMergeFields error = nil, want fetch failure")
	}
	if orig.Title != "stale title" || len(orig.Commits) != 1 {
		t.Errorf("input PR mutated: title=%q commits=%d", orig.Title, len(orig.Commits))
	}
	msg := formatPRMergeMessage(got, domain.MergeMethodSquash, domain.SquashCommitTitleAndCommits, domain.MergeCommitTitle)
	args := ghMergeArgs(orig.Number, "--squash", msg)
	want := []string{"pr", "merge", "12", "--squash"}
	if len(args) != len(want) {
		t.Fatalf("ghMergeArgs after fetch error = %v, want %v", args, want)
	}
	for i := range want {
		if args[i] != want[i] {
			t.Fatalf("ghMergeArgs after fetch error = %v, want %v", args, want)
		}
	}
}

func TestWithGitHubMergeFieldsUsesLiveTitle(t *testing.T) {
	prev := fetchGitHubMergeFieldsFn
	t.Cleanup(func() { fetchGitHubMergeFieldsFn = prev })
	fetchGitHubMergeFieldsFn = func(context.Context, string, int) (githubMergeFields, error) {
		return githubMergeFields{Title: "Live title", Body: "Live body"}, nil
	}
	orig := domain.PR{
		Number:   12,
		Source:   domain.SourceGitHub,
		Title:    "stale",
		Comments: []domain.PRComment{{Kind: "description", BodyMD: "stale body"}},
	}
	got, err := withGitHubMergeFields(context.Background(), "", orig)
	if err != nil {
		t.Fatalf("withGitHubMergeFields: %v", err)
	}
	if orig.Title != "stale" || orig.Comments[0].BodyMD != "stale body" {
		t.Errorf("input PR mutated: title=%q body=%q", orig.Title, orig.Comments[0].BodyMD)
	}
	msg := formatPRMergeMessage(got, domain.MergeMethodMerge, domain.SquashCommitTitleAndCommits, domain.MergeCommitTitle)
	if msg.Subject != "Live title (#12)" {
		t.Errorf("merge subject = %q, want Live title (#12)", msg.Subject)
	}
	if prDescription(got) != "Live body" {
		t.Errorf("overlay description = %q, want Live body", prDescription(got))
	}
}

func TestGHMergeArgsPassesSubjectAndBody(t *testing.T) {
	got := ghMergeArgs(482, "--squash", mergeCommitText{Subject: "Fix foo (#482)", Body: "* commit a"})
	want := []string{"pr", "merge", "482", "--squash", "--subject", "Fix foo (#482)", "--body", "* commit a"}
	if len(got) != len(want) {
		t.Fatalf("ghMergeArgs len = %d, want %d (%v)", len(got), len(want), got)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("ghMergeArgs = %v, want %v", got, want)
		}
	}

	got = ghMergeArgs(7, "--rebase", mergeCommitText{Subject: "ignored", Body: "ignored"})
	want = []string{"pr", "merge", "7", "--rebase"}
	if len(got) != len(want) {
		t.Fatalf("ghMergeArgs(rebase) = %v, want %v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("ghMergeArgs(rebase) = %v, want %v", got, want)
		}
	}

	got = ghMergeArgs(11, "--squash", mergeCommitText{Subject: "Only title (#11)", OmitBody: true})
	want = []string{"pr", "merge", "11", "--squash", "--subject", "Only title (#11)"}
	if len(got) != len(want) {
		t.Fatalf("ghMergeArgs(omit body) = %v, want %v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("ghMergeArgs(omit body) = %v, want %v", got, want)
		}
	}
}

func TestMergeGitHubMarksMergedWithoutAuthProbe(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "gh-merge-fast", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-merge-fast", WorkspaceID: ws.ID, Number: 88, Source: domain.SourceGitHub,
		Title: "fast", State: "In review", Group: "you",
		URL: "https://github.com/acme/app/pull/88",
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() { getGitHubPRSnapshot = prevSnap })
	snapped := make(chan struct{}, 1)
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		select {
		case snapped <- struct{}{}:
		default:
		}
		return ghPRSnapshot{State: "MERGED", Title: "fast", URL: "https://github.com/acme/app/pull/88"}, nil
	}

	prev := mergeGitHubPR
	t.Cleanup(func() { mergeGitHubPR = prev })
	var gotMethod domain.MergeMethod
	var called bool
	mergeGitHubPR = func(_ context.Context, _ string, p domain.PR, method domain.MergeMethod) error {
		called = true
		gotMethod = method
		if p.ID != pr.ID {
			t.Errorf("merge PR id = %q, want %q", p.ID, pr.ID)
		}
		return nil
	}

	merged, ok := eng.MergePR(pr.ID, domain.MergeMethodSquash)
	if !ok || !called {
		t.Fatalf("MergePR ok=%v called=%v", ok, called)
	}
	if gotMethod != domain.MergeMethodSquash {
		t.Errorf("method = %q, want squash", gotMethod)
	}
	if merged.State != "Merged" || merged.Group != "merged" {
		t.Errorf("state=%q group=%q, want Merged merged", merged.State, merged.Group)
	}
	select {
	case <-snapped:
	case <-time.After(2 * time.Second):
		t.Error("ScheduleRefreshPRConversation was not started after merge")
	}
}

func TestGitHubMergeFailureNote(t *testing.T) {
	cases := []struct {
		err     string
		want    string
		notWant string
	}{
		{
			err:  "gh api -X PUT repos/acme/app/pulls/88/merge: gh: Bad credentials (HTTP 401)",
			want: "not authenticated",
		},
		{
			err:     "gh api graphql mergePullRequest: HTTP 403: Resource not accessible by personal access token",
			want:    "GitHub merge failed",
			notWant: "not authenticated",
		},
		{
			err:     "gh: API rate limit exceeded for user ID 1234. (HTTP 403)",
			want:    "rate limit exceeded",
			notWant: "not authenticated",
		},
		{
			err:     "Resource protected by organization SAML enforcement. You must grant your OAuth token access to this organization. (HTTP 403)",
			want:    "SAML enforcement",
			notWant: "not authenticated",
		},
		{
			err:  "the gh CLI is not installed",
			want: "not installed",
		},
		{
			err:  "To get started with GitHub CLI, please run:  gh auth login\nAlternatively, populate the GH_TOKEN environment variable",
			want: "not authenticated",
		},
		{
			err:  "Pull Request is not mergeable",
			want: "GitHub merge failed: Pull Request is not mergeable",
		},
	}
	for _, tt := range cases {
		got := githubMergeFailureNote(fmt.Errorf("%s", tt.err))
		if !strings.Contains(got, tt.want) {
			t.Errorf("githubMergeFailureNote(%q) = %q, want substring %q", tt.err, got, tt.want)
		}
		if tt.notWant != "" && strings.Contains(got, tt.notWant) {
			t.Errorf("githubMergeFailureNote(%q) = %q, must not contain %q", tt.err, got, tt.notWant)
		}
	}
	auth := githubMergeFailureNote(fmt.Errorf("gh: Bad credentials (HTTP 401)"))
	if !strings.Contains(auth, "Bad credentials") {
		t.Errorf("auth failure note dropped the gh detail: %q", auth)
	}
}

func TestRetryGitHubRESTMergeRetriesMergeability(t *testing.T) {
	prev := githubMergeRetryDelay
	githubMergeRetryDelay = 0
	t.Cleanup(func() { githubMergeRetryDelay = prev })
	var n int
	err := retryGitHubRESTMerge(context.Background(), func() error {
		n++
		if n < 3 {
			return fmt.Errorf("gh api PUT: gh: Pull Request is not mergeable (HTTP 405)")
		}
		return nil
	})
	if err != nil || n != 3 {
		t.Fatalf("retry n=%d err=%v, want 3 nil", n, err)
	}
}

func TestRetryGitHubRESTMergeDoesNotRetryOtherErrors(t *testing.T) {
	var n int
	err := retryGitHubRESTMerge(context.Background(), func() error {
		n++
		return fmt.Errorf("gh api PUT: required status checks have not passed")
	})
	if err == nil || n != 1 {
		t.Fatalf("retry n=%d err=%v, want 1 error", n, err)
	}
}

// TestPRNumberFromURL unit-tests the gh PR-URL parser (no gh required).
func TestPRNumberFromURL(t *testing.T) {
	cases := map[string]int{
		"https://github.com/owner/repo/pull/482": 482,
		"https://github.com/owner/repo/pull/1":   1,
		"https://github.com/owner/repo/pull/":    0,
		"not a url":                              0,
		"https://github.com/owner/repo/pull/abc": 0,
	}
	for in, want := range cases {
		if got := prNumberFromURL(in); got != want {
			t.Errorf("prNumberFromURL(%q) = %d, want %d", in, got, want)
		}
	}
}

func TestGitHubCreateNumberPrefersURLThenHead(t *testing.T) {
	if got := githubCreateNumber("https://github.com/owner/repo/pull/482", 99); got != 482 {
		t.Errorf("githubCreateNumber(url, 99) = %d, want 482", got)
	}
	if got := githubCreateNumber("not a url", 39); got != 39 {
		t.Errorf("githubCreateNumber(no url, 39) = %d, want 39 (existing head PR)", got)
	}
	if got := githubCreateNumber("not a url", 0); got != 0 {
		t.Errorf("githubCreateNumber(no url, 0) = %d, want 0", got)
	}
}

// TestGHFailReason asserts a swallowed git-push error is preserved in the composed
// failure reason (the bug this phase fixes) alongside the `gh pr create` error, and
// that a nil push error keeps the message to just the create failure.
func TestGHFailReason(t *testing.T) {
	createErr := fmt.Errorf("gh pr create: some create failure")

	if got := ghFailReason(nil, createErr); got != "gh pr create failed: some create failure" {
		t.Errorf("ghFailReason(nil, %q) = %q, want gh pr create failed: some create failure", createErr, got)
	}

	pushErr := fmt.Errorf("git push: permission denied")
	got := ghFailReason(pushErr, createErr)
	if !strings.Contains(got, "permission denied") {
		t.Errorf("ghFailReason(%q, %q) = %q, want the push error output", pushErr, createErr, got)
	}
	if !strings.Contains(got, "some create failure") {
		t.Errorf("ghFailReason(%q, %q) = %q, want the create error output", pushErr, createErr, got)
	}
}

func TestGithubPRHeadLookupArgsListsOpenHeadBase(t *testing.T) {
	got := githubPRHeadLookupArgs("feat", "main")
	joined := strings.Join(got, " ")
	if len(got) < 2 || got[0] != "pr" || got[1] != "list" {
		t.Errorf("githubPRHeadLookupArgs(feat, main) = %v, want pr list …", got)
	}
	for _, flag := range []string{"--head", "feat", "--base", "main", "--state", "open"} {
		if !strings.Contains(joined, flag) {
			t.Errorf("githubPRHeadLookupArgs(feat, main) = %v, want %s", got, flag)
		}
	}
	if strings.Contains(joined, "pr view") {
		t.Errorf("githubPRHeadLookupArgs(feat, main) = %v, must not use pr view", got)
	}
}

func TestGithubListFirstNumber(t *testing.T) {
	if got := githubListFirstNumber(`[{"number":42}]`); got != 42 {
		t.Errorf("githubListFirstNumber([{\"number\":42}]) = %d, want 42", got)
	}
	if got := githubListFirstNumber(`[]`); got != 0 {
		t.Errorf("githubListFirstNumber([]) = %d, want 0", got)
	}
	if got := githubListFirstNumber("not json"); got != 0 {
		t.Errorf("githubListFirstNumber(not json) = %d, want 0", got)
	}
}

func TestGithubPRNumberForHeadUsesListNotViewHead(t *testing.T) {
	dir := installFakeGH(t, `#!/bin/sh
printf '%s\n' "$*" >> "$0.args"
if [ "$1" = "pr" ] && [ "$2" = "view" ]; then
  echo "unknown flag: --head" >&2
  exit 1
fi
if [ "$1" = "pr" ] && [ "$2" = "list" ]; then
  echo '[{"number":42}]'
  exit 0
fi
exit 1
`)
	got := githubPRNumberForHead(context.Background(), "", "feat", "main")
	if got != 42 {
		t.Errorf("githubPRNumberForHead(_, _, feat, main) = %d, want 42", got)
	}
	logged, err := os.ReadFile(filepath.Join(dir, "gh.args"))
	if err != nil {
		t.Fatalf("read fake gh args: %v", err)
	}
	gotArgs := string(logged)
	if strings.Contains(gotArgs, "pr view") {
		t.Errorf("githubPRNumberForHead invoked %q, must not call gh pr view", gotArgs)
	}
	if !strings.Contains(gotArgs, "pr list") || !strings.Contains(gotArgs, "--head feat") ||
		!strings.Contains(gotArgs, "--base main") || !strings.Contains(gotArgs, "--state open") {
		t.Errorf("githubPRNumberForHead invoked %q, want pr list --head feat --base main --state open", gotArgs)
	}
}

func TestCreateGitHubPRSurfacesPushFailureWhenPRExists(t *testing.T) {
	repo := initRepo(t)
	installFakeGH(t, `#!/bin/sh
if [ "$1" = "pr" ] && [ "$2" = "create" ]; then
  echo "a pull request already exists: https://github.com/owner/repo/pull/99"
  exit 1
fi
exit 1
`)
	orig := probeGHCLIFn
	t.Cleanup(func() {
		probeGHCLIFn = orig
		resetGHAvailCache()
	})
	probeGHCLIFn = func() bool { return true }
	resetGHAvailCache()

	n, ok, reason := (&Engine{}).CreateGitHubPR(context.Background(), repo, repo, "feat", "main", "Title", "Body", false)
	if ok || n != 0 {
		t.Errorf("CreateGitHubPR(push fail, existing PR URL) = %d, ok=%v, reason=%q; want 0, false (do not hide the push failure)", n, ok, reason)
	}
	if !strings.Contains(reason, "git push failed") {
		t.Errorf("CreateGitHubPR(...) reason = %q, want git push failed", reason)
	}
}

func TestCreateGitHubPRCapsTitleAt256Runes(t *testing.T) {
	repo := initRepo(t)
	dir := installFakeGH(t, `#!/bin/sh
printf '%s\0' "$@" >> "$0.args"
if [ "$1" = "pr" ] && [ "$2" = "create" ]; then
  echo "https://github.com/owner/repo/pull/1"
  exit 0
fi
exit 1
`)
	orig := probeGHCLIFn
	t.Cleanup(func() {
		probeGHCLIFn = orig
		resetGHAvailCache()
	})
	probeGHCLIFn = func() bool { return true }
	resetGHAvailCache()

	// Newline sits inside the 256-rune window so truncation alone cannot hide it.
	title := strings.Repeat("x", 200) + "\n" + strings.Repeat("y", 200)
	(&Engine{}).CreateGitHubPR(context.Background(), repo, repo, "feat", "main", title, "Body", false)

	logged, err := os.ReadFile(filepath.Join(dir, "gh.args"))
	if err != nil {
		t.Fatalf("read fake gh args: %v", err)
	}
	gotTitles := fakeGHFlagValues(string(logged), "--title")
	if len(gotTitles) != 1 {
		t.Fatalf("CreateGitHubPR(--title 200-rune/newline) logged %d --title values, want 1; args=%q", len(gotTitles), logged)
	}
	got := gotTitles[0]
	if strings.Contains(got, "\n") {
		t.Errorf("CreateGitHubPR(--title %q) logged %q, want one line", title, got)
	}
	if n := utf8.RuneCountInString(got); n > githubPRTitleMax {
		t.Errorf("CreateGitHubPR(--title) rune count = %d, want <= %d", n, githubPRTitleMax)
	}
	want := strings.Repeat("x", 200)
	if got != want {
		t.Errorf("CreateGitHubPR(--title) = %q, want %q", got, want)
	}
}

func TestCreateGitHubPRRetriesOnceOnTitleTooLong(t *testing.T) {
	repo := initRepo(t)
	addBareOrigin(t, repo)
	dir := installFakeGH(t, `#!/bin/sh
if [ "$1" = "pr" ] && [ "$2" = "create" ]; then
  n=0
  [ -f "$0.count" ] && n=$(cat "$0.count")
  n=$((n+1))
  printf '%s' "$n" > "$0.count"
  printf '%s\0' "$@" >> "$0.args"
  if [ "$n" = "1" ]; then
    echo "Title is too long (maximum is 256 characters) (createPullRequest)" >&2
    exit 1
  fi
  echo "https://github.com/owner/repo/pull/7"
  exit 0
fi
exit 1
`)
	orig := probeGHCLIFn
	t.Cleanup(func() {
		probeGHCLIFn = orig
		resetGHAvailCache()
	})
	probeGHCLIFn = func() bool { return true }
	resetGHAvailCache()

	title := strings.Repeat("z", 400)
	n, ok, reason := (&Engine{}).CreateGitHubPR(context.Background(), repo, repo, "feat", "main", title, "Body", false)
	if !ok || n != 7 {
		t.Errorf("CreateGitHubPR(title-too-long retry) = %d, ok=%v, reason=%q; want 7, true", n, ok, reason)
	}

	countB, err := os.ReadFile(filepath.Join(dir, "gh.count"))
	if err != nil {
		t.Fatalf("read fake gh count: %v", err)
	}
	if got := string(countB); got != "2" {
		t.Errorf("CreateGitHubPR(title-too-long retry) pr create calls = %s, want 2", got)
	}
	logged, err := os.ReadFile(filepath.Join(dir, "gh.args"))
	if err != nil {
		t.Fatalf("read fake gh args: %v", err)
	}
	gotTitles := fakeGHFlagValues(string(logged), "--title")
	if len(gotTitles) != 2 {
		t.Fatalf("CreateGitHubPR(title-too-long retry) logged %d --title values, want 2; args=%q", len(gotTitles), logged)
	}
	if n := utf8.RuneCountInString(gotTitles[1]); n > 80 {
		t.Errorf("CreateGitHubPR retry --title rune count = %d, want <= 80 (title=%q)", n, gotTitles[1])
	}
}

func TestCreateGitHubPRDoesNotRetryNonTitleError(t *testing.T) {
	repo := initRepo(t)
	dir := installFakeGH(t, `#!/bin/sh
if [ "$1" = "pr" ] && [ "$2" = "create" ]; then
  n=0
  [ -f "$0.count" ] && n=$(cat "$0.count")
  n=$((n+1))
  printf '%s' "$n" > "$0.count"
  echo "HTTP 401" >&2
  exit 1
fi
exit 1
`)
	orig := probeGHCLIFn
	t.Cleanup(func() {
		probeGHCLIFn = orig
		resetGHAvailCache()
	})
	probeGHCLIFn = func() bool { return true }
	resetGHAvailCache()

	n, ok, reason := (&Engine{}).CreateGitHubPR(context.Background(), repo, repo, "feat", "main",
		"Title is too long", "Body mentions Title is too long (maximum is 256 characters)", false)
	if ok || n != 0 {
		t.Errorf("CreateGitHubPR(HTTP 401) = %d, ok=%v, reason=%q; want 0, false", n, ok, reason)
	}
	countB, err := os.ReadFile(filepath.Join(dir, "gh.count"))
	if err != nil {
		t.Fatalf("read fake gh count: %v", err)
	}
	if got := string(countB); got != "1" {
		t.Errorf("CreateGitHubPR(HTTP 401) pr create calls = %s, want 1", got)
	}
}

func addBareOrigin(t *testing.T, repo string) {
	t.Helper()
	bare := filepath.Join(t.TempDir(), "origin.git")
	cmd := exec.Command("git", "init", "--bare", bare)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git init --bare: %v\n%s", err, out)
	}
	cmd = exec.Command("git", "remote", "add", "origin", bare)
	cmd.Dir = repo
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git remote add origin: %v\n%s", err, out)
	}
}

func fakeGHFlagValues(logged, flag string) []string {
	var vals []string
	args := strings.Split(strings.TrimSuffix(logged, "\x00"), "\x00")
	for i, arg := range args {
		if arg == flag && i+1 < len(args) {
			vals = append(vals, args[i+1])
		}
	}
	return vals
}

func installFakeGH(t *testing.T, script string) string {
	t.Helper()
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "gh"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", dir+string(os.PathListSeparator)+os.Getenv("PATH"))
	return dir
}

// TestGitHubMergeGated is the REAL GitHub round-trip. It is SKIPPED by default and
// only runs with RECUR_GH=1 + gh installed & authenticated, because it has outward
// side effects: it creates a throwaway PRIVATE repo on the authed account (which it
// tries to delete afterward — without the delete_repo token scope the leftover must
// be deleted by hand). Zero claude cost (ScriptedRunner drives the loop).
//
//	throwaway remote repo → scripted loop on a PROnGitHub project → the engine
//	pushes the branch + opens a REAL GitHub PR (real number) → MergePR routes via
//	`gh pr merge` → the PR is MERGED on GitHub.
func TestGitHubMergeGated(t *testing.T) {
	if os.Getenv("RECUR_GH") != "1" {
		t.Skip("set RECUR_GH=1 (with gh installed + authenticated) to run the gh PR round-trip")
	}
	if !ghCLIAvailable() {
		t.Skip("gh not available/authenticated")
	}

	// Throwaway private repo seeded from a local main.
	repo := initRepo(t)
	name := fmt.Sprintf("recur-gh-e2e-%d", time.Now().Unix())
	ctx, cancel := context.WithTimeout(context.Background(), 120*time.Second)
	defer cancel()
	if out, err := runGH(ctx, repo, "repo", "create", name, "--private", "--source=.", "--remote=origin", "--push"); err != nil {
		t.Fatalf("gh repo create: %v\n%s", err, out)
	}
	ownerOut, _ := runGH(ctx, repo, "api", "user", "-q", ".login")
	full := strings.TrimSpace(ownerOut) + "/" + name
	t.Cleanup(func() {
		cctx, ccancel := context.WithTimeout(context.Background(), 60*time.Second)
		defer ccancel()
		if _, err := runGH(cctx, repo, "repo", "delete", full, "--yes"); err != nil {
			t.Logf("NOTE: could not delete throwaway repo %s (token lacks delete_repo?) — delete it by hand: gh repo delete %s --yes", full, full)
		}
	})

	// Scripted loop (no claude) on a GitHub-mode project.
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	p := domain.Project{ID: "ghproj", Name: "ghproj", RepoPath: repo, BaseBranch: "main", PROnGitHub: true}
	if _, err := st.AddProject(p); err != nil {
		t.Fatalf("AddProject: %v", err)
	}

	id, err := eng.StartWorkspace("ghproj", "Add a real GitHub PR feature", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 90*time.Second, "loop to reach pr_open", func() bool {
		l, ok := st.Workspace(id)
		return ok && (l.State == domain.StatePROpen || l.State == domain.StateFailed)
	})

	pr, ok := st.PRByWorkspace(id)
	if !ok {
		t.Fatal("no PR created")
	}
	if pr.Source != domain.SourceGitHub {
		t.Fatalf("PR source: got %q, want gh (feed: %+v)", pr.Source, st.Feed(id))
	}
	if pr.Number <= 0 {
		t.Fatalf("GitHub PR number not captured: %d", pr.Number)
	}
	if out, err := runGH(ctx, repo, "pr", "view", fmt.Sprintf("%d", pr.Number), "--json", "state", "-q", ".state"); err != nil || strings.TrimSpace(out) != "OPEN" {
		t.Fatalf("PR not OPEN on GitHub: %q err=%v", strings.TrimSpace(out), err)
	}

	// Merge routes via gh and lands on GitHub.
	merged, ok := eng.MergePR(pr.ID, "")
	if !ok || merged.State != "Merged" {
		t.Fatalf("MergePR failed: ok=%v state=%q", ok, merged.State)
	}
	if out, err := runGH(ctx, repo, "pr", "view", fmt.Sprintf("%d", pr.Number), "--json", "state", "-q", ".state"); err != nil || strings.TrimSpace(out) != "MERGED" {
		t.Fatalf("PR not MERGED on GitHub: %q err=%v", strings.TrimSpace(out), err)
	}
	t.Logf("GitHub round-trip OK: %s PR #%d created and merged", full, pr.Number)
}

func TestParseGitHubPRChecks(t *testing.T) {
	t.Parallel()
	tests := []struct {
		name    string
		raw     string
		want    []domain.Check
		wantOK  bool
		wantErr bool
	}{
		{
			name:   "empty means no checks reported",
			raw:    "",
			want:   nil,
			wantOK: true,
		},
		{
			name:   "no checks reported text",
			raw:    "no checks reported on the \"recur/foo\" branch",
			want:   nil,
			wantOK: true,
		},
		{
			name: "pass fail pending skipping",
			raw: `[
				{"name":"test","state":"SUCCESS","bucket":"pass","workflow":"CI","description":"142 passed","startedAt":"2026-08-21T12:00:00Z","completedAt":"2026-08-21T12:01:04Z","link":"https://github.com/acme/app/actions/runs/1"},
				{"name":"lint","state":"FAILURE","bucket":"fail","workflow":"CI","description":"1 error","startedAt":"2026-08-21T12:00:00Z","completedAt":"2026-08-21T12:00:12Z"},
				{"name":"build","state":"PENDING","bucket":"pending","workflow":"CI","description":"","startedAt":"2026-08-21T12:00:00Z"},
				{"name":"deploy","state":"SKIPPED","bucket":"skipping","workflow":"Release","description":""}
			]`,
			want: []domain.Check{
				{Name: "test", Detail: "1m 4s", Passed: true, Status: "pass", URL: "https://github.com/acme/app/actions/runs/1", StartedAt: "2026-08-21T12:00:00Z"},
				{Name: "lint", Detail: "12s", Passed: false, Status: "fail", StartedAt: "2026-08-21T12:00:00Z"},
				{Name: "build", Detail: "45s", Passed: false, Status: "pending", StartedAt: "2026-08-21T12:00:00Z"},
				{Name: "deploy", Detail: "skipped", Passed: false, Status: "skipping"},
			},
			wantOK: false,
		},
		{
			name: "skipped does not fail the rollup",
			raw: `[
				{"name":"test","state":"SUCCESS","bucket":"pass","workflow":"CI","description":"","startedAt":"2026-08-21T12:00:00Z","completedAt":"2026-08-21T12:00:08Z"},
				{"name":"deploy","state":"SKIPPED","bucket":"skipping","workflow":"Release","description":""}
			]`,
			want: []domain.Check{
				{Name: "test", Detail: "8s", Passed: true, Status: "pass", StartedAt: "2026-08-21T12:00:00Z"},
				{Name: "deploy", Detail: "skipped", Passed: false, Status: "skipping"},
			},
			wantOK: true,
		},
		{
			name: "state fallback when bucket missing",
			raw:  `[{"name":"ci","state":"IN_PROGRESS","workflow":"CI"}]`,
			want: []domain.Check{
				{Name: "ci", Detail: "pending", Passed: false, Status: "pending"},
			},
			wantOK: true,
		},
		{
			name: "expected required check is pending",
			raw:  `[{"name":"license","state":"EXPECTED"}]`,
			want: []domain.Check{
				{Name: "license", Detail: "pending", Passed: false, Status: "pending"},
			},
			wantOK: true,
		},
		{
			name: "stale completed conclusion is not pending",
			raw:  `[{"name":"ci","state":"STALE"}]`,
			want: []domain.Check{
				{Name: "ci", Detail: "cancelled", Passed: false, Status: "cancel"},
			},
			wantOK: false,
		},
		{
			name:    "non-json is an error",
			raw:     "GraphQL: Could not resolve to a PullRequest",
			wantErr: true,
		},
	}
	now := time.Date(2026, 8, 21, 12, 0, 45, 0, time.UTC)
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, ok, err := parseGitHubPRChecksAt(tt.raw, now)
			if tt.wantErr {
				if err == nil {
					t.Fatalf("parseGitHubPRChecks(%q) error = nil, want error", tt.name)
				}
				return
			}
			if err != nil {
				t.Fatalf("parseGitHubPRChecks(%q) error = %v, want nil", tt.name, err)
			}
			if ok != tt.wantOK {
				t.Errorf("parseGitHubPRChecks(%q) ok = %v, want %v", tt.name, ok, tt.wantOK)
			}
			if len(got) != len(tt.want) {
				t.Fatalf("parseGitHubPRChecks(%q) = %+v, want %+v", tt.name, got, tt.want)
			}
			for i := range tt.want {
				if got[i] != tt.want[i] {
					t.Errorf("parseGitHubPRChecks(%q)[%d] = %+v, want %+v", tt.name, i, got[i], tt.want[i])
				}
			}
		})
	}
}

func TestParseGitHubPRChecksGraphQLIncludesPending(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 21, 12, 0, 45, 0, time.UTC)
	raw := `{
		"data": {
			"repository": {
				"pullRequest": {
					"url": "https://github.com/acme/app/pull/11",
					"commits": {
						"nodes": [{
							"commit": {
								"statusCheckRollup": {
									"contexts": {
										"nodes": [
											{
												"__typename": "CheckRun",
												"name": "build",
												"status": "IN_PROGRESS",
												"detailsUrl": "https://github.com/acme/app/actions/runs/1/job/2",
												"permalink": "https://github.com/acme/app/runs/99",
												"startedAt": "2026-08-21T12:00:00Z",
												"checkSuite": {"workflowRun": {"workflow": {"name": "CI"}}}
											},
											{
												"__typename": "StatusContext",
												"context": "license/snyk",
												"state": "EXPECTED",
												"description": "Waiting for status to be reported"
											}
										]
									}
								},
								"checkSuites": {
									"nodes": [
										{
											"status": "QUEUED",
											"app": {"name": "GitHub Actions"},
											"workflowRun": {"url": "https://github.com/acme/app/actions/runs/2", "workflow": {"name": "Release"}},
											"checkRuns": {"totalCount": 0}
										},
										{
											"status": "IN_PROGRESS",
											"workflowRun": {"workflow": {"name": "CI"}},
											"checkRuns": {"totalCount": 1}
										}
									]
								}
							}
						}]
					}
				}
			}
		}
	}`
	got, ok, err := parseGitHubPRChecksGraphQLAt(raw, now)
	if err != nil {
		t.Fatalf("parseGitHubPRChecksGraphQL: %v", err)
	}
	if !ok {
		t.Error("ok = false, want true while checks are pending and none failed")
	}
	want := []domain.Check{
		{Name: "build", Detail: "45s", Passed: false, Status: "pending", URL: "https://github.com/acme/app/runs/99", StartedAt: "2026-08-21T12:00:00Z"},
		{Name: "license/snyk", Detail: "pending", Passed: false, Status: "pending", URL: "https://github.com/acme/app/pull/11/checks"},
		{Name: "Release", Detail: "pending", Passed: false, Status: "pending", URL: "https://github.com/acme/app/pull/11/checks"},
	}
	if len(got) != len(want) {
		t.Fatalf("got %+v, want %+v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("[%d] = %+v, want %+v", i, got[i], want[i])
		}
	}
}

func TestParseGitHubPRChecksGraphQLRequiredNotAppSuite(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 21, 12, 0, 45, 0, time.UTC)
	raw := `{
		"data": {
			"repository": {
				"defaultBranchRef": {"name": "main"},
				"rulesets": {
					"nodes": [{
						"enforcement": "ACTIVE",
						"target": "BRANCH",
						"conditions": {"refName": {"include": ["~DEFAULT_BRANCH"], "exclude": []}},
						"rules": {"nodes": [{
							"type": "REQUIRED_STATUS_CHECKS",
							"parameters": {"requiredStatusChecks": [{"context": "CI Success"}]}
						}]}
					}]
				},
				"pullRequest": {
					"url": "https://github.com/acme/apollo/pull/204",
					"baseRef": {"name": "main"},
					"commits": {
						"nodes": [{
							"commit": {
								"statusCheckRollup": null,
								"checkSuites": {
									"nodes": [{
										"status": "QUEUED",
										"app": {"name": "Depot Code Access"},
										"workflowRun": null,
										"checkRuns": {"totalCount": 0}
									}]
								}
							}
						}]
					}
				}
			}
		}
	}`
	got, ok, err := parseGitHubPRChecksGraphQLAt(raw, now)
	if err != nil {
		t.Fatalf("parseGitHubPRChecksGraphQL: %v", err)
	}
	if !ok {
		t.Error("ok = false, want true while required CI Success is pending and none failed")
	}
	if len(got) != 1 || got[0].Name != "CI Success" || got[0].Status != "pending" {
		t.Fatalf("got %+v, want pending CI Success (not Depot Code Access)", got)
	}
	if got[0].URL != "https://github.com/acme/apollo/pull/204/checks" {
		t.Errorf("url = %q, want PR checks tab", got[0].URL)
	}

	rawOtherBase := strings.Replace(raw, `"baseRef": {"name": "main"}`, `"baseRef": {"name": "develop"}`, 1)
	got, ok, err = parseGitHubPRChecksGraphQLAt(rawOtherBase, now)
	if err != nil {
		t.Fatalf("parseGitHubPRChecksGraphQL(develop): %v", err)
	}
	if !ok || len(got) != 0 {
		t.Errorf("develop base = %+v ok=%v, want no required checks", got, ok)
	}
}

func TestParseGitHubPRChecksGraphQLNullPR(t *testing.T) {
	t.Parallel()
	_, _, err := parseGitHubPRChecksGraphQLAt(`{"data":{"repository":{"pullRequest":null}}}`, time.Now())
	if err == nil {
		t.Fatal("parseGitHubPRChecksGraphQL(null PR) error = nil, want error")
	}
}

func TestParseGitHubPRChecksGraphQLFieldErrorKeepsPR(t *testing.T) {
	t.Parallel()
	raw := `{
		"data": {"repository": {"pullRequest": {"url": "https://github.com/acme/app/pull/1", "commits": {"nodes": []}}}},
		"errors": [{"message": "Field 'rulesets' doesn't exist on type 'Repository'"}]
	}`
	_, _, err := parseGitHubPRChecksGraphQLAt(raw, time.Now())
	if err == nil {
		t.Fatal("parseGitHubPRChecksGraphQL(empty commits + errors) error = nil, want error so persisted checks stay")
	}
}

func TestGithubGraphQLCommandError(t *testing.T) {
	t.Parallel()
	parseErr := fmt.Errorf("parse github checks graphql: invalid character 'g' looking for beginning of value")
	execErr := fmt.Errorf("exit status 1")
	got := githubGraphQLCommandError("gh: authentication required for host github.com", execErr, parseErr)
	if got == nil {
		t.Fatal("githubGraphQLCommandError(...) = nil, want error")
	}
	if !strings.Contains(got.Error(), "authentication required") {
		t.Errorf("githubGraphQLCommandError(...) = %q, want gh diagnostic", got.Error())
	}
	if strings.Contains(got.Error(), "invalid character") {
		t.Errorf("githubGraphQLCommandError(...) = %q, want no parse noise", got.Error())
	}
	if githubGraphQLCommandError("", nil, parseErr) != parseErr {
		t.Errorf("githubGraphQLCommandError(no exec) = %v, want parseErr", githubGraphQLCommandError("", nil, parseErr))
	}
}

func TestParseGitHubPRChecksGraphQLTruncatedNotOK(t *testing.T) {
	t.Parallel()
	raw := `{
		"data": {"repository": {"pullRequest": {
			"url": "https://github.com/acme/app/pull/1",
			"commits": {"nodes": [{"commit": {"statusCheckRollup": {
				"contexts": {
					"pageInfo": {"hasNextPage": true},
					"nodes": [{"__typename": "CheckRun", "name": "build", "status": "COMPLETED", "conclusion": "SUCCESS"}]
				}
			}}}]}
		}}}
	}`
	got, ok, err := parseGitHubPRChecksGraphQLAt(raw, time.Now())
	if err != nil {
		t.Fatalf("parseGitHubPRChecksGraphQL(truncated) error = %v, want nil", err)
	}
	if ok {
		t.Error("parseGitHubPRChecksGraphQL(truncated) ok = true, want false so a partial rollup is not green")
	}
	if len(got) != 1 || got[0].Name != "build" || !got[0].Passed {
		t.Errorf("parseGitHubPRChecksGraphQL(truncated) = %+v, want passing build row", got)
	}
}

func TestParseGitHubPRChecksGraphQLFieldErrorKeepsRows(t *testing.T) {
	t.Parallel()
	raw := `{
		"data": {"repository": {"pullRequest": {
			"url": "https://github.com/acme/app/pull/1",
			"commits": {"nodes": [{"commit": {"statusCheckRollup": {"contexts": {"nodes": [
				{"__typename": "CheckRun", "name": "build", "status": "IN_PROGRESS"}
			]}}}}]}
		}}},
		"errors": [{"message": "Field 'rulesets' doesn't exist on type 'Repository'"}]
	}gh: Field 'rulesets' doesn't exist on type 'Repository'`
	got, ok, err := parseGitHubPRChecksGraphQLAt(raw, time.Now())
	if err != nil {
		t.Fatalf("parseGitHubPRChecksGraphQL(combined output) error = %v, want nil", err)
	}
	if !ok || len(got) != 1 || got[0].Name != "build" || got[0].Status != "pending" {
		t.Errorf("parseGitHubPRChecksGraphQL(combined output) = %+v ok=%v, want pending build ok", got, ok)
	}
}

func TestGithubRefGlobDoubleStar(t *testing.T) {
	t.Parallel()
	if !githubRefGlob("refs/heads/**", "refs/heads/release/v2") {
		t.Errorf("githubRefGlob(%q, %q) = false, want true", "refs/heads/**", "refs/heads/release/v2")
	}
	if !githubRulesetAppliesToBase([]string{"refs/heads/**"}, nil, "release/v2", "main") {
		t.Errorf("githubRulesetAppliesToBase(refs/heads/**, release/v2) = false, want true")
	}
	if githubRefGlob("refs/heads/*", "refs/heads/release/v2") {
		t.Errorf("githubRefGlob(%q, %q) = true, want false", "refs/heads/*", "refs/heads/release/v2")
	}
}

func TestGithubRefGlobNonASCII(t *testing.T) {
	t.Parallel()
	if !githubRefGlob("refs/heads/naïve/*", "refs/heads/naïve/v1") {
		t.Errorf("githubRefGlob non-ASCII = false, want true")
	}
}

func TestApplyGitHubChecksTabURLs(t *testing.T) {
	t.Parallel()
	prURL := "https://github.com/acme/app/pull/11"
	got := applyGitHubChecksTabURLs([]domain.Check{
		{Name: "build", Status: "pending", URL: "https://github.com/acme/app/actions/runs/1/job/2"},
		{Name: "lint", Status: "pending", URL: "https://github.com/acme/app/runs/99"},
		{Name: "test", Status: "pending"},
		{Name: "typecheck", Status: "pass", URL: "https://github.com/acme/app/actions/runs/1/job/3"},
	}, prURL)
	if got[0].URL != "https://github.com/acme/app/pull/11/checks" {
		t.Errorf("pending actions url = %q, want PR checks tab", got[0].URL)
	}
	if got[1].URL != "https://github.com/acme/app/runs/99" {
		t.Errorf("permalink = %q, want unchanged", got[1].URL)
	}
	if got[2].URL != "https://github.com/acme/app/pull/11/checks" {
		t.Errorf("empty pending url = %q, want PR checks tab", got[2].URL)
	}
	if got[3].URL != "https://github.com/acme/app/actions/runs/1/job/3" {
		t.Errorf("completed actions url = %q, want unchanged", got[3].URL)
	}
}

func TestFormatCheckDuration(t *testing.T) {
	cases := []struct {
		d    time.Duration
		want string
	}{
		{0, "0s"},
		{time.Second, "1s"},
		{45 * time.Second, "45s"},
		{60 * time.Second, "1m"},
		{64 * time.Second, "1m 4s"},
		{12 * time.Minute, "12m"},
		{time.Hour, "1h"},
		{time.Hour + 2*time.Minute, "1h 2m"},
	}
	for _, tt := range cases {
		if got := formatCheckDuration(tt.d); got != tt.want {
			t.Errorf("formatCheckDuration(%v) = %q, want %q", tt.d, got, tt.want)
		}
	}
}

func TestRefreshPRChecksUsesGitHubRollup(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "gh-checks", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	local, err := st.AddPR(domain.PR{
		ID: "10", WorkspaceID: ws.ID, Number: 10, Source: domain.SourceLocal,
		Title: "local", State: "In review", Group: "you",
		Checks:   []domain.Check{{Name: "Independent review", Passed: true}},
		ChecksOK: true,
	})
	if err != nil {
		t.Fatalf("AddPR local: %v", err)
	}
	gh, err := st.AddPR(domain.PR{
		ID: "gh-11", WorkspaceID: ws.ID, Number: 11, Source: domain.SourceGitHub,
		Title: "gh", State: "In review", Group: "you",
		Checks:   []domain.Check{{Name: "Independent review", Passed: true}},
		ChecksOK: true,
	})
	if err != nil {
		t.Fatalf("AddPR gh: %v", err)
	}

	prev := getGitHubPRChecks
	t.Cleanup(func() { getGitHubPRChecks = prev })
	var fetchedNum int
	getGitHubPRChecks = func(_ context.Context, _ string, prNumber int, _ string) ([]domain.Check, bool, error) {
		fetchedNum = prNumber
		return []domain.Check{
			{Name: "test", Detail: "CI", Passed: false, Status: "fail"},
		}, false, nil
	}

	eng.RefreshPRChecks(local.ID)
	gotLocal, _ := st.PR(local.ID)
	if len(gotLocal.Checks) != 1 || gotLocal.Checks[0].Name != "Independent review" {
		t.Errorf("RefreshPRChecks(local) mutated checks: %+v", gotLocal.Checks)
	}

	eng.RefreshPRChecks(gh.ID)
	if fetchedNum != 11 {
		t.Errorf("getGitHubPRChecks prNumber = %d, want 11", fetchedNum)
	}
	gotGH, _ := st.PR(gh.ID)
	if gotGH.ChecksOK || len(gotGH.Checks) != 1 || gotGH.Checks[0].Name != "test" {
		t.Errorf("RefreshPRChecks(gh) = checks=%+v checksOK=%v, want GitHub fail rollup", gotGH.Checks, gotGH.ChecksOK)
	}

	eng.RefreshPRChecks("missing")
	if fetchedNum != 11 {
		t.Errorf("RefreshPRChecks(missing) called getGitHubPRChecks with %d", fetchedNum)
	}
}

func TestRefreshPRChecksSkipsMerged(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "merged-checks", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-12", WorkspaceID: ws.ID, Number: 12, Source: domain.SourceGitHub,
		Title: "merged", State: "Merged", Group: "merged",
		Checks:   []domain.Check{{Name: "stale", Passed: true}},
		ChecksOK: true,
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	prev := getGitHubPRChecks
	t.Cleanup(func() { getGitHubPRChecks = prev })
	getGitHubPRChecks = func(context.Context, string, int, string) ([]domain.Check, bool, error) {
		t.Error("getGitHubPRChecks must not run for a merged PR")
		return nil, false, nil
	}
	eng.RefreshPRChecks(pr.ID)
	got, _ := st.PR(pr.ID)
	if len(got.Checks) != 1 || got.Checks[0].Name != "stale" {
		t.Errorf("RefreshPRChecks(merged) = %+v, want persisted stale check", got.Checks)
	}
}

func TestGithubPRChecksDegradesOnError(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, &ScriptedRunner{})
	prev := getGitHubPRChecks
	t.Cleanup(func() { getGitHubPRChecks = prev })
	getGitHubPRChecks = func(context.Context, string, int, string) ([]domain.Check, bool, error) {
		return nil, false, fmt.Errorf("network down")
	}
	checks, ok := eng.githubPRChecks(context.Background(), "/tmp", 1)
	if !ok || len(checks) != 0 {
		t.Errorf("githubPRChecks(error) = checks=%+v ok=%v, want empty/true", checks, ok)
	}
}

func TestGitHubRulesRequirePR(t *testing.T) {
	require, ok := githubRulesRequirePR(`[{"type":"pull_request","parameters":{"required_approving_review_count":1}}]`)
	if !ok || !require {
		t.Errorf("pull_request rule = require %v ok=%v, want true true", require, ok)
	}
	require, ok = githubRulesRequirePR(`[{"type":"deletion"},{"type":"non_fast_forward"}]`)
	if !ok || require {
		t.Errorf("no pull_request rule = require %v ok=%v, want false true", require, ok)
	}
	require, ok = githubRulesRequirePR(`[]`)
	if !ok || require {
		t.Errorf("empty rules = require %v ok=%v, want false true", require, ok)
	}
	if _, ok := githubRulesRequirePR(`not json`); ok {
		t.Error("invalid json ok=true, want false")
	}
	slurped := `[[{"type":"deletion"}],[{"type":"pull_request"}]]`
	require, ok = githubRulesRequirePR(flattenJSONArrayPages(slurped))
	if !ok || !require {
		t.Errorf("pull_request on page 2 = require %v ok=%v, want true true", require, ok)
	}
}

func TestGitHubClassicProtectionRequiresPR(t *testing.T) {
	require, ok := githubClassicProtectionRequiresPR(`{"required_pull_request_reviews":{"required_approving_review_count":1}}`, nil)
	if !ok || !require {
		t.Errorf("classic reviews = require %v ok=%v, want true true", require, ok)
	}
	require, ok = githubClassicProtectionRequiresPR(`{"required_status_checks":{"strict":true}}`, nil)
	if !ok || require {
		t.Errorf("classic without reviews = require %v ok=%v, want false true", require, ok)
	}
	require, ok = githubClassicProtectionRequiresPR("", fmt.Errorf("gh api: gh: Not Found (HTTP 404)"))
	if !ok || require {
		t.Errorf("classic 404 = require %v ok=%v, want false true", require, ok)
	}
	if _, ok := githubClassicProtectionRequiresPR("", fmt.Errorf("gh api: network down")); ok {
		t.Error("classic network error ok=true, want false")
	}
}

func TestGitHubBranchRequiresPRFromAPIs(t *testing.T) {
	notFound := fmt.Errorf("gh api: gh: Not Found (HTTP 404)")
	require, ok := githubBranchRequiresPRFromAPIs(`[]`, nil, "", notFound)
	if !ok || require {
		t.Errorf("empty rulesets + classic 404 = require %v ok=%v, want false true", require, ok)
	}
	require, ok = githubBranchRequiresPRFromAPIs(`[]`, nil, `{"required_pull_request_reviews":{}}`, nil)
	if !ok || !require {
		t.Errorf("empty rulesets + classic reviews = require %v ok=%v, want true true", require, ok)
	}
	require, ok = githubBranchRequiresPRFromAPIs(`[[{"type":"deletion"}],[{"type":"pull_request"}]]`, nil, "", notFound)
	if !ok || !require {
		t.Errorf("paginated ruleset PR + classic 404 = require %v ok=%v, want true true", require, ok)
	}
	require, ok = githubBranchRequiresPRFromAPIs(`[]`, nil, "", fmt.Errorf("timeout"))
	if ok {
		t.Errorf("empty rulesets + classic error = require %v ok=%v, want unknown", require, ok)
	}
}

func TestOwnerRepoFromRemoteURL(t *testing.T) {
	cases := []struct {
		in   string
		want string
	}{
		{"https://github.com/acme/app.git", "acme/app"},
		{"https://github.com/acme/app", "acme/app"},
		{"git@github.com:acme/app.git", "acme/app"},
		{"ssh://git@github.com/acme/parent.git", "acme/parent"},
		{"/tmp/origin.git", ""},
		{"", ""},
	}
	for _, c := range cases {
		got, ok := ownerRepoFromRemoteURL(c.in)
		if c.want == "" {
			if ok {
				t.Errorf("ownerRepoFromRemoteURL(%q) = %q ok=true, want false", c.in, got)
			}
			continue
		}
		if !ok || got != c.want {
			t.Errorf("ownerRepoFromRemoteURL(%q) = %q ok=%v, want %q true", c.in, got, ok, c.want)
		}
	}
}

func repoWithGitHubRemote(t *testing.T, name, ownerRepo string) string {
	t.Helper()
	repo := initRepo(t)
	cmd := exec.Command("git", "remote", "add", name, "https://github.com/"+ownerRepo+".git")
	cmd.Dir = repo
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git remote add %s: %v\n%s", name, err, out)
	}
	return repo
}

func TestGitHubRemoteRequiresPRUsesStore(t *testing.T) {
	st := store.NewInMemory()
	eng := New(st)
	projects := st.Projects()
	if len(projects) == 0 {
		t.Fatal("in-memory store has no projects")
	}
	p := projects[0]
	p.RepoPath = repoWithGitHubRemote(t, "origin", "acme/app")
	st.SetGitHubBranchRequirePR(p.ID, "acme/app", "main", true)

	origFetch := fetchGitHubBranchRequiresPRFn
	t.Cleanup(func() { fetchGitHubBranchRequiresPRFn = origFetch })
	fetchGitHubBranchRequiresPRFn = func(context.Context, string, string, string) (bool, bool) {
		t.Fatal("fresh store hit must not probe GitHub")
		return false, false
	}
	if !eng.githubRemoteRequiresPR(context.Background(), p, "origin", "main") {
		t.Fatal("githubRemoteRequiresPR = false, want stored true without hitting GitHub")
	}
}

func TestGitHubRemoteRequiresPRStaleRefreshAsync(t *testing.T) {
	st := store.NewInMemory()
	eng := New(st)
	var sink recordingSink
	eng.Bus.Subscribe(sink.sink)
	p := st.Projects()[0]
	p.RepoPath = repoWithGitHubRemote(t, "origin", "acme/app")
	st.SetGitHubBranchRequirePR(p.ID, "acme/app", "main", false)

	origTTL := githubBranchPRTTL
	origFetch := fetchGitHubBranchRequiresPRFn
	t.Cleanup(func() {
		githubBranchPRTTL = origTTL
		fetchGitHubBranchRequiresPRFn = origFetch
	})
	githubBranchPRTTL = 0
	started := make(chan struct{})
	block := make(chan struct{})
	fetchGitHubBranchRequiresPRFn = func(context.Context, string, string, string) (bool, bool) {
		close(started)
		<-block
		return true, true
	}
	start := time.Now()
	if eng.githubRemoteRequiresPR(context.Background(), p, "origin", "main") {
		t.Fatal("stale store value must be returned without waiting on GitHub")
	}
	if d := time.Since(start); d > 100*time.Millisecond {
		t.Fatalf("githubRemoteRequiresPR blocked %s on a background refresh", d)
	}
	select {
	case <-started:
	case <-time.After(2 * time.Second):
		t.Fatal("expected a background GitHub refresh")
	}
	close(block)
	deadline := time.Now().Add(2 * time.Second)
	var sawStore, sawEvent bool
	for time.Now().Before(deadline) {
		got, _, ok := st.GitHubBranchRequirePR(p.ID, "acme/app", "main")
		if ok && got {
			sawStore = true
		}
		if sink.has("github.branch_rules") {
			sawEvent = true
		}
		if sawStore && sawEvent {
			return
		}
		time.Sleep(5 * time.Millisecond)
	}
	if !sawStore {
		t.Fatal("background refresh did not persist the new require_pr")
	}
	if !sawEvent {
		t.Fatal("changed background probe must publish github.branch_rules")
	}
}

func TestGitHubRemoteRequiresPRUnknownFailClosed(t *testing.T) {
	st := store.NewInMemory()
	eng := New(st)
	p := st.Projects()[0]
	p.RepoPath = repoWithGitHubRemote(t, "origin", "acme/app")

	origFetch := fetchGitHubBranchRequiresPRFn
	t.Cleanup(func() { fetchGitHubBranchRequiresPRFn = origFetch })
	fetchGitHubBranchRequiresPRFn = func(context.Context, string, string, string) (bool, bool) {
		return false, false
	}
	if !eng.githubRemoteRequiresPR(context.Background(), p, "origin", "main") {
		t.Fatal("never-known + failed probe must fail closed (require PR)")
	}
	if _, _, ok := st.GitHubBranchRequirePR(p.ID, "acme/app", "main"); ok {
		t.Fatal("failed probe must not be written to the store")
	}
}

func TestGitHubRemoteRequiresPRPersistsKnownProbe(t *testing.T) {
	st := store.NewInMemory()
	eng := New(st)
	p := st.Projects()[0]
	p.RepoPath = repoWithGitHubRemote(t, "origin", "acme/app")

	origFetch := fetchGitHubBranchRequiresPRFn
	t.Cleanup(func() { fetchGitHubBranchRequiresPRFn = origFetch })
	n := 0
	fetchGitHubBranchRequiresPRFn = func(context.Context, string, string, string) (bool, bool) {
		n++
		return false, true
	}
	if eng.githubRemoteRequiresPR(context.Background(), p, "origin", "main") {
		t.Fatal("known probe require=false, got true")
	}
	got, _, ok := st.GitHubBranchRequirePR(p.ID, "acme/app", "main")
	if !ok || got {
		t.Fatalf("store after known probe = require %v ok=%v, want false true", got, ok)
	}
	if eng.githubRemoteRequiresPR(context.Background(), p, "origin", "main") {
		t.Fatal("second call should reuse the store")
	}
	if n != 1 {
		t.Errorf("probes = %d, want 1", n)
	}
}

func TestGitHubRemoteRequiresPRScopesCacheByRepo(t *testing.T) {
	st := store.NewInMemory()
	eng := New(st)
	p := st.Projects()[0]
	p.RepoPath = repoWithGitHubRemote(t, "origin", "acme/fork")
	cmd := exec.Command("git", "remote", "add", "upstream", "https://github.com/acme/parent.git")
	cmd.Dir = p.RepoPath
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git remote add upstream: %v\n%s", err, out)
	}

	origFetch := fetchGitHubBranchRequiresPRFn
	t.Cleanup(func() { fetchGitHubBranchRequiresPRFn = origFetch })
	seen := map[string]int{}
	fetchGitHubBranchRequiresPRFn = func(_ context.Context, _, ownerRepo, branch string) (bool, bool) {
		seen[ownerRepo+" "+branch]++
		return ownerRepo == "acme/parent", true
	}
	if eng.githubRemoteRequiresPR(context.Background(), p, "origin", "main") {
		t.Fatal("githubRemoteRequiresPR(origin/main) = true, want false for unprotected fork")
	}
	if !eng.githubRemoteRequiresPR(context.Background(), p, "upstream", "main") {
		t.Fatal("githubRemoteRequiresPR(upstream/main) = false, want true for protected parent")
	}
	fork, _, ok := st.GitHubBranchRequirePR(p.ID, "acme/fork", "main")
	if !ok || fork {
		t.Errorf("store fork main = require %v ok=%v, want false true", fork, ok)
	}
	parent, _, ok := st.GitHubBranchRequirePR(p.ID, "acme/parent", "main")
	if !ok || !parent {
		t.Errorf("store parent main = require %v ok=%v, want true true", parent, ok)
	}
	if seen["acme/fork main"] != 1 || seen["acme/parent main"] != 1 {
		t.Errorf("probes = %v, want one per repository", seen)
	}
}

func TestFetchGitHubBranchRequiresPRUsesRemoteRepo(t *testing.T) {
	repo := repoWithGitHubRemote(t, "upstream", "acme/parent")
	installFakeGH(t, `#!/bin/sh
if [ "$1" = "repo" ]; then
  echo "gh repo view must not choose the default repository" >&2
  exit 1
fi
path=""
for a in "$@"; do
  case "$a" in
    repos/*) path="$a" ;;
  esac
done
case "$path" in
  repos/acme/parent/rules/branches/main)
    paginate=0
    slurp=0
    for a in "$@"; do
      [ "$a" = "--paginate" ] && paginate=1
      [ "$a" = "--slurp" ] && slurp=1
    done
    if [ "$paginate" != 1 ] || [ "$slurp" != 1 ]; then
      echo "rules endpoint must paginate" >&2
      exit 1
    fi
    echo '[[{"type":"deletion"}],[{"type":"pull_request"}]]'
    exit 0
    ;;
  repos/acme/parent/branches/main/protection)
    echo 'gh: Not Found (HTTP 404)' >&2
    exit 1
    ;;
esac
echo "unexpected gh $*" >&2
exit 1
`)
	require, known := fetchGitHubBranchRequiresPR(context.Background(), repo, "acme/parent", "main")
	if !known || !require {
		t.Fatalf("fetchGitHubBranchRequiresPR(acme/parent, main) = require %v known=%v, want true true", require, known)
	}
}

func TestFetchGitHubBranchRequiresPRClassicProtection(t *testing.T) {
	repo := repoWithGitHubRemote(t, "origin", "acme/app")
	installFakeGH(t, `#!/bin/sh
path=""
for a in "$@"; do
  case "$a" in
    repos/*) path="$a" ;;
  esac
done
case "$path" in
  repos/acme/app/rules/branches/main)
    echo '[]'
    exit 0
    ;;
  repos/acme/app/branches/main/protection)
    echo '{"required_pull_request_reviews":{"required_approving_review_count":1}}'
    exit 0
    ;;
esac
echo "unexpected gh $*" >&2
exit 1
`)
	require, known := fetchGitHubBranchRequiresPR(context.Background(), repo, "acme/app", "main")
	if !known || !require {
		t.Fatalf("fetchGitHubBranchRequiresPR classic reviews = require %v known=%v, want true true", require, known)
	}
}
