package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaUpdateFinding = json.RawMessage(`{
  "type": "object",
  "properties": {
    "finding_id": {"type": "string", "description": "Finding id from list_findings or the fix prompt"},
    "status": {"type": "string", "enum": ["resolved","invalid"], "description": "resolved = fixed; invalid = disputed/wrong"},
    "reason": {"type": "string", "description": "Required when status is invalid — why the finding is wrong or out of scope"}
  },
  "required": ["finding_id","status"]
}`)

type updateFindingTool struct{}

func (updateFindingTool) Name() string { return "update_finding" }

func (updateFindingTool) Description() string {
	return "Update one finding's status. status must be resolved " +
		"(the issue is fixed) or invalid (wrong/out of scope — reason required). Does not " +
		"decide convergence; a later review still re-checks the diff."
}

func (updateFindingTool) Schema() json.RawMessage { return schemaUpdateFinding }

func (updateFindingTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		FindingID string `json:"finding_id"`
		Status    string `json:"status"`
		Reason    string `json:"reason"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.FindingID) == "" {
		return "", fmt.Errorf("update_finding: finding_id is required")
	}
	switch strings.TrimSpace(in.Status) {
	case "resolved", "invalid":
	default:
		return "", fmt.Errorf("update_finding: status must be resolved or invalid, got %q", in.Status)
	}
	f, err := h.UpdateFinding(c.WorkspaceID, c.AgentCode, in.FindingID, in.Status, in.Reason)
	if err != nil {
		return "", err
	}
	// Structured JSON for the chat card (id/status/file/reason) — agents read
	// the same payload without scraping prose.
	type noteRow struct {
		ID        string `json:"id"`
		Author    string `json:"author"`
		Body      string `json:"body"`
		CreatedAt string `json:"createdAt"`
	}
	var notes []noteRow
	for _, n := range f.Notes {
		notes = append(notes, noteRow{
			ID: n.ID, Author: n.Author, Body: n.Body, CreatedAt: n.CreatedAt,
		})
	}
	out := struct {
		ID           string    `json:"id"`
		Status       string    `json:"status"`
		File         string    `json:"file"`
		Line         int       `json:"line,omitempty"`
		EndLine      int       `json:"endLine,omitempty"`
		Severity     string    `json:"severity,omitempty"`
		Message      string    `json:"message,omitempty"`
		Summary      string    `json:"summary,omitempty"`
		StatusReason string    `json:"statusReason,omitempty"`
		UpdatedBy    string    `json:"updatedBy,omitempty"`
		Notes        []noteRow `json:"notes,omitempty"`
	}{
		ID: f.ID, Status: f.Status, File: f.File, Line: f.Line, EndLine: f.EndLine,
		Severity: string(f.Severity), Message: f.Message, Summary: f.Summary,
		StatusReason: f.StatusReason, UpdatedBy: f.UpdatedBy, Notes: notes,
	}
	b, err := json.Marshal(out)
	if err != nil {
		return "", err
	}
	return string(b), nil
}
