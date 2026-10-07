package agenttools

import (
	"encoding/json"
)

type listArtifactsTool struct{}

func (listArtifactsTool) Name() string { return "list_artifacts" }

func (listArtifactsTool) Description() string {
	return "List artifacts created for this workspace (id, title, kind, path, createdAt)."
}

func (listArtifactsTool) Schema() json.RawMessage {
	return json.RawMessage(`{"type":"object","properties":{}}`)
}

func (listArtifactsTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	list, err := h.ListArtifacts(c.WorkspaceID, c.AgentCode)
	if err != nil {
		return "", err
	}
	artifacts := any(list)
	if list == nil {
		artifacts = []any{}
	}
	b, _ := json.Marshal(map[string]any{"count": len(list), "artifacts": artifacts})
	return string(b), nil
}
