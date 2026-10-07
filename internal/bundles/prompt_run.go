package bundles

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"time"
)

// RunPrompt dispatches a [[prompt]] by interpreter. Shebang runs in-process;
// TypeScript compiles to CJS and executes with node (the Worker twin — the
// loop goroutine must not call the webview Worker).
func RunPrompt(ctx context.Context, cacheDir string, prompt PromptAction, rc PromptRunContext) (PromptResult, error) {
	if IsScriptInterpreter(prompt.Interpreter) {
		return RunPromptTypeScript(ctx, cacheDir, prompt, rc)
	}
	return RunPromptShebang(ctx, prompt, rc)
}

// nodePromptBoot mirrors frontend/src/lib/macro-worker.ts PROMPT_WORKER_BOOT.
const nodePromptBoot = `
const fs = require("fs");
const codePath = process.argv[2];
const ctxPath = process.argv[3];
const fallbackName = process.argv[4] || "";
const code = fs.readFileSync(codePath, "utf8");
const ctx = JSON.parse(fs.readFileSync(ctxPath, "utf8"));
(async () => {
  const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
  const runner = new AsyncFunction(
    "ctx",
    "const module = { exports: {} };\n" +
      "const exports = module.exports;\n" +
      String(code) +
      "\n" +
      "const __fn = module.exports.default || module.exports;\n" +
      "if (typeof __fn !== 'function') throw new Error('prompt must export default function (ctx)');\n" +
      "return await __fn(ctx);"
  );
  const frozen = Object.freeze(Object.assign({}, ctx));
  const raw = await runner(frozen);
  if (!raw || typeof raw !== "object") {
    throw new Error("prompt must return { prompt, display? }");
  }
  const prompt = raw.prompt != null ? String(raw.prompt) : "";
  process.stdout.write(JSON.stringify({ prompt: prompt, display: raw.display != null ? String(raw.display) : "" }));
})().catch((err) => {
  const message = err && err.message ? String(err.message) : String(err);
  process.stderr.write(message);
  process.exit(1);
});
`

// RunPromptTypeScript compiles a typescript prompt and runs it with node.
func RunPromptTypeScript(ctx context.Context, cacheDir string, prompt PromptAction, rc PromptRunContext) (PromptResult, error) {
	if !IsScriptInterpreter(prompt.Interpreter) {
		return PromptResult{}, fmt.Errorf("prompt %q is not typescript", prompt.ID)
	}
	compiled, err := CompilePromptMacro(cacheDir, prompt)
	if err != nil {
		return PromptResult{}, err
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

	dir, err := os.MkdirTemp("", "recur-prompt-ts-*")
	if err != nil {
		return PromptResult{}, err
	}
	defer os.RemoveAll(dir)

	codePath := filepath.Join(dir, "prompt.js")
	ctxPath := filepath.Join(dir, "ctx.json")
	bootPath := filepath.Join(dir, "boot.js")
	if err := os.WriteFile(codePath, []byte(compiled.JS), 0o644); err != nil {
		return PromptResult{}, err
	}
	if err := os.WriteFile(bootPath, []byte(nodePromptBoot), 0o644); err != nil {
		return PromptResult{}, err
	}
	ctxJSON, err := json.Marshal(promptNodeCtx(rc))
	if err != nil {
		return PromptResult{}, err
	}
	if err := os.WriteFile(ctxPath, ctxJSON, 0o644); err != nil {
		return PromptResult{}, err
	}

	c := exec.CommandContext(ctx, "node", bootPath, codePath, ctxPath, prompt.Name)
	var stdout, stderr bytes.Buffer
	c.Stdout = &stdout
	c.Stderr = &stderr
	if err := c.Run(); err != nil {
		if ctx.Err() != nil {
			return PromptResult{}, fmt.Errorf("prompt cancelled or timed out: %w", ctx.Err())
		}
		if ee, ok := err.(*exec.ExitError); ok {
			return PromptResult{}, fmt.Errorf("prompt %q exited %d: %s", prompt.ID, ee.ExitCode(), stderr.String())
		}
		return PromptResult{}, fmt.Errorf("node: %w (is node on PATH?)", err)
	}
	return parsePromptResultJSON(stdout.String(), prompt.Name)
}

func promptNodeCtx(rc PromptRunContext) map[string]any {
	tab := rc.TabSize
	if tab == 0 {
		tab = 4
	}
	fileName := rc.FileName
	if fileName == "" && rc.FilePath != "" {
		fileName = filepath.Base(rc.FilePath)
	}
	return map[string]any{
		"workspaceId":  rc.WorkspaceID,
		"projectId":    rc.ProjectID,
		"worktree":     rc.Worktree,
		"port":         rc.Port,
		"selectedText": rc.SelectedText,
		"document":     rc.Document,
		"currentLine":  rc.CurrentLine,
		"lineNumber":   rc.LineNumber,
		"column":       rc.Column,
		"currentWord":  rc.CurrentWord,
		"filepath":     rc.FilePath,
		"filename":     fileName,
		"scope":        rc.Scope,
		"clipboard":    rc.Clipboard,
		"tabSize":      tab,
		"softTabs":     rc.SoftTabs,
	}
}
