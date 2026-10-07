package agenttools

import (
	"encoding/json"
	"fmt"
	"os"
	"strings"
)

var schemaEdit = json.RawMessage(`{"type":"object","properties":{"file_path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["file_path","old_string","new_string"]}`)

type editTool struct{}

func (editTool) Name() string { return "Edit" }
func (editTool) Description() string {
	return "Exact string replacement in a worktree file. Fails if old_string is not unique unless replace_all is true."
}
func (editTool) Schema() json.RawMessage { return schemaEdit }
func (editTool) Execute(c Call) (string, error) {
	var in struct {
		FilePath   *string `json:"file_path"`
		OldString  *string `json:"old_string"`
		NewString  *string `json:"new_string"`
		ReplaceAll bool    `json:"replace_all"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", fmt.Errorf("invalid Edit args: %w", err)
	}
	if in.FilePath == nil || strings.TrimSpace(*in.FilePath) == "" {
		return "", fmt.Errorf("file_path is required")
	}
	if in.OldString == nil || *in.OldString == "" {
		return "", fmt.Errorf("old_string is required")
	}
	if in.NewString == nil {
		return "", fmt.Errorf("new_string is required")
	}
	abs, err := resolveWritePath(c.Worktree, *in.FilePath)
	if err != nil {
		return "", err
	}
	data, err := os.ReadFile(abs)
	if err != nil {
		return "", err
	}
	content := string(data)
	n := strings.Count(content, *in.OldString)
	if n == 0 {
		return "", fmt.Errorf("old_string not found in %s", *in.FilePath)
	}
	if n > 1 && !in.ReplaceAll {
		return "", fmt.Errorf("old_string matched %d times; provide more context or set replace_all", n)
	}
	var next string
	if in.ReplaceAll {
		next = strings.ReplaceAll(content, *in.OldString, *in.NewString)
	} else {
		next = strings.Replace(content, *in.OldString, *in.NewString, 1)
	}
	if err := os.WriteFile(abs, []byte(next), 0o644); err != nil {
		return "", err
	}
	return fmt.Sprintf("updated %s (%d replacement(s))", *in.FilePath, n), nil
}
