package agenttools

import (
	"encoding/json"
	"fmt"
)

var schemaTodo = json.RawMessage(`{"type":"object","properties":{"todos":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"content":{"type":"string"},"status":{"type":"string"}},"required":["id","content","status"]}}},"required":["todos"]}`)

type todoTool struct{}

func (todoTool) Name() string { return "TodoWrite" }
func (todoTool) Description() string {
	return "Replace or merge the session todo list. Each item has id, content, and status (pending|in_progress|completed)."
}
func (todoTool) Schema() json.RawMessage { return schemaTodo }
func (todoTool) Execute(c Call) (string, error) {
	var in struct {
		Todos []TodoItem `json:"todos"`
	}
	if err := json.Unmarshal(c.Args, &in); err != nil {
		return "", fmt.Errorf("invalid TodoWrite args: %w", err)
	}
	if c.Todos == nil {
		return "", fmt.Errorf("todo store unavailable")
	}
	merged := mergeTodos(c.Todos.Get(c.SessionID), in.Todos)
	c.Todos.Put(c.SessionID, merged)
	return todosJSON(merged), nil
}
