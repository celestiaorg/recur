package bundles_test

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"testing/fstest"

	"recur/internal/bundles"
)

func TestRunBundleTestsTypeScriptAndShebang(t *testing.T) {
	if _, err := exec.LookPath("node"); err != nil {
		t.Skip("node not on PATH")
	}
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "bundle.toml"), `[bundle]
name = "Demo"
id = "demo-bundle"
version = "1.0.0"

[[command]]
id = "upper-ts"
name = "Upper"
interpreter = "typescript"
input = "selection"
output = "replaceSelection"
script = "src/upper.ts"

[[command]]
id = "echo-sh"
name = "Echo"
interpreter = "shebang"
input = "selection"
output = "replaceSelection"
body = '''
#!/bin/sh
printf 'X%sX' "$(cat)"
'''

[[command]]
id = "diagram-ts"
name = "Diagram"
interpreter = "typescript"
input = "none"
output = "newDocument"
script = "src/diagram.ts"

[[command]]
id = "manifest-preview"
name = "Manifest Preview"
interpreter = "typescript"
input = "none"
output = "newDocument"
language = "mermaid"
openInPreview = true
script = "src/plain.ts"
`)
	mustWrite(t, filepath.Join(dir, "src", "upper.ts"), `
import { type MacroContext, type MacroResult } from '@recur/sdk'
export default function (ctx: MacroContext): MacroResult {
  return { text: ctx.input.toUpperCase(), output: "replaceSelection" }
}
`)
	mustWrite(t, filepath.Join(dir, "src", "diagram.ts"), `
import { type MacroContext, type MacroResult, Output } from '@recur/sdk'
export default function (_ctx: MacroContext): MacroResult {
  return { text: "graph TD; A-->B", output: Output.newDocument, language: "mermaid", openInPreview: true }
}
`)
	mustWrite(t, filepath.Join(dir, "src", "plain.ts"), `
export default function () { return "graph TD; X-->Y" }
`)
	mustWrite(t, filepath.Join(dir, "tests", "cases.toml"), `
[[test]]
name = "uppercases selection"
command = "upper-ts"
[test.context]
input = "Hi"
selectedText = "Hi"
[test.expect]
stdout = "HI"
output = "replaceSelection"

[[test]]
name = "shebang echoes"
command = "Echo"
[test.context]
input = "ab"
[test.expect]
stdout = "XabX"

[[test]]
name = "mermaid preview routing"
command = "diagram-ts"
[test.expect]
stdout = "graph TD; A-->B"
output = "newDocument"
language = "mermaid"
openInPreview = true

[[test]]
name = "manifest language and preview defaults"
command = "manifest-preview"
[test.expect]
stdout = "graph TD; X-->Y"
output = "newDocument"
language = "mermaid"
openInPreview = true
`)

	cache := t.TempDir()
	rep, err := bundles.RunBundleTests(context.Background(), dir, cache)
	if err != nil {
		t.Fatalf("RunBundleTests: %v", err)
	}
	if !rep.OK() {
		t.Fatalf("expected all pass:\n%s", bundles.FormatReport(rep, false))
	}
	if rep.Passed != 4 {
		t.Fatalf("Passed = %d, want 4", rep.Passed)
	}
}

func TestRunBundleTestsFailureDetail(t *testing.T) {
	if _, err := exec.LookPath("node"); err != nil {
		t.Skip("node not on PATH")
	}
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "bundle.toml"), `[bundle]
name = "Demo"
id = "demo-bundle"
version = "1.0.0"
[[command]]
id = "upper-ts"
name = "Upper"
interpreter = "typescript"
script = "src/upper.ts"
`)
	mustWrite(t, filepath.Join(dir, "src", "upper.ts"), `
export default function (ctx) { return ctx.input.toUpperCase() }
`)
	mustWrite(t, filepath.Join(dir, "tests", "bad.toml"), `
[[test]]
name = "wrong expect"
command = "upper-ts"
[test.context]
input = "a"
[test.expect]
stdout = "b"
`)
	rep, err := bundles.RunBundleTests(context.Background(), dir, t.TempDir())
	if err != nil {
		t.Fatalf("RunBundleTests: %v", err)
	}
	if rep.OK() {
		t.Fatal("expected failure")
	}
	if rep.Failed != 1 {
		t.Fatalf("Failed = %d, want 1", rep.Failed)
	}
	if !strings.Contains(rep.Results[0].Detail, "stdout mismatch") {
		t.Fatalf("Detail = %q, want stdout mismatch", rep.Results[0].Detail)
	}
}

func TestSeedDefaultsSkipsTestsDir(t *testing.T) {
	src := fstest.MapFS{
		"Demo/bundle.toml": &fstest.MapFile{Data: []byte(`[bundle]
name = "Demo"
id = "d1"
version = "1.0.0"
[[command]]
id = "c1"
name = "Hi"
body = "#!/bin/sh\necho hi\n"
`)},
		"Demo/tests/case.toml": &fstest.MapFile{Data: []byte(`[[test]]
name = "n"
command = "c1"
[test.expect]
stdout = "hi\n"
`)},
		"Demo/src/a.ts": &fstest.MapFile{Data: []byte(`export default (ctx) => ctx.input`)},
	}
	dest := t.TempDir()
	if err := bundles.SeedDefaults(dest, src, "."); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(dest, "Demo", "tests")); !os.IsNotExist(err) {
		t.Fatalf("tests/ should not be seeded, err=%v", err)
	}
	if _, err := os.Stat(filepath.Join(dest, "Demo", "src", "a.ts")); err != nil {
		t.Fatalf("src should be seeded: %v", err)
	}
}

func mustWrite(t *testing.T, path, body string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}
