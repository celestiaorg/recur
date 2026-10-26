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

// SendBackToLoop is the PR Send Back control: flush pending line notes, put the
// review thread in conductor context, and start a Maestro turn. Maestro answers
// or calls reenter_phase.
func (e *Engine) SendBackToLoop(prID string, comments []string) (domain.PR, bool) {
	return e.sendBackToLoop(prID, comments)
}

// MaestroSendBack is mcp__recur__send_back: look up the open PR for workspaceID
// and start the same Send Back path the PR UI uses. Returns a short status
// string for the tool result.
func (e *Engine) MaestroSendBack(workspaceID, instruction string) string {
	instruction = strings.TrimSpace(instruction)
	if instruction == "" {
		return "send_back refused: instruction is empty."
	}
	pr, ok := e.Store.OpenPRByWorkspace(workspaceID)
	if !ok {
		return "send_back refused: this loop has no open PR — use plan to steer while it's still running."
	}
	if pr.State == "Merged" || pr.Group == "merged" {
		return "send_back refused: the PR is already merged."
	}
	if _, ok := e.sendBackToLoop(pr.ID, []string{instruction}); !ok {
		return "send_back failed — the PR or its loop is unavailable."
	}
	return "Sent review back — Maestro will answer or call reenter_phase."
}

var errPRUnavailable = errors.New("update-with-base failed: the PR or its loop is unavailable")

// UpdatePRWithBase integrates the latest base tip into the PR's worktree (D54).
// Clean path: engine runs git merge/rebase directly (independent of D53 PR
// merge method). method is "merge" or "rebase"; empty uses project/settings
// UpdateBaseMethod prefs. Conflict path: leave markers and return
// Status=conflict so the UI invokes /update in the workspace's ops tab. The PR
// stays In review. UpdatingBase ("git") is persisted so the PR UI can show progress.
func (e *Engine) UpdatePRWithBase(prID, method string) (domain.UpdateWorkspaceResult, error) {
	pr, ok := e.Store.PR(prID)
	if !ok {
		return domain.UpdateWorkspaceResult{}, errPRUnavailable
	}
	if pr.State == "Merged" || pr.Group == "merged" {
		return domain.UpdateWorkspaceResult{}, errPRUnavailable
	}
	if pr.UpdatingBase != "" {
		return domain.UpdateWorkspaceResult{}, nil // already in flight
	}

	loop, ok := e.Store.Workspace(pr.WorkspaceID)
	if !ok || loop.WorktreePath == "" {
		return domain.UpdateWorkspaceResult{}, errPRUnavailable
	}
	proj, ok := e.Store.Project(loop.ProjectID)
	if !ok || proj.RepoPath == "" {
		return domain.UpdateWorkspaceResult{}, errPRUnavailable
	}
	base := pr.Base
	if base == "" {
		base = loop.BaseBranch
	}
	if base == "" {
		base = proj.BaseBranch
	}
	if base == "" {
		return domain.UpdateWorkspaceResult{}, errPRUnavailable
	}

	resolved := domain.NormalizeUpdateBaseMethod(method)
	if strings.TrimSpace(method) == "" {
		resolved = domain.EffectiveUpdateBaseMethod(proj, e.settings())
	}
	mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
	fetchCtx, fetchCancel := context.WithTimeout(context.Background(), gitpkg.RemoteFetchTimeout)
	if ref, _, err := mgr.BehindBase(fetchCtx, loop.WorktreePath, base, true); err == nil && ref != "" {
		base = ref
	}
	fetchCancel()

	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	if mgr.UpdateInProgress(ctx, loop.WorktreePath) {
		return domain.UpdateWorkspaceResult{Status: domain.UpdateWorkspaceConflict, Workspace: loop, Base: base}, nil
	}

	// Mark git phase so the UI can show an Updating banner while merge/rebase runs.
	if _, ok := e.Store.UpdatePR(prID, func(p *domain.PR) { p.UpdatingBase = "git" }); ok {
		e.publishPRUpdated(prID)
	}

	// UpdateWorktreeWithBase aborts on conflict; Begin re-runs it leaving markers.
	err := mgr.UpdateWorktreeWithBase(ctx, loop.WorktreePath, base, string(resolved))
	if err != nil {
		err = mgr.BeginUpdateWithBase(ctx, loop.WorktreePath, base, string(resolved))
	}
	if err == nil {
		if _, ok := e.finishUpdateWithBase(prID, loop, mgr, base, resolved); !ok {
			return domain.UpdateWorkspaceResult{}, errPRUnavailable
		}
		return domain.UpdateWorkspaceResult{Status: domain.UpdateWorkspaceUpdated, Workspace: loop, Base: base}, nil
	}

	if _, ok := e.Store.UpdatePR(prID, func(p *domain.PR) { p.UpdatingBase = "" }); ok {
		e.publishPRUpdated(prID)
	}
	if !mgr.UpdateInProgress(ctx, loop.WorktreePath) {
		return domain.UpdateWorkspaceResult{}, fmt.Errorf("update with %s: %w", base, err)
	}
	e.emitWorkspaceEvent(loop.ID, "MA", "Maestro",
		fmt.Sprintf("Update with base (%s) hit conflicts. Resolving them with /update.",
			html.EscapeString(domain.UpdateBaseMethodLabel(resolved))))
	return domain.UpdateWorkspaceResult{Status: domain.UpdateWorkspaceConflict, Workspace: loop, Base: base}, nil
}

// finishUpdateWithBase refreshes the PR diff/commits after a successful base
// integration and clears BehindBase. PR stays In review.
func (e *Engine) finishUpdateWithBase(prID string, loop domain.Workspace, mgr *gitpkg.Manager, base string, method domain.UpdateBaseMethod) (domain.PR, bool) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	var files []domain.FileDiff
	var add, del int
	var commits []domain.Commit
	if d, err := mgr.DiffPR(ctx, loop.WorktreePath, base); err == nil {
		files, add, del = d.Files, d.Add, d.Del
	}
	if cs, err := mgr.LogCommits(ctx, loop.WorktreePath, base); err == nil {
		commits = cs
		for i := range commits {
			if cf, err := mgr.CommitDiff(ctx, loop.WorktreePath, commits[i].Hash); err == nil {
				commits[i].Files = cf
			}
			if commits[i].When == "" {
				commits[i].When = "now"
			}
		}
	}

	label := domain.UpdateBaseMethodLabel(method)
	// GitHub PRs reflect the base update via real commits, and this local note is
	// wiped/reordered on the next GitHub thread sync — skip it to avoid flicker.
	var note domain.PRComment
	cur, _ := e.Store.PR(prID)
	postNote := cur.Source != domain.SourceGitHub
	if postNote {
		note = nowComment("MA", "Maestro",
			fmt.Sprintf("Updated with latest base (%s).", html.EscapeString(label)))
	}
	updated, ok := e.Store.UpdatePR(prID, func(p *domain.PR) {
		p.Files = files
		p.FileCount = len(files)
		p.Diff = domain.DiffStat{Add: add, Del: del}
		p.Commits = commits
		p.BehindBase = 0
		p.UpdatingBase = ""
		if postNote {
			p.Comments = append(p.Comments, note)
		}
	})
	if !ok {
		return domain.PR{}, false
	}
	if postNote {
		e.publishPRComment(prID, note)
	}
	e.publishPRUpdated(prID)
	e.emitWorkspaceEvent(loop.ID, "MA", "Maestro",
		fmt.Sprintf("Updated PR with latest <code>%s</code> (%s).",
			html.EscapeString(base), html.EscapeString(label)))
	return updated, true
}

func (e *Engine) sendBackToLoop(prID string, comments []string) (domain.PR, bool) {
	pr, ok := e.Store.PR(prID)
	if !ok {
		return domain.PR{}, false
	}
	if pr.State == "Merged" || pr.Group == "merged" {
		return pr, false
	}

	sendBackNotes := append([]string(nil), comments...)
	pending := append([]domain.PendingLineComment(nil), pr.PendingComments...)
	if n := len(pending); n > 0 {
		extra := make([]string, 0, n)
		for _, pc := range pending {
			if strings.TrimSpace(pc.Body) != "" {
				extra = append(extra, pc.Body)
			}
		}
		comments = append(append([]string{}, comments...), extra...)
		if pr.Source == domain.SourceGitHub {
			ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
			err := submitGitHubPRReviewFn(ctx, e.prRepoPath(pr), pr.Number, "comment", "", pending)
			cancel()
			if err != nil {
				e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro",
					"Could not submit the review to GitHub: "+html.EscapeString(strings.TrimSpace(err.Error())))
				return pr, false
			}
		}
		var flushed []domain.PRComment
		if updatedPending, ok := e.Store.UpdatePR(prID, func(p *domain.PR) {
			for _, pc := range p.PendingComments {
				flushed = append(flushed, pendingToComment(pc, p.Files))
			}
			p.Comments = append(p.Comments, flushed...)
			p.PendingComments = nil
		}); ok {
			pr = updatedPending
			for _, c := range flushed {
				e.publishPRComment(prID, c)
			}
			e.resolvePendingFindings(pr, pending)
		}
	}

	// Persist the send-back notes on the thread so this and later conductor
	// turns see them in maestroContext.PRThread (not only the user message).
	var posted []domain.PRComment
	if updated, ok := e.Store.UpdatePR(prID, func(p *domain.PR) {
		for _, body := range sendBackNotes {
			body = strings.TrimSpace(body)
			if body == "" {
				continue
			}
			c := nowComment("AR", "You", html.EscapeString(body))
			c.BodyMD = body
			p.Comments = append(p.Comments, c)
			posted = append(posted, c)
		}
	}); ok {
		pr = updated
		for _, c := range posted {
			e.publishPRComment(prID, c)
		}
	}

	instruction := distillInstruction(comments)
	ws, ok := e.Store.Workspace(pr.WorkspaceID)
	if !ok {
		return pr, false
	}
	code := loopOwnerCode(ws)
	if code == "" {
		code = maestroAgentCode
	}
	text := "The human sent this PR's review back to you."
	if instruction != "" {
		text += "\n\n" + instruction
	}
	text += "\n\nThe conversation and line notes are in your loop context. Answer if this is a question. If it is work, call reenter_phase on an allowed phase (usually converge)."
	go e.sendMaestroMessage(pr.WorkspaceID, code, text, nil, "Sent review back")
	e.clearPRInbox(prID)
	e.publishPRUpdated(prID)
	return pr, true
}

// distillInstruction concatenates send-back notes into the conductor user turn.
func distillInstruction(comments []string) string {
	var nonEmpty []string
	for _, c := range comments {
		if s := strings.TrimSpace(c); s != "" {
			nonEmpty = append(nonEmpty, s)
		}
	}
	if len(nonEmpty) == 0 {
		return "Address the reviewer's feedback on this PR with minimal, correct edits."
	}
	if len(nonEmpty) == 1 {
		return "Address this review comment: " + nonEmpty[0]
	}
	return "Address the following review comments:\n- " + strings.Join(nonEmpty, "\n- ")
}
