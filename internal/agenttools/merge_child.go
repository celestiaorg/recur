package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaMergeChild = json.RawMessage(`{
  "type": "object",
  "properties": {
    "workspace_id": {"type": "string", "description": "Id of the child workspace to merge into this worktree"}
  },
  "required": ["workspace_id"]
}`)

type mergeChildTool struct{}

func (mergeChildTool) Name() string { return "merge_child" }

func (mergeChildTool) Description() string {
	return "Land a finished subagent's commits: merge its child workspace branch --no-ff into this " +
		"worktree and archive it. Commit your own changes here first. On conflicts, resolve the " +
		"markers, git commit, and call it again to archive. Refused while the child is still running."
}

func (mergeChildTool) Schema() json.RawMessage { return schemaMergeChild }

func (mergeChildTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}
	var in struct {
		WorkspaceID string `json:"workspace_id"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.WorkspaceID) == "" {
		return "", fmt.Errorf("merge_child: workspace_id is required")
	}
	return h.MergeChild(c.WorkspaceID, c.AgentCode, strings.TrimSpace(in.WorkspaceID)), nil
}
