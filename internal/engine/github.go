package engine

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"html"
	"log"
	"net/url"
	"os/exec"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

// ghAvailTTL reuses a recent gh-auth probe so GetPR / snapshots do not spawn
// `gh auth status` on every 15s poll.
const ghAvailTTL = 30 * time.Second

var (
	ghAvailMu sync.Mutex
	ghAvailAt time.Time
	ghAvailOK bool
)

// ghCLIAvailable reports whether the `gh` CLI is installed AND authenticated, so the
// GitHub PR path (PRD §12, M6 brief F) can run. It is deliberately conservative: a
// missing binary or a failing `gh auth status` both return false, so the engine
// DEGRADES GRACEFULLY to the Local path / a clear message rather than erroring. The
// check is bounded so a hung gh never wedges a merge.
func ghCLIAvailable() bool {
	ghAvailMu.Lock()
	if !ghAvailAt.IsZero() && time.Since(ghAvailAt) < ghAvailTTL {
		ok := ghAvailOK
		ghAvailMu.Unlock()
		return ok
	}
	ghAvailMu.Unlock()
	ok := probeGHCLIFn()
	ghAvailMu.Lock()
	ghAvailOK = ok
	ghAvailAt = time.Now()
	ghAvailMu.Unlock()
	return ok
}

// probeGHCLIFn is the uncached gh probe. Tests replace this.
var probeGHCLIFn = probeGHCLI

func probeGHCLI() bool {
	if _, err := exec.LookPath("gh"); err != nil {
		return false
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "gh", "auth", "status")
	return cmd.Run() == nil
}

func resetGHAvailCache() {
	ghAvailMu.Lock()
	ghAvailAt = time.Time{}
	ghAvailOK = false
	ghAvailMu.Unlock()
}

// ghCLIAvailableFn is the sweep/create gate. Tests replace this so SyncGitHubPRs
// does not depend on a developer machine's `gh auth status`.
var ghCLIAvailableFn = ghCLIAvailable

// GitHubLogin returns the authenticated `gh` account login, or "" when gh is
// missing, logged out, or the probe times out. Settings uses this once to fill
// the branch-prefix field; workspace create/rename never call it.
func GitHubLogin() string {
	if _, err := exec.LookPath("gh"); err != nil {
		return ""
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	out, err := exec.CommandContext(ctx, "gh", "api", "user", "-q", ".login").Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

// CreateGitHubPR pushes the loop/workspace HEAD and opens a GitHub PR via
// `gh pr create` (PRD §12, M6 brief F). draft adds --draft. It is REAL but GATED: if gh is
// absent/unauthenticated it returns ok=false with a clear reason so the caller
// can fall back to a Local PR or surface the message. It never blocks the loop
// (bounded context). The returned number is the GitHub PR number.
//
// Push runs from worktree (when set) as `git push -u origin HEAD:refs/heads/<branch>`
// so a renamed local branch still pushes under the intended remote head name.
// repoPath is used as the gh cwd (remote discovery).
//
// Create/merge go through gh. The PR view mirrors GitHub's thread, reviews,
// and checks (see github_thread.go). The default experience and default tests
// still use the Local path.
func (e *Engine) CreateGitHubPR(ctx context.Context, repoPath, worktree, branch, base, title, body string, draft bool) (number int, ok bool, reason string) {
	if !ghCLIAvailable() {
		return 0, false, "gh CLI is not installed or not authenticated — using a Local PR instead"
	}
	if strings.TrimSpace(branch) == "" {
		return 0, false, "no branch to push — using a Local PR instead"
	}

	// gh --base wants the short branch name (main), not origin/main.
	base = githubPRBase(ctx, repoPath, base)

	title = hardTruncateRunes(singleLine(title), githubPRTitleMax)
	if title == "" {
		title = "Workspace changes"
	}

	// Push HEAD to the named remote branch via GIT (`gh` has no `push` subcommand —
	// this previously ran `gh push`, which always failed and was swallowed). Prefer
	// the worktree so the push matches whatever is checked out there (renames/safe
	// against a stale cached branch name). Best effort: fall through to `gh pr create`
	// in case the branch is already on the remote; if that ALSO fails, the push error
	// is the more useful diagnostic, so it is carried into the failure reason rather
	// than silently dropped (never lose it, PRD honesty bar).
	pushDir := worktree
	if pushDir == "" {
		pushDir = repoPath
	}
	var pushErr error
	if _, err := runGit(ctx, pushDir, "push", "-u", "origin", "HEAD:refs/heads/"+branch); err != nil {
		pushErr = err
	}

	out, err := runGH(ctx, repoPath, ghCreatePRArgs(branch, base, title, body, draft)...)
	// Match gh's own output, not err.Error(): runGH wraps argv (--title/--body) into the error.
	if err != nil && strings.Contains(strings.ToLower(out), "title is too long") {
		title = hardTruncateRunes(title, 80)
		out, err = runGH(ctx, repoPath, ghCreatePRArgs(branch, base, title, body, draft)...)
	}
	n := githubCreateNumber(out, 0)
	if n <= 0 && pushErr == nil {
		n = githubPRNumberForHead(ctx, repoPath, branch, base)
	}
	if n > 0 && pushErr == nil {
		return n, true, ""
	}
	if pushErr != nil || err != nil {
		return 0, false, ghFailReason(pushErr, err)
	}
	return 0, false, "gh pr create did not print a PR URL"
}

// githubCreateNumber is the GitHub PR number from `gh pr create` stdout, or
// existingHead when create did not print a URL (already-open PR for this head).
func githubCreateNumber(createOut string, existingHead int) int {
	n := prNumberFromURL(strings.TrimSpace(createOut))
	if n > 0 {
		return n
	}
	if existingHead > 0 {
		return existingHead
	}
	return 0
}

// githubPRNumberForHead is the open PR for head/base, via `gh pr list`.
// `gh pr view` takes the branch as a positional arg — it has no `--head` flag.
func githubPRNumberForHead(ctx context.Context, repoPath, branch, base string) int {
	if strings.TrimSpace(branch) == "" {
		return 0
	}
	out, err := runGH(ctx, repoPath, githubPRHeadLookupArgs(branch, base)...)
	if err != nil {
		return 0
	}
	return githubListFirstNumber(out)
}

func githubPRHeadLookupArgs(branch, base string) []string {
	args := []string{"pr", "list", "--head", branch, "--state", "open", "--limit", "1", "--json", "number"}
	if b := strings.TrimSpace(base); b != "" {
		args = append(args, "--base", b)
	}
	return args
}

func githubListFirstNumber(raw string) int {
	raw = strings.TrimSpace(raw)
	if i := strings.IndexByte(raw, '['); i >= 0 {
		raw = raw[i:]
	}
	var rows []struct {
		Number int `json:"number"`
	}
	if json.Unmarshal([]byte(raw), &rows) != nil || len(rows) == 0 || rows[0].Number <= 0 {
		return 0
	}
	return rows[0].Number
}

// ghFailReason composes the fallback reason shown when a GitHub PR can't be opened,
// preserving a swallowed push error (fixed here — it used to be dropped even though
// it is often the real cause of the subsequent `gh pr create` failure).
func ghFailReason(pushErr, createErr error) string {
	var b strings.Builder
	if pushErr != nil {
		b.WriteString("git push failed: ")
		b.WriteString(cmdErrOutput(pushErr))
		if createErr != nil {
			b.WriteString("; ")
		}
	}
	if createErr != nil {
		b.WriteString("gh pr create failed: ")
		b.WriteString(cmdErrOutput(createErr))
	}
	return b.String()
}

func cmdErrOutput(err error) string {
	if err == nil {
		return ""
	}
	return stripCmdWrap(err.Error())
}

// ghFailDiag is the display form of a GitHub create failure: command output
// only, so a long `gh pr create --body` is not what the 240-rune cap keeps.
func ghFailDiag(s string) string {
	s = strings.TrimSpace(s)
	const createPfx = "gh pr create failed: "
	const pushPfx = "git push failed: "
	if i := strings.Index(s, "; "+createPfx); i >= 0 {
		push := strings.TrimPrefix(s[:i], pushPfx)
		create := s[i+len("; "+createPfx):]
		return pushPfx + stripCmdWrap(push) + "; " + createPfx + stripCmdWrap(create)
	}
	if strings.HasPrefix(s, createPfx) {
		return createPfx + stripCmdWrap(s[len(createPfx):])
	}
	if strings.HasPrefix(s, pushPfx) {
		return pushPfx + stripCmdWrap(s[len(pushPfx):])
	}
	return stripCmdWrap(s)
}

// stripCmdWrap is the CombinedOutput tail of a runGH/runGit error
// (`gh <argv>: <out>` / `git <argv>: <out>`). Last ": " is the wrap — argv
// can contain "HEAD:refs/heads/…" but that has no space after the colon.
func stripCmdWrap(s string) string {
	s = strings.TrimSpace(s)
	if strings.HasPrefix(s, "gh ") || strings.HasPrefix(s, "git ") {
		if i := strings.LastIndex(s, ": "); i >= 0 {
			if out := strings.TrimSpace(s[i+2:]); out != "" {
				return out
			}
		}
	}
	return s
}

// pushGitHubHead pushes the worktree's current HEAD to the remote head ref that an
// already-open GitHub PR tracks (PR.Branch, frozen at create). Local renames therefore
// do not orphan the open PR — commits keep landing on the same remote head.
// force uses --force-with-lease (rebase onto main, then update the PR).
// A fork PR (IsCrossRepository) pushes to the head repo, never origin, leases
// against refs/recur/pr/N, and advances that ref so ahead/behind sees the push.
func pushGitHubHead(ctx context.Context, worktree, repoPath string, pr domain.PR, remoteHead string, force bool) error {
	if strings.TrimSpace(remoteHead) == "" {
		return fmt.Errorf("empty remote head")
	}
	pushDir := worktree
	if pushDir == "" {
		pushDir = repoPath
	}
	remote, lease := "origin", "--force-with-lease"
	prRef := gitpkg.PRHeadRef(pr.Number)
	if pr.IsCrossRepository {
		url, err := githubPRHeadRepoURL(ctx, repoPath, pr.Number)
		if err != nil {
			return fmt.Errorf("could not resolve the fork for PR #%d: %w", pr.Number, err)
		}
		remote = url
		if force {
			sha, err := runGit(ctx, pushDir, "rev-parse", "--verify", prRef)
			if err != nil {
				return fmt.Errorf("no fetched head for PR #%d to lease against: %w", pr.Number, err)
			}
			lease += "=refs/heads/" + remoteHead + ":" + strings.TrimSpace(sha)
		}
	}
	args := []string{"push"}
	if force {
		args = append(args, lease)
	}
	args = append(args, remote, "HEAD:refs/heads/"+remoteHead)
	if _, err := runGit(ctx, pushDir, args...); err != nil {
		return err
	}
	if pr.IsCrossRepository {
		_, _ = runGit(ctx, pushDir, "update-ref", prRef, "HEAD")
	}
	return nil
}

// githubPRHeadRepoURL is the push URL of a fork PR's head repo, in origin's
// URL scheme (ssh or https).
func githubPRHeadRepoURL(ctx context.Context, repoPath string, number int) (string, error) {
	out, err := runGH(ctx, repoPath, "pr", "view", strconv.Itoa(number), "--json", "headRepositoryOwner,headRepository")
	if err != nil {
		return "", err
	}
	var head struct {
		Owner struct {
			Login string `json:"login"`
		} `json:"headRepositoryOwner"`
		Repo struct {
			Name string `json:"name"`
		} `json:"headRepository"`
	}
	if err := json.Unmarshal([]byte(strings.TrimSpace(out)), &head); err != nil || head.Owner.Login == "" || head.Repo.Name == "" {
		return "", fmt.Errorf("gh pr view %d returned no head repository", number)
	}
	origin, err := runGit(ctx, repoPath, "remote", "get-url", "origin")
	if err != nil {
		return "", err
	}
	url, ok := forkRemoteURL(origin, head.Owner.Login, head.Repo.Name)
	if !ok {
		return "", fmt.Errorf("cannot derive a fork URL from origin %q", strings.TrimSpace(origin))
	}
	return url, nil
}

// forkRemoteURL replaces the trailing owner/repo of a remote URL
// (git@host:owner/repo.git, https://host/owner/repo) with owner/repo.git.
func forkRemoteURL(remoteURL, owner, repo string) (string, bool) {
	s := strings.TrimSuffix(strings.TrimSuffix(strings.TrimSpace(remoteURL), "/"), ".git")
	i := strings.LastIndexByte(s, '/')
	if i <= 0 {
		return "", false
	}
	j := strings.LastIndexAny(s[:i], "/:")
	if j < 0 || j == i-1 {
		return "", false
	}
	return s[:j+1] + owner + "/" + repo + ".git", true
}

// githubPRBase is the branch name `gh pr create/edit --base` accepts. base is a
// stored local compare ref (origin/main or origin/origin/release), not a GitHub
// baseRefName — ShortBranchName strips the configured remote prefix.
func githubPRBase(ctx context.Context, repoPath, base string) string {
	base = strings.TrimSpace(base)
	if base == "" || repoPath == "" {
		return base
	}
	return gitpkg.NewManager(repoPath, "").ShortBranchName(ctx, base)
}

// ghMergeArgs builds the `gh pr merge` argv. Merge is by PR number (not branch name)
// so a local rename after open cannot break merge.
// ghCreatePRArgs is the `gh pr create` argv. draft adds --draft.
func ghCreatePRArgs(branch, base, title, body string, draft bool) []string {
	args := []string{"pr", "create", "--head", branch, "--base", base, "--title", title, "--body", body}
	if draft {
		args = append(args, "--draft")
	}
	return args
}

func ghMergeArgs(prNumber int, methodFlag string, msg mergeCommitText) []string {
	args := []string{"pr", "merge", fmt.Sprintf("%d", prNumber), methodFlag}
	if methodFlag == "--rebase" || msg.Subject == "" {
		return args
	}
	args = append(args, "--subject", msg.Subject)
	if !msg.OmitBody {
		args = append(args, "--body", msg.Body)
	}
	return args
}

func ghMergeRESTArgs(ownerRepo string, number int, method string) []string {
	return []string{
		"api", "-X", "PUT",
		fmt.Sprintf("repos/%s/pulls/%d/merge", ownerRepo, number),
		"-f", "merge_method=" + method,
	}
}

func githubMergeCLIFlag(method domain.MergeMethod) string {
	switch domain.NormalizeMergeMethod(string(method)) {
	case domain.MergeMethodSquash:
		return "--squash"
	case domain.MergeMethodRebase:
		return "--rebase"
	default:
		return "--merge"
	}
}

func githubGraphQLMergeMethod(method domain.MergeMethod) string {
	switch domain.NormalizeMergeMethod(string(method)) {
	case domain.MergeMethodSquash:
		return "SQUASH"
	case domain.MergeMethodRebase:
		return "REBASE"
	default:
		return "MERGE"
	}
}

type githubMergeKind string

const (
	githubMergeREST    githubMergeKind = "rest"
	githubMergeGraphQL githubMergeKind = "graphql"
	githubMergeCLI     githubMergeKind = "cli"
)

type githubMergePlan struct {
	kind     githubMergeKind
	restArgs []string
	nodeID   string
	cliArgs  []string
}

// planGitHubMerge prefers a single merge API call over `gh pr merge`. The CLI
// command re-fetches the PR and often waits while GitHub computes mergeable,
// which is the delay after Confirm in the app.
func planGitHubMerge(pr domain.PR, method domain.MergeMethod) githubMergePlan {
	restMethod := string(domain.NormalizeMergeMethod(string(method)))
	if ownerRepo, ok := ownerRepoFromPRURL(pr.URL); ok && pr.Number > 0 {
		return githubMergePlan{kind: githubMergeREST, restArgs: ghMergeRESTArgs(ownerRepo, pr.Number, restMethod)}
	}
	if id := strings.TrimSpace(pr.NodeID); id != "" {
		return githubMergePlan{kind: githubMergeGraphQL, nodeID: id}
	}
	return githubMergePlan{kind: githubMergeCLI, cliArgs: ghMergeArgs(pr.Number, githubMergeCLIFlag(method), mergeCommitText{})}
}

// mergeGitHubPR executes a GitHub merge. queued is true when the base branch
// requires a merge queue and the PR was enqueued instead of merged. Tests
// replace this.
var mergeGitHubPR = doMergeGitHubPR

func doMergeGitHubPR(ctx context.Context, repoPath string, pr domain.PR, method domain.MergeMethod) (queued bool, err error) {
	if _, err := exec.LookPath("gh"); err != nil {
		return false, fmt.Errorf("the gh CLI is not installed")
	}
	if pr.MergeQueueEnabled {
		return enqueueGitHubPR(ctx, repoPath, pr)
	}
	plan := planGitHubMerge(pr, method)
	switch plan.kind {
	case githubMergeREST:
		err = retryGitHubRESTMerge(ctx, func() error {
			_, err := runGH(ctx, repoPath, plan.restArgs...)
			return err
		})
	case githubMergeGraphQL:
		err = mergeGitHubGraphQL(ctx, repoPath, plan.nodeID, method)
	default:
		_, err = runGH(ctx, repoPath, plan.cliArgs...)
	}
	// The queue was enabled after the last snapshot; the merge APIs refuse it.
	if err != nil && strings.Contains(strings.ToLower(err.Error()), "merge queue") {
		return enqueueGitHubPR(ctx, repoPath, pr)
	}
	return false, err
}

// enqueueGitHubPR adds the PR to its base branch's merge queue, or enables
// auto-merge until required checks pass. The queue owns the merge method.
func enqueueGitHubPR(ctx context.Context, repoPath string, pr domain.PR) (bool, error) {
	_, err := runGH(ctx, repoPath, ghEnqueueArgs(pr)...)
	return err == nil, err
}

func ghEnqueueArgs(pr domain.PR) []string {
	target := pr.URL
	if target == "" {
		target = fmt.Sprintf("%d", pr.Number)
	}
	return []string{"pr", "merge", target}
}

// githubMergeRetryDelay is the pause between REST merge attempts while GitHub
// is still computing mergeability (HTTP 405). Tests set it to 0.
var githubMergeRetryDelay = time.Second

func retryGitHubRESTMerge(ctx context.Context, attempt func() error) error {
	var err error
	for i := 0; i < 3; i++ {
		if i > 0 && githubMergeRetryDelay > 0 {
			timer := time.NewTimer(githubMergeRetryDelay)
			select {
			case <-ctx.Done():
				timer.Stop()
				if err != nil {
					return err
				}
				return ctx.Err()
			case <-timer.C:
			}
		}
		err = attempt()
		if err == nil || !isGitHubMergeabilityPending(err) {
			return err
		}
	}
	return err
}

func isGitHubMergeabilityPending(err error) bool {
	if err == nil {
		return false
	}
	lower := strings.ToLower(err.Error())
	return strings.Contains(lower, "not mergeable") || strings.Contains(lower, "base branch was modified")
}

func mergeGitHubGraphQL(ctx context.Context, repoPath, nodeID string, method domain.MergeMethod) error {
	payload := struct {
		Query     string         `json:"query"`
		Variables map[string]any `json:"variables"`
	}{
		Query: `mutation($id:ID!,$method:PullRequestMergeMethod!){mergePullRequest(input:{pullRequestId:$id,mergeMethod:$method}){pullRequest{merged}}}`,
		Variables: map[string]any{
			"id":     nodeID,
			"method": githubGraphQLMergeMethod(method),
		},
	}
	raw, err := json.Marshal(payload)
	if err != nil {
		return err
	}
	out, err := runGHWithStdin(ctx, repoPath, string(raw), "api", "graphql", "--input", "-")
	if err != nil {
		return fmt.Errorf("gh api graphql mergePullRequest: %w", err)
	}
	if strings.Contains(out, `"errors"`) {
		return fmt.Errorf("gh api graphql mergePullRequest: %s", strings.TrimSpace(out))
	}
	return nil
}

// mergeGitHub merges a GitHub PR (PRD §12, M6 brief F, D53). It uses the merge
// REST/GraphQL API when the PR URL or node id is already known so Confirm is
// not blocked on `gh auth status` + `gh pr merge` mergeable polling. The CLI
// is still required (`gh api`); a missing binary fails fast without an auth
// probe. method maps to merge/squash/rebase. On success it marks the PR merged
// locally so the UI reflects the merge. A merge-queue branch enqueues the PR
// and leaves it open; the PR sweep marks it merged once the queue lands it.
func (e *Engine) mergeGitHub(pr domain.PR, method domain.MergeMethod) (domain.PR, bool) {
	proj, ok := e.Store.Project(e.prProjectID(pr))
	repoPath := ""
	if ok {
		repoPath = proj.RepoPath
	}
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	if !domain.MergeMethodPermitted(pr.AllowedMergeMethods, method) {
		e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro",
			"GitHub doesn't allow "+html.EscapeString(domain.MergeMethodLabel(method))+" on this repository.")
		return pr, false
	}

	queued, err := mergeGitHubPR(ctx, repoPath, pr, method)
	if err != nil {
		e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro", githubMergeFailureNote(err))
		return pr, false
	}
	if queued {
		if updated, ok := e.Store.UpdatePR(pr.ID, func(p *domain.PR) {
			p.MergeQueueEnabled = true
			p.MergeQueued = true
		}); ok {
			pr = updated
		}
		e.publishPRUpdated(pr.ID)
		e.ScheduleRefreshPRConversation(pr.ID)
		e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro", fmt.Sprintf(
			"Added GitHub PR <code>#%d</code> to the merge queue.", pr.Number))
		return pr, true
	}
	label := domain.MergeMethodLabel(method)
	merged := e.markMerged(pr.ID, "")
	e.prConvLast.Delete(pr.ID)
	e.ScheduleRefreshPRConversation(pr.ID)
	e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro", fmt.Sprintf(
		"%s: GitHub PR <code>#%d</code> via gh.", html.EscapeString(label), pr.Number))
	return merged, true
}

func githubMergeFailureNote(err error) string {
	msg := strings.TrimSpace(err.Error())
	switch {
	case isGitHubAuthFailure(msg):
		return "Can't merge on GitHub: the gh CLI is not authenticated (" + html.EscapeString(msg) + ")."
	case strings.Contains(strings.ToLower(msg), "the gh cli is not installed"):
		return "Can't merge on GitHub: the gh CLI is not installed."
	default:
		return "GitHub merge failed: " + html.EscapeString(msg)
	}
}

func isGitHubAuthFailure(msg string) bool {
	lower := strings.ToLower(msg)
	if strings.Contains(lower, "bad credentials") {
		return true
	}
	if strings.Contains(lower, "requires authentication") || strings.Contains(lower, "must authenticate") {
		return true
	}
	if strings.Contains(lower, "gh auth login") || strings.Contains(lower, "gh_token") {
		return true
	}
	return strings.Contains(lower, "http 401")
}

// fetchGitHubMergeFieldsFn loads live title/body/commits for a GitHub merge
// message. Tests replace this. GitHub has no "suggested squash message" API;
// `gh pr view` is the same data the UI uses to prefill.
var fetchGitHubMergeFieldsFn = fetchGitHubMergeFields

type githubMergeFields struct {
	Title   string
	Body    string
	Commits []domain.Commit
}

func withGitHubMergeFields(ctx context.Context, repoPath string, pr domain.PR) (domain.PR, error) {
	fields, err := fetchGitHubMergeFieldsFn(ctx, repoPath, pr.Number)
	if err != nil {
		// Drop cached title/commits so we omit --subject/--body rather than
		// overwriting GitHub's server-side message with Recur's stale copy.
		pr.Title = ""
		pr.Commits = nil
		pr.Comments = nil
		return pr, err
	}
	return overlayGitHubMergeFields(pr, fields), nil
}

func overlayGitHubMergeFields(pr domain.PR, fields githubMergeFields) domain.PR {
	if t := strings.TrimSpace(fields.Title); t != "" {
		pr.Title = t
	} else {
		pr.Title = ""
	}
	pr.Commits = fields.Commits
	pr.Comments = append([]domain.PRComment(nil), pr.Comments...)
	replaced := false
	body := strings.TrimSpace(fields.Body)
	for i, c := range pr.Comments {
		if c.Kind == "description" {
			pr.Comments[i].BodyMD = body
			replaced = true
			break
		}
	}
	if !replaced {
		pr.Comments = append(pr.Comments, domain.PRComment{
			Kind: "description", BodyMD: body,
		})
	}
	return pr
}

func fetchGitHubMergeFields(ctx context.Context, repoPath string, prNumber int) (githubMergeFields, error) {
	if prNumber <= 0 {
		return githubMergeFields{}, fmt.Errorf("invalid GitHub PR number %d", prNumber)
	}
	out, err := runGH(ctx, repoPath, "pr", "view", fmt.Sprintf("%d", prNumber),
		"--json", "title,body,commits")
	if err != nil {
		return githubMergeFields{}, err
	}
	return parseGitHubMergeFields(out)
}

func parseGitHubMergeFields(raw string) (githubMergeFields, error) {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return githubMergeFields{}, fmt.Errorf("empty gh pr view json")
	}
	var view struct {
		Title   string         `json:"title"`
		Body    string         `json:"body"`
		Commits []ghViewCommit `json:"commits"`
	}
	if err := json.Unmarshal([]byte(raw), &view); err != nil {
		return githubMergeFields{}, fmt.Errorf("parse gh pr view json: %w", err)
	}
	return githubMergeFields{
		Title:   strings.TrimSpace(view.Title),
		Body:    strings.TrimSpace(view.Body),
		Commits: commitsFromGHView(view.Commits),
	}, nil
}

// commitsFromGHView maps `gh pr view --json commits` into Recur commits.
// gh returns chronological (oldest-first); Recur commit lists are newest-first
// (git log), so this reverses to match.
func commitsFromGHView(cs []ghViewCommit) []domain.Commit {
	out := make([]domain.Commit, 0, len(cs))
	for _, c := range cs {
		msg := strings.TrimSpace(c.MessageHeadline)
		if msg == "" {
			continue
		}
		oid := strings.TrimSpace(c.OID)
		out = append(out, domain.Commit{
			Message: msg,
			Body:    strings.TrimSpace(c.MessageBody),
			SHA:     oid,
			Hash:    shortGitSHA(oid),
		})
	}
	slices.Reverse(out)
	return out
}

// runGH runs a gh subcommand in repoPath and returns trimmed stdout or an error
// that includes gh's stderr. repoPath may be "" (gh then uses the cwd).
func runGH(ctx context.Context, repoPath string, args ...string) (string, error) {
	out, _, err := runGHOutput(ctx, repoPath, args...)
	if err != nil {
		return out, fmt.Errorf("gh %s: %s", strings.Join(args, " "), strings.TrimSpace(out))
	}
	return out, nil
}

func runGHWithStdin(ctx context.Context, repoPath, stdin string, args ...string) (string, error) {
	cmd := exec.CommandContext(ctx, "gh", args...)
	if repoPath != "" {
		cmd.Dir = repoPath
	}
	cmd.Stdin = strings.NewReader(stdin)
	b, err := cmd.CombinedOutput()
	out := string(b)
	if err != nil {
		return out, fmt.Errorf("gh %s: %s", strings.Join(args, " "), strings.TrimSpace(out))
	}
	return out, nil
}

// runGHOutput is runGH without wrapping the error, so callers that need gh's
// exit code (e.g. `pr checks`: 0 pass, 1 fail, 8 pending) can inspect it.
func runGHOutput(ctx context.Context, repoPath string, args ...string) (out string, exitCode int, err error) {
	cmd := exec.CommandContext(ctx, "gh", args...)
	if repoPath != "" {
		cmd.Dir = repoPath
	}
	b, err := cmd.CombinedOutput()
	out = string(b)
	if err != nil {
		var ee *exec.ExitError
		if errors.As(err, &ee) {
			return out, ee.ExitCode(), err
		}
		return out, -1, err
	}
	return out, 0, nil
}

// getGitHubPRChecks fetches check runs for a GitHub PR. Tests replace this.
var getGitHubPRChecks = fetchGitHubPRChecks

// fetchGitHubPRChecks loads the GitHub Checks tab rollup: check runs, expected
// status contexts, queued Actions workflows, and required checks from branch
// rulesets that have not reported yet. `gh pr checks` misses those pending
// rows and links to Actions job pages, so GraphQL is preferred; the CLI JSON
// is a fallback. GitHub App suites with no workflow (e.g. Depot Code Access)
// are not Checks-tab rows and are omitted.
func fetchGitHubPRChecks(ctx context.Context, repoPath string, prNumber int, prURL string) ([]domain.Check, bool, error) {
	if prNumber <= 0 {
		return nil, false, fmt.Errorf("invalid GitHub PR number %d", prNumber)
	}
	if !ghCLIAvailable() {
		return nil, false, fmt.Errorf("gh CLI is not installed or not authenticated")
	}
	if checks, ok, err := fetchGitHubPRChecksGraphQL(ctx, repoPath, prNumber); err == nil {
		return checks, ok, nil
	} else {
		logGitHubPRChecksGraphQL(repoPath, prNumber, err)
	}
	return fetchGitHubPRChecksCLI(ctx, repoPath, prNumber, prURL)
}

func fetchGitHubPRChecksCLI(ctx context.Context, repoPath string, prNumber int, prURL string) ([]domain.Check, bool, error) {
	out, _, err := runGHOutput(ctx, repoPath,
		"pr", "checks", fmt.Sprintf("%d", prNumber),
		"--json", "name,state,bucket,workflow,description,startedAt,completedAt,link")
	checks, ok, parseErr := parseGitHubPRChecks(out)
	if parseErr == nil {
		if strings.TrimSpace(prURL) == "" {
			prURL = githubPRViewURL(ctx, repoPath, prNumber)
		}
		return applyGitHubChecksTabURLs(checks, prURL), ok, nil
	}
	if err != nil {
		msg := strings.TrimSpace(out)
		if msg == "" {
			msg = err.Error()
		}
		return nil, false, fmt.Errorf("gh pr checks %d: %s", prNumber, msg)
	}
	return nil, false, parseErr
}

const githubPRChecksGraphQL = `query($o:String!,$r:String!,$n:Int!){
  repository(owner:$o,name:$r){
    defaultBranchRef { name }
    rulesets(first:20){
      pageInfo { hasNextPage }
      nodes{
        enforcement
        target
        conditions { refName { include exclude } }
        rules(first:40){
          pageInfo { hasNextPage }
          nodes{
            type
            parameters {
              ... on RequiredStatusChecksParameters {
                requiredStatusChecks { context }
              }
            }
          }
        }
      }
    }
    pullRequest(number:$n){
      url
      baseRef {
        name
        branchProtectionRule { requiredStatusCheckContexts }
      }
      commits(last:1){
        nodes{
          commit{
            statusCheckRollup{
              contexts(first:100){
                pageInfo { hasNextPage }
                nodes{
                  __typename
                  ... on CheckRun {
                    name
                    status
                    conclusion
                    detailsUrl
                    permalink
                    startedAt
                    completedAt
                    checkSuite { workflowRun { workflow { name } } }
                  }
                  ... on StatusContext {
                    context
                    state
                    targetUrl
                    description
                    createdAt
                  }
                }
              }
            }
            checkSuites(first:30){
              pageInfo { hasNextPage }
              nodes{
                status
                conclusion
                workflowRun { url workflow { name } }
                checkRuns(first:1){ totalCount }
              }
            }
          }
        }
      }
    }
  }
}`

func fetchGitHubPRChecksGraphQL(ctx context.Context, repoPath string, prNumber int) ([]domain.Check, bool, error) {
	owner, repo, err := githubRepoOwnerName(ctx, repoPath)
	if err != nil {
		return nil, false, err
	}
	out, _, err := runGHOutput(ctx, repoPath, "api", "graphql",
		"-f", "query="+githubPRChecksGraphQL,
		"-f", "o="+owner,
		"-f", "r="+repo,
		"-F", "n="+fmt.Sprintf("%d", prNumber))
	checks, ok, parseErr := parseGitHubPRChecksGraphQL(out)
	if parseErr == nil {
		return checks, ok, nil
	}
	return nil, false, githubGraphQLCommandError(out, err, parseErr)
}

func githubGraphQLCommandError(out string, execErr, parseErr error) error {
	if parseErr == nil {
		return nil
	}
	if execErr == nil {
		return parseErr
	}
	msg := truncateGHDiag(out)
	if msg == "" {
		msg = execErr.Error()
	}
	return fmt.Errorf("gh api graphql: %s", msg)
}

func truncateGHDiag(s string) string {
	s = strings.TrimSpace(s)
	if len(s) > 240 {
		return s[:240] + "…"
	}
	return s
}

var githubRepoOwnerCache sync.Map // repoPath → "owner/repo"

// githubBranchPRTTL is how long a stored GitHub branch-rules probe is treated
// as fresh. Land state always returns the stored value; a refresh after this
// interval runs in the background so the titlebar is not blocked on gh.
var githubBranchPRTTL = 10 * time.Minute

// githubBranchRequiresPRFn, when set, replaces the store+gh probe. Tests only.
var githubBranchRequiresPRFn func(context.Context, string, string) bool

// ReplaceGitHubBranchRequiresPR swaps the branch-rules probe. Tests only.
func ReplaceGitHubBranchRequiresPR(fn func(context.Context, string, string) bool) func() {
	prev := githubBranchRequiresPRFn
	githubBranchRequiresPRFn = fn
	return func() { githubBranchRequiresPRFn = prev }
}

const githubBranchPRRefreshTO = 10 * time.Second

var githubBranchPRInflight sync.Map // projectID\0owner/repo\0branch

func (e *Engine) githubRemoteRequiresPR(ctx context.Context, proj domain.Project, remote, branch string) bool {
	branch = strings.TrimSpace(branch)
	remote = strings.TrimSpace(remote)
	if githubBranchRequiresPRFn != nil {
		return githubBranchRequiresPRFn(ctx, proj.RepoPath, branch)
	}
	if proj.ID == "" || proj.RepoPath == "" || remote == "" || branch == "" {
		return true
	}
	ownerRepo, err := githubOwnerRepoForRemote(ctx, proj.RepoPath, remote)
	if err != nil || ownerRepo == "" {
		return true
	}
	if cached, at, ok := e.Store.GitHubBranchRequirePR(proj.ID, ownerRepo, branch); ok {
		if time.Since(at) >= githubBranchPRTTL {
			e.scheduleGitHubBranchPRRefresh(proj, ownerRepo, remote, branch)
		}
		return cached
	}
	return e.fetchGitHubBranchRequirePR(ctx, proj, ownerRepo, remote, branch)
}

func (e *Engine) fetchGitHubBranchRequirePR(ctx context.Context, proj domain.Project, ownerRepo, remote, branch string) bool {
	require, known := fetchGitHubBranchRequiresPRFn(ctx, proj.RepoPath, ownerRepo, branch)
	if !known {
		return true
	}
	e.storeGitHubBranchRequirePR(proj.ID, ownerRepo, remote, branch, require, false)
	return require
}

func (e *Engine) scheduleGitHubBranchPRRefresh(proj domain.Project, ownerRepo, remote, branch string) {
	key := proj.ID + "\x00" + ownerRepo + "\x00" + branch
	if _, loaded := githubBranchPRInflight.LoadOrStore(key, struct{}{}); loaded {
		return
	}
	go func() {
		defer githubBranchPRInflight.Delete(key)
		ctx, cancel := context.WithTimeout(context.Background(), githubBranchPRRefreshTO)
		defer cancel()
		require, known := fetchGitHubBranchRequiresPRFn(ctx, proj.RepoPath, ownerRepo, branch)
		if known {
			e.storeGitHubBranchRequirePR(proj.ID, ownerRepo, remote, branch, require, true)
		}
	}()
}

// storeGitHubBranchRequirePR writes the probe. notify is for a background
// refresh whose answer changed — land RPCs already return the first probe.
func (e *Engine) storeGitHubBranchRequirePR(projectID, ownerRepo, remote, branch string, require, notify bool) {
	prev, _, had := e.Store.GitHubBranchRequirePR(projectID, ownerRepo, branch)
	e.Store.SetGitHubBranchRequirePR(projectID, ownerRepo, branch, require)
	if !notify || !had || prev == require || e.Bus == nil {
		return
	}
	e.Bus.Publish(Event{Name: "github.branch_rules", Data: map[string]any{
		"projectId": projectID,
		"repo":      ownerRepo,
		"remote":    remote,
		"branch":    branch,
		"requirePR": require,
	}})
}

var fetchGitHubBranchRequiresPRFn = fetchGitHubBranchRequiresPR

func fetchGitHubBranchRequiresPR(ctx context.Context, repoPath, ownerRepo, branch string) (require bool, known bool) {
	owner, repo, ok := splitOwnerRepo(ownerRepo)
	if !ok {
		return false, false
	}
	escaped := url.PathEscape(branch)
	rulesPath := fmt.Sprintf("repos/%s/%s/rules/branches/%s", owner, repo, escaped)
	rulesOut, rulesErr := runGH(ctx, repoPath, "api", "--paginate", "--slurp", rulesPath)
	if rulesErr == nil {
		if require, ok := githubRulesRequirePR(flattenJSONArrayPages(rulesOut)); ok && require {
			return true, true
		}
	}
	protPath := fmt.Sprintf("repos/%s/%s/branches/%s/protection", owner, repo, escaped)
	protOut, protErr := runGH(ctx, repoPath, "api", protPath)
	return githubBranchRequiresPRFromAPIs(rulesOut, rulesErr, protOut, protErr)
}

func githubBranchRequiresPRFromAPIs(rulesRaw string, rulesErr error, protRaw string, protErr error) (require bool, known bool) {
	var rulesRequire, rulesOK bool
	if rulesErr == nil {
		rulesRequire, rulesOK = githubRulesRequirePR(flattenJSONArrayPages(rulesRaw))
	}
	if rulesOK && rulesRequire {
		return true, true
	}
	classicRequire, classicOK := githubClassicProtectionRequiresPR(protRaw, protErr)
	if classicOK && classicRequire {
		return true, true
	}
	if rulesOK && classicOK {
		return false, true
	}
	return false, false
}

func githubRulesRequirePR(raw string) (require bool, ok bool) {
	raw = strings.TrimSpace(raw)
	if i := strings.IndexByte(raw, '['); i >= 0 {
		if j := strings.LastIndexByte(raw, ']'); j > i {
			raw = raw[i : j+1]
		}
	}
	if raw == "" {
		return false, false
	}
	var rules []struct {
		Type string `json:"type"`
	}
	if json.Unmarshal([]byte(raw), &rules) != nil {
		return false, false
	}
	for _, r := range rules {
		if strings.EqualFold(strings.TrimSpace(r.Type), "pull_request") {
			return true, true
		}
	}
	return false, true
}

func githubClassicProtectionRequiresPR(raw string, err error) (require bool, ok bool) {
	if err != nil {
		if githubHTTPNotFound(err) {
			return false, true
		}
		return false, false
	}
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return false, false
	}
	var prot struct {
		RequiredPullRequestReviews json.RawMessage `json:"required_pull_request_reviews"`
	}
	if json.Unmarshal([]byte(raw), &prot) != nil {
		return false, false
	}
	rev := strings.TrimSpace(string(prot.RequiredPullRequestReviews))
	if rev == "" || rev == "null" {
		return false, true
	}
	return true, true
}

func githubHTTPNotFound(err error) bool {
	return err != nil && strings.Contains(err.Error(), "(HTTP 404)")
}

func githubOwnerRepoForRemote(ctx context.Context, repoPath, remote string) (string, error) {
	out, err := runGit(ctx, repoPath, "remote", "get-url", remote)
	if err != nil {
		return "", err
	}
	ownerRepo, ok := OwnerRepoFromRemoteURL(out)
	if !ok {
		return "", fmt.Errorf("remote %s is not a GitHub URL", remote)
	}
	return ownerRepo, nil
}

// OwnerRepoFromRemoteURL parses owner/repo from a GitHub ssh or https remote URL.
func OwnerRepoFromRemoteURL(raw string) (string, bool) {
	s := strings.TrimSpace(raw)
	if s == "" || strings.HasPrefix(s, "/") || strings.HasPrefix(s, ".") {
		return "", false
	}
	var path string
	if i := strings.Index(s, "://"); i >= 0 {
		rest := s[i+3:]
		slash := strings.IndexByte(rest, '/')
		if slash < 0 {
			return "", false
		}
		path = rest[slash+1:]
	} else if i := strings.IndexByte(s, ':'); i >= 0 {
		path = s[i+1:]
	} else {
		return "", false
	}
	path = strings.TrimPrefix(path, "/")
	path = strings.TrimSuffix(path, "/")
	path = strings.TrimSuffix(path, ".git")
	parts := strings.Split(path, "/")
	if len(parts) < 2 || parts[0] == "" || parts[1] == "" {
		return "", false
	}
	return parts[0] + "/" + strings.TrimSuffix(parts[1], ".git"), true
}

var githubPRChecksGQLLogged sync.Map // repoPath\0prNumber\0msg → struct{}

func logGitHubPRChecksGraphQL(repoPath string, prNumber int, err error) {
	msg := truncateGHDiag(err.Error())
	key := repoPath + "\x00" + strconv.Itoa(prNumber) + "\x00" + msg
	if _, loaded := githubPRChecksGQLLogged.LoadOrStore(key, struct{}{}); loaded {
		return
	}
	log.Printf("recur: github PR checks graphql %d: %s", prNumber, msg)
}

func githubRepoOwnerName(ctx context.Context, repoPath string) (owner, repo string, err error) {
	if v, ok := githubRepoOwnerCache.Load(repoPath); ok {
		if owner, repo, ok := splitOwnerRepo(v.(string)); ok {
			return owner, repo, nil
		}
	}
	out, err := runGH(ctx, repoPath, "repo", "view", "--json", "nameWithOwner")
	if err != nil {
		return "", "", err
	}
	var meta struct {
		NameWithOwner string `json:"nameWithOwner"`
	}
	if err := json.Unmarshal([]byte(strings.TrimSpace(out)), &meta); err != nil {
		return "", "", fmt.Errorf("parse gh repo view: %w", err)
	}
	owner, repo, ok := splitOwnerRepo(meta.NameWithOwner)
	if !ok {
		return "", "", fmt.Errorf("missing owner/repo")
	}
	githubRepoOwnerCache.Store(repoPath, meta.NameWithOwner)
	return owner, repo, nil
}

func githubPRViewURL(ctx context.Context, repoPath string, prNumber int) string {
	out, err := runGH(ctx, repoPath, "pr", "view", fmt.Sprintf("%d", prNumber), "--json", "url")
	if err != nil {
		return ""
	}
	var meta struct {
		URL string `json:"url"`
	}
	if err := json.Unmarshal([]byte(strings.TrimSpace(out)), &meta); err != nil {
		return ""
	}
	return strings.TrimSpace(meta.URL)
}

// ghPRCheck is one row from `gh pr checks --json`.
type ghPRCheck struct {
	Name        string `json:"name"`
	State       string `json:"state"`
	Bucket      string `json:"bucket"`
	Workflow    string `json:"workflow"`
	Description string `json:"description"`
	StartedAt   string `json:"startedAt"`
	CompletedAt string `json:"completedAt"`
	Link        string `json:"link"`
}

// parseGitHubPRChecks maps `gh pr checks --json` output to Check rows.
// Empty output or "no checks reported" is a clean rollup (nothing failed).
func parseGitHubPRChecks(raw string) ([]domain.Check, bool, error) {
	return parseGitHubPRChecksAt(raw, time.Now())
}

func parseGitHubPRChecksAt(raw string, now time.Time) ([]domain.Check, bool, error) {
	raw = strings.TrimSpace(raw)
	if raw == "" || strings.Contains(strings.ToLower(raw), "no checks reported") {
		return nil, true, nil
	}
	if i := strings.IndexByte(raw, '['); i >= 0 {
		if j := strings.LastIndexByte(raw, ']'); j > i {
			raw = raw[i : j+1]
		}
	}
	var rows []ghPRCheck
	if err := json.Unmarshal([]byte(raw), &rows); err != nil {
		return nil, false, fmt.Errorf("parse gh pr checks json: %w", err)
	}
	checks := make([]domain.Check, 0, len(rows))
	for _, row := range rows {
		checks = append(checks, githubCheckFromRow(row, now))
	}
	return checks, allChecksPassed(checks), nil
}

type ghGQLChecksPayload struct {
	Data struct {
		Repository struct {
			DefaultBranchRef *struct {
				Name string `json:"name"`
			} `json:"defaultBranchRef"`
			Rulesets struct {
				PageInfo ghGQLPageInfo  `json:"pageInfo"`
				Nodes    []ghGQLRuleset `json:"nodes"`
			} `json:"rulesets"`
			PullRequest struct {
				URL     string `json:"url"`
				BaseRef *struct {
					Name                 string `json:"name"`
					BranchProtectionRule *struct {
						RequiredStatusCheckContexts []string `json:"requiredStatusCheckContexts"`
					} `json:"branchProtectionRule"`
				} `json:"baseRef"`
				Commits struct {
					Nodes []struct {
						Commit struct {
							StatusCheckRollup *struct {
								Contexts struct {
									PageInfo ghGQLPageInfo       `json:"pageInfo"`
									Nodes    []ghGQLCheckContext `json:"nodes"`
								} `json:"contexts"`
							} `json:"statusCheckRollup"`
							CheckSuites struct {
								PageInfo ghGQLPageInfo     `json:"pageInfo"`
								Nodes    []ghGQLCheckSuite `json:"nodes"`
							} `json:"checkSuites"`
						} `json:"commit"`
					} `json:"nodes"`
				} `json:"commits"`
			} `json:"pullRequest"`
		} `json:"repository"`
	} `json:"data"`
	Errors []struct {
		Message string `json:"message"`
	} `json:"errors"`
}

type ghGQLPageInfo struct {
	HasNextPage     bool `json:"hasNextPage"`
	HasPreviousPage bool `json:"hasPreviousPage"`
}

type ghGQLRuleset struct {
	Enforcement string `json:"enforcement"`
	Target      string `json:"target"`
	Conditions  struct {
		RefName struct {
			Include []string `json:"include"`
			Exclude []string `json:"exclude"`
		} `json:"refName"`
	} `json:"conditions"`
	Rules struct {
		PageInfo ghGQLPageInfo `json:"pageInfo"`
		Nodes    []struct {
			Type       string `json:"type"`
			Parameters *struct {
				RequiredStatusChecks []struct {
					Context string `json:"context"`
				} `json:"requiredStatusChecks"`
			} `json:"parameters"`
		} `json:"nodes"`
	} `json:"rules"`
}

type ghGQLCheckContext struct {
	TypeName    string `json:"__typename"`
	Name        string `json:"name"`
	Status      string `json:"status"`
	Conclusion  string `json:"conclusion"`
	DetailsURL  string `json:"detailsUrl"`
	Permalink   string `json:"permalink"`
	StartedAt   string `json:"startedAt"`
	CompletedAt string `json:"completedAt"`
	CheckSuite  struct {
		WorkflowRun struct {
			Workflow struct {
				Name string `json:"name"`
			} `json:"workflow"`
		} `json:"workflowRun"`
	} `json:"checkSuite"`
	Context     string `json:"context"`
	State       string `json:"state"`
	TargetURL   string `json:"targetUrl"`
	Description string `json:"description"`
	CreatedAt   string `json:"createdAt"`
}

type ghGQLCheckSuite struct {
	Status      string `json:"status"`
	Conclusion  string `json:"conclusion"`
	WorkflowRun *struct {
		URL      string `json:"url"`
		Workflow struct {
			Name string `json:"name"`
		} `json:"workflow"`
	} `json:"workflowRun"`
	CheckRuns struct {
		TotalCount int `json:"totalCount"`
	} `json:"checkRuns"`
}

func parseGitHubPRChecksGraphQL(raw string) ([]domain.Check, bool, error) {
	return parseGitHubPRChecksGraphQLAt(raw, time.Now())
}

func parseGitHubPRChecksGraphQLAt(raw string, now time.Time) ([]domain.Check, bool, error) {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return nil, false, fmt.Errorf("empty github checks graphql")
	}
	if i := strings.IndexByte(raw, '{'); i > 0 {
		raw = raw[i:]
	}
	var payload ghGQLChecksPayload
	if err := json.NewDecoder(strings.NewReader(raw)).Decode(&payload); err != nil {
		return nil, false, fmt.Errorf("parse github checks graphql: %w", err)
	}
	pr := payload.Data.Repository.PullRequest
	if strings.TrimSpace(pr.URL) == "" {
		if len(payload.Errors) > 0 {
			return nil, false, fmt.Errorf("github checks graphql: %s", payload.Errors[0].Message)
		}
		return nil, false, fmt.Errorf("github checks graphql: missing pull request")
	}
	if len(pr.Commits.Nodes) == 0 {
		if len(payload.Errors) > 0 {
			return nil, false, fmt.Errorf("github checks graphql: %s", payload.Errors[0].Message)
		}
		return nil, true, nil
	}
	commit := pr.Commits.Nodes[0].Commit
	prURL := strings.TrimSpace(pr.URL)
	rows := make([]ghPRCheck, 0)
	seen := map[string]bool{}
	if commit.StatusCheckRollup != nil {
		for _, n := range commit.StatusCheckRollup.Contexts.Nodes {
			row := githubCheckRowFromGQLContext(n)
			if strings.TrimSpace(row.Name) == "" {
				continue
			}
			rows = append(rows, row)
			seen[row.Name] = true
		}
	}
	for _, suite := range commit.CheckSuites.Nodes {
		if suite.CheckRuns.TotalCount > 0 {
			continue
		}
		bucket := bucketFromGHState(firstNonEmptyStr(suite.Conclusion, suite.Status))
		if bucket != "pending" {
			continue
		}
		name := ""
		if suite.WorkflowRun != nil {
			name = strings.TrimSpace(suite.WorkflowRun.Workflow.Name)
		}
		// GitHub App suites with no workflow (e.g. "Depot Code Access") are not
		// Checks-tab rows. Only queued Actions workflows get a placeholder.
		if name == "" || seen[name] {
			continue
		}
		seen[name] = true
		rows = append(rows, ghPRCheck{
			Name:   name,
			State:  suite.Status,
			Bucket: bucket,
		})
	}
	for _, name := range requiredGitHubCheckNames(payload) {
		if seen[name] {
			continue
		}
		seen[name] = true
		rows = append(rows, ghPRCheck{
			Name:   name,
			State:  "EXPECTED",
			Bucket: "pending",
		})
	}
	if len(rows) == 0 && len(payload.Errors) > 0 {
		return nil, false, fmt.Errorf("github checks graphql: %s", payload.Errors[0].Message)
	}
	checks := make([]domain.Check, 0, len(rows))
	for _, row := range rows {
		checks = append(checks, githubCheckFromRow(row, now))
	}
	ok := allChecksPassed(checks) && !githubChecksGraphQLTruncated(payload)
	return applyGitHubChecksTabURLs(checks, prURL), ok, nil
}

func githubChecksGraphQLTruncated(payload ghGQLChecksPayload) bool {
	repo := payload.Data.Repository
	if repo.Rulesets.PageInfo.HasNextPage {
		return true
	}
	for _, rs := range repo.Rulesets.Nodes {
		if rs.Rules.PageInfo.HasNextPage {
			return true
		}
	}
	if len(repo.PullRequest.Commits.Nodes) == 0 {
		return false
	}
	commit := repo.PullRequest.Commits.Nodes[0].Commit
	if commit.StatusCheckRollup != nil && commit.StatusCheckRollup.Contexts.PageInfo.HasNextPage {
		return true
	}
	return commit.CheckSuites.PageInfo.HasNextPage
}

func githubCheckRowFromGQLContext(n ghGQLCheckContext) ghPRCheck {
	if strings.EqualFold(n.TypeName, "StatusContext") || strings.TrimSpace(n.Context) != "" && n.Name == "" {
		return ghPRCheck{
			Name:        firstNonEmptyStr(strings.TrimSpace(n.Context), "check"),
			State:       n.State,
			Bucket:      bucketFromGHState(n.State),
			Description: n.Description,
			StartedAt:   n.CreatedAt,
			Link:        strings.TrimSpace(n.TargetURL),
		}
	}
	state := n.Status
	if strings.EqualFold(n.Status, "COMPLETED") {
		state = n.Conclusion
	}
	link := strings.TrimSpace(n.Permalink)
	if link == "" {
		link = strings.TrimSpace(n.DetailsURL)
	}
	return ghPRCheck{
		Name:        firstNonEmptyStr(firstNonEmptyStr(strings.TrimSpace(n.Name), n.CheckSuite.WorkflowRun.Workflow.Name), "check"),
		State:       state,
		Bucket:      bucketFromGHState(state),
		Workflow:    n.CheckSuite.WorkflowRun.Workflow.Name,
		Description: n.Description,
		StartedAt:   n.StartedAt,
		CompletedAt: n.CompletedAt,
		Link:        link,
	}
}

func requiredGitHubCheckNames(payload ghGQLChecksPayload) []string {
	repo := payload.Data.Repository
	base := ""
	if repo.PullRequest.BaseRef != nil {
		base = strings.TrimSpace(repo.PullRequest.BaseRef.Name)
	}
	defaultBranch := ""
	if repo.DefaultBranchRef != nil {
		defaultBranch = strings.TrimSpace(repo.DefaultBranchRef.Name)
	}
	seen := map[string]bool{}
	var out []string
	add := func(name string) {
		name = strings.TrimSpace(name)
		if name == "" || seen[name] {
			return
		}
		seen[name] = true
		out = append(out, name)
	}
	if repo.PullRequest.BaseRef != nil && repo.PullRequest.BaseRef.BranchProtectionRule != nil {
		for _, name := range repo.PullRequest.BaseRef.BranchProtectionRule.RequiredStatusCheckContexts {
			add(name)
		}
	}
	for _, rs := range repo.Rulesets.Nodes {
		if !strings.EqualFold(rs.Enforcement, "ACTIVE") {
			continue
		}
		if rs.Target != "" && !strings.EqualFold(rs.Target, "BRANCH") {
			continue
		}
		if !githubRulesetAppliesToBase(rs.Conditions.RefName.Include, rs.Conditions.RefName.Exclude, base, defaultBranch) {
			continue
		}
		for _, rule := range rs.Rules.Nodes {
			if !strings.EqualFold(rule.Type, "REQUIRED_STATUS_CHECKS") || rule.Parameters == nil {
				continue
			}
			for _, c := range rule.Parameters.RequiredStatusChecks {
				add(c.Context)
			}
		}
	}
	return out
}

func githubRulesetAppliesToBase(include, exclude []string, base, defaultBranch string) bool {
	base = strings.TrimSpace(base)
	if base == "" {
		return false
	}
	for _, pat := range exclude {
		if githubRefConditionMatches(pat, base, defaultBranch) {
			return false
		}
	}
	if len(include) == 0 {
		return true
	}
	for _, pat := range include {
		if githubRefConditionMatches(pat, base, defaultBranch) {
			return true
		}
	}
	return false
}

func githubRefConditionMatches(pat, base, defaultBranch string) bool {
	pat = strings.TrimSpace(pat)
	base = strings.TrimSpace(base)
	if pat == "" || base == "" {
		return false
	}
	switch strings.ToUpper(pat) {
	case "~ALL":
		return true
	case "~DEFAULT_BRANCH":
		return base == strings.TrimSpace(defaultBranch)
	}
	full := base
	if !strings.HasPrefix(full, "refs/") {
		full = "refs/heads/" + base
	}
	return githubRefGlob(pat, base) || githubRefGlob(pat, full)
}

func githubRefGlob(pat, name string) bool {
	if pat == name {
		return true
	}
	var b strings.Builder
	b.WriteByte('^')
	runes := []rune(pat)
	for i := 0; i < len(runes); i++ {
		r := runes[i]
		if r == '*' {
			if i+1 < len(runes) && runes[i+1] == '*' {
				b.WriteString(".*")
				i++
				continue
			}
			b.WriteString("[^/]*")
			continue
		}
		if r == '?' {
			b.WriteString("[^/]")
			continue
		}
		b.WriteString(regexp.QuoteMeta(string(r)))
	}
	b.WriteByte('$')
	re, err := regexp.Compile(b.String())
	if err != nil {
		return false
	}
	return re.MatchString(name)
}

func githubPRChecksTabURL(prURL string) string {
	u := strings.TrimRight(strings.TrimSpace(prURL), "/")
	if u == "" {
		return ""
	}
	if strings.HasSuffix(strings.ToLower(u), "/checks") {
		return u
	}
	return u + "/checks"
}

func isGitHubActionsURL(u string) bool {
	return strings.Contains(u, "/actions/runs/")
}

func applyGitHubChecksTabURLs(checks []domain.Check, prURL string) []domain.Check {
	tab := githubPRChecksTabURL(prURL)
	if tab == "" || len(checks) == 0 {
		return checks
	}
	out := slices.Clone(checks)
	for i, c := range out {
		if c.Status == "pending" {
			if c.URL == "" || isGitHubActionsURL(c.URL) {
				out[i].URL = tab
			}
			continue
		}
		if c.URL == "" {
			out[i].URL = tab
		}
	}
	return out
}

func githubCheckFromRow(row ghPRCheck, now time.Time) domain.Check {
	bucket := strings.ToLower(strings.TrimSpace(row.Bucket))
	if bucket == "" {
		bucket = bucketFromGHState(row.State)
	}
	name := strings.TrimSpace(row.Name)
	if name == "" {
		name = firstNonEmptyStr(row.Workflow, "check")
	}
	c := domain.Check{
		Name:   name,
		Detail: githubCheckDetail(row, bucket, now),
		Passed: bucket == "pass",
		Status: bucket,
		URL:    strings.TrimSpace(row.Link),
	}
	if t, ok := parseGHTime(row.StartedAt); ok {
		c.StartedAt = t.UTC().Format(time.RFC3339)
	}
	return c
}

func githubCheckDetail(row ghPRCheck, bucket string, now time.Time) string {
	if bucket == "skipping" {
		return "skipped"
	}
	start, hasStart := parseGHTime(row.StartedAt)
	end, hasEnd := parseGHTime(row.CompletedAt)
	if hasStart && hasEnd && !end.Before(start) {
		return formatCheckDuration(end.Sub(start))
	}
	if bucket == "pending" && hasStart && !now.Before(start) {
		return formatCheckDuration(now.Sub(start))
	}
	if bucket == "pending" {
		return "pending"
	}
	if bucket == "cancel" {
		return "cancelled"
	}
	return ""
}

func parseGHTime(s string) (time.Time, bool) {
	s = strings.TrimSpace(s)
	if s == "" {
		return time.Time{}, false
	}
	if t, err := time.Parse(time.RFC3339Nano, s); err == nil {
		return t, true
	}
	if t, err := time.Parse(time.RFC3339, s); err == nil {
		return t, true
	}
	return time.Time{}, false
}

func formatCheckDuration(d time.Duration) string {
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
	return fmt.Sprintf("%dm %ds", m, s)
}

func bucketFromGHState(state string) string {
	switch strings.ToUpper(strings.TrimSpace(state)) {
	case "SUCCESS", "NEUTRAL":
		return "pass"
	case "SKIPPED":
		return "skipping"
	case "PENDING", "QUEUED", "IN_PROGRESS", "WAITING", "REQUESTED", "EXPECTED":
		return "pending"
	case "STALE", "CANCELLED", "CANCELED":
		return "cancel"
	default:
		if state == "" {
			return "pending"
		}
		return "fail"
	}
}

// githubPRChecks is the best-effort fetch used at PR open. A fetch error
// returns an empty rollup and checksOK=true (unknown is not a failure).
func (e *Engine) githubPRChecks(ctx context.Context, repoPath string, prNumber int) ([]domain.Check, bool) {
	if ctx == nil {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()
	}
	checks, ok, err := getGitHubPRChecks(ctx, repoPath, prNumber, "")
	if err != nil {
		return nil, true
	}
	return checks, ok
}

// runGit runs a git command in repoPath (branch pushes for the GitHub PR path go
// through git, not gh). Same contract as runGH.
func runGit(ctx context.Context, repoPath string, args ...string) (string, error) {
	cmd := gitpkg.Command(ctx, args...)
	if repoPath != "" {
		cmd.Dir = repoPath
	}
	out, err := cmd.CombinedOutput()
	if err != nil {
		return string(out), fmt.Errorf("git %s: %s", strings.Join(args, " "), strings.TrimSpace(string(out)))
	}
	return string(out), nil
}

// prNumberFromURL extracts the trailing PR number from a gh-printed PR URL
// (e.g. https://github.com/owner/repo/pull/482 → 482). Returns 0 on a non-numeric
// tail.
func prNumberFromURL(url string) int {
	url = strings.TrimRight(url, "/")
	i := strings.LastIndexByte(url, '/')
	if i < 0 {
		return 0
	}
	n := 0
	for _, r := range url[i+1:] {
		if r < '0' || r > '9' {
			return 0
		}
		n = n*10 + int(r-'0')
	}
	return n
}
