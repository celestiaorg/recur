package engine

import (
	"context"
	"errors"
	"fmt"
	"html"
	"log"
	"os"
	"strings"
	"time"

	"recur/internal/domain"
	"recur/internal/runconfig"
	"recur/internal/wsenv"
)

const projectHookTimeout = 10 * time.Minute

// runTeardownHook runs teardown_cmd best-effort. Failures are noted on the feed
// and never block archive/delete.
func (e *Engine) runTeardownHook(ws domain.Workspace, proj domain.Project) {
	// Never fall back to the project's real checkout — a still-creating
	// workspace has an empty WorktreePath, and running teardown there would
	// mutate the user's main repo.
	if strings.TrimSpace(ws.WorktreePath) == "" {
		return
	}
	if err := e.runProjectHook(proj.TeardownCmd, ws, proj, "teardown"); err != nil {
		if _, ok := e.Store.Workspace(ws.ID); ok {
			e.emitWorkspaceEvent(ws.ID, "MA", "Maestro",
				"Teardown command failed: "+html.EscapeString(err.Error()))
		} else {
			log.Printf("recur: teardown command for deleted workspace %s: %v", ws.ID, err)
		}
	}
}

func (e *Engine) runProjectHook(shellCmd string, ws domain.Workspace, proj domain.Project, kind string) error {
	ctx, cancel := context.WithTimeout(context.Background(), projectHookTimeout)
	defer cancel()
	return e.runProjectHookWithContext(ctx, shellCmd, ws, proj, kind)
}

func (e *Engine) runProjectHookWithContext(ctx context.Context, shellCmd string, ws domain.Workspace, proj domain.Project, kind string) error {
	shellCmd = strings.TrimSpace(shellCmd)
	if shellCmd == "" {
		return nil
	}
	dir := ws.WorktreePath
	if dir == "" {
		dir = proj.RepoPath
	}
	if dir != "" {
		if fi, err := os.Stat(dir); err != nil || !fi.IsDir() {
			dir = ""
		}
	}
	env := wsenv.FromWorkspace(ws, proj).Environ(os.Environ())
	tail, err := runconfig.RunCmdOutput(ctx, shellCmd, dir, env)
	if err != nil {
		cancelled := errors.Is(err, context.Canceled)
		head := err.Error()
		switch {
		case errors.Is(err, context.DeadlineExceeded):
			head = "timed out after 10m"
		case cancelled:
			head = "cancelled"
		}
		if tail != "" {
			err = fmt.Errorf("%s\n%s", head, tail)
		} else {
			err = errors.New(head)
		}
		if kind == "setup" && !cancelled {
			e.emitWorkspaceEvent(ws.ID, "MA", "Maestro",
				"Setup command failed: "+html.EscapeString(err.Error()))
			log.Printf("recur: setup command for workspace %s failed: %v", ws.ID, err)
		}
		return err
	}
	if kind == "setup" {
		e.emitWorkspaceEvent(ws.ID, "MA", "Maestro", "Setup command finished.")
	}
	return nil
}
