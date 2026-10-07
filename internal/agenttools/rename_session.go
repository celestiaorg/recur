package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaRenameSession = json.RawMessage(`{
  "type": "object",
  "properties": {
    "title": {"type": "string"}
  },
  "required": ["title"]
}`)

type renameSessionTool struct{}

func (renameSessionTool) Name() string { return "rename_session" }

func (renameSessionTool) Description() string {
	return "Rename the current chat/session to a short 3–5 word topic title. No confirmation."
}

func (renameSessionTool) Schema() json.RawMessage { return schemaRenameSession }

func (renameSessionTool) Execute(c Call) (string, error) {
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
		return "", fmt.Errorf("rename_session: title is required")
	}
	return h.RenameSession(c.WorkspaceID, c.AgentCode, c.ChatCode, in.Title), nil
}
