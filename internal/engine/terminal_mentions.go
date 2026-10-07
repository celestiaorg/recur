package engine

import (
	"fmt"
	"regexp"
	"strings"
	"time"

	"recur/internal/term"
)

// terminalAtRe matches @terminal or @terminal:<id-or-title> in chat wire text.
// Qualifier is a non-space token (session uuid, prefix, or unique tab title).
// Keep in sync with frontend TERMINAL_AT_RE (frontend/src/lib/agentchat.ts).
var terminalAtRe = regexp.MustCompile(`@terminal(?::([^\s@]+))?`)

const (
	terminalWrapTailLines = term.DefaultTailLines
	maxTerminalSnapshots  = 4
)

func fenceTerminalSnapshot(body string) string {
	body = strings.ReplaceAll(body, "</system_instruction>", "< /system_instruction>")
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

type terminalMention struct {
	chip  string // token after @ (terminal or terminal:<qual>)
	qual  string // empty for bare @terminal
	start int
}

// collectTerminalMentions returns unique @terminal chips in first-seen order.
func collectTerminalMentions(text string) []terminalMention {
	if text == "" || !strings.Contains(text, "@terminal") {
		return nil
	}
	seen := map[string]bool{}
	var out []terminalMention
	for _, idx := range terminalAtRe.FindAllStringSubmatchIndex(text, -1) {
		if len(idx) < 2 {
			continue
		}
		start, end := idx[0], idx[1]
		if start > 0 {
			prev := text[start-1]
			if prev != ' ' && prev != '\t' && prev != '\n' && prev != '\r' {
				continue
			}
		}
		if end < len(text) {
			next := text[end]
			if next != ' ' && next != '\t' && next != '\n' && next != '\r' {
				continue
			}
		}
		qual := ""
		if len(idx) >= 4 && idx[2] >= 0 {
			qual = text[idx[2]:idx[3]]
		}
		chip := "terminal"
		if qual != "" {
			chip = "terminal:" + qual
		}
		if seen[chip] {
			continue
		}
		seen[chip] = true
		out = append(out, terminalMention{chip: chip, qual: qual, start: start})
	}
	return out
}

// wrapAttachedTerminalMentions prepends a snapshot of attached Observatory
// terminals (ANSI-stripped, last N lines). No-op with no mentions or no manager.
func wrapAttachedTerminalMentions(text, workspaceID string, mgr *term.Manager) string {
	return attachTerminalMentions(text, collectTerminalMentions(text), workspaceID, mgr)
}

func attachTerminalMentions(text string, mentions []terminalMention, workspaceID string, mgr *term.Manager) string {
	if len(mentions) == 0 || mgr == nil || workspaceID == "" {
		return text
	}
	sessions := mgr.List(workspaceID)
	active := mgr.ActiveID(workspaceID)

	var b strings.Builder
	b.WriteString("<system_instruction>\n")
	b.WriteString("The user attached Observatory terminal output. It is a snapshot of the live PTY ")
	b.WriteString("(ANSI stripped, last lines) — not a worktree file. Do not search the repo for these paths. ")
	b.WriteString("For a later snapshot or another tab, call mcp__recur__list_terminals / mcp__recur__read_terminal.\n")
	wrote := 0
	for _, m := range mentions {
		if wrote >= maxTerminalSnapshots {
			break
		}
		hits := resolveTerminalMentions(m, sessions, active)
		if len(hits) == 0 {
			fmt.Fprintf(&b, "\n### @%s\n(no matching terminal)\n", m.chip)
			wrote++
			continue
		}
		for _, info := range hits {
			if wrote >= maxTerminalSnapshots {
				break
			}
			body, err := mgr.ReadPlain(info.ID, terminalWrapTailLines)
			title := strings.TrimSpace(info.Title)
			if title == "" {
				title = "terminal"
			}
			state := terminalExitedNote(info)
			fmt.Fprintf(&b, "\n### @%s (%s, id %s%s)\n", m.chip, title, info.ID, state)
			if err != nil {
				fmt.Fprintf(&b, "(could not read: %s)\n", err.Error())
				wrote++
				continue
			}
			if strings.TrimSpace(body) == "" {
				b.WriteString("(idle — no recent output)\n")
				wrote++
				continue
			}
			b.WriteString(fenceTerminalSnapshot(body))
			wrote++
		}
	}
	if wrote == 0 {
		return text
	}
	b.WriteString("</system_instruction>\n\n")
	b.WriteString(text)
	return b.String()
}

func resolveTerminalMentions(m terminalMention, sessions []term.Info, activeID string) []term.Info {
	if len(sessions) == 0 {
		return nil
	}
	if m.qual == "" {
		if activeID != "" {
			for _, s := range sessions {
				if s.ID == activeID {
					return []term.Info{s}
				}
			}
		}
		return newestLiveOrLast(sessions)
	}
	qual := m.qual
	var prefix, title []term.Info
	for _, s := range sessions {
		if s.ID == qual {
			return []term.Info{s}
		}
		if strings.HasPrefix(s.ID, qual) {
			prefix = append(prefix, s)
		}
		if strings.EqualFold(s.Title, qual) {
			title = append(title, s)
		}
	}
	if len(prefix) == 1 {
		return prefix
	}
	if len(title) == 1 {
		return title
	}
	return nil
}

func newestLiveOrLast(sessions []term.Info) []term.Info {
	bestLive, bestAny := -1, 0
	for i, s := range sessions {
		if !createdAtBefore(s, sessions[bestAny]) {
			bestAny = i
		}
		if s.Exited {
			continue
		}
		if bestLive < 0 || !createdAtBefore(s, sessions[bestLive]) {
			bestLive = i
		}
	}
	if bestLive >= 0 {
		return []term.Info{sessions[bestLive]}
	}
	return []term.Info{sessions[bestAny]}
}

func createdAtBefore(a, b term.Info) bool {
	ta, errA := time.Parse(time.RFC3339Nano, a.CreatedAt)
	tb, errB := time.Parse(time.RFC3339Nano, b.CreatedAt)
	if errA != nil || errB != nil {
		return false
	}
	return ta.Before(tb)
}

func terminalExitedNote(info term.Info) string {
	if !info.Exited {
		return ""
	}
	code := 0
	if info.ExitCode != nil {
		code = *info.ExitCode
	}
	return fmt.Sprintf(", exited (%d)", code)
}
