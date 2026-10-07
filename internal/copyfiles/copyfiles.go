// Package copyfiles copies glob-matched files from a project's checkout into
// a newly created workspace worktree (untracked dotenv, secrets, local config).
package copyfiles

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"

	"recur/internal/code"
	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

const (
	maxCopyFiles     = 10_000
	maxPreviewFiles  = 250
	maxCopyFileBytes = 32 << 20
	previewTimeout   = 15 * time.Second
)

// scanLimit is the match cap shared by Preview and Copy. Tests may lower it.
var scanLimit = maxCopyFiles

var (
	errNegation = errors.New("negation (!) is not supported")
	errExists   = errors.New("already exists")
)

func errTooMany() error {
	return fmt.Errorf("more than %d matching files; narrow the globs", scanLimit)
}

// ParsePatterns splits CopyFiles text into glob patterns. Blank lines and
// # comments are ignored. Backslashes become slashes; a leading slash is dropped.
func ParsePatterns(text string) []string {
	var out []string
	for _, line := range strings.Split(text, "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") || strings.HasPrefix(line, "!") {
			continue
		}
		line = strings.ReplaceAll(line, `\`, "/")
		line = strings.TrimLeft(line, "/")
		if line == "" {
			continue
		}
		out = append(out, line)
	}
	return out
}

func hasNegation(text string) bool {
	for _, line := range strings.Split(text, "\n") {
		if strings.HasPrefix(strings.TrimSpace(line), "!") {
			return true
		}
	}
	return false
}

// Match reports whether a repo-relative forward-slash path matches any pattern.
// A pattern with no slash matches at any depth (.env* → apps/web/.env.local).
func Match(rel string, patterns []string) bool {
	rel = strings.ReplaceAll(rel, `\`, "/")
	rel = strings.TrimLeft(rel, "/")
	if rel == "" || rel == "." {
		return false
	}
	return compileMatcher(patterns).match(rel)
}

// Preview lists files under root that match patterns, for the settings page.
// A missing root or empty patterns yields an empty preview (no error).
// Bounded by ctx and, if ctx has no deadline, previewTimeout.
func Preview(ctx context.Context, root, patterns string) domain.CopyFilesPreview {
	if ctx == nil {
		ctx = context.Background()
	}
	if _, ok := ctx.Deadline(); !ok {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, previewTimeout)
		defer cancel()
	}
	root = strings.TrimSpace(gitpkg.ExpandHome(root))
	out := domain.CopyFilesPreview{Root: root}
	if hasNegation(patterns) {
		out.Error = errNegation.Error()
		return out
	}
	pats := ParsePatterns(patterns)
	if root == "" || len(pats) == 0 {
		return out
	}
	files, err := list(ctx, root, pats, scanLimit+1)
	if err != nil {
		out.Error = previewError(root, err)
		return out
	}
	if len(files) > scanLimit {
		out.Error = errTooMany().Error()
		return out
	}
	out.Count = len(files)
	if files == nil {
		files = []string{}
	}
	if len(files) > maxPreviewFiles {
		out.Truncated = true
		files = files[:maxPreviewFiles]
	}
	out.Files = files
	return out
}

func previewError(root string, err error) string {
	if errors.Is(err, context.DeadlineExceeded) || errors.Is(err, context.Canceled) {
		return "Scan timed out — narrow the globs"
	}
	if os.IsNotExist(err) {
		return "Checkout not found at " + root
	}
	return err.Error()
}

// Copy writes matching files from src into dest, preserving relative paths and
// file mode. Dest directories are created as needed. Existing dest files are
// left in place so a restored worktree keeps its own copies. Per-file failures
// (unreadable, oversized, unjoinable names) are logged and skipped and never
// fail the copy — one bad match must not abort workspace create. Copy errors
// when the walk fails, the match cap is exceeded, the context is cancelled,
// or the patterns use unsupported negation.
func Copy(ctx context.Context, src, dest, patterns string) (int, error) {
	if ctx == nil {
		ctx = context.Background()
	}
	src = strings.TrimSpace(gitpkg.ExpandHome(src))
	dest = strings.TrimSpace(dest)
	if hasNegation(patterns) {
		return 0, errNegation
	}
	pats := ParsePatterns(patterns)
	if src == "" || dest == "" || len(pats) == 0 {
		return 0, nil
	}
	srcAbs, err := filepath.Abs(src)
	if err != nil {
		return 0, err
	}
	destAbs, err := filepath.Abs(dest)
	if err != nil {
		return 0, err
	}
	if srcAbs == destAbs {
		return 0, nil
	}
	files, err := list(ctx, srcAbs, pats, scanLimit+1)
	if err != nil {
		return 0, err
	}
	if len(files) > scanLimit {
		return 0, errTooMany()
	}
	n := 0
	for _, rel := range files {
		if err := ctx.Err(); err != nil {
			return n, err
		}
		if err := copyOne(srcAbs, destAbs, rel); err != nil {
			if errors.Is(err, errExists) {
				continue
			}
			log.Printf("recur: copy files: skip %s: %v", rel, err)
			continue
		}
		n++
	}
	return n, nil
}

func copyOne(srcRoot, destRoot, rel string) error {
	from, err := joinUnderRoot(srcRoot, rel)
	if err != nil {
		return err
	}
	to, err := joinUnderRoot(destRoot, rel)
	if err != nil {
		return err
	}
	if _, err := os.Lstat(to); err == nil {
		return errExists
	} else if !os.IsNotExist(err) {
		return err
	}
	info, err := os.Lstat(from)
	if err != nil {
		return err
	}
	if info.Mode()&os.ModeSymlink != 0 || !info.Mode().IsRegular() {
		return nil
	}
	if info.Size() > maxCopyFileBytes {
		return fmt.Errorf("file larger than %d bytes", maxCopyFileBytes)
	}
	in, err := os.Open(from)
	if err != nil {
		return err
	}
	defer in.Close()
	if err := os.MkdirAll(filepath.Dir(to), 0o755); err != nil {
		return err
	}
	out, err := os.OpenFile(to, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, info.Mode().Perm())
	if err != nil {
		return err
	}
	_, copyErr := io.Copy(out, in)
	closeErr := out.Close()
	if copyErr != nil {
		return copyErr
	}
	return closeErr
}

// joinUnderRoot joins root + rel and ensures the result stays under root.
// Unlike code.SafeJoin it allows ".." inside a filename (.env..bak) and
// rejects ".." only as a path component (caught by Abs + prefix).
func joinUnderRoot(root, rel string) (string, error) {
	rootAbs, err := filepath.Abs(root)
	if err != nil {
		return "", err
	}
	rel = strings.TrimPrefix(filepath.ToSlash(rel), "/")
	if rel == "" || rel == "." {
		return "", fmt.Errorf("empty path")
	}
	for _, p := range strings.Split(rel, "/") {
		if p == ".." {
			return "", fmt.Errorf("path escapes root")
		}
	}
	fullAbs, err := filepath.Abs(filepath.Join(rootAbs, filepath.FromSlash(rel)))
	if err != nil {
		return "", err
	}
	prefix := rootAbs + string(os.PathSeparator)
	if fullAbs != rootAbs && !strings.HasPrefix(fullAbs, prefix) {
		return "", fmt.Errorf("path escapes root")
	}
	return fullAbs, nil
}

func list(ctx context.Context, root string, patterns []string, limit int) ([]string, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	rootAbs, err := filepath.Abs(root)
	if err != nil {
		return nil, err
	}
	st, err := os.Stat(rootAbs)
	if err != nil {
		return nil, err
	}
	if !st.IsDir() {
		return nil, fmt.Errorf("%s is not a directory", rootAbs)
	}
	m := compileMatcher(patterns)
	var out []string
	err = filepath.WalkDir(rootAbs, func(path string, d os.DirEntry, err error) error {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		if err != nil {
			return nil
		}
		if d.IsDir() {
			if path != rootAbs && skipWalkDir(d.Name(), patterns) {
				return filepath.SkipDir
			}
			return nil
		}
		if d.Type()&os.ModeSymlink != 0 {
			return nil
		}
		rel := code.RelSlash(rootAbs, path)
		if rel == "" || rel == "." || strings.HasPrefix(rel, "../") {
			return nil
		}
		if !m.match(rel) {
			return nil
		}
		out = append(out, rel)
		if len(out) >= limit {
			return filepath.SkipAll
		}
		return nil
	})
	if errors.Is(err, filepath.SkipAll) {
		err = nil
	}
	if err != nil {
		return nil, err
	}
	sort.Strings(out)
	return out, nil
}

type matcher struct {
	res []*regexp.Regexp
}

func compileMatcher(patterns []string) matcher {
	var res []*regexp.Regexp
	for _, p := range patterns {
		for _, cand := range expandPattern(p) {
			res = append(res, regexp.MustCompile(globToRegexp(cand)))
		}
	}
	return matcher{res: res}
}

func (m matcher) match(rel string) bool {
	for _, re := range m.res {
		if re.MatchString(rel) {
			return true
		}
	}
	return false
}

// expandPattern turns gitignore-style globs into path-anchored VS Code globs.
// A pattern whose original form has no slash (secrets/, .env*) matches at any
// depth. Trailing slashes mean "this directory" (secrets/ → **/secrets/**).
func expandPattern(p string) []string {
	anyDepth := !strings.Contains(strings.TrimSuffix(p, "/"), "/")
	if strings.HasSuffix(p, "/") {
		p += "**"
	}
	if anyDepth {
		return []string{"**/" + p}
	}
	return []string{p}
}

func globToRegexp(glob string) string {
	var out strings.Builder
	out.WriteByte('^')
	for i := 0; i < len(glob); i++ {
		c := glob[i]
		if c == '*' && i+1 < len(glob) && glob[i+1] == '*' {
			if i+2 < len(glob) && glob[i+2] == '/' {
				out.WriteString("(?:.*/)?")
				i += 2
			} else {
				out.WriteString(".*")
				i++
			}
			continue
		}
		if c == '*' {
			out.WriteString("[^/]*")
			continue
		}
		if c == '?' {
			out.WriteString("[^/]")
			continue
		}
		if strings.ContainsRune(`.+^${}()|[]\`, rune(c)) {
			out.WriteByte('\\')
			out.WriteByte(c)
			continue
		}
		out.WriteByte(c)
	}
	out.WriteByte('$')
	return out.String()
}

// skipWalkDir prunes .git always, and well-known heavy dirs unless a pattern
// names them or is a catch-all.
func skipWalkDir(name string, patterns []string) bool {
	if name == ".git" {
		return true
	}
	if !heavyDir[name] {
		return false
	}
	for _, p := range patterns {
		if p == "**" || p == "**/*" || p == "*" {
			return false
		}
		if strings.Contains(p, name) {
			return false
		}
	}
	return true
}

var heavyDir = map[string]bool{
	"node_modules": true, "dist": true, "build": true, "vendor": true,
	".next": true, "coverage": true, "__pycache__": true, ".pytest_cache": true,
	".mypy_cache": true, ".tox": true, ".venv": true, "venv": true,
	".turbo": true, ".cache": true, "target": true, "Pods": true,
	".gradle": true, ".idea": true, "out": true, "obj": true,
}
