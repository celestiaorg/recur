package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaRenameWorkspace = json.RawMessage(`{
  "type": "object",
  "properties": {
    "title": {"type": "string", "description": "Short workspace title (at most 8 words, sentence case)"}
  },
  "required": ["title"]
}`)

type renameWorkspaceTool struct{}

func (renameWorkspaceTool) Name() string { return "rename_workspace" }

func (renameWorkspaceTool) Description() string {
	return "Rename this workspace (sidebar title) to a short topic title (at most 8 words, sentence " +
		"case). Allowed during loops. No confirmation."
}

func (renameWorkspaceTool) Schema() json.RawMessage { return schemaRenameWorkspace }

func (renameWorkspaceTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		Title string `json:"title"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.Title) == "" {
		return "", fmt.Errorf("rename_workspace: title is required")
	}
	return h.RenameWorkspace(c.WorkspaceID, c.AgentCode, in.Title), nil
}
