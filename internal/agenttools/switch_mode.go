package agenttools

import (
	"encoding/json"
	"strings"
)

var schemaSwitchMode = json.RawMessage(`{
  "type": "object",
  "properties": {
    "mode": {"type": "string", "enum": ["ask", "plan", "agent", "review"]}
  },
  "required": ["mode"]
}`)

type switchModeTool struct{}

func (switchModeTool) Name() string { return "switch_mode" }

func (switchModeTool) Description() string {
	return "Switch this interactive chat's agent mode. Privilege-safe: agent may switch to " +
		"ask/plan/review; ask/plan/review may switch among each other; never upgrade " +
		"ask/plan/review to agent (ask the user to flip the dock instead). Not available on " +
		"Maestro. A successful switch remounts this turn immediately with the new tool policy. " +
		"Stop after the call and continue on the remounted turn."
}

func (switchModeTool) Schema() json.RawMessage { return schemaSwitchMode }

func (switchModeTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		Mode string `json:"mode"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	return h.SwitchMode(c.WorkspaceID, c.AgentCode, strings.TrimSpace(in.Mode)), nil
}

func (switchModeTool) Halts() bool { return true }
