package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaActivateSkill = json.RawMessage(`{
  "type": "object",
  "properties": {
    "name": {"type": "string", "description": "Skill name from list_skills (e.g. go-code-review)"}
  },
  "required": ["name"]
}`)

type activateSkillTool struct{}

func (activateSkillTool) Name() string { return "activate_skill" }

func (activateSkillTool) Description() string {
	return "Load the full instructions for one Agent Skill by name (from list_skills). Returns the " +
		"skill body wrapped for use in this turn. Prefer this over reading SKILL.md paths " +
		"yourself. Do not re-activate a skill already present as <skill_content> in the user " +
		"message."
}

func (activateSkillTool) Schema() json.RawMessage { return schemaActivateSkill }

func (activateSkillTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		Name string `json:"name"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	name := strings.TrimSpace(in.Name)
	if name == "" {
		return "", fmt.Errorf("activate_skill: name is required")
	}
	return h.ActivateSkill(c.WorkspaceID, c.AgentCode, name)
}
