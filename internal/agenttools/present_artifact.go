package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaPresentArtifact = json.RawMessage(`{
  "type": "object",
  "properties": {
    "id": {"type": "string"}
  },
  "required": ["id"]
}`)

type presentArtifactTool struct{}

func (presentArtifactTool) Name() string { return "present_artifact" }

func (presentArtifactTool) Description() string {
	return "Present one artifact to the user in the Artifacts panel / preview."
}

func (presentArtifactTool) Schema() json.RawMessage { return schemaPresentArtifact }

func (presentArtifactTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		ID string `json:"id"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.ID) == "" {
		return "", fmt.Errorf("present_artifact: id is required")
	}
	return h.PresentArtifact(c.WorkspaceID, c.AgentCode, in.ID), nil
}
