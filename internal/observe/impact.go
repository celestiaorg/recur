package observe

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"recur/internal/code"
	"recur/internal/domain"
)

const (
	// impactRefsPerSymbol is how many reference sites we report per changed symbol.
	impactRefsPerSymbol = 6
	// impactRefScan is how many reference sites we collect per symbol before
	// ranking them by package proximity. Larger than impactRefsPerSymbol so the
	// same-package preference has candidates to choose from, but bounded so a
	// diff touching a very common name can't blow up memory.
	impactRefScan = 60
)

// ComputeImpact builds a trustworthy blast-radius list from a diff:
// only symbols whose definition line is in the change set, then package-preferring
// refs (capped), plus co-located tests.
//
// The reference lookup indexes the repo ONCE for every changed symbol at the
// same time. Previously this called FindRefsInPackage per symbol, and each of
// those walked the whole tree twice — a 40-symbol diff meant 80 full-repo walks
// inside a single synchronous binding call.
func ComputeImpact(ctx context.Context, root string, files []domain.FileDiff) []domain.ImpactItem {
	changedLines := map[string]map[int]bool{}
	var changedPaths []string
	for _, f := range files {
		path := filepathToSlash(f.Path)
		changedPaths = append(changedPaths, path)
		lines := map[int]bool{}
		for _, l := range f.Lines {
			// Definitions appear on add lines; ignore pure context for "touched def".
			if l.Type == "add" {
				if n := parseLine(l.New); n > 0 {
					lines[n] = true
				}
			}
		}
		changedLines[path] = lines
	}

	defs := code.ChangedDefs(root, changedLines)
	var impact []domain.ImpactItem
	seen := map[string]bool{}

	// One repo walk for every changed symbol, before the reporting loop.
	var names []string
	for _, d := range defs {
		if len(d.Name) < 3 || isNoiseIdent(d.Name) {
			continue
		}
		names = append(names, d.Name)
	}
	refIndex := &code.SymbolIndex{}
	if len(names) > 0 {
		if idx, err := code.IndexSymbols(ctx, root, names, impactRefScan); err == nil {
			refIndex = idx
		}
	}

	for _, d := range defs {
		// Skip tiny / noisy names that explode false refs.
		if len(d.Name) < 3 || isNoiseIdent(d.Name) {
			continue
		}
		key := "sym:" + d.Path + ":" + d.Name
		if seen[key] {
			continue
		}
		seen[key] = true
		impact = append(impact, domain.ImpactItem{
			Kind:    "symbol",
			Name:    d.Name,
			Path:    d.Path,
			Line:    d.Line,
			Detail:  fmt.Sprintf("%s definition in diff", d.Kind),
			Changed: true,
		})
		refs := code.RankRefsInPackage(refIndex.Refs[d.Name], d.Path, impactRefsPerSymbol)
		for _, r := range refs {
			rkey := "ref:" + r.Path + ":" + d.Name
			if seen[rkey] {
				continue
			}
			seen[rkey] = true
			impact = append(impact, domain.ImpactItem{
				Kind:   "api",
				Name:   d.Name,
				Path:   r.Path,
				Line:   r.Line,
				Detail: "references changed " + d.Kind,
			})
		}
	}

	for _, p := range changedPaths {
		base := filepath.Base(p)
		stem := strings.TrimSuffix(base, filepath.Ext(base))
		stem = strings.TrimSuffix(stem, "_test")
		stem = strings.TrimSuffix(stem, ".test")
		stem = strings.TrimSuffix(stem, ".spec")
		baseNoExt := strings.TrimSuffix(p, filepath.Ext(p))
		dir := filepath.Dir(p)
		candidates := []string{
			baseNoExt + "_test.go",
			baseNoExt + "_test.py",
			baseNoExt + "_test.rs",
			baseNoExt + ".test.ts",
			baseNoExt + ".test.tsx",
			baseNoExt + ".test.js",
			baseNoExt + ".spec.ts",
			baseNoExt + ".spec.tsx",
			baseNoExt + ".spec.js",
			baseNoExt + "_spec.rb",
			baseNoExt + "Test.java",
			baseNoExt + "Tests.java",
			baseNoExt + "Test.kt",
			baseNoExt + "Tests.swift",
			dir + "/" + stem + "_test.go",
			dir + "/test_" + stem + ".py",
			dir + "/" + stem + "_test.py",
		}
		if stem != "" {
			candidates = append(candidates, dir+"/"+strings.ToUpper(stem[:1])+stem[1:]+"Test.java")
		}
		for _, c := range candidates {
			c = filepathToSlash(c)
			if c == p || seen["test:"+c] {
				continue
			}
			full, err := code.SafeJoin(root, c)
			if err != nil {
				continue
			}
			if _, err := os.Stat(full); err != nil {
				continue
			}
			seen["test:"+c] = true
			impact = append(impact, domain.ImpactItem{
				Kind:   "test",
				Name:   filepath.Base(c),
				Path:   c,
				Detail: "likely covers " + p,
			})
		}
	}

	if len(impact) > 36 {
		impact = impact[:36]
	}
	return impact
}

func isNoiseIdent(name string) bool {
	switch strings.ToLower(name) {
	case "new", "get", "set", "update", "delete", "create", "init", "main",
		"error", "err", "ok", "data", "value", "item", "type", "name", "id",
		"run", "start", "stop", "close", "open", "read", "write", "string",
		"int", "bool", "handle", "handler", "config", "options", "result":
		return true
	}
	return false
}
