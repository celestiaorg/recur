package engine

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strconv"
	"strings"
	"time"

	"recur/internal/domain"
	"recur/internal/store"
)

const appChatTimeout = 120 * time.Second

var appChatTools = []string{mcpListWorkspaceBriefs, mcpGetWorkspaceContext, mcpPresentWorkspace}

const appChatAppendPrompt = "You are Ask Recur, Recur's app-scoped coordinator. You answer questions about " +
	"what needs attention, what is running, and where the human left off. Always call " +
	"mcp__recur__list_workspace_briefs before making a factual status claim — your transcript is " +
	"conversational memory, not current truth. Use mcp__recur__get_workspace_context for one " +
	"workspace's detail and mcp__recur__present_workspace to pin a workspace/chat/question/PR " +
	"card in this transcript (workspace:<id>, chat:<id>:<code>, question:<id>, pr:<id>). " +
	"present_workspace does not switch the UI — the human opens the card. You cannot edit files, " +
	"mutate workspaces, answer questions, approve permissions, retry runs, or message workspace " +
	"agents. If the user asks you to change code, tell them to open that workspace's Agent chat."

const appChatAgentIDPrefix = "app-"

// appCode normalizes an Ask Recur tab code ("" = the primary APP tab).
func appCode(code string) string {
	code = strings.TrimSpace(code)
	if code == "" {
		return domain.AppAgentCode
	}
	return code
}

// isAppChatCode reports whether code addresses an Ask Recur tab (APP, AP2, …).
func isAppChatCode(code string) bool {
	return code == domain.AppAgentCode || strings.HasPrefix(code, "AP")
}

// appChatAgentID is the agent.updated agentId for an Ask Recur tab. Empty
// workspaceId plus this prefix lets the UI light the chip without a fake
// workspace Agent row (or a sidebar running dot).
func appChatAgentID(code string) string {
	return appChatAgentIDPrefix + appCode(code)
}

func (e *Engine) publishAppChatStatus(code, status string) {
	if e == nil || e.Bus == nil {
		return
	}
	code = appCode(code)
	e.Bus.Publish(Event{Name: "agent.updated", Data: map[string]any{
		"workspaceId": "",
		"agentId":     appChatAgentID(code),
		"status":      status,
		"surface":     "app",
		"code":        code,
	}})
}

// appSessionByCode finds the newest OPEN app-kind session for one tab code.
// Legacy sessions without a config code count as the primary APP tab; closed
// sessions are skipped so a fresh tab can never resurrect their transcript.
func (e *Engine) appSessionByCode(code string) (domain.Session, bool) {
	code = appCode(code)
	list := e.Store.SessionsForAppKind(domain.SessionKindApp)
	for i := len(list) - 1; i >= 0; i-- {
		var meta struct {
			Closed bool `json:"closed"`
		}
		_ = json.Unmarshal([]byte(list[i].ConfigJSON), &meta)
		if meta.Closed {
			continue
		}
		cfg := parseAgentConfig(list[i].ConfigJSON)
		c := cfg.Code
		if c == "" {
			c = domain.AppAgentCode
		}
		if c == code {
			return list[i], true
		}
	}
	return domain.Session{}, false
}

// AppChatItems returns one Ask Recur tab's persistent transcript.
func (e *Engine) AppChatItems(code string) []domain.ChatItem {
	sess, ok := e.appSessionByCode(code)
	if !ok {
		return nil
	}
	msgs, err := e.Store.SessionTranscript(sess.ID)
	if err != nil {
		return nil
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	return flattenTranscript(msgs, cfg.Model, cfg.Flags)
}

// ListAppChats returns the open Ask Recur tabs, primary first, in creation
// order — the same shape the workspace/PR chat strips consume. The primary
// APP entry is always present: with no open APP session it represents the
// fresh empty chat the next send will create (close-the-last-tab reset).
func (e *Engine) ListAppChats() []domain.WorkspaceChatTab {
	tabs := []domain.WorkspaceChatTab{{Code: domain.AppAgentCode, Title: "Ask Recur", Mode: string(domain.ChatModeAsk)}}
	for _, sess := range e.Store.SessionsForAppKind(domain.SessionKindApp) {
		var cfg map[string]any
		_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
		if closed, _ := cfg["closed"].(bool); closed {
			continue
		}
		ac := parseAgentConfig(sess.ConfigJSON)
		code := ac.Code
		if code == "" {
			code = domain.AppAgentCode
		}
		unseen, _ := cfg["unseen"].(bool)
		if code == domain.AppAgentCode {
			tabs[0].Title = sessionTitle(ac, sess)
			if tabs[0].Title == domain.AppAgentCode {
				tabs[0].Title = "Ask Recur"
			}
			tabs[0].Unseen = unseen
			tabs[0].UpdatedAt = sess.UpdatedAt
			continue
		}
		tabs = append(tabs, domain.WorkspaceChatTab{
			Code: code, Title: sessionTitle(ac, sess), Mode: string(domain.ChatModeAsk),
			Unseen: unseen, UpdatedAt: sess.UpdatedAt,
		})
	}
	return tabs
}

// NewAppChat opens a fresh Ask Recur tab and returns it. Codes are AP2, AP3, …
// and are never reused, so a closed tab's history cannot leak into a new one.
func (e *Engine) NewAppChat() domain.WorkspaceChatTab {
	maxN := 1
	for _, sess := range e.Store.SessionsForAppKind(domain.SessionKindApp) {
		cfg := parseAgentConfig(sess.ConfigJSON)
		if n, err := strconv.Atoi(strings.TrimPrefix(cfg.Code, "AP")); err == nil && n > maxN {
			maxN = n
		}
	}
	code := fmt.Sprintf("AP%d", maxN+1)
	sess := e.ensureAppSession(code, "")
	if sess.ID == "" {
		return domain.WorkspaceChatTab{}
	}
	return domain.WorkspaceChatTab{Code: code, Title: "New chat", Mode: string(domain.ChatModeAsk), UpdatedAt: sess.UpdatedAt}
}

// CloseAppChat closes an Ask Recur tab (history stays in the session store,
// same as CloseWorkspaceChat). Closing the primary APP tab resets it to a
// fresh empty chat — the next send creates a new session under the same code.
// Returns the remaining open tabs.
func (e *Engine) CloseAppChat(code string) []domain.WorkspaceChatTab {
	code = appCode(code)
	if sess, ok := e.appSessionByCode(code); ok {
		var cfg map[string]any
		_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
		if cfg == nil {
			cfg = map[string]any{}
		}
		cfg["closed"] = true
		delete(cfg, "unseen")
		b, _ := json.Marshal(cfg)
		_ = e.Store.UpdateSessionConfigJSON(sess.ID, string(b))
		e.publishAttention("", code, false)
	}
	return e.ListAppChats()
}

// AppChatSelection returns the last model/flags for one tab (mode is always ask).
func (e *Engine) AppChatSelection(code string) (model string, flags map[string]string) {
	sess, ok := e.appSessionByCode(code)
	if !ok {
		set := e.settings()
		return set.AskModel().String(), set.AskFlags()
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	if cfg.Model == "" {
		set := e.settings()
		return set.AskModel().String(), set.AskFlags()
	}
	return cfg.Model, cfg.Flags
}

// RememberAppChatSelection persists a tab's model picker.
func (e *Engine) RememberAppChatSelection(code, model string, flags map[string]string) {
	code = appCode(code)
	model = strings.TrimSpace(model)
	sess := e.ensureAppSession(code, model)
	if sess.ID == "" {
		return
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	cfg.Code = code
	if cfg.Name == "" {
		cfg.Name = appChatTabName(code)
	}
	cfg.Mode = domain.ChatModeAsk
	if model != "" {
		cfg.Model = model
		cfg.Flags = flags
		p := domain.ParseModelRef(model).Provider
		if p != "" && sess.Provider != p {
			_ = e.Store.UpdateSessionProvider(sess.ID, p)
		}
	}
	b, _ := json.Marshal(cfg)
	_ = e.Store.UpdateSessionConfigJSON(sess.ID, string(b))
}

func appChatTabName(code string) string {
	if code == domain.AppAgentCode {
		return "Ask Recur"
	}
	return "New chat"
}

func (e *Engine) ensureAppSession(code, model string) domain.Session {
	code = appCode(code)
	if sess, ok := e.appSessionByCode(code); ok {
		return sess
	}
	provider := domain.ParseModelRef(model).Provider
	if provider == "" {
		provider = domain.ProviderClaudeCode
	}
	cfg, _ := json.Marshal(agentConfig{Code: code, Name: appChatTabName(code), Mode: domain.ChatModeAsk, Model: model})
	sess, err := e.Store.CreateSession(domain.Session{
		Kind:        domain.SessionKindApp,
		ContextMode: domain.ContextModeProvider,
		Provider:    provider,
		ConfigJSON:  string(cfg),
	})
	if err != nil {
		return domain.Session{}
	}
	return sess
}

// stampAppChatTitle labels a tab from its first user line (same rule as
// workspace chat tabs) so the strip reads by topic, not "New chat" forever.
func (e *Engine) stampAppChatTitle(code, text string) {
	sess, ok := e.appSessionByCode(code)
	if !ok {
		return
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	if strings.TrimSpace(cfg.Title) != "" {
		return
	}
	if msgs, err := e.Store.SessionTranscript(sess.ID); err != nil || len(msgs) > 0 {
		return
	}
	title := firstUserChatTitle(text)
	if title == "" {
		return
	}
	if err := e.Store.PatchSessionConfigTitle(sess.ID, title); err != nil {
		return
	}
	if e.Bus != nil {
		e.Bus.Publish(Event{Name: "app.chat_tab", Data: map[string]any{"code": code, "title": title}})
	}
}

// SendAppMessage runs one Ask Recur turn on a tab. Governor wait is cancelable.
func (e *Engine) SendAppMessage(code, text, model string, flags map[string]string) string {
	code = appCode(code)
	text = strings.TrimSpace(text)
	if text == "" {
		return ""
	}
	cwd, err := store.AppChatDir()
	if err != nil {
		return "I can't run right now: " + err.Error()
	}
	set := e.settings()
	if model == "" {
		model = set.AskModel().String()
	}
	if flags == nil {
		flags = set.AskFlags()
	}
	e.RememberAppChatSelection(code, model, flags)
	e.stampAppChatTitle(code, text)
	spec := AgentSpec{
		AgentID:            code,
		Role:               domain.RoleMaestro,
		Model:              model,
		Flags:              flags,
		AppendSystemPrompt: appChatAppendPrompt,
		AllowedTools:       appChatTools,
		Prompt:             text,
		Worktree:           cwd,
		PermissionMode:     set.PermissionMode(),
		IncludePartials:    true,
		Route: SessionRoute{
			Kind: domain.SessionKindApp,
			Code: code,
			Name: appChatTabName(code),
		},
	}
	token, mcpURL, err := e.registerMCP("", code, code, spec.AllowedTools, spec.Model)
	if err != nil {
		return "I can't run right now: " + err.Error()
	}
	defer e.unregisterMCP(token)
	spec.MCPURL = mcpURL

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	turn := e.registerChatTurn("", code, cancel)
	if turn == nil {
		return "I can't start Ask Recur right now."
	}
	turn.arm(appChatTimeout)
	defer e.unregisterChatTurn("", code, turn)
	release, gotSlot := e.acquireAgent(ctx.Done())
	if !gotSlot {
		if errors.Is(ctx.Err(), context.Canceled) {
			return chatStoppedReply
		}
		return "I'm at capacity right now — every agent slot is busy. Try again in a moment."
	}
	defer release()

	e.publishAppChatStatus(code, "working")
	var turnErr error
	defer func() {
		// Same shutdown caveat as workspace chat: a cancelled unwind during
		// quit must stay live so Recover can resume; anything else goes idle.
		if e.isShuttingDown() && errors.Is(turnErr, context.Canceled) {
			return
		}
		e.publishAppChatStatus(code, "idle")
	}()

	stream := &chatStream{e: e, workspaceID: "", agentCode: code}
	started := time.Now()
	var res Result
	res, turnErr = e.runAgent(ctx, spec, stream.emit)
	e.publishResource()

	reply := e.finalizeChatReply("", code, text, "", stream.text(), res, turnErr,
		"I couldn't produce a reply.", started, spec.SkipRecordUser, spec.UserMessageID)
	if !errors.Is(turnErr, context.Canceled) && !e.isAppChatFocused(code) {
		e.flagAppChatUnseen(code)
	}
	return reply
}

// CancelAppMessage interrupts a tab's in-flight Ask Recur turn.
func (e *Engine) CancelAppMessage(code string) {
	e.CancelChat("", appCode(code))
}

// SetAppChatFocus records which Ask Recur tab is visible ("" code = panel hidden).
func (e *Engine) SetAppChatFocus(visible bool, code string) {
	code = appCode(code)
	e.chatFocusMu.Lock()
	if visible {
		e.chatFocus.workspaceID = ""
		e.chatFocus.code = code
	} else if isAppChatCode(e.chatFocus.code) && e.chatFocus.workspaceID == "" {
		e.chatFocus.workspaceID = ""
		e.chatFocus.code = ""
	}
	attentive := visible && e.chatSurfaceAttentiveLocked()
	e.chatFocusMu.Unlock()
	if attentive {
		e.MarkAppChatSeen(code)
	}
}

func (e *Engine) isAppChatFocused(code string) bool {
	e.chatFocusMu.Lock()
	defer e.chatFocusMu.Unlock()
	return e.chatSurfaceAttentiveLocked() && e.chatFocus.code == appCode(code) && e.chatFocus.workspaceID == ""
}

// MarkAppChatSeen clears one tab's unseen flag.
func (e *Engine) MarkAppChatSeen(code string) {
	code = appCode(code)
	if e.setAppChatUnseen(code, false) {
		e.publishAttention("", code, false)
	}
}

func (e *Engine) flagAppChatUnseen(code string) {
	code = appCode(code)
	if e.isAppChatFocused(code) {
		return
	}
	if e.setAppChatUnseen(code, true) {
		e.publishAttention("", code, true)
	}
}

// setAppChatUnseen persists one tab's unseen flag, reporting whether it changed.
func (e *Engine) setAppChatUnseen(code string, unseen bool) bool {
	sess := e.ensureAppSession(code, "")
	if sess.ID == "" {
		return false
	}
	var cfg map[string]any
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	if cfg == nil {
		cfg = map[string]any{}
	}
	cur, _ := cfg["unseen"].(bool)
	if cur == unseen {
		return false
	}
	if unseen {
		cfg["unseen"] = true
	} else {
		delete(cfg, "unseen")
	}
	b, _ := json.Marshal(cfg)
	_ = e.Store.UpdateSessionConfigJSON(sess.ID, string(b))
	return true
}

// PresentWorkspace validates a navigation target and returns a JSON card payload.
// It does not switch the visible shell — Ask Recur renders the card in-transcript.
func (e *Engine) PresentWorkspace(target string) string {
	target = strings.TrimSpace(target)
	out := domain.PresentWorkspaceResult{Target: target}
	if target == "" {
		return marshalPresentWorkspace(domain.PresentWorkspaceResult{
			Error: "Refused: target is required (workspace:<id>, chat:<id>:<code>, question:<id>, or pr:<id>).",
		})
	}
	kind, _, _ := strings.Cut(target, ":")
	out.Kind = kind
	if err := e.validatePresentTarget(target); err != nil {
		out.Error = "Refused: " + err.Error()
		return marshalPresentWorkspace(out)
	}
	out.OK = true
	out.Brief = e.briefForPresentTarget(target)
	return marshalPresentWorkspace(out)
}

func marshalPresentWorkspace(out domain.PresentWorkspaceResult) string {
	b, err := json.Marshal(out)
	if err != nil {
		if out.OK {
			return "Presented " + out.Target + "."
		}
		if out.Error != "" {
			return out.Error
		}
		return "Refused: could not present that target."
	}
	return string(b)
}

// briefForPresentTarget attaches the Overview card for a validated target so
// the transcript can render it without a second round-trip. Missing from the
// Overview (settled / empty) still yields a minimal identity card.
func (e *Engine) briefForPresentTarget(target string) *domain.WorkspaceBrief {
	if e == nil || e.Store == nil {
		return nil
	}
	kind, rest, _ := strings.Cut(target, ":")
	rest = strings.TrimSpace(rest)
	wsID := ""
	switch kind {
	case "workspace", "loop":
		wsID, _, _ = strings.Cut(rest, ":")
	case "chat":
		wsID, _, _ = strings.Cut(rest, ":")
	case "pr":
		for _, p := range e.Store.PRs() {
			if p.ID == rest {
				wsID = p.WorkspaceID
				break
			}
		}
	case "question":
		for _, q := range e.PendingAskUsers() {
			if q.ID == rest {
				wsID = q.WorkspaceID
				break
			}
		}
	}
	wsID = strings.TrimSpace(wsID)
	if wsID == "" {
		return nil
	}
	for _, b := range e.BriefingSnapshot().Workspaces {
		if b.WorkspaceID == wsID {
			cp := b
			return &cp
		}
	}
	ws, ok := e.Store.Workspace(wsID)
	if !ok || ws.Archived {
		return nil
	}
	title := strings.TrimSpace(ws.Short)
	if title == "" {
		title = strings.TrimSpace(ws.Goal)
	}
	if title == "" {
		title = ws.ID
	}
	b := domain.WorkspaceBrief{
		WorkspaceID: ws.ID,
		ProjectID:   ws.ProjectID,
		Title:       title,
		Branch:      ws.Branch,
		State:       string(ws.State),
		Phase:       ws.Phase,
		Primary:     domain.BriefAction{Label: "Open", Target: target, Kind: domain.BriefActionOpen},
	}
	if p, ok := e.Store.Project(ws.ProjectID); ok {
		name := strings.TrimSpace(p.Name)
		if name == "" {
			name = p.ID
		}
		b.ProjectName = name
	}
	return &b
}

func (e *Engine) validatePresentTarget(target string) error {
	kind, rest, _ := strings.Cut(target, ":")
	rest = strings.TrimSpace(rest)
	switch kind {
	case "workspace", "loop":
		wsID, _, _ := strings.Cut(rest, ":")
		ws, ok := e.Store.Workspace(wsID)
		if !ok || ws.Archived {
			return errors.New("that workspace is not available")
		}
		return nil
	case "chat":
		wsID, code, ok := strings.Cut(rest, ":")
		if !ok {
			return errors.New("chat target must be chat:<workspaceId>:<code>")
		}
		ws, exists := e.Store.Workspace(wsID)
		if !exists || ws.Archived {
			return errors.New("that workspace is not available")
		}
		if !isChatAttentionCode(code) {
			return errors.New("unknown chat tab")
		}
		return nil
	case "question":
		for _, q := range e.PendingAskUsers() {
			if q.ID == rest {
				return nil
			}
		}
		return errors.New("that question is no longer pending")
	case "pr":
		for _, p := range e.Store.PRs() {
			if p.ID == rest {
				return nil
			}
		}
		return errors.New("that pull request is not available")
	default:
		return errors.New("unknown target kind")
	}
}
