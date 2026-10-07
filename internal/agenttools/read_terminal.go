package agenttools

import (
	"encoding/json"
	"strings"
)

var schemaReadTerminal = json.RawMessage(`{
  "type": "object",
  "properties": {
    "id": {"type": "string", "description": "Session id from list_terminals; omit to read the focused tab"},
    "tail_lines": {"type": "integer", "description": "Last N lines (default 200, max 2000)"}
  }
}`)

type readTerminalTool struct{}

func (readTerminalTool) Name() string { return "read_terminal" }

func (readTerminalTool) Description() string {
	return "Read recent output from an Observatory terminal (ANSI stripped). id is a session id from " +
		"list_terminals (or omit to read the focused tab). tail_lines defaults to 200 (max 2000). " +
		"Prefer this over guessing that output lives in a worktree file."
}

func (readTerminalTool) Schema() json.RawMessage { return schemaReadTerminal }

func (readTerminalTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		ID        string `json:"id"`
		TailLines int    `json:"tail_lines"`
	}
	raw := strings.TrimSpace(string(c.Args))
	if raw != "" && raw != "null" {
		if err := strictUnmarshal(c.Args, &in); err != nil {
			return "", err
		}
	}
	return h.ReadTerminal(c.WorkspaceID, c.AgentCode, in.ID, in.TailLines)
}
