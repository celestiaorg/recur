package agenttools

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"recur/internal/code"
	"recur/internal/domain"
)

var schemaGrep = json.RawMessage(`{"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"},"glob":{"type":"string"}},"required":["pattern"]}`)

type grepTool struct{}

func (grepTool) Name() string { return "Grep" }
func (grepTool) Description() string {
	return "Search file contents with a regex. Optional path (directory) and glob filter."
}
func (grepTool) Schema() json.RawMessage { return schemaGrep }
func (grepTool) Execute(c Call) (string, error) {
	var in struct {
		Pattern string `json:"pattern"`
		Path    string `json:"path"`
		Glob    string `json:"glob"`
	}
	if err := json.Unmarshal(c.Args, &in); err != nil {
		return "", fmt.Errorf("invalid Grep args: %w", err)
	}
	if in.Pattern == "" {
		return "", fmt.Errorf("pattern is required")
	}
	root := c.Worktree
	if strings.TrimSpace(in.Path) != "" {
		abs, err := resolvePath(c.Worktree, in.Path)
		if err != nil {
			return "", err
		}
		st, err := os.Stat(abs)
		if err != nil {
			return "", err
		}
		if !st.IsDir() {
			root = filepath.Dir(abs)
			if in.Glob == "" {
				in.Glob = filepath.Base(abs)
			}
		} else {
			root = abs
		}
	}
	res, err := code.SearchOpts(c.ctx(), root, in.Pattern, domain.SearchOpts{
		Regex: true,
		Glob:  in.Glob,
		Limit: maxGrepHits,
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
	var b strings.Builder
	for _, h := range res.Hits {
		fmt.Fprintf(&b, "%s:%d:%s\n", h.Path, h.Line, strings.TrimRight(h.Preview, "\n"))
	}
	if res.Truncated {
		fmt.Fprintf(&b, "\n<truncated at %d hits>\n", maxGrepHits)
	}
	return b.String(), nil
}
