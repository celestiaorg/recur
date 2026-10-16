package engine

import (
	"context"
	"errors"
	"fmt"
	"html"
	"strings"
	"time"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

// MergeWorkspace lands a hand-driven workspace onto its merge target.
// An open GitHub PR is refused — merge from the PR page. GitHub-PR
// projects refuse a remote-tracking base when GitHub requires a pull
// request for that branch. If the base has a remote-tracking counterpart
// (including a local "main" that CompareRef upgrades), the land is
// pushed; a purely local base stays on-disk only. Loops still merge
// from their PR. method is "merge"/"squash"/"rebase"; empty =
// project/settings prefs (D53). Idempotent once the workspace is already merged.
func (e *Engine) MergeWorkspace(workspaceID string, method domain.MergeMethod) (domain.Workspace, error) {
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return domain.Workspace{}, fmt.Errorf("workspace %q not found", workspaceID)
	}
	if ws.IsMain || ws.IsPRChatHost() {
		return domain.Workspace{}, fmt.Errorf("the main working copy can't merge — its checkout is already the project")
	}
	if landLockedByRun(ws) {
		return domain.Workspace{}, fmt.Errorf("a loop is running — pause it before merging from the workspace")
	}
	if ws.State == domain.StateMerged {
		return ws, nil
	}
	proj, ok := e.Store.Project(ws.ProjectID)
	if !ok || proj.RepoPath == "" {
		return domain.Workspace{}, fmt.Errorf("project for workspace %q not found", workspaceID)
	}

	if method == "" {
		method = domain.EffectiveMergeMethod(proj, e.settings())
	} else {
		method = domain.NormalizeMergeMethod(string(method))
	}

	if existing, ok := e.Store.PRByWorkspace(workspaceID); ok &&
		existing.Source == domain.SourceGitHub && existing.State != "Merged" && existing.State != "Closed" {
		return ws, fmt.Errorf("this workspace has a GitHub PR — merge from the PR")
	}

	base := ws.BaseBranch
	if base == "" {
		base = proj.BaseBranch
	}
	if base == "" {
		base = "main"
	}

	branch := e.syncWorkspaceBranch(workspaceID)
	if branch == "" {
		branch = ws.Branch
	}
	if refreshed, rok := e.Store.Workspace(workspaceID); rok {
		ws = refreshed
	}

	mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	if risk := gitpkg.InspectDiscardRisk(ctx, ws.WorktreePath); risk.DirtyCount > 0 || risk.Unmerged {
		return domain.Workspace{}, fmt.Errorf("commit or discard uncommitted changes first — they would not be included in the merge")
	}

	if behind, berr := mgr.CommitsBehind(ctx, ws.WorktreePath, base); berr == nil && behind > 0 {
		return domain.Workspace{}, fmt.Errorf("this branch is %d commits behind %s — update with base first", behind, base)
	}

	// Match WorkspaceLandState: a local name (main) upgrades to origin/main
	// when that tracking ref exists, so the Merge tooltip's "(push)" is real.
	compare := mgr.CompareRef(ctx, base)
	remote, short, isRemote := mgr.ParseRemoteTracking(ctx, compare)
	if isRemote {
		if short == "" {
			short = mgr.ShortBranchName(ctx, compare)
		}
		if proj.PROnGitHub && e.githubRemoteRequiresPR(ctx, proj, remote, short) {
			return ws, fmt.Errorf("%s/%s requires a GitHub pull request — Open PR instead of merging here", remote, short)
		}
	}

	diff, err := mgr.DiffPR(ctx, ws.WorktreePath, base)
	if err != nil {
		return domain.Workspace{}, fmt.Errorf("compute diff: %w", err)
	}
	commits, _ := mgr.LogCommits(ctx, ws.WorktreePath, base)
	if len(diff.Files) == 0 && len(commits) == 0 {
		return domain.Workspace{}, fmt.Errorf("nothing to merge — commit changes in this workspace first")
	}

	sq := domain.EffectiveSquashCommitMessage(proj, e.settings())
	msg := workspaceMergeCommitMessage(ws, base, method, commits, sq)
	if _, err := mgr.Merge(ctx, branch, base, msg, string(method)); err != nil {
		e.emitWorkspaceEvent(workspaceID, "AR", "You",
			"Local merge failed: "+html.EscapeString(err.Error()))
		var dirty gitpkg.DirtyBaseError
		if errors.As(err, &dirty) {
			return ws, err
		}
		return ws, fmt.Errorf("merge into %s: %w", base, err)
	}

	// Persist merged before pushing so a failed push cannot leave the workspace
	// looking unmerged while the base already contains the land (squash/rebase
	// retries would then fail with "nothing to commit").
	if existing, ok := e.Store.PRByWorkspace(workspaceID); ok && existing.State != "Merged" {
		e.markMerged(existing.ID, "")
	} else {
		e.setWorkspaceState(workspaceID, domain.StateMerged, "merged")
	}

	if isRemote {
		if err := mgr.PushBranch(ctx, remote, short); err != nil {
			e.emitWorkspaceEvent(workspaceID, "AR", "You",
				"Landed on local <code>"+html.EscapeString(short)+"</code> but could not push to <code>"+
					html.EscapeString(remote)+"</code>: "+html.EscapeString(err.Error()))
			if refreshed, rok := e.Store.Workspace(workspaceID); rok {
				ws = refreshed
			}
			return ws, fmt.Errorf("push %s to %s: %w", short, remote, err)
		}
	}

	landed := mgr.ShortBranchName(ctx, base)
	if landed == "" {
		landed = base
	}
	note := fmt.Sprintf("%s into <code>%s</code> on disk — no PR.",
		html.EscapeString(domain.MergeMethodLabel(method)), html.EscapeString(landed))
	if isRemote {
		note = fmt.Sprintf("%s to <code>%s/%s</code>.",
			html.EscapeString(domain.MergeMethodLabel(method)), html.EscapeString(remote), html.EscapeString(short))
	}
	e.emitWorkspaceEvent(workspaceID, "AR", "You", note)

	ws, ok = e.Store.Workspace(workspaceID)
	if !ok {
		return domain.Workspace{}, fmt.Errorf("workspace %q not found after merge", workspaceID)
	}
	return ws, nil
}

// workspaceMergeCommitMessage is the on-disk commit message for a no-PR local
// land. Squash subjects are the workspace name (plus commit details when the
// squash message setting asks for them). Merge commits keep a Merge prefix.
func workspaceMergeCommitMessage(ws domain.Workspace, base string, method domain.MergeMethod, commits []domain.Commit, squash domain.SquashCommitMessage) string {
	title := singleLine(ws.Goal)
	if title == "" || title == defaultWorkspaceTitle {
		title = "Workspace changes"
	}
	if method == domain.MergeMethodSquash {
		return formatSquashCommitMessage(title, 0, false, commits, "", squash).Combined()
	}
	return fmt.Sprintf("Merge %s into %s", title, base)
}

// WorkspaceLandState reports whether a hand-driven workspace can merge, or
// should Commit / Update first. The main working copy uses the same dirty flag
// plus ahead/behind versus its tracking remote (no merge base). Best-effort
// zeros when the worktree is missing.
func (e *Engine) WorkspaceLandState(workspaceID string) domain.WorkspaceLandState {
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok || ws.WorktreePath == "" {
		return domain.WorkspaceLandState{}
	}
	if ws.IsMain {
		return e.mainWorkingCopyLandState(ws)
	}
	base := strings.TrimSpace(ws.BaseBranch)
	if base == "" {
		if p, pok := e.Store.Project(ws.ProjectID); pok {
			base = strings.TrimSpace(p.BaseBranch)
		}
	}
	st := domain.WorkspaceLandState{Base: base, BehindBase: ws.BehindBase}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	risk := gitpkg.InspectDiscardRisk(ctx, ws.WorktreePath)
	st.Unmerged = risk.Unmerged
	st.Dirty = risk.DirtyCount > 0 && !risk.Unmerged
	var mgr *gitpkg.Manager
	var requireRemote, requireBranch string
	var checkPR bool
	if proj, pok := e.Store.Project(ws.ProjectID); pok && proj.RepoPath != "" && base != "" {
		mgr = gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
		compare := mgr.CompareRef(ctx, base)
		if remote, short, ok := mgr.ParseRemoteTracking(ctx, compare); ok {
			st.Remote = remote
			if short == "" {
				short = mgr.ShortBranchName(ctx, compare)
			}
			checkPR = proj.PROnGitHub
			requireRemote, requireBranch = remote, short
		}
		if _, n, err := mgr.BehindBase(ctx, ws.WorktreePath, base, false); err == nil {
			st.BehindBase = n
		}
		if n, err := mgr.CommitsAhead(ctx, ws.WorktreePath, compare); err == nil {
			st.AheadBase = n
		}
	}
	e.attachOpenPRLand(ctx, &st, ws.ID, ws.WorktreePath, mgr)
	if checkPR {
		if proj, pok := e.Store.Project(ws.ProjectID); pok {
			st.RequirePR = e.githubRemoteRequiresPR(ctx, proj, requireRemote, requireBranch)
		}
	}
	return st
}

// attachOpenPRLand stamps an open (non-merged) PR onto land state so the
// titlebar can switch Open PR → View PR / Pull updates / Push updates.
func (e *Engine) attachOpenPRLand(ctx context.Context, st *domain.WorkspaceLandState, workspaceID, worktree string, mgr *gitpkg.Manager) {
	pr, ok := e.Store.PRByWorkspace(workspaceID)
	if !ok || pr.State == "Merged" || pr.Group == "merged" {
		return
	}
	st.PRID = pr.ID
	st.PRNumber = pr.Number
	st.PRSource = string(pr.Source)
	st.PRDraft = pr.Draft
	if pr.Source != domain.SourceGitHub || worktree == "" || mgr == nil {
		return
	}
	ref, _, _ := githubPRTrackingRef(pr)
	if ref == "" {
		return
	}
	if n, err := mgr.CommitsAhead(ctx, worktree, ref); err == nil {
		st.AheadRemote = n
	} else if st.AheadBase > 0 {
		// The tracking ref may not exist yet (offline, never fetched). Treat
		// unique local commits as unpushed so Push updates still shows.
		st.AheadRemote = st.AheadBase
	}
	if n, err := mgr.CommitsBehind(ctx, worktree, ref); err == nil {
		st.BehindRemote = n
	}
}

// githubPRTrackingRef is origin/<short> for a frozen GitHub PR.Branch
// (recur/foo, not the local branch of the same name). A fork PR's head is not
// on origin, so it tracks refs/recur/pr/N with empty remote and short.
func githubPRTrackingRef(pr domain.PR) (ref, remote, short string) {
	if pr.IsCrossRepository {
		return gitpkg.PRHeadRef(pr.Number), "", ""
	}
	short = strings.TrimSpace(pr.Branch)
	short = strings.TrimPrefix(short, "origin/")
	if short == "" {
		return "", "", ""
	}
	return "origin/" + short, "origin", short
}

// fetchGitHubPRHead refreshes githubPRTrackingRef(pr) from GitHub.
func fetchGitHubPRHead(ctx context.Context, mgr *gitpkg.Manager, pr domain.PR) {
	if pr.IsCrossRepository {
		_, _ = mgr.FetchPRHead(ctx, pr.Number)
		return
	}
	if _, remote, short := githubPRTrackingRef(pr); short != "" {
		_ = mgr.FetchRemoteBranch(ctx, remote, short)
	}
}

func (e *Engine) mainWorkingCopyLandState(ws domain.Workspace) domain.WorkspaceLandState {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	risk := gitpkg.InspectDiscardRisk(ctx, ws.WorktreePath)
	st := domain.WorkspaceLandState{
		Unmerged: risk.Unmerged,
		Dirty:    risk.DirtyCount > 0 && !risk.Unmerged,
	}
	if !gitpkg.IsRepo(ws.WorktreePath) {
		return st
	}
	mgr := gitpkg.NewManager(ws.WorktreePath, e.worktreeRoot)
	up, ok := mgr.Upstream(ctx, ws.WorktreePath)
	if !ok {
		return st
	}
	st.Base = up.Ref
	st.Remote = up.Remote
	if n, err := mgr.CommitsBehind(ctx, ws.WorktreePath, up.Ref); err == nil {
		st.BehindBase = n
	}
	if n, err := mgr.CommitsAhead(ctx, ws.WorktreePath, up.Ref); err == nil {
		st.AheadBase = n
	}
	return st
}

// UpdateWorkspaceWithBase brings a hand-driven workspace up to date with its
// merge base (D54 git-direct). Clean path: merge or rebase in the worktree, no
// LLM. Conflict path: leave markers and return Status=conflict so the UI
// invokes /update. method is "merge" or "rebase"; empty uses project/settings
// UpdateBaseMethod prefs. A clean update on a workspace with an open GitHub
// PR also pushes (or force-with-lease after a rebase) so the PR stays current.
func (e *Engine) UpdateWorkspaceWithBase(workspaceID, method string) (domain.UpdateWorkspaceResult, error) {
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return domain.UpdateWorkspaceResult{}, fmt.Errorf("workspace %q not found", workspaceID)
	}
	if ws.IsMain || ws.IsPRChatHost() {
		return domain.UpdateWorkspaceResult{}, fmt.Errorf("the main working copy is already the project checkout")
	}
	if landLockedByRun(ws) {
		return domain.UpdateWorkspaceResult{}, fmt.Errorf("a loop is running — pause it before updating from the workspace")
	}
	if ws.State == domain.StateMerged {
		return domain.UpdateWorkspaceResult{Status: domain.UpdateWorkspaceNoop, Workspace: ws}, nil
	}
	proj, ok := e.Store.Project(ws.ProjectID)
	if !ok || proj.RepoPath == "" {
		return domain.UpdateWorkspaceResult{}, fmt.Errorf("project for workspace %q not found", workspaceID)
	}
	base := strings.TrimSpace(ws.BaseBranch)
	if base == "" {
		base = strings.TrimSpace(proj.BaseBranch)
	}
	if base == "" {
		base = "main"
	}
	resolved := domain.NormalizeUpdateBaseMethod(method)
	if strings.TrimSpace(method) == "" {
		resolved = domain.EffectiveUpdateBaseMethod(proj, e.settings())
	}
	mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	base, behind, _ := mgr.BehindBase(ctx, ws.WorktreePath, base, true)
	if mgr.UpdateInProgress(ctx, ws.WorktreePath) {
		return domain.UpdateWorkspaceResult{Status: domain.UpdateWorkspaceConflict, Workspace: ws, Base: base}, nil
	}
	if behind == 0 {
		return domain.UpdateWorkspaceResult{Status: domain.UpdateWorkspaceNoop, Workspace: ws, Base: base}, nil
	}

	err := mgr.BeginUpdateWithBase(ctx, ws.WorktreePath, base, string(resolved))
	if err == nil {
		e.syncWorkspaceDiff(workspaceID, true)
		e.emitWorkspaceEvent(workspaceID, "AR", "You", fmt.Sprintf(
			"Updated with latest <code>%s</code> (%s).",
			html.EscapeString(mgr.ShortBranchName(ctx, base)), html.EscapeString(domain.UpdateBaseMethodLabel(resolved))))
		ws, _ = e.Store.Workspace(workspaceID)
		res := domain.UpdateWorkspaceResult{Status: domain.UpdateWorkspaceUpdated, Workspace: ws, Base: base}
		e.syncGitHubPRAfterBaseUpdate(ctx, ws, proj, mgr, base, resolved, &res)
		return res, nil
	}
	if mgr.UpdateInProgress(ctx, ws.WorktreePath) {
		e.emitWorkspaceEvent(workspaceID, "AR", "You",
			"Update with base hit conflicts — resolve them in chat (/update).")
		return domain.UpdateWorkspaceResult{Status: domain.UpdateWorkspaceConflict, Workspace: ws, Base: base}, nil
	}
	return domain.UpdateWorkspaceResult{}, fmt.Errorf("update with %s: %w", base, err)
}

// syncGitHubPRAfterBaseUpdate pushes a clean update-with-base onto the open
// GitHub PR. Force-with-lease only after a rebase (history rewrite is expected).
// A merge-method update is a regular push — if the remote advanced, the push
// fails and the local update stays; the titlebar then offers Pull / Force push.
// Do not fetch immediately before a force: --force-with-lease compares against
// the last-known origin/<PR.Branch>, so a fetch would make the lease always hold.
func (e *Engine) syncGitHubPRAfterBaseUpdate(ctx context.Context, ws domain.Workspace, proj domain.Project, mgr *gitpkg.Manager, base string, method domain.UpdateBaseMethod, res *domain.UpdateWorkspaceResult) {
	if res == nil {
		return
	}
	existing, ok := e.Store.PRByWorkspace(ws.ID)
	if !ok || existing.Source != domain.SourceGitHub || existing.State == "Merged" || existing.Group == "merged" {
		return
	}
	force := method == domain.UpdateBaseRebase
	if _, _, err := e.syncExistingWorkspacePR(ctx, ws, proj, existing, mgr, base, "", "", force, false); err != nil {
		e.emitWorkspaceEvent(ws.ID, "AR", "You",
			"Updated with base but could not push to GitHub PR: "+html.EscapeString(strings.TrimSpace(err.Error())))
		return
	}
	res.PushedPR = existing.Number
	res.ForcePushed = force
}

// PullWorkspacePR fetches the open GitHub PR's remote head and merges or
// rebases it into the worktree (same UpdateBaseMethod prefs as Update with
// base). Used when GitHub is ahead of local HEAD. Conflicts leave markers
// and return Status=conflict so the UI invokes /update.
func (e *Engine) PullWorkspacePR(workspaceID string) (domain.UpdateWorkspaceResult, error) {
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return domain.UpdateWorkspaceResult{}, fmt.Errorf("workspace %q not found", workspaceID)
	}
	if ws.IsMain || ws.IsPRChatHost() {
		return domain.UpdateWorkspaceResult{}, fmt.Errorf("the main working copy has no workspace PR")
	}
	if landLockedByRun(ws) {
		return domain.UpdateWorkspaceResult{}, fmt.Errorf("a loop is running — pause it before pulling")
	}
	pr, ok := e.Store.PRByWorkspace(workspaceID)
	if !ok || pr.State == "Merged" || pr.Group == "merged" {
		return domain.UpdateWorkspaceResult{}, fmt.Errorf("no open pull request for this workspace")
	}
	if pr.Source != domain.SourceGitHub {
		return domain.UpdateWorkspaceResult{}, fmt.Errorf("this is a Local PR — there is no GitHub head to pull")
	}
	proj, ok := e.Store.Project(ws.ProjectID)
	if !ok || proj.RepoPath == "" {
		return domain.UpdateWorkspaceResult{}, fmt.Errorf("project for workspace %q not found", workspaceID)
	}
	ref, _, _ := githubPRTrackingRef(pr)
	if ref == "" {
		return domain.UpdateWorkspaceResult{}, fmt.Errorf("the open PR has no remote branch")
	}
	method := domain.EffectiveUpdateBaseMethod(proj, e.settings())
	mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	if mgr.UpdateInProgress(ctx, ws.WorktreePath) {
		return domain.UpdateWorkspaceResult{Status: domain.UpdateWorkspaceConflict, Workspace: ws, Base: ref}, nil
	}
	fetchGitHubPRHead(ctx, mgr, pr)
	behind, _ := mgr.CommitsBehind(ctx, ws.WorktreePath, ref)
	if behind == 0 {
		return domain.UpdateWorkspaceResult{Status: domain.UpdateWorkspaceNoop, Workspace: ws, Base: ref}, nil
	}

	err := mgr.BeginUpdateWithBase(ctx, ws.WorktreePath, ref, string(method))
	if err == nil {
		e.syncWorkspaceDiff(workspaceID, true)
		base := strings.TrimSpace(ws.BaseBranch)
		if base == "" {
			base = strings.TrimSpace(proj.BaseBranch)
		}
		if base == "" {
			base = "main"
		}
		e.refreshWorkspacePRSnapshot(ctx, ws, pr, mgr, base)
		e.publishPRUpdated(pr.ID)
		e.emitWorkspaceEvent(workspaceID, "AR", "You", fmt.Sprintf(
			"Pulled updates from GitHub PR <code>#%d</code> (%s).",
			pr.Number, html.EscapeString(domain.UpdateBaseMethodLabel(method))))
		ws, _ = e.Store.Workspace(workspaceID)
		return domain.UpdateWorkspaceResult{Status: domain.UpdateWorkspaceUpdated, Workspace: ws, Base: ref}, nil
	}
	if mgr.UpdateInProgress(ctx, ws.WorktreePath) {
		e.emitWorkspaceEvent(workspaceID, "AR", "You",
			"Pull from GitHub hit conflicts — resolve them in chat (/update).")
		return domain.UpdateWorkspaceResult{Status: domain.UpdateWorkspaceConflict, Workspace: ws, Base: ref}, nil
	}
	return domain.UpdateWorkspaceResult{}, fmt.Errorf("pull from GitHub PR #%d: %w", pr.Number, err)
}

// OpenWorkspacePR opens a PR from a workspace the autopilot is not currently
// driving (hand-driven, or a loop that has idled). First open snapshots the worktree's diff + commits
// versus the workspace base and creates a Local PR (or a GitHub PR when the
// project opts in). If a PR is already open, this pushes new commits (GitHub)
// and refreshes the Recur snapshot without rewriting title, body, or comments.
// force is --force-with-lease (titlebar Force push). Title and body on first
// open are engine-generated from the diff; OpenWorkspacePRWith is the agent
// path that supplies them.
func (e *Engine) OpenWorkspacePR(workspaceID string, force bool) (string, error) {
	id, _, err := e.openWorkspacePR(workspaceID, "", "", false, force)
	return id, err
}

// OpenWorkspacePRWith is mcp__recur__open_pr: same as OpenWorkspacePR, but the
// agent-written title and markdown body go on the GitHub (or Local) PR. If a
// GitHub PR is already open, title and body are ignored — Recur pushes and
// refreshes. If only a Local PR is open, title and body update it (and retry
// GitHub create when the project opts in). draft is gh pr create --draft
// (titlebar Open draft PR). The agent path never force-pushes; use the
// titlebar Force push for that.
func (e *Engine) OpenWorkspacePRWith(workspaceID, title, body string, draft bool) (string, error) {
	id, _, err := e.openWorkspacePR(workspaceID, title, body, draft, false)
	return id, err
}

func (e *Engine) openWorkspacePR(workspaceID, title, body string, draft, force bool) (id, ghFail string, err error) {
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return "", "", fmt.Errorf("workspace %q not found", workspaceID)
	}
	if ws.IsMain || ws.IsPRChatHost() {
		return "", "", fmt.Errorf("the main working copy can't open a PR — create a workspace for changes you want to review")
	}
	if landLockedByRun(ws) {
		return "", "", fmt.Errorf("a loop is running — wait until it pauses, then open a PR from the titlebar or /open-pr")
	}
	proj, ok := e.Store.Project(ws.ProjectID)
	if !ok || proj.RepoPath == "" {
		return "", "", fmt.Errorf("project for workspace %q not found", workspaceID)
	}
	base := ws.BaseBranch
	if base == "" {
		base = proj.BaseBranch
	}
	if base == "" {
		base = "main"
	}

	mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	if risk := gitpkg.InspectDiscardRisk(ctx, ws.WorktreePath); risk.DirtyCount > 0 || risk.Unmerged {
		return "", "", fmt.Errorf("commit or discard uncommitted changes first — they would not be included in the PR")
	}

	if existing, ok := e.Store.OpenPRByWorkspace(workspaceID); ok {
		// Already open: retry GitHub create if we only have a Local fallback,
		// otherwise push new commits and refresh the snapshot. Do not rewrite
		// the title/body on a GitHub PR. Prefer GitHub over a leftover Local
		// row so a second Open PR cannot mint a duplicate.
		return e.syncExistingWorkspacePR(ctx, ws, proj, existing, mgr, base, title, body, force, draft)
	}

	diff, err := mgr.DiffPR(ctx, ws.WorktreePath, base)
	if err != nil {
		return "", "", fmt.Errorf("compute diff: %w", err)
	}
	commits, _ := mgr.LogCommits(ctx, ws.WorktreePath, base)
	for i := range commits {
		if cf, cerr := mgr.CommitDiff(ctx, ws.WorktreePath, commits[i].Hash); cerr == nil {
			commits[i].Files = cf
		}
		if commits[i].When == "" {
			commits[i].When = "now"
		}
	}
	if len(diff.Files) == 0 && len(commits) == 0 {
		return "", "", fmt.Errorf("nothing to open a PR for — commit changes in this workspace first")
	}

	prNum := e.Store.NextPRNumber()
	prID := fmt.Sprintf("%d", prNum)
	source := domain.SourceLocal

	name := strings.TrimSpace(ws.GoalLabel())
	agentTitle := strings.TrimSpace(singleLine(title))
	title = agentTitle
	if title == "" {
		title = prTitle(firstNonEmptyStr(name, "Workspace changes"))
	} else {
		title = hardTruncateRunes(title, 80)
	}
	ghBody := capGitHubPRBody(strings.TrimSpace(body))
	if ghBody == "" {
		_, diffBody := mgr.ChangedDiff(ctx, ws.WorktreePath, base)
		if t, b, gerr := e.generatePRDescription(ctx, workspaceID, ws.WorktreePath, diffBody, commitSubjects(commits)); gerr == nil {
			if agentTitle == "" && t != "" {
				title = t
			}
			ghBody = b
		}
	}
	if ghBody == "" {
		ghBody = githubPRMarkdown(prFallbackIntro(ws.Goal, ws.GoalDisplay), commits, len(diff.Files), diff.Add, diff.Del, nil)
	}

	// Live worktree branch wins over the cached name (human/agent renames).
	branch := e.syncWorkspaceBranch(workspaceID)
	if branch == "" {
		branch = ws.Branch
	}

	if proj.PROnGitHub {
		if ghNum, ghOK, reason := e.CreateGitHubPR(ctx, proj.RepoPath, ws.WorktreePath, branch, base, title, ghBody, draft); ghOK {
			source = domain.SourceGitHub
			prNum = ghNum
			prID = fmt.Sprintf("gh-%d", ghNum)
			kind := "GitHub PR"
			if draft {
				kind = "GitHub draft PR"
			}
			e.emitWorkspaceEvent(workspaceID, "AR", "You", fmt.Sprintf("Opened %s #%d.", kind, ghNum))
		} else {
			ghFail = reason
			e.emitWorkspaceEvent(workspaceID, "AR", "You", "GitHub PR not opened ("+html.EscapeString(reason)+") — opening a Local PR instead.")
		}
	}

	checksOK := true
	var checks []domain.Check
	if source == domain.SourceGitHub {
		checks, checksOK = e.githubPRChecks(ctx, proj.RepoPath, prNum)
	}
	pr := domain.PR{
		ID:          prID,
		WorkspaceID: workspaceID,
		ProjectID:   ws.ProjectID,
		Group:       "you",
		Number:      prNum,
		Source:      source,
		Branch:      branch,
		Base:        base,
		Title:       title,
		Draft:       draft,
		Summary:     workspacePRSummary(name, len(diff.Files), diff.Add, diff.Del, len(commits)),
		Focus:       engineFocus(diff.Files, nil, nil),
		State:       "In review",
		Diff:        domain.DiffStat{Add: diff.Add, Del: diff.Del},
		FileCount:   len(diff.Files),
		ChecksOK:    checksOK,
		When:        "now",
		Commits:     commits,
		Files:       diff.Files,
		Checks:      checks,
	}
	if source != domain.SourceGitHub {
		upsertDescriptionComment(&pr, ghBody)
	}
	saved, err := e.Store.AddPR(pr)
	if err != nil {
		return "", "", fmt.Errorf("persist PR: %w", err)
	}
	e.dropUnmergedSiblingPRs(workspaceID, saved.ID)

	opened := workspacePRSourceLabel(source) + " PR"
	if draft && source == domain.SourceGitHub {
		opened = "GitHub draft PR"
	}
	e.emitWorkspaceEvent(workspaceID, "AR", "You", fmt.Sprintf(
		"Opened a %s <code>#%d</code> (+%d −%d across %s) from this workspace.",
		opened, saved.Number, saved.Diff.Add, saved.Diff.Del, plural(saved.FileCount, "file", "files")))
	e.publishPRUpdated(saved.ID)
	e.emitWorkspacePRInbox(saved, name)
	return saved.ID, ghFail, nil
}

// syncExistingWorkspacePR pushes new commits onto an already-open PR and
// refreshes the Recur snapshot. GitHub title/body stay put. A Local PR takes
// a new agent title/body (/open-pr retry). force is an explicit
// --force-with-lease (titlebar Force push, or a rebase update). Divergence
// alone must not force: that would drop remote-only commits.
func (e *Engine) syncExistingWorkspacePR(ctx context.Context, ws domain.Workspace, proj domain.Project, existing domain.PR, mgr *gitpkg.Manager, base, title, body string, force, draft bool) (string, string, error) {
	branch := e.syncWorkspaceBranch(ws.ID)
	if branch == "" {
		branch = ws.Branch
	}
	ghFail := ""
	if existing.Source == domain.SourceLocal && proj.PROnGitHub {
		id, fail, ok := e.promoteLocalPRToGitHub(ctx, ws, proj, existing, mgr, base, branch, title, body, draft)
		if ok {
			return id, "", nil
		}
		ghFail = fail
	}
	if existing.Source == domain.SourceLocal {
		e.applyLocalPRDetails(existing.ID, title, body)
	}
	if existing.Source == domain.SourceGitHub {
		head := existing.Branch
		if head == "" {
			head = branch
		}
		if perr := pushGitHubHead(ctx, ws.WorktreePath, proj.RepoPath, existing, head, force); perr != nil {
			note := "Could not push to GitHub: " + html.EscapeString(strings.TrimSpace(perr.Error()))
			e.emitWorkspaceEvent(ws.ID, "AR", "You", note)
			return "", "", fmt.Errorf("push to GitHub: %w", perr)
		}
		branch = head
	}
	if err := e.refreshWorkspacePRSnapshot(ctx, ws, existing, mgr, base); err != nil {
		return "", ghFail, err
	}
	if existing.Source != domain.SourceGitHub {
		e.Store.UpdatePR(existing.ID, func(p *domain.PR) { p.Branch = branch })
	}
	updated, ok := e.Store.PR(existing.ID)
	if !ok {
		return "", ghFail, fmt.Errorf("update PR %s", existing.ID)
	}
	kind := workspacePRSourceLabel(existing.Source)
	verb := "Updated"
	if existing.Source == domain.SourceGitHub {
		verb = "Pushed"
		if force {
			verb = "Force-pushed"
		}
	}
	e.emitWorkspaceEvent(ws.ID, "AR", "You", fmt.Sprintf(
		"%s updates to %s PR <code>#%d</code> (+%d −%d across %s).",
		verb, kind, updated.Number, updated.Diff.Add, updated.Diff.Del, plural(updated.FileCount, "file", "files")))
	e.publishPRUpdated(updated.ID)
	return updated.ID, ghFail, nil
}

// promoteLocalPRToGitHub retries gh pr create after a Local fallback (offline
// gh, missing auth, …). On success the existing Recur PR is marked GitHub;
// the Recur id stays put so the comment thread and inbox row keep working.
func (e *Engine) promoteLocalPRToGitHub(ctx context.Context, ws domain.Workspace, proj domain.Project, existing domain.PR, mgr *gitpkg.Manager, base, branch, title, body string, draft bool) (string, string, bool) {
	if existing.Draft {
		draft = true
	}
	title = strings.TrimSpace(singleLine(title))
	if title == "" {
		title = existing.Title
	}
	if title == "" {
		title = prTitle(firstNonEmptyStr(strings.TrimSpace(ws.GoalLabel()), "Workspace changes"))
	} else {
		title = hardTruncateRunes(title, 80)
	}
	ghBody := capGitHubPRBody(strings.TrimSpace(body))
	if ghBody == "" {
		ghBody = prDescription(existing)
	}
	if ghBody == "" {
		diff, _ := mgr.DiffPR(ctx, ws.WorktreePath, base)
		commits, _ := mgr.LogCommits(ctx, ws.WorktreePath, base)
		add, del, files := 0, 0, 0
		if diff.Files != nil {
			add, del, files = diff.Add, diff.Del, len(diff.Files)
		}
		ghBody = githubPRMarkdown(prFallbackIntro(ws.Goal, ws.GoalDisplay), commits, files, add, del, nil)
	}
	ghNum, ghOK, reason := e.CreateGitHubPR(ctx, proj.RepoPath, ws.WorktreePath, branch, base, title, ghBody, draft)
	if !ghOK {
		e.emitWorkspaceEvent(ws.ID, "AR", "You", "GitHub PR still not opened ("+html.EscapeString(reason)+") — keeping the Local PR.")
		return "", reason, false
	}
	updated, ok := e.Store.UpdatePR(existing.ID, func(p *domain.PR) {
		p.Source = domain.SourceGitHub
		p.Number = ghNum
		p.Branch = branch
		p.Title = title
		p.Draft = draft
		upsertDescriptionComment(p, ghBody)
	})
	if !ok {
		return "", "", false
	}
	opened := "GitHub PR"
	if draft {
		opened = "GitHub draft PR"
	}
	if err := e.refreshWorkspacePRSnapshot(ctx, ws, updated, mgr, base); err != nil {
		e.emitWorkspaceEvent(ws.ID, "AR", "You", fmt.Sprintf("Opened %s #%d (snapshot refresh failed).", opened, ghNum))
		e.publishPRUpdated(updated.ID)
		e.dropUnmergedSiblingPRs(ws.ID, updated.ID)
		return updated.ID, "", true
	}
	e.emitWorkspaceEvent(ws.ID, "AR", "You", fmt.Sprintf("Opened %s #%d.", opened, ghNum))
	e.publishPRUpdated(updated.ID)
	e.dropUnmergedSiblingPRs(ws.ID, updated.ID)
	return updated.ID, "", true
}

func (e *Engine) applyLocalPRDetails(prID, title, body string) {
	title = strings.TrimSpace(singleLine(title))
	body = capGitHubPRBody(strings.TrimSpace(body))
	if title == "" && body == "" {
		return
	}
	e.Store.UpdatePR(prID, func(p *domain.PR) {
		if p.Source == domain.SourceGitHub {
			return
		}
		if title != "" {
			p.Title = hardTruncateRunes(title, 80)
		}
		if body != "" {
			upsertDescriptionComment(p, body)
		}
		touchPRUpdated(p)
	})
}

// refreshWorkspacePRSnapshot reloads files/commits/summary from the worktree
// without touching title, body, comments, or the frozen GitHub head.
func (e *Engine) refreshWorkspacePRSnapshot(ctx context.Context, ws domain.Workspace, existing domain.PR, mgr *gitpkg.Manager, base string) error {
	diff, err := mgr.DiffPR(ctx, ws.WorktreePath, base)
	if err != nil {
		return fmt.Errorf("compute diff: %w", err)
	}
	commits, _ := mgr.LogCommits(ctx, ws.WorktreePath, base)
	for i := range commits {
		if cf, cerr := mgr.CommitDiff(ctx, ws.WorktreePath, commits[i].Hash); cerr == nil {
			commits[i].Files = cf
		}
		commits[i].When = "now"
	}
	name := strings.TrimSpace(ws.GoalLabel())
	if _, ok := e.Store.UpdatePR(existing.ID, func(p *domain.PR) {
		p.Files = diff.Files
		p.Diff = domain.DiffStat{Add: diff.Add, Del: diff.Del}
		p.FileCount = len(diff.Files)
		p.Commits = commits
		p.Summary = workspacePRSummary(name, len(diff.Files), diff.Add, diff.Del, len(commits))
		p.Focus = engineFocus(diff.Files, nil, nil)
		p.When = "now"
		p.UpdatedAt = time.Now().UTC().Format(time.RFC3339)
	}); !ok {
		return fmt.Errorf("update PR %s", existing.ID)
	}
	return nil
}

// workspacePRSummary is a hand-driven-workspace PR summary — no convergence/review
// claims (it ran no agents through a pipeline), just what changed.
func workspacePRSummary(name string, fileCount, add, del, commitCount int) []domain.SummaryPara {
	label := "this workspace"
	if strings.TrimSpace(name) != "" {
		label = html.EscapeString(strings.TrimSpace(name))
	}
	return []domain.SummaryPara{
		{Kind: "p", HTML: fmt.Sprintf("Changes from the <b>%s</b> workspace.", label)},
		{Kind: "bullets", Items: []string{
			fmt.Sprintf("%s across %s.", plural(commitCount, "commit", "commits"), plural(fileCount, "file", "files")),
			fmt.Sprintf("Net diff <code>+%d −%d</code> versus the base branch.", add, del),
		}},
	}
}

// emitWorkspacePRInbox adds the actionable "ready for review" inbox row for a
// workspace PR (worded plainly — a workspace PR ran no review/verify).
func (e *Engine) emitWorkspacePRInbox(pr domain.PR, name string) {
	sub := html.EscapeString(truncate(strings.TrimSpace(name), 56))
	if sub == "" {
		sub = "Workspace changes"
	}
	e.Store.AddInboxItem(domain.InboxItem{
		ID:          "ib-pr-" + pr.ID,
		Group:       "ready",
		Unread:      true,
		Kind:        "pr",
		Color:       "var(--emerald)",
		Title:       fmt.Sprintf("<b>#%d</b> is ready for review", pr.Number),
		Sub:         sub,
		Time:        "now",
		Action:      "Review",
		ActionClass: "go",
		Target:      "pr:" + pr.ID,
		WorkspaceID: pr.WorkspaceID,
	})
	e.Bus.Publish(Event{Name: "inbox.updated", Data: map[string]any{"unread": e.Store.UnreadCount()}})
	e.Bus.Publish(Event{Name: "notify", Data: map[string]any{
		"title":  fmt.Sprintf("PR #%d is ready for review", pr.Number),
		"body":   truncate(strings.TrimSpace(name), 120),
		"target": "pr:" + pr.ID,
	}})
}

// workspacePRSourceLabel is the human word for a PR source.
func workspacePRSourceLabel(s domain.PRSource) string {
	if s == domain.SourceGitHub {
		return "GitHub"
	}
	return "Local"
}

// dropUnmergedSiblingPRs deletes other unmerged PRs on the same workspace so a
// Local fallback cannot sit beside a later GitHub PR (one open PR per workspace).
func (e *Engine) dropUnmergedSiblingPRs(workspaceID, keepID string) {
	if workspaceID == "" || keepID == "" {
		return
	}
	for _, p := range e.Store.PRs() {
		if p.WorkspaceID != workspaceID || p.ID == keepID {
			continue
		}
		if p.State == "Merged" || p.Group == "merged" {
			continue
		}
		if !e.Store.DeletePR(p.ID) {
			continue
		}
		e.clearPRInbox(p.ID)
		e.Bus.Publish(Event{Name: "pr.removed", Data: map[string]any{"prId": p.ID}})
	}
}

// landLockedByRun is true while the autopilot actively owns the worktree.
// After a loop idles (converge reviews, pause, stop), the human can merge,
// update, or open a PR themselves — same as a hand-driven workspace.
func landLockedByRun(ws domain.Workspace) bool {
	return ws.HasRun() && domain.RunActive(ws.State)
}
