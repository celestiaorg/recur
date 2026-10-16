package engine

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
)

func TestParseGitHubPRList(t *testing.T) {
	t.Parallel()
	rows, err := parseGitHubPRList(`[
		{"number": 12, "title": "Add OAuth", "url": "https://github.com/acme/app/pull/12",
		 "headRefName": "feat/oauth", "baseRefName": "main",
		 "author": {"login": "sam"}, "isDraft": false, "createdAt": "2026-08-01T09:00:00Z", "updatedAt": "2026-08-21T12:00:00Z",
		 "additions": 10, "deletions": 2, "changedFiles": 3, "reviewDecision": "REVIEW_REQUIRED",
		 "reviewRequests": [{"login": "avery"}], "assignees": [], "state": "OPEN"}
	]`)
	if err != nil {
		t.Fatalf("parseGitHubPRList: %v", err)
	}
	if len(rows) != 1 || rows[0].Number != 12 || rows[0].Author.Login != "sam" {
		t.Fatalf("parseGitHubPRList = %+v", rows)
	}
	if !listedNeedsAttention(rows[0], "avery") {
		t.Error("review request of viewer should need attention")
	}
	got := listedAttentionReasons(rows[0], "avery")
	if len(got) != 1 || got[0] != domain.AttentionReviewRequested {
		t.Errorf("listedAttentionReasons(avery) = %v, want [review_requested]", got)
	}
	if listedNeedsAttention(rows[0], "sam") {
		t.Error("author without changes/failing checks should not need attention")
	}
	if listedCreatedAt(rows[0]) != "2026-08-01T09:00:00Z" {
		t.Errorf("listedCreatedAt = %q, want 2026-08-01T09:00:00Z", listedCreatedAt(rows[0]))
	}
	state, group := listedStateGroup(rows[0], "avery")
	if state != "Review required" || group != prGroupYou {
		t.Errorf("listedStateGroup = %q %q, want Review required / you", state, group)
	}
	state, group = listedStateGroup(rows[0], "other")
	if group != prGroupOpen || state != "Review required" {
		t.Errorf("other viewer listedStateGroup = %q %q, want Review required / open", state, group)
	}
}

func TestListedNeedsAttentionAuthorChanges(t *testing.T) {
	t.Parallel()
	row := ghListedPR{
		State:          "OPEN",
		Author:         ghListedActor{Login: "avery"},
		ReviewDecision: "CHANGES_REQUESTED",
	}
	if !listedNeedsAttention(row, "avery") {
		t.Error("author with changes requested should need attention")
	}
	got := listedAttentionReasons(row, "avery")
	if len(got) != 1 || got[0] != domain.AttentionChangesRequested {
		t.Errorf("listedAttentionReasons(changes) = %v, want [changes_requested]", got)
	}
	if listedNeedsAttention(row, "pat") {
		t.Error("bystander should not need attention")
	}
	row.ReviewDecision = ""
	row.StatusCheckRollup = []byte(`[{"conclusion":"FAILURE"}]`)
	if !listedNeedsAttention(row, "avery") {
		t.Error("author with failing checks should need attention")
	}
	got = listedAttentionReasons(row, "avery")
	if len(got) != 1 || got[0] != domain.AttentionChecksFailing {
		t.Errorf("listedAttentionReasons(checks) = %v, want [checks_failing]", got)
	}
	row.StatusCheckRollup = []byte(`[{"status":"IN_PROGRESS"}]`)
	if listedNeedsAttention(row, "avery") {
		t.Error("author with pending checks should not need attention")
	}
	got = listedAttentionReasons(row, "avery")
	if got != nil {
		t.Errorf("listedAttentionReasons(pending) = %v, want nil", got)
	}
	row.State = "MERGED"
	if listedNeedsAttention(row, "avery") {
		t.Error("merged PR should not need attention")
	}
}

func TestListedNeedsAttentionDraft(t *testing.T) {
	t.Parallel()
	row := ghListedPR{
		State:             "OPEN",
		IsDraft:           true,
		Author:            ghListedActor{Login: "avery"},
		ReviewDecision:    "REVIEW_REQUIRED",
		ReviewRequests:    []ghListedActor{{Login: "avery"}},
		Assignees:         []ghListedActor{{Login: "avery"}},
		StatusCheckRollup: []byte(`[{"conclusion":"FAILURE"}]`),
	}
	if listedNeedsAttention(row, "avery") {
		t.Error("listedNeedsAttention(draft) = true, want false")
	}
	got := listedAttentionReasons(row, "avery")
	if got != nil {
		t.Errorf("listedAttentionReasons(draft) = %v, want nil", got)
	}
	state, group := listedStateGroup(row, "avery")
	if group != prGroupOpen {
		t.Errorf("listedStateGroup(draft) group = %q, want %q", group, prGroupOpen)
	}
	if state != "Review required" {
		t.Errorf("listedStateGroup(draft) state = %q, want Review required", state)
	}
}

func TestListedAttentionReasonsAssigned(t *testing.T) {
	t.Parallel()
	row := ghListedPR{
		State:     "OPEN",
		Author:    ghListedActor{Login: "sam"},
		Assignees: []ghListedActor{{Login: "avery"}},
	}
	got := listedAttentionReasons(row, "avery")
	if len(got) != 1 || got[0] != domain.AttentionAssigned {
		t.Errorf("listedAttentionReasons(assigned) = %v, want [assigned]", got)
	}
}

func TestListedChecksOKReadsConclusionNotName(t *testing.T) {
	t.Parallel()
	ok := listedChecksOK(ghListedPR{StatusCheckRollup: []byte(`[{"name":"Error","conclusion":"success"}]`)})
	if !ok {
		t.Fatal("listedChecksOK(check named Error, conclusion success) = false, want true")
	}
	fail := listedChecksOK(ghListedPR{StatusCheckRollup: []byte(`[{"conclusion":"FAILURE"}]`)})
	if fail {
		t.Fatal("listedChecksOK(conclusion FAILURE) = true, want false")
	}
	_, has := listedCheckRollup(ghListedPR{StatusCheckRollup: []byte(`[{"conclusion":"success"}]`)})
	if !has {
		t.Fatal("listedCheckRollup(success) has = false, want true")
	}
	_, has = listedCheckRollup(ghListedPR{})
	if has {
		t.Fatal("listedCheckRollup(empty) has = true, want false")
	}
	pending, has := listedCheckRollup(ghListedPR{StatusCheckRollup: []byte(`[{"name":"build","status":"IN_PROGRESS","conclusion":null}]`)})
	if !pending || !has {
		t.Fatal("listedCheckRollup(IN_PROGRESS) ok/has = false, want true/true")
	}
	pending, has = listedCheckRollup(ghListedPR{StatusCheckRollup: []byte(`[{"context":"license","state":"PENDING"}]`)})
	if !pending || !has {
		t.Fatal("listedCheckRollup(PENDING) ok/has = false, want true/true")
	}
	pending, has = listedCheckRollup(ghListedPR{StatusCheckRollup: []byte(`[{"name":"lint","status":"QUEUED"}]`)})
	if !pending || !has {
		t.Fatal("listedCheckRollup(QUEUED) ok/has = false, want true/true")
	}
	failRows, failOK, failHas := listedChecks(ghListedPR{StatusCheckRollup: []byte(`[{"name":"build","conclusion":"FAILURE"}]`)})
	if failOK || !failHas || len(failRows) != 1 || failRows[0].Status != "fail" || failRows[0].Passed {
		t.Fatalf("listedChecks(FAILURE) = %+v ok=%v has=%v, want fail row", failRows, failOK, failHas)
	}
	pendRows, pendOK, pendHas := listedChecks(ghListedPR{StatusCheckRollup: []byte(`[{"name":"build","status":"IN_PROGRESS","conclusion":null}]`)})
	if !pendOK || !pendHas || len(pendRows) != 1 || pendRows[0].Status != "pending" {
		t.Fatalf("listedChecks(IN_PROGRESS) = %+v ok=%v has=%v, want pending row", pendRows, pendOK, pendHas)
	}
}

func TestCompactAgo(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC)
	cases := []struct {
		in   time.Time
		want string
	}{
		{now.Add(-30 * time.Second), "now"},
		{now.Add(-5 * time.Minute), "5m"},
		{now.Add(-3 * time.Hour), "3h"},
		{now.Add(-2 * 24 * time.Hour), "2d"},
	}
	for _, c := range cases {
		if got := compactAgo(c.in, now); got != c.want {
			t.Errorf("compactAgo(%v) = %q, want %q", c.in, got, c.want)
		}
	}
}

func TestSyncGitHubPRsImportsAndDedupes(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}

	origList := listGitHubPRsFn
	origLogin := githubLoginFn
	t.Cleanup(func() {
		listGitHubPRsFn = origList
		githubLoginFn = origLogin
	})
	githubLoginFn = func(context.Context) string { return "avery" }
	listGitHubPRsFn = func(_ context.Context, repoPath, state string, _ int) ([]ghListedPR, error) {
		if !strings.Contains(repoPath, repo) && repoPath != repo {
			return nil, nil
		}
		if state == "merged" {
			return []ghListedPR{{
				Number: 9, Title: "Done", URL: "https://github.com/acme/app/pull/9",
				HeadRefName: "done", BaseRefName: "main", Author: ghListedActor{Login: "sam"},
				State: "MERGED", UpdatedAt: time.Now().UTC().Format(time.RFC3339),
			}}, nil
		}
		return []ghListedPR{
			{
				Number: 12, Title: "Please review", URL: "https://github.com/acme/app/pull/12",
				HeadRefName: "feat/oauth", BaseRefName: "main", Author: ghListedActor{Login: "sam"},
				State: "OPEN", ReviewDecision: "REVIEW_REQUIRED",
				ReviewRequests: []ghListedActor{{Login: "avery"}},
				Additions:      4, Deletions: 1, ChangedFiles: 2,
				UpdatedAt: time.Now().UTC().Format(time.RFC3339),
			},
			{
				Number: 13, Title: "Unrelated open", URL: "https://github.com/acme/app/pull/13",
				HeadRefName: "chore/docs", BaseRefName: "main", Author: ghListedActor{Login: "pat"},
				State: "OPEN", Additions: 1, ChangedFiles: 1,
				UpdatedAt: time.Now().UTC().Format(time.RFC3339),
			},
		}, nil
	}
	got := eng.SyncGitHubPRs()
	var you, open, merged int
	for _, p := range got {
		if p.ProjectID != "acme" {
			continue
		}
		switch p.Group {
		case prGroupYou:
			you++
			if p.Number != 12 {
				t.Errorf("needs-you PR = #%d, want #12", p.Number)
			}
			if !p.ReviewRequested {
				t.Error("needs-you PR ReviewRequested = false, want true")
			}
			if len(p.AttentionReasons) != 1 || p.AttentionReasons[0] != domain.AttentionReviewRequested {
				t.Errorf("needs-you PR AttentionReasons = %v, want [review_requested]", p.AttentionReasons)
			}
		case prGroupOpen:
			if p.State != "Merged" && p.State != "Closed" {
				open++
			}
		case prGroupMerged:
			merged++
		}
	}
	if you != 1 || open != 1 || merged != 1 {
		t.Errorf("acme groups you=%d open=%d merged=%d, want 1/1/1", you, open, merged)
	}
}

func TestSyncGitHubPRsTTLKeepsStore(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddPR(domain.PR{ID: "keep", Title: "Keep", Source: domain.SourceGitHub, Number: 1}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	githubPRSyncMinInterval = time.Hour
	eng.syncGitHubPRsAt = time.Now()
	got := eng.SyncGitHubPRs()
	if len(got) != 1 || got[0].ID != "keep" {
		t.Errorf("SyncGitHubPRs within TTL = %d PRs, want stored [keep]", len(got))
	}
}

func TestApplyListedGitHubPRReconcilesStaleChecks(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := st.AddPR(domain.PR{
		ID: "gh-acme-12", ProjectID: "acme", Number: 12, Source: domain.SourceGitHub,
		Title: "OAuth", URL: "https://github.com/acme/app/pull/12",
		Branch: "feat/oauth", State: "Open", Group: prGroupOpen,
		Checks:   []domain.Check{{Name: "build", Passed: false, Status: "pending", URL: "https://github.com/acme/app/runs/1"}},
		ChecksOK: true, HasChecks: true,
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	row := ghListedPR{
		Number: 12, Title: "OAuth", URL: "https://github.com/acme/app/pull/12",
		HeadRefName: "feat/oauth", BaseRefName: "main", State: "OPEN",
		Author:            ghListedActor{Login: "sam"},
		StatusCheckRollup: []byte(`[{"name":"build","conclusion":"FAILURE"}]`),
	}
	if _, created := eng.applyListedGitHubPR("acme", row, "avery", time.Now()); created {
		t.Fatal("expected update")
	}
	got, ok := st.PR("gh-acme-12")
	if !ok {
		t.Fatal("PR missing")
	}
	if got.ChecksOK {
		t.Fatal("ChecksOK after fail list = true, want false")
	}
	if len(got.Checks) != 1 || got.Checks[0].Status != "fail" || got.Checks[0].Passed {
		t.Fatalf("Checks after fail list = %+v, want fail row", got.Checks)
	}

	row.StatusCheckRollup = []byte(`[{"name":"build","conclusion":"SUCCESS"}]`)
	eng.applyListedGitHubPR("acme", row, "avery", time.Now())
	got, _ = st.PR("gh-acme-12")
	if !got.ChecksOK {
		t.Fatal("ChecksOK after pass list = false, want true")
	}
	if len(got.Checks) != 1 || got.Checks[0].Status != "pass" {
		t.Fatalf("Checks after pass list = %+v, want pass row", got.Checks)
	}

	st.UpdatePR("gh-acme-12", func(p *domain.PR) {
		p.Checks = []domain.Check{{Name: "build", Passed: true, Status: "pass", URL: "https://github.com/acme/app/runs/1"}}
		p.ChecksOK = true
		p.HasChecks = true
	})
	eng.applyListedGitHubPR("acme", row, "avery", time.Now())
	got, _ = st.PR("gh-acme-12")
	if got.Checks[0].URL != "https://github.com/acme/app/runs/1" {
		t.Fatalf("same-kind list sync dropped GetPR url: %+v", got.Checks)
	}
}

func TestApplyListedGitHubPRKeepsUnreportedPendingChecks(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	prev := getGitHubPRChecks
	t.Cleanup(func() { getGitHubPRChecks = prev })
	getGitHubPRChecks = func(context.Context, string, int, string) ([]domain.Check, bool, error) {
		return nil, false, fmt.Errorf("graphql unavailable")
	}
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := st.AddPR(domain.PR{
		ID: "gh-acme-12", ProjectID: "acme", Number: 12, Source: domain.SourceGitHub,
		Title: "OAuth", URL: "https://github.com/acme/app/pull/12",
		Branch: "feat/oauth", State: "Open", Group: prGroupOpen,
		Checks: []domain.Check{
			{Name: "lint", Passed: true, Status: "pass", URL: "https://github.com/acme/app/runs/1"},
			{Name: "CI Success", Passed: false, Status: "pending"},
		},
		ChecksOK: true, HasChecks: true,
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	row := ghListedPR{
		Number: 12, Title: "OAuth", URL: "https://github.com/acme/app/pull/12",
		HeadRefName: "feat/oauth", BaseRefName: "main", State: "OPEN",
		Author:            ghListedActor{Login: "sam"},
		StatusCheckRollup: []byte(`[{"name":"lint","conclusion":"SUCCESS"}]`),
	}
	if _, created := eng.applyListedGitHubPR("acme", row, "avery", time.Now()); created {
		t.Fatal("expected update")
	}
	got, ok := st.PR("gh-acme-12")
	if !ok {
		t.Fatal("PR missing")
	}
	if checkRowsKind(got.Checks) != "pending" {
		t.Fatalf("list sync kind = %q checks=%+v, want pending (kept unreported CI Success)", checkRowsKind(got.Checks), got.Checks)
	}
	if len(got.Checks) != 2 || got.Checks[0].Name != "lint" || got.Checks[0].URL != "https://github.com/acme/app/runs/1" ||
		got.Checks[1].Name != "CI Success" || got.Checks[1].Status != "pending" {
		t.Fatalf("Checks = %+v, want lint pass with GetPR url + pending CI Success", got.Checks)
	}
}

func TestApplyListedGitHubPRRetiresQueuedWorkflowPlaceholder(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	prev := getGitHubPRChecks
	t.Cleanup(func() { getGitHubPRChecks = prev })
	fetched := 0
	getGitHubPRChecks = func(context.Context, string, int, string) ([]domain.Check, bool, error) {
		fetched++
		return []domain.Check{{Name: "publish", Passed: true, Status: "pass"}}, true, nil
	}
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := st.AddPR(domain.PR{
		ID: "gh-acme-12", ProjectID: "acme", Number: 12, Source: domain.SourceGitHub,
		Title: "OAuth", URL: "https://github.com/acme/app/pull/12",
		Branch: "feat/oauth", State: "Open", Group: prGroupOpen,
		Checks: []domain.Check{
			{Name: "Release", Passed: false, Status: "pending"},
		},
		ChecksOK: true, HasChecks: true,
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	row := ghListedPR{
		Number: 12, Title: "OAuth", URL: "https://github.com/acme/app/pull/12",
		HeadRefName: "feat/oauth", BaseRefName: "main", State: "OPEN",
		Author:            ghListedActor{Login: "sam"},
		StatusCheckRollup: []byte(`[{"name":"publish","conclusion":"SUCCESS"}]`),
	}
	if _, created := eng.applyListedGitHubPR("acme", row, "avery", time.Now()); created {
		t.Fatal("expected update")
	}
	got, ok := st.PR("gh-acme-12")
	if !ok {
		t.Fatal("PR missing")
	}
	if fetched != 1 {
		t.Fatalf("GetPR refresh calls = %d, want 1", fetched)
	}
	if checkRowsKind(got.Checks) != "pass" {
		t.Fatalf("list sync kind = %q checks=%+v, want pass (retired queued Release)", checkRowsKind(got.Checks), got.Checks)
	}
	if len(got.Checks) != 1 || got.Checks[0].Name != "publish" || got.Checks[0].Status != "pass" {
		t.Fatalf("Checks = %+v, want publish pass without Release placeholder", got.Checks)
	}

	eng.applyListedGitHubPR("acme", row, "avery", time.Now())
	got, _ = st.PR("gh-acme-12")
	if fetched != 1 {
		t.Fatalf("repeat list sync refresh calls = %d, want 1", fetched)
	}
	if checkRowsKind(got.Checks) != "pass" || len(got.Checks) != 1 || got.Checks[0].Name != "publish" {
		t.Fatalf("repeat list sync checks = %+v, want publish pass", got.Checks)
	}
}

func TestApplyListedGitHubPRReopensClosedPR(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := st.AddPR(domain.PR{
		ID: "gh-acme-12", ProjectID: "acme", Number: 12, Source: domain.SourceGitHub,
		Title: "OAuth", URL: "https://github.com/acme/app/pull/12",
		Branch: "feat/oauth", State: "Closed", Group: prGroupMerged,
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	id, created := eng.applyListedGitHubPR("acme", ghListedPR{
		Number: 12, Title: "OAuth", URL: "https://github.com/acme/app/pull/12",
		HeadRefName: "feat/oauth", BaseRefName: "main", State: "OPEN",
		Author: ghListedActor{Login: "sam"},
	}, "avery", time.Now())
	if created || id != "gh-acme-12" {
		t.Fatalf("applyListedGitHubPR created=%v id=%q, want update gh-acme-12", created, id)
	}
	got, ok := st.PR(id)
	if !ok {
		t.Fatal("PR missing after reopen list")
	}
	if got.State != "Open" || got.Group != prGroupOpen {
		t.Errorf("applyListedGitHubPR state=%q group=%q, want Open %s", got.State, got.Group, prGroupOpen)
	}
}

func TestApplyListedGitHubPRRefreshesQueuedPR(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := st.AddPR(domain.PR{
		ID: "gh-acme-12", ProjectID: "acme", Number: 12, Source: domain.SourceGitHub,
		Title: "OAuth", URL: "https://github.com/acme/app/pull/12",
		Branch: "feat/oauth", State: "Open", Group: prGroupOpen,
		MergeQueueEnabled: true, MergeQueued: true,
	}); err != nil {
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
		return ghPRSnapshot{State: "OPEN", Title: "OAuth", URL: "https://github.com/acme/app/pull/12", MergeQueueEnabled: true}, nil
	}

	eng.applyListedGitHubPR("acme", ghListedPR{
		Number: 12, Title: "OAuth", URL: "https://github.com/acme/app/pull/12",
		HeadRefName: "feat/oauth", BaseRefName: "main", State: "OPEN",
		Author: ghListedActor{Login: "sam"},
	}, "avery", time.Now())
	select {
	case <-snapped:
	case <-time.After(2 * time.Second):
		t.Fatal("sweep did not refresh the queued PR's snapshot")
	}
	for deadline := time.Now().Add(2 * time.Second); time.Now().Before(deadline); time.Sleep(10 * time.Millisecond) {
		if _, busy := eng.prConvBusy.Load("gh-acme-12"); !busy {
			break
		}
	}
	if got, _ := st.PR("gh-acme-12"); got.MergeQueued {
		t.Error("MergeQueued still true after snapshot reported the PR left the queue")
	}
}

func TestApplyListedGitHubPRLinksMatchingWorkspace(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{ID: "ws-oauth", ProjectID: "acme", Branch: "feat/oauth"}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	id, created := eng.applyListedGitHubPR("acme", ghListedPR{
		Number: 12, Title: "OAuth", URL: "https://github.com/acme/app/pull/12",
		HeadRefName: "refs/heads/feat/oauth", BaseRefName: "main", State: "OPEN",
		Author: ghListedActor{Login: "sam"},
	}, "avery", time.Now())
	if !created || id == "" {
		t.Fatalf("applyListedGitHubPR created=%v id=%q", created, id)
	}
	got, ok := st.PR(id)
	if !ok || got.WorkspaceID != "ws-oauth" {
		t.Fatalf("imported PR workspace = %q, want ws-oauth (%+v)", got.WorkspaceID, got)
	}
}

func TestApplyListedGitHubPRStoresCreatedAt(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	opened := "2026-01-15T12:00:00Z"
	touched := "2026-08-21T12:00:00Z"
	id, created := eng.applyListedGitHubPR("acme", ghListedPR{
		Number: 12, Title: "OAuth", URL: "https://github.com/acme/app/pull/12",
		HeadRefName: "feat/oauth", BaseRefName: "main", State: "OPEN",
		Author: ghListedActor{Login: "sam"}, CreatedAt: opened, UpdatedAt: touched,
	}, "avery", time.Now())
	if !created {
		t.Fatal("expected create")
	}
	got, ok := st.PR(id)
	if !ok {
		t.Fatal("missing PR")
	}
	if got.CreatedAt != opened {
		t.Errorf("CreatedAt = %q, want %q", got.CreatedAt, opened)
	}
	if got.UpdatedAt != touched {
		t.Errorf("UpdatedAt = %q, want %q", got.UpdatedAt, touched)
	}
	_, created = eng.applyListedGitHubPR("acme", ghListedPR{
		Number: 12, Title: "OAuth v2", URL: "https://github.com/acme/app/pull/12",
		HeadRefName: "feat/oauth", BaseRefName: "main", State: "OPEN",
		Author: ghListedActor{Login: "sam"}, CreatedAt: opened,
	}, "avery", time.Now())
	if created {
		t.Fatal("second apply should update")
	}
	got, _ = st.PR(id)
	if got.CreatedAt != opened {
		t.Errorf("CreatedAt after update = %q, want %q", got.CreatedAt, opened)
	}
}

func TestApplyListedGitHubPRStoresParticipants(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	id, created := eng.applyListedGitHubPR("acme", ghListedPR{
		Number: 12, Title: "OAuth", URL: "https://github.com/acme/app/pull/12",
		HeadRefName: "feat/oauth", BaseRefName: "main", State: "OPEN",
		Author:         ghListedActor{Login: "sam"},
		Assignees:      []ghListedActor{{Login: "pat"}},
		ReviewRequests: []ghListedActor{{Login: "avery"}},
		LatestReviews:  []ghListedReview{{Author: ghListedActor{Login: "sam"}}},
	}, "avery", time.Now())
	if !created {
		t.Fatal("expected create")
	}
	got, ok := st.PR(id)
	if !ok {
		t.Fatal("missing PR")
	}
	if len(got.Participants) != 3 || got.Participants[0].Login != "sam" || got.Participants[1].Login != "pat" || got.Participants[2].Login != "avery" {
		t.Errorf("Participants = %+v, want sam, pat, avery", got.Participants)
	}
}

func TestApplyListedGitHubPRLinksWorkspaceOnLaterSync(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	row := ghListedPR{
		Number: 12, Title: "OAuth", URL: "https://github.com/acme/app/pull/12",
		HeadRefName: "feat/oauth", BaseRefName: "main", State: "OPEN",
		Author: ghListedActor{Login: "sam"},
	}
	id, created := eng.applyListedGitHubPR("acme", row, "avery", time.Now())
	if !created {
		t.Fatal("expected create")
	}
	got, _ := st.PR(id)
	if got.WorkspaceID != "" {
		t.Fatalf("WorkspaceID = %q, want empty before workspace exists", got.WorkspaceID)
	}
	if _, err := st.AddWorkspace(domain.Workspace{ID: "ws-oauth", ProjectID: "acme", Branch: "feat/oauth"}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	_, created = eng.applyListedGitHubPR("acme", row, "avery", time.Now())
	if created {
		t.Fatal("second apply should update")
	}
	got, _ = st.PR(id)
	if got.WorkspaceID != "ws-oauth" {
		t.Errorf("WorkspaceID after later sync = %q, want ws-oauth", got.WorkspaceID)
	}
}

func TestApplyListedGitHubPRKeepsLocalBehindBase(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: "ws-oauth", ProjectID: "acme", Branch: "feat/oauth", WorktreePath: "/tmp/acme-oauth",
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-12", ProjectID: "acme", WorkspaceID: "ws-oauth", Number: 12, Source: domain.SourceGitHub,
		Title: "OAuth", URL: "https://github.com/acme/app/pull/12", Branch: "feat/oauth", Base: "main",
		Group: prGroupOpen, State: "Open", BehindBase: 3, AuthorLogin: "sam",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	row := ghListedPR{
		Number: 12, Title: "OAuth", URL: "https://github.com/acme/app/pull/12",
		HeadRefName: "feat/oauth", BaseRefName: "main", State: "OPEN",
		Author: ghListedActor{Login: "sam"}, MergeStateStatus: "CLEAN",
	}
	_, created := eng.applyListedGitHubPR("acme", row, "avery", time.Now())
	if created {
		t.Fatal("expected update of existing PR")
	}
	got, _ := st.PR(pr.ID)
	if got.BehindBase != 3 {
		t.Errorf("BehindBase = %d, want 3 (listing CLEAN must not clobber git-computed count)", got.BehindBase)
	}

	noWS := domain.PR{
		ID: "gh-acme-13", ProjectID: "acme", Number: 13, Source: domain.SourceGitHub,
		Title: "Docs", URL: "https://github.com/acme/app/pull/13", Branch: "docs", Base: "main",
		Group: prGroupOpen, State: "Open", BehindBase: 3, AuthorLogin: "sam",
	}
	if _, err := st.AddPR(noWS); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	_, created = eng.applyListedGitHubPR("acme", ghListedPR{
		Number: 13, Title: "Docs", URL: "https://github.com/acme/app/pull/13",
		HeadRefName: "docs", BaseRefName: "main", State: "OPEN",
		Author: ghListedActor{Login: "sam"}, MergeStateStatus: "BEHIND",
	}, "avery", time.Now())
	if created {
		t.Fatal("expected update of imported PR")
	}
	got, _ = st.PR(noWS.ID)
	if got.BehindBase != 1 {
		t.Errorf("imported BehindBase = %d, want 1 from listing BEHIND", got.BehindBase)
	}
}

func TestLinkUnlinkedGitHubPRsAttachesExistingWorkspace(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-12", ProjectID: "acme", Number: 12, Source: domain.SourceGitHub,
		Title: "OAuth", Branch: "feat/oauth", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{ID: "ws-oauth", ProjectID: "acme", Branch: "feat/oauth"}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	eng.linkUnlinkedGitHubPRs()
	got, _ := st.PR(pr.ID)
	if got.WorkspaceID != "ws-oauth" {
		t.Errorf("WorkspaceID = %q, want ws-oauth", got.WorkspaceID)
	}
}

func TestSyncGitHubPRsLinksExistingWorkspace(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme-not-a-repo", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-12", ProjectID: "acme", Number: 12, Source: domain.SourceGitHub,
		Title: "OAuth", Branch: "feat/oauth", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{ID: "ws-oauth", ProjectID: "acme", Branch: "feat/oauth"}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	eng.SyncGitHubPRs()
	got, _ := st.PR(pr.ID)
	if got.WorkspaceID != "ws-oauth" {
		t.Errorf("after SyncGitHubPRs WorkspaceID = %q, want ws-oauth", got.WorkspaceID)
	}
}

func TestWorkspaceMatchingPRBranchSkipsProjectBase(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{ID: "ws-main", ProjectID: "acme", Branch: "main"}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	if _, found := eng.workspaceMatchingPRBranch("acme", "main"); found {
		t.Fatal("should not link a PR whose head is the project base")
	}
}

func TestWorkspaceMatchingPRBranchViaUpstream(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	clone := initCloneTrackingRemoteBranch(t, "feat/oauth", "local-oauth")
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: clone, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: "ws-oauth", ProjectID: "acme", Branch: "local-oauth", WorktreePath: clone,
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}

	got, found := eng.workspaceMatchingPRBranch("acme", "feat/oauth")
	if !found || got.ID != "ws-oauth" {
		t.Fatalf("workspaceMatchingPRBranch(acme, feat/oauth) = id=%q found=%v, want ws-oauth", got.ID, found)
	}
	got, found = eng.workspaceMatchingPRBranch("acme", "local-oauth")
	if !found || got.ID != "ws-oauth" {
		t.Fatalf("workspaceMatchingPRBranch(acme, local-oauth) = id=%q found=%v, want ws-oauth", got.ID, found)
	}
}

func TestWorkspaceMatchingPRBranchSkipsTrackedProjectBase(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	clone := initCloneTrackingRemoteBranch(t, "main", "work")
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: clone, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: "ws-work", ProjectID: "acme", Branch: "work", WorktreePath: clone,
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	if _, found := eng.workspaceMatchingPRBranch("acme", "main"); found {
		t.Fatal("workspaceMatchingPRBranch(acme, main) found a workspace, want none (project base, even via @{upstream})")
	}
}

func TestWorkspaceMatchingPRBranchPrefersLocalName(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	clone := initCloneTrackingRemoteBranch(t, "feat/oauth", "local-oauth")
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: clone, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: "ws-track", ProjectID: "acme", Branch: "local-oauth", WorktreePath: clone,
	}); err != nil {
		t.Fatalf("AddWorkspace track: %v", err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: "ws-exact", ProjectID: "acme", Branch: "feat/oauth",
	}); err != nil {
		t.Fatalf("AddWorkspace exact: %v", err)
	}
	got, found := eng.workspaceMatchingPRBranch("acme", "feat/oauth")
	if !found || got.ID != "ws-exact" {
		t.Fatalf("workspaceMatchingPRBranch(acme, feat/oauth) = id=%q found=%v, want ws-exact", got.ID, found)
	}
}

func TestLinkUnlinkedGitHubPRsViaUpstream(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	clone := initCloneTrackingRemoteBranch(t, "feat/oauth", "local-oauth")
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: clone, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-12", ProjectID: "acme", Number: 12, Source: domain.SourceGitHub,
		Title: "OAuth", Branch: "feat/oauth", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: "ws-oauth", ProjectID: "acme", Branch: "local-oauth", WorktreePath: clone,
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	eng.linkUnlinkedGitHubPRs()
	got, _ := st.PR(pr.ID)
	if got.WorkspaceID != "ws-oauth" {
		t.Errorf("linkUnlinkedGitHubPRs WorkspaceID = %q, want ws-oauth", got.WorkspaceID)
	}
}

// A local branch re-pointed with `git branch -u` outside Recur links to the
// replacement PR on the next sweep, and the open PR wins over the closed one
// still linked to the workspace.
func TestSweepGitHubPRsLinksUpstreamSetLater(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, nil)
	clone := initCloneTrackingRemoteBranch(t, "recur/tab-close", "me/roasted-risotto")
	runGitIn(t, clone, "branch", "--unset-upstream")
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: clone, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: "ws-r", ProjectID: "acme", Branch: "me/roasted-risotto", WorktreePath: clone,
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	if _, err := st.AddPR(domain.PR{
		ID: "gh-acme-9", WorkspaceID: "ws-r", ProjectID: "acme", Number: 9, Source: domain.SourceGitHub,
		Branch: "recur/tab-close", Base: "main", State: "Closed", Group: prGroupMerged,
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	origList := listGitHubPRsFn
	t.Cleanup(func() { listGitHubPRsFn = origList })
	listGitHubPRsFn = func(_ context.Context, _, state string, _ int) ([]ghListedPR, error) {
		if state != "open" {
			return nil, nil
		}
		return []ghListedPR{{
			Number: 13, Title: "Tab close menu", URL: "https://github.com/acme/app/pull/13",
			HeadRefName: "recur/tab-close", BaseRefName: "main", State: "OPEN",
			UpdatedAt: time.Now().UTC().Format(time.RFC3339),
		}}, nil
	}

	eng.sweepGitHubPRs()
	if got, _ := st.PR("gh-acme-13"); got.WorkspaceID != "" {
		t.Fatalf("before upstream: #13 WorkspaceID = %q, want unlinked", got.WorkspaceID)
	}

	runGitIn(t, clone, "branch", "-u", "origin/recur/tab-close")
	eng.sweepGitHubPRs()
	if got, _ := st.PR("gh-acme-13"); got.WorkspaceID != "ws-r" {
		t.Fatalf("after upstream: #13 WorkspaceID = %q, want ws-r", got.WorkspaceID)
	}
	if got, _ := st.PR("gh-acme-9"); got.WorkspaceID != "ws-r" {
		t.Errorf("closed #9 WorkspaceID = %q, want ws-r kept", got.WorkspaceID)
	}
	var land domain.WorkspaceLandState
	eng.attachOpenPRLand(context.Background(), &land, "ws-r", "", nil)
	if land.PRNumber != 13 {
		t.Errorf("land PRNumber = %d, want open #13", land.PRNumber)
	}
	linked := false
	for _, ev := range sink.byName("pr.updated") {
		d, _ := ev.Data.(map[string]any)
		if d["prId"] == "gh-acme-13" && d["workspaceId"] == "ws-r" {
			linked = true
		}
	}
	if !linked {
		t.Error("no pr.updated carrying #13's workspace link")
	}
}

func TestLinkWorkspaceToMatchingPRsViaUpstream(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	clone := initCloneTrackingRemoteBranch(t, "feat/oauth", "local-oauth")
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: clone, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-12", ProjectID: "acme", Number: 12, Source: domain.SourceGitHub,
		Title: "OAuth", Branch: "feat/oauth", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	ws, err := st.AddWorkspace(domain.Workspace{
		ID: "ws-oauth", ProjectID: "acme", Branch: "local-oauth", WorktreePath: clone,
	})
	if err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	eng.linkWorkspaceToMatchingPRs(ws)
	got, _ := st.PR(pr.ID)
	if got.WorkspaceID != ws.ID {
		t.Errorf("linkWorkspaceToMatchingPRs WorkspaceID = %q, want %q", got.WorkspaceID, ws.ID)
	}
}

func TestCreateWorkspaceFromPRLinksTrackingWorkspace(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	clone := initCloneTrackingRemoteBranch(t, "feat/oauth", "local-oauth")
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: clone, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: "ws-oauth", ProjectID: "acme", Branch: "local-oauth", WorktreePath: clone,
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-42", ProjectID: "acme", Number: 42, Source: domain.SourceGitHub,
		Title: "From GitHub", Branch: "feat/oauth", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	got, err := eng.CreateWorkspaceFromPR(pr.ID)
	if err != nil {
		t.Fatalf("CreateWorkspaceFromPR: %v", err)
	}
	if got.ID != "ws-oauth" {
		t.Errorf("CreateWorkspaceFromPR id = %q, want ws-oauth", got.ID)
	}
	linked, _ := st.PR(pr.ID)
	if linked.WorkspaceID != "ws-oauth" {
		t.Errorf("PR.WorkspaceID = %q, want ws-oauth", linked.WorkspaceID)
	}
}

func TestApplyListedGitHubPRRelinksArchivedWorkspace(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{ID: "ws-old", ProjectID: "acme", Branch: "feat/oauth", Archived: true}); err != nil {
		t.Fatalf("AddWorkspace old: %v", err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{ID: "ws-new", ProjectID: "acme", Branch: "feat/oauth"}); err != nil {
		t.Fatalf("AddWorkspace new: %v", err)
	}
	if _, err := st.AddPR(domain.PR{
		ID: "gh-acme-12", WorkspaceID: "ws-old", ProjectID: "acme", Number: 12,
		Source: domain.SourceGitHub, Title: "OAuth", Branch: "feat/oauth",
		URL: "https://github.com/acme/app/pull/12", Group: prGroupOpen, State: "Open",
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	_, created := eng.applyListedGitHubPR("acme", ghListedPR{
		Number: 12, Title: "OAuth", URL: "https://github.com/acme/app/pull/12",
		HeadRefName: "feat/oauth", BaseRefName: "main", State: "OPEN",
	}, "avery", time.Now())
	if created {
		t.Fatal("should update existing PR")
	}
	got, _ := st.PR("gh-acme-12")
	if got.WorkspaceID != "ws-new" {
		t.Errorf("WorkspaceID = %q, want ws-new (live match, not archived)", got.WorkspaceID)
	}
}

func TestCreateWorkspaceFromPRLinksExisting(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "existing", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-42", ProjectID: "acme", Number: 42, Source: domain.SourceGitHub,
		Title: "From GitHub", Branch: ws.Branch, Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	got, err := eng.CreateWorkspaceFromPR(pr.ID)
	if err != nil {
		t.Fatalf("CreateWorkspaceFromPR: %v", err)
	}
	if got.ID != ws.ID {
		t.Errorf("CreateWorkspaceFromPR id = %q, want existing %q", got.ID, ws.ID)
	}
	linked, _ := st.PR(pr.ID)
	if linked.WorkspaceID != ws.ID {
		t.Errorf("PR.WorkspaceID = %q, want %q", linked.WorkspaceID, ws.ID)
	}
}

func TestCreateWorkspaceFromPRHoistsHostChatWhenLinkingExisting(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-42", ProjectID: "acme", Number: 42, Source: domain.SourceGitHub,
		Title: "OAuth", Branch: "feat/oauth", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	sess, ok := st.SessionForLoopAgent(host.ID, "PA")
	if !ok {
		t.Fatal("EnsurePRChatHost did not create a PA session")
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: "ws-oauth", ProjectID: "acme", Branch: "feat/oauth", WorktreePath: repo,
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	got, err := eng.CreateWorkspaceFromPR(pr.ID)
	if err != nil {
		t.Fatalf("CreateWorkspaceFromPR: %v", err)
	}
	if got.ID != "ws-oauth" {
		t.Fatalf("CreateWorkspaceFromPR id = %q, want ws-oauth", got.ID)
	}
	moved, ok := st.SessionForLoopAgent("ws-oauth", "PA")
	if !ok || moved.ID != sess.ID {
		t.Fatalf("session on linked workspace = %+v ok=%v, want %s", moved, ok, sess.ID)
	}
	if _, still := st.SessionForLoopAgent(host.ID, "PA"); still {
		t.Error("host still has the PA session after hoist")
	}
	if _, ok := st.Agent("ws-oauth", "PA"); !ok {
		t.Error("PA agent missing on linked workspace after hoist")
	}
	archived, _ := st.Workspace(host.ID)
	if !archived.Archived {
		t.Error("host should be archived after linking the existing worktree")
	}
}

func TestSendWorkspaceMessageFollowsHoistedHostAlias(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("hoist-send-%d", call), "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-42s", ProjectID: "acme", Number: 42, Source: domain.SourceGitHub,
		Title: "OAuth", Branch: "feat/oauth", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: "ws-oauth", ProjectID: "acme", Branch: "feat/oauth", WorktreePath: repo,
	}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	got, err := eng.CreateWorkspaceFromPR(pr.ID)
	if err != nil {
		t.Fatalf("CreateWorkspaceFromPR: %v", err)
	}
	if got.ID != "ws-oauth" {
		t.Fatalf("CreateWorkspaceFromPR id = %q, want ws-oauth", got.ID)
	}
	reply := eng.SendWorkspaceMessage(host.ID, "PA", "what changed?", "sonnet", nil, nil, domain.ChatModeReview, "")
	if strings.Contains(reply, "can't find") || strings.Contains(reply, "Create a workspace") {
		t.Fatalf("send after hoist still targeted archived host: %q", reply)
	}
	if !strings.Contains(reply, "ok") {
		t.Fatalf("reply = %q, want dest turn", reply)
	}
	spec, ok := runner.specWithRole(domain.RoleCoder)
	if !ok || spec.WorkspaceID != "ws-oauth" || spec.AgentID != "ws-oauth-PA" {
		t.Fatalf("spec = %+v ok=%v, want dest ws-oauth-PA", spec, ok)
	}
}

func TestCreateWorkspaceFromPRReturnsLinked(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-7", WorkspaceID: ws.ID, ProjectID: "acme", Number: 7,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	got, err := eng.CreateWorkspaceFromPR(pr.ID)
	if err != nil {
		t.Fatalf("CreateWorkspaceFromPR: %v", err)
	}
	if got.ID != ws.ID {
		t.Errorf("id = %q, want %q", got.ID, ws.ID)
	}
}

func TestCreateWorkspaceFromPRUsesRemoteTrackingBase(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	clone := initCloneWithPullHead(t, 42, "feat/oauth")
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: clone, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-42", ProjectID: "acme", Number: 42, Source: domain.SourceGitHub,
		Title: "OAuth", Branch: "feat/oauth", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	got, err := eng.CreateWorkspaceFromPR(pr.ID)
	if err != nil {
		t.Fatalf("CreateWorkspaceFromPR: %v", err)
	}
	if got.BaseBranch != "origin/main" {
		t.Errorf("BaseBranch = %q, want origin/main (local tracking ref for GitHub base main)", got.BaseBranch)
	}
	linked, _ := st.PR(pr.ID)
	if linked.Base != "main" {
		t.Errorf("PR.Base = %q, want main (GitHub's short name)", linked.Base)
	}
}

func TestCreateWorkspaceFromPRLocalOnlyKeepsShortBase(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-7", ProjectID: "acme", Number: 7, Source: domain.SourceGitHub,
		Title: "Local only", Branch: "feat", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	got, err := eng.CreateWorkspaceFromPR(pr.ID)
	if err != nil {
		t.Fatalf("CreateWorkspaceFromPR: %v", err)
	}
	if got.BaseBranch != "main" {
		t.Errorf("BaseBranch = %q, want main (no remote-tracking ref)", got.BaseBranch)
	}
}

func TestLocalBaseFromGitHubPrefersProjectRemote(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	clone := initCloneWithUpstreamFork(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: clone, BaseBranch: "upstream/main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if got := eng.localBaseFromGitHub("acme", "main"); got != "upstream/main" {
		t.Errorf("localBaseFromGitHub(acme, main) = %q, want upstream/main", got)
	}
}

func TestLocalBaseFromGitHubKeepsProjectRemoteWhenUnfetched(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	clone := initCloneWithUpstreamFork(t)
	runGitIn(t, clone, "checkout", "-b", "release")
	runGitIn(t, clone, "push", "origin", "release")
	runGitIn(t, clone, "checkout", "main")
	runGitIn(t, clone, "branch", "-D", "release")
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: clone, BaseBranch: "upstream/main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if got := eng.localBaseFromGitHub("acme", "release"); got != "upstream/release" {
		t.Errorf("localBaseFromGitHub(acme, release) = %q, want upstream/release (not origin/release)", got)
	}
}

func TestLocalBaseFromGitHubSlashyGitHubBranch(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	clone := initCloneTrackingRemoteBranch(t, "origin/release", "origin/release")
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: clone, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if got := eng.localBaseFromGitHub("acme", "origin/release"); got != "origin/origin/release" {
		t.Errorf("localBaseFromGitHub(acme, origin/release) = %q, want origin/origin/release", got)
	}
}

func TestPrRepoPathUsesProjectID(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{ID: "orphan", ProjectID: "acme", Number: 1, Source: domain.SourceGitHub}
	if got := eng.prRepoPath(pr); got != repo {
		t.Errorf("prRepoPath = %q, want %q", got, repo)
	}
}

func TestGithubImportedPRID(t *testing.T) {
	t.Parallel()
	if got := githubImportedPRID("acme", 12); got != "gh-acme-12" {
		t.Errorf("githubImportedPRID = %q", got)
	}
}

func TestFindGitHubPRURLIsProjectScoped(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject acme: %v", err)
	}
	if _, err := st.AddProject(domain.Project{ID: "beta", Name: "beta", RepoPath: "/tmp/beta", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject beta: %v", err)
	}
	url := "https://github.com/acme/app/pull/12"
	if _, err := st.AddPR(domain.PR{
		ID: "gh-acme-12", ProjectID: "acme", Number: 12, Source: domain.SourceGitHub, URL: url,
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if _, ok := eng.findGitHubPR("beta", 12, url); ok {
		t.Fatal("findGitHubPR(beta, same URL) found acme's PR, want miss")
	}
	got, ok := eng.findGitHubPR("acme", 12, url)
	if !ok || got.ID != "gh-acme-12" {
		t.Fatalf("findGitHubPR(acme) = id=%q ok=%v, want gh-acme-12", got.ID, ok)
	}
}

func TestReconcileMissingGitHubPRsClosesUnseen(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := st.AddPR(domain.PR{
		ID: "gh-acme-99", ProjectID: "acme", Number: 99, Source: domain.SourceGitHub,
		Title: "Stale", Branch: "feat/x", Group: prGroupOpen, State: "Open",
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	orig := githubPRViewStateFn
	t.Cleanup(func() { githubPRViewStateFn = orig })
	githubPRViewStateFn = func(context.Context, string, int) (ghPRViewState, error) {
		return ghPRViewState{State: "CLOSED", UpdatedAt: "2026-08-28T15:04:00Z"}, nil
	}
	eng.reconcileMissingGitHubPRs(context.Background(), "acme", "/tmp/acme", map[int]struct{}{}, false)
	got, ok := st.PR("gh-acme-99")
	if !ok {
		t.Fatal("reconcileMissingGitHubPRs deleted gh-acme-99, want Closed")
	}
	if got.State != "Closed" || got.Group != prGroupMerged {
		t.Errorf("reconcileMissingGitHubPRs state=%q group=%q, want Closed/merged", got.State, got.Group)
	}
	if got.UpdatedAt != "2026-08-28T15:04:00Z" {
		t.Errorf("reconcileMissingGitHubPRs UpdatedAt = %q, want GitHub closed time", got.UpdatedAt)
	}
}

func TestReconcileMissingGitHubPRsMarksMerged(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := st.AddPR(domain.PR{
		ID: "local-418", ProjectID: "acme", Number: 418, Source: domain.SourceGitHub,
		Title: "Recur opened", Branch: "feat/x", Group: prGroupYou, State: "In review",
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	orig := githubPRViewStateFn
	t.Cleanup(func() { githubPRViewStateFn = orig })
	githubPRViewStateFn = func(context.Context, string, int) (ghPRViewState, error) {
		return ghPRViewState{State: "MERGED"}, nil
	}
	eng.reconcileMissingGitHubPRs(context.Background(), "acme", "/tmp/acme", map[int]struct{}{}, false)
	got, ok := st.PR("local-418")
	if !ok {
		t.Fatal("reconcileMissingGitHubPRs deleted Recur-owned PR local-418")
	}
	if got.State != "Merged" {
		t.Errorf("reconcileMissingGitHubPRs Recur-owned state=%q, want Merged", got.State)
	}
}

func TestReconcileMissingGitHubPRsMarksMergedLiveWorkspace(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := st.AddWorkspace(domain.Workspace{ID: "ws-live", ProjectID: "acme", Branch: "feat/x"})
	if err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	const frozen = "2026-08-01T12:00:00Z"
	const mergedAt = "2026-08-20T09:00:00Z"
	if _, err := st.AddPR(domain.PR{
		ID: "gh-live-50", ProjectID: "acme", WorkspaceID: ws.ID, Number: 50,
		Source: domain.SourceGitHub, Title: "Linked", Branch: "feat/x",
		Group: prGroupYou, State: "In review",
		CreatedAt: frozen, UpdatedAt: frozen,
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	orig := githubPRViewStateFn
	t.Cleanup(func() { githubPRViewStateFn = orig })
	githubPRViewStateFn = func(context.Context, string, int) (ghPRViewState, error) {
		return ghPRViewState{State: "MERGED", UpdatedAt: mergedAt}, nil
	}
	eng.reconcileMissingGitHubPRs(context.Background(), "acme", "/tmp/acme", map[int]struct{}{}, false)
	got, ok := st.PR("gh-live-50")
	if !ok {
		t.Fatal("reconcileMissingGitHubPRs deleted gh-live-50")
	}
	if got.State != "Merged" || got.Group != prGroupMerged {
		t.Errorf("live workspace PR state=%q group=%q, want Merged/merged", got.State, got.Group)
	}
	if got.UpdatedAt != mergedAt {
		t.Errorf("live workspace PR UpdatedAt = %q, want GitHub merge time %q", got.UpdatedAt, mergedAt)
	}
	loop, ok := st.Workspace(ws.ID)
	if !ok {
		t.Fatal("workspace missing")
	}
	if loop.State != domain.StateMerged {
		t.Errorf("workspace State = %q, want merged", loop.State)
	}
}

func TestReconcileMissingGitHubPRsSkipsWhenIncomplete(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := st.AddPR(domain.PR{
		ID: "gh-acme-99", ProjectID: "acme", Number: 99, Source: domain.SourceGitHub,
		Title: "Stale", Branch: "feat/x", Group: prGroupOpen, State: "Open",
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	eng.reconcileMissingGitHubPRs(context.Background(), "acme", "/tmp/acme", map[int]struct{}{}, true)
	if _, ok := st.PR("gh-acme-99"); !ok {
		t.Fatal("reconcileMissingGitHubPRs(skip=true) deleted gh-acme-99, want kept")
	}
}

func TestCheckoutPRHeadNameSkipsBaseAndForks(t *testing.T) {
	t.Parallel()
	if got := checkoutPRHeadName("feat/oauth", "main", "main", false); got != "feat/oauth" {
		t.Errorf("checkoutPRHeadName(feat/oauth) = %q, want feat/oauth", got)
	}
	if got := checkoutPRHeadName("main", "develop", "main", false); got != "" {
		t.Errorf("checkoutPRHeadName(main vs project base) = %q, want empty", got)
	}
	if got := checkoutPRHeadName("main", "main", "main", false); got != "" {
		t.Errorf("checkoutPRHeadName(main vs PR base) = %q, want empty", got)
	}
	if got := checkoutPRHeadName("feat/oauth", "main", "main", true); got != "" {
		t.Errorf("checkoutPRHeadName(cross-repo) = %q, want empty", got)
	}
}

func TestLinkUnlinkedGitHubPRsSkipsMerged(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: "/tmp/acme", BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	if _, err := st.AddPR(domain.PR{
		ID: "gh-acme-12", ProjectID: "acme", Number: 12, Source: domain.SourceGitHub,
		Title: "Old", Branch: "feat/x", Group: prGroupMerged, State: "Merged",
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if _, err := st.AddWorkspace(domain.Workspace{ID: "ws-new", ProjectID: "acme", Branch: "feat/x"}); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	eng.linkUnlinkedGitHubPRs()
	got, _ := st.PR("gh-acme-12")
	if got.WorkspaceID != "" {
		t.Errorf("linkUnlinkedGitHubPRs attached merged PR to %q, want empty", got.WorkspaceID)
	}
}

func TestCreateWorkspaceFromPRReusesInFlight(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-42", ProjectID: "acme", Number: 42, Source: domain.SourceGitHub,
		Title: "OAuth", Branch: "feat/oauth", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	rec, err := eng.persistCreatingWorkspace("acme", "OAuth", "main")
	if err != nil {
		t.Fatalf("persistCreatingWorkspace: %v", err)
	}
	eng.prHeadCheckout.Store(rec.ID, prHeadCheckoutSpec{Number: 42, Branch: "feat/oauth", PRID: pr.ID})
	got, err := eng.CreateWorkspaceFromPR(pr.ID)
	if err != nil {
		t.Fatalf("CreateWorkspaceFromPR: %v", err)
	}
	if got.ID != rec.ID {
		t.Errorf("CreateWorkspaceFromPR while in-flight id = %q, want %q", got.ID, rec.ID)
	}
	n := 0
	for _, w := range st.Workspaces() {
		if !w.Archived && w.ProjectID == "acme" {
			n++
		}
	}
	if n != 1 {
		t.Errorf("CreateWorkspaceFromPR while in-flight workspace count = %d, want 1", n)
	}
}

func TestCreateWorkspaceFromPRChecksOutNamedHead(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	clone := initCloneWithPullHead(t, 42, "feat/oauth")
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: clone, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-42", ProjectID: "acme", Number: 42, Source: domain.SourceGitHub,
		Title: "OAuth", Branch: "feat/oauth", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	rec, err := eng.persistCreatingWorkspace("acme", "OAuth", "main")
	if err != nil {
		t.Fatalf("persistCreatingWorkspace: %v", err)
	}
	eng.prHeadCheckout.Store(rec.ID, prHeadCheckoutSpec{Number: 42, Branch: "feat/oauth", PRID: pr.ID})
	if err := eng.allocateWorkspaceWorktree(context.Background(), rec); err != nil {
		t.Fatalf("allocateWorkspaceWorktree: %v", err)
	}
	ws, ok := st.Workspace(rec.ID)
	if !ok {
		t.Fatal("workspace missing after allocate")
	}
	if ws.Branch != "feat/oauth" {
		t.Errorf("Workspace.Branch = %q, want feat/oauth", ws.Branch)
	}
	if ws.WorktreePath == "" {
		t.Error("WorktreePath is empty after allocate")
	}
	linked, _ := st.PR(pr.ID)
	if linked.WorkspaceID != rec.ID {
		t.Errorf("PR.WorkspaceID = %q, want %q", linked.WorkspaceID, rec.ID)
	}
}

func initCloneWithPullHead(t *testing.T, number int, branch string) string {
	t.Helper()
	seed := initRepo(t)
	runGitIn(t, seed, "checkout", "-b", branch)
	if err := os.WriteFile(filepath.Join(seed, "pr.txt"), []byte("pr head\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGitIn(t, seed, "add", ".")
	runGitIn(t, seed, "commit", "-m", "pr head")
	sha := strings.TrimSpace(gitHeadSHA(t, seed))
	runGitIn(t, seed, "checkout", "main")
	bare := filepath.Join(t.TempDir(), "upstream.git")
	runGitIn(t, t.TempDir(), "clone", "--bare", seed, bare)
	clone := filepath.Join(t.TempDir(), "work")
	runGitIn(t, t.TempDir(), "clone", bare, clone)
	runGitIn(t, clone, "push", "origin", sha+":refs/pull/"+strconv.Itoa(number)+"/head")
	return clone
}

func gitHeadSHA(t *testing.T, dir string) string {
	t.Helper()
	cmd := exec.Command("git", "-C", dir, "rev-parse", "HEAD")
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("git rev-parse HEAD: %v\n%s", err, out)
	}
	return string(out)
}

// initCloneTrackingRemoteBranch is a clone whose local branch tracks origin/<remoteBranch>
// under a different name — the case PR linking used to miss.
func initCloneTrackingRemoteBranch(t *testing.T, remoteBranch, localBranch string) string {
	t.Helper()
	seed := initRepo(t)
	if remoteBranch != "main" {
		runGitIn(t, seed, "checkout", "-b", remoteBranch)
		if err := os.WriteFile(filepath.Join(seed, "on-remote.txt"), []byte("on remote\n"), 0o644); err != nil {
			t.Fatal(err)
		}
		runGitIn(t, seed, "add", ".")
		runGitIn(t, seed, "commit", "-m", "on "+remoteBranch)
		runGitIn(t, seed, "checkout", "main")
	}
	bare := filepath.Join(t.TempDir(), "upstream.git")
	runGitIn(t, t.TempDir(), "clone", "--bare", seed, bare)
	clone := filepath.Join(t.TempDir(), "work")
	runGitIn(t, t.TempDir(), "clone", bare, clone)
	if localBranch != remoteBranch {
		runGitIn(t, clone, "checkout", "-b", localBranch, "--track", "origin/"+remoteBranch)
	}
	return clone
}

// initCloneWithUpstreamFork is a clone with origin and upstream, both carrying main.
func initCloneWithUpstreamFork(t *testing.T) string {
	t.Helper()
	seed := initRepo(t)
	originBare := filepath.Join(t.TempDir(), "origin.git")
	upstreamBare := filepath.Join(t.TempDir(), "upstream.git")
	runGitIn(t, t.TempDir(), "clone", "--bare", seed, originBare)
	runGitIn(t, t.TempDir(), "clone", "--bare", seed, upstreamBare)
	clone := filepath.Join(t.TempDir(), "work")
	runGitIn(t, t.TempDir(), "clone", originBare, clone)
	runGitIn(t, clone, "remote", "add", "upstream", upstreamBare)
	runGitIn(t, clone, "fetch", "upstream")
	return clone
}

func TestEnsurePRChatHostCreatesHiddenWorkspace(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-9", ProjectID: "acme", Number: 9, Source: domain.SourceGitHub,
		Title: "Imported PR", Branch: "feat/import", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	if !host.IsPRChatHost() || host.HostPRID != pr.ID {
		t.Errorf("host HostPRID = %q, want %q", host.HostPRID, pr.ID)
	}
	if host.WorktreePath != repo {
		t.Errorf("host WorktreePath = %q, want project checkout %q", host.WorktreePath, repo)
	}
	linked, _ := st.PR(pr.ID)
	if linked.WorkspaceID != "" {
		t.Errorf("PR.WorkspaceID = %q, want empty until CreateWorkspaceFromPR", linked.WorkspaceID)
	}
	again, err := eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost second call: %v", err)
	}
	if again.ID != host.ID {
		t.Errorf("EnsurePRChatHost reused id = %q, want %q", again.ID, host.ID)
	}
	if n := len(st.Workspaces()); n != 1 {
		t.Errorf("workspace count = %d, want 1 host", n)
	}
	tab := eng.CreateWorkspaceChat(host.ID)
	if tab.Mode != domain.ChatModeReview {
		t.Errorf("CreateWorkspaceChat(host).Mode = %q, want review", tab.Mode)
	}
	if _, _, mode := eng.WorkspaceChatSelection(host.ID, tab.Code); mode != domain.ChatModeReview {
		t.Errorf("WorkspaceChatSelection(host, %s) mode = %q, want review", tab.Code, mode)
	}
	var remaining []domain.WorkspaceChatTab
	for _, c := range eng.ListWorkspaceChats(host.ID) {
		remaining = eng.CloseWorkspaceChat(host.ID, c.Code)
	}
	if len(remaining) != 1 {
		t.Fatalf("after closing all tabs: %d tabs, want 1 replacement", len(remaining))
	}
	if _, _, mode := eng.WorkspaceChatSelection(host.ID, remaining[0].Code); mode != domain.ChatModeReview {
		t.Errorf("replacement tab %s mode = %q, want review", remaining[0].Code, mode)
	}
}

func TestEnsurePRChatHostReturnsLinkedWorkspace(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-8", WorkspaceID: ws.ID, ProjectID: "acme", Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	got, err := eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	if got.ID != ws.ID {
		t.Errorf("EnsurePRChatHost id = %q, want linked %q", got.ID, ws.ID)
	}
	if got.IsPRChatHost() {
		t.Error("linked workspace should not be marked as a PR chat host")
	}
}

func TestEnsurePRChatHostHoistsLeftoverHostWhenLinked(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-8", WorkspaceID: ws.ID, ProjectID: "acme", Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host", ProjectID: "acme", Goal: "Already linked", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	st.UpsertAgent(host.ID, domain.Agent{ID: "a-pa", Code: "PA", Name: "Agent"})
	got, err := eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	if got.ID != ws.ID {
		t.Fatalf("EnsurePRChatHost id = %q, want linked %q", got.ID, ws.ID)
	}
	moved, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok || moved.ID != sess.ID {
		t.Fatalf("session on linked workspace = %+v ok=%v, want %s", moved, ok, sess.ID)
	}
	archived, _ := st.Workspace(host.ID)
	if !archived.Archived {
		t.Error("leftover host should be archived after opening the linked PR")
	}
}

func TestEnsurePRChatHostHoistMergesPlaceholderPA(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.UpsertAgent(ws.ID, domain.Agent{ID: ws.ID + "-PA", WorkspaceID: ws.ID, Code: "PA", Name: "Agent", Status: "idle"})
	pr := domain.PR{
		ID: "gh-acme-8b", WorkspaceID: ws.ID, ProjectID: "acme", Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host-merge", ProjectID: "acme", Goal: "Already linked", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	if _, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	}); err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	st.UpsertAgent(host.ID, domain.Agent{
		ID: "a-pa", Code: "PA", Name: "Agent", WorkspaceID: host.ID,
		SessionID: "prov-host", Unseen: true, Model: "sonnet", Status: "idle",
	})
	if _, err := eng.EnsurePRChatHost(pr.ID); err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	got, ok := st.Agent(ws.ID, "PA")
	if !ok {
		t.Fatal("linked PA agent missing after hoist")
	}
	if got.SessionID != "prov-host" {
		t.Errorf("linked PA SessionID = %q, want prov-host", got.SessionID)
	}
	if got.ID != ws.ID+"-PA" {
		t.Errorf("linked PA ID = %q, want dest identity", got.ID)
	}
	if !got.Unseen {
		t.Error("linked PA Unseen = false, want true")
	}
}

func TestEnsurePRChatHostDefersHoistWhileChatLive(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-8c", WorkspaceID: ws.ID, ProjectID: "acme", Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host-live", ProjectID: "acme", Goal: "Already linked", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	st.UpsertAgent(host.ID, domain.Agent{ID: "a-pa", Code: "PA", Name: "Agent"})
	eng.chats.Store(chatTurnKey(host.ID, "PA"), &chatTurn{})
	got, err := eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	if got.ID != host.ID {
		t.Fatalf("EnsurePRChatHost id = %q, want leftover host %q while chat is live", got.ID, host.ID)
	}
	still, _ := st.Workspace(host.ID)
	if still.Archived {
		t.Fatal("leftover host archived while chat is live")
	}
	if _, ok := st.SessionForLoopAgent(host.ID, "PA"); !ok {
		t.Fatal("session left the host while chat is live")
	}
	eng.chats.Delete(chatTurnKey(host.ID, "PA"))
	got, err = eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost after idle: %v", err)
	}
	if got.ID != ws.ID {
		t.Fatalf("EnsurePRChatHost after idle id = %q, want linked %q", got.ID, ws.ID)
	}
	moved, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok || moved.ID != sess.ID {
		t.Fatalf("session on linked workspace = %+v ok=%v, want %s", moved, ok, sess.ID)
	}
	archived, _ := st.Workspace(host.ID)
	if !archived.Archived {
		t.Error("leftover host should be archived once the chat is idle")
	}
}

func TestEnsurePRChatHostDefersHoistWhileDestChatLive(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	pr := domain.PR{
		ID: "gh-acme-8d", WorkspaceID: ws.ID, ProjectID: "acme", Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host-dest-live", ProjectID: "acme", Goal: "Already linked", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	st.UpsertAgent(host.ID, domain.Agent{ID: "a-pa", Code: "PA", Name: "Agent"})
	eng.chats.Store(chatTurnKey(ws.ID, "PA"), &chatTurn{})
	got, err := eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	if got.ID != host.ID {
		t.Fatalf("EnsurePRChatHost id = %q, want leftover host %q while dest chat is live", got.ID, host.ID)
	}
	still, _ := st.Workspace(host.ID)
	if still.Archived {
		t.Fatal("leftover host archived while dest chat is live")
	}
	if _, ok := st.SessionForLoopAgent(host.ID, "PA"); !ok {
		t.Fatal("session left the host while dest chat is live")
	}
	eng.chats.Delete(chatTurnKey(ws.ID, "PA"))
	got, err = eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost after dest idle: %v", err)
	}
	if got.ID != ws.ID {
		t.Fatalf("EnsurePRChatHost after dest idle id = %q, want linked %q", got.ID, ws.ID)
	}
	moved, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok || moved.ID != sess.ID {
		t.Fatalf("session on linked workspace = %+v ok=%v, want %s", moved, ok, sess.ID)
	}
	archived, _ := st.Workspace(host.ID)
	if !archived.Archived {
		t.Error("leftover host should be archived once the dest chat is idle")
	}
}

func TestEnsurePRChatHostHoistRemapsCollidingTranscripts(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	liveSess, err := st.CreateSession(domain.Session{
		WorkspaceID: ws.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession live: %v", err)
	}
	if _, err := st.AppendMessage(liveSess.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("live chat")},
	}); err != nil {
		t.Fatalf("AppendMessage live: %v", err)
	}
	st.UpsertAgent(ws.ID, domain.Agent{
		ID: ws.ID + "-PA", WorkspaceID: ws.ID, Code: "PA", SessionID: "prov-live",
	})
	pr := domain.PR{
		ID: "gh-acme-8d", WorkspaceID: ws.ID, ProjectID: "acme", Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host-collide", ProjectID: "acme", Goal: "Already linked", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	hostSess, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession host: %v", err)
	}
	if _, err := st.AppendMessage(hostSess.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("host chat")},
	}); err != nil {
		t.Fatalf("AppendMessage host: %v", err)
	}
	st.UpsertAgent(host.ID, domain.Agent{
		ID: "a-pa", Code: "PA", Name: "Agent", WorkspaceID: host.ID, SessionID: "prov-host",
	})
	if _, err := eng.EnsurePRChatHost(pr.ID); err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	gotLive, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok || gotLive.ID != liveSess.ID {
		t.Fatalf("dest PA session = %+v ok=%v, want %s", gotLive, ok, liveSess.ID)
	}
	gotHost, ok := st.SessionForLoopAgent(ws.ID, "PA2")
	if !ok || gotHost.ID != hostSess.ID {
		t.Fatalf("hoisted PA2 session = %+v ok=%v, want %s", gotHost, ok, hostSess.ID)
	}
	liveAg, ok := st.Agent(ws.ID, "PA")
	if !ok || liveAg.SessionID != "prov-live" {
		t.Fatalf("dest PA agent = %+v ok=%v, want prov-live", liveAg, ok)
	}
	hostAg, ok := st.Agent(ws.ID, "PA2")
	if !ok || hostAg.SessionID != "prov-host" {
		t.Fatalf("hoisted PA2 agent = %+v ok=%v, want prov-host", hostAg, ok)
	}
	if hostAg.ID != ws.ID+"-PA2" || hostAg.WorkspaceID != ws.ID {
		t.Errorf("hoisted agent identity = %s/%s, want dest rewrite", hostAg.ID, hostAg.WorkspaceID)
	}
	if id, code := eng.resolveHoistedChat(host.ID, "PA"); id != ws.ID || code != "PA2" {
		t.Fatalf("resolveHoistedChat = %s/%s, want %s/PA2", id, code, ws.ID)
	}
}

func TestDropContinueGenRetriesPRHostHoist(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	pr := domain.PR{
		ID: "gh-acme-8m", WorkspaceID: ws.ID, ProjectID: "acme", Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host-continue", ProjectID: "acme", Goal: "Already linked", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	st.UpsertAgent(host.ID, domain.Agent{ID: "a-pa", Code: "PA", Name: "Agent"})
	gen := eng.allocContinueGen(host.ID, "PA")
	got, err := eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	if got.ID != host.ID {
		t.Fatalf("EnsurePRChatHost id = %q, want leftover host %q while continue gen is live", got.ID, host.ID)
	}
	eng.dropContinueGen(gen)
	moved, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok || moved.ID != sess.ID {
		t.Fatalf("session on linked workspace = %+v ok=%v, want %s", moved, ok, sess.ID)
	}
	archived, _ := st.Workspace(host.ID)
	if !archived.Archived {
		t.Error("leftover host should be archived when the switch-mode gen drops")
	}
}

func TestDropContinueGenDefersHoistWhileChatLifeHeld(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	pr := domain.PR{
		ID: "gh-acme-8m2", WorkspaceID: ws.ID, ProjectID: "acme", Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host-continue-life", ProjectID: "acme", Goal: "Already linked", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	st.UpsertAgent(host.ID, domain.Agent{ID: "a-pa", Code: "PA", Name: "Agent"})
	unlock, held := eng.lockChatLife(host.ID, "PA")
	if !held {
		t.Fatal("lockChatLife refused the leftover host")
	}
	gen := eng.allocContinueGen(host.ID, "PA")
	done := make(chan struct{})
	go func() {
		eng.dropContinueGen(gen)
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(time.Second):
		unlock()
		t.Fatal("dropContinueGen deadlocked while lockChatLife held")
	}
	still, _ := st.Workspace(host.ID)
	if still.Archived {
		unlock()
		t.Fatal("host archived while lockChatLife held")
	}
	unlock()
	moved, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok || moved.ID != sess.ID {
		t.Fatalf("session on linked workspace = %+v ok=%v, want %s", moved, ok, sess.ID)
	}
	archived, _ := st.Workspace(host.ID)
	if !archived.Archived {
		t.Fatal("host should archive once lockChatLife released")
	}
}

func TestModeSwitchContinueSetupFailureRetriesPendingHoist(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	eng.MCP().Addr = deadMCPAddr
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	pr := domain.PR{
		ID: "gh-acme-8m3", WorkspaceID: ws.ID, ProjectID: "acme", Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host-continue-setup", ProjectID: "acme", Goal: "Already linked", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	st.UpsertAgent(host.ID, domain.Agent{ID: "a-pa", Code: "PA", Name: "Agent"})
	gen := eng.allocContinueGen(host.ID, "PA")
	done := make(chan struct{})
	go func() {
		eng.resumeAfterModeSwitch(host.ID, "PA", domain.ChatModeAgent, domain.ChatModeAsk, gen)
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("mode-switch continuation deadlocked on hoist while lockChatLife held")
	}
	moved, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok || moved.ID != sess.ID {
		t.Fatalf("session on linked workspace = %+v ok=%v, want %s", moved, ok, sess.ID)
	}
	archived, _ := st.Workspace(host.ID)
	if !archived.Archived {
		t.Fatal("host should archive after continuation setup fails")
	}
}

func TestEnsurePRChatHostHoistMovesPlanAndFindings(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-8n", WorkspaceID: ws.ID, ProjectID: "acme", Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host-plan", ProjectID: "acme", Goal: "Already linked", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
		DefinitionOfDone: "Ship the toggle",
		Plan: &domain.Plan{
			Summary:  "Dark mode",
			Body:     "Touch Settings.svelte",
			ChatCode: "PA",
			Steps:    []domain.PlanStep{{ID: "s1", Title: "Add the toggle", Status: domain.StepPending}},
		},
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	st.AddFinding(domain.Finding{
		ID: "F-host-1", WorkspaceID: host.ID, File: "Settings.svelte", Line: 12,
		Severity: "medium", Message: "contrast", Status: string(domain.FindingOpen),
	})
	st.UpsertAgent(host.ID, domain.Agent{ID: "a-pa", Code: "PA", Name: "Agent"})
	if _, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	}); err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	if _, err := eng.EnsurePRChatHost(pr.ID); err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	got, ok := st.Workspace(ws.ID)
	if !ok {
		t.Fatal("dest workspace missing")
	}
	if got.Plan == nil || got.Plan.Summary != "Dark mode" || got.Plan.ChatCode != "PA" {
		t.Fatalf("dest plan = %+v, want host plan on PA", got.Plan)
	}
	if got.DefinitionOfDone != "Ship the toggle" {
		t.Errorf("dest DefinitionOfDone = %q, want Ship the toggle", got.DefinitionOfDone)
	}
	findings := st.Findings(ws.ID)
	if len(findings) != 1 || findings[0].ID != "F-host-1" || findings[0].WorkspaceID != ws.ID {
		t.Fatalf("dest findings = %+v, want moved F-host-1", findings)
	}
	if n := st.OpenFindingsCount(host.ID); n != 0 {
		t.Errorf("host OpenFindingsCount = %d, want 0", n)
	}
	foundFindings := false
	for _, ev := range sink.byName("findings.updated") {
		d, _ := ev.Data.(map[string]any)
		if d["workspaceId"] == ws.ID {
			foundFindings = true
			break
		}
	}
	if !foundFindings {
		t.Error("hoist did not publish findings.updated for dest")
	}
	foundChats := false
	for _, ev := range sink.byName("workspace.updated") {
		d, _ := ev.Data.(map[string]any)
		if d["workspaceId"] == ws.ID && d["chatsChanged"] == true && d["fromWorkspaceId"] == host.ID {
			foundChats = true
			break
		}
	}
	if !foundChats {
		t.Error("hoist did not publish dest workspace.updated with chatsChanged")
	}
}

func TestEnsurePRChatHostHoistTransfersHostFocus(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	liveSess, err := st.CreateSession(domain.Session{
		WorkspaceID: ws.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession live: %v", err)
	}
	if _, err := st.AppendMessage(liveSess.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("live chat")},
	}); err != nil {
		t.Fatalf("AppendMessage live: %v", err)
	}
	st.UpsertAgent(ws.ID, domain.Agent{
		ID: ws.ID + "-PA", WorkspaceID: ws.ID, Code: "PA", SessionID: "prov-live",
	})
	pr := domain.PR{
		ID: "gh-acme-8o", WorkspaceID: ws.ID, ProjectID: "acme", Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host-focus", ProjectID: "acme", Goal: "Already linked", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	hostSess, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession host: %v", err)
	}
	if _, err := st.AppendMessage(hostSess.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("host chat")},
	}); err != nil {
		t.Fatalf("AppendMessage host: %v", err)
	}
	st.UpsertAgent(host.ID, domain.Agent{
		ID: "a-pa", Code: "PA", Name: "Agent", WorkspaceID: host.ID, SessionID: "prov-host",
	})
	eng.SetWorkspaceChatFocus(host.ID, "PA")
	if _, err := eng.EnsurePRChatHost(pr.ID); err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	if !eng.isChatFocused(ws.ID, "PA2") {
		t.Fatal("isChatFocused(dest, PA2) = false, want remapped host focus")
	}
	foundFocus := false
	for _, ev := range sink.byName("workspace.updated") {
		d, _ := ev.Data.(map[string]any)
		if d["workspaceId"] == ws.ID && d["focusChatCode"] == "PA2" {
			foundFocus = true
			break
		}
	}
	if !foundFocus {
		t.Error("hoist did not publish dest workspace.updated with focusChatCode=PA2")
	}
}

func TestEnsurePRChatHostHoistReservesSourceCodes(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	liveSess, err := st.CreateSession(domain.Session{
		WorkspaceID: ws.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession live: %v", err)
	}
	if _, err := st.AppendMessage(liveSess.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("live chat")},
	}); err != nil {
		t.Fatalf("AppendMessage live: %v", err)
	}
	st.UpsertAgent(ws.ID, domain.Agent{
		ID: ws.ID + "-PA", WorkspaceID: ws.ID, Code: "PA", SessionID: "prov-live",
	})
	pr := domain.PR{
		ID: "gh-acme-8e", WorkspaceID: ws.ID, ProjectID: "acme", Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host-src-codes", ProjectID: "acme", Goal: "Already linked", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	hostPA, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession host PA: %v", err)
	}
	if _, err := st.AppendMessage(hostPA.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("host PA")},
	}); err != nil {
		t.Fatalf("AppendMessage host PA: %v", err)
	}
	hostPA2, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA2"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession host PA2: %v", err)
	}
	if _, err := st.AppendMessage(hostPA2.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("host PA2")},
	}); err != nil {
		t.Fatalf("AppendMessage host PA2: %v", err)
	}
	st.UpsertAgent(host.ID, domain.Agent{
		ID: "a-pa", Code: "PA", WorkspaceID: host.ID, SessionID: "prov-host-pa",
	})
	st.UpsertAgent(host.ID, domain.Agent{
		ID: "a-pa2", Code: "PA2", WorkspaceID: host.ID, SessionID: "prov-host-pa2",
	})
	if _, err := eng.EnsurePRChatHost(pr.ID); err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	gotLive, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok || gotLive.ID != liveSess.ID {
		t.Fatalf("dest PA session = %+v ok=%v, want %s", gotLive, ok, liveSess.ID)
	}
	gotPA2, ok := st.SessionForLoopAgent(ws.ID, "PA2")
	if !ok || gotPA2.ID != hostPA2.ID {
		t.Fatalf("source PA2 session = %+v ok=%v, want %s", gotPA2, ok, hostPA2.ID)
	}
	gotPA3, ok := st.SessionForLoopAgent(ws.ID, "PA3")
	if !ok || gotPA3.ID != hostPA.ID {
		t.Fatalf("hoisted PA3 session = %+v ok=%v, want %s", gotPA3, ok, hostPA.ID)
	}
	if ag, ok := st.Agent(ws.ID, "PA2"); !ok || ag.SessionID != "prov-host-pa2" {
		t.Fatalf("source PA2 agent = %+v ok=%v, want prov-host-pa2", ag, ok)
	}
	if ag, ok := st.Agent(ws.ID, "PA3"); !ok || ag.SessionID != "prov-host-pa" {
		t.Fatalf("hoisted PA3 agent = %+v ok=%v, want prov-host-pa", ag, ok)
	}
	var moved []string
	remapPA := ""
	for _, ev := range sink.byName("workspace.updated") {
		d, _ := ev.Data.(map[string]any)
		if d["workspaceId"] != ws.ID || d["chatsChanged"] != true || d["fromWorkspaceId"] != host.ID {
			continue
		}
		if raw, ok := d["codeRemap"].(map[string]string); ok {
			remapPA = raw["PA"]
		}
		switch got := d["movedCodes"].(type) {
		case []string:
			moved = got
		case []any:
			for _, v := range got {
				if s, ok := v.(string); ok {
					moved = append(moved, s)
				}
			}
		}
		break
	}
	if remapPA != "PA3" {
		t.Errorf("codeRemap[PA] = %q, want PA3", remapPA)
	}
	has := map[string]bool{}
	for _, c := range moved {
		has[c] = true
	}
	if !has["PA2"] || !has["PA3"] {
		t.Errorf("movedCodes = %v, want PA2 and PA3", moved)
	}
}

func TestEnsurePRChatHostHoistWaitsForChatLife(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-8f", WorkspaceID: ws.ID, ProjectID: "acme", Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host-wait", ProjectID: "acme", Goal: "Already linked", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	st.UpsertAgent(host.ID, domain.Agent{ID: "a-pa", Code: "PA", Name: "Agent"})
	_ = eng.ListWorkspaceChats(ws.ID)
	unlock, held := eng.lockChatLife(host.ID, "PA")
	if !held {
		t.Fatal("lockChatLife refused the leftover host")
	}
	done := make(chan error, 1)
	go func() {
		_, err := eng.EnsurePRChatHost(pr.ID)
		done <- err
	}()
	select {
	case err := <-done:
		unlock()
		t.Fatalf("EnsurePRChatHost returned while lockChatLife held: %v", err)
	case <-time.After(50 * time.Millisecond):
	}
	still, _ := st.Workspace(host.ID)
	if still.Archived {
		unlock()
		t.Fatal("leftover host archived while lockChatLife held")
	}
	if tab := eng.CreateWorkspaceChat(ws.ID); tab.Code != "" {
		unlock()
		t.Fatalf("CreateWorkspaceChat during hoist = %q, want empty", tab.Code)
	}
	if tab := eng.ForkWorkspaceChat(ws.ID, "PA", "fork-during-hoist"); tab.Code != "" {
		unlock()
		t.Fatalf("ForkWorkspaceChat during hoist = %q, want empty", tab.Code)
	}
	if tabs := eng.CloseWorkspaceChat(ws.ID, "PA"); len(tabs) != 1 || tabs[0].Code != "PA" {
		unlock()
		t.Fatalf("CloseWorkspaceChat during hoist = %+v, want PA left in place", tabs)
	}
	eng.RememberWorkspaceChatSelection(ws.ID, "PA", "sonnet", nil, "agent", nil)
	if _, ok := st.SessionForLoopAgent(ws.ID, "PA"); ok {
		unlock()
		t.Fatal("RememberWorkspaceChatSelection created a dest session during hoist")
	}
	eng.RememberWorkspaceChatPromptGroup(ws.ID, "PA", "ops")
	if _, ok := st.SessionForLoopAgent(ws.ID, "PA"); ok {
		unlock()
		t.Fatal("RememberWorkspaceChatPromptGroup created a dest session during hoist")
	}
	eng.RememberChatLoopPolicy(ws.ID, "PA", "test.loops.converge", "")
	if _, ok := st.SessionForLoopAgent(ws.ID, "PA"); ok {
		unlock()
		t.Fatal("RememberChatLoopPolicy created a dest session during hoist")
	}
	unlock()
	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("EnsurePRChatHost: %v", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("EnsurePRChatHost blocked after lockChatLife unlock")
	}
	moved, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok || moved.ID != sess.ID {
		t.Fatalf("session on linked workspace = %+v ok=%v, want %s", moved, ok, sess.ID)
	}
	archived, _ := st.Workspace(host.ID)
	if !archived.Archived {
		t.Error("leftover host should be archived once lockChatLife released")
	}
}

func TestUnregisterChatTurnArchivesLinkedPRHost(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-8g", WorkspaceID: ws.ID, ProjectID: "acme", Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host-unreg", ProjectID: "acme", Goal: "Already linked", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	st.UpsertAgent(host.ID, domain.Agent{ID: "a-pa", Code: "PA", Name: "Agent"})
	turn := &chatTurn{cancel: func() {}}
	eng.chats.Store(chatTurnKey(host.ID, "PA"), turn)
	got, err := eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	if got.ID != host.ID {
		t.Fatalf("EnsurePRChatHost id = %q, want leftover host %q while chat is live", got.ID, host.ID)
	}
	eng.unregisterChatTurn(host.ID, "PA", turn)
	moved, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok || moved.ID != sess.ID {
		t.Fatalf("session on linked workspace = %+v ok=%v, want %s", moved, ok, sess.ID)
	}
	archived, _ := st.Workspace(host.ID)
	if !archived.Archived {
		t.Error("leftover host should be archived when the live turn unregisters")
	}
}

func TestUnregisterChatTurnArchivesLinkedPRHostFromDest(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	_ = eng.ListWorkspaceChats(ws.ID)
	pr := domain.PR{
		ID: "gh-acme-8j", WorkspaceID: ws.ID, ProjectID: "acme", Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host-dest-unreg", ProjectID: "acme", Goal: "Already linked", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	st.UpsertAgent(host.ID, domain.Agent{ID: "a-pa", Code: "PA", Name: "Agent"})
	turn := &chatTurn{cancel: func() {}}
	eng.chats.Store(chatTurnKey(ws.ID, "PA"), turn)
	got, err := eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	if got.ID != host.ID {
		t.Fatalf("EnsurePRChatHost id = %q, want leftover host %q while dest chat is live", got.ID, host.ID)
	}
	eng.unregisterChatTurn(ws.ID, "PA", turn)
	moved, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok || moved.ID != sess.ID {
		t.Fatalf("session on linked workspace = %+v ok=%v, want %s", moved, ok, sess.ID)
	}
	archived, _ := st.Workspace(host.ID)
	if !archived.Archived {
		t.Error("leftover host should be archived when the dest turn unregisters")
	}
}

func TestCloseWorkspaceChatDuringHoistDoesNotReseedSource(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-8k", WorkspaceID: ws.ID, ProjectID: "acme", Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host-reseed", ProjectID: "acme", Goal: "Already linked", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	st.UpsertAgent(host.ID, domain.Agent{ID: "a-pa", Code: "PA", Name: "Agent"})
	release, ok := eng.holdPRChatHoist(host.ID, ws.ID)
	if !ok {
		t.Fatal("holdPRChatHoist refused idle ids")
	}
	defer release()
	st.MoveWorkspaceAgents(host.ID, ws.ID, nil)
	if _, still := st.Agent(host.ID, "PA"); still {
		t.Fatal("MoveWorkspaceAgents left PA on the source")
	}
	eng.CloseWorkspaceChat(host.ID, "PA")
	if _, reseeded := st.Agent(host.ID, "PA"); reseeded {
		t.Fatal("CloseWorkspaceChat reseeded PA on a hoisting source")
	}
}

func TestUnregisterChatTurnArchivesSettledPRHost(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-8h", ProjectID: "acme", Number: 8, Source: domain.SourceGitHub,
		Title: "Done", Branch: "feat", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host-settled", ProjectID: "acme", Goal: "Done", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	st.UpsertAgent(host.ID, domain.Agent{ID: "a-pa", Code: "PA", Name: "Agent"})
	turn := &chatTurn{cancel: func() {}}
	eng.chats.Store(chatTurnKey(host.ID, "PA"), turn)
	st.UpdatePR(pr.ID, func(p *domain.PR) {
		p.State = "Merged"
		p.Group = prGroupMerged
	})
	eng.archivePRChatHosts(pr.ID, "")
	still, _ := st.Workspace(host.ID)
	if still.Archived {
		t.Fatal("settled host archived while chat is live")
	}
	eng.unregisterChatTurn(host.ID, "PA", turn)
	archived, _ := st.Workspace(host.ID)
	if !archived.Archived {
		t.Error("settled host should be archived once the live turn unregisters")
	}
	kept, ok := st.SessionForLoopAgent(host.ID, "PA")
	if !ok || kept.ID != sess.ID {
		t.Fatalf("settled archive moved session = %+v ok=%v, want %s on host", kept, ok, sess.ID)
	}
}

func TestUnregisterChatTurnKeepsOpenUnlinkedPRHost(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-8i", ProjectID: "acme", Number: 8, Source: domain.SourceGitHub,
		Title: "Open", Branch: "feat", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	turn := &chatTurn{cancel: func() {}}
	eng.chats.Store(chatTurnKey(host.ID, "PA"), turn)
	eng.unregisterChatTurn(host.ID, "PA", turn)
	still, _ := st.Workspace(host.ID)
	if still.Archived {
		t.Fatal("open unlinked PR host archived when its chat turn ended")
	}
	if got, err := eng.EnsurePRChatHost(pr.ID); err != nil || got.ID != host.ID {
		t.Fatalf("EnsurePRChatHost after unreg = %+v err=%v, want host %s", got, err, host.ID)
	}
}

func TestHoldPRChatHoistIsExclusive(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, nil)
	release, ok := eng.holdPRChatHoist("host-a", "keep")
	if !ok {
		t.Fatal("holdPRChatHoist refused idle ids")
	}
	started := make(chan struct{})
	done := make(chan struct{})
	heldCh := make(chan bool, 1)
	go func() {
		close(started)
		releaseB, held := eng.holdPRChatHoist("host-b", "keep")
		heldCh <- held
		releaseB()
		close(done)
	}()
	<-started
	select {
	case <-done:
		t.Fatal("second hold acquired keep while the first still held it")
	case <-time.After(40 * time.Millisecond):
	}
	if _, ok := eng.hoisting.Load("keep"); !ok {
		t.Fatal("keep tombstone missing during first hold")
	}
	if _, ok := eng.hoisting.Load("host-a"); !ok {
		t.Fatal("host-a tombstone missing during first hold")
	}
	release()
	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("second hold did not acquire after first release")
	}
	if _, ok := eng.hoisting.Load("keep"); ok {
		t.Fatal("keep tombstone leaked after both holds released")
	}
	if !<-heldCh {
		t.Error("holdPRChatHoist(host-b, keep) ok = false, want true after first release")
	}
}

func TestHoldPRChatHoistRefusesDeleting(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, nil)
	eng.deleting.Store("keep", struct{}{})
	release, ok := eng.holdPRChatHoist("host-a", "keep")
	release()
	if ok {
		t.Fatal("holdPRChatHoist acquired a deleting destination")
	}
	if _, busy := eng.hoisting.Load("host-a"); busy {
		t.Fatal("host-a tombstone leaked after refusing a deleting keep")
	}
}

func TestReserveDeletingRefusesHoist(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, nil)
	release, ok := eng.holdPRChatHoist("host-a", "keep")
	if !ok {
		t.Fatal("holdPRChatHoist refused idle ids")
	}
	if eng.reserveDeleting("keep") {
		t.Fatal("reserveDeleting acquired keep while a hoist still held it")
	}
	if _, busy := eng.deleting.Load("keep"); busy {
		t.Fatal("deleting tombstone set while hoist still held keep")
	}
	release()
	if !eng.reserveDeleting("keep") {
		t.Fatal("reserveDeleting ok = false, want true after hoist release")
	}
	if _, busy := eng.deleting.Load("keep"); !busy {
		t.Fatal("deleting tombstone missing after reserveDeleting")
	}
}

func TestReserveDeletingRefusesExisting(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, nil)
	if !eng.reserveDeleting("keep") {
		t.Fatal("first reserveDeleting refused idle id")
	}
	if eng.reserveDeleting("keep") {
		t.Fatal("second reserveDeleting acquired an already-deleting id")
	}
	if _, busy := eng.deleting.Load("keep"); !busy {
		t.Fatal("deleting tombstone cleared by the refused second acquire")
	}
	eng.deleting.Delete("keep")
	if !eng.reserveDeleting("keep") {
		t.Fatal("reserveDeleting refused after owner released")
	}
}

func TestDeleteWorkspaceRefusesExistingDeleting(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	addSoloWorkspace(t, st, "ws-busy")
	eng.deleting.Store("ws-busy", struct{}{})
	if err := eng.DeleteWorkspace("ws-busy"); err == nil || !strings.Contains(err.Error(), "being removed") {
		t.Fatalf("DeleteWorkspace: err=%v", err)
	}
	if _, ok := st.Workspace("ws-busy"); !ok {
		t.Fatal("store row should remain when reservation is refused")
	}
}

func TestDeleteWorkspaceRefusesHoistingDestination(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	addSoloWorkspace(t, st, "keep")
	release, ok := eng.holdPRChatHoist("host-a", "keep")
	if !ok {
		t.Fatal("holdPRChatHoist refused idle ids")
	}
	defer release()
	if err := eng.DeleteWorkspace("keep"); err == nil || !strings.Contains(err.Error(), "being removed") {
		t.Fatalf("DeleteWorkspace: err=%v", err)
	}
	if _, still := st.Workspace("keep"); !still {
		t.Fatal("store row should remain when a hoist covers the destination")
	}
}

func TestRemoveWorkspaceRefusesHoistingDestination(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	addSoloWorkspace(t, st, "keep")
	release, ok := eng.holdPRChatHoist("host-a", "keep")
	if !ok {
		t.Fatal("holdPRChatHoist refused idle ids")
	}
	defer release()
	if _, err := eng.RemoveWorkspace("keep"); err == nil || !strings.Contains(err.Error(), "being removed") {
		t.Fatalf("RemoveWorkspace: err=%v", err)
	}
	got, ok := st.Workspace("keep")
	if !ok {
		t.Fatal("store row should remain when a hoist covers the destination")
	}
	if got.Archived {
		t.Fatal("keep archived while a hoist still held it")
	}
}

func TestRemoveWorkspaceReservesBeforeWaitingCreate(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	addSoloWorkspace(t, st, "keep")
	addSoloWorkspace(t, st, "host")
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: "host", Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	done := make(chan struct{})
	closed := false
	closeDone := func() {
		if !closed {
			closed = true
			close(done)
		}
	}
	defer closeDone()
	eng.creating.Store("keep", &creatingAlloc{cancel: func() {}, done: done})
	errc := make(chan error, 1)
	go func() {
		_, err := eng.RemoveWorkspace("keep")
		errc <- err
	}()
	deadline := time.Now().Add(2 * time.Second)
	for {
		if _, busy := eng.deleting.Load("keep"); busy {
			break
		}
		select {
		case err := <-errc:
			t.Fatalf("RemoveWorkspace returned before create wait: %v", err)
		default:
		}
		if time.Now().After(deadline) {
			t.Fatal("RemoveWorkspace did not reserve deleting before waitAlloc")
		}
		time.Sleep(time.Millisecond)
	}
	eng.archiveOnePRChatHost("host", "keep")
	host, _ := st.Workspace("host")
	if host.Archived {
		t.Fatal("host archived onto a destination that is being removed")
	}
	if got, ok := st.SessionForLoopAgent("host", "PA"); !ok || got.ID != sess.ID {
		t.Fatalf("host session = %+v ok=%v, want %s still on host", got, ok, sess.ID)
	}
	closeDone()
	select {
	case err := <-errc:
		if err != nil {
			t.Fatalf("RemoveWorkspace: %v", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("RemoveWorkspace did not finish after create wait")
	}
	got, ok := st.Workspace("keep")
	if !ok || !got.Archived {
		t.Fatal("keep should be archived after RemoveWorkspace")
	}
}

func TestCreateWorkspaceFromPRPromotesChatHost(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	clone := initCloneWithPullHead(t, 42, "feat/oauth")
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: clone, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-42", ProjectID: "acme", Number: 42, Source: domain.SourceGitHub,
		Title: "OAuth", Branch: "feat/oauth", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	got, err := eng.CreateWorkspaceFromPR(pr.ID)
	if err != nil {
		t.Fatalf("CreateWorkspaceFromPR: %v", err)
	}
	if got.ID != host.ID {
		t.Fatalf("promoted id = %q, want host %q", got.ID, host.ID)
	}
	if host.BaseBranch != "origin/main" {
		t.Errorf("host BaseBranch = %q, want origin/main (local tracking ref, not GitHub's short name)", host.BaseBranch)
	}
	linked, _ := st.PR(pr.ID)
	if linked.WorkspaceID != "" {
		t.Fatalf("PR.WorkspaceID = %q immediately after promote, want empty until allocate", linked.WorkspaceID)
	}
	deadline := time.Now().Add(8 * time.Second)
	var ws domain.Workspace
	for time.Now().Before(deadline) {
		ws, _ = st.Workspace(host.ID)
		linked, _ = st.PR(pr.ID)
		if !ws.IsPRChatHost() && ws.WorktreePath != "" && ws.WorktreePath != clone && linked.WorkspaceID == host.ID {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}
	if ws.IsPRChatHost() {
		t.Errorf("HostPRID = %q after allocate, want empty", ws.HostPRID)
	}
	if ws.WorktreePath == "" || ws.WorktreePath == clone {
		t.Errorf("WorktreePath = %q, want a Recur worktree (not the project checkout)", ws.WorktreePath)
	}
	if linked.WorkspaceID != host.ID {
		t.Errorf("PR.WorkspaceID after allocate = %q, want %q", linked.WorkspaceID, host.ID)
	}
	if ws.BaseBranch != "origin/main" {
		t.Errorf("promoted BaseBranch = %q, want origin/main", ws.BaseBranch)
	}
	if n := len(st.Workspaces()); n != 1 {
		t.Errorf("workspace count = %d, want 1 (promoted in place)", n)
	}
}

func TestSendWorkspaceMessageRejectsAgentOnPRChatHost(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-3", ProjectID: "acme", Number: 3, Source: domain.SourceGitHub,
		Title: "Docs", Branch: "docs", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	reply := eng.SendWorkspaceMessage(host.ID, "PA", "edit the files", "sonnet", nil, nil, domain.ChatModeAgent, "")
	if !strings.Contains(reply, "Create a workspace") {
		t.Errorf("agent on host reply = %q, want create-workspace guidance", reply)
	}
	loop := eng.SendWorkspaceMessage(host.ID, "PA", "ship it", "sonnet", nil, nil, domain.ChatModeLoop, "")
	if !strings.Contains(loop, "Create a workspace") {
		t.Errorf("loop on host reply = %q, want create-workspace guidance", loop)
	}
}

func TestSendWorkspaceMessagePRChatHostInjectsGitHubSurface(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("rev-%d", call), "Reviewed.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-3", ProjectID: "acme", Number: 3, Source: domain.SourceGitHub,
		Title: "Docs", Branch: "docs", Base: "main", Group: prGroupOpen, State: "Open",
		URL: "https://github.com/acme/app/pull/3",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	_ = eng.SendWorkspaceMessage(host.ID, "PA", "what changed?", "sonnet", nil, nil, domain.ChatModeReview, "")
	specs := runner.specsWithRole(domain.RoleCoder)
	if len(specs) == 0 {
		t.Fatal("no review turn ran")
	}
	got := specs[0].TurnAppendPrompt
	if !strings.Contains(got, "GitHub pull request #3") {
		t.Errorf("host review append missing GitHub PR #3, got %q", got)
	}
	if !strings.Contains(got, "working copy") {
		t.Errorf("host review append missing working-copy warning, got %q", got)
	}
	if strings.Contains(specs[0].Prompt, "recur-surface") {
		t.Errorf("surface marker leaked into Prompt = %q", specs[0].Prompt)
	}
}

func TestSendWorkspaceMessagePeelsGitHubPRSurfaceMarker(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("surf-1", "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-8", WorkspaceID: ws.ID, ProjectID: projID, Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
		URL: "https://github.com/acme/app/pull/8", Branch: "feat", Base: "main",
		State: "In review",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	wire := `⟦recur-surface⟧{"surface":"github_pr","prId":"gh-acme-8","prNumber":8}⟦/recur-surface⟧
please review`
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", wire, "sonnet", nil, nil, domain.ChatModeReview, "")
	specs := runner.specsWithRole(domain.RoleCoder)
	if len(specs) == 0 {
		t.Fatal("no review turn ran")
	}
	if specs[0].Prompt != "please review" {
		t.Errorf("Prompt = %q, want peeled %q", specs[0].Prompt, "please review")
	}
	got := specs[0].TurnAppendPrompt
	if !strings.Contains(got, "GitHub pull request #8") {
		t.Errorf("linked workspace on PR view append missing GitHub PR #8, got %q", got)
	}
	if strings.Contains(got, "different surface") {
		t.Errorf("PR-view surface should not use the workspace linked-PR caveat, got %q", got)
	}
}

func TestSendWorkspaceMessageRetryKeepsGitHubPRSurface(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("retry-%d", call), "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-8", WorkspaceID: ws.ID, ProjectID: projID, Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
		URL: "https://github.com/acme/app/pull/8", Branch: "feat", Base: "main",
		State: "In review",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	wire := `⟦recur-surface⟧{"surface":"github_pr","prId":"gh-acme-8","prNumber":8}⟦/recur-surface⟧
please review`
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", wire, "sonnet", nil, nil, domain.ChatModeReview, "")
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "ignored", "sonnet", nil, nil, domain.ChatModeReview, chatRetryDisplaySentinel)
	specs := runner.specsWithRole(domain.RoleCoder)
	if len(specs) < 2 {
		t.Fatalf("retry specs = %d, want 2", len(specs))
	}
	got := specs[1].TurnAppendPrompt
	if !strings.Contains(got, "GitHub pull request #8") {
		t.Errorf("retry append missing GitHub PR #8, got %q", got)
	}
	if strings.Contains(got, "this Recur workspace") {
		t.Errorf("retry fell back to workspace surface, got %q", got)
	}
}

func TestHoistPRChatHostRekeysLastChatSurfaceForRetry(t *testing.T) {
	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture(fmt.Sprintf("hoist-surf-%d", call), "ok") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	repo := initRepo(t)
	projID := addProject(t, st, repo)
	ws, err := eng.CreateWorkspace(projID, "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	liveSess, err := st.CreateSession(domain.Session{
		WorkspaceID: ws.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession live: %v", err)
	}
	if _, err := st.AppendMessage(liveSess.ID, domain.Message{
		Role: "user", Content: []domain.ContentBlock{domain.TextBlock("live chat")},
	}); err != nil {
		t.Fatalf("AppendMessage live: %v", err)
	}
	st.UpsertAgent(ws.ID, domain.Agent{
		ID: ws.ID + "-PA", WorkspaceID: ws.ID, Code: "PA", SessionID: "prov-live",
	})
	pr := domain.PR{
		ID: "gh-acme-8h", ProjectID: projID, Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
		URL: "https://github.com/acme/app/pull/8", Branch: "feat", Base: "main",
		State: "In review",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host-surf", ProjectID: projID, Goal: "Already linked", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	if _, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	}); err != nil {
		t.Fatalf("CreateSession host: %v", err)
	}
	st.UpsertAgent(host.ID, domain.Agent{ID: "a-pa", Code: "PA", Name: "Agent", WorkspaceID: host.ID})
	_ = eng.SendWorkspaceMessage(host.ID, "PA", "what changed?", "sonnet", nil, nil, domain.ChatModeReview, "")
	// Link after the host turn: an idle host whose PR is already linked is
	// archived on unlock, so the send above would never run.
	st.UpdatePR(pr.ID, func(p *domain.PR) { p.WorkspaceID = ws.ID })
	if err := eng.hoistPRChatHost(host.ID, ws.ID); err != nil {
		t.Fatalf("hoistPRChatHost: %v", err)
	}
	if _, ok := eng.lastChatSurfaces.Load(ws.ID + "\x00" + "PA2"); !ok {
		t.Fatal("lastChatSurfaces missing dest PA2 after hoist")
	}
	if _, ok := eng.lastChatSurfaces.Load(host.ID + "\x00" + "PA"); ok {
		t.Fatal("lastChatSurfaces still keyed to host PA after hoist")
	}
	_ = eng.SendWorkspaceMessage(ws.ID, "PA2", "ignored", "sonnet", nil, nil, domain.ChatModeReview, chatRetryDisplaySentinel)
	specs := runner.specsWithRole(domain.RoleCoder)
	if len(specs) < 2 {
		t.Fatalf("retry specs = %d, want 2", len(specs))
	}
	got := specs[len(specs)-1].TurnAppendPrompt
	if !strings.Contains(got, "GitHub pull request #8") {
		t.Errorf("dest retry append missing GitHub PR #8, got %q", got)
	}
	if strings.Contains(got, "this Recur workspace") {
		t.Errorf("dest retry fell back to workspace surface, got %q", got)
	}
}

func TestEnsurePRChatHostRejectsSettledPR(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-9", ProjectID: "acme", Number: 9, Source: domain.SourceGitHub,
		Title: "Done", Branch: "feat", Base: "main", Group: prGroupMerged, State: "Merged",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if _, err := eng.EnsurePRChatHost(pr.ID); err == nil {
		t.Fatal("EnsurePRChatHost(merged) error = nil, want settled rejection")
	}
	if n := len(st.Workspaces()); n != 0 {
		t.Errorf("EnsurePRChatHost(merged) workspace count = %d, want 0", n)
	}
}

func TestCreateWorkspaceFromPRPromotesWhenWorkspaceIDPointsAtHost(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	clone := initCloneWithPullHead(t, 42, "feat/oauth")
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: clone, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-42", ProjectID: "acme", Number: 42, Source: domain.SourceGitHub,
		Title: "OAuth", Branch: "feat/oauth", Base: "main", Group: prGroupOpen, State: "Open",
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := eng.EnsurePRChatHost(pr.ID)
	if err != nil {
		t.Fatalf("EnsurePRChatHost: %v", err)
	}
	st.UpdatePR(pr.ID, func(p *domain.PR) { p.WorkspaceID = host.ID })
	got, err := eng.CreateWorkspaceFromPR(pr.ID)
	if err != nil {
		t.Fatalf("CreateWorkspaceFromPR: %v", err)
	}
	if got.ID != host.ID {
		t.Fatalf("CreateWorkspaceFromPR id = %q, want host %q (promote, not early return)", got.ID, host.ID)
	}
	if got.Phase != phaseCreating {
		t.Errorf("promoted Phase = %q, want %q", got.Phase, phaseCreating)
	}
}

func TestAskUserResumeHoldsPRHostBusy(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	ws, err := eng.CreateWorkspace("acme", "linked", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-8ask", WorkspaceID: ws.ID, ProjectID: "acme", Number: 8,
		Source: domain.SourceGitHub, Title: "Already linked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host-ask", ProjectID: "acme", Goal: "Already linked", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	st.UpsertAgent(host.ID, domain.Agent{ID: "a-pa", Code: "PA", Name: "Agent"})

	q := eng.askUsers.park(domain.UserQuestion{
		WorkspaceID: host.ID, AgentCode: "PA", Question: "color?",
	}, nil)
	if !eng.prChatHostBusy(host.ID) {
		t.Fatal("parked ask_user should keep the host busy")
	}
	got, _, ok := eng.askUsers.Answer(q.ID, domain.UserQuestionAnswer{Choice: "Blue"})
	if !ok {
		t.Fatal("Answer returned false")
	}
	if len(eng.PendingAskUsers()) != 0 {
		t.Fatalf("still pending after answer: %+v", eng.PendingAskUsers())
	}
	if !eng.prChatHostBusy(host.ID) {
		t.Fatal("answer-resume reservation should keep the host busy")
	}
	eng.maybeArchiveLinkedPRChatHost(host.ID)
	still, _ := st.Workspace(host.ID)
	if still.Archived {
		t.Fatal("host archived during ask_user resume gap")
	}
	if _, onHost := st.SessionForLoopAgent(host.ID, "PA"); !onHost {
		t.Fatal("session moved during ask_user resume gap")
	}
	eng.askUsers.dropResume(got.ID)
	if eng.prChatHostBusy(host.ID) {
		t.Fatal("host still busy after dropResume")
	}
	eng.maybeArchiveLinkedPRChatHost(host.ID)
	archived, _ := st.Workspace(host.ID)
	if !archived.Archived {
		t.Fatal("host should archive after the resume reservation drops")
	}
	moved, ok := st.SessionForLoopAgent(ws.ID, "PA")
	if !ok || moved.ID != sess.ID {
		t.Fatalf("session on linked workspace = %+v ok=%v, want %s", moved, ok, sess.ID)
	}
}

func TestArchiveOnePRChatHostRevalidatesLink(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	repo := initRepo(t)
	if _, err := st.AddProject(domain.Project{ID: "acme", Name: "acme", RepoPath: repo, BaseBranch: "main"}); err != nil {
		t.Fatalf("AddProject: %v", err)
	}
	w1, err := eng.CreateWorkspace("acme", "keep-stale", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace w1: %v", err)
	}
	w2, err := eng.CreateWorkspace("acme", "keep-live", "main")
	if err != nil {
		t.Fatalf("CreateWorkspace w2: %v", err)
	}
	pr := domain.PR{
		ID: "gh-acme-8relink", WorkspaceID: w2.ID, ProjectID: "acme", Number: 8,
		Source: domain.SourceGitHub, Title: "Relinked", Group: prGroupYou,
	}
	if _, err := st.AddPR(pr); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	host, err := st.AddWorkspace(domain.Workspace{
		ID: "W-host-relink", ProjectID: "acme", Goal: "Relinked", HostPRID: pr.ID,
		WorktreePath: repo, Type: domain.WorkspaceSolo,
	})
	if err != nil {
		t.Fatalf("AddWorkspace host: %v", err)
	}
	sess, err := st.CreateSession(domain.Session{
		WorkspaceID: host.ID, Kind: "pair", Provider: domain.ProviderClaudeCode,
		ConfigJSON: `{"code":"PA"}`,
	})
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	st.UpsertAgent(host.ID, domain.Agent{ID: "a-pa", Code: "PA", Name: "Agent"})

	eng.archiveOnePRChatHost(host.ID, w1.ID)
	still, _ := st.Workspace(host.ID)
	if still.Archived {
		t.Fatal("host archived onto a keepID the PR is no longer linked to")
	}
	if _, onW1 := st.SessionForLoopAgent(w1.ID, "PA"); onW1 {
		t.Fatal("session hoisted onto stale keepID")
	}
	kept, ok := st.SessionForLoopAgent(host.ID, "PA")
	if !ok || kept.ID != sess.ID {
		t.Fatalf("session after stale hoist = %+v ok=%v, want %s on host", kept, ok, sess.ID)
	}

	eng.archiveOnePRChatHost(host.ID, w2.ID)
	archived, _ := st.Workspace(host.ID)
	if !archived.Archived {
		t.Fatal("host should archive onto the PR's current live workspace")
	}
	moved, ok := st.SessionForLoopAgent(w2.ID, "PA")
	if !ok || moved.ID != sess.ID {
		t.Fatalf("session on live keep = %+v ok=%v, want %s", moved, ok, sess.ID)
	}
}

func TestForkRemoteURL(t *testing.T) {
	t.Parallel()
	for _, tc := range []struct{ in, want string }{
		{"git@github.com:celestiaorg/recur.git", "git@github.com:auricom/recur.git"},
		{"https://github.com/celestiaorg/recur", "https://github.com/auricom/recur.git"},
		{"https://github.com/celestiaorg/recur.git/\n", "https://github.com/auricom/recur.git"},
		{"ssh://git@github.com/celestiaorg/recur.git", "ssh://git@github.com/auricom/recur.git"},
		{"/tmp/x/celestiaorg/recur.git", "/tmp/x/auricom/recur.git"},
		{"recur", ""},
	} {
		got, ok := forkRemoteURL(tc.in, "auricom", "recur")
		if got != tc.want || ok != (tc.want != "") {
			t.Errorf("forkRemoteURL(%q) = %q, %v; want %q", tc.in, got, ok, tc.want)
		}
	}
}
