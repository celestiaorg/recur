package agenttools

import (
	"encoding/json"
	"fmt"
)

var schemaReportResult = json.RawMessage(`{
  "type": "object",
  "properties": {
    "passed": {"type": "boolean", "description": "Whether the tests/checks passed"},
    "summary": {"type": "string", "description": "Short summary of the result"}
  },
  "required": ["passed"]
}`)

type reportResultTool struct{}

func (reportResultTool) Name() string { return "report_result" }

func (reportResultTool) Description() string {
	return "Report a test/verification result: whether it passed, with a short summary."
}

func (reportResultTool) Schema() json.RawMessage { return schemaReportResult }

func (reportResultTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	// passed is required and must be distinguishable from omitted, so it is a
	// pointer; summary is optional. Strict-unmarshal the whole shape so a typo'd
	// field is rejected rather than silently dropped.
	var in struct {
		Passed  *bool  `json:"passed"`
		Summary string `json:"summary"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if in.Passed == nil {
		return "", fmt.Errorf("report_result: passed is required")
	}
	h.ReportResult(c.WorkspaceID, c.AgentCode, *in.Passed, in.Summary)
	verb := "failed"
	if *in.Passed {
		verb = "passed"
	}
	return "Recorded result: " + verb + ".", nil
}
