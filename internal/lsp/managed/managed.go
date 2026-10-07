// Package managed downloads and pins language-server binaries under DataDir
// (same spirit as internal/rg).
//
//	gopls                     → go install (no official multi-arch tarballs)
//	rust-analyzer / marksman / ruff / taplo → GitHub release assets + SHA-256
//	npm language servers      → npm install --prefix (needs node/npm)
package managed

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"time"
)

// Env overrides for a managed binary (absolute path). When set and executable,
// Ensure returns it without installing.
const (
	EnvGopls        = "RECUR_LSP_GOPLS"
	EnvRustAnalyzer = "RECUR_LSP_RUST_ANALYZER"
	EnvMarksman     = "RECUR_LSP_MARKSMAN"
	EnvTypeScript   = "RECUR_LSP_TYPESCRIPT_LANGUAGE_SERVER"
	EnvBasedPyright = "RECUR_LSP_BASEDPYRIGHT"
	EnvRuff         = "RECUR_LSP_RUFF"
	EnvTaplo        = "RECUR_LSP_TAPLO"
	EnvJSON         = "RECUR_LSP_JSON"
	EnvHTML         = "RECUR_LSP_HTML"
	EnvCSS          = "RECUR_LSP_CSS"
	EnvYAML         = "RECUR_LSP_YAML"
	EnvBash         = "RECUR_LSP_BASH_LANGUAGE_SERVER"
	EnvSvelte       = "RECUR_LSP_SVELTE_LANGUAGE_SERVER"
	EnvVue          = "RECUR_LSP_VUE_LANGUAGE_SERVER"
)

var ensureMu sync.Mutex

// Ensure returns an absolute path to a usable language server for managedID.
// Soft-falls back to PATH when install fails. Serialized so concurrent first-use
// installs cannot race on a shared prefix (vscode-langservers-extracted).
func Ensure(ctx context.Context, dataDir, managedID string) (string, error) {
	ensureMu.Lock()
	defer ensureMu.Unlock()
	switch strings.ToLower(strings.TrimSpace(managedID)) {
	case "gopls":
		return ensureGopls(ctx, dataDir)
	case "rust-analyzer":
		return ensureRustAnalyzer(ctx, dataDir)
	case "marksman":
		return ensureMarksman(ctx, dataDir)
	case "typescript-language-server":
		return ensureTypeScriptLanguageServer(ctx, dataDir)
	case "basedpyright":
		return ensureBasedPyright(ctx, dataDir)
	case "ruff":
		return ensureRuff(ctx, dataDir)
	case "taplo":
		return ensureTaplo(ctx, dataDir)
	case "yaml-language-server":
		return ensureYAMLLanguageServer(ctx, dataDir)
	case "bash-language-server":
		return ensureBashLanguageServer(ctx, dataDir)
	case "svelte-language-server":
		return ensureSvelteLanguageServer(ctx, dataDir)
	case "vue-language-server":
		return ensureVueLanguageServer(ctx, dataDir)
	case "vscode-json-language-server":
		return ensureVSCodeJSONLanguageServer(ctx, dataDir)
	case "vscode-html-language-server":
		return ensureVSCodeHTMLLanguageServer(ctx, dataDir)
	case "vscode-css-language-server":
		return ensureVSCodeCSSLanguageServer(ctx, dataDir)
	default:
		return "", fmt.Errorf("lsp managed: unknown id %q", managedID)
	}
}

func managedBinDir(dataDir, id string) string {
	return filepath.Join(dataDir, "bin", "lsp", id)
}

// VuePluginPath is the @vue/language-server package dir (hosts @vue/typescript-plugin).
// simplify: managed npm prefix only; PATH/env Vue binaries skip pairing until that prefix exists.
func VuePluginPath(dataDir string) string {
	if dataDir == "" {
		return ""
	}
	p := filepath.Join(managedBinDir(dataDir, "vue-language-server"), "node_modules", "@vue", "language-server")
	st, err := os.Stat(p)
	if err != nil || !st.IsDir() {
		return ""
	}
	return p
}

func managedVersion(dataDir, id string) string {
	b, err := os.ReadFile(filepath.Join(managedBinDir(dataDir, id), id+".version"))
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(b))
}

func writeManagedVersion(dataDir, id, version string) error {
	dir := managedBinDir(dataDir, id)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(dir, id+".version"), []byte(version+"\n"), 0o644)
}

func isExecutableFile(path string) bool {
	st, err := os.Stat(path)
	if err != nil || st.IsDir() {
		return false
	}
	if runtime.GOOS == "windows" {
		return true
	}
	return st.Mode()&0o111 != 0
}

func lookPathFallback(names ...string) string {
	for _, name := range names {
		if p, err := execLookPath(name); err == nil && isExecutableFile(p) {
			return p
		}
	}
	return ""
}

// execLookPath is a thin wrapper so tests can stub if needed later.
func execLookPath(name string) (string, error) {
	return lookPath(name)
}

func ensurePinnedBinary(
	ctx context.Context,
	dataDir, id, version, envKey, pathName, binName string,
	install func(ctx context.Context, dataDir, dest string) error,
) (string, error) {
	if p := os.Getenv(envKey); p != "" && isExecutableFile(p) {
		return p, nil
	}
	if binName == "" {
		binName = pathName
	}
	if dataDir != "" {
		dest := filepath.Join(managedBinDir(dataDir, id), binName)
		if isExecutableFile(dest) && managedVersion(dataDir, id) == version {
			return dest, nil
		}
		if err := install(ctx, dataDir, dest); err != nil {
			if p := lookPathFallback(pathName); p != "" {
				return p, nil
			}
			return "", err
		}
		return dest, nil
	}
	if p := lookPathFallback(pathName); p != "" {
		return p, nil
	}
	return "", fmt.Errorf("lsp managed: no data directory and %s not on PATH", pathName)
}

func nativeBinName(name string) string {
	if runtime.GOOS == "windows" && !strings.HasSuffix(strings.ToLower(name), ".exe") {
		return name + ".exe"
	}
	return name
}

func downloadVerified(ctx context.Context, url, wantSHA256, dest string) error {
	ctx, cancel := context.WithTimeout(ctx, 5*time.Minute)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return err
	}
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		return fmt.Errorf("lsp managed: download: %w", err)
	}
	defer res.Body.Close()
	if res.StatusCode != http.StatusOK {
		return fmt.Errorf("lsp managed: download: HTTP %s", res.Status)
	}
	if err := os.MkdirAll(filepath.Dir(dest), 0o755); err != nil {
		return err
	}
	staging := dest + ".partial"
	f, err := os.OpenFile(staging, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0o644)
	if err != nil {
		return err
	}
	hasher := sha256.New()
	_, copyErr := io.Copy(io.MultiWriter(f, hasher), res.Body)
	closeErr := f.Close()
	if copyErr != nil {
		_ = os.Remove(staging)
		return fmt.Errorf("lsp managed: download: %w", copyErr)
	}
	if closeErr != nil {
		_ = os.Remove(staging)
		return closeErr
	}
	got := hex.EncodeToString(hasher.Sum(nil))
	if wantSHA256 != "" && got != wantSHA256 {
		_ = os.Remove(staging)
		return fmt.Errorf("lsp managed: SHA-256 mismatch: got %s want %s", got, wantSHA256)
	}
	if err := os.Rename(staging, dest); err != nil {
		_ = os.Remove(staging)
		return err
	}
	return nil
}

func gunzipFile(src, dest string) error {
	in, err := os.Open(src)
	if err != nil {
		return err
	}
	defer in.Close()
	return gunzipReader(in, dest)
}
