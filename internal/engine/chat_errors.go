package engine

import (
	"context"
	"errors"
	"strings"
	"time"

	"recur/internal/domain"
)

// chatStoppedReply is the durable / live note when the user interrupts a turn.
const chatStoppedReply = "Stopped by user"

// chatInterruptedReply is persisted when Recover heals a turn the process
// did not finish (quit/crash). Distinct from Stopped by user so AutoResume
// can continue the work instead of treating it as a deliberate cancel.
const chatInterruptedReply = "Interrupted: Recur closed before this turn finished."

// chatResumePrompt / chatResumeDisplay are the AutoResume continuation turn
// after Recover queued an interrupted chat. Display is the composer bubble;
// the wire prompt tells the resumed provider session to pick up, not restart.
const chatResumePrompt = "You were interrupted because Recur restarted. Continue from where you left off."
const chatResumeDisplay = "Recur restarted: continuing"

// chatAgentErrorPrefix is the stable prefix on synthesized chat failure replies.
// The frontend uses it to append the note even when some assistant text already
// streamed (otherwise finishTurn would drop the provider error).
const chatAgentErrorPrefix = "Agent error: "

// chatRetryDisplaySentinel is the UI-only display_text the chat Retry button
// sends so the engine can skip a duplicate user row. Word joiner (U+2060);
// keep in sync with frontend CHAT_RETRY_DISPLAY.
const chatRetryDisplaySentinel = "\u2060"

func isChatRetryDisplay(s string) bool {
	return s == chatRetryDisplaySentinel
}

// chatInjectedSentinel is the SendWorkspaceMessage reply when a follow-up was
// injected into a live Cursor turn (CancelRun + Send) instead of starting a
// new Run. Word joiner + "inject" — keep in sync with frontend CHAT_INJECTED_REPLY.
const chatInjectedSentinel = "\u2060inject"

// chatTurnTimedOutReply is the note when chatTurn's wedged-turn deadline fires.
// Prefer this over surfacing Go's bare "context deadline exceeded".
const chatTurnTimedOutReply = chatAgentErrorPrefix +
	"this turn hit Recur's time limit — send again to continue (use Stop if it looked stuck)."

// formatChatAgentError builds a user-visible chat reply for a failed turn.
// Prefers the provider's own words (same assembly as loop D56) so Cursor/Claude
// failures are not a silent "nothing happened" after the UI resyncs from the
// session store.
func formatChatAgentError(err error, res Result) string {
	if errors.Is(err, context.DeadlineExceeded) {
		return chatTurnTimedOutReply
	}
	detail := strings.TrimSpace(agentFailureReason(err, res))
	if detail == "" || detail == "the agent turn failed" {
		return "I hit an error trying to respond just now. Try again."
	}
	return chatAgentErrorPrefix + truncate(detail, 400)
}

// persistChatFailure records a failed chat turn in the durable session so
// post-send resync keeps it visible. When the failure happened before the
// mirror attached (e.g. missing Cursor key), the session may not exist yet and
// the user prompt was never stored — ensure both (wire text + optional
// display_text), except Retry, switch_mode, and skipUser remounts, which stay
// off the transcript. userMsgID, when set, is the preassigned id of that user
// row so a start/end checkpoint taken under it still resolves. durationMS,
// when > 0, is stamped on the note so the
// "completed in …" footer survives reload (same place a normal turn's provider
// duration lands). Token/context fields from usage ride on that same stamp so
// the composer meter reloads even when KindResult cleared DurationMS to keep
// the footer after this note.
func (e *Engine) persistChatFailure(workspaceID, code, prompt, displayText, errNote string, durationMS int64, usage Result, skipUser bool, userMsgID string) {
	errNote = strings.TrimSpace(errNote)
	if e == nil || e.Store == nil || errNote == "" {
		return
	}
	if workspaceID == "" && code != domain.AppAgentCode {
		return
	}
	if workspaceID != "" && e.workspaceTeardownCommitted(workspaceID) {
		return
	}
	sess, ok := e.chatSessionForPersist(workspaceID, code)
	if !ok {
		return
	}
	prompt = strings.TrimSpace(prompt)
	lookup := strings.TrimSpace(displayText)
	if !skipUser && !isChatRetryDisplay(lookup) && !isModeSwitchContinueDisplay(lookup) {
		if lookup == "" {
			lookup = prompt
		}
		if prompt != "" && !e.sessionHasPersistedUser(sess.ID, userMsgID, lookup) {
			blocks := userContentBlocks(prompt, displayText, nil)
			if len(blocks) > 0 {
				_, _ = e.Store.AppendMessage(sess.ID, domain.Message{
					ID:      userMsgID,
					Role:    "user",
					Content: blocks,
				})
			}
		}
	}
	msg, err := e.Store.AppendMessage(sess.ID, domain.Message{
		Role:         "assistant",
		StreamStatus: domain.StreamError,
		Content:      []domain.ContentBlock{domain.TextBlock(errNote)},
	})
	if err != nil || msg.ID == "" || durationMS <= 0 {
		return
	}
	_ = e.Store.CompleteMessage(msg.ID, domain.StreamError, domain.Message{
		DurationMS:          durationMS,
		InputTokens:         usage.InputTokens,
		OutputTokens:        usage.OutputTokens,
		CacheCreationTokens: usage.CacheCreationTokens,
		CacheReadTokens:     usage.CacheReadTokens,
		ContextWindow:       usage.ContextWindow,
	})
}

// chatFailureDuration is the turn wall time to stamp on a synthesized Stop or
// Agent-error note. If this run already published session.turn_end, return 0
// so persistChatFailure does not emit a second footer.
func chatFailureDuration(res Result, started time.Time) int64 {
	if res.TurnEndEmitted {
		return 0
	}
	dur := int64(res.DurationMS)
	if dur <= 0 && !started.IsZero() {
		dur = time.Since(started).Milliseconds()
	}
	return dur
}

// chatSessionForPersist returns the agent's session, creating it when a
// pre-mirror failure left none (so the error note has somewhere to land).
func (e *Engine) chatSessionForPersist(workspaceID, code string) (domain.Session, bool) {
	if workspaceID == "" {
		return e.Store.SessionForAppKind(domain.SessionKindApp)
	}
	if sess, ok := e.Store.SessionForLoopAgent(workspaceID, code); ok {
		return sess, true
	}
	kind, name, provider := e.chatSessionMeta(workspaceID, code)
	return e.ensureAgentSession(workspaceID, kind, code, name, provider)
}

// chatSessionMeta picks role/name/provider for a chat tab's durable session.
func (e *Engine) chatSessionMeta(workspaceID, code string) (kind, name, provider string) {
	if code == maestroAgentCode {
		name = "Maestro"
		provider = domain.ProviderClaudeCode
		if a, ok := e.Store.Agent(workspaceID, code); ok {
			if a.Name != "" {
				name = a.Name
			}
			if ref := domain.ParseModelRef(a.Model); ref.Provider != "" {
				provider = ref.Provider
			}
		}
		return string(domain.RoleMaestro), name, provider
	}
	name = "Agent"
	provider = domain.ProviderClaudeCode
	if a, ok := e.Store.Agent(workspaceID, code); ok {
		if a.Name != "" {
			name = a.Name
		}
		if ref := domain.ParseModelRef(a.Model); ref.Provider != "" {
			provider = ref.Provider
		}
	}
	return string(domain.RoleCoder), name, provider
}

// sessionHasPersistedUser reports whether persistChatFailure should skip
// synthesizing a user row. When userMsgID is set (the checkpoint key), only
// that exact message counts — same visible text on an older prompt must not
// hide a new turn. Empty userMsgID keeps the legacy text match for callers
// that do not preassign an id.
func (e *Engine) sessionHasPersistedUser(sessionID, userMsgID, userText string) bool {
	if userMsgID != "" {
		msgs, err := e.Store.SessionTranscript(sessionID)
		if err != nil {
			return false
		}
		for _, m := range msgs {
			if m.ID == userMsgID {
				return true
			}
		}
		return false
	}
	return e.sessionHasChatUserPrompt(sessionID, userText)
}

// sessionHasChatUserPrompt reports whether the transcript already has this
// human chat prompt (mirror recordUser). Walks newest-first and ignores
// tool_result carrier rows.
func (e *Engine) sessionHasChatUserPrompt(sessionID, userText string) bool {
	msgs, err := e.Store.SessionTranscript(sessionID)
	if err != nil || len(msgs) == 0 {
		return false
	}
	want := strings.TrimSpace(userText)
	for i := len(msgs) - 1; i >= 0; i-- {
		m := msgs[i]
		if m.Role != "user" || !userMessageIsChatPrompt(m) {
			continue
		}
		it, ok := flattenUserMessage(m)
		if !ok {
			continue
		}
		return strings.TrimSpace(it.Text) == want
	}
	return false
}

// finalizeChatReply picks the reply shown in chat after a turn. Failed turns
// always synthesize a provider-aware error note and persist it (even when some
// assistant text already streamed) so reload/resync keep the failure visible.
// prompt/displayText match AgentSpec (wire + optional UI label) so pre-mirror
// failures still store the same shape. A user interrupt (context.Canceled)
// becomes a calm "Stopped by user" note — not an "Agent error: context canceled"
// scare — and records how long the turn ran (provider duration when present,
// else wall time since started).
func (e *Engine) finalizeChatReply(workspaceID, code, prompt, displayText, streamed string, res Result, err error, emptyFallback string, started time.Time, skipUser bool, userMsgID string) string {
	streamed = strings.TrimSpace(streamed)
	if errors.Is(err, context.Canceled) {
		// ask_user park-stop: tab goes idle with the question card; do not
		// surface "Stopped by user" or persist a failure note.
		if e != nil && e.chatTurnWasParked(workspaceID, code) {
			return streamed
		}
		// Wedged-turn deadline (chatTurn.arm) also cancels with Canceled — report
		// it as a timeout, not a human Stop click.
		if e != nil && e.chatTurnWasTimedOut(workspaceID, code) {
			e.persistChatFailure(workspaceID, code, prompt, displayText, chatTurnTimedOutReply, 0, Result{}, skipUser, userMsgID)
			return chatTurnTimedOutReply
		}
		// App quit cancelled this turn so processes can be reaped. Leave the
		// transcript and agent status for Recover + AutoResume (same as a
		// loop interrupt) instead of recording "Stopped by user".
		if e != nil && e.isShuttingDown() {
			return streamed
		}
		e.persistChatFailure(workspaceID, code, prompt, displayText, chatStoppedReply, chatFailureDuration(res, started), res, skipUser, userMsgID)
		return chatStoppedReply
	}
	failed := err != nil || res.IsError
	if failed {
		note := formatChatAgentError(err, res)
		e.persistChatFailure(workspaceID, code, prompt, displayText, note, chatFailureDuration(res, started), res, skipUser, userMsgID)
		if workspaceID != "" {
			e.touchWorkspaceActivity(workspaceID, "Chat turn failed", e.now())
		}
		return note
	}
	out := streamed
	if out == "" {
		out = strings.TrimSpace(res.Text)
	}
	if out == "" {
		out = emptyFallback
	}
	if workspaceID != "" {
		e.touchWorkspaceActivity(workspaceID, "Chat turn finished", e.now())
	}
	return out
}
