package app

import (
	"fmt"

	"recur/internal/code"
	"recur/internal/domain"
)

// ListOpenInTargets returns installed apps that can open a workspace folder.
func (s *WorkspacesService) ListOpenInTargets() []domain.OpenInTarget {
	return code.ListOpenInTargets()
}

// OpenWorkspaceIn opens the workspace's on-disk folder in the named app
// (finder, cursor, terminal, …).
func (s *WorkspacesService) OpenWorkspaceIn(workspaceID, targetID string) error {
	ws, ok := s.st().Workspace(workspaceID)
	if !ok {
		return fmt.Errorf("workspace %q not found", workspaceID)
	}
	if ws.WorktreePath == "" {
		return fmt.Errorf("workspace %q has no folder on disk", workspaceID)
	}
	return code.OpenIn(ws.WorktreePath, targetID)
}
