package agenttools

import (
	"encoding/json"
)

type listTerminalsTool struct{}

func (listTerminalsTool) Name() string { return "list_terminals" }

func (listTerminalsTool) Description() string {
	return "List this workspace's live Observatory terminal tabs (id, title, cwd, whether it is the " +
		"focused tab). Use when the user mentions the terminal, a build/test error on screen, or " +
		"before read_terminal if you need an id."
}

func (listTerminalsTool) Schema() json.RawMessage {
	return json.RawMessage(`{"type":"object","properties":{}}`)
}

func (listTerminalsTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	list := h.ListTerminals(c.WorkspaceID, c.AgentCode)
	b, _ := json.Marshal(map[string]any{"count": len(list), "terminals": list})
	return string(b), nil
}
