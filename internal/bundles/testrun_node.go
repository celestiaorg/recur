package bundles

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
)

// nodeMacroBoot mirrors frontend/src/lib/macro-worker.ts WORKER_BOOT so CLI
// tests exercise the same CJS + default-export contract as the app Worker.
const nodeMacroBoot = `
const fs = require("fs");
const codePath = process.argv[2];
const ctxPath = process.argv[3];
const code = fs.readFileSync(codePath, "utf8");
const ctx = JSON.parse(fs.readFileSync(ctxPath, "utf8"));
(async () => {
  const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
  // In this Go raw string, \n is two chars (backslash + n) so the written JS
  // source has real escape sequences — same shape as macro-worker WORKER_BOOT.
  const runner = new AsyncFunction(
    "ctx",
    "const module = { exports: {} };\n" +
      "const exports = module.exports;\n" +
      String(code) +
      "\n" +
      "const __fn = module.exports.default || module.exports;\n" +
      "if (typeof __fn !== 'function') throw new Error('macro must export default function (ctx)');\n" +
      "return await __fn(ctx);"
  );
  const frozen = Object.freeze(
    Object.assign({}, ctx, {
      prompts: Object.freeze(Object.assign({}, (ctx && ctx.prompts) || {})),
    })
  );
  const raw = await runner(frozen);
  let text = "";
  let output = undefined;
  let language = undefined;
  let openInPreview = undefined;
  if (typeof raw === "string") {
    text = raw;
  } else if (raw && typeof raw === "object") {
    text = raw.text != null ? String(raw.text) : "";
    if (raw.output != null) output = String(raw.output);
    if (raw.language != null && String(raw.language).trim()) language = String(raw.language).trim();
    if (typeof raw.openInPreview === "boolean") openInPreview = raw.openInPreview;
  } else if (raw != null) {
    throw new Error("macro must return a string or { text, output?, language?, openInPreview? }");
  }
  process.stdout.write(JSON.stringify({ text, output, language, openInPreview }));
})().catch((err) => {
  const message = err && err.message ? String(err.message) : String(err);
  process.stderr.write(message);
  process.exit(1);
});
`

type nodeMacroOut struct {
	Text          string `json:"text"`
	Output        string `json:"output"`
	Language      string `json:"language"`
	OpenInPreview *bool  `json:"openInPreview"`
}

// runTypeScriptMacro compiles cmd and executes it with node (Worker twin).
func runTypeScriptMacro(ctx context.Context, cacheDir string, cmd Command, tc TestContext) (RunResult, error) {
	compiled, err := CompileMacro(cacheDir, cmd)
	if err != nil {
		return RunResult{}, err
	}
	if _, ok := ctx.Deadline(); !ok {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, DefaultTimeout)
		defer cancel()
	}

	dir, err := os.MkdirTemp("", "recur-macro-test-*")
	if err != nil {
		return RunResult{}, err
	}
	defer os.RemoveAll(dir)

	codePath := filepath.Join(dir, "macro.js")
	ctxPath := filepath.Join(dir, "ctx.json")
	bootPath := filepath.Join(dir, "boot.js")
	if err := os.WriteFile(codePath, []byte(compiled.JS), 0o644); err != nil {
		return RunResult{}, err
	}
	if err := os.WriteFile(bootPath, []byte(nodeMacroBoot), 0o644); err != nil {
		return RunResult{}, err
	}
	ctxJSON, err := json.Marshal(macroCtxFromTest(tc, cmd))
	if err != nil {
		return RunResult{}, err
	}
	if err := os.WriteFile(ctxPath, ctxJSON, 0o644); err != nil {
		return RunResult{}, err
	}

	c := exec.CommandContext(ctx, "node", bootPath, codePath, ctxPath)
	var stdout, stderr bytes.Buffer
	c.Stdout = &stdout
	c.Stderr = &stderr
	if err := c.Run(); err != nil {
		res := RunResult{
			Stdout: stdout.String(),
			Stderr: stderr.String(),
			Output: cmd.Output,
		}
		if ee, ok := err.(*exec.ExitError); ok {
			res.ExitCode = ee.ExitCode()
			return res, nil
		}
		if ctx.Err() != nil {
			return res, fmt.Errorf("typescript macro timed out: %w", ctx.Err())
		}
		return res, fmt.Errorf("node: %w (is node on PATH?)", err)
	}
	var out nodeMacroOut
	if err := json.Unmarshal(stdout.Bytes(), &out); err != nil {
		return RunResult{}, fmt.Errorf("parse node result: %w (stdout=%q stderr=%q)", err, stdout.String(), stderr.String())
	}
	routing := cmd.Output
	if out.Output != "" {
		routing = out.Output
	}
	if err := ValidateOutput(routing); err != nil {
		return RunResult{}, fmt.Errorf("macro output: %w", err)
	}
	// Effective values match the app (commands.ts: result ?? cmd).
	language := cmd.Language
	if out.Language != "" {
		language = out.Language
	}
	openInPreview := cmd.OpenInPreview
	if out.OpenInPreview != nil {
		openInPreview = *out.OpenInPreview
	}
	oip := openInPreview
	return RunResult{
		Stdout:        out.Text,
		Stderr:        stderr.String(),
		ExitCode:      0,
		Output:        routing,
		Language:      language,
		OpenInPreview: &oip,
	}, nil
}

func macroCtxFromTest(tc TestContext, cmd Command) map[string]any {
	input := tc.Input
	if input == "" {
		input = ResolveInput(cmd, runContextFromTest(tc, cmd.BundleDir))
	}
	selected := tc.SelectedText
	document := tc.Document
	if document == "" {
		document = input
	}
	word := tc.CurrentWord
	if word == "" {
		word = input
	}
	tab := tc.TabSize
	if tab == 0 {
		tab = 4
	}
	prompts := tc.Prompts
	if prompts == nil {
		prompts = map[string]string{}
	}
	return map[string]any{
		"input":        input,
		"selectedText": selected,
		"document":     document,
		"currentLine":  tc.CurrentLine,
		"lineNumber":   tc.LineNumber,
		"column":       tc.Column,
		"currentWord":  word,
		"filepath":     tc.FilePath,
		"filename":     tc.FileName,
		"scope":        tc.Scope,
		"clipboard":    tc.Clipboard,
		"tabSize":      tab,
		"softTabs":     tc.SoftTabs,
		"prompts":      prompts,
	}
}
