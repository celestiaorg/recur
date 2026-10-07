package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaChildID = json.RawMessage(`{
  "type": "object",
  "properties": {
    "workspace_id": {"type": "string", "description": "Id of the subagent child workspace (from create_workspace or its report)"}
  },
  "required": ["workspace_id"]
}`)

func childIDArg(c Call, tool string) (string, error) {
	var in struct {
		WorkspaceID string `json:"workspace_id"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.WorkspaceID) == "" {
		return "", fmt.Errorf("%s: workspace_id is required", tool)
	}
	return strings.TrimSpace(in.WorkspaceID), nil
}

var schemaMessageChild = json.RawMessage(`{
  "type": "object",
  "properties": {
    "workspace_id": {"type": "string", "description": "Id of the child workspace (from create_workspace or its report)"},
    "message": {"type": "string", "description": "Instruction for the child (steers a running turn, or starts a new turn on a finished child)"}
  },
  "required": ["workspace_id", "message"]
}`)

func childIDAndMessage(c Call, tool string) (id, message string, err error) {
	var in struct {
		WorkspaceID string `json:"workspace_id"`
		Message     string `json:"message"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", "", err
	}
	id = strings.TrimSpace(in.WorkspaceID)
	message = strings.TrimSpace(in.Message)
	if id == "" || message == "" {
		return "", "", fmt.Errorf("%s: workspace_id and message are required", tool)
	}
	return id, message, nil
}

type readChildTool struct{}

func (readChildTool) Name() string { return "read_child" }

func (readChildTool) Description() string {
	return "Read a subagent's report: its state, final reply, handoff notes, findings, plan, " +
		"committed and uncommitted changes against this branch with the diff, and artifact ids " +
		"(open those with read_artifact workspace_id). Works on running, finished, and archived children."
}

func (readChildTool) Schema() json.RawMessage { return schemaChildID }

func (readChildTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}
	id, err := childIDArg(c, "read_child")
	if err != nil {
		return "", err
	}
	return h.ReadChild(c.WorkspaceID, c.AgentCode, id), nil
}

type archiveChildTool struct{}

func (archiveChildTool) Name() string { return "archive_child" }

func (archiveChildTool) Description() string {
	return "Close a finished subagent without merging: archives its child workspace once you have " +
		"what you need from its report (research, a review, a spike). Its worktree and branch are " +
		"deleted, so any unmerged commits are gone; merge_child first if you want them. Refused " +
		"while the child is still running."
}

func (archiveChildTool) Schema() json.RawMessage { return schemaChildID }

func (archiveChildTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}
	id, err := childIDArg(c, "archive_child")
	if err != nil {
		return "", err
	}
	return h.ArchiveChild(c.WorkspaceID, c.AgentCode, id), nil
}

type messageChildTool struct{}

func (messageChildTool) Name() string { return "message_child" }

func (messageChildTool) Description() string {
	return "Send a message to a child workspace. A running child is steered mid-turn: returns once it has accepted the message (may take until its next tool call) and it continues. " +
		"A finished (not merged or archived) child starts a new turn with the message and reports back again when it finishes. Works with any harness."
}

func (messageChildTool) Schema() json.RawMessage { return schemaMessageChild }

func (messageChildTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}
	id, message, err := childIDAndMessage(c, "message_child")
	if err != nil {
		return "", err
	}
	return h.MessageChild(c.WorkspaceID, c.AgentCode, id, message), nil
}
