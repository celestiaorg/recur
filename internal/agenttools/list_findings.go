package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaListFindings = json.RawMessage(`{
  "type": "object",
  "properties": {
    "status": {"type": "string", "enum": ["open","resolved","ignored","invalid","all"], "description": "Filter by status (default open)"}
  }
}`)

type listFindingsTool struct{}

func (listFindingsTool) Name() string { return "list_findings" }

func (listFindingsTool) Description() string {
	return "List reviewer findings for this workspace. Optional status filter: open (default), " +
		"resolved, ignored, invalid, or all. Returns JSON with status, count, and " +
		"findings[{id,file,line,severity,message,…}]."
}

func (listFindingsTool) Schema() json.RawMessage { return schemaListFindings }

func (listFindingsTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	status := "open"
	raw := strings.TrimSpace(string(c.Args))
	if raw != "" && raw != "null" {
		var in struct {
			Status string `json:"status"`
		}
		if err := strictUnmarshal(c.Args, &in); err != nil {
			return "", err
		}
		if s := strings.TrimSpace(in.Status); s != "" {
			status = s
		}
	}
	switch status {
	case "open", "resolved", "ignored", "invalid", "all":
	default:
		return "", fmt.Errorf("list_findings: status must be open/resolved/ignored/invalid/all, got %q", status)
	}
	list, err := h.ListFindings(c.WorkspaceID, c.AgentCode, status)
	if err != nil {
		return "", err
	}
	// Structured JSON so the chat UI can render finding rows (and agents can
	// still read ids/paths/messages without scraping prose).
	type noteRow struct {
		ID        string `json:"id"`
		Author    string `json:"author"`
		Body      string `json:"body"`
		CreatedAt string `json:"createdAt"`
	}
	type row struct {
		ID           string    `json:"id"`
		File         string    `json:"file"`
		Line         int       `json:"line,omitempty"`
		EndLine      int       `json:"endLine,omitempty"`
		Severity     string    `json:"severity"`
		Message      string    `json:"message"`
		Summary      string    `json:"summary,omitempty"`
		Status       string    `json:"status"`
		StatusReason string    `json:"statusReason,omitempty"`
		UpdatedBy    string    `json:"updatedBy,omitempty"`
		Notes        []noteRow `json:"notes,omitempty"`
	}
	out := struct {
		Status   string `json:"status"`
		Count    int    `json:"count"`
		Findings []row  `json:"findings"`
	}{
		Status:   status,
		Count:    len(list),
		Findings: make([]row, 0, len(list)),
	}
	for _, f := range list {
		var notes []noteRow
		for _, n := range f.Notes {
			notes = append(notes, noteRow{
				ID: n.ID, Author: n.Author, Body: n.Body, CreatedAt: n.CreatedAt,
			})
		}
		out.Findings = append(out.Findings, row{
			ID: f.ID, File: f.File, Line: f.Line, EndLine: f.EndLine,
			Severity: string(f.Severity), Message: f.Message, Summary: f.Summary,
			Status: f.Status, StatusReason: f.StatusReason, UpdatedBy: f.UpdatedBy,
			Notes: notes,
		})
	}
	b, err := json.Marshal(out)
	if err != nil {
		return "", err
	}
	return string(b), nil
}
