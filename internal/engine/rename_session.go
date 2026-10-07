package engine

import (
	"context"
	"fmt"
	"strings"
	"time"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

func (r engineReporter) RenameSession(workspaceID, agentCode, ownerChat, title string) string {
	return r.eng.renameChatSession(workspaceID, agentCode, ownerChat, title)
}

func (r engineReporter) RenameWorkspace(workspaceID, agentCode, title string) string {
	return r.eng.RenameWorkspace(workspaceID, title)
}

func (r engineReporter) RenameBranch(workspaceID, agentCode, name string) string {
	return r.eng.RenameWorkspaceBranch(workspaceID, name)
}

func (r engineReporter) OpenPR(workspaceID, agentCode, title, body string, draft bool) string {
	_ = agentCode
	existing, had := r.eng.Store.PRByWorkspace(workspaceID)
	hadOpen := had && existing.State != "Merged" && existing.Group != "merged"
	wasLocal := hadOpen && existing.Source == domain.SourceLocal
	id, ghFail, err := r.eng.openWorkspacePR(workspaceID, title, body, draft, false)
	if err != nil {
		return "open_pr: " + err.Error()
	}
	pr, ok := r.eng.Store.PR(id)
	if !ok {
		return "Opened PR " + id + "."
	}
	wantGitHub := false
	if ws, ok := r.eng.Store.Workspace(workspaceID); ok {
		if proj, pok := r.eng.Store.Project(ws.ProjectID); pok {
			wantGitHub = proj.PROnGitHub
		}
	}
	return formatOpenPRToolResult(pr, wantGitHub, hadOpen, wasLocal, ghFail)
}

func formatOpenPRToolResult(pr domain.PR, wantGitHub, hadOpen, wasLocal bool, ghFail string) string {
	if pr.Source == domain.SourceGitHub {
		kind := "GitHub PR"
		if pr.Draft {
			kind = "GitHub draft PR"
		}
		if wasLocal {
			return fmt.Sprintf("Opened %s #%d (promoted from Local).", kind, pr.Number)
		}
		if hadOpen {
			return fmt.Sprintf("Pushed new commits to GitHub PR #%d (description unchanged).", pr.Number)
		}
		return fmt.Sprintf("Opened %s #%d.", kind, pr.Number)
	}
	fail := strings.TrimSpace(ghFail)
	if fail != "" {
		fail = hardTruncateRunes(ghFailDiag(fail), 240)
	}
	if hadOpen {
		if wantGitHub {
			if fail != "" {
				return fmt.Sprintf("PR #%d is already open (Local). GitHub create failed (%s) — retry Open PR.", pr.Number, fail)
			}
			return fmt.Sprintf("PR #%d is already open (Local). GitHub create failed — retry Open PR.", pr.Number)
		}
		return fmt.Sprintf("PR #%d is already open (Local).", pr.Number)
	}
	if wantGitHub {
		if fail != "" {
			return fmt.Sprintf("Opened Local PR #%d (GitHub create failed: %s — retry Open PR).", pr.Number, fail)
		}
		return fmt.Sprintf("Opened Local PR #%d (GitHub create failed — retry Open PR).", pr.Number)
	}
	return fmt.Sprintf("Opened Local PR #%d.", pr.Number)
}

// RenameChatSession sets the chat tab title for Maestro or a PA* session.
// Specialist role codes (PL, …) retarget the owning loop's chat tab.
func (e *Engine) RenameChatSession(workspaceID, agentCode, title string) string {
	return e.renameChatSession(workspaceID, agentCode, "", title)
}

func (e *Engine) renameChatSession(workspaceID, agentCode, ownerChat, title string) string {
	title = strings.TrimSpace(title)
	if title == "" {
		return "rename_session: title is required"
	}
	// Hard rune cap — truncate() appends "…" (display ellipsis), wrong for titles.
	title = hardTruncateRunes(title, 200)
	code := strings.TrimSpace(agentCode)
	if code == "" {
		return "rename_session: missing agent"
	}
	if isSpecialistChatCode(code) {
		if owner := strings.TrimSpace(ownerChat); owner != "" {
			code = owner
		} else if lp := e.runningLoopForDone(workspaceID, code); lp != nil && strings.TrimSpace(lp.ChatCode) != "" {
			code = lp.ChatCode
		} else {
			n := 0
			e.forEachRunningLoop(workspaceID, func(*runningLoop) { n++ })
			// Multiple walks share role codes; LoopChatCode is the first tab
			// (dispatchAdditionalChatRun leaves it). Don't retarget a sibling.
			if n <= 1 {
				if ws, ok := e.Store.Workspace(workspaceID); ok {
					if owner := loopOwnerCode(ws); owner != "" {
						code = owner
					}
				}
			}
		}
	}
	if _, ok := e.Store.Agent(workspaceID, code); !ok {
		return "rename_session: unknown chat tab"
	}
	// Provider must come from the tab's own model, not the Settings coder
	// default: ensureAgentSession overwrites session.Provider, and a foreign
	// value there makes the next rememberPairChatSelection think the dock
	// switched providers and set forkInject (full-transcript re-send).
	kind, name, provider := e.chatSessionMeta(workspaceID, code)
	sess, ok := e.ensureAgentSession(workspaceID, kind, code, name, provider)
	if !ok {
		return "rename_session: could not open session"
	}
	if err := e.Store.PatchSessionConfigTitle(sess.ID, title); err != nil {
		return "rename_session: " + err.Error()
	}
	e.Bus.Publish(Event{Name: "chat.renamed", Data: map[string]any{
		"workspaceId": workspaceID,
		"agentCode":   code,
		"title":       title,
	}})
	e.Bus.Publish(Event{Name: "workspace.chat_tab", Data: map[string]any{
		"workspaceId": workspaceID,
		"code":        code,
		"title":       title,
	}})
	return "Renamed chat to " + title + "."
}

// RenameWorkspace sets the sidebar title (Goal/Short). The main working
// copy keeps the project name; a loop run no longer pins the title to the task.
func (e *Engine) RenameWorkspace(workspaceID, title string) string {
	title = strings.TrimSpace(title)
	if title == "" {
		return "rename_workspace: title is required"
	}
	title = hardTruncateRunes(title, 200)
	title = sanitizeTitle(title, 60)
	if title == "" {
		return "rename_workspace: invalid title"
	}
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return "rename_workspace: unknown workspace"
	}
	if ws.IsMain {
		return "rename_workspace: the main working copy keeps the project name"
	}
	updated, ok := e.Store.UpdateWorkspace(workspaceID, func(lp *domain.Workspace) {
		if lp.IsMain {
			return
		}
		lp.Goal = title
		lp.Short = truncate(title, 48)
	})
	if !ok {
		return "rename_workspace: unknown workspace"
	}
	if updated.IsMain || updated.Goal != title {
		return "rename_workspace: the main working copy keeps the project name"
	}
	e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
	return "Renamed workspace to " + title + "."
}

// RenameWorkspaceBranch sanitizes name and runs `git branch -m` in the worktree,
// then syncs Workspace.Branch (and a local PR) from the live checkout.
func (e *Engine) RenameWorkspaceBranch(workspaceID, name string) string {
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return "rename_branch: unknown workspace"
	}
	if strings.TrimSpace(ws.WorktreePath) == "" {
		return "rename_branch: no worktree"
	}
	if domain.RunActive(ws.State) && ws.State != domain.StatePlanning {
		return "rename_branch: a loop is running — wait until it pauses"
	}
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	applied, err := gitpkg.RenameBranchWithPrefix(ctx, ws.WorktreePath, e.branchNamePrefix(), name)
	if err != nil {
		return "rename_branch: " + err.Error()
	}
	live := e.syncWorkspaceBranch(workspaceID)
	if live == "" {
		live = applied
	}
	return fmt.Sprintf("Renamed branch to %s.", live)
}
