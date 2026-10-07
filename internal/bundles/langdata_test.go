package bundles_test

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"recur/internal/bundles"
)

func TestParseManifestLanguageDataOnly(t *testing.T) {
	dir := t.TempDir()
	mustWriteFile(t, filepath.Join(dir, "bundle.toml"), `[bundle]
name = "LangData"
id = "demo-langdata-1"
version = "1.0.0"

[[languageData]]
id = "demo.langdata.yaml"
scopeName = "source.yaml"
lineComment = "#"
disableIndentCorrections = true
increaseIndentPattern = "^\\s*.*:$"
closeBrackets = ["<"]
`)

	data, err := os.ReadFile(filepath.Join(dir, "bundle.toml"))
	if err != nil {
		t.Fatal(err)
	}
	parsed, err := bundles.ParseManifestFull(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if len(parsed.Commands) != 0 || len(parsed.Grammars) != 0 {
		t.Fatalf("expected language-data-only bundle, got %+v", parsed)
	}
	if len(parsed.LanguageData) != 1 {
		t.Fatalf("languageData: %+v", parsed.LanguageData)
	}
	d := parsed.LanguageData[0]
	if d.ID != "demo.langdata.yaml" || d.ScopeName != "source.yaml" || d.LineComment != "#" {
		t.Fatalf("entry: %+v", d)
	}
	if !d.DisableIndentCorrections || d.IncreaseIndentPattern != `^\s*.*:$` {
		t.Fatalf("indent fields: %+v", d)
	}
	if len(d.CloseBrackets) != 1 || d.CloseBrackets[0] != "<" {
		t.Fatalf("closeBrackets: %+v", d.CloseBrackets)
	}
}

func TestParseManifestLanguageDataRejects(t *testing.T) {
	dir := t.TempDir()
	cases := []struct {
		name string
		toml string
		want string
	}{
		{"empty payload", `[bundle]
name = "T"
id = "b"
[[languageData]]
id = "x"
scopeName = "source.x"
`, "comment tokens, indent"},
		{"block unpaired", `[bundle]
name = "T"
id = "b"
[[languageData]]
id = "x"
scopeName = "source.x"
blockCommentStart = "<!--"
`, "both be set"},
		{"closeBrackets not a char", `[bundle]
name = "T"
id = "b"
[[languageData]]
id = "x"
scopeName = "source.x"
closeBrackets = ["<>"]
`, "single character"},
		{"dup scope", `[bundle]
name = "T"
id = "b"
[[languageData]]
id = "x"
scopeName = "source.x"
lineComment = "#"
[[languageData]]
id = "y"
scopeName = "source.x"
lineComment = "//"
`, "duplicate scopeName"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, err := bundles.ParseManifestFull(dir, []byte(tc.toml))
			if err == nil || !strings.Contains(err.Error(), tc.want) {
				t.Fatalf("want %q in error, got %v", tc.want, err)
			}
		})
	}
}

func TestDiscoverCatalogUserOverridesLanguageDataByID(t *testing.T) {
	def := t.TempDir()
	user := t.TempDir()
	mustWriteFile(t, filepath.Join(def, "LD", "bundle.toml"), `[bundle]
name = "LD"
id = "ld-default-1"

[[languageData]]
id = "demo.langdata"
scopeName = "source.demo"
lineComment = "#"
`)
	mustWriteFile(t, filepath.Join(user, "LD", "bundle.toml"), `[bundle]
name = "LD"
id = "ld-user-1"

[[languageData]]
id = "demo.langdata"
scopeName = "source.demo"
lineComment = "//"
`)

	cat, err := bundles.DiscoverCatalog(bundles.DiscoverRoots{UserDir: user, DefaultDir: def})
	if err != nil {
		t.Fatal(err)
	}
	d, ok := bundles.FindLanguageDataByID(cat.LanguageData, "demo.langdata")
	if !ok {
		t.Fatal("missing languageData by id")
	}
	if d.LineComment != "//" || d.SourceRoot != "user" {
		t.Fatalf("user should win by id: %+v", d)
	}
}

func TestResolveLanguageDataPrefix(t *testing.T) {
	all := []bundles.LanguageData{
		{ID: "src", ScopeName: "source", LineComment: "//"},
		{ID: "py", ScopeName: "source.python", LineComment: "#"},
	}
	d, ok := bundles.ResolveLanguageData(all, "source.go")
	if !ok || d.LineComment != "//" {
		t.Fatalf("source.go should inherit source: %+v ok=%v", d, ok)
	}
	d, ok = bundles.ResolveLanguageData(all, "source.python")
	if !ok || d.LineComment != "#" {
		t.Fatalf("source.python should be exact: %+v ok=%v", d, ok)
	}
	if _, ok := bundles.ResolveLanguageData(all, "text.html.basic"); ok {
		t.Fatal("unrelated scope should miss")
	}
}

func TestShippedLanguageDataComments(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("caller")
	}
	root := filepath.Join(filepath.Dir(thisFile), "..", "..", "bundles")
	cat, err := bundles.DiscoverCatalog(bundles.DiscoverRoots{DefaultDir: root})
	if err != nil {
		t.Fatal(err)
	}

	check := func(scope, line, blockStart, blockEnd string) {
		t.Helper()
		d, ok := bundles.ResolveLanguageData(cat.LanguageData, scope)
		if !ok {
			t.Fatalf("missing languageData for %s", scope)
		}
		if d.LineComment != line || d.BlockCommentStart != blockStart || d.BlockCommentEnd != blockEnd {
			t.Fatalf("%s comments: line=%q block=%q %q", scope, d.LineComment, d.BlockCommentStart, d.BlockCommentEnd)
		}
	}
	check("source.yaml", "#", "", "")
	check("source.dotenv", "#", "", "")
	check("text.html.basic", "", "<!--", "-->")
	html, ok := bundles.FindLanguageDataByScope(cat.LanguageData, "text.html.basic")
	if !ok || len(html.CloseBrackets) != 1 || html.CloseBrackets[0] != "<" {
		t.Fatalf("html closeBrackets: %+v", html.CloseBrackets)
	}
	md, ok := bundles.FindLanguageDataByScope(cat.LanguageData, "text.html.markdown")
	if !ok || strings.Join(md.CloseBrackets, "") != "<_*" {
		t.Fatalf("markdown closeBrackets: %+v", md.CloseBrackets)
	}
	check("source.go", "//", "/*", "*/")
	if d, ok := bundles.ResolveLanguageData(cat.LanguageData, "source.json"); ok {
		if d.LineComment != "" || d.BlockCommentStart != "" {
			t.Fatalf("strict JSON should omit comment tokens: %+v", d)
		}
	}
	py, ok := bundles.ResolveLanguageData(cat.LanguageData, "source.python")
	if !ok || py.IncreaseIndentPattern == "" || !py.DisableIndentCorrections {
		t.Fatalf("python indent: %+v", py)
	}
	mk, ok := bundles.FindLanguageDataByScope(cat.LanguageData, "source.makefile")
	if !ok || mk.IncreaseIndentPattern == "" || !mk.DisableIndentCorrections {
		t.Fatalf("makefile indent: %+v", mk)
	}
	for _, d := range cat.LanguageData {
		for _, p := range []string{
			d.IncreaseIndentPattern, d.DecreaseIndentPattern,
			d.IndentNextLinePattern, d.ZeroIndentPattern,
		} {
			if strings.Contains(p, "(?x)") || strings.Contains(p, "(?i)") {
				t.Fatalf("%s: indent pattern must be JS-compatible, not Oniguruma: %q", d.ID, p)
			}
		}
	}
}
