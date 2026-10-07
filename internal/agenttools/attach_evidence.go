package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaAttachEvidence = json.RawMessage(`{
  "type": "object",
  "properties": {
    "step": {"type": "string", "description": "What you did/observed in the running app for this step"},
    "screenshot_path": {"type": "string", "description": "Absolute path to a screenshot captured for this step (optional)"},
    "passed": {"type": "boolean", "description": "Whether this verification step passed"}
  },
  "required": ["step","passed"]
}`)

type attachEvidenceTool struct{}

func (attachEvidenceTool) Name() string { return "attach_evidence" }

func (attachEvidenceTool) Description() string {
	return "Record one browser-verification evidence step: a short description of what you " +
		"did/observed in the running app, the absolute path to a screenshot you captured, and " +
		"whether the step passed. The engine collects these to decide whether the verify passed; " +
		"a failing step blocks opening a PR (rail #1)."
}

func (attachEvidenceTool) Schema() json.RawMessage { return schemaAttachEvidence }

func (attachEvidenceTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	// passed is required and must be distinguishable from omitted, so it is a
	// pointer; screenshot_path is optional. Strict-unmarshal so a typo'd field is
	// rejected rather than silently dropped.
	var in struct {
		Step           string `json:"step"`
		ScreenshotPath string `json:"screenshot_path"`
		Passed         *bool  `json:"passed"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.Step) == "" {
		return "", fmt.Errorf("attach_evidence: step is required")
	}
	if in.Passed == nil {
		return "", fmt.Errorf("attach_evidence: passed is required")
	}
	h.AttachEvidence(c.WorkspaceID, c.AgentCode, Evidence{
		Step: in.Step, ScreenshotPath: in.ScreenshotPath, Passed: *in.Passed,
	})
	verb := "failed"
	if *in.Passed {
		verb = "passed"
	}
	return "Recorded evidence (" + verb + ").", nil
}
