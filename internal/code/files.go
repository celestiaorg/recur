package code

import (
	"context"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

const maxListFiles = 20000

// ListFiles returns repo-relative forward-slash paths for all files under root
// (for Quick Open). Skips heavy/vendor dirs and common binary/noise files.
// Cancelling ctx aborts the walk (Quick Open closes, the walk stops).
func ListFiles(ctx context.Context, root string) ([]string, error) {
	var out []string
	err := filepath.WalkDir(root, func(path string, d os.DirEntry, err error) error {
		if err != nil || len(out) >= maxListFiles {
			if len(out) >= maxListFiles {
				return filepath.SkipAll
			}
			return nil
		}
		if cerr := ctx.Err(); cerr != nil {
			return cerr
		}
		if d.IsDir() {
			if path != root && skipDir(d.Name()) {
				return filepath.SkipDir
			}
			return nil
		}
		if skipFile(d.Name()) {
			return nil
		}
		out = append(out, RelSlash(root, path))
		return nil
	})
	if err == filepath.SkipAll {
		err = nil
	}
	if err != nil {
		return nil, err
	}
	sort.Strings(out)
	return out, nil
}

// MatchFiles filters paths by a simple fuzzy query (subsequence / substring).
// Used by tests; the UI may fuzzy-match client-side.
func MatchFiles(paths []string, query string, limit int) []string {
	q := strings.TrimSpace(strings.ToLower(query))
	if limit <= 0 {
		limit = 50
	}
	if q == "" {
		if len(paths) > limit {
			return paths[:limit]
		}
		return paths
	}
	var out []string
	for _, p := range paths {
		if fuzzyMatch(strings.ToLower(p), q) {
			out = append(out, p)
			if len(out) >= limit {
				break
			}
		}
	}
	return out
}

func fuzzyMatch(path, query string) bool {
	if strings.Contains(path, query) {
		return true
	}
	// subsequence match on basename-first feel: whole path
	qi := 0
	for i := 0; i < len(path) && qi < len(query); i++ {
		if path[i] == query[qi] {
			qi++
		}
	}
	return qi == len(query)
}
