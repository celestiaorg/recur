package bundles_test

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"recur/internal/bundles"
)

func TestParseManifestSymbolOnly(t *testing.T) {
	dir := t.TempDir()
	mustWriteFile(t, filepath.Join(dir, "bundle.toml"), `[bundle]
name = "Syms"
id = "demo-syms-1"

[[symbol]]
id = "demo.func"
scope = "entity.name.function"
kind = "func"
`)

	data, err := os.ReadFile(filepath.Join(dir, "bundle.toml"))
	if err != nil {
		t.Fatal(err)
	}
	parsed, err := bundles.ParseManifestFull(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if len(parsed.Commands) != 0 || len(parsed.Symbols) != 1 {
		t.Fatalf("expected symbol-only bundle, got %+v", parsed)
	}
	s := parsed.Symbols[0]
	if s.ID != "demo.func" || s.Scope != "entity.name.function" || !s.Show || s.Kind != "func" {
		t.Fatalf("entry: %+v", s)
	}
}

func TestParseManifestSymbolShowFalse(t *testing.T) {
	dir := t.TempDir()
	mustWriteFile(t, filepath.Join(dir, "bundle.toml"), `[bundle]
name = "Syms"
id = "demo-syms-hide"

[[symbol]]
id = "demo.hide"
scope = "entity.name.function.support"
show = false
`)
	data, err := os.ReadFile(filepath.Join(dir, "bundle.toml"))
	if err != nil {
		t.Fatal(err)
	}
	parsed, err := bundles.ParseManifestFull(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if len(parsed.Symbols) != 1 || parsed.Symbols[0].Show {
		t.Fatalf("show=false: %+v", parsed.Symbols)
	}
}

func TestParseManifestSymbolRejects(t *testing.T) {
	dir := t.TempDir()
	cases := []struct {
		name string
		toml string
		want string
	}{
		{"missing scope", `[bundle]
name = "T"
id = "b"
[[symbol]]
id = "x"
`, "scope is required"},
		{"dup id", `[bundle]
name = "T"
id = "b"
[[symbol]]
id = "x"
scope = "entity.name.function"
[[symbol]]
id = "x"
scope = "entity.name.type"
`, "duplicate id"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, err := bundles.ParseManifestFull(dir, []byte(tc.toml))
			if err == nil || !strings.Contains(err.Error(), tc.want) {
				t.Fatalf("ParseManifestFull() error = %v, want %q", err, tc.want)
			}
		})
	}
}

func TestDiscoverCatalogUserOverridesSymbolByID(t *testing.T) {
	def := t.TempDir()
	user := t.TempDir()
	mustWriteFile(t, filepath.Join(def, "S", "bundle.toml"), `[bundle]
name = "S"
id = "s-default"

[[symbol]]
id = "demo.func"
scope = "entity.name.function"
kind = "func"
`)
	mustWriteFile(t, filepath.Join(user, "S", "bundle.toml"), `[bundle]
name = "S"
id = "s-user"

[[symbol]]
id = "demo.func"
scope = "entity.name.function.go"
kind = "func"
`)

	cat, err := bundles.DiscoverCatalog(bundles.DiscoverRoots{UserDir: user, DefaultDir: def})
	if err != nil {
		t.Fatal(err)
	}
	s, ok := bundles.FindSymbolByID(cat.Symbols, "demo.func")
	if !ok {
		t.Fatal("missing symbol by id")
	}
	if s.Scope != "entity.name.function.go" || s.SourceRoot != "user" {
		t.Fatalf("user should win by id: %+v", s)
	}
}

func TestShippedSymbolCatalog(t *testing.T) {
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
		"recur.symbols.source.function",
		"recur.symbols.source.hide-calls",
		"recur.symbols.python.hide-decorator",
		"recur.symbols.markdown.heading",
		"recur.symbols.html.id",
		"recur.symbols.yaml.key",
		"recur.symbols.css.selector",
	}
	for _, id := range need {
		if _, ok := bundles.FindSymbolByID(cat.Symbols, id); !ok {
			t.Errorf("FindSymbolByID(%q) missing", id)
		}
	}
	fn, ok := bundles.FindSymbolByID(cat.Symbols, "recur.symbols.source.function")
	if !ok || !fn.Show {
		t.Fatalf("function include: %+v", fn)
	}
	hide, ok := bundles.FindSymbolByID(cat.Symbols, "recur.symbols.source.hide-calls")
	if !ok || hide.Show {
		t.Fatalf("hide-calls exclude: %+v", hide)
	}
	md, ok := bundles.FindSymbolByID(cat.Symbols, "recur.symbols.markdown.heading")
	if !ok || md.Scope != "markup.heading.markdown" || md.Transform == "" {
		t.Fatalf("markdown heading: %+v", md)
	}
}
