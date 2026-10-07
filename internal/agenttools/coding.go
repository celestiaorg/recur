package agenttools

import (
	"bytes"
	"context"
	"time"

	"recur/internal/store"
)

const (
	maxReadLines    = 2000
	maxLineChars    = 2000
	maxBashLines    = 500
	maxGrepHits     = 200
	defaultBashWait = 2 * time.Minute
	maxBashWait     = 10 * time.Minute
)

// CodingTools is the native-harness file/shell/todo set (not registered on MCP).
func CodingTools() []Tool {
	return []Tool{
		readTool{},
		editTool{},
		writeTool{},
		globTool{},
		grepTool{},
		bashTool{},
		todoTool{},
	}
}

func (c Call) ctx() context.Context {
	if c.Ctx != nil {
		return c.Ctx
	}
	return context.Background()
}

func isBinary(data []byte) bool {
	n := len(data)
	if n > 512 {
		n = 512
	}
	return bytes.IndexByte(data[:n], 0) >= 0
}

func dataDir() string {
	d, _ := store.DataDir()
	return d
}
