package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaRenameBranch = json.RawMessage(`{
  "type": "object",
  "properties": {
    "name": {"type": "string", "description": "Kebab-case branch topic; the configured prefix is applied if omitted"}
  },
  "required": ["name"]
}`)

type renameBranchTool struct{}

func (renameBranchTool) Name() string { return "rename_branch" }

func (renameBranchTool) Description() string {
	return "Rename this workspace's git branch to a kebab-case topic (e.g. add-dark-mode). The " +
		"configured branch prefix (default recur/) is applied if omitted. Allowed during planning; " +
		"refused while a loop is coding, reviewing, or verifying. No confirmation."
}

func (renameBranchTool) Schema() json.RawMessage { return schemaRenameBranch }

func (renameBranchTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		Name string `json:"name"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.Name) == "" {
		return "", fmt.Errorf("rename_branch: name is required")
	}
	return h.RenameBranch(c.WorkspaceID, c.AgentCode, in.Name), nil
}
