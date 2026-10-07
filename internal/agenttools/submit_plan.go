package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaSubmitPlan = json.RawMessage(`{
  "type": "object",
  "properties": {
    "summary": {"type": "string", "description": "Short headline of what this change does and why (one or two sentences)"},
    "body": {"type": "string", "description": "The FULL detailed plan as markdown: relevant files, approach, edge cases, and a definition of done. Other agents execute from this, not from chat. Do not leave the detail only in the conversation."},
    "steps": {
      "type": "array",
      "description": "Ordered, concrete checklist — each one actionable sentence naming the file/function and the change. A step may be a string, or an object {title, files, depends_on}: files = repo-relative paths the step touches; depends_on = ids (S1, S2, … by position in this list) that must finish first. Steps with disjoint files and no unmet depends_on can run in parallel.",
      "items": {"anyOf": [
        {"type": "string"},
        {"type": "object", "properties": {
          "title": {"type": "string"},
          "files": {"type": "array", "items": {"type": "string"}},
          "depends_on": {"type": "array", "items": {"type": "string"}}
        }, "required": ["title"]}
      ]}
    },
    "attachments": {
      "type": "array",
      "description": "Optional ids of artifacts (from create_artifact) or user-attached images (⟦attachment:id⟧) that belong to this plan",
      "items": {"type": "string"}
    }
  },
  "required": ["summary","body","steps"]
}`)

type submitPlanTool struct{}

func (submitPlanTool) Name() string { return "submit_plan" }

func (submitPlanTool) Description() string {
	return "File the STRUCTURED plan for this task. Put the FULL detailed plan in body (relevant " +
		"files, approach, edge cases, definition of done) — that is what other agents execute " +
		"from, not the chat transcript. Also provide a short summary headline, an ordered " +
		"checklist of concrete steps (each one actionable sentence naming the file/function and " +
		"the change), and optional attachments (artifact ids from create_artifact, or user image " +
		"ids from ⟦attachment:id⟧). The engine persists this as the workspace's plan artifact — " +
		"the human reviews, edits, and approves it in the Plan panel."
}

func (submitPlanTool) Schema() json.RawMessage { return schemaSubmitPlan }

func (submitPlanTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		Summary     string            `json:"summary"`
		Body        string            `json:"body"`
		Steps       []json.RawMessage `json:"steps"`
		Attachments []string          `json:"attachments"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.Summary) == "" {
		return "", fmt.Errorf("submit_plan: summary is required")
	}
	if strings.TrimSpace(in.Body) == "" {
		return "", fmt.Errorf("submit_plan: body is required — file the full detailed plan, not just a headline")
	}
	steps, meta, err := parsePlanSteps(in.Steps)
	if err != nil {
		return "", err
	}
	if len(steps) == 0 {
		return "", fmt.Errorf("submit_plan: at least one non-empty step is required")
	}
	atts := make([]string, 0, len(in.Attachments))
	seen := map[string]bool{}
	for _, id := range in.Attachments {
		t := strings.TrimSpace(id)
		if t == "" || seen[t] {
			continue
		}
		seen[t] = true
		atts = append(atts, t)
		if len(atts) >= 20 {
			break
		}
	}
	return h.SubmitPlan(c.WorkspaceID, c.AgentCode, PlanSubmission{
		Summary: in.Summary, Body: in.Body, Steps: steps, StepMeta: meta, AttachmentIDs: atts,
	}), nil
}

// parsePlanSteps accepts each step as a string or {title, files, depends_on}.
// Blank titles are dropped; meta stays aligned with the returned titles.
func parsePlanSteps(raw []json.RawMessage) (titles []string, meta []PlanStepMeta, err error) {
	for i, r := range raw {
		var title string
		if jerr := json.Unmarshal(r, &title); jerr == nil {
			if t := strings.TrimSpace(title); t != "" {
				titles = append(titles, t)
				meta = append(meta, PlanStepMeta{})
			}
			continue
		}
		var obj struct {
			Title     string   `json:"title"`
			Files     []string `json:"files"`
			DependsOn []string `json:"depends_on"`
		}
		if jerr := strictUnmarshal(r, &obj); jerr != nil {
			return nil, nil, fmt.Errorf("submit_plan: steps[%d] must be a string or {title, files, depends_on}: %v", i, jerr)
		}
		t := strings.TrimSpace(obj.Title)
		if t == "" {
			continue
		}
		titles = append(titles, t)
		meta = append(meta, PlanStepMeta{Files: trimList(obj.Files), DependsOn: trimList(obj.DependsOn), Explicit: true})
	}
	return titles, meta, nil
}

func trimList(in []string) []string {
	var out []string
	for _, s := range in {
		if t := strings.TrimSpace(s); t != "" {
			out = append(out, t)
		}
	}
	return out
}
