package engine

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
)

// fileAtRe matches @-mentions of workspace files in chat wire text:
//   - absolute paths (`/Users/…/app.go`)
//   - relative paths with a slash (`src/main.go`)
//   - name.ext (`app.css`)
//   - leading-dot files (`.gitignore`)
//   - directories with a trailing slash (`src/`, `src/lib/`)
//   - optional loc `[:line[:col][-line[:col]]]` (`@path:12`, `@path:12-40`,
//     `@path:12:5-12:18`, `@path:12:5-13:20`)
//
// Absolute path segments omit `:` so the optional loc suffix can be captured.
// Left-boundary is enforced in collectFileMentions (Go RE2 has no lookbehind).
// Keep in sync with frontend FILE_AT_RE (frontend/src/lib/agentchat.ts).
var fileAtRe = regexp.MustCompile(
	`@((?:/(?:[^\s@:]+(?:/[^\s@:]+)*)|(?:[\w.-]+/)+[\w./+-]+|[\w.-]+/|[\w.-]+\.[A-Za-z][\w.-]*|\.[A-Za-z0-9_][\w.-]*)(?::\d+(?::\d+)?(?:-\d+(?::\d+)?)?)?)`,
)

// Loc suffix: [:line[:col][-line[:col]]]. Optional groups are -1 in submatch index.
var fileLocRe = regexp.MustCompile(`:(\d+)(?::(\d+))?(?:-(\d+)(?::(\d+))?)?$`)

type fileMention struct {
	chip      string // token as written after @ (path[:loc], usually worktree-relative)
	abs       string
	dir       bool
	startLine int
	endLine   int
	startCol  int
	endCol    int
}

// @token not covered by fileAtRe (bare dir names like @src). Existence-gated.
var looseAtRe = regexp.MustCompile(`@([^\s@]+)`)

func reservedAtChip(chip string) bool {
	s := strings.ToLower(strings.TrimSpace(chip))
	return s == "terminal" || strings.HasPrefix(s, "terminal:") ||
		s == "artifact" || strings.HasPrefix(s, "artifact:")
}

func atoiPositive(s string) int {
	n, err := strconv.Atoi(s)
	if err != nil || n <= 0 {
		return 0
	}
	return n
}

// splitFileMentionChip separates path from an optional loc suffix
// (`[:line[:col][-line[:col]]]`). Invalid line numbers (`:0`, Atoi overflow)
// strip the suffix and attach the whole file rather than treating "path:0" as
// a nonexistent path. Invalid columns keep the line(s) and drop the column span.
func splitFileMentionChip(chip string) (path string, startLine, endLine, startCol, endCol int) {
	chip = strings.TrimSpace(chip)
	if chip == "" {
		return "", 0, 0, 0, 0
	}
	loc := fileLocRe.FindStringSubmatchIndex(chip)
	if loc == nil {
		return chip, 0, 0, 0, 0
	}
	path = chip[:loc[0]]
	if path == "" {
		return chip, 0, 0, 0, 0
	}
	startLine = atoiPositive(chip[loc[2]:loc[3]])
	if startLine == 0 {
		return path, 0, 0, 0, 0
	}
	if loc[4] >= 0 {
		startCol = atoiPositive(chip[loc[4]:loc[5]])
	}
	if loc[6] >= 0 {
		endLine = atoiPositive(chip[loc[6]:loc[7]])
	}
	if loc[8] >= 0 {
		endCol = atoiPositive(chip[loc[8]:loc[9]])
	}
	if startCol > 0 && endCol > 0 {
		endLn := endLine
		if endLn == 0 {
			endLn = startLine
		}
		if endLn < startLine || (endLn == startLine && endCol < startCol) {
			return path, startLine, 0, 0, 0
		}
		if endLn == startLine {
			return path, startLine, 0, startCol, endCol
		}
		return path, startLine, endLn, startCol, endCol
	}
	if endLine <= startLine {
		endLine = 0
	}
	return path, startLine, endLine, 0, 0
}

// formatFileMentionChip builds path, path:line, path:start-end, or a column
// span. Column spans always write both line:col ends (`12:5-12:18`).
func formatFileMentionChip(path string, startLine, endLine, startCol, endCol int) string {
	path = strings.TrimSpace(path)
	if path == "" {
		return ""
	}
	if startLine <= 0 {
		return path
	}
	if startCol > 0 && endCol > 0 {
		endLn := max(endLine, startLine)
		if endLn == startLine && endCol < startCol {
			endCol = startCol
		}
		return fmt.Sprintf("%s:%d:%d-%d:%d", path, startLine, startCol, endLn, endCol)
	}
	if endLine > startLine {
		return fmt.Sprintf("%s:%d-%d", path, startLine, endLine)
	}
	return fmt.Sprintf("%s:%d", path, startLine)
}

// canonicalizeAbs resolves worktree-relative chips to an absolute path and, when
// the file exists, follows symlinks so the same file mentioned two ways
// (@README.md and @/abs/…/README.md) collapses to one preamble row.
func canonicalizeAbs(chip, worktree string) (abs string, ok bool) {
	if chip == "" {
		return "", false
	}
	if filepath.IsAbs(chip) {
		abs = filepath.Clean(chip)
	} else {
		if worktree == "" {
			return "", false
		}
		abs = filepath.Clean(filepath.Join(worktree, filepath.FromSlash(chip)))
	}
	if resolved, err := filepath.EvalSymlinks(abs); err == nil && resolved != "" {
		abs = resolved
	}
	return abs, true
}

func canonicalWorktree(worktree string) string {
	worktree = strings.TrimRight(worktree, string(filepath.Separator))
	if worktree == "" {
		return ""
	}
	if resolved, err := filepath.EvalSymlinks(worktree); err == nil && resolved != "" {
		return resolved
	}
	return filepath.Clean(worktree)
}

func atMentionBoundary(text string, start int) bool {
	if start == 0 {
		return true
	}
	prev := text[start-1]
	return prev == ' ' || prev == '\t' || prev == '\n' || prev == '\r'
}

func mentionPreamblePath(pathChip, abs, worktree string) string {
	if filepath.IsAbs(pathChip) && worktree != "" {
		if rel, err := filepath.Rel(worktree, abs); err == nil && rel != "" && !strings.HasPrefix(rel, "..") {
			return filepath.ToSlash(rel)
		}
	}
	preamblePath := filepath.ToSlash(filepath.Clean(filepath.FromSlash(pathChip)))
	if preamblePath == "." || preamblePath == "" {
		return pathChip
	}
	return preamblePath
}

func appendMention(out []fileMention, seen map[string]bool, chip, worktree string, dirsOnly bool) []fileMention {
	if chip == "" || reservedAtChip(chip) {
		return out
	}
	pathChip, startLine, endLine, startCol, endCol := splitFileMentionChip(chip)
	pathChip = strings.TrimRight(pathChip, "/")
	abs, ok := canonicalizeAbs(pathChip, worktree)
	if !ok {
		return out
	}
	info, err := os.Stat(abs)
	if err != nil {
		return out
	}
	dir := info.IsDir()
	if dirsOnly && !dir {
		return out
	}
	if dir {
		startLine, endLine, startCol, endCol = 0, 0, 0, 0
	}
	seenKey := abs
	if startLine > 0 {
		seenKey = formatFileMentionChip(abs, startLine, endLine, startCol, endCol)
	}
	if seen[seenKey] {
		return out
	}
	seen[seenKey] = true
	preambleChip := formatFileMentionChip(mentionPreamblePath(pathChip, abs, worktree), startLine, endLine, startCol, endCol)
	return append(out, fileMention{
		chip:      preambleChip,
		abs:       abs,
		dir:       dir,
		startLine: startLine,
		endLine:   endLine,
		startCol:  startCol,
		endCol:    endCol,
	})
}

// collectFileMentions returns unique @file / @dir mentions in first-seen order.
// Dedupes by canonical absolute path + line span so repeated chips (and abs+rel
// aliases) produce a single attached-files preamble line. Mentions must be at
// start of text or after whitespace (not emails / decorators / scoped packages),
// and the resolved path must exist as a file or directory.
func collectFileMentions(text, worktree string) []fileMention {
	if text == "" || !strings.Contains(text, "@") {
		return nil
	}
	worktree = canonicalWorktree(worktree)
	seen := map[string]bool{}
	var out []fileMention
	for _, idx := range fileAtRe.FindAllStringSubmatchIndex(text, -1) {
		if len(idx) < 4 {
			continue
		}
		start, chip := idx[0], text[idx[2]:idx[3]]
		if !atMentionBoundary(text, start) {
			continue
		}
		out = appendMention(out, seen, chip, worktree, false)
	}
	for _, idx := range looseAtRe.FindAllStringSubmatchIndex(text, -1) {
		if len(idx) < 4 {
			continue
		}
		start, chip := idx[0], text[idx[2]:idx[3]]
		if !atMentionBoundary(text, start) {
			continue
		}
		out = appendMention(out, seen, chip, worktree, true)
	}
	return out
}

// wrapAttachedFileMentions prepends an attached-files
// system_instruction that maps absolute paths → short @chips, and leaves the
// body with those short @chips (no repeated abs paths). No-op with no mentions.
func wrapAttachedFileMentions(text, worktree string) string {
	mentions := collectFileMentions(text, worktree)
	if len(mentions) == 0 {
		return text
	}
	var files, dirs []fileMention
	for _, m := range mentions {
		if m.dir {
			dirs = append(dirs, m)
		} else {
			files = append(files, m)
		}
	}
	var b strings.Builder
	b.WriteString("<system_instruction>\n")
	if len(files) > 0 {
		b.WriteString("The user has attached these files. Read them before proceeding.\n")
		for _, m := range files {
			lineNote := mentionFocusNote(m)
			if info, err := os.Stat(m.abs); err == nil {
				fmt.Fprintf(&b, "- %s as @%s (%s)%s\n", m.abs, m.chip, formatAttachSize(info.Size()), lineNote)
			} else {
				fmt.Fprintf(&b, "- %s as @%s%s\n", m.abs, m.chip, lineNote)
			}
		}
	}
	if len(dirs) > 0 {
		b.WriteString("The user pointed at these directories. Explore them as needed; do not dump the whole tree.\n")
		for _, m := range dirs {
			fmt.Fprintf(&b, "- %s as @%s\n", m.abs, m.chip)
		}
	}
	b.WriteString("</system_instruction>\n\n")
	b.WriteString(text)
	return b.String()
}

func mentionFocusNote(m fileMention) string {
	if m.startLine <= 0 {
		return ""
	}
	if m.startCol > 0 && m.endCol > 0 {
		if m.endLine > m.startLine {
			return fmt.Sprintf(" — focus on line %d column %d through line %d column %d", m.startLine, m.startCol, m.endLine, m.endCol)
		}
		if m.endCol == m.startCol {
			return fmt.Sprintf(" — focus on line %d, column %d", m.startLine, m.startCol)
		}
		return fmt.Sprintf(" — focus on line %d, columns %d–%d", m.startLine, m.startCol, m.endCol)
	}
	if m.endLine > m.startLine {
		return fmt.Sprintf(" — focus on lines %d–%d", m.startLine, m.endLine)
	}
	return fmt.Sprintf(" — focus on line %d", m.startLine)
}

func formatAttachSize(n int64) string {
	switch {
	case n < 1024:
		return fmt.Sprintf("%d B", n)
	case n < 1024*1024:
		v := float64(n) / 1024
		if v < 10 {
			return fmt.Sprintf("%.1f KB", v)
		}
		return fmt.Sprintf("%.0f KB", v)
	default:
		v := float64(n) / (1024 * 1024)
		if v < 10 {
			return fmt.Sprintf("%.1f MB", v)
		}
		return fmt.Sprintf("%.0f MB", v)
	}
}
