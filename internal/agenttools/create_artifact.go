package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaCreateArtifact = json.RawMessage(`{
  "type": "object",
  "properties": {
    "title": {"type": "string"},
    "kind": {"type": "string", "enum": ["html", "svg", "mermaid", "png", "jpeg", "csv", "json", "md"]},
    "content": {"type": "string", "description": "Text content (or base64 for png/jpeg)"}
  },
  "required": ["title", "kind", "content"]
}`)

type createArtifactTool struct{}

func (createArtifactTool) Name() string { return "create_artifact" }

func (createArtifactTool) Description() string {
	return "Create a standalone analytical artifact (html, svg, mermaid, png, jpeg, csv, json, or " +
		"md) stored under the workspace data dir. Prefer this over dumping large tables into chat " +
		"markdown. HTML may include interactive JS; preview runs it in an opaque sandboxed iframe " +
		"(no parent access). https CDN scripts/styles and https/wss fetch work; http and localhost " +
		"do not; localStorage is in-memory only; self-navigation still possible. Returns the " +
		"artifact id and path."
}

func (createArtifactTool) Schema() json.RawMessage { return schemaCreateArtifact }

func (createArtifactTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		Title   string `json:"title"`
		Kind    string `json:"kind"`
		Content string `json:"content"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.Title) == "" || strings.TrimSpace(in.Kind) == "" {
		return "", fmt.Errorf("create_artifact: title and kind are required")
	}
	a, err := h.CreateArtifact(c.WorkspaceID, c.AgentCode, CreateArtifactRequest{
		Title: in.Title, Kind: in.Kind, Content: in.Content,
	})
	if err != nil {
		return "", err
	}
	b, _ := json.Marshal(a)
	return string(b), nil
}
