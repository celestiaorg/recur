package bundles_test

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"recur/internal/bundles"
)

func TestCompileMacroTypeScriptAndCache(t *testing.T) {
	cache := t.TempDir()
	cmd := bundles.Command{
		ID:          "ts-upper",
		Interpreter: bundles.InterpreterTypeScript,
		Body: `
import { Output, type MacroContext, type MacroResult } from '@recur/sdk'
export default function (ctx: MacroContext): MacroResult {
  return { text: ctx.input.toUpperCase(), output: Output.replaceSelection }
}
`,
	}
	r1, err := bundles.CompileMacro(cache, cmd)
	if err != nil {
		t.Fatal(err)
	}
	if r1.CacheHit {
		t.Fatal("first compile should miss cache")
	}
	if strings.TrimSpace(r1.JS) == "" {
		t.Fatal("empty js")
	}
	if !strings.Contains(r1.JS, "toUpperCase") {
		t.Fatalf("expected toUpperCase in output:\n%s", r1.JS)
	}
	if !strings.Contains(r1.JS, "replaceSelection") || !strings.Contains(r1.JS, "Object.freeze") {
		t.Fatalf("expected bundled @recur/sdk in output:\n%s", r1.JS)
	}
	r2, err := bundles.CompileMacro(cache, cmd)
	if err != nil {
		t.Fatal(err)
	}
	if !r2.CacheHit {
		t.Fatal("second compile should hit cache")
	}
	if r1.JS != r2.JS {
		t.Fatal("cache mismatch")
	}
	entries, _ := os.ReadDir(cache)
	if len(entries) != 1 {
		t.Fatalf("want 1 cache file, got %d", len(entries))
	}
}

func TestCompileMacroWrapsBareBody(t *testing.T) {
	cache := t.TempDir()
	cmd := bundles.Command{
		ID:          "ts-bare",
		Interpreter: bundles.InterpreterTypeScript,
		Body:        `return replaceEdited(ctx, ctx.input + "!")`,
	}
	r, err := bundles.CompileMacro(cache, cmd)
	if err != nil {
		t.Fatal(err)
	}
	if strings.TrimSpace(r.JS) == "" {
		t.Fatal("empty")
	}
	if !strings.Contains(r.JS, "replaceEdited") {
		t.Fatalf("bare body should bundle @recur/sdk:\n%s", r.JS)
	}
}

func TestCompileMacroRejectsOtherImports(t *testing.T) {
	// Import must be referenced so esbuild still resolves it (unused imports
	// are dropped before resolve).
	_, err := bundles.CompileMacro(t.TempDir(), bundles.Command{
		ID:          "ts-bad",
		Interpreter: bundles.InterpreterTypeScript,
		Body: `
import fs from "fs"
export default (ctx: { input: string }) => { void fs; return ctx.input }
`,
	})
	if err == nil || !strings.Contains(err.Error(), "not allowed") {
		t.Fatalf("want reject non-sdk import, got %v", err)
	}
}

func TestCompileMacroSDKCoversOutputConstants(t *testing.T) {
	for _, v := range []string{
		bundles.OutputDiscard, bundles.OutputReplaceSelection, bundles.OutputReplaceDocument,
		bundles.OutputReplaceInput, bundles.OutputInsertText, bundles.OutputInsertSnippet,
		bundles.OutputNewDocument, bundles.OutputShowAsTooltip,
		bundles.OutputCopyToClipboard, bundles.OutputShowAsToast,
	} {
		if !strings.Contains(bundles.MacroSDKSource, `"`+v+`"`) {
			t.Fatalf("macro SDK missing Output value %q", v)
		}
	}
}

func TestMacroSDKMatchesAuthoringCopy(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("caller")
	}
	// internal/bundles/compile_test.go → repo root → bundles/sdk/index.ts
	author := filepath.Join(filepath.Dir(thisFile), "..", "..", "bundles", "sdk", "index.ts")
	data, err := os.ReadFile(author)
	if err != nil {
		t.Fatal(err)
	}
	// Strip leading comment blocks that differ, compare export surface by normalizing
	// to the embedded source (which is the compile source of truth).
	got := normalizeSDK(string(data))
	want := normalizeSDK(bundles.MacroSDKSource)
	if got != want {
		t.Fatalf("bundles/sdk/index.ts must match internal/bundles/macro_sdk.ts\n--- author ---\n%s\n--- embed ---\n%s", got, want)
	}
}

func normalizeSDK(s string) string {
	lines := strings.Split(s, "\n")
	out := make([]string, 0, len(lines))
	for _, line := range lines {
		trim := strings.TrimSpace(line)
		if strings.HasPrefix(trim, "//") {
			continue
		}
		out = append(out, line)
	}
	return strings.TrimSpace(strings.Join(out, "\n"))
}

func TestCompileMacroRejectsShebang(t *testing.T) {
	_, err := bundles.CompileMacro(t.TempDir(), bundles.Command{
		ID: "x", Interpreter: bundles.InterpreterShebang, Body: "true",
	})
	if err == nil {
		t.Fatal("expected error")
	}
}

func TestCompileMacroInvalidatesOnEdit(t *testing.T) {
	cache := t.TempDir()
	cmd := bundles.Command{
		ID:          "ts-edit",
		Interpreter: bundles.InterpreterTypeScript,
		Body: `
import type { MacroContext } from '@recur/sdk'
export default (ctx: MacroContext) => ctx.input
`,
	}
	r1, err := bundles.CompileMacro(cache, cmd)
	if err != nil {
		t.Fatal(err)
	}
	cmd.Body = `
import type { MacroContext } from '@recur/sdk'
export default (ctx: MacroContext) => ctx.input + "x"
`
	r2, err := bundles.CompileMacro(cache, cmd)
	if err != nil {
		t.Fatal(err)
	}
	if r2.CacheHit {
		t.Fatal("edited source must miss cache")
	}
	if r1.Hash == r2.Hash {
		t.Fatal("hash should change")
	}
	matches, _ := filepath.Glob(filepath.Join(cache, "ts-edit.*.js"))
	if len(matches) != 1 {
		t.Fatalf("pruned cache: %v", matches)
	}
}
