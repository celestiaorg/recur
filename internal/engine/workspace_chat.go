package engine

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/google/uuid"

	"recur/internal/domain"
)

// pairAgentCode is the default avatar for a workspace coding-agent chat
// (distinct from the Maestro "MA"). Additional chats use PA2, PA3, …. Each
// agent's Claude session id lives on that agent's SessionID so a later message
// resumes the same conversation.
const pairAgentCode = "PA"

// pairTools is the workspace coding agent's base tool set: full read/explore +
// edit. Agent mode additionally wires Recur MCP findings tools (list/update)
// via chatModeShape so Review→Agent can close findings structurally — not done,
// which is a loop-specialist handback signal.
var pairTools = append(append([]string{}, readExploreTools...), editTools...)

// normalizePairChatCode maps an empty/unknown code to the default PA tab.
func normalizePairChatCode(code string) string {
	code = strings.TrimSpace(code)
	if code == "" {
		return pairAgentCode
	}
	return code
}

// isPairChatCode reports whether code is a solo/pair Agent-panel chat (PA, PA2, …).
func isPairChatCode(code string) bool {
	if code == pairAgentCode {
		return true
	}
	if !strings.HasPrefix(code, "PA") || len(code) < 3 {
		return false
	}
	n, err := strconv.Atoi(code[2:])
	return err == nil && n >= 2
}

func isSpecialistChatCode(code string) bool {
	return code != "" && code != maestroAgentCode && !isPairChatCode(code)
}

func pairChatCodeNum(code string) int {
	if code == pairAgentCode {
		return 1
	}
	n, _ := strconv.Atoi(strings.TrimPrefix(code, "PA"))
	return n
}

func pairChatCodeFromNum(n int) string {
	if n <= 1 {
		return pairAgentCode
	}
	return fmt.Sprintf("PA%d", n)
}

// SendWorkspaceMessage runs one turn of a workspace coding-agent chat tab: an
// interactive Claude Code agent in the workspace worktree. code selects the
// chat tab (PA, PA2, …; empty → PA). Unlike the Maestro it does NOT override
// Claude Code's system prompt — it is a plain coding assistant, shaped by the
// tab's chat MODE: agent = read + edit (the
// default); ask/plan/review = read-only tool scoping. model is a qualified ref
// (or Claude alias) or "" for the mode's Settings-role default (Ask/Agent →
// Coder, Plan/Loop → Orchestrator, Review → Reviewer). flags are the per-model
// dimensions from the chat dock (nil/empty → that role's defaults). attachments
// are images pasted into the composer (may be nil). mode is the dock's mode
// ("" → the tab's persisted mode, else agent). displayText is an optional UI
// label for the user bubble (prompt actions); "" for normal composer sends.
// Returns the reply text; tokens also stream live on the bus (session.delta,
// tagged with the chat code).
func (e *Engine) SendWorkspaceMessage(workspaceID, code, text, model string, flags map[string]string, attachments []domain.ChatAttachment, mode, displayText string) string {
	return e.SendWorkspaceTurn(workspaceID, code, text, model, flags, attachments, mode, displayText, false)
}

// SendWorkspaceTurn is SendWorkspaceMessage plus freshSession: when true, this
// turn starts a new provider session on the same Recur tab (prompt actions).
func (e *Engine) SendWorkspaceTurn(workspaceID, code, text, model string, flags map[string]string, attachments []domain.ChatAttachment, mode, displayText string, freshSession bool) string {
	return e.sendWorkspaceTurn(workspaceID, code, text, model, flags, attachments, mode, displayText, freshSession, 0)
}

func (e *Engine) sendWorkspaceTurn(workspaceID, code, text, model string, flags map[string]string, attachments []domain.ChatAttachment, mode, displayText string, freshSession bool, continueEpoch uint64) string {
	code = normalizePairChatCode(code)
	text, wiredSurface, hasSurface := peelChatSurface(text)
	// The Maestro thread is a chat tab like any other in the unified surface
	// — routed to the Maestro implementation, which
	// owns its model/tools AND its own attachment prep; mode is implicitly loop.
	if code == maestroAgentCode {
		workspaceID, code = e.resolveHoistedChat(workspaceID, code)
		return e.sendMaestroMessage(workspaceID, code, text, attachments, displayText)
	}
	if !isPairChatCode(code) {
		code = pairAgentCode
	}
	workspaceID, code = e.resolveHoistedChat(workspaceID, code)
	slot := e.chatTabSlot(workspaceID, code)
	seq, stopGen := slot.begin()
	advanced := false
	defer func() {
		if !advanced {
			slot.claim(seq)
			slot.finish()
		}
	}()
	if testBeforeChatClaim != nil {
		testBeforeChatClaim()
	}
	retry := isChatRetryDisplay(displayText)
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return "I can't find that workspace anymore — it may have been removed."
	}
	mode = e.resolveChatMode(workspaceID, code, mode)
	if ws.IsPRChatHost() && (mode == domain.ChatModeAgent || mode == domain.ChatModeLoop) {
		return "Create a workspace from this pull request to edit files in Agent or Loop. Ask, Plan, and Review still work from the project checkout."
	}
	if retry && mode == domain.ChatModeLoop {
		// Loop on a regular tab means "engage the autopilot", which Retry must
		// not do. Fall back to the tab's persisted non-loop mode (or agent) so
		// the loop-tab lock and tool shape still apply.
		if persisted := domain.NormalizeChatMode(e.chatModeFor(workspaceID, code)); persisted != "" && persisted != domain.ChatModeLoop {
			mode = persisted
		} else {
			mode = domain.ChatModeAgent
		}
	}
	if !retry && mode == domain.ChatModeLoop {
		return e.sendLoopChatMessage(ws, code, text, attachments, displayText)
	}
	unlockLife, ok := e.lockChatLife(workspaceID, code)
	defer unlockLife()
	if !ok {
		if isModeSwitchContinueDisplay(displayText) {
			e.uncountModeRemount(workspaceID, code)
		}
		return "I can't find that workspace anymore — it may have been removed."
	}
	var (
		ctx    context.Context
		cancel context.CancelFunc
		turn   *chatTurn
	)
	if isModeSwitchContinueDisplay(displayText) {
		if !e.continueGenLive(continueEpoch) {
			return ""
		}
		if e.workspaceRemoveBlocksChat(workspaceID) {
			e.uncountModeRemount(workspaceID, code)
			return ""
		}
		ctx, cancel = context.WithCancel(context.Background())
		defer cancel()
		// Register after FIFO claim so a later ticket cannot pin ended while
		// an earlier send is still waiting at claim.
		defer func() {
			if turn == nil {
				e.uncountModeRemount(workspaceID, code)
			}
		}()
	}
	var imgs []UserImage
	if retry {
		last, ok := e.lastChatUserPrompt(workspaceID, code)
		if !ok {
			return "There's nothing to retry — send a new message."
		}
		text, imgs = last.wire, last.imgs
		if v, ok := e.lastChatSurfaces.Load(workspaceID + "\x00" + code); ok {
			if s, ok := v.(chatSurface); ok && strings.TrimSpace(s.Surface) != "" {
				wiredSurface, hasSurface = s, true
			}
		}
	} else {
		var err error
		text, imgs, err = prepareChatAttachments(workspaceID, text, attachments)
		if err != nil {
			return "I couldn't save that image attachment. Try a smaller PNG/JPEG/GIF/WebP."
		}
		if text == "" && len(imgs) == 0 {
			return ""
		}
		if !isModeSwitchContinueDisplay(displayText) {
			e.resetModeRemounts(workspaceID, code)
			if p, ok := e.takeModeContinue(workspaceID, code); ok {
				e.dropContinueGen(p.epoch)
			}
		}
	}
	if mode == domain.ChatModeAgent && ws.HasRun() && domain.RunActive(ws.State) &&
		(loopOwnerCode(ws) == code || e.chatDrivesLiveLoop(workspaceID, code)) {
		// Only the tab driving the loop is locked; sibling tabs share the
		// worktree the same way sibling Loop tabs already do.
		return "A loop is running in this tab, so Agent-mode edits here wait until it pauses or settles. " +
			"Other tabs keep working."
	}
	set := e.settings()
	defModel, defFlags := set.ModelForChatMode(mode)
	if model == "" {
		// A qualified ref, so the chat turn routes to whichever provider the
		// mode's Settings → Models slot is pointed at.
		model = defModel
	}
	if len(flags) == 0 {
		flags = defFlags
	}

	// Persist mode + model + flags as soon as they're resolved — before the
	// turn runs — so a mid-turn DB read / reload never sees a stale config_json.
	e.ensurePairAgent(workspaceID, code, model)
	e.rememberPairChatSelection(workspaceID, code, model, flags, mode, nil)

	// Nudge is computed before the provisional title lands in Goal, otherwise
	// isUnnamedWorkspace is already false and the all-three first-session
	// prompt never fires. Prefer displayText (prompt-action label) else the
	// raw composer text so titles stay human. Retry re-runs the last stored
	// prompt — skip naming (and don't treat the sentinel as a title).
	nudge := ""
	nameFrom := displayText
	if nameFrom == "" {
		nameFrom = text
	}
	if nameFrom == "" && len(imgs) > 0 {
		nameFrom = "image attachment"
	}
	if !retry {
		ws = e.dropEarlyTitles(ws, code, nameFrom)
		nudge = e.renameNudgePrompt(workspaceID, code, mode, nameFrom)
		e.maybeApplyProvisionalWorkspaceTitle(ws, nameFrom)
		e.maybeApplyProvisionalChatTitle(workspaceID, code, nameFrom)
	}

	surface := e.resolveChatSurface(ws, wiredSurface, hasSurface)
	if !retry {
		e.lastChatSurfaces.Store(workspaceID+"\x00"+code, surface)
	}

	// ExpandComposer: leading [[prompt]] slash then wrapComposer (@file,
	// /skill, @terminal, raw regions). Retry replays the already-expanded
	// wire from the session — do not expand again.
	prompt := text
	display := displayText
	isolate := false
	if retry {
		display = chatRetryDisplaySentinel
	} else {
		expanded, err := e.ExpandComposer(context.Background(), text, expandOpts{
			worktree: ws.WorktreePath, workspaceID: workspaceID,
		})
		if errors.Is(err, ErrEmptyPromptAction) {
			return "" // no-send success (e.g. /update finished in git)
		}
		if err != nil {
			return "I couldn't run that prompt action: " + err.Error()
		}
		prompt = expanded.Prompt
		if display == "" && (expanded.Display != "" || prompt != text) {
			display = expanded.Display
			if display == "" {
				display = text
			}
		}
		isolate = expanded.FreshSession || freshSession
	}
	if isModeSwitchContinueDisplay(displayText) || displayText == chatResumeDisplay {
		isolate = false
	}

	// Resume the prior session (multi-turn continuity). Isolated prompt
	// actions start a new provider session on the same Recur tab.
	// Drop the stored id and pending fork inject now so a pre-init failure
	// cannot retry into the previous conversation, and a later typed
	// follow-up resumes this isolated session instead of injecting fork history.
	if isolate {
		e.clearAgentSession(workspaceID, code)
		e.consumeForkInject(workspaceID, code)
		e.markNativeHistoryCutoff(workspaceID, code)
	}
	resume := ""
	if !isolate {
		if a, ok := e.Store.Agent(workspaceID, code); ok {
			resume = a.SessionID
		}
	}

	nested := strings.TrimSpace(ws.ParentID) != ""
	shape := chatModeShape(mode, set, nested)
	specDisplay := display
	if retry {
		specDisplay = ""
	}
	// The user message id is minted here so the worktree can be checkpointed
	// under it before the agent touches anything (checkpoint.go).
	userMsgID := uuid.Must(uuid.NewV7()).String()
	resumeTurn := displayText == chatResumeDisplay
	spec := AgentSpec{
		WorkspaceID:                 workspaceID,
		ChatChild:                   nested,
		AgentID:                     workspaceID + "-" + code,
		Role:                        domain.RoleCoder, // coder-style session/tools; NOTE: no SystemPrompt (below)
		Model:                       model,
		Flags:                       flags,
		SystemPrompt:                "", // deliberately empty — keep Claude Code's own system prompt
		AppendSystemPrompt:          joinAppendPrompt(joinAppendPrompt(shape.appendPrompt, e.simplifyChatAppend(workspaceID, code, mode)), nudge),
		TurnAppendPrompt:            subjectAppendPrompt(surface),
		AllowedTools:                shape.tools,
		Prompt:                      prompt,
		DisplayText:                 specDisplay,
		SkipRecordUser:              retry || isModeSwitchContinueDisplay(displayText),
		UserMessageID:               userMsgID,
		Images:                      imgs,
		Worktree:                    ws.WorktreePath,
		PermissionMode:              shape.permission,
		ResumeSession:               resume,
		IncludePartials:             true,
		RefreshStandingInstructions: e.refreshStanding(workspaceID, code, mode, displayText, resume),
	}

	// Cancel-only context: the wedged-turn deadline lives on chatTurn so ask_user
	// can hold it (a parked question must not die when this budget expires).
	// Deadline from Settings.AgentTurnTimeoutMinutes (default 4h) — last-resort
	// kill for a wedged CLI, not a "task should be done by now" budget.
	if ctx == nil {
		ctx, cancel = context.WithCancel(context.Background())
		defer cancel()
	}
	// Release life lock before waiting on the FIFO — a later Send that already
	// holds it would otherwise deadlock the earlier seq at claim.
	unlockLife()
	slot.claim(seq)
	advanced = true
	unlockLife, ok = e.lockChatLife(workspaceID, code)
	defer unlockLife()
	if !ok {
		slot.finish()
		return "I can't find that workspace anymore — it may have been removed."
	}
	slot.pause()
	steered := false
	// An agent-to-agent notice (hidden display: a subagent report) never
	// steers a live turn — injected mid-reasoning it gets skimmed and a
	// second one blurs into the first. It waits its turn in the FIFO and runs
	// as its own turn on the same session.
	notice := strings.HasPrefix(displayText, displayHiddenSentinel)
	if !retry && !resumeTurn && !notice && !isModeSwitchContinueDisplay(displayText) && !isolate && len(imgs) == 0 {
		ui := display
		if ui == "" {
			ui = text
		}
		// Claude InjectPlain acks after a tool_result. Holding life across
		// that wait deadlocks switch_mode → RememberWorkspaceChatSelection.
		unlockLife()
		steered = e.tryInjectChat(workspaceID, code, encodeInjectPlain(
			joinAppendPrompt(subjectAppendPrompt(surface), prompt), ui))
		unlockLife, ok = e.lockChatLife(workspaceID, code)
		defer unlockLife()
		if !ok {
			slot.resume()
			slot.finish()
			return "I can't find that workspace anymore — it may have been removed."
		}
	}
	slot.resume()
	if steered {
		slot.finish()
		return chatInjectedSentinel
	}
	if slot.stopGen.Load() != stopGen {
		slot.finish()
		return chatStoppedReply
	}
	// Wait out a live predecessor before resume/fork so a follow-up that
	// lost inject still continues that conversation (not an empty snapshot).
	if turn == nil && !resumeTurn {
		if ended := e.chatTurnEnded(workspaceID, code); ended != nil {
			slot.pause()
			unlockLife()
			<-ended
			unlockLife, ok = e.lockChatLife(workspaceID, code)
			defer unlockLife()
			if !ok {
				slot.resume()
				slot.finish()
				return "I can't find that workspace anymore — it may have been removed."
			}
			slot.resume()
			if slot.stopGen.Load() != stopGen {
				slot.finish()
				return chatStoppedReply
			}
		}
		if !isolate {
			resume = ""
			if a, ok := e.Store.Agent(workspaceID, code); ok {
				resume = a.SessionID
			}
			spec.ResumeSession = resume
			spec.RefreshStandingInstructions = e.refreshStanding(workspaceID, code, mode, displayText, resume)
		}
	}
	slot.pause()
	if testBeforeApplyChatFork != nil {
		testBeforeApplyChatFork()
	}
	injected := false
	if !isolate {
		injected = e.applyChatFork(workspaceID, code, &spec, resumeTurn)
	}
	slot.resume()
	if slot.stopGen.Load() != stopGen {
		slot.finish()
		return chatStoppedReply
	}
	if turn == nil {
		if resumeTurn || isModeSwitchContinueDisplay(displayText) {
			turn = e.registerChatTurnIfIdle(workspaceID, code, cancel)
			if turn == nil {
				slot.finish()
				if e.workspaceRemoveBlocksChat(workspaceID) {
					return "I can't find that workspace anymore — it may have been removed."
				}
				return ""
			}
		} else {
			turn = e.registerChatTurn(workspaceID, code, cancel)
			if turn == nil {
				slot.finish()
				return "I can't find that workspace anymore — it may have been removed."
			}
		}
		defer e.unregisterChatTurn(workspaceID, code, turn)
		defer e.maybeContinueAfterModeSwitch(workspaceID, code, turn)
	}
	turn.arm(set.AgentTurnTimeout())
	if nested {
		e.reopenChatChild(workspaceID)
	}
	spec.Inject = turn.armChatInject()
	spec.InjectAck = turn.injectAck
	spec.InjectPlain = true
	slot.finish()
	if shape.mcp {
		// Every mode needs the per-agent Recur MCP endpoint: for its tools
		// (submit_plan / report_finding / list+update_finding) and, when bypass
		// is off, to route permission prompts. A text-only fallback would let a
		// Plan-mode turn "file" a plan that went nowhere.
		token, mcpURL, err := e.registerMCP(workspaceID, code, code, spec.AllowedTools, spec.Model)
		if err != nil {
			return "I can't run right now: " + err.Error()
		}
		defer e.unregisterMCP(token)
		spec.MCPURL = mcpURL
	}
	unlockLife()
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

	e.ensurePairAgentRun(workspaceID, code, model)
	e.updateAgentStatus(workspaceID, code, "working")
	var turnErr error
	defer func() {
		// Only leave status live when quit cancelled the in-flight turn. A
		// turn that already returned success (or a non-cancel error) must go
		// idle even if Shutdown is racing the unwind — otherwise Recover
		// treats a finished reply as interrupted and auto-resumes it.
		if e.isShuttingDown() && errors.Is(turnErr, context.Canceled) {
			return
		}
		e.updateAgentStatus(workspaceID, code, "idle")
	}()

	stream := &chatStream{
		e: e, workspaceID: workspaceID, agentCode: code,
		// Plan-mode chats should file via submit_plan; if Cursor uses createPlan
		// instead, harvest the prose into the Plan panel (summary-only).
		harvestCreatePlan: mode == domain.ChatModePlan,
		onInit: func(sessionID string) {
			e.setAgentSession(workspaceID, code, sessionID)
		},
	}
	// Retry / switch-mode remounts, and a restart whose last prompt is still
	// an open turn (start, no footer), append to that user message: its start
	// checkpoint already exists, only the end moves. Lookup after IfIdle so a
	// user send that lands during the prologue cannot have its work folded
	// into a stale interrupted pair.
	resumeOpenID := ""
	if resumeTurn {
		resumeOpenID = e.lastOpenChatPromptID(ws, code)
		if resumeOpenID != "" {
			spec.SkipRecordUser = true
		}
	}
	if spec.SkipRecordUser {
		if resumeOpenID != "" {
			userMsgID = resumeOpenID
		} else {
			userMsgID = lastChatPromptID(e.Store, workspaceID, code)
		}
	} else {
		e.takeCheckpoint(ws, userMsgID, "start")
	}
	started := time.Now()
	var res Result
	res, turnErr = e.runAgent(ctx, spec, func(ev ParsedEvent) {
		if ev.Kind == KindResult {
			turn.stopChatInject()
		}
		stream.emit(ev)
	})
	if turnErr == nil {
		e.rememberStandingMode(workspaceID, code, mode)
	}
	e.takeCheckpoint(ws, userMsgID, "end")
	e.publishResource()

	if res.SessionID != "" {
		e.setAgentSession(workspaceID, code, res.SessionID)
	}
	e.finishChatFork(workspaceID, code, injected, res)
	// History persists via the session mirror (mirror.go). Failed turns also
	// persist an error note so the UI resync doesn't erase the provider failure.
	return e.finalizeChatReply(workspaceID, code, prompt, display, stream.text(), res, turnErr, "Done.", started, spec.SkipRecordUser, spec.UserMessageID)
}

// markNativeHistoryCutoff stamps the last transcript message so app-mode
// harnesses replay only messages after this fresh_session reset. The visible
// Recur transcript is unchanged.
func (e *Engine) markNativeHistoryCutoff(workspaceID, code string) {
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return
	}
	msgs, err := e.Store.SessionTranscript(sess.ID)
	if err != nil || len(msgs) == 0 {
		return
	}
	last := strings.TrimSpace(msgs[len(msgs)-1].ID)
	if last == "" {
		return
	}
	raw, err := json.Marshal(map[string]string{"nativeHistoryAfter": last})
	if err != nil {
		return
	}
	_ = e.Store.PatchSessionConfigMerge(sess.ID, string(raw))
}

// testBeforeChatClaim, if set, runs after a send takes its FIFO ticket and
// before inject/register (tests only).
var testBeforeChatClaim func()

// testBeforeApplyChatFork, if set, runs after the post-inject Stop check while
// the tab-slot lock is released for applyChatFork (tests only).
var testBeforeApplyChatFork func()

// chatTabTitleLimit caps Agent-panel tab labels (narrower than workspace titles).
const chatTabTitleLimit = 28

// chatHasUserMessage reports whether this chat tab already has a user text turn.
func (e *Engine) chatHasUserMessage(workspaceID, code string) bool {
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return false
	}
	text := e.Store.SessionFirstUserPrompt(sess.ID)
	return strings.TrimSpace(text) != "" && !isChatRetryDisplay(text)
}

// isFirstChatMessage is true when this tab has no prior user text (the message
// about to be sent is the first). Checked before the turn is mirrored.
func (e *Engine) isFirstChatMessage(workspaceID, code string) bool {
	return !e.chatHasUserMessage(workspaceID, code)
}

// isFirstWorkspaceSession is true when no PA* (or Maestro) chat in this
// workspace has a user message yet — this send is the workspace's first session.
func (e *Engine) isFirstWorkspaceSession(workspaceID, code string) bool {
	if !e.isFirstChatMessage(workspaceID, code) {
		return false
	}
	for _, a := range e.pairChatAgents(workspaceID) {
		if a.Code != code && e.chatHasUserMessage(workspaceID, a.Code) {
			return false
		}
	}
	return !e.chatHasUserMessage(workspaceID, maestroAgentCode)
}

// renameNudgePrompt asks the agent to name the workspace/branch/chat via MCP
// on first session of an untitled hand-driven workspace, or just the chat on a
// later tab's first message (and on a first session that is already named).
func (e *Engine) renameNudgePrompt(workspaceID, code, mode, nameFrom string) string {
	if e.isFirstWorkspaceSession(workspaceID, code) {
		if ws, ok := e.Store.Workspace(workspaceID); ok {
			provisional := provisionalWorkspaceTitle(nameFrom)
			justNamed := provisional != "" && ws.Goal == provisional
			if isUnnamedWorkspace(ws) || justNamed {
				if domain.NormalizeChatMode(mode) == domain.ChatModeAgent || domain.NormalizeChatMode(mode) == domain.ChatModeLoop {
					return firstWorkspaceRenamePrompt
				}
				return firstWorkspaceRenamePromptNoBranch
			}
		}
	}
	if e.isFirstChatMessage(workspaceID, code) {
		return firstChatRenamePrompt
	}
	return ""
}

// workspaceTitleWords is the provisional sidebar cap, matching
// mcp__recur__rename_workspace ("at most 8 words").
const workspaceTitleWords = 8

// workspaceTitleLimit is a hard rune cap so a single huge "word" cannot blow
// the sidebar. Matches sanitizeWorkspaceTitle.
const workspaceTitleLimit = 60

// isUnnamedWorkspace is true until the first message is truncated into Goal
// or the agent calls rename_workspace (or the human named it at create).
func isUnnamedWorkspace(ws domain.Workspace) bool {
	g := strings.TrimSpace(ws.Goal)
	return g == "" || g == defaultWorkspaceTitle
}

// truncateWords keeps the first n whitespace-separated words of s.
func truncateWords(s string, n int) string {
	if n <= 0 {
		return ""
	}
	fields := strings.Fields(s)
	if len(fields) <= n {
		return strings.Join(fields, " ")
	}
	return strings.Join(fields[:n], " ")
}

// provisionalWorkspaceTitle is the untitled-workspace sidebar label: first
// non-empty line of the message, at most workspaceTitleWords, rune-capped.
// Empty / image-only / the default title itself yield "" (leave New Workspace).
func provisionalWorkspaceTitle(text string) string {
	text = stripAttachmentMarkers(text)
	line := ""
	for _, l := range strings.Split(text, "\n") {
		l = strings.TrimSpace(l)
		if l != "" {
			line = l
			break
		}
	}
	if line == "" {
		return ""
	}
	title := hardTruncateRunes(truncateWords(line, workspaceTitleWords), workspaceTitleLimit)
	if title == "" || title == defaultWorkspaceTitle {
		return ""
	}
	return title
}

// maybeApplyProvisionalWorkspaceTitle sets Goal/Short from the first user
// message while the workspace is still untitled. Explicit create names are
// left alone; rename_workspace overwrites later.
func (e *Engine) maybeApplyProvisionalWorkspaceTitle(ws domain.Workspace, text string) bool {
	if ws.HasRun() || !isUnnamedWorkspace(ws) {
		return false
	}
	title := provisionalWorkspaceTitle(text)
	if title == "" {
		return false
	}
	updated, ok := e.Store.UpdateWorkspace(ws.ID, func(lp *domain.Workspace) {
		if lp.HasRun() || !isUnnamedWorkspace(*lp) {
			return
		}
		lp.Goal = title
		lp.Short = truncate(title, 48)
	})
	if !ok || updated.Goal != title {
		return false
	}
	e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
	return true
}

// ApplyProvisionalTitles stamps the first-message workspace and chat tab
// titles for a send queued behind worktree setup, so they do not wait for the
// turn to start. SendWorkspaceTurn applies the same titles if this never ran.
func (e *Engine) ApplyProvisionalTitles(workspaceID, code, text string) {
	if isChatRetryDisplay(text) {
		return
	}
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return
	}
	if e.maybeApplyProvisionalWorkspaceTitle(ws, text) {
		e.earlyTitles.Store(workspaceID, earlyWorkspaceTitle{
			goal: provisionalWorkspaceTitle(text), prevGoal: ws.Goal, prevShort: ws.Short,
		})
	}
	if title := e.maybeApplyProvisionalChatTitle(workspaceID, code, text); title != "" {
		e.earlyTitles.Store(workspaceID+"\x00"+code, title)
	}
}

type earlyWorkspaceTitle struct{ goal, prevGoal, prevShort string }

// dropEarlyTitles undoes titles ApplyProvisionalTitles stamped from a queued
// message when the first real send would name differently (the queued row was
// removed), so the send re-derives them and still gets the first-session nudge.
func (e *Engine) dropEarlyTitles(ws domain.Workspace, code, nameFrom string) domain.Workspace {
	if v, ok := e.earlyTitles.LoadAndDelete(ws.ID); ok {
		t := v.(earlyWorkspaceTitle)
		if title := provisionalWorkspaceTitle(nameFrom); title != "" && title != t.goal {
			if updated, ok := e.Store.UpdateWorkspace(ws.ID, func(lp *domain.Workspace) {
				if lp.Goal == t.goal {
					lp.Goal, lp.Short = t.prevGoal, t.prevShort
				}
			}); ok {
				ws = updated
			}
		}
	}
	if v, ok := e.earlyTitles.LoadAndDelete(ws.ID + "\x00" + code); ok {
		if title := firstUserChatTitle(nameFrom); title != "" && title != v.(string) {
			if sess, ok := e.Store.SessionForLoopAgent(ws.ID, code); ok {
				var cfg agentConfig
				_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
				if cfg.Title == v.(string) {
					_ = e.Store.PatchSessionConfigTitle(sess.ID, "")
				}
			}
		}
	}
	return ws
}

// sanitizeTitle cleans a model reply into a UI-safe title of at most maxLen.
func sanitizeTitle(raw string, maxLen int) string {
	s := strings.TrimSpace(raw)
	if s == "" {
		return ""
	}
	// Models sometimes wrap titles in quotes or add a trailing period.
	s = strings.Trim(s, "\"'`")
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		s = strings.TrimSpace(s[:i])
	}
	s = strings.TrimRight(s, " .,;:!")
	s = truncate(strings.TrimSpace(s), maxLen)
	if s == "" || s == defaultWorkspaceTitle {
		return ""
	}
	return s
}

// sanitizeWorkspaceTitle cleans a model reply into a sidebar-safe title.
func sanitizeWorkspaceTitle(raw string) string {
	return sanitizeTitle(raw, 60)
}

// maybeApplyProvisionalChatTitle writes the first-message tab label so later
// ListWorkspaceChats / ListClosedWorkspaceChats can stay off the transcript.
// An LLM rename_session overwrites this; retry must not stamp the sentinel.
func (e *Engine) maybeApplyProvisionalChatTitle(workspaceID, code, text string) string {
	if code != maestroAgentCode && !isPairChatCode(code) {
		return ""
	}
	// Legacy tabs have an empty config title; only stamp the first send so a
	// follow-up cannot rewrite the label the list used to derive from the
	// first user line.
	if !e.isFirstChatMessage(workspaceID, code) {
		return ""
	}
	title := firstUserChatTitle(text)
	if title == "" {
		return ""
	}
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return ""
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	if strings.TrimSpace(cfg.Title) != "" {
		return ""
	}
	if err := e.Store.PatchSessionConfigTitle(sess.ID, title); err != nil {
		return ""
	}
	e.Bus.Publish(Event{Name: "workspace.chat_tab", Data: map[string]any{
		"workspaceId": workspaceID,
		"code":        code,
		"title":       title,
	}})
	return title
}

// firstUserChatTitle is the first non-empty user line for a chat tab.
// Display width is applied by chatTabTitle (truncate) so stored and
// derived labels cap the same way. The 200-rune cap matches rename_session.
// Empty / retry-sentinel yield "".
func firstUserChatTitle(text string) string {
	if isChatRetryDisplay(text) {
		return ""
	}
	text = stripAttachmentMarkers(text)
	line := ""
	for _, l := range strings.Split(text, "\n") {
		l = strings.TrimSpace(l)
		if l != "" {
			line = l
			break
		}
	}
	if line == "" || isChatRetryDisplay(line) {
		return ""
	}
	return hardTruncateRunes(line, 200)
}

// chatStream collects one interactive chat turn's reply text (for the bound call's
// return value) and captures the provider session id. Live streaming to the UI is
// NOT done here — the mirroringRunner's sessionBroadcast publishes every turn's
// events (session.delta/thinking/step/tool_result/notice) tagged by agent code, one
// publisher for chat and loop specialists alike.
type chatStream struct {
	e           *Engine
	workspaceID string
	agentCode   string
	onInit      func(sessionID string) // optional per-caller session-id capture

	// harvestCreatePlan adopts Cursor createPlan args into the workspace Plan
	// when the chat is in Plan mode (submit_plan is preferred but not enforceable).
	harvestCreatePlan bool

	reply    strings.Builder
	gotDelta bool
}

// text returns the turn's accumulated assistant text.
func (c *chatStream) text() string { return strings.TrimSpace(c.reply.String()) }

func (c *chatStream) emit(ev ParsedEvent) {
	if ev.nested() {
		return
	}
	switch ev.Kind {
	case KindTextDelta:
		c.gotDelta = true
		c.reply.WriteString(ev.Text)
	case KindText:
		if c.gotDelta {
			return
		}
		c.reply.WriteString(ev.Text)
	case KindInit:
		if c.onInit != nil {
			c.onInit(ev.SessionID)
		}
	case KindStep:
		if c.harvestCreatePlan && isCreatePlanTool(ev.Tool) {
			if p := createPlanBody(ev.ToolInputRaw); p != "" {
				c.e.adoptProsePlan(c.workspaceID, c.agentCode, p)
			}
		}
	}
}

// WorkspaceChatItems returns the full agent-chat transcript for one coding-agent
// chat tab (code PA, PA2, …; empty → PA). See sessionChatItems.
func (e *Engine) WorkspaceChatItems(workspaceID, code string) []domain.ChatItem {
	return e.sessionChatItems(workspaceID, normalizePairChatCode(code))
}

// sessionChatItems flattens the durable session transcript for one (workspace, agent
// code) into UI-ready chat items: user/assistant text, reasoning (thinking), and the
// agent's tool calls, in order. History comes from the session store — the SINGLE
// source of truth (mirror.go records text + thinking + tool_use losslessly) — so any
// chat surface (the workspace agent, the loop Maestro) reads the same way and shows
// the same fidelity live and on reload. Empty when the agent hasn't run yet.
func (e *Engine) sessionChatItems(workspaceID, agentCode string) []domain.ChatItem {
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, agentCode)
	if !ok {
		return nil
	}
	msgs, err := e.Store.SessionTranscript(sess.ID)
	if err != nil {
		return nil
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	items := flattenTranscript(msgs, cfg.Model, cfg.Flags)
	if ws, ok := e.Store.Workspace(workspaceID); ok {
		e.markCheckpoints(ws, msgs, items)
	}
	return items
}

// flattenTranscript turns a session's messages into UI-ready chat items.
// contentItems drops turn footers, leaving only conversation content. Tests that
// assert on what was said use this so a metadata row does not shift their
// indices.
func contentItems(items []domain.ChatItem) []domain.ChatItem {
	out := items[:0:0]
	for _, it := range items {
		if it.Kind != "turn" {
			out = append(out, it)
		}
	}
	return out
}

// rawToolRef is what a tool RESULT needs to be rendered: results are stored in
// the provider's own envelope with a per-tool shape, so unpacking one requires
// remembering which provider and which tool produced it.
type rawToolRef struct {
	provider string
	name     string
}

// userMessageIsChatPrompt reports whether a role=user message is a human chat
// turn (text/image) rather than a provider tool_result carrier.
func userMessageIsChatPrompt(m domain.Message) bool {
	hasContent := false
	for _, b := range m.Content {
		switch b.Type {
		case domain.BlockToolResult, domain.BlockToolUse, domain.BlockThinking:
			return false
		case domain.BlockText, domain.BlockImage:
			hasContent = true
		}
	}
	return hasContent
}

// flattenUserMessage rebuilds a user ChatItem from text + image content blocks,
// restoring ⟦attachment:id⟧ markers so the UI can place thumbnails inline.
// Per text block, the bubble body uses display_text when set, else text (wire),
// collapsing known Coder dispatch prompts to the same short labels as /fix-findings.
// Once a display_text label has been used, later blocks with empty display_text
// are host wire (fork transcript, skill wrap) and must not join the bubble.
func flattenUserMessage(m domain.Message) (domain.ChatItem, bool) {
	var text strings.Builder
	var atts []domain.ChatImageRef
	has := false
	usedDisplay := false
	for _, b := range m.Content {
		switch b.Type {
		case domain.BlockText:
			if p, ok := b.AsText(); ok {
				shown := p.DisplayText
				if shown != "" {
					usedDisplay = true
					if shown == displayHiddenSentinel {
						continue
					}
				} else if usedDisplay {
					continue
				} else {
					shown = p.Text
					if label := coderDispatchDisplay(shown); label != "" {
						shown = label
					} else {
						stripped := userTextAfterForkPreamble(shown)
						if stripped != shown {
							usedDisplay = true
						}
						shown = stripped
					}
				}
				if shown == chatRetryDisplaySentinel || isModeSwitchContinueDisplay(shown) {
					continue
				}
				text.WriteString(shown)
				if strings.TrimSpace(shown) != "" {
					has = true
				}
			}
		case domain.BlockImage:
			if p, ok := b.AsImage(); ok {
				has = true
				marker := AttachmentMarker(p.ID)
				if !strings.Contains(text.String(), marker) {
					text.WriteString(marker)
				}
				atts = append(atts, domain.ChatImageRef{
					ID: p.ID, Name: p.Name, Mime: p.Mime,
					URL: "/" + filepath.ToSlash(p.Path),
				})
			}
		}
	}
	if !has {
		return domain.ChatItem{}, false
	}
	return domain.ChatItem{
		Role: "user", Kind: "text", Text: text.String(), Attachments: atts,
		Ts: chatItemTS(m, domain.ContentBlock{}),
	}, true
}

// userTextAfterForkPreamble returns the human message that follows a fork
// inject when display_text was not stored. The copied transcript is prepended
// for the provider; the bubble should only show what the user typed after
// </transcript>. Empty when the lead is present but nothing follows the
// close tag (image-first composer). Unchanged when the lead is absent.
func userTextAfterForkPreamble(s string) string {
	if !strings.Contains(s, forkPreambleLead) {
		return s
	}
	const close = "</transcript>"
	i := strings.LastIndex(s, close)
	if i < 0 {
		return s
	}
	rest := strings.TrimLeft(s[i+len(close):], "\n")
	if strings.TrimSpace(rest) == "" {
		return ""
	}
	return rest
}

// chatItemTS prefers the content-block stamp, then the message's CreatedAt.
func chatItemTS(m domain.Message, b domain.ContentBlock) int64 {
	if b.CreatedAt > 0 {
		return b.CreatedAt
	}
	if m.CreatedAt > 0 {
		return m.CreatedAt
	}
	if m.CompletedAt > 0 {
		return m.CompletedAt
	}
	return m.StartedAt
}

// lastChatPrompt is the stored wire + images for the most recent human chat turn.
type lastChatPrompt struct {
	wire string
	imgs []UserImage
}

// lastChatUserPrompt returns the last human chat prompt in this tab's session
// (already-wrapped wire text plus any persisted images). Chat Retry replays it
// instead of trusting the UI bubble, which is display_text (a prompt-action
// label, a short ask_user answer, …).
func (e *Engine) lastChatUserPrompt(workspaceID, code string) (lastChatPrompt, bool) {
	if e == nil || e.Store == nil {
		return lastChatPrompt{}, false
	}
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return lastChatPrompt{}, false
	}
	msgs, err := e.Store.SessionTranscript(sess.ID)
	if err != nil || len(msgs) == 0 {
		return lastChatPrompt{}, false
	}
	for i := len(msgs) - 1; i >= 0; i-- {
		m := msgs[i]
		if m.Role != "user" || !userMessageIsChatPrompt(m) || userMessageIsModeSwitchContinue(m) {
			continue
		}
		// Stop at the newest chat prompt. Falling through to an older one
		// would retry the wrong turn (e.g. image-only whose file is gone).
		return reconstructChatPrompt(m)
	}
	return lastChatPrompt{}, false
}

// firstChatUserPrompt is the oldest human chat prompt on this tab. Loop
// engage may be a later turn; prefer stampLoopGoalImages when present.
func (e *Engine) firstChatUserPrompt(workspaceID, code string) (lastChatPrompt, bool) {
	if e == nil || e.Store == nil {
		return lastChatPrompt{}, false
	}
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return lastChatPrompt{}, false
	}
	msgs, err := e.Store.SessionTranscript(sess.ID)
	if err != nil || len(msgs) == 0 {
		return lastChatPrompt{}, false
	}
	for _, m := range msgs {
		if m.Role != "user" || !userMessageIsChatPrompt(m) || userMessageIsModeSwitchContinue(m) {
			continue
		}
		return reconstructChatPrompt(m)
	}
	return lastChatPrompt{}, false
}

// loopGoalImages is the opening paste set for this loop: ids recorded at
// engage/create when present, else images on the first chat turn (legacy).
func (e *Engine) loopGoalImages(workspaceID, code string) []UserImage {
	if ids, ok := e.chatLoopGoalImages(workspaceID, code); ok {
		return userImagesByIDs(workspaceID, ids)
	}
	if p, ok := e.firstChatUserPrompt(workspaceID, code); ok {
		return p.imgs
	}
	return nil
}

func (e *Engine) chatLoopGoalImages(workspaceID, code string) ([]string, bool) {
	if e == nil || e.Store == nil || strings.TrimSpace(code) == "" {
		return nil, false
	}
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return nil, false
	}
	var cfg agentConfig
	if err := json.Unmarshal([]byte(sess.ConfigJSON), &cfg); err != nil {
		return nil, false
	}
	if cfg.GoalImages == nil {
		return nil, false
	}
	return *cfg.GoalImages, true
}

// reconstructChatPrompt rebuilds the provider wire (text + attachment markers)
// and reloads image bytes from disk.
func reconstructChatPrompt(m domain.Message) (lastChatPrompt, bool) {
	var text strings.Builder
	var imgs []UserImage
	has := false
	for _, b := range m.Content {
		switch b.Type {
		case domain.BlockText:
			if p, ok := b.AsText(); ok {
				if p.Text == chatRetryDisplaySentinel {
					continue
				}
				text.WriteString(p.Text)
				if strings.TrimSpace(p.Text) != "" {
					has = true
				}
			}
		case domain.BlockImage:
			if p, ok := b.AsImage(); ok {
				img, ok := userImageFromStored(p)
				if !ok {
					continue
				}
				text.WriteString(AttachmentMarker(p.ID))
				imgs = append(imgs, img)
				has = true
			}
		}
	}
	if !has {
		return lastChatPrompt{}, false
	}
	return lastChatPrompt{wire: text.String(), imgs: imgs}, true
}

func flattenTranscript(msgs []domain.Message, model string, flags map[string]string) []domain.ChatItem {
	var nodes []chatNode
	toolIdx := map[string]int{}        // internal tool_use id -> nodes index, to attach results
	toolRaw := map[string]rawToolRef{} // internal tool_use id -> how to unpack its result
	push := func(it domain.ChatItem, parent string) int {
		nodes = append(nodes, chatNode{item: it, parent: parent})
		return len(nodes) - 1
	}
	// After a crash, a tool_use may have no result. Live turns still have a
	// streaming/pending row; finished transcripts should not keep spinning.
	// Messages with no stream status (unit tests / legacy) keep the old
	// "no result ⇒ pending" behavior.
	anyStatus, sessionLive := false, false
	for _, m := range msgs {
		if m.StreamStatus != "" {
			anyStatus = true
		}
		if m.StreamStatus == domain.StreamStreaming || m.StreamStatus == domain.StreamPending {
			sessionLive = true
		}
	}
	for _, m := range msgs {
		// Human chat turns interleave text + image blocks. Collapse those into one
		// ChatItem so the bubble keeps marker positions + thumbs. Provider
		// tool_result rows also arrive as role=user — leave those on the normal
		// path so results still attach to their tool_use rows.
		if m.Role == "user" && userMessageIsChatPrompt(m) {
			if it, ok := flattenUserMessage(m); ok {
				push(it, "")
			}
			if m.DurationMS > 0 {
				push(turnChatItem(m, model, flags), "")
			}
			continue
		}
		for _, b := range m.Content {
			parent := b.ParentToolUseID()
			switch b.Type {
			case domain.BlockText:
				if p, ok := b.AsText(); ok && strings.TrimSpace(p.Text) != "" {
					shown := p.Text
					if p.DisplayText != "" {
						shown = p.DisplayText
					}
					push(domain.ChatItem{Role: m.Role, Kind: "text", Text: shown, Ts: chatItemTS(m, b)}, parent)
				}
			case domain.BlockThinking:
				// Redacted thinking (headless withholds the text) still surfaces as a
				// "Thought ~N tokens" item via the stamped estimate.
				if p, ok := b.AsThinking(); ok && (strings.TrimSpace(p.Thinking) != "" || p.EstimatedTokens > 0) {
					push(domain.ChatItem{Role: "assistant", Kind: "thinking", Text: p.Thinking, Tokens: p.EstimatedTokens, Ts: chatItemTS(m, b)}, parent)
				}
			case domain.BlockCompact:
				if p, ok := b.AsCompact(); ok {
					push(domain.ChatItem{
						Role: "assistant", Kind: "compact",
						Text: p.Summary, Tokens: p.PreTokens, Description: p.Trigger,
						Ts: chatItemTS(m, b),
					}, parent)
				}
			case domain.BlockFallback:
				if p, ok := b.AsFallback(); ok && strings.TrimSpace(p.To) != "" {
					to := claudeModelLabel(p.To)
					if to == "" {
						to = p.To
					}
					from := claudeModelLabel(p.From)
					if from == "" {
						from = p.From
					}
					push(domain.ChatItem{
						Role: "assistant", Kind: "fallback",
						Text: to, Description: from,
						Ts: chatItemTS(m, b),
					}, parent)
				}
			case domain.BlockToolUse:
				if p, ok := b.AsToolUse(); ok {
					// Blocks hold the provider's RAW name and args; ChatItem is a
					// UI-ready view, so it carries the common vocabulary. Deriving
					// it here (rather than at write time) means a mapping fix
					// re-renders history instead of only helping new turns.
					d := displayTool(m.Provider, p.Name, p.Input)
					i := push(domain.ChatItem{
						Role: "assistant", Kind: "tool",
						Tool:        d.Name,
						ToolInput:   summarizeToolInput(d.Name, d.Input),
						Description: toolDescription(d.Name, d.Input),
						Input:       chatToolInput(d.Name, d.Input),
						ToolID:      p.ProviderToolUseID,
						Ts:          chatItemTS(m, b),
					}, parent)
					// Pending until a result block attaches below — unless this
					// is a finished transcript with no live streaming row.
					nodes[i].item.ResultPending = !anyStatus || sessionLive
					toolIdx[p.ID] = i
					toolRaw[p.ID] = rawToolRef{provider: m.Provider, name: p.Name}
				}
			case domain.BlockToolResult:
				if p, ok := b.AsToolResult(); ok {
					if i, seen := toolIdx[p.ToolUseID]; seen {
						// Results are stored in the provider's own envelope, so
						// they need the originating call's provider+name to unpack.
						ref := toolRaw[p.ToolUseID]
						nodes[i].item.Result = chatToolResult(nodes[i].item.Tool, displayToolResult(ref.provider, ref.name, p.Result))
						nodes[i].item.ResultError = p.IsError
						nodes[i].item.ResultTruncated = p.Truncated
						nodes[i].item.ResultPending = false
					}
				}
			}
		}
		// A turn ends where the provider stamped its duration (the last message of
		// the turn). Emitting a footer here — rather than
		// hanging the time off whatever content block happened to come last — keeps
		// it correct for turns that end on a tool call rather than text.
		if m.DurationMS > 0 {
			push(turnChatItem(m, model, flags), "")
		}
	}
	return assembleChatItems(nodes)
}

// chatNode is one flattened row plus the provider tool-use id it nests under
// (empty = top-level transcript).
type chatNode struct {
	item   domain.ChatItem
	parent string
}

// assembleChatItems hangs nested subagent rows under their Agent/Task parent
// and drops orphans whose parent was never seen — they must not leak as
// siblings in the main transcript.
func assembleChatItems(nodes []chatNode) []domain.ChatItem {
	children := map[string][]domain.ChatItem{}
	var top []domain.ChatItem
	for _, n := range nodes {
		if n.parent == "" {
			top = append(top, n.item)
			continue
		}
		children[n.parent] = append(children[n.parent], n.item)
	}
	var attach func([]domain.ChatItem) []domain.ChatItem
	attach = func(items []domain.ChatItem) []domain.ChatItem {
		for i := range items {
			if items[i].Kind != "tool" || items[i].ToolID == "" {
				continue
			}
			ch, ok := children[items[i].ToolID]
			if !ok {
				continue
			}
			delete(children, items[i].ToolID)
			items[i].Children = attach(ch)
		}
		return items
	}
	return attach(top)
}

// turnChatItem is the footer for a finished agent turn: wall time plus context
// usage so the composer meter reloads from the transcript.
func turnChatItem(m domain.Message, model string, flags map[string]string) domain.ChatItem {
	return domain.ChatItem{
		Role:                "assistant",
		Kind:                "turn",
		DurationMS:          m.DurationMS,
		MessageID:           m.ID,
		InputTokens:         m.InputTokens,
		CacheCreationTokens: m.CacheCreationTokens,
		CacheReadTokens:     m.CacheReadTokens,
		ContextWindow:       contextWindowForMessage(m, model, flags),
		Ts:                  chatItemTS(m, domain.ContentBlock{}),
	}
}

// contextWindowForMessage resolves the meter limit for a finished turn.
// Claude uses model + dock flags (always-1M models; 4.6's 200K↔1M). Cursor
// falls back to the model table when older turns stored tokens without a limit.
func contextWindowForMessage(m domain.Message, sessionModel string, flags map[string]string) int64 {
	used := m.InputTokens + m.CacheReadTokens + m.CacheCreationTokens
	if m.Provider == domain.ProviderClaudeCode {
		if used <= 0 && m.ContextWindow <= 0 {
			return 0
		}
		model := m.ModelID
		if model == "" {
			model = sessionModel
		}
		if model != "" || hasClaudeContextFlag(flags) {
			return ResolveClaudeContextWindow(model, flags)
		}
		if m.ContextWindow > 0 {
			return m.ContextWindow
		}
		return ResolveClaudeContextWindow("", flags)
	}
	if m.ContextWindow > 0 {
		return m.ContextWindow
	}
	if used <= 0 {
		return 0
	}
	if m.Provider == domain.ProviderCursor {
		// Turn-final tool_result rows often lack ModelID (assistant-only stamp);
		// fall back to the session's qualified ref and strip the provider prefix
		// so LimitsFor can match bare ids like "gemini-3.1-pro".
		model := m.ModelID
		if model == "" {
			model = sessionModel
		}
		if ref := domain.ParseModelRef(model); ref.ID != "" {
			model = ref.ID
		}
		return ResolveCursorContextWindow(model, flags)
	}
	return 0
}

// WorkspaceSessions returns specialist sessions for one chat (store order:
// oldest first), flattened for the Plan panel (todos) and Loop chat cockpit
// (specialist cards). Pair (PA*) and Maestro transcripts load via
// WorkspaceChatItems for the open tab — pulling them here re-read every
// message_content row on workspace switch and the UI discarded them. Sessions
// with no transcript yet (e.g. a dock mode flip that created the durable row
// before the first send) are omitted.
//
// An empty agentCode loads all specialists owned by the chat; otherwise only
// that specialist is read. Legacy unowned sessions belong to PA and MA.
func (e *Engine) WorkspaceSessions(workspaceID, ownerCode, agentCode string) []domain.SessionView {
	var out []domain.SessionView
	for _, s := range e.Store.SessionsForLoop(workspaceID) {
		var cfg agentConfig
		_ = json.Unmarshal([]byte(s.ConfigJSON), &cfg)
		if cfg.Code == maestroAgentCode || isPairChatCode(cfg.Code) {
			continue
		}
		owns := cfg.OwnerCode == ownerCode || cfg.OwnerCode == "" && (ownerCode == pairAgentCode || ownerCode == maestroAgentCode)
		if !owns || (agentCode != "" && cfg.Code != agentCode) {
			continue
		}
		msgs, err := e.Store.SessionTranscript(s.ID)
		if err != nil || len(msgs) == 0 {
			continue
		}
		out = append(out, domain.SessionView{
			ID: s.ID, Code: cfg.Code, Name: cfg.Name, Kind: s.Kind, Status: s.Status,
			OwnerCode: cfg.OwnerCode, CreatedAt: s.CreatedAt, UpdatedAt: s.UpdatedAt,
			Items: flattenTranscript(msgs, cfg.Model, cfg.Flags),
		})
	}
	return out
}

// WorkspaceFacts returns the loop-level facts — outcomes and deviations from the
// durable events table — with real timestamps (FeedItem.Ts), so the Loop chat
// cockpit can interleave them with Maestro turns and specialist cards.
func (e *Engine) WorkspaceFacts(workspaceID string) []domain.FeedItem {
	rows := e.loopFactRows(workspaceID)
	out := make([]domain.FeedItem, 0, len(rows))
	for _, r := range rows {
		it := r.item
		it.Ts = r.ts
		out = append(out, it)
	}
	return out
}

// ensurePairAgent makes sure a coding-agent record exists for the chat tab so its
// session id has somewhere durable to live and the Agents view shows it.
// New records start idle — List/Create/Close also call this, and seeding
// "working" falsely lit the sidebar (and now the chat busy indicator) with no
// turn in flight. SendWorkspaceMessage sets "working" immediately after ensure;
// for a brand-new agent that path creates with "working" via ensurePairAgentRun
// so an idle→working reorder on the Wails bridge cannot suppress the first-turn
// running indicator.
func (e *Engine) ensurePairAgent(workspaceID, code, model string) {
	e.ensurePairAgentWithStatus(workspaceID, code, model, "idle")
}

// ensurePairAgentRun is ensurePairAgent for the Send path: create already
// "working" so the first agent.updated is live (avoids idle/working reorder).
func (e *Engine) ensurePairAgentRun(workspaceID, code, model string) {
	e.ensurePairAgentWithStatus(workspaceID, code, model, "working")
}

func (e *Engine) ensurePairAgentWithStatus(workspaceID, code, model, status string) {
	code = normalizePairChatCode(code)
	if _, ok := e.Store.Agent(workspaceID, code); ok {
		return
	}
	if status == "" {
		status = "idle"
	}
	e.upsertAgent(workspaceID, domain.Agent{
		ID: workspaceID + "-" + code, WorkspaceID: workspaceID, Code: code,
		Name: "Agent", Role: "codes alongside you", Model: model,
		Status: status, Color: "var(--cyan)", Cur: -1,
	})
}

// pairChatAgents returns coding-agent chat tabs for a workspace, ordered PA, PA2, ….
func (e *Engine) pairChatAgents(workspaceID string) []domain.Agent {
	var out []domain.Agent
	for _, a := range e.Store.Agents(workspaceID) {
		if isPairChatCode(a.Code) {
			out = append(out, a)
		}
	}
	sort.Slice(out, func(i, j int) bool {
		return pairChatCodeNum(out[i].Code) < pairChatCodeNum(out[j].Code)
	})
	return out
}

// chatTabTitle is the tab label: stored title, else the first user message
// truncated, else "New chat". Must not flatten the full transcript — listing
// tabs on a workspace switch used to re-read every tool block for this.
func (e *Engine) chatTabTitle(workspaceID, code string) string {
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return "New chat"
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	if t := strings.TrimSpace(cfg.Title); t != "" {
		return truncate(t, chatTabTitleLimit)
	}
	if t := firstUserChatTitle(e.Store.SessionFirstUserPrompt(sess.ID)); t != "" {
		return truncate(t, chatTabTitleLimit)
	}
	return "New chat"
}

// ListWorkspaceChats returns every open chat tab for a workspace. A hand-driven
// workspace always has at least the default PA tab. Loop is a mode on those
// chats. A Maestro (MA) conductor stays in the
// strip as a peer tab when it is the loop chat — creating a pair tab must not
// hide it. A run with no pair tabs still lists MA so the strip is never empty.
// A saved ChatTabOrder (drag-drop) overrides default order.
func (e *Engine) ListWorkspaceChats(workspaceID string) []domain.WorkspaceChatTab {
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return nil
	}
	return applyChatTabOrder(e.workspaceChatTabs(ws), ws.ChatTabOrder)
}

// workspaceChatTabs is the default-ordered strip (Maestro first when it is a
// real chat tab, then PA, PA2, …) before a saved ChatTabOrder is applied.
func (e *Engine) workspaceChatTabs(ws domain.Workspace) []domain.WorkspaceChatTab {
	agents := e.pairChatAgents(ws.ID)
	if len(agents) == 0 && !ws.HasRun() && !e.chatLifeReserved(ws.ID) {
		e.ensurePairAgent(ws.ID, pairAgentCode, "")
		agents = e.pairChatAgents(ws.ID)
	}
	out := make([]domain.WorkspaceChatTab, 0, len(agents)+1)
	if e.includeMaestroChatTab(ws, len(agents)) {
		out = append(out, e.maestroChatTab(ws.ID))
	}
	for _, a := range agents {
		tab := domain.WorkspaceChatTab{
			Code:   a.Code,
			Title:  e.chatTabTitle(ws.ID, a.Code),
			Unseen: a.Unseen,
		}
		if sess, ok := e.Store.SessionForLoopAgent(ws.ID, a.Code); ok {
			tab.UpdatedAt = sess.UpdatedAt
			var cfg agentConfig
			_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
			tab.Mode = cfg.Mode
			tab.PromptGroup = cfg.PromptGroup
		}
		e.fillChatTabLoop(&tab, ws.ID, a.Code)
		out = append(out, tab)
	}
	return out
}

// includeMaestroChatTab reports whether MA belongs on the strip as a peer chat.
// HasRun alone does not inject it (a PA* tab can be the loop). Closing MA
// clears LoopChatCode so the tab stays gone; reopen restores the agent row.
func (e *Engine) includeMaestroChatTab(ws domain.Workspace, _ int) bool {
	if strings.TrimSpace(ws.LoopChatCode) == maestroAgentCode {
		return true
	}
	if _, ok := e.Store.Agent(ws.ID, maestroAgentCode); !ok {
		return false
	}
	return e.chatLoopStarted(ws.ID, maestroAgentCode) || e.chatHasUserMessage(ws.ID, maestroAgentCode)
}

func (e *Engine) maestroChatTab(workspaceID string) domain.WorkspaceChatTab {
	tab := domain.WorkspaceChatTab{
		Code: maestroAgentCode, Title: e.chatTabTitle(workspaceID, maestroAgentCode),
		Mode: domain.ChatModeLoop,
	}
	if sess, ok := e.Store.SessionForLoopAgent(workspaceID, maestroAgentCode); ok {
		tab.UpdatedAt = sess.UpdatedAt
	}
	if a, ok := e.Store.Agent(workspaceID, maestroAgentCode); ok {
		tab.Unseen = a.Unseen
	}
	e.fillChatTabLoop(&tab, workspaceID, maestroAgentCode)
	return tab
}

// applyChatTabOrder rearranges tabs to match a saved strip order. Unknown /
// closed codes are skipped. Tabs missing from the save (a new chat, a first
// Loop after engage) keep their default relative positions: Loop still leads
// until the human has placed it; other new tabs append.
func applyChatTabOrder(tabs []domain.WorkspaceChatTab, order []string) []domain.WorkspaceChatTab {
	if len(order) == 0 || len(tabs) < 2 {
		return tabs
	}
	byCode := make(map[string]domain.WorkspaceChatTab, len(tabs))
	for _, t := range tabs {
		byCode[t.Code] = t
	}
	inOrder := make(map[string]bool, len(order))
	for _, code := range order {
		inOrder[code] = true
	}
	out := make([]domain.WorkspaceChatTab, 0, len(tabs))
	seen := make(map[string]bool, len(tabs))
	if t, ok := byCode[maestroAgentCode]; ok && !inOrder[maestroAgentCode] {
		out = append(out, t)
		seen[maestroAgentCode] = true
	}
	for _, code := range order {
		t, ok := byCode[code]
		if !ok || seen[code] {
			continue
		}
		out = append(out, t)
		seen[code] = true
	}
	for _, t := range tabs {
		if seen[t.Code] {
			continue
		}
		out = append(out, t)
	}
	return out
}

// ReorderWorkspaceChats persists a new Agent-panel tab order. codes must be a
// permutation of the currently open tab codes. Returns the open tabs in that
// order, or the current list plus an error when the set does not match.
func (e *Engine) ReorderWorkspaceChats(workspaceID string, codes []string) ([]domain.WorkspaceChatTab, error) {
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return nil, fmt.Errorf("reorder chats: unknown workspace %q", workspaceID)
	}
	tabs := e.workspaceChatTabs(ws)
	if err := chatTabCodesPermutation(tabs, codes); err != nil {
		return applyChatTabOrder(tabs, ws.ChatTabOrder), err
	}
	e.Store.UpdateWorkspace(workspaceID, func(w *domain.Workspace) {
		w.ChatTabOrder = append([]string(nil), codes...)
	})
	return applyChatTabOrder(tabs, codes), nil
}

func chatTabCodesPermutation(tabs []domain.WorkspaceChatTab, codes []string) error {
	if len(codes) != len(tabs) {
		return fmt.Errorf("reorder chats: got %d codes, have %d tabs", len(codes), len(tabs))
	}
	have := make(map[string]bool, len(tabs))
	for _, t := range tabs {
		have[t.Code] = true
	}
	seen := make(map[string]bool, len(codes))
	for _, code := range codes {
		if !have[code] {
			return fmt.Errorf("reorder chats: unknown tab %q", code)
		}
		if seen[code] {
			return fmt.Errorf("reorder chats: duplicate tab %q", code)
		}
		seen[code] = true
	}
	return nil
}

// CreateWorkspaceChat opens a new coding-agent chat tab (next free PA* code).
// A hand-driven workspace always has the default PA tab (same as ListWorkspaceChats),
// so the first "new chat" is PA2. A loop with no pair agents starts at PA.
func (e *Engine) CreateWorkspaceChat(workspaceID string) domain.WorkspaceChatTab {
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return domain.WorkspaceChatTab{}
	}
	unlock, ok := e.lockChatLife(workspaceID, pairAgentCode)
	defer unlock()
	if !ok {
		return domain.WorkspaceChatTab{}
	}
	if !ws.HasRun() && len(e.pairChatAgents(workspaceID)) == 0 {
		e.ensurePairAgent(workspaceID, pairAgentCode, "")
	}
	code := e.newPairChat(ws)
	if ws.IsPRChatHost() {
		return domain.WorkspaceChatTab{Code: code, Title: "New chat", Mode: domain.ChatModeReview}
	}
	return domain.WorkspaceChatTab{Code: code, Title: "New chat"}
}

// newPairChat adds a fresh pair tab, seeded to review mode on PR chat hosts.
func (e *Engine) newPairChat(ws domain.Workspace) string {
	code := e.nextPairChatCode(ws.ID)
	e.ensurePairAgent(ws.ID, code, "")
	if ws.IsPRChatHost() {
		e.rememberPairChatSelection(ws.ID, code, "", nil, domain.ChatModeReview, nil)
	}
	return code
}

// CloseWorkspaceChat removes a chat tab's agent record. Durable session history
// stays available via ListClosedWorkspaceChats / ReopenWorkspaceChat. Closing the
// last pair tab replaces it with a fresh empty chat on a new code (so
// SessionForLoopAgent does not revive the closed transcript). The walk stops
// and LoopChatCode clears — a later Review & fix must not land on the closed
// conductor. Reopen from recents is the way back. Closing the last MA tab on a
// run also seeds a fresh pair tab instead of resurrecting Loop.
// Returns the remaining tabs.
func (e *Engine) CloseWorkspaceChat(workspaceID, code string) []domain.WorkspaceChatTab {
	code = normalizePairChatCode(code)
	if _, ok := e.Store.Workspace(workspaceID); !ok {
		return nil
	}
	unlock, held := e.lockChatLife(workspaceID, code)
	defer unlock()
	if !held {
		return e.ListWorkspaceChats(workspaceID)
	}
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return nil
	}
	agents := e.pairChatAgents(workspaceID)
	cleared := false
	unseenGone := false
	if len(agents) <= 1 && !ws.HasRun() {
		for _, a := range agents {
			if a.Unseen {
				unseenGone = true
			}
			if e.clearChatInbox(workspaceID, a.Code, false) {
				cleared = true
			}
			e.dropMCPAgent(workspaceID, a.Code)
			e.Store.DeleteAgent(workspaceID, a.Code)
		}
		e.newPairChat(ws)
		if cleared {
			e.publishInboxUpdated()
		}
		if cleared || unseenGone {
			e.publishAttention(workspaceID, "", false)
		}
		return e.ListWorkspaceChats(workspaceID)
	}
	if isPairChatCode(code) || code == maestroAgentCode {
		if a, ok := e.Store.Agent(workspaceID, code); ok && a.Unseen {
			unseenGone = true
		}
		if e.clearChatInbox(workspaceID, code, false) {
			cleared = true
		}
		e.stopLoopByChat(workspaceID, code)
		parked := false
		updated, ok := e.Store.UpdateWorkspace(workspaceID, func(w *domain.Workspace) {
			if strings.TrimSpace(w.LoopChatCode) != code {
				return
			}
			w.LoopChatCode = ""
			if w.State == domain.StateQueued {
				w.State = domain.StatePaused
				w.Phase = "idle"
				w.StatusLabel = statusLabel(domain.StatePaused)
				w.Progress = progressFor(domain.StatePaused)
				w.ElapsedLabel = "paused"
				parked = true
			}
		})
		if ok && parked {
			e.publishLoopUpdated(workspaceID, updated.State, updated.Phase, updated.Iteration)
			e.publishResource()
		}
		e.Store.DeleteAgent(workspaceID, code)
		e.dropMCPAgent(workspaceID, code)
	}
	if cleared {
		e.publishInboxUpdated()
	}
	if cleared || unseenGone {
		e.publishAttention(workspaceID, code, false)
	}
	if remaining := e.ListWorkspaceChats(workspaceID); len(remaining) > 0 {
		return remaining
	}
	e.newPairChat(ws)
	return e.ListWorkspaceChats(workspaceID)
}

// ListClosedWorkspaceChats returns pair-chat sessions that still have a
// transcript but whose agent row was deleted (CloseWorkspaceChat). Newest first.
func (e *Engine) ListClosedWorkspaceChats(workspaceID string) []domain.WorkspaceChatTab {
	if _, ok := e.Store.Workspace(workspaceID); !ok {
		return nil
	}
	open := map[string]bool{}
	for _, a := range e.pairChatAgents(workspaceID) {
		open[a.Code] = true
	}
	if _, ok := e.Store.Agent(workspaceID, maestroAgentCode); ok {
		open[maestroAgentCode] = true
	}
	var out []domain.WorkspaceChatTab
	seen := map[string]bool{}
	for _, s := range e.Store.SessionsForLoop(workspaceID) {
		var cfg agentConfig
		_ = json.Unmarshal([]byte(s.ConfigJSON), &cfg)
		if (!isPairChatCode(cfg.Code) && cfg.Code != maestroAgentCode) || open[cfg.Code] || seen[cfg.Code] {
			continue
		}
		if !e.Store.SessionHasMessages(s.ID) {
			continue
		}
		seen[cfg.Code] = true
		tab := domain.WorkspaceChatTab{
			Code:        cfg.Code,
			Title:       e.chatTabTitle(workspaceID, cfg.Code),
			UpdatedAt:   s.UpdatedAt,
			Mode:        cfg.Mode,
			PromptGroup: cfg.PromptGroup,
		}
		out = append(out, tab)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].UpdatedAt > out[j].UpdatedAt })
	return out
}

// DeleteClosedWorkspaceChat permanently drops a closed pair/MA chat's session
// and specialist sessions it owned (messages a fork still shares are kept).
// Open tabs, in-flight turns, and running loops are refused so the UI can
// restore client state. Returns the remaining closed chats (the recents menu).
func (e *Engine) DeleteClosedWorkspaceChat(workspaceID, code string) ([]domain.WorkspaceChatTab, error) {
	remaining := func() []domain.WorkspaceChatTab {
		out := e.ListClosedWorkspaceChats(workspaceID)
		if out == nil {
			return []domain.WorkspaceChatTab{}
		}
		return out
	}
	if _, ok := e.Store.Workspace(workspaceID); !ok {
		return remaining(), nil
	}
	if code != maestroAgentCode {
		code = normalizePairChatCode(code)
		if !isPairChatCode(code) {
			return remaining(), nil
		}
	}
	unlock, ok := e.lockChatLife(workspaceID, code)
	defer unlock()
	if !ok {
		return remaining(), nil
	}
	if _, open := e.Store.Agent(workspaceID, code); open {
		return remaining(), fmt.Errorf("chat is still open")
	}
	if e.liveChatTurn(workspaceID, code) != nil {
		return remaining(), fmt.Errorf("chat is still running")
	}
	if e.runningLoopRecordByChat(workspaceID, code) != nil {
		return remaining(), fmt.Errorf("chat is still running")
	}
	e.takeModeContinue(workspaceID, code)
	e.resetModeRemounts(workspaceID, code)
	e.invalidateContinueGens(workspaceID, code)
	if err := e.Store.DeleteClosedChat(workspaceID, code); err != nil {
		return remaining(), fmt.Errorf("delete closed chat: %w", err)
	}
	return remaining(), nil
}

// ReopenWorkspaceChat restores a closed pair-chat tab so SessionForLoopAgent
// finds the existing transcript. Unknown/open codes are ignored.
func (e *Engine) ReopenWorkspaceChat(workspaceID, code string) []domain.WorkspaceChatTab {
	if code == maestroAgentCode {
		unlock, ok := e.lockChatLife(workspaceID, code)
		defer unlock()
		if !ok {
			return nil
		}
		if _, ok := e.Store.Workspace(workspaceID); !ok {
			return nil
		}
		if _, ok := e.Store.SessionForLoopAgent(workspaceID, maestroAgentCode); !ok {
			return e.ListWorkspaceChats(workspaceID)
		}
		e.Store.UpdateWorkspace(workspaceID, func(w *domain.Workspace) {
			if strings.TrimSpace(w.LoopChatCode) == "" {
				w.LoopChatCode = maestroAgentCode
			}
		})
		e.ensureMaestroAgent(workspaceID, "")
		return e.ListWorkspaceChats(workspaceID)
	}
	code = normalizePairChatCode(code)
	unlock, ok := e.lockChatLife(workspaceID, code)
	defer unlock()
	if !ok {
		return nil
	}
	if _, ok := e.Store.Workspace(workspaceID); !ok {
		return nil
	}
	if !isPairChatCode(code) {
		return e.ListWorkspaceChats(workspaceID)
	}
	if _, ok := e.Store.SessionForLoopAgent(workspaceID, code); !ok {
		return e.ListWorkspaceChats(workspaceID)
	}
	for _, a := range e.pairChatAgents(workspaceID) {
		if a.Code == code {
			return e.ListWorkspaceChats(workspaceID)
		}
	}
	e.ensurePairAgent(workspaceID, code, "")
	return e.ListWorkspaceChats(workspaceID)
}

// nextPairChatCode picks the next free PA* code, considering both open agents and
// any prior coding-chat sessions so a "fresh" tab never resumes a closed chat.
func (e *Engine) nextPairChatCode(workspaceID string) string {
	max := 0
	for _, a := range e.pairChatAgents(workspaceID) {
		if n := pairChatCodeNum(a.Code); n > max {
			max = n
		}
	}
	for _, s := range e.Store.SessionsForLoop(workspaceID) {
		var cfg agentConfig
		_ = json.Unmarshal([]byte(s.ConfigJSON), &cfg)
		if isPairChatCode(cfg.Code) {
			if n := pairChatCodeNum(cfg.Code); n > max {
				max = n
			}
		}
	}
	return pairChatCodeFromNum(max + 1)
}

// WorkspaceChatSelection returns the last dock selection for a coding-agent chat
// tab (code PA, PA2, …; empty → PA). Prefers the session config (updated every
// send); falls back to the agent record's Model for older sessions that predate
// flags. Empty model means that chat has never sent a turn; empty mode means the
// dock should default to agent.
func (e *Engine) WorkspaceChatSelection(workspaceID, code string) (model string, flags map[string]string, mode string) {
	code = normalizePairChatCode(code)
	if code == maestroAgentCode {
		// The Maestro thread's model comes from Settings and its mode is fixed.
		return "", nil, domain.ChatModeLoop
	}
	if sess, ok := e.Store.SessionForLoopAgent(workspaceID, code); ok {
		var cfg agentConfig
		_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
		if strings.TrimSpace(cfg.Model) != "" {
			return cfg.Model, cfg.Flags, cfg.Mode
		}
		mode = cfg.Mode
	}
	if a, ok := e.Store.Agent(workspaceID, code); ok && strings.TrimSpace(a.Model) != "" {
		return a.Model, nil, mode
	}
	return "", nil, mode
}

// resolveChatMode picks the effective mode for a chat turn: an explicit dock
// value wins (normalized), else the tab's persisted mode, else agent.
func (e *Engine) resolveChatMode(workspaceID, code, mode string) string {
	if m := strings.TrimSpace(mode); m != "" {
		return domain.NormalizeChatMode(m)
	}
	return domain.NormalizeChatMode(e.chatModeFor(workspaceID, code))
}

// chatModeFor returns a chat tab's persisted mode ("" when never set).
func (e *Engine) chatModeFor(workspaceID, code string) string {
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, normalizePairChatCode(code))
	if !ok {
		return ""
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	return cfg.Mode
}

// refreshStanding is true when Cursor/Codex must re-attach mode notes: an
// agent-initiated remount, or a human dock change since the last run turn.
func (e *Engine) refreshStanding(workspaceID, code, mode, displayText, resume string) bool {
	if isModeSwitchContinueDisplay(displayText) {
		return true
	}
	if resume == "" {
		return false
	}
	last := e.standingModeFor(workspaceID, code)
	// Missing StandingMode (pre-upgrade sessions) is unknown, not Agent —
	// NormalizeChatMode("") would hide an Ask/Plan/Review → Agent first resume.
	return last != mode
}

func (e *Engine) standingModeFor(workspaceID, code string) string {
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, normalizePairChatCode(code))
	if !ok {
		return ""
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	return strings.TrimSpace(cfg.StandingMode)
}

func standingPromptSHA(prompt string) string {
	sum := sha256.Sum256([]byte(prompt))
	return hex.EncodeToString(sum[:])
}

func (e *Engine) standingPromptChanged(workspaceID, code, prompt, resume string) bool {
	if resume == "" {
		return false
	}
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return true
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	last := strings.TrimSpace(cfg.StandingPromptSHA)
	return last != standingPromptSHA(prompt)
}

func (e *Engine) rememberStandingPrompt(workspaceID, code, prompt string) {
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return
	}
	sha := standingPromptSHA(prompt)
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	if cfg.StandingPromptSHA == sha {
		return
	}
	cfg.StandingPromptSHA = sha
	raw, err := json.Marshal(cfg)
	if err != nil {
		return
	}
	_ = e.Store.UpdateSessionConfigJSON(sess.ID, string(raw))
}

// sendLoopChatMessage is one Loop-mode send: this tab is its own loop.
func (e *Engine) sendLoopChatMessage(ws domain.Workspace, code, text string, attachments []domain.ChatAttachment, displayText string) string {
	loopID, policyID := e.chatLoopPolicy(ws.ID, code)
	if loopID == "" {
		loopID = strings.TrimSpace(ws.LoopID)
	}
	if policyID == "" {
		policyID = strings.TrimSpace(ws.PolicyID)
	}
	if e.chatLoopStarted(ws.ID, code) || (ws.HasRun() && loopOwnerCode(ws) == code) {
		return e.sendMaestroMessage(ws.ID, code, text, attachments, displayText)
	}
	unlockLife, ok := e.lockChatLife(ws.ID, code)
	defer unlockLife()
	if !ok {
		return "I can't find that workspace anymore — it may have been removed."
	}
	text, imgs, err := prepareChatAttachments(ws.ID, text, attachments)
	if err != nil {
		return "I couldn't save that image attachment. Try a smaller PNG/JPEG/GIF/WebP."
	}
	if text == "" && len(imgs) == 0 {
		return ""
	}
	// No conductor LLM turn on engage: the provisional label is a
	// placeholder and the planner turn carries the rename nudge.
	// Specialists do the phase work. plan/steer is a follow-up if the
	// human talks.
	nameFrom := strings.TrimSpace(displayText)
	goal := stripAttachmentMarkers(text)
	if goal == "" && len(imgs) > 0 {
		goal = "image attachment"
	}
	if nameFrom == "" {
		nameFrom = goal
	}
	if goal == "" {
		goal = nameFrom
	}
	if nameFrom == "" {
		return ""
	}
	ws = e.dropEarlyTitles(ws, code, nameFrom)
	if nudge := e.renameNudgePrompt(ws.ID, code, domain.ChatModeLoop, nameFrom); nudge != "" {
		e.loopRenameNudge.Store(ws.ID+loopMapSep+code, nudge)
	}
	if !ws.HasRun() {
		on := e.effectiveChatSimplify(ws.ID, code)
		e.rememberChatSimplify(ws.ID, code, on)
		e.rememberChatSimplify(ws.ID, maestroAgentCode, on)
		e.ensurePairAgent(ws.ID, code, "")
		e.maybeApplyProvisionalWorkspaceTitle(ws, nameFrom)
		e.maybeApplyProvisionalChatTitle(ws.ID, code, nameFrom)
		e.Store.UpdateWorkspace(ws.ID, func(w *domain.Workspace) {
			w.LoopChatCode = code
		})
		e.stampChatLoop(ws.ID, code, loopID, policyID, goal, displayText, false)
		e.stampLoopGoalImages(ws.ID, code, imgs)
		e.recordLoopEngageUser(ws.ID, code, text, displayText, imgs)
		unlockLife() // dispatch locks this tab itself
		if _, err := e.StartRunOnWorkspaceWith(ws.ID, goal, loopID, policyID); err != nil {
			return "I couldn't engage the loop: " + err.Error()
		}
		unlock2, held := e.lockChatLife(ws.ID, code)
		defer unlock2()
		if !held {
			return ""
		}
		if _, ok := e.Store.Agent(ws.ID, code); !ok {
			return ""
		}
		e.stampLoopOwner(ws.ID, code, loopID, policyID, goal, displayText)
		return ""
	}
	// Another chat already engaged this worktree. This tab still gets its own
	// walk from its [[loop]] entry — do not resume a sibling checkpoint.
	e.stampChatLoop(ws.ID, code, loopID, policyID, goal, displayText, true)
	e.stampLoopGoalImages(ws.ID, code, imgs)
	e.maybeApplyProvisionalChatTitle(ws.ID, code, nameFrom)
	e.recordLoopEngageUser(ws.ID, code, text, displayText, imgs)
	unlockLife()
	e.startChatLoopWalk(ws.ID, code, loopID, policyID, goal)
	return ""
}

// recordLoopEngageUser persists the user bubble without a conductor LLM turn.
func (e *Engine) recordLoopEngageUser(workspaceID, code, text, displayText string, imgs []UserImage) {
	e.ensurePairAgent(workspaceID, code, "")
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		provider := e.settings().CoderModel().Provider
		sess, ok = e.ensureAgentSession(workspaceID, string(domain.RoleCoder), code, "Agent", provider)
		if !ok {
			return
		}
	}
	newSessionMirror(e.Store, sess.ID, "", sess.Provider).recordUser("", text, displayText, imgs)
}

// startChatLoopWalk begins this chat's graph walk. A live sibling walk keeps
// its goroutine; this one is keyed per chat. An idle workspace re-dispatches
// from the new [[loop]] entry (stale checkpoints from a different graph idle
// immediately as "phase missing").
func (e *Engine) startChatLoopWalk(workspaceID, code, loopID, policyID, goal string) {
	if e.runningLoopByChat(workspaceID, code) != nil {
		return
	}
	unlock, held := e.lockChatLife(workspaceID, code)
	defer unlock()
	if !held {
		return
	}
	if _, ok := e.Store.Agent(workspaceID, code); !ok {
		return
	}
	rec, ok := e.Store.Workspace(workspaceID)
	if !ok || rec.WorktreePath == "" {
		return
	}
	if _, inflight := e.creating.Load(workspaceID); inflight {
		return
	}
	if strings.TrimSpace(loopID) != "" {
		rec.LoopID = loopID
	}
	if strings.TrimSpace(policyID) != "" {
		rec.PolicyID = policyID
	}
	rec.LoopChatCode = code
	display := ""
	if g, d := e.chatLoopGoal(workspaceID, code); g == strings.TrimSpace(goal) {
		display = d
	}
	rec.ReplaceGoal(goal, display)
	if e.hasOpenSiblingWalk(workspaceID, code) {
		unlock()
		if !e.gov.TryAcquireLoop() {
			return
		}
		rec.Checkpoint = nil
		rec.Iteration = 0
		if err := e.dispatchAdditionalChatRun(rec); err != nil {
			e.gov.ReleaseLoop()
			return
		}
		e.publishResource()
		return
	}
	e.Store.UpdateWorkspace(workspaceID, func(w *domain.Workspace) {
		if strings.TrimSpace(loopID) != "" {
			w.LoopID = loopID
		}
		if strings.TrimSpace(policyID) != "" {
			w.PolicyID = policyID
		}
		w.LoopChatCode = code
		w.ReplaceGoal(goal, display)
		w.Checkpoint = nil
		w.Iteration = 0
	})
	if rec.State == domain.StateMerged {
		return
	}
	unlock()
	if e.startWalkHook != nil {
		e.startWalkHook()
	}
	if e.runningLoopByID(workspaceID) != nil {
		if !e.gov.TryAcquireLoop() {
			return
		}
		rec.Checkpoint = nil
		rec.Iteration = 0
		if err := e.dispatchAdditionalChatRun(rec); err != nil {
			e.gov.ReleaseLoop()
			return
		}
		e.publishResource()
		return
	}
	e.setWorkspaceState(workspaceID, domain.StatePaused, "starting")
	e.resume(workspaceID, code)
}

// simplify: plan/findings/verify still live on the workspace. Per-chat copies
// when two loops on one worktree need different plans.

// hasOpenSiblingWalk reports another chat's goroutine still running on this
// worktree (not a walk we just CmdStop'd whose agent row is already gone).
func (e *Engine) hasOpenSiblingWalk(workspaceID, code string) bool {
	code = strings.TrimSpace(code)
	found := false
	e.forEachRunningLoop(workspaceID, func(rl *runningLoop) {
		c := strings.TrimSpace(rl.loop.ChatCode)
		if c == "" || c == code {
			return
		}
		if _, ok := e.Store.Agent(workspaceID, c); ok {
			found = true
		}
	})
	return found
}

func loopOwnerCode(ws domain.Workspace) string {
	if c := strings.TrimSpace(ws.LoopChatCode); c != "" {
		return c
	}
	if ws.HasRun() {
		return maestroAgentCode
	}
	return ""
}

func (e *Engine) chatLoopPolicy(workspaceID, code string) (loopID, policyID string) {
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return "", ""
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	return strings.TrimSpace(cfg.LoopID), strings.TrimSpace(cfg.PolicyID)
}

func (e *Engine) chatLoopGoal(workspaceID, code string) (goal, display string) {
	if strings.TrimSpace(code) == "" {
		return "", ""
	}
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return "", ""
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	return strings.TrimSpace(cfg.Goal), strings.TrimSpace(cfg.GoalDisplay)
}

func (e *Engine) chatLoopStarted(workspaceID, code string) bool {
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return false
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	return cfg.LoopStarted
}

func (e *Engine) stampLoopOwner(workspaceID, code, loopID, policyID, goal, display string) {
	e.Store.UpdateWorkspace(workspaceID, func(w *domain.Workspace) {
		w.LoopChatCode = code
		if strings.TrimSpace(loopID) != "" {
			w.LoopID = loopID
		}
		if strings.TrimSpace(policyID) != "" {
			w.PolicyID = policyID
		}
	})
	if strings.TrimSpace(goal) == "" {
		if rec, ok := e.Store.Workspace(workspaceID); ok {
			goal, display = rec.Goal, rec.GoalDisplay
		}
	}
	e.stampChatLoop(workspaceID, code, loopID, policyID, goal, display, true)
}

func (e *Engine) stampChatLoop(workspaceID, code, loopID, policyID, goal, display string, started bool) {
	if code != maestroAgentCode && !isPairChatCode(code) {
		return
	}
	if code == maestroAgentCode {
		e.ensureMaestroAgent(workspaceID, "")
	} else {
		e.ensurePairAgent(workspaceID, code, "")
	}
	// Provider from the tab's model (see RenameChatSession) — the Settings coder
	// default would clobber session.Provider on a Claude/Codex tab.
	kind, name, provider := e.chatSessionMeta(workspaceID, code)
	sess, ok := e.ensureAgentSession(workspaceID, kind, code, name, provider)
	if !ok {
		return
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	if strings.TrimSpace(loopID) != "" {
		cfg.LoopID = loopID
	}
	if strings.TrimSpace(policyID) != "" {
		cfg.PolicyID = policyID
	}
	if g := strings.TrimSpace(goal); g != "" {
		cfg.Goal = g
		cfg.GoalDisplay = strings.TrimSpace(display)
	}
	if started {
		cfg.LoopStarted = true
	}
	cfg.Mode = domain.ChatModeLoop
	if cfg.Code == "" {
		cfg.Code = code
	}
	raw, err := json.Marshal(cfg)
	if err != nil {
		return
	}
	_ = e.Store.UpdateSessionConfigJSON(sess.ID, string(raw))
}

func (e *Engine) stampLoopGoalImages(workspaceID, code string, imgs []UserImage) {
	if code != maestroAgentCode && !isPairChatCode(code) {
		return
	}
	ids := make([]string, 0, len(imgs))
	for _, img := range imgs {
		if id := strings.TrimSpace(img.ID); id != "" {
			ids = append(ids, id)
		}
	}
	if code == maestroAgentCode {
		e.ensureMaestroAgentWithStatus(workspaceID, "", "idle")
	} else {
		e.ensurePairAgent(workspaceID, code, "")
	}
	kind, name, provider := e.chatSessionMeta(workspaceID, code)
	sess, ok := e.ensureAgentSession(workspaceID, kind, code, name, provider)
	if !ok {
		return
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	cfg.GoalImages = &ids
	raw, err := json.Marshal(cfg)
	if err != nil {
		return
	}
	_ = e.Store.UpdateSessionConfigJSON(sess.ID, string(raw))
}

func (e *Engine) rememberStandingMode(workspaceID, code, mode string) {
	code = normalizePairChatCode(code)
	mode = domain.NormalizeChatMode(mode)
	if mode == "" || mode == domain.ChatModeLoop {
		return
	}
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	if cfg.StandingMode == mode {
		return
	}
	cfg.StandingMode = mode
	raw, err := json.Marshal(cfg)
	if err != nil {
		return
	}
	_ = e.Store.UpdateSessionConfigJSON(sess.ID, string(raw))
}

func (e *Engine) fillChatTabLoop(tab *domain.WorkspaceChatTab, workspaceID, code string) {
	if tab == nil {
		return
	}
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	tab.LoopID = cfg.LoopID
	tab.PolicyID = cfg.PolicyID
	tab.LoopStarted = cfg.LoopStarted
}

// RememberChatLoopPolicy stores this tab's [[loop]] + [[policy]] before engage.
func (e *Engine) RememberChatLoopPolicy(workspaceID, code, loopID, policyID string) {
	if _, ok := e.Store.Workspace(workspaceID); !ok {
		return
	}
	code = normalizePairChatCode(code)
	if code != maestroAgentCode && !isPairChatCode(code) {
		code = pairAgentCode
	}
	unlock, ok := e.lockChatLife(workspaceID, code)
	defer unlock()
	if !ok {
		return
	}
	if _, ok := e.Store.Agent(workspaceID, code); !ok {
		if code == maestroAgentCode {
			e.ensureMaestroAgent(workspaceID, "")
		} else {
			e.ensurePairAgent(workspaceID, code, "")
		}
	}
	e.stampChatLoop(workspaceID, code, loopID, policyID, "", "", false)
}

func (e *Engine) stampChatPromptGroup(workspaceID, code, group string) {
	group = strings.TrimSpace(group)
	if e == nil || e.Store == nil || group == "" {
		return
	}
	// A newly created chat has an Agent row before any dock Remember, and the
	// frontend fires selection + group as separate unawaited calls. Create the
	// durable session here so a group stamp that wins the race still persists.
	// Missing agent: do not resurrect a tab CloseWorkspaceChat just removed.
	if _, ok := e.Store.Agent(workspaceID, code); !ok {
		return
	}
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		sess, ok = e.ensureAgentSession(workspaceID, string(domain.RoleCoder), code, "Agent", "")
		if !ok {
			return
		}
	}
	raw, err := json.Marshal(map[string]string{"promptGroup": group})
	if err != nil {
		return
	}
	_ = e.Store.PatchSessionConfigMerge(sess.ID, string(raw))
}

// RememberWorkspaceChatSelection persists the dock selection for a chat tab
// without sending a turn (mode/model/flags/simplify should survive reload).
// Validates like the other chat entry points: workspace must exist, code is
// coerced to a pair-chat tab, and a missing agent is not recreated (so a Remember
// racing CloseWorkspaceChat cannot resurrect a just-closed tab).
// simplify nil leaves the chip unset (IfUnset freeze from Settings on first write).
func (e *Engine) RememberWorkspaceChatSelection(workspaceID, code, model string, flags map[string]string, mode string, simplify *bool) {
	if _, ok := e.Store.Workspace(workspaceID); !ok {
		return
	}
	code = normalizePairChatCode(code)
	if !isPairChatCode(code) {
		code = pairAgentCode
	}
	unlock, ok := e.lockChatLife(workspaceID, code)
	defer unlock()
	if !ok {
		return
	}
	if _, ok := e.Store.Agent(workspaceID, code); !ok {
		return
	}
	e.rememberPairChatSelection(workspaceID, code, model, flags, mode, simplify)
}

// RememberWorkspaceChatPromptGroup stamps a [[prompt]] tab_group on this chat
// so later grouped actions reuse it instead of the operator's working tab.
func (e *Engine) RememberWorkspaceChatPromptGroup(workspaceID, code, group string) {
	if _, ok := e.Store.Workspace(workspaceID); !ok {
		return
	}
	code = normalizePairChatCode(code)
	if !isPairChatCode(code) {
		code = pairAgentCode
	}
	unlock, ok := e.lockChatLife(workspaceID, code)
	defer unlock()
	if !ok {
		return
	}
	e.stampChatPromptGroup(workspaceID, code, group)
}

// rememberPairChatSelection persists the dock's model + flags + mode + simplify
// onto the chat's session (and agent.Model) so a reload restores the pickers.
// Creates the durable session if needed so a mode change before the first send
// still survives restart (otherwise the dock would fall back to agent).
func (e *Engine) rememberPairChatSelection(workspaceID, code, model string, flags map[string]string, mode string, simplify *bool) {
	code = normalizePairChatCode(code)
	model = strings.TrimSpace(model)
	mode = strings.TrimSpace(mode)
	if model == "" && mode == "" {
		return
	}
	if code == maestroAgentCode {
		// Loop tab mode is fixed; model comes from Settings.
		return
	}
	if !isPairChatCode(code) {
		// Coding-chat sessions only — never write dock selection onto RV/CO/….
		return
	}
	e.ensurePairAgent(workspaceID, code, model)
	if model != "" {
		e.Store.UpdateAgent(workspaceID, code, func(a *domain.Agent) {
			a.Model = model
		})
	}
	provider := domain.ParseModelRef(model).Provider
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		sess, ok = e.ensureAgentSession(workspaceID, string(domain.RoleCoder), code, "Agent", provider)
		if !ok {
			return
		}
	}
	oldProvider := sess.Provider
	if model != "" && provider != "" && sess.Provider != provider {
		_ = e.Store.UpdateSessionProvider(sess.ID, provider)
		sess.Provider = provider
	}
	// Persist the dock via json_patch so a concurrent simplify json_set is
	// not clobbered. Do not clear Agent.SessionID — AutoResume after restart
	// must --resume a usable id. Mark ForkInject so a later *user* send
	// injects instead of passing a foreign id.
	patch := map[string]any{"code": code, "name": "Agent"}
	var flagsJSON string
	if model != "" {
		if a, ok := e.Store.Agent(workspaceID, code); ok && a.SessionID != "" {
			switched := oldProvider != "" && provider != "" && oldProvider != provider
			if switched || !resumeFitsProvider(a.SessionID, provider) {
				patch["forkInject"] = true
			}
		}
		patch["model"] = model
		rawFlags, err := json.Marshal(flags)
		if err != nil || flags == nil {
			rawFlags = []byte("{}")
		}
		flagsJSON = string(rawFlags)
	}
	if mode != "" {
		patch["mode"] = domain.NormalizeChatMode(mode)
	}
	raw, err := json.Marshal(patch)
	if err != nil {
		return
	}
	_ = e.Store.PatchSessionConfigDock(sess.ID, string(raw), flagsJSON)
	if simplify != nil {
		_ = e.Store.PatchSessionConfigSimplify(sess.ID, *simplify)
		return
	}
	// Freeze Simplify on first dock/send write so a later Settings change does
	// not flip a chat that never touched the chip. No-op if the chip already
	// json_set.
	_ = e.Store.PatchSessionConfigSimplifyIfUnset(sess.ID, e.settings().SimplifyEnabled())
}

// RemapChatsOffDisabledProviders rewrites pair-chat dock picks whose provider
// is now turned off. Claude-on uses the static Claude fallback; Claude-off uses
// the already-remapped Settings.ModelAgent (live catalogue, filled by app).
func (e *Engine) RemapChatsOffDisabledProviders(set domain.Settings) {
	if e == nil || e.Store == nil {
		return
	}
	for _, ws := range e.Store.Workspaces() {
		for _, a := range e.pairChatAgents(ws.ID) {
			if next, _ := remapChatOffDisabled(set, a.Model, nil); next != a.Model {
				e.Store.UpdateAgent(ws.ID, a.Code, func(ag *domain.Agent) {
					ag.Model = next
				})
			}
			sess, ok := e.Store.SessionForLoopAgent(ws.ID, a.Code)
			if !ok {
				continue
			}
			var cfg agentConfig
			_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
			if strings.TrimSpace(cfg.Model) == "" {
				continue
			}
			next, flags := remapChatOffDisabled(set, cfg.Model, cfg.Flags)
			if next == cfg.Model {
				continue
			}
			patch, err := json.Marshal(map[string]any{"model": next})
			if err != nil {
				continue
			}
			rawFlags, err := json.Marshal(flags)
			if err != nil || flags == nil {
				rawFlags = []byte("{}")
			}
			_ = e.Store.PatchSessionConfigDock(sess.ID, string(patch), string(rawFlags))
		}
	}
}

func remapChatOffDisabled(set domain.Settings, ref string, flags map[string]string) (string, map[string]string) {
	if strings.TrimSpace(ref) == "" {
		return ref, flags
	}
	next, fl := set.RemapDisabledModel(ref, flags, false)
	if next != ref {
		return next, fl
	}
	if !set.ModelRefUnavailable(ref, false) {
		return ref, flags
	}
	agent := strings.TrimSpace(set.ModelAgent)
	if agent != "" && !set.ModelRefUnavailable(agent, false) {
		return agent, set.AgentFlags()
	}
	return ref, flags
}
