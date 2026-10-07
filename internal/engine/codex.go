package engine

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"os"
	"strings"
	"sync"
	"time"

	"recur/internal/codex"
	"recur/internal/domain"
	"recur/internal/store"
)

// CodexRunner drives OpenAI Codex through `codex app-server` (JSON-RPC over
// stdio). Peer of ClaudeRunner / CursorRunner on the AgentRunner contract.
//
// Mapping:
//
//	AgentSpec.Worktree              → cwd + cmd.Dir
//	AgentSpec.Model                 → thread/turn model id
//	AgentSpec.SystemPrompt          → developerInstructions (thread/start + thread/resume;
//	                                  never also prepended into the user text)
//	AgentSpec.AppendSystemPrompt    → user-text suffix on a fresh thread / remount
//	AgentSpec.TurnAppendPrompt      → user-text suffix every turn (surface / PR)
//	AgentSpec.MCPURL / Playwright   → --config mcp_servers… (no worktree writes)
//	AgentSpec.ResumeSession         → thread/resume id
//	AgentSpec.Inject                → turn/steer (D49)
//	Ask/Plan/Review (no Edit/Write) → sandbox read-only + network (gh / truncated diffs)
//	Settings.CodexNetworkAccess     → workspace-write networkAccess (not full FS)
//	Settings.BypassPermissions      → danger-full-access + approval never;
//	                                  off → on-request + Recur permission prompts
//
// CODEX_HOME is a single Recur-owned directory under the data dir so sessions
// and auth persist without dirtying the loop worktree or sharing ~/.codex
// mutable state. Worktree isolation is via cwd on each thread/turn.
type CodexRunner struct {
	Bin    string
	APIKey string
	// ChatGPTAuth seeds Home from ~/.codex/auth.json when no API key is set.
	ChatGPTAuth bool
	// Home is an explicit CODEX_HOME. Empty → <store.DataDir>/codex-home.
	Home string
	// NetworkAccess opts into sandbox_workspace_write.network_access while
	// keeping workspace-write (not danger-full-access). Ask/Plan/Review get
	// network on the read-only sandbox regardless. See Settings.CodexNetworkAccess.
	NetworkAccess bool
	// AskPermission, when set and the turn is not bypassing or denying, routes
	// Codex app-server approval requests to the human. Nil → auto-accept (probes).
	// PermissionModeDeny auto-denies residual approvals (unattended one-shots).
	AskPermission func(context.Context, domain.PermissionRequest) (domain.PermissionDecision, error)
}

// NeedsPermissionPromptTool reports that Codex does not use Recur's
// permission_request MCP tool. Approvals arrive as app-server JSON-RPC
// (item/commandExecution/requestApproval, item/permissions/requestApproval, …)
// and are routed through AskPermission when BypassPermissions is off.
func (*CodexRunner) NeedsPermissionPromptTool() bool { return false }

// Run executes one agent turn.
func (r *CodexRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if strings.TrimSpace(spec.Worktree) == "" {
		return Result{}, fmt.Errorf("codex runner: worktree path is required")
	}
	bin := r.Bin
	if bin == "" {
		var err error
		bin, err = codex.Locate("")
		if err != nil {
			return Result{}, err
		}
	}

	home := r.Home
	if home == "" {
		dataDir := os.TempDir()
		if d, err := store.DataDir(); err == nil {
			dataDir = d
		}
		home = codex.HomeDir(dataDir)
	}

	servers := mcpServersFor(spec)
	overrides := codex.ConfigOverrides(servers)
	overrides = append(overrides, codexLongContextOverrides(spec.Flags)...)
	overrides = append(overrides, codexNetworkOverrides(r.NetworkAccess)...)
	overrides = append(overrides, `approval_policy="`+codexApprovalPolicy(spec.PermissionMode)+`"`)

	// ChatGPT login takes precedence: a leftover API key must not win while the
	// toggle is on (Settings hides the key field in that mode).
	apiKey := r.APIKey
	if r.ChatGPTAuth {
		apiKey = ""
	}
	srv := &codex.Server{
		Bin:             bin,
		Home:            home,
		Worktree:        spec.Worktree,
		APIKey:          apiKey,
		ConfigOverrides: overrides,
	}
	if apiKey == "" && r.ChatGPTAuth {
		srv.SeedAuthFrom = codex.DefaultSeedAuthPath()
	}

	if err := srv.Start(ctx); err != nil {
		return Result{}, fmt.Errorf("codex start: %w", err)
	}
	defer srv.Close()

	client := codex.NewClient(srv.Conn())
	fileItems := &sync.Map{} // item id → fileChange item (approval payloads omit paths)
	r.installApprovals(ctx, spec, srv.Conn(), fileItems)

	sandbox, turnSandbox := codexSandbox(spec, r.NetworkAccess)
	policy := codexApprovalPolicy(spec.PermissionMode)

	threadParams := codex.ThreadStartParams{
		Model:                 spec.Model,
		CWD:                   spec.Worktree,
		ApprovalPolicy:        policy,
		Sandbox:               sandbox,
		DeveloperInstructions: spec.SystemPrompt,
		ServiceName:           "recur",
	}

	var (
		thread  codex.Thread
		err     error
		resumed bool
	)
	if spec.ResumeSession != "" {
		thread, err = client.ThreadResume(ctx, spec.ResumeSession, threadParams)
		if err != nil {
			log.Printf("recur: codex thread/resume %q failed (%v) — starting a new thread", spec.ResumeSession, err)
			thread, err = client.ThreadStart(ctx, threadParams)
		} else {
			resumed = true
		}
	} else {
		thread, err = client.ThreadStart(ctx, threadParams)
	}
	if err != nil {
		return Result{}, fmt.Errorf("codex thread: %w\n%s", err, srv.StderrTail())
	}

	if emit != nil {
		emit(ParsedEvent{
			Kind:       KindInit,
			Provider:   domain.ProviderCodex,
			SessionID:  thread.ID,
			Model:      spec.Model,
			MCPServers: mcpStatusFrom(servers),
		})
	}

	// Register notifications before turn/start so a fast turn/completed (or
	// early item/started) is not dropped while the handler is still nil.
	prompt := codexUserPrompt(spec, resumed)
	return r.consumeTurn(ctx, client, srv, spec, thread.ID, turnSandbox, prompt, emit, fileItems)
}

// codexUserPrompt is the turn/start user text. SystemPrompt is already
// developerInstructions (including the Maestro plan on loop Coders) — never
// prepend it here, or a 20k-char plan is billed twice. Chat-mode
// AppendSystemPrompt has no Codex flag, so it rides in-band on a fresh thread
// and on mode-switch remounts, not on an ordinary resume.
func codexUserPrompt(spec AgentSpec, resumed bool) string {
	prompt := joinAppendPrompt(spec.Prompt, spec.TurnAppendPrompt)
	if spec.AppendSystemPrompt != "" && standingInUserPrompt(spec, resumed) {
		prompt = joinAppendPrompt(prompt, spec.AppendSystemPrompt)
	}
	return prompt
}

func (r *CodexRunner) consumeTurn(
	ctx context.Context,
	client *codex.Client,
	srv *codex.Server,
	spec AgentSpec,
	threadID string,
	turnSandbox *codex.SandboxPolicy,
	prompt string,
	emit func(ParsedEvent),
	fileItems *sync.Map,
) (Result, error) {
	type turnDone struct {
		status string
		errMsg string
	}
	doneCh := make(chan turnDone, 1)
	var (
		mu          sync.Mutex
		finalText   strings.Builder
		usageLast   codex.TokenUsage // live context meter (single-request footprint)
		usageTurn   codex.TokenUsage // turn tokens = total − baseline
		baseline    codex.TokenUsage
		baselineSet bool
		contextWin  int64
		activeTurn  string
		openTools   = map[string]string{}          // item id → tool label
		mcpArgs     = map[string]json.RawMessage{} // item id → MCP args (start may be the only copy)
		emitMu      sync.Mutex
		emitClosed  bool
	)

	// Serialize every emit (read-loop notifications + final KindResult). A bool
	// gate alone races: OnNotification(nil) does not wait for an in-flight
	// callback that already passed a check-then-act test.
	emitEv := func(ev ParsedEvent) {
		if emit == nil {
			return
		}
		ev.Provider = domain.ProviderCodex
		emitMu.Lock()
		defer emitMu.Unlock()
		if emitClosed {
			return
		}
		emit(ev)
	}
	// detachEmit stops further item events without KindResult so cancel/reap
	// settlement (finalizeChatReply) can own the turn footer.
	detachEmit := func() {
		client.Conn().OnNotification(nil)
		emitMu.Lock()
		emitClosed = true
		emitMu.Unlock()
	}
	finishEmit := func(ev ParsedEvent) {
		client.Conn().OnNotification(nil)
		emitMu.Lock()
		defer emitMu.Unlock()
		emitClosed = true
		if emit != nil {
			ev.Provider = domain.ProviderCodex
			emit(ev)
		}
	}

	client.Conn().OnNotification(func(method string, params json.RawMessage) {
		switch method {
		case "turn/completed":
			var p struct {
				Turn struct {
					ID     string `json:"id"`
					Status string `json:"status"`
					Error  *struct {
						Message string `json:"message"`
					} `json:"error"`
				} `json:"turn"`
				ThreadID string `json:"threadId"`
			}
			_ = json.Unmarshal(params, &p)
			msg := ""
			if p.Turn.Error != nil {
				msg = p.Turn.Error.Message
			}
			select {
			case doneCh <- turnDone{status: p.Turn.Status, errMsg: msg}:
			default:
			}
		case "item/agentMessage/delta":
			var p struct {
				Delta string `json:"delta"`
				Text  string `json:"text"`
			}
			_ = json.Unmarshal(params, &p)
			chunk := p.Delta
			if chunk == "" {
				chunk = p.Text
			}
			if chunk == "" {
				return
			}
			mu.Lock()
			finalText.WriteString(chunk)
			mu.Unlock()
			if spec.IncludePartials {
				emitEv(ParsedEvent{Kind: KindTextDelta, Text: chunk})
			}
		case "item/completed":
			r.handleItemCompleted(params, &mu, &finalText, openTools, mcpArgs, emitEv)
		case "item/started":
			r.handleItemStarted(params, openTools, mcpArgs, fileItems, emitEv)
		case "item/reasoning/summaryTextDelta", "item/reasoning/textDelta":
			// summaryTextDelta = readable summary (GPT-5.x); textDelta = raw
			// reasoning when a model exposes it. Both map to thinking blocks.
			var p struct {
				Delta string `json:"delta"`
			}
			_ = json.Unmarshal(params, &p)
			if p.Delta != "" && spec.IncludePartials {
				emitEv(ParsedEvent{Kind: KindThinkingDelta, Text: p.Delta})
			}
		case "item/reasoning/summaryPartAdded":
			// Boundary between summary sections (skip index 0 — that's the first part).
			var p struct {
				SummaryIndex int `json:"summaryIndex"`
			}
			_ = json.Unmarshal(params, &p)
			if spec.IncludePartials && p.SummaryIndex > 0 {
				emitEv(ParsedEvent{Kind: KindThinkingDelta, Text: "\n\n"})
			}
		case "thread/tokenUsage/updated":
			var st codexUsageState
			st.observe(params)
			mu.Lock()
			if !tokenUsageEmpty(st.last) {
				usageLast = st.last
			}
			if st.window > 0 {
				contextWin = st.window
			}
			// Token usage uses the delta of thread-cumulative `total` so multi-step
			// turns don't under-report by only counting the final request.
			if !baselineSet && !tokenUsageEmpty(st.total) {
				baseline = subTokenUsage(st.total, st.last)
				baselineSet = true
			}
			if baselineSet {
				usageTurn = subTokenUsage(st.total, baseline)
			} else {
				usageTurn = st.last
			}
			last := usageLast
			win := contextWin
			mu.Unlock()
			var usage Result
			applyCodexMeterUsage(&usage, last, win)
			emitEv(ParsedEvent{Kind: KindUsage, Result: &usage})
		}
	})

	// Codex turn/completed has no duration field. Stamp wall time so KindResult
	// can publish session.turn_end and flattenTranscript can emit the footer
	// that hosts the findings digest. Floor at 1ms — DurationMS==0 skips both.
	started := time.Now()
	turn, err := client.TurnStart(ctx, codex.TurnStartParams{
		ThreadID:       threadID,
		Model:          spec.Model,
		CWD:            spec.Worktree,
		ApprovalPolicy: codexApprovalPolicy(spec.PermissionMode),
		SandboxPolicy:  turnSandbox,
		Effort:         spec.Flags["effort"],
		ServiceTier:    codexServiceTierFromFlags(spec.Flags),
		Input:          []codex.UserInput{{Type: "text", Text: prompt}},
	})
	if err != nil {
		client.Conn().OnNotification(nil)
		return Result{}, fmt.Errorf("codex turn/start: %w\n%s", err, srv.StderrTail())
	}
	mu.Lock()
	activeTurn = turn.ID
	mu.Unlock()

	// Mid-turn steers (D49). turnCtx is cancelled when the turn completes so
	// the steer loop exits without stealing from doneCh.
	turnCtx, endTurn := context.WithCancel(ctx)
	defer endTurn()

	steerDone := make(chan struct{})
	go func() {
		defer close(steerDone)
		if spec.Inject == nil {
			return
		}
		for {
			select {
			case <-turnCtx.Done():
				return
			case msg, ok := <-spec.Inject:
				if !ok {
					return
				}
				msg = strings.TrimSpace(msg)
				if msg == "" {
					ackInject(spec, false)
					continue
				}
				text := msg
				if spec.InjectPlain {
					text = injectStdinText(spec, msg)
				}
				if text == "" {
					ackInject(spec, false)
					continue
				}
				sctx, cancel := context.WithTimeout(turnCtx, 30*time.Second)
				_, err := client.TurnSteer(sctx, codex.TurnSteerParams{
					ThreadID: threadID,
					Input:    []codex.UserInput{{Type: "text", Text: text}},
				})
				cancel()
				if err != nil {
					ackInject(spec, false)
					continue
				}
				emitInjectUser(spec, emitEv, msg)
				ackInject(spec, true)
			}
		}
	}()

	// Cancel → interrupt.
	stopInterrupt := context.AfterFunc(ctx, func() {
		cctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		mu.Lock()
		tid := activeTurn
		mu.Unlock()
		_ = client.TurnInterrupt(cctx, threadID, tid)
	})
	defer stopInterrupt()

	var done turnDone
	select {
	case done = <-doneCh:
		endTurn()
	case <-srv.Reaped():
		endTurn()
		<-steerDone
		tail := srv.StderrTail()
		msg := "codex app-server exited before turn completed"
		if tail != "" {
			msg = msg + "\n" + tail
		}
		res := Result{
			SessionID: threadID,
			Subtype:   "error_during_execution",
			IsError:   true,
			Stderr:    tail,
			Text:      msg,
		}
		stampCodexDuration(started, &res)
		detachEmit()
		return res, fmt.Errorf("%s", msg)
	case <-ctx.Done():
		endTurn()
		<-steerDone
		// Detach without KindResult. Cancellation is classified by
		// finalizeChatReply (user Stop vs ask_user/switch_mode park). Emitting
		// session.turn_end here would close the mirrored turn before that
		// settlement and duplicate the Stop footer (or add one on a quiet park).
		detachEmit()
		res := Result{
			SessionID: threadID,
			Subtype:   "error_during_execution",
			IsError:   true,
			Stderr:    srv.StderrTail(),
			Text:      ctx.Err().Error(),
		}
		stampCodexDuration(started, &res)
		return res, ctx.Err()
	}
	<-steerDone

	mu.Lock()
	text := finalText.String()
	last := usageLast
	turnU := usageTurn
	cw := contextWin
	mu.Unlock()

	res := Result{
		SessionID: threadID,
		Subtype:   "success",
		Text:      text,
		Stderr:    srv.StderrTail(),
	}
	stampCodexTurnUsage(&res, last, turnU, cw)
	status := strings.ToLower(done.status)
	if status != "" && status != "completed" && status != "success" {
		res.IsError = true
		res.Subtype = "error_during_execution"
		if done.errMsg != "" {
			res.Text = done.errMsg
		}
	}
	stampCodexDuration(started, &res)
	if res.IsError {
		// Persist tokens/context via KindResult, but clear DurationMS so
		// session.turn_end is not published. finalizeChatReply stamps the
		// footer on the synthesized Agent-error note from the returned Result.
		usage := res
		usage.DurationMS = 0
		finishEmit(ParsedEvent{Kind: KindResult, Result: &usage, Text: res.Text, IsError: true})
		return res, nil
	}
	finishEmit(ParsedEvent{Kind: KindResult, Result: &res, Text: res.Text})
	return res, nil
}

// elapsedDurationMS is wall time since started, floored at 1ms. Codex does not
// report duration on turn/completed; a 0 value would skip session.turn_end and
// the transcript footer (and the findings digest rendered on that footer).
func elapsedDurationMS(started time.Time) int {
	ms := int(time.Since(started).Milliseconds())
	if ms < 1 {
		return 1
	}
	return ms
}

func stampCodexDuration(started time.Time, res *Result) {
	if res == nil {
		return
	}
	res.DurationMS = elapsedDurationMS(started)
}

// codexUsageState is the last thread/tokenUsage/updated snapshot. last is the
// single-request footprint for the composer meter; total is thread-cumulative
// billing.
type codexUsageState struct {
	last   codex.TokenUsage
	total  codex.TokenUsage
	window int64
}

func (s *codexUsageState) observe(params json.RawMessage) {
	if s == nil {
		return
	}
	var p struct {
		TokenUsage struct {
			Total              codex.TokenUsage `json:"total"`
			Last               codex.TokenUsage `json:"last"`
			ModelContextWindow int64            `json:"modelContextWindow"`
		} `json:"tokenUsage"`
	}
	_ = json.Unmarshal(params, &p)
	s.total = p.TokenUsage.Total
	s.window = p.TokenUsage.ModelContextWindow
	s.last = p.TokenUsage.Last
	if tokenUsageEmpty(s.last) {
		s.last = s.total
	}
}

func tokenUsageEmpty(u codex.TokenUsage) bool {
	return u.TotalTokens == 0 && u.InputTokens == 0 && u.CachedInputTokens == 0 && u.CacheWriteInputTokens == 0
}

// codexFreshInput is uncached prompt tokens. Codex inputTokens is OpenAI-style
// (cached is a subset); Recur's meter adds cacheRead on top of input.
func codexFreshInput(u codex.TokenUsage) int64 {
	fresh := u.InputTokens - u.CachedInputTokens
	if fresh < 0 {
		return u.InputTokens
	}
	return fresh
}

// applyCodexMeterUsage copies a single-request Codex footprint onto Result.
func applyCodexMeterUsage(out *Result, u codex.TokenUsage, window int64) {
	if out == nil {
		return
	}
	out.InputTokens = codexFreshInput(u)
	out.OutputTokens = u.OutputTokens
	out.CacheReadTokens = u.CachedInputTokens
	out.CacheCreationTokens = u.CacheWriteInputTokens
	if window > 0 {
		out.ContextWindow = window
	}
}

// stampCodexTurnUsage copies the last-request footprint onto the context meter.
// If last is empty, turnU (turn-cumulative tokens) is used instead.
func stampCodexTurnUsage(res *Result, last, turnU codex.TokenUsage, window int64) {
	if res == nil {
		return
	}
	if tokenUsageEmpty(last) {
		last = turnU
	}
	applyCodexMeterUsage(res, last, window)
}

func subTokenUsage(total, sub codex.TokenUsage) codex.TokenUsage {
	clamp := func(a, b int64) int64 {
		if a < b {
			return 0
		}
		return a - b
	}
	return codex.TokenUsage{
		TotalTokens:           clamp(total.TotalTokens, sub.TotalTokens),
		InputTokens:           clamp(total.InputTokens, sub.InputTokens),
		CachedInputTokens:     clamp(total.CachedInputTokens, sub.CachedInputTokens),
		CacheWriteInputTokens: clamp(total.CacheWriteInputTokens, sub.CacheWriteInputTokens),
		OutputTokens:          clamp(total.OutputTokens, sub.OutputTokens),
		ReasoningOutputTokens: clamp(total.ReasoningOutputTokens, sub.ReasoningOutputTokens),
	}
}

func (r *CodexRunner) handleItemStarted(params json.RawMessage, openTools map[string]string, mcpArgs map[string]json.RawMessage, fileItems *sync.Map, emit func(ParsedEvent)) {
	var p struct {
		Item json.RawMessage `json:"item"`
	}
	if json.Unmarshal(params, &p) != nil {
		return
	}
	var head struct {
		Type   string `json:"type"`
		ID     string `json:"id"`
		Server string `json:"server"`
		Tool   string `json:"tool"`
	}
	if json.Unmarshal(p.Item, &head) != nil {
		return
	}
	switch head.Type {
	case "mcpToolCall":
		var item struct {
			ID        string          `json:"id"`
			Server    string          `json:"server"`
			Tool      string          `json:"tool"`
			Arguments json.RawMessage `json:"arguments"`
		}
		_ = json.Unmarshal(p.Item, &item)
		if item.ID == "" {
			item.ID = head.ID
		}
		if item.Server == "" {
			item.Server = head.Server
		}
		if item.Tool == "" {
			item.Tool = head.Tool
		}
		name := "mcp__" + item.Server + "__" + item.Tool
		openTools[item.ID] = name
		raw := item.Arguments
		if len(raw) == 0 {
			raw = json.RawMessage(`{}`)
		} else if mcpArgs != nil {
			mcpArgs[item.ID] = append(json.RawMessage(nil), raw...)
		}
		emit(ParsedEvent{
			Kind:         KindStep,
			Tool:         name,
			ToolUseID:    item.ID,
			ToolInputRaw: raw,
			ToolInput:    item.Server + "/" + item.Tool,
		})
	case "commandExecution":
		var item struct {
			ID      string `json:"id"`
			Command string `json:"command"`
		}
		_ = json.Unmarshal(p.Item, &item)
		openTools[item.ID] = "shell"
		raw, _ := json.Marshal(map[string]string{"command": item.Command})
		emit(ParsedEvent{
			Kind:         KindStep,
			Tool:         "shell",
			ToolUseID:    item.ID,
			ToolInputRaw: raw,
			ToolInput:    item.Command,
		})
	case "fileChange":
		// simplify: stash item/started only — Codex has no live patchUpdated
		// method; empty changes[] here leaves the approval card identifier-only.
		if fileItems != nil && head.ID != "" {
			fileItems.Store(head.ID, append(json.RawMessage(nil), p.Item...))
		}
		tool, path, raw := codexFileChangeStep(p.Item)
		openTools[head.ID] = tool
		emit(ParsedEvent{
			Kind:         KindStep,
			Tool:         tool,
			ToolUseID:    head.ID,
			ToolInputRaw: raw,
			ToolInput:    path,
		})
	}
}

func (r *CodexRunner) handleItemCompleted(
	params json.RawMessage,
	mu *sync.Mutex,
	finalText *strings.Builder,
	openTools map[string]string,
	mcpArgs map[string]json.RawMessage,
	emit func(ParsedEvent),
) {
	var p struct {
		Item json.RawMessage `json:"item"`
	}
	if json.Unmarshal(params, &p) != nil {
		return
	}
	var head struct {
		Type string `json:"type"`
		ID   string `json:"id"`
	}
	if json.Unmarshal(p.Item, &head) != nil {
		return
	}
	switch head.Type {
	case "agentMessage":
		var item struct {
			Text    string `json:"text"`
			Content []struct {
				Type string `json:"type"`
				Text string `json:"text"`
			} `json:"content"`
		}
		_ = json.Unmarshal(p.Item, &item)
		text := item.Text
		if text == "" {
			for _, c := range item.Content {
				if c.Type == "text" {
					text += c.Text
				}
			}
		}
		if text != "" {
			mu.Lock()
			// Prefer completed block over streamed deltas when both arrive.
			if finalText.Len() == 0 {
				finalText.WriteString(text)
			}
			mu.Unlock()
			emit(ParsedEvent{Kind: KindText, Text: text})
		}
	case "reasoning":
		// App-server sends summary as []string; older/docs shapes use
		// [{text:…}]. Accept both so thinking blocks always settle.
		text := codexReasoningSummaryText(p.Item)
		if text != "" {
			emit(ParsedEvent{Kind: KindThinking, Text: text})
		}
	case "mcpToolCall":
		var item struct {
			ID     string          `json:"id"`
			Server string          `json:"server"`
			Tool   string          `json:"tool"`
			Status string          `json:"status"`
			Result json.RawMessage `json:"result"`
			Error  *struct {
				Message string `json:"message"`
			} `json:"error"`
			Arguments json.RawMessage `json:"arguments"`
		}
		_ = json.Unmarshal(p.Item, &item)
		name := openTools[item.ID]
		if name == "" {
			name = "mcp__" + item.Server + "__" + item.Tool
		}
		text := string(item.Result)
		isErr := item.Error != nil || strings.EqualFold(item.Status, "failed") || strings.EqualFold(item.Status, "error")
		if item.Error != nil {
			text = item.Error.Message
		}
		args := item.Arguments
		if len(args) == 0 && mcpArgs != nil {
			args = mcpArgs[item.ID]
		}
		if len(args) > 0 && mcpArgs != nil {
			mcpArgs[item.ID] = append(json.RawMessage(nil), args...)
		}
		emit(ParsedEvent{
			Kind:         KindToolResult,
			Tool:         name,
			ToolUseID:    item.ID,
			Text:         text,
			IsError:      isErr,
			ToolInputRaw: args,
		})
	case "commandExecution":
		var item struct {
			ID               string `json:"id"`
			Command          string `json:"command"`
			AggregatedOutput string `json:"aggregatedOutput"`
			ExitCode         *int   `json:"exitCode"`
			Status           string `json:"status"`
		}
		_ = json.Unmarshal(p.Item, &item)
		isErr := item.ExitCode != nil && *item.ExitCode != 0
		emit(ParsedEvent{
			Kind:      KindToolResult,
			Tool:      "shell",
			ToolUseID: item.ID,
			Text:      item.AggregatedOutput,
			IsError:   isErr,
		})
	case "fileChange":
		tool, _, raw := codexFileChangeStep(p.Item)
		if name := openTools[head.ID]; name != "" {
			tool = name
		}
		var item struct {
			ID     string `json:"id"`
			Status string `json:"status"`
		}
		_ = json.Unmarshal(p.Item, &item)
		text := codexFileChangeResultText(p.Item, tool)
		emit(ParsedEvent{
			Kind:         KindToolResult,
			Tool:         tool,
			ToolUseID:    head.ID,
			Text:         text,
			IsError:      strings.EqualFold(item.Status, "failed"),
			ToolInputRaw: raw,
		})
	}
}

func mcpServersFor(spec AgentSpec) []codex.MCPServer {
	var out []codex.MCPServer
	if spec.MCPURL != "" {
		out = append(out, codex.MCPServer{
			Name:     "recur",
			URL:      spec.MCPURL,
			Required: true,
		})
	}
	if spec.Playwright {
		out = append(out, codex.MCPServer{
			Name:    "playwright",
			Command: "npx",
			Args:    []string{"-y", "@playwright/mcp@latest"},
		})
	}
	for _, u := range spec.UserMCP {
		name := strings.TrimSpace(u.Name)
		if name == "" {
			continue
		}
		out = append(out, codex.MCPServer{
			Name:        name,
			URL:         u.URL,
			Command:     u.Command,
			Args:        u.Args,
			Env:         u.Env,
			HTTPHeaders: u.Headers,
		})
	}
	return out
}

func mcpStatusFrom(servers []codex.MCPServer) []MCPServerStatus {
	out := make([]MCPServerStatus, 0, len(servers))
	for _, s := range servers {
		out = append(out, MCPServerStatus{Name: s.Name, Status: "connected"})
	}
	return out
}

// codexSandbox picks thread/turn sandbox from the allow-list (same rule as
// Cursor plan mode) so PA Ask/Plan/Review chats are read-only even though they
// are RoleCoder. RoleReviewer stays read-only if AllowedTools is empty.
// BypassPermissions on a write role is Codex's danger-full-access (Claude's
// bypass analog); read-only roles stay sandboxed. Read-only still gets
// networkAccess so `gh pr view` / `gh pr diff` work — Recur's GitHub PR
// surface tells them to use those, and injected diffs truncate.
func codexSandbox(spec AgentSpec, network bool) (string, *codex.SandboxPolicy) {
	if isReadOnlyRole(spec) || roleIsReadOnly(spec.Role) {
		return "read-only", &codex.SandboxPolicy{Type: "readOnly", NetworkAccess: true}
	}
	if spec.PermissionMode == domain.PermissionModeBypass {
		return "danger-full-access", &codex.SandboxPolicy{Type: "dangerFullAccess"}
	}
	return "workspace-write", &codex.SandboxPolicy{Type: "workspaceWrite", NetworkAccess: network}
}

func codexApprovalPolicy(mode string) string {
	switch mode {
	case domain.PermissionModeBypass, domain.PermissionModeDeny:
		return "never"
	default:
		return "on-request"
	}
}

func specAgentCode(spec AgentSpec) string {
	code := strings.TrimPrefix(spec.AgentID, spec.WorkspaceID+"-")
	if code == "" || code == spec.AgentID {
		return spec.AgentID
	}
	return code
}

func (r *CodexRunner) installApprovals(ctx context.Context, spec AgentSpec, conn *codex.Conn, fileItems *sync.Map) {
	if conn == nil || spec.PermissionMode == domain.PermissionModeBypass {
		return
	}
	if spec.PermissionMode != domain.PermissionModeDeny && r.AskPermission == nil {
		return
	}
	ask := r.AskPermission
	conn.OnServerRequest(func(method string, params json.RawMessage) (any, error) {
		return handleCodexServerRequest(ctx, spec, ask, method, params, fileItems)
	})
}

func handleCodexServerRequest(
	ctx context.Context,
	spec AgentSpec,
	ask func(context.Context, domain.PermissionRequest) (domain.PermissionDecision, error),
	method string,
	params json.RawMessage,
	fileItems *sync.Map,
) (any, error) {
	tool, human := codexApprovalTool(method)
	if !human {
		return codex.AutoApproveServerRequest(method, params)
	}
	if spec.PermissionMode == domain.PermissionModeDeny {
		return codexApprovalResponse(method, domain.PermissionDecision{}, params), nil
	}
	if ask == nil {
		return codex.AutoApproveServerRequest(method, params)
	}
	input := enrichCodexApprovalParams(method, params, fileItems)
	d, err := ask(ctx, domain.PermissionRequest{
		WorkspaceID: spec.WorkspaceID,
		AgentCode:   specAgentCode(spec),
		Tool:        tool,
		Input:       string(input),
		Summary:     permissionSummary(tool, input),
	})
	if err != nil {
		return codexApprovalResponse(method, domain.PermissionDecision{}, params), nil
	}
	return codexApprovalResponse(method, d, params), nil
}

// enrichCodexApprovalParams copies the matching fileChange item's changes
// (kind, path, diff, movePath) onto an Edit approval. item/fileChange/requestApproval
// only has itemId/reason/grantRoot — the patch lives on item/started (same itemId).
func enrichCodexApprovalParams(method string, params json.RawMessage, fileItems *sync.Map) json.RawMessage {
	tool, _ := codexApprovalTool(method)
	if tool != "Edit" || fileItems == nil {
		return params
	}
	var head struct {
		ItemID   string `json:"itemId"`
		Path     string `json:"path"`
		FilePath string `json:"file_path"`
		Diff     string `json:"diff"`
	}
	_ = json.Unmarshal(params, &head)
	if head.Path != "" || head.FilePath != "" || head.Diff != "" {
		return params
	}
	raw, ok := fileItems.Load(head.ItemID)
	if !ok {
		return params
	}
	item, _ := raw.(json.RawMessage)
	extra := codexFileChangePermissionInput(item)
	if len(extra) == 0 {
		return params
	}
	var out map[string]any
	_ = json.Unmarshal(params, &out)
	if out == nil {
		out = map[string]any{}
	}
	for k, v := range extra {
		out[k] = v
	}
	b, err := json.Marshal(out)
	if err != nil {
		return params
	}
	return b
}

func codexFileChangePermissionInput(item json.RawMessage) map[string]any {
	var parsed struct {
		Changes []map[string]any `json:"changes"`
	}
	_ = json.Unmarshal(item, &parsed)
	if len(parsed.Changes) == 0 {
		return nil
	}
	paths := make([]string, 0, len(parsed.Changes))
	var diffs strings.Builder
	for _, c := range parsed.Changes {
		if p, _ := c["path"].(string); p != "" {
			paths = append(paths, p)
		}
		if d, _ := c["diff"].(string); d != "" {
			diffs.WriteString(d)
		}
	}
	out := map[string]any{"changes": parsed.Changes}
	if len(paths) > 0 {
		joined := strings.Join(paths, ", ")
		out["path"] = joined
		out["file_path"] = joined
	}
	if diffs.Len() > 0 {
		out["diff"] = diffs.String()
	}
	return out
}

func codexApprovalTool(method string) (tool string, human bool) {
	switch method {
	case "item/commandExecution/requestApproval", "commandExecution/requestApproval", "execCommandApproval":
		return "Bash", true
	case "item/fileChange/requestApproval", "fileChange/requestApproval", "applyPatchApproval":
		return "Edit", true
	case "item/permissions/requestApproval":
		return "Permissions", true
	default:
		return "", false
	}
}

func codexApprovalResponse(method string, d domain.PermissionDecision, params json.RawMessage) any {
	if method == "item/permissions/requestApproval" {
		if !d.Allow {
			return map[string]any{"permissions": map[string]any{}}
		}
		scope := "turn"
		if d.Remember {
			scope = "session"
		}
		perms := any(map[string]any{})
		var p struct {
			Permissions json.RawMessage `json:"permissions"`
		}
		if json.Unmarshal(params, &p) == nil && len(p.Permissions) > 0 {
			var v any
			if json.Unmarshal(p.Permissions, &v) == nil {
				perms = v
			}
		}
		return map[string]any{"scope": scope, "permissions": perms}
	}
	if method == "execCommandApproval" {
		if d.Allow {
			return map[string]any{"decision": "approved"}
		}
		return map[string]any{"decision": "denied"}
	}
	decision := "decline"
	if d.Allow {
		decision = "accept"
		if d.Remember {
			decision = "acceptForSession"
		}
	}
	return map[string]any{"decision": decision}
}

func roleIsReadOnly(role domain.AgentRole) bool {
	switch role {
	case domain.RoleReviewer:
		return true
	default:
		// Also treat explicit plan-like roles as read-only when present.
		return strings.EqualFold(string(role), "reviewer")
	}
}

// codexFileChangeStep maps a fileChange item onto Recur's Edit/Write/Delete
// vocabulary. Codex put create bodies in `changes[].diff` (not a unified diff);
// updates use hunk-only unified diffs (@@… without ---/+++ headers).
func codexFileChangeStep(raw json.RawMessage) (tool, path string, input json.RawMessage) {
	var item struct {
		Changes []struct {
			Path string          `json:"path"`
			Kind json.RawMessage `json:"kind"`
			Diff string          `json:"diff"`
		} `json:"changes"`
	}
	_ = json.Unmarshal(raw, &item)
	if len(item.Changes) == 0 {
		b, _ := json.Marshal(map[string]string{})
		return "edit", "", b
	}
	c0 := item.Changes[0]
	path = c0.Path
	switch codexFileChangeKind(c0.Kind) {
	case "add":
		b, _ := json.Marshal(map[string]string{"file_path": path, "content": c0.Diff})
		return "write", path, b
	case "delete":
		b, _ := json.Marshal(map[string]string{"file_path": path})
		return "delete", path, b
	default:
		b, _ := json.Marshal(map[string]string{"path": path})
		return "edit", path, b
	}
}

func codexFileChangeKind(raw json.RawMessage) string {
	var k struct {
		Type string `json:"type"`
	}
	_ = json.Unmarshal(raw, &k)
	return strings.ToLower(strings.TrimSpace(k.Type))
}

// codexFileChangeResultText returns the tool-result body. Writes carry content
// in args (Claude shape), so the result stays empty; edits concatenate hunk
// diffs for the unified-diff viewer.
func codexFileChangeResultText(raw json.RawMessage, tool string) string {
	if tool == "write" || tool == "Write" || tool == "delete" || tool == "Delete" {
		return ""
	}
	var item struct {
		Changes []struct {
			Diff string `json:"diff"`
		} `json:"changes"`
	}
	_ = json.Unmarshal(raw, &item)
	var b strings.Builder
	for _, c := range item.Changes {
		b.WriteString(c.Diff)
	}
	return b.String()
}

// codexReasoningSummaryText extracts readable summary text from a completed
// reasoning item. App-server currently sends `summary` as []string; the docs
// also describe [{text:…}] / [{type,text}].
func codexReasoningSummaryText(raw json.RawMessage) string {
	var wrap struct {
		Summary json.RawMessage `json:"summary"`
	}
	if err := json.Unmarshal(raw, &wrap); err != nil || len(wrap.Summary) == 0 {
		return ""
	}
	var asStrings []string
	if err := json.Unmarshal(wrap.Summary, &asStrings); err == nil {
		var b strings.Builder
		for _, s := range asStrings {
			s = strings.TrimSpace(s)
			if s == "" {
				continue
			}
			if b.Len() > 0 {
				b.WriteString("\n\n")
			}
			b.WriteString(s)
		}
		return b.String()
	}
	var asObjs []struct {
		Text string `json:"text"`
	}
	if err := json.Unmarshal(wrap.Summary, &asObjs); err == nil {
		var b strings.Builder
		for _, s := range asObjs {
			s.Text = strings.TrimSpace(s.Text)
			if s.Text == "" {
				continue
			}
			if b.Len() > 0 {
				b.WriteString("\n\n")
			}
			b.WriteString(s.Text)
		}
		return b.String()
	}
	return ""
}

// gpt56DefaultContextWindow is Codex's default GPT-5.6 window. Larger prompts
// are billed at 2× input / 1.5× output, so Recur only raises the window when
// the dock Context flag is 1M.
const gpt56DefaultContextWindow int64 = 272_000

func codexNetworkOverrides(on bool) []string {
	if !on {
		return nil
	}
	return []string{`sandbox_workspace_write.network_access=true`}
}

// codexLongContextOverrides returns -c pairs that raise Codex's model window.
// Empty when the dock is on the default 272K (or the flag is unset).
func codexLongContextOverrides(flags map[string]string) []string {
	if parseContextFlag(flags["context"]) <= gpt56DefaultContextWindow {
		return nil
	}
	return []string{
		`model_context_window=1000000`,
		`model_auto_compact_token_limit=900000`,
	}
}

// codexFastServiceTier is the fallback app-server id when an older saved flag
// still stores fast=true instead of a concrete tier id.
const codexFastServiceTier = "priority"

// codexServiceTierFromFlags maps the Fast flag (and any legacy
// serviceTier value) onto turn/start serviceTier. The catalogue stores the
// boost tier id as the On value (e.g. "priority"); legacy "true"/"on" map to
// the common Fast id.
func codexServiceTierFromFlags(flags map[string]string) string {
	if flags == nil {
		return ""
	}
	// Legacy Speed dimension from an earlier catalogue shape.
	if v := strings.TrimSpace(flags["serviceTier"]); v != "" {
		switch strings.ToLower(v) {
		case "standard", "default", "off", "false":
			return ""
		default:
			return v
		}
	}
	v := strings.TrimSpace(flags["fast"])
	switch strings.ToLower(v) {
	case "", "false", "off", "0", "standard", "default":
		return ""
	case "true", "on", "1":
		return codexFastServiceTier
	default:
		return v // concrete tier id from model/list
	}
}
