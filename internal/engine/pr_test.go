package engine

import (
	"context"
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"recur/internal/domain"
	"recur/internal/store"
)

// TestPRCreatedAtPROpen is the M6 PR-creation test (brief B): a scripted loop
// converges (and, rail #1 OFF here, skips verify) → the engine opens a Local PR
// with the REAL diff + commits, links it to the loop, and persists it so it
// survives a store reopen.
func TestPRCreatedAtPROpen(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0}} // converges on iteration 1
	eng, st, sink, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Add a real PR feature", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}

	waitFor(t, 10*time.Second, "loop to reach pr_open", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})

	// A Local PR was created and linked to the loop.
	pr, ok := st.PRByWorkspace(id)
	if !ok {
		t.Fatal("no PR created for the converged loop")
	}
	if pr.Source != domain.SourceLocal {
		t.Errorf("engine-created PR source: got %q, want local", pr.Source)
	}
	if pr.State != "In review" || pr.Group != "you" {
		t.Errorf("new PR state wrong: state=%q group=%q", pr.State, pr.Group)
	}
	if len(pr.AttentionReasons) != 1 || pr.AttentionReasons[0] != domain.AttentionReviewRequested {
		t.Errorf("new local PR AttentionReasons = %v, want [review_requested]", pr.AttentionReasons)
	}
	if pr.Branch == "" || pr.Base != "main" {
		t.Errorf("PR branch/base wrong: branch=%q base=%q", pr.Branch, pr.Base)
	}

	// REAL diff: the convergeRunner writes work.txt, so the PR has at least one file
	// with parsed add lines (HTML-escaped code).
	if pr.FileCount == 0 || len(pr.Files) == 0 {
		t.Fatalf("PR has no files; expected the real diff: %+v", pr)
	}
	foundWork := false
	for _, f := range pr.Files {
		if f.Path == "work.txt" {
			foundWork = true
			if f.Diff.Add == 0 {
				t.Errorf("work.txt should have additions, got %+v", f.Diff)
			}
		}
	}
	if !foundWork {
		t.Errorf("expected work.txt in the PR diff, got files: %+v", pr.Files)
	}
	if pr.Diff.Add == 0 {
		t.Errorf("PR-wide additions should be > 0, got %+v", pr.Diff)
	}

	// REAL commits: at least the iteration-1 engine commit. Current subjects are
	// a plain summary (no recur(Coder) prefix), so AuthorRole falls back to Author.
	if len(pr.Commits) == 0 {
		t.Fatal("PR has no commits")
	}
	if msg := singleLine(pr.Commits[0].Message); msg == "" {
		t.Errorf("PR commit message is empty: %+v", pr.Commits[0])
	}

	gotBody := githubPRMarkdown("Add a real PR feature", pr.Commits, pr.FileCount, pr.Diff.Add, pr.Diff.Del,
		[]string{"Converged to 0 independent-review findings."})
	if !strings.Contains(gotBody, "Add a real PR feature") {
		t.Errorf("githubPRMarkdown missing goal, got %q", gotBody)
	}
	if !strings.Contains(gotBody, "## Changes") || !strings.Contains(gotBody, "versus the base branch") {
		t.Errorf("githubPRMarkdown missing changes section, got %q", gotBody)
	}
	if !strings.Contains(gotBody, "Converged to 0 independent-review findings.") {
		t.Errorf("githubPRMarkdown missing convergence note, got %q", gotBody)
	}
	if strings.Contains(gotBody, "Opened by Recur") {
		t.Errorf("githubPRMarkdown must not use the stub body, got %q", gotBody)
	}
	got := prDescription(pr)
	if !strings.Contains(got, "Add a real PR feature") || !strings.Contains(got, "## Changes") {
		t.Errorf("prDescription(local PR) = %q, want goal and changes section", got)
	}
	if strings.Contains(got, "Opened by Recur") {
		t.Errorf("prDescription(local PR) = %q, must not stub", got)
	}

	// Summary + focus + checks present.
	if len(pr.Summary) == 0 {
		t.Error("PR has no summary")
	}
	if pr.Focus.Title == "" {
		t.Error("PR has no focus note")
	}
	hasReviewCheck := false
	for _, c := range pr.Checks {
		if c.Name == "Independent review" && c.Passed {
			hasReviewCheck = true
		}
	}
	if !hasReviewCheck {
		t.Errorf("PR checks should include a passing independent review, got %+v", pr.Checks)
	}

	foundPRFact := false
	for _, it := range st.Feed(id) {
		if strings.Contains(it.Text, `data-pr="`+pr.ID+`"`) && strings.Contains(it.Text, "Completed in") {
			foundPRFact = true
			break
		}
	}
	if !foundPRFact {
		t.Errorf("feed missing clickable completed-PR line, got %+v", st.Feed(id))
	}

	// pr.updated fired.
	if !sink.has("pr.updated") {
		t.Error("expected a pr.updated event when the PR opened")
	}

	// The PR survives a store reopen (persisted, PRD §6.5).
	st.Close()
	st2, err := store.New() // same $RECUR_DATA_DIR
	if err != nil {
		t.Fatalf("reopen store: %v", err)
	}
	defer st2.Close()
	pr2, ok := st2.PRByWorkspace(id)
	if !ok {
		t.Fatal("PR missing after store reopen")
	}
	if pr2.FileCount != pr.FileCount || len(pr2.Files) != len(pr.Files) {
		t.Errorf("reopened PR diff lost: before files=%d after=%d", len(pr.Files), len(pr2.Files))
	}
	if len(pr2.Commits) != len(pr.Commits) {
		t.Errorf("reopened PR commits lost: before=%d after=%d", len(pr.Commits), len(pr2.Commits))
	}
	if prDescription(pr2) != prDescription(pr) {
		t.Errorf("reopened PR description = %q, want %q", prDescription(pr2), prDescription(pr))
	}
}

// TestLocalMergeAdvancesBase is the M6 merge test (brief E): a converged loop's
// Local PR merges its branch into the base ON DISK; the base ref advances and the
// PR + loop are marked merged.
func TestLocalMergeAdvancesBase(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Mergeable feature", "", "")
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

	baseBefore := refHash(t, repo, "main")

	merged, mok := eng.MergePR(pr.ID, "")
	if !mok {
		t.Fatal("MergePR should succeed for a Local PR")
	}
	if merged.State != "Merged" || merged.Group != "merged" {
		t.Errorf("merged PR state: state=%q group=%q", merged.State, merged.Group)
	}

	// The base ref advanced on disk to include the loop branch's commit(s).
	baseAfter := refHash(t, repo, "main")
	if baseAfter == baseBefore {
		t.Errorf("base ref did not advance after Local merge: %s == %s", baseAfter, baseBefore)
	}

	// The loop is marked merged.
	if l, _ := st.Workspace(id); l.State != domain.StateMerged {
		t.Errorf("loop state after merge: got %q, want merged", l.State)
	}

	// A second merge is an idempotent no-op (already merged).
	if _, ok := eng.MergePR(pr.ID, ""); !ok {
		t.Error("re-merging an already-merged PR should be an idempotent ok")
	}
}

// TestPROpenPublishesNotify is the Phase 3.6 regression test: a converged loop
// opening its PR publishes a generic "notify" bus event (main.go turns this into a
// native "PR ready for review" OS notification) alongside the existing inbox row.
func TestPROpenPublishesNotify(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, sink, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Add a feature that notifies on PR-ready", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}
	waitFor(t, 10*time.Second, "loop to reach pr_open", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})
	if !sink.has("notify") {
		t.Error("expected a notify event when the PR opened")
	}
}

func TestLoopWrapUpHTML(t *testing.T) {
	got := loopWrapUpHTML("Root cause: race in checkout.\nLanded retries in the parent and both children.")
	if !strings.Contains(got, `class="loop-wrap"`) || !strings.Contains(got, "Root cause: race in checkout.") || !strings.Contains(got, "<br>") {
		t.Errorf("loopWrapUpHTML = %q", got)
	}
	if loopWrapUpHTML("  ") != "" {
		t.Errorf("blank wrap should be empty")
	}
	esc := loopWrapUpHTML("<script>x</script>")
	if strings.Contains(esc, "<script>") {
		t.Errorf("wrap must escape, got %q", esc)
	}
	fenced := loopWrapUpHTML("```\nRoot cause: nil deref\n```")
	if !strings.Contains(fenced, "Root cause: nil deref") || strings.Contains(fenced, "```") {
		t.Errorf("fenced wrap = %q", fenced)
	}
}

func TestWrapUpPromptSeesEveryLanding(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	if _, err := st.AddWorkspace(domain.Workspace{ID: "L-fan", ProjectID: "P1", Goal: "Fix flaky checkout"}); err != nil {
		t.Fatal(err)
	}
	lp := &Loop{ID: "L-fan", Goal: "Fix flaky checkout", eng: eng, boardHandoff: "S1: mutex in cart\nS2: retry the token"}
	lp.noteDoneSummary("Add cart mutex", "")
	lp.noteDoneSummary("Retry payment token", "")
	got := lp.wrapUpPrompt()
	for _, want := range []string{
		"Fix flaky checkout",
		"S1: mutex in cart",
		"S2: retry the token",
		"Add cart mutex",
		"Retry payment token",
		"every turn, not one Coder",
	} {
		if !strings.Contains(got, want) {
			t.Errorf("wrapUpPrompt missing %q, got %q", want, got)
		}
	}
}

func TestEmitSettledWrapUpIdle(t *testing.T) {
	reply := "Root cause: race in checkout. Landed retries in the parent and both child steps."
	var saw AgentSpec
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{
		Lines: maestroFixture("wrap-1", reply),
		OnRun: func(s AgentSpec) { saw = s },
	})
	if _, err := st.AddWorkspace(domain.Workspace{ID: "L-wrap", ProjectID: "P1", Goal: "Fix flaky checkout"}); err != nil {
		t.Fatal(err)
	}
	lp := &Loop{
		ID: "L-wrap", Goal: "Fix flaky checkout", eng: eng,
		exitState: domain.StatePaused, exitPhase: "idle",
		startedAt:    time.Now().Add(-12 * time.Second),
		boardHandoff: "S1: mutex\nS2: retry",
	}
	lp.emitSettledWrapUp()

	if !strings.Contains(saw.Prompt, "Fix flaky checkout") || !strings.Contains(saw.Prompt, "S1: mutex") {
		t.Errorf("wrap-up prompt missing whole-run context, got %q", saw.Prompt)
	}
	if saw.Role != "" {
		t.Errorf("wrap-up Role = %q, want empty so it is not a chat turn", saw.Role)
	}

	found := false
	for _, it := range st.Feed("L-wrap") {
		if !strings.Contains(it.Text, "Completed in") {
			continue
		}
		found = true
		if !strings.Contains(it.Text, `class="loop-wrap"`) || !strings.Contains(it.Text, "Landed retries in the parent") {
			t.Errorf("idle wrap-up missing Maestro prose, got %q", it.Text)
		}
		if strings.Contains(it.Text, "data-pr=") {
			t.Errorf("idle wrap-up should not be a PR fact, got %q", it.Text)
		}
	}
	if !found {
		t.Errorf("feed missing idle completion, got %+v", st.Feed("L-wrap"))
	}

	child := &Loop{
		ID: "L-child", Goal: "S1", eng: eng, parentID: "L-wrap",
		exitState: domain.StatePaused, exitPhase: "idle",
	}
	before := len(st.Feed("L-wrap"))
	child.emitSettledWrapUp()
	if len(st.Feed("L-wrap")) != before {
		t.Error("child settle must not wrap up on the parent feed")
	}
}

func TestAttachLoopWrapPatchesPRFact(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	if _, err := st.AddWorkspace(domain.Workspace{ID: "L-prw", ProjectID: "P1", Goal: "t"}); err != nil {
		t.Fatal(err)
	}
	eng.emitFeedPRCreated("L-prw", domain.PR{ID: "12", Number: 12, Source: domain.SourceLocal}, 5*time.Second)
	wrap := loopWrapUpHTML("Root cause: nil deref. Guarded the lookup.")
	eng.attachLoopWrap("L-prw", wrap)
	got := ""
	for _, it := range st.Feed("L-prw") {
		if strings.Contains(it.Text, `data-pr="12"`) {
			got = it.Text
			break
		}
	}
	if !strings.Contains(got, "Completed in") || !strings.Contains(got, "Root cause: nil deref") {
		t.Errorf("patched PR fact = %q", got)
	}
}

func TestFormatLoopElapsed(t *testing.T) {
	cases := []struct {
		d    time.Duration
		want string
	}{
		{3 * time.Second, "3s"},
		{90 * time.Second, "1m 30s"},
		{5 * time.Minute, "5m"},
		{time.Hour + 2*time.Minute, "1h 2m"},
		{2 * time.Hour, "2h"},
	}
	for _, c := range cases {
		if got := formatLoopElapsed(c.d); got != c.want {
			t.Errorf("formatLoopElapsed(%v) = %q, want %q", c.d, got, c.want)
		}
	}
}

func TestCommitRefMatchesShortAndFullHash(t *testing.T) {
	t.Parallel()
	c := domain.Commit{Hash: "3f1d02b", SHA: "3f1d02babcdef0123456789"}
	for _, ref := range []string{"3f1d02b", "3F1D02B", "3f1d02babcdef0123456789"} {
		if !commitRefMatches(c, ref) {
			t.Errorf("commitRefMatches(%q) = false, want true", ref)
		}
	}
	if commitRefMatches(c, "deadbeef") || commitRefMatches(c, "3f1") {
		t.Error("commitRefMatches() accepted a non-matching or too-short ref")
	}
}

func TestMergeCommitLogKeepsDiffsAndFillsBody(t *testing.T) {
	t.Parallel()
	existing := []domain.Commit{
		{Hash: "3f1d02b", Message: "Enable the tab", Files: []domain.FileDiff{{Path: "a.ts"}}, Verified: true, CheckStatus: "pass"},
	}
	fresh := []domain.Commit{
		{Hash: "3f1d02b", SHA: "3f1d02babcdef", Message: "Enable the tab", Body: "Turn it on by default.", Author: "mycodecrafting", When: "2h"},
	}
	got := mergeCommitLog(existing, fresh)
	if len(got) != 1 {
		t.Fatalf("mergeCommitLog() len = %d, want 1", len(got))
	}
	if got[0].Body != "Turn it on by default." || got[0].Author != "mycodecrafting" || got[0].When != "2h" {
		t.Errorf("mergeCommitLog() meta = %+v, want body/author/when from git log", got[0])
	}
	if len(got[0].Files) != 1 || got[0].Files[0].Path != "a.ts" || !got[0].Verified || got[0].CheckStatus != "pass" {
		t.Errorf("mergeCommitLog() kept = %+v, want stored files + GH rollup", got[0])
	}
}

func TestCommentHunkFromFiles(t *testing.T) {
	t.Parallel()
	files := []domain.FileDiff{{
		Path: "auth.go",
		Lines: []domain.DiffLine{
			{Type: "hunk", CodeHTML: "@@ -10,3 +10,3 @@"},
			{Type: "ctx", Old: "10", New: "10", CodeHTML: " keep"},
			{Type: "del", Old: "11", CodeHTML: `<span class="kw">old</span>`},
			{Type: "add", New: "11", CodeHTML: "new"},
		},
	}}
	got := commentHunkFromFiles(files, "auth.go", 11, "RIGHT")
	if !strings.Contains(got, "@@ -10,3 +10,3 @@") || !strings.Contains(got, "-old") || !strings.Contains(got, "+new") {
		t.Errorf("commentHunkFromFiles() = %q, want hunk through +new", got)
	}
}

func TestPendingLoc(t *testing.T) {
	t.Parallel()
	got := pendingLoc(domain.PendingLineComment{File: "auth.go", Line: 12, StartLine: 10})
	if got != "auth.go:10-12" {
		t.Errorf("pendingLoc(auth.go 10-12) = %q, want auth.go:10-12", got)
	}
	got = pendingLoc(domain.PendingLineComment{File: "auth.go", Line: 12})
	if got != "auth.go:12" {
		t.Errorf("pendingLoc(auth.go 12) = %q, want auth.go:12", got)
	}
}

// refHash reads a ref's hash in repo.
func refHash(t *testing.T, repo, ref string) string {
	t.Helper()
	out, err := gitOut(repo, repo, "rev-parse", ref)
	if err != nil {
		t.Fatalf("rev-parse %s: %v", ref, err)
	}
	return strings.TrimSpace(out)
}

func TestPRTitle(t *testing.T) {
	long := strings.Repeat("x", 300)
	tests := []struct {
		goal, want string
	}{
		{goal: "first line\nsecond line", want: "first line"},
		{goal: long, want: strings.Repeat("x", 80)},
		{goal: "", want: "Loop changes"},
	}
	for _, tt := range tests {
		if got := prTitle(tt.goal); got != tt.want {
			t.Errorf("prTitle(%q) = %q, want %q", tt.goal, got, tt.want)
		}
	}
}

func TestGitHubPRMarkdown(t *testing.T) {
	commits := []domain.Commit{
		{Message: "feat: add gateway\n\nlonger body"},
		{Message: "fix: handle empty input"},
	}
	got := githubPRMarkdown("Ship the gateway", commits, 3, 40, 2, []string{"Verified in a browser."})
	for _, want := range []string{
		"Ship the gateway",
		"## Changes",
		"2 commits across 3 files",
		"Net +40 −2 versus the base branch",
		"## Commits",
		"- feat: add gateway",
		"- fix: handle empty input",
		"Verified in a browser.",
	} {
		if !strings.Contains(got, want) {
			t.Errorf("githubPRMarkdown missing %q, got %q", want, got)
		}
	}
	if strings.Contains(got, "Opened by Recur") {
		t.Errorf("githubPRMarkdown = %q, must not stub", got)
	}

	empty := githubPRMarkdown("", nil, 0, 0, 0, nil)
	if !strings.Contains(empty, "0 commits across 0 files") {
		t.Errorf("githubPRMarkdown(empty) = %q, want a changes line", empty)
	}
}

func TestParsePRDescription(t *testing.T) {
	tests := []struct {
		in, wantTitle, wantBody string
	}{
		{
			in:        "Fix the overlay\n\nKeep the sidebar from covering the composer on short viewports.",
			wantTitle: "Fix the overlay",
			wantBody:  "Keep the sidebar from covering the composer on short viewports.",
		},
		{
			in:        "Just a title",
			wantTitle: "Just a title",
		},
		{
			in:        "Fix overlay\n\nTested with `go test ./...`",
			wantTitle: "Fix overlay",
			wantBody:  "Tested with `go test ./...`",
		},
		{
			in:        "Add helper\n\nExample:\n\n```\nfoo()\n```",
			wantTitle: "Add helper",
			wantBody:  "Example:\n\n```\nfoo()\n```",
		},
		{
			in:        "```\nFix overlay\n\nTested with `go test ./...`\n```",
			wantTitle: "Fix overlay",
			wantBody:  "Tested with `go test ./...`",
		},
		{
			in:        "```markdown\nAdd helper\n\nExample:\n\n```\nfoo()\n```\n```",
			wantTitle: "Add helper",
			wantBody:  "Example:\n\n```\nfoo()\n```",
		},
	}
	for _, tc := range tests {
		title, body := parsePRDescription(tc.in)
		if title != tc.wantTitle || body != tc.wantBody {
			t.Errorf("parsePRDescription(%q) = %q %q, want %q %q", tc.in, title, body, tc.wantTitle, tc.wantBody)
		}
	}
}

func TestCapGitHubPRBody(t *testing.T) {
	long := strings.Repeat("x", githubPRBodyMax+50)
	capped := capGitHubPRBody(long)
	if len(capped) <= githubPRBodyMax {
		t.Errorf("capGitHubPRBody len = %d, want > %d (includes truncation note)", len(capped), githubPRBodyMax)
	}
	if !strings.Contains(capped, "truncated") {
		t.Errorf("capGitHubPRBody(%d bytes) = %q, want truncation note", len(long), capped[len(capped)-40:])
	}

	wide := strings.Repeat("−", githubPRBodyMax+10)
	wideCapped := capGitHubPRBody(wide)
	if !utf8.ValidString(wideCapped) {
		t.Errorf("capGitHubPRBody(multi-byte) produced invalid UTF-8")
	}
	if !strings.Contains(wideCapped, "truncated") {
		t.Errorf("capGitHubPRBody(multi-byte) = %q, want truncation note", wideCapped[len(wideCapped)-40:])
	}
	body, _, _ := strings.Cut(wideCapped, "\n\n…(truncated)")
	if got := utf8.RuneCountInString(body); got != githubPRBodyMax {
		t.Errorf("capGitHubPRBody(multi-byte) body runes = %d, want %d", got, githubPRBodyMax)
	}
}

func TestPRFallbackIntro(t *testing.T) {
	tests := []struct {
		goal, display, want string
	}{
		{goal: "Add dark mode", want: "Add dark mode"},
		{goal: "  Add dark mode  ", want: "Add dark mode"},
		{
			goal:    "The human already wrote this change. Review the current worktree, then fix and re-review until the policy bar passes, then open a PR.",
			display: "Review & fix to a PR",
			want:    "",
		},
		{goal: "Linear ENG-123\n\nFull issue body.", display: "Linear ENG-123 · title", want: ""},
	}
	for _, tt := range tests {
		if got := prFallbackIntro(tt.goal, tt.display); got != tt.want {
			t.Errorf("prFallbackIntro(%q, %q) = %q, want %q", tt.goal, tt.display, got, tt.want)
		}
	}
}

func TestLocalPRDescriptionSkipsPromptActionWire(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{0}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)
	wire := "The human already wrote this change. Review the current worktree, then fix and re-review until the policy bar passes, then open a PR."
	label := "Review & fix to a PR"
	rec, acquired, err := eng.queueStartWorkspace(projID, wire, label, "", "", "", "", nil)
	if err != nil {
		t.Fatalf("queueStartWorkspace: %v", err)
	}
	if !acquired {
		t.Fatal("queueStartWorkspace: expected a loop slot")
	}
	if err := eng.dispatch(rec); err != nil {
		eng.gov.ReleaseLoop()
		t.Fatalf("dispatch: %v", err)
	}
	waitFor(t, 10*time.Second, "loop to reach pr_open", func() bool {
		l, ok := st.Workspace(rec.ID)
		return ok && l.State == domain.StatePROpen
	})
	pr, ok := st.PRByWorkspace(rec.ID)
	if !ok {
		t.Fatal("no PR created for the prompt-action loop")
	}
	if pr.Title != label {
		t.Errorf("PR title = %q, want the GoalDisplay label %q", pr.Title, label)
	}
	got := prDescription(pr)
	if strings.Contains(got, "The human already wrote") || strings.Contains(got, "Do not reopen scope") {
		t.Errorf("prDescription dumped the prompt-action wire: %q", got)
	}
	if !strings.Contains(got, "## Changes") {
		t.Errorf("prDescription = %q, want the factual changes fallback", got)
	}
}

// describePRConverge is a convergeRunner that answers the PR-description one-shot
// with a workspace-wide summary (the production title-model path).
type describePRConverge struct {
	convergeRunner
}

func (r *describePRConverge) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if strings.Contains(spec.Prompt, "pull request title") {
		return Result{
			Subtype: "success",
			Text:    "Cover the checkout flake\n\nLock the overlay so it cannot cover the composer, and restabilize the checkout tests versus the base branch.",
		}, nil
	}
	return r.convergeRunner.Run(ctx, spec, emit)
}

func TestLocalPRDescriptionCoversWorkspaceDiff(t *testing.T) {
	repo := initRepo(t)
	runner := &describePRConverge{convergeRunner: convergeRunner{findingsPerIter: []int{0}}}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)
	wire := "The human already wrote this change. Review the current worktree, then fix and re-review until the policy bar passes, then open a PR."
	label := "Review & fix to a PR"
	rec, acquired, err := eng.queueStartWorkspace(projID, wire, label, "", "", "", "", nil)
	if err != nil {
		t.Fatalf("queueStartWorkspace: %v", err)
	}
	if !acquired {
		t.Fatal("queueStartWorkspace: expected a loop slot")
	}
	if err := eng.dispatch(rec); err != nil {
		eng.gov.ReleaseLoop()
		t.Fatalf("dispatch: %v", err)
	}
	waitFor(t, 10*time.Second, "loop to reach pr_open", func() bool {
		l, ok := st.Workspace(rec.ID)
		return ok && l.State == domain.StatePROpen
	})
	pr, ok := st.PRByWorkspace(rec.ID)
	if !ok {
		t.Fatal("no PR created for the prompt-action loop")
	}
	if pr.Title != "Cover the checkout flake" {
		t.Errorf("PR title = %q, want the generated workspace title", pr.Title)
	}
	got := prDescription(pr)
	if strings.Contains(got, "The human already wrote") || strings.Contains(got, "Do not reopen scope") {
		t.Errorf("prDescription dumped the prompt-action wire: %q", got)
	}
	if strings.Contains(got, "## Changes") {
		t.Errorf("prDescription = %q, want the generated workspace summary, not the stats fallback", got)
	}
	if !strings.Contains(got, "versus the base branch") {
		t.Errorf("prDescription = %q, want a workspace-wide summary versus the base", got)
	}
}

func TestFormatPRThreadIncludesLineNotes(t *testing.T) {
	pr := domain.PR{
		Number: 12,
		Title:  "Ship the gateway",
		Comments: []domain.PRComment{
			{Kind: "description", BodyMD: "hidden description"},
			{Author: "ada", BodyMD: "please rename the config key"},
			{Author: "ada", Path: "oauth.ts", Line: 40, BodyMD: "client secret read", Resolved: true},
			{Kind: "action", BodyHTML: "<p>approved</p>"},
		},
		PendingComments: []domain.PendingLineComment{
			{File: "auth.go", Line: 8, Body: "nit: unwrap this error"},
		},
	}
	got := formatPRThread(pr)
	for _, want := range []string{
		"PR #12",
		"Ship the gateway",
		"ada: please rename the config key",
		"ada on oauth.ts:40 (resolved): client secret read",
		"pending on auth.go:8: nit: unwrap this error",
	} {
		if !strings.Contains(got, want) {
			t.Errorf("formatPRThread() = %q, want substring %q", got, want)
		}
	}
	if strings.Contains(got, "hidden description") || strings.Contains(got, "approved") {
		t.Errorf("formatPRThread() = %q, should omit description/action rows", got)
	}
}

func TestMaestroContextIncludesPRThread(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	ws := domain.Workspace{ID: "L-thread", ProjectID: "P", Goal: "fix auth", State: domain.StatePaused, RunStarted: true}
	if _, err := st.AddWorkspace(ws); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	if _, err := st.AddPR(domain.PR{
		ID: "12", WorkspaceID: ws.ID, Number: 12, Title: "fix auth",
		Comments: []domain.PRComment{{Author: "ada", Path: "a.go", Line: 3, BodyMD: "off-by-one"}},
	}); err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	ctx := eng.maestroContext(ws)
	if !strings.Contains(ctx.PRThread, "ada on a.go:3: off-by-one") {
		t.Errorf("maestroContext().PRThread = %q, want line note", ctx.PRThread)
	}
	prompt := maestroSystemPrompt(ctx)
	if !strings.Contains(prompt, "off-by-one") || !strings.Contains(prompt, "reenter_phase") {
		t.Errorf("maestroSystemPrompt() = %q, want thread + reenter_phase", prompt)
	}
}

func TestLocalPRActivityTouchesUpdatedAt(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	const frozen = "2026-08-01T12:00:00Z"
	pr, err := st.AddPR(domain.PR{
		ID: "local-touch", Number: 10, Source: domain.SourceLocal, Title: "old",
		State: "In review", CreatedAt: frozen, UpdatedAt: frozen,
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	if _, ok := st.UpdatePR(pr.ID, func(p *domain.PR) {
		p.Files = []domain.FileDiff{{Path: "a.go"}}
	}); !ok {
		t.Fatal("cache-fill UpdatePR ok=false")
	}
	got, _ := st.PR(pr.ID)
	if got.UpdatedAt != frozen {
		t.Fatalf("highlight/cache fill bumped UpdatedAt = %q, want frozen", got.UpdatedAt)
	}

	if _, ok := eng.AddPRComment(pr.ID, "a.go", 1, "draft note", 0, ""); !ok {
		t.Fatal("AddPRComment pending ok=false")
	}
	got, _ = st.PR(pr.ID)
	if got.UpdatedAt != frozen {
		t.Fatalf("unsubmitted draft bumped UpdatedAt = %q, want frozen", got.UpdatedAt)
	}

	if _, ok := eng.AddPRComment(pr.ID, "", 0, "please take a look", 0, ""); !ok {
		t.Fatal("AddPRComment ok=false")
	}
	got, _ = st.PR(pr.ID)
	if got.UpdatedAt == frozen {
		t.Fatal("conversation comment did not bump UpdatedAt")
	}
	if got.CreatedAt != frozen {
		t.Fatalf("CreatedAt = %q, want frozen", got.CreatedAt)
	}

	if _, ok := st.UpdatePR(pr.ID, func(p *domain.PR) { p.UpdatedAt = frozen }); !ok {
		t.Fatal("reset UpdatedAt")
	}
	if _, ok := eng.SubmitReview(pr.ID, "comment", "lgtm"); !ok {
		t.Fatal("SubmitReview ok=false")
	}
	got, _ = st.PR(pr.ID)
	if got.UpdatedAt == frozen {
		t.Fatal("SubmitReview did not bump UpdatedAt")
	}

	if _, ok := st.UpdatePR(pr.ID, func(p *domain.PR) { p.UpdatedAt = frozen }); !ok {
		t.Fatal("reset UpdatedAt")
	}
	updated, ok := eng.UpdatePRDetails(pr.ID, "new title", "new body")
	if !ok {
		t.Fatal("UpdatePRDetails ok=false")
	}
	if updated.UpdatedAt == frozen {
		t.Fatal("UpdatePRDetails did not bump UpdatedAt")
	}
}
