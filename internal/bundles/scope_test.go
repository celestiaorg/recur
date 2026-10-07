package bundles_test

import (
	"testing"

	"recur/internal/bundles"
)

func TestMatchesUnscoped(t *testing.T) {
	cmd := bundles.Command{Name: "Any"}
	if !bundles.Matches(cmd, "", "") {
		t.Fatal("unscoped should match empty")
	}
	if !bundles.Matches(cmd, "source.go", "main.go") {
		t.Fatal("unscoped should match everything")
	}
}

func TestMatchesGrammarPrefix(t *testing.T) {
	cmd := bundles.Command{Scope: "source.go"}
	if !bundles.Matches(cmd, "source.go", "a.go") {
		t.Fatal("exact scope")
	}
	if !bundles.Matches(cmd, "source.go.string", "a.go") {
		t.Fatal("prefix scope")
	}
	if bundles.Matches(cmd, "source.python", "a.py") {
		t.Fatal("other language")
	}
	if bundles.Matches(cmd, "", "a.go") {
		t.Fatal("empty have-scope with required want")
	}
}

func TestMatchesFileGlob(t *testing.T) {
	cmd := bundles.Command{FileGlob: "*.go"}
	if !bundles.Matches(cmd, "", "pkg/main.go") {
		t.Fatal("basename glob")
	}
	if bundles.Matches(cmd, "", "readme.md") {
		t.Fatal("non-go")
	}
	cmd2 := bundles.Command{FileGlob: "**/internal/*.go"}
	if !bundles.Matches(cmd2, "", "foo/internal/x.go") {
		t.Fatal("**/ glob")
	}
	cmd3 := bundles.Command{FileGlob: "*.{js,ts,tsx}"}
	if !bundles.Matches(cmd3, "", "src/app.tsx") {
		t.Fatal("brace glob should match tsx")
	}
	if bundles.Matches(cmd3, "", "src/app.go") {
		t.Fatal("brace glob should miss go")
	}
}

func TestMatchFileGlobAnyPattern(t *testing.T) {
	if !bundles.MatchFileGlob(nil, "anything.go") {
		t.Fatal("empty patterns match all")
	}
	if !bundles.MatchFileGlob([]string{"*.go", "*.mod"}, "go.mod") {
		t.Fatal("second pattern")
	}
	if bundles.MatchFileGlob([]string{"*.go"}, "readme.md") {
		t.Fatal("should miss")
	}
}

func TestMatchesScopeAndGlobTogether(t *testing.T) {
	cmd := bundles.Command{Scope: "source.go", FileGlob: "*.go"}
	if !bundles.Matches(cmd, "source.go", "main.go") {
		t.Fatal("both match")
	}
	if bundles.Matches(cmd, "source.go", "main.py") {
		t.Fatal("glob fail")
	}
	if bundles.Matches(cmd, "source.python", "main.go") {
		t.Fatal("scope fail")
	}
}

func TestMatchesCommaScopeOR(t *testing.T) {
	markup := bundles.Command{
		Scope:    "text.html, source.svelte, source.vue, source.tsx, source.js.jsx, source.astro",
		FileGlob: "*.{html,htm,svelte,vue,tsx,jsx,astro,mdx,hbs,handlebars}",
	}
	tests := []struct {
		name  string
		scope string
		path  string
		want  bool
	}{
		{name: "svelte", scope: "source.svelte", path: "App.svelte", want: true},
		{name: "html", scope: "text.html.basic", path: "index.html", want: true},
		{name: "vue", scope: "text.html.vue", path: "App.vue", want: true},
		{name: "tsx", scope: "source.tsx", path: "App.tsx", want: true},
		{name: "jsx", scope: "source.js.jsx", path: "App.jsx", want: true},
		{name: "astro", scope: "source.astro", path: "index.astro", want: true},
		{name: "mdx", scope: "text.html.markdown", path: "page.mdx", want: true},
		{name: "handlebars", scope: "text.html.handlebars", path: "page.hbs", want: true},
		{name: "go", scope: "source.go", path: "main.go", want: false},
		{name: "plain ts", scope: "source.ts", path: "app.ts", want: false},
		{name: "plain js", scope: "source.js", path: "app.js", want: false},
		{name: "markdown", scope: "text.html.markdown", path: "readme.md", want: false},
		{name: "tsx glob miss", scope: "source.tsx", path: "App.go", want: false},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := bundles.Matches(markup, tt.scope, tt.path)
			if got != tt.want {
				t.Errorf("Matches(%q, %q) = %v, want %v", tt.scope, tt.path, got, tt.want)
			}
		})
	}
}
