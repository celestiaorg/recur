package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaDone = json.RawMessage(`{
  "type": "object",
  "properties": {
    "summary": {"type": "string", "description": "One line: what this turn did"},
    "notes": {"type": "string", "description": "Optional handoff: deviations from the plan, concerns, unfinished work, steps to split. Empty when nothing deviated."}
  },
  "required": ["summary"]
}`)

type doneTool struct{}

func (doneTool) Name() string { return "done" }

func (doneTool) Description() string {
	return "Signal that this loop-specialist turn is complete and hand back to the Maestro. " +
		"Provide a short summary of what you did. Use notes for the handoff the Reviewer and " +
		"Maestro cannot see from the diff: deviations from the plan, concerns, work you could " +
		"not finish, or a step that should be split. Leave notes empty when nothing deviated. " +
		"Fails if any findings are still open — close each with update_finding first. " +
		"Not for interactive human chat turns."
}

func (doneTool) Schema() json.RawMessage { return schemaDone }

func (doneTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		Summary string `json:"summary"`
		Notes   string `json:"notes"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.Summary) == "" {
		return "", fmt.Errorf("done: summary is required")
	}
	if err := h.Done(c.WorkspaceID, c.AgentCode, in.Summary, in.Notes); err != nil {
		return "", err
	}
	return "Acknowledged — turn marked done.", nil
}
