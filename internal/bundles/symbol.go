package bundles

import (
	"fmt"
	"strings"
)

// symbolTOML is the on-disk [[symbol]] shape. Show is a pointer so omitted
// means include (TextMate showInSymbolList default 1).
type symbolTOML struct {
	ID        string `toml:"id"`
	Scope     string `toml:"scope"`
	Transform string `toml:"transform"`
	Kind      string `toml:"kind"`
	Show      *bool  `toml:"show"`
}

// Symbol is one [[symbol]] entry — a TextMate showInSymbolList preference.
// Scope is a token-scope selector (not a file grammar). Show=false excludes
// a more-specific scope from a broader include.
type Symbol struct {
	ID        string `toml:"-" json:"id"`
	Scope     string `toml:"-" json:"scope"`
	Transform string `toml:"-" json:"transform,omitempty"`
	Kind      string `toml:"-" json:"kind,omitempty"`
	Show      bool   `toml:"-" json:"show"`

	BundleName string `toml:"-" json:"bundleName"`
	BundleID   string `toml:"-" json:"bundleID"`
	SourceRoot string `toml:"-" json:"sourceRoot"`
	BundleDir  string `toml:"-" json:"bundleDir"`
}

func parseSymbols(bundleDir string, meta BundleMeta, raw []symbolTOML) ([]Symbol, error) {
	out := make([]Symbol, 0, len(raw))
	seenID := map[string]struct{}{}
	for i, r := range raw {
		id := strings.TrimSpace(r.ID)
		if id == "" {
			return nil, fmt.Errorf("symbol[%d]: id is required", i)
		}
		if !validBundleID(id) {
			return nil, fmt.Errorf("symbol[%d]: id is not a safe id", i)
		}
		if _, dup := seenID[id]; dup {
			return nil, fmt.Errorf("symbol: duplicate id %q in bundle", id)
		}
		seenID[id] = struct{}{}

		scope := strings.TrimSpace(r.Scope)
		if scope == "" {
			return nil, fmt.Errorf("symbol %q: scope is required", id)
		}

		show := true
		if r.Show != nil {
			show = *r.Show
		}

		out = append(out, Symbol{
			ID:         id,
			Scope:      scope,
			Transform:  strings.TrimSpace(r.Transform),
			Kind:       strings.TrimSpace(r.Kind),
			Show:       show,
			BundleName: meta.Name,
			BundleID:   meta.ID,
			BundleDir:  bundleDir,
		})
	}
	return out, nil
}

// FindSymbolByID returns the symbol rule with the given id.
func FindSymbolByID(all []Symbol, id string) (Symbol, bool) {
	id = strings.TrimSpace(id)
	for _, s := range all {
		if s.ID == id {
			return s, true
		}
	}
	return Symbol{}, false
}
