package cliinstall

import (
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
)

// EnsurePATH replaces launchd's stunted PATH with the user's login-shell PATH
// so LookPath finds claude, gh, Homebrew, nvm, and the rest. Recur.app from
// the Dock does not inherit zshrc; a CLI launch (`open bin/Recur.app`) does.
// Common directories are still merged in so a missing ~/.local/bin in the
// captured PATH does not hide the native Claude installer.
func EnsurePATH() {
	base := os.Getenv("PATH")
	if p := readLoginPATH(); p != "" {
		base = p
	}
	merged := mergePATH(base, extraPATHDirs())
	if merged == os.Getenv("PATH") {
		return
	}
	_ = os.Setenv("PATH", merged)
}

func extraPATHDirs() []string {
	var dirs []string
	if home, err := os.UserHomeDir(); err == nil {
		dirs = append(dirs, userPATHDirs(home)...)
	}
	dirs = append(dirs, systemPATHDirs()...)
	if runtime.GOOS == "darwin" {
		dirs = append(dirs, readPathHelperDirs("/")...)
	}
	return dirs
}

// userPATHDirs are home-relative bin dirs shells usually prepend. Missing
// directories are skipped later; listing a path that does not exist is cheap.
func userPATHDirs(home string) []string {
	rel := [][]string{
		{".local", "bin"},       // Claude Code native, pipx, uv, Recur CLI
		{"bin"},                 // classic ~/bin
		{".cargo", "bin"},       // rustup
		{"go", "bin"},           // go install (default GOPATH)
		{".volta", "bin"},       // Volta node
		{".asdf", "shims"},      // asdf
		{".pyenv", "shims"},     // pyenv
		{".rbenv", "shims"},     // rbenv
		{".nix-profile", "bin"}, // Nix user profile
		{".claude", "local"},    // legacy Claude Code npm local
		{".claude", "local", "node_modules", ".bin"},
	}
	dirs := make([]string, 0, len(rel)+1)
	for _, parts := range rel {
		dirs = append(dirs, filepath.Join(append([]string{home}, parts...)...))
	}
	if runtime.GOOS == "darwin" {
		dirs = append(dirs, filepath.Join(home, "Library", "pnpm"))
	}
	return dirs
}

func systemPATHDirs() []string {
	return []string{
		"/opt/homebrew/bin",
		"/opt/homebrew/sbin",
		SystemBinDir(),
		"/usr/local/sbin",
		"/opt/local/bin",
		"/opt/local/sbin",
		"/nix/var/nix/profiles/default/bin",
		"/home/linuxbrew/.linuxbrew/bin",
		"/home/linuxbrew/.linuxbrew/sbin",
	}
}

// readPathHelperDirs mirrors /usr/libexec/path_helper: /etc/paths, then
// /etc/paths.d in lexical order. Used as a fallback when the login shell
// cannot be probed, not as a substitute for user dotfiles.
func readPathHelperDirs(root string) []string {
	var dirs []string
	dirs = append(dirs, readPathLines(filepath.Join(root, "etc", "paths"))...)
	entries, err := os.ReadDir(filepath.Join(root, "etc", "paths.d"))
	if err != nil {
		return dirs
	}
	names := make([]string, 0, len(entries))
	for _, e := range entries {
		if e.IsDir() || strings.HasPrefix(e.Name(), ".") {
			continue
		}
		names = append(names, e.Name())
	}
	sort.Strings(names)
	for _, name := range names {
		dirs = append(dirs, readPathLines(filepath.Join(root, "etc", "paths.d", name))...)
	}
	return dirs
}

func readPathLines(path string) []string {
	raw, err := os.ReadFile(path)
	if err != nil {
		return nil
	}
	var out []string
	for _, line := range strings.Split(string(raw), "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		out = append(out, line)
	}
	return out
}

// mergePATH prepends extra directories that exist and are not already on
// path, preserving path's order for everything else.
func mergePATH(path string, extra []string) string {
	sep := string(os.PathListSeparator)
	parts := filepath.SplitList(path)
	seen := make(map[string]bool, len(parts)+len(extra))
	for _, p := range parts {
		if p != "" {
			seen[p] = true
		}
	}
	var prepend []string
	for _, d := range extra {
		d = strings.TrimSpace(d)
		if d == "" || seen[d] || !isDir(d) {
			continue
		}
		prepend = append(prepend, d)
		seen[d] = true
	}
	if len(prepend) == 0 {
		return path
	}
	return strings.Join(append(prepend, parts...), sep)
}

func isDir(p string) bool {
	st, err := os.Stat(p)
	return err == nil && st.IsDir()
}
