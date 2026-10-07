package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaReportFinding = json.RawMessage(`{
  "type": "object",
  "properties": {
    "file": {"type": "string", "description": "Path of the file the finding is in"},
    "line": {"type": "integer", "description": "1-based start line (0 if not line-specific)"},
    "end_line": {"type": "integer", "description": "Inclusive end line for a range; omit or 0 for a single line"},
    "severity": {"type": "string", "enum": ["info","low","medium","high","critical"], "description": "Finding severity"},
    "message": {"type": "string", "description": "What is wrong and why"},
    "summary": {"type": "string", "description": "Optional markdown shown to the end user in the turn digest; omit to use message"}
  },
  "required": ["file","message","severity"]
}`)

type reportFindingTool struct{}

func (reportFindingTool) Name() string { return "report_finding" }

func (reportFindingTool) Description() string {
	return "Report one structured code-review finding for the diff under review. The engine collects " +
		"these to decide convergence; you cannot approve your own work. Optional summary is " +
		"markdown shown to the end user in the turn digest (defaults to message). Do not restate " +
		"filed findings in your closing reply — the app summarizes still-open filings."
}

func (reportFindingTool) Schema() json.RawMessage { return schemaReportFinding }

func (reportFindingTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		File     string `json:"file"`
		Line     int    `json:"line"`
		EndLine  int    `json:"end_line"`
		Severity string `json:"severity"`
		Message  string `json:"message"`
		Summary  string `json:"summary"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.File) == "" {
		return "", fmt.Errorf("report_finding: file is required")
	}
	if strings.TrimSpace(in.Message) == "" {
		return "", fmt.Errorf("report_finding: message is required")
	}
	sev, ok := parseSeverity(in.Severity)
	if !ok {
		return "", fmt.Errorf("report_finding: severity must be one of info/low/medium/high/critical, got %q", in.Severity)
	}
	if in.Line < 0 {
		return "", fmt.Errorf("report_finding: line must be >= 0")
	}
	if in.EndLine < 0 {
		return "", fmt.Errorf("report_finding: end_line must be >= 0")
	}
	if in.EndLine > 0 {
		if in.Line <= 0 {
			return "", fmt.Errorf("report_finding: line is required when end_line is set")
		}
		if in.EndLine < in.Line {
			return "", fmt.Errorf("report_finding: end_line must be >= line")
		}
	}
	end := in.EndLine
	if end == in.Line {
		end = 0 // normalize single-line ranges
	}
	f := h.ReportFinding(c.WorkspaceID, c.AgentCode, Finding{
		File: in.File, Line: in.Line, EndLine: end, Severity: sev,
		Message: in.Message, Summary: strings.TrimSpace(in.Summary),
	})
	// Structured JSON for the chat digest (id/file/summary) — agents read the
	// same payload without scraping prose.
	out := struct {
		ID       string `json:"id"`
		File     string `json:"file"`
		Line     int    `json:"line,omitempty"`
		EndLine  int    `json:"endLine,omitempty"`
		Severity string `json:"severity"`
		Message  string `json:"message"`
		Summary  string `json:"summary,omitempty"`
		Status   string `json:"status"`
	}{
		ID: f.ID, File: f.File, Line: f.Line, EndLine: f.EndLine,
		Severity: string(f.Severity), Message: f.Message, Summary: f.Summary,
		Status: f.Status,
	}
	b, err := json.Marshal(out)
	if err != nil {
		return "", err
	}
	return string(b), nil
}
