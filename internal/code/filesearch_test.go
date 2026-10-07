package code

import (
	"context"
	"fmt"
	"path/filepath"
	"strings"
	"testing"
)

func TestRankFilesHypHyperlane(t *testing.T) {
	paths := []string{
		"validators/decode-hyperlane-message.sh",
		"validators/hyperlane_validator2.sh",
		"hypothesis.txt",
		"README.md",
	}
	bases := basenamesOf(paths)
	hits := RankFiles(context.Background(), paths, bases, "hyp", 50)
	seen := map[string]bool{}
	for _, h := range hits {
		seen[h.Path] = true
	}
	if !seen["validators/hyperlane_validator2.sh"] || !seen["validators/decode-hyperlane-message.sh"] {
		t.Fatalf("hits=%v missing hyperlane scripts", hits)
	}
}

func TestRankFilesBasenameBeatsDeepPath(t *testing.T) {
	paths := []string{
		"frontend/src/lib/deeply/nested/path/match-util.ts",
	}
	for i := 0; i < 15; i++ {
		paths = append(paths, fmt.Sprintf("a%02d/match.md", i))
	}
	bases := basenamesOf(paths)
	hits := RankFiles(context.Background(), paths, bases, "match", 50)
	if len(hits) != 16 {
		t.Fatalf("len=%d want 16", len(hits))
	}
	seen := false
	for _, h := range hits {
		if h.Path == "frontend/src/lib/deeply/nested/path/match-util.ts" {
			seen = true
		}
	}
	if !seen {
		t.Fatal("nested match-util.ts missing from top 50")
	}
}

func TestFilterForIndexExclude(t *testing.T) {
	paths := []string{"src/a.go", "dist/bundle.js", "src/b.go", "dir/.DS_Store"}
	got := filterForIndex(paths, compileExcludes([]string{"dist/**", "**/.DS_Store"}))
	want := map[string]bool{"src/a.go": true, "src/b.go": true}
	if len(got) != 2 {
		t.Fatalf("got %v", got)
	}
	for _, p := range got {
		if !want[p] {
			t.Fatalf("unexpected %q in %v", p, got)
		}
	}
}

func TestExcludeToRgGlobs(t *testing.T) {
	globs := excludeToRgGlobs([]string{"**/.git", "**/.DS_Store", "dist/**", "build", ""})
	joined := fmt.Sprintf("%v", globs)
	if !containsStr(globs, "!**/.DS_Store") || !containsStr(globs, "!**/.DS_Store/**") {
		t.Fatalf("want DS_Store + descendant: %v", globs)
	}
	if !containsStr(globs, "!dist/**") {
		t.Fatalf("globs=%v", globs)
	}
	if containsStr(globs, "!dist/**/**") {
		t.Fatalf("should not double /**: %s", joined)
	}
	if !containsStr(globs, "!build") || !containsStr(globs, "!build/**") {
		t.Fatalf("want build + build/**: %v", globs)
	}
	if containsStr(globs, "!**/.git") || containsStr(globs, "!.git") {
		t.Fatalf("default .git should be omitted from extras: %s", joined)
	}
}

func containsStr(ss []string, want string) bool {
	for _, s := range ss {
		if s == want {
			return true
		}
	}
	return false
}

func TestRankFilesCamelHump(t *testing.T) {
	paths := []string{"pkg/UserController.go", "pkg/other.go"}
	bases := basenamesOf(paths)
	hits := RankFiles(context.Background(), paths, bases, "uc", 10)
	if len(hits) == 0 || hits[0].Path != "pkg/UserController.go" {
		t.Fatalf("hits=%v want UserController first", hits)
	}
}

func TestFastExcludeMatcherDefaults(t *testing.T) {
	ex := compileExcludes([]string{"**/.git", "**/.DS_Store", "dist/**", "**/*.map"})
	cases := []struct {
		path string
		want bool
	}{
		{".git", true},
		{"foo/.git", true},
		{"foo/.git/config", true}, // dir prune — descendants of **/name
		{"src/a.go", false},
		{"dir/.DS_Store", true},
		{"dist/bundle.js", true},
		{"distribute/x", false},
		{"app.js.map", true},
		{"app.js", false},
	}
	for _, tc := range cases {
		if got := excluded(tc.path, ex); got != tc.want {
			t.Fatalf("excluded(%q)=%v want %v", tc.path, got, tc.want)
		}
	}
}

func TestWordBoundaryMatch(t *testing.T) {
	cases := []struct {
		text, q string
		want    bool
	}{
		{"foo/bar_baz.go", "bar", true},
		{"foo/bar_baz.go", "baz", true},
		{"foo/bar_baz.go", "foo", true},
		{"foo/bar_baz.go", "ar", false},
		{"", "x", false},
	}
	for _, tc := range cases {
		if got := wordBoundaryMatch(tc.text, tc.q); got != tc.want {
			t.Fatalf("wordBoundaryMatch(%q,%q)=%v want %v", tc.text, tc.q, got, tc.want)
		}
	}
}

func TestUniqueParentDirs(t *testing.T) {
	got := uniqueParentDirs([]string{
		"src/lib/a.ts",
		"src/lib/b.ts",
		"README.md",
		"frontend/src/main.go",
	})
	want := map[string]bool{"src": true, "src/lib": true, "frontend": true, "frontend/src": true}
	if len(got) != len(want) {
		t.Fatalf("got %v", got)
	}
	for _, p := range got {
		if !want[p] {
			t.Fatalf("unexpected %q in %v", p, got)
		}
	}
}

func TestRankFilesAndDirs(t *testing.T) {
	paths := []string{"src/lib/main.go", "src/app.ts", "README.md"}
	hits := RankFilesAndDirs(context.Background(), paths, basenamesOf(paths), "src", 50)
	var sawDir, sawFile bool
	for _, h := range hits {
		path := strings.TrimSuffix(h.Path, "/")
		if path == "src" && h.Dir {
			sawDir = true
		}
		if path == "src/lib" && h.Dir {
			sawDir = true
		}
		if h.Path == "src/app.ts" && !h.Dir {
			sawFile = true
		}
		if h.Dir && (path == "src/app.ts" || path == "README.md") {
			t.Fatalf("file marked dir: %+v", h)
		}
	}
	if !sawDir || !sawFile {
		t.Fatalf("hits=%v want src dir and src/app.ts", hits)
	}
}

func TestRankHitsFilesOnlyKeepsFilesWhenDirsDominate(t *testing.T) {
	paths := make([]string, 50)
	for i := 0; i < 50; i++ {
		paths[i] = fmt.Sprintf("pkg%02d/src/index.ts", i)
	}
	mixed := RankFilesAndDirs(context.Background(), paths, basenamesOf(paths), "src", 50)
	for _, h := range mixed {
		if !h.Dir {
			t.Fatalf("mixed limit filled by dirs, got file %s", h.Path)
		}
	}
	files := RankHits(context.Background(), paths, basenamesOf(paths), "src", 50, true)
	if len(files) != 50 {
		t.Fatalf("files-only: got %d", len(files))
	}
	for _, h := range files {
		if h.Dir {
			t.Fatalf("RankHits filesOnly marked dir: %+v", h)
		}
		if !strings.HasSuffix(h.Path, "/src/index.ts") {
			t.Fatalf("unexpected file %s", h.Path)
		}
	}
}

func basenamesOf(paths []string) []string {
	bases := make([]string, len(paths))
	for i, p := range paths {
		bases[i] = filepath.Base(p)
	}
	return bases
}

func BenchmarkRankFilesShortQuery(b *testing.B) {
	const n = 50_000
	paths := make([]string, n)
	bases := make([]string, n)
	for i := 0; i < n; i++ {
		paths[i] = fmt.Sprintf("pkg/mod%03d/file_%05d.go", i%200, i)
		bases[i] = filepath.Base(paths[i])
	}
	b.ReportAllocs()
	b.ResetTimer()
	var sink int
	for i := 0; i < b.N; i++ {
		hits := RankFiles(context.Background(), paths, bases, "fi", 50)
		sink += len(hits)
	}
	_ = sink
}
