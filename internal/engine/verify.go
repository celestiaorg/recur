package engine

import (
	"context"
	"fmt"
	"html"
	"os"
	"time"

	"recur/internal/domain"
	"recur/internal/mcp"
	"recur/internal/runconfig"
	"recur/internal/store"
	"recur/internal/wsenv"
)

// verifyProbeTimeout bounds how long the verify phase waits for the dev server to
// come up before declaring the verify failed (never hangs the loop). It mirrors the
// runconfig default and is intentionally small so a misconfigured project surfaces
// quickly rather than wedging the loop.
const verifyProbeTimeout = 15 * time.Second

// runVerify is the verify-in-browser phase (PRD §10): it boots the loop's app on the
// workspace's persisted $RECUR_PORT in the worktree (setup_cmd already ran at
// worktree create), waits for the app URL, then spawns the QA/Browser agent
// (Playwright MCP + mcp__recur__attach_evidence ONLY) to drive the app and file
// evidence. It records the evidence on the loop and returns whether the verify
// PASSED (all steps passed, at least one step recorded). The dev server's whole
// process group is torn down before returning — never leaked, never hung past the
// probe timeout.
//
// Invoked by the walker when the graph has a verify phase. A project without a
// dev_cmd, a server that won't boot, or a failing handshake all yield passed=false.
func (l *Loop) runVerify(ctx context.Context) (passed bool, err error) {
	l.setState(domain.StateVerifying, "verifying in a real browser")

	proj, ok := l.eng.Store.Project(l.ProjectID)
	if !ok {
		l.recordVerifyAbsent("project not found")
		return false, nil
	}
	devCmd := domain.VerifyDevCmd(proj)
	if devCmd == "" {
		// No run config → a loop CAN'T verify (PRD §10). Surface it clearly; rail #1
		// then blocks the PR. This is an "absent" verify, not a hard error.
		l.recordVerifyAbsent("no dev server command configured — set the project run config so loops can verify")
		return false, nil
	}

	l.emitFeed("MA", "Maestro", "Booting the app to verify the change in a real browser.")

	// Boot the dev server (reuses the proven runconfig process-group teardown). Bound
	// the whole boot to the probe timeout via a child context so a hung boot can't
	// stall the loop even if the parent ctx is long-lived. Use the workspace's
	// durable port + project env (setup_cmd already ran on worktree create).
	wsRec, _ := l.eng.Store.Workspace(l.ID)
	wsRec.WorktreePath = l.Worktree
	if wsRec.ID == "" {
		wsRec.ID = l.ID
		wsRec.ProjectID = l.ProjectID
		wsRec.Port = 0
	}
	env := wsenv.FromWorkspace(wsRec, proj).Environ(os.Environ())
	bootCtx, cancelBoot := context.WithTimeout(ctx, verifyProbeTimeout+5*time.Second)
	booted, berr := runconfig.Boot(bootCtx, devCmd, proj.AppURL, runconfig.BootOptions{
		WorkDir:      l.Worktree,
		Port:         wsRec.Port,
		Env:          env,
		ProbeTimeout: verifyProbeTimeout,
	})
	cancelBoot()
	if berr != nil {
		l.recordVerifyAbsent("could not boot the app: " + berr.Error())
		return false, nil
	}
	// ALWAYS tear the server (and its children) down — never leak/hang (PRD §10).
	defer booted.Stop()

	l.emitFeed("MA", "Maestro", fmt.Sprintf(
		"App booted on $RECUR_PORT=%s (%s) — handing to QA.", html.EscapeString(booted.Port), html.EscapeString(booted.AppURL)))

	// Run the QA agent; it files evidence via attach_evidence through the MCP. If the
	// agent turn errors (or ctx is cancelled), surface it.
	if rerr := l.runQA(ctx, proj, booted.AppURL); rerr != nil {
		return false, rerr
	}

	// Read the accumulated evidence (filed concurrently via the MCP callback) and
	// settle the verify result. The ENGINE decides pass/fail off the recorded steps,
	// not off anything the QA agent says — a verify with no steps, or any failing
	// step, is NOT a pass.
	evidence := l.evidenceThisVerify()
	passed = verifyPassed(evidence)
	// Handshake is report_result. Missing
	// the call is that verb's fail — evidence alone is not enough.
	if ws, ok := l.eng.Store.Workspace(l.ID); !ok || ws.TestResult == nil {
		passed = false
	} else {
		passed = ws.TestResult.Passed
	}
	l.persistVerify(evidence, passed)

	if passed {
		l.emitFeed("MA", "Maestro", fmt.Sprintf(
			"Verify passed: %s — all green.", plural(len(evidence), "browser step", "browser steps")))
	} else {
		l.emitFeed("MA", "Maestro", "Verify did not pass.")
	}
	return passed, nil
}

// runQA spawns the QA/Browser agent in the loop's worktree, wired to BOTH the
// Recur MCP (for attach_evidence) AND a Playwright browser MCP (to drive the app).
// Its tools are scoped to the browser MCP + attach_evidence ONLY (no Read/Edit/
// Write/Bash) — enforced in Go via the QA role's allowedTools (PRD §8.2, §9).
func (l *Loop) runQA(ctx context.Context, proj domain.Project, appURL string) error {
	ctx, cancel := l.turnContext(ctx)
	defer cancel()

	rs := l.slotRoleSpec("verifier", roles[domain.RoleQA])
	qaID := l.ID + "-" + rs.code
	set := l.eng.settings()
	model, flags := l.roleModel(rs, set.CoderModel().String(), set.CoderFlags())
	l.eng.upsertAgent(l.ID, domain.Agent{
		ID: qaID, WorkspaceID: l.ID, Code: rs.code, Name: rs.name,
		Role: "drives the live app like a user", Model: domain.ParseModelRef(model).ID,
		Status: "working", Color: "var(--coral)", Task: l.goalLabel(), Cur: -1,
	})

	// The screenshot directory for this loop (M1 store/paths helper). Best-effort:
	// if it can't be created the QA agent simply files evidence without screenshots.
	shotDir := ""
	if root, derr := store.DataDir(); derr == nil {
		if dir, aerr := store.ScreenshotsDir(root, l.ID); aerr == nil {
			shotDir = dir
		}
	}

	// Reset the evidence collector BEFORE the QA agent runs so every attach_evidence
	// callback during this turn is counted for this verify run.
	l.beginVerify()

	// attach_evidence is the ONLY way this turn can report what it saw, so
	// without MCP a verify would pass vacuously (no failing evidence recorded).
	token, mcpURL, err := l.eng.registerMCP(l.ID, rs.code, l.ChatCode, rs.allowedTools, model)
	if err != nil {
		return err
	}
	defer l.eng.unregisterMCP(token)

	inject, clearInject := l.bindInject(rs.code)
	defer clearInject()

	sysPrompt := qaSystemPrompt(l.Goal, appURL, shotDir)
	qaUser := fmt.Sprintf("Verify in the browser: %s", l.Goal)
	prompt := l.applyPendingSteer(qaUser)
	display := fmt.Sprintf("Verify in the browser: %s", l.goalLabel())
	sysPrompt = l.bundleSystemPrompt(l.phaseRoleID("verifier"), sysPrompt, l.Goal, qaRuntimeExtra(appURL, shotDir))
	if expanded, disp, eerr := l.expandPhasePrompt(ctx, "verify", prompt); eerr != nil {
		return eerr
	} else {
		prompt = expanded
		if disp != "" {
			display = disp
		}
	}

	spec := AgentSpec{
		WorkspaceID:    l.ID,
		ChatCode:       l.ChatCode,
		AgentID:        qaID,
		Role:           domain.RoleQA,
		Model:          model,
		Flags:          flags,
		SystemPrompt:   sysPrompt,
		AllowedTools:   rs.allowedTools,
		Prompt:         prompt,
		DisplayText:    display,
		Worktree:       l.Worktree,
		PermissionMode: set.PermissionMode(),
		MCPURL:         mcpURL,
		Playwright:     true,
		Inject:         inject,
	}

	release, ok := l.acquireTurnSlot(ctx)
	if !ok {
		return ctx.Err()
	}
	defer release()

	res, err := l.eng.runAgent(ctx, spec, func(ev ParsedEvent) {
		l.onAgentEvent(rs, ev)
	})
	l.eng.publishResource()
	l.eng.updateAgentStatus(l.ID, rs.code, "idle")

	if err != nil || res.IsError {
		return newTurnError(rs.name, err, res)
	}
	return nil
}

// verifyPassed reports whether a verify run passed: at least one evidence step was
// recorded AND every step passed. An empty evidence set is NOT a pass (an absent
// verify must not silently satisfy rail #1).
func verifyPassed(evidence []mcp.Evidence) bool {
	if len(evidence) == 0 {
		return false
	}
	for _, e := range evidence {
		if !e.Passed {
			return false
		}
	}
	return true
}

// persistVerify records the QA evidence + the pass/fail outcome on the loop so the
// guard can read it (rail #1) and the eventual PR (M6) can surface it. The verify
// steps are converted to domain.VerifyStep (HTML-escaped description + screenshot
// path); VerifyState is "passed" or "failed".
func (l *Loop) persistVerify(evidence []mcp.Evidence, passed bool) {
	steps := make([]domain.VerifyStep, 0, len(evidence))
	for _, e := range evidence {
		steps = append(steps, domain.VerifyStep{
			HTML:       html.EscapeString(e.Step),
			Passed:     e.Passed,
			Screenshot: e.ScreenshotPath,
		})
	}
	state := verifyStateFailed
	if passed {
		state = verifyStatePassed
	}
	updated, ok := l.eng.Store.UpdateWorkspace(l.ID, func(lp *domain.Workspace) {
		lp.Verify = steps
		lp.VerifyState = state
	})
	if !ok {
		return
	}
	l.eng.publishLoopUpdated(l.ID, updated.State, updated.Phase, updated.Iteration)
}

// recordVerifyAbsent records a verify that could not run (no run config, boot
// failure) as a single failing evidence step + VerifyState=failed, so rail #1
// blocks the PR with a clear human reason rather than silently.
func (l *Loop) recordVerifyAbsent(reason string) {
	l.eng.Store.UpdateWorkspace(l.ID, func(lp *domain.Workspace) {
		lp.Verify = []domain.VerifyStep{{HTML: html.EscapeString(reason), Passed: false}}
		lp.VerifyState = verifyStateFailed
	})
	l.emitFeed("MA", "Maestro", "Verify could not run: "+html.EscapeString(reason))
}

// verify-state constants stored on the loop (read by the guard for rail #1).
const (
	verifyStatePassed = "passed"
	verifyStateFailed = "failed"
)
