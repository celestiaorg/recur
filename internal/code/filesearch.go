package code

import (
	"context"
	"path/filepath"
	"regexp"
	"strings"
	"sync"
	"unicode"

	"recur/internal/domain"
)

// RankFiles scores paths for query and returns up to limit hits (best first).
// Empty query returns the first limit paths with score 0.
// The corpus is already files.exclude-filtered at index build time — no
// per-query exclude pass. Scoring mirrors the former frontend ranking.
// ctx may cancel a long scan (short queries walk the whole corpus); nil is fine.
func RankFiles(ctx context.Context, paths, basenames []string, query string, limit int) []domain.FileSearchHit {
	if limit <= 0 {
		limit = 50
	}
	if ctx == nil {
		ctx = context.Background()
	}
	q := strings.TrimSpace(strings.ToLower(query))
	if q == "" {
		if len(paths) > limit {
			paths = paths[:limit]
		}
		out := make([]domain.FileSearchHit, len(paths))
		for i, p := range paths {
			out[i] = domain.FileSearchHit{Path: p, Score: 0}
		}
		return out
	}

	// Keep a bounded best-N list (insertion by score) instead of collecting every
	// subsequence match then sorting — short queries can hit most of a monorepo.
	out := make([]domain.FileSearchHit, 0, limit)
	for i, p := range paths {
		if i&511 == 0 && ctx.Err() != nil {
			return out
		}
		base := p
		if i < len(basenames) && basenames[i] != "" {
			base = basenames[i]
		} else {
			base = filepath.Base(p)
		}
		pl := strings.ToLower(p)
		bl := strings.ToLower(base)
		baseScore := fuzzyScorePathLower(pl, q)
		if baseScore < 0 {
			continue
		}
		score := scoreFileLower(pl, bl, base, q, baseScore)
		if len(out) == limit {
			worst := out[limit-1]
			if score < worst.Score || (score == worst.Score && p >= worst.Path) {
				continue
			}
		}
		// Insert score-desc, path-asc.
		j := len(out)
		for j > 0 {
			prev := out[j-1]
			if prev.Score > score || (prev.Score == score && prev.Path < p) {
				break
			}
			j--
		}
		if len(out) < limit {
			out = append(out, domain.FileSearchHit{})
		}
		copy(out[j+1:], out[j:])
		out[j] = domain.FileSearchHit{Path: p, Score: score}
		if len(out) > limit {
			out = out[:limit]
		}
	}
	return out
}

func scoreFileLower(pl, bl, basename, q string, baseScore int) int {
	score := baseScore
	switch {
	case bl == q:
		score += 5000
	case strings.HasPrefix(bl, q):
		score += 2000
	case strings.Contains(bl, q):
		score += 1200
	case camelHumpMatch(basename, q):
		score += 900
	case wordBoundaryMatch(bl, q):
		score += 700
	}
	if strings.Contains(pl, "/"+q) {
		score += 200
	}
	bonus := 80 - len(basename)
	if bonus > 0 {
		score += bonus
	}
	return score
}

// uniqueParentDirs returns repo-relative directories implied by file paths
// (`src/lib/a.ts` → `src/lib`, `src`). Empty dirs with no files are omitted.
func uniqueParentDirs(paths []string) []string {
	seen := make(map[string]struct{}, len(paths)/4)
	out := make([]string, 0, 32)
	for _, p := range paths {
		for {
			i := strings.LastIndexByte(p, '/')
			if i <= 0 {
				break
			}
			p = p[:i]
			if _, ok := seen[p]; ok {
				break
			}
			seen[p] = struct{}{}
			out = append(out, p)
		}
	}
	return out
}

func dirBasenames(dirs []string) []string {
	out := make([]string, len(dirs))
	for i, d := range dirs {
		out[i] = filepath.Base(d)
	}
	return out
}

// RankHits is RankFiles when filesOnly, otherwise RankFilesAndDirs.
func RankHits(ctx context.Context, paths, basenames []string, query string, limit int, filesOnly bool) []domain.FileSearchHit {
	if filesOnly {
		return RankFiles(ctx, paths, basenames, query, limit)
	}
	return RankFilesAndDirs(ctx, paths, basenames, query, limit)
}

// RankFilesAndDirs ranks files then parent directories and merges by score.
func RankFilesAndDirs(ctx context.Context, paths, basenames []string, query string, limit int) []domain.FileSearchHit {
	files := RankFiles(ctx, paths, basenames, query, limit)
	dirs := uniqueParentDirs(paths)
	if len(dirs) == 0 {
		return files
	}
	dirHits := RankFiles(ctx, dirs, dirBasenames(dirs), query, limit)
	return mergeFileDirHits(files, dirHits, limit)
}

func mergeFileDirHits(files, dirs []domain.FileSearchHit, limit int) []domain.FileSearchHit {
	if limit <= 0 {
		limit = 50
	}
	for i := range dirs {
		dirs[i].Dir = true
		p := dirs[i].Path
		if p != "" && !strings.HasSuffix(p, "/") {
			dirs[i].Path = p + "/"
		}
	}
	if len(dirs) == 0 {
		return files
	}
	if len(files) == 0 {
		if len(dirs) > limit {
			return dirs[:limit]
		}
		return dirs
	}
	out := make([]domain.FileSearchHit, 0, limit)
	i, j := 0, 0
	for len(out) < limit && (i < len(files) || j < len(dirs)) {
		switch {
		case i >= len(files):
			out = append(out, dirs[j])
			j++
		case j >= len(dirs):
			out = append(out, files[i])
			i++
		case dirs[j].Score > files[i].Score || (dirs[j].Score == files[i].Score && dirs[j].Path < files[i].Path):
			out = append(out, dirs[j])
			j++
		default:
			out = append(out, files[i])
			i++
		}
	}
	return out
}

// fuzzyScorePathLower ports frontend scoreLower against an already-lowercased
// path. -1 = no match.
func fuzzyScorePathLower(t, q string) int {
	if q == "" {
		return 0
	}
	if t == q {
		return 10_000
	}
	if idx := strings.Index(t, q); idx >= 0 {
		slash := strings.LastIndex(t, "/")
		base := t
		if slash >= 0 {
			base = t[slash+1:]
		}
		baseBonus := 0
		if strings.HasPrefix(base, q) {
			baseBonus = 500
		} else if strings.Contains(base, q) {
			baseBonus = 200
		}
		lenBonus := 100 - len(t)
		if lenBonus < 0 {
			lenBonus = 0
		}
		return 1000 - idx + baseBonus + lenBonus
	}
	qi := 0
	score := 0
	prev := -2
	for i := 0; i < len(t) && qi < len(q); i++ {
		if t[i] == q[qi] {
			if i == prev+1 {
				score += 8
			} else {
				score += 3
			}
			if i == 0 || t[i-1] == '/' || t[i-1] == '_' || t[i-1] == '-' {
				score += 12
			}
			prev = i
			qi++
		}
	}
	if qi < len(q) {
		return -1
	}
	return score - len(t)/4
}

func camelHumpMatch(text, q string) bool {
	var humps []string
	var cur strings.Builder
	flush := func() {
		if cur.Len() > 0 {
			humps = append(humps, cur.String())
			cur.Reset()
		}
	}
	runes := []rune(text)
	for i, r := range runes {
		if i > 0 && (unicode.IsUpper(r) || ((r == '_' || r == '-' || r == '.') && cur.Len() > 0)) {
			flush()
			if unicode.IsLetter(r) || unicode.IsDigit(r) {
				cur.WriteRune(r)
			}
			continue
		}
		if unicode.IsLetter(r) || unicode.IsDigit(r) {
			cur.WriteRune(r)
		}
	}
	flush()
	if len(humps) == 0 {
		return false
	}
	var initials strings.Builder
	for _, h := range humps {
		for _, r := range h {
			initials.WriteRune(unicode.ToLower(r))
			break
		}
	}
	if strings.HasPrefix(initials.String(), q) {
		return true
	}
	qi := 0
	for _, h := range humps {
		hl := strings.ToLower(h)
		for i := 0; i < len(hl) && qi < len(q); i++ {
			if hl[i] == q[qi] {
				qi++
			} else if i > 0 {
				break
			}
		}
		if qi >= len(q) {
			return true
		}
	}
	return qi >= len(q)
}

func wordBoundaryMatch(lowerText, q string) bool {
	if q == "" {
		return false
	}
	if strings.HasPrefix(lowerText, q) {
		return true
	}
	for i := 0; i < len(lowerText)-len(q); i++ {
		c := lowerText[i]
		if c == '/' || c == '_' || c == '-' || c == '.' {
			if strings.HasPrefix(lowerText[i+1:], q) {
				return true
			}
		}
	}
	return false
}

// ---- exclude globs (VS Code–style * / **, ported from frontend/src/lib/glob.ts) ----
// Used when building the FileIndex (rg --glob + WalkDir/FS splice post-filter),
// not on the SearchFiles hot path.

type excludeMatcher func(path string) bool

type excludeSet struct {
	matchers []excludeMatcher
}

var excludeCache sync.Map // pattern → excludeMatcher

// normalizeExcludePatterns trims blanks and leading slashes; preserves order.
func normalizeExcludePatterns(patterns []string) []string {
	var out []string
	for _, pat := range patterns {
		p := strings.TrimSpace(pat)
		p = strings.TrimLeft(p, "/")
		if p == "" {
			continue
		}
		out = append(out, p)
	}
	return out
}

// excludeToRgGlobs converts VS Code files.exclude patterns to negated ripgrep
// --glob args. Always-on DefaultGlobs (!.git / !node_modules) stay in rg.FilesArgs.
// Directory-shaped patterns also get a `!pat/**` companion so rg prunes children
// the same way DefaultGlobs pairs `!.git` with `!.git/**`.
func excludeToRgGlobs(patterns []string) []string {
	var out []string
	for _, p := range normalizeExcludePatterns(patterns) {
		// Already covered by rg.DefaultGlobs — skip duplicates.
		switch strings.TrimPrefix(p, "**/") {
		case ".git", ".git/**", "node_modules", "node_modules/**":
			continue
		}
		if p == ".git" || p == "node_modules" || p == ".git/**" || p == "node_modules/**" {
			continue
		}
		neg := p
		if !strings.HasPrefix(neg, "!") {
			neg = "!" + neg
		}
		out = append(out, neg)
		if shouldEmitDescendantGlob(p) {
			out = append(out, neg+"/**")
		}
	}
	return out
}

// shouldEmitDescendantGlob reports whether pat should also exclude children
// (dir-style prune). Skip patterns that already end in /** or are file globs
// like **/*.map.
func shouldEmitDescendantGlob(pat string) bool {
	p := strings.TrimPrefix(pat, "!")
	if strings.HasSuffix(p, "/**") {
		return false
	}
	base := p
	if i := strings.LastIndex(p, "/"); i >= 0 {
		base = p[i+1:]
	}
	if strings.ContainsAny(base, "*?") {
		return false // **/*.ext etc.
	}
	return true
}

func samePatterns(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

func compileExcludes(patterns []string) excludeSet {
	var matchers []excludeMatcher
	for _, pat := range patterns {
		p := strings.TrimSpace(pat)
		if p == "" {
			continue
		}
		p = strings.TrimLeft(p, "/")
		if v, ok := excludeCache.Load(p); ok {
			matchers = append(matchers, v.(excludeMatcher))
			continue
		}
		m := compileExcludePattern(p)
		excludeCache.Store(p, m)
		matchers = append(matchers, m)
	}
	return excludeSet{matchers: matchers}
}

// compileExcludePattern prefers O(path) string ops for the common files.exclude
// shapes (**/name, name/**, **/*.ext). Regex MatchString × corpus was the
// Quick Open lag vs @-mentions (which pass no excludes).
func compileExcludePattern(p string) excludeMatcher {
	if m := fastExcludeMatcher(p); m != nil {
		return m
	}
	re := regexp.MustCompile(globToRegexp(p))
	return func(path string) bool { return re.MatchString(path) }
}

func fastExcludeMatcher(p string) excludeMatcher {
	if strings.HasPrefix(p, "**/") {
		rest := p[3:]
		if rest == "" {
			return nil
		}
		// **/foo or **/foo/bar (no wildcards) — also prune descendants so
		// WalkDir/FS splices match rg's directory-glob behavior.
		if !strings.ContainsAny(rest, "*?") {
			return matchPathOrUnder(rest)
		}
		// **/*.ext
		if strings.HasPrefix(rest, "*.") && !strings.ContainsAny(rest[2:], "*?") {
			ext := rest[1:] // ".ext"
			return func(path string) bool {
				return strings.HasSuffix(path, ext)
			}
		}
		return nil
	}
	if strings.HasSuffix(p, "/**") {
		prefix := p[:len(p)-3]
		if prefix != "" && !strings.ContainsAny(prefix, "*?") {
			return matchPathOrUnder(prefix)
		}
	}
	if !strings.ContainsAny(p, "*?") {
		return matchPathOrUnder(p)
	}
	return nil
}

// matchPathOrUnder matches path == target, …/target, target/…, or …/target/….
func matchPathOrUnder(target string) excludeMatcher {
	suf := "/" + target
	pre := target + "/"
	mid := "/" + target + "/"
	return func(path string) bool {
		return path == target ||
			strings.HasSuffix(path, suf) ||
			strings.HasPrefix(path, pre) ||
			strings.Contains(path, mid)
	}
}

func excluded(path string, ex excludeSet) bool {
	if len(ex.matchers) == 0 {
		return false
	}
	rel := path
	if strings.HasPrefix(rel, "/") {
		rel = strings.TrimLeft(rel, "/")
	}
	for _, m := range ex.matchers {
		if m(rel) {
			return true
		}
	}
	return false
}

func globToRegexp(glob string) string {
	var out strings.Builder
	out.WriteByte('^')
	for i := 0; i < len(glob); i++ {
		c := glob[i]
		if c == '*' && i+1 < len(glob) && glob[i+1] == '*' {
			if i+2 < len(glob) && glob[i+2] == '/' {
				out.WriteString("(?:.*/)?")
				i += 2
			} else {
				out.WriteString(".*")
				i++
			}
			continue
		}
		if c == '*' {
			out.WriteString("[^/]*")
			continue
		}
		if c == '?' {
			out.WriteString("[^/]")
			continue
		}
		if strings.ContainsRune(`.+^${}()|[]\`, rune(c)) {
			out.WriteByte('\\')
			out.WriteByte(c)
			continue
		}
		out.WriteByte(c)
	}
	out.WriteByte('$')
	return out.String()
}
