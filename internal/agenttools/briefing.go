package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"

	"recur/internal/domain"
)

func requireAppCoordinator(c Call) error {
	if c.AgentCode != domain.AppAgentCode {
		return fmt.Errorf("this tool is only available to the Ask Recur coordinator")
	}
	return nil
}

type listWorkspaceBriefsTool struct{}

func (listWorkspaceBriefsTool) Name() string { return "list_workspace_briefs" }
func (listWorkspaceBriefsTool) Description() string {
	return "List the current Briefing overview: every workspace that needs attention, " +
		"is working, or can be picked up, with deterministic Now/Next copy and routes. " +
		"Always re-query before a factual status claim — the chat transcript is not current truth. " +
		"App coordinator only."
}
func (listWorkspaceBriefsTool) Schema() json.RawMessage {
	return json.RawMessage(`{"type":"object","properties":{}}`)
}
func (listWorkspaceBriefsTool) Execute(c Call) (string, error) {
	if err := requireAppCoordinator(c); err != nil {
		return "", err
	}
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}
	snap := h.ListWorkspaceBriefs()
	b, err := json.Marshal(snap)
	if err != nil {
		return "", fmt.Errorf("list_workspace_briefs: %w", err)
	}
	return string(b), nil
}

type getWorkspaceContextTool struct{}

func (getWorkspaceContextTool) Name() string { return "get_workspace_context" }
func (getWorkspaceContextTool) Description() string {
	return "Get bounded detail for one workspace: plan steps, active agents, open asks, " +
		"recent chat headers, PR state, and a short conversation excerpt. App coordinator only."
}
func (getWorkspaceContextTool) Schema() json.RawMessage {
	return json.RawMessage(`{
  "type": "object",
  "properties": {
    "workspace_id": {"type": "string", "description": "Workspace id from list_workspace_briefs"}
  },
  "required": ["workspace_id"]
}`)
}
func (getWorkspaceContextTool) Execute(c Call) (string, error) {
	if err := requireAppCoordinator(c); err != nil {
		return "", err
	}
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}
	var in struct {
		WorkspaceID string `json:"workspace_id"`
	}
	if err := json.Unmarshal(c.Args, &in); err != nil {
		return "", fmt.Errorf("get_workspace_context: invalid arguments")
	}
	id := strings.TrimSpace(in.WorkspaceID)
	if id == "" {
		return "", fmt.Errorf("get_workspace_context: workspace_id is required")
	}
	ctx, err := h.WorkspaceContext(id)
	if err != nil {
		return "", err
	}
	b, err := json.Marshal(ctx)
	if err != nil {
		return "", fmt.Errorf("get_workspace_context: %w", err)
	}
	return string(b), nil
}

type presentWorkspaceTool struct{}

func (presentWorkspaceTool) Name() string { return "present_workspace" }
func (presentWorkspaceTool) Description() string {
	return "Pin a workspace, chat, question, or PR card in the Ask Recur transcript " +
		"for the human to open. Does not switch the visible UI. target is workspace:<id> " +
		"(optional :plan, :findings, or :review), chat:<workspaceId>:<code>, question:<id>, or pr:<id>. " +
		"Validated against live state. " +
		"App coordinator only."
}
func (presentWorkspaceTool) Schema() json.RawMessage {
	return json.RawMessage(`{
  "type": "object",
  "properties": {
    "target": {"type": "string", "description": "Navigation target from a brief's primary/secondary action"}
  },
  "required": ["target"]
}`)
}
func (presentWorkspaceTool) Execute(c Call) (string, error) {
	if err := requireAppCoordinator(c); err != nil {
		return "", err
	}
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}
	var in struct {
		Target string `json:"target"`
	}
	if err := json.Unmarshal(c.Args, &in); err != nil {
		return "", fmt.Errorf("present_workspace: invalid arguments")
	}
	return h.PresentWorkspace(strings.TrimSpace(in.Target)), nil
}
