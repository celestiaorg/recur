package code

import (
	"bufio"
	"context"
	"fmt"
	"os"
	"path"
	"path/filepath"
	"regexp"
	"strings"
	"unicode/utf8"

	"recur/internal/domain"
)

const defaultSearchHits = 200
const maxSearchHitsCap = 2000
const maxSearchFileBytes = 256 * 1024

// Search walks root for case-insensitive substring matches of query.
func Search(ctx context.Context, root, query string) ([]domain.SearchHit, error) {
	res, err := SearchOpts(ctx, root, query, domain.SearchOpts{})
	return res.Hits, err
}

// SearchOpts walks root with regex/glob/pagination options and honours ctx
// cancellation between files — a pathological regex over a monorepo, or a user
// who navigated away, must not pin a core until the walk finishes.
//
// Pagination note: Offset is implemented by walking from the top and discarding
// the first Offset matches, so "load more" costs a full re-walk and page N costs
// roughly N× page 1. Resuming mid-walk would need per-query server state; the
// cheaper mitigation is a bigger first page (Limit, up to maxSearchHitsCap).
func SearchOpts(ctx context.Context, root, query string, opts domain.SearchOpts) (domain.SearchResult, error) {
	var out domain.SearchResult
	q := strings.TrimSpace(query)
	if q == "" {
		return out, nil
	}
	limit := opts.Limit
	if limit <= 0 {
		limit = defaultSearchHits
	}
	if limit > maxSearchHitsCap {
		limit = maxSearchHitsCap
	}
	offset := opts.Offset
	if offset < 0 {
		offset = 0
	}

	var re *regexp.Regexp
	var needle string
	if opts.Regex {
		flags := ""
		if !opts.CaseSensitive {
			flags = "(?i)"
		}
		compiled, err := regexp.Compile(flags + q)
		if err != nil {
			return out, fmt.Errorf("invalid regex: %w", err)
		}
		re = compiled
	} else if opts.CaseSensitive {
		needle = q
	} else {
		needle = strings.ToLower(q)
	}

	glob := strings.TrimSpace(opts.Glob)
	var hits []domain.SearchHit
	skipped := 0
	err := filepath.WalkDir(root, func(full string, d os.DirEntry, err error) error {
		if err != nil || len(hits) >= limit {
			if len(hits) >= limit {
				return filepath.SkipAll
			}
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
		if skipFile(d.Name()) {
			return nil
		}
		if d.Type()&os.ModeSymlink != 0 {
			resolved, rerr := filepath.EvalSymlinks(full)
			if rerr != nil {
				return nil
			}
			relResolved, rerr := filepath.Rel(root, resolved)
			if rerr != nil || relResolved == ".." || strings.HasPrefix(relResolved, ".."+string(filepath.Separator)) {
				return nil
			}
		}
		rel := RelSlash(root, full)
		if glob != "" && !matchSimpleGlob(glob, rel) {
			return nil
		}
		st, err := d.Info()
		if err != nil {
			return nil
		}
		if st.Size() > maxSearchFileBytes {
			// Reported back as SkippedLarge so the UI can say "we didn't look
			// in N files" instead of implying there were no matches.
			out.SkippedLarge++
			return nil
		}
		f, err := os.Open(full)
		if err != nil {
			return nil
		}
		defer f.Close()
		sc := bufio.NewScanner(f)
		sc.Buffer(make([]byte, 64*1024), 1024*1024)
		lineNo := 0
		for sc.Scan() {
			lineNo++
			line := sc.Text()
			if !utf8.ValidString(line) {
				continue
			}
			col, ok := matchLine(line, needle, re, opts)
			if !ok {
				continue
			}
			if skipped < offset {
				skipped++
				continue
			}
			hits = append(hits, domain.SearchHit{
				Path:    rel,
				Line:    lineNo,
				Col:     col,
				Text:    q,
				Preview: strings.TrimSpace(line),
			})
			if len(hits) >= limit {
				return filepath.SkipAll
			}
		}
		return nil
	})
	if err == filepath.SkipAll {
		err = nil
	}
	if err != nil {
		return domain.SearchResult{}, err
	}
	out.Hits = hits
	out.Truncated = len(hits) >= limit
	return out, nil
}

func matchLine(line, needle string, re *regexp.Regexp, opts domain.SearchOpts) (col int, ok bool) {
	if re != nil {
		loc := re.FindStringIndex(line)
		if loc == nil {
			return 0, false
		}
		return loc[0] + 1, true
	}
	hay := line
	if !opts.CaseSensitive {
		hay = strings.ToLower(line)
	}
	idx := strings.Index(hay, needle)
	if idx < 0 {
		return 0, false
	}
	return idx + 1, true
}

// matchSimpleGlob supports * and ? only. The pattern is tried against the whole
// repo-relative path first; a pattern with no separator (e.g. "*.go", which
// filepath.Match will not match across "/") also gets tried against the
// basename, so callers pass the full path once and get both behaviours.
func matchSimpleGlob(pattern, name string) bool {
	pattern = filepath.ToSlash(pattern)
	name = filepath.ToSlash(name)
	if ok, err := filepath.Match(pattern, name); err == nil && ok {
		return true
	}
	if strings.Contains(pattern, "/") {
		return false
	}
	ok, err := filepath.Match(pattern, path.Base(name))
	return err == nil && ok
}

// ReplaceInFiles replaces matches under root (worktree). Returns counts.
// Regex/case/glob follow SearchOpts; Limit/Offset are ignored (all matches
// replaced). Cancelling ctx stops the walk; files already rewritten stay
// rewritten (there is no transaction over the filesystem) and the partial
// counts are returned with the error.
func ReplaceInFiles(ctx context.Context, root, query, replacement string, opts domain.SearchOpts) (domain.ReplaceResult, error) {
	q := strings.TrimSpace(query)
	if q == "" {
		return domain.ReplaceResult{}, fmt.Errorf("empty search")
	}
	var re *regexp.Regexp
	if opts.Regex {
		flags := ""
		if !opts.CaseSensitive {
			flags = "(?i)"
		}
		compiled, err := regexp.Compile(flags + q)
		if err != nil {
			return domain.ReplaceResult{}, fmt.Errorf("invalid regex: %w", err)
		}
		re = compiled
	}

	glob := strings.TrimSpace(opts.Glob)
	var result domain.ReplaceResult
	err := filepath.WalkDir(root, func(full string, d os.DirEntry, err error) error {
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
		if skipFile(d.Name()) {
			return nil
		}
		rel := RelSlash(root, full)
		if glob != "" && !matchSimpleGlob(glob, rel) {
			return nil
		}
		st, err := d.Info()
		if err != nil || st.Size() > maxSearchFileBytes {
			return nil
		}
		data, err := os.ReadFile(full)
		if err != nil || !utf8.Valid(data) {
			return nil
		}
		src := string(data)
		var next string
		var n int
		if re != nil {
			idxs := re.FindAllStringIndex(src, -1)
			n = len(idxs)
			if n == 0 {
				return nil
			}
			next = re.ReplaceAllString(src, replacement)
		} else if opts.CaseSensitive {
			n = strings.Count(src, q)
			if n == 0 {
				return nil
			}
			next = strings.ReplaceAll(src, q, replacement)
		} else {
			// case-insensitive replace preserving simple substitution
			next, n = replaceFold(src, q, replacement)
			if n == 0 {
				return nil
			}
		}
		if next == src {
			return nil
		}
		if err := os.WriteFile(full, []byte(next), st.Mode().Perm()); err != nil {
			return nil
		}
		result.FilesChanged++
		result.Replacements += n
		return nil
	})
	return result, err
}

func replaceFold(src, query, replacement string) (string, int) {
	if query == "" {
		return src, 0
	}
	lower := strings.ToLower(src)
	ql := strings.ToLower(query)
	var b strings.Builder
	b.Grow(len(src))
	n := 0
	i := 0
	for {
		idx := strings.Index(lower[i:], ql)
		if idx < 0 {
			b.WriteString(src[i:])
			break
		}
		idx += i
		b.WriteString(src[i:idx])
		b.WriteString(replacement)
		n++
		i = idx + len(query)
	}
	return b.String(), n
}
