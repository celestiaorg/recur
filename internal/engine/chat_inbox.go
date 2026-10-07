package engine

import (
	"fmt"
	"html"
	"strings"

	"recur/internal/domain"
)

const chatInboxIDPrefix = "ib-chat-"

func chatInboxID(workspaceID, code string) string {
	return chatInboxIDPrefix + workspaceID + "-" + code
}

func chatInboxTarget(workspaceID, code string) string {
	return "chat:" + workspaceID + ":" + code
}

// ParseChatInboxID extracts workspace + tab code from an ib-chat-* inbox id.
func ParseChatInboxID(id string) (workspaceID, code string, ok bool) {
	rest, found := strings.CutPrefix(id, chatInboxIDPrefix)
	if !found {
		return "", "", false
	}
	i := strings.LastIndex(rest, "-")
	if i <= 0 || i == len(rest)-1 {
		return "", "", false
	}
	ws, code := rest[:i], rest[i+1:]
	if !isChatAttentionCode(code) {
		return "", "", false
	}
	return ws, code, true
}

func isChatInboxItem(it domain.InboxItem) bool {
	return it.Kind == "chat" || strings.HasPrefix(it.ID, chatInboxIDPrefix)
}

func (e *Engine) publishInboxUpdated() {
	e.Bus.Publish(Event{Name: "inbox.updated", Data: map[string]any{"unread": e.Store.UnreadCount()}})
}

// emitChatInbox upserts the inbox row for an unseen chat tab.
// loopFinished is the event that made the tab unseen — do not infer it from
// the workspace's current settled state (a later ordinary reply on a historically
// finished loop is still "finished a reply"). Returns true only when a row was
// actually kept. Does not publish — callers batch inbox.updated. A concurrent
// mark-seen that clears Unseen (or deletes the agent) after the unseen write is
// re-checked so we do not leave an orphan.
//
// simplify: SyncChatInbox heals missing rows as a reply; persist a completion
// mark on Agent if boot must restore the "Loop finished" phrase.
func (e *Engine) emitChatInbox(workspaceID, code string, loopFinished bool) bool {
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok || ws.Archived || !isChatAttentionCode(code) {
		return false
	}
	// A parked ask_user already has a Needs-you row for this tab — don't stack
	// a second "finished a reply" item on the same question.
	if e.askUsers != nil && e.askUsers.pendingFor(workspaceID, code) {
		return false
	}
	if e.pendingModeContinueFor(workspaceID, code) {
		return false
	}
	a, ok := e.Store.Agent(workspaceID, code)
	if !ok || !a.Unseen {
		return false
	}
	title := e.chatTabTitle(workspaceID, code)
	if code == maestroAgentCode {
		title = "Loop"
	}
	done := "finished a reply"
	if loopFinished {
		title = "Loop"
		done = "finished"
	}
	wsLabel := workspaceID
	if s := strings.TrimSpace(ws.Short); s != "" {
		wsLabel = s
	}
	if ws.IsPRChatHost() {
		if pr, ok := e.Store.PR(ws.HostPRID); ok {
			titleBit := strings.TrimSpace(pr.Title)
			if titleBit == "" {
				titleBit = wsLabel
			}
			wsLabel = fmt.Sprintf("#%d · %s", pr.Number, titleBit)
		}
	}
	color := "var(--cyan)"
	if code == maestroAgentCode {
		color = "var(--amber)"
	}
	if strings.TrimSpace(a.Color) != "" {
		color = a.Color
	}
	e.Store.AddInboxItem(domain.InboxItem{
		ID:          chatInboxID(workspaceID, code),
		Group:       "ready",
		Unread:      true,
		Kind:        "chat",
		Color:       color,
		Title:       fmt.Sprintf("<b>%s</b> %s", html.EscapeString(title), done),
		Sub:         html.EscapeString(truncate(wsLabel, 64)),
		Time:        "now",
		Action:      "View",
		ActionClass: "go",
		Target:      chatInboxTarget(workspaceID, code),
		WorkspaceID: workspaceID,
	})
	if a2, ok := e.Store.Agent(workspaceID, code); !ok || !a2.Unseen {
		e.Store.RemoveInboxItem(chatInboxID(workspaceID, code))
		return false
	}
	return true
}

// clearChatInbox drops the unseen-chat inbox row. notify publishes inbox.updated
// when a row was actually removed.
func (e *Engine) clearChatInbox(workspaceID, code string, notify bool) bool {
	if !e.Store.RemoveInboxItem(chatInboxID(workspaceID, code)) {
		return false
	}
	if notify {
		e.publishInboxUpdated()
	}
	return true
}

// MarkAllChatsSeen clears every unseen chat tab and drops all chat inbox rows
// (including orphans whose agent is gone). One inbox.updated + one
// attention.updated for the whole pass.
func (e *Engine) MarkAllChatsSeen() {
	e.chatUnseenMu.Lock()
	defer e.chatUnseenMu.Unlock()
	changed := false
	for _, w := range e.Store.Workspaces() {
		if w.Archived {
			continue
		}
		for _, a := range e.Store.Agents(w.ID) {
			if !a.Unseen || !isChatAttentionCode(a.Code) {
				continue
			}
			e.Store.UpdateAgent(w.ID, a.Code, func(ag *domain.Agent) { ag.Unseen = false })
			changed = true
		}
	}
	if e.Store.RemoveChatInboxItems() > 0 {
		changed = true
	}
	if !changed {
		return
	}
	e.publishInboxUpdated()
	e.publishAttentionEvt("", "", false, true)
}

// SyncChatInbox heals inbox rows from Agent.Unseen (D42 derived-from-real-state):
// every live unseen chat gets a row; stale chat rows are dropped. Call on boot
// so chats that finished before this build still appear in Inbox.
func (e *Engine) SyncChatInbox() {
	want := map[string][2]string{}
	for _, w := range e.Store.Workspaces() {
		if w.Archived {
			continue
		}
		for _, a := range e.Store.Agents(w.ID) {
			if a.Unseen && isChatAttentionCode(a.Code) {
				want[chatInboxID(w.ID, a.Code)] = [2]string{w.ID, a.Code}
			}
		}
	}
	changed := false
	for _, it := range e.Store.Inbox() {
		if !isChatInboxItem(it) {
			continue
		}
		if _, ok := want[it.ID]; ok {
			delete(want, it.ID)
			continue
		}
		if e.Store.RemoveInboxItem(it.ID) {
			changed = true
		}
	}
	for _, pair := range want {
		if e.emitChatInbox(pair[0], pair[1], false) {
			changed = true
		}
	}
	if changed {
		e.publishInboxUpdated()
	}
}

// loopSettled reports a walk that finished (PR open, idle, or artifact) rather
// than stopped/failed/blocked. Composer Stop parks idle via wantsIdle and is
// filtered before this is consulted. state/phase are this walk's last setState,
// not the workspace row (a sibling chat walk overwrites that).
func loopSettled(state domain.WorkspaceState, phase string) bool {
	switch state {
	case domain.StatePROpen:
		return true
	case domain.StatePaused:
		return phase == "idle" || phase == "artifact"
	}
	return false
}

// maybeFlagLoopFinished raises unseen + a "Loop finished" inbox row on the
// conductor tab when a walk completes off-screen. Stop, fail, and composer
// Stop do not — those already have their own signals (or the human just did it).
func (e *Engine) maybeFlagLoopFinished(l *Loop) {
	if l == nil || e == nil || l.wantsIdle() || l.isStopped() {
		return
	}
	ws, ok := e.Store.Workspace(l.ID)
	if !ok || ws.Archived || !loopSettled(l.exitState, l.exitPhase) {
		return
	}
	code := strings.TrimSpace(l.ChatCode)
	if code == "" {
		// Empty ChatCode is a legacy MA-owned walk, not the current workspace owner.
		code = maestroAgentCode
	}
	e.flagLoopFinished(l.ID, code)
}

// flagLoopFinished marks the conductor tab unseen when it is still open.
// Holds lockChatLife so CloseWorkspaceChat cannot sample Unseen, drop the
// inbox row, then lose a completion that lands before DeleteAgent.
// A missing agent means CloseWorkspaceChat already deleted the tab — do not
// resurrect it just to hang a badge.
func (e *Engine) flagLoopFinished(workspaceID, code string) {
	code = strings.TrimSpace(code)
	if !isChatAttentionCode(code) {
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
	e.flagChatUnseen(workspaceID, code, true)
}
