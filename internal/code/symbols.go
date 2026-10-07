package code

import (
	"bufio"
	"context"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"unicode"

	"recur/internal/domain"
)

var identRE = regexp.MustCompile(`\b([A-Z][A-Za-z0-9_]{2,}|[a-z][A-Za-z0-9_]{3,})\b`)

const (
	// maxSymbolDefs bounds FindDefs. A definition walk parses every source file
	// in the repo; without a cap a common name in a monorepo builds an
	// arbitrarily large result nobody scrolls through.
	maxSymbolDefs = 200
	// maxRefsPerName bounds the reference sites collected for one symbol.
	maxRefsPerName = 120
)

// FindDefs returns definition sites matching name (exact) under root, capped at
// maxSymbolDefs. It parses each file's lines directly (rather than building a
// full Outline per file) so nothing is allocated for the symbols we discard.
func FindDefs(ctx context.Context, root, name string) ([]domain.SymbolDef, error) {
	name = strings.TrimSpace(name)
	if name == "" {
		return nil, nil
	}
	var defs []domain.SymbolDef
	err := walkSource(ctx, root, func(rel, full string) error {
		ext := strings.ToLower(filepathExt(rel))
		return scanLines(full, func(lineNo int, line string) error {
			sym, ok := outlineLine(ext, line)
			if !ok || sym.name != name {
				return nil
			}
			defs = append(defs, domain.SymbolDef{
				Name: sym.name, Kind: sym.kind, Path: rel, Line: lineNo,
			})
			if len(defs) >= maxSymbolDefs {
				return filepath.SkipAll
			}
			return nil
		})
	})
	if err == filepath.SkipAll {
		err = nil
	}
	return defs, err
}

// SymbolIndex holds the reference sites found for a set of symbol names in one
// walk of the repo.
type SymbolIndex struct {
	// Refs maps a symbol name to its reference sites, definitions included
	// (marked Def). Order is walk order, i.e. stable for a stable tree.
	Refs map[string][]domain.SymbolRef
}

// IndexSymbols walks root ONCE and collects reference sites for every name in
// names, marking the sites that are also definitions.
//
// This exists because the caller that matters — observe.ComputeImpact — asks
// about every symbol touched by a diff. Doing that one name at a time meant
// FindRefs per symbol, and FindRefs itself walked the tree twice (once to find
// definitions, once to find references): a 40-symbol diff was 80 full-repo
// walks. Here every file is read once and each line is checked for definitions
// and for all the names at the same time, so the cost is one walk regardless of
// how many symbols the diff touched.
func IndexSymbols(ctx context.Context, root string, names []string, perName int) (*SymbolIndex, error) {
	if perName <= 0 {
		perName = maxRefsPerName
	}
	idx := &SymbolIndex{Refs: map[string][]domain.SymbolRef{}}
	// Dedupe and drop anything that isn't a plain identifier — the alternation
	// below is built from these, and a non-identifier can't be word-matched.
	var wanted []string
	seen := map[string]bool{}
	for _, n := range names {
		n = strings.TrimSpace(n)
		if n == "" || seen[n] || !isIdent(n) {
			continue
		}
		seen[n] = true
		wanted = append(wanted, n)
	}
	if len(wanted) == 0 {
		return idx, nil
	}

	// One alternation over all names: a single regex pass per line beats N
	// per-name passes, and the match text tells us which name(s) hit.
	quoted := make([]string, len(wanted))
	for i, n := range wanted {
		quoted[i] = regexp.QuoteMeta(n)
	}
	word, err := regexp.Compile(`\b(?:` + strings.Join(quoted, "|") + `)\b`)
	if err != nil {
		return idx, err
	}

	saturated := 0
	err = walkSource(ctx, root, func(rel, full string) error {
		ext := strings.ToLower(filepathExt(rel))
		return scanLines(full, func(lineNo int, line string) error {
			hits := word.FindAllString(line, -1)
			if hits == nil {
				return nil
			}
			// A definition on this line makes the matching reference a def site.
			def, isDef := outlineLine(ext, line)
			text := strings.TrimSpace(line)
			for _, name := range dedupeStrings(hits) {
				refs := idx.Refs[name]
				if len(refs) >= perName {
					continue
				}
				idx.Refs[name] = append(refs, domain.SymbolRef{
					Path: rel,
					Line: lineNo,
					Text: text,
					Def:  isDef && def.name == name,
				})
				if len(idx.Refs[name]) == perName {
					saturated++
				}
			}
			if saturated >= len(wanted) {
				return filepath.SkipAll
			}
			return nil
		})
	})
	if err == filepath.SkipAll {
		err = nil
	}
	return idx, err
}

// FindRefs returns definition and reference sites for name under root.
func FindRefs(ctx context.Context, root, name string) ([]domain.SymbolRef, error) {
	idx, err := IndexSymbols(ctx, root, []string{name}, maxRefsPerName)
	if err != nil {
		return nil, err
	}
	return idx.Refs[strings.TrimSpace(name)], nil
}

// scanLines feeds each line of a file to fn. Returning filepath.SkipAll from fn
// stops the enclosing walk (it is propagated, not swallowed); unreadable files
// are skipped silently, as everywhere else in these best-effort walks.
func scanLines(full string, fn func(lineNo int, line string) error) error {
	f, err := os.Open(full)
	if err != nil {
		return nil
	}
	defer f.Close() //nolint:errcheck
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 64*1024), 1024*1024)
	lineNo := 0
	for sc.Scan() {
		lineNo++
		if err := fn(lineNo, sc.Text()); err != nil {
			return err
		}
	}
	return nil
}

func dedupeStrings(in []string) []string {
	if len(in) < 2 {
		return in
	}
	out := in[:0:0]
	seen := make(map[string]bool, len(in))
	for _, s := range in {
		if seen[s] {
			continue
		}
		seen[s] = true
		out = append(out, s)
	}
	return out
}

// ChangedSymbols extracts outline symbols whose lines fall in changed line sets.
// changedLines maps path → set of new-side line numbers (from a diff).
// Returned symbols use End as a path-index sentinel is unused; callers recover
// path via the changedLines keys (see observe.ComputeImpact).
func ChangedSymbols(root string, changedLines map[string]map[int]bool) ([]domain.OutlineSymbol, error) {
	var out []domain.OutlineSymbol
	for rel, lines := range changedLines {
		syms, err := Outline(root, rel)
		if err != nil {
			continue
		}
		for _, s := range syms {
			include := lines[s.Line]
			if !include {
				for ln := range lines {
					if abs(ln-s.Line) <= 3 {
						include = true
						break
					}
				}
			}
			if include {
				// Stash path in unused End field? No — keep clean; impact maps path.
				_ = rel
				out = append(out, s)
			}
		}
	}
	return out, nil
}

func walkSource(ctx context.Context, root string, fn func(rel, abs string) error) error {
	return filepath.WalkDir(root, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if cerr := ctx.Err(); cerr != nil {
			return cerr
		}
		if d.IsDir() {
			if skipDir(d.Name()) {
				return filepath.SkipDir
			}
			return nil
		}
		if !IsSourceFile(d.Name()) {
			return nil
		}
		rel := RelSlash(root, path)
		return fn(rel, path)
	})
}

func isIdent(s string) bool {
	if s == "" {
		return false
	}
	for i, r := range s {
		if i == 0 && !unicode.IsLetter(r) && r != '_' {
			return false
		}
		if !unicode.IsLetter(r) && !unicode.IsDigit(r) && r != '_' {
			return false
		}
	}
	return true
}

func abs(n int) int {
	if n < 0 {
		return -n
	}
	return n
}

// ExtractIdents pulls identifier-like tokens from a line of source (for impact).
func ExtractIdents(line string) []string {
	ms := identRE.FindAllString(line, -1)
	seen := map[string]bool{}
	var out []string
	for _, m := range ms {
		if seen[m] || len(m) < 3 {
			continue
		}
		seen[m] = true
		out = append(out, m)
	}
	return out
}
