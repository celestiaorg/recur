package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaCreateWorkspace = json.RawMessage(`{
  "type": "object",
  "properties": {
    "title": {"type": "string", "description": "Short workspace title. Reuse the same title for the same task so repeat calls dedupe"},
    "prompt": {"type": "string", "description": "First message for the new workspace's chat: the task plus the context it needs to work alone"},
    "project_id": {"type": "string", "description": "Project id; defaults to this workspace's project"},
    "base_branch": {"type": "string", "description": "Standalone only: branch to start from (defaults to the project's base branch). Setting it implies standalone"},
    "mode": {"type": "string", "enum": ["ask", "plan", "agent", "review", "loop"], "description": "Chat mode for the first tab (default agent). loop engages the autopilot with prompt as its goal"},
    "loop_id": {"type": "string", "description": "Bundle [[loop]] id for mode=loop; defaults to Settings"},
    "policy_id": {"type": "string", "description": "Bundle [[policy]] id for mode=loop; defaults to Settings"},
    "standalone": {"type": "boolean", "description": "true: an independent workspace that is never merged back here (its own branch off base_branch, its own PR). Default false: the workspace is a subagent of this one, branched off this branch; when it finishes you get a report and must call merge_child to land its work (or archive_child to close it). Scheduled runs and mode=loop default to standalone"}
  },
  "required": ["title", "prompt"]
}`)

type createWorkspaceTool struct{}

func (createWorkspaceTool) Name() string { return "create_workspace" }

func (createWorkspaceTool) Description() string {
	return "Run a SUBAGENT: this is how subagents work in Recur. Creates a NEW workspace (fresh git " +
		"worktree from the base branch) and runs prompt on its first chat tab, or as a loop when mode " +
		"is loop. Use to fan work out into " +
		"independent workspaces, one per task. Give each a stable, descriptive title: a live " +
		"workspace in the project with the same title is returned instead of a duplicate, so " +
		"repeating the call is safe. Model and flags follow Settings (this chat vs that mode's default); Simplify follows this chat. The new " +
		"workspace runs on its own; this chat does not wait for it. Each call starts an agent " +
		"run — one child per distinct task, not extras. Prefer a child for anything beyond a very " +
		"simple change. By default the workspace is a subagent (child) " +
		"of this one: it starts from this branch. When it finishes you get a report; call " +
		"merge_child to land its commits here, or archive_child to close it without merging. " +
		"Pass standalone=true for independent work that should ship on its own."
}

func (createWorkspaceTool) Schema() json.RawMessage { return schemaCreateWorkspace }

func (createWorkspaceTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		Title      string `json:"title"`
		Prompt     string `json:"prompt"`
		ProjectID  string `json:"project_id"`
		BaseBranch string `json:"base_branch"`
		Mode       string `json:"mode"`
		LoopID     string `json:"loop_id"`
		PolicyID   string `json:"policy_id"`
		Standalone *bool  `json:"standalone"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.Title) == "" || strings.TrimSpace(in.Prompt) == "" {
		return "", fmt.Errorf("create_workspace: title and prompt are required")
	}
	return h.CreateWorkspace(c.WorkspaceID, c.AgentCode, CreateWorkspaceRequest{
		ProjectID: in.ProjectID, Title: in.Title, Prompt: in.Prompt, BaseBranch: in.BaseBranch,
		Mode: in.Mode, LoopID: in.LoopID, PolicyID: in.PolicyID, Standalone: in.Standalone,
	}), nil
}
