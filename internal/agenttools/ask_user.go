package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaAskUser = json.RawMessage(`{
  "type": "object",
  "properties": {
    "question": {"type": "string", "description": "The question for the human"},
    "context": {"type": "string", "description": "Short one-read context"},
    "options": {
      "type": "array",
      "description": "Optional 2-4 choices (shortcuts; free-text is always available)",
      "items": {
        "type": "object",
        "properties": {
          "text": {"type": "string"},
          "tag": {"type": "string"},
          "desc": {"type": "string"}
        },
        "required": ["text"]
      }
    }
  },
  "required": ["question"]
}`)

type askUserTool struct{}

func (askUserTool) Name() string { return "ask_user" }

func (askUserTool) Description() string {
	return "Ask the human a clarifying question. Use for interactive chats (including Maestro) when " +
		"you need a decision without pausing the whole workspace. Provide a clear question and " +
		"optional 2-4 options. This ends your current turn — the chat goes idle and their answer " +
		"arrives later as a user message. An inbox notification is raised. Do NOT use escalate " +
		"from interactive chats — that is for non-interactive loop agents only."
}

func (askUserTool) Schema() json.RawMessage { return schemaAskUser }

func (askUserTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		Question string `json:"question"`
		Context  string `json:"context"`
		Options  []struct {
			Text string `json:"text"`
			Tag  string `json:"tag"`
			Desc string `json:"desc"`
		} `json:"options"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.Question) == "" {
		return "", fmt.Errorf("ask_user: question is required")
	}
	opts := make([]EscalationOption, 0, len(in.Options))
	for _, o := range in.Options {
		if strings.TrimSpace(o.Text) == "" {
			return "", fmt.Errorf("ask_user: each option needs non-empty text")
		}
		opts = append(opts, EscalationOption{Text: o.Text, Tag: o.Tag, Desc: o.Desc})
	}
	return h.AskUser(c.WorkspaceID, c.AgentCode, AskUserRequest{
		Question: in.Question, Context: in.Context, Options: opts,
	}), nil
}

func (askUserTool) Halts() bool { return true }
