package engine

import (
	"context"
	"fmt"
	"log"
	"strings"
	"time"

	"recur/internal/domain"
	"recur/internal/mcp"
)

// CreateWorkspaceRequest is the create_workspace MCP tool input.
type CreateWorkspaceRequest = mcp.CreateWorkspaceRequest

// CreateWorkspaceFromChat starts a fresh workspace on behalf of an Agent chat
// (triage schedules, "open one workspace per issue"). Model and flags follow
// Settings (calling chat vs that mode's default); Simplify follows the calling
// chat. Idempotent on title: a live workspace in the same project with that
// title is returned instead of a duplicate, so a recurring schedule cannot
// fan out the same issue twice.
func (e *Engine) CreateWorkspaceFromChat(callerWorkspaceID, callerCode string, req CreateWorkspaceRequest) string {
	if e.isShuttingDown() {
		return "create_workspace: Recur is closing"
	}
	title, prompt := strings.TrimSpace(req.Title), strings.TrimSpace(req.Prompt)
	if title == "" || prompt == "" {
		return "create_workspace: title and prompt are required"
	}
	caller, ok := e.Store.Workspace(callerWorkspaceID)
	if !ok {
		return "create_workspace: calling workspace not found"
	}
	projectID := strings.TrimSpace(req.ProjectID)
	if projectID == "" {
		projectID = caller.ProjectID
	}
	proj, ok := e.Store.Project(projectID)
	if !ok {
		return fmt.Sprintf("create_workspace: project %q not found", projectID)
	}
	mode := domain.NormalizeChatMode(req.Mode)
	if mode == "" {
		mode = domain.ChatModeAgent
	}
	l := promptLaunch{ProjectID: projectID, Name: title, Prompt: prompt, Mode: mode, BaseBranch: strings.TrimSpace(req.BaseBranch)}
	child, reason := wantChild(caller, req, mode, projectID)
	if reason != "" {
		return "create_workspace: " + reason
	}
	if child {
		l.BaseBranch = caller.Branch
	}
	if l.BaseBranch == "" {
		l.BaseBranch = proj.BaseBranch
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if !e.gitManager(proj.RepoPath).BaseBranchExists(ctx, l.BaseBranch) {
		return fmt.Sprintf("create_workspace: base branch %q not found in %s", l.BaseBranch, proj.Name)
	}
	if mode == domain.ChatModeLoop {
		guard, reason := e.resolveGuard(req.LoopID, req.PolicyID, e.Store.Policy().Autonomy)
		if reason != "" {
			return "create_workspace: " + reason
		}
		l.LoopID, l.PolicyID = guard.Loop.ID, guard.Policy.ID
	}
	set := e.settings()
	if set.EffectiveChildWorkspaceModel() == domain.ChildWorkspaceModelMode {
		l.Model, l.Flags = set.ModelForChatMode(mode)
	} else {
		l.Model, l.Flags, _ = e.WorkspaceChatSelection(callerWorkspaceID, callerCode)
	}
	l.Simplify = e.ChatSimplify(callerWorkspaceID, callerCode)
	e.scheduleMu.Lock()
	defer e.scheduleMu.Unlock()
	if e.isShuttingDown() {
		return "create_workspace: Recur is closing"
	}
	for _, w := range e.Store.Workspaces() {
		if w.ProjectID == projectID && !w.Archived && !w.IsMain && strings.EqualFold(strings.TrimSpace(w.Goal), title) {
			return fmt.Sprintf("Workspace %s already exists for %q; not creating a duplicate.", w.ID, title)
		}
	}
	var link func(string) error
	wave := 0
	if child {
		wave = e.chatChildWave(callerWorkspaceID, callerCode)
		link = func(wsID string) error {
			e.Store.UpdateWorkspace(wsID, func(w *domain.Workspace) {
				w.ParentID, w.FanOut = callerWorkspaceID, &domain.ChildFanOut{Chat: callerCode, Mode: mode, Wave: wave}
			})
			return nil
		}
	}
	id, err := e.launchPromptWorkspace(l, link, func(status, detail string) {
		log.Printf("recur: create_workspace %s from %s: %s %s", title, callerWorkspaceID, status, detail)
	})
	if err != nil {
		return "create_workspace: " + err.Error()
	}
	if child {
		if ws, ok := e.Store.Workspace(id); ok {
			e.emitChildCard(callerWorkspaceID, ws)
		}
		go e.watchChatChild(callerWorkspaceID, id)
		return fmt.Sprintf("Created subagent %s (%q) as a child workspace off %s; its %s chat is running the prompt. This turn does not wait: when it finishes you get a new turn here with its report, then read_child, merge_child, or archive_child it.", id, title, l.BaseBranch, mode)
	}
	return fmt.Sprintf("Created workspace %s (%q) in %s from %s; its %s chat is running the prompt. It appears in the sidebar as it allocates.", id, title, proj.Name, l.BaseBranch, mode)
}

// wantChild decides whether the new workspace is a merge-back child of the
// caller. Parent workspaces default yes; standalone when asked, when the caller
// is a scheduled run (its branch is scratch), when mode is loop (a loop ships
// its own PR), or when a base branch was named. Nested callers cannot create
// workspaces. An explicit standalone=false still needs a caller that can be merged into.
func wantChild(caller domain.Workspace, req CreateWorkspaceRequest, mode, projectID string) (child bool, reason string) {
	if strings.TrimSpace(caller.ParentID) != "" {
		return false, "a child workspace cannot create workspaces"
	}
	switch {
	case req.Standalone != nil:
		child = !*req.Standalone
	default:
		child = caller.ScheduleID == "" && mode != domain.ChatModeLoop && strings.TrimSpace(req.BaseBranch) == ""
	}
	if !child {
		return false, ""
	}
	if projectID != caller.ProjectID || strings.TrimSpace(caller.Branch) == "" || strings.TrimSpace(caller.WorktreePath) == "" {
		return false, "a child workspace needs a calling workspace with a branch and worktree in the same project; pass standalone=true"
	}
	return true, ""
}

func (r engineReporter) MergeChild(workspaceID, agentCode, childID string) string {
	return r.eng.MergeChildFromChat(workspaceID, childID)
}

func (r engineReporter) ReadChild(workspaceID, agentCode, childID string) string {
	return r.eng.ReadChildFromChat(workspaceID, childID)
}

func (r engineReporter) ArchiveChild(workspaceID, agentCode, childID string) string {
	return r.eng.ArchiveChildFromChat(workspaceID, childID)
}

func (r engineReporter) MessageChild(workspaceID, agentCode, childID, message string) string {
	return r.eng.MessageChildFromChat(workspaceID, childID, message)
}

func (r engineReporter) ReadChildArtifact(workspaceID, agentCode, childID, artifactID string) (string, error) {
	return r.eng.ReadChildArtifactFromChat(workspaceID, childID, artifactID)
}

func (r engineReporter) CreateWorkspace(workspaceID, agentCode string, req CreateWorkspaceRequest) string {
	return r.eng.CreateWorkspaceFromChat(workspaceID, agentCode, req)
}
