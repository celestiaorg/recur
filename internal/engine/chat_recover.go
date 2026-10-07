package engine

import (
	"log"
	"os"
	"strings"

	"recur/internal/domain"
)

// interruptedChat is one coding/Maestro tab Recover found live with no goroutine.
type interruptedChat struct {
	workspaceID string
	code        string
}

// isLiveAgentStatus matches the frontend sidebar/composer busy set
// (working / directing / planning).
func isLiveAgentStatus(status string) bool {
	switch status {
	case "working", "directing", "planning":
		return true
	default:
		return false
	}
}

// recoverInterruptedChats idles every agent persisted live with no in-memory
// turn or loop, closes leftover streaming session rows, and queues PA*/MA
// chats for AutoResume. Call from Recover at startup, when nothing is live.
func (e *Engine) recoverInterruptedChats() {
	if e == nil || e.Store == nil {
		return
	}
	e.interruptedChats = nil
	for _, ws := range e.Store.Workspaces() {
		if ws.Archived {
			continue
		}
		for _, a := range e.Store.Agents(ws.ID) {
			if !isLiveAgentStatus(a.Status) {
				continue
			}
			if e.liveChatTurn(ws.ID, a.Code) != nil {
				continue
			}
			if _, running := e.loops.Load(ws.ID); running && !isChatAttentionCode(a.Code) {
				continue
			}
			resume := e.shouldAutoResumeChat(ws, a)
			note := ""
			if isChatAttentionCode(a.Code) {
				note = chatInterruptedReply
			}
			e.healInterruptedChat(ws.ID, a.Code, note)
			if resume {
				e.interruptedChats = append(e.interruptedChats, interruptedChat{
					workspaceID: ws.ID, code: a.Code,
				})
			}
		}
		if ws.ScheduledPromptPending {
			e.Store.UpdateWorkspace(ws.ID, func(w *domain.Workspace) { w.ScheduledPromptPending = false })
		}
	}
	for _, sess := range e.Store.StreamingAppSessions() {
		_ = e.Store.SetSessionStatus(sess.ID, "idle")
	}
}

// shouldAutoResumeChat is true for a human-facing chat tab that was mid-turn
// (PA* working, MA directing) whose worktree directory still exists. Loop-owned
// MA "planning" and specialists are left to loop AutoResume. A missing path
// (seed/demo, or a pruned worktree) must not spawn a provider turn that would
// fail on cwd and write "Agent error" into the chat.
func (e *Engine) shouldAutoResumeChat(ws domain.Workspace, a domain.Agent) bool {
	// A scheduled run's interrupted first turn is not replayed; a subagent's
	// is, or its parent waits forever for a report.
	if ws.ScheduledPromptPending && !isChatChild(ws) {
		return false
	}
	path := strings.TrimSpace(ws.WorktreePath)
	if path == "" {
		return false
	}
	st, err := os.Stat(path)
	if err != nil || !st.IsDir() {
		return false
	}
	if !isChatAttentionCode(a.Code) {
		return false
	}
	switch a.Code {
	case maestroAgentCode:
		return a.Status == "directing"
	default:
		return a.Status == "working"
	}
}

// healInterruptedChat returns a zombie live agent to idle, closes streaming
// session messages, attaches error results to unfinished tool calls, and
// records note in the transcript. Safe when the agent or session is missing.
func (e *Engine) healInterruptedChat(workspaceID, code, note string) {
	if e == nil || e.Store == nil || workspaceID == "" || code == "" {
		return
	}
	e.closeInterruptedChatSession(workspaceID, code, note)
	e.idleAgentNoUnseen(workspaceID, code)
}

// idleAgentNoUnseen writes status=idle and publishes agent.updated without
// the unseen-attention badge (Recover/zombie Stop are not "a reply landed
// while you were looking away").
func (e *Engine) idleAgentNoUnseen(workspaceID, code string) {
	e.Store.UpdateAgent(workspaceID, code, func(a *domain.Agent) {
		a.Status = "idle"
	})
	e.Bus.Publish(Event{Name: "agent.updated", Data: map[string]any{
		"workspaceId": workspaceID, "agentId": workspaceID + "-" + code, "status": "idle",
	}})
}

// closeInterruptedChatSession finalizes leftover streaming rows and pending
// tool_use blocks so reload does not paint the turn as still in flight.
func (e *Engine) closeInterruptedChatSession(workspaceID, code, note string) {
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		if strings.TrimSpace(note) != "" {
			e.persistChatFailure(workspaceID, code, "", "", note, 0, Result{}, false, "")
		}
		return
	}
	msgs, err := e.Store.SessionTranscript(sess.ID)
	if err != nil {
		_ = e.Store.SetSessionStatus(sess.ID, "idle")
		return
	}
	pending := map[string]struct{}{}
	var order []string
	for _, m := range msgs {
		if m.StreamStatus == domain.StreamStreaming || m.StreamStatus == domain.StreamPending {
			_ = e.Store.CompleteMessage(m.ID, domain.StreamError, domain.Message{})
		}
		for _, b := range m.Content {
			if tu, ok := b.AsToolUse(); ok && tu.ID != "" {
				if _, seen := pending[tu.ID]; !seen {
					order = append(order, tu.ID)
				}
				pending[tu.ID] = struct{}{}
			}
			if tr, ok := b.AsToolResult(); ok {
				delete(pending, tr.ToolUseID)
			}
		}
	}
	var blocks []domain.ContentBlock
	resultNote := strings.TrimSpace(note)
	if resultNote == "" {
		resultNote = chatInterruptedReply
	}
	for _, id := range order {
		if _, ok := pending[id]; !ok {
			continue
		}
		blocks = append(blocks, domain.ToolResultBlock(id, resultNote, true))
	}
	if len(blocks) > 0 {
		_, _ = e.Store.AppendMessage(sess.ID, domain.Message{
			Role:         "user",
			StreamStatus: domain.StreamComplete,
			Content:      blocks,
		})
	}
	_ = e.Store.SetSessionStatus(sess.ID, "idle")
	if strings.TrimSpace(note) != "" {
		e.persistChatFailure(workspaceID, code, "", "", note, 0, Result{}, false, "")
	}
}

// healZombieChat idles a tab the UI thinks is live but that has no registered
// turn — Recover missed it, or the UI seeded busy from a stale agent row.
// Returns true when it changed something (so CancelChat can report success).
func (e *Engine) healZombieChat(workspaceID, code string) bool {
	if e == nil || e.Store == nil {
		return false
	}
	code = strings.TrimSpace(code)
	if isPairChatCode(code) {
		code = normalizePairChatCode(code)
	}
	if !isChatAttentionCode(code) {
		return false
	}
	a, ok := e.Store.Agent(workspaceID, code)
	if !ok || !isLiveAgentStatus(a.Status) {
		return false
	}
	if e.liveChatTurn(workspaceID, code) != nil {
		return false
	}
	if code == maestroAgentCode {
		if _, running := e.loops.Load(workspaceID); running {
			return false
		}
	}
	e.healInterruptedChat(workspaceID, code, chatStoppedReply)
	return true
}

// launchInterruptedChats continues Recover-queued chat tabs. Each send runs on
// its own goroutine so a slow provider turn cannot block loop AutoResume or
// the window appearing.
func (e *Engine) launchInterruptedChats(chats []interruptedChat) {
	if e == nil {
		return
	}
	for _, c := range chats {
		wsID, code := c.workspaceID, c.code
		log.Printf("recur: auto-resuming chat %s %s after restart", wsID, code)
		e.resumingChats.Store(chatTurnKey(wsID, code), struct{}{})
		go e.resumeInterruptedChat(wsID, code)
	}
}

// chatResuming reports a tab of the workspace that AutoResume launched and
// whose resumed turn has not finished yet.
func (e *Engine) chatResuming(workspaceID string) bool {
	found := false
	e.resumingChats.Range(func(k, _ any) bool {
		if strings.HasPrefix(k.(string), workspaceID+"\x00") {
			found = true
			return false
		}
		return true
	})
	return found
}

func (e *Engine) launchInterruptedChatsExcept(hold map[string]struct{}) {
	if e == nil {
		return
	}
	if len(hold) == 0 {
		e.launchInterruptedChats(e.interruptedChats)
		e.interruptedChats = nil
		return
	}
	var now, rest []interruptedChat
	for _, c := range e.interruptedChats {
		if _, skip := hold[c.workspaceID]; skip {
			rest = append(rest, c)
			continue
		}
		now = append(now, c)
	}
	e.interruptedChats = rest
	e.launchInterruptedChats(now)
}

func (e *Engine) launchInterruptedChatsFor(workspaceID string) {
	if e == nil || workspaceID == "" {
		return
	}
	var now, rest []interruptedChat
	for _, c := range e.interruptedChats {
		if c.workspaceID == workspaceID {
			now = append(now, c)
			continue
		}
		rest = append(rest, c)
	}
	e.interruptedChats = rest
	e.launchInterruptedChats(now)
}

func (e *Engine) resumeInterruptedChat(workspaceID, code string) {
	if e == nil || e.Store == nil {
		return
	}
	defer e.resumingChats.Delete(chatTurnKey(workspaceID, code))
	if !e.chatResumeStillIdle(workspaceID, code) {
		return
	}
	if _, ok := e.Store.Workspace(workspaceID); !ok {
		return
	}
	model, flags, mode := e.WorkspaceChatSelection(workspaceID, code)
	if code == maestroAgentCode || domain.NormalizeChatMode(mode) == domain.ChatModeLoop {
		if _, running := e.loops.Load(workspaceID); !running {
			ws, ok := e.Store.Workspace(workspaceID)
			if !ok || maestroResumeNeedsLoop(ws) {
				return
			}
		}
		e.sendMaestroMessage(workspaceID, code, chatResumePrompt, nil, chatResumeDisplay)
		return
	}
	// Reuse the tab's dock selection. Empty model/mode falls back to Settings'
	// role default (often Claude) while Agent.SessionID may still be a Cursor
	// agent-… id — Claude then rejects `claude -p --resume agent-…`.
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return
	}
	if e.chatResumeBlockedByRunLock(ws, mode) {
		return
	}
	if !e.chatResumeStillIdle(workspaceID, code) {
		return
	}
	e.SendWorkspaceMessage(workspaceID, code, chatResumePrompt, model, flags, nil, mode, chatResumeDisplay)
}

// maestroResumeNeedsLoop is true when a Maestro continuation could steer a
// run that has no goroutine to receive it. That includes in-flight and
// queued (RunActive) plus parked-but-resumable (paused, escalated,
// retryable-failed). A steer would emit "Re-planned" on the feed and never
// land. Settled loops (pr_open, merged, stopped, non-retryable failed) are
// conversational only.
func maestroResumeNeedsLoop(ws domain.Workspace) bool {
	return domain.RunActive(ws.State) || resumable(ws)
}

// chatResumeBlockedByRunLock is true when an agent-mode continuation would
// edit a worktree the autopilot owns or is about to own (paused/interrupted
// is not RunActive yet, but AutoResume is about to dispatch the Coder).
func (e *Engine) chatResumeBlockedByRunLock(ws domain.Workspace, mode string) bool {
	if domain.NormalizeChatMode(mode) != domain.ChatModeAgent || !ws.HasRun() {
		return false
	}
	if domain.RunActive(ws.State) {
		return true
	}
	return ws.State == domain.StatePaused && ws.Phase == phaseInterrupted
}

// chatResumeStillIdle is true when Recover's queued tab is still waiting: the
// agent exists, is idle, and no in-flight turn has been registered. A user send
// or CloseWorkspaceChat in the window after Recover must not be overwritten.
func (e *Engine) chatResumeStillIdle(workspaceID, code string) bool {
	if e.liveChatTurn(workspaceID, code) != nil {
		return false
	}
	a, ok := e.Store.Agent(workspaceID, code)
	if !ok || a.Status != "idle" {
		return false
	}
	return true
}
