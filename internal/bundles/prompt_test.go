package bundles_test

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strconv"
	"strings"
	"testing"
	"time"

	"recur/internal/bundles"
)

func TestParseManifestPromptActions(t *testing.T) {
	dir := t.TempDir()
	data := []byte(`
[bundle]
name = "Chat"
id = "bundle-chat"

[[prompt]]
id = "prompt-fix"
name = "Fix Findings"
description = "Fix open findings"
mode = "agent"
timeoutMs = 10000
interpreter = "shebang"
body = '''
#!/bin/sh
printf '{"prompt":"fix it","display":"Fix"}'
'''

[[prompt]]
id = "prompt-explain"
name = "Explain Selection"
interpreter = "typescript"
takesArgs = true
body = '''
import type { PromptContext, PromptResult } from '@recur/sdk'
export default function (ctx: PromptContext): PromptResult {
  return { prompt: ctx.selectedText, display: "Explain" }
}
'''
`)
	meta, cmds, prompts, err := bundles.ParseManifest(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if meta.Name != "Chat" || len(cmds) != 0 {
		t.Fatalf("meta=%+v cmds=%d", meta, len(cmds))
	}
	if len(prompts) != 2 {
		t.Fatalf("prompts=%d", len(prompts))
	}
	fix := prompts[0]
	if fix.Slash != "fix-findings" {
		t.Fatalf("default slash=%q", fix.Slash)
	}
	if fix.Mode != "agent" || fix.TimeoutMs != 10000 || fix.FreshSession {
		t.Fatalf("fix fields: %+v", fix)
	}
	explain := prompts[1]
	if explain.Slash != "explain-selection" {
		t.Fatalf("explain slash=%q", explain.Slash)
	}
	if explain.Interpreter != bundles.InterpreterTypeScript {
		t.Fatalf("interpreter=%q", explain.Interpreter)
	}
	if !explain.TakesArgs {
		t.Fatal("explain should take args")
	}
	if explain.FreshSession {
		t.Fatalf("explain FreshSession = true, want false")
	}
}

func TestParseManifestFreshSession(t *testing.T) {
	dir := t.TempDir()
	data := []byte(`
[bundle]
name = "Chat"
id = "bundle-chat"

[[prompt]]
id = "prompt-review"
name = "Review"
slash = "review"
fresh_session = true
interpreter = "shebang"
body = '''
#!/bin/sh
printf '{"prompt":"review","display":"Review"}'
'''
`)
	_, _, prompts, err := bundles.ParseManifest(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if len(prompts) != 1 || !prompts[0].FreshSession {
		t.Fatalf("ParseManifest FreshSession = %+v, want [{FreshSession:true}]", prompts)
	}
}

func TestParseManifestTabGroup(t *testing.T) {
	dir := t.TempDir()
	data := []byte(`
[bundle]
name = "Chat"
id = "bundle-chat"

[[prompt]]
id = "prompt-commit"
name = "Commit"
slash = "commit"
mode = "agent"
tab_group = "ops"
interpreter = "shebang"
body = '''
#!/bin/sh
printf '{"prompt":"commit","display":"Commit"}'
'''
`)
	_, _, prompts, err := bundles.ParseManifest(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if len(prompts) != 1 || prompts[0].TabGroup != "ops" {
		t.Fatalf("ParseManifest TabGroup = %+v, want [{TabGroup:ops}]", prompts)
	}
}

func TestParseManifestPromptComplete(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "complete.sh"), []byte("#!/bin/sh\nprintf '{\"hits\":[]}\\n'\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	data := []byte(`
[bundle]
name = "T"
id = "b"

[[prompt]]
id = "p1"
name = "X"
slash = "x"
takesArgs = true
argPlaceholder = "id or URL"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
complete = "complete.sh"
`)
	_, _, prompts, err := bundles.ParseManifest(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if len(prompts) != 1 {
		t.Fatalf("prompts=%d", len(prompts))
	}
	p := prompts[0]
	if p.ArgPlaceholder != "id or URL" {
		t.Fatalf("argPlaceholder=%q", p.ArgPlaceholder)
	}
	if len(p.Args) != 1 || p.Args[0].Name != "args" {
		t.Fatalf("synthesized args=%+v", p.Args)
	}
	if p.CompleteInterpreter != bundles.InterpreterShebang {
		t.Fatalf("complete interpreter=%q", p.CompleteInterpreter)
	}
	if !strings.Contains(p.CompleteBody, "hits") {
		t.Fatalf("complete body empty or unexpected: %q", p.CompleteBody)
	}
	if p.CompletePath != filepath.Join(dir, "complete.sh") {
		t.Fatalf("complete path=%q", p.CompletePath)
	}
}

func TestParseManifestPromptIcon(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "mark.svg"), []byte(`<svg viewBox="0 0 8 8"><path fill="#222326" d="M0 0h8v8z"/></svg>`), 0o644); err != nil {
		t.Fatal(err)
	}
	data := []byte(`
[bundle]
name = "T"
id = "b"

[[prompt]]
id = "p1"
name = "X"
slash = "x"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
icon = "mark.svg"
`)
	_, _, prompts, err := bundles.ParseManifest(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if len(prompts) != 1 || !strings.Contains(prompts[0].IconSVG, `fill="currentColor"`) {
		t.Fatalf("icon=%q err=%v", prompts[0].IconSVG, err)
	}
	if err := os.WriteFile(filepath.Join(dir, "bad.svg"), []byte(`<svg><script>alert(1)</script></svg>`), 0o644); err != nil {
		t.Fatal(err)
	}
	_, _, _, err = bundles.ParseManifest(dir, []byte(`
[bundle]
name = "T"
id = "b"
[[prompt]]
id = "p1"
name = "X"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
icon = "bad.svg"
`))
	if err == nil || !strings.Contains(err.Error(), "static icon") {
		t.Fatalf("want static icon error, got %v", err)
	}
}

func TestParseManifestPromptNamedArgs(t *testing.T) {
	dir := t.TempDir()
	data := []byte(`
[bundle]
name = "T"
id = "b"

[[prompt]]
id = "p1"
name = "X"
slash = "linear"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"

[[prompt.arg]]
name = "issue"
placeholder = "TEAM-123"
`)
	_, _, prompts, err := bundles.ParseManifest(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if len(prompts) != 1 {
		t.Fatalf("prompts=%d", len(prompts))
	}
	p := prompts[0]
	if !p.TakesArgs {
		t.Fatal("named args should imply takesArgs")
	}
	if len(p.Args) != 1 || p.Args[0].Name != "issue" || p.Args[0].Placeholder != "TEAM-123" {
		t.Fatalf("args=%+v", p.Args)
	}
	if p.Args[0].Optional {
		t.Fatal("issue slot should be required")
	}
	if p.ArgPlaceholder != "TEAM-123" {
		t.Fatalf("argPlaceholder=%q", p.ArgPlaceholder)
	}
}

func TestParseManifestPromptOptionalArg(t *testing.T) {
	dir := t.TempDir()
	data := []byte(`
[bundle]
name = "T"
id = "b"

[[prompt]]
id = "p1"
name = "X"
slash = "fix-checks"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"

[[prompt.arg]]
name = "check"
placeholder = "all or a failing check"
optional = true
`)
	_, _, prompts, err := bundles.ParseManifest(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if len(prompts) != 1 {
		t.Fatalf("prompts=%d", len(prompts))
	}
	p := prompts[0]
	if !p.TakesArgs {
		t.Fatal("named args should imply takesArgs")
	}
	if len(p.Args) != 1 || p.Args[0].Name != "check" || !p.Args[0].Optional {
		t.Fatalf("args=%+v", p.Args)
	}
}

func TestShippedFixChecksPromptArgs(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller")
	}
	bundleDir := filepath.Join(filepath.Dir(thisFile), "../../bundles/Chat")
	data, err := os.ReadFile(filepath.Join(bundleDir, "bundle.toml"))
	if err != nil {
		t.Fatal(err)
	}
	_, _, prompts, err := bundles.ParseManifest(bundleDir, data)
	if err != nil {
		t.Fatal(err)
	}
	var p *bundles.PromptAction
	for i := range prompts {
		if prompts[i].Slash == "fix-checks" {
			p = &prompts[i]
			break
		}
	}
	if p == nil {
		t.Fatal("missing shipped /fix-checks")
	}
	if !p.TakesArgs {
		t.Fatal("/fix-checks should take args")
	}
	if strings.TrimSpace(p.CompleteBody) == "" {
		t.Fatal("/fix-checks missing complete script")
	}
	if len(p.Args) != 1 || p.Args[0].Name != "check" || !p.Args[0].Optional {
		t.Fatalf("args=%+v", p.Args)
	}
	if !strings.Contains(p.IconSVG, `fill="currentColor"`) {
		t.Fatalf("/fix-checks iconSvg=%q, want themed GitHub mark", p.IconSVG)
	}
	if !p.FreshSession {
		t.Fatal("/fix-checks should start a new provider session")
	}
	if p.TabGroup != "ops" {
		t.Fatal("/fix-checks should use or create the ops chat")
	}
}

func TestParseManifestPromptLoopMode(t *testing.T) {
	dir := t.TempDir()
	data := []byte(`
[bundle]
name = "Chat"
id = "bundle-chat"

[[prompt]]
id = "prompt-converge"
name = "Review & fix"
slash = "review-and-fix"
mode = "loop"
loop = "recur.loops.converge"
interpreter = "shebang"
body = '''
#!/bin/sh
printf '{"prompt":"review it","display":"Review & fix"}'
'''

[[prompt]]
id = "prompt-implied"
name = "Implied Loop"
loop = "recur.loops.docs"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`)
	_, _, prompts, err := bundles.ParseManifest(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if len(prompts) != 2 {
		t.Fatalf("prompts=%d", len(prompts))
	}
	got := prompts[0]
	if got.Mode != bundles.PromptModeLoop || got.Loop != "recur.loops.converge" {
		t.Fatalf("loop prompt: %+v", got)
	}
	implied := prompts[1]
	if implied.Mode != bundles.PromptModeLoop || implied.Loop != "recur.loops.docs" {
		t.Fatalf("implied loop mode: %+v", implied)
	}
}

func TestParseShippedPromptComplete(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller")
	}
	root := filepath.Join(filepath.Dir(thisFile), "../..")
	cases := []struct {
		dir, slash, placeholder string
	}{
		{filepath.Join(root, "bundles/Linear"), "linear", "TEAM-123"},
	}
	for _, tc := range cases {
		t.Run(tc.slash, func(t *testing.T) {
			data, err := os.ReadFile(filepath.Join(tc.dir, "bundle.toml"))
			if err != nil {
				t.Fatal(err)
			}
			parsed, err := bundles.ParseManifestFull(tc.dir, data)
			if err != nil {
				t.Fatal(err)
			}
			prompts := parsed.Prompts
			var p *bundles.PromptAction
			for i := range prompts {
				if prompts[i].Slash == tc.slash {
					p = &prompts[i]
					break
				}
			}
			if p == nil {
				t.Fatalf("slash %q not found", tc.slash)
			}
			if p.Mode != "" {
				t.Fatalf("mode=%q, want empty (stay on current chat)", p.Mode)
			}
			if !p.TakesArgs {
				t.Fatal("takesArgs")
			}
			if !strings.Contains(p.ArgPlaceholder, tc.placeholder) {
				t.Fatalf("argPlaceholder=%q", p.ArgPlaceholder)
			}
			if p.Interpreter != bundles.InterpreterShebang {
				t.Fatalf("interpreter=%q", p.Interpreter)
			}
			if !strings.HasPrefix(strings.TrimLeft(p.Body, "\uFEFF"), "#!") {
				t.Fatal("prompt missing shebang")
			}
			if p.CompleteInterpreter != bundles.InterpreterShebang {
				t.Fatalf("complete interpreter=%q", p.CompleteInterpreter)
			}
			if strings.TrimSpace(p.CompleteBody) == "" {
				t.Fatal("empty complete body")
			}
			if !strings.HasPrefix(strings.TrimLeft(p.CompleteBody, "\uFEFF"), "#!") {
				t.Fatal("complete missing shebang")
			}
			if len(p.Args) != 1 || p.Args[0].Name != "issue" {
				t.Fatalf("args=%+v", p.Args)
			}
			if !strings.Contains(p.IconSVG, "currentColor") || !strings.Contains(p.IconSVG, "viewBox") {
				t.Fatalf("icon svg=%q", p.IconSVG)
			}
			if len(parsed.Searches) != 1 {
				t.Fatalf("searches=%d", len(parsed.Searches))
			}
			se := parsed.Searches[0]
			if se.Name != "Linear" || se.Interpreter != bundles.InterpreterShebang {
				t.Fatalf("search=%+v", se)
			}
			if !strings.Contains(se.IconSVG, "viewBox") {
				t.Fatalf("search icon svg=%q", se.IconSVG)
			}
			if !strings.HasPrefix(strings.TrimLeft(se.Body, "\uFEFF"), "#!") {
				t.Fatal("search missing shebang")
			}
			if len(parsed.Expands) != 1 {
				t.Fatalf("expands=%d", len(parsed.Expands))
			}
			ex := parsed.Expands[0]
			if ex.Name != "Linear issue" || ex.MatchRE == nil {
				t.Fatalf("expand=%+v", ex)
			}
			if !ex.Matches("ENG-123 also tests") || ex.Matches("fix the flaky test") || ex.Matches("2026-09-01") {
				t.Fatalf("match ENG-123=%v prose=%v date=%v", ex.Matches("ENG-123 also tests"), ex.Matches("fix the flaky test"), ex.Matches("2026-09-01"))
			}
			if !strings.HasPrefix(strings.TrimLeft(ex.Body, "\uFEFF"), "#!") {
				t.Fatal("expand missing shebang")
			}
		})
	}
}

func TestShippedChatLoopPrompts(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller")
	}
	bundleDir := filepath.Join(filepath.Dir(thisFile), "../../bundles/Chat")
	data, err := os.ReadFile(filepath.Join(bundleDir, "bundle.toml"))
	if err != nil {
		t.Fatal(err)
	}
	_, _, prompts, err := bundles.ParseManifest(bundleDir, data)
	if err != nil {
		t.Fatal(err)
	}
	want := map[string]string{
		"review-and-fix":  "recur.loops.converge",
		"review-to-pr":    "recur.loops.converge-pr",
		"to-a-pr":         "recur.loops.pr",
		"write-docs":      "recur.loops.docs",
		"security-review": "recur.loops.security",
	}
	found := map[string]bool{}
	for _, p := range prompts {
		loop, ok := want[p.Slash]
		if !ok {
			continue
		}
		found[p.Slash] = true
		if p.Mode != bundles.PromptModeLoop || p.Loop != loop {
			t.Errorf("/%s: mode=%q loop=%q, want mode=loop loop=%q", p.Slash, p.Mode, p.Loop, loop)
		}
		res, err := bundles.RunPromptShebang(shebangCtx(t), p, bundles.PromptRunContext{
			BundleSupport: bundleDir,
		})
		if err != nil {
			t.Errorf("/%s shebang: %v", p.Slash, err)
			continue
		}
		if strings.TrimSpace(res.Prompt) == "" || strings.TrimSpace(res.Display) == "" {
			t.Errorf("/%s empty result: %+v", p.Slash, res)
		}
	}
	for slash := range want {
		if !found[slash] {
			t.Errorf("missing shipped loop prompt /%s", slash)
		}
	}
}

func TestShippedChatFreshSession(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller")
	}
	bundleDir := filepath.Join(filepath.Dir(thisFile), "../../bundles/Chat")
	data, err := os.ReadFile(filepath.Join(bundleDir, "bundle.toml"))
	if err != nil {
		t.Fatal(err)
	}
	_, _, prompts, err := bundles.ParseManifest(bundleDir, data)
	if err != nil {
		t.Fatal(err)
	}
	want := map[string]bool{
		"fix-findings":    true,
		"commit":          true,
		"commit-push":     true,
		"open-pr":         true,
		"open-draft-pr":   true,
		"review":          true,
		"re-review":       true,
		"simplify-review": true,
		"simplify-audit":  true,
		"update":          true,
		"review-and-fix":  false,
		"review-to-pr":    false,
		"to-a-pr":         false,
		"write-docs":      false,
		"security-review": false,
	}
	found := map[string]bool{}
	for _, p := range prompts {
		expect, ok := want[p.Slash]
		if !ok {
			continue
		}
		found[p.Slash] = true
		if p.FreshSession != expect {
			t.Errorf("/%s fresh_session = %v, want %v", p.Slash, p.FreshSession, expect)
		}
	}
	for slash := range want {
		if !found[slash] {
			t.Errorf("missing shipped prompt /%s", slash)
		}
	}
}

func TestShippedChatTabGroup(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller")
	}
	bundleDir := filepath.Join(filepath.Dir(thisFile), "../../bundles/Chat")
	data, err := os.ReadFile(filepath.Join(bundleDir, "bundle.toml"))
	if err != nil {
		t.Fatal(err)
	}
	_, _, prompts, err := bundles.ParseManifest(bundleDir, data)
	if err != nil {
		t.Fatal(err)
	}
	want := map[string]string{
		"fix-findings":    "ops",
		"commit":          "ops",
		"commit-push":     "ops",
		"open-pr":         "ops",
		"open-draft-pr":   "ops",
		"fix-checks":      "ops",
		"update":          "ops",
		"review":          "review",
		"simplify-review": "simplify-review",
		"simplify-audit":  "simplify-audit",
		"re-review":       "",
		"review-and-fix":  "",
		"review-to-pr":    "",
		"to-a-pr":         "",
		"write-docs":      "",
		"security-review": "",
	}
	found := map[string]bool{}
	for _, p := range prompts {
		expect, ok := want[p.Slash]
		if !ok {
			continue
		}
		found[p.Slash] = true
		if p.TabGroup != expect {
			t.Errorf("/%s tab_group = %q, want %q", p.Slash, p.TabGroup, expect)
		}
	}
	for slash := range want {
		if !found[slash] {
			t.Errorf("missing shipped prompt /%s", slash)
		}
	}
}

func TestParseManifestPromptValidation(t *testing.T) {
	dir := t.TempDir()
	cases := []struct {
		name string
		toml string
		want string
	}{
		{"bad slash", `
[bundle]
name = "T"
id = "b"
[[prompt]]
id = "p1"
name = "X"
slash = "Bad_Slash"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`, "slash"},
		{"bad mode", `
[bundle]
name = "T"
id = "b"
[[prompt]]
id = "p1"
name = "X"
mode = "workflow"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`, "unknown mode"},
		{"loop requires loop mode", `
[bundle]
name = "T"
id = "b"
[[prompt]]
id = "p1"
name = "X"
mode = "agent"
loop = "recur.loops.converge"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`, "loop requires mode"},
		{"fresh session with loop mode", `
[bundle]
name = "T"
id = "b"
[[prompt]]
id = "p1"
name = "X"
mode = "loop"
fresh_session = true
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`, `fresh_session is not valid with mode "loop"`},
		{"tab group without mode", `
[bundle]
name = "T"
id = "b"
[[prompt]]
id = "p1"
name = "X"
tab_group = "ops"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`, "tab_group requires mode"},
		{"bad tab group", `
[bundle]
name = "T"
id = "b"
[[prompt]]
id = "p1"
name = "X"
mode = "agent"
tab_group = "Ops"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`, "tab_group"},
		{"bad loop id", `
[bundle]
name = "T"
id = "b"
[[prompt]]
id = "p1"
name = "X"
mode = "loop"
loop = "../evil"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`, "not a safe id"},
		{"timeout ceiling", `
[bundle]
name = "T"
id = "b"
[[prompt]]
id = "p1"
name = "X"
timeoutMs = 30001
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`, "timeoutMs"},
		{"body and script", `
[bundle]
name = "T"
id = "b"
[[prompt]]
id = "p1"
name = "X"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
script = "nope.sh"
`, "body or script"},
		{"missing name", `
[bundle]
name = "T"
id = "b"
[[prompt]]
id = "p1"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`, "name is required"},
		{"complete without args", `
[bundle]
name = "T"
id = "b"
[[prompt]]
id = "p1"
name = "X"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
complete = "complete.sh"
`, "complete requires takesArgs"},
		{"bad arg name", `
[bundle]
name = "T"
id = "b"
[[prompt]]
id = "p1"
name = "X"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
[[prompt.arg]]
name = "Issue-ID"
`, "must match"},
		{"search typescript", `
[bundle]
name = "T"
id = "b"
[[search]]
id = "s1"
name = "S"
interpreter = "typescript"
body = "export function complete() { return { hits: [] } }"
`, "shebang"},
		{"expand bad match", `
[bundle]
name = "T"
id = "b"
[[expand]]
id = "e1"
name = "E"
match = "("
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`, "match"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, _, _, err := bundles.ParseManifest(dir, []byte(tc.toml))
			if err == nil || !strings.Contains(err.Error(), tc.want) {
				t.Fatalf("want %q in error, got %v", tc.want, err)
			}
		})
	}
}

func TestDiscoverPromptSlashUserBeatsDefault(t *testing.T) {
	def := t.TempDir()
	user := t.TempDir()
	writeBundle(t, def, "Chat", `
[bundle]
name = "Chat"
id = "b-chat-default"
[[prompt]]
id = "prompt-audit-default"
name = "Audit"
slash = "audit"
interpreter = "shebang"
body = "#!/bin/sh\nprintf '{\"prompt\":\"default\"}'\n"
`)
	writeBundle(t, user, "Chat", `
[bundle]
name = "Chat"
id = "b-chat-user"
[[prompt]]
id = "prompt-audit-user"
name = "Audit User"
slash = "audit"
interpreter = "shebang"
body = "#!/bin/sh\nprintf '{\"prompt\":\"user\"}'\n"
`)
	_, prompts, skips, err := bundles.DiscoverReport(bundles.DiscoverRoots{UserDir: user, DefaultDir: def})
	if err != nil {
		t.Fatal(err)
	}
	p, ok := bundles.FindPromptBySlash(prompts, "audit")
	if !ok {
		t.Fatal("missing audit")
	}
	if p.ID != "prompt-audit-user" || p.SourceRoot != "user" {
		t.Fatalf("want user winner, got %+v", p)
	}
	if len(skips) != 1 || !strings.Contains(skips[0].Reason, "collides") {
		t.Fatalf("want slash skip, got %+v", skips)
	}
}

func TestDiscoverPromptSlashSameSourceTiebreak(t *testing.T) {
	def := t.TempDir()
	writeBundle(t, def, "A", `
[bundle]
name = "A"
id = "bundle-bbb"
[[prompt]]
id = "prompt-zzz"
name = "Later"
slash = "shared"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`)
	writeBundle(t, def, "B", `
[bundle]
name = "B"
id = "bundle-aaa"
[[prompt]]
id = "prompt-aaa"
name = "Earlier"
slash = "shared"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`)
	_, prompts, skips, err := bundles.DiscoverReport(bundles.DiscoverRoots{DefaultDir: def})
	if err != nil {
		t.Fatal(err)
	}
	if len(prompts) != 1 {
		t.Fatalf("prompts=%d", len(prompts))
	}
	if prompts[0].ID != "prompt-aaa" || prompts[0].BundleID != "bundle-aaa" {
		t.Fatalf("want lexicographically smaller bundle/prompt id, got %+v", prompts[0])
	}
	if len(skips) != 1 {
		t.Fatalf("skips=%+v", skips)
	}
}

func TestDiscoverPromptUUIDUserOverride(t *testing.T) {
	def := t.TempDir()
	user := t.TempDir()
	writeBundle(t, def, "Chat", `
[bundle]
name = "Chat"
id = "b-chat"
[[prompt]]
id = "shared-prompt"
name = "From Default"
slash = "fix-findings"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`)
	writeBundle(t, user, "Chat", `
[bundle]
name = "Chat"
id = "b-chat-user"
[[prompt]]
id = "shared-prompt"
name = "From User"
slash = "fix-findings"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`)
	_, prompts, err := bundles.Discover(bundles.DiscoverRoots{UserDir: user, DefaultDir: def})
	if err != nil {
		t.Fatal(err)
	}
	p, ok := bundles.FindPromptByID(prompts, "shared-prompt")
	if !ok {
		t.Fatal("missing shared-prompt")
	}
	if p.Name != "From User" || p.SourceRoot != "user" {
		t.Fatalf("want user override, got %+v", p)
	}
}

func TestDiscoverPromptsSortedBySlash(t *testing.T) {
	def := t.TempDir()
	writeBundle(t, def, "Chat", `
[bundle]
name = "Chat"
id = "b-chat"
[[prompt]]
id = "p-z"
name = "Zed"
slash = "zed"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
[[prompt]]
id = "p-a"
name = "Audit"
slash = "audit"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`)
	_, prompts, err := bundles.Discover(bundles.DiscoverRoots{DefaultDir: def})
	if err != nil {
		t.Fatal(err)
	}
	if len(prompts) != 2 || prompts[0].Slash != "audit" || prompts[1].Slash != "zed" {
		t.Fatalf("order: %+v", prompts)
	}
}

func TestLintPromptSlashDupes(t *testing.T) {
	root := t.TempDir()
	a := filepath.Join(root, "A")
	b := filepath.Join(root, "B")
	for _, dir := range []string{a, b} {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.WriteFile(filepath.Join(a, "bundle.toml"), []byte(`
[bundle]
name = "A"
id = "bundle-a"
[[prompt]]
id = "p-a"
name = "Audit A"
slash = "audit"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(b, "bundle.toml"), []byte(`
[bundle]
name = "B"
id = "bundle-b"
[[prompt]]
id = "p-b"
name = "Audit B"
slash = "audit"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`), 0o644); err != nil {
		t.Fatal(err)
	}
	rep, err := bundles.LintBundles([]string{a, b}, t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if rep.OK() {
		t.Fatalf("expected slash dupe error:\n%s", bundles.FormatLintReport(rep, false))
	}
	joined := bundles.FormatLintReport(rep, false)
	if !strings.Contains(joined, `slash "audit" duplicated`) {
		t.Fatalf("missing dupe finding:\n%s", joined)
	}
}

func TestLintPromptSameBundleSlashDupe(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "bundle.toml"), []byte(`
[bundle]
name = "Chat"
id = "b-chat"
[[prompt]]
id = "p1"
name = "One"
slash = "same"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
[[prompt]]
id = "p2"
name = "Two"
slash = "same"
interpreter = "shebang"
body = "#!/bin/sh\ntrue\n"
`), 0o644); err != nil {
		t.Fatal(err)
	}
	rep, err := bundles.LintBundles([]string{dir}, t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if rep.OK() {
		t.Fatal("expected error")
	}
	if !strings.Contains(bundles.FormatLintReport(rep, false), `slash "same" duplicated`) {
		t.Fatalf("findings:\n%s", bundles.FormatLintReport(rep, false))
	}
}

func TestCompilePromptMacro(t *testing.T) {
	cache := t.TempDir()
	prompt := bundles.PromptAction{
		ID:          "prompt-ts",
		Name:        "Explain",
		Interpreter: bundles.InterpreterTypeScript,
		Body: `
import type { PromptContext, PromptResult } from '@recur/sdk'
export default function (ctx: PromptContext): PromptResult {
  return { prompt: "explain " + ctx.selectedText, display: "Explain" }
}
`,
	}
	r, err := bundles.CompilePromptMacro(cache, prompt)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(r.JS, "selectedText") || !strings.Contains(r.JS, "Explain") {
		t.Fatalf("js missing expected content:\n%s", r.JS)
	}
	_, err = bundles.CompilePromptMacro(cache, bundles.PromptAction{
		ID: "prompt-sh", Interpreter: bundles.InterpreterShebang, Body: "true",
	})
	if err == nil {
		t.Fatal("expected reject shebang")
	}
}

func TestCompileCompleteMacro(t *testing.T) {
	r, err := bundles.CompileCompleteMacro(t.TempDir(), bundles.PromptAction{
		ID:                  "prompt-complete",
		CompleteInterpreter: bundles.InterpreterTypeScript,
		CompleteBody: `import type { PromptCompleteContext, PromptCompleteResult } from '@recur/sdk'
export default function (ctx: PromptCompleteContext): PromptCompleteResult {
  return { hits: [{ insert: ctx.args, label: ctx.args }] }
}
`,
	})
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(r.JS, "hits") {
		t.Fatalf("js missing hits:\n%s", r.JS)
	}
}

func TestRunPromptCompleteShebang(t *testing.T) {
	prompt := bundles.PromptAction{
		ID:                  "prompt-complete",
		Slash:               "linear",
		CompleteInterpreter: bundles.InterpreterShebang,
		CompleteBody:        "#!/bin/sh\nprintf '%s\\n' '{\"hits\":[{\"insert\":\"ENG-1\",\"label\":\"ENG-1\",\"detail\":\"Fix login\",\"badge\":\"Issue\"},{\"label\":\"Connect Linear…\",\"action\":\"settings:integrations\"}],\"message\":\"ok\"}'\n",
	}
	res, err := bundles.RunPromptCompleteShebang(shebangCtx(t), prompt, bundles.PromptRunContext{Args: "ENG"})
	if err != nil {
		t.Fatal(err)
	}
	if res.Message != "ok" || len(res.Hits) != 2 {
		t.Fatalf("got %+v", res)
	}
	if res.Hits[0].Insert != "ENG-1" || res.Hits[0].Badge != "Issue" {
		t.Fatalf("hit0=%+v", res.Hits[0])
	}
	if res.Hits[1].Action != "settings:integrations" || res.Hits[1].Label != "Connect Linear…" {
		t.Fatalf("hit1=%+v", res.Hits[1])
	}
}

func TestRunPromptCompleteShebangEmpty(t *testing.T) {
	prompt := bundles.PromptAction{
		ID:                  "prompt-complete-empty",
		CompleteInterpreter: bundles.InterpreterShebang,
		CompleteBody:        "#!/bin/sh\nprintf '\\n'\n",
	}
	res, err := bundles.RunPromptCompleteShebang(shebangCtx(t), prompt, bundles.PromptRunContext{})
	if err != nil {
		t.Fatal(err)
	}
	if len(res.Hits) != 0 {
		t.Fatalf("hits=%v", res.Hits)
	}
}

func TestCollectSearchGroupsOneFailure(t *testing.T) {
	ok := bundles.SearchAction{
		ID: "ok", Name: "Good", Interpreter: bundles.InterpreterShebang,
		Body: "#!/bin/sh\nprintf '%s\\n' '{\"hits\":[{\"insert\":\"ENG-1\",\"label\":\"Fix\",\"psub\":\"ENG-1\"}]}'\n",
	}
	bad := bundles.SearchAction{
		ID: "bad", Name: "Bad", Interpreter: bundles.InterpreterShebang,
		Body: "#!/bin/sh\nexit 1\n",
	}
	groups := bundles.CollectSearchGroups(context.Background(), []bundles.SearchAction{ok, bad}, nil)
	if len(groups) != 2 {
		t.Fatalf("groups=%d", len(groups))
	}
	if groups[0].Name != "Good" || len(groups[0].Hits) != 1 || groups[0].Hits[0].Psub != "ENG-1" {
		t.Fatalf("good=%+v", groups[0])
	}
	if groups[1].Name != "Bad" || len(groups[1].Hits) != 0 {
		t.Fatalf("bad=%+v", groups[1])
	}
}

func TestExpandMatching(t *testing.T) {
	re := regexp.MustCompile(`(?i)^[A-Z][A-Z0-9]+-\d+`)
	match := bundles.ExpandAction{
		ID: "e", Name: "E", Interpreter: bundles.InterpreterShebang, MatchRE: re,
		Body: "#!/bin/sh\nprintf '{\"prompt\":\"Work on %s\",\"display\":\"Linear\"}\\n' \"$RECUR_PROMPT_ARGS\"\n",
	}
	skip := bundles.ExpandAction{
		ID: "s", Name: "S", Interpreter: bundles.InterpreterShebang,
		MatchRE: regexp.MustCompile(`^ENG-123`),
		Body:    "#!/bin/sh\nprintf '%s\\n' '{\"skip\":true}'\n",
	}
	prose, err := bundles.ExpandMatching("fix the flaky test", []bundles.ExpandAction{match}, nil)
	if err != nil || prose.Prompt != "fix the flaky test" {
		t.Fatalf("prose: %+v %v", prose, err)
	}
	hit, err := bundles.ExpandMatching("ENG-123 also tests", []bundles.ExpandAction{match}, nil)
	if err != nil || !strings.Contains(hit.Prompt, "ENG-123 also tests") {
		t.Fatalf("hit: %+v %v", hit, err)
	}
	skipped, err := bundles.ExpandMatching("ENG-123", []bundles.ExpandAction{skip, match}, nil)
	if err != nil || !strings.Contains(skipped.Prompt, "ENG-123") {
		t.Fatalf("skip then next: %+v %v", skipped, err)
	}
	empty := bundles.ExpandAction{ID: "never", Name: "Never"}
	if empty.Matches("ENG-123") {
		t.Fatal("empty match must not auto-run")
	}
}

func TestRunPromptShebang(t *testing.T) {
	prompt := bundles.PromptAction{
		ID:          "prompt-echo",
		Name:        "Echo Prompt",
		Interpreter: bundles.InterpreterShebang,
		// %s so JSON keeps a \n escape (valid); NormalizePromptDisplay flattens it.
		Body: "#!/bin/sh\nprintf '%s\\n' '{\"prompt\":\"wire @src/main.go /skill go-code-review\",\"display\":\"Echo\\nLabel\"}'\n",
	}
	rc := bundles.PromptRunContext{
		WorkspaceID:  "WS1",
		ProjectID:    "proj",
		Worktree:     t.TempDir(),
		Port:         4242,
		SelectedText: "sel",
		Clipboard:    "clip",
	}
	res, err := bundles.RunPromptShebang(shebangCtx(t), prompt, rc)
	if err != nil {
		t.Fatal(err)
	}
	if res.Prompt != "wire @src/main.go /skill go-code-review" {
		t.Fatalf("prompt=%q", res.Prompt)
	}
	if res.Display != "Echo Label" {
		t.Fatalf("display=%q", res.Display)
	}

	// Env should include workspace + editor vars.
	promptEnv := bundles.PromptAction{
		ID:          "prompt-env",
		Name:        "Env",
		Interpreter: bundles.InterpreterShebang,
		Body: `#!/bin/sh
printf '{"prompt":"id=%s port=%s clip=%s","display":"Env"}' \
  "$RECUR_WORKSPACE_ID" "$RECUR_PORT" "$RECUR_CLIPBOARD"
`,
	}
	res, err = bundles.RunPromptShebang(shebangCtx(t), promptEnv, rc)
	if err != nil {
		t.Fatal(err)
	}
	if res.Prompt != "id=WS1 port=4242 clip=clip" {
		t.Fatalf("env prompt=%q", res.Prompt)
	}

	promptBase := bundles.PromptAction{
		ID:          "prompt-base",
		Name:        "Base",
		Interpreter: bundles.InterpreterShebang,
		Body: `#!/bin/sh
printf '{"prompt":"base=%s method=%s","display":"Base"}' \
  "$RECUR_BASE_BRANCH" "$RECUR_UPDATE_BASE_METHOD"
`,
	}
	rc.BaseBranch = "origin/main"
	rc.UpdateBaseMethod = "rebase"
	res, err = bundles.RunPromptShebang(shebangCtx(t), promptBase, rc)
	if err != nil {
		t.Fatal(err)
	}
	if res.Prompt != "base=origin/main method=rebase" {
		t.Fatalf("base env prompt=%q, want %q", res.Prompt, "base=origin/main method=rebase")
	}

	promptPR := bundles.PromptAction{
		ID:          "prompt-pr",
		Name:        "PR",
		Interpreter: bundles.InterpreterShebang,
		Body: `#!/bin/sh
printf '{"prompt":"id=%s num=%s src=%s","display":"PR"}' \
  "$RECUR_PR_ID" "$RECUR_PR_NUMBER" "$RECUR_PR_SOURCE"
`,
	}
	rc.PRID = "gh-7"
	rc.PRNumber = 7
	rc.PRSource = "gh"
	res, err = bundles.RunPromptShebang(shebangCtx(t), promptPR, rc)
	if err != nil {
		t.Fatal(err)
	}
	if res.Prompt != "id=gh-7 num=7 src=gh" {
		t.Fatalf("pr env prompt=%q, want %q", res.Prompt, "id=gh-7 num=7 src=gh")
	}
}

func TestRunPromptShebangRejectsTypeScript(t *testing.T) {
	_, err := bundles.RunPromptShebang(shebangCtx(t), bundles.PromptAction{
		ID: "p", Name: "X", Interpreter: bundles.InterpreterTypeScript, Body: "x",
	}, bundles.PromptRunContext{})
	if err == nil || !strings.Contains(err.Error(), "CompiledPromptModule") {
		t.Fatalf("err=%v", err)
	}
}

func TestRunPromptShebangBadJSONAndEmptyPrompt(t *testing.T) {
	_, err := bundles.RunPromptShebang(shebangCtx(t), bundles.PromptAction{
		ID: "p", Name: "X", Interpreter: bundles.InterpreterShebang,
		Body: "#!/bin/sh\necho not-json\n",
	}, bundles.PromptRunContext{WorkDir: t.TempDir()})
	if err == nil || !strings.Contains(err.Error(), "JSON") {
		t.Fatalf("RunPromptShebang(bad JSON) err=%v, want JSON error", err)
	}
	res, err := bundles.RunPromptShebang(shebangCtx(t), bundles.PromptAction{
		ID: "p2", Name: "X", Interpreter: bundles.InterpreterShebang,
		Body: "#!/bin/sh\nprintf '{\"prompt\":\"\",\"display\":\"Updated with latest main\"}'\n",
	}, bundles.PromptRunContext{WorkDir: t.TempDir()})
	if err != nil {
		t.Fatalf("RunPromptShebang(empty prompt) err=%v, want nil", err)
	}
	if res.Prompt != "" {
		t.Errorf("RunPromptShebang(empty prompt).Prompt = %q, want empty", res.Prompt)
	}
	if res.Display != "Updated with latest main" {
		t.Errorf("RunPromptShebang(empty prompt).Display = %q, want %q", res.Display, "Updated with latest main")
	}
}

func TestRunPromptShebangTimeout(t *testing.T) {
	prompt := bundles.PromptAction{
		ID: "sleep", Name: "Sleep", Interpreter: bundles.InterpreterShebang,
		TimeoutMs: 200,
		Body:      "#!/bin/sh\nsleep 5\n",
	}
	// No deadline on ctx so manifest TimeoutMs applies.
	_, err := bundles.RunPromptShebang(context.Background(), prompt, bundles.PromptRunContext{WorkDir: t.TempDir()})
	if err == nil || (!strings.Contains(err.Error(), "timed out") && !strings.Contains(err.Error(), "cancelled")) {
		t.Fatalf("err=%v", err)
	}
}

func TestRunPromptShebangSourcesViaBundleSupport(t *testing.T) {
	// Mirrors shipped Chat review.sh: RunPromptShebang copies the body to a
	// temp file, so sibling scripts must be sourced via RECUR_BUNDLE_SUPPORT.
	dir := t.TempDir()
	support := filepath.Join(dir, "src")
	if err := os.MkdirAll(support, 0o755); err != nil {
		t.Fatal(err)
	}
	helper := filepath.Join(support, "helper.sh")
	if err := os.WriteFile(helper, []byte("HELPER_OK=1\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	prompt := bundles.PromptAction{
		ID: "src-sib", Name: "Src", Interpreter: bundles.InterpreterShebang,
		BundleDir: dir,
		Body: `#!/bin/sh
set -e
. "${RECUR_BUNDLE_SUPPORT}/src/helper.sh"
printf '{"prompt":"ok=%s","display":"Src"}' "$HELPER_OK"
`,
	}
	res, err := bundles.RunPromptShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		WorkDir: dir, BundleSupport: dir,
	})
	if err != nil {
		t.Fatal(err)
	}
	if res.Prompt != "ok=1" {
		t.Fatalf("prompt=%q", res.Prompt)
	}
}

func TestPromptEnvironCapsLargeDocument(t *testing.T) {
	const cap = 64 * 1024 // keep in sync with promptEnvBlobMax
	big := strings.Repeat("x", cap+1024)
	env := bundles.PromptEnviron(bundles.PromptRunContext{Document: big, Clipboard: big})
	var doc, clip string
	for _, e := range env {
		if strings.HasPrefix(e, "RECUR_DOCUMENT=") {
			doc = strings.TrimPrefix(e, "RECUR_DOCUMENT=")
		}
		if strings.HasPrefix(e, "RECUR_CLIPBOARD=") {
			clip = strings.TrimPrefix(e, "RECUR_CLIPBOARD=")
		}
	}
	if len(doc) >= len(big) || !strings.Contains(doc, "truncated") {
		t.Fatalf("document should be capped with note, len=%d", len(doc))
	}
	if len(clip) >= len(big) || !strings.Contains(clip, "truncated") {
		t.Fatalf("clipboard should be capped with note, len=%d", len(clip))
	}
}

func TestPromptEnvironIncludesArgs(t *testing.T) {
	env := bundles.PromptEnviron(bundles.PromptRunContext{Args: "ENG-123 extra", PromptSlash: "linear"})
	var args, slash string
	for _, e := range env {
		if strings.HasPrefix(e, "RECUR_PROMPT_ARGS=") {
			args = strings.TrimPrefix(e, "RECUR_PROMPT_ARGS=")
		}
		if strings.HasPrefix(e, "RECUR_PROMPT_SLASH=") {
			slash = strings.TrimPrefix(e, "RECUR_PROMPT_SLASH=")
		}
	}
	if args != "ENG-123 extra" {
		t.Fatalf("RECUR_PROMPT_ARGS=%q", args)
	}
	if slash != "linear" {
		t.Fatalf("RECUR_PROMPT_SLASH=%q", slash)
	}
}

func TestPromptEnvironIncludesSearchQuery(t *testing.T) {
	env := bundles.PromptEnviron(bundles.PromptRunContext{SearchQuery: "ACME-114"})
	var q string
	for _, e := range env {
		if strings.HasPrefix(e, "RECUR_SEARCH_QUERY=") {
			q = strings.TrimPrefix(e, "RECUR_SEARCH_QUERY=")
		}
	}
	if q != "ACME-114" {
		t.Fatalf("RECUR_SEARCH_QUERY=%q", q)
	}
}

func TestPromptEnvironIncludesParams(t *testing.T) {
	env := bundles.PromptEnviron(bundles.PromptRunContext{
		Args:        "ENG-123",
		CompleteArg: "issue",
		Params:      map[string]string{"issue": "ENG-123"},
	})
	var issue, params, completeArg string
	for _, e := range env {
		if strings.HasPrefix(e, "RECUR_PROMPT_ARG_ISSUE=") {
			issue = strings.TrimPrefix(e, "RECUR_PROMPT_ARG_ISSUE=")
		}
		if strings.HasPrefix(e, "RECUR_PROMPT_PARAMS=") {
			params = strings.TrimPrefix(e, "RECUR_PROMPT_PARAMS=")
		}
		if strings.HasPrefix(e, "RECUR_PROMPT_COMPLETE_ARG=") {
			completeArg = strings.TrimPrefix(e, "RECUR_PROMPT_COMPLETE_ARG=")
		}
	}
	if issue != "ENG-123" {
		t.Fatalf("RECUR_PROMPT_ARG_ISSUE=%q", issue)
	}
	if !strings.Contains(params, `"issue":"ENG-123"`) {
		t.Fatalf("RECUR_PROMPT_PARAMS=%q", params)
	}
	if completeArg != "issue" {
		t.Fatalf("RECUR_PROMPT_COMPLETE_ARG=%q", completeArg)
	}
}

func TestPromptEnvironBundleSettings(t *testing.T) {
	env := bundles.PromptEnviron(bundles.PromptRunContext{
		BundleSettings: map[string]string{"api_key": "lin_secret", "team_key": "ENG"},
	})
	var key, team, blob string
	for _, e := range env {
		if strings.HasPrefix(e, "RECUR_SETTING_API_KEY=") {
			key = strings.TrimPrefix(e, "RECUR_SETTING_API_KEY=")
		}
		if strings.HasPrefix(e, "RECUR_SETTING_TEAM_KEY=") {
			team = strings.TrimPrefix(e, "RECUR_SETTING_TEAM_KEY=")
		}
		if strings.HasPrefix(e, "RECUR_BUNDLE_SETTINGS=") {
			blob = strings.TrimPrefix(e, "RECUR_BUNDLE_SETTINGS=")
		}
	}
	if key != "lin_secret" || team != "ENG" {
		t.Fatalf("key=%q team=%q", key, team)
	}
	if !strings.Contains(blob, `"api_key":"lin_secret"`) || !strings.Contains(blob, `"team_key":"ENG"`) {
		t.Fatalf("RECUR_BUNDLE_SETTINGS=%q", blob)
	}
}

func TestPromptEnvironOmitsEmptyBundleSettings(t *testing.T) {
	env := bundles.PromptEnviron(bundles.PromptRunContext{})
	for _, e := range env {
		if strings.HasPrefix(e, "RECUR_SETTING_") || strings.HasPrefix(e, "RECUR_BUNDLE_SETTINGS=") {
			t.Fatalf("unexpected %q", e)
		}
		if strings.HasPrefix(e, "LINEAR_API_KEY=") {
			t.Fatalf("must not inject LINEAR_API_KEY: %q", e)
		}
	}
}

func TestParamsFromArgs(t *testing.T) {
	one := bundles.PromptAction{Args: []bundles.PromptArgSpec{{Name: "issue"}}}
	got := bundles.ParamsFromArgs(one, "ENG-1 extra")
	if got["issue"] != "ENG-1 extra" {
		t.Fatalf("one=%v", got)
	}
	multi := bundles.PromptAction{Args: []bundles.PromptArgSpec{
		{Name: "question"}, {Name: "choice_1"}, {Name: "choice_2"},
	}}
	got = bundles.ParamsFromArgs(multi, `"Which color?" Red Blue extra`)
	if got["question"] != "Which color?" || got["choice_1"] != "Red" || got["choice_2"] != "Blue extra" {
		t.Fatalf("quoted=%v", got)
	}
}

func TestNormalizePromptDisplay(t *testing.T) {
	if got := bundles.NormalizePromptDisplay("", "Fix Findings"); got != "Fix Findings" {
		t.Fatalf("fallback=%q", got)
	}
	long := strings.Repeat("a", 200)
	got := bundles.NormalizePromptDisplay(long, "x")
	if len([]rune(got)) != bundles.PromptDisplayMaxLen || !strings.HasSuffix(got, "…") {
		t.Fatalf("cap=%q len=%d", got, len([]rune(got)))
	}
}

func TestUpdatePromptShebangCleanAndConflict(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller")
	}
	bundleDir := filepath.Join(filepath.Dir(thisFile), "../../bundles/Chat")
	body, err := os.ReadFile(filepath.Join(bundleDir, "src/update.sh"))
	if err != nil {
		t.Fatalf("read update.sh: %v", err)
	}
	prompt := bundles.PromptAction{
		ID: "recur.chat.update", Name: "Update",
		Interpreter: bundles.InterpreterShebang,
		BundleDir:   bundleDir,
		Body:        string(body),
	}

	repo := t.TempDir()
	run := func(dir string, args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = dir
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run(repo, "init", "-b", "main")
	run(repo, "config", "user.name", "test")
	run(repo, "config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(repo, "README.md"), []byte("hi\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(repo, "add", "README.md")
	run(repo, "commit", "-m", "init")
	run(repo, "checkout", "-b", "feature")
	if err := os.WriteFile(filepath.Join(repo, "feature.txt"), []byte("feat\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(repo, "add", "feature.txt")
	run(repo, "commit", "-m", "feature")
	run(repo, "checkout", "main")
	if err := os.WriteFile(filepath.Join(repo, "extra.txt"), []byte("base moved\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(repo, "add", "extra.txt")
	run(repo, "commit", "-m", "advance main")
	run(repo, "checkout", "feature")

	rc := bundles.PromptRunContext{
		Worktree: repo, WorkDir: repo, BundleSupport: bundleDir,
		BaseBranch: "main", UpdateBaseMethod: "merge",
	}
	if _, err := bundles.RunPromptShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: repo, WorkDir: repo, BundleSupport: bundleDir,
		BaseBranch: "no-such-branch", UpdateBaseMethod: "merge",
	}); err == nil {
		t.Fatal("RunPromptShebang(update invalid base) err=nil, want error")
	}
	res, err := bundles.RunPromptShebang(shebangCtx(t), prompt, rc)
	if err != nil {
		t.Fatalf("RunPromptShebang(update clean) err=%v, want nil", err)
	}
	if res.Prompt != "" {
		t.Errorf("RunPromptShebang(update clean).Prompt = %q, want empty", res.Prompt)
	}
	if !strings.Contains(res.Display, "Updated with latest") {
		t.Errorf("RunPromptShebang(update clean).Display = %q, want Updated with latest…", res.Display)
	}
	if out, gerr := exec.Command("git", "-C", repo, "show", "HEAD:extra.txt").CombinedOutput(); gerr != nil || strings.TrimSpace(string(out)) != "base moved" {
		t.Fatalf("extra.txt after update: %v (%s)", gerr, out)
	}

	// Second run: conflict on README.md.
	run(repo, "checkout", "main")
	if err := os.WriteFile(filepath.Join(repo, "README.md"), []byte("main edit\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(repo, "add", "README.md")
	run(repo, "commit", "-m", "main readme")
	run(repo, "checkout", "feature")
	if err := os.WriteFile(filepath.Join(repo, "README.md"), []byte("feature edit\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(repo, "add", "README.md")
	run(repo, "commit", "-m", "feature readme")

	res, err = bundles.RunPromptShebang(shebangCtx(t), prompt, rc)
	if err != nil {
		t.Fatalf("RunPromptShebang(update conflict) err=%v, want nil", err)
	}
	if !strings.Contains(res.Prompt, "conflict") {
		t.Errorf("RunPromptShebang(update conflict).Prompt = %q, want conflict instructions", res.Prompt)
	}
	if _, gerr := exec.Command("git", "-C", repo, "rev-parse", "-q", "--verify", "MERGE_HEAD").Output(); gerr != nil {
		t.Error("RunPromptShebang(update conflict) should leave MERGE_HEAD")
	}
}

func TestUpdatePromptShebangDirtyWorktreeDefersToAgent(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller")
	}
	bundleDir := filepath.Join(filepath.Dir(thisFile), "../../bundles/Chat")
	body, err := os.ReadFile(filepath.Join(bundleDir, "src/update.sh"))
	if err != nil {
		t.Fatalf("read update.sh: %v", err)
	}
	prompt := bundles.PromptAction{
		ID: "recur.chat.update", Name: "Update",
		Interpreter: bundles.InterpreterShebang,
		BundleDir:   bundleDir,
		Body:        string(body),
	}

	repo := t.TempDir()
	run := func(dir string, args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = dir
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run(repo, "init", "-b", "main")
	run(repo, "config", "user.name", "test")
	run(repo, "config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(repo, "README.md"), []byte("hi\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(repo, "add", "README.md")
	run(repo, "commit", "-m", "init")
	run(repo, "checkout", "-b", "feature")
	if err := os.WriteFile(filepath.Join(repo, "feature.txt"), []byte("feat\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(repo, "add", "feature.txt")
	run(repo, "commit", "-m", "feature")
	run(repo, "checkout", "main")
	if err := os.WriteFile(filepath.Join(repo, "README.md"), []byte("main edit\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(repo, "add", "README.md")
	run(repo, "commit", "-m", "advance main")
	run(repo, "checkout", "feature")
	if err := os.WriteFile(filepath.Join(repo, "README.md"), []byte("local dirty\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	res, err := bundles.RunPromptShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: repo, WorkDir: repo, BundleSupport: bundleDir,
		BaseBranch: "main", UpdateBaseMethod: "merge",
	})
	if err != nil {
		t.Fatalf("RunPromptShebang(update dirty) err=%v, want nil (defer to agent)", err)
	}
	if res.Prompt == "" {
		t.Fatal("RunPromptShebang(update dirty).Prompt is empty, want agent handoff")
	}
	if !strings.Contains(res.Prompt, "overwritten") {
		t.Errorf("RunPromptShebang(update dirty).Prompt = %q, want git 'overwritten' failure", res.Prompt)
	}
	if !strings.Contains(res.Display, "Finish updating") {
		t.Errorf("RunPromptShebang(update dirty).Display = %q, want Finish updating…", res.Display)
	}
	if _, gerr := exec.Command("git", "-C", repo, "rev-parse", "-q", "--verify", "MERGE_HEAD").Output(); gerr == nil {
		t.Error("RunPromptShebang(update dirty) should not leave MERGE_HEAD")
	}
	got, rerr := os.ReadFile(filepath.Join(repo, "README.md"))
	if rerr != nil {
		t.Fatalf("read README.md: %v", rerr)
	}
	if strings.TrimSpace(string(got)) != "local dirty" {
		t.Errorf("README.md after dirty update = %q, want %q (local work preserved)", strings.TrimSpace(string(got)), "local dirty")
	}
}

func TestOpenPRPromptShebangEmbedsDiffAndOpenPRTool(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller")
	}
	bundleDir := filepath.Join(filepath.Dir(thisFile), "../../bundles/Chat")
	body, err := os.ReadFile(filepath.Join(bundleDir, "src/open-pr.sh"))
	if err != nil {
		t.Fatalf("read open-pr.sh: %v", err)
	}
	prompt := bundles.PromptAction{
		ID: "recur.chat.open-pr", Name: "Open PR",
		Interpreter: bundles.InterpreterShebang,
		BundleDir:   bundleDir,
		Body:        string(body),
	}

	repo := t.TempDir()
	run := func(dir string, args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = dir
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run(repo, "init", "-b", "main")
	run(repo, "config", "user.name", "test")
	run(repo, "config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(repo, "README.md"), []byte("hi\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(repo, "add", "README.md")
	run(repo, "commit", "-m", "init")
	run(repo, "checkout", "-b", "feature")
	if err := os.WriteFile(filepath.Join(repo, "feature.txt"), []byte("feat\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(repo, "add", "feature.txt")
	run(repo, "commit", "-m", "feature")

	res, err := bundles.RunPromptShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: repo, WorkDir: repo, BundleSupport: bundleDir,
		BaseBranch: "main",
	})
	if err != nil {
		t.Fatalf("RunPromptShebang(open-pr) err=%v", err)
	}
	if res.Display != "Open a pull request" {
		t.Errorf("RunPromptShebang(open-pr).Display = %q, want %q", res.Display, "Open a pull request")
	}
	for _, want := range []string{
		"mcp__recur__open_pr",
		"Do not run `gh pr create` yourself",
		"feature.txt",
		"Current branch: feature",
		"Compare base: main",
	} {
		if !strings.Contains(res.Prompt, want) {
			t.Errorf("RunPromptShebang(open-pr).Prompt missing %q, got %q", want, res.Prompt)
		}
	}
	if strings.Contains(res.Prompt, "Opened by Recur") {
		t.Errorf("RunPromptShebang(open-pr).Prompt must not stub the GitHub body, got %q", res.Prompt)
	}
	if strings.Contains(res.Prompt, "draft set to true") {
		t.Errorf("RunPromptShebang(open-pr).Prompt must not request a draft, got %q", res.Prompt)
	}
}

func TestOpenPRPromptShebangFastFailsWhenPRAlreadyOpen(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller")
	}
	bundleDir := filepath.Join(filepath.Dir(thisFile), "../../bundles/Chat")
	body, err := os.ReadFile(filepath.Join(bundleDir, "src/open-pr.sh"))
	if err != nil {
		t.Fatalf("read open-pr.sh: %v", err)
	}
	prompt := bundles.PromptAction{
		ID: "recur.chat.open-pr", Name: "Open PR",
		Interpreter: bundles.InterpreterShebang,
		BundleDir:   bundleDir,
		Body:        string(body),
	}
	res, err := bundles.RunPromptShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: t.TempDir(), WorkDir: t.TempDir(), BundleSupport: bundleDir,
		PRID: "gh-12", PRNumber: 12, PRSource: "gh",
	})
	if err != nil {
		t.Fatalf("RunPromptShebang(open-pr already open) err=%v", err)
	}
	if res.Prompt != "" {
		t.Errorf("RunPromptShebang(open-pr already open).Prompt = %q, want empty (no LLM turn)", res.Prompt)
	}
	if res.Display != "PR #12 is already open" {
		t.Errorf("RunPromptShebang(open-pr already open).Display = %q, want %q", res.Display, "PR #12 is already open")
	}
}

func TestOpenPRPromptShebangDoesNotFastFailOnLocalPR(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller")
	}
	bundleDir := filepath.Join(filepath.Dir(thisFile), "../../bundles/Chat")
	body, err := os.ReadFile(filepath.Join(bundleDir, "src/open-pr.sh"))
	if err != nil {
		t.Fatalf("read open-pr.sh: %v", err)
	}
	prompt := bundles.PromptAction{
		ID: "recur.chat.open-pr", Name: "Open PR",
		Interpreter: bundles.InterpreterShebang,
		BundleDir:   bundleDir,
		Body:        string(body),
	}
	repo := t.TempDir()
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = repo
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-b", "main")
	run("config", "user.name", "test")
	run("config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(repo, "f.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", "f.txt")
	run("commit", "-m", "init")

	res, err := bundles.RunPromptShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: repo, WorkDir: repo, BundleSupport: bundleDir,
		PRID: "7", PRNumber: 7, PRSource: "local",
		BaseBranch: "main",
	})
	if err != nil {
		t.Fatalf("RunPromptShebang(open-pr local PR) err=%v", err)
	}
	if res.Prompt == "" {
		t.Error("RunPromptShebang(open-pr local PR).Prompt is empty, want an agent turn so GitHub create can retry")
	}
	if strings.Contains(res.Display, "already open") {
		t.Errorf("RunPromptShebang(open-pr local PR).Display = %q, must not fast-fail", res.Display)
	}
}

func TestOpenDraftPRPromptShebangAsksForDraftToolArg(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller")
	}
	bundleDir := filepath.Join(filepath.Dir(thisFile), "../../bundles/Chat")
	body, err := os.ReadFile(filepath.Join(bundleDir, "src/open-pr.sh"))
	if err != nil {
		t.Fatalf("read open-pr.sh: %v", err)
	}
	prompt := bundles.PromptAction{
		ID: "recur.chat.open-draft-pr", Name: "Open draft PR", Slash: "open-draft-pr",
		Interpreter: bundles.InterpreterShebang,
		BundleDir:   bundleDir,
		Body:        string(body),
	}
	repo := t.TempDir()
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = repo
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-b", "main")
	run("config", "user.name", "test")
	run("config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(repo, "f.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", "f.txt")
	run("commit", "-m", "init")

	res, err := bundles.RunPromptShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: repo, WorkDir: repo, BundleSupport: bundleDir,
		BaseBranch: "main",
	})
	if err != nil {
		t.Fatalf("RunPromptShebang(open-draft-pr) err=%v", err)
	}
	if res.Display != "Open a draft pull request" {
		t.Errorf("RunPromptShebang(open-draft-pr).Display = %q, want %q", res.Display, "Open a draft pull request")
	}
	for _, want := range []string{
		"draft pull request",
		"draft set to true",
		"mcp__recur__open_pr",
	} {
		if !strings.Contains(res.Prompt, want) {
			t.Errorf("RunPromptShebang(open-draft-pr).Prompt missing %q, got %q", want, res.Prompt)
		}
	}
}

func chatFixChecksPrompt(t *testing.T) bundles.PromptAction {
	t.Helper()
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller")
	}
	bundleDir := filepath.Join(filepath.Dir(thisFile), "../../bundles/Chat")
	body, err := os.ReadFile(filepath.Join(bundleDir, "src/fix-checks.sh"))
	if err != nil {
		t.Fatalf("read fix-checks.sh: %v", err)
	}
	return bundles.PromptAction{
		ID: "recur.chat.fix-checks", Name: "Fix checks", Slash: "fix-checks",
		Interpreter: bundles.InterpreterShebang,
		BundleDir:   bundleDir,
		Body:        string(body),
	}
}

func installFakeGHChecks(t *testing.T, payload string) {
	t.Helper()
	bin := t.TempDir()
	out := filepath.Join(bin, "checks.json")
	if err := os.WriteFile(out, []byte(payload), 0o644); err != nil {
		t.Fatal(err)
	}
	// Match `gh pr checks`: 1 when any check failed, 8 while pending, 0 otherwise.
	exit := 0
	switch {
	case strings.Contains(payload, `"bucket":"fail"`), strings.Contains(payload, `"state":"FAILURE"`):
		exit = 1
	case strings.Contains(payload, `"bucket":"pending"`), strings.Contains(payload, `"state":"PENDING"`):
		exit = 8
	}
	script := "#!/bin/sh\ncat " + strconv.Quote(out) + "\nexit " + strconv.Itoa(exit) + "\n"
	if err := os.WriteFile(filepath.Join(bin, "gh"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
}

func TestFixChecksPromptShebangEmbedsFailingChecks(t *testing.T) {
	prompt := chatFixChecksPrompt(t)
	installFakeGHChecks(t, `[{"name":"lint","state":"FAILURE","bucket":"fail","link":"https://example.com/lint","description":"exit 1"},{"name":"build","state":"SUCCESS","bucket":"pass","link":"https://example.com/build","description":""}]`)

	res, err := bundles.RunPromptShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: t.TempDir(), WorkDir: t.TempDir(), BundleSupport: prompt.BundleDir,
		PRNumber: 12, PRSource: "gh", PRURL: "https://github.com/acme/r/pull/12",
		PRTitle: "OAuth", PRBranch: "feat/oauth",
	})
	if err != nil {
		t.Fatalf("RunPromptShebang(fix-checks) err=%v", err)
	}
	if res.Display != "Fix 1 failing GitHub check" {
		t.Errorf("RunPromptShebang(fix-checks).Display = %q, want Fix 1 failing GitHub check", res.Display)
	}
	for _, want := range []string{
		"GitHub pull request #12",
		"gh run view --log-failed",
		`"name": "lint"`,
		"do not force-push",
		"feat/oauth",
	} {
		if !strings.Contains(res.Prompt, want) {
			t.Errorf("RunPromptShebang(fix-checks).Prompt missing %q, got %q", want, res.Prompt)
		}
	}
	if strings.Contains(res.Prompt, `"name": "build"`) {
		t.Errorf("RunPromptShebang(fix-checks).Prompt included a passing check, got %q", res.Prompt)
	}
	prefix, _, ok := strings.Cut(res.Prompt, "⟦raw:")
	if !ok {
		t.Errorf("RunPromptShebang(fix-checks).Prompt missing raw region, got %q", res.Prompt)
	} else if strings.Contains(prefix, "OAuth") {
		t.Errorf("RunPromptShebang(fix-checks).Prompt leaked PR title outside raw region, got %q", res.Prompt)
	}
}

func TestFixChecksPromptShebangFocusesNamedCheck(t *testing.T) {
	prompt := chatFixChecksPrompt(t)
	installFakeGHChecks(t, `[{"name":"lint","state":"FAILURE","bucket":"fail","link":"https://example.com/lint","description":"exit 1"},{"name":"typecheck","state":"FAILURE","bucket":"fail","link":"https://example.com/ts","description":"tsc"}]`)

	res, err := bundles.RunPromptShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: t.TempDir(), WorkDir: t.TempDir(), BundleSupport: prompt.BundleDir,
		PRNumber: 12, PRSource: "gh", Args: "lint",
	})
	if err != nil {
		t.Fatalf("RunPromptShebang(fix-checks lint) err=%v", err)
	}
	if res.Display != "Fix failing check: lint" {
		t.Errorf("RunPromptShebang(fix-checks lint).Display = %q", res.Display)
	}
	if !strings.Contains(res.Prompt, `Focus on the check named "lint"`) {
		t.Errorf("RunPromptShebang(fix-checks lint).Prompt missing focus, got %q", res.Prompt)
	}
	prefix, _, ok := strings.Cut(res.Prompt, "⟦raw:")
	if !ok {
		t.Errorf("RunPromptShebang(fix-checks lint).Prompt missing raw region, got %q", res.Prompt)
	} else if strings.Contains(prefix, `Focus on the check named`) {
		t.Errorf("RunPromptShebang(fix-checks lint).Prompt leaked check name outside raw region, got %q", res.Prompt)
	}
	if strings.Contains(res.Prompt, "typecheck") {
		t.Errorf("RunPromptShebang(fix-checks lint).Prompt included an unfocused failure, got %q", res.Prompt)
	}
}

func TestFixChecksPromptShebangFastFailsWhenPassing(t *testing.T) {
	prompt := chatFixChecksPrompt(t)
	installFakeGHChecks(t, `[{"name":"lint","state":"SUCCESS","bucket":"pass","link":"https://example.com/lint","description":""}]`)

	res, err := bundles.RunPromptShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: t.TempDir(), WorkDir: t.TempDir(), BundleSupport: prompt.BundleDir,
		PRNumber: 12, PRSource: "gh",
	})
	if err != nil {
		t.Fatalf("RunPromptShebang(fix-checks passing) err=%v", err)
	}
	if res.Prompt != "" {
		t.Errorf("RunPromptShebang(fix-checks passing).Prompt = %q, want empty", res.Prompt)
	}
	if res.Display != "All checks passed" {
		t.Errorf("RunPromptShebang(fix-checks passing).Display = %q", res.Display)
	}
}

func TestFixChecksPromptShebangFastFailsWithoutGitHubPR(t *testing.T) {
	prompt := chatFixChecksPrompt(t)
	res, err := bundles.RunPromptShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: t.TempDir(), WorkDir: t.TempDir(), BundleSupport: prompt.BundleDir,
	})
	if err != nil {
		t.Fatalf("RunPromptShebang(fix-checks no pr) err=%v", err)
	}
	if res.Prompt != "" {
		t.Errorf("RunPromptShebang(fix-checks no pr).Prompt = %q, want empty", res.Prompt)
	}
	if res.Display != "No GitHub pull request on this workspace" {
		t.Errorf("RunPromptShebang(fix-checks no pr).Display = %q", res.Display)
	}
}

func TestFixChecksPromptShebangFastFailsOnPRHost(t *testing.T) {
	prompt := chatFixChecksPrompt(t)
	res, err := bundles.RunPromptShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: t.TempDir(), WorkDir: t.TempDir(), BundleSupport: prompt.BundleDir,
		PRNumber: 12, PRSource: "gh", PRHost: true,
	})
	if err != nil {
		t.Fatalf("RunPromptShebang(fix-checks host) err=%v", err)
	}
	if res.Prompt != "" {
		t.Errorf("RunPromptShebang(fix-checks host).Prompt = %q, want empty", res.Prompt)
	}
	if res.Display != "Create a workspace from this pull request to edit files" {
		t.Errorf("RunPromptShebang(fix-checks host).Display = %q", res.Display)
	}
}

func TestFixChecksPromptShebangTreatsAllAsUnfocused(t *testing.T) {
	prompt := chatFixChecksPrompt(t)
	installFakeGHChecks(t, `[{"name":"lint","state":"FAILURE","bucket":"fail","link":"https://example.com/lint","description":"exit 1"},{"name":"typecheck","state":"FAILURE","bucket":"fail","link":"https://example.com/ts","description":"tsc"}]`)

	res, err := bundles.RunPromptShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: t.TempDir(), WorkDir: t.TempDir(), BundleSupport: prompt.BundleDir,
		PRNumber: 12, PRSource: "gh", Args: "all",
	})
	if err != nil {
		t.Fatalf("RunPromptShebang(fix-checks all) err=%v", err)
	}
	if res.Display != "Fix 2 failing GitHub checks" {
		t.Errorf("RunPromptShebang(fix-checks all).Display = %q, want Fix 2 failing GitHub checks", res.Display)
	}
	if strings.Contains(res.Prompt, `Focus on the check named`) {
		t.Errorf("RunPromptShebang(fix-checks all).Prompt focused a check, got %q", res.Prompt)
	}
	if !strings.Contains(res.Prompt, `"name": "lint"`) || !strings.Contains(res.Prompt, `"name": "typecheck"`) {
		t.Errorf("RunPromptShebang(fix-checks all).Prompt missing both failures, got %q", res.Prompt)
	}
}

func TestFixChecksPromptShebangPrefersNamedParam(t *testing.T) {
	prompt := chatFixChecksPrompt(t)
	installFakeGHChecks(t, `[{"name":"lint","state":"FAILURE","bucket":"fail","link":"https://example.com/lint","description":"exit 1"},{"name":"typecheck","state":"FAILURE","bucket":"fail","link":"https://example.com/ts","description":"tsc"}]`)

	res, err := bundles.RunPromptShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: t.TempDir(), WorkDir: t.TempDir(), BundleSupport: prompt.BundleDir,
		PRNumber: 12, PRSource: "gh", Args: "all", Params: map[string]string{"check": "lint"},
	})
	if err != nil {
		t.Fatalf("RunPromptShebang(fix-checks param) err=%v", err)
	}
	if res.Display != "Fix failing check: lint" {
		t.Errorf("RunPromptShebang(fix-checks param).Display = %q, want Fix failing check: lint", res.Display)
	}
	if strings.Contains(res.Prompt, "typecheck") {
		t.Errorf("RunPromptShebang(fix-checks param).Prompt included an unfocused failure, got %q", res.Prompt)
	}
}

func chatFixChecksComplete(t *testing.T) bundles.PromptAction {
	t.Helper()
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller")
	}
	bundleDir := filepath.Join(filepath.Dir(thisFile), "../../bundles/Chat")
	body, err := os.ReadFile(filepath.Join(bundleDir, "src/complete-checks.sh"))
	if err != nil {
		t.Fatalf("read complete-checks.sh: %v", err)
	}
	return bundles.PromptAction{
		ID: "recur.chat.fix-checks", Name: "Fix checks", Slash: "fix-checks",
		CompleteInterpreter: bundles.InterpreterShebang,
		CompleteBody:        string(body),
		BundleDir:           bundleDir,
	}
}

func TestFixChecksCompleteListsAllAndFailing(t *testing.T) {
	prompt := chatFixChecksComplete(t)
	installFakeGHChecks(t, `[{"name":"lint","state":"FAILURE","bucket":"fail"},{"name":"build","state":"SUCCESS","bucket":"pass"},{"name":"typecheck","state":"FAILURE","bucket":"fail"}]`)

	res, err := bundles.RunPromptCompleteShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: t.TempDir(), WorkDir: t.TempDir(), BundleSupport: prompt.BundleDir,
		PRNumber: 12, PRSource: "gh", CompleteArg: "check",
	})
	if err != nil {
		t.Fatalf("RunPromptCompleteShebang(fix-checks) err=%v", err)
	}
	if len(res.Hits) != 3 {
		t.Fatalf("RunPromptCompleteShebang(fix-checks) hits=%d, want 3; %+v", len(res.Hits), res.Hits)
	}
	if res.Hits[0].Insert != "all" || res.Hits[0].Badge != "All" {
		t.Errorf("RunPromptCompleteShebang(fix-checks) hit0=%+v, want all", res.Hits[0])
	}
	got := map[string]bool{}
	for _, h := range res.Hits[1:] {
		got[h.Insert] = true
		if h.Badge != "Check" {
			t.Errorf("RunPromptCompleteShebang(fix-checks) hit=%+v, want badge Check", h)
		}
	}
	if !got["lint"] || !got["typecheck"] || got["build"] {
		t.Errorf("RunPromptCompleteShebang(fix-checks) names=%v, want lint and typecheck", got)
	}
}

func TestFixChecksCompleteFiltersByTerm(t *testing.T) {
	prompt := chatFixChecksComplete(t)
	installFakeGHChecks(t, `[{"name":"lint","state":"FAILURE","bucket":"fail"},{"name":"typecheck","state":"FAILURE","bucket":"fail"}]`)

	res, err := bundles.RunPromptCompleteShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: t.TempDir(), WorkDir: t.TempDir(), BundleSupport: prompt.BundleDir,
		PRNumber: 12, PRSource: "gh", CompleteArg: "check", Args: "lint",
	})
	if err != nil {
		t.Fatalf("RunPromptCompleteShebang(fix-checks lint) err=%v", err)
	}
	if len(res.Hits) != 1 || res.Hits[0].Insert != "lint" {
		t.Errorf("RunPromptCompleteShebang(fix-checks lint) hits=%+v, want [lint]", res.Hits)
	}
}

func TestFixChecksCompleteWithoutGitHubPR(t *testing.T) {
	prompt := chatFixChecksComplete(t)
	res, err := bundles.RunPromptCompleteShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: t.TempDir(), WorkDir: t.TempDir(), BundleSupport: prompt.BundleDir,
		CompleteArg: "check",
	})
	if err != nil {
		t.Fatalf("RunPromptCompleteShebang(fix-checks no pr) err=%v", err)
	}
	if res.Message != "No GitHub pull request on this workspace" {
		t.Errorf("RunPromptCompleteShebang(fix-checks no pr).Message = %q", res.Message)
	}
	if len(res.Hits) != 1 || res.Hits[0].Insert != "all" {
		t.Errorf("RunPromptCompleteShebang(fix-checks no pr) hits=%+v, want [all]", res.Hits)
	}
}

func TestReviewPromptAdaptsToSurface(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller")
	}
	bundleDir := filepath.Join(filepath.Dir(thisFile), "../../bundles/Chat")
	body, err := os.ReadFile(filepath.Join(bundleDir, "src/review.sh"))
	if err != nil {
		t.Fatalf("read review.sh: %v", err)
	}
	prompt := bundles.PromptAction{
		ID: "recur.chat.review", Name: "Review",
		Interpreter: bundles.InterpreterShebang,
		BundleDir:   bundleDir,
		Body:        string(body),
	}

	repo := t.TempDir()
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = repo
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-b", "main")
	run("config", "user.name", "test")
	run("config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(repo, "f.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", "f.txt")
	run("commit", "-m", "init")

	ws, err := bundles.RunPromptShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: repo, WorkDir: repo, BundleSupport: bundleDir,
		Surface: "workspace", BaseBranch: "main",
	})
	if err != nil {
		t.Fatalf("RunPromptShebang(review workspace) err=%v", err)
	}
	if !strings.Contains(ws.Prompt, "changes in this worktree") {
		t.Errorf("review workspace prompt missing worktree subject, got %q", ws.Prompt)
	}
	if ws.Display != "Please review the changes in this worktree and file any real issues" {
		t.Errorf("review workspace display = %q", ws.Display)
	}

	ghBin := t.TempDir()
	fake := `#!/bin/sh
if [ "$1" = "pr" ] && [ "$2" = "view" ]; then
  printf '%s\n' '{"title":"OAuth","url":"https://github.com/acme/r/pull/42","baseRefName":"main","headRefName":"feat"}'
  exit 0
fi
if [ "$1" = "pr" ] && [ "$2" = "diff" ]; then
  if [ "${3:-}" = "--stat" ]; then
    printf '%s\n' ' file.go | 2 +-'
    exit 0
  fi
  printf '%s\n' 'diff --git a/file.go b/file.go'
  printf '%s\n' '+fmt.Println()'
  exit 0
fi
exit 1
`
	if err := os.WriteFile(filepath.Join(ghBin, "gh"), []byte(fake), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", ghBin+string(os.PathListSeparator)+os.Getenv("PATH"))

	gh, err := bundles.RunPromptShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: repo, WorkDir: repo, BundleSupport: bundleDir,
		Surface: "github_pr", PRNumber: 42, PRURL: "https://github.com/acme/r/pull/42",
		PRTitle: "OAuth", PRHost: true, PRBase: "main", PRBranch: "feat",
	})
	if err != nil {
		t.Fatalf("RunPromptShebang(review github_pr) err=%v", err)
	}
	if gh.Display != "Please review GitHub PR #42" {
		t.Errorf("review github display = %q, want Please review GitHub PR #42", gh.Display)
	}
	for _, want := range []string{
		"GitHub pull request #42",
		"gh pr diff",
		"working copy",
		"+fmt.Println()",
		"local drafts",
	} {
		if !strings.Contains(gh.Prompt, want) {
			t.Errorf("review github prompt missing %q, got %q", want, gh.Prompt)
		}
	}
	if strings.Contains(gh.Prompt, "changes in this worktree") {
		t.Errorf("review github prompt still used worktree wording, got %q", gh.Prompt)
	}

	local, err := bundles.RunPromptShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
		Worktree: repo, WorkDir: repo, BundleSupport: bundleDir,
		Surface: "local_pr", PRNumber: 7, PRTitle: "WIP", BaseBranch: "main",
	})
	if err != nil {
		t.Fatalf("RunPromptShebang(review local_pr) err=%v", err)
	}
	if local.Display != "Please review Local PR #7" {
		t.Errorf("review local display = %q, want Please review Local PR #7", local.Display)
	}
	if !strings.Contains(local.Prompt, "Local PR #7") || !strings.Contains(local.Prompt, "not a GitHub") {
		t.Errorf("review local prompt = %q, want Local PR wording", local.Prompt)
	}
	if strings.Contains(ws.Prompt, "over-engineering only") {
		t.Errorf("/review should stay a bug hunt, got %q", ws.Prompt)
	}
}

func TestSimplifyReviewAndAuditPrompts(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller")
	}
	bundleDir := filepath.Join(filepath.Dir(thisFile), "../../bundles/Chat")
	repo := t.TempDir()
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = repo
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-b", "main")
	run("config", "user.name", "test")
	run("config", "user.email", "test@example.com")
	if err := os.WriteFile(filepath.Join(repo, "f.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", "f.txt")
	run("commit", "-m", "init")

	runKind := func(script, id string) bundles.PromptResult {
		t.Helper()
		body, err := os.ReadFile(filepath.Join(bundleDir, "src", script))
		if err != nil {
			t.Fatalf("read %s: %v", script, err)
		}
		prompt := bundles.PromptAction{
			ID: id, Name: id,
			Interpreter: bundles.InterpreterShebang,
			BundleDir:   bundleDir,
			Body:        string(body),
		}
		got, err := bundles.RunPromptShebang(shebangCtx(t), prompt, bundles.PromptRunContext{
			Worktree: repo, WorkDir: repo, BundleSupport: bundleDir,
			Surface: "workspace", BaseBranch: "main",
		})
		if err != nil {
			t.Fatalf("RunPromptShebang(%s) err=%v", id, err)
		}
		return got
	}

	rev := runKind("simplify-review.sh", "recur.chat.simplify-review")
	if rev.Display != "Simplify review — file over-engineering findings" {
		t.Errorf("simplify-review display = %q", rev.Display)
	}
	for _, want := range []string{
		"over-engineering only",
		"mcp__recur__report_finding",
		"The diff's best outcome is getting shorter",
		"Lean already. Ship.",
		"yagni:",
		"end_line: 38",
		"changes in this worktree",
	} {
		if !strings.Contains(rev.Prompt, want) {
			t.Errorf("simplify-review prompt missing %q, got %q", want, rev.Prompt)
		}
	}
	if strings.Contains(rev.Prompt, "every real bug, risk, or issue") {
		t.Errorf("simplify-review still asked for bugs, got %q", rev.Prompt)
	}
	if strings.Contains(rev.Prompt, "L<line>") || strings.Contains(rev.Prompt, "`L12-38:") {
		t.Errorf("simplify-review should not use a location-in-message format, got %q", rev.Prompt)
	}
	if strings.Contains(strings.ToLower(rev.Prompt), "ponytail") {
		t.Errorf("simplify-review must not mention ponytail, got %q", rev.Prompt)
	}

	audit := runKind("simplify-audit.sh", "recur.chat.simplify-audit")
	if audit.Display != "Simplify audit — file over-engineering findings" {
		t.Errorf("simplify-audit display = %q", audit.Display)
	}
	for _, want := range []string{
		"Audit this repository for over-engineering",
		"mcp__recur__report_finding",
		"Scan the whole tree instead of a diff",
		"Lean already. Ship.",
		"## Hunt",
	} {
		if !strings.Contains(audit.Prompt, want) {
			t.Errorf("simplify-audit prompt missing %q, got %q", want, audit.Prompt)
		}
	}
	if strings.Contains(strings.ToLower(audit.Prompt), "ponytail") {
		t.Errorf("simplify-audit must not mention ponytail, got %q", audit.Prompt)
	}
	if strings.Contains(audit.Prompt, "Git context is already gathered") || strings.Contains(audit.Prompt, "## git diff") {
		t.Errorf("simplify-audit must not embed git diffs, got %q", audit.Prompt)
	}

	ghBin := t.TempDir()
	fake := `#!/bin/sh
if [ "$1" = "pr" ] && [ "$2" = "view" ]; then
  printf '%s\n' '{"title":"OAuth","url":"https://github.com/acme/r/pull/42","baseRefName":"main","headRefName":"feat"}'
  exit 0
fi
if [ "$1" = "pr" ] && [ "$2" = "diff" ]; then
  if [ "${3:-}" = "--stat" ]; then
    printf '%s\n' ' file.go | 2 +-'
    exit 0
  fi
  printf '%s\n' 'diff --git a/file.go b/file.go'
  printf '%s\n' '+fmt.Println()'
  exit 0
fi
exit 1
`
	if err := os.WriteFile(filepath.Join(ghBin, "gh"), []byte(fake), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", ghBin+string(os.PathListSeparator)+os.Getenv("PATH"))

	body, err := os.ReadFile(filepath.Join(bundleDir, "src", "simplify-audit.sh"))
	if err != nil {
		t.Fatalf("read simplify-audit.sh: %v", err)
	}
	host, err := bundles.RunPromptShebang(shebangCtx(t), bundles.PromptAction{
		ID: "recur.chat.simplify-audit", Name: "audit",
		Interpreter: bundles.InterpreterShebang,
		BundleDir:   bundleDir,
		Body:        string(body),
	}, bundles.PromptRunContext{
		Worktree: repo, WorkDir: repo, BundleSupport: bundleDir,
		Surface: "github_pr", PRNumber: 42, PRURL: "https://github.com/acme/r/pull/42",
		PRTitle: "OAuth", PRHost: true, PRBase: "main", PRBranch: "feat",
	})
	if err != nil {
		t.Fatalf("RunPromptShebang(simplify-audit github_pr) err=%v", err)
	}
	if host.Display != "Simplify audit isn't available on a GitHub PR — use /simplify-review" {
		t.Errorf("simplify-audit github_pr display = %q", host.Display)
	}
	for _, want := range []string{
		"Refused",
		"/simplify-review",
		"working copy",
	} {
		if !strings.Contains(host.Prompt, want) {
			t.Errorf("simplify-audit github_pr host prompt missing %q, got %q", want, host.Prompt)
		}
	}
	if strings.Contains(host.Prompt, "Scan the whole tree") {
		t.Errorf("simplify-audit github_pr must not walk cwd as the repo, got %q", host.Prompt)
	}
	if strings.Contains(host.Prompt, "gh pr diff") {
		t.Errorf("simplify-audit github_pr must not fall back to a diff-only audit, got %q", host.Prompt)
	}
	if strings.Contains(host.Prompt, "mcp__recur__report_finding") {
		t.Errorf("simplify-audit github_pr refusal must not ask to file findings, got %q", host.Prompt)
	}
}

// shebangCtx outlives the production prompt budget: under full-suite load,
// spawning fresh temp scripts plus python3 can exceed a few seconds on macOS.
func shebangCtx(t *testing.T) context.Context {
	ctx, cancel := context.WithTimeout(t.Context(), time.Minute)
	t.Cleanup(cancel)
	return ctx
}
