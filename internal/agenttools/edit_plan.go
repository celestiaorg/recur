package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

type editPlanTool struct{}

func (editPlanTool) Name() string { return "edit_plan" }

func (editPlanTool) Description() string {
	return "Rewrite the loop's plan (its definition of done). The engine persists it, logs a " +
		"plan-change deviation, and steers subsequent specialist turns with the new plan. Provide " +
		"the complete new plan text, not a diff."
}

func (editPlanTool) Schema() json.RawMessage { return schemaSummaryOnly("plan") }

func (editPlanTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		Plan string `json:"plan"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.Plan) == "" {
		return "", fmt.Errorf("edit_plan: plan is required")
	}
	return h.EditPlan(c.WorkspaceID, c.AgentCode, in.Plan), nil
}
