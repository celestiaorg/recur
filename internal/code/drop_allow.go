package code

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"sync"
)

// Absolute paths the user dropped onto the app (WindowFilesDropped).
// /chat-image and /external-file only serve paths from this set so the webview
// cannot fetch arbitrary files by crafting a URL.
//
// Native drops persist the set under DataDir so a relaunch can still serve a
// transcript chip or a restored editor tab. There is no frontend-callable
// "allow this path" RPC, and editor-session restore does not grant paths —
// UpdateEditorSession is reachable from the webview, so a session snapshot
// is not evidence of a user drop.
const maxExternalAllowedDefault = 512

var (
	externalMu         sync.Mutex
	externalAllowed    = map[string]struct{}{}
	externalOrder      []string // oldest-first; eviction drops index 0
	externalAllowFile  string
	maxExternalAllowed = maxExternalAllowedDefault
)

// LoadExternalAllowFile points the persist file at path and merges any
// previously written entries into the in-memory set. Pass "" to disable
// persist (tests). Called once from main after DataDir is known.
func LoadExternalAllowFile(path string) {
	externalMu.Lock()
	externalAllowFile = path
	externalMu.Unlock()
	if path == "" {
		return
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return
	}
	var paths []string
	if json.Unmarshal(data, &paths) != nil {
		return
	}
	externalMu.Lock()
	for _, p := range paths {
		rememberExternalPath(p)
	}
	externalMu.Unlock()
}

// AllowExternalPath records an absolute path from a native file drop and
// persists the allowlist when a persist file is configured.
func AllowExternalPath(abs string) {
	externalMu.Lock()
	defer externalMu.Unlock()
	if !rememberExternalPath(abs) {
		return
	}
	persistExternalAllowLocked()
}

// ResetExternalPaths clears the in-memory allowlist. Tests only. Does not
// delete the persist file; call LoadExternalAllowFile("") to disable persist.
func ResetExternalPaths() {
	externalMu.Lock()
	externalAllowed = map[string]struct{}{}
	externalOrder = nil
	externalMu.Unlock()
}

func rememberExternalPath(abs string) bool {
	clean := filepath.Clean(strings.TrimSpace(abs))
	if clean == "" || !filepath.IsAbs(clean) {
		return false
	}
	if _, ok := externalAllowed[clean]; ok {
		return false
	}
	if len(externalAllowed) >= maxExternalAllowed {
		evictOldestLocked()
	}
	externalAllowed[clean] = struct{}{}
	externalOrder = append(externalOrder, clean)
	return true
}

func evictOldestLocked() {
	if len(externalOrder) == 0 {
		return
	}
	oldest := externalOrder[0]
	externalOrder = externalOrder[1:]
	delete(externalAllowed, oldest)
}

func persistExternalAllowLocked() {
	if externalAllowFile == "" {
		return
	}
	data, err := json.Marshal(externalOrder)
	if err != nil {
		return
	}
	tmp := externalAllowFile + ".tmp"
	if err := os.WriteFile(tmp, data, 0o600); err != nil {
		return
	}
	_ = os.Rename(tmp, externalAllowFile)
}

func externalPathAllowed(abs string) bool {
	externalMu.Lock()
	defer externalMu.Unlock()
	_, ok := externalAllowed[filepath.Clean(abs)]
	return ok
}
