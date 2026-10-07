package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"

	"recur/internal/domain"
)

var schemaUpdatePlanStep = json.RawMessage(`{
  "type": "object",
  "properties": {
    "step_id": {"type": "string", "description": "The plan step id to update"},
    "status": {"type": "string", "enum": ["pending","doing","done","skipped"], "description": "The step's new status"},
    "note": {"type": "string", "description": "Optional short note (e.g. why a step was skipped)"}
  },
  "required": ["step_id","status"]
}`)

type updatePlanStepTool struct{}

func (updatePlanStepTool) Name() string { return "update_plan_step" }

func (updatePlanStepTool) Description() string {
	return "Update one plan step's status (pending/doing/done/skipped) by its step id, with an " +
		"optional short note. Use it as work advances so plan progress stays honest."
}

func (updatePlanStepTool) Schema() json.RawMessage { return schemaUpdatePlanStep }

func (updatePlanStepTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		StepID string `json:"step_id"`
		Status string `json:"status"`
		Note   string `json:"note"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.StepID) == "" {
		return "", fmt.Errorf("update_plan_step: step_id is required")
	}
	switch strings.ToLower(strings.TrimSpace(in.Status)) {
	case domain.StepPending, domain.StepDoing, domain.StepDone, domain.StepSkipped:
	default:
		return "", fmt.Errorf("update_plan_step: status must be pending/doing/done/skipped, got %q", in.Status)
	}
	return h.UpdatePlanStep(c.WorkspaceID, c.AgentCode, in.StepID,
		strings.ToLower(strings.TrimSpace(in.Status)), in.Note), nil
}
