package code

import (
	"context"
	"path/filepath"
	"strings"

	"recur/internal/domain"
)

// ChangedDef is a symbol whose definition line falls in a diff hunk.
type ChangedDef struct {
	Name string
	Kind string
	Path string
	Line int
}

// ChangedDefs returns symbols whose definition line is in the changed line set
// (exact line match only — no ±3 proximity, which inflated false impact).
func ChangedDefs(root string, changedLines map[string]map[int]bool) []ChangedDef {
	var out []ChangedDef
	for rel, lines := range changedLines {
		syms, err := Outline(root, rel)
		if err != nil {
			continue
		}
		for _, s := range syms {
			if !lines[s.Line] {
				continue
			}
			out = append(out, ChangedDef{
				Name: s.Name, Kind: s.Kind, Path: rel, Line: s.Line,
			})
		}
	}
	return out
}

// FindRefsInPackage returns refs for name, preferring files under the same
// top-level package/dir as defPath. Caps results.
//
// This walks the repo for one symbol. Callers that need refs for MANY symbols
// (observe.ComputeImpact) should build a SymbolIndex once and rank it with
// RankRefsInPackage instead — otherwise they pay a full walk per symbol.
func FindRefsInPackage(ctx context.Context, root, name, defPath string, limit int) ([]domain.SymbolRef, error) {
	all, err := FindRefs(ctx, root, name)
	if err != nil {
		return nil, err
	}
	return RankRefsInPackage(all, defPath, limit), nil
}

// RankRefsInPackage drops definition sites and orders refs so those under the
// same top-level package/dir as defPath come first, then truncates to limit.
func RankRefsInPackage(all []domain.SymbolRef, defPath string, limit int) []domain.SymbolRef {
	if limit <= 0 {
		limit = 8
	}
	pkg := topPkg(defPath)
	var near, far []domain.SymbolRef
	for _, r := range all {
		if r.Def {
			continue
		}
		if pkg != "" && topPkg(r.Path) == pkg {
			near = append(near, r)
		} else {
			far = append(far, r)
		}
	}
	out := append(near, far...)
	if len(out) > limit {
		out = out[:limit]
	}
	return out
}

func topPkg(path string) string {
	path = filepath.ToSlash(path)
	if i := strings.IndexByte(path, '/'); i >= 0 {
		return path[:i]
	}
	return path
}
