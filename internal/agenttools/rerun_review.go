package agenttools

import (
	"encoding/json"
)

type rerunReviewTool struct{}

func (rerunReviewTool) Name() string { return "rerun_review" }

func (rerunReviewTool) Description() string {
	return "Dispatch a fresh Reviewer pass over the loop's current worktree. Findings are recorded " +
		"like any review and gate the PR as usual."
}

func (rerunReviewTool) Schema() json.RawMessage {
	return json.RawMessage(`{"type":"object","properties":{}}`)
}

func (rerunReviewTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct{}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	return h.RerunReview(c.WorkspaceID, c.AgentCode), nil
}
