package agenttools

import (
	"encoding/json"
	"fmt"
)

var schemaExtendIterations = json.RawMessage(`{
  "type": "object",
  "properties": {
    "iterations": {"type": "integer", "minimum": 1, "description": "How many extra fix iterations to request"}
  },
  "required": ["iterations"]
}`)

type extendIterationsTool struct{}

func (extendIterationsTool) Name() string { return "extend_iterations" }

func (extendIterationsTool) Description() string {
	return "Grant the RUNNING loop extra fix iterations toward convergence. The engine clamps the " +
		"grant to the autonomy level's extension allowance (rail #4 arithmetic is the engine's)."
}

func (extendIterationsTool) Schema() json.RawMessage { return schemaExtendIterations }

func (extendIterationsTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		Iterations int `json:"iterations"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if in.Iterations < 1 {
		return "", fmt.Errorf("extend_iterations: iterations must be >= 1")
	}
	return h.ExtendIterations(c.WorkspaceID, c.AgentCode, in.Iterations), nil
}
