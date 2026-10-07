package agenttools

import "encoding/json"

var schemaExtendRounds = json.RawMessage(`{
  "type": "object",
  "properties": {
    "n": {"type": "integer", "minimum": 1, "description": "Extra rounds; default and clamp are the policy extend_rounds"}
  }
}`)

type extendRoundsTool struct{}

func (extendRoundsTool) Name() string { return "extend_rounds" }

func (extendRoundsTool) Description() string {
	return "Grant extra converge rounds. Legal only while the live converge is still " +
		"escalated at the round cap — not agent questions, walk-step waits, or a stopped loop. " +
		"After the loop has stopped, use reenter_phase instead. Optional n defaults to the policy value."
}

func (extendRoundsTool) Schema() json.RawMessage { return schemaExtendRounds }

func (extendRoundsTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		N int `json:"n"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	return h.ExtendRounds(c.WorkspaceID, c.AgentCode, in.N), nil
}
