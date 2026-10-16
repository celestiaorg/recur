package engine

import (
	"context"
	"errors"
	"fmt"
	"html"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"recur/internal/code"
	"recur/internal/domain"
	gitpkg "recur/internal/git"
	"recur/internal/observe"
)

// touchPRUpdated records last-activity time for a genuine mutation (review,
// comment, rework, merge). Cache fills such as highlighting or commit-diff
// backfill must not call this — those writes would float the row to the top
// of "Recently updated" when the user merely opened the PR.
func touchPRUpdated(p *domain.PR) {
	p.UpdatedAt = time.Now().UTC().Format(time.RFC3339)
}

// openPR builds and persists the Local PR for a converged+verified loop the moment
// it reaches pr_open (PRD §7.2, §12, M6 brief B). It computes the REAL diff vs the
// base branch and the REAL commit list from the loop's worktree (PRD §11), attaches
// the loop's M5 verify evidence + a checks rollup, writes an engine-generated
// summary + "where to focus" (a real Maestro-written summary is M7), persists the
// PR (prs table, survives a reopen), links it to the loop, and emits pr.updated so
// the PR list/detail render live.
//
// It returns the created PR id. A pre-existing PR for this loop (a rework round)
// is RE-CREATED in place under the same id so the PR list never grows a duplicate.
// Best effort on the git reads: a diff/log error degrades to an empty diff rather
// than wedging the loop (the loop still settles at pr_open).
func (l *Loop) openPR(ctx context.Context) (string, error) {
	base := baseBranchFor(l)

	// Reuse the existing PR id (and number) on a rework round so reload shows one PR
	// with its history, not a fresh row each time.
	prID := ""
	prNum := 0
	source := domain.SourceLocal
	if existing, ok := l.eng.Store.PRByWorkspace(l.ID); ok {
		prID = existing.ID
		prNum = existing.Number
		source = existing.Source
	}
	if prID == "" {
		prNum = l.eng.Store.NextPRNumber()
		prID = fmt.Sprintf("%d", prNum)
	}

	// REAL diff + commits from the worktree (PRD §11). Degrade gracefully on error.
	var files []domain.FileDiff
	var add, del int
	var commits []domain.Commit
	if l.git != nil && l.Worktree != "" {
		if pr, err := l.git.DiffPR(ctx, l.Worktree, base); err == nil {
			files, add, del = pr.Files, pr.Add, pr.Del
		} else {
			l.emitFeed("MA", "Maestro", "Could not compute the PR diff: "+html.EscapeString(err.Error()))
		}
		if cs, err := l.git.LogCommits(ctx, l.Worktree, base); err == nil {
			commits = l.fillCommitDiffs(ctx, cs)
		}
	}

	// GitHub mode (PRD §12.1): when the project opts in, a NEW PR is opened on GitHub
	// (push + `gh pr create`) and carries the real GitHub number; on any failure we
	// fall back to a Local PR with an honest feed note (never wedge the loop). A
	// REWORK re-open of an existing GitHub PR pushes the new commits so the remote PR
	// (which `gh pr merge` will merge) reflects the rework.
	loop, _ := l.eng.Store.Workspace(l.ID)
	proj, _ := l.eng.Store.Project(l.ProjectID)
	// Live worktree branch wins over the cached name (human/agent renames).
	branch := l.eng.syncWorkspaceBranch(l.ID)
	if branch == "" {
		branch = l.Branch
	}
	l.Branch = branch
	var ghPR domain.PR
	if source == domain.SourceGitHub {
		ghPR, _ = l.eng.Store.PRByWorkspace(l.ID)
	}
	notes := []string{"Loop policy bar passed — ready for review."}
	if loop.VerifyState == verifyStatePassed {
		notes = append(notes, "Verified in a browser.")
	}
	label := l.goalLabel()
	title := prTitle(label)
	body := ""
	if l.git != nil && l.Worktree != "" {
		_, diff := l.git.ChangedDiff(ctx, l.Worktree, base)
		if t, b, gerr := l.eng.generatePRDescription(ctx, l.ID, l.Worktree, diff, commitSubjects(commits)); gerr == nil {
			if t != "" {
				title = t
			}
			body = b
		}
	}
	if strings.TrimSpace(body) == "" {
		body = githubPRMarkdown(prFallbackIntro(l.Goal, l.GoalDisplay), commits, len(files), add, del, notes)
	}
	newPR := prID == fmt.Sprintf("%d", prNum) && source == domain.SourceLocal
	if newPR && proj.PROnGitHub {
		ghNum, ok, reason := l.eng.CreateGitHubPR(ctx, proj.RepoPath, l.Worktree, branch, base,
			title, body, false)
		if ok {
			source = domain.SourceGitHub
			prNum = ghNum
			prID = githubImportedPRID(l.ProjectID, ghNum)
		} else {
			l.emitFeed("MA", "Maestro", "GitHub PR not opened ("+html.EscapeString(reason)+") — opening a Local PR instead.")
		}
	} else if source == domain.SourceGitHub {
		head := ghPR.Branch
		if head == "" {
			head = branch
		}
		// Regular push only — divergence must not force-push. A reviewer's
		// suggestion commit on the PR would be dropped, and unlike the
		// titlebar there is no human clicking Force push.
		if err := pushGitHubHead(ctx, l.Worktree, proj.RepoPath, ghPR, head, false); err != nil {
			l.emitFeed("MA", "Maestro", "Could not push the rework to GitHub: "+html.EscapeString(strings.TrimSpace(err.Error())))
		}
		// Keep PR.Branch as the frozen GitHub remote head.
		branch = head
	}

	// Observatory: project open findings onto the diff, compute impact, run checks.
	findings := l.eng.Store.Findings(l.ID)
	_ = observe.ProjectFindingsOntoDiff(files, findings)
	var impact []domain.ImpactItem
	var extraChecks []domain.Check
	var analysisDelta *domain.MetricDelta
	if l.Worktree != "" {
		if root, err := code.ResolveRoot("", l.Worktree); err == nil {
			impact = observe.ComputeImpact(ctx, root, files)
			cctx, cancel := context.WithTimeout(ctx, 25*time.Second)
			extraChecks = observe.RunLightChecks(cctx, root, loop.TestResult)
			cancel()
			var prev *domain.AnalysisRun
			if a, ok := l.eng.Store.LatestAnalysis(l.ProjectID, ""); ok {
				prev = &a
			}
			run := observe.Analyze(ctx, observe.AnalyzeOpts{
				ProjectID: l.ProjectID, WorkspaceID: l.ID, Root: root,
				Files: files, TestResult: loop.TestResult, RunChecks: false,
				Previous: prev,
			})
			run.Impact = impact
			run.Checks = extraChecks
			analysisDelta = run.Delta
			l.eng.Store.AddAnalysis(run)
		}
	}

	checks := engineChecks(loop, len(commits))
	checks = append(checks, extraChecks...)
	checksOK := allChecksPassed(checks)
	// GitHub PRs show the PR's GH check runs, not Recur-local typecheck/review.
	if source == domain.SourceGitHub {
		checks, checksOK = l.eng.githubPRChecks(ctx, proj.RepoPath, prNum)
	}

	pr := domain.PR{
		ID:          prID,
		WorkspaceID: l.ID,
		ProjectID:   l.ProjectID,
		Group:       "you", // awaiting you
		Number:      prNum,
		Source:      source,
		Branch:      branch,
		Base:        base,
		Title:       title,
		Summary:     engineSummary(label, len(files), add, del, len(commits)),
		Focus:       engineFocus(files, impact, analysisDelta),
		State:       "In review",
		Diff:        domain.DiffStat{Add: add, Del: del},
		FileCount:   len(files),
		ChecksOK:    checksOK,
		HasChecks:   len(checks) > 0,
		When:        "now",
		Commits:     commits,
		Files:       files,
		Comments:    l.preservedComments(prID),
		Verify:      append([]domain.VerifyStep(nil), loop.Verify...),
		Checks:      checks,
		Impact:      impact,
	}
	if source != domain.SourceGitHub {
		pr.AttentionReasons = []string{domain.AttentionReviewRequested}
		if prDescription(pr) == "" {
			upsertDescriptionComment(&pr, body)
		}
	}
	pr.Participants = participantsFromPR(pr)
	pr.Reviewers = commentsReviewers(pr.Comments)

	saved, err := l.eng.Store.AddPR(pr)
	if err != nil {
		return "", fmt.Errorf("persist PR: %w", err)
	}
	l.eng.dropUnmergedSiblingPRs(l.ID, saved.ID)

	elapsed := time.Duration(0)
	if !l.startedAt.IsZero() {
		elapsed = time.Since(l.startedAt)
	}
	l.eng.emitFeedPRCreated(l.ID, saved, elapsed)
	l.eng.publishPRUpdated(saved.ID)

	// Actionable inbox row (PRD §16.11, §21.5 / D5): a converged+verified PR is
	// "ready for you". Ordinary progress stays in the loop feed, NOT the inbox.
	l.eng.emitPRReadyInbox(saved)
	return saved.ID, nil
}

// fillCommitDiffs attaches each commit's own file diff (git show) so the Commits
// tab can drill in. Best effort: a per-commit diff error leaves that commit's
// Files empty rather than failing the whole list.
func (l *Loop) fillCommitDiffs(ctx context.Context, commits []domain.Commit) []domain.Commit {
	for i := range commits {
		if cf, err := l.git.CommitDiff(ctx, l.Worktree, commits[i].Hash); err == nil {
			commits[i].Files = cf
		}
		if commits[i].When == "" {
			commits[i].When = "now"
		}
	}
	return commits
}

// preservedComments carries forward the comment thread from an existing PR for this
// loop (a rework round re-creates the PR but must keep the conversation history).
func (l *Loop) preservedComments(prID string) []domain.PRComment {
	if existing, ok := l.eng.Store.PR(prID); ok {
		return append([]domain.PRComment(nil), existing.Comments...)
	}
	return nil
}

const prThreadContextMax = 8000

// formatPRThread is the review conversation + line notes the conductor sees.
func formatPRThread(pr domain.PR) string {
	var b strings.Builder
	if pr.Number > 0 {
		fmt.Fprintf(&b, "PR #%d", pr.Number)
		if t := strings.TrimSpace(pr.Title); t != "" {
			fmt.Fprintf(&b, " %q", t)
		}
		b.WriteByte('\n')
	}
	wrote := false
	for _, c := range pr.Comments {
		if c.Kind == "description" || c.Kind == "action" || c.CommitSHA != "" {
			continue
		}
		body := strings.TrimSpace(c.BodyMD)
		if body == "" {
			body = strings.TrimSpace(stripTags(c.BodyHTML))
		}
		if body == "" {
			continue
		}
		who := strings.TrimSpace(c.Author)
		if who == "" {
			who = "comment"
		}
		if c.IsLineNote() {
			loc := strings.TrimSpace(c.Path)
			if c.Line > 0 {
				if loc != "" {
					loc = fmt.Sprintf("%s:%d", loc, c.Line)
				} else {
					loc = fmt.Sprintf("line %d", c.Line)
				}
			}
			if loc != "" {
				who += " on " + loc
			}
			if c.Resolved {
				who += " (resolved)"
			}
		}
		fmt.Fprintf(&b, "- %s: %s\n", who, truncate(body, 400))
		wrote = true
	}
	for _, pc := range pr.PendingComments {
		body := strings.TrimSpace(pc.Body)
		if body == "" {
			continue
		}
		loc := strings.TrimSpace(pc.File)
		if pc.Line > 0 {
			if loc != "" {
				loc = fmt.Sprintf("%s:%d", loc, pc.Line)
			} else {
				loc = fmt.Sprintf("line %d", pc.Line)
			}
		}
		if loc != "" {
			fmt.Fprintf(&b, "- pending on %s: %s\n", loc, truncate(body, 400))
		} else {
			fmt.Fprintf(&b, "- pending: %s\n", truncate(body, 400))
		}
		wrote = true
	}
	if !wrote {
		return ""
	}
	out := b.String()
	if utf8.RuneCountInString(out) > prThreadContextMax {
		runes := []rune(out)
		out = string(runes[:prThreadContextMax]) + "\n…"
	}
	return out
}

// prTitle is the first line of the loop goal, capped at 80 runes. GitHub
// titles max out at 256 characters; 80 matches agent-written titles.
func prTitle(goal string) string {
	g := hardTruncateRunes(singleLine(goal), 80)
	if g == "" {
		return "Loop changes"
	}
	return g
}

// githubPRTitleMax is GitHub's PR title character limit (Unicode characters).
const githubPRTitleMax = 256

// githubPRBodyMax is well under GitHub's 65536-character PR body limit.
const githubPRBodyMax = 60000

// prFallbackIntro is the human-facing first paragraph of an engine-generated
// PR body. Prompt-action walks keep the recipe in Goal and the UI label in
// GoalDisplay — the recipe is not a pull-request description.
func prFallbackIntro(goal, display string) string {
	if strings.TrimSpace(display) != "" {
		return ""
	}
	return strings.TrimSpace(goal)
}

// githubPRMarkdown writes a factual PR description from the diff when the
// /open-pr one-shot is empty. An agent-written body (mcp__recur__open_pr)
// replaces this; this fallback must never dump prompt-action wire or a stub
// like "Opened by Recur".
func githubPRMarkdown(goal string, commits []domain.Commit, fileCount, add, del int, notes []string) string {
	var b strings.Builder
	if g := strings.TrimSpace(goal); g != "" {
		b.WriteString(g)
		b.WriteString("\n\n")
	}
	b.WriteString("## Changes\n\n")
	fmt.Fprintf(&b, "- %s across %s\n",
		plural(len(commits), "commit", "commits"), plural(fileCount, "file", "files"))
	fmt.Fprintf(&b, "- Net +%d −%d versus the base branch\n", add, del)
	if subjects := commitSubjects(commits); len(subjects) > 0 {
		b.WriteString("\n## Commits\n\n")
		for _, s := range subjects {
			b.WriteString("- ")
			b.WriteString(s)
			b.WriteByte('\n')
		}
	}
	for _, n := range notes {
		n = strings.TrimSpace(n)
		if n == "" {
			continue
		}
		b.WriteByte('\n')
		b.WriteString(n)
		b.WriteByte('\n')
	}
	return capGitHubPRBody(strings.TrimSpace(b.String()))
}

func capGitHubPRBody(s string) string {
	if utf8.RuneCountInString(s) <= githubPRBodyMax {
		return s
	}
	return hardTruncateRunes(s, githubPRBodyMax) + "\n\n…(truncated)…"
}

// engineSummary writes a reasonable Maestro-style PR summary for now (a real
// LLM-written summary is M7, PRD §7.2). It states what the loop did and the diff
// shape so the human has a one-read overview. HTML in the bullets is engine-owned
// (trusted) but the goal is escaped since it is user-supplied.
func engineSummary(goal string, fileCount, add, del, commitCount int) []domain.SummaryPara {
	g := html.EscapeString(strings.TrimSpace(goal))
	return []domain.SummaryPara{
		{Kind: "p", HTML: fmt.Sprintf("This loop implemented: <b>%s</b>.", g)},
		{Kind: "p", HTML: "<b>What changed</b>"},
		{Kind: "bullets", Items: []string{
			fmt.Sprintf("%s across %s.",
				plural(commitCount, "commit", "commits"), plural(fileCount, "file", "files")),
			fmt.Sprintf("Net diff <code>+%d −%d</code> on the loop branch vs its base.", add, del),
			"The change converged to 0 independent-review findings before this PR opened.",
		}},
		{Kind: "p", HTML: "<b>How it converged:</b> the Coder and an independent Reviewer iterated code↔review until the Reviewer filed zero findings; the engine — not the Coder — declared convergence."},
	}
}

// engineFocus writes the "where to focus your review" callout (PRD §12.2). It
// points at the largest changed file, and when observatory impact is available,
// mentions the blast radius (symbols/tests touched).
func engineFocus(files []domain.FileDiff, impact []domain.ImpactItem, delta *domain.MetricDelta) domain.FocusNote {
	if len(files) == 0 {
		return domain.FocusNote{Title: "Where to focus your review", Body: "No file changes were produced."}
	}
	biggest := files[0]
	for _, f := range files {
		if f.Diff.Add+f.Diff.Del > biggest.Diff.Add+biggest.Diff.Del {
			biggest = f
		}
	}
	body := fmt.Sprintf("The bulk of the change is in <code>%s</code> (+%d −%d) — start there.",
		html.EscapeString(biggest.Path), biggest.Diff.Add, biggest.Diff.Del)
	symN, apiN, testN := 0, 0, 0
	for _, it := range impact {
		switch it.Kind {
		case "symbol":
			symN++
		case "api":
			apiN++
		case "test":
			testN++
		}
	}
	if symN+apiN+testN > 0 {
		body += fmt.Sprintf(" Blast radius: <b>%d</b> defs · <b>%d</b> refs · <b>%d</b> tests.", symN, apiN, testN)
	}
	if delta != nil && (delta.LOC != 0 || delta.MaxComplexity != 0 || delta.Hotspots != 0) {
		body += fmt.Sprintf(" Health Δ: LOC %+d · max complexity %+d · hotspots %+d.",
			delta.LOC, delta.MaxComplexity, delta.Hotspots)
	}
	return domain.FocusNote{Title: "Where to focus your review", Body: body}
}

// engineChecks builds the Recur checks rollup for a Local PR (PRD §12.2 Checks):
// independent review (always 0 findings at pr_open, by rail #3) + verify-in-browser
// (from the loop's M5 evidence, when rail #1 ran). GitHub PRs use the GitHub
// Checks tab rollup instead (see githubPRChecks).
func engineChecks(loop domain.Workspace, commitCount int) []domain.Check {
	iter := loop.Iteration
	if iter == 0 {
		iter = len(loop.Convergence)
	}
	checks := []domain.Check{
		{Name: "Independent review", Detail: fmt.Sprintf("0 findings · iter %d", iter), Passed: true},
	}
	switch loop.VerifyState {
	case verifyStatePassed:
		checks = append(checks, domain.Check{
			Name: "Verify in browser", Detail: fmt.Sprintf("%s ✓", plural(len(loop.Verify), "step", "steps")), Passed: true,
		})
	case verifyStateFailed:
		checks = append(checks, domain.Check{
			Name: "Verify in browser", Detail: "did not pass", Passed: false,
		})
	}
	return checks
}

func allChecksPassed(checks []domain.Check) bool {
	for _, c := range checks {
		if c.Status == "pending" || c.Status == "skipping" {
			continue
		}
		if !c.Passed {
			return false
		}
	}
	return true
}

func checksEqual(a, b []domain.Check) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

// ---- engine-side PR service methods (called by the bound PRsService) ----

// AddPRComment records a review note. A file+line note is pending until
// SubmitReview (or SendBackToLoop). A conversation-only note (no file) still
// posts immediately. A negative line means the LEFT (old) side of a split;
// empty body removes the pending note whose ID or FindingID is findingID.
// startLine is the first line of a GitHub multiline comment (same sign
// convention as line); 0 means a single line. The body is HTML-escaped.
// findingID tags a line draft queued from an observatory finding so reload
// can still resolve it after submit.
func (e *Engine) AddPRComment(prID, file string, line int, body string, startLine int, findingID string) (domain.PRComment, bool) {
	pr, ok := e.Store.PR(prID)
	if !ok {
		return domain.PRComment{}, false
	}
	file = strings.TrimSpace(file)
	body = strings.TrimSpace(body)
	if pr.ConversationLocked() && body != "" {
		return domain.PRComment{}, false
	}
	side, line := splitPendingLine(line)
	startSide, start := splitPendingLine(startLine)
	if start <= 0 || start >= line {
		start = 0
		startSide = ""
	} else if startSide == "" {
		startSide = side
	}
	if file != "" && line > 0 {
		return e.upsertPendingLineComment(pr, file, line, side, start, startSide, body, findingID)
	}
	if pr.Source == domain.SourceGitHub {
		return e.addGitHubPRComment(pr, file, line, body)
	}
	c := nowComment("AR", "You", html.EscapeString(body))
	if _, ok := e.Store.UpdatePR(prID, func(p *domain.PR) {
		p.Comments = append(p.Comments, c)
		touchPRUpdated(p)
	}); !ok {
		return domain.PRComment{}, false
	}
	e.publishPRComment(prID, c)
	e.publishPRUpdated(prID)
	return c, true
}

func splitPendingLine(line int) (side string, n int) {
	if line < 0 {
		return "LEFT", -line
	}
	return "RIGHT", line
}

func (e *Engine) upsertPendingLineComment(pr domain.PR, file string, line int, side string, start int, startSide, body, findingID string) (domain.PRComment, bool) {
	if body == "" {
		id := strings.TrimSpace(findingID)
		if id == "" {
			return domain.PRComment{}, false
		}
		removed := false
		if _, ok := e.Store.UpdatePR(pr.ID, func(p *domain.PR) {
			next := make([]domain.PendingLineComment, 0, len(p.PendingComments))
			for _, pc := range p.PendingComments {
				if pc.ID == id || strings.TrimSpace(pc.FindingID) == id {
					removed = true
					continue
				}
				next = append(next, pc)
			}
			if !removed {
				return
			}
			p.PendingComments = next
		}); !ok {
			return domain.PRComment{}, false
		}
		if !removed {
			return domain.PRComment{}, false
		}
		e.publishPRPendingUpdated(pr.ID)
		return domain.PRComment{ID: "removed"}, true
	}
	pc := domain.PendingLineComment{
		ID:        fmt.Sprintf("pend-%d", time.Now().UnixNano()),
		File:      file,
		Line:      line,
		Side:      side,
		StartLine: start,
		StartSide: startSide,
		Body:      body,
		FindingID: strings.TrimSpace(findingID),
	}
	if _, ok := e.Store.UpdatePR(pr.ID, func(p *domain.PR) {
		p.PendingComments = append(p.PendingComments, pc)
	}); !ok {
		return domain.PRComment{}, false
	}
	e.publishPRPendingUpdated(pr.ID)
	return nowComment("AR", "You", html.EscapeString(body)), true
}

func pendingSide(side string) string {
	if strings.EqualFold(side, "LEFT") {
		return "LEFT"
	}
	return "RIGHT"
}

func pendingToComment(pc domain.PendingLineComment, files []domain.FileDiff) domain.PRComment {
	c := nowComment("AR", "You", "<code>"+pendingLoc(pc)+"</code>")
	c.BodyMD = strings.TrimSpace(pc.Body)
	c.Path = pc.File
	c.Line = pc.Line
	c.Side = pendingSide(pc.Side)
	c.StartLine = pc.StartLine
	c.StartSide = pc.StartSide
	c.DiffHunk = commentHunkFromFiles(files, pc.File, pc.Line, c.Side)
	c.ThreadID = c.ID
	return c
}

func pendingLoc(pc domain.PendingLineComment) string {
	loc := html.EscapeString(pc.File)
	if pc.Line <= 0 {
		return loc
	}
	if pc.StartLine > 0 && pc.StartLine < pc.Line {
		return fmt.Sprintf("%s:%d-%d", loc, pc.StartLine, pc.Line)
	}
	return fmt.Sprintf("%s:%d", loc, pc.Line)
}

func commentHunkFromFiles(files []domain.FileDiff, path string, line int, side string) string {
	var file domain.FileDiff
	for _, f := range files {
		if f.Path == path {
			file = f
			break
		}
	}
	if file.Path == "" || line <= 0 {
		return ""
	}
	wantLeft := strings.EqualFold(side, "LEFT")
	var buf []string
	got := false
	for _, ln := range file.Lines {
		if ln.Type == "hunk" {
			if got {
				break
			}
			buf = buf[:0]
			text := visibleDiffText(ln.CodeHTML)
			if text == "" {
				text = "@@"
			}
			buf = append(buf, text)
			continue
		}
		if len(buf) == 0 {
			continue
		}
		n := 0
		prefix := " "
		switch ln.Type {
		case "add":
			prefix = "+"
			n, _ = strconv.Atoi(ln.New)
		case "del":
			prefix = "-"
			n, _ = strconv.Atoi(ln.Old)
		default:
			if wantLeft {
				n, _ = strconv.Atoi(ln.Old)
			} else {
				n, _ = strconv.Atoi(ln.New)
			}
		}
		buf = append(buf, prefix+visibleDiffText(ln.CodeHTML))
		match := false
		if wantLeft {
			match = n == line && ln.Type != "add"
		} else {
			match = n == line && ln.Type != "del"
		}
		if match {
			got = true
			break
		}
	}
	if !got {
		return ""
	}
	return strings.Join(buf, "\n")
}

func visibleDiffText(codeHTML string) string {
	var b strings.Builder
	inTag := false
	for _, r := range codeHTML {
		switch {
		case r == '<':
			inTag = true
		case r == '>':
			inTag = false
		case !inTag:
			b.WriteRune(r)
		}
	}
	return html.UnescapeString(b.String())
}

func (e *Engine) addGitHubPRComment(pr domain.PR, file string, line int, body string) (domain.PRComment, bool) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	if err := postGitHubPRCommentFn(ctx, e.prRepoPath(pr), pr.Number, file, line, body); err != nil {
		e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro",
			"Could not post the comment to GitHub: "+html.EscapeString(strings.TrimSpace(err.Error())))
		return domain.PRComment{}, false
	}
	e.RefreshPRConversation(pr.ID)
	e.publishPRUpdated(pr.ID)
	updated, _ := e.Store.PR(pr.ID)
	if n := len(updated.Comments); n > 0 {
		return updated.Comments[n-1], true
	}
	return nowComment("AR", "You", html.EscapeString(strings.TrimSpace(body))), true
}

// SubmitReview persists a Comment/Approve/Request-changes review as a thread entry
// (PRD §12.2, §14) and emits pr.comment/pr.updated. Approve/Request-changes reflect
// on the PR state words; the kind is recorded in the comment so reload shows it.
func (e *Engine) SubmitReview(prID, kind, body string) (domain.PR, bool) {
	pr, ok := e.Store.PR(prID)
	if !ok {
		return domain.PR{}, false
	}
	if pr.ConversationLocked() {
		return pr, false
	}
	if pr.Source == domain.SourceGitHub {
		if (kind == "approve" || kind == "request") && !pr.CanApproveReview() {
			return pr, false
		}
		return e.submitGitHubReview(pr, kind, body)
	}
	_, stateWords := reviewLabelState(kind)
	c := nowComment("AR", "You", html.EscapeString(strings.TrimSpace(body)))
	c.Verb = reviewCardVerb(kind)
	var flushed []domain.PRComment
	var pending []domain.PendingLineComment
	updated, ok := e.Store.UpdatePR(prID, func(p *domain.PR) {
		pending = append([]domain.PendingLineComment(nil), p.PendingComments...)
		for _, pc := range p.PendingComments {
			fc := pendingToComment(pc, p.Files)
			fc.ParentID = c.ID
			flushed = append(flushed, fc)
		}
		p.Comments = append(p.Comments, c)
		p.Comments = append(p.Comments, flushed...)
		p.PendingComments = nil
		if stateWords != "" {
			p.State = stateWords
		}
		p.Reviewers = commentsReviewers(p.Comments)
		touchPRUpdated(p)
	})
	if !ok {
		return domain.PR{}, false
	}
	e.resolvePendingFindings(updated, pending)
	for _, fc := range flushed {
		e.publishPRComment(prID, fc)
	}
	e.publishPRComment(prID, c)
	e.publishPRUpdated(prID)
	return updated, true
}

func (e *Engine) submitGitHubReview(pr domain.PR, kind, body string) (domain.PR, bool) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	pending := append([]domain.PendingLineComment(nil), pr.PendingComments...)
	if err := submitGitHubPRReviewFn(ctx, e.prRepoPath(pr), pr.Number, kind, body, pr.PendingComments); err != nil {
		e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro",
			"Could not submit the review to GitHub: "+html.EscapeString(strings.TrimSpace(err.Error())))
		return pr, false
	}
	e.resolvePendingFindings(pr, pending)
	e.Store.UpdatePR(pr.ID, func(p *domain.PR) { p.PendingComments = nil })
	e.RefreshPRConversation(pr.ID)
	e.publishPRUpdated(pr.ID)
	updated, ok := e.Store.PR(pr.ID)
	if !ok {
		return pr, true
	}
	return updated, true
}

// resolvePendingFindings marks still-open observatory findings resolved once
// their drafts have been submitted. Ignored/invalid stay put. Runs against a
// preserved pending slice so a failed conversation refresh cannot lose the conversion.
func (e *Engine) resolvePendingFindings(pr domain.PR, pending []domain.PendingLineComment) {
	var ids []string
	seen := map[string]bool{}
	for _, pc := range pending {
		id := strings.TrimSpace(pc.FindingID)
		if id == "" || seen[id] {
			continue
		}
		seen[id] = true
		ids = append(ids, id)
	}
	if len(ids) == 0 {
		return
	}
	wsID := e.PRFindingsWorkspaceID(pr)
	if wsID == "" {
		return
	}
	n := 0
	for _, id := range ids {
		got, err := e.Store.UpdateFindingStatus(wsID, id, string(domain.FindingResolved), "", domain.FindingActorEngine)
		if err != nil || got.Status != string(domain.FindingResolved) {
			continue
		}
		n++
	}
	if n == 0 {
		return
	}
	e.Bus.Publish(Event{Name: "findings.updated", Data: map[string]any{"workspaceId": wsID}})
}

// PRFindingsWorkspaceID is the workspace findings are stored on for this PR:
// a live linked workspace, else the chat host that filed them before promote.
func (e *Engine) PRFindingsWorkspaceID(p domain.PR) string {
	if id := strings.TrimSpace(p.WorkspaceID); id != "" {
		if ws, ok := e.Store.Workspace(id); ok && !ws.Archived {
			return id
		}
	}
	if host, ok := e.prChatHost(p.ID); ok {
		return host.ID
	}
	return strings.TrimSpace(p.WorkspaceID)
}

// UpdatePRDetails changes the title and description. GitHub PRs go through
// `gh pr edit` when the viewer is the author or has write access.
func (e *Engine) UpdatePRDetails(prID, title, body string) (domain.PR, bool) {
	title = strings.TrimSpace(title)
	if title == "" {
		return domain.PR{}, false
	}
	pr, ok := e.Store.PR(prID)
	if !ok || !pr.CanEditDetails() {
		return domain.PR{}, false
	}
	if pr.Source == domain.SourceGitHub {
		return e.updateGitHubPRDetails(pr, title, body)
	}
	updated, ok := e.Store.UpdatePR(prID, func(p *domain.PR) {
		p.Title = title
		upsertDescriptionComment(p, body)
		touchPRUpdated(p)
	})
	if !ok {
		return domain.PR{}, false
	}
	e.publishPRUpdated(prID)
	return updated, true
}

func (e *Engine) updateGitHubPRDetails(pr domain.PR, title, body string) (domain.PR, bool) {
	curBody := ""
	for _, c := range pr.Comments {
		if c.Kind == "description" {
			curBody = c.BodyMD
			break
		}
	}
	titleChanged := title != strings.TrimSpace(pr.Title)
	bodyChanged := strings.TrimSpace(body) != strings.TrimSpace(curBody)
	if !titleChanged && !bodyChanged {
		return pr, true
	}
	editBody := body
	if !bodyChanged {
		editBody = omitPREditBody
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	if err := editGitHubPRFn(ctx, e.prRepoPath(pr), pr.Number, title, editBody); err != nil {
		e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro",
			"Could not update the pull request on GitHub: "+html.EscapeString(strings.TrimSpace(err.Error())))
		return pr, false
	}
	updated, ok := e.Store.UpdatePR(pr.ID, func(p *domain.PR) {
		p.Title = title
		if bodyChanged {
			upsertDescriptionComment(p, body)
		}
		touchPRUpdated(p)
	})
	if !ok {
		return pr, true
	}
	e.publishPRUpdated(pr.ID)
	return updated, true
}

// UpdatePRComment edits a conversation comment the viewer owns.
func (e *Engine) UpdatePRComment(prID, commentID, body string) (domain.PRComment, bool) {
	body = strings.TrimSpace(body)
	commentID = strings.TrimSpace(commentID)
	if body == "" || commentID == "" {
		return domain.PRComment{}, false
	}
	pr, ok := e.Store.PR(prID)
	if !ok {
		return domain.PRComment{}, false
	}
	idx := findPRComment(pr.Comments, commentID)
	if idx < 0 || !pr.CanEditComment(pr.Comments[idx]) {
		return domain.PRComment{}, false
	}
	if pr.Source == domain.SourceGitHub {
		return e.updateGitHubPRComment(pr, commentID, body)
	}
	var saved domain.PRComment
	if _, ok := e.Store.UpdatePR(prID, func(p *domain.PR) {
		i := findPRComment(p.Comments, commentID)
		if i < 0 {
			return
		}
		p.Comments[i].BodyMD = body
		if p.Comments[i].Path == "" {
			p.Comments[i].BodyHTML = html.EscapeString(body)
		}
		saved = p.Comments[i]
		touchPRUpdated(p)
	}); !ok || saved.ID == "" {
		return domain.PRComment{}, false
	}
	e.publishPRThreadUpdated(prID)
	return saved, true
}

func (e *Engine) updateGitHubPRComment(pr domain.PR, commentID, body string) (domain.PRComment, bool) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	if err := editGitHubPRCommentFn(ctx, e.prRepoPath(pr), pr.Number, commentID, body); err != nil {
		e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro",
			"Could not update the comment on GitHub: "+html.EscapeString(strings.TrimSpace(err.Error())))
		return domain.PRComment{}, false
	}
	var saved domain.PRComment
	if _, ok := e.Store.UpdatePR(pr.ID, func(p *domain.PR) {
		i := findPRComment(p.Comments, commentID)
		if i < 0 {
			return
		}
		p.Comments[i].BodyMD = body
		saved = p.Comments[i]
		touchPRUpdated(p)
	}); !ok {
		return domain.PRComment{ID: commentID, BodyMD: body}, true
	}
	e.publishPRThreadUpdated(pr.ID)
	if saved.ID == "" {
		return domain.PRComment{ID: commentID, BodyMD: body}, true
	}
	return saved, true
}

func (e *Engine) DeletePRComment(prID, commentID string) (domain.PR, bool) {
	commentID = strings.TrimSpace(commentID)
	if commentID == "" {
		return domain.PR{}, false
	}
	pr, ok := e.Store.PR(prID)
	if !ok {
		return domain.PR{}, false
	}
	idx := findPRComment(pr.Comments, commentID)
	if idx < 0 || !pr.CanDeleteComment(pr.Comments[idx]) {
		return domain.PR{}, false
	}
	if pr.Source == domain.SourceGitHub {
		return e.deleteGitHubPRComment(pr, commentID)
	}
	updated, ok := e.Store.UpdatePR(prID, func(p *domain.PR) {
		p.Comments = removePRComment(p.Comments, commentID)
		touchPRUpdated(p)
	})
	if !ok {
		return domain.PR{}, false
	}
	e.publishPRThreadUpdated(prID)
	return updated, true
}

func (e *Engine) deleteGitHubPRComment(pr domain.PR, commentID string) (domain.PR, bool) {
	ids := commentIDsToRemove(pr.Comments, commentID)
	taken := takePRComments(pr.Comments, ids)
	e.rememberDroppedComments(pr.ID, ids)
	updated, ok := e.Store.UpdatePR(pr.ID, func(p *domain.PR) {
		p.Comments = removePRComment(p.Comments, commentID)
		touchPRUpdated(p)
	})
	if !ok {
		e.forgetDroppedComments(pr.ID, ids)
		return domain.PR{}, false
	}
	e.publishPRThreadUpdated(pr.ID)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	if err := deleteGitHubPRCommentFn(ctx, e.prRepoPath(pr), pr.Number, commentID); err != nil {
		e.forgetDroppedComments(pr.ID, ids)
		e.Store.UpdatePR(pr.ID, func(p *domain.PR) {
			p.Comments = append(p.Comments, taken...)
			p.UpdatedAt = pr.UpdatedAt
		})
		e.publishPRThreadUpdated(pr.ID)
		e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro",
			"Could not delete the comment on GitHub: "+html.EscapeString(strings.TrimSpace(err.Error())))
		return domain.PR{}, false
	}
	return updated, true
}

func (e *Engine) ReplyPRComment(prID, commentID, body string) (domain.PRComment, bool) {
	body = strings.TrimSpace(body)
	commentID = strings.TrimSpace(commentID)
	if body == "" || commentID == "" {
		return domain.PRComment{}, false
	}
	pr, ok := e.Store.PR(prID)
	if !ok {
		return domain.PRComment{}, false
	}
	if pr.ConversationLocked() {
		return domain.PRComment{}, false
	}
	root := findLineThreadRoot(pr.Comments, commentID)
	if root.ID == "" || !root.IsLineNote() {
		return domain.PRComment{}, false
	}
	if pr.Source == domain.SourceGitHub {
		return e.replyGitHubPRComment(pr, root.ID, body)
	}
	c := nowComment("AR", "You", html.EscapeString(body))
	c.BodyMD = body
	attachLineReply(&c, root)
	if _, ok := e.Store.UpdatePR(prID, func(p *domain.PR) {
		p.Comments = append(p.Comments, c)
		touchPRUpdated(p)
	}); !ok {
		return domain.PRComment{}, false
	}
	e.publishPRComment(prID, c)
	e.publishPRThreadUpdated(prID)
	return c, true
}

func (e *Engine) replyGitHubPRComment(pr domain.PR, commentID, body string) (domain.PRComment, bool) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	raw, err := replyGitHubPRCommentFn(ctx, e.prRepoPath(pr), pr.Number, commentID, body)
	if err != nil {
		e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro",
			"Could not reply on GitHub: "+html.EscapeString(strings.TrimSpace(err.Error())))
		return domain.PRComment{}, false
	}
	root := findLineThreadRoot(pr.Comments, commentID)
	c, ok := parsePostedInlineComment(raw, time.Now(), pr.URL)
	if !ok {
		c = nowComment("AR", "You", html.EscapeString(body))
		c.BodyMD = body
		c.AuthorLogin = pr.ViewerLogin
	}
	attachLineReply(&c, root)
	if _, ok := e.Store.UpdatePR(pr.ID, func(p *domain.PR) {
		p.Comments = append(p.Comments, c)
		touchPRUpdated(p)
	}); !ok {
		return domain.PRComment{}, false
	}
	e.publishPRComment(pr.ID, c)
	e.publishPRThreadUpdated(pr.ID)
	return c, true
}

func attachLineReply(c *domain.PRComment, root domain.PRComment) {
	c.InReplyToID = root.ID
	c.ParentID = root.ParentID
	c.ThreadID = root.ThreadID
	if c.ThreadID == "" {
		c.ThreadID = root.ID
	}
	c.Resolved = root.Resolved
	if c.Path == "" {
		c.Path = root.Path
		c.Line = root.Line
		c.Side = root.Side
	}
}

// TogglePRCommentReaction adds or removes the viewer's emoji on a conversation item.
func (e *Engine) TogglePRCommentReaction(prID, commentID, content string) (domain.PR, bool) {
	content, ok := domain.NormalizeReactionContent(content)
	if !ok {
		return domain.PR{}, false
	}
	commentID = strings.TrimSpace(commentID)
	if commentID == "" {
		return domain.PR{}, false
	}
	pr, ok := e.Store.PR(prID)
	if !ok {
		return domain.PR{}, false
	}
	idx := findPRComment(pr.Comments, commentID)
	if idx < 0 || !pr.CanReactComment(pr.Comments[idx]) {
		return domain.PR{}, false
	}
	next := domain.ToggleReactions(pr.Comments[idx].Reactions, content)
	adding := domain.ReactionMine(next, content)
	if pr.Source == domain.SourceGitHub {
		return e.toggleGitHubPRCommentReaction(pr, commentID, content, adding, next)
	}
	updated, ok := e.Store.UpdatePR(prID, func(p *domain.PR) {
		i := findPRComment(p.Comments, commentID)
		if i < 0 {
			return
		}
		p.Comments[i].Reactions = next
		touchPRUpdated(p)
	})
	if !ok {
		return domain.PR{}, false
	}
	e.publishPRThreadUpdated(prID)
	return updated, true
}

func (e *Engine) toggleGitHubPRCommentReaction(pr domain.PR, commentID, content string, adding bool, next []domain.PRReaction) (domain.PR, bool) {
	idx := findPRComment(pr.Comments, commentID)
	if idx < 0 {
		return domain.PR{}, false
	}
	subject := reactionSubjectID(pr.Comments[idx], pr)
	if subject == "" {
		return domain.PR{}, false
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	if err := setGitHubReactionFn(ctx, e.prRepoPath(pr), subject, content, adding); err != nil {
		e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro",
			"Could not update the reaction on GitHub: "+html.EscapeString(strings.TrimSpace(err.Error())))
		return domain.PR{}, false
	}
	e.rememberCommentReactions(pr.ID, commentID, next)
	updated, ok := e.Store.UpdatePR(pr.ID, func(p *domain.PR) {
		i := findPRComment(p.Comments, commentID)
		if i < 0 {
			return
		}
		p.Comments[i].Reactions = next
		touchPRUpdated(p)
	})
	if !ok {
		e.forgetCommentReactions(pr.ID, commentID)
		return domain.PR{}, false
	}
	e.publishPRThreadUpdated(pr.ID)
	return updated, true
}

func (e *Engine) SetPRThreadResolved(prID, threadID string, resolved bool) (domain.PR, bool) {
	threadID = strings.TrimSpace(threadID)
	if threadID == "" {
		return domain.PR{}, false
	}
	pr, ok := e.Store.PR(prID)
	if !ok || !pr.CanResolveThread() {
		return domain.PR{}, false
	}
	key := resolveThreadKey(pr.Comments, threadID)
	if key == "" {
		return domain.PR{}, false
	}
	if pr.Source == domain.SourceGitHub {
		return e.setGitHubThreadResolved(pr, key, resolved)
	}
	updated, ok := e.Store.UpdatePR(prID, func(p *domain.PR) {
		markThreadResolved(p.Comments, key, resolved)
		touchPRUpdated(p)
	})
	if !ok {
		return domain.PR{}, false
	}
	e.publishPRThreadUpdated(prID)
	return updated, true
}

func (e *Engine) setGitHubThreadResolved(pr domain.PR, threadID string, resolved bool) (domain.PR, bool) {
	if strings.HasPrefix(threadID, "gh-") || strings.HasPrefix(threadID, "c-") {
		return domain.PR{}, false
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	if err := setGitHubReviewThreadFn(ctx, e.prRepoPath(pr), threadID, resolved); err != nil {
		e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro",
			"Could not update the conversation on GitHub: "+html.EscapeString(strings.TrimSpace(err.Error())))
		return domain.PR{}, false
	}
	e.rememberThreadResolved(pr.ID, threadID, resolved)
	updated, ok := e.Store.UpdatePR(pr.ID, func(p *domain.PR) {
		markThreadResolved(p.Comments, threadID, resolved)
		touchPRUpdated(p)
	})
	if !ok {
		e.forgetThreadResolved(pr.ID, threadID)
		return domain.PR{}, false
	}
	e.publishPRThreadUpdated(pr.ID)
	return updated, true
}

func resolveThreadKey(comments []domain.PRComment, id string) string {
	for _, c := range comments {
		if c.ThreadID == id {
			return c.ThreadID
		}
	}
	root := findLineThreadRoot(comments, id)
	if root.ID == "" {
		return ""
	}
	if root.ThreadID != "" {
		return root.ThreadID
	}
	if root.IsLineNote() {
		return root.ID
	}
	return ""
}

func markThreadResolved(comments []domain.PRComment, threadID string, resolved bool) {
	for i, c := range comments {
		match := c.ThreadID == threadID || c.ID == threadID
		if !match && c.IsLineNote() {
			root := findLineThreadRoot(comments, c.ID)
			match = root.ID == threadID || root.ThreadID == threadID
		}
		if !match {
			continue
		}
		comments[i].Resolved = resolved
		if comments[i].ThreadID == "" {
			comments[i].ThreadID = threadID
		}
	}
}

func findLineThreadRoot(comments []domain.PRComment, id string) domain.PRComment {
	byID := map[string]domain.PRComment{}
	for _, c := range comments {
		byID[c.ID] = c
	}
	cur, ok := byID[id]
	if !ok {
		return domain.PRComment{}
	}
	seen := map[string]bool{}
	for cur.InReplyToID != "" && !seen[cur.ID] {
		seen[cur.ID] = true
		next, ok := byID[cur.InReplyToID]
		if !ok {
			break
		}
		cur = next
	}
	return cur
}

func commentIDsToRemove(comments []domain.PRComment, id string) []string {
	ids := []string{id}
	for _, c := range comments {
		if c.InReplyToID == id {
			ids = append(ids, c.ID)
		}
	}
	return ids
}

func takePRComments(comments []domain.PRComment, ids []string) []domain.PRComment {
	drop := map[string]bool{}
	for _, id := range ids {
		drop[id] = true
	}
	var taken []domain.PRComment
	for _, c := range comments {
		if drop[c.ID] {
			taken = append(taken, c)
		}
	}
	return taken
}

func removePRComment(comments []domain.PRComment, id string) []domain.PRComment {
	drop := map[string]bool{}
	for _, rid := range commentIDsToRemove(comments, id) {
		drop[rid] = true
	}
	out := make([]domain.PRComment, 0, len(comments))
	for _, c := range comments {
		if drop[c.ID] {
			continue
		}
		out = append(out, c)
	}
	return out
}

func findPRComment(comments []domain.PRComment, id string) int {
	for i, c := range comments {
		if c.ID == id {
			return i
		}
	}
	return -1
}

func upsertDescriptionComment(p *domain.PR, body string) {
	body = strings.TrimSpace(body)
	for i := range p.Comments {
		if p.Comments[i].Kind == "description" {
			p.Comments[i].BodyMD = body
			p.Comments[i].BodyHTML = ""
			return
		}
	}
	if body == "" {
		return
	}
	p.Comments = append([]domain.PRComment{{
		ID: "gh-description", Author: "You", Kind: "description",
		BodyMD: body, Time: "now",
	}}, p.Comments...)
}

// reviewLabelState maps a review kind to a human label + the PR state words it
// implies. Comment leaves state unchanged ("").
func reviewLabelState(kind string) (label, stateWords string) {
	switch kind {
	case "approve":
		return "Approved", "Approved"
	case "request":
		return "Requested changes", "Changes requested"
	default:
		return "Commented", ""
	}
}

func reviewCardVerb(kind string) string {
	switch kind {
	case "approve":
		return "approved"
	case "request":
		return "requested changes"
	default:
		return "commented"
	}
}

// MergePR merges a Local PR's branch into its base ON DISK (PRD §12.1) or, for a
// GitHub PR, via the gh CLI (PRD §12, M6 brief F). On a successful Local merge the
// base ref advances on disk, the PR is marked merged, and the loop is marked merged.
//
// method is the merge strategy ("merge"/"squash"/"rebase"). Empty = resolve from
// project/settings preferences (D53).
func (e *Engine) MergePR(prID string, method domain.MergeMethod) (domain.PR, bool) {
	pr, ok := e.Store.PR(prID)
	if !ok {
		return domain.PR{}, false
	}
	if pr.State == "Merged" {
		return pr, true // idempotent
	}
	if pr.State == "Closed" {
		e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro",
			"Merge refused: this GitHub PR is closed.")
		return pr, false
	}

	// Resolve merge method from prefs when the caller didn't specify one.
	if method == "" {
		proj, _ := e.Store.Project(e.prProjectID(pr))
		method = domain.EffectiveMergeMethod(proj, e.settings())
	} else {
		method = domain.NormalizeMergeMethod(string(method))
	}

	switch pr.Source {
	case domain.SourceGitHub:
		return e.mergeGitHub(pr, method)
	default:
		return e.mergeLocal(pr, method)
	}
}

// ReadyPR marks a GitHub draft PR ready for review via `gh pr ready`.
func (e *Engine) ReadyPR(prID string) (domain.PR, bool) {
	return e.SetPRDraft(prID, false)
}

// SetPRDraft converts a PR to a draft or marks it ready for review.
// GitHub: `gh pr ready` / `gh pr ready --undo`. Local: persist the flag so a
// later GitHub open keeps the same intent.
func (e *Engine) SetPRDraft(prID string, draft bool) (domain.PR, bool) {
	pr, ok := e.Store.PR(prID)
	if !ok {
		return domain.PR{}, false
	}
	if pr.State == "Merged" || pr.State == "Closed" || pr.Group == "merged" {
		e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro",
			"Can't change draft state on a merged or closed PR.")
		return pr, false
	}
	if pr.Draft == draft {
		return pr, true
	}
	if pr.Source == domain.SourceGitHub && pr.Number > 0 {
		repoPath := e.prRepoPath(pr)
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		if err := setGitHubPRDraftFn(ctx, repoPath, pr.Number, draft); err != nil {
			verb := "Mark ready failed: "
			if draft {
				verb = "Convert to draft failed: "
			}
			e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro",
				verb+html.EscapeString(strings.TrimSpace(err.Error())))
			return pr, false
		}
	} else if pr.Source == domain.SourceGitHub {
		e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro",
			"Only GitHub PRs can be converted to or from draft.")
		return pr, false
	}
	e.Store.UpdatePR(prID, func(p *domain.PR) {
		p.Draft = draft
		touchPRUpdated(p)
	})
	// Don't wait on RefreshPRConversation — that is a multi-gh snapshot and
	// publishPRUpdated already triggers GetPR in the background.
	e.publishPRUpdated(prID)
	if updated, ok := e.Store.PR(prID); ok {
		return updated, true
	}
	return pr, true
}

// mergeLocal merges the loop branch into its base in the source repo on disk
// (PRD §12.1). A remote-tracking base (origin/main) is then pushed. On success
// it marks the PR and workspace merged. A merge or push failure returns
// ok=false with a feed note.
func (e *Engine) mergeLocal(pr domain.PR, method domain.MergeMethod) (domain.PR, bool) {
	proj, ok := e.Store.Project(e.prProjectID(pr))
	if !ok || proj.RepoPath == "" {
		return e.markMerged(pr.ID, ""), true
	}
	// Pick up a mid-session rename before merging the cached branch name.
	e.syncWorkspaceBranch(pr.WorkspaceID)
	if refreshed, rok := e.Store.PR(pr.ID); rok {
		pr = refreshed
	}
	mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	base := pr.Base
	if base == "" {
		base = proj.BaseBranch
	}
	sq, mg := e.mergeMessageStyles(pr.WorkspaceID)
	msg := localMergeCommitMessage(pr, method, sq, mg)
	if _, err := mgr.Merge(ctx, pr.Branch, base, msg, string(method)); err != nil {
		note := "Local merge failed: " + html.EscapeString(err.Error())
		var dirty gitpkg.DirtyBaseError
		if !errors.As(err, &dirty) {
			note += " — resolve the conflict and try again."
		}
		e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro", note)
		return pr, false
	}
	remote, short, isRemote := mgr.ParseRemoteTracking(ctx, base)
	if isRemote {
		if short == "" {
			short = mgr.ShortBranchName(ctx, base)
		}
		if err := mgr.PushBranch(ctx, remote, short); err != nil {
			e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro",
				"Landed on local <code>"+html.EscapeString(short)+"</code> but could not push to <code>"+
					html.EscapeString(remote)+"</code>: "+html.EscapeString(err.Error()))
			return pr, false
		}
	}
	label := domain.MergeMethodLabel(method)
	merged := e.markMerged(pr.ID, "")
	dest := html.EscapeString(base)
	note := fmt.Sprintf("%s: Local PR <code>#%d</code> into <code>%s</code> on disk — you were the only gate.",
		html.EscapeString(label), pr.Number, dest)
	if isRemote {
		note = fmt.Sprintf("%s: Local PR <code>#%d</code> to <code>%s/%s</code>.",
			html.EscapeString(label), pr.Number, html.EscapeString(remote), html.EscapeString(short))
	}
	e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro", note)
	return merged, true
}

// localMergeCommitMessage builds the on-disk commit message for a Local PR merge.
// Squash commits become permanent history, so the subject is the PR title (the
// work description) — not the ephemeral food-name branch. Merge commits keep a
// Merge prefix but still carry the title. Rebase ignores the message. squash/merge
// choose GitHub-style body contents (D65).
func localMergeCommitMessage(pr domain.PR, method domain.MergeMethod, squash domain.SquashCommitMessage, merge domain.MergeCommitMessage) string {
	return formatPRMergeMessage(pr, method, squash, merge).Combined()
}

// mergeCommitText is the subject + body passed to git commit or `gh pr merge`.
// OmitBody skips `--body` so GitHub can fill a squash commit list when Recur
// has none.
type mergeCommitText struct {
	Subject  string
	Body     string
	OmitBody bool
}

func (m mergeCommitText) Combined() string {
	body := strings.TrimSpace(m.Body)
	if body == "" {
		return m.Subject
	}
	return m.Subject + "\n\n" + body
}

func (e *Engine) mergeMessageStyles(workspaceID string) (domain.SquashCommitMessage, domain.MergeCommitMessage) {
	proj, _ := e.Store.Project(loopProjectID(e, workspaceID))
	return domain.EffectiveSquashCommitMessage(proj, e.settings()), domain.EffectiveMergeCommitMessage(proj, e.settings())
}

func formatPRMergeMessage(pr domain.PR, method domain.MergeMethod, squash domain.SquashCommitMessage, merge domain.MergeCommitMessage) mergeCommitText {
	title := singleLine(pr.Title)
	github := pr.Source == domain.SourceGitHub
	if title == "" {
		if github {
			return mergeCommitText{}
		}
		title = "Loop changes"
	}
	switch method {
	case domain.MergeMethodSquash:
		return formatSquashCommitMessage(title, pr.Number, github, pr.Commits, prDescription(pr), squash)
	case domain.MergeMethodRebase:
		return mergeCommitText{}
	default:
		return formatMergeCommitMessage(title, pr.Number, github, prDescription(pr), merge)
	}
}

func formatSquashCommitMessage(title string, number int, github bool, commits []domain.Commit, desc string, style domain.SquashCommitMessage) mergeCommitText {
	style = domain.NormalizeSquashCommitMessage(string(style))
	subject := title
	if github && number > 0 {
		subject = fmt.Sprintf("%s (#%d)", title, number)
	}
	var parts []string
	commitList := ""
	switch style {
	case domain.SquashCommitTitleAndCommits:
		commitList = formatCommitBulletList(commits)
		if commitList != "" {
			parts = append(parts, commitList)
		}
	case domain.SquashCommitTitleAndDescription:
		if d := strings.TrimSpace(desc); d != "" {
			parts = append(parts, d)
		}
	}
	if !github && number > 0 {
		parts = append(parts, fmt.Sprintf("Recur Local PR #%d", number))
	}
	out := mergeCommitText{Subject: subject, Body: strings.Join(parts, "\n\n")}
	if github && style == domain.SquashCommitTitleAndCommits && commitList == "" {
		out.OmitBody = true
	}
	return out
}

func formatMergeCommitMessage(title string, number int, github bool, desc string, style domain.MergeCommitMessage) mergeCommitText {
	style = domain.NormalizeMergeCommitMessage(string(style))
	subject := title
	switch {
	case github && number > 0:
		subject = fmt.Sprintf("%s (#%d)", title, number)
	case !github && number > 0:
		subject = fmt.Sprintf("Merge Local PR #%d: %s", number, title)
	}
	var body string
	if style == domain.MergeCommitTitleAndDescription {
		body = strings.TrimSpace(desc)
	}
	return mergeCommitText{Subject: subject, Body: body}
}

func formatCommitBulletList(commits []domain.Commit) string {
	subjects := commitSubjects(commits)
	if len(subjects) == 0 {
		return ""
	}
	// git log / Recur slices are newest-first; GitHub squash bodies are oldest-first.
	for i, j := 0, len(subjects)-1; i < j; i, j = i+1, j-1 {
		subjects[i], subjects[j] = subjects[j], subjects[i]
	}
	var b strings.Builder
	for i, s := range subjects {
		if i > 0 {
			b.WriteByte('\n')
		}
		b.WriteString("* ")
		b.WriteString(s)
	}
	return b.String()
}

func prDescription(pr domain.PR) string {
	for _, c := range pr.Comments {
		if c.Kind == "description" {
			return strings.TrimSpace(c.BodyMD)
		}
	}
	return ""
}

// singleLine returns the first non-empty line of s, trimmed.
func singleLine(s string) string {
	s = strings.TrimSpace(s)
	if s == "" {
		return ""
	}
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		s = strings.TrimSpace(s[:i])
	}
	return s
}

// commitSubjects returns the first line of each commit message, skipping blanks.
func commitSubjects(commits []domain.Commit) []string {
	out := make([]string, 0, len(commits))
	for _, c := range commits {
		if s := singleLine(c.Message); s != "" {
			out = append(out, s)
		}
	}
	return out
}

// markMerged flips a PR to merged + its loop to merged and emits pr.updated.
// updatedAt is GitHub's last-activity stamp when known; empty means now
// (the user just merged locally).
func (e *Engine) markMerged(prID, updatedAt string) domain.PR {
	merged, ok := e.Store.UpdatePR(prID, func(p *domain.PR) {
		p.State = "Merged"
		p.Group = "merged"
		stampReconcileUpdatedAt(p, updatedAt)
	})
	if !ok {
		return domain.PR{}
	}
	e.setWorkspaceState(merged.WorkspaceID, domain.StateMerged, "merged")
	// The human acted on the PR — clear its "ready for you" inbox row (it is no longer
	// actionable) and emit inbox.updated (PRD §21.5 / D5).
	e.clearPRInbox(prID)
	e.publishPRUpdated(prID)
	e.dropUnmergedSiblingPRs(merged.WorkspaceID, merged.ID)
	e.archivePRChatHosts(prID, e.livePRWorkspaceID(merged.WorkspaceID))
	return merged
}

// loopProjectID resolves the project id for a loop (helper for merge).
func loopProjectID(e *Engine, workspaceID string) string {
	if l, ok := e.Store.Workspace(workspaceID); ok {
		return l.ProjectID
	}
	return ""
}

// prProjectID is the Recur project this PR belongs to. Imported GitHub PRs
// stamp ProjectID directly; Recur-opened PRs inherit it from the workspace.
func (e *Engine) prProjectID(pr domain.PR) string {
	if id := strings.TrimSpace(pr.ProjectID); id != "" {
		return id
	}
	return loopProjectID(e, pr.WorkspaceID)
}

// ---- engine-side emit helpers for PRs ----

// emitFeedPRCreated records a feed line that a PR opened/updated (PRD §7.2).
// elapsed is the loop wall time so the cockpit can show "Completed in …" instead
// of a truncated diff-stat sentence. data-pr lets the UI open the PR in-app.
// The Maestro wrap-up is patched on after the walk settles (attachLoopWrap).
func (e *Engine) emitFeedPRCreated(workspaceID string, pr domain.PR, elapsed time.Duration) {
	kind := "Local PR"
	if pr.Source == domain.SourceGitHub {
		kind = "GitHub PR"
	}
	line := fmt.Sprintf(`<a class="loop-pr-link" data-pr="%s">%s #%d</a>`,
		html.EscapeString(pr.ID), kind, pr.Number)
	if elapsed > 0 {
		line = "Completed in " + formatLoopElapsed(elapsed) + " · " + line
	}
	e.emitWorkspaceEvent(workspaceID, "MA", "Maestro", line)
}

const (
	wrapUpTimeout   = 60 * time.Second
	wrapUpActivityN = 24
	wrapUpBodyMax   = 1200
	wrapUpLineMax   = 160
)

const wrapUpSystemPrompt = "You are the Maestro wrapping up ONE finished loop for the operator. " +
	"You see the whole run: the task, the plan (every step / child), every implementer handoff, " +
	"and recent activity. Do not describe a single Coder turn as if it were the loop. " +
	"Reply with ONLY the wrap-up, plain text, 3–8 short lines: root cause if this was a bug or ticket, " +
	"what landed across the whole run, and anything unfinished or notable. " +
	"No greeting, no markdown fences, no tools."

func (l *Loop) maestroWrapHTML() string {
	if l == nil || l.eng == nil {
		return ""
	}
	ctx, cancel := context.WithTimeout(context.Background(), wrapUpTimeout)
	defer cancel()

	set := l.eng.settings()
	ref := set.MaestroModel()
	flags := set.MaestroFlags()
	if len(flags) == 0 && ref.Provider != domain.ProviderCursor {
		flags = map[string]string{"effort": "low"}
	}
	noTools := ""
	spec := AgentSpec{
		WorkspaceID:    l.ID,
		Model:          ref.String(),
		Flags:          flags,
		SystemPrompt:   wrapUpSystemPrompt,
		Prompt:         l.wrapUpPrompt(),
		Worktree:       l.Worktree,
		PermissionMode: domain.PermissionModeDeny,
		Tools:          &noTools,
		MaxTurns:       1,
		SkipThinking:   true,
	}
	if ref.Provider == domain.ProviderCursor {
		spec.AllowedTools = []string{"Read", "Glob", "Grep"}
	}
	l.eng.updateAgentStatus(l.ID, maestroAgentCode, "summarizing")
	res, err := l.eng.runAgent(ctx, spec, nil)
	l.eng.publishResource()
	l.eng.updateAgentStatus(l.ID, maestroAgentCode, "idle")
	if err != nil || res.IsError {
		return ""
	}
	return loopWrapUpHTML(res.Text)
}

func (l *Loop) wrapUpPrompt() string {
	var b strings.Builder
	b.WriteString("Task:\n")
	b.WriteString(strings.TrimSpace(l.Goal))
	b.WriteByte('\n')
	if ws, ok := l.eng.Store.Workspace(l.ID); ok && ws.Plan != nil && len(ws.Plan.Steps) > 0 {
		b.WriteString("\nPlan:\n")
		b.WriteString(ws.Plan.Board())
		b.WriteByte('\n')
	}
	if h := strings.TrimSpace(l.reviewHandoff()); h != "" {
		b.WriteString("\nHandoffs (every landing, including children):\n")
		b.WriteString(h)
		b.WriteByte('\n')
	}
	if d := l.wrapUpDone(); d != "" {
		b.WriteString("\nImplementer done() notes (every turn, not one Coder):\n")
		b.WriteString(d)
		b.WriteByte('\n')
	}
	if pr, ok := l.eng.Store.PRByWorkspace(l.ID); ok {
		if subs := commitSubjects(pr.Commits); len(subs) > 0 {
			b.WriteString("\nCommits:\n")
			for _, s := range subs {
				b.WriteString("- ")
				b.WriteString(s)
				b.WriteByte('\n')
			}
		}
	}
	if recent := l.eng.recentActivity(l.ID, wrapUpActivityN); len(recent) > 0 {
		b.WriteString("\nRecent activity (newest first):\n")
		for _, r := range recent {
			b.WriteString("- ")
			b.WriteString(truncate(r, wrapUpLineMax))
			b.WriteByte('\n')
		}
	}
	b.WriteString("\nWrite the wrap-up.")
	return b.String()
}

// attachLoopWrap appends wrapHTML to the latest Completed / PR-opened fact.
func (e *Engine) attachLoopWrap(workspaceID, wrapHTML string) {
	wrapHTML = strings.TrimSpace(wrapHTML)
	if workspaceID == "" || wrapHTML == "" {
		return
	}
	for _, it := range e.Store.Feed(workspaceID) {
		if !isCompletionFact(it.Text) || strings.Contains(it.Text, `class="loop-wrap"`) {
			continue
		}
		it.Text += wrapHTML
		e.Store.UpsertFeed(workspaceID, it)
		payload := map[string]any{"workspaceId": workspaceID, "event": it}
		e.persistEvent(workspaceID, "", "workspace.event", payload)
		e.Bus.Publish(Event{Name: "workspace.event", Data: payload})
		return
	}
}

func isCompletionFact(text string) bool {
	if strings.Contains(text, "loop-pr-link") {
		return true
	}
	plain := strings.TrimSpace(stripTags(text))
	return strings.HasPrefix(plain, "Completed")
}

// loopWrapUpHTML is the cockpit body under "Completed". User/model prose is escaped.
func loopWrapUpHTML(prose string) string {
	s := strings.TrimSpace(sanitizeWrapProse(prose))
	if s == "" {
		return ""
	}
	s = truncate(s, wrapUpBodyMax)
	s = html.EscapeString(s)
	s = strings.ReplaceAll(s, "\n", "<br>")
	return `<div class="loop-wrap">` + s + `</div>`
}

func sanitizeWrapProse(raw string) string {
	s := strings.TrimSpace(raw)
	if s == "" {
		return ""
	}
	if strings.HasPrefix(s, "```") {
		s = strings.TrimPrefix(s, "```")
		if i := strings.IndexByte(s, '\n'); i >= 0 {
			s = s[i+1:]
		}
		if j := strings.LastIndex(s, "```"); j >= 0 {
			s = s[:j]
		}
		s = strings.TrimSpace(s)
	}
	return strings.Trim(s, "\"'`")
}

// formatLoopElapsed is the compact wall-clock label for a finished loop.
func formatLoopElapsed(d time.Duration) string {
	if d < 0 {
		d = 0
	}
	sec := int(d.Round(time.Second).Seconds())
	if sec < 60 {
		return fmt.Sprintf("%ds", sec)
	}
	m := sec / 60
	s := sec % 60
	if m >= 60 {
		h := m / 60
		m %= 60
		if m == 0 {
			return fmt.Sprintf("%dh", h)
		}
		return fmt.Sprintf("%dh %dm", h, m)
	}
	if s == 0 {
		return fmt.Sprintf("%dm", m)
	}
	return fmt.Sprintf("%dm %02ds", m, s)
}

// emitPRReadyInbox adds the actionable "ready for you" inbox row for a converged PR
// (PRD §16.11, §21.5 / D5: actionable-only). It is derived from REAL state — a PR
// that actually reached pr_open — not seeded. The row is cleared when the PR
// merges or the human sends the review back (clearPRInbox). It emits inbox.updated
// so the badge/list refresh live (PRD §15.2).
func (e *Engine) emitPRReadyInbox(pr domain.PR) {
	title := fmt.Sprintf("<b>#%d</b> is ready for review", pr.Number)
	sub := html.EscapeString(truncate(prTitle(prGoal(e, pr.WorkspaceID)), 56))
	if prVerified(pr) {
		sub += " · verified in browser · checks ✓"
	} else {
		sub += " · 0 findings"
	}
	e.Store.AddInboxItem(domain.InboxItem{
		ID:          "ib-pr-" + pr.ID,
		Group:       "ready",
		Unread:      true,
		Kind:        "pr",
		Color:       "var(--emerald)",
		Title:       title,
		Sub:         sub,
		Time:        "now",
		Action:      "Review",
		ActionClass: "go",
		Target:      "pr:" + pr.ID,
		WorkspaceID: pr.WorkspaceID,
	})
	e.Bus.Publish(Event{Name: "inbox.updated", Data: map[string]any{"unread": e.Store.UnreadCount()}})

	// Generic "notify" bus event (Phase 3.6): main.go turns this into a native OS
	// notification. Wording mirrors the inbox row title, minus the HTML markup.
	e.Bus.Publish(Event{Name: "notify", Data: map[string]any{
		"title":  fmt.Sprintf("PR #%d is ready for review", pr.Number),
		"body":   truncate(prTitle(prGoal(e, pr.WorkspaceID)), 120),
		"target": "pr:" + pr.ID,
	}})
}

// clearPRInbox removes the actionable inbox row for a PR (e.g. when it merges — the
// human has acted, so it is no longer "what needs me"). Emits inbox.updated.
func (e *Engine) clearPRInbox(prID string) {
	if e.Store.RemoveInboxByTarget("pr:"+prID) > 0 {
		e.Bus.Publish(Event{Name: "inbox.updated", Data: map[string]any{"unread": e.Store.UnreadCount()}})
	}
}

// prVerified reports whether a PR carries passing browser-verify evidence (rail #1
// ran and all steps passed) — used to word the "ready" inbox row honestly.
func prVerified(pr domain.PR) bool {
	if len(pr.Verify) == 0 {
		return false
	}
	for _, v := range pr.Verify {
		if !v.Passed {
			return false
		}
	}
	return true
}

// prGoal resolves the loop goal for a PR's loop (for inbox row text), or "" on miss.
func prGoal(e *Engine, workspaceID string) string {
	if l, ok := e.Store.Workspace(workspaceID); ok {
		return l.GoalLabel()
	}
	return ""
}

// RefreshPRCommits reloads commit metadata (subject, body, author, date, SHA)
// from the worktree. Per-commit file diffs and GitHub rollup fields are kept
// when the hash still matches. Fetch errors leave the persisted list alone.
func (e *Engine) RefreshPRCommits(prID string) {
	pr, ok := e.Store.PR(prID)
	if !ok || pr.UpdatingBase != "" {
		return
	}
	loop, ok := e.Store.Workspace(e.livePRWorkspaceID(pr.WorkspaceID))
	if !ok || loop.WorktreePath == "" {
		return
	}
	proj, ok := e.Store.Project(loop.ProjectID)
	if !ok || proj.RepoPath == "" {
		return
	}
	base := pr.Base
	if base == "" {
		base = loop.BaseBranch
	}
	if base == "" {
		base = proj.BaseBranch
	}
	if base == "" {
		return
	}
	mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	fresh, err := mgr.LogCommits(ctx, loop.WorktreePath, base)
	if err != nil || len(fresh) == 0 {
		return
	}
	merged := mergeCommitLog(pr.Commits, fresh)
	if commitsEqual(pr.Commits, merged) {
		return
	}
	e.Store.UpdatePR(prID, func(p *domain.PR) { p.Commits = merged })
}

// EnsureCommitDiff fills one commit's file diff (`git show --first-parent`) when
// the stored row has none. LogCommits leaves Files nil on purpose so the list
// stays cheap; the Commits tab drills in through GetCommitDiff. A non-nil empty
// Files slice is a cache hit (empty / already-shown merge). Missing worktrees
// fall back to the project repo so archived PRs still open.
func (e *Engine) EnsureCommitDiff(prID, hash string) (domain.Commit, error) {
	pr, ok := e.Store.PR(prID)
	if !ok {
		return domain.Commit{}, fmt.Errorf("PR %q not found", prID)
	}
	_, c, ok := findPRCommit(pr.Commits, hash)
	if !ok {
		return domain.Commit{}, fmt.Errorf("commit %q not found on PR %q", hash, prID)
	}
	if len(c.Files) > 0 {
		if gitpkg.HighlightFiles(c.Files) {
			if updated, ok := e.Store.UpdatePR(prID, func(p *domain.PR) {
				if i, _, found := findPRCommit(p.Commits, hash); found {
					p.Commits[i].Files = c.Files
				}
			}); ok {
				if _, filled, found := findPRCommit(updated.Commits, hash); found {
					return filled, nil
				}
			}
		}
		return c, nil
	}
	if c.Files != nil {
		// Empty slice means we already ran git show (merge / empty commit).
		return c, nil
	}
	loop, ok := e.Store.Workspace(e.livePRWorkspaceID(pr.WorkspaceID))
	projID := e.prProjectID(pr)
	proj, pok := e.Store.Project(projID)
	if !pok || strings.TrimSpace(proj.RepoPath) == "" {
		return domain.Commit{}, fmt.Errorf("commit %q: no repo for PR %q", hash, prID)
	}
	ref := strings.TrimSpace(c.SHA)
	if ref == "" {
		ref = strings.TrimSpace(c.Hash)
	}
	if ref == "" {
		ref = strings.TrimSpace(hash)
	}
	repo := strings.TrimSpace(proj.RepoPath)
	dir := ""
	if ok {
		dir = strings.TrimSpace(loop.WorktreePath)
	}
	if dir == "" {
		dir = repo
	}
	mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	if pr.Source == domain.SourceGitHub && pr.Number > 0 && dir == repo && !mgr.CommitExists(ctx, ref) {
		fetchCtx, fetchCancel := context.WithTimeout(ctx, 4*time.Second)
		_, _ = mgr.FetchPRHead(fetchCtx, pr.Number)
		fetchCancel()
	}
	files, err := mgr.CommitDiffFirstParent(ctx, dir, ref)
	if err != nil && dir != repo {
		files, err = mgr.CommitDiffFirstParent(ctx, repo, ref)
	}
	if err != nil {
		return domain.Commit{}, fmt.Errorf("commit diff %s: %w", ref, err)
	}
	if files == nil {
		files = []domain.FileDiff{}
	}
	gitpkg.HighlightFiles(files)
	updated, ok := e.Store.UpdatePR(prID, func(p *domain.PR) {
		if i, _, found := findPRCommit(p.Commits, hash); found {
			p.Commits[i].Files = files
		}
	})
	if !ok {
		c.Files = files
		return c, nil
	}
	if _, filled, found := findPRCommit(updated.Commits, hash); found {
		return filled, nil
	}
	c.Files = files
	return c, nil
}

func findPRCommit(commits []domain.Commit, ref string) (int, domain.Commit, bool) {
	for i, c := range commits {
		if commitRefMatches(c, ref) {
			return i, c, true
		}
	}
	return -1, domain.Commit{}, false
}

func commitRefMatches(c domain.Commit, ref string) bool {
	ref = strings.TrimSpace(strings.ToLower(ref))
	if ref == "" {
		return false
	}
	h := strings.ToLower(strings.TrimSpace(c.Hash))
	s := strings.ToLower(strings.TrimSpace(c.SHA))
	if h == ref || s == ref {
		return true
	}
	if len(ref) < 7 {
		return false
	}
	return (h != "" && (strings.HasPrefix(h, ref) || strings.HasPrefix(ref, h))) ||
		(s != "" && (strings.HasPrefix(s, ref) || strings.HasPrefix(ref, s)))
}

// mergeCommitLog overlays a fresh `git log` onto the stored commit list. Files
// and GitHub-only fields stay on the matching stored row so we do not re-run
// `git show` or drop verified/checks.
func mergeCommitLog(existing, fresh []domain.Commit) []domain.Commit {
	byShort := make(map[string]domain.Commit, len(existing))
	byFull := make(map[string]domain.Commit, len(existing))
	for _, c := range existing {
		if h := strings.TrimSpace(c.Hash); h != "" {
			byShort[h] = c
		}
		if s := strings.TrimSpace(c.SHA); s != "" {
			byFull[s] = c
		}
	}
	out := make([]domain.Commit, 0, len(fresh))
	for _, n := range fresh {
		old, ok := byFull[strings.TrimSpace(n.SHA)]
		if !ok {
			old, ok = byShort[strings.TrimSpace(n.Hash)]
		}
		if !ok && len(n.Hash) >= 7 {
			for h, c := range byShort {
				if strings.HasPrefix(c.SHA, n.Hash) || strings.HasPrefix(n.SHA, h) || strings.HasPrefix(h, n.Hash) {
					old, ok = c, true
					break
				}
			}
		}
		if ok {
			n.Files = old.Files
			n.AvatarURL = old.AvatarURL
			n.Verified = old.Verified
			n.CheckStatus = old.CheckStatus
		}
		out = append(out, n)
	}
	return out
}

// RefreshPRBehind recomputes how many commits the merge-base tip is ahead of
// the PR branch (D51) and persists BehindBase. Best-effort: missing worktree /
// base / merged PRs leave BehindBase at 0. Fetches the remote-tracking
// counterpart of the stored base (main → origin/main) so a stale local main
// does not hide that GitHub's merge base has moved. Called from GetPR.
func (e *Engine) RefreshPRBehind(prID string) {
	pr, ok := e.Store.PR(prID)
	if !ok || pr.State == "Merged" || pr.Group == "merged" || pr.UpdatingBase != "" {
		return
	}
	loop, ok := e.Store.Workspace(e.livePRWorkspaceID(pr.WorkspaceID))
	if !ok || loop.WorktreePath == "" {
		return
	}
	proj, ok := e.Store.Project(loop.ProjectID)
	if !ok || proj.RepoPath == "" {
		return
	}
	base := pr.Base
	if base == "" {
		base = loop.BaseBranch
	}
	if base == "" {
		base = proj.BaseBranch
	}
	if base == "" {
		return
	}
	mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	_, behind, err := mgr.BehindBase(ctx, loop.WorktreePath, base, true)
	if err != nil {
		return
	}
	if pr.BehindBase == behind {
		return
	}
	e.Store.UpdatePR(prID, func(p *domain.PR) { p.BehindBase = behind })
}

// RefreshPRChecks replaces a GitHub PR's checks with the live GitHub Checks
// tab rollup (check runs, expected statuses, queued suites). Local PRs, merged
// PRs, and fetch errors leave the persisted checks alone (same best-effort
// contract as RefreshPRBehind).
func (e *Engine) RefreshPRChecks(prID string) {
	pr, ok := e.Store.PR(prID)
	if !ok || pr.Source != domain.SourceGitHub || pr.State == "Merged" || pr.Group == "merged" {
		return
	}
	if pr.Number <= 0 {
		return
	}
	repoPath := e.prRepoPath(pr)
	if repoPath == "" {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	checks, checksOK, err := getGitHubPRChecks(ctx, repoPath, pr.Number, pr.URL)
	if err != nil {
		return
	}
	if pr.ChecksOK == checksOK && checksEqual(pr.Checks, checks) && pr.HasChecks == (len(checks) > 0) {
		return
	}
	e.Store.UpdatePR(prID, func(p *domain.PR) {
		p.Checks = checks
		p.ChecksOK = checksOK
		p.HasChecks = len(checks) > 0
	})
}

// prRepoPath resolves the project repo for a PR (gh cwd / git root).
func (e *Engine) prRepoPath(pr domain.PR) string {
	if id := e.prProjectID(pr); id != "" {
		if proj, ok := e.Store.Project(id); ok {
			return proj.RepoPath
		}
	}
	return ""
}

// publishPRUpdated emits pr.updated so the PR list/detail update live (PRD §15.2).
// It carries the fields the frontend merges into its store — including group so
// merge/rework move the PR between list buckets without a full refetch. The
// frontend upserts on an unknown prId (GetPR) so engine-created PRs appear in-session.
func (e *Engine) publishPRUpdated(prID string) {
	e.emitPRUpdated(prID, false)
}

// publishPRPendingUpdated is pr.updated for a local pending-comment change.
// The frontend merges pendingComments and skips GetPR (gh checks / conversation).
func (e *Engine) publishPRPendingUpdated(prID string) {
	e.emitPRUpdated(prID, true)
}

// publishPRThreadUpdated is pr.updated after a local thread write (reply / edit /
// delete). The RPC already returned the new comment or PR; skip GetPR.
func (e *Engine) publishPRThreadUpdated(prID string) {
	e.emitPRUpdated(prID, true)
}

func (e *Engine) emitPRUpdated(prID string, pendingOnly bool) {
	pr, ok := e.Store.PR(prID)
	if !ok {
		return
	}
	payload := map[string]any{
		"prId": prID, "state": pr.State,
		"group": pr.Group, "checksOk": pr.ChecksOK, "hasChecks": pr.HasChecks, "fileCount": pr.FileCount,
		"title": pr.Title, "number": pr.Number, "workspaceId": pr.WorkspaceID, "projectId": pr.ProjectID,
		"source": pr.Source, "branch": pr.Branch, "base": pr.Base,
		"when": pr.When, "diff": pr.Diff, "behindBase": pr.BehindBase,
		"updatingBase":        pr.UpdatingBase,
		"pendingComments":     pr.PendingComments,
		"draft":               pr.Draft,
		"mergeable":           pr.Mergeable,
		"mergeStateStatus":    pr.MergeStateStatus,
		"reviewDecision":      pr.ReviewDecision,
		"reviewRequested":     pr.ReviewRequested,
		"attentionReasons":    pr.AttentionReasons,
		"participants":        pr.Participants,
		"reviewers":           pr.Reviewers,
		"allowedMergeMethods": pr.AllowedMergeMethods,
	}
	if pendingOnly {
		payload["pendingOnly"] = true
	}
	e.Bus.Publish(Event{Name: "pr.updated", Data: payload})
}

// publishPRComment emits pr.comment for a new thread entry (PRD §15.2). The comment
// is already persisted on the PR; this drives the live thread update.
func (e *Engine) publishPRComment(prID string, c domain.PRComment) {
	payload := map[string]any{"prId": prID, "comment": c}
	e.Bus.Publish(Event{Name: "pr.comment", Data: payload})
}

// nowComment builds a PRComment with a stable id + the standard avatar CSS used by
// the seeded thread, so engine/agent-authored rework comments match. code is the
// 2-letter avatar.
func nowComment(code, author, bodyHTML string) domain.PRComment {
	return domain.PRComment{
		ID:        fmt.Sprintf("c-%d", time.Now().UnixNano()),
		Code:      code,
		Author:    author,
		AvatarCSS: commentAvatarCSS(code),
		Time:      "now",
		Verb:      "commented",
		BodyHTML:  bodyHTML,
	}
}

// commentAvatarCSS is the fallback avatar fill when a comment has no GitHub photo.
func commentAvatarCSS(code string) string {
	switch code {
	case "CO":
		return "background:var(--cyan);color:#0A0C10"
	case "PR":
		return "background:#b08bd8;color:#0A0C10"
	case "MA":
		return "background:var(--amber);color:#0A0C10"
	case "RV":
		return "background:var(--indigo);color:#0A0C10"
	default:
		return "background:linear-gradient(135deg,#39434f,#1d242c);color:var(--muted)"
	}
}
