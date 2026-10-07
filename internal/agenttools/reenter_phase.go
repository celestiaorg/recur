package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaPhaseID = json.RawMessage(`{
  "type": "object",
  "properties": {
    "phase_id": {"type": "string", "description": "Phase id on the engaged loop"}
  },
  "required": ["phase_id"]
}`)

type reenterPhaseTool struct{}

func (reenterPhaseTool) Name() string { return "reenter_phase" }

func (reenterPhaseTool) Description() string {
	return "Idle only (no live walk: stopped/failed/settled, or paused with no goroutine). " +
		"Start a new walk from a graph phase on the loop's reenter list (usually converge). " +
		"Not for a live walk — including a checkpoint pause (Resume continues that) or a " +
		"round-cap escalation (extend_rounds)."
}

func (reenterPhaseTool) Schema() json.RawMessage { return schemaPhaseID }

func (reenterPhaseTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		PhaseID string `json:"phase_id"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.PhaseID) == "" {
		return "", fmt.Errorf("reenter_phase: phase_id is required")
	}
	return h.ReenterPhase(c.WorkspaceID, c.AgentCode, in.PhaseID), nil
}
