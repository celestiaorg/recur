package engine

import (
	"encoding/json"
	"strings"
	"time"

	"recur/internal/domain"
)

// switchModeAllowed reports whether an interactive chat may move from→to.
// agent may go anywhere; ask/plan/review may move among themselves; never up to agent.
func switchModeAllowed(from, to string) bool {
	from = domain.NormalizeChatMode(from)
	to = domain.NormalizeChatMode(to)
	if to == domain.ChatModeLoop || to == "" {
		return false
	}
	if from == to {
		return true
	}
	if from == domain.ChatModeAgent {
		return to == domain.ChatModeAsk || to == domain.ChatModePlan || to == domain.ChatModeReview
	}
	readonly := map[string]bool{
		domain.ChatModeAsk: true, domain.ChatModePlan: true, domain.ChatModeReview: true,
	}
	return readonly[from] && readonly[to]
}

func (r engineReporter) SwitchMode(workspaceID, agentCode, mode string) string {
	return r.eng.SwitchChatMode(workspaceID, agentCode, mode)
}

// switchModeParkResult is the tool result on a mid-turn remount. The model must
// stop; the engine parks this turn (ask_user pattern) and starts a continuation
// with the destination mode's tools (Claude --allowedTools / Cursor plan vs agent).
const switchModeParkResultSuffix = " Stop now. This turn is remounting with the new tool policy. " +
	"Do not keep working with the old tools; a continuation turn is starting."

// maxModeRemounts caps consecutive switch_mode remounts on one tab so a
// plan/ask flip-flop cannot spawn provider turns forever. A user send resets it.
const maxModeRemounts = 3

// pendingModeContinue is one in-flight switch_mode remount, keyed like chats.
type pendingModeContinue struct {
	from, to string
	turn     *chatTurn
	epoch    uint64
}

func modeContinueKey(workspaceID, code string) string {
	if isPairChatCode(code) {
		code = normalizePairChatCode(code)
	}
	return chatTurnKey(workspaceID, code)
}

func (e *Engine) noteModeContinue(workspaceID, code, from, to string, turn *chatTurn) {
	if e == nil || turn == nil {
		return
	}
	gen := e.allocContinueGen(workspaceID, code)
	e.modeContinueMu.Lock()
	if e.modeContinues == nil {
		e.modeContinues = map[string]pendingModeContinue{}
	}
	key := modeContinueKey(workspaceID, code)
	old, replaced := e.modeContinues[key]
	e.modeContinues[key] = pendingModeContinue{
		from: from, to: to, turn: turn, epoch: gen,
	}
	e.modeContinueMu.Unlock()
	if replaced {
		e.dropContinueGen(old.epoch)
	}
}

func (e *Engine) takeModeContinue(workspaceID, code string) (pendingModeContinue, bool) {
	return e.takeModeContinueFor(workspaceID, code, nil)
}

// takeModeContinueFor removes the pending remount. When expect is non-nil, the
// entry is taken only if it still belongs to that turn, so an older defer cannot
// steal a newer remount or leak its live generation.
func (e *Engine) takeModeContinueFor(workspaceID, code string, expect *chatTurn) (pendingModeContinue, bool) {
	if e == nil {
		return pendingModeContinue{}, false
	}
	e.modeContinueMu.Lock()
	defer e.modeContinueMu.Unlock()
	if e.modeContinues == nil {
		return pendingModeContinue{}, false
	}
	key := modeContinueKey(workspaceID, code)
	p, ok := e.modeContinues[key]
	if !ok {
		return pendingModeContinue{}, false
	}
	if expect != nil && p.turn != expect {
		return pendingModeContinue{}, false
	}
	delete(e.modeContinues, key)
	return p, true
}

func (e *Engine) pendingModeContinueFor(workspaceID, code string) bool {
	if e == nil {
		return false
	}
	e.modeContinueMu.Lock()
	defer e.modeContinueMu.Unlock()
	if e.modeContinues == nil {
		return false
	}
	_, ok := e.modeContinues[modeContinueKey(workspaceID, code)]
	return ok
}

func (e *Engine) bumpModeRemounts(workspaceID, code string) int {
	if e == nil {
		return 0
	}
	e.modeContinueMu.Lock()
	defer e.modeContinueMu.Unlock()
	if e.modeRemountDepth == nil {
		e.modeRemountDepth = map[string]int{}
	}
	key := modeContinueKey(workspaceID, code)
	e.modeRemountDepth[key]++
	return e.modeRemountDepth[key]
}

func (e *Engine) resetModeRemounts(workspaceID, code string) {
	if e == nil {
		return
	}
	e.modeContinueMu.Lock()
	defer e.modeContinueMu.Unlock()
	if e.modeRemountDepth != nil {
		delete(e.modeRemountDepth, modeContinueKey(workspaceID, code))
	}
}

func (e *Engine) uncountModeRemount(workspaceID, code string) {
	if e == nil {
		return
	}
	e.modeContinueMu.Lock()
	defer e.modeContinueMu.Unlock()
	if e.modeRemountDepth == nil {
		return
	}
	key := modeContinueKey(workspaceID, code)
	n := e.modeRemountDepth[key]
	if n <= 1 {
		delete(e.modeRemountDepth, key)
		return
	}
	e.modeRemountDepth[key] = n - 1
}

func (e *Engine) clearModeContinuesForWorkspace(workspaceID string) {
	workspaceID = strings.TrimSpace(workspaceID)
	if e == nil || workspaceID == "" {
		return
	}
	prefix := workspaceID + "\x00"
	var gens []uint64
	e.modeContinueMu.Lock()
	for k, p := range e.modeContinues {
		if strings.HasPrefix(k, prefix) {
			gens = append(gens, p.epoch)
			delete(e.modeContinues, k)
		}
	}
	for k := range e.modeRemountDepth {
		if strings.HasPrefix(k, prefix) {
			delete(e.modeRemountDepth, k)
		}
	}
	e.modeContinueMu.Unlock()
	for _, gen := range gens {
		e.dropContinueGen(gen)
	}
}

// SwitchChatMode applies a privilege-safe mode change for a PA* chat session.
// When a turn is in flight, the current process is parked and a continuation
// turn starts with the destination chatModeShape so the new tools are live
// immediately (Cursor plan vs agent is bound at spawn; Claude's allowlist is too).
func (e *Engine) SwitchChatMode(workspaceID, agentCode, mode string) string {
	code := strings.TrimSpace(agentCode)
	if code == "" || code == maestroAgentCode || !strings.HasPrefix(code, "PA") {
		return "switch_mode is only available on interactive pair chats (not Maestro or loop specialists)."
	}
	to := domain.NormalizeChatMode(mode)
	if to != domain.ChatModeAsk && to != domain.ChatModePlan && to != domain.ChatModeAgent && to != domain.ChatModeReview {
		return "Unknown mode " + mode + ". Use ask, plan, agent, or review."
	}

	from := domain.ChatModeAgent
	var flags map[string]string
	model := ""
	if sess, ok := e.Store.SessionForLoopAgent(workspaceID, code); ok {
		var cfg agentConfig
		_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
		if m := domain.NormalizeChatMode(cfg.Mode); m != "" && m != domain.ChatModeLoop {
			from = m
		}
		model = cfg.Model
		flags = cfg.Flags
	}

	if from == to {
		return "Already in " + to + " mode."
	}

	if !switchModeAllowed(from, to) {
		if to == domain.ChatModeAgent {
			return "Cannot switch from " + from + " to agent (would gain write access). " +
				"Ask the user to switch this chat to Agent mode from the dock."
		}
		return "Cannot switch from " + from + " to " + to + "."
	}

	e.RememberWorkspaceChatSelection(workspaceID, code, model, flags, to, nil)
	e.Bus.Publish(Event{Name: "chat.mode_changed", Data: map[string]any{
		"workspaceId": workspaceID,
		"agentCode":   code,
		"mode":        to,
		"from":        from,
	}})

	msg := "Switched chat mode from " + from + " to " + to + "."
	turn := e.liveChatTurn(workspaceID, code)
	if turn == nil {
		return msg + " Subsequent turns use the new tool policy."
	}
	if e.bumpModeRemounts(workspaceID, code) > maxModeRemounts {
		return msg + " Subsequent turns use the new tool policy."
	}
	e.noteModeContinue(workspaceID, code, from, to, turn)
	go func() {
		time.Sleep(askUserParkDelay)
		e.parkChatTurnIf(workspaceID, code, turn)
	}()
	return msg + switchModeParkResultSuffix
}

// maybeContinueAfterModeSwitch starts the remounted turn after switch_mode.
// Registered as a defer on SendWorkspaceMessage so the agent slot is released
// first. unregisterMCP runs after this hook (it is deferred earlier); the hook
// only schedules a goroutine, so the stale token is gone before the
// continuation starts. User Stop, shutdown, or a parked ask_user drop the remount.
// The model finishing before the park lands still remounts (the pending entry
// is the intent; isParked is not required).
func (e *Engine) maybeContinueAfterModeSwitch(workspaceID, code string, turn *chatTurn) {
	if e == nil || turn == nil {
		return
	}
	p, ok := e.takeModeContinueFor(workspaceID, code, turn)
	if !ok {
		return
	}
	if turn.isStopped() {
		e.uncountModeRemount(workspaceID, code)
		e.dropContinueGen(p.epoch)
		return
	}
	if e.askUsers != nil && e.askUsers.pendingFor(workspaceID, code) {
		e.uncountModeRemount(workspaceID, code)
		e.dropContinueGen(p.epoch)
		return
	}
	from, to, epoch := p.from, p.to, p.epoch
	e.teardownMu.Lock()
	if e.shuttingDown {
		e.teardownMu.Unlock()
		e.uncountModeRemount(workspaceID, code)
		e.dropContinueGen(epoch)
		return
	}
	e.teardownWG.Add(1)
	e.teardownMu.Unlock()
	go func() {
		defer e.teardownWG.Done()
		time.Sleep(50 * time.Millisecond)
		e.resumeAfterModeSwitch(workspaceID, code, from, to, epoch)
	}()
}

func (e *Engine) resumeAfterModeSwitch(workspaceID, code, from, to string, epoch uint64) {
	defer e.dropContinueGen(epoch)
	if e == nil || e.isShuttingDown() {
		return
	}
	if e.liveChatTurn(workspaceID, code) != nil {
		e.uncountModeRemount(workspaceID, code)
		return
	}
	if !e.continueGenLive(epoch) {
		e.uncountModeRemount(workspaceID, code)
		return
	}
	model, flags, mode := e.WorkspaceChatSelection(workspaceID, code)
	if domain.NormalizeChatMode(mode) == "" {
		mode = to
	}
	prompt := formatModeSwitchResumePrompt(from, mode)
	display := formatModeSwitchDisplay(mode)
	reply := e.sendWorkspaceTurn(workspaceID, code, prompt, model, flags, nil, mode, display, false, epoch)
	if isModeSwitchResumeDeadEnd(reply) {
		e.persistChatFailure(workspaceID, code, prompt, display, reply, 0, Result{}, false, "")
		e.flagChatUnseen(workspaceID, code, false)
	}
}

// flagChatUnseen marks a tab unseen and raises inbox/attention when the human
// is not looking, so a remount that died before working→idle is still noticed.
// After writing Unseen, recheck focus: MarkWorkspaceChatSeen can win the race
// between the first focus check and the write, and would otherwise leave a
// sticky badge on a tab the human is already viewing.
func (e *Engine) flagChatUnseen(workspaceID, code string, loopFinished bool) {
	if e == nil || !isChatAttentionCode(code) || e.isChatFocused(workspaceID, code) {
		return
	}
	e.chatUnseenMu.Lock()
	defer e.chatUnseenMu.Unlock()
	e.Store.UpdateAgent(workspaceID, code, func(a *domain.Agent) {
		a.Unseen = true
	})
	kept := e.emitChatInbox(workspaceID, code, loopFinished)
	if e.isChatFocused(workspaceID, code) {
		hadUnseen := false
		if a, ok := e.Store.Agent(workspaceID, code); ok && a.Unseen {
			hadUnseen = true
		}
		e.Store.UpdateAgent(workspaceID, code, func(a *domain.Agent) { a.Unseen = false })
		cleared := e.clearChatInbox(workspaceID, code, false)
		if cleared {
			e.publishInboxUpdated()
		}
		if cleared || hadUnseen {
			e.publishAttention(workspaceID, code, false)
		}
		return
	}
	a, ok := e.Store.Agent(workspaceID, code)
	if !ok || !a.Unseen {
		e.clearChatInbox(workspaceID, code, false)
		return
	}
	if kept {
		e.publishInboxUpdated()
	}
	e.publishAttention(workspaceID, code, true)
}

func isModeSwitchResumeDeadEnd(reply string) bool {
	reply = strings.TrimSpace(reply)
	if reply == "" {
		return false
	}
	return strings.HasPrefix(reply, "I can't find that workspace") ||
		strings.HasPrefix(reply, "I can't run right now") ||
		strings.HasPrefix(reply, "I'm at capacity")
}

func formatModeSwitchResumePrompt(from, to string) string {
	from = domain.NormalizeChatMode(from)
	to = domain.NormalizeChatMode(to)
	var b strings.Builder
	b.WriteString("Your switch_mode call remounted this chat from ")
	b.WriteString(from)
	b.WriteString(" to ")
	b.WriteString(to)
	b.WriteString(". The new tool policy is live on THIS turn. Continue the user's request. ")
	b.WriteString("Do not ask them to switch modes, and do not use Cursor's built-in SwitchMode.\n\n")
	switch to {
	case domain.ChatModePlan:
		b.WriteString("You are in PLAN mode (read-only on the git worktree). Investigate if needed, then file the FULL detailed plan with mcp__recur__submit_plan (body is what later agents execute from, not chat). You may create artifacts with mcp__recur__create_artifact in addition to, never instead of, submit_plan, and pass their ids in attachments.")
	case domain.ChatModeAsk:
		b.WriteString("You are in ASK mode (read-only on the git worktree). Answer questions; do not edit repository files. You may create artifacts with mcp__recur__create_artifact.")
	case domain.ChatModeReview:
		b.WriteString("You are in REVIEW mode (read-only on the git worktree). File findings with mcp__recur__report_finding. On a re-review, mark prior findings resolved or invalid with mcp__recur__update_finding. You may create artifacts with mcp__recur__create_artifact in addition to, never instead of, report_finding.")
	default:
		b.WriteString("You are in AGENT mode (read+edit). Proceed with the task; do not ask the user to switch to Agent.")
	}
	return b.String()
}

func formatModeSwitchDisplay(to string) string {
	to = domain.NormalizeChatMode(to)
	if to == "" {
		to = domain.ChatModeAgent
	}
	return "Switched to " + strings.ToUpper(to[:1]) + to[1:] + ": continuing"
}

func isModeSwitchContinueDisplay(s string) bool {
	return strings.HasPrefix(s, "Switched to ") && strings.HasSuffix(s, ": continuing")
}

func userMessageIsModeSwitchContinue(m domain.Message) bool {
	for _, b := range m.Content {
		if p, ok := b.AsText(); ok {
			if isModeSwitchContinueDisplay(p.DisplayText) || isModeSwitchContinueDisplay(p.Text) {
				return true
			}
		}
	}
	return false
}
