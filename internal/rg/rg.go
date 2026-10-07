// Package rg locates and runs a ripgrep binary for Recur (file listing today;
// content search later). Resolution order: $RECUR_RG, <dataDir>/bin/rg, PATH.
package rg

import (
	"bufio"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// EnvPath overrides ripgrep resolution (dev/tests).
const EnvPath = "RECUR_RG"

// BinName is the executable name under DataDir()/bin/.
const BinName = "rg"

// Locate returns an executable ripgrep path, or an error listing what was tried.
func Locate(dataDir string) (string, error) {
	var tried []string
	add := func(p string) string {
		if p == "" {
			return ""
		}
		if isExecutableFile(p) {
			return p
		}
		tried = append(tried, p)
		return ""
	}

	if p := add(os.Getenv(EnvPath)); p != "" {
		return p, nil
	}
	if dataDir != "" {
		if p := add(filepath.Join(dataDir, "bin", BinName)); p != "" {
			return p, nil
		}
	}
	if p, err := exec.LookPath(BinName); err == nil {
		if resolved := add(p); resolved != "" {
			return resolved, nil
		}
	} else {
		tried = append(tried, "PATH")
	}
	return "", fmt.Errorf("ripgrep (rg) not found (looked in: %s) — set %s or install into <dataDir>/bin/rg",
		strings.Join(tried, ", "), EnvPath)
}

func isExecutableFile(p string) bool {
	fi, err := os.Stat(p)
	if err != nil || fi.IsDir() {
		return false
	}
	return fi.Mode()&0o111 != 0
}

// DefaultGlobs excludes heavy trees from `rg --files -uu` listings.
// Hidden files stay included (-uu); only these path globs are dropped.
var DefaultGlobs = []string{
	"!.git",
	"!.git/**",
	"!node_modules",
	"!node_modules/**",
}

// FilesArgs builds the argv for a recursive file listing under root.
func FilesArgs(root string, extraGlobs ...string) []string {
	args := []string{"--files", "-uu", "--no-messages", "--color", "never"}
	for _, g := range DefaultGlobs {
		args = append(args, "--glob", g)
	}
	for _, g := range extraGlobs {
		if g == "" {
			continue
		}
		args = append(args, "--glob", g)
	}
	args = append(args, root)
	return args
}

// Files runs `rg --files` and invokes emit for each repo-relative path (forward
// slashes). extraGlobs are additional --glob args (typically negated
// files.exclude patterns like "!**/.DS_Store"). emit may return a non-nil
// error to abort (e.g. ctx canceled). Paths are relative to root even when rg
// prints absolute paths.
func Files(ctx context.Context, rgPath, root string, extraGlobs []string, emit func(rel string) error) error {
	if rgPath == "" {
		return fmt.Errorf("rg: empty binary path")
	}
	rootAbs, err := filepath.Abs(root)
	if err != nil {
		return err
	}
	cmd := exec.CommandContext(ctx, rgPath, FilesArgs(rootAbs, extraGlobs...)...)
	cmd.Dir = rootAbs
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return err
	}
	if err := cmd.Start(); err != nil {
		return fmt.Errorf("rg start: %w", err)
	}
	sc := bufio.NewScanner(stdout)
	// Large monorepos can have long paths; raise the token limit.
	sc.Buffer(make([]byte, 0, 64*1024), 1024*1024)
	var emitErr error
	for sc.Scan() {
		if err := ctx.Err(); err != nil {
			_ = cmd.Process.Kill()
			return err
		}
		line := strings.TrimSpace(sc.Text())
		if line == "" {
			continue
		}
		rel := relSlash(rootAbs, line)
		if rel == "" || rel == "." {
			continue
		}
		if emitErr = emit(rel); emitErr != nil {
			_ = cmd.Process.Kill()
			break
		}
	}
	waitErr := cmd.Wait()
	if emitErr != nil {
		return emitErr
	}
	if scanErr := sc.Err(); scanErr != nil {
		return scanErr
	}
	// rg exits 1 when zero matches for content search; --files with matches is 0.
	// Treat canceled ctx as the cause when present.
	if ctx.Err() != nil {
		return ctx.Err()
	}
	if waitErr != nil {
		if _, ok := waitErr.(*exec.ExitError); ok {
			// Empty tree or all filtered — still a successful listing.
			return nil
		}
		return waitErr
	}
	return nil
}

func relSlash(root, path string) string {
	p := path
	if !filepath.IsAbs(p) {
		p = filepath.Join(root, p)
	}
	rel, err := filepath.Rel(root, p)
	if err != nil {
		return filepath.ToSlash(path)
	}
	rel = filepath.ToSlash(rel)
	if strings.HasPrefix(rel, "../") || rel == ".." {
		return ""
	}
	return rel
}
