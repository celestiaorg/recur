package bundles

import (
	"fmt"
	"strings"
	"unicode"
)

// tabTriggerRE: letter/underscore then word chars (ife, forof, py3, ifmain).
func validTabTrigger(s string) bool {
	if s == "" || len(s) > 40 {
		return false
	}
	for i, r := range s {
		if i == 0 {
			if !unicode.IsLetter(r) && r != '_' {
				return false
			}
			continue
		}
		if unicode.IsLetter(r) || unicode.IsDigit(r) || r == '_' {
			continue
		}
		return false
	}
	return true
}

// Snippet is one [[snippet]] entry — a tab-trigger (and/or keyEquivalent)
// snippet body in VS Code placeholder syntax ($0 / $1 / ${1:default}).
type Snippet struct {
	ID            string `toml:"id" json:"id"`
	Name          string `toml:"name" json:"name"`
	TabTrigger    string `toml:"tabTrigger" json:"tabTrigger"`
	Scope         string `toml:"scope" json:"scope"`
	FileGlob      string `toml:"fileGlob" json:"fileGlob"`
	Body          string `toml:"body" json:"body"`
	KeyEquivalent string `toml:"keyEquivalent" json:"keyEquivalent"`
	Order         int    `toml:"order" json:"order"`

	// Filled by Discover — not part of the TOML schema.
	BundleName string `toml:"-" json:"bundleName"`
	BundleID   string `toml:"-" json:"bundleID"`
	SourceRoot string `toml:"-" json:"sourceRoot"`
	BundleDir  string `toml:"-" json:"bundleDir"`
}

func parseSnippets(bundleDir string, meta BundleMeta, raw []Snippet) ([]Snippet, error) {
	out := make([]Snippet, 0, len(raw))
	seenID := map[string]struct{}{}
	for i, s := range raw {
		id := strings.TrimSpace(s.ID)
		if id == "" {
			return nil, fmt.Errorf("snippet[%d]: id is required", i)
		}
		if !validBundleID(id) {
			return nil, fmt.Errorf("snippet[%d]: id is not a safe id", i)
		}
		if _, dup := seenID[id]; dup {
			return nil, fmt.Errorf("snippet: duplicate id %q in bundle", id)
		}
		seenID[id] = struct{}{}
		s.ID = id

		name := strings.TrimSpace(s.Name)
		if name == "" {
			return nil, fmt.Errorf("snippet %q: name is required", id)
		}
		s.Name = name

		s.TabTrigger = strings.TrimSpace(s.TabTrigger)
		s.KeyEquivalent = strings.TrimSpace(s.KeyEquivalent)
		if s.TabTrigger == "" && s.KeyEquivalent == "" {
			return nil, fmt.Errorf("snippet %q: set tabTrigger and/or keyEquivalent", id)
		}
		if s.TabTrigger != "" && !validTabTrigger(s.TabTrigger) {
			return nil, fmt.Errorf("snippet %q: tabTrigger %q must be a word (letter, then letters/digits/_)", id, s.TabTrigger)
		}

		s.Scope = strings.TrimSpace(s.Scope)
		s.FileGlob = strings.TrimSpace(s.FileGlob)
		s.Body = strings.TrimSpace(s.Body)
		if s.Body == "" {
			return nil, fmt.Errorf("snippet %q: body is required", id)
		}

		s.BundleName = meta.Name
		s.BundleID = meta.ID
		s.BundleDir = bundleDir
		out = append(out, s)
	}
	return out, nil
}

// FindSnippetByID returns the snippet with the given id.
func FindSnippetByID(all []Snippet, id string) (Snippet, bool) {
	id = strings.TrimSpace(id)
	for _, s := range all {
		if s.ID == id {
			return s, true
		}
	}
	return Snippet{}, false
}

// SnippetMatches reports whether snip applies to the editor scope/path.
func SnippetMatches(s Snippet, fileScope, filePath string) bool {
	return Matches(Command{Scope: s.Scope, FileGlob: s.FileGlob}, fileScope, filePath)
}
