package bundles_test

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"recur/internal/bundles"
)

func TestDiscoverShippedBundles(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("caller")
	}
	root := filepath.Join(filepath.Dir(thisFile), "..", "..", "bundles")
	cmds, _, err := bundles.Discover(bundles.DiscoverRoots{DefaultDir: root})
	if err != nil {
		t.Fatal(err)
	}
	if len(cmds) < 30 {
		t.Fatalf("expected many shipped commands, got %d", len(cmds))
	}
	// Compile every typescript command once.
	cache := t.TempDir()
	n := 0
	for _, c := range cmds {
		if !bundles.IsScriptInterpreter(c.Interpreter) {
			continue
		}
		if _, err := bundles.CompileMacro(cache, c); err != nil {
			t.Fatalf("compile %s (%s): %v", c.Name, c.ID, err)
		}
		n++
	}
	if n < 20 {
		t.Fatalf("expected many typescript macros, compiled %d", n)
	}
	t.Logf("shipped commands=%d typescript=%d", len(cmds), n)
}

func TestShippedSVGGrammar(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("caller")
	}
	root := filepath.Join(filepath.Dir(thisFile), "..", "..", "bundles")
	cat, err := bundles.DiscoverCatalog(bundles.DiscoverRoots{DefaultDir: root})
	if err != nil {
		t.Fatal(err)
	}
	g, ok := bundles.FindGrammarByScope(cat.Grammars, "text.xml.svg")
	if !ok {
		t.Fatal("missing text.xml.svg grammar")
	}
	if g.ID != "recur.languages.svg" || g.Language != "svg" {
		t.Fatalf("svg grammar: %+v", g)
	}
	found := false
	for _, ft := range g.FileTypes {
		if ft == "svg" {
			found = true
			break
		}
	}
	if !found {
		t.Fatalf("fileTypes: %v", g.FileTypes)
	}
	body, err := os.ReadFile(g.AbsPath)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(body), `"include": "text.xml"`) {
		t.Fatalf("svg grammar should include text.xml, got:\n%s", body)
	}
	if len(cat.TerminalThemes) < 8 {
		t.Fatalf("expected shipped terminal themes, got %d", len(cat.TerminalThemes))
	}
	if _, ok := bundles.FindTerminalThemeByID(cat.TerminalThemes, "homebrew"); !ok {
		t.Fatal("missing shipped homebrew terminal theme")
	}
}
