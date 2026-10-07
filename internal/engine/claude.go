package engine

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"syscall"
	"time"
)

// claudeKillGrace is how long we wait after SIGTERM before SIGKILLing the agent's
// process group on context cancel (matches the runconfig teardown budget).
const claudeKillGrace = 2 * time.Second

// ClaudeRunner is the PRODUCTION AgentRunner: it spawns `claude -p` with
// stream-json output, parses each NDJSON line into typed events, and returns the
// final Result (headless ref §1/§3/§8). It runs in the loop's worktree, sets the
// child in its own process group, and kills the whole GROUP on ctx cancel using
// the close-broadcast reap pattern proven in internal/runconfig (SIGTERM → grace
// → SIGKILL; exactly one cmd.Wait()).
//
// Per DECISIONS D8 it does NOT pass --bare, so the user's Claude Code login works.
// Tool scopes come from the role table (roles.go); permission mode from AgentSpec.
//
// When AgentSpec.Inject is set (specialist turns), the runner also opens
// `--input-format stream-json` stdin: the initial Prompt is the first user
// message, and mid-turn Maestro steers are written as additional user messages
// on the next tool_result turn (D49 — prompt-cache safe).
type ClaudeRunner struct {
	// Bin is the claude executable; "" defaults to "claude" on PATH.
	Bin string
}

// NewClaudeRunner returns the production runner.
func NewClaudeRunner() *ClaudeRunner { return &ClaudeRunner{} }

// NeedsPermissionPromptTool reports that Claude Code requires
// permission_request advertised on the MCP binding for --permission-prompt-tool
// (docs/claude-headless-reference.md §1a).
func (*ClaudeRunner) NeedsPermissionPromptTool() bool { return true }

// Run spawns claude for one turn, streams parsed events to emit, and returns the
// final Result. The returned error is non-nil if the process failed to start or
// exited non-zero before producing a `result` line; a `result` line with an error
// subtype is reported via the Result (IsError), not as a Go error.
func (r *ClaudeRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	bin := r.Bin
	if bin == "" {
		bin = "claude"
	}

	// Stream-json stdin is required for mid-turn steers (Inject), image
	// content blocks (Images), and oversized prompts (PromptOnStdin) — -p
	// can only carry a plain string, and a large argv hits ARG_MAX.
	useInject := spec.Inject != nil || len(spec.Images) > 0 || spec.PromptOnStdin

	// We manage cancellation ourselves (process-group kill) rather than letting
	// exec.CommandContext SIGKILL only the leader, which would orphan children.
	cmd := exec.Command(bin, r.args(spec)...) //nolint:gosec // argv built from app-owned spec
	cmd.Dir = spec.Worktree
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	cmd.Env = withoutLinearAPIKey(os.Environ())

	// Capture stderr. The CLI reports usage/spend limits, auth problems and provider
	// outages there, and discarding it left a spend-limit failure surfacing as a bare
	// "error_during_execution" with no way to tell a transient limit from a real defect
	// (D56). Bounded so a chatty run cannot grow this without limit.
	var errTail tailBuffer
	cmd.Stderr = &errTail

	var stdin io.WriteCloser
	if useInject {
		w, err := cmd.StdinPipe()
		if err != nil {
			return Result{}, fmt.Errorf("claude stdin pipe: %w", err)
		}
		stdin = w
	}

	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return Result{}, fmt.Errorf("claude stdout pipe: %w", err)
	}

	if err := cmd.Start(); err != nil {
		return Result{}, fmt.Errorf("start claude: %w", err)
	}

	// Sole owner of cmd.Wait(): a single reaper goroutine waits and close-broadcasts
	// so both the cancel watcher and the main path observe the exit without racing a
	// second wait4 (the deadlock/double-Wait bug we must not reintroduce).
	reaped := make(chan struct{})
	var waitErr error
	go func() { waitErr = cmd.Wait(); close(reaped) }()

	// Cancel watcher: on ctx cancel, terminate the whole group (SIGTERM → grace →
	// SIGKILL). It only signals + observes reaped; it never calls Wait itself.
	watchDone := make(chan struct{})
	go func() {
		defer close(watchDone)
		select {
		case <-reaped:
			// Process exited on its own; nothing to kill.
		case <-ctx.Done():
			killGroup(cmd, reaped)
		}
	}()

	// Bidirectional mode: seed the initial Prompt as a user message, then keep
	// stdin open for mid-turn steers. Close stdin when the turn ends so a
	// one-shot specialist doesn't hang waiting for more input.
	if useInject {
		if werr := writeStreamUserMessage(stdin, spec.Prompt, spec.Images); werr != nil {
			_ = stdin.Close()
			<-reaped
			<-watchDone
			return Result{}, fmt.Errorf("claude seed prompt: %w", werr)
		}
	}

	var final Result
	var haveResult bool
	if useInject {
		final, haveResult = r.runInjecting(ctx, stdin, stdout, spec, emit)
	} else {
		final, haveResult = r.runStdoutOnly(stdout, emit)
	}

	// stdout closed → process is finishing; wait for the reaper, then settle the
	// cancel watcher so no goroutine leaks past Run.
	<-reaped
	<-watchDone

	if ctx.Err() != nil {
		return final, ctx.Err()
	}
	// Attach whatever the CLI said on stderr to the Result so the caller can tell a
	// transient limit/outage from a genuine failure (D56). (The scan-error check that
	// used to sit here went away with main's runInjecting/runStdoutOnly refactor, which
	// reports only (final, haveResult) — a read failure now surfaces as "no result".)
	final.Stderr = errTail.String()
	if !haveResult {
		// No result line: report the process error (non-zero exit) if any, including the
		// stderr tail — this is the shape a hard usage-limit refusal takes.
		detail := strings.TrimSpace(final.Stderr)
		if waitErr != nil {
			if detail != "" {
				return final, fmt.Errorf("claude exited without a result: %w: %s", waitErr, detail)
			}
			return final, fmt.Errorf("claude exited without a result: %w", waitErr)
		}
		if detail != "" {
			return final, fmt.Errorf("claude produced no result line: %s", detail)
		}
		return final, fmt.Errorf("claude produced no result line")
	}
	return final, nil
}

// tailBuffer keeps only the LAST tailBufferMax bytes written to it — enough to carry a
// provider error message out of a subprocess without letting a noisy run accumulate
// unbounded memory. Writes come from the exec package's stderr copier goroutine, so
// they are serialized with respect to Wait; String is read after the reaper observes
// the exit.
type tailBuffer struct {
	buf []byte
}

const tailBufferMax = 8 * 1024

func (t *tailBuffer) Write(p []byte) (int, error) {
	t.buf = append(t.buf, p...)
	if len(t.buf) > tailBufferMax {
		t.buf = t.buf[len(t.buf)-tailBufferMax:]
	}
	return len(p), nil
}

func (t *tailBuffer) String() string { return string(t.buf) }

// runStdoutOnly is the classic one-shot path: parse stdout, no stdin.
func (r *ClaudeRunner) runStdoutOnly(stdout io.Reader, emit func(ParsedEvent)) (Result, bool) {
	var final Result
	haveResult := false
	var ctxUsage contextUsageTracker
	sc := bufio.NewScanner(stdout)
	sc.Buffer(make([]byte, 0, 64*1024), 8*1024*1024)
	for sc.Scan() {
		evs, perr := parseStreamLine(sc.Bytes())
		if perr != nil {
			continue
		}
		for _, ev := range evs {
			// Meter: last assistant.message.usage over cumulative result.usage.
			ctxUsage.observe(&ev)
			if ev.Kind == KindResult && ev.Result != nil {
				final = *ev.Result
				haveResult = true
			}
			// Session mirroring happens in the mirroringRunner's wrapped emit (see
			// mirror.go); here we only forward parsed events to emit.
			if emit != nil {
				emit(ev)
			}
		}
	}
	return final, haveResult
}

// runInjecting reads stdout on a side goroutine and multiplexes mid-turn steers
// onto stdin. A steer is flushed as a user message on the next tool_result (the
// natural next "user" turn), preserving the cached prompt prefix (D49). Steers
// that arrive after at least one tool_result has already been seen flush
// immediately. A loop steer that never finds a tool_result slot is left on the
// loop's pendingSteer for the next newly spawned agent. Workspace InjectPlain
// steers flush on KindResult instead of dropping, so a follow-up sent before
// the first tool still reaches Claude as the next stdin user message.
//
// CRITICAL: with `--input-format stream-json`, claude stays alive after a `result`
// waiting for another user message. We MUST close stdin on KindResult (once no
// InjectPlain payload is still pending) so the process exits; otherwise Run
// deadlocks (stdout never closes ↔ stdin never closes). After closing, we drain
// remaining stdout until the process exits.
func (r *ClaudeRunner) runInjecting(
	ctx context.Context,
	stdin io.WriteCloser,
	stdout io.Reader,
	spec AgentSpec,
	emit func(ParsedEvent),
) (Result, bool) {
	inject := spec.Inject
	lines := make(chan []byte, 16)
	go func() {
		defer close(lines)
		sc := bufio.NewScanner(stdout)
		sc.Buffer(make([]byte, 0, 64*1024), 8*1024*1024)
		for sc.Scan() {
			// Copy the line — Scanner reuses its buffer.
			line := append([]byte(nil), sc.Bytes()...)
			select {
			case lines <- line:
			case <-ctx.Done():
				return
			}
		}
	}()

	var final Result
	haveResult := false
	seenToolResult := false
	var pending []string
	stdinOpen := true
	bgTasks := 0
	var held *ParsedEvent
	var holdCeiling <-chan time.Time
	var ctxUsage contextUsageTracker
	closeStdin := func() {
		if stdinOpen {
			stdinOpen = false
			_ = stdin.Close()
		}
	}
	defer closeStdin()

	// Workspace InjectPlain acks only after a successful stdin write. Loop
	// steers ack on enqueue so an unflushed payload can land on pendingSteer.
	delayAck := spec.InjectPlain
	flushPending := func() {
		for _, raw := range pending {
			text := injectStdinText(spec, raw)
			ok := false
			if text != "" && stdinOpen {
				if err := writeStreamUserMessage(stdin, text, nil); err == nil {
					emitInjectUser(spec, emit, raw)
					ok = true
				}
			}
			if delayAck {
				ackInject(spec, ok)
			}
		}
		pending = nil
	}
	rejectPending := func() {
		if !delayAck {
			return
		}
		for range pending {
			ackInject(spec, false)
		}
		pending = nil
	}

	for {
		select {
		case <-ctx.Done():
			rejectPending()
			return final, haveResult
		case <-holdCeiling:
			closeStdin()
		case msg := <-inject:
			if !stdinOpen {
				ackInject(spec, false)
				continue
			}
			pending = append(pending, msg)
			if !delayAck {
				ackInject(spec, true)
			}
			if seenToolResult {
				flushPending()
			}
		case line, ok := <-lines:
			if !ok {
				// Process exited — an unflushed loop pendingInject stays on
				// loop.pendingSteer. Workspace InjectPlain must nack instead.
				rejectPending()
				if held != nil && emit != nil {
					emit(*held)
				}
				return final, haveResult
			}
			if n, ok := backgroundTaskCount(line); ok {
				bgTasks = n
				if n == 0 && held != nil {
					closeStdin()
				}
			}
			evs, perr := parseStreamLine(line)
			if perr != nil {
				continue
			}
			for _, ev := range evs {
				if ev.Kind == KindToolResult {
					seenToolResult = true
					flushPending()
				}
				// Meter: last assistant.message.usage over cumulative result.usage.
				ctxUsage.observe(&ev)
				if ev.Kind == KindResult && ev.Result != nil {
					if spec.InjectPlain && len(pending) > 0 {
						flushPending()
						continue
					}
					final = *ev.Result
					haveResult = true
					// On stdin EOF claude kills background shells after a short
					// grace. While tasks are live, stay open (up to the ceiling):
					// claude runs a follow-up turn on their task_notification and
					// emits another result. A held result is emitted only if none
					// follows before exit.
					if bgTasks > 0 && stdinOpen {
						held = &ev
						if holdCeiling == nil {
							t := time.NewTimer(bgTaskHoldCeiling)
							defer t.Stop()
							holdCeiling = t.C
						}
						continue
					}
					held = nil
					// End the one-shot turn: closing stdin lets claude exit instead
					// of waiting forever for another stream-json user message.
					closeStdin()
				}
				// Event mirroring happens in the wrapped emit (mirror.go); raw lines
				// are mirrored above.
				if emit != nil {
					emit(ev)
				}
			}
		}
	}
}

// bgTaskHoldCeiling bounds how long a result is held for live background
// tasks (a dev server never finishes). Matches claude's own print-mode
// background wait ceiling.
var bgTaskHoldCeiling = 10 * time.Minute

// backgroundTaskCount reads claude's system/background_tasks_changed frame,
// which carries the full list of still-running background tasks. Ambient
// entries (housekeeping, auto-started watchers) are not activity.
func backgroundTaskCount(line []byte) (int, bool) {
	if !bytes.Contains(line, []byte(`"background_tasks_changed"`)) {
		return 0, false
	}
	var msg struct {
		Type    string `json:"type"`
		Subtype string `json:"subtype"`
		Tasks   []struct {
			Ambient bool `json:"ambient"`
		} `json:"tasks"`
	}
	if json.Unmarshal(line, &msg) != nil || msg.Type != "system" || msg.Subtype != "background_tasks_changed" {
		return 0, false
	}
	n := 0
	for _, t := range msg.Tasks {
		if !t.Ambient {
			n++
		}
	}
	return n, true
}

// writeStreamUserMessage writes one NDJSON user turn to a stream-json stdin
// (headless ref §4). Plain text when there are no images; otherwise a
// content-block array with text/image parts interleaved at attachment markers.
func writeStreamUserMessage(w io.Writer, content string, imgs []UserImage) error {
	payload := struct {
		Type            string `json:"type"`
		Message         any    `json:"message"`
		ParentToolUseID any    `json:"parent_tool_use_id"`
	}{
		Type: "user",
		Message: map[string]any{
			"role":    "user",
			"content": claudeUserContent(content, imgs),
		},
		ParentToolUseID: nil,
	}
	b, err := json.Marshal(payload)
	if err != nil {
		return err
	}
	b = append(b, '\n')
	_, err = w.Write(b)
	return err
}

// claudeUserContent builds the stream-json user message content: a plain string
// when there are no images, otherwise an Anthropic content-block array.
func claudeUserContent(text string, imgs []UserImage) any {
	if len(imgs) == 0 {
		return text
	}
	byID := map[string]UserImage{}
	for _, img := range imgs {
		byID[img.ID] = img
	}
	if !attachmentMarkerRE.MatchString(text) {
		// No markers — send text (if any) then every image.
		var blocks []any
		if strings.TrimSpace(text) != "" {
			blocks = append(blocks, map[string]any{"type": "text", "text": text})
		}
		for _, img := range imgs {
			blocks = append(blocks, claudeImageBlock(img))
		}
		return prependClaudeAttachmentIDs(text, imgs, blocks)
	}
	var blocks []any
	last := 0
	seen := map[string]bool{}
	for _, loc := range attachmentMarkerRE.FindAllStringSubmatchIndex(text, -1) {
		if loc[0] > last {
			if seg := text[last:loc[0]]; seg != "" {
				blocks = append(blocks, map[string]any{"type": "text", "text": seg})
			}
		}
		id := text[loc[2]:loc[3]]
		if img, ok := byID[id]; ok {
			blocks = append(blocks, claudeImageBlock(img))
			seen[id] = true
		} else {
			blocks = append(blocks, map[string]any{"type": "text", "text": text[loc[0]:loc[1]]})
		}
		last = loc[1]
	}
	if last < len(text) {
		if seg := text[last:]; seg != "" {
			blocks = append(blocks, map[string]any{"type": "text", "text": seg})
		}
	}
	for _, img := range imgs {
		if !seen[img.ID] {
			blocks = append(blocks, claudeImageBlock(img))
		}
	}
	if len(blocks) == 0 {
		return text
	}
	return prependClaudeAttachmentIDs(text, imgs, blocks)
}

func claudeImageBlock(img UserImage) map[string]any {
	return map[string]any{
		"type": "image",
		"source": map[string]any{
			"type":       "base64",
			"media_type": img.Mime,
			"data":       img.Data,
		},
	}
}

// args builds the `claude -p` argv from a spec (headless ref §1, §8; DECISIONS D8).
//
//	claude -p <prompt> --output-format stream-json --verbose
//	  [--model m] [--append-system-prompt p] [--allowedTools a,b,c]
//	  [--disallowedTools …] [--tools …] [--max-turns n]
//	  --disable-slash-commands --permission-mode dontAsk|bypassPermissions
//
// When Inject is set, the Prompt is NOT passed as a -p argument — it is written
// as the first stdin user message under `--input-format stream-json` so mid-turn
// steers can share the same stdin (D49).
//
// --verbose is mandatory with stream-json (headless ref §1). PermissionMode is
// machine-global (Settings.BypassPermissions) and applies to every turn; see
// permissionArgs for how it and --permission-prompt-tool are emitted. When
// AllowedTools omits Edit/Write/NotebookEdit, --disallowedTools removes them
// from context so bypassPermissions cannot resurrect edit tools (headless ref
// §1). AskUserQuestion and Skill are denied whenever builtins may be visible
// (even with no allowlist) — headless -p cannot collect answers, and Recur owns
// skill loading. Skipped only when Tools is "" (no builtins at all).
// When spec.MCPURL is set, the Recur MCP is wired via --mcp-config (inline
// JSON) + --strict-mcp-config so ONLY the app's MCP is loaded (no ambient
// .mcp.json), and the role's mcp__recur__* tools are already in
// spec.AllowedTools (headless ref §5; acceptEdits/dontAsk do NOT auto-approve
// MCP, so the explicit allowedTools entry is required).
func (r *ClaudeRunner) args(spec AgentSpec) []string {
	var args []string
	if spec.Inject != nil || len(spec.Images) > 0 || spec.PromptOnStdin {
		// Bidirectional / multimodal / large prompt: Prompt (+ images) go on
		// stdin; keep -p for non-interactive mode without a string prompt argument.
		args = []string{"-p", "--output-format", "stream-json", "--input-format", "stream-json", "--verbose"}
	} else {
		args = []string{"-p", spec.Prompt, "--output-format", "stream-json", "--verbose"}
	}
	// Recur owns skill catalog/activation (list_skills / activate_skill).
	// --disable-slash-commands drops Claude's native skill slash expansion;
	// --disallowedTools Skill (below) removes the Skill tool from context.
	// Independent of --bare (D8): OAuth/keychain and CLAUDE.md still load.
	args = append(args, "--disable-slash-commands")
	if spec.IncludePartials {
		// Token-level text deltas for Maestro chat streaming (headless ref §3).
		args = append(args, "--include-partial-messages")
	}
	if spec.ResumeSession != "" {
		// Multi-turn continuity: continue the prior session (headless ref §8.1, the
		// Maestro chat, PRD §8.5). --resume goes before the role flags.
		args = append(args, "--resume", spec.ResumeSession)
		if spec.ForkSession {
			args = append(args, "--fork-session")
		}
	}
	if spec.Model != "" {
		args = append(args, "--model", ResolveClaudeModel(spec.Model, spec.Flags))
	}
	if effort := ResolveClaudeEffort(spec.Flags); effort != "" {
		args = append(args, "--effort", effort)
	}
	// Surface the model's reasoning: without --thinking-display, headless claude
	// REDACTS thinking (empty text + signature, token estimates only). adaptive
	// lets the model decide when to think; summarized streams readable thinking
	// text (thinking_delta) and includes it in the complete assistant message, so
	// the session mirror persists it. The parser still degrades gracefully to
	// token-estimate rows if a CLI/model redacts anyway. SkipThinking opts out for
	// cheap one-shot completions that shouldn't pay for reasoning tokens.
	if !spec.SkipThinking {
		args = append(args, "--thinking", "adaptive", "--thinking-display", "summarized")
	}
	if ap := joinAppendPrompt(joinAppendPrompt(spec.SystemPrompt, spec.AppendSystemPrompt), spec.TurnAppendPrompt); ap != "" {
		args = append(args, "--append-system-prompt", ap)
	}
	if len(spec.AllowedTools) > 0 {
		args = append(args, "--allowedTools", strings.Join(spec.AllowedTools, ","))
	}
	// Bare-name disallowedTools removes tools from context (headless ref).
	// AskUserQuestion and Skill are always in the deny list when builtins may
	// be visible; omitted edit builtins join them when an allowlist is present.
	// --tools "" disables the whole toolbox, so no deny list is needed.
	toolsDisabled := spec.Tools != nil && *spec.Tools == ""
	if !toolsDisabled {
		if deny := disallowedClaudeTools(spec.AllowedTools); len(deny) > 0 {
			args = append(args, "--disallowedTools", strings.Join(deny, ","))
		}
	}
	// --tools sets the complete built-in toolbox the model can see (distinct from
	// --allowedTools auto-approval). An explicit empty string disables all tools.
	if spec.Tools != nil {
		args = append(args, "--tools", *spec.Tools)
	}
	if spec.MaxTurns > 0 {
		args = append(args, "--max-turns", strconv.Itoa(spec.MaxTurns))
	}
	if spec.MCPURL != "" || spec.Playwright || len(spec.UserMCP) > 0 {
		args = append(args, "--mcp-config", mcpConfigJSON(spec), "--strict-mcp-config")
	}
	args = append(args, permissionArgs(spec)...)
	return args
}

// mcpConfigJSON builds the inline --mcp-config value (headless ref §5, D7). It
// always wires the Recur MCP over HTTP when url is non-empty
// ({"recur":{"type":"http","url":...}}); Playwright is a stdio server; UserMCP
// entries are extra HTTP (headers) or stdio servers from Settings → MCP.
// --strict-mcp-config means ONLY the listed servers load (no ambient .mcp.json).
func mcpConfigJSON(spec AgentSpec) string {
	servers := map[string]any{}
	if spec.MCPURL != "" {
		servers["recur"] = map[string]any{"type": "http", "url": spec.MCPURL}
	}
	if spec.Playwright {
		servers["playwright"] = playwrightMCPServer()
	}
	for _, u := range spec.UserMCP {
		name := strings.TrimSpace(u.Name)
		if name == "" {
			continue
		}
		if u.URL != "" {
			entry := map[string]any{"type": "http", "url": u.URL}
			if len(u.Headers) > 0 {
				entry["headers"] = u.Headers
			}
			servers[name] = entry
			continue
		}
		if u.Command != "" {
			entry := map[string]any{"command": u.Command}
			if len(u.Args) > 0 {
				entry["args"] = u.Args
			}
			if len(u.Env) > 0 {
				entry["env"] = u.Env
			}
			servers[name] = entry
		}
	}
	if len(servers) == 0 {
		return ""
	}
	b, err := json.Marshal(map[string]any{"mcpServers": servers})
	if err != nil {
		return ""
	}
	return string(b)
}

// playwrightMCPServer is the stdio MCP-config entry for Microsoft's official
// Playwright MCP server (PRD §10, headless ref §5). The current correct package is
// `@playwright/mcp` (npm, scope @playwright, maintained by the Playwright team);
// `npx -y @playwright/mcp@latest` fetches and runs it on demand, so no global
// install is required and the version stays current. Its tools name as
// mcp__playwright__* (the QA role allow-lists them via the mcp__playwright__*
// wildcard). In production (the built app always uses ClaudeRunner) this launches a
// real browser via the Playwright MCP during the verify phase. In the default test
// run the ScriptedRunner is injected instead, so this config is exercised only for
// SHAPE (tests assert it is present + correct); the opt-in RECUR_E2E smoke and a
// real app run drive an actual browser.
func playwrightMCPServer() map[string]any {
	return map[string]any{
		"command": "npx",
		"args":    []string{"-y", "@playwright/mcp@latest"},
	}
}

// killGroup terminates cmd's whole process group (SIGTERM, then SIGKILL after a
// grace) and blocks until the reaper observes the exit. It never calls Wait
// itself — the reaper owns it — and because reaped is closed (not sent on),
// receiving returns immediately and repeatably, so this never deadlocks. Mirrors
// the proven internal/runconfig.killGroup.
func killGroup(cmd *exec.Cmd, reaped <-chan struct{}) {
	if cmd.Process == nil {
		return
	}
	pgid := cmd.Process.Pid // Setpgid → pgid == pid
	_ = syscall.Kill(-pgid, syscall.SIGTERM)
	select {
	case <-reaped:
	case <-time.After(claudeKillGrace):
		_ = syscall.Kill(-pgid, syscall.SIGKILL)
		<-reaped
	}
}
