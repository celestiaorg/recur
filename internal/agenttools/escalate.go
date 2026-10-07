package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaEscalate = json.RawMessage(`{
  "type": "object",
  "properties": {
    "question": {"type": "string", "description": "The scoped decision you need from the human"},
    "context": {"type": "string", "description": "One-read context: what you built, what's ambiguous, what you considered"},
    "options": {
      "type": "array",
      "description": "2-4 reasoned options, each with a short trade-off",
      "items": {
        "type": "object",
        "properties": {
          "text": {"type": "string", "description": "The option, e.g. 'Reject with 429'"},
          "tag": {"type": "string", "description": "Short label, e.g. simplest / smoothest / recommended"},
          "desc": {"type": "string", "description": "The trade-off of this option"}
        },
        "required": ["text"]
      }
    }
  },
  "required": ["question"]
}`)

type escalateTool struct{}

func (escalateTool) Name() string { return "escalate" }

func (escalateTool) Description() string {
	return "Pause the loop and ask the human a scoped decision you should NOT guess on (genuine " +
		"ambiguity, not laziness). Provide a clear question, 2-4 reasoned options each with a " +
		"short trade-off, and one-read context: what you built, what's ambiguous, what you " +
		"considered. The loop blocks until the human answers, then resumes with their guidance."
}

func (escalateTool) Schema() json.RawMessage { return schemaEscalate }

func (escalateTool) Execute(c Call) (string, error) {
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
		return "", fmt.Errorf("escalate: question is required")
	}
	opts := make([]EscalationOption, 0, len(in.Options))
	for _, o := range in.Options {
		if strings.TrimSpace(o.Text) == "" {
			return "", fmt.Errorf("escalate: each option needs a non-empty text")
		}
		opts = append(opts, EscalationOption{Text: o.Text, Tag: o.Tag, Desc: o.Desc})
	}
	h.Escalate(c.WorkspaceID, c.AgentCode, EscalationRequest{
		Question: in.Question, Options: opts, Context: in.Context,
	})
	return "Escalation raised — the loop is paused until the human answers.", nil
}
