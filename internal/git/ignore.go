package git

import (
	"bytes"
	"context"
	"strings"
)

// IgnoredSet reports which of paths git ignores in dir (gitignore,
// info/exclude, and the user's global excludes). Tracked files that happen
// to match a pattern are not ignored — same as `git check-ignore`.
// Best-effort: returns nil when dir is not a work tree, git fails, or
// paths is empty.
//
// Paths are sent NUL-delimited (`check-ignore -z --stdin`) so names with
// spaces, newlines, or non-ASCII bytes round-trip without C-quoting.
func IgnoredSet(ctx context.Context, dir string, paths []string) map[string]bool {
	if dir == "" || len(paths) == 0 {
		return nil
	}
	var b strings.Builder
	n := 0
	for _, p := range paths {
		if p == "" || p == "." {
			continue
		}
		b.WriteString(p)
		b.WriteByte(0)
		n++
	}
	if n == 0 {
		return nil
	}
	cmd := Command(ctx, "-C", dir, "check-ignore", "-z", "--stdin")
	cmd.Stdin = strings.NewReader(b.String())
	out, err := cmd.Output()
	if err != nil {
		return nil
	}
	return ParseCheckIgnore(out)
}

// ParseCheckIgnore parses `git check-ignore -z --stdin` stdout (NUL-delimited
// raw paths, no quoting) into a set.
func ParseCheckIgnore(out []byte) map[string]bool {
	if len(out) == 0 {
		return nil
	}
	m := make(map[string]bool)
	for _, p := range bytes.Split(out, []byte{0}) {
		if len(p) == 0 {
			continue
		}
		m[string(p)] = true
	}
	if len(m) == 0 {
		return nil
	}
	return m
}
