package bundles_test

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"recur/internal/bundles"
)

func TestLintBundlesKeyEquivalent(t *testing.T) {
	root := t.TempDir()
	writePack := func(name, toml string) string {
		dir := filepath.Join(root, name)
		if err := os.MkdirAll(filepath.Join(dir, "src"), 0o755); err != nil {
			t.Fatal(err)
		}
		ts := `import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'
export default function (ctx: MacroContext): MacroResult {
  return replaceEdited(ctx, ctx.input)
}
`
		if err := os.WriteFile(filepath.Join(dir, "src", "x.ts"), []byte(ts), 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, "bundle.toml"), []byte(toml), 0o644); err != nil {
			t.Fatal(err)
		}
		return dir
	}

	a := writePack("A", `
[bundle]
name = "A"
id = "11111111-1111-4111-8111-111111111111"
[[command]]
id = "11111111-1111-4111-8111-111111111201"
name = "Upper"
category = "Text"
interpreter = "typescript"
script = "src/x.ts"
keyEquivalent = "cmd+shift+u"
`)
	b := writePack("B", `
[bundle]
name = "B"
id = "22222222-2222-4222-8222-222222222222"
[[command]]
id = "22222222-2222-4222-8222-222222222201"
name = "Also Upper"
category = "Text"
interpreter = "typescript"
script = "src/x.ts"
keyEquivalent = "shift+cmd+u"
[[command]]
id = "22222222-2222-4222-8222-222222222202"
name = "Save Clash"
category = "Text"
interpreter = "typescript"
script = "src/x.ts"
keyEquivalent = "cmd+s"
[[command]]
id = "22222222-2222-4222-8222-222222222203"
name = "Bad Chord"
category = "Text"
interpreter = "typescript"
script = "src/x.ts"
keyEquivalent = "mod+u"
`)

	cache := t.TempDir()
	rep, err := bundles.LintBundles([]string{a, b}, cache)
	if err != nil {
		t.Fatal(err)
	}
	if rep.OK() {
		t.Fatalf("expected errors:\n%s", bundles.FormatLintReport(rep, false))
	}
	var msgs []string
	for _, f := range rep.Findings {
		msgs = append(msgs, string(f.Severity)+":"+f.Message)
	}
	joined := strings.Join(msgs, "\n")
	if !strings.Contains(joined, "collides with app shortcut") {
		t.Fatalf("missing reserved collision:\n%s", joined)
	}
	if !strings.Contains(joined, "unknown modifier") && !strings.Contains(joined, "keyEquivalent:") {
		t.Fatalf("missing bad chord:\n%s", joined)
	}
	if !strings.Contains(joined, "shared by") {
		t.Fatalf("missing share info:\n%s", joined)
	}
}

func TestLintOpenInPreview(t *testing.T) {
	root := t.TempDir()
	dir := filepath.Join(root, "Prev")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	toml := `
[bundle]
name = "Prev"
id = "33333333-3333-4333-8333-333333333333"
[[command]]
id = "33333333-3333-4333-8333-333333333301"
name = "Bad Output"
output = "replaceInput"
openInPreview = true
language = "html"
interpreter = "shebang"
body = '''
#!/bin/sh
cat
'''
[[command]]
id = "33333333-3333-4333-8333-333333333302"
name = "Bad Lang"
output = "newDocument"
openInPreview = true
language = "diff"
interpreter = "shebang"
body = '''
#!/bin/sh
cat
'''
[[command]]
id = "33333333-3333-4333-8333-333333333303"
name = "OK HTML"
output = "newDocument"
openInPreview = true
language = "html"
interpreter = "shebang"
body = '''
#!/bin/sh
cat
'''
`
	if err := os.WriteFile(filepath.Join(dir, "bundle.toml"), []byte(toml), 0o644); err != nil {
		t.Fatal(err)
	}
	rep, err := bundles.LintBundles([]string{dir}, t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if rep.OK() {
		t.Fatalf("expected errors:\n%s", bundles.FormatLintReport(rep, false))
	}
	var msgs []string
	for _, f := range rep.Findings {
		if f.Severity == bundles.LintError {
			msgs = append(msgs, f.Message)
		}
	}
	joined := strings.Join(msgs, "\n")
	if !strings.Contains(joined, `openInPreview requires output = "newDocument"`) {
		t.Fatalf("missing output check:\n%s", joined)
	}
	if !strings.Contains(joined, `language "diff" has no Preview chip`) {
		t.Fatalf("missing language check:\n%s", joined)
	}
	for _, f := range rep.Findings {
		if f.Command == "OK HTML" && f.Severity == bundles.LintError {
			t.Fatalf("OK HTML should not error: %s", f.Message)
		}
	}
}

func TestPreviewableLanguage(t *testing.T) {
	for _, ok := range []string{"html", "HTML", ".md", "markdown", "svg", "htm", "mdx", "mmd", "mermaid", ".mermaid"} {
		if !bundles.PreviewableLanguage(ok) {
			t.Fatalf("expected previewable: %q", ok)
		}
	}
	for _, no := range []string{"", "diff", "json", "ts", "txt"} {
		if bundles.PreviewableLanguage(no) {
			t.Fatalf("expected not previewable: %q", no)
		}
	}
}

func TestLintSymbolTransformOniguruma(t *testing.T) {
	root := t.TempDir()
	dir := filepath.Join(root, "Sym")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	toml := `
[bundle]
name = "Sym"
id = "33333333-3333-4333-8333-333333333333"
[[symbol]]
id = "sym.js-group"
scope = "entity.name.function"
transform = '''
s/(?:foo)bar//
s/(?=x)y//
'''
[[symbol]]
id = "sym.oni"
scope = "entity.name.class"
transform = '''
s/(?x)foo//
'''
`
	if err := os.WriteFile(filepath.Join(dir, "bundle.toml"), []byte(toml), 0o644); err != nil {
		t.Fatal(err)
	}
	rep, err := bundles.LintBundles([]string{dir}, t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	var oni []string
	for _, f := range rep.Findings {
		if strings.Contains(f.Message, "Oniguruma") {
			oni = append(oni, f.Command)
		}
	}
	if len(oni) != 1 || oni[0] != "sym.oni" {
		t.Errorf("LintBundles Oniguruma notes = %v, want [sym.oni]", oni)
	}
}
