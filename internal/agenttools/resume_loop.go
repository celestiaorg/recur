package agenttools

import (
	"encoding/json"
)

type resumeLoopTool struct{}

func (resumeLoopTool) Name() string { return "resume_loop" }

func (resumeLoopTool) Description() string {
	return "RESTART a STOPPED or FAILED loop over its existing worktree. The run continues from the " +
		"worktree's current state, steered by the recorded plan. A running loop does not need " +
		"this; a loop settled at pr_open wants send_back instead."
}

func (resumeLoopTool) Schema() json.RawMessage {
	return json.RawMessage(`{"type":"object","properties":{}}`)
}

func (resumeLoopTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct{}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	return h.ResumeLoop(c.WorkspaceID, c.AgentCode), nil
}
