package lsp

import (
	"testing"

	"recur/internal/bundles"
)

func TestLookupGo(t *testing.T) {
	s := Lookup("go")
	if s == nil || s.Key != "gopls" {
		t.Fatalf("want gopls, got %#v", s)
	}
	if Lookup("go.mod") == nil || Lookup("go.mod").Key != "gopls" {
		t.Fatal("want gopls for go.mod")
	}
	if Lookup("go.work") == nil || Lookup("go.sum") == nil {
		t.Fatal("want gopls for go.work/go.sum")
	}
	if Lookup("gomod") == nil || Lookup("gowork") == nil {
		t.Fatal("want gopls for gomod/gowork aliases")
	}
}

func TestLookupPythonBoth(t *testing.T) {
	t.Cleanup(func() {
		SetBundleServers(nil)
		SetDisabledServers(nil)
	})
	all := LookupAll("python", "")
	if len(all) < 2 {
		t.Fatalf("want basedpyright+ruff, got %+v", all)
	}
	if all[0].Key != "basedpyright" || all[1].Key != "ruff" {
		t.Fatalf("order=%v %v", all[0].Key, all[1].Key)
	}
	if Lookup("python") == nil || Lookup("python").Key != "basedpyright" {
		t.Fatal("primary python server should be basedpyright")
	}
	SetDisabledServers([]string{"basedpyright"})
	s := Lookup("python")
	if s == nil || s.Key != "ruff" {
		t.Fatalf("disabled basedpyright should fall through to ruff, got %#v", s)
	}
}

func TestLookupPATHOnly(t *testing.T) {
	if Lookup("c") == nil || Lookup("c").Key != "clangd" {
		t.Fatal("clangd")
	}
	if Lookup("java") == nil || Lookup("java").Key != "jdtls" {
		t.Fatal("jdtls")
	}
	if Lookup("csharp") == nil || Lookup("csharp").Key != "omnisharp" {
		t.Fatal("omnisharp")
	}
	if Lookup("haskell") == nil || Lookup("haskell").Key != "haskell-language-server" {
		t.Fatal("hls")
	}
	if Lookup("typescriptreact") == nil || Lookup("typescriptreact").Key != "typescript-language-server" {
		t.Fatal("tsx should use typescript-language-server")
	}
	if Lookup("javascriptreact") == nil || Lookup("javascriptreact").Key != "typescript-language-server" {
		t.Fatal("jsx should use typescript-language-server")
	}
}

func TestBundleServerOverridesBuiltin(t *testing.T) {
	t.Cleanup(func() {
		SetBundleServers(nil)
		SetDisabledServers(nil)
	})
	SetBundleServers([]bundles.LanguageServer{{
		ID:        "gopls",
		Languages: []string{"go"},
		Command:   "/tmp/custom-gopls",
		Managed:   "gopls",
	}})
	s := Lookup("go")
	if s == nil || s.Command != "/tmp/custom-gopls" || s.Source != "bundle" {
		t.Fatalf("want bundle override, got %#v", s)
	}
	SetDisabledServers([]string{"gopls"})
	if Lookup("go") != nil {
		t.Fatal("disabled gopls should not resolve")
	}
}

func TestBundleFileGlobFiltersLookup(t *testing.T) {
	t.Cleanup(func() {
		SetBundleServers(nil)
		SetDisabledServers(nil)
	})
	SetBundleServers([]bundles.LanguageServer{{
		ID:        "basedpyright",
		Languages: []string{"python"},
		Command:   "basedpyright-langserver",
		FileGlob:  []string{"src/*.py"},
	}})
	all := LookupAll("python", "src/app.py")
	if len(all) < 1 || all[0].Key != "basedpyright" {
		t.Fatalf("src/app.py should match src glob, got %+v", all)
	}
	all = LookupAll("python", "tests/test_app.py")
	for _, s := range all {
		if s.Key == "basedpyright" {
			t.Fatalf("tests/test_app.py should not match src glob, got %+v", all)
		}
	}
}

func TestLookupVuePairsTypeScript(t *testing.T) {
	t.Cleanup(func() {
		SetBundleServers(nil)
		SetDisabledServers(nil)
	})
	SetBundleServers([]bundles.LanguageServer{
		{
			ID:        "vue-language-server",
			Languages: []string{"vue"},
			Command:   "vue-language-server",
			FileGlob:  []string{"*.vue"},
		},
		{
			ID:        "typescript-language-server",
			Languages: []string{"typescript", "javascript", "vue"},
			Command:   "typescript-language-server",
			FileGlob:  []string{"*.{js,ts,vue}"},
		},
	})
	all := LookupAll("vue", "App.vue")
	if len(all) != 2 || all[0].Key != "vue-language-server" || all[1].Key != "typescript-language-server" {
		t.Fatalf("want vue then typescript for App.vue, got %+v", all)
	}
	for _, s := range LookupAll("vue", "util.ts") {
		if s.Key == "vue-language-server" {
			t.Fatalf("vue LS should not attach to util.ts, got %+v", LookupAll("vue", "util.ts"))
		}
	}
}

func TestBundleEnvCopiedToSpec(t *testing.T) {
	t.Cleanup(func() {
		SetBundleServers(nil)
		SetDisabledServers(nil)
	})
	SetBundleServers([]bundles.LanguageServer{{
		ID:        "gopls",
		Languages: []string{"go"},
		Command:   "gopls",
		Env:       []string{"GOPACKAGESDRIVER=off"},
	}})
	s := Lookup("go")
	if s == nil || len(s.Env) != 1 || s.Env[0] != "GOPACKAGESDRIVER=off" {
		t.Fatalf("env not copied: %#v", s)
	}
}

func TestLookupAllFileGlobWithoutLanguage(t *testing.T) {
	t.Cleanup(func() {
		SetBundleServers(nil)
		SetDisabledServers(nil)
	})
	SetBundleServers([]bundles.LanguageServer{{
		ID:        "solidity-language-server",
		Languages: []string{"solidity"},
		Command:   "solidity-language-server",
		FileGlob:  []string{"*.sol"},
	}})
	all := LookupAll("", "contracts/Token.sol")
	if len(all) != 1 || all[0].Key != "solidity-language-server" {
		t.Fatalf("fileGlob should claim .sol with no language id, got %+v", all)
	}
	if len(LookupAll("", "README.md")) != 0 {
		t.Fatal("unrelated path must not match")
	}
	if len(LookupAll("", "main.go")) != 0 {
		t.Fatal("builtin servers without FileGlob must not match an empty language id")
	}
}
