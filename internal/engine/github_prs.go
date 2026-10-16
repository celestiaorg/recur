package engine

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

const (
	prGroupYou    = "you"
	prGroupLoop   = "loop"
	prGroupOpen   = "open"
	prGroupMerged = "merged"

	githubOpenPRLimit   = 100
	githubMergedPRLimit = 20
)

// listGitHubPRsFn lists GitHub PRs for a repo. Tests replace this.
var listGitHubPRsFn = fetchGitHubPRList

// githubLoginFn is the authenticated gh user. Tests replace this.
var githubLoginFn = fetchGitHubLogin

// ghListedPR is one row from `gh pr list --json`.
type ghListedPR struct {
	Number            int              `json:"number"`
	Title             string           `json:"title"`
	URL               string           `json:"url"`
	HeadRefName       string           `json:"headRefName"`
	BaseRefName       string           `json:"baseRefName"`
	Author            ghListedActor    `json:"author"`
	IsDraft           bool             `json:"isDraft"`
	IsCrossRepository bool             `json:"isCrossRepository"`
	CreatedAt         string           `json:"createdAt"`
	UpdatedAt         string           `json:"updatedAt"`
	Additions         int              `json:"additions"`
	Deletions         int              `json:"deletions"`
	ChangedFiles      int              `json:"changedFiles"`
	ReviewDecision    string           `json:"reviewDecision"`
	ReviewRequests    []ghListedActor  `json:"reviewRequests"`
	Assignees         []ghListedActor  `json:"assignees"`
	LatestReviews     []ghListedReview `json:"latestReviews"`
	State             string           `json:"state"`
	MergeStateStatus  string           `json:"mergeStateStatus"`
	StatusCheckRollup json.RawMessage  `json:"statusCheckRollup"`
}

type ghListedActor struct {
	Login     string `json:"login"`
	AvatarURL string `json:"avatarUrl,omitempty"`
}

type ghListedReview struct {
	Author ghListedActor `json:"author"`
	State  string        `json:"state"`
}

func fetchGitHubLogin(ctx context.Context) string {
	out, err := runGH(ctx, "", "api", "user", "-q", ".login")
	if err != nil {
		return ""
	}
	return strings.TrimSpace(out)
}

func fetchGitHubPRList(ctx context.Context, repoPath, state string, limit int) ([]ghListedPR, error) {
	if strings.TrimSpace(repoPath) == "" {
		return nil, fmt.Errorf("empty repo path")
	}
	if limit <= 0 {
		limit = githubOpenPRLimit
	}
	state = strings.ToLower(strings.TrimSpace(state))
	if state == "" {
		state = "open"
	}
	jsonFields := "number,title,url,headRefName,baseRefName,author,isDraft,isCrossRepository,createdAt,updatedAt,additions,deletions,changedFiles,reviewDecision,reviewRequests,assignees,latestReviews,state,mergeStateStatus,statusCheckRollup"
	args := []string{"pr", "list", "--limit", fmt.Sprintf("%d", limit), "--json", jsonFields}
	if state == "merged" {
		// Default `gh pr list --state merged` orders by created-at, so a
		// long-lived PR that just merged falls off the 20-row cap. Search by
		// updated-desc so recently merged rows stay in `seen`.
		args = append(args, "--state", "merged", "--search", "sort:updated-desc")
	} else {
		args = append(args, "--state", state)
	}
	out, err := runGH(ctx, repoPath, args...)
	if err != nil {
		return nil, err
	}
	return parseGitHubPRList(out)
}

var githubPRViewStateFn = fetchGitHubPRViewState

type ghPRViewState struct {
	State     string
	UpdatedAt string
}

func fetchGitHubPRViewState(ctx context.Context, repoPath string, number int) (ghPRViewState, error) {
	if number <= 0 {
		return ghPRViewState{}, fmt.Errorf("invalid PR number %d", number)
	}
	out, err := runGH(ctx, repoPath, "pr", "view", fmt.Sprintf("%d", number), "--json", "state,updatedAt")
	if err != nil {
		return ghPRViewState{}, err
	}
	raw := strings.TrimSpace(out)
	if i := strings.IndexByte(raw, '{'); i >= 0 {
		if j := strings.LastIndexByte(raw, '}'); j > i {
			raw = raw[i : j+1]
		}
	}
	var row struct {
		State     string `json:"state"`
		UpdatedAt string `json:"updatedAt"`
	}
	if err := json.Unmarshal([]byte(raw), &row); err != nil {
		return ghPRViewState{}, fmt.Errorf("parse gh pr view state: %w", err)
	}
	return ghPRViewState{
		State:     strings.ToUpper(strings.TrimSpace(row.State)),
		UpdatedAt: normalizePRCreatedAt(row.UpdatedAt),
	}, nil
}

func parseGitHubPRList(raw string) ([]ghListedPR, error) {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return nil, nil
	}
	if i := strings.IndexByte(raw, '['); i >= 0 {
		if j := strings.LastIndexByte(raw, ']'); j > i {
			raw = raw[i : j+1]
		}
	}
	var rows []ghListedPR
	if err := json.Unmarshal([]byte(raw), &rows); err != nil {
		return nil, fmt.Errorf("parse gh pr list json: %w", err)
	}
	return rows, nil
}

func listedActorLogin(a ghListedActor) string {
	return strings.TrimSpace(a.Login)
}

func listedHasLogin(actors []ghListedActor, login string) bool {
	login = strings.TrimSpace(login)
	if login == "" {
		return false
	}
	for _, a := range actors {
		if strings.EqualFold(listedActorLogin(a), login) {
			return true
		}
	}
	return false
}

type listedCheckItem struct {
	Name       string `json:"name"`
	Context    string `json:"context"`
	Conclusion string `json:"conclusion"`
	Status     string `json:"status"`
	State      string `json:"state"`
}

func parseListedCheckItems(row ghListedPR) []listedCheckItem {
	raw := strings.TrimSpace(string(row.StatusCheckRollup))
	if raw == "" || raw == "null" {
		return nil
	}
	var items []listedCheckItem
	if err := json.Unmarshal([]byte(raw), &items); err != nil {
		var wrap struct {
			Nodes []listedCheckItem `json:"nodes"`
		}
		if err := json.Unmarshal([]byte(raw), &wrap); err != nil {
			return nil
		}
		items = wrap.Nodes
	}
	return items
}

func listedChecks(row ghListedPR) (checks []domain.Check, ok, has bool) {
	ok = true
	items := parseListedCheckItems(row)
	if len(items) == 0 {
		return nil, true, false
	}
	has = true
	checks = make([]domain.Check, 0, len(items))
	for _, it := range items {
		if listedCheckFailed(it.Conclusion, it.State) {
			ok = false
		}
		checks = append(checks, listedCheck(it))
	}
	return checks, ok, has
}

func listedCheck(it listedCheckItem) domain.Check {
	name := strings.TrimSpace(it.Name)
	if name == "" {
		name = strings.TrimSpace(it.Context)
	}
	if name == "" {
		name = "check"
	}
	bucket := listedCheckBucket(it)
	return domain.Check{Name: name, Passed: bucket == "pass", Status: bucket}
}

func listedCheckBucket(it listedCheckItem) string {
	status := strings.TrimSpace(it.Status)
	if strings.EqualFold(status, "COMPLETED") || status == "" {
		if c := strings.TrimSpace(it.Conclusion); c != "" {
			return bucketFromGHState(c)
		}
	}
	return bucketFromGHState(firstNonEmptyStr(status, it.State))
}

func checkRowsKind(checks []domain.Check) string {
	fail, pending := false, false
	n := 0
	for _, c := range checks {
		if strings.EqualFold(c.Status, "skipping") {
			continue
		}
		n++
		if strings.EqualFold(c.Status, "pending") {
			pending = true
			continue
		}
		if !c.Passed {
			fail = true
		}
	}
	if n == 0 {
		return "none"
	}
	if fail {
		return "fail"
	}
	if pending {
		return "pending"
	}
	return "pass"
}

func reconcileListedChecks(existing, listed []domain.Check) (out []domain.Check, refresh bool) {
	if len(listed) == 0 {
		return existing, false
	}
	byName := make(map[string]domain.Check, len(existing))
	for _, c := range existing {
		if name := strings.TrimSpace(c.Name); name != "" {
			byName[strings.ToLower(name)] = c
		}
	}
	out = make([]domain.Check, 0, len(listed)+len(existing))
	seen := make(map[string]struct{}, len(listed))
	for _, c := range listed {
		key := strings.ToLower(strings.TrimSpace(c.Name))
		seen[key] = struct{}{}
		if ex, ok := byName[key]; ok && checkRowsKind([]domain.Check{ex}) == checkRowsKind([]domain.Check{c}) {
			out = append(out, ex)
			continue
		}
		out = append(out, c)
	}
	for _, c := range existing {
		key := strings.ToLower(strings.TrimSpace(c.Name))
		if _, ok := seen[key]; ok {
			continue
		}
		if strings.EqualFold(c.Status, "pending") {
			out = append(out, c)
			refresh = true
		}
	}
	return out, refresh
}

func listedCheckRollup(row ghListedPR) (ok, has bool) {
	_, ok, has = listedChecks(row)
	return ok, has
}

func listedCheckFailed(conclusion, state string) bool {
	for _, v := range []string{conclusion, state} {
		switch strings.ToLower(strings.TrimSpace(v)) {
		case "failure", "error", "cancelled", "canceled", "timed_out", "action_required", "failing", "startup_failure":
			return true
		}
	}
	return false
}

func listedChecksOK(row ghListedPR) bool {
	ok, _ := listedCheckRollup(row)
	return ok
}

type attentionInput struct {
	State          string
	Author         string
	Viewer         string
	ReviewDecision string
	ReviewRequests []ghListedActor
	Assignees      []ghListedActor
	ChecksOK       bool
	Draft          bool
}

func attentionReasons(in attentionInput) []string {
	if strings.EqualFold(in.State, "MERGED") || strings.EqualFold(in.State, "CLOSED") {
		return nil
	}
	if in.Draft {
		return nil
	}
	viewer := strings.TrimSpace(in.Viewer)
	if viewer == "" {
		return nil
	}
	var out []string
	if listedHasLogin(in.ReviewRequests, viewer) {
		out = append(out, domain.AttentionReviewRequested)
	}
	if listedHasLogin(in.Assignees, viewer) {
		out = append(out, domain.AttentionAssigned)
	}
	if strings.EqualFold(strings.TrimSpace(in.Author), viewer) {
		if strings.EqualFold(in.ReviewDecision, "CHANGES_REQUESTED") {
			out = append(out, domain.AttentionChangesRequested)
		}
		if !in.ChecksOK {
			out = append(out, domain.AttentionChecksFailing)
		}
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

func listedAttentionReasons(row ghListedPR, viewer string) []string {
	return attentionReasons(attentionInput{
		State:          row.State,
		Author:         listedActorLogin(row.Author),
		Viewer:         viewer,
		ReviewDecision: row.ReviewDecision,
		ReviewRequests: row.ReviewRequests,
		Assignees:      row.Assignees,
		ChecksOK:       listedChecksOK(row),
		Draft:          row.IsDraft,
	})
}

func listedNeedsAttention(row ghListedPR, viewer string) bool {
	return len(listedAttentionReasons(row, viewer)) > 0
}

func listedReviewRequested(row ghListedPR, viewer string) bool {
	return listedHasLogin(row.ReviewRequests, viewer)
}

func snapshotAttentionReasons(snap ghPRSnapshot, viewer string, checksOK bool, draft bool) []string {
	return attentionReasons(attentionInput{
		State:          snap.State,
		Author:         snap.AuthorLogin,
		Viewer:         viewer,
		ReviewDecision: snap.ReviewDecision,
		ReviewRequests: snap.ReviewRequests,
		Assignees:      snap.Assignees,
		ChecksOK:       checksOK,
		Draft:          draft,
	})
}

// conversationAttention overlays GitHub review-request / assignee reasons onto
// a conversation refresh. Without a viewer login it leaves stored reasons and
// grouping alone so a partial snapshot cannot wipe the dashboard.
func conversationAttention(pr domain.PR, snap ghPRSnapshot, state, group string) (reasons []string, reviewReq bool, nextGroup string) {
	reasons, reviewReq, nextGroup = pr.AttentionReasons, pr.ReviewRequested, group
	viewer := strings.TrimSpace(snap.ViewerLogin)
	if viewer == "" {
		viewer = strings.TrimSpace(pr.ViewerLogin)
	}
	if viewer == "" {
		return
	}
	reasons = snapshotAttentionReasons(snap, viewer, pr.ChecksOK, snap.IsDraft || pr.Draft)
	reviewReq = listedHasLogin(snap.ReviewRequests, viewer)
	if group == prGroupLoop || state == "Merged" || state == "Closed" {
		return
	}
	if len(reasons) > 0 {
		nextGroup = prGroupYou
	} else {
		nextGroup = prGroupOpen
	}
	return
}

func listedBehind(row ghListedPR) int {
	if strings.EqualFold(strings.TrimSpace(row.MergeStateStatus), "BEHIND") {
		return 1
	}
	return 0
}

// prHasLocalBehind is true when RefreshPRBehind can compute a real count from
// a live worktree. Listing's 0/1 mergeStateStatus must not overwrite that.
func (e *Engine) prHasLocalBehind(workspaceID string) bool {
	id := e.livePRWorkspaceID(workspaceID)
	if id == "" {
		return false
	}
	w, ok := e.Store.Workspace(id)
	return ok && strings.TrimSpace(w.WorktreePath) != ""
}

func prSettled(p domain.PR) bool {
	return p.State == "Merged" || p.State == "Closed" || p.Group == prGroupMerged
}

func listedStateGroup(row ghListedPR, viewer string) (state, group string) {
	switch strings.ToUpper(strings.TrimSpace(row.State)) {
	case "MERGED":
		return "Merged", prGroupMerged
	case "CLOSED":
		return "Closed", prGroupMerged
	}
	if listedNeedsAttention(row, viewer) {
		group = prGroupYou
	} else {
		group = prGroupOpen
	}
	switch strings.ToUpper(strings.TrimSpace(row.ReviewDecision)) {
	case "APPROVED":
		return "Approved", group
	case "CHANGES_REQUESTED":
		return "Changes requested", group
	case "REVIEW_REQUIRED":
		return "Review required", group
	}
	if group == prGroupYou {
		return "In review", group
	}
	return "Open", group
}

func compactAgo(t, now time.Time) string {
	if t.IsZero() {
		return ""
	}
	d := now.Sub(t)
	if d < 0 {
		d = 0
	}
	if d < time.Minute {
		return "now"
	}
	if d < time.Hour {
		return fmt.Sprintf("%dm", int(d.Minutes()))
	}
	if d < 24*time.Hour {
		return fmt.Sprintf("%dh", int(d.Hours()))
	}
	days := int(d.Hours() / 24)
	if days < 30 {
		return fmt.Sprintf("%dd", days)
	}
	months := days / 30
	if months < 12 {
		return fmt.Sprintf("%dmo", months)
	}
	return fmt.Sprintf("%dy", months/12)
}

func parseGitHubTime(s string) time.Time {
	s = strings.TrimSpace(s)
	if s == "" {
		return time.Time{}
	}
	if t, err := time.Parse(time.RFC3339, s); err == nil {
		return t
	}
	if t, err := time.Parse(time.RFC3339Nano, s); err == nil {
		return t
	}
	return time.Time{}
}

func normalizePRCreatedAt(s string) string {
	t := parseGitHubTime(s)
	if t.IsZero() {
		return ""
	}
	return t.UTC().Format(time.RFC3339)
}

func listedCreatedAt(row ghListedPR) string {
	return normalizePRCreatedAt(row.CreatedAt)
}

func listedUpdatedAt(row ghListedPR) string {
	return normalizePRCreatedAt(row.UpdatedAt)
}

func githubImportedPRID(projectID string, number int) string {
	return fmt.Sprintf("gh-%s-%d", strings.TrimSpace(projectID), number)
}

func normalizePRURL(u string) string {
	return strings.TrimRight(strings.ToLower(strings.TrimSpace(u)), "/")
}

func normalizePRBranch(b string) string {
	b = strings.TrimSpace(b)
	b = strings.TrimPrefix(b, "refs/heads/")
	return b
}

// livePRWorkspaceID returns id when that workspace still exists and is not
// archived; otherwise empty so import can attach a different match.
func (e *Engine) livePRWorkspaceID(id string) string {
	id = strings.TrimSpace(id)
	if id == "" {
		return ""
	}
	w, ok := e.Store.Workspace(id)
	if !ok || w.Archived || w.IsPRChatHost() {
		return ""
	}
	return w.ID
}

// findGitHubPR locates a stored GitHub PR for this project+number (or URL).
func (e *Engine) findGitHubPR(projectID string, number int, url string) (domain.PR, bool) {
	url = normalizePRURL(url)
	for _, p := range e.Store.PRs() {
		if p.Source != domain.SourceGitHub {
			continue
		}
		if url != "" && normalizePRURL(p.URL) == url && e.prProjectID(p) == projectID {
			return p, true
		}
		if number <= 0 || p.Number != number {
			continue
		}
		pid := e.prProjectID(p)
		if pid == projectID {
			return p, true
		}
	}
	return domain.PR{}, false
}

func (e *Engine) projectBaseBranch(projectID string) string {
	if proj, ok := e.Store.Project(projectID); ok {
		return normalizePRBranch(proj.BaseBranch)
	}
	return ""
}

// localBaseFromGitHub maps a GitHub baseRefName to the local compare ref.
// GitHub names are always repo branch names (main, or origin/release when
// that is the branch) — never local tracking refs — so this must not use
// CompareRef, which treats a remote-name prefix as already qualified.
func (e *Engine) localBaseFromGitHub(projectID, githubBase string) string {
	githubBase = strings.TrimPrefix(strings.TrimSpace(githubBase), "refs/heads/")
	if githubBase == "" {
		return ""
	}
	proj, ok := e.Store.Project(projectID)
	if !ok || strings.TrimSpace(proj.RepoPath) == "" {
		return githubBase
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
	projectBase := strings.TrimSpace(proj.BaseBranch)
	if r, short, ok := mgr.ParseRemoteTracking(ctx, projectBase); ok {
		if short == githubBase {
			return projectBase
		}
		// Pin to the project's remote even if the tracking ref is not fetched yet.
		// Falling through to origin would pick a same-named fork branch.
		return r + "/" + githubBase
	}
	if remote := githubDefaultRemote(ctx, mgr); remote != "" {
		ref := remote + "/" + githubBase
		if mgr.CommitExists(ctx, ref) {
			return ref
		}
	}
	return githubBase
}

// githubDefaultRemote prefers origin, else the first configured remote.
func githubDefaultRemote(ctx context.Context, mgr *gitpkg.Manager) string {
	remotes, err := mgr.Remotes(ctx)
	if err != nil || len(remotes) == 0 {
		return ""
	}
	for _, r := range remotes {
		if r == "origin" {
			return r
		}
	}
	return remotes[0]
}

// lookupWorkspaceUpstreamBranch is the remote branch @{upstream} points at
// (origin/feat/oauth → feat/oauth). Empty when the worktree is missing or the
// local branch has no tracking ref.
func (e *Engine) lookupWorkspaceUpstreamBranch(ctx context.Context, ws domain.Workspace) string {
	path := strings.TrimSpace(ws.WorktreePath)
	if path == "" {
		return ""
	}
	if v, ok := e.wsUpstreamCache.Load(ws.ID); ok {
		s, _ := v.(string)
		return s
	}
	if ctx == nil {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
	}
	repo := path
	if proj, ok := e.Store.Project(ws.ProjectID); ok && strings.TrimSpace(proj.RepoPath) != "" {
		repo = proj.RepoPath
	}
	mgr := gitpkg.NewManager(repo, e.worktreeRoot)
	up, ok := mgr.Upstream(ctx, path)
	name := ""
	if ok {
		name = normalizePRBranch(up.Branch)
	}
	e.wsUpstreamCache.Store(ws.ID, name)
	return name
}

// indexWorkspacesByPRHead maps a GitHub head name to a live workspace on that
// branch. Local checkout names win; @{upstream} fills in when the local name
// differs from the remote (git branch -m, or checkout -b local --track origin/head).
// The project base is never a key — a worktree sitting on main is not a PR head.
func (e *Engine) indexWorkspacesByPRHead(ctx context.Context, projectID string) map[string]domain.Workspace {
	idx := make(map[string]domain.Workspace)
	if projectID == "" {
		return idx
	}
	base := e.projectBaseBranch(projectID)
	var live []domain.Workspace
	for _, w := range e.Store.Workspaces() {
		if w.Archived || w.IsMain || w.IsPRChatHost() || w.ProjectID != projectID {
			continue
		}
		local := normalizePRBranch(w.Branch)
		if local == "" || (base != "" && local == base) {
			continue
		}
		live = append(live, w)
		if _, exists := idx[local]; !exists {
			idx[local] = w
		}
	}
	for _, w := range live {
		local := normalizePRBranch(w.Branch)
		up := e.lookupWorkspaceUpstreamBranch(ctx, w)
		if up == "" || up == local || (base != "" && up == base) {
			continue
		}
		if _, exists := idx[up]; !exists {
			idx[up] = w
		}
	}
	return idx
}

func (e *Engine) workspaceMatchingPRBranch(projectID, branch string) (domain.Workspace, bool) {
	branch = normalizePRBranch(branch)
	if projectID == "" || branch == "" {
		return domain.Workspace{}, false
	}
	if base := e.projectBaseBranch(projectID); base != "" && branch == base {
		// Don't attach every PR whose head is the project default (or a
		// stray worktree sitting on main) — that's not "the same branch."
		return domain.Workspace{}, false
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	var needUp []domain.Workspace
	for _, w := range e.Store.Workspaces() {
		if w.Archived || w.IsMain || w.IsPRChatHost() || w.ProjectID != projectID {
			continue
		}
		local := normalizePRBranch(w.Branch)
		if local == "" {
			continue
		}
		if local == branch {
			return w, true
		}
		needUp = append(needUp, w)
	}
	for _, w := range needUp {
		if e.lookupWorkspaceUpstreamBranch(ctx, w) == branch {
			return w, true
		}
	}
	return domain.Workspace{}, false
}

// workspacePRHeads is the local branch plus @{upstream}, excluding the project
// base. Used when a single workspace just became known (allocate, rename).
func (e *Engine) workspacePRHeads(ctx context.Context, ws domain.Workspace) []string {
	if ws.Archived || ws.IsMain || ws.IsPRChatHost() {
		return nil
	}
	base := e.projectBaseBranch(ws.ProjectID)
	local := normalizePRBranch(ws.Branch)
	if local == "" || (base != "" && local == base) {
		return nil
	}
	heads := []string{local}
	if up := e.lookupWorkspaceUpstreamBranch(ctx, ws); up != "" && up != local && (base == "" || up != base) {
		heads = append(heads, up)
	}
	return heads
}

// linkUnlinkedGitHubPRs attaches any GitHub PR that has no live workspace to a
// Recur workspace already checked out on the same head branch (or tracking it
// under a different local name). Import calls this so a workspace that existed
// first (or appeared after the first sync) shows View workspace without a click.
func (e *Engine) linkUnlinkedGitHubPRs() {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	indexes := map[string]map[string]domain.Workspace{}
	for _, p := range e.Store.PRs() {
		if p.Source != domain.SourceGitHub {
			continue
		}
		if prSettled(p) {
			continue
		}
		if e.livePRWorkspaceID(p.WorkspaceID) != "" {
			continue
		}
		pid := e.prProjectID(p)
		idx, ok := indexes[pid]
		if !ok {
			idx = e.indexWorkspacesByPRHead(ctx, pid)
			indexes[pid] = idx
		}
		w, found := idx[normalizePRBranch(p.Branch)]
		if !found {
			continue
		}
		e.Store.UpdatePR(p.ID, func(pr *domain.PR) {
			pr.WorkspaceID = w.ID
			if strings.TrimSpace(pr.ProjectID) == "" {
				pr.ProjectID = pid
			}
		})
		e.archivePRChatHosts(p.ID, w.ID)
	}
}

// linkWorkspaceToMatchingPRs is the reverse of import linking: a workspace
// whose branch just became known (allocate, rename) picks up a GitHub PR
// already sitting on that head — including when the local name differs from
// the remote the worktree tracks.
func (e *Engine) linkWorkspaceToMatchingPRs(ws domain.Workspace) {
	if ws.Archived || ws.IsMain || ws.IsPRChatHost() {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	heads := e.workspacePRHeads(ctx, ws)
	if ws.ProjectID == "" || len(heads) == 0 {
		return
	}
	match := make(map[string]struct{}, len(heads))
	for _, h := range heads {
		match[h] = struct{}{}
	}
	for _, p := range e.Store.PRs() {
		if p.Source != domain.SourceGitHub {
			continue
		}
		if prSettled(p) {
			continue
		}
		if e.prProjectID(p) != ws.ProjectID {
			continue
		}
		if e.livePRWorkspaceID(p.WorkspaceID) != "" {
			continue
		}
		if _, ok := match[normalizePRBranch(p.Branch)]; !ok {
			continue
		}
		e.Store.UpdatePR(p.ID, func(pr *domain.PR) {
			pr.WorkspaceID = ws.ID
			if strings.TrimSpace(pr.ProjectID) == "" {
				pr.ProjectID = ws.ProjectID
			}
		})
		e.publishPRUpdated(p.ID)
		e.archivePRChatHosts(p.ID, ws.ID)
	}
}

func (e *Engine) applyListedGitHubPR(projectID string, row ghListedPR, viewer string, now time.Time) (id string, created bool) {
	return e.applyListedGitHubPRIndexed(projectID, row, viewer, now, nil, nil)
}

func (e *Engine) indexStoredGitHubPRs() map[string]domain.PR {
	out := make(map[string]domain.PR)
	for _, p := range e.Store.PRs() {
		if p.Source != domain.SourceGitHub {
			continue
		}
		pid := e.prProjectID(p)
		if pid == "" {
			continue
		}
		if u := normalizePRURL(p.URL); u != "" {
			out[pid+"\x00u\x00"+u] = p
		}
		if p.Number > 0 {
			out[pid+"\x00n\x00"+strconv.Itoa(p.Number)] = p
		}
	}
	return out
}

func lookupIndexedGitHubPR(idx map[string]domain.PR, projectID string, row ghListedPR) (domain.PR, bool) {
	if idx == nil {
		return domain.PR{}, false
	}
	if u := normalizePRURL(row.URL); u != "" {
		if p, ok := idx[projectID+"\x00u\x00"+u]; ok {
			return p, true
		}
	}
	if row.Number > 0 {
		if p, ok := idx[projectID+"\x00n\x00"+strconv.Itoa(row.Number)]; ok {
			return p, true
		}
	}
	return domain.PR{}, false
}

func (e *Engine) applyListedGitHubPRIndexed(projectID string, row ghListedPR, viewer string, now time.Time, idx map[string]domain.Workspace, stored map[string]domain.PR) (id string, created bool) {
	if row.Number <= 0 || strings.TrimSpace(projectID) == "" {
		return "", false
	}
	state, group := listedStateGroup(row, viewer)
	existing, ok := lookupIndexedGitHubPR(stored, projectID, row)
	if !ok {
		existing, ok = e.findGitHubPR(projectID, row.Number, row.URL)
	}
	wsID := ""
	if ok {
		wsID = e.livePRWorkspaceID(existing.WorkspaceID)
	}
	if wsID == "" && state != "Merged" && state != "Closed" {
		head := normalizePRBranch(row.HeadRefName)
		if idx == nil {
			if w, found := e.workspaceMatchingPRBranch(projectID, head); found {
				wsID = w.ID
			}
		} else if w, found := idx[head]; found {
			wsID = w.ID
		}
	}
	when := compactAgo(parseGitHubTime(row.UpdatedAt), now)
	author := listedActorLogin(row.Author)
	listed, checksOK, hasChecks := listedChecks(row)
	reasons := listedAttentionReasons(row, viewer)
	reviewReq := listedReviewRequested(row, viewer)
	people := mergeParticipants(existing.Participants, listedParticipants(row))
	reviewers := listedReviewers(row)
	if ok {
		keepGroup := existing.Group == prGroupLoop
		wantWS := existing.WorkspaceID
		if wsID != "" {
			wantWS = wsID
		}
		wantTitle := existing.Title
		if row.Title != "" {
			wantTitle = row.Title
		}
		wantURL := existing.URL
		if row.URL != "" {
			wantURL = row.URL
		}
		wantBranch := existing.Branch
		if row.HeadRefName != "" {
			wantBranch = row.HeadRefName
		}
		wantBase := existing.Base
		if row.BaseRefName != "" {
			wantBase = row.BaseRefName
		}
		wantFiles := existing.FileCount
		if row.ChangedFiles > 0 {
			wantFiles = row.ChangedFiles
		}
		wantWhen := existing.When
		if when != "" {
			wantWhen = when
		}
		wantCreated := existing.CreatedAt
		if c := listedCreatedAt(row); c != "" {
			wantCreated = c
		}
		wantUpdated := existing.UpdatedAt
		if u := listedUpdatedAt(row); u != "" {
			wantUpdated = u
		}
		wantState, wantGroup := existing.State, existing.Group
		if !keepGroup {
			wantState, wantGroup = state, group
		} else if state == "Merged" || state == "Closed" {
			wantState = state
			wantGroup = prGroupMerged
		}
		wantBehind := existing.BehindBase
		if existing.UpdatingBase == "" && !e.prHasLocalBehind(wantWS) {
			wantBehind = listedBehind(row)
		}
		wantViewer := existing.ViewerLogin
		if strings.TrimSpace(viewer) != "" {
			wantViewer = viewer
		}
		wantChecks := existing.Checks
		refreshChecks := false
		if hasChecks {
			wantChecks, refreshChecks = reconcileListedChecks(existing.Checks, listed)
		}
		wantHasChecks := hasChecks || len(wantChecks) > 0
		if !refreshChecks && existing.ProjectID == projectID && existing.WorkspaceID == wantWS &&
			existing.Title == wantTitle && existing.URL == wantURL &&
			existing.Branch == wantBranch && existing.Base == wantBase &&
			existing.Draft == row.IsDraft && existing.AuthorLogin == author &&
			existing.ViewerLogin == wantViewer &&
			existing.ReviewDecision == row.ReviewDecision && existing.MergeStateStatus == row.MergeStateStatus &&
			existing.IsCrossRepository == row.IsCrossRepository &&
			existing.Diff.Add == row.Additions && existing.Diff.Del == row.Deletions &&
			existing.FileCount == wantFiles && existing.ChecksOK == checksOK &&
			existing.HasChecks == wantHasChecks && checksEqual(existing.Checks, wantChecks) &&
			existing.BehindBase == wantBehind &&
			existing.When == wantWhen && existing.CreatedAt == wantCreated &&
			existing.UpdatedAt == wantUpdated &&
			existing.State == wantState && existing.Group == wantGroup &&
			existing.ReviewRequested == reviewReq && slices.Equal(existing.AttentionReasons, reasons) &&
			slices.Equal(existing.Participants, people) && slices.Equal(existing.Reviewers, reviewers) {
			return existing.ID, false
		}
		e.Store.UpdatePR(existing.ID, func(p *domain.PR) {
			p.ProjectID = projectID
			p.WorkspaceID = wantWS
			p.Title = wantTitle
			p.URL = wantURL
			p.Branch = wantBranch
			p.Base = wantBase
			p.Draft = row.IsDraft
			p.AuthorLogin = author
			if strings.TrimSpace(viewer) != "" {
				p.ViewerLogin = viewer
			}
			p.ReviewDecision = row.ReviewDecision
			p.ReviewRequested = reviewReq
			p.AttentionReasons = reasons
			p.Participants = people
			p.Reviewers = reviewers
			p.MergeStateStatus = row.MergeStateStatus
			p.IsCrossRepository = row.IsCrossRepository
			p.Diff = domain.DiffStat{Add: row.Additions, Del: row.Deletions}
			p.FileCount = wantFiles
			p.Checks = wantChecks
			p.ChecksOK = checksOK
			p.HasChecks = wantHasChecks
			p.BehindBase = wantBehind
			p.When = wantWhen
			p.CreatedAt = wantCreated
			p.UpdatedAt = wantUpdated
			p.State = wantState
			p.Group = wantGroup
		})
		if refreshChecks {
			e.RefreshPRChecks(existing.ID)
		}
		return existing.ID, false
	}
	if when == "" {
		when = "now"
	}
	pr := domain.PR{
		ID:                githubImportedPRID(projectID, row.Number),
		WorkspaceID:       wsID,
		ProjectID:         projectID,
		Group:             group,
		Number:            row.Number,
		Source:            domain.SourceGitHub,
		URL:               row.URL,
		Draft:             row.IsDraft,
		Branch:            row.HeadRefName,
		Base:              row.BaseRefName,
		Title:             row.Title,
		State:             state,
		Diff:              domain.DiffStat{Add: row.Additions, Del: row.Deletions},
		FileCount:         row.ChangedFiles,
		Checks:            listed,
		ChecksOK:          checksOK,
		HasChecks:         hasChecks,
		When:              when,
		CreatedAt:         listedCreatedAt(row),
		UpdatedAt:         listedUpdatedAt(row),
		AuthorLogin:       author,
		ViewerLogin:       viewer,
		ReviewDecision:    row.ReviewDecision,
		ReviewRequested:   reviewReq,
		AttentionReasons:  reasons,
		Participants:      listedParticipants(row),
		Reviewers:         listedReviewers(row),
		MergeStateStatus:  row.MergeStateStatus,
		IsCrossRepository: row.IsCrossRepository,
		BehindBase:        listedBehind(row),
	}
	if _, err := e.Store.AddPR(pr); err != nil {
		return "", false
	}
	return pr.ID, true
}

// githubPRSyncMinInterval skips a repeat `gh pr list` sweep. Tests may set this
// to 0. Concurrent callers still wait for an in-flight sweep (single-flight).
var githubPRSyncMinInterval = 30 * time.Second

// prHeadCheckoutSpec is stored on prHeadCheckout *before* startAllocate so the
// worktree goroutine cannot miss the PR-head marker.
type prHeadCheckoutSpec struct {
	Number    int
	Branch    string
	PRID      string
	CrossRepo bool
}

// SyncGitHubPRs imports open (and recently merged) GitHub PRs for every git
// project Recur knows, including PRs the user did not open. Recur-created PRs
// are updated in place. Best-effort: missing gh or a per-repo list error skips
// that project rather than failing the dashboard.
func (e *Engine) SyncGitHubPRs() []domain.PR {
	e.syncGitHubPRsMu.Lock()
	if !e.syncGitHubPRsAt.IsZero() && githubPRSyncMinInterval > 0 && time.Since(e.syncGitHubPRsAt) < githubPRSyncMinInterval {
		e.syncGitHubPRsMu.Unlock()
		return e.Store.PRs()
	}
	if wait := e.syncGitHubPRsWait; wait != nil {
		e.syncGitHubPRsMu.Unlock()
		<-wait
		return e.Store.PRs()
	}
	done := make(chan struct{})
	e.syncGitHubPRsWait = done
	e.syncGitHubPRsMu.Unlock()
	defer func() {
		e.syncGitHubPRsMu.Lock()
		e.syncGitHubPRsAt = time.Now()
		e.syncGitHubPRsWait = nil
		close(done)
		e.syncGitHubPRsMu.Unlock()
	}()

	if ghCLIAvailableFn() {
		ctx, cancel := context.WithTimeout(context.Background(), 90*time.Second)
		defer cancel()
		viewer := githubLoginFn(ctx)
		now := time.Now()
		var projWG sync.WaitGroup
		sem := make(chan struct{}, githubPRSyncProjectConcurrency)
		for _, proj := range e.Store.Projects() {
			if proj.ID == domain.FilesProjectID || strings.TrimSpace(proj.RepoPath) == "" {
				continue
			}
			if !gitpkg.IsRepo(proj.RepoPath) {
				continue
			}
			sem <- struct{}{}
			projWG.Go(func() {
				defer func() { <-sem }()
				e.syncGitHubProjectPRs(ctx, proj, viewer, now)
			})
		}
		projWG.Wait()
	}
	// Even without gh: a workspace created (or renamed) onto a PR head after
	// the first import should still attach so the dashboard shows View workspace.
	e.linkUnlinkedGitHubPRs()
	return e.Store.PRs()
}

// githubPRSyncProjectConcurrency bounds projects listed at once. Each project
// runs two `gh pr list` calls, so this is up to 2x gh processes in flight.
const githubPRSyncProjectConcurrency = 8

func (e *Engine) syncGitHubProjectPRs(ctx context.Context, proj domain.Project, viewer string, now time.Time) {
	var openRows, mergedRows []ghListedPR
	var err, mergedErr error
	var listWG sync.WaitGroup
	listWG.Go(func() {
		openRows, err = listGitHubPRsFn(ctx, proj.RepoPath, "open", githubOpenPRLimit)
	})
	listWG.Go(func() {
		mergedRows, mergedErr = listGitHubPRsFn(ctx, proj.RepoPath, "merged", githubMergedPRLimit)
	})
	listWG.Wait()
	if err != nil {
		return
	}
	idx := e.indexWorkspacesByPRHead(ctx, proj.ID)
	stored := e.indexStoredGitHubPRs()
	seen := make(map[int]struct{}, len(openRows)+len(mergedRows))
	for _, row := range slices.Concat(openRows, mergedRows) {
		e.applyListedGitHubPRIndexed(proj.ID, row, viewer, now, idx, stored)
		if row.Number > 0 {
			seen[row.Number] = struct{}{}
		}
	}
	skipReconcile := mergedErr != nil || len(openRows) >= githubOpenPRLimit || len(mergedRows) >= githubMergedPRLimit
	e.reconcileMissingGitHubPRs(ctx, proj.ID, proj.RepoPath, seen, skipReconcile)
	e.publishProjectPRsSynced(proj.ID)
}

// publishProjectPRsSynced streams one project's list rows to the dashboard so
// it updates before the whole sweep finishes.
func (e *Engine) publishProjectPRsSynced(projectID string) {
	var rows []domain.PR
	for _, p := range e.Store.PRs() {
		if e.prProjectID(p) == projectID {
			rows = append(rows, p)
		}
	}
	if len(rows) == 0 {
		return
	}
	e.Bus.Publish(Event{Name: "prs.synced", Data: map[string]any{"projectId": projectID, "prs": rows}})
}

// reconcileMissingGitHubPRs settles GitHub PRs that disappeared from both the
// open and merged lists. skip is true when either list was truncated or the
// merged call failed — we cannot tell missing from unlisted, so rows stay.
// Unseen rows are confirmed with `gh pr view` before settling so a just-merged
// PR that fell off the created-at merged cap is not labelled Closed. Recur-owned
// records are never deleted (review history stays); imports without a live
// workspace are marked Closed/Merged rather than wiped.
func (e *Engine) reconcileMissingGitHubPRs(ctx context.Context, projectID, repoPath string, seen map[int]struct{}, skip bool) {
	if skip || projectID == "" {
		return
	}
	if ctx == nil {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
	}
	for _, p := range e.Store.PRs() {
		if p.Source != domain.SourceGitHub || e.prProjectID(p) != projectID {
			continue
		}
		if p.Number <= 0 {
			continue
		}
		if _, ok := seen[p.Number]; ok {
			continue
		}
		if prSettled(p) {
			continue
		}
		view, err := githubPRViewStateFn(ctx, repoPath, p.Number)
		if err != nil || view.State == "" || view.State == "OPEN" {
			continue
		}
		if view.State == "MERGED" {
			if e.livePRWorkspaceID(p.WorkspaceID) != "" {
				e.markMerged(p.ID, view.UpdatedAt)
				continue
			}
			e.Store.UpdatePR(p.ID, func(pr *domain.PR) {
				pr.State = "Merged"
				pr.Group = prGroupMerged
				stampReconcileUpdatedAt(pr, view.UpdatedAt)
			})
			e.publishPRUpdated(p.ID)
			e.archivePRChatHosts(p.ID, "")
			continue
		}
		e.Store.UpdatePR(p.ID, func(pr *domain.PR) {
			pr.State = "Closed"
			pr.Group = prGroupMerged
			stampReconcileUpdatedAt(pr, view.UpdatedAt)
		})
		e.publishPRUpdated(p.ID)
		e.archivePRChatHosts(p.ID, "")
	}
}

func stampReconcileUpdatedAt(pr *domain.PR, updatedAt string) {
	if u := normalizePRCreatedAt(updatedAt); u != "" {
		pr.UpdatedAt = u
		return
	}
	touchPRUpdated(pr)
}

// checkoutPRHeadName is the local branch AddAtRef may create. Cross-repo heads
// and names that collide with the project/PR base (forks whose head is `main`)
// return empty so allocate uses a unique recur/<slug> instead of `git branch -f main`.
func checkoutPRHeadName(head, prBase, projectBase string, crossRepo bool) string {
	head = normalizePRBranch(head)
	if head == "" || crossRepo {
		return ""
	}
	if b := normalizePRBranch(projectBase); b != "" && strings.EqualFold(head, b) {
		return ""
	}
	if b := normalizePRBranch(prBase); b != "" && strings.EqualFold(head, b) {
		return ""
	}
	return head
}

// workspaceCreatingFromPR returns the in-flight workspace for this PR, if any.
// CreateWorkspaceFromPR stores the spec before startAllocate; allocate keeps
// it until it returns so a second click cannot persist another worktree.
func (e *Engine) workspaceCreatingFromPR(prID string) (domain.Workspace, bool) {
	prID = strings.TrimSpace(prID)
	if prID == "" {
		return domain.Workspace{}, false
	}
	var found domain.Workspace
	ok := false
	e.prHeadCheckout.Range(func(k, v any) bool {
		spec, sok := v.(prHeadCheckoutSpec)
		if !sok || spec.PRID != prID {
			return true
		}
		id, _ := k.(string)
		ws, wok := e.Store.Workspace(id)
		if !wok || ws.Archived {
			return true
		}
		found = ws
		ok = true
		return false
	})
	return found, ok
}

// CreateWorkspaceFromPR allocates a Recur workspace on this PR's head and
// links it. A workspace already on the PR's GitHub head — or tracking that
// head under a different local name — is attached at import
// (see applyListedGitHubPR); this only reuses that workspace so we don't
// create a second worktree if sync hasn't run yet. Otherwise it creates one.
func (e *Engine) CreateWorkspaceFromPR(prID string) (domain.Workspace, error) {
	pr, ok := e.Store.PR(prID)
	if !ok {
		return domain.Workspace{}, fmt.Errorf("PR %q not found", prID)
	}
	if id := strings.TrimSpace(pr.WorkspaceID); id != "" {
		if ws, wok := e.Store.Workspace(id); wok && !ws.Archived && !ws.IsPRChatHost() {
			e.archivePRChatHosts(prID, ws.ID)
			return ws, nil
		}
	}
	projectID := e.prProjectID(pr)
	if projectID == "" {
		return domain.Workspace{}, fmt.Errorf("PR %q has no Recur project", prID)
	}
	if w, found := e.workspaceMatchingPRBranch(projectID, pr.Branch); found {
		e.Store.UpdatePR(prID, func(p *domain.PR) { p.WorkspaceID = w.ID; p.ProjectID = projectID })
		e.publishPRUpdated(prID)
		e.archivePRChatHosts(prID, w.ID)
		return w, nil
	}
	if w, found := e.workspaceCreatingFromPR(prID); found {
		e.archivePRChatHosts(prID, w.ID)
		return w, nil
	}
	if host, found := e.prChatHost(prID); found {
		return e.promotePRChatHost(host, pr)
	}
	name := strings.TrimSpace(pr.Title)
	if name == "" {
		name = fmt.Sprintf("PR #%d", pr.Number)
	}
	base := strings.TrimSpace(pr.Base)
	if base == "" {
		base = e.projectBaseBranch(projectID)
	}
	base = e.localBaseFromGitHub(projectID, base)
	rec, err := e.persistCreatingWorkspace(projectID, name, base)
	if err != nil {
		return domain.Workspace{}, err
	}
	if pr.Source == domain.SourceGitHub && pr.Number > 0 {
		e.prHeadCheckout.Store(rec.ID, prHeadCheckoutSpec{
			Number:    pr.Number,
			Branch:    checkoutPRHeadName(pr.Branch, pr.Base, e.projectBaseBranch(projectID), pr.IsCrossRepository),
			PRID:      prID,
			CrossRepo: pr.IsCrossRepository,
		})
	}
	e.startAllocate(rec)
	return rec, nil
}

// prChatHost is the hidden chat-only workspace for this PR, if any.
func (e *Engine) prChatHost(prID string) (domain.Workspace, bool) {
	prID = strings.TrimSpace(prID)
	if prID == "" {
		return domain.Workspace{}, false
	}
	for _, w := range e.Store.Workspaces() {
		if w.Archived {
			continue
		}
		if strings.TrimSpace(w.HostPRID) == prID {
			return w, true
		}
	}
	return domain.Workspace{}, false
}

// archivePRChatHosts hides leftover chat-host rows for this PR. keepID is a
// live workspace that just got linked (skip it if it still carries HostPRID
// mid-promote). Chats on those hosts are hoisted onto keepID first. Settled
// PRs pass keepID empty. A hoist tombstone covers the busy check, remap, move,
// and archive so a concurrent send cannot land mid-move. Destination live
// turns defer the hoist the same way leftover-host turns do; completion of
// either side retries via maybeArchiveLinkedPRChatHost.
func (e *Engine) archivePRChatHosts(prID, keepID string) {
	prID = strings.TrimSpace(prID)
	if prID == "" {
		return
	}
	keepID = strings.TrimSpace(keepID)
	for _, w := range e.Store.Workspaces() {
		if w.Archived || strings.TrimSpace(w.HostPRID) != prID {
			continue
		}
		if w.ID == keepID {
			continue
		}
		e.archiveOnePRChatHost(w.ID, keepID)
	}
}

func (e *Engine) archiveOnePRChatHost(hostID, keepID string) {
	for {
		release, held := e.holdPRChatHoist(hostID, keepID)
		if !held {
			return
		}
		e.waitChatLifeIdle(hostID)
		if keepID != "" {
			e.waitChatLifeIdle(keepID)
		}
		if !e.prChatHoistStillValid(hostID, keepID) {
			release()
			return
		}
		busy := e.prChatHostBusy(hostID) || (keepID != "" && e.prChatHostBusy(keepID))
		if busy {
			release()
			// Lost-wakeup: a turn may have finished while the tombstone hid
			// maybeArchiveLinkedPRChatHost. Recheck after callbacks are
			// observable; if still busy, dest/source completion retries.
			if e.prChatHostBusy(hostID) || (keepID != "" && e.prChatHostBusy(keepID)) {
				return
			}
			continue
		}
		if keepID != "" {
			if err := e.hoistPRChatHost(hostID, keepID); err != nil {
				log.Printf("recur: hoist PR chat host %s → %s: %v", hostID, keepID, err)
				release()
				return
			}
		}
		updated, ok := e.Store.UpdateWorkspace(hostID, func(ws *domain.Workspace) {
			ws.Archived = true
		})
		release()
		if ok {
			e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
		}
		return
	}
}

// holdPRChatHoist exclusively reserves ids for a hoist. It waits for another
// hoist, and refuses if Remove/Delete already reserved any id (chats must not
// move onto a workspace being torn down). ok is false when the reservation
// failed; unlock is still safe to call.
func (e *Engine) holdPRChatHoist(ids ...string) (unlock func(), ok bool) {
	nop := func() {}
	seen := map[string]bool{}
	var held []string
	for _, id := range ids {
		id = strings.TrimSpace(id)
		if id == "" || seen[id] {
			continue
		}
		seen[id] = true
		held = append(held, id)
	}
	if len(held) == 0 {
		return nop, false
	}
	slices.Sort(held)
	e.hoistMu.Lock()
	for {
		hoistBusy := false
		deleting := false
		for _, id := range held {
			if _, busy := e.hoisting.Load(id); busy {
				hoistBusy = true
				break
			}
			if _, busy := e.deleting.Load(id); busy {
				deleting = true
			}
		}
		if hoistBusy {
			e.hoistMu.Unlock()
			time.Sleep(time.Millisecond)
			e.hoistMu.Lock()
			continue
		}
		if deleting {
			e.hoistMu.Unlock()
			return nop, false
		}
		for _, id := range held {
			e.hoisting.Store(id, struct{}{})
		}
		e.hoistMu.Unlock()
		return func() {
			e.hoistMu.Lock()
			defer e.hoistMu.Unlock()
			for _, id := range held {
				e.hoisting.Delete(id)
			}
		}, true
	}
}

// hoistPRChatHost moves chat sessions and agent tabs from a leftover PR host
// onto the live worktree so Create Workspace does not orphan PR-detail chats.
func (e *Engine) hoistPRChatHost(fromID, toID string) error {
	fromID = strings.TrimSpace(fromID)
	toID = strings.TrimSpace(toID)
	if fromID == "" || toID == "" || fromID == toID {
		return nil
	}
	remap := e.hoistChatCodeRemaps(fromID, toID)
	movedCodes := e.hoistMovedDestCodes(fromID, remap)
	focus := ""
	e.chatFocusMu.Lock()
	if e.chatFocus.workspaceID == fromID {
		focus = e.chatFocus.code
	}
	e.chatFocusMu.Unlock()
	if next := remap[focus]; next != "" {
		focus = next
	}
	if err := e.hoistWorkspaceArtifacts(fromID, toID, remap); err != nil {
		return err
	}
	if err := e.Store.ReassignWorkspaceSessions(fromID, toID, remap); err != nil {
		return err
	}
	e.noteChatHoistAliases(fromID, toID, remap)
	e.hoistLastChatSurfaces(fromID, toID, remap)
	e.Store.MoveWorkspaceAgents(fromID, toID, remap)
	movedFindings := e.Store.ReassignWorkspaceFindings(fromID, toID)
	from, ok := e.Store.Workspace(fromID)
	if ok {
		e.Store.UpdateWorkspace(toID, func(w *domain.Workspace) {
			hoistPRChatHostPlan(w, from, remap)
			if len(from.ChatTabOrder) == 0 {
				return
			}
			order := hoistRemapCodes(from.ChatTabOrder, remap)
			if len(w.ChatTabOrder) == 0 {
				w.ChatTabOrder = order
				return
			}
			have := make(map[string]bool, len(w.ChatTabOrder))
			for _, c := range w.ChatTabOrder {
				have[c] = true
			}
			for _, c := range order {
				if !have[c] {
					w.ChatTabOrder = append(w.ChatTabOrder, c)
					have[c] = true
				}
			}
		})
	}
	if focus != "" {
		e.SetWorkspaceChatFocus(toID, focus)
	}
	e.SyncChatInbox()
	if dest, ok := e.Store.Workspace(toID); ok {
		e.publishLoopUpdated(toID, dest.State, dest.Phase, dest.Iteration)
		data := map[string]any{
			"workspaceId":     toID,
			"fromWorkspaceId": fromID,
			"codeRemap":       remap,
			"movedCodes":      movedCodes,
			"chatsChanged":    true,
		}
		if focus != "" {
			data["focusChatCode"] = focus
		}
		e.Bus.Publish(Event{Name: "workspace.updated", Data: data})
	}
	if movedFindings > 0 {
		e.Bus.Publish(Event{Name: "findings.updated", Data: map[string]any{"workspaceId": toID}})
	}
	return nil
}

// hoistMovedDestCodes lists dest tab codes that receive a source session.
// Call before ReassignWorkspaceSessions — source rows are gone after the move.
// remap is collision-only; identity-moved codes still belong in this set.
func (e *Engine) hoistMovedDestCodes(fromID string, remap map[string]string) []string {
	if e == nil || fromID == "" {
		return nil
	}
	seen := map[string]bool{}
	var out []string
	note := func(code string) {
		code = strings.TrimSpace(code)
		if code == "" {
			return
		}
		dest := code
		if next := remap[code]; next != "" {
			dest = next
		}
		if seen[dest] {
			return
		}
		seen[dest] = true
		out = append(out, dest)
	}
	for _, a := range e.Store.Agents(fromID) {
		note(a.Code)
	}
	for _, s := range e.Store.SessionsForLoop(fromID) {
		var cfg agentConfig
		_ = json.Unmarshal([]byte(s.ConfigJSON), &cfg)
		note(cfg.Code)
	}
	for src := range remap {
		note(src)
	}
	return out
}

// noteChatHoistAliases records host→dest tab keys so a send that still names
// the archived host follows after the hoist event (or the next RPC). Call only
// after fallible migration so a failed hoist does not steal routing.
func (e *Engine) noteChatHoistAliases(fromID, toID string, remap map[string]string) {
	if e == nil || fromID == "" || toID == "" || fromID == toID {
		return
	}
	seen := map[string]bool{}
	note := func(code string) {
		code = strings.TrimSpace(code)
		if code == "" || seen[code] {
			return
		}
		seen[code] = true
		dest := code
		if next := remap[code]; next != "" {
			dest = next
		}
		e.chatHoistAlias.Store(chatTurnKey(fromID, code), chatTurnKey(toID, dest))
	}
	for _, a := range e.Store.Agents(fromID) {
		note(a.Code)
	}
	for _, s := range e.Store.SessionsForLoop(fromID) {
		var cfg agentConfig
		_ = json.Unmarshal([]byte(s.ConfigJSON), &cfg)
		note(cfg.Code)
	}
	for src := range remap {
		note(src)
	}
}

// resolveHoistedChat follows a leftover-host send onto the live worktree.
func (e *Engine) resolveHoistedChat(workspaceID, code string) (string, string) {
	if e == nil {
		return workspaceID, code
	}
	v, ok := e.chatHoistAlias.Load(chatTurnKey(workspaceID, code))
	if !ok {
		return workspaceID, code
	}
	dest, _ := v.(string)
	toID, toCode, ok := strings.Cut(dest, "\x00")
	if !ok || toID == "" || toCode == "" {
		return workspaceID, code
	}
	if ws, exists := e.Store.Workspace(toID); !exists || ws.Archived {
		return workspaceID, code
	}
	return toID, toCode
}

// hoistLastChatSurfaces rekeys Retry's looking-at cache onto dest so a host
// Review/Ask turn stays a GitHub PR subject after the chat moves.
func (e *Engine) hoistLastChatSurfaces(fromID, toID string, remap map[string]string) {
	if e == nil || fromID == "" || toID == "" || fromID == toID {
		return
	}
	prefix := fromID + "\x00"
	e.lastChatSurfaces.Range(func(k, v any) bool {
		key, ok := k.(string)
		if !ok || !strings.HasPrefix(key, prefix) {
			return true
		}
		code := strings.TrimPrefix(key, prefix)
		if next := remap[code]; next != "" {
			code = next
		}
		e.lastChatSurfaces.Store(toID+"\x00"+code, v)
		e.lastChatSurfaces.Delete(k)
		return true
	})
}

// hoistPRChatHostPlan copies host Plan/DoD onto dest when dest has neither.
// simplify: dest already-filed plan wins; host plan stays on the archived row.
func hoistPRChatHostPlan(dest *domain.Workspace, from domain.Workspace, remap map[string]string) {
	if dest.Plan != nil || strings.TrimSpace(dest.DefinitionOfDone) != "" {
		return
	}
	if from.Plan != nil {
		p := *from.Plan
		p.Steps = append([]domain.PlanStep(nil), from.Plan.Steps...)
		p.Attachments = append([]domain.PlanAttachment(nil), from.Plan.Attachments...)
		for i := range p.Attachments {
			p.Attachments[i].Path = rewriteHoistArtifactPath(p.Attachments[i].Path, from.ID, dest.ID)
		}
		if next := remap[p.ChatCode]; next != "" {
			p.ChatCode = next
		}
		dest.Plan = &p
	}
	dest.DefinitionOfDone = from.DefinitionOfDone
}

func rewriteHoistArtifactPath(path, fromID, toID string) string {
	fromID, toID = strings.TrimSpace(fromID), strings.TrimSpace(toID)
	if path == "" || fromID == "" || toID == "" || fromID == toID {
		return path
	}
	old := "workspaces/" + fromID + "/artifacts/"
	if !strings.Contains(path, old) {
		return path
	}
	return strings.Replace(path, old, "workspaces/"+toID+"/artifacts/", 1)
}

func hoistRemapCodes(order []string, remap map[string]string) []string {
	if len(order) == 0 {
		return nil
	}
	out := make([]string, 0, len(order))
	seen := make(map[string]bool, len(order))
	for _, c := range order {
		if next := remap[c]; next != "" {
			c = next
		}
		if c == "" || seen[c] {
			continue
		}
		seen[c] = true
		out = append(out, c)
	}
	return out
}

// hoistChatCodeRemaps assigns a free PA* code to each source tab whose dest
// already has a transcript (or a provider SessionID). Source codes are reserved
// first so a colliding PA is not remapped onto the source's existing PA2.
// Placeholder dest PA tabs are not remapped — MoveWorkspaceAgents merges onto them.
func (e *Engine) hoistChatCodeRemaps(fromID, toID string) map[string]string {
	used := map[string]bool{}
	occupied := map[string]bool{}
	for _, s := range e.Store.SessionsForLoop(toID) {
		var cfg agentConfig
		_ = json.Unmarshal([]byte(s.ConfigJSON), &cfg)
		if cfg.Code == "" {
			continue
		}
		used[cfg.Code] = true
		if e.Store.SessionHasMessages(s.ID) {
			occupied[cfg.Code] = true
		}
	}
	for _, a := range e.Store.Agents(toID) {
		used[a.Code] = true
		if strings.TrimSpace(a.SessionID) != "" {
			occupied[a.Code] = true
		}
	}
	for _, s := range e.Store.SessionsForLoop(fromID) {
		var cfg agentConfig
		_ = json.Unmarshal([]byte(s.ConfigJSON), &cfg)
		if cfg.Code != "" {
			used[cfg.Code] = true
		}
	}
	for _, a := range e.Store.Agents(fromID) {
		if a.Code != "" {
			used[a.Code] = true
		}
	}
	max := 0
	for code := range used {
		if n := pairChatCodeNum(code); n > max {
			max = n
		}
	}
	remap := map[string]string{}
	for _, s := range e.Store.SessionsForLoop(fromID) {
		var cfg agentConfig
		_ = json.Unmarshal([]byte(s.ConfigJSON), &cfg)
		if cfg.Code == "" || !occupied[cfg.Code] || remap[cfg.Code] != "" || !isPairChatCode(cfg.Code) {
			continue
		}
		max++
		next := pairChatCodeFromNum(max)
		for used[next] {
			max++
			next = pairChatCodeFromNum(max)
		}
		remap[cfg.Code] = next
		used[next] = true
	}
	return remap
}

// prChatHoistStillValid is the post-reservation check: host is still a live
// leftover for this PR, and keepID is still that PR's live workspace (or
// empty because the PR settled with no dest). Linking is check-then-update
// elsewhere, so keepID can go stale between archivePRChatHosts and here.
func (e *Engine) prChatHoistStillValid(hostID, keepID string) bool {
	host, ok := e.Store.Workspace(hostID)
	if !ok || host.Archived || strings.TrimSpace(host.HostPRID) == "" || host.ID == keepID {
		return false
	}
	pr, ok := e.Store.PR(strings.TrimSpace(host.HostPRID))
	if !ok {
		return false
	}
	liveKeep := e.livePRWorkspaceID(pr.WorkspaceID)
	if keepID != "" {
		return liveKeep == keepID
	}
	return liveKeep == "" && prSettled(pr)
}

// prChatHostBusy is true while a leftover host still has a live turn, a
// lockChatLife holder, a parked or answer-resuming ask_user, or a
// switch_mode remount in flight.
func (e *Engine) prChatHostBusy(id string) bool {
	if e == nil || id == "" {
		return false
	}
	if e.workspaceHasLiveChat(id) {
		return true
	}
	if e.askUsers != nil && e.askUsers.workspaceBusy(id) {
		return true
	}
	e.continueGenMu.Lock()
	for _, c := range e.liveContinues {
		if c.workspaceID == id {
			e.continueGenMu.Unlock()
			return true
		}
	}
	e.continueGenMu.Unlock()
	return e.chatLifeBusy(id)
}

// maybeArchiveLinkedPRChatHost hoists a leftover host onto its linked worktree
// once the host or destination is idle (unregisterChatTurn / lockChatLife
// unlock). Settled PRs have no destination — keep is empty and the host is
// archived in place. An open PR with only a chat host must keep that host.
func (e *Engine) maybeArchiveLinkedPRChatHost(workspaceID string) {
	workspaceID = strings.TrimSpace(workspaceID)
	if e == nil || workspaceID == "" {
		return
	}
	if _, busy := e.hoisting.Load(workspaceID); busy {
		return
	}
	w, ok := e.Store.Workspace(workspaceID)
	if ok && w.IsPRChatHost() {
		pr, ok := e.Store.PR(strings.TrimSpace(w.HostPRID))
		if !ok {
			return
		}
		keep := e.livePRWorkspaceID(pr.WorkspaceID)
		if keep == workspaceID {
			return
		}
		if keep == "" && !prSettled(pr) {
			return
		}
		e.archivePRChatHosts(w.HostPRID, keep)
		return
	}
	pr, ok := e.Store.PRByWorkspace(workspaceID)
	if !ok {
		return
	}
	keep := e.livePRWorkspaceID(pr.WorkspaceID)
	if keep == "" && !prSettled(pr) {
		return
	}
	e.archivePRChatHosts(pr.ID, keep)
}

// EnsurePRChatHost returns a workspace the PR-detail agent dock can bind to.
// A live linked workspace wins; otherwise a hidden host is created (or reused)
// without setting PR.WorkspaceID. The host is later promoted in place by
// CreateWorkspaceFromPR so chat tabs survive.
func (e *Engine) EnsurePRChatHost(prID string) (domain.Workspace, error) {
	pr, ok := e.Store.PR(prID)
	if !ok {
		return domain.Workspace{}, fmt.Errorf("PR %q not found", prID)
	}
	if id := e.livePRWorkspaceID(pr.WorkspaceID); id != "" {
		if ws, wok := e.Store.Workspace(id); wok {
			e.archivePRChatHosts(prID, ws.ID)
			if host, found := e.prChatHost(prID); found {
				return host, nil
			}
			return ws, nil
		}
	}
	if prSettled(pr) {
		return domain.Workspace{}, fmt.Errorf("PR %q is %s — open an open pull request to chat", prID, pr.State)
	}
	projectID := e.prProjectID(pr)
	if projectID == "" {
		return domain.Workspace{}, fmt.Errorf("PR %q has no Recur project", prID)
	}
	if w, found := e.workspaceCreatingFromPR(prID); found {
		return w, nil
	}
	if w, found := e.workspaceMatchingPRBranch(projectID, pr.Branch); found {
		e.Store.UpdatePR(prID, func(p *domain.PR) { p.WorkspaceID = w.ID; p.ProjectID = projectID })
		e.publishPRUpdated(prID)
		e.archivePRChatHosts(prID, w.ID)
		return w, nil
	}
	e.idMu.Lock()
	if host, found := e.prChatHost(prID); found {
		e.idMu.Unlock()
		return host, nil
	}
	proj, ok := e.Store.Project(projectID)
	if !ok {
		e.idMu.Unlock()
		return domain.Workspace{}, fmt.Errorf("project %q not found", projectID)
	}
	root := strings.TrimSpace(proj.RepoPath)
	if root == "" {
		e.idMu.Unlock()
		return domain.Workspace{}, fmt.Errorf("project %q has no repo path", projectID)
	}
	if !gitpkg.IsRepo(root) {
		e.idMu.Unlock()
		return domain.Workspace{}, fmt.Errorf("project %q is not a git repository — PR chat needs a git checkout", projectID)
	}
	name := strings.TrimSpace(pr.Title)
	if name == "" {
		name = fmt.Sprintf("PR #%d", pr.Number)
	}
	base := strings.TrimSpace(pr.Base)
	if base == "" {
		base = proj.BaseBranch
	}
	base = e.localBaseFromGitHub(projectID, base)
	id := e.mintWorkspaceID(projectID, e.gitManager(root))
	rec := domain.Workspace{
		ID:           id,
		ProjectID:    projectID,
		Type:         domain.WorkspaceSolo,
		Group:        domain.GroupActive,
		Goal:         name,
		Short:        truncate(name, 48),
		StatusLabel:  "Review",
		Phase:        "editing",
		HostPRID:     prID,
		WorktreePath: root,
		BaseBranch:   base,
		Last:         "pr chat",
		ElapsedLabel: "just now",
	}
	saved, err := e.Store.AddWorkspace(rec)
	e.idMu.Unlock()
	if err != nil {
		return domain.Workspace{}, err
	}
	e.ensurePairAgent(saved.ID, pairAgentCode, "")
	e.RememberWorkspaceChatSelection(saved.ID, pairAgentCode, "", nil, domain.ChatModeReview, nil)
	e.publishLoopUpdated(saved.ID, saved.State, saved.Phase, saved.Iteration)
	return saved, nil
}

// promotePRChatHost turns a chat-host row into a real PR worktree in place so
// sessions and agent tabs keep the same workspace ID. HostPRID stays set until
// allocate writes the new WorktreePath — tearing down the host must never
// remove the project's checkout. PR.WorkspaceID is stamped only after the
// worktree lands (allocateWorkspaceWorktree), matching persistCreatingWorkspace.
func (e *Engine) promotePRChatHost(host domain.Workspace, pr domain.PR) (domain.Workspace, error) {
	prID := strings.TrimSpace(pr.ID)
	if prID == "" {
		prID = strings.TrimSpace(host.HostPRID)
	}
	projectID := e.prProjectID(pr)
	if projectID == "" {
		projectID = host.ProjectID
	}
	if updated, ok := e.Store.UpdateWorkspace(host.ID, func(w *domain.Workspace) {
		w.Phase = phaseCreating
		w.StatusLabel = statusCreating
		w.LastError = ""
		w.Last = "creating"
	}); ok {
		host = updated
		e.publishLoopUpdated(host.ID, host.State, host.Phase, host.Iteration)
	}
	if pr.Source == domain.SourceGitHub && pr.Number > 0 {
		e.prHeadCheckout.Store(host.ID, prHeadCheckoutSpec{
			Number:    pr.Number,
			Branch:    checkoutPRHeadName(pr.Branch, pr.Base, e.projectBaseBranch(projectID), pr.IsCrossRepository),
			PRID:      prID,
			CrossRepo: pr.IsCrossRepository,
		})
	}
	e.startAllocate(host)
	return host, nil
}

const prDiffMinInterval = 60 * time.Second

// ScheduleRefreshPRDiff hydrates Files from `gh pr diff` in the background
// (same throttle as conversation snapshots) so GetPR's 15s poll cannot block
// on a 20s diff. Failures still stamp prDiffLast so they are not retried
// every poll.
func (e *Engine) ScheduleRefreshPRDiff(prID string) {
	pr, ok := e.Store.PR(prID)
	if !ok || pr.Source != domain.SourceGitHub || pr.Number <= 0 {
		return
	}
	if len(pr.Files) > 0 {
		return
	}
	if t, ok := e.prDiffLast.Load(prID); ok {
		if time.Since(t.(time.Time)) < prDiffMinInterval {
			return
		}
	}
	if _, loaded := e.prDiffBusy.LoadOrStore(prID, struct{}{}); loaded {
		return
	}
	go func() {
		defer e.prDiffBusy.Delete(prID)
		e.RefreshPRDiff(prID)
		e.prDiffLast.Store(prID, time.Now())
	}()
}

// RefreshPRDiff fills Files from `gh pr diff` when a GitHub PR has no stored
// diff (imported PRs, no worktree yet). Fetch errors leave Files alone.
func (e *Engine) RefreshPRDiff(prID string) {
	pr, ok := e.Store.PR(prID)
	if !ok || pr.Source != domain.SourceGitHub || pr.Number <= 0 {
		return
	}
	if len(pr.Files) > 0 {
		return
	}
	repoPath := e.prRepoPath(pr)
	if repoPath == "" {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	out, err := runGH(ctx, repoPath, "pr", "diff", fmt.Sprintf("%d", pr.Number))
	if err != nil {
		return
	}
	files := gitpkg.ParseUnifiedDiffPlain(out)
	if len(files) == 0 {
		return
	}
	add, del := 0, 0
	for _, f := range files {
		add += f.Diff.Add
		del += f.Diff.Del
	}
	e.Store.UpdatePR(prID, func(p *domain.PR) {
		p.Files = files
		p.FileCount = len(files)
		p.Diff = domain.DiffStat{Add: add, Del: del}
	})
	e.publishPRUpdated(prID)
}
