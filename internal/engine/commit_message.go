package engine

import (
	"context"
	"fmt"
	"path"
	"sort"
	"strings"
	"time"
	"unicode/utf8"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

// commitMessageTimeout bounds the cheap one-shot commit-message turn.
const commitMessageTimeout = 45 * time.Second

// commitMessageDiffCap limits staged-diff bytes sent to the title model.
const commitMessageDiffCap = 12000

// commitMessageMaxLen caps the sanitized commit message (subject + optional body).
const commitMessageMaxLen = 500

const commitMessageSystemPrompt = "You write git commit messages from a staged diff. " +
	"Reply with ONLY the commit message: a short subject line (at most ~72 characters), " +
	"optionally followed by a blank line and a brief body. " +
	"Match the style of the recent commit subjects when provided. " +
	"No quotes, no markdown fences, no explanation, no preamble. " +
	"Do not call tools, read files, or explore the codebase — use the diff text alone."

// GenerateCommitMessage runs a cheap one-shot turn on the workspace title model
// and returns a sanitized commit message for the staged diff. recentSubjects
// (newest first) are style hints; may be empty.
func (e *Engine) GenerateCommitMessage(ctx context.Context, workspaceID, worktree, diff string, recentSubjects []string) (string, error) {
	diff = strings.TrimSpace(diff)
	if diff == "" {
		return "", fmt.Errorf("stage changes to generate a commit message")
	}
	if ctx == nil {
		ctx = context.Background()
	}
	ctx, cancel := context.WithTimeout(ctx, commitMessageTimeout)
	defer cancel()

	set := e.settings()
	ref := set.WorkspaceTitleModel()
	flags := set.WorkspaceTitleFlags()
	if len(flags) == 0 && ref.Provider != domain.ProviderCursor {
		flags = map[string]string{"effort": "low"}
	}

	prompt := buildCommitMessagePrompt(diff, recentSubjects)
	noTools := ""
	spec := AgentSpec{
		// WorkspaceID attributes the turn; Role left empty so attachMirror /
		// sessionBroadcast are no-ops — this turn must not pollute the chat
		// transcript or Activity feed.
		WorkspaceID:  workspaceID,
		Model:        ref.String(),
		Flags:        flags,
		SystemPrompt: commitMessageSystemPrompt,
		Prompt:       prompt,
		Worktree:     worktree,
		// Exempt from the machine-global permission mode: --tools "" leaves this
		// turn with nothing to call, so there is no permission to grant and
		// nobody to ask (it runs without an MCP endpoint, unattended).
		PermissionMode: domain.PermissionModeDeny,
		Tools:          &noTools,
		MaxTurns:       1,
		SkipThinking:   true,
	}
	if ref.Provider == domain.ProviderCursor {
		spec.AllowedTools = []string{"Read", "Glob", "Grep"}
	}
	res, err := e.runAgent(ctx, spec, nil)
	e.publishResource()
	if err != nil {
		return "", fmt.Errorf("generate commit message: %w", err)
	}
	if res.IsError {
		return "", fmt.Errorf("generate commit message failed")
	}
	msg := sanitizeCommitMessage(res.Text, commitMessageMaxLen)
	if msg == "" {
		return "", fmt.Errorf("empty commit message from model")
	}
	return msg, nil
}

const prDescriptionTimeout = 45 * time.Second

const prDescriptionSystemPrompt = "You write pull request titles and descriptions from a branch diff. " +
	"Describe ALL changes versus the base branch, not one commit or chat turn. " +
	"Reply with ONLY a title line (at most 80 characters), a blank line, and a markdown body. " +
	"Keep the body under five sentences unless the change is large. " +
	"No quotes, no markdown fences around the whole reply, no greeting, no tools."

// generatePRDescription is the /open-pr one-shot: a workspace-wide title + body
// from the merge-base diff. Empty diff or an empty model reply is an error so
// callers can fall back to a factual template.
func (e *Engine) generatePRDescription(ctx context.Context, workspaceID, worktree, diff string, commits []string) (title, body string, err error) {
	diff = strings.TrimSpace(diff)
	if diff == "" {
		return "", "", fmt.Errorf("empty diff")
	}
	if ctx == nil {
		ctx = context.Background()
	}
	ctx, cancel := context.WithTimeout(ctx, prDescriptionTimeout)
	defer cancel()

	set := e.settings()
	ref := set.WorkspaceTitleModel()
	flags := set.WorkspaceTitleFlags()
	if len(flags) == 0 && ref.Provider != domain.ProviderCursor {
		flags = map[string]string{"effort": "low"}
	}
	noTools := ""
	spec := AgentSpec{
		WorkspaceID:    workspaceID,
		Model:          ref.String(),
		Flags:          flags,
		SystemPrompt:   prDescriptionSystemPrompt,
		Prompt:         buildPRDescriptionPrompt(diff, commits),
		Worktree:       worktree,
		PermissionMode: domain.PermissionModeDeny,
		Tools:          &noTools,
		MaxTurns:       1,
		SkipThinking:   true,
	}
	if ref.Provider == domain.ProviderCursor {
		spec.AllowedTools = []string{"Read", "Glob", "Grep"}
	}
	res, err := e.runAgent(ctx, spec, nil)
	e.publishResource()
	if err != nil {
		return "", "", fmt.Errorf("generate PR description: %w", err)
	}
	if res.IsError {
		return "", "", fmt.Errorf("generate PR description failed")
	}
	title, body = parsePRDescription(res.Text)
	if title == "" && body == "" {
		return "", "", fmt.Errorf("empty PR description from model")
	}
	return title, body, nil
}

func buildPRDescriptionPrompt(diff string, commits []string) string {
	var b strings.Builder
	b.WriteString("Write a pull request title and description covering every change versus the base.\n\n")
	if len(commits) > 0 {
		b.WriteString("Commit subjects (newest first):\n")
		for _, s := range commits {
			b.WriteString("- ")
			b.WriteString(s)
			b.WriteByte('\n')
		}
		b.WriteByte('\n')
	}
	b.WriteString("Diff:\n\n")
	b.WriteString(capDiffCoveringFiles(diff, commitMessageDiffCap))
	return b.String()
}

func parsePRDescription(raw string) (title, body string) {
	s := unwrapPRReply(raw)
	if s == "" {
		return "", ""
	}
	titleLine, rest, found := strings.Cut(s, "\n")
	title = hardTruncateRunes(singleLine(unwrapLineQuotes(titleLine)), 80)
	if found {
		body = strings.TrimSpace(rest)
		if body != "" {
			body = capGitHubPRBody(body)
		}
	}
	return title, body
}

// unwrapPRReply strips a matched wrapper around the entire model reply (an outer
// markdown fence, or matching quotes). Inner fences and trailing inline code
// stay intact for the PR body.
func unwrapPRReply(raw string) string {
	s := strings.TrimSpace(raw)
	if s == "" {
		return ""
	}
	if strings.HasPrefix(s, "```") && strings.HasSuffix(s, "```") && len(s) > 6 {
		inner := strings.TrimSpace(s[3 : len(s)-3])
		if i := strings.IndexByte(inner, '\n'); i >= 0 {
			tag := strings.TrimSpace(inner[:i])
			if tag != "" && !strings.ContainsAny(tag, " \t") {
				inner = strings.TrimSpace(inner[i+1:])
			}
		}
		s = inner
	}
	return unwrapLineQuotes(s)
}

func unwrapLineQuotes(s string) string {
	s = strings.TrimSpace(s)
	if n := len(s); n >= 2 {
		if q := s[0]; (q == '"' || q == '\'') && s[n-1] == q && strings.Count(s, string(q)) == 2 {
			return strings.TrimSpace(s[1 : n-1])
		}
	}
	return s
}

// capDiffCoveringFiles bounds a unified diff so every file still appears.
func capDiffCoveringFiles(diff string, capBytes int) string {
	if capBytes <= 0 || len(diff) <= capBytes {
		return diff
	}
	parts := splitUnifiedDiffFiles(diff)
	if len(parts) <= 1 {
		return truncateDiff(diff, capBytes)
	}
	per := capBytes / len(parts)
	if per < 64 {
		per = 64
	}
	var b strings.Builder
	for i, p := range parts {
		if i > 0 {
			b.WriteByte('\n')
		}
		b.WriteString(capOneDiffFile(p, per))
	}
	return b.String()
}

func splitUnifiedDiffFiles(diff string) []string {
	if diff == "" {
		return nil
	}
	var parts []string
	start := 0
	for {
		rel := strings.Index(diff[start:], "\ndiff --git ")
		if rel < 0 {
			parts = append(parts, diff[start:])
			return parts
		}
		parts = append(parts, diff[start:start+rel])
		start += rel + 1
	}
}

func capOneDiffFile(p string, per int) string {
	p = strings.TrimRight(p, "\n")
	if len(p) <= per {
		return p
	}
	cut := per
	if i := strings.IndexByte(p, '\n'); i >= 0 && cut < i+1 {
		cut = i + 1
	}
	if cut >= len(p) {
		return p
	}
	for cut > 0 && !utf8.RuneStart(p[cut]) {
		cut--
	}
	return strings.TrimRight(p[:cut], "\n") + "\n[truncated]"
}

func truncateDiff(diff string, capBytes int) string {
	if len(diff) <= capBytes {
		return diff
	}
	cut := capBytes
	for cut > 0 && !utf8.RuneStart(diff[cut]) {
		cut--
	}
	return strings.TrimRight(diff[:cut], "\n") + "\n\n[diff truncated]"
}

func buildCommitMessagePrompt(diff string, recentSubjects []string) string {
	var b strings.Builder
	if len(recentSubjects) > 0 {
		b.WriteString("Recent commit subjects (newest first):\n")
		for _, s := range recentSubjects {
			b.WriteString("- ")
			b.WriteString(s)
			b.WriteByte('\n')
		}
		b.WriteByte('\n')
	}
	b.WriteString("Staged diff:\n\n")
	if len(diff) > commitMessageDiffCap {
		diff = diff[:commitMessageDiffCap] + "\n\n[diff truncated]"
	}
	b.WriteString(diff)
	return b.String()
}

// sanitizeCommitMessage cleans a model reply into a usable commit message.
// Unlike sanitizeTitle, multi-line bodies are kept.
func sanitizeCommitMessage(raw string, maxLen int) string {
	s := strings.TrimSpace(raw)
	if s == "" {
		return ""
	}
	if strings.HasPrefix(s, "```") {
		s = strings.TrimPrefix(s, "```")
		// Drop an optional language tag on the first line.
		if i := strings.IndexByte(s, '\n'); i >= 0 {
			s = s[i+1:]
		}
		if j := strings.LastIndex(s, "```"); j >= 0 {
			s = s[:j]
		}
		s = strings.TrimSpace(s)
	}
	s = strings.Trim(s, "\"'`")
	s = strings.TrimSpace(s)
	if s == "" {
		return ""
	}
	if maxLen > 0 && len(s) > maxLen {
		s = truncate(s, maxLen)
	}
	return s
}

// loopCommitSummaryMax caps the engine commit subject.
const loopCommitSummaryMax = 72

const loopCommitFileHintMax = 3

// loopCommitMessage is the engine commit subject for one coding iteration.
// Prefer the Coder's mcp__recur__done summary (what this turn actually did).
// Else a short goal headline (Linear Title / first line) plus this iteration's
// changed files — never the expanded ticket dump, never a role/iteration prefix.
func loopCommitMessage(doneSummary, goal string, files []string) string {
	s := sanitizeLoopCommitSummary(doneSummary)
	fromDone := s != ""
	if !fromDone {
		s = commitSummaryFromGoal(goal)
	}
	if s == "" {
		s = "changes"
	}
	if !fromDone {
		s = appendFileHint(s, files)
	}
	return s
}

func appendFileHint(summary string, files []string) string {
	hint := fileHint(files)
	if hint == "" {
		return summary
	}
	overhead := utf8.RuneCountInString(" ()") + utf8.RuneCountInString(hint)
	room := loopCommitSummaryMax - overhead
	if room < 40 && utf8.RuneCountInString(summary) > room {
		return summary
	}
	summary = hardTruncateRunes(summary, room)
	return summary + " (" + hint + ")"
}

func sanitizeLoopCommitSummary(s string) string {
	s = strings.TrimSpace(s)
	if s == "" {
		return ""
	}
	if i := strings.IndexAny(s, "\n\r"); i >= 0 {
		s = strings.TrimSpace(s[:i])
	}
	s = strings.TrimSpace(strings.TrimLeft(s, "#"))
	s = strings.Trim(s, "`\"'")
	s = strings.TrimSpace(s)
	if s == "" {
		return ""
	}
	return hardTruncateRunes(s, loopCommitSummaryMax)
}

func commitSummaryFromGoal(goal string) string {
	ident, title := linearIssueMeta(goal)
	if title != "" {
		if ident != "" {
			return sanitizeLoopCommitSummary(ident + ": " + title)
		}
		return sanitizeLoopCommitSummary(title)
	}
	return sanitizeLoopCommitSummary(firstLine(goal))
}

func linearIssueMeta(goal string) (ident, title string) {
	for _, line := range strings.Split(goal, "\n") {
		line = strings.TrimSpace(line)
		if ident == "" {
			if rest, ok := strings.CutPrefix(line, "Identifier:"); ok {
				ident = strings.TrimSpace(rest)
			}
		}
		if title == "" {
			if rest, ok := strings.CutPrefix(line, "Title:"); ok {
				title = strings.TrimSpace(rest)
			}
		}
		if ident != "" && title != "" {
			return ident, title
		}
	}
	return ident, title
}

func statusPaths(ctx context.Context, worktree string) []string {
	st := gitpkg.StatusMap(ctx, worktree)
	if len(st) == 0 {
		return nil
	}
	paths := make([]string, 0, len(st))
	for p := range st {
		if strings.TrimSpace(p) != "" {
			paths = append(paths, p)
		}
	}
	sort.Strings(paths)
	return paths
}

func fileHint(paths []string) string {
	bases := uniqueBasenames(paths)
	if len(bases) == 0 {
		return ""
	}
	extra := 0
	if len(bases) > loopCommitFileHintMax {
		extra = len(bases) - loopCommitFileHintMax
		bases = bases[:loopCommitFileHintMax]
	}
	s := strings.Join(bases, ", ")
	if extra > 0 {
		s += fmt.Sprintf(" +%d", extra)
	}
	return s
}

func uniqueBasenames(paths []string) []string {
	seen := map[string]bool{}
	var out []string
	for _, p := range paths {
		b := path.Base(strings.ReplaceAll(strings.TrimSpace(p), "\\", "/"))
		if b == "" || b == "." || b == "/" || seen[b] {
			continue
		}
		seen[b] = true
		out = append(out, b)
	}
	sort.Strings(out)
	return out
}
