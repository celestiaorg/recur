package agenttools

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"

	"recur/internal/code"
	"recur/internal/domain"
	"recur/internal/rg"
)

var schemaGrep = json.RawMessage(`{"type":"object","properties":{` +
	`"pattern":{"type":"string","description":"Regex (ripgrep syntax)"},` +
	`"path":{"type":"string","description":"File or directory to search (default: worktree root)"},` +
	`"glob":{"type":"string","description":"Filter files by glob relative to path, e.g. *.go or **/*.{ts,tsx}"},` +
	`"type":{"type":"string","description":"ripgrep file type, e.g. go, ts, py"},` +
	`"output_mode":{"type":"string","enum":["content","files_with_matches","count"],"description":"content (default): matching lines; files_with_matches: paths only; count: matches per file"},` +
	`"-i":{"type":"boolean","description":"Case insensitive"},` +
	`"-A":{"type":"number","description":"Lines of context after each match (content mode)"},` +
	`"-B":{"type":"number","description":"Lines of context before each match (content mode)"},` +
	`"-C":{"type":"number","description":"Lines of context around each match (content mode)"},` +
	`"multiline":{"type":"boolean","description":"Let patterns span lines; . matches newlines"}` +
	`},"required":["pattern"]}`)

type grepTool struct{}

func (grepTool) Name() string { return "Grep" }
func (grepTool) Description() string {
	return "Search file contents with ripgrep. Case sensitive unless -i. Respects .gitignore and includes hidden files. Prefer this over shell grep/rg."
}
func (grepTool) Schema() json.RawMessage { return schemaGrep }
func (grepTool) Execute(c Call) (string, error) {
	var in struct {
		Pattern    string  `json:"pattern"`
		Path       string  `json:"path"`
		Glob       string  `json:"glob"`
		Type       string  `json:"type"`
		OutputMode string  `json:"output_mode"`
		I          bool    `json:"-i"`
		A          float64 `json:"-A"`
		B          float64 `json:"-B"`
		C          float64 `json:"-C"`
		Multiline  bool    `json:"multiline"`
	}
	if err := json.Unmarshal(c.Args, &in); err != nil {
		return "", fmt.Errorf("invalid Grep args: %w", err)
	}
	if in.Pattern == "" {
		return "", fmt.Errorf("pattern is required")
	}
	dir, target, err := searchRoot(c.Worktree, in.Path)
	if err != nil {
		return "", err
	}
	rgPath, err := rg.Locate(dataDir())
	if err != nil {
		return grepFallback(c, dir, target, in.Pattern, in.Glob, in.I)
	}
	args := []string{"--no-config", "--no-messages", "--color", "never", "--hidden", "--glob", "!.git",
		"--max-columns", strconv.Itoa(maxLineChars), "--max-columns-preview"}
	switch in.OutputMode {
	case "", "content":
		args = append(args, "--line-number", "--with-filename", "--no-heading")
		for _, ctxArg := range []struct {
			flag string
			n    float64
		}{{"-A", in.A}, {"-B", in.B}, {"-C", in.C}} {
			if ctxArg.n > 0 {
				args = append(args, ctxArg.flag, strconv.Itoa(int(ctxArg.n)))
			}
		}
	case "files_with_matches":
		args = append(args, "--files-with-matches")
	case "count":
		args = append(args, "--count", "--with-filename")
	default:
		return "", fmt.Errorf("output_mode must be content, files_with_matches, or count")
	}
	if in.I {
		args = append(args, "--ignore-case")
	}
	if in.Multiline {
		args = append(args, "--multiline", "--multiline-dotall")
	}
	args = append(args, globArgs(in.Glob)...)
	if in.Type != "" {
		args = append(args, "--type", in.Type)
	}
	args = append(args, "-e", in.Pattern, target)
	lines, truncated, err := runRg(c.ctx(), rgPath, c.Worktree, dir, args, maxGrepHits)
	if err != nil {
		return "", err
	}
	if len(lines) == 0 {
		return "No matches.", nil
	}
	out := strings.Join(lines, "\n") + "\n"
	if truncated {
		out += fmt.Sprintf("\n<truncated at %d lines>\n", maxGrepHits)
	}
	return out, nil
}

// globArgs filters rg by glob without un-ignoring files: a positive --glob
// overrides .gitignore, a --type filter does not. Type globs only match file
// names, so path globs still use --glob.
// simplify: path globs like src/*.go also match gitignored files; filter rg's
// output in Go if that starts to matter.
func globArgs(g string) []string {
	g = strings.TrimSpace(g)
	if g == "" {
		return nil
	}
	if base := strings.TrimPrefix(g, "**/"); !strings.HasPrefix(g, "!") && !strings.Contains(base, "/") {
		return []string{"--type-add", "recurglob:" + base, "--type", "recurglob"}
	}
	return []string{"--glob", g}
}

// searchRoot resolves path to the directory rg runs in and the target it
// searches there ("." or "./<file>"), so globs are relative to path.
func searchRoot(worktree, path string) (dir, target string, err error) {
	if strings.TrimSpace(path) == "" {
		dir, err = absEval(worktree)
		return dir, ".", err
	}
	abs, err := resolvePath(worktree, path)
	if err != nil {
		return "", "", err
	}
	st, err := os.Stat(abs)
	if err != nil {
		return "", "", err
	}
	if st.IsDir() {
		return abs, ".", nil
	}
	return filepath.Dir(abs), "./" + filepath.Base(abs), nil
}

// worktreePrefix is dir relative to the worktree with a trailing slash ("" at the root).
func worktreePrefix(worktree, dir string) string {
	wt, err := absEval(worktree)
	if err != nil {
		return ""
	}
	rel, err := filepath.Rel(wt, dir)
	if err != nil || rel == "." {
		return ""
	}
	return filepath.ToSlash(rel) + "/"
}

// runRg runs ripgrep in dir and returns up to limit (0 = all) output lines with
// leading "./" paths rewritten to worktree-relative ones.
func runRg(ctx context.Context, rgPath, worktree, dir string, args []string, limit int) (lines []string, truncated bool, err error) {
	prefix := worktreePrefix(worktree, dir)
	cmd := exec.CommandContext(ctx, rgPath, args...)
	cmd.Dir = dir
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, false, err
	}
	if err := cmd.Start(); err != nil {
		return nil, false, fmt.Errorf("rg start: %w", err)
	}
	sc := bufio.NewScanner(stdout)
	sc.Buffer(make([]byte, 0, 64*1024), 1024*1024)
	for sc.Scan() {
		if limit > 0 && len(lines) == limit {
			truncated = true
			_ = cmd.Process.Kill()
			break
		}
		line := sc.Text()
		if rest, ok := strings.CutPrefix(line, "./"); ok {
			line = prefix + rest
		}
		lines = append(lines, line)
	}
	waitErr := cmd.Wait()
	if truncated {
		return lines, true, nil
	}
	if ctx.Err() != nil {
		return nil, false, ctx.Err()
	}
	// Exit 1 is "no matches"; exit 2 with output is a partial error (unreadable file).
	var exitErr *exec.ExitError
	if errors.As(waitErr, &exitErr) && (exitErr.ExitCode() == 1 || len(lines) > 0) {
		return lines, false, nil
	}
	if waitErr != nil {
		if msg := strings.TrimSpace(stderr.String()); msg != "" {
			return nil, false, errors.New(msg)
		}
		return nil, false, waitErr
	}
	return lines, false, nil
}

// grepFallback searches without ripgrep: the editor's Go walk, which skips
// dot-dirs and build dirs and ignores output_mode, type, context, and multiline.
func grepFallback(c Call, dir, target, pattern, glob string, ignoreCase bool) (string, error) {
	if target != "." && glob == "" {
		glob = strings.TrimPrefix(target, "./")
	}
	res, err := code.SearchOpts(c.ctx(), dir, pattern, domain.SearchOpts{
		Regex:         true,
		CaseSensitive: !ignoreCase,
		Glob:          glob,
		Limit:         maxGrepHits,
	})
	if err != nil {
		return "", err
	}
	if len(res.Hits) == 0 {
		msg := "No matches."
		if res.SkippedLarge > 0 {
			msg += fmt.Sprintf(" (%d large files skipped)", res.SkippedLarge)
		}
		return msg, nil
	}
	prefix := worktreePrefix(c.Worktree, dir)
	var b strings.Builder
	for _, h := range res.Hits {
		fmt.Fprintf(&b, "%s%s:%d:%s\n", prefix, h.Path, h.Line, strings.TrimRight(h.Preview, "\n"))
	}
	if res.Truncated {
		fmt.Fprintf(&b, "\n<truncated at %d hits>\n", maxGrepHits)
	}
	return b.String(), nil
}
