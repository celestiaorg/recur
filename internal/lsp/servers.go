package lsp

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"

	"recur/internal/bundles"
)

// ServerSpec describes how to launch a language server for a language id.
type ServerSpec struct {
	// Key is a stable session key (usually the primary language id / server id).
	Key string
	// Languages lists EditorLangId / LSP languageId values this server handles.
	Languages []string
	// Command is looked up on PATH (or absolute).
	Command string
	// Args are passed after Command.
	Args []string
	// Managed is a pin-registry id (e.g. "gopls"); empty means PATH-only.
	Managed string
	// FileGlob limits this server to matching repo-relative paths. Empty means
	// every file of Languages.
	FileGlob []string
	// Env is KEY=VAL extras appended to the child environment.
	Env []string
	// Source is "bundle" or "builtin".
	Source string
	// BundleName is set when Source is bundle.
	BundleName string
}

// builtinServers is the v1 PATH registry. Soft-degrade when the binary is missing.
// Bundle [[languageServer]] entries override builtins with the same Key / language.
// Multiple specs may share a language (e.g. basedpyright + ruff for python).
var builtinServers = []ServerSpec{
	{
		Key:       "gopls",
		Languages: []string{"go", "go.mod", "go.work", "go.sum", "gomod", "gowork"},
		Command:   "gopls",
		Args:      []string{"serve"},
		Source:    "builtin",
	},
	{
		Key:       "rust-analyzer",
		Languages: []string{"rust"},
		Command:   "rust-analyzer",
		Source:    "builtin",
	},
	{
		Key:       "typescript-language-server",
		Languages: []string{"typescript", "javascript", "typescriptreact", "javascriptreact", "tsx", "jsx", "vue"},
		Command:   "typescript-language-server",
		Args:      []string{"--stdio"},
		Source:    "builtin",
	},
	{
		Key:       "basedpyright",
		Languages: []string{"python"},
		Command:   "basedpyright-langserver",
		Args:      []string{"--stdio"},
		Source:    "builtin",
	},
	{
		Key:       "ruff",
		Languages: []string{"python"},
		Command:   "ruff",
		Args:      []string{"server"},
		Source:    "builtin",
	},
	{
		Key:       "clangd",
		Languages: []string{"c", "cpp"},
		Command:   "clangd",
		Source:    "builtin",
	},
	{
		Key:       "jdtls",
		Languages: []string{"java"},
		Command:   "jdtls",
		Source:    "builtin",
	},
	{
		Key:       "omnisharp",
		Languages: []string{"csharp"},
		Command:   "omnisharp",
		Args:      []string{"-lsp"},
		Source:    "builtin",
	},
	{
		Key:       "haskell-language-server",
		Languages: []string{"haskell"},
		Command:   "haskell-language-server-wrapper",
		Args:      []string{"--lsp"},
		Source:    "builtin",
	},
	{
		Key:       "vscode-json-language-server",
		Languages: []string{"json", "jsonc"},
		Command:   "vscode-json-language-server",
		Args:      []string{"--stdio"},
		Source:    "builtin",
	},
	{
		Key:       "vscode-html-language-server",
		Languages: []string{"html"},
		Command:   "vscode-html-language-server",
		Args:      []string{"--stdio"},
		Source:    "builtin",
	},
	{
		Key:       "vscode-css-language-server",
		Languages: []string{"css", "scss", "less"},
		Command:   "vscode-css-language-server",
		Args:      []string{"--stdio"},
		Source:    "builtin",
	},
	{
		Key:       "yaml-language-server",
		Languages: []string{"yaml"},
		Command:   "yaml-language-server",
		Args:      []string{"--stdio"},
		Source:    "builtin",
	},
	{
		Key:       "taplo",
		Languages: []string{"toml"},
		Command:   "taplo",
		Args:      []string{"lsp", "stdio"},
		Source:    "builtin",
	},
	{
		Key:       "bash-language-server",
		Languages: []string{"shellscript", "shell", "bash"},
		Command:   "bash-language-server",
		Args:      []string{"start"},
		Source:    "builtin",
	},
	{
		Key:       "svelte-language-server",
		Languages: []string{"svelte"},
		Command:   "svelteserver",
		Args:      []string{"--stdio"},
		Source:    "builtin",
	},
	{
		Key:       "marksman",
		Languages: []string{"markdown"},
		Command:   "marksman",
		Args:      []string{"server"},
		Source:    "builtin",
	},
}

var (
	regMu       sync.RWMutex
	bundleSpecs []ServerSpec
	disabledIDs map[string]bool
)

// SetBundleServers replaces the Bundle-contributed language server list.
func SetBundleServers(servers []bundles.LanguageServer) {
	specs := make([]ServerSpec, 0, len(servers))
	for _, s := range servers {
		specs = append(specs, ServerSpec{
			Key:        s.ID,
			Languages:  append([]string(nil), s.Languages...),
			Command:    s.Command,
			Args:       append([]string(nil), s.Args...),
			Managed:    s.Managed,
			FileGlob:   append([]string(nil), s.FileGlob...),
			Env:        append([]string(nil), s.Env...),
			Source:     "bundle",
			BundleName: s.BundleName,
		})
	}
	regMu.Lock()
	bundleSpecs = specs
	regMu.Unlock()
}

// SetDisabledServers marks server keys the user turned off in Settings.
func SetDisabledServers(ids []string) {
	next := make(map[string]bool, len(ids))
	for _, id := range ids {
		id = strings.TrimSpace(id)
		if id != "" {
			next[id] = true
		}
	}
	regMu.Lock()
	disabledIDs = next
	regMu.Unlock()
}

// AllServers returns merged Bundle + builtin specs (Bundle key wins).
func AllServers() []ServerSpec {
	regMu.RLock()
	bundlesCopy := append([]ServerSpec(nil), bundleSpecs...)
	regMu.RUnlock()

	byKey := map[string]ServerSpec{}
	order := make([]string, 0, len(builtinServers)+len(bundlesCopy))
	for _, s := range builtinServers {
		byKey[s.Key] = s
		order = append(order, s.Key)
	}
	for _, s := range bundlesCopy {
		if _, ok := byKey[s.Key]; !ok {
			order = append(order, s.Key)
		}
		byKey[s.Key] = s
	}
	out := make([]ServerSpec, 0, len(order))
	seen := map[string]bool{}
	for _, k := range order {
		if seen[k] {
			continue
		}
		seen[k] = true
		out = append(out, byKey[k])
	}
	return out
}

// Lookup returns the first server for a language id, or nil if unsupported / disabled.
func Lookup(languageID string) *ServerSpec {
	all := LookupAll(languageID, "")
	if len(all) == 0 {
		return nil
	}
	cp := all[0]
	return &cp
}

// LookupAll returns every non-disabled server that handles languageID
// (e.g. basedpyright and ruff both claim python). relPath applies FileGlob
// when set; empty relPath skips glob filtering.
//
// Empty languageID matches only servers whose FileGlob claims relPath, so a
// third-party pack can attach without a core language-id map.
func LookupAll(languageID, relPath string) []ServerSpec {
	lang := strings.ToLower(strings.TrimSpace(languageID))
	regMu.RLock()
	disabled := disabledIDs
	regMu.RUnlock()

	var out []ServerSpec
	for _, s := range AllServers() {
		if disabled[s.Key] {
			continue
		}
		if lang == "" {
			if relPath == "" || len(s.FileGlob) == 0 || !s.matchesPath(relPath) {
				continue
			}
		} else if !languageListed(s.Languages, lang) || !s.matchesPath(relPath) {
			continue
		}
		out = append(out, s)
	}
	return preferVuePrimary(out)
}

// Vue 3 needs vue-language-server as the feature client; TS is the tsserver host.
func preferVuePrimary(specs []ServerSpec) []ServerSpec {
	i := -1
	for n, s := range specs {
		if s.Key == "vue-language-server" {
			i = n
			break
		}
	}
	if i <= 0 {
		return specs
	}
	out := make([]ServerSpec, 0, len(specs))
	out = append(out, specs[i])
	out = append(out, specs[:i]...)
	out = append(out, specs[i+1:]...)
	return out
}

func languageListed(langs []string, lang string) bool {
	for _, l := range langs {
		if l == lang {
			return true
		}
	}
	return false
}

func (s ServerSpec) matchesPath(relPath string) bool {
	if relPath == "" || len(s.FileGlob) == 0 {
		return true
	}
	return bundles.MatchFileGlob(s.FileGlob, relPath)
}

// Available reports whether Command resolves on PATH (or as an existing file).
// Managed installs are resolved separately in Manager.Ensure before this check.
func (s ServerSpec) Available() bool {
	if s.Command == "" {
		return false
	}
	if filepath.IsAbs(s.Command) {
		st, err := os.Stat(s.Command)
		return err == nil && !st.IsDir()
	}
	_, err := exec.LookPath(s.Command)
	return err == nil
}
