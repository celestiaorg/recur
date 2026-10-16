package engine

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"sync"
	"time"

	"recur/internal/agenttools"
	"recur/internal/domain"
	"recur/internal/mcp"
	"recur/internal/responses"
	"recur/internal/store"
)

const harnessParallelLimit = 8

var parallelToolNames = map[string]bool{
	"Read": true, "Glob": true, "Grep": true,
	"list_findings": true, "list_skills": true, "list_artifacts": true, "read_artifact": true, "list_terminals": true,
	"list_workspace_briefs": true, "get_workspace_context": true,
}

// HarnessRunner is Recur's in-process coding agent: it owns the tool loop and
// calls an OpenAI-compatible POST /v1/responses endpoint.
type HarnessRunner struct {
	Provider domain.CompatibleProvider
	Client   *responses.Client
	Store    *store.Store
	Reporter mcp.Reporter
	Todos    *agenttools.TodoStore
}

// NeedsPermissionPromptTool reports that the native harness gates Bash itself.
func (*HarnessRunner) NeedsPermissionPromptTool() bool { return false }

func (e *Engine) newHarnessRunner(p domain.CompatibleProvider) *HarnessRunner {
	return &HarnessRunner{
		Provider: p,
		Client:   responses.NewClient(p),
		Store:    e.Store,
		Reporter: engineReporter{eng: e},
		Todos:    &e.todos,
	}
}

func toolsDisabled(spec AgentSpec) bool {
	return spec.Tools != nil && strings.TrimSpace(*spec.Tools) == ""
}

func harnessAgentCode(spec AgentSpec) string {
	if spec.Route.Code != "" {
		return spec.Route.Code
	}
	code := strings.TrimPrefix(spec.AgentID, spec.WorkspaceID+"-")
	if code == "" {
		return spec.AgentID
	}
	return code
}

func toolAllowedOnTurn(name string, spec AgentSpec) bool {
	if toolsDisabled(spec) {
		return false
	}
	bare := agenttools.BareName(name)
	if bare == "permission_request" {
		return false
	}
	if spec.AllowedTools == nil {
		return true
	}
	for _, a := range spec.AllowedTools {
		if a == name || agenttools.BareName(a) == bare {
			return true
		}
	}
	return false
}

// Run executes one agent turn against the compatible Responses API.
func (r *HarnessRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if emit == nil {
		emit = func(ParsedEvent) {}
	}
	start := time.Now()
	model := spec.Model
	if model == "" && len(r.Provider.Models) > 0 {
		model = r.Provider.Models[0].ID
	}
	reg := agenttools.NewRegistry(append(agenttools.HostTools(), agenttools.CodingTools()...)...)
	var tools []responses.Tool
	if !toolsDisabled(spec) {
		tools = r.responsesTools(reg, spec.AllowedTools)
	}
	input := r.buildInput(spec)
	instructions := harnessInstructions(spec)

	var usage responses.Usage
	var text strings.Builder
	var lastID string
	rounds := 0

	emit(ParsedEvent{Kind: KindInit, Provider: spec.Provider, Model: model, SessionID: spec.RecurSessionID})

	for {
		if err := ctx.Err(); err != nil {
			return r.result(spec, start, text.String(), usage, lastID), err
		}
		rounds++
		if spec.MaxTurns > 0 && rounds > spec.MaxTurns {
			res := r.result(spec, start, text.String(), usage, lastID)
			res.IsError = true
			res.Subtype = "error_max_turns"
			emit(ParsedEvent{Kind: KindResult, Provider: spec.Provider, Result: &res, Text: res.Text})
			return res, nil
		}
		calls, roundText, roundUsage, respID, err := r.round(ctx, spec, model, instructions, input, tools, emit)
		if err != nil {
			return r.result(spec, start, text.String(), usage, lastID), err
		}
		if respID != "" {
			lastID = respID
		}
		if roundUsage.InputTokens > 0 || roundUsage.OutputTokens > 0 {
			usage = roundUsage
		}
		text.WriteString(roundText)
		if roundText != "" {
			input = append(input, responses.AssistantText(roundText))
		}

		halt := false
		if len(calls) > 0 {
			results := r.execCalls(ctx, spec, reg, calls, emit)
			if err := ctx.Err(); err != nil {
				return r.result(spec, start, text.String(), usage, lastID), err
			}
			for i, call := range calls {
				input = append(input, responses.FunctionCallItem(call.CallID, call.Name, call.Arguments))
				input = append(input, responses.FunctionOutputItem(call.CallID, results[i].text))
				if results[i].halt {
					halt = true
				}
			}
		}
		if halt {
			break
		}
		steers := drainSteers(spec.Inject)
		if len(steers) == 0 {
			if len(calls) == 0 {
				break
			}
			continue
		}
		for _, s := range steers {
			prompt, display := s, ""
			if spec.InjectPlain {
				if prompt, display = decodeInjectPlain(s); display == prompt {
					display = ""
				}
			}
			emit(ParsedEvent{Kind: KindUser, Provider: spec.Provider, Text: prompt, DisplayText: display})
			input = append(input, responses.UserText(prompt))
		}
	}

	res := r.result(spec, start, text.String(), usage, lastID)
	emit(ParsedEvent{Kind: KindResult, Provider: spec.Provider, Result: &res, Text: res.Text})
	return res, nil
}

func drainSteers(ch <-chan string) []string {
	if ch == nil {
		return nil
	}
	var out []string
	for {
		select {
		case s, ok := <-ch:
			if !ok {
				return out
			}
			if strings.TrimSpace(s) != "" {
				out = append(out, s)
			}
		default:
			return out
		}
	}
}

func (r *HarnessRunner) buildInput(spec AgentSpec) []responses.InputItem {
	var msgs []domain.Message
	if r.Store != nil && spec.RecurSessionID != "" {
		msgs, _ = r.Store.SessionTranscript(spec.RecurSessionID)
		if spec.HistoryAfterID == "" {
			if sess, ok := r.Store.GetSession(spec.RecurSessionID); ok {
				spec.HistoryAfterID = nativeHistoryAfterID(sess.ConfigJSON)
			}
		}
	}
	return harnessHistory(msgs, spec)
}

func (r *HarnessRunner) responsesTools(reg *agenttools.Registry, allowed []string) []responses.Tool {
	listed := reg.List(allowed)
	out := make([]responses.Tool, 0, len(listed))
	for _, t := range listed {
		if t.Name() == "permission_request" {
			continue
		}
		name := agenttools.AdvertisedName(t, allowed)
		out = append(out, responses.FunctionTool(name, t.Description(), t.Schema()))
	}
	return out
}

func (r *HarnessRunner) round(ctx context.Context, spec AgentSpec, model, instructions string, input []responses.InputItem, tools []responses.Tool, emit func(ParsedEvent)) (calls []responses.FunctionCall, text string, usage responses.Usage, respID string, err error) {
	req := responses.Request{
		Model:             model,
		Instructions:      instructions,
		Input:             input,
		Tools:             tools,
		ToolChoice:        "auto",
		Stream:            true,
		Store:             false,
		ParallelToolCalls: true,
	}
	raw, err := json.Marshal(req)
	if err != nil {
		return nil, "", usage, "", err
	}
	var body map[string]any
	if err := json.Unmarshal(raw, &body); err != nil {
		return nil, "", usage, "", err
	}
	if m := r.Provider.ModelByID(model); m != nil {
		responses.ApplyFlags(body, m.Flags, spec.Flags)
	}
	payload, err := json.Marshal(body)
	if err != nil {
		return nil, "", usage, "", err
	}

	pending := map[string]*responses.FunctionCall{}
	var order []string
	items := map[string]*harnessItem{}
	var itemOrder []string
	anon := 0
	item := func(ev responses.Event) *harnessItem {
		id := ev.ItemID
		if id == "" {
			if b := items[""]; b != nil && (b.textFinal != "" || b.thinkFinal != "") {
				anon++
				id = fmt.Sprintf("anon-%d", anon)
			}
		}
		if b := items[id]; b != nil {
			return b
		}
		itemOrder = append(itemOrder, id)
		b := &harnessItem{}
		items[id] = b
		return b
	}
	var status string
	var sawCompleted bool
	var fail error
	client := r.Client
	if client == nil {
		client = responses.NewClient(r.Provider)
	}
	err = client.StreamJSON(ctx, payload, func(ev responses.Event) {
		if ev.ResponseID != "" {
			respID = ev.ResponseID
		}
		if ev.Status != "" {
			status = ev.Status
		}
		if ev.Type == "response.completed" {
			sawCompleted = true
		}
		if ev.Type == "response.incomplete" {
			status = "incomplete"
		}
		if ev.ErrorMessage != "" || ev.Type == "error" || ev.Type == "response.failed" {
			msg := ev.ErrorMessage
			if msg == "" {
				msg = ev.Type
			}
			fail = fmt.Errorf("responses: %s", msg)
			return
		}
		switch {
		case ev.Type == "response.output_text.delta" || strings.HasSuffix(ev.Type, "output_text.delta"):
			if ev.Delta == "" {
				return
			}
			item(ev).textStream.WriteString(ev.Delta)
			if spec.IncludePartials {
				emit(ParsedEvent{Kind: KindTextDelta, Provider: spec.Provider, Text: ev.Delta})
			}
		case ev.Type == "response.reasoning_summary_text.delta" || ev.Type == "response.reasoning_text.delta":
			if ev.Delta == "" {
				return
			}
			item(ev).thinkStream.WriteString(ev.Delta)
			if spec.IncludePartials {
				emit(ParsedEvent{Kind: KindThinkingDelta, Provider: spec.Provider, Text: ev.Delta})
			}
		case ev.Type == "response.reasoning_summary_text.done" || ev.Type == "response.reasoning_text.done":
			b := item(ev)
			if b.thinkStream.Len() == 0 {
				b.thinkFinal = ev.Delta
			}
		case ev.Type == "response.output_text.done":
			b := item(ev)
			if b.textStream.Len() == 0 {
				b.textFinal = ev.Delta
			}
		case (ev.Type == "response.output_item.done" || strings.HasSuffix(ev.Type, "output_item.done")) && ev.OutputText != "":
			b := item(ev)
			if b.textStream.Len() == 0 {
				b.textFinal = ev.OutputText
			}
		case ev.Call != nil:
			id := ev.Call.ID
			if id == "" {
				id = ev.Call.CallID
			}
			if id == "" {
				id = fmt.Sprintf("call-%d", len(order))
			}
			c := ev.Call
			if c.CallID == "" {
				c.CallID = id
			}
			if pending[id] == nil {
				order = append(order, id)
			}
			pending[id] = c
		case ev.Type == "response.function_call_arguments.delta" && ev.Delta != "":
			id := ev.ItemID
			if p := pending[id]; p != nil {
				p.Arguments += ev.Delta
			}
		}
		if ev.Usage != nil {
			usage = *ev.Usage
		}
	})
	text, think := flushHarnessItems(items, itemOrder)
	if fail != nil {
		return nil, text, usage, respID, fail
	}
	if err != nil {
		return nil, text, usage, respID, err
	}
	if err := terminalResponseError(status, sawCompleted); err != nil {
		return nil, text, usage, respID, err
	}
	if text != "" {
		emit(ParsedEvent{Kind: KindText, Provider: spec.Provider, Text: text})
	}
	if think != "" {
		emit(ParsedEvent{Kind: KindThinking, Provider: spec.Provider, Text: think})
	}
	for _, id := range order {
		c := pending[id]
		if c == nil || c.Name == "" {
			continue
		}
		calls = append(calls, *c)
	}
	return calls, text, usage, respID, nil
}

// harnessItem accumulates one Responses output item's streamed vs final text.
type harnessItem struct {
	textStream, thinkStream strings.Builder
	textFinal, thinkFinal   string
}

func flushHarnessItems(items map[string]*harnessItem, order []string) (text, think string) {
	var tb, hb strings.Builder
	for _, id := range order {
		b := items[id]
		if b == nil {
			continue
		}
		if b.textStream.Len() > 0 {
			tb.WriteString(b.textStream.String())
		} else {
			tb.WriteString(b.textFinal)
		}
		if b.thinkStream.Len() > 0 {
			hb.WriteString(b.thinkStream.String())
		} else {
			hb.WriteString(b.thinkFinal)
		}
	}
	return tb.String(), hb.String()
}

func terminalResponseError(status string, sawCompleted bool) error {
	switch status {
	case "failed", "cancelled", "canceled", "incomplete":
		return fmt.Errorf("responses status %s", status)
	}
	if sawCompleted || status == "completed" {
		return nil
	}
	return fmt.Errorf("responses: stream ended before completion")
}

type callResult struct {
	text string
	halt bool
}

func (r *HarnessRunner) execCalls(ctx context.Context, spec AgentSpec, reg *agenttools.Registry, calls []responses.FunctionCall, emit func(ParsedEvent)) []callResult {
	out := make([]callResult, len(calls))
	var emitMu sync.Mutex
	ser := func(ev ParsedEvent) {
		emitMu.Lock()
		defer emitMu.Unlock()
		emit(ev)
	}
	var parallel []int
	var serial []int
	for i, c := range calls {
		if parallelToolNames[agenttools.BareName(c.Name)] {
			parallel = append(parallel, i)
		} else {
			serial = append(serial, i)
		}
	}
	if ctx.Err() != nil {
		return out
	}
	if len(parallel) > 0 {
		sem := make(chan struct{}, harnessParallelLimit)
		var wg sync.WaitGroup
		for _, i := range parallel {
			select {
			case <-ctx.Done():
			case sem <- struct{}{}:
				wg.Add(1)
				go func(i int) {
					defer wg.Done()
					defer func() { <-sem }()
					out[i] = r.execOne(ctx, spec, reg, calls[i], ser)
				}(i)
				continue
			}
			break
		}
		wg.Wait()
	}
	for _, i := range serial {
		if ctx.Err() != nil {
			break
		}
		out[i] = r.execOne(ctx, spec, reg, calls[i], ser)
		if out[i].halt || ctx.Err() != nil {
			break
		}
	}
	return out
}

func (r *HarnessRunner) execOne(ctx context.Context, spec AgentSpec, reg *agenttools.Registry, call responses.FunctionCall, emit func(ParsedEvent)) callResult {
	name := call.Name
	args := json.RawMessage(call.Arguments)
	if !json.Valid(args) {
		args = json.RawMessage([]byte("{}"))
	}
	emit(ParsedEvent{
		Kind: KindStep, Provider: spec.Provider, Tool: name, ToolUseID: call.CallID,
		ToolInputRaw: args, ToolInput: string(args),
	})
	if !toolAllowedOnTurn(name, spec) {
		msg := "tool not allowed: " + name
		emit(ParsedEvent{Kind: KindToolResult, Provider: spec.Provider, Tool: name, ToolUseID: call.CallID, Text: msg, IsError: true})
		return callResult{text: msg}
	}
	t, ok := reg.Get(name)
	if !ok {
		msg := "unknown tool: " + name
		emit(ParsedEvent{Kind: KindToolResult, Provider: spec.Provider, Tool: name, ToolUseID: call.CallID, Text: msg, IsError: true})
		return callResult{text: msg}
	}
	text, err := t.Execute(agenttools.Call{
		Ctx:         ctx,
		WorkspaceID: spec.WorkspaceID,
		AgentCode:   harnessAgentCode(spec),
		ChatCode:    spec.ChatCode,
		Worktree:    spec.Worktree,
		SessionID:   spec.RecurSessionID,
		Args:        args,
		BypassPerms: spec.PermissionMode == domain.PermissionModeBypass,
		Host:        r.Reporter,
		Permitter:   harnessPermitter{rep: r.Reporter},
		Todos:       r.Todos,
	})
	isErr := err != nil
	if isErr {
		text = err.Error()
	}
	emit(ParsedEvent{Kind: KindToolResult, Provider: spec.Provider, Tool: name, ToolUseID: call.CallID, Text: text, IsError: isErr})
	halt := !isErr && agenttools.HaltAfter(name, text)
	return callResult{text: text, halt: halt}
}

func (r *HarnessRunner) result(spec AgentSpec, start time.Time, text string, usage responses.Usage, respID string) Result {
	win := int64(0)
	if m := r.Provider.ModelByID(spec.Model); m != nil {
		win = m.ContextWindow
	}
	id := spec.RecurSessionID
	if respID != "" {
		id = respID
	}
	return Result{
		SessionID:       id,
		Subtype:         "success",
		Text:            text,
		DurationMS:      int(time.Since(start) / time.Millisecond),
		InputTokens:     responsesFreshInput(usage),
		OutputTokens:    usage.OutputTokens,
		CacheReadTokens: usage.CacheReadTokens,
		ContextWindow:   win,
	}
}

func responsesFreshInput(u responses.Usage) int64 {
	fresh := u.InputTokens - u.CacheReadTokens
	if fresh < 0 {
		return u.InputTokens
	}
	return fresh
}

type harnessPermitter struct {
	rep mcp.Reporter
}

func (p harnessPermitter) RequestPermission(ctx context.Context, workspaceID, agentCode, tool string, input json.RawMessage) (bool, string, error) {
	if p.rep == nil {
		return false, "no permission broker", nil
	}
	v, err := p.rep.RequestPermission(ctx, workspaceID, agentCode, mcp.PermissionAsk{Tool: tool, Input: input})
	if err != nil {
		return false, "", err
	}
	return v.Allow, v.Reason, nil
}
