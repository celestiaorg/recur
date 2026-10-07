package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

type runPhaseTool struct{}

func (runPhaseTool) Name() string { return "run_phase" }

func (runPhaseTool) Description() string {
	return "Run an on-demand phase (e.g. security), then resume the parked graph walk."
}

func (runPhaseTool) Schema() json.RawMessage { return schemaPhaseID }

func (runPhaseTool) Execute(c Call) (string, error) {
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
		return "", fmt.Errorf("run_phase: phase_id is required")
	}
	return h.RunPhase(c.WorkspaceID, c.AgentCode, in.PhaseID), nil
}
