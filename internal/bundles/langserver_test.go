package bundles_test

import (
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"recur/internal/bundles"
)

func TestParseLanguageServer(t *testing.T) {
	dir := t.TempDir()
	data := []byte(`
[bundle]
name = "Go"
id = "go-bundle"
[[languageServer]]
id = "gopls"
languages = ["go", "GoMod"]
command = "gopls"
args = ["serve"]
managed = "gopls"
`)
	parsed, err := bundles.ParseManifestFull(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if parsed.Meta.Name != "Go" || len(parsed.Commands) != 0 || len(parsed.Prompts) != 0 {
		t.Fatalf("meta/cmds/prompts unexpected: %+v %d %d", parsed.Meta, len(parsed.Commands), len(parsed.Prompts))
	}
	if len(parsed.LanguageServers) != 1 {
		t.Fatalf("servers=%d", len(parsed.LanguageServers))
	}
	s := parsed.LanguageServers[0]
	if s.ID != "gopls" || s.Managed != "gopls" || s.Command != "gopls" {
		t.Fatalf("server=%+v", s)
	}
	if len(s.Languages) != 2 || s.Languages[0] != "go" || s.Languages[1] != "gomod" {
		t.Fatalf("languages=%v", s.Languages)
	}
	if len(s.Args) != 1 || s.Args[0] != "serve" {
		t.Fatalf("args=%v", s.Args)
	}
}

func TestDiscoverLanguageServerUserWins(t *testing.T) {
	def := t.TempDir()
	user := t.TempDir()
	writeLS := func(root, name, body string) {
		t.Helper()
		dir := filepath.Join(root, name)
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, "bundle.toml"), []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	writeLS(def, "Go", `
[bundle]
name = "Go"
id = "go-default"
[[languageServer]]
id = "gopls"
languages = ["go"]
command = "gopls-default"
`)
	writeLS(user, "Go", `
[bundle]
name = "Go User"
id = "go-user"
[[languageServer]]
id = "gopls"
languages = ["go"]
command = "gopls-user"
managed = "gopls"
`)
	cat, err := bundles.DiscoverCatalog(bundles.DiscoverRoots{UserDir: user, DefaultDir: def})
	if err != nil {
		t.Fatal(err)
	}
	if len(cat.LanguageServers) != 1 {
		t.Fatalf("servers=%+v", cat.LanguageServers)
	}
	s := cat.LanguageServers[0]
	if s.Command != "gopls-user" || s.Managed != "gopls" || s.SourceRoot != "user" {
		t.Fatalf("want user override, got %+v", s)
	}
}

func TestDiscoverShippedLanguageServers(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("caller")
	}
	root := filepath.Join(filepath.Dir(thisFile), "..", "..", "bundles")
	cat, err := bundles.DiscoverCatalog(bundles.DiscoverRoots{DefaultDir: root})
	if err != nil {
		t.Fatal(err)
	}
	want := []string{
		"basedpyright", "bash-language-server", "gopls", "marksman", "ruff",
		"rust-analyzer", "solidity-language-server", "svelte-language-server", "taplo",
		"typescript-language-server", "vscode-css-language-server",
		"vscode-html-language-server", "vscode-json-language-server",
		"vue-language-server", "yaml-language-server",
	}
	pathOnly := map[string]bool{"solidity-language-server": true}
	got := map[string]bool{}
	managed := map[string]bool{}
	for _, s := range cat.LanguageServers {
		got[s.ID] = true
		managed[s.ID] = s.Managed != ""
	}
	for _, id := range want {
		if !got[id] {
			t.Errorf("missing shipped language server %s (got %v)", id, got)
		} else if !pathOnly[id] && !managed[id] {
			t.Errorf("shipped language server %s is missing managed pin", id)
		}
	}
}
