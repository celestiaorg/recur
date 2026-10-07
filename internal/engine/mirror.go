package engine

import (
	"context"
	"encoding/json"
	"strings"

	"github.com/google/uuid"

	"recur/internal/domain"
	"recur/internal/store"
)

// agentConfig is the per-session config_json Recur writes for an agent session:
// the feed identity (avatar code + display name) the activity-feed projection
// renders rows under. Distinct from the provider config (system prompt, tools),
// which claude-code owns; this is what Recur needs to render the transcript.
//
// Model/Flags remember the last dock selection for solo/pair chat (code PA) so
// reopening the workspace restores the pickers. Title is the Agent-panel tab
// label (LLM-upgraded from the first user message).
type agentConfig struct {
	Code  string            `json:"code"`
	Name  string            `json:"name"`
	Title string            `json:"title,omitempty"` // chat tab label (PA / PA2 / …)
	Model string            `json:"model,omitempty"` // qualified ref, e.g. cursor/composer-2.5
	Flags map[string]string `json:"flags,omitempty"`
	// Mode is the chat tab's agent mode (ask/plan/agent/review/loop) — persisted
	// per session like Model/Flags so a reload restores the dock's mode selector.
	Mode string `json:"mode,omitempty"`
	// StandingMode is the chat mode whose notes were last attached to the
	// provider session. Dock picks update Mode immediately; StandingMode lags
	// until a turn actually succeeds so a human mode change can re-attach notes.
	StandingMode string `json:"standingMode,omitempty"`
	// StandingPromptSHA is sha256 of the SystemPrompt last attached to a
	// Cursor/Codex session (Maestro live context). Reinject when it changes.
	StandingPromptSHA string `json:"standingPromptSha,omitempty"`
	// Simplify is the composer chip for this session. nil = never set (inject
	// uses Settings.SimplifyEnabled). Explicit true/false is a session override;
	// Settings is only the default for new chats.
	Simplify *bool `json:"simplify,omitempty"`
	// LoopID / PolicyID are this chat's autopilot (one chat = one loop).
	LoopID   string `json:"loopId,omitempty"`
	PolicyID string `json:"policyId,omitempty"`
	// Goal is this chat's objective. Workspace.Goal stays the owner's until
	// this chat becomes owner; reenter/Maestro restore from here.
	Goal string `json:"goal,omitempty"`
	// GoalDisplay is the prompt-action label for Goal ("Review & fix").
	GoalDisplay string `json:"goalDisplay,omitempty"`
	// GoalImages is the opening paste set for this loop (attachment ids).
	// Non-nil (including empty) means the set was recorded at engage/create;
	// nil is legacy and falls back to the first chat user prompt.
	GoalImages *[]string `json:"goalImages,omitempty"`
	// LoopStarted is true after this chat engaged its own run.
	LoopStarted bool `json:"loopStarted,omitempty"`
	// OwnerCode is the conductor tab (PA*, MA) this specialist session belongs
	// to. Empty on pair/Maestro rows and on pre-isolation specialist transcripts.
	OwnerCode string `json:"ownerCode,omitempty"`
	// ForkInject is a one-shot: the next send prepends the copied transcript
	// so a fresh provider session can continue. Cleared once that send carries
	// the preamble (Retry uses the stored wire and must not inject again).
	ForkInject bool `json:"forkInject,omitempty"`
	// PromptGroup is the shared slot for agent-mode fresh_session flagships.
	PromptGroup string `json:"promptGroup,omitempty"`
	// NativeHistoryAfter is the last transcript message id before a
	// fresh_session reset. App-mode harnesses skip through this id. Cleared on
	// fork so the copied transcript is the new tab's context.
	NativeHistoryAfter string `json:"nativeHistoryAfter,omitempty"`
}

func nativeHistoryAfterID(configJSON string) string {
	var cfg agentConfig
	_ = json.Unmarshal([]byte(configJSON), &cfg)
	return strings.TrimSpace(cfg.NativeHistoryAfter)
}

// mirroringRunner wraps an AgentRunner so every turn is mirrored into the session
// store without each call site opting in. It attaches a
// per-turn mirror (creating the driving agent's session on first use), delegates
// to the inner runner, then finishes the mirror. Wrapping is the whole S1 mirror
// wiring: because all specialist and maestro turns funnel through e.runAgent, one
// wrapper covers them all — nothing to forget at a new turn site.
type mirroringRunner struct {
	inner AgentRunner
	eng   *Engine
}

// withMirroring wraps r so its turns mirror through eng, unless r is already
// wrapped (idempotent, so New + SetRunner can both call it safely).
func withMirroring(eng *Engine, r AgentRunner) AgentRunner {
	if r == nil {
		return nil
	}
	if _, ok := r.(mirroringRunner); ok {
		return r
	}
	return mirroringRunner{inner: r, eng: eng}
}

func (r mirroringRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	finish := r.eng.attachMirror(&spec)
	defer finish()
	// Mirror + broadcast parsed events by wrapping emit — the one path EVERY
	// runner drives, including test doubles that synthesize events without going
	// through the parser. The mirror persists the lossless transcript; the
	// broadcaster streams the SAME events onto the bus tagged (workspaceId, code)
	// so any surface renders any agent's session live — the chat docks and the
	// Activity panel alike, with no per-callsite publishing.
	bc := r.eng.newSessionBroadcast(&spec)
	if !spec.SkipRecordUser && bc != nil {
		if shown := userBubbleText(spec.Prompt, spec.DisplayText); shown != "" {
			bc.publish("session.user", map[string]any{"text": shown})
		}
	}
	var turnEndEmitted bool
	if spec.Mirror != nil || bc != nil {
		m := spec.Mirror
		inner := emit
		// Claude stream-json can re-emit the same tool_use (same toolu_… id) on
		// multiple assistant NDJSON lines — e.g. one line per content block when
		// thinking is on. Cursor had the same failure mode (emitStepOnce). Every
		// KindStep mints a tool_use row and a live session.step, so duplicates
		// triple the transcript and orphan results onto the last clone only.
		seenSteps := map[string]bool{}
		emit = func(ev ParsedEvent) {
			// Cursor follow-up Send restarts the run and may reuse call ids.
			// A live delivered steer keeps those tools running.
			if ev.Kind == KindUser && spec.Provider == domain.ProviderCursor && !ev.LiveSteer {
				seenSteps = map[string]bool{}
			}
			if ev.Kind == KindStep && ev.ToolUseID != "" {
				if seenSteps[ev.ToolUseID] {
					return
				}
				seenSteps[ev.ToolUseID] = true
			}
			// Resolve the active context limit before mirror/broadcast.
			// Claude: prefer model+dock flags (always-1M models; 4.6's 200K↔1M).
			// Cursor: no provider window — use the hardcoded model table + flags.
			if (ev.Kind == KindResult || ev.Kind == KindUsage) && ev.Result != nil {
				switch spec.Provider {
				case domain.ProviderClaudeCode:
					ev.Result.ContextWindow = ResolveClaudeContextWindow(spec.Model, spec.Flags)
				case domain.ProviderCursor:
					if ev.Result.ContextWindow == 0 {
						ev.Result.ContextWindow = ResolveCursorContextWindow(spec.Model, spec.Flags)
					}
				}
			}
			// Same DurationMS > 0 gate as session.turn_end. Record it on the
			// wrapper, not ev.Result: Claude copies the parsed Result before
			// emit, and Cursor returns a freshly built Result, so mutating the
			// event pointer never reaches the value Run returns.
			if ev.Kind == KindResult && ev.Result != nil && ev.Result.DurationMS > 0 {
				turnEndEmitted = true
			}
			if m != nil {
				m.Event(ev)
			}
			if bc != nil {
				bc.emit(ev)
			}
			if inner != nil {
				inner(ev)
			}
		}
	}
	res, err := r.inner.Run(ctx, spec, emit)
	if turnEndEmitted {
		res.TurnEndEmitted = true
	}
	return res, err
}

// sessionBroadcast streams one turn's parsed events onto the bus, tagged with the
// workspace + agent code, so the frontend can render ANY agent's session live.
// One publisher for every turn — chat and loop specialists alike.
//
// Text/thinking deltas are published as CUMULATIVE snapshots with a monotonic
// seq (not raw chunks). Wails v3 (beta.9+) delivers frontend events in emit
// order; seq still lets the UI drop a stale last-seen snapshot. Seq is
// process-wide per workspace+agent+kind (seeded from Engine.streamSeq) so a
// later turn never restarts at 1 and gets dropped by the UI's last-seen gate.
// Final KindText / KindThinking after deltas publish replace+done so the live
// row settles. Redacted thinking (empty text + signature) streams as token estimates.
type nestedBuf struct {
	gotDelta    bool
	gotThinking bool
	textBuf     string
	thinkBuf    string
	textSeq     int
	thinkSeq    int
}

type sessionBroadcast struct {
	e     *Engine
	wsID  string
	code  string
	owner string

	gotDelta    bool
	gotThinking bool
	textBuf     string
	thinkBuf    string
	textSeq     int
	thinkSeq    int
	// stepName is ToolUseID → display name from KindStep. Claude Code's
	// KindToolResult has an empty Tool; Cursor's is the literal "mcp".
	// chatToolResult needs the real name to keep list_findings parseable.
	stepName map[string]string
	// nested streams keyed by the parent Agent/Task tool-use id so subagent
	// deltas never share the parent's cumulative text/thinking buffers.
	nested  map[string]*nestedBuf
	surface string // "app" for the coordinator; empty for workspace chats
}

func (e *Engine) newSessionBroadcast(spec *AgentSpec) *sessionBroadcast {
	if spec.Route.Kind != "" {
		code := spec.Route.Code
		if code == "" {
			code, _ = e.agentIdentity(spec)
		}
		surface := ""
		if spec.Route.WorkspaceID == "" {
			surface = "app"
		}
		textSeq, thinkSeq := e.streamSeqSeed(spec.Route.WorkspaceID, code)
		return &sessionBroadcast{
			e: e, wsID: spec.Route.WorkspaceID, code: code, surface: surface,
			textSeq: textSeq, thinkSeq: thinkSeq,
		}
	}
	if spec.WorkspaceID == "" || spec.Role == "" {
		return nil
	}
	code, _ := e.agentIdentity(spec)
	textSeq, thinkSeq := e.streamSeqSeed(spec.WorkspaceID, code)
	return &sessionBroadcast{
		e: e, wsID: spec.WorkspaceID, code: code, owner: spec.ChatCode,
		textSeq: textSeq, thinkSeq: thinkSeq,
	}
}

func streamSeqKey(wsID, code, kind string) string {
	return wsID + "\x00" + code + "\x00" + kind
}

func (e *Engine) streamSeqSeed(wsID, code string) (text, think int) {
	if e == nil {
		return 0, 0
	}
	e.streamSeqMu.Lock()
	defer e.streamSeqMu.Unlock()
	if e.streamSeq == nil {
		return 0, 0
	}
	return e.streamSeq[streamSeqKey(wsID, code, "text")],
		e.streamSeq[streamSeqKey(wsID, code, "thinking")]
}

func (e *Engine) rememberStreamSeq(wsID, code, kind string, seq int) {
	if e == nil || seq <= 0 {
		return
	}
	e.streamSeqMu.Lock()
	defer e.streamSeqMu.Unlock()
	if e.streamSeq == nil {
		e.streamSeq = map[string]int{}
	}
	key := streamSeqKey(wsID, code, kind)
	if seq > e.streamSeq[key] {
		e.streamSeq[key] = seq
	}
}

func (b *sessionBroadcast) nextTextSeq() int {
	b.textSeq++
	if b.e != nil {
		b.e.rememberStreamSeq(b.wsID, b.code, "text", b.textSeq)
	}
	return b.textSeq
}

func (b *sessionBroadcast) nextThinkSeq() int {
	b.thinkSeq++
	if b.e != nil {
		b.e.rememberStreamSeq(b.wsID, b.code, "thinking", b.thinkSeq)
	}
	return b.thinkSeq
}

func (b *sessionBroadcast) publish(name string, data map[string]any) {
	if b.surface != "" {
		data["surface"] = b.surface
	}
	if b.wsID != "" {
		data["workspaceId"] = b.wsID
	}
	data["code"] = b.code
	if b.owner != "" {
		data["ownerCode"] = b.owner
	}
	b.e.Bus.Publish(Event{Name: name, Data: data})
}

func (b *sessionBroadcast) nest(parent string) *nestedBuf {
	if b.nested == nil {
		b.nested = map[string]*nestedBuf{}
	}
	n := b.nested[parent]
	if n == nil {
		n = &nestedBuf{}
		b.nested[parent] = n
	}
	return n
}

func (b *sessionBroadcast) emit(ev ParsedEvent) {
	if ev.nested() {
		b.emitNested(ev)
		return
	}
	switch ev.Kind {
	case KindTextDelta:
		// Stream-json line uuids are per-event, not per-frame — never reset the
		// cumulative buffer on uuid change. KindFallback / KindStep are the
		// real block boundaries.
		b.gotDelta = true
		b.textBuf += ev.Text
		b.publish("session.delta", map[string]any{
			"text": b.textBuf, "seq": b.nextTextSeq(), "replace": true,
		})
	case KindText:
		if strings.TrimSpace(ev.Text) == "" {
			return
		}
		if b.gotDelta {
			b.publish("session.delta", withUUID(map[string]any{
				"text": ev.Text, "seq": b.nextTextSeq(), "replace": true, "done": true,
			}, ev.UUID))
			b.textBuf = ""
			b.gotDelta = false
			return
		}
		b.publish("session.delta", withUUID(map[string]any{"text": ev.Text, "done": true}, ev.UUID))
	case KindThinkingDelta:
		b.gotThinking = true
		b.thinkBuf += ev.Text
		b.publish("session.thinking", map[string]any{
			"text": b.thinkBuf, "seq": b.nextThinkSeq(), "replace": true,
		})
	case KindThinkingTokens:
		b.publish("session.thinking", map[string]any{"tokens": ev.Tokens})
	case KindThinking:
		if strings.TrimSpace(ev.Text) == "" {
			return
		}
		if b.gotThinking {
			b.publish("session.thinking", withUUID(map[string]any{
				"text": ev.Text, "seq": b.nextThinkSeq(), "replace": true, "done": true,
			}, ev.UUID))
			b.thinkBuf = ""
			b.gotThinking = false
			return
		}
		b.publish("session.thinking", withUUID(map[string]any{"text": ev.Text, "done": true}, ev.UUID))
	case KindStep:
		// A tool call ends the current text/thinking block; clear stream state so
		// the next reply starts a fresh cumulative buffer.
		b.textBuf, b.thinkBuf = "", ""
		b.gotDelta, b.gotThinking = false, false
		// The live row shows the COMMON vocabulary; the raw provider call is what
		// gets persisted (see the mirror's KindStep below).
		d := displayToolFor(ev)
		if ev.ToolUseID != "" {
			if b.stepName == nil {
				b.stepName = map[string]string{}
			}
			b.stepName[ev.ToolUseID] = d.Name
		}
		b.publish("session.step", withUUID(map[string]any{
			"id": ev.ToolUseID, "tool": d.Name, "toolInput": ev.ToolInput,
			"description": toolDescription(d.Name, d.Input),
			"input":       chatToolInput(d.Name, d.Input),
			"parentId":    ev.ParentToolUseID,
		}, ev.UUID))
	case KindToolResult:
		d := displayTool(ev.Provider, ev.Tool, ev.ToolInputRaw)
		name := d.Name
		if !isListFindingsTool(name) {
			if n := b.stepName[ev.ToolUseID]; n != "" {
				name = n
			}
		}
		payload := map[string]any{
			"id":      ev.ToolUseID,
			"result":  chatToolResult(name, displayToolResult(ev.Provider, ev.Tool, ev.Text)),
			"isError": ev.IsError,
		}
		// Some providers (Codex MCP) only attach full args on completion. Forward
		// them so the live row can fill in names (e.g. activate_skill) instead of
		// staying stuck on the empty start envelope.
		if len(ev.ToolInputRaw) > 0 {
			if in := chatToolInput(d.Name, d.Input); in != "" && in != "{}" {
				payload["input"] = in
				payload["toolInput"] = summarizeDisplayInput(ev.Provider, ev.Tool, ev.ToolInputRaw)
			}
		}
		b.publish("session.tool_result", payload)
	case KindCompact:
		payload := map[string]any{
			"status":  ev.CompactStatus,
			"trigger": ev.CompactTrigger,
		}
		if ev.Text != "" {
			payload["summary"] = ev.Text
		}
		if ev.Tokens > 0 {
			payload["preTokens"] = ev.Tokens
		}
		b.publish("session.compact", payload)
	case KindUser:
		show := strings.TrimSpace(ev.DisplayText)
		if show == "" {
			show = ev.Text
		}
		if strings.TrimSpace(show) == "" {
			return
		}
		b.textBuf, b.thinkBuf = "", ""
		b.gotDelta, b.gotThinking = false, false
		b.publish("session.user", map[string]any{"text": show})
	case KindFallback:
		from, to := ev.FromModel, ev.ToModel
		if to == "" && len(ev.RetractedUUIDs) == 0 {
			return
		}
		b.textBuf, b.thinkBuf = "", ""
		b.gotDelta, b.gotThinking = false, false
		payload := map[string]any{
			"from":      from,
			"to":        to,
			"fromLabel": claudeModelLabel(from),
			"toLabel":   claudeModelLabel(to),
		}
		if len(ev.RetractedUUIDs) > 0 {
			payload["retracted"] = ev.RetractedUUIDs
		}
		b.publish("session.fallback", payload)
	case KindUsage:
		// Mid-turn context fill (Cursor streams usage frames). Live only —
		// the session mirror ignores this; KindResult stamps the durable copy.
		b.publishContextUsage(ev.Result)
	case KindResult:
		// Close the live turn with the PROVIDER's own wall time, so the footer
		// shown while streaming matches the one after a reload. A UI-side clock
		// would drift from it by the round trip. Also ship final context usage
		// so the composer meter reconciles without waiting for a transcript resync.
		if ev.Result != nil && ev.Result.DurationMS > 0 {
			payload := map[string]any{"durationMs": ev.Result.DurationMS}
			mergeContextUsage(payload, ev.Result)
			b.publish("session.turn_end", payload)
		} else {
			b.publishContextUsage(ev.Result)
		}
	case KindNotice:
		// Recur status (drain, retry) — not assistant text. Chat listens on
		// session.notice; folding into session.delta would pollute the reply.
		if strings.TrimSpace(ev.Text) == "" {
			return
		}
		b.publish("session.notice", map[string]any{"text": ev.Text})
	}
}

// emitNested publishes nested subagent events tagged with parentId so the UI
// can attach them inside the Agent/Task card instead of the main transcript.
// Nested KindResult is ignored — it must not close the parent turn.
func (b *sessionBroadcast) emitNested(ev ParsedEvent) {
	parent := ev.ParentToolUseID
	switch ev.Kind {
	case KindTextDelta:
		n := b.nest(parent)
		n.gotDelta = true
		n.textBuf += ev.Text
		n.textSeq++
		b.publish("session.delta", map[string]any{
			"text": n.textBuf, "seq": n.textSeq, "replace": true, "parentId": parent,
		})
	case KindText:
		if strings.TrimSpace(ev.Text) == "" {
			return
		}
		n := b.nest(parent)
		if n.gotDelta {
			n.textSeq++
			b.publish("session.delta", map[string]any{
				"text": ev.Text, "seq": n.textSeq, "replace": true, "done": true, "parentId": parent,
			})
			n.textBuf = ""
			n.gotDelta = false
			return
		}
		b.publish("session.delta", map[string]any{"text": ev.Text, "done": true, "parentId": parent})
	case KindThinkingDelta:
		n := b.nest(parent)
		n.gotThinking = true
		n.thinkBuf += ev.Text
		n.thinkSeq++
		b.publish("session.thinking", map[string]any{
			"text": n.thinkBuf, "seq": n.thinkSeq, "replace": true, "parentId": parent,
		})
	case KindThinkingTokens:
		b.publish("session.thinking", map[string]any{"tokens": ev.Tokens, "parentId": parent})
	case KindThinking:
		if strings.TrimSpace(ev.Text) == "" {
			return
		}
		n := b.nest(parent)
		if n.gotThinking {
			n.thinkSeq++
			b.publish("session.thinking", map[string]any{
				"text": ev.Text, "seq": n.thinkSeq, "replace": true, "done": true, "parentId": parent,
			})
			n.thinkBuf = ""
			n.gotThinking = false
			return
		}
		b.publish("session.thinking", map[string]any{"text": ev.Text, "done": true, "parentId": parent})
	case KindStep:
		// Same reset as top-level emit: a nested tool call ends the current
		// text/thinking block so the next nested deltas start a fresh buffer.
		n := b.nest(parent)
		n.textBuf, n.thinkBuf = "", ""
		n.gotDelta, n.gotThinking = false, false
		d := displayToolFor(ev)
		if ev.ToolUseID != "" {
			if b.stepName == nil {
				b.stepName = map[string]string{}
			}
			b.stepName[ev.ToolUseID] = d.Name
		}
		b.publish("session.step", map[string]any{
			"id": ev.ToolUseID, "tool": d.Name, "toolInput": ev.ToolInput,
			"description": toolDescription(d.Name, d.Input),
			"input":       chatToolInput(d.Name, d.Input),
			"parentId":    parent,
		})
	case KindToolResult:
		d := displayTool(ev.Provider, ev.Tool, ev.ToolInputRaw)
		name := d.Name
		if !isListFindingsTool(name) {
			if n := b.stepName[ev.ToolUseID]; n != "" {
				name = n
			}
		}
		payload := map[string]any{
			"id":      ev.ToolUseID,
			"result":  chatToolResult(name, displayToolResult(ev.Provider, ev.Tool, ev.Text)),
			"isError": ev.IsError,
		}
		if len(ev.ToolInputRaw) > 0 {
			if in := chatToolInput(d.Name, d.Input); in != "" && in != "{}" {
				payload["input"] = in
				payload["toolInput"] = summarizeDisplayInput(ev.Provider, ev.Tool, ev.ToolInputRaw)
			}
		}
		b.publish("session.tool_result", payload)
	}
}

func (b *sessionBroadcast) publishContextUsage(r *Result) {
	if b == nil || r == nil || contextUsedTokens(r) <= 0 {
		return
	}
	payload := map[string]any{}
	mergeContextUsage(payload, r)
	b.publish("session.context_usage", payload)
}

func mergeContextUsage(payload map[string]any, r *Result) {
	if r == nil || contextUsedTokens(r) <= 0 {
		return
	}
	payload["inputTokens"] = r.InputTokens
	payload["cacheCreationTokens"] = r.CacheCreationTokens
	payload["cacheReadTokens"] = r.CacheReadTokens
	payload["contextWindow"] = r.ContextWindow
}

func withUUID(payload map[string]any, id string) map[string]any {
	if id != "" {
		payload["uuid"] = id
	}
	return payload
}

// ensureAgentSession returns the durable session for a (loop, agent code),
// creating it on first use as a provider-mode session and reusing it across
// this agent's turns so the whole conversation accretes into one transcript.
// A later turn on a different provider overwrites session.Provider so the
// row matches who actually ran (or is selected). kind is the role (session
// vocabulary); code+name are the feed identity stored in config_json. Keying
// on code — not role — keeps the normal and security Reviewer (both
// role=reviewer) in separate transcripts.
// ok is false only when the store has no database (the legacy in-memory path).
func (e *Engine) ensureAgentSession(workspaceID, kind, code, name, provider string) (domain.Session, bool) {
	return e.ensureOwnedAgentSession(workspaceID, kind, code, name, provider, "")
}

func (e *Engine) ensureOwnedAgentSession(workspaceID, kind, code, name, provider, owner string) (domain.Session, bool) {
	owner = strings.TrimSpace(owner)
	e.sessionEnsureMu.Lock()
	defer e.sessionEnsureMu.Unlock()
	if isSpecialistChatCode(code) {
		if sess, ok := e.Store.SessionForChatAgent(workspaceID, owner, code); ok {
			if provider != "" && sess.Provider != provider {
				_ = e.Store.UpdateSessionProvider(sess.ID, provider)
				sess.Provider = provider
			}
			return sess, true
		}
		cfg, _ := json.Marshal(agentConfig{Code: code, Name: name, OwnerCode: owner})
		sess, err := e.Store.CreateSession(domain.Session{
			WorkspaceID: workspaceID,
			Kind:        kind,
			ContextMode: domain.ContextModeProvider,
			Provider:    provider,
			ConfigJSON:  string(cfg),
		})
		if err != nil {
			return domain.Session{}, false
		}
		return sess, true
	}
	if sess, ok := e.Store.SessionForLoopAgent(workspaceID, code); ok {
		if provider != "" && sess.Provider != provider {
			_ = e.Store.UpdateSessionProvider(sess.ID, provider)
			sess.Provider = provider
		}
		return sess, true
	}
	cfg, _ := json.Marshal(agentConfig{Code: code, Name: name})
	mode := domain.ContextModeProvider
	if provider != "" && !domain.BuiltinProvider(provider) {
		mode = domain.ContextModeApp
	}
	sess, err := e.Store.CreateSession(domain.Session{
		WorkspaceID: workspaceID,
		Kind:        kind,
		ContextMode: mode,
		Provider:    provider,
		ConfigJSON:  string(cfg),
	})
	if err != nil {
		return domain.Session{}, false
	}
	return sess, true
}

// specProvider is the provider that ran (or will run) a turn, defaulting to
// Claude Code for specs built outside runAgent (tests, direct runner calls).
func specProvider(spec *AgentSpec) string {
	if spec != nil && spec.Provider != "" {
		return spec.Provider
	}
	return domain.ProviderClaudeCode
}

// attachMirror ensures a durable session for the agent this spec drives, attaches
// a per-turn mirror that records the prompt and mirrors the stream, and returns a
// finish func to defer. Additive and defensive: it is a no-op (returning a no-op
// finish) when there's no loop/role, when a mirror is already attached, or when the
// store has no database.
func (e *Engine) attachMirror(spec *AgentSpec) func() {
	if spec == nil || spec.Mirror != nil {
		return func() {}
	}
	var sess domain.Session
	var ok bool
	if spec.Route.Kind != "" {
		sess, ok = e.ensureRoutedSession(spec)
	} else if spec.WorkspaceID != "" && spec.Role != "" {
		code, name := e.agentIdentity(spec)
		sess, ok = e.ensureOwnedAgentSession(spec.WorkspaceID, string(spec.Role), code, name, specProvider(spec), spec.ChatCode)
	} else {
		return func() {}
	}
	if !ok {
		return func() {}
	}
	m := newSessionMirror(e.Store, sess.ID, spec.Model, specProvider(spec))
	spec.RecurSessionID = sess.ID
	if spec.HistoryAfterID == "" {
		spec.HistoryAfterID = nativeHistoryAfterID(sess.ConfigJSON)
	}
	// Always persist Prompt as wire text; optional DisplayText is UI-only.
	// Retry and switch_mode remounts skip the user row: Retry would duplicate
	// the last bubble, and a remount should look like the same run.
	if !spec.SkipRecordUser {
		if id := m.recordUser(spec.UserMessageID, spec.Prompt, spec.DisplayText, spec.Images); id != "" {
			spec.UserMessageID = id
		}
	} else {
		spec.UserMessageID = resolveSessionUserMessageID(e.Store, sess.ID, spec.UserMessageID)
	}
	spec.Mirror = m
	return m.finish
}

func (e *Engine) ensureRoutedSession(spec *AgentSpec) (domain.Session, bool) {
	code := spec.Route.Code
	name := spec.Route.Name
	if code == "" {
		code, name = e.agentIdentity(spec)
	}
	if spec.Route.WorkspaceID == "" {
		// App sessions are per tab code (Ask Recur strip) — route to the tab's
		// own session, never "the newest app session".
		if spec.Route.Kind == domain.SessionKindApp {
			if sess, ok := e.appSessionByCode(code); ok {
				if p := specProvider(spec); p != "" && sess.Provider != p {
					_ = e.Store.UpdateSessionProvider(sess.ID, p)
					sess.Provider = p
				}
				return sess, true
			}
		} else if sess, ok := e.Store.SessionForAppKind(spec.Route.Kind); ok {
			if p := specProvider(spec); p != "" && sess.Provider != p {
				_ = e.Store.UpdateSessionProvider(sess.ID, p)
				sess.Provider = p
			}
			return sess, true
		}
		cfg, _ := json.Marshal(agentConfig{Code: code, Name: name, Mode: domain.ChatModeAsk})
		sess, err := e.Store.CreateSession(domain.Session{
			Kind:        spec.Route.Kind,
			ContextMode: domain.ContextModeProvider,
			Provider:    specProvider(spec),
			ConfigJSON:  string(cfg),
		})
		if err != nil {
			return domain.Session{}, false
		}
		return sess, true
	}
	return e.ensureAgentSession(spec.Route.WorkspaceID, spec.Route.Kind, code, name, specProvider(spec))
}

// agentIdentity resolves the avatar code + display name for the agent a spec
// drives. The code is the AgentID suffix (AgentID is workspaceID+"-"+code); the name
// comes from the persisted Agent record (so the security Reviewer reads
// "Security Reviewer", not the plain role name), falling back to the role table.
func (e *Engine) agentIdentity(spec *AgentSpec) (code, name string) {
	code = strings.TrimPrefix(spec.AgentID, spec.WorkspaceID+"-")
	if code == "" || code == spec.AgentID {
		code = string(spec.Role)
	}
	if a, ok := e.Store.Agent(spec.WorkspaceID, code); ok {
		name = a.Name
	}
	if name == "" {
		if rs, ok := roles[spec.Role]; ok {
			name = rs.name
		}
	}
	return code, name
}

// sessionMirror is the production TurnMirror: it projects a Claude turn's stream
// onto the normalized session store while the live feed is
// unaffected. It is created per turn, bound to one provider-mode session, and used
// synchronously from the runner's read loop — so its bookkeeping needs no locks.
//
// Message boundaries follow Anthropic's own conversation shape: assistant
// thinking, text, and tool_use blocks accrue into an assistant message;
// tool_result blocks (which arrive on "user" stream lines) accrue into a
// following user message. This normalized transcript is the single source of
// truth; a block type the projection doesn't model yet round-trips verbatim via
// domain.RawBlock, so nothing is lost and there is no parallel raw-line store.
type sessionMirror struct {
	store    *store.Store
	session  string
	model    string
	provider string

	curMsg  string // id of the currently open message ("" when none)
	curRole string // role of the currently open message

	// toolIDs maps a provider tool-use id (toolu_…) to the internal uuid we minted
	// for it, so a later tool_result references the internal id (the dual-ID trick
	// that makes cross-provider replay tractable).
	toolIDs map[string]string

	// thinkTokens is the running estimate for the CURRENT (redacted) thinking
	// block, keyed by ParentToolUseID ("" = parent turn) so a nested Task's
	// estimate is not stamped onto the parent's next thinking block.
	// thinkTokenUUID is the provider frame that last raised the parent-turn
	// estimate — token-only progress never creates a message, so retraction
	// must key off this.
	thinkTokens    map[string]int64
	thinkTokenUUID string

	// lastFallbackFrom/To skip a duplicate KindFallback already persisted this
	// turn: stream_event start plus the complete assistant block, sticky
	// usage.iterations after a tool hop, or a later model_refusal_fallback
	// audit for the same serving model. An incomplete first hop (empty from)
	// is enriched in place rather than skipped or appended after the answer.
	lastFallbackFrom string
	lastFallbackTo   string
	lastFallbackMsg  string

	// uuidMsgs maps a Claude Code frame uuid to Recur message ids written while
	// that frame was the current provider line (refusal fallback retraction).
	uuidMsgs map[string][]string
}

// newSessionMirror builds a mirror for a provider-mode session.
func newSessionMirror(st *store.Store, sessionID, model, provider string) *sessionMirror {
	return &sessionMirror{
		store: st, session: sessionID, model: model, provider: provider,
		toolIDs: map[string]string{},
	}
}

// recordUser appends the human's prompt as a completed user message before the
// turn streams. text is always the full wire (provider replay); displayText, when
// set, is stored as display_text for the chat bubble. Images (chat paste) become
// interleaved content blocks at their marker positions. No-op when there is
// neither text nor images.
func (m *sessionMirror) recordUser(id, text, displayText string, imgs []UserImage) string {
	if m == nil {
		return ""
	}
	blocks := userContentBlocks(text, displayText, imgs)
	if len(blocks) == 0 {
		return ""
	}
	got, err := m.store.AppendMessage(m.session, domain.Message{
		ID:      id,
		Role:    "user",
		Content: blocks,
	})
	if err != nil {
		return strings.TrimSpace(id)
	}
	return got.ID
}

// resolveSessionUserMessageID returns want if it is a completed chat-prompt
// row in the session; otherwise the latest such row. Retry mints a fresh
// UserMessageID for checkpointing and never writes it back — harness history
// still needs the recorded row so spec.Prompt is not appended twice.
func resolveSessionUserMessageID(st *store.Store, sessionID, want string) string {
	if st == nil || sessionID == "" {
		return want
	}
	msgs, err := st.SessionTranscript(sessionID)
	if err != nil {
		return want
	}
	last := ""
	for _, m := range msgs {
		if m.Role != "user" || !userMessageIsChatPrompt(m) {
			continue
		}
		if m.StreamStatus == domain.StreamPending || m.StreamStatus == domain.StreamStreaming {
			continue
		}
		last = m.ID
		if want != "" && m.ID == want {
			return want
		}
	}
	if last != "" {
		return last
	}
	return want
}

// RequiresDisplayText reports whether prompt carries host wrap markers that must
// not appear alone in the chat bubble — callers must set DisplayText whenever
// this is true (attached-files / skill-activation / fork-transcript wire).
func RequiresDisplayText(prompt string) bool {
	if strings.Contains(prompt, "<skill_content") {
		return true
	}
	if strings.Contains(prompt, "The user has attached these files") {
		return true
	}
	if strings.Contains(prompt, "The user activated skill(s) via /name") {
		return true
	}
	if strings.Contains(prompt, forkPreambleLead) {
		return true
	}
	return false
}

// userBubbleText is the chat-bubble body for a turn's user row: DisplayText
// when set, else a known Coder dispatch label, else the wire Prompt.
func userBubbleText(prompt, display string) string {
	if d := strings.TrimSpace(display); d != "" {
		return d
	}
	if label := coderDispatchDisplay(prompt); label != "" {
		return label
	}
	return strings.TrimSpace(prompt)
}

// Event projects one parsed event onto normalized message/content rows.
func (m *sessionMirror) Event(ev ParsedEvent) {
	if m == nil {
		return
	}
	switch ev.Kind {
	case KindInit:
		// The provider's session id rotates every turn; keep the resume ref current.
		if ev.SessionID != "" {
			_ = m.store.UpdateSessionRef(m.session, ev.SessionID)
		}
	case KindText:
		m.ensure("assistant")
		m.noteProviderUUID(ev.UUID)
		_ = m.store.AppendContentBlock(m.curMsg, domain.TextBlock(ev.Text).WithParent(ev.ParentToolUseID))
	case KindThinkingTokens:
		if m.thinkTokens == nil {
			m.thinkTokens = map[string]int64{}
		}
		if id := strings.TrimSpace(ev.UUID); id != "" && ev.ParentToolUseID == "" && id != m.thinkTokenUUID {
			m.thinkTokens[""] = 0
			m.thinkTokenUUID = id
		}
		if ev.Tokens > m.thinkTokens[ev.ParentToolUseID] {
			m.thinkTokens[ev.ParentToolUseID] = ev.Tokens
		}
	case KindThinking:
		// Extended-thinking block — persisted as a normalized thinking block so the
		// session store is lossless and any view can render (or ignore) reasoning.
		// Headless Claude Code REDACTS the text (empty + signature); the running
		// token estimate is stamped on so "Thought ~N tokens" survives a reload.
		tok := m.thinkTokens[ev.ParentToolUseID]
		delete(m.thinkTokens, ev.ParentToolUseID)
		m.ensure("assistant")
		m.noteProviderUUID(ev.UUID)
		_ = m.store.AppendContentBlock(m.curMsg, domain.ThinkingBlock(ev.Text, ev.Signature, tok).WithParent(ev.ParentToolUseID))
		if ev.ParentToolUseID == "" {
			m.thinkTokenUUID = ""
		}
	case KindStep:
		// Skip a provider id we've already persisted this turn. Re-emitted
		// tool_use blocks would otherwise mint orphan rows (result attaches only
		// to the last clone via toolIDs). Blank ids cannot be correlated, so they
		// are never deduped — better a duplicate row than a swallowed call.
		if ev.ToolUseID != "" {
			if _, ok := m.toolIDs[ev.ToolUseID]; ok {
				return
			}
		}
		m.ensure("assistant")
		m.noteProviderUUID(ev.UUID)
		internal := uuid.NewString()
		if ev.ToolUseID != "" {
			m.toolIDs[ev.ToolUseID] = internal
		}
		// Stored VERBATIM: the provider's own tool name and argument JSON. The
		// common-vocabulary rendering is derived at read time (toolview.go), so a
		// mapping change re-renders old transcripts instead of leaving them stale.
		_ = m.store.AppendContentBlock(m.curMsg,
			domain.ToolUseBlockNested(internal, ev.ToolUseID, ev.ParentToolUseID, ev.Tool, ev.ToolInputRaw))
	case KindToolResult:
		m.ensure("user")
		m.noteProviderUUID(ev.UUID)
		internal := m.toolIDs[ev.ToolUseID]
		if internal == "" {
			internal = ev.ToolUseID // no matching call seen; keep the provider id
		}
		// Providers like Codex MCP may only attach full args on completion —
		// patch the earlier tool_use block so reloads still show the question/name.
		if len(ev.ToolInputRaw) > 0 && string(ev.ToolInputRaw) != "{}" && internal != "" {
			_ = m.store.PatchToolUseInput(internal, ev.ToolInputRaw)
		}
		_ = m.store.AppendContentBlock(m.curMsg,
			domain.TruncatedToolResultBlock(internal, ev.Text, ev.IsError, ev.Truncated))
	case KindCompact:
		// Persist the completed compact marker so reload shows "Context compacted".
		// started is live-only (the UI shows Compacting… from the bus event).
		if ev.CompactStatus != domain.CompactDone {
			return
		}
		m.ensure("assistant")
		m.noteProviderUUID(ev.UUID)
		_ = m.store.AppendContentBlock(m.curMsg, domain.CompactBlock(
			ev.CompactStatus, ev.CompactTrigger, ev.Text, ev.Tokens).WithParent(ev.ParentToolUseID))
		// session_context is the provider-neutral compaction pointer.
		// CompactBlock is only the inline UI chip.
		if strings.TrimSpace(ev.Text) != "" && m.curMsg != "" {
			_, _ = m.store.WriteSessionContext(m.session, ev.Text, m.curMsg)
		}
	case KindUser:
		// Cursor D49 follow-up: a new user message, not a tool_result carrier.
		// closeCurrent first — finishSteerCancel may have left a user message
		// open for synthetic tool results.
		if strings.TrimSpace(ev.Text) == "" {
			return
		}
		m.closeCurrent(domain.Message{})
		// Cursor follow-up Send starts a new run that may reuse call ids.
		// A live delivered steer (and Claude/Codex KindUser) keeps outstanding
		// tools running — clearing here would persist the result under the
		// provider id and drop it from WorkspaceChatItems on reload.
		if m.provider == domain.ProviderCursor && !ev.LiveSteer {
			m.toolIDs = map[string]string{}
		}
		m.ensure("user")
		_ = m.store.AppendContentBlock(m.curMsg, domain.TextBlockDisplay(ev.Text, ev.DisplayText))
	case KindFallback:
		if len(ev.RetractedUUIDs) > 0 {
			m.dropProviderUUIDs(ev.RetractedUUIDs)
		}
		from, to := strings.TrimSpace(ev.FromModel), strings.TrimSpace(ev.ToModel)
		if to == "" {
			return
		}
		if to == m.lastFallbackTo {
			if from != "" && m.lastFallbackFrom == "" && m.lastFallbackMsg != "" {
				_ = m.store.PatchFallbackFrom(m.lastFallbackMsg, from)
				m.lastFallbackFrom = from
			}
			return
		}
		m.lastFallbackFrom, m.lastFallbackTo = from, to
		// Stamp the serving model before opening the row so FallbackBlock and the
		// replacement response are not recorded under the declining model.
		m.model = to
		if m.curMsg != "" && m.curRole == "assistant" {
			m.closeCurrent(domain.Message{})
		}
		m.ensure("assistant")
		m.noteProviderUUID(ev.UUID)
		_ = m.store.AppendContentBlock(m.curMsg, domain.FallbackBlock(from, to))
		m.lastFallbackMsg = m.curMsg
	case KindResult:
		if ev.nested() {
			return
		}
		// Turn done: finalize the open message, stamping duration + token usage.
		var usage domain.Message
		if ev.Result != nil {
			// The turn's wall time, as the PROVIDER measured it. Deriving it from
			// our own message timestamps would time the mirror's bookkeeping
			// rather than the turn.
			usage.DurationMS = int64(ev.Result.DurationMS)
			usage.InputTokens = ev.Result.InputTokens
			usage.OutputTokens = ev.Result.OutputTokens
			usage.CacheCreationTokens = ev.Result.CacheCreationTokens
			usage.CacheReadTokens = ev.Result.CacheReadTokens
			usage.ContextWindow = ev.Result.ContextWindow
		}
		m.closeCurrent(usage)
	}
}

// contextUsedTokens is the prompt-side fill for the context meter: fresh input
// plus cache read/create. Output tokens are not part of the occupied window.
func contextUsedTokens(r *Result) int64 {
	if r == nil {
		return 0
	}
	return r.InputTokens + r.CacheReadTokens + r.CacheCreationTokens
}

// preferContextUsage overlays per-inference token fields onto a terminal result.
// Claude/Cursor result.usage is cumulative billing across the turn and can
// exceed the model window; the composer meter needs the latest footprint
// (assistant.message.usage / mid-stream usage frames). ContextWindow on dst
// is left untouched.
func preferContextUsage(dst *Result, context *Result) {
	if dst == nil || contextUsedTokens(context) <= 0 {
		return
	}
	dst.InputTokens = context.InputTokens
	dst.OutputTokens = context.OutputTokens
	dst.CacheCreationTokens = context.CacheCreationTokens
	dst.CacheReadTokens = context.CacheReadTokens
}

// contextUsageTracker remembers the latest KindUsage footprint so a following
// KindResult can stamp the meter without using cumulative result.usage.
type contextUsageTracker struct {
	last Result
}

func (t *contextUsageTracker) observe(ev *ParsedEvent) {
	if t == nil || ev == nil {
		return
	}
	switch ev.Kind {
	case KindUsage:
		if ev.Result != nil && contextUsedTokens(ev.Result) > 0 {
			t.last = *ev.Result
		}
	case KindResult:
		if ev.Result != nil {
			preferContextUsage(ev.Result, &t.last)
		}
	}
}

// finish closes any still-open message and returns the session to idle. Called by
// the caller after Run so a turn that ended without a result line (an error) still
// leaves no message stuck in "streaming".
func (m *sessionMirror) finish() {
	if m == nil {
		return
	}
	m.closeCurrent(domain.Message{})
	_ = m.store.SetSessionStatus(m.session, "idle")
}

// ensure makes the currently open message one of the given role, closing a
// different-role message first and opening a fresh one as needed.
func (m *sessionMirror) ensure(role string) {
	if m.curMsg != "" && m.curRole == role {
		return
	}
	m.closeCurrent(domain.Message{})
	id, err := m.store.CreateMessage(m.session, domain.Message{
		Role:         role,
		ModelID:      m.assistantModel(role),
		Provider:     m.provider,
		StreamStatus: domain.StreamStreaming,
	})
	if err != nil {
		m.curMsg, m.curRole = "", ""
		return
	}
	m.curMsg, m.curRole = id, role
	_ = m.store.SetSessionStatus(m.session, "streaming")
}

// closeCurrent marks the open message complete (stamping any usage) and clears it.
func (m *sessionMirror) closeCurrent(usage domain.Message) {
	if m.curMsg == "" {
		return
	}
	_ = m.store.CompleteMessage(m.curMsg, domain.StreamComplete, usage)
	m.curMsg, m.curRole = "", ""
}

func (m *sessionMirror) noteProviderUUID(id string) {
	id = strings.TrimSpace(id)
	if id == "" || m.curMsg == "" {
		return
	}
	if m.uuidMsgs == nil {
		m.uuidMsgs = map[string][]string{}
	}
	for _, mid := range m.uuidMsgs[id] {
		if mid == m.curMsg {
			return
		}
	}
	m.uuidMsgs[id] = append(m.uuidMsgs[id], m.curMsg)
}

func (m *sessionMirror) dropProviderUUIDs(uuids []string) {
	if len(uuids) == 0 {
		return
	}
	for _, u := range uuids {
		if strings.TrimSpace(u) == m.thinkTokenUUID {
			if m.thinkTokens != nil {
				delete(m.thinkTokens, "")
			}
			m.thinkTokenUUID = ""
			break
		}
	}
	if m.store == nil {
		return
	}
	seen := map[string]bool{}
	var ids []string
	for _, u := range uuids {
		for _, mid := range m.uuidMsgs[strings.TrimSpace(u)] {
			if mid == "" || seen[mid] {
				continue
			}
			seen[mid] = true
			ids = append(ids, mid)
		}
	}
	if len(ids) == 0 {
		return
	}
	if m.curMsg != "" && seen[m.curMsg] {
		m.curMsg, m.curRole = "", ""
	}
	_ = m.store.DropSessionMessages(m.session, ids)
	for u, mids := range m.uuidMsgs {
		kept := mids[:0]
		for _, mid := range mids {
			if !seen[mid] {
				kept = append(kept, mid)
			}
		}
		if len(kept) == 0 {
			delete(m.uuidMsgs, u)
		} else {
			m.uuidMsgs[u] = kept
		}
	}
}

// assistantModel returns the per-message model stamp — only assistant messages
// carry one (a user turn is the human's, not the model's).
func (m *sessionMirror) assistantModel(role string) string {
	if role == "assistant" {
		return m.model
	}
	return ""
}
