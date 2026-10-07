package engine

import (
	"context"
	"fmt"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	"recur/internal/code"
	"recur/internal/domain"
)

// skillSlashRe finds `/skill-name` candidates. Go's RE2 has no lookaround, so
// boundary checks happen in collectSkillMentions (keep semantics in sync with
// frontend SKILL_SLASH_RE in frontend/src/lib/agentchat.ts).
var skillSlashRe = regexp.MustCompile(`/([a-z][a-z0-9-]*)`)

const (
	maxActivatedSkills   = 8
	maxSkillBodyBytes    = 100_000
	skillsCatalogTimeout = 2 * time.Second
	skillActivateTimeout = 2 * time.Second
)

// collectSkillMentions returns unique `/skill-name` tokens in first-seen order.
func collectSkillMentions(text string) []string {
	if text == "" || !strings.Contains(text, "/") {
		return nil
	}
	seen := map[string]bool{}
	var out []string
	for _, idx := range skillSlashRe.FindAllStringSubmatchIndex(text, -1) {
		// idx: full start/end, name start/end
		if len(idx) < 4 {
			continue
		}
		start, end, name := idx[0], idx[1], text[idx[2]:idx[3]]
		if name == "" || seen[name] {
			continue
		}
		if start > 0 {
			prev := text[start-1]
			if prev != ' ' && prev != '\t' && prev != '\n' && prev != '\r' {
				continue // e.g. /Users/… or path/skill
			}
		}
		if end < len(text) {
			next := text[end]
			if isSkillNameContinue(next) {
				continue
			}
		}
		seen[name] = true
		out = append(out, name)
	}
	return out
}

func isSkillNameContinue(b byte) bool {
	return b == '-' || b == '/' || b == '_' ||
		(b >= 'a' && b <= 'z') || (b >= 'A' && b <= 'Z') || (b >= '0' && b <= '9')
}

// wrapActivatedSkills host-activates `/skill-name` mentions (Agent Skills tier 2):
// loads each SKILL.md body and prepends structured <skill_content> blocks so
// any provider receives instructions without relying on native slash expansion.
// Successfully activated slash tokens are stripped from the user body. Claude
// Code also gets --disable-slash-commands / --disallowedTools Skill so its
// native Skill tool cannot load them a second time.
// enabled reports whether a skill may be host-activated (nil → all allowed).
func wrapActivatedSkills(text, worktree string, enabled func(string) bool) string {
	names := collectSkillMentions(text)
	if len(names) == 0 {
		return text
	}
	if len(names) > maxActivatedSkills {
		names = names[:maxActivatedSkills]
	}
	ctx, cancel := context.WithTimeout(context.Background(), skillActivateTimeout)
	defer cancel()
	all, err := code.ListSkills(ctx, worktree)
	if err != nil || len(all) == 0 {
		return text
	}
	byName := make(map[string]domain.AgentSkill, len(all))
	for _, s := range all {
		byName[s.Name] = s
	}

	var blocks []string
	var activated []string
	var disabled []string
	for _, name := range names {
		sk, ok := byName[name]
		if !ok || sk.Path == "" || !sk.UserInvocable {
			// user-invocable: false → model activate_skill only, not user `/`.
			continue
		}
		if enabled != nil && !enabled(name) {
			// Still strip the token so Claude Code can't natively expand it.
			disabled = append(disabled, name)
			continue
		}
		body := code.ReadSkillBody(sk.Path)
		if strings.TrimSpace(body) == "" {
			continue
		}
		blocks = append(blocks, formatSkillContent(name, sk.Path, body))
		activated = append(activated, name)
	}
	if len(blocks) == 0 && len(disabled) == 0 {
		return text
	}

	strip := append(append([]string{}, activated...), disabled...)
	body := stripActivatedSkillMentions(text, strip)
	var out strings.Builder
	out.WriteString("<system_instruction>\n")
	if len(blocks) > 0 {
		out.WriteString("The user activated skill(s) via /name. Follow each skill's instructions below.\n")
		out.WriteString("Do not search for these skills or re-load them — their content is already included.\n")
	}
	if len(disabled) > 0 {
		out.WriteString("These skill mentions were ignored because they are disabled in Settings/Project: ")
		out.WriteString(strings.Join(disabled, ", "))
		out.WriteString(".\nDo not search for or load these skills.\n")
	}
	out.WriteString("</system_instruction>")
	for _, block := range blocks {
		out.WriteString("\n\n")
		out.WriteString(block)
	}
	if strings.TrimSpace(body) != "" {
		out.WriteString("\n\n")
		out.WriteString(body)
	}
	return out.String()
}

// formatSkillContent wraps a skill body for host slash activation or
// mcp__recur__activate_skill (same shape either way).
func formatSkillContent(name, skillMDPath, body string) string {
	if len(body) > maxSkillBodyBytes {
		body = body[:maxSkillBodyBytes] + "\n\n…(skill truncated)…"
	}
	dir := filepath.Dir(skillMDPath)
	var b strings.Builder
	fmt.Fprintf(&b, "<skill_content name=%q>\n", name)
	b.WriteString(body)
	b.WriteByte('\n')
	fmt.Fprintf(&b, "\nSkill directory: %s\n", dir)
	b.WriteString("Relative paths in this skill are relative to the skill directory.\n")
	if res := code.ListSkillResources(skillMDPath, 40); len(res) > 0 {
		b.WriteString("\n<skill_resources>\n")
		for _, r := range res {
			fmt.Fprintf(&b, "  <file>%s</file>\n", r)
		}
		b.WriteString("</skill_resources>\n")
	}
	b.WriteString("</skill_content>")
	return b.String()
}

func stripActivatedSkillMentions(text string, names []string) string {
	if len(names) == 0 {
		return text
	}
	set := map[string]bool{}
	for _, n := range names {
		set[n] = true
	}
	var b strings.Builder
	last := 0
	for _, idx := range skillSlashRe.FindAllStringSubmatchIndex(text, -1) {
		if len(idx) < 4 {
			continue
		}
		start, end, name := idx[0], idx[1], text[idx[2]:idx[3]]
		if !set[name] {
			continue
		}
		if start > 0 {
			prev := text[start-1]
			if prev != ' ' && prev != '\t' && prev != '\n' && prev != '\r' {
				continue
			}
		}
		if end < len(text) && isSkillNameContinue(text[end]) {
			continue
		}
		// Eat one adjacent space/tab so "foo /skill bar" → "foo bar", without
		// collapsing indentation elsewhere in the message. Do not step before
		// last: "/a /b" at end-of-string shares one space, and eating it as
		// both the first trailer and the second leader panics on [last:cutStart].
		cutStart, cutEnd := start, end
		if cutEnd < len(text) && (text[cutEnd] == ' ' || text[cutEnd] == '\t') {
			cutEnd++
		} else if cutStart > last && (text[cutStart-1] == ' ' || text[cutStart-1] == '\t') {
			cutStart--
		}
		b.WriteString(text[last:cutStart])
		last = cutEnd
	}
	b.WriteString(text[last:])
	return strings.TrimSpace(b.String())
}

func applySkillGuidance(spec *AgentSpec) {
	has := false
	for _, t := range spec.AllowedTools {
		if t == mcpListSkills {
			has = true
			break
		}
	}
	if !has {
		return
	}
	if strings.Contains(spec.SystemPrompt, "mcp__recur__list_skills") ||
		strings.Contains(spec.AppendSystemPrompt, "mcp__recur__list_skills") {
		return
	}
	spec.AppendSystemPrompt = joinAppendPrompt(spec.AppendSystemPrompt, skillsToolsAppendPrompt)
}

// applyCommentHygiene overlays commentHygieneAppendPrompt on turns that can
// edit files. Empty allowlists (title/commit one-shots) stay off.
func applyCommentHygiene(spec *AgentSpec) {
	if !allowlistCanEdit(spec.AllowedTools) {
		return
	}
	if strings.Contains(spec.SystemPrompt, commentHygieneNeedle) ||
		strings.Contains(spec.AppendSystemPrompt, commentHygieneNeedle) {
		return
	}
	spec.AppendSystemPrompt = joinAppendPrompt(spec.AppendSystemPrompt, commentHygieneAppendPrompt)
}

func allowlistCanEdit(tools []string) bool {
	for _, t := range tools {
		switch t {
		case "Edit", "Write", "NotebookEdit":
			return true
		}
	}
	return false
}

func joinAppendPrompt(base, extra string) string {
	base = strings.TrimSpace(base)
	extra = strings.TrimSpace(extra)
	switch {
	case base == "":
		return extra
	case extra == "":
		return base
	default:
		return base + "\n\n" + extra
	}
}
