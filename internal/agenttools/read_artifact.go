package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaReadArtifact = json.RawMessage(`{
  "type": "object",
  "properties": {
    "id": {"type": "string", "description": "Artifact id from list_artifacts"},
    "workspace_id": {"type": "string", "description": "Read from one of this workspace's subagent children instead (id from read_child); default this workspace"}
  },
  "required": ["id"]
}`)

type readArtifactTool struct{}

func (readArtifactTool) Name() string { return "read_artifact" }

func (readArtifactTool) Description() string {
	return "Read one workspace artifact by id (from list_artifacts). Returns JSON with id, title, " +
		"kind, path, and content for text kinds. PNG/JPEG are binary and are not inlined — use " +
		"present_artifact to show them. Prefer this over searching the git worktree or the Recur data dir."
}

func (readArtifactTool) Schema() json.RawMessage { return schemaReadArtifact }

func (readArtifactTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		ID          string `json:"id"`
		WorkspaceID string `json:"workspace_id"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.ID) == "" {
		return "", fmt.Errorf("read_artifact: id is required")
	}
	if child := strings.TrimSpace(in.WorkspaceID); child != "" && child != c.WorkspaceID {
		return h.ReadChildArtifact(c.WorkspaceID, c.AgentCode, child, in.ID)
	}
	return h.ReadArtifact(c.WorkspaceID, c.AgentCode, in.ID)
}
