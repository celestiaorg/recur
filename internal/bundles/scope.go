package bundles

import (
	"path/filepath"
	"strings"
)

// Matches reports whether cmd applies to the given editor grammar scope and
// file path. Unscoped commands (empty scope and fileGlob) match everything.
// Grammar matching is TextMate-style prefix: scope "source.go" matches
// "source.go" and "source.go.string"; fileGlob uses filepath.Match against
// the base name and the full slash path.
func Matches(cmd Command, fileScope, filePath string) bool {
	if cmd.Scope == "" && cmd.FileGlob == "" {
		return true
	}
	if cmd.Scope != "" && !scopePrefixMatch(cmd.Scope, fileScope) {
		return false
	}
	if cmd.FileGlob != "" && !globMatch(cmd.FileGlob, filePath) {
		return false
	}
	return true
}

func scopePrefixMatch(want, have string) bool {
	want = strings.TrimSpace(want)
	have = strings.TrimSpace(have)
	if want == "" {
		return true
	}
	if have == "" {
		return false
	}
	for _, alt := range strings.Split(want, ",") {
		alt = strings.TrimSpace(alt)
		if alt == "" {
			continue
		}
		if have == alt || strings.HasPrefix(have, alt+".") {
			return true
		}
	}
	return false
}

// MatchFileGlob reports whether filePath matches any of the patterns.
// Empty patterns match every path. Each pattern uses the same rules as
// command fileGlob (basename, slash path, **/, and one {a,b} brace group).
func MatchFileGlob(patterns []string, filePath string) bool {
	if len(patterns) == 0 {
		return true
	}
	for _, p := range patterns {
		if globMatch(p, filePath) {
			return true
		}
	}
	return false
}

func globMatch(pattern, filePath string) bool {
	pattern = strings.TrimSpace(pattern)
	if pattern == "" {
		return true
	}
	// Support one `{a,b,c}` brace group (e.g. `*.{js,ts}`) — filepath.Match does not.
	if alts := expandBraces(pattern); len(alts) > 1 {
		for _, p := range alts {
			if globMatch(p, filePath) {
				return true
			}
		}
		return false
	}
	filePath = filepath.ToSlash(filePath)
	base := filepath.Base(filePath)
	if ok, _ := filepath.Match(pattern, base); ok {
		return true
	}
	if ok, _ := filepath.Match(pattern, filePath); ok {
		return true
	}
	// Also try matching against leading **/
	if strings.HasPrefix(pattern, "**/") {
		suf := pattern[3:]
		if ok, _ := filepath.Match(suf, base); ok {
			return true
		}
		for i := 0; i < len(filePath); i++ {
			if filePath[i] == '/' {
				if ok, _ := filepath.Match(suf, filePath[i+1:]); ok {
					return true
				}
			}
		}
	}
	return false
}

func expandBraces(pattern string) []string {
	start := strings.Index(pattern, "{")
	end := strings.LastIndex(pattern, "}")
	if start < 0 || end <= start {
		return []string{pattern}
	}
	prefix := pattern[:start]
	suffix := pattern[end+1:]
	parts := strings.Split(pattern[start+1:end], ",")
	out := make([]string, 0, len(parts))
	for _, p := range parts {
		p = strings.TrimSpace(p)
		if p == "" {
			continue
		}
		out = append(out, prefix+p+suffix)
	}
	if len(out) == 0 {
		return []string{pattern}
	}
	return out
}
