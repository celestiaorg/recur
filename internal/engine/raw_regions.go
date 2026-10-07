package engine

import (
	"fmt"
	"regexp"
	"strings"

	"recur/internal/domain"
	"recur/internal/term"
)

// Raw regions exclude @path / /skill host wrapping. Prefer nonce-paired markers
// so embedded blobs (git diffs) that quote the bare syntax cannot close early:
//
//	⟦raw:8f3a⟧ … ⟦/raw:8f3a⟧
//
// Bare ⟦raw⟧…⟦/raw⟧ is still accepted for small author-controlled snippets that
// cannot contain the close marker. Markers are stripped before storage/provider.
var (
	rawOpenRe = regexp.MustCompile(`⟦raw(?::([A-Za-z0-9_-]+))?⟧`)
	// Any leftover open/close after processing (stray / unmatched).
	rawMarkerStripRe = regexp.MustCompile(`⟦/?raw(?::[A-Za-z0-9_-]+)?⟧`)
)

// wrapComposerDialect applies @file and /skill wrapping, skipping raw regions.
// Markers are removed from the returned text. enabled filters host skill
// activation (nil → all allowed); same contract as wrapActivatedSkills.
func wrapComposerDialect(text, worktree string, enabled func(string) bool) string {
	return wrapComposer(text, worktree, "", enabled, nil)
}

// wrapComposer is wrapComposerDialect plus @terminal snapshots and @artifact
// bodies when workspaceID is set.
func wrapComposer(text, worktree, workspaceID string, enabled func(string) bool, terms *term.Manager) string {
	masked, regions := maskRawRegions(text)
	// Drop unmatched markers in scannable text only — restored raw bodies may
	// legitimately quote ⟦raw⟧ / ⟦/raw⟧ (docs, this source) and must keep them.
	masked = stripStrayRawMarkers(masked)
	mentions := collectTerminalMentions(masked)
	arts := collectArtifactMentions(masked)
	var artList []domain.Artifact
	if workspaceID != "" && strings.Contains(masked, "@") {
		list, err := listArtifacts(workspaceID)
		if err != nil {
			list = nil
		}
		artList = list
		arts = append(arts, collectLooseArtifactTitleMentions(masked, list)...)
	}
	out := wrapAttachedFileMentions(masked, worktree)
	out = wrapActivatedSkills(out, worktree, enabled)
	// Terminal / artifact bodies last so their contents are not scanned for
	// @file / /skill. Mentions are taken from the pre-wrap composer so injected
	// SKILL.md bodies that document `@terminal` / `@artifact` / `@Title` cannot
	// attach something the user never asked for.
	out = attachTerminalMentions(out, mentions, workspaceID, terms)
	out = attachArtifactMentions(out, arts, artList)
	return unmaskRawRegions(out, regions)
}

func maskRawRegions(text string) (masked string, regions []string) {
	if text == "" || !strings.Contains(text, "⟦raw") {
		return text, nil
	}
	var b strings.Builder
	rest := text
	for {
		loc := rawOpenRe.FindStringSubmatchIndex(rest)
		if loc == nil {
			b.WriteString(rest)
			break
		}
		b.WriteString(rest[:loc[0]])
		id := ""
		if loc[2] >= 0 {
			id = rest[loc[2]:loc[3]]
		}
		afterOpen := rest[loc[1]:]
		closeIdx, closeLen := findMatchingRawClose(afterOpen, id)
		if closeIdx < 0 {
			// Unclosed — remainder is raw; open marker dropped.
			regions = append(regions, afterOpen)
			b.WriteString(rawPlaceholder(len(regions) - 1))
			break
		}
		regions = append(regions, afterOpen[:closeIdx])
		b.WriteString(rawPlaceholder(len(regions) - 1))
		rest = afterOpen[closeIdx+closeLen:]
	}
	return b.String(), regions
}

// findMatchingRawClose returns the index and length of the close marker in s
// that matches id (empty id → bare ⟦/raw⟧ only, not ⟦/raw:nonce⟧).
func findMatchingRawClose(s, id string) (idx, length int) {
	if id != "" {
		want := "⟦/raw:" + id + "⟧"
		j := strings.Index(s, want)
		if j < 0 {
			return -1, 0
		}
		return j, len(want)
	}
	const bare = "⟦/raw⟧"
	for i := 0; ; {
		j := strings.Index(s[i:], bare)
		if j < 0 {
			return -1, 0
		}
		j += i
		// Skip nonced closes that share the ⟦/raw prefix: ⟦/raw:nonce⟧.
		if strings.HasPrefix(s[j:], "⟦/raw:") {
			i = j + len("⟦/raw:")
			continue
		}
		return j, len(bare)
	}
}

func unmaskRawRegions(text string, regions []string) string {
	if len(regions) == 0 {
		return text
	}
	out := text
	for i, body := range regions {
		out = strings.Replace(out, rawPlaceholder(i), body, 1)
	}
	return out
}

func stripStrayRawMarkers(text string) string {
	if text == "" || !strings.Contains(text, "⟦") {
		return text
	}
	return rawMarkerStripRe.ReplaceAllString(text, "")
}

func rawPlaceholder(i int) string {
	// No @ or / after whitespace — must not look like a file/skill mention.
	return fmt.Sprintf("⟦rawph:%d⟧", i)
}
