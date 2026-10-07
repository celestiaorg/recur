package engine

import (
	"context"
	"errors"
	"fmt"
	"html"
	"strings"
	"time"

	"recur/internal/domain"
)

// maestroAgentCode is the avatar/agent code the Maestro chat session is recorded
// under (matches the seeded "MA" agent the UI already renders). The Maestro's
// claude session_id is persisted on this agent's SessionID so a later chat turn
// resumes the same conversation (PRD §8.5).
const maestroAgentCode = "MA"

// maestroChatTimeout bounds one Maestro chat turn so a wedged claude -p (or a
// missing/hung CLI in prod) can never block the bound service call — and, more
// importantly, never block the loop goroutine (the chat runs OFF the loop, but it
// shares the engine/store, so it must finish). The loop goroutine is untouched.
const maestroChatTimeout = 120 * time.Second

// SendMaestroMessage runs one Maestro chat turn for a loop and returns the streamed
// reply text (PRD §8.5, §15.1). It is the real implementation behind
// MaestroService.SendMaestroMessage (the M0 stub).
//
// CONCURRENCY (the load-bearing part): this turn runs on the CALLER's goroutine
// (the bound service call), NOT the loop's goroutine. It must never block or wedge a
// running loop. It only reads the loop record + Maestro agent session (store, mutex-
// guarded) and runs an INDEPENDENT claude -p turn via the same AgentRunner; the loop
// goroutine keeps driving its state machine concurrently. First engage does not
// call this — sendLoopChatMessage starts the walk with no conductor LLM turn.
// The chat's only mutations are: persisting the Maestro session id, appending feed
// lines, and — if the Maestro steers — applying a plan change WITHIN the rails (a rail
// can never be crossed via chat; see applyMaestroSteer), which queues the steer onto
// the running loop for the next specialist turn. All of those go through the
// already-concurrency-safe store + bus + loop.queueSteer, so there is no
// shared-mutable-state race and no double-dispatch.
//
// Multi-turn continuity (PRD §8.5): the first message starts a session; the engine
// captures the returned session_id on the Maestro agent record; later messages
// resume it via --resume <session_id>. A turn that fails to produce a reply returns
// an honest fallback string rather than an error (the chat box should never wedge).
func (e *Engine) SendMaestroMessage(workspaceID, text string, attachments []domain.ChatAttachment) string {
	return e.sendMaestroMessage(workspaceID, maestroAgentCode, text, attachments, "")
}

func (e *Engine) sendMaestroMessage(workspaceID, code, text string, attachments []domain.ChatAttachment, displayText string) string {
	if strings.TrimSpace(code) == "" {
		code = maestroAgentCode
	}
	unlockLife, ok := e.lockChatLife(workspaceID, code)
	defer unlockLife()
	if !ok {
		return "I can't find that loop anymore — it may have been removed."
	}
	retry := isChatRetryDisplay(displayText)
	var imgs []UserImage
	if retry {
		last, ok := e.lastChatUserPrompt(workspaceID, code)
		if !ok {
			return "There's nothing to retry — send a new message."
		}
		text, imgs = last.wire, last.imgs
	} else {
		var err error
		text, imgs, err = prepareChatAttachments(workspaceID, text, attachments)
		if err != nil {
			return "I couldn't save that image attachment. Try a smaller PNG/JPEG/GIF/WebP."
		}
		if text == "" && len(imgs) == 0 {
			return ""
		}
	}

	loop, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return "I can't find that loop anymore — it may have been removed."
	}

	nudge := ""
	nameFrom := displayText
	if nameFrom == "" {
		nameFrom = text
	}
	if nameFrom == "" && len(imgs) > 0 {
		nameFrom = "image attachment"
	}
	if !retry {
		loop = e.dropEarlyTitles(loop, code, nameFrom)
		nudge = e.renameNudgePrompt(workspaceID, code, domain.ChatModeLoop, nameFrom)
		e.maybeApplyProvisionalWorkspaceTitle(loop, nameFrom)
		if isPairChatCode(code) {
			e.ensurePairAgent(workspaceID, code, "")
		} else {
			e.ensureMaestroAgent(workspaceID, loop.GoalLabel())
		}
		e.rememberPairChatSelection(workspaceID, code, "", nil, domain.ChatModeLoop, nil)
		e.maybeApplyProvisionalChatTitle(workspaceID, code, nameFrom)
	}

	// Resume the prior session for this tab (multi-turn continuity).
	resume := ""
	if a, ok := e.Store.Agent(workspaceID, code); ok {
		resume = a.SessionID
	}

	set := e.settings()
	persistDisplay := displayText
	specDisplay := displayText
	prompt := text
	if retry {
		persistDisplay = chatRetryDisplaySentinel
		specDisplay = ""
	} else {
		raw := text
		expanded, err := e.ExpandComposer(context.Background(), text, expandOpts{
			worktree: loop.WorktreePath, workspaceID: workspaceID,
		})
		if errors.Is(err, ErrEmptyPromptAction) {
			return ""
		}
		if err != nil {
			return "I couldn't run that prompt action: " + err.Error()
		}
		prompt = expanded.Prompt
		if persistDisplay == "" && (expanded.Display != "" || prompt != raw) {
			persistDisplay = expanded.Display
			if persistDisplay == "" {
				persistDisplay = raw
			}
			specDisplay = persistDisplay
		}
	}
	sysPrompt := maestroSystemPrompt(e.maestroContextForChat(loop, code))
	spec := AgentSpec{
		WorkspaceID:                 workspaceID,
		ChatCode:                    code,
		AgentID:                     workspaceID + "-" + code,
		Role:                        domain.RoleMaestro,
		Model:                       set.MaestroModel().String(),
		Flags:                       set.MaestroFlags(),
		SystemPrompt:                sysPrompt,
		AppendSystemPrompt:          nudge,
		AllowedTools:                maestroTools,
		Prompt:                      prompt,
		DisplayText:                 specDisplay,
		SkipRecordUser:              retry,
		Images:                      imgs,
		Worktree:                    loop.WorktreePath,
		PermissionMode:              set.PermissionMode(),
		ResumeSession:               resume,
		IncludePartials:             true,
		RefreshStandingInstructions: e.standingPromptChanged(workspaceID, code, sysPrompt, resume),
	}

	// Wire the Maestro to the Recur MCP so a plan/escalate tool call becomes a typed
	// engine callback (steering). Cleaned up when the turn ends.
	token, mcpURL, err := e.registerMCP(workspaceID, code, code, spec.AllowedTools, spec.Model)
	if err != nil {
		// Without the MCP the Maestro can't steer, escalate, or file a plan —
		// it could still produce prose, but every action it claimed to take
		// would be a no-op. Say so instead.
		return "I can't run right now: " + err.Error()
	}
	defer e.unregisterMCP(token)
	spec.MCPURL = mcpURL

	// This turn is mirrored into the durable session store by the
	// engine's mirroringRunner wrapper, keyed on spec.Role — the SINGLE source of
	// truth for the Maestro transcript (there is no separate chat store), read back
	// via MaestroChatItems. No wiring needed here.

	// Acquire an agent slot so a flurry of chat turns can't blow past the machine-wide
	// agent cap (PRD §6.4). Bound the turn so it can't hang the service call/loop.
	// registerChatTurn exposes cancel to CancelChat (Stop in the chat dock).
	// Cancel-only context: chatTurn owns the wedged-turn deadline so ask_user
	// can hold it. Maestro's budget is short (2m) — without a hold, parked
	// questions were auto-abandoned almost immediately.
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	turn := e.registerChatTurn(workspaceID, code, cancel)
	if turn == nil {
		return "I can't find that loop anymore — it may have been removed."
	}
	unlockLife()
	turn.arm(maestroChatTimeout)
	defer e.unregisterChatTurn(workspaceID, code, turn)
	release, gotSlot := e.acquireAgent(ctx.Done())
	if !gotSlot {
		if errors.Is(ctx.Err(), context.Canceled) {
			if e.chatTurnWasParked(workspaceID, code) {
				return ""
			}
			return chatStoppedReply
		}
		return "I'm at capacity right now — every agent slot is busy. Try again in a moment."
	}
	defer release()

	e.updateAgentStatus(workspaceID, code, "directing")
	var turnErr error
	defer func() {
		if e.isShuttingDown() && errors.Is(turnErr, context.Canceled) {
			return
		}
		e.updateAgentStatus(workspaceID, code, "idle")
	}()

	stream := &chatStream{e: e, workspaceID: workspaceID, onInit: func(sessionID string) {
		e.setAgentSession(workspaceID, code, sessionID)
		e.appendAgentStep(workspaceID, code, domain.AgentStep{
			Kind: "note", Text: "chat: " + truncate(text, 80),
		})
	}}
	started := time.Now()
	var res Result
	res, turnErr = e.runAgent(ctx, spec, stream.emit)
	if turnErr == nil {
		e.rememberStandingPrompt(workspaceID, code, spec.SystemPrompt)
	}
	e.publishResource()

	if res.SessionID != "" {
		e.setAgentSession(workspaceID, code, res.SessionID)
	}

	return e.finalizeChatReply(workspaceID, code, prompt, persistDisplay, stream.text(), res, turnErr,
		"On it — I'll keep working within your rails and keep you posted.", started, spec.SkipRecordUser, spec.UserMessageID)
}

// MaestroChatItems returns the loop's Maestro chat transcript (session code MA) from
// the session store — the same lossless source the workspace agent chat reads.
func (e *Engine) MaestroChatItems(workspaceID string) []domain.ChatItem {
	return e.sessionChatItems(workspaceID, maestroAgentCode)
}

// applyMaestroSteer applies a Maestro re-plan from chat steering (PRD §8.5): the
// steer is (1) recorded as a plan-change deviation on the feed (PRD §7.4) and
// (2) queued on the running loop so specialists are steered without busting the
// prompt cache (D49): newly spawned agents get it in their initial Prompt; the
// in-flight agent gets it as a stdin user message on its next tool_result turn.
//
// There is deliberately NO text matching against the rails here:
// rails are enforced where actions EXECUTE — the guard already refuses a
// rail-crossing outcome (e.g. pr_open with findings, skipping verify) regardless
// of what any prompt says, and names the rail in its refusal. Rejecting the
// SENTENCE by keyword match bought no safety and cost trust.
//
// Concurrency: called from the MCP HTTP handler goroutine (the Maestro chat turn),
// concurrently with the loop goroutine. It writes through the store + bus (mutex-
// guarded) and queues the steer on the loop via queueSteer (also mutex-guarded). It
// never mutates the loop's state machine directly, so there is no race / double-dispatch.
func (e *Engine) applyMaestroSteer(workspaceID, agentCode, summary string) {
	summary = strings.TrimSpace(summary)
	if summary == "" {
		return
	}
	e.emitDeviation(workspaceID,
		"Re-planned from your chat: "+html.EscapeString(truncate(summary, 200)),
		"default = the original plan",
		"you steered the loop via the Maestro chat (rails still hold at execution).")
	if lp := e.runningLoopForHand(workspaceID, agentCode); lp != nil {
		lp.queueSteer(summary)
	}
}

// maestroSession returns the persisted Maestro claude session id for a loop, or ""
// if no chat session has started yet (the first message starts one).
func (e *Engine) maestroSession(workspaceID string) string {
	a, ok := e.Store.Agent(workspaceID, maestroAgentCode)
	if !ok {
		return ""
	}
	return a.SessionID
}

// ensureMaestroAgent makes sure a Maestro agent record exists for the loop so the
// chat session id has somewhere durable to live and the Agents tab shows it. It is
// upserted (no-op if it already exists with the same code) — never clobbers a
// session id, because UpsertAgent matches on code and the caller only sets fields
// once; the session id is set separately via setAgentSession.
// Chat sends use "directing" so the first agent.updated is live (idle→directing
// can reorder on the Wails bridge). Dispatch uses idle — no MA turn is in flight.
func (e *Engine) ensureMaestroAgent(workspaceID, goal string) {
	e.ensureMaestroAgentWithStatus(workspaceID, goal, "directing")
}

func (e *Engine) ensureMaestroAgentWithStatus(workspaceID, goal, status string) {
	if _, ok := e.Store.Agent(workspaceID, maestroAgentCode); ok {
		return // already present (keeps its session id)
	}
	if status == "" {
		status = "idle"
	}
	e.putAgent(workspaceID, domain.Agent{
		ID: workspaceID + "-" + maestroAgentCode, WorkspaceID: workspaceID, Code: maestroAgentCode,
		Name: "Maestro", Role: "orchestrates the loop", Model: e.settings().MaestroModel().ID,
		Status: status, Color: "var(--amber)", Task: goal, Cur: -1,
	})
}

// maestroContext builds the live loop context seeded into the Maestro chat turn
// (PRD §8.5): goal, [[loop]] prompt (conductor charter), coarse state + iteration,
// and a few recent activity lines (tags stripped). It reads the durable feed so
// the Maestro is grounded in what actually happened, even across a reload.
func (e *Engine) maestroContext(loop domain.Workspace) MaestroContext {
	return e.maestroContextForChat(loop, "")
}

func (e *Engine) maestroContextForChat(loop domain.Workspace, chatCode string) MaestroContext {
	recent := e.recentActivity(loop.ID, 5)
	lp := e.runningLoopForHand(loop.ID, chatCode)
	ctx := MaestroContext{
		Goal:      loop.Goal,
		Mission:   e.loopMission(loop, chatCode),
		State:     string(loop.State),
		Live:      lp != nil,
		Iteration: loop.Iteration,
		Recent:    recent,
		Handoff:   loop.Handoff,
	}
	if g, _ := e.chatLoopGoal(loop.ID, chatCode); g != "" {
		ctx.Goal = g
	}
	if lp != nil {
		lp.mu.Lock()
		if lp.walkState != "" {
			ctx.State = string(lp.walkState)
		} else if lp.escalated {
			ctx.State = string(domain.StateEscalated)
		} else if lp.paused {
			ctx.State = string(domain.StatePaused)
		} else if ctx.State == string(domain.StateEscalated) || ctx.State == string(domain.StatePaused) {
			ctx.State = string(domain.StateCoding)
		}
		ctx.Iteration = lp.walkIter
		lp.mu.Unlock()
		if g := strings.TrimSpace(lp.Goal); g != "" {
			ctx.Goal = g
		}
	}
	if pr, ok := e.Store.PRByWorkspace(loop.ID); ok {
		ctx.PRThread = formatPRThread(pr)
		if len(pr.Impact) > 0 {
			sym, api, test := 0, 0, 0
			for _, it := range pr.Impact {
				switch it.Kind {
				case "symbol":
					sym++
				case "api":
					api++
				case "test":
					test++
				}
			}
			ctx.Impact = fmt.Sprintf("%d defs · %d refs · %d tests (from last PR analysis)", sym, api, test)
		}
	}
	if ctx.Impact == "" {
		if a, ok := e.Store.LatestAnalysis(loop.ProjectID, loop.ID); ok && len(a.Impact) > 0 {
			ctx.Impact = a.Summary
		}
	}
	return ctx
}

// loopMission is [[loop]] prompt (catalog Prompts["mission"]): standing
// instructions for the conductor session. The graph still drives the walk.
func (e *Engine) loopMission(ws domain.Workspace, chatCode string) string {
	loopID, policyID := "", ""
	if chatCode != "" {
		loopID, policyID = e.chatLoopPolicy(ws.ID, chatCode)
	}
	if loopID == "" {
		loopID = ws.LoopID
	}
	if policyID == "" {
		policyID = ws.PolicyID
	}
	scoped := ws
	scoped.LoopID, scoped.PolicyID = loopID, policyID
	g, _ := e.guardForWorkspace(scoped)
	if g.Loop.Prompts == nil {
		return ""
	}
	return strings.TrimSpace(g.Loop.Prompts["mission"])
}

// recentActivity returns up to n recent activity-feed lines for a loop (newest
// first), with HTML tags stripped to plain text for the prompt. It prefers the live
// in-memory feed and falls back to the reconstructed durable feed (reload path).
func (e *Engine) recentActivity(workspaceID string, n int) []string {
	feed := e.Store.Feed(workspaceID)
	if len(feed) == 0 {
		feed = e.ReconstructFeed(workspaceID)
	}
	out := make([]string, 0, n)
	for _, it := range feed {
		if len(out) >= n {
			break
		}
		line := strings.TrimSpace(stripTags(it.Text))
		if line == "" {
			continue
		}
		name := it.Name
		if name == "" {
			name = "•"
		}
		out = append(out, fmt.Sprintf("%s: %s", name, truncate(line, 120)))
	}
	return out
}

// stripTags removes simple HTML tags from a feed line for the plain-text prompt
// context (the feed stores trusted HTML; the prompt wants plain text). It is a small
// state machine over the bytes — no regexp dependency.
func stripTags(s string) string {
	var b strings.Builder
	depth := 0
	for _, r := range s {
		switch r {
		case '<':
			depth++
		case '>':
			if depth > 0 {
				depth--
			}
		default:
			if depth == 0 {
				b.WriteRune(r)
			}
		}
	}
	return b.String()
}
