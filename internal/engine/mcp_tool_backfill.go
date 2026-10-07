package engine

import (
	"encoding/json"
	"strings"
	"time"

	"recur/internal/mcp"
)

// mcpArgsBackfillDelay retries transcript arg patch after the provider's
// item/started mirror lands (often after the MCP HTTP handler returns).
const mcpArgsBackfillDelay = 150 * time.Millisecond

// NoteToolArgs implements mcp.Reporter: backfill empty transcript tool_use
// args from the real tools/call payload (Codex mcpToolCall items often store {}).
func (r engineReporter) NoteToolArgs(workspaceID, agentCode, toolName string, args json.RawMessage) {
	r.eng.noteMCPToolArgs(workspaceID, agentCode, toolName, args)
}

func (e *Engine) noteMCPToolArgs(workspaceID, agentCode, toolName string, args json.RawMessage) {
	if e == nil || strings.TrimSpace(toolName) == "" {
		return
	}
	raw := json.RawMessage(append([]byte(nil), args...))
	if !mcpArgsWorthBackfill(raw) {
		return
	}
	e.backfillMCPToolInput(workspaceID, agentCode, toolName, raw)
	go func() {
		time.Sleep(mcpArgsBackfillDelay)
		e.backfillMCPToolInput(workspaceID, agentCode, toolName, raw)
	}()
}

func mcpArgsWorthBackfill(args json.RawMessage) bool {
	cur := strings.TrimSpace(string(args))
	return cur != "" && cur != "{}" && cur != "null"
}

// backfillMCPToolInput patches the newest matching empty tool_use in the
// agent's session and publishes session.tool_input for the live row.
func (e *Engine) backfillMCPToolInput(workspaceID, agentCode, toolName string, args json.RawMessage) {
	if e == nil || e.Store == nil || !mcpArgsWorthBackfill(args) {
		return
	}
	bare := strings.TrimSpace(toolName)
	if bare == "" {
		return
	}
	a, ok := e.Store.Agent(workspaceID, agentCode)
	if !ok || a.SessionID == "" {
		return
	}
	tu, ok := e.Store.LatestToolUseByNameSuffix(a.SessionID, "__"+bare)
	if !ok {
		tu, ok = e.Store.LatestToolUseByNameSuffix(a.SessionID, bare)
	}
	if !ok {
		return
	}
	if mcpArgsWorthBackfill(tu.Input) {
		return
	}
	_ = e.Store.PatchToolUseInput(tu.ID, args)
	toolID := tu.ProviderToolUseID
	if toolID == "" {
		toolID = tu.ID
	}
	fullName := "mcp__recur__" + bare
	if tu.Name != "" {
		fullName = tu.Name
	}
	e.Bus.Publish(Event{Name: "session.tool_input", Data: map[string]any{
		"workspaceId": workspaceID,
		"code":        agentCode,
		"id":          toolID,
		"input":       string(args),
		"toolInput":   summarizeToolInput(fullName, args),
	}})
}

// Ensure engineReporter stays a mcp.Reporter as NoteToolArgs is added.
var _ mcp.Reporter = engineReporter{}
