package engine

import (
	"encoding/json"
	"strings"

	"recur/internal/domain"
	"recur/internal/simplify"
)

// simplifyChatAppend is the condensed rails for a workspace chat turn.
// Per-session: the composer chip (nil → Settings default). Ask/Review stay off.
func (e *Engine) simplifyChatAppend(workspaceID, code, chatMode string) string {
	if !simplify.AppliesToChat(chatMode) || !e.effectiveChatSimplify(workspaceID, code) {
		return ""
	}
	return simplify.Instructions()
}

// simplifyCoderAppend is the condensed rails for a loop Coder turn.
// A loop that's on in the composer stays on for agents the loop starts
// (Coder writes code). Reads the Maestro session, not the Coder's own.
func (e *Engine) simplifyCoderAppend(workspaceID string) string {
	if !e.effectiveChatSimplify(workspaceID, maestroAgentCode) {
		return ""
	}
	return simplify.Instructions()
}

// ChatSimplify returns the session's explicit Simplify flag (nil = never set).
func (e *Engine) ChatSimplify(workspaceID, code string) *bool {
	code = normalizePairChatCode(code)
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return nil
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	return cfg.Simplify
}

// effectiveChatSimplify is the inject gate: an explicit session flag wins,
// otherwise Settings.SimplifyEnabled (nil/omitted → on).
func (e *Engine) effectiveChatSimplify(workspaceID, code string) bool {
	if s := e.ChatSimplify(workspaceID, code); s != nil {
		return *s
	}
	return e.settings().SimplifyEnabled()
}

// inheritSimplify picks a loop's starting Simplify: the pair-chat that filed
// the plan (Plan-panel "Run as loop"), else the newest pair-chat that already
// chose, else the Settings default.
func (e *Engine) inheritSimplify(workspaceID string) bool {
	if ws, ok := e.Store.Workspace(workspaceID); ok && ws.Plan != nil {
		if code := strings.TrimSpace(ws.Plan.ChatCode); isPairChatCode(code) {
			if s := e.ChatSimplify(workspaceID, code); s != nil {
				return *s
			}
		}
	}
	var best int64
	var found *bool
	for _, a := range e.pairChatAgents(workspaceID) {
		sess, ok := e.Store.SessionForLoopAgent(workspaceID, a.Code)
		if !ok {
			continue
		}
		var cfg agentConfig
		_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
		if cfg.Simplify == nil {
			continue
		}
		if found == nil || sess.UpdatedAt >= best {
			best = sess.UpdatedAt
			found = cfg.Simplify
		}
	}
	if found != nil {
		return *found
	}
	return e.settings().SimplifyEnabled()
}

// stampMaestroSimplifyIfUnset freezes Simplify on the Loop (MA) session at
// run start so a later Settings change does not flip an in-flight loop.
// Atomic if-unset so a concurrent MA chip toggle is not overwritten.
func (e *Engine) stampMaestroSimplifyIfUnset(workspaceID string) {
	kind, name := string(domain.RoleMaestro), "Maestro"
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, maestroAgentCode)
	if !ok {
		sess, ok = e.ensureAgentSession(workspaceID, kind, maestroAgentCode, name, "")
		if !ok {
			return
		}
	}
	_ = e.Store.PatchSessionConfigSimplifyIfUnset(sess.ID, e.inheritSimplify(workspaceID))
}

// RememberWorkspaceChatSimplify persists the composer Simplify chip for a
// chat tab (PA* or the Loop/MA tab) without sending a turn.
// Validates like RememberWorkspaceChatSelection: workspace must exist, and a
// missing pair-chat agent is not recreated (so a Remember racing
// CloseWorkspaceChat cannot resurrect a just-closed tab). MA is allowed
// without an agent row — the Loop tab is not closed that way. Neither path
// creates a session (that raced the first send).
func (e *Engine) RememberWorkspaceChatSimplify(workspaceID, code string, on bool) {
	if _, ok := e.Store.Workspace(workspaceID); !ok {
		return
	}
	code = normalizePairChatCode(code)
	if code == maestroAgentCode {
		e.rememberChatSimplify(workspaceID, code, on)
		return
	}
	if !isPairChatCode(code) {
		return
	}
	if _, ok := e.Store.Agent(workspaceID, code); !ok {
		return
	}
	e.rememberChatSimplify(workspaceID, code, on)
}

// rememberChatSimplify writes cfg.Simplify onto the tab's durable session.
// Patch only: minting an empty row from a chip toggle raced the first send
// (duplicate PA/MA, vanished transcript). Create via dock selection (PA) or
// stampMaestroSimplifyIfUnset / attachMirror (MA).
func (e *Engine) rememberChatSimplify(workspaceID, code string, on bool) {
	code = normalizePairChatCode(code)
	if code != maestroAgentCode && !isPairChatCode(code) {
		return
	}
	if isPairChatCode(code) {
		if _, ok := e.Store.Agent(workspaceID, code); !ok {
			return
		}
	}
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return
	}
	_ = e.Store.PatchSessionConfigSimplify(sess.ID, on)
}
