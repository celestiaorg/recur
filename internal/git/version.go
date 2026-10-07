package git

import (
	"context"
	"errors"
	"fmt"
	"strconv"
	"strings"
	"sync"
	"time"
)

// MinVersion is the oldest git Recur supports. --diff-merges=first-parent
// (LogCommits merge numstat) landed in 2.31; older git fatals that flag and
// left the Commits tab empty.
var MinVersion = Version{Major: 2, Minor: 31}

// ErrUnparsed is returned by Probe when `git version` ran but the string
// could not be parsed (distinct from a missing binary).
var ErrUnparsed = errors.New("unparsed git version")

// Version is a git X.Y.Z triple (extra tokens like Apple Git / windows.1 ignored).
type Version struct {
	Major, Minor, Patch int
}

func (v Version) String() string {
	if v.Patch == 0 {
		return fmt.Sprintf("%d.%d", v.Major, v.Minor)
	}
	return fmt.Sprintf("%d.%d.%d", v.Major, v.Minor, v.Patch)
}

// Less reports whether v is strictly older than o.
func (v Version) Less(o Version) bool {
	if v.Major != o.Major {
		return v.Major < o.Major
	}
	if v.Minor != o.Minor {
		return v.Minor < o.Minor
	}
	return v.Patch < o.Patch
}

// ParseVersion reads `git version` stdout (e.g. "git version 2.50.1 (Apple Git-155)").
// A non-numeric patch token (source builds: "2.45.GIT") is treated as patch 0 so
// major.minor still compare against MinVersion.
func ParseVersion(s string) (Version, bool) {
	s = strings.TrimSpace(s)
	if i := strings.Index(s, "git version "); i >= 0 {
		s = s[i+len("git version "):]
	}
	if i := strings.IndexAny(s, " \t("); i >= 0 {
		s = s[:i]
	}
	parts := strings.Split(s, ".")
	if len(parts) < 2 {
		return Version{}, false
	}
	major, ok1 := leadingInt(parts[0])
	minor, ok2 := leadingInt(parts[1])
	if !ok1 || !ok2 {
		return Version{}, false
	}
	patch := 0
	if len(parts) >= 3 {
		if p, ok := leadingInt(parts[2]); ok {
			patch = p
		}
	}
	return Version{Major: major, Minor: minor, Patch: patch}, true
}

func leadingInt(s string) (int, bool) {
	n := 0
	for n < len(s) && s[n] >= '0' && s[n] <= '9' {
		n++
	}
	if n == 0 {
		return 0, false
	}
	v, err := strconv.Atoi(s[:n])
	return v, err == nil
}

var (
	probeOnce sync.Once
	probeVer  Version
	probeErr  error
)

// Probe returns the git on PATH's version. One subprocess per process.
func Probe() (Version, error) {
	probeOnce.Do(func() {
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		out, err := Command(ctx, "version").Output()
		if err != nil {
			probeErr = fmt.Errorf("git version: %w", err)
			return
		}
		v, ok := ParseVersion(string(out))
		if !ok {
			probeErr = fmt.Errorf("%w: %q", ErrUnparsed, strings.TrimSpace(string(out)))
			return
		}
		probeVer = v
	})
	return probeVer, probeErr
}

// MessageIfUnsupported is the user-facing alert when git is missing, unreadable,
// or older than MinVersion. Empty when the install is fine.
func MessageIfUnsupported(v Version, err error) string {
	if err != nil {
		if errors.Is(err, ErrUnparsed) {
			return "Recur needs Git " + MinVersion.String() + " or newer but could not read this Git's version. Update Git so pull-request commits load correctly."
		}
		return "Recur needs Git " + MinVersion.String() + " or newer on PATH. Git was not found."
	}
	if v.Less(MinVersion) {
		return fmt.Sprintf("Recur needs Git %s or newer (found %s). Update Git so pull-request commits load correctly.", MinVersion, v)
	}
	return ""
}

// UnsupportedAlert is the OS-notification title + body for a failed git probe.
func UnsupportedAlert(v Version, err error) (title, body string) {
	body = MessageIfUnsupported(v, err)
	if body == "" {
		return "", ""
	}
	switch {
	case err == nil:
		return "Git is too old", body
	case errors.Is(err, ErrUnparsed):
		return "Git version unknown", body
	default:
		return "Git was not found", body
	}
}

// logNumstatFlags is --diff-merges=first-parent when git is new enough; omitted
// on older git so `git log` does not fatal and empty the Commits tab.
func logNumstatFlags(v Version, err error) []string {
	if err == nil && !v.Less(MinVersion) {
		return []string{"--diff-merges=first-parent"}
	}
	return nil
}
