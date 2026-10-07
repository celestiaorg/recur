package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaDispatchSpecialist = json.RawMessage(`{
  "type": "object",
  "properties": {
    "role": {"type": "string", "enum": ["reviewer","security"], "description": "Which specialist to run"},
    "instruction": {"type": "string", "description": "Concrete instruction for the specialist"}
  },
  "required": ["role","instruction"]
}`)

type dispatchSpecialistTool struct{}

func (dispatchSpecialistTool) Name() string { return "dispatch_specialist" }

func (dispatchSpecialistTool) Description() string {
	return "Run one bounded specialist turn over the loop's worktree with a concrete instruction. " +
		"role is 'reviewer' (code review, findings) or 'security' (security-focused review). The " +
		"turn is governed like any agent turn; findings are recorded."
}

func (dispatchSpecialistTool) Schema() json.RawMessage { return schemaDispatchSpecialist }

func (dispatchSpecialistTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		Role        string `json:"role"`
		Instruction string `json:"instruction"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	role := strings.ToLower(strings.TrimSpace(in.Role))
	if role != "reviewer" && role != "security" {
		return "", fmt.Errorf("dispatch_specialist: role must be reviewer or security, got %q", in.Role)
	}
	if strings.TrimSpace(in.Instruction) == "" {
		return "", fmt.Errorf("dispatch_specialist: instruction is required")
	}
	return h.DispatchSpecialist(c.WorkspaceID, c.AgentCode, role, in.Instruction), nil
}
