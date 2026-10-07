package git

import (
	"context"
	"path"
	"strings"
)

// StatusMap runs `git status --porcelain` in dir and returns repo-relative
// paths mapped to a single letter: m (modified), a (added), d (deleted),
// u (untracked). Matches FileDiff status letters, plus u for untracked.
// Best-effort: returns nil when dir is not a git work tree or git fails.
func StatusMap(ctx context.Context, dir string) map[string]string {
	if dir == "" {
		return nil
	}
	return statusMapFlights.do(ctx, dir, func(cctx context.Context) map[string]string {
		return statusMapWork(cctx, dir)
	})
}

// statusMapNow always shells git. DiscardFile must not join an in-flight poll
// whose snapshot predates the caller's own write (a coalesced "path absent"
// would make Discard a silent no-op; a stale "u" would os.RemoveAll a tracked file).
func statusMapNow(ctx context.Context, dir string) map[string]string {
	if dir == "" {
		return nil
	}
	return statusMapWork(ctx, dir)
}

func statusMapWork(ctx context.Context, dir string) map[string]string {
	if statusComputeHook != nil {
		statusComputeHook("porcelain")
	}
	out, err := Command(ctx, "-C", dir, "status", "--porcelain", "-uall").Output()
	if err != nil {
		return nil
	}
	return ParseStatusPorcelain(string(out))
}

// ParseStatusPorcelain parses `git status --porcelain` (-z not required) into
// path → m|a|d|u. Renames/copies mark the destination as added and the source
// as deleted. Ignored (!!) entries are skipped.
func ParseStatusPorcelain(out string) map[string]string {
	out = strings.TrimSuffix(out, "\n")
	if out == "" {
		return nil
	}
	m := make(map[string]string)
	for _, line := range strings.Split(out, "\n") {
		if len(line) < 3 {
			continue
		}
		xy := line[:2]
		if xy == "!!" {
			continue
		}
		rest := line[3:]
		if rest == "" {
			continue
		}
		from, to := splitStatusPath(rest)
		to = strings.TrimSuffix(to, "/")
		if to == "" {
			continue
		}
		if from != "" {
			from = strings.TrimSuffix(from, "/")
			if from != "" {
				m[from] = "d"
			}
			m[to] = "a"
			continue
		}
		st := statusLetter(xy)
		if st == "" {
			continue
		}
		m[to] = st
	}
	if len(m) == 0 {
		return nil
	}
	return m
}

// splitStatusPath handles "path", "old -> new", and quoted forms.
func splitStatusPath(rest string) (from, to string) {
	rest = strings.TrimSpace(rest)
	if i := strings.Index(rest, " -> "); i >= 0 {
		return unquotePath(rest[:i]), unquotePath(rest[i+4:])
	}
	return "", unquotePath(rest)
}

func unquotePath(p string) string {
	p = strings.TrimSpace(p)
	if len(p) >= 2 && p[0] == '"' && p[len(p)-1] == '"' {
		// Porcelain quotes are C-style; unescape the common cases.
		p = p[1 : len(p)-1]
		p = strings.ReplaceAll(p, `\"`, `"`)
		p = strings.ReplaceAll(p, `\\`, `\`)
		p = strings.ReplaceAll(p, `\t`, "\t")
		p = strings.ReplaceAll(p, `\n`, "\n")
	}
	p = strings.ReplaceAll(p, "\\", "/")
	if p == "." || p == "" {
		return ""
	}
	return path.Clean(p)
}

// statusLetter collapses the XY index/worktree pair into one UI letter.
// Priority: deleted > added > modified > untracked.
func statusLetter(xy string) string {
	if xy == "??" {
		return "u"
	}
	x, y := xy[0], xy[1]
	rank := func(c byte) int {
		switch c {
		case 'D':
			return 4
		case 'A', 'C':
			return 3
		case 'M', 'R', 'T', 'U':
			return 2
		case '?':
			return 1
		default:
			return 0
		}
	}
	pick := x
	if rank(y) > rank(x) {
		pick = y
	}
	switch pick {
	case 'D':
		return "d"
	case 'A', 'C':
		return "a"
	case 'M', 'R', 'T', 'U':
		return "m"
	case '?':
		return "u"
	default:
		return ""
	}
}
