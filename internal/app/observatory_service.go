package app

import (
	"fmt"
	"strings"

	"recur/internal/domain"
	"recur/internal/engine"
)

// ObservatoryService exposes Observatory UI affordances (findings today).
type ObservatoryService struct{ base }

// ListFindings returns reviewer findings for a loop (all statuses).
func (s *ObservatoryService) ListFindings(workspaceID string) []domain.Finding {
	return s.st().Findings(workspaceID)
}

// SetFindingStatus updates one finding's status (open/resolved/ignored/invalid)
// from the Findings panel. Publishes findings.updated so the UI refreshes.
func (s *ObservatoryService) SetFindingStatus(workspaceID, findingID, status string) (domain.Finding, error) {
	f, err := s.st().SetFindingStatus(workspaceID, findingID, status)
	if err != nil {
		return domain.Finding{}, err
	}
	s.eng.Bus.Publish(engine.Event{
		Name: "findings.updated",
		Data: map[string]any{"workspaceId": workspaceID},
	})
	return f, nil
}

// AddFindingNote appends a human operator comment on a finding (Findings panel /
// chat digest). Publishes findings.updated so the UI refreshes.
func (s *ObservatoryService) AddFindingNote(workspaceID, findingID, body string) (domain.Finding, error) {
	f, err := s.st().AddFindingNote(workspaceID, findingID, body, domain.FindingActorHuman)
	if err != nil {
		return domain.Finding{}, err
	}
	s.eng.Bus.Publish(engine.Event{
		Name: "findings.updated",
		Data: map[string]any{"workspaceId": workspaceID},
	})
	return f, nil
}

// DeleteFindingNote removes one operator note from a finding. Publishes
// findings.updated so the UI refreshes.
func (s *ObservatoryService) DeleteFindingNote(workspaceID, findingID, noteID string) (domain.Finding, error) {
	f, err := s.st().DeleteFindingNote(workspaceID, findingID, noteID)
	if err != nil {
		return domain.Finding{}, err
	}
	s.eng.Bus.Publish(engine.Event{
		Name: "findings.updated",
		Data: map[string]any{"workspaceId": workspaceID},
	})
	return f, nil
}

// CreateFinding records a user note from the Observatory review surface as an
// open info finding (distinct from agent reviewer findings via Source=user).
// endLine is the inclusive end of a multiline span (0 or == line → single line).
func (s *ObservatoryService) CreateFinding(workspaceID, file string, line int, message string, endLine int) (domain.Finding, error) {
	workspaceID = strings.TrimSpace(workspaceID)
	file = strings.TrimSpace(file)
	message = strings.TrimSpace(message)
	if workspaceID == "" {
		return domain.Finding{}, fmt.Errorf("workspace id required")
	}
	if file == "" {
		return domain.Finding{}, fmt.Errorf("file required")
	}
	if message == "" {
		return domain.Finding{}, fmt.Errorf("message required")
	}
	if line < 0 {
		return domain.Finding{}, fmt.Errorf("line must be ≥ 0")
	}
	if endLine < 0 {
		return domain.Finding{}, fmt.Errorf("end line must be ≥ 0")
	}
	if endLine > 0 {
		if line <= 0 {
			return domain.Finding{}, fmt.Errorf("line is required when end line is set")
		}
		if endLine < line {
			return domain.Finding{}, fmt.Errorf("end line must be ≥ line")
		}
		if endLine == line {
			endLine = 0
		}
	}
	ws, ok := s.st().Workspace(workspaceID)
	if !ok {
		return domain.Finding{}, fmt.Errorf("workspace %q not found", workspaceID)
	}
	iter := 1
	if ws.Iteration > 0 {
		iter = ws.Iteration
	}
	f := s.st().AddFinding(domain.Finding{
		WorkspaceID: workspaceID,
		Iteration:   iter,
		File:        file,
		Line:        line,
		EndLine:     endLine,
		Severity:    domain.SevInfo,
		Message:     message,
		Status:      string(domain.FindingOpen),
		UpdatedBy:   domain.FindingActorHuman,
		Source:      domain.FindingSourceUser,
	})
	s.eng.Bus.Publish(engine.Event{
		Name: "findings.updated",
		Data: map[string]any{"workspaceId": workspaceID},
	})
	return f, nil
}
