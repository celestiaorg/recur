package app

import (
	"fmt"
	"time"

	"recur/internal/domain"
)

// ListScheduledPrompts lists schedules and their latest outcomes. An empty
// projectID returns schedules across all projects.
func (s *ProjectsService) ListScheduledPrompts(projectID string) ([]domain.ScheduledPrompt, error) {
	all, err := s.eng.Store.ScheduledPrompts()
	if err != nil {
		return nil, err
	}
	out := []domain.ScheduledPrompt{}
	for _, p := range all {
		if projectID == "" || p.ProjectID == projectID {
			out = append(out, p)
		}
	}
	return out, nil
}

// SaveScheduledPrompt accepts only models offered by the existing model picker.
func (s *ProjectsService) SaveScheduledPrompt(p domain.ScheduledPrompt) (domain.ScheduledPrompt, error) {
	for _, group := range (&SettingsService{base: s.base}).ListModelCatalog() {
		for _, model := range group.Models {
			if model.Ref == p.Model {
				return s.eng.SaveScheduledPrompt(p)
			}
		}
	}
	return p, fmt.Errorf("choose an available model")
}

// PauseScheduledPrompt changes whether future occurrences may launch.
func (s *ProjectsService) PauseScheduledPrompt(id string, paused bool) error {
	return s.eng.PauseScheduledPrompt(id, paused)
}

// DeleteScheduledPrompt keeps any workspaces already created by the schedule.
func (s *ProjectsService) DeleteScheduledPrompt(id string) error {
	return s.eng.DeleteScheduledPrompt(id)
}

// RunScheduledPromptNow returns the workspace while setup proceeds in the background.
func (s *ProjectsService) RunScheduledPromptNow(id string) (string, error) {
	return s.eng.RunScheduledPromptNow(id)
}

// PreviewScheduledPrompt validates timing and returns the next three run times.
func (s *ProjectsService) PreviewScheduledPrompt(p domain.ScheduledPrompt) ([]int64, error) {
	out := []int64{}
	after := time.Now()
	for i := 0; i < 3; i++ {
		next, err := domain.NextScheduledTime(p, after)
		if err != nil {
			return nil, err
		}
		if next.IsZero() {
			if len(out) == 0 && !p.OnStartup() {
				return nil, fmt.Errorf("choose a future date and time")
			}
			break
		}
		out = append(out, next.UnixMilli())
		after = next
	}
	return out, nil
}
