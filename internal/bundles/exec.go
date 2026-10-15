package bundles

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"time"

	"recur/internal/domain"
)

// DefaultTimeout is the per-command wall-clock limit when the caller does not
// set a shorter deadline on ctx.
const DefaultTimeout = 30 * time.Second

const killGrace = 500 * time.Millisecond

// RunResult is the captured outcome of a shebang-script command (or a
// typescript macro under recur bundles test).
type RunResult struct {
	Stdout   string `json:"stdout"`
	Stderr   string `json:"stderr"`
	ExitCode int    `json:"exitCode"`
	Output   string `json:"output"` // resolved routing from the command
	// Language / OpenInPreview are effective values: MacroResult override when
	// set, otherwise the manifest (same resolution as the app / bundle tests).
	Language      string `json:"language,omitempty"`
	OpenInPreview *bool  `json:"openInPreview,omitempty"`
}

// Run executes cmd with rc. Native interpreter commands are not executed here —
// the frontend runs those locally.
func Run(ctx context.Context, cmd Command, rc RunContext) (RunResult, error) {
	if cmd.Interpreter == InterpreterNative || IsScriptInterpreter(cmd.Interpreter) {
		return RunResult{}, fmt.Errorf("%s command %q must run in the frontend", cmd.Interpreter, cmd.ID)
	}
	if _, ok := ctx.Deadline(); !ok {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, DefaultTimeout)
		defer cancel()
	}

	input := ResolveInput(cmd, rc)
	rc.Input = input
	if rc.BundleSupport == "" {
		rc.BundleSupport = cmd.BundleDir
	}

	// Always run from a temp copy so we never chmod the author's script file
	// (which would dirty git mode bits for shipped packs under bundles/).
	body := cmd.Body
	if body == "" {
		return RunResult{}, fmt.Errorf("command %q has empty body", cmd.ID)
	}
	if cmd.ScriptPath != "" && !hasShebang(body) {
		return RunResult{}, fmt.Errorf("command %q script missing #! shebang", cmd.ID)
	}
	if !hasShebang(body) {
		body = "#!/usr/bin/env bash\n" + body
	}
	scriptPath, err := writeTempScript("recur-cmd-*", body)
	if err != nil {
		return RunResult{}, err
	}
	defer func() { _ = os.Remove(scriptPath) }()

	workDir := rc.WorkDir
	if workDir == "" {
		workDir = rc.ProjectDirectory
	}
	if workDir == "" {
		workDir = filepath.Dir(scriptPath)
	}

	c := exec.Command(scriptPath) //nolint:gosec // user-installed / shipped bundle scripts by design
	c.Dir = workDir
	c.Env = Environ(rc)
	c.Stdin = bytes.NewBufferString(input)
	var stdout, stderr bytes.Buffer
	c.Stdout = &stdout
	c.Stderr = &stderr
	c.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}

	if err := c.Start(); err != nil {
		return RunResult{}, err
	}
	reaped := make(chan error, 1)
	go func() { reaped <- c.Wait() }()

	var waitErr error
	select {
	case waitErr = <-reaped:
	case <-ctx.Done():
		killGroup(c, reaped)
		res := runResultFromCmd(cmd, stdout.String(), stderr.String(), -1)
		return res, fmt.Errorf("command cancelled or timed out: %w", ctx.Err())
	}

	res := runResultFromCmd(cmd, stdout.String(), stderr.String(), 0)
	if waitErr != nil {
		if ee, ok := waitErr.(*exec.ExitError); ok {
			res.ExitCode = ee.ExitCode()
			return res, nil
		}
		return res, waitErr
	}
	res.ExitCode = 0
	return res, nil
}

func runResultFromCmd(cmd Command, stdout, stderr string, exitCode int) RunResult {
	oip := cmd.OpenInPreview
	return RunResult{
		Stdout:        stdout,
		Stderr:        stderr,
		ExitCode:      exitCode,
		Output:        cmd.Output,
		Language:      cmd.Language,
		OpenInPreview: &oip,
	}
}

func killGroup(cmd *exec.Cmd, reaped <-chan error) {
	if cmd.Process == nil {
		return
	}
	pgid := cmd.Process.Pid
	_ = syscall.Kill(-pgid, syscall.SIGTERM)
	select {
	case <-reaped:
	case <-time.After(killGrace):
		_ = syscall.Kill(-pgid, syscall.SIGKILL)
		<-reaped
	}
}

func hasShebang(body string) bool {
	return len(body) >= 2 && body[0] == '#' && body[1] == '!'
}

// writeTempScript writes an executable temp script. ForkLock keeps concurrent
// forks from inheriting the write fd, which makes exec fail with ETXTBSY on Linux.
func writeTempScript(pattern, body string) (string, error) {
	syscall.ForkLock.RLock()
	defer syscall.ForkLock.RUnlock()
	f, err := os.CreateTemp("", pattern)
	if err != nil {
		return "", fmt.Errorf("temp script: %w", err)
	}
	_, werr := f.WriteString(body)
	cerr := f.Close()
	if err := errors.Join(werr, cerr, os.Chmod(f.Name(), 0o700)); err != nil {
		_ = os.Remove(f.Name())
		return "", err
	}
	return f.Name(), nil
}

// PromptResult is the validated outcome of a shebang [[prompt]] action.
type PromptResult struct {
	Prompt  string `json:"prompt"`
	Display string `json:"display"`
	Skip    bool   `json:"skip,omitempty"`
	// Issue, when set, links the workspace the prompt ran in to that issue.
	Issue *domain.IssueRef `json:"issue,omitempty"`
}

// RunPromptShebang executes a shebang prompt action and parses stdout JSON
// {"prompt":"...","display":"..."}. TypeScript prompts must use CompilePromptMacro
// + the frontend Worker instead.
func RunPromptShebang(ctx context.Context, prompt PromptAction, rc PromptRunContext) (PromptResult, error) {
	if IsScriptInterpreter(prompt.Interpreter) {
		return PromptResult{}, fmt.Errorf("prompt %q is %s — use CompiledPromptModule", prompt.ID, prompt.Interpreter)
	}
	if prompt.Interpreter != InterpreterShebang && prompt.Interpreter != "" {
		return PromptResult{}, fmt.Errorf("prompt %q: unsupported interpreter %q", prompt.ID, prompt.Interpreter)
	}
	timeout := PromptTimeout
	if prompt.TimeoutMs > 0 {
		timeout = time.Duration(prompt.TimeoutMs) * time.Millisecond
	}
	if _, ok := ctx.Deadline(); !ok {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, timeout)
		defer cancel()
	}

	if rc.BundleSupport == "" {
		rc.BundleSupport = prompt.BundleDir
	}
	if rc.PromptSlash == "" {
		rc.PromptSlash = prompt.Slash
	}
	if prompt.Slash == "open-draft-pr" {
		rc.PRDraft = true
	}

	body := prompt.Body
	if body == "" {
		return PromptResult{}, fmt.Errorf("prompt %q has empty body", prompt.ID)
	}
	if prompt.ScriptPath != "" && !hasShebang(body) {
		return PromptResult{}, fmt.Errorf("prompt %q script missing #! shebang", prompt.ID)
	}
	if !hasShebang(body) {
		body = "#!/usr/bin/env bash\n" + body
	}
	scriptPath, err := writeTempScript("recur-prompt-*", body)
	if err != nil {
		return PromptResult{}, err
	}
	defer func() { _ = os.Remove(scriptPath) }()

	workDir := rc.WorkDir
	if workDir == "" {
		workDir = rc.Worktree
	}
	if workDir == "" {
		workDir = filepath.Dir(scriptPath)
	}

	c := exec.Command(scriptPath) //nolint:gosec // user-installed / shipped bundle scripts by design
	c.Dir = workDir
	c.Env = PromptEnviron(rc)
	var stdout, stderr bytes.Buffer
	c.Stdout = &stdout
	c.Stderr = &stderr
	c.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}

	if err := c.Start(); err != nil {
		return PromptResult{}, err
	}
	reaped := make(chan error, 1)
	go func() { reaped <- c.Wait() }()

	var waitErr error
	select {
	case waitErr = <-reaped:
	case <-ctx.Done():
		killGroup(c, reaped)
		return PromptResult{}, fmt.Errorf("prompt cancelled or timed out: %w", ctx.Err())
	}

	if waitErr != nil {
		if ee, ok := waitErr.(*exec.ExitError); ok {
			return PromptResult{}, fmt.Errorf(
				"prompt %q exited %d: %s",
				prompt.ID, ee.ExitCode(), strings.TrimSpace(stderr.String()),
			)
		}
		return PromptResult{}, waitErr
	}

	return parsePromptResultJSON(stdout.String(), prompt.Name)
}

func parsePromptResultJSON(stdout, fallbackName string) (PromptResult, error) {
	raw := strings.TrimSpace(stdout)
	if raw == "" {
		return PromptResult{}, fmt.Errorf("prompt stdout empty (want JSON {\"prompt\",\"display\"})")
	}
	var parsed struct {
		Prompt  string           `json:"prompt"`
		Display string           `json:"display"`
		Skip    bool             `json:"skip"`
		Issue   *domain.IssueRef `json:"issue"`
	}
	if err := json.Unmarshal([]byte(raw), &parsed); err != nil {
		return PromptResult{}, fmt.Errorf("prompt stdout is not valid JSON: %w", err)
	}
	// Empty prompt is a no-send success (e.g. /update completed via git with
	// no LLM turn). Display still falls back to the action name.
	return PromptResult{
		Prompt:  strings.TrimSpace(parsed.Prompt),
		Display: NormalizePromptDisplay(parsed.Display, fallbackName),
		Skip:    parsed.Skip,
		Issue:   parsed.Issue,
	}, nil
}

// PromptCompleteHit is one trailing-arg suggestion from a [[prompt]] complete script.
type PromptCompleteHit struct {
	Insert string `json:"insert"`
	Label  string `json:"label"`
	Psub   string `json:"psub,omitempty"`
	Detail string `json:"detail,omitempty"`
	Badge  string `json:"badge,omitempty"`
	Action string `json:"action,omitempty"`
}

// PromptCompleteResult is stdout JSON from a complete script.
type PromptCompleteResult struct {
	Hits    []PromptCompleteHit `json:"hits"`
	Message string              `json:"message,omitempty"`
}

// RunPromptCompleteShebang runs prompt.CompleteBody as a shebang and parses
// {"hits":[...],"message"?}. TypeScript complete scripts use CompileCompleteMacro.
func RunPromptCompleteShebang(ctx context.Context, prompt PromptAction, rc PromptRunContext) (PromptCompleteResult, error) {
	if prompt.CompleteInterpreter != InterpreterShebang && prompt.CompleteInterpreter != "" {
		return PromptCompleteResult{}, fmt.Errorf("prompt %q complete is %s — use CompiledCompleteModule", prompt.ID, prompt.CompleteInterpreter)
	}
	if strings.TrimSpace(prompt.CompleteBody) == "" {
		return PromptCompleteResult{}, fmt.Errorf("prompt %q has no complete script", prompt.ID)
	}
	timeout := PromptCompleteTimeout
	if _, ok := ctx.Deadline(); !ok {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, timeout)
		defer cancel()
	}
	if rc.BundleSupport == "" {
		rc.BundleSupport = prompt.BundleDir
	}
	if rc.PromptSlash == "" {
		rc.PromptSlash = prompt.Slash
	}
	body := prompt.CompleteBody
	if prompt.CompletePath != "" && !hasShebang(body) {
		return PromptCompleteResult{}, fmt.Errorf("prompt %q complete script missing #! shebang", prompt.ID)
	}
	if !hasShebang(body) {
		body = "#!/usr/bin/env bash\n" + body
	}
	scriptPath, err := writeTempScript("recur-prompt-complete-*", body)
	if err != nil {
		return PromptCompleteResult{}, err
	}
	defer func() { _ = os.Remove(scriptPath) }()

	workDir := rc.WorkDir
	if workDir == "" {
		workDir = rc.Worktree
	}
	if workDir == "" {
		workDir = filepath.Dir(scriptPath)
	}

	c := exec.Command(scriptPath) //nolint:gosec // user-installed / shipped bundle scripts by design
	c.Dir = workDir
	c.Env = PromptEnviron(rc)
	var stdout, stderr bytes.Buffer
	c.Stdout = &stdout
	c.Stderr = &stderr
	c.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}

	if err := c.Start(); err != nil {
		return PromptCompleteResult{}, err
	}
	reaped := make(chan error, 1)
	go func() { reaped <- c.Wait() }()

	var waitErr error
	select {
	case waitErr = <-reaped:
	case <-ctx.Done():
		killGroup(c, reaped)
		return PromptCompleteResult{}, fmt.Errorf("complete cancelled or timed out: %w", ctx.Err())
	}
	if waitErr != nil {
		if ee, ok := waitErr.(*exec.ExitError); ok {
			return PromptCompleteResult{}, fmt.Errorf(
				"prompt %q complete exited %d: %s",
				prompt.ID, ee.ExitCode(), strings.TrimSpace(stderr.String()),
			)
		}
		return PromptCompleteResult{}, waitErr
	}
	return parsePromptCompleteJSON(stdout.String())
}

func parsePromptCompleteJSON(stdout string) (PromptCompleteResult, error) {
	raw := strings.TrimSpace(stdout)
	if raw == "" {
		return PromptCompleteResult{Hits: []PromptCompleteHit{}}, nil
	}
	var parsed PromptCompleteResult
	if err := json.Unmarshal([]byte(raw), &parsed); err != nil {
		return PromptCompleteResult{}, fmt.Errorf("complete stdout is not valid JSON: %w", err)
	}
	out := make([]PromptCompleteHit, 0, len(parsed.Hits))
	for _, h := range parsed.Hits {
		h.Insert = strings.TrimSpace(h.Insert)
		h.Label = strings.TrimSpace(h.Label)
		h.Psub = strings.TrimSpace(h.Psub)
		h.Detail = strings.TrimSpace(h.Detail)
		h.Badge = strings.TrimSpace(h.Badge)
		h.Action = strings.TrimSpace(h.Action)
		if h.Label == "" {
			h.Label = h.Insert
		}
		if h.Label == "" && h.Action == "" {
			continue
		}
		out = append(out, h)
		if len(out) >= PromptCompleteMaxHits {
			break
		}
	}
	return PromptCompleteResult{
		Hits:    out,
		Message: strings.TrimSpace(parsed.Message),
	}, nil
}

// RunSearchShebang runs a [[search]] shebang. Same stdout as prompt complete.
func RunSearchShebang(ctx context.Context, search SearchAction, rc PromptRunContext) (PromptCompleteResult, error) {
	return RunPromptCompleteShebang(ctx, PromptAction{
		ID:                  search.ID,
		Name:                search.Name,
		CompleteInterpreter: InterpreterShebang,
		CompleteBody:        search.Body,
		CompletePath:        search.ScriptPath,
		BundleDir:           search.BundleDir,
	}, rc)
}

// RunExpandShebang runs a [[expand]] shebang. Same stdout as [[prompt]] invoke.
func RunExpandShebang(ctx context.Context, expand ExpandAction, rc PromptRunContext) (PromptResult, error) {
	ms := expand.TimeoutMs
	if ms <= 0 {
		ms = int(ExpandTimeout / time.Millisecond)
	}
	return RunPromptShebang(ctx, PromptAction{
		ID:          expand.ID,
		Name:        expand.Name,
		Interpreter: InterpreterShebang,
		Body:        expand.Body,
		ScriptPath:  expand.ScriptPath,
		BundleDir:   expand.BundleDir,
		TimeoutMs:   ms,
	}, rc)
}

// PaletteSearchGroup is one ⌘K section from CollectSearchGroups.
type PaletteSearchGroup struct {
	Name    string
	IconSVG string
	Hits    []PromptCompleteHit
}

// CollectSearchGroups runs every [[search]] concurrently. A failed or timed-out
// pack contributes an empty hit list; the rest still return. makeRC may be nil.
func CollectSearchGroups(ctx context.Context, searches []SearchAction, makeRC func(SearchAction) PromptRunContext) []PaletteSearchGroup {
	slots := make([]PaletteSearchGroup, len(searches))
	var wg sync.WaitGroup
	for i, se := range searches {
		wg.Add(1)
		go func(i int, se SearchAction) {
			defer wg.Done()
			var rc PromptRunContext
			if makeRC != nil {
				rc = makeRC(se)
			}
			cctx, cancel := context.WithTimeout(ctx, se.timeout())
			defer cancel()
			res, err := RunSearchShebang(cctx, se, rc)
			g := PaletteSearchGroup{Name: se.Name, IconSVG: se.IconSVG}
			if err == nil {
				g.Hits = res.Hits
			}
			slots[i] = g
		}(i, se)
	}
	wg.Wait()
	return slots
}

// ExpandMatching runs the first matching expander that returns a non-skip prompt.
// No match (or every match skips) returns the original text with no error.
// A matched expander that errors is returned so the host can toast.
func ExpandMatching(text string, expands []ExpandAction, makeRC func(ExpandAction) PromptRunContext) (PromptResult, error) {
	trimmed := strings.TrimSpace(text)
	for _, ex := range expands {
		if !ex.Matches(trimmed) {
			continue
		}
		var rc PromptRunContext
		if makeRC != nil {
			rc = makeRC(ex)
		}
		if strings.TrimSpace(rc.Args) == "" {
			rc.Args = trimmed
		}
		ctx, cancel := context.WithTimeout(context.Background(), ex.timeout())
		res, err := RunExpandShebang(ctx, ex, rc)
		cancel()
		if err != nil {
			return PromptResult{}, err
		}
		if res.Skip || strings.TrimSpace(res.Prompt) == "" {
			continue
		}
		return res, nil
	}
	return PromptResult{Prompt: trimmed}, nil
}
