package engine

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func writeMentionFile(t *testing.T, root, rel string) {
	t.Helper()
	p := filepath.Join(root, filepath.FromSlash(rel))
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(p, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestCollectFileMentions(t *testing.T) {
	root := t.TempDir()
	writeMentionFile(t, root, "PROGRESS.md")
	writeMentionFile(t, root, "frontend/src/lib/agentchat.ts")
	in := "how does @PROGRESS.md compare to @frontend/src/lib/agentchat.ts ?"
	got := collectFileMentions(in, root)
	if len(got) != 2 {
		t.Fatalf("got %v", got)
	}
	if got[0].chip != "PROGRESS.md" || !strings.HasSuffix(got[0].abs, "PROGRESS.md") {
		t.Fatalf("first=%+v", got[0])
	}
	if got[1].chip != "frontend/src/lib/agentchat.ts" {
		t.Fatalf("second chip=%q", got[1].chip)
	}
	// Absolute body mention → preamble chip relativized.
	abs := filepath.Join(root, "DECISIONS.md")
	if err := os.WriteFile(abs, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	got2 := collectFileMentions("see @"+abs, root)
	if len(got2) != 1 || got2[0].chip != "DECISIONS.md" {
		t.Fatalf("abs mention: %+v", got2)
	}
}

func TestCollectFileMentionsRequiresBoundary(t *testing.T) {
	root := t.TempDir()
	writeMentionFile(t, root, "acme.io")
	writeMentionFile(t, root, "fixture.py")
	// Emails / decorators / scoped packages must not become file mentions.
	for _, in := range []string{
		"email john@acme.io please",
		"use @pytest.fixture here",
		"import @sveltejs/kit",
	} {
		if got := collectFileMentions(in, root); len(got) != 0 {
			t.Fatalf("%q → %+v", in, got)
		}
	}
	// Leading @ after whitespace is fine when the file exists.
	if got := collectFileMentions("see @acme.io", root); len(got) != 1 {
		t.Fatalf("boundary ok: %+v", got)
	}
	// Missing files are skipped (no attach preamble for ghosts).
	if got := collectFileMentions("see @nope.md", root); len(got) != 0 {
		t.Fatalf("missing file: %+v", got)
	}
}

func TestCollectFileMentionsDedupes(t *testing.T) {
	root := t.TempDir()
	readme := filepath.Join(root, "README.md")
	if err := os.WriteFile(readme, []byte("hi"), 0o644); err != nil {
		t.Fatal(err)
	}
	// Same chip repeated, plus abs alias of the same file.
	in := "see @README.md and again @README.md and also @" + readme
	got := collectFileMentions(in, root)
	if len(got) != 1 {
		t.Fatalf("want 1 unique file, got %+v", got)
	}
	if got[0].chip != "README.md" {
		t.Fatalf("chip=%q", got[0].chip)
	}
	wrapped := wrapAttachedFileMentions(in, root)
	if strings.Count(wrapped, " as @README.md") != 1 {
		t.Fatalf("preamble should list README once: %q", wrapped)
	}
	// Body still has every chip occurrence — only the attach list is de-duped.
	if strings.Count(wrapped, "@README.md") < 2 {
		t.Fatalf("body should keep repeated chips: %q", wrapped)
	}
}

func TestWrapAttachedFileMentions(t *testing.T) {
	root := t.TempDir()
	p := filepath.Join(root, "README.md")
	if err := os.WriteFile(p, []byte("hello world"), 0o644); err != nil {
		t.Fatal(err)
	}
	got := wrapAttachedFileMentions("what is @README.md ?", root)
	if !strings.Contains(got, "<system_instruction>") {
		t.Fatalf("missing instruction: %q", got)
	}
	if !strings.Contains(got, "The user has attached these files. Read them before proceeding.") {
		t.Fatalf("missing attach blurb: %q", got)
	}
	if !strings.Contains(got, " as @README.md (") {
		t.Fatalf("missing map line for README: %q", got)
	}
	if !strings.Contains(got, filepath.Base(p)) { // path appears in preamble
		t.Fatalf("missing file in preamble: %q", got)
	}
	// Body keeps the short chip — no abs path repeated there.
	if !strings.HasSuffix(strings.TrimSpace(got), "what is @README.md ?") {
		t.Fatalf("body should keep relative chip: %q", got)
	}
	// Abs path only in the preamble line (not also rewritten into the body).
	if strings.Count(got, "as @README.md") != 1 {
		t.Fatalf("expected one preamble mapping: %q", got)
	}
	// No mentions → unchanged.
	plain := "hello there"
	if wrapAttachedFileMentions(plain, root) != plain {
		t.Fatalf("plain text should be unchanged")
	}
}

func TestCollectFileMentionsLineSpan(t *testing.T) {
	root := t.TempDir()
	writeMentionFile(t, root, "frontend/src/lib/agentchat.ts")
	in := "look at @frontend/src/lib/agentchat.ts:22-48 please"
	got := collectFileMentions(in, root)
	if len(got) != 1 {
		t.Fatalf("got %+v", got)
	}
	if got[0].chip != "frontend/src/lib/agentchat.ts:22-48" {
		t.Fatalf("chip=%q", got[0].chip)
	}
	if got[0].startLine != 22 || got[0].endLine != 48 {
		t.Fatalf("lines=%d-%d", got[0].startLine, got[0].endLine)
	}
	if !strings.HasSuffix(got[0].abs, "agentchat.ts") {
		t.Fatalf("abs=%q", got[0].abs)
	}
	// Whole-file + line-span of the same path are distinct attach rows.
	writeMentionFile(t, root, "PROGRESS.md")
	both := collectFileMentions("see @PROGRESS.md and @PROGRESS.md:3", root)
	if len(both) != 2 {
		t.Fatalf("want 2 mentions, got %+v", both)
	}
	wrapped := wrapAttachedFileMentions("see @PROGRESS.md:3", root)
	if !strings.Contains(wrapped, " as @PROGRESS.md:3 (") {
		t.Fatalf("missing lined chip in preamble: %q", wrapped)
	}
	if !strings.Contains(wrapped, "focus on line 3") {
		t.Fatalf("missing line focus note: %q", wrapped)
	}
	// Absolute path with line suffix still resolves + relativizes.
	abs := filepath.Join(root, "frontend", "src", "lib", "agentchat.ts")
	gotAbs := collectFileMentions("see @"+abs+":10", root)
	if len(gotAbs) != 1 || gotAbs[0].chip != "frontend/src/lib/agentchat.ts:10" {
		t.Fatalf("abs+line: %+v", gotAbs)
	}
}

func TestSplitFileMentionChip(t *testing.T) {
	path, start, end, sc, ec := splitFileMentionChip("src/a.ts:12-40")
	if path != "src/a.ts" || start != 12 || end != 40 || sc != 0 || ec != 0 {
		t.Fatalf("got %q %d %d %d %d", path, start, end, sc, ec)
	}
	path, start, end, sc, ec = splitFileMentionChip("src/a.ts:12")
	if path != "src/a.ts" || start != 12 || end != 0 || sc != 0 || ec != 0 {
		t.Fatalf("single: %q %d %d %d %d", path, start, end, sc, ec)
	}
	path, start, end, sc, ec = splitFileMentionChip("src/a.ts")
	if path != "src/a.ts" || start != 0 || end != 0 || sc != 0 || ec != 0 {
		t.Fatalf("plain: %q %d %d %d %d", path, start, end, sc, ec)
	}
	// :0 / invalid line → whole-file path (not "src/a.ts:0" as the path).
	path, start, end, sc, ec = splitFileMentionChip("src/a.ts:0")
	if path != "src/a.ts" || start != 0 || end != 0 {
		t.Fatalf(":0 fallback: %q %d %d", path, start, end)
	}
	path, start, end, sc, ec = splitFileMentionChip("src/a.ts:0-12")
	if path != "src/a.ts" || start != 0 || end != 0 {
		t.Fatalf(":0-12 fallback: %q %d %d", path, start, end)
	}
	path, start, end, sc, ec = splitFileMentionChip("src/a.ts:12:5-12:18")
	if path != "src/a.ts" || start != 12 || end != 0 || sc != 5 || ec != 18 {
		t.Fatalf("cols: %q %d %d %d %d", path, start, end, sc, ec)
	}
	path, start, end, sc, ec = splitFileMentionChip("src/a.ts:12:5-13:20")
	if path != "src/a.ts" || start != 12 || end != 13 || sc != 5 || ec != 20 {
		t.Fatalf("cross: %q %d %d %d %d", path, start, end, sc, ec)
	}
	// :12:5-18 is line:col-line (no end col), not same-line columns.
	path, start, end, sc, ec = splitFileMentionChip("src/a.ts:12:5-18")
	if path != "src/a.ts" || start != 12 || end != 18 || sc != 0 || ec != 0 {
		t.Fatalf("col-to-line: %q %d %d %d %d", path, start, end, sc, ec)
	}
	path, start, end, sc, ec = splitFileMentionChip("src/a.ts:12:0-12:18")
	if path != "src/a.ts" || start != 12 || sc != 0 || ec != 0 {
		t.Fatalf("bad cols keep line: %q %d %d %d", path, start, sc, ec)
	}
}

func TestCollectFileMentionsInvalidLineFallsBackToWholeFile(t *testing.T) {
	root := t.TempDir()
	writeMentionFile(t, root, "src/a.ts")
	got := collectFileMentions("see @src/a.ts:0 please", root)
	if len(got) != 1 {
		t.Fatalf("want whole-file attach, got %+v", got)
	}
	if got[0].chip != "src/a.ts" || got[0].startLine != 0 {
		t.Fatalf("chip=%q start=%d", got[0].chip, got[0].startLine)
	}
}

func TestCollectFileMentionsChipVariantsAtStartAndEnd(t *testing.T) {
	root := t.TempDir()
	writeMentionFile(t, root, "src/a.ts")
	writeMentionFile(t, root, "PROGRESS.md")
	writeMentionFile(t, root, ".gitignore")

	type want struct {
		chip      string
		startLine int
		endLine   int
	}
	cases := []struct {
		name string
		in   string
		want want
	}{
		// Sole prompt — chip is the entire message
		{"plain sole", "@src/a.ts", want{"src/a.ts", 0, 0}},
		{"line sole", "@src/a.ts:12", want{"src/a.ts:12", 12, 0}},
		{"span sole", "@src/a.ts:12-40", want{"src/a.ts:12-40", 12, 40}},
		{"name.ext sole", "@PROGRESS.md", want{"PROGRESS.md", 0, 0}},
		{"dotfile sole", "@.gitignore", want{".gitignore", 0, 0}},
		{"invalid line sole → whole file", "@src/a.ts:0", want{"src/a.ts", 0, 0}},
		// Start of prompt — chip first, then more text
		{"plain start", "@src/a.ts please review", want{"src/a.ts", 0, 0}},
		{"line start", "@src/a.ts:12 please review", want{"src/a.ts:12", 12, 0}},
		{"span start", "@src/a.ts:12-40 please review", want{"src/a.ts:12-40", 12, 40}},
		{"name.ext start", "@PROGRESS.md please review", want{"PROGRESS.md", 0, 0}},
		{"dotfile start", "@.gitignore please review", want{".gitignore", 0, 0}},
		{"invalid line start → whole file", "@src/a.ts:0 please review", want{"src/a.ts", 0, 0}},
		// End of prompt — text first, chip last
		{"plain end", "please review @src/a.ts", want{"src/a.ts", 0, 0}},
		{"line end", "please review @src/a.ts:12", want{"src/a.ts:12", 12, 0}},
		{"span end", "please review @src/a.ts:12-40", want{"src/a.ts:12-40", 12, 40}},
		{"name.ext end", "compare @PROGRESS.md", want{"PROGRESS.md", 0, 0}},
		{"dotfile end", "ignore @.gitignore", want{".gitignore", 0, 0}},
		{"invalid line end → whole file", "see @src/a.ts:0", want{"src/a.ts", 0, 0}},
		// Mid-prompt (⌘L-style insert after words + space)
		{"plain mid", "explain this @src/a.ts please", want{"src/a.ts", 0, 0}},
		{"span mid", "explain this @src/a.ts:12-40 please", want{"src/a.ts:12-40", 12, 40}},
		{"cols sole", "@src/a.ts:12:5-12:18", want{"src/a.ts:12:5-12:18", 12, 0}},
		{"cols end", "please review @src/a.ts:12:5-12:18", want{"src/a.ts:12:5-12:18", 12, 0}},
		{"cross sole", "@src/a.ts:12:5-13:20", want{"src/a.ts:12:5-13:20", 12, 13}},
		{"cross mid", "explain this @src/a.ts:12:5-13:20 please", want{"src/a.ts:12:5-13:20", 12, 13}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := collectFileMentions(tc.in, root)
			if len(got) != 1 {
				t.Fatalf("collectFileMentions(%q) len=%d got %+v, want 1", tc.in, len(got), got)
			}
			if got[0].chip != tc.want.chip || got[0].startLine != tc.want.startLine || got[0].endLine != tc.want.endLine {
				t.Fatalf("collectFileMentions(%q) chip=%q lines=%d-%d, want chip=%q lines=%d-%d",
					tc.in, got[0].chip, got[0].startLine, got[0].endLine,
					tc.want.chip, tc.want.startLine, tc.want.endLine)
			}
		})
	}

	// Without a leading whitespace boundary the chip must not attach (the
	// insertFileMention bug: "explain this@src/a.ts" looks chipped in the UI
	// but never reaches the agent).
	if got := collectFileMentions("explain this@src/a.ts", root); len(got) != 0 {
		t.Fatalf("missing boundary should not attach: %+v", got)
	}
	if got := collectFileMentions("this@src/a.ts:12-40", root); len(got) != 0 {
		t.Fatalf("missing boundary + span should not attach: %+v", got)
	}
	if got := collectFileMentions("this@src/a.ts:12:5-12:18", root); len(got) != 0 {
		t.Fatalf("missing boundary + cols should not attach: %+v", got)
	}
}

func TestWrapAttachedFileMentionsColumnSpan(t *testing.T) {
	root := t.TempDir()
	p := filepath.Join(root, "src", "a.ts")
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		t.Fatal(err)
	}
	body := "alpha\nhello world\ngamma\n"
	if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	got := collectFileMentions("see @src/a.ts:2:7-2:11", root)
	if len(got) != 1 {
		t.Fatalf("got %+v", got)
	}
	if got[0].chip != "src/a.ts:2:7-2:11" || got[0].startLine != 2 || got[0].startCol != 7 || got[0].endCol != 11 {
		t.Fatalf("mention=%+v", got[0])
	}
	wrapped := wrapAttachedFileMentions("see @src/a.ts:2:7-2:11", root)
	if !strings.Contains(wrapped, " as @src/a.ts:2:7-2:11 (") {
		t.Fatalf("missing col chip in preamble: %q", wrapped)
	}
	if !strings.Contains(wrapped, "focus on line 2, columns 7–11") {
		t.Fatalf("missing column focus note: %q", wrapped)
	}
	if strings.Contains(wrapped, "“world”") {
		t.Fatalf("wrap must not quote disk text (stale vs dirty buffer): %q", wrapped)
	}
	cross := wrapAttachedFileMentions("see @src/a.ts:1:2-3:4", root)
	if !strings.Contains(cross, "focus on line 1 column 2 through line 3 column 4") {
		t.Fatalf("cross-line note should be a start→end span: %q", cross)
	}
	if strings.Contains(cross, "lines 1–3, columns") {
		t.Fatalf("cross-line note must not read as a column rectangle: %q", cross)
	}
}

func TestCollectDirMentions(t *testing.T) {
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, "src", "lib"), 0o755); err != nil {
		t.Fatal(err)
	}
	writeMentionFile(t, root, "src/main.go")

	bare := collectFileMentions("look at @src", root)
	if len(bare) != 1 || !bare[0].dir || bare[0].chip != "src" {
		t.Fatalf("bare dir: %+v", bare)
	}
	nested := collectFileMentions("look at @src/lib", root)
	if len(nested) != 1 || !nested[0].dir || nested[0].chip != "src/lib" {
		t.Fatalf("nested dir: %+v", nested)
	}
	if got := collectFileMentions("email josh@src please", root); len(got) != 0 {
		t.Fatalf("email-shaped @src: %+v", got)
	}
	if got := collectFileMentions("see @terminal", root); len(got) != 0 {
		t.Fatalf("reserved @terminal: %+v", got)
	}

	slash := collectFileMentions("look at @src/", root)
	if len(slash) != 1 || !slash[0].dir || slash[0].chip != "src" {
		t.Fatalf("trailing-slash dir: %+v", slash)
	}
	if err := os.MkdirAll(filepath.Join(root, "terminal"), 0o755); err != nil {
		t.Fatal(err)
	}
	termDir := collectFileMentions("look at @terminal/", root)
	if len(termDir) != 1 || !termDir[0].dir || termDir[0].chip != "terminal" {
		t.Fatalf("reserved-name dir: %+v", termDir)
	}
	if err := os.MkdirAll(filepath.Join(root, ".github"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(root, "foo.bar"), 0o755); err != nil {
		t.Fatal(err)
	}
	dot := collectFileMentions("look at @.github/", root)
	if len(dot) != 1 || !dot[0].dir || dot[0].chip != ".github" {
		t.Fatalf("dotted dir: %+v", dot)
	}
	ext := collectFileMentions("look at @foo.bar/", root)
	if len(ext) != 1 || !ext[0].dir || ext[0].chip != "foo.bar" {
		t.Fatalf("dotted-name dir: %+v", ext)
	}

	wrapped := wrapAttachedFileMentions("look at @src", root)
	if !strings.Contains(wrapped, "The user pointed at these directories. Explore them as needed; do not dump the whole tree.") {
		t.Fatalf("missing dir blurb: %q", wrapped)
	}
	if strings.Contains(wrapped, "The user has attached these files") {
		t.Fatalf("dir-only wrap should not use the file blurb: %q", wrapped)
	}
	if !strings.Contains(wrapped, " as @src") {
		t.Fatalf("missing dir chip: %q", wrapped)
	}
	if !strings.HasSuffix(strings.TrimSpace(wrapped), "look at @src") {
		t.Fatalf("body should keep @src: %q", wrapped)
	}

	both := wrapAttachedFileMentions("see @src and @src/main.go", root)
	if !strings.Contains(both, "The user has attached these files. Read them before proceeding.") {
		t.Fatalf("missing file blurb: %q", both)
	}
	if !strings.Contains(both, "The user pointed at these directories") {
		t.Fatalf("missing dir blurb in mixed wrap: %q", both)
	}
}
