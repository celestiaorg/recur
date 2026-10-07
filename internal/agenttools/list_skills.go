package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

type listSkillsTool struct{}

func (listSkillsTool) Name() string { return "list_skills" }

func (listSkillsTool) Description() string {
	return "List available Agent Skills as an <available_skills> catalog (name + description). Call " +
		"this when a task may match a specialized skill, then activate_skill with the skill's " +
		"name to load full instructions."
}

func (listSkillsTool) Schema() json.RawMessage {
	return json.RawMessage(`{"type":"object","properties":{}}`)
}

func (listSkillsTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	list, err := h.ListSkills(c.WorkspaceID, c.AgentCode)
	if err != nil {
		return "", err
	}
	// Agent Skills client shape (agentskills.io): name + description.
	// location omitted — activate_skill loads the body.
	var b strings.Builder
	b.WriteString("<available_skills>\n")
	for _, s := range list {
		b.WriteString("  <skill>\n")
		fmt.Fprintf(&b, "    <name>%s</name>\n", xmlEscapeCatalog(s.Name))
		if s.Description != "" {
			fmt.Fprintf(&b, "    <description>%s</description>\n", xmlEscapeCatalog(s.Description))
		}
		b.WriteString("  </skill>\n")
	}
	b.WriteString("</available_skills>")
	return b.String(), nil
}
