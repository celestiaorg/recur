package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

type planTool struct{}

func (planTool) Name() string { return "plan" }

func (planTool) Description() string {
	return "Record a short plan summary for the task."
}

func (planTool) Schema() json.RawMessage { return schemaSummaryOnly("summary") }

func (planTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		Summary string `json:"summary"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.Summary) == "" {
		return "", fmt.Errorf("plan: summary is required")
	}
	h.Plan(c.WorkspaceID, c.AgentCode, in.Summary)
	return "Plan recorded.", nil
}
