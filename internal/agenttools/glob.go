package agenttools

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"recur/internal/rg"
)

var schemaGlob = json.RawMessage(`{"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"}},"required":["pattern"]}`)

type globTool struct{}

func (globTool) Name() string { return "Glob" }
func (globTool) Description() string {
	return "Find files by glob pattern under the worktree (or path). Prefer this over shell find."
}
func (globTool) Schema() json.RawMessage { return schemaGlob }
func (globTool) Execute(c Call) (string, error) {
	var in struct {
		Pattern string `json:"pattern"`
		Path    string `json:"path"`
	}
	if err := json.Unmarshal(c.Args, &in); err != nil {
		return "", fmt.Errorf("invalid Glob args: %w", err)
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
		root = abs
	}
	rgPath, err := rg.Locate(dataDir())
	if err != nil {
		return walkGlob(root, in.Pattern)
	}
	var matches []string
	g := in.Pattern
	if !strings.Contains(g, "/") && !strings.HasPrefix(g, "*") {
		g = "**/" + g
	}
	err = rg.Files(c.ctx(), rgPath, root, []string{g}, func(rel string) error {
		matches = append(matches, rel)
		return nil
	})
	if err != nil {
		return "", err
	}
	if len(matches) == 0 {
		return "No files matched.", nil
	}
	return strings.Join(matches, "\n"), nil
}

func walkGlob(root, pattern string) (string, error) {
	var matches []string
	err := filepath.WalkDir(root, func(p string, d os.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() {
			switch d.Name() {
			case ".git", "node_modules":
				return filepath.SkipDir
			}
			return nil
		}
		rel, err := filepath.Rel(root, p)
		if err != nil {
			return nil
		}
		ok, _ := filepath.Match(pattern, filepath.Base(rel))
		if !ok {
			ok, _ = filepath.Match(pattern, rel)
		}
		if ok {
			matches = append(matches, filepath.ToSlash(rel))
		}
		return nil
	})
	if err != nil {
		return "", err
	}
	if len(matches) == 0 {
		return "No files matched.", nil
	}
	return strings.Join(matches, "\n"), nil
}
