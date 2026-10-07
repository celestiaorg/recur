package agenttools

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

var schemaWrite = json.RawMessage(`{"type":"object","properties":{"file_path":{"type":"string"},"content":{"type":"string"}},"required":["file_path","content"]}`)

type writeTool struct{}

func (writeTool) Name() string { return "Write" }
func (writeTool) Description() string {
	return "Create or overwrite a file in the worktree."
}
func (writeTool) Schema() json.RawMessage { return schemaWrite }
func (writeTool) Execute(c Call) (string, error) {
	var in struct {
		FilePath *string `json:"file_path"`
		Content  *string `json:"content"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", fmt.Errorf("invalid Write args: %w", err)
	}
	if in.FilePath == nil || strings.TrimSpace(*in.FilePath) == "" {
		return "", fmt.Errorf("file_path is required")
	}
	if in.Content == nil {
		return "", fmt.Errorf("content is required")
	}
	abs, err := resolveWritePath(c.Worktree, *in.FilePath)
	if err != nil {
		return "", err
	}
	if err := os.MkdirAll(filepath.Dir(abs), 0o755); err != nil {
		return "", err
	}
	if err := os.WriteFile(abs, []byte(*in.Content), 0o644); err != nil {
		return "", err
	}
	return fmt.Sprintf("wrote %s", *in.FilePath), nil
}
