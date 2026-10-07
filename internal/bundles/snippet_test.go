package bundles_test

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"recur/internal/bundles"
)

func TestParseManifestSnippetOnly(t *testing.T) {
	dir := t.TempDir()
	mustWriteFile(t, filepath.Join(dir, "bundle.toml"), `[bundle]
name = "Snips"
id = "demo-snips-1"

[[snippet]]
id = "demo.ife"
name = "If"
tabTrigger = "ife"
scope = "source.go"
body = '''
if ${1:condition} {
	$0
}
'''
`)
	data, err := os.ReadFile(filepath.Join(dir, "bundle.toml"))
	if err != nil {
		t.Fatal(err)
	}
	parsed, err := bundles.ParseManifestFull(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if len(parsed.Snippets) != 1 || parsed.Snippets[0].TabTrigger != "ife" {
		t.Fatalf("snippets: %+v", parsed.Snippets)
	}
	if !strings.Contains(parsed.Snippets[0].Body, "${1:condition}") {
		t.Fatalf("body: %q", parsed.Snippets[0].Body)
	}
}

func TestParseManifestSnippetRejects(t *testing.T) {
	dir := t.TempDir()
	_, err := bundles.ParseManifestFull(dir, []byte(`[bundle]
name = "T"
id = "b"
[[snippet]]
id = "x"
name = "X"
body = "hi"
`))
	if err == nil || !strings.Contains(err.Error(), "tabTrigger") {
		t.Fatalf("want tabTrigger/keyEquivalent error, got %v", err)
	}
}

func TestDiscoverCatalogUserOverridesSnippetByID(t *testing.T) {
	def := t.TempDir()
	user := t.TempDir()
	mustWriteFile(t, filepath.Join(def, "S", "bundle.toml"), `[bundle]
name = "S"
id = "s-default"

[[snippet]]
id = "demo.ife"
name = "If"
tabTrigger = "ife"
scope = "source.go"
body = "default"
`)
	mustWriteFile(t, filepath.Join(user, "S", "bundle.toml"), `[bundle]
name = "S"
id = "s-user"

[[snippet]]
id = "demo.ife"
name = "If"
tabTrigger = "ife"
scope = "source.go"
body = "user"
`)
	cat, err := bundles.DiscoverCatalog(bundles.DiscoverRoots{UserDir: user, DefaultDir: def})
	if err != nil {
		t.Fatal(err)
	}
	s, ok := bundles.FindSnippetByID(cat.Snippets, "demo.ife")
	if !ok || s.Body != "user" || s.SourceRoot != "user" {
		t.Fatalf("user should win: %+v ok=%v", s, ok)
	}
}

func TestSnippetMatchesScope(t *testing.T) {
	s := bundles.Snippet{Scope: "source.go", TabTrigger: "ife"}
	if !bundles.SnippetMatches(s, "source.go", "main.go") {
		t.Fatal("expected match")
	}
	if bundles.SnippetMatches(s, "source.python", "main.py") {
		t.Fatal("python should not match go snippet")
	}
}

func TestShippedSnippetCatalog(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("caller")
	}
	root := filepath.Join(filepath.Dir(thisFile), "..", "..", "bundles")
	cat, err := bundles.DiscoverCatalog(bundles.DiscoverRoots{DefaultDir: root})
	if err != nil {
		t.Fatal(err)
	}
	need := []string{
		"recur.go.ife", "recur.javascript.log", "recur.python.def",
		"recur.shell.bash", "recur.html.wrap", "recur.source.quote",
	}
	for _, id := range need {
		if _, ok := bundles.FindSnippetByID(cat.Snippets, id); !ok {
			t.Errorf("missing shipped snippet %s", id)
		}
	}
}
