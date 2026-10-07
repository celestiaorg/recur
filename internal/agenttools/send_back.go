package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

type sendBackTool struct{}

func (sendBackTool) Name() string { return "send_back" }

func (sendBackTool) Description() string {
	return "Send an open PR back into the loop for rework. Use this when the loop has already opened " +
		"a PR (state pr_open) and the human wants corrections, more work, or an update with the " +
		"latest base branch — plan alone cannot restart a finished loop. Provide a concrete " +
		"instruction for the Coder."
}

func (sendBackTool) Schema() json.RawMessage { return schemaSummaryOnly("instruction") }

func (sendBackTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		Instruction string `json:"instruction"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.Instruction) == "" {
		return "", fmt.Errorf("send_back: instruction is required")
	}
	status := h.SendBack(c.WorkspaceID, c.AgentCode, in.Instruction)
	if status == "" {
		status = "Send-back requested."
	}
	return status, nil
}
