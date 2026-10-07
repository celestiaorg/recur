package engine

import (
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"unicode/utf8"

	"recur/internal/domain"
	"recur/internal/store"
)

// artifactAtRe matches @artifact or @artifact:<id-or-title> in chat wire text.
// Qualifier is a non-space token (uuid, prefix, or unique title without spaces).
// Keep in sync with frontend ARTIFACT_AT_RE (frontend/src/lib/agentchat.ts).
var artifactAtRe = regexp.MustCompile(`@artifact(?::([^\s@]+))?`)

const (
	maxArtifactSnapshots = 4
	// maxArtifactBytes caps @artifact mention snapshots. read_artifact reads the full body.
	maxArtifactBytes = 32 * 1024
)

func fenceArtifactBody(body string) string {
	body = scrubInstructionText(body)
	n := 3
	for strings.Contains(body, strings.Repeat("`", n)) && n < 32 {
		n++
	}
	fence := strings.Repeat("`", n)
	var b strings.Builder
	b.WriteString(fence)
	b.WriteByte('\n')
	b.WriteString(body)
	if !strings.HasSuffix(body, "\n") {
		b.WriteByte('\n')
	}
	b.WriteString(fence)
	b.WriteByte('\n')
	return b.String()
}

type artifactMention struct {
	chip  string // token after @ (artifact, artifact:<qual>, or a unique title)
	qual  string // empty for bare @artifact
	start int
	// loose is @Title (the Artifacts panel name) rather than @artifact / @artifact:qual.
	// Unmatched loose mentions are left as prose — they are not "no matching artifact".
	loose bool
}

var atNameRe = regexp.MustCompile(`@([^\s@]+)`)

func atTokenBounded(text string, start, end int) bool {
	if start > 0 {
		prev := text[start-1]
		if prev != ' ' && prev != '\t' && prev != '\n' && prev != '\r' {
			return false
		}
	}
	if end < len(text) {
		next := text[end]
		if next != ' ' && next != '\t' && next != '\n' && next != '\r' {
			return false
		}
	}
	return true
}

func reservedAtName(token string) bool {
	t := strings.ToLower(token)
	return t == "artifact" || strings.HasPrefix(t, "artifact:") ||
		t == "terminal" || strings.HasPrefix(t, "terminal:")
}

func atNameIsFileMention(token string) bool {
	s := "@" + token
	return fileAtRe.FindString(s) == s
}

func scrubInstructionText(s string) string {
	return strings.ReplaceAll(s, "</system_instruction>", "< /system_instruction>")
}

// collectArtifactMentions returns unique @artifact chips in first-seen order.
func collectArtifactMentions(text string) []artifactMention {
	if text == "" || !strings.Contains(text, "@artifact") {
		return nil
	}
	seen := map[string]bool{}
	var out []artifactMention
	for _, idx := range artifactAtRe.FindAllStringSubmatchIndex(text, -1) {
		if len(idx) < 2 {
			continue
		}
		start, end := idx[0], idx[1]
		if !atTokenBounded(text, start, end) {
			continue
		}
		qual := ""
		if len(idx) >= 4 && idx[2] >= 0 {
			qual = text[idx[2]:idx[3]]
		}
		chip := "artifact"
		if qual != "" {
			chip = "artifact:" + qual
		}
		if seen[chip] {
			continue
		}
		seen[chip] = true
		out = append(out, artifactMention{chip: chip, qual: qual, start: start})
	}
	return out
}

// collectLooseArtifactTitleMentions returns @Title tokens that uniquely match
// an Artifacts-panel title (no spaces — those are picked from the @ menu).
func collectLooseArtifactTitleMentions(text string, list []domain.Artifact) []artifactMention {
	if text == "" || !strings.Contains(text, "@") || len(list) == 0 {
		return nil
	}
	unique := uniqueSingleTokenTitles(list)
	if len(unique) == 0 {
		return nil
	}
	seen := map[string]bool{}
	var out []artifactMention
	for _, idx := range atNameRe.FindAllStringSubmatchIndex(text, -1) {
		if len(idx) < 4 || idx[2] < 0 {
			continue
		}
		start, end := idx[0], idx[1]
		if !atTokenBounded(text, start, end) {
			continue
		}
		token := text[idx[2]:idx[3]]
		if reservedAtName(token) || atNameIsFileMention(token) {
			continue
		}
		key := strings.ToLower(token)
		if unique[key] != 1 {
			continue
		}
		if seen[key] {
			continue
		}
		seen[key] = true
		out = append(out, artifactMention{chip: token, qual: token, start: start, loose: true})
	}
	return out
}

func uniqueSingleTokenTitles(list []domain.Artifact) map[string]int {
	n := map[string]int{}
	for i := range list {
		title := strings.TrimSpace(list[i].Title)
		if title == "" || strings.ContainsAny(title, " \t\n\r") {
			continue
		}
		if reservedAtName(title) {
			continue
		}
		n[strings.ToLower(title)]++
	}
	return n
}

// wrapAttachedArtifactMentions prepends the contents of attached artifacts
// (text kinds inlined; png/jpeg noted as binary). No-op with no mentions.
func wrapAttachedArtifactMentions(text, workspaceID string) string {
	if workspaceID == "" {
		return text
	}
	mentions := collectArtifactMentions(text)
	var list []domain.Artifact
	if strings.Contains(text, "@") {
		var err error
		list, err = listArtifacts(workspaceID)
		if err != nil {
			list = nil
		}
		mentions = append(mentions, collectLooseArtifactTitleMentions(text, list)...)
	}
	return attachArtifactMentions(text, mentions, list)
}

func attachArtifactMentions(text string, mentions []artifactMention, list []domain.Artifact) string {
	if len(mentions) == 0 || len(list) == 0 {
		return text
	}

	var b strings.Builder
	b.WriteString("<system_instruction>\n")
	b.WriteString("The user attached Observatory artifacts. These live under the managed data dir ")
	b.WriteString("(workspaces/<id>/artifacts/) — not the git worktree. Do not search the repo for these paths. ")
	b.WriteString("For another artifact, call mcp__recur__list_artifacts / mcp__recur__read_artifact / mcp__recur__present_artifact.\n")
	wrote := 0
	seenID := map[string]bool{}
	for _, m := range mentions {
		if wrote >= maxArtifactSnapshots {
			break
		}
		hit := resolveArtifactMention(m, list)
		if hit == nil {
			if m.loose {
				continue
			}
			fmt.Fprintf(&b, "\n### @%s\n(no matching artifact)\n", scrubInstructionText(m.chip))
			wrote++
			continue
		}
		if seenID[hit.ID] {
			continue
		}
		seenID[hit.ID] = true
		title := strings.TrimSpace(hit.Title)
		if title == "" {
			title = hit.ID
		}
		fmt.Fprintf(&b, "\n### @%s (%s, %s, id %s)\n",
			scrubInstructionText(m.chip),
			scrubInstructionText(title),
			scrubInstructionText(string(hit.Kind)),
			scrubInstructionText(hit.ID),
		)
		if artifactKindIsBinary(hit.Kind) {
			fmt.Fprintf(&b, "(binary %s image — open in the Artifacts panel; bytes are not inlined)\n", hit.Kind)
			wrote++
			continue
		}
		body, rerr := readArtifactText(*hit)
		if rerr != nil {
			fmt.Fprintf(&b, "(could not read: %s)\n", rerr.Error())
			wrote++
			continue
		}
		if strings.TrimSpace(body) == "" {
			b.WriteString("(empty)\n")
			wrote++
			continue
		}
		b.WriteString(fenceArtifactBody(body))
		wrote++
	}
	if wrote == 0 {
		return text
	}
	b.WriteString("</system_instruction>\n\n")
	b.WriteString(text)
	return b.String()
}

func resolveArtifactMention(m artifactMention, list []domain.Artifact) *domain.Artifact {
	if len(list) == 0 {
		return nil
	}
	if m.qual == "" {
		// Newest first (listArtifacts sorts by createdAt desc).
		return &list[0]
	}
	qual := m.qual
	if m.loose {
		found := -1
		for i := range list {
			if strings.EqualFold(strings.TrimSpace(list[i].Title), qual) {
				if found >= 0 {
					return nil
				}
				found = i
			}
		}
		if found < 0 {
			return nil
		}
		return &list[found]
	}
	var prefix, title []domain.Artifact
	for i := range list {
		a := list[i]
		if a.ID == qual {
			return &list[i]
		}
		if strings.HasPrefix(a.ID, qual) {
			prefix = append(prefix, a)
		}
		if strings.EqualFold(strings.TrimSpace(a.Title), qual) {
			title = append(title, a)
		}
	}
	if len(prefix) == 1 {
		return &prefix[0]
	}
	if len(title) == 1 {
		return &title[0]
	}
	return nil
}

func artifactKindIsBinary(k domain.ArtifactKind) bool {
	return k == domain.ArtifactPNG || k == domain.ArtifactJPEG
}

func readArtifactText(a domain.Artifact) (string, error) {
	return readArtifactTextLimit(a, maxArtifactBytes)
}

func artifactAbsPath(a domain.Artifact) (string, error) {
	root, err := store.DataDir()
	if err != nil {
		return "", err
	}
	rel := filepath.FromSlash(strings.TrimPrefix(a.Path, "/"))
	full := filepath.Join(root, rel)
	dir, err := store.ArtifactsDir(root, a.WorkspaceID)
	if err != nil {
		return "", err
	}
	if !strings.HasPrefix(full, dir+string(os.PathSeparator)) && full != dir {
		return "", fmt.Errorf("artifact path outside workspace")
	}
	return full, nil
}

// readArtifactTextLimit reads a text artifact. maxBytes <= 0 reads the whole file;
// otherwise the body is capped and marked truncated (mention snapshots).
func readArtifactTextLimit(a domain.Artifact, maxBytes int) (string, error) {
	full, err := artifactAbsPath(a)
	if err != nil {
		return "", err
	}
	f, err := os.Open(full)
	if err != nil {
		return "", err
	}
	defer f.Close()
	var raw []byte
	more := false
	if maxBytes <= 0 {
		raw, err = io.ReadAll(f)
		if err != nil {
			return "", err
		}
	} else {
		buf := make([]byte, maxBytes+1)
		n, err := f.Read(buf)
		if err != nil && err != io.EOF {
			return "", err
		}
		more = n > maxBytes
		if more {
			n = maxBytes
		}
		for n > 0 && !utf8.RuneStart(buf[n]) {
			n--
		}
		raw = buf[:n]
	}
	s := string(raw)
	if !utf8.ValidString(s) {
		return "", fmt.Errorf("not valid UTF-8")
	}
	if more {
		s += "\n…(truncated)"
	}
	return s, nil
}
