package bundles_test

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"recur/internal/bundles"
)

func mustWriteFile(t *testing.T, path, body string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestParseManifestGrammarAndTheme(t *testing.T) {
	dir := t.TempDir()
	mustWriteFile(t, filepath.Join(dir, "syntaxes", "demo.tmLanguage.json"),
		`{"scopeName":"source.demo","patterns":[]}`)
	mustWriteFile(t, filepath.Join(dir, "themes", "demo.tmTheme.json"),
		`{"name":"Demo","tokenColors":[{"settings":{"foreground":"#fff","background":"#000"}}]}`)
	mustWriteFile(t, filepath.Join(dir, "bundle.toml"), `[bundle]
name = "DemoLang"
id = "demo-lang-0001"
version = "1.0.0"

[[grammar]]
id = "demo.grammar"
scopeName = "source.demo"
path = "syntaxes/demo.tmLanguage.json"
fileTypes = ["demo"]

[[theme]]
id = "demo"
name = "Demo"
path = "themes/demo.tmTheme.json"
uiTheme = "dark"
order = 42
preview = { bg = "#06071D", fg = "#E0E0E0", fn = "#B683CA", name = "#2BF1DC", string = "#8DFF8E" }
`)

	data, err := os.ReadFile(filepath.Join(dir, "bundle.toml"))
	if err != nil {
		t.Fatal(err)
	}
	parsed, err := bundles.ParseManifestFull(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if len(parsed.Grammars) != 1 || parsed.Grammars[0].ScopeName != "source.demo" {
		t.Fatalf("grammars: %+v", parsed.Grammars)
	}
	if parsed.Grammars[0].ID != "demo.grammar" {
		t.Fatalf("grammar id: %+v", parsed.Grammars[0])
	}
	if len(parsed.Themes) != 1 || parsed.Themes[0].Name != "Demo" {
		t.Fatalf("themes: %+v", parsed.Themes)
	}
	th := parsed.Themes[0]
	if th.ID != "demo" || th.Order != 42 {
		t.Fatalf("id/order: %+v", th)
	}
	if th.Preview.Bg != "#06071D" || th.Preview.Fg != "#E0E0E0" ||
		th.Preview.Fn != "#B683CA" || th.Preview.Name != "#2BF1DC" || th.Preview.String != "#8DFF8E" {
		t.Fatalf("preview swatches: %+v", th.Preview)
	}
	if len(parsed.Commands) != 0 {
		t.Fatalf("expected grammar-only bundle, got %d commands", len(parsed.Commands))
	}
}

func TestParseManifestTerminalTheme(t *testing.T) {
	dir := t.TempDir()
	mustWriteFile(t, filepath.Join(dir, "themes", "dracula.json"), `{
  "background": "#282a36",
  "foreground": "#f8f8f2",
  "cursor": "#f8f8f2",
  "selectionBackground": "rgba(68,71,90,0.6)",
  "red": "#ff5555",
  "green": "#50fa7b"
}`)
	mustWriteFile(t, filepath.Join(dir, "bundle.toml"), `[bundle]
name = "Term"
id = "term-pack-1"

[[terminalTheme]]
id = "dracula"
name = "Dracula"
path = "themes/dracula.json"
uiTheme = "dark"
order = 10
`)
	data, err := os.ReadFile(filepath.Join(dir, "bundle.toml"))
	if err != nil {
		t.Fatal(err)
	}
	parsed, err := bundles.ParseManifestFull(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if len(parsed.TerminalThemes) != 1 {
		t.Fatalf("terminalThemes: %+v", parsed.TerminalThemes)
	}
	th := parsed.TerminalThemes[0]
	if th.ID != "dracula" || th.Name != "Dracula" || th.Order != 10 {
		t.Fatalf("meta: %+v", th)
	}
	if th.Palette.Background != "#282a36" || th.Palette.Foreground != "#f8f8f2" || th.Palette.Red != "#ff5555" {
		t.Fatalf("palette: %+v", th.Palette)
	}
	if th.Palette.SelectionBackground != "rgba(68,71,90,0.6)" {
		t.Fatalf("selection: %q", th.Palette.SelectionBackground)
	}
}

func TestParseManifestTerminalThemeRequiresBgFg(t *testing.T) {
	dir := t.TempDir()
	mustWriteFile(t, filepath.Join(dir, "themes", "bad.json"), `{"red":"#f00"}`)
	mustWriteFile(t, filepath.Join(dir, "bundle.toml"), `[bundle]
name = "Term"
id = "term-pack-bad"

[[terminalTheme]]
id = "bad"
name = "Bad"
path = "themes/bad.json"
`)
	data, err := os.ReadFile(filepath.Join(dir, "bundle.toml"))
	if err != nil {
		t.Fatal(err)
	}
	if _, err := bundles.ParseManifestFull(dir, data); err == nil {
		t.Fatal("expected background/foreground error")
	}
}

func TestDiscoverCatalogUserOverridesTerminalThemeByID(t *testing.T) {
	def := t.TempDir()
	user := t.TempDir()
	mustWriteFile(t, filepath.Join(def, "Term", "themes", "x.json"), `{"background":"#000000","foreground":"#ffffff"}`)
	mustWriteFile(t, filepath.Join(def, "Term", "bundle.toml"), `[bundle]
name = "Term"
id = "term-default"

[[terminalTheme]]
id = "x"
name = "Default"
path = "themes/x.json"
`)
	mustWriteFile(t, filepath.Join(user, "Term", "themes", "x.json"), `{"background":"#111111","foreground":"#eeeeee"}`)
	mustWriteFile(t, filepath.Join(user, "Term", "bundle.toml"), `[bundle]
name = "Term"
id = "term-user"

[[terminalTheme]]
id = "x"
name = "User"
path = "themes/x.json"
`)
	cat, err := bundles.DiscoverCatalog(bundles.DiscoverRoots{UserDir: user, DefaultDir: def})
	if err != nil {
		t.Fatal(err)
	}
	th, ok := bundles.FindTerminalThemeByID(cat.TerminalThemes, "x")
	if !ok {
		t.Fatal("missing terminal theme")
	}
	if th.Name != "User" || th.SourceRoot != "user" || th.Palette.Background != "#111111" {
		t.Fatalf("user should win: %+v", th)
	}
}

func TestParseManifestRejectsBadPreviewColor(t *testing.T) {
	dir := t.TempDir()
	mustWriteFile(t, filepath.Join(dir, "themes", "demo.tmTheme.json"), `{"name":"Demo"}`)
	_, err := bundles.ParseManifestFull(dir, []byte(`[bundle]
name = "BadPreview"
id = "bad-preview-0001"

[[theme]]
id = "bad-preview-theme"
name = "Bad"
path = "themes/demo.tmTheme.json"
preview = { bg = "not-a-color" }
`))
	if err == nil {
		t.Fatal("expected invalid preview colour error")
	}
}

func TestParseManifestRequiresGrammarID(t *testing.T) {
	dir := t.TempDir()
	mustWriteFile(t, filepath.Join(dir, "syntaxes", "demo.tmLanguage.json"),
		`{"scopeName":"source.demo","patterns":[]}`)
	_, err := bundles.ParseManifestFull(dir, []byte(`[bundle]
name = "NoID"
id = "noid.bundle"

[[grammar]]
scopeName = "source.demo"
path = "syntaxes/demo.tmLanguage.json"
`))
	if err == nil || !strings.Contains(err.Error(), "id is required") {
		t.Fatalf("expected grammar id required, got %v", err)
	}
}

func TestParseManifestRejectsGrammarPathEscape(t *testing.T) {
	dir := t.TempDir()
	_, err := bundles.ParseManifestFull(dir, []byte(`[bundle]
name = "Bad"
id = "bad-gram-0001"

[[grammar]]
id = "bad.grammar"
scopeName = "source.bad"
path = "../outside.tmLanguage.json"
fileTypes = ["bad"]
`))
	if err == nil {
		t.Fatal("expected path escape error")
	}
}

func TestDiscoverCatalogUserOverridesGrammarByID(t *testing.T) {
	def := t.TempDir()
	user := t.TempDir()
	mustWriteFile(t, filepath.Join(def, "Lang", "syntaxes", "demo.tmLanguage.json"),
		`{"scopeName":"source.demo","patterns":[]}`)
	mustWriteFile(t, filepath.Join(def, "Lang", "bundle.toml"), `[bundle]
name = "Lang"
id = "lang-default-1"

[[grammar]]
id = "demo.grammar"
scopeName = "source.demo"
path = "syntaxes/demo.tmLanguage.json"
fileTypes = ["demo"]
language = "default"
`)
	mustWriteFile(t, filepath.Join(user, "Lang", "syntaxes", "demo.tmLanguage.json"),
		`{"scopeName":"source.demo","patterns":[]}`)
	mustWriteFile(t, filepath.Join(user, "Lang", "bundle.toml"), `[bundle]
name = "Lang"
id = "lang-user-1"

[[grammar]]
id = "demo.grammar"
scopeName = "source.demo"
path = "syntaxes/demo.tmLanguage.json"
fileTypes = ["demo"]
language = "user"
`)

	cat, err := bundles.DiscoverCatalog(bundles.DiscoverRoots{UserDir: user, DefaultDir: def})
	if err != nil {
		t.Fatal(err)
	}
	g, ok := bundles.FindGrammarByID(cat.Grammars, "demo.grammar")
	if !ok {
		t.Fatal("missing grammar by id")
	}
	if g.Language != "user" || g.SourceRoot != "user" {
		t.Fatalf("user should win by id: %+v", g)
	}
	byScope, ok := bundles.FindGrammarByScope(cat.Grammars, "source.demo")
	if !ok || byScope.Language != "user" {
		t.Fatalf("scope map should also be user: %+v", byScope)
	}
}

func TestGrammarThemeConventionHelpers(t *testing.T) {
	if !bundles.GrammarConventionOK("syntaxes/go.tmLanguage.json") {
		t.Fatal("expected go.tmLanguage.json ok")
	}
	if !bundles.ThemeConventionOK("themes/cool-glow.tmTheme.json") {
		t.Fatal("expected cool-glow.tmTheme.json ok")
	}
	if bundles.ThemeConventionOK("themes/cool-glow-color-theme.json") {
		t.Fatal("non-conventional theme name should fail helper")
	}
	if !bundles.TerminalThemeConventionOK("themes/dracula.json") {
		t.Fatal("expected dracula.json ok")
	}
	if bundles.TerminalThemeConventionOK("themes/nord.tmTheme.json") {
		t.Fatal("tmTheme.json should not count as a terminal theme")
	}
}
