package app

import (
	"path/filepath"
	"sync"
)

// looseFiles is the allowlist of absolute paths the user opened that are not
// inside a project worktree. Observatory Read/Write may touch these; agent
// tools still go through SafeJoin and cannot see them.
type looseFiles struct {
	mu      sync.Mutex
	allowed map[string]struct{}
}

func newLooseFiles() *looseFiles {
	return &looseFiles{allowed: map[string]struct{}{}}
}

func (l *looseFiles) allow(abs string) {
	if l == nil || abs == "" {
		return
	}
	abs = filepath.Clean(abs)
	l.mu.Lock()
	l.allowed[abs] = struct{}{}
	l.mu.Unlock()
}

func (l *looseFiles) ok(abs string) bool {
	if l == nil || abs == "" {
		return false
	}
	abs = filepath.Clean(abs)
	l.mu.Lock()
	_, ok := l.allowed[abs]
	l.mu.Unlock()
	return ok
}
