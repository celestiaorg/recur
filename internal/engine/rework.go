package engine

import (
	"context"
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

// UpdatePRWithBase integrates the latest base tip into the PR's worktree (D54).
// Clean path: engine runs git merge/rebase directly (independent of D53 PR
// merge method). method is "merge" or "rebase"; empty uses project/settings
// UpdateBaseMethod prefs. Conflict path: abort, re-begin leaving markers,
// single Coder turn resolves, engine finishes the git op. The PR stays In review.
// UpdatingBase ("git" / "coder") is persisted so the PR UI can show progress.
func (e *Engine) UpdatePRWithBase(prID, method string) (domain.PR, bool) {
	pr, ok := e.Store.PR(prID)
	if !ok {
		return domain.PR{}, false
	}
	if pr.State == "Merged" || pr.Group == "merged" {
		return pr, false
	}
	if pr.UpdatingBase != "" {
		return pr, true // already in flight
	}
	if _, busy := e.updatingBase.Load(prID); busy {
		return pr, true
	}

	loop, ok := e.Store.Workspace(pr.WorkspaceID)
	if !ok || loop.WorktreePath == "" {
		return pr, false
	}
	proj, ok := e.Store.Project(loop.ProjectID)
	if !ok || proj.RepoPath == "" {
		return pr, false
	}
	base := pr.Base
	if base == "" {
		base = loop.BaseBranch
	}
	if base == "" {
		base = proj.BaseBranch
	}
	if base == "" {
		return pr, false
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

	// Mark git phase so the UI can show an Updating banner while merge/rebase runs.
	if started, ok := e.Store.UpdatePR(prID, func(p *domain.PR) { p.UpdatingBase = "git" }); ok {
		pr = started
		e.publishPRUpdated(prID)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	err := mgr.UpdateWorktreeWithBase(ctx, loop.WorktreePath, base, string(resolved))
	cancel()
	if err == nil {
		return e.finishUpdateWithBase(prID, loop, mgr, base, resolved)
	}

	// Conflict: fall back to a single Coder turn.
	if _, loaded := e.updatingBase.LoadOrStore(prID, true); loaded {
		return pr, true
	}
	note := nowComment("MA", "Maestro",
		fmt.Sprintf("Update with base (%s) hit conflicts — resolving with a single Coder turn.",
			html.EscapeString(domain.UpdateBaseMethodLabel(resolved))))
	updated, uok := e.Store.UpdatePR(prID, func(p *domain.PR) {
		p.UpdatingBase = "coder"
		p.Comments = append(p.Comments, note)
	})
	if uok {
		e.publishPRComment(prID, note)
		e.publishPRUpdated(prID)
	} else {
		updated = pr
	}
	e.emitWorkspaceEvent(pr.WorkspaceID, "MA", "Maestro",
		"Update with base conflicted — dispatching a lightweight Coder turn (no review/verify).")

	go e.resolveUpdateBaseConflict(prID, base, resolved)
	return updated, true
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

// resolveUpdateBaseConflict is the D54 conflict fallback: re-begin the git op
// (leaving markers), run one Coder turn to resolve them, then finish the git op
// and refresh the PR.
func (e *Engine) resolveUpdateBaseConflict(prID, base string, method domain.UpdateBaseMethod) {
	defer e.updatingBase.Delete(prID)

	pr, ok := e.Store.PR(prID)
	if !ok {
		return
	}
	loop, ok := e.Store.Workspace(pr.WorkspaceID)
	if !ok || loop.WorktreePath == "" {
		return
	}
	proj, ok := e.Store.Project(loop.ProjectID)
	if !ok || proj.RepoPath == "" {
		return
	}
	mgr := gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Minute)
	defer cancel()

	if err := mgr.BeginUpdateWithBase(ctx, loop.WorktreePath, base, string(method)); err != nil {
		// Conflict is expected — leave markers for the Coder. If Begin failed for
		// a non-conflict reason (e.g. missing base), abort and report.
		if !mgr.UpdateInProgress(ctx, loop.WorktreePath) {
			_ = mgr.AbortUpdateWithBase(ctx, loop.WorktreePath, string(method))
			e.failUpdateBase(prID, pr.WorkspaceID, "Could not start conflict resolution: "+err.Error())
			return
		}
	} else {
		// Clean on retry (race: base moved?) — just finish the refresh.
		e.finishUpdateWithBase(prID, loop, mgr, base, method)
		return
	}

	if err := e.runUpdateBaseCoder(ctx, loop, base, method); err != nil {
		_ = mgr.AbortUpdateWithBase(ctx, loop.WorktreePath, string(method))
		e.failUpdateBase(prID, pr.WorkspaceID, "Coder could not resolve conflicts: "+err.Error())
		return
	}

	if err := mgr.FinishUpdateWithBase(ctx, loop.WorktreePath, string(method)); err != nil {
		_ = mgr.AbortUpdateWithBase(ctx, loop.WorktreePath, string(method))
		e.failUpdateBase(prID, pr.WorkspaceID, "Could not finish update after conflict resolution: "+err.Error())
		return
	}

	e.finishUpdateWithBase(prID, loop, mgr, base, method)
}

func (e *Engine) failUpdateBase(prID, workspaceID, reason string) {
	note := nowComment("MA", "Maestro",
		html.EscapeString(reason)+" — send back to the conductor if you want more loop work.")
	if _, ok := e.Store.UpdatePR(prID, func(p *domain.PR) {
		p.UpdatingBase = ""
		p.Comments = append(p.Comments, note)
	}); ok {
		e.publishPRComment(prID, note)
		e.publishPRUpdated(prID)
	}
	e.emitWorkspaceEvent(workspaceID, "MA", "Maestro", html.EscapeString(reason))
}

// runUpdateBaseCoder runs a single Coder turn whose only job is resolving
// conflict markers left by BeginUpdateWithBase. No commit — FinishUpdateWithBase
// completes the git op afterwards.
func (e *Engine) runUpdateBaseCoder(ctx context.Context, loop domain.Workspace, base string, method domain.UpdateBaseMethod) error {
	rs := roles[domain.RoleCoder]
	agentID := loop.ID + "-" + rs.code
	set := e.settings()
	e.upsertAgent(loop.ID, domain.Agent{
		ID: agentID, WorkspaceID: loop.ID, Code: rs.code, Name: rs.name,
		Role: "writes & edits code", Model: set.CoderModel().ID,
		Status: "working", Color: "var(--cyan)", Task: "resolve update-with-base conflicts", Cur: -1,
	})

	prompt := fmt.Sprintf(
		"The worktree has an in-progress %s with %s that produced conflict markers. "+
			"Resolve every conflicted file: edit out the conflict markers and keep the "+
			"correct combined result. Do not change feature code beyond what's needed to "+
			"resolve conflicts. Do not run git commit, git merge, or git rebase — the "+
			"orchestrator will finish the git operation. When done, call mcp__recur__done.",
		domain.UpdateBaseMethodLabel(method), base,
	)

	token, mcpURL, err := e.registerMCP(loop.ID, rs.code, loopOwnerCode(loop), rs.allowedTools, set.CoderModel().String())
	if err != nil {
		return err
	}
	defer e.unregisterMCP(token)

	spec := AgentSpec{
		WorkspaceID:    loop.ID,
		ChatCode:       loopOwnerCode(loop),
		AgentID:        agentID,
		Role:           domain.RoleCoder,
		Model:          set.CoderModel().String(),
		Flags:          set.CoderFlags(),
		SystemPrompt:   coderSystemPrompt("Resolve merge/rebase conflicts from updating with base.", "", false),
		AllowedTools:   rs.allowedTools,
		Prompt:         prompt,
		Worktree:       loop.WorktreePath,
		PermissionMode: set.PermissionMode(),
		MCPURL:         mcpURL,
	}

	release, ok := e.acquireAgent(ctx.Done())
	if !ok {
		e.updateAgentStatus(loop.ID, rs.code, "idle")
		return ctx.Err()
	}
	defer release()

	// Reuse Loop.onAgentEvent so tool rows render identically to a normal Coder
	// turn (step kind tint, ToolUseID pairing, tool_result attach).
	lp := newLoop(e, loop)
	if proj, pok := e.Store.Project(loop.ProjectID); pok {
		lp.git = gitpkg.NewManager(proj.RepoPath, e.worktreeRoot)
	}

	res, err := e.runAgent(ctx, spec, func(ev ParsedEvent) {
		lp.onAgentEvent(rs, ev)
	})
	e.publishResource()
	e.updateAgentStatus(loop.ID, rs.code, "idle")

	if err != nil {
		return err
	}
	if res.IsError {
		return fmt.Errorf("coder turn failed: %s", res.Subtype)
	}
	return nil
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
