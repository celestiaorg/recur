package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaPermissionRequest = json.RawMessage(`{
  "type": "object",
  "properties": {
    "tool_name": {"type": "string", "description": "The tool awaiting permission"},
    "input": {"type": "object", "description": "The arguments that tool was called with"},
    "tool_use_id": {"type": "string", "description": "Correlates with the assistant's tool_use block"}
  },
  "required": ["tool_name"]
}`)

type permissionRequestTool struct{}

func (permissionRequestTool) Name() string { return "permission_request" }

func (permissionRequestTool) Description() string {
	return "Permission broker for this Recur workspace. Claude Code calls this itself via " +
		"--permission-prompt-tool when a tool needs approval; do not call it directly."
}

func (permissionRequestTool) Schema() json.RawMessage { return schemaPermissionRequest }

func marshalPermissionVerdict(v PermissionVerdict, input json.RawMessage) string {
	if v.Allow {
		out, err := json.Marshal(map[string]any{
			"behavior":     "allow",
			"updatedInput": input,
		})
		if err == nil {
			return string(out)
		}
		// An un-marshalable input cannot be echoed back, and allow without
		// updatedInput is rejected by the CLI, so fail closed.
		v = PermissionVerdict{Reason: "could not encode the approved input"}
	}
	reason := strings.TrimSpace(v.Reason)
	if reason == "" {
		reason = "the human denied this tool call"
	}
	out, _ := json.Marshal(map[string]any{"behavior": "deny", "message": reason})
	return string(out)
}

func (permissionRequestTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		ToolName  string          `json:"tool_name"`
		Input     json.RawMessage `json:"input"`
		ToolUseID string          `json:"tool_use_id"`
	}
	if err := json.Unmarshal(c.Args, &in); err != nil {
		return "", fmt.Errorf("permission_request: %w", err)
	}
	if strings.TrimSpace(in.ToolName) == "" {
		return "", fmt.Errorf("permission_request: tool_name is required")
	}
	if len(in.Input) == 0 {
		in.Input = json.RawMessage(`{}`)
	}

	verdict, err := h.RequestPermission(c.ctx(), c.WorkspaceID, c.AgentCode, PermissionAsk{
		Tool:      in.ToolName,
		Input:     in.Input,
		ToolUseID: in.ToolUseID,
	})
	if err != nil {
		return "", fmt.Errorf("permission_request: %w", err)
	}
	return marshalPermissionVerdict(verdict, in.Input), nil
}
