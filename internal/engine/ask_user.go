package engine

import (
	"fmt"
	"html"
	"sort"
	"strings"
	"sync"
	"time"

	"recur/internal/domain"
	"recur/internal/mcp"
)

// askUserParkResult is the tool result returned immediately when a question is
// parked. The model must stop; the human's answer arrives later as a new user
// message on a resumed turn (Cursor/Claude MCP clients time out blocked POSTs).
const askUserParkResult = "Question delivered to the user. Stop now and wait — " +
	"their answer will arrive as a later message. Do not invent an answer or continue working."

// askUserParkDelay lets the MCP HTTP response flush to the agent before we
// cancel the turn so the chat can go fully idle.
const askUserParkDelay = 150 * time.Millisecond

type pendingAskUser struct {
	req  domain.UserQuestion
	turn *chatTurn // asking turn; park/answer only cancel if this is still live
}

// askUserBroker parks interactive ask_user prompts for the human. Unlike the
// old block-on-MCP design (which Cursor's ~60s/5m MCP client timeouts killed),
// park returns immediately, the chat turn is stopped quietly, and answering
// starts a new resumed turn with the answer as user input.
type askUserBroker struct {
	mu       sync.Mutex
	pending  map[string]*pendingAskUser
	resuming map[string]domain.UserQuestion // answered; resume turn not registered yet
	seq      int
	now      func() time.Time

	notify   func(domain.UserQuestion)
	resolved func(id string, ans domain.UserQuestionAnswer)
}

func newAskUserBroker() *askUserBroker {
	return &askUserBroker{
		pending:  map[string]*pendingAskUser{},
		resuming: map[string]domain.UserQuestion{},
		now:      time.Now,
	}
}

// park stores a pending question and notifies the UI. Returns the parked
// question (with ID). Does not block. turn is the asking chatTurn (may be nil).
func (b *askUserBroker) park(req domain.UserQuestion, turn *chatTurn) domain.UserQuestion {
	b.mu.Lock()
	b.seq++
	req.ID = fmt.Sprintf("ask-%d-%d", b.now().UnixNano(), b.seq)
	if req.AskedAgo == "" {
		req.AskedAgo = "now"
	}
	b.pending[req.ID] = &pendingAskUser{req: req, turn: turn}
	notify := b.notify
	b.mu.Unlock()

	if notify != nil {
		notify(req)
	}
	return req
}

func (b *askUserBroker) Answer(id string, ans domain.UserQuestionAnswer) (domain.UserQuestion, *chatTurn, bool) {
	b.mu.Lock()
	p, ok := b.pending[id]
	if !ok {
		b.mu.Unlock()
		return domain.UserQuestion{}, nil, false
	}
	delete(b.pending, id)
	if b.resuming == nil {
		b.resuming = map[string]domain.UserQuestion{}
	}
	b.resuming[p.req.ID] = p.req
	b.mu.Unlock()
	b.publishResolved(id, ans)
	return p.req, p.turn, true
}

func (b *askUserBroker) Pending() []domain.UserQuestion {
	b.mu.Lock()
	defer b.mu.Unlock()
	out := make([]domain.UserQuestion, 0, len(b.pending))
	for _, p := range b.pending {
		out = append(out, p.req)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out
}

// dropWorkspace removes all pending questions for workspaceID and returns their ids
// (so callers can clear matching inbox rows).
func (b *askUserBroker) dropWorkspace(workspaceID string) []string {
	workspaceID = strings.TrimSpace(workspaceID)
	if workspaceID == "" {
		return nil
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	var ids []string
	for id, p := range b.pending {
		if p.req.WorkspaceID == workspaceID {
			ids = append(ids, id)
			delete(b.pending, id)
		}
	}
	for id, q := range b.resuming {
		if q.WorkspaceID == workspaceID {
			delete(b.resuming, id)
		}
	}
	sort.Strings(ids)
	return ids
}

func (b *askUserBroker) dropResume(id string) {
	if b == nil {
		return
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	delete(b.resuming, id)
}

// workspaceBusy is true while a parked ask_user or an answer-resume gap
// still belongs to this workspace. Hoist must wait — Answer removes the
// pending row before the resumed turn is registered.
func (b *askUserBroker) workspaceBusy(workspaceID string) bool {
	if b == nil || workspaceID == "" {
		return false
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	for _, p := range b.pending {
		if p.req.WorkspaceID == workspaceID {
			return true
		}
	}
	for _, q := range b.resuming {
		if q.WorkspaceID == workspaceID {
			return true
		}
	}
	return false
}

func (b *askUserBroker) publishResolved(id string, ans domain.UserQuestionAnswer) {
	b.mu.Lock()
	fn := b.resolved
	b.mu.Unlock()
	if fn != nil {
		fn(id, ans)
	}
}

// AskUser parks a clarifying question and returns immediately. The calling chat
// turn is then stopped quietly so the tab is fully idle until the human answers.
func (r engineReporter) AskUser(workspaceID, agentCode string, req mcp.AskUserRequest) string {
	q := domain.UserQuestion{
		WorkspaceID: workspaceID,
		AgentCode:   agentCode,
		Question:    req.Question,
		Context:     req.Context,
	}
	if a, ok := r.eng.Store.Agent(workspaceID, agentCode); ok {
		q.SessionID = a.SessionID
	}
	for _, o := range req.Options {
		q.Options = append(q.Options, domain.EscalationOption{Text: o.Text, Tag: o.Tag, Desc: o.Desc})
	}
	turn := r.eng.liveChatTurn(workspaceID, agentCode)
	r.eng.askUsers.park(q, turn)

	// Flush the MCP tool result, then park-stop only this asking turn.
	// Transcript arg backfill is handled generically by NoteToolArgs (onCallTool).
	go func() {
		time.Sleep(askUserParkDelay)
		r.eng.parkChatTurnIf(workspaceID, agentCode, turn)
	}()

	return askUserParkResult
}

// AnswerAskUser delivers a human answer to a parked ask_user prompt and starts
// a new chat turn with that answer as user input (session resumed).
// attachments are optional images from the chat composer (same as a normal send).
func (e *Engine) AnswerAskUser(id, choice, freeText string, attachments []domain.ChatAttachment) bool {
	ans := domain.UserQuestionAnswer{Choice: choice, FreeText: freeText}
	q, turn, ok := e.askUsers.Answer(id, ans)
	if !ok {
		return false
	}
	// Quiet-stop the asking turn only — never a newer unrelated send on the tab.
	e.parkChatTurnIf(q.WorkspaceID, q.AgentCode, turn)
	go func() {
		time.Sleep(50 * time.Millisecond)
		e.resumeAfterAskUser(q, ans, attachments)
	}()
	return true
}

// PendingAskUsers returns unanswered ask_user prompts.
func (e *Engine) PendingAskUsers() []domain.UserQuestion {
	return e.askUsers.Pending()
}

func (b *askUserBroker) pendingFor(workspaceID, code string) bool {
	if b == nil {
		return false
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	for _, p := range b.pending {
		if p.req.WorkspaceID == workspaceID && p.req.AgentCode == code {
			return true
		}
	}
	for _, q := range b.resuming {
		if q.WorkspaceID == workspaceID && q.AgentCode == code {
			return true
		}
	}
	return false
}

// clearAskUsersForWorkspace drops parked questions + inbox rows for a workspace
// (delete/archive). Safe when the broker is nil.
func (e *Engine) clearAskUsersForWorkspace(workspaceID string) {
	if e == nil || e.askUsers == nil {
		return
	}
	ids := e.askUsers.dropWorkspace(workspaceID)
	if len(ids) == 0 {
		return
	}
	for _, id := range ids {
		e.Store.RemoveInboxItem("ib-ask-" + id)
		e.Bus.Publish(Event{Name: "question.answered", Data: map[string]any{
			"id":     id,
			"answer": domain.UserQuestionAnswer{},
		}})
	}
	e.Bus.Publish(Event{Name: "inbox.updated", Data: map[string]any{"unread": e.Store.UnreadCount()}})
}

func (e *Engine) resumeAfterAskUser(q domain.UserQuestion, ans domain.UserQuestionAnswer, attachments []domain.ChatAttachment) {
	defer func() {
		if e.askUsers != nil {
			e.askUsers.dropResume(q.ID)
		}
		e.maybeArchiveLinkedPRChatHost(q.WorkspaceID)
	}()
	// Persist the FULL resume prompt as wire text for provider replay/forks.
	// display_text is the short Answer line for the chat bubble.
	prompt := formatAskUserResumePrompt(q, ans)
	display := formatAskUserDisplay(ans)
	code := strings.TrimSpace(q.AgentCode)
	if code == "" || code == maestroAgentCode {
		_ = e.SendMaestroMessage(q.WorkspaceID, prompt, attachments)
		return
	}
	// Reuse the tab's dock selection. Passing empty model/mode here falls back to
	// Settings' role default for the mode (Plan → Maestro, often Claude) while
	// Agent.SessionID may still be a Cursor agent-… id — Claude then rejects
	// `claude -p --resume agent-…`.
	model, flags, mode := e.WorkspaceChatSelection(q.WorkspaceID, code)
	_ = e.SendWorkspaceMessage(q.WorkspaceID, code, prompt, model, flags, attachments, mode, display)
}

func formatAskUserResumePrompt(q domain.UserQuestion, ans domain.UserQuestionAnswer) string {
	var b strings.Builder
	b.WriteString("The user answered your earlier ask_user question.\n\n")
	b.WriteString("Question: ")
	b.WriteString(strings.TrimSpace(q.Question))
	b.WriteByte('\n')
	if ctx := strings.TrimSpace(q.Context); ctx != "" {
		b.WriteString("Context: ")
		b.WriteString(ctx)
		b.WriteByte('\n')
	}
	choice := strings.TrimSpace(ans.Choice)
	free := strings.TrimSpace(ans.FreeText)
	switch {
	case choice != "" && free != "":
		b.WriteString("Answer: ")
		b.WriteString(choice)
		b.WriteString("\nDetails: ")
		b.WriteString(free)
	case choice != "":
		b.WriteString("Answer: ")
		b.WriteString(choice)
	case free != "":
		b.WriteString("Answer: ")
		b.WriteString(free)
	default:
		b.WriteString("Answer: (empty)")
	}
	b.WriteString("\n\nContinue with this answer. Do not re-ask the same question unless you still need clarification.")
	return b.String()
}

// formatAskUserDisplay is the short chat-bubble label for an ask_user answer.
func formatAskUserDisplay(ans domain.UserQuestionAnswer) string {
	choice := strings.TrimSpace(ans.Choice)
	free := strings.TrimSpace(ans.FreeText)
	switch {
	case choice != "" && free != "":
		return choice + " — " + free
	case choice != "":
		return choice
	case free != "":
		return free
	default:
		return "(empty)"
	}
}

func (e *Engine) publishAskUserRequested(q domain.UserQuestion) {
	short := q.WorkspaceID
	if l, ok := e.Store.Workspace(q.WorkspaceID); ok && l.Short != "" {
		short = l.Short
	}
	e.Store.AddInboxItem(domain.InboxItem{
		ID:          "ib-ask-" + q.ID,
		Group:       "needs",
		Unread:      true,
		Kind:        "pause",
		Color:       "var(--amber)",
		Title:       fmt.Sprintf("<b>%s</b> asked a question", html.EscapeString(q.AgentCode)),
		Sub:         html.EscapeString(truncate(short, 64)) + " · " + html.EscapeString(truncate(q.Question, 64)),
		Time:        "now",
		Action:      "Answer",
		ActionClass: "urgent",
		Target:      "question:" + q.ID,
		WorkspaceID: q.WorkspaceID,
	})
	e.Bus.Publish(Event{Name: "inbox.updated", Data: map[string]any{"unread": e.Store.UnreadCount()}})
	e.Bus.Publish(Event{Name: "question.asked", Data: map[string]any{
		"workspaceId": q.WorkspaceID,
		"question":    q,
	}})
	e.Bus.Publish(Event{Name: "notify", Data: map[string]any{
		"title":  "An agent has a question",
		"body":   truncate(q.Question, 120),
		"target": "question:" + q.ID,
	}})
}

func (e *Engine) publishAskUserResolved(id string, ans domain.UserQuestionAnswer) {
	e.Store.RemoveInboxItem("ib-ask-" + id)
	e.Bus.Publish(Event{Name: "inbox.updated", Data: map[string]any{"unread": e.Store.UnreadCount()}})
	payload := map[string]any{"id": id, "answer": ans}
	e.Bus.Publish(Event{Name: "question.answered", Data: payload})
}
