package agenttools

import (
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"unicode/utf8"
)

var schemaRead = json.RawMessage(`{"type":"object","properties":{"file_path":{"type":"string"},"offset":{"type":"integer"},"limit":{"type":"integer"}},"required":["file_path"]}`)

type readTool struct{}

func (readTool) Name() string { return "Read" }
func (readTool) Description() string {
	return "Read a file from the worktree. Returns cat -n format. Optional offset (1-based) and limit (default 2000 lines)."
}
func (readTool) Schema() json.RawMessage { return schemaRead }
func (readTool) Execute(c Call) (string, error) {
	var in struct {
		FilePath string `json:"file_path"`
		Offset   int    `json:"offset"`
		Limit    int    `json:"limit"`
	}
	if err := json.Unmarshal(c.Args, &in); err != nil {
		return "", fmt.Errorf("invalid Read args: %w", err)
	}
	abs, err := resolvePath(c.Worktree, in.FilePath)
	if err != nil {
		return "", err
	}
	data, err := os.ReadFile(abs)
	if err != nil {
		return "", err
	}
	if isBinary(data) {
		return "", fmt.Errorf("binary file")
	}
	offset := in.Offset
	if offset < 1 {
		offset = 1
	}
	limit := in.Limit
	if limit <= 0 {
		limit = maxReadLines
	}
	lines := strings.Split(string(data), "\n")
	// Split leaves a trailing empty element when the file ends in newline.
	if len(lines) > 0 && lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	}
	if offset > len(lines)+1 {
		return "", fmt.Errorf("offset %d is past end of file (%d lines)", offset, len(lines))
	}
	end := offset - 1 + limit
	if end > len(lines) {
		end = len(lines)
	}
	var b strings.Builder
	for i := offset - 1; i < end; i++ {
		line := lines[i]
		if utf8.RuneCountInString(line) > maxLineChars {
			runes := []rune(line)
			line = string(runes[:maxLineChars]) + "…"
		}
		fmt.Fprintf(&b, "%6d\t%s\n", i+1, line)
	}
	if end < len(lines) {
		fmt.Fprintf(&b, "\n<response clipped: showing lines %d–%d of %d>\n", offset, end, len(lines))
	}
	if b.Len() == 0 {
		return "<empty file>\n", nil
	}
	return b.String(), nil
}
