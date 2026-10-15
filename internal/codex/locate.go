// Package codex is a Go client for OpenAI's `codex app-server` JSON-RPC
// surface. Recur drives Codex agents over stdio (newline-delimited JSON,
// JSON-RPC 2.0 with the `"jsonrpc":"2.0"` header omitted) rather than via the
// process-per-turn `codex exec` CLI or a Python/TS SDK wrapper.
//
// Why app-server (not exec): mid-turn steer (`turn/steer`), server→client
// approvals/elicitations, and thread resume map cleanly onto Recur's
// AgentRunner contract (D49 Inject, ProviderSessionRef, Recur MCP).
//
// Isolation rules (Cursor's hard lesson applied here):
//   - CODEX_HOME is Recur-owned under the app data dir — never ~/.codex for
//     writes, and never a `.codex/` directory inside the loop worktree.
//   - Per-agent Recur MCP is injected via `--config mcp_servers…` on the
//     process argv, so nothing is written into the worktree the diff/PR flow
//     reads.
package codex

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
)

// EnvBinPath overrides binary resolution. Used for development and tests.
const EnvBinPath = "RECUR_CODEX"

// Locate resolves the `codex` CLI binary, in precedence order:
//
//  1. $RECUR_CODEX — explicit override (dev/test).
//  2. configured — Settings.CodexBinPath.
//  3. `codex` on PATH (must be a real executable, not a broken npm shim).
//  4. Common Homebrew cask locations on macOS.
func Locate(configured string) (string, error) {
	var tried []string
	add := func(p string) string {
		p = strings.TrimSpace(p)
		if p == "" {
			return ""
		}
		if resolved := resolveCandidate(p); resolved != "" {
			return resolved
		}
		tried = append(tried, p)
		return ""
	}

	if p := add(os.Getenv(EnvBinPath)); p != "" {
		return p, nil
	}
	if p := add(configured); p != "" {
		return p, nil
	}
	if p, err := exec.LookPath("codex"); err == nil {
		if resolved := resolveCandidate(p); resolved != "" {
			return resolved, nil
		}
		tried = append(tried, p+" (unusable)")
	}
	for _, cand := range commonBinCandidates() {
		if p := add(cand); p != "" {
			return p, nil
		}
	}
	hint := "install a native codex binary from https://github.com/openai/codex/releases"
	if runtime.GOOS == "darwin" {
		hint = "install the Codex cask (`brew install --cask codex`)"
	}
	return "", fmt.Errorf("codex binary not found (looked in: %s) — %s or set %s",
		strings.Join(tried, ", "), hint, EnvBinPath)
}

func resolveCandidate(p string) string {
	abs, err := filepath.Abs(p)
	if err != nil {
		return ""
	}
	fi, err := os.Stat(abs)
	if err != nil || fi.IsDir() {
		return ""
	}
	// Reject the broken npm JS shim that spawns a missing vendor binary.
	if !looksLikeNativeCodex(abs) {
		return ""
	}
	return abs
}

// looksLikeNativeCodex rejects the Node wrapper at
// node_modules/@openai/codex/bin/codex.js (and its PATH symlink) whose vendor
// binary is often missing. A real CLI is a Mach-O / ELF binary, not a script.
func looksLikeNativeCodex(path string) bool {
	f, err := os.Open(path)
	if err != nil {
		return false
	}
	defer f.Close()
	var hdr [4]byte
	if _, err := f.Read(hdr[:]); err != nil {
		return false
	}
	// Mach-O (fat/thin) or ELF.
	if hdr[0] == 0xCF && hdr[1] == 0xFA && hdr[2] == 0xED && hdr[3] == 0xFE {
		return true
	}
	if hdr[0] == 0xCE && hdr[1] == 0xFA && hdr[2] == 0xED && hdr[3] == 0xFE {
		return true
	}
	if hdr[0] == 0xCA && hdr[1] == 0xFE && hdr[2] == 0xBA && hdr[3] == 0xBE {
		return true
	}
	if hdr[0] == 0x7F && hdr[1] == 'E' && hdr[2] == 'L' && hdr[3] == 'F' {
		return true
	}
	return false
}

func commonBinCandidates() []string {
	if runtime.GOOS != "darwin" {
		return nil
	}
	var out []string
	for _, root := range []string{"/opt/homebrew/Caskroom/codex", "/usr/local/Caskroom/codex"} {
		out = append(out, filepath.Join(root, "latest", "bin", "codex"))
		entries, err := os.ReadDir(root)
		if err != nil {
			continue
		}
		for _, e := range entries {
			if !e.IsDir() || e.Name() == "latest" {
				continue
			}
			out = append(out, filepath.Join(root, e.Name(), "codex-package", "bin", "codex"))
			out = append(out, filepath.Join(root, e.Name(), "bin", "codex"))
		}
	}
	out = append(out, "/Applications/Codex.app/Contents/Resources/bin/codex")
	return out
}

// HomeDirName is the single Recur-owned CODEX_HOME directory under the app
// data dir. One home for the whole app — auth, sessions, and trust live here;
// worktree isolation is via cwd on each thread/turn, not separate homes.
const HomeDirName = "codex-home"

// HomeDir returns the Recur-owned CODEX_HOME under dataDir.
func HomeDir(dataDir string) string {
	return filepath.Join(dataDir, HomeDirName)
}
