package engine

import (
	"encoding/json"
	"fmt"
	"strconv"
	"strings"

	"recur/internal/domain"
)

// Looking-at surfaces the operator and agent share. Prompt actions (/review)
// and every chat turn should target this subject, not "whatever git diff is
// in cwd".
const (
	ChatSurfaceWorkspace = "workspace"
	ChatSurfaceLocalPR   = "local_pr"
	ChatSurfaceGitHubPR  = "github_pr"
)

const (
	chatSurfaceOpen  = "⟦recur-surface⟧"
	chatSurfaceClose = "⟦/recur-surface⟧"
)

// chatSurface is the operator's current view: a Recur workspace, a Local PR,
// or a GitHub PR. The frontend stamps this onto the wire; the engine also
// infers it from a PR chat host (HostPRID).
type chatSurface struct {
	Surface  string `json:"surface"`
	PRID     string `json:"prId,omitempty"`
	PRNumber int    `json:"prNumber,omitempty"`
	PRSource string `json:"prSource,omitempty"`
	PRURL    string `json:"prUrl,omitempty"`
	PRTitle  string `json:"prTitle,omitempty"`
	PRBase   string `json:"prBase,omitempty"`
	PRBranch string `json:"prBranch,omitempty"`
	Host     bool   `json:"host,omitempty"`
}

func normalizeChatSurfaceKind(s string) string {
	switch strings.TrimSpace(strings.ToLower(s)) {
	case ChatSurfaceLocalPR, "local-pr", "local":
		return ChatSurfaceLocalPR
	case ChatSurfaceGitHubPR, "github-pr", "gh", "github":
		return ChatSurfaceGitHubPR
	default:
		return ChatSurfaceWorkspace
	}
}

// peelChatSurface strips a leading looking-at marker the UI prepends to the
// wire (not shown in the composer bubble). Invalid markers are left in place.
func peelChatSurface(text string) (rest string, surf chatSurface, ok bool) {
	s := strings.TrimLeft(text, " \t\r\n")
	if !strings.HasPrefix(s, chatSurfaceOpen) {
		return text, chatSurface{}, false
	}
	s = strings.TrimPrefix(s, chatSurfaceOpen)
	i := strings.Index(s, chatSurfaceClose)
	if i < 0 {
		return text, chatSurface{}, false
	}
	raw := strings.TrimSpace(s[:i])
	rest = strings.TrimLeft(s[i+len(chatSurfaceClose):], " \t\r\n")
	if err := json.Unmarshal([]byte(raw), &surf); err != nil {
		return text, chatSurface{}, false
	}
	if strings.TrimSpace(surf.Surface) == "" {
		return text, chatSurface{}, false
	}
	surf.Surface = normalizeChatSurfaceKind(surf.Surface)
	return rest, surf, true
}

func (e *Engine) resolveChatSurface(ws domain.Workspace, wired chatSurface, hasWire bool) chatSurface {
	out := wired
	if ws.IsPRChatHost() {
		out.Host = true
		out.Surface = ChatSurfaceGitHubPR
		if strings.TrimSpace(out.PRID) == "" {
			out.PRID = strings.TrimSpace(ws.HostPRID)
		}
	} else if !hasWire || strings.TrimSpace(out.Surface) == "" {
		out.Surface = ChatSurfaceWorkspace
	} else {
		out.Surface = normalizeChatSurfaceKind(out.Surface)
	}
	if pr, ok := e.surfacePR(ws, out); ok {
		out = mergePRIntoSurface(out, pr)
	}
	return out
}

func (e *Engine) surfacePR(ws domain.Workspace, surf chatSurface) (domain.PR, bool) {
	if id := strings.TrimSpace(surf.PRID); id != "" {
		if pr, ok := e.Store.PR(id); ok {
			return pr, true
		}
	}
	if ws.IsPRChatHost() {
		if pr, ok := e.Store.PR(strings.TrimSpace(ws.HostPRID)); ok {
			return pr, true
		}
	}
	if pr, ok := e.Store.OpenPRByWorkspace(ws.ID); ok {
		return pr, true
	}
	if pr, ok := e.Store.PRByWorkspace(ws.ID); ok && pr.State != "Merged" && pr.Group != "merged" {
		return pr, true
	}
	return domain.PR{}, false
}

func mergePRIntoSurface(s chatSurface, pr domain.PR) chatSurface {
	if strings.TrimSpace(s.PRID) == "" {
		s.PRID = pr.ID
	}
	if s.PRNumber == 0 {
		s.PRNumber = pr.Number
	}
	if strings.TrimSpace(s.PRSource) == "" {
		s.PRSource = string(pr.Source)
	}
	if strings.TrimSpace(s.PRURL) == "" {
		s.PRURL = pr.URL
	}
	if strings.TrimSpace(s.PRTitle) == "" {
		s.PRTitle = pr.Title
	}
	if strings.TrimSpace(s.PRBase) == "" {
		s.PRBase = pr.Base
	}
	if strings.TrimSpace(s.PRBranch) == "" {
		s.PRBranch = pr.Branch
	}
	return s
}

func subjectAppendPrompt(s chatSurface) string {
	switch s.Surface {
	case ChatSurfaceGitHubPR:
		return githubPRSubjectPrompt(s)
	case ChatSurfaceLocalPR:
		return localPRSubjectPrompt(s)
	default:
		return workspaceSubjectPrompt(s)
	}
}

func githubPRSubjectPrompt(s chatSurface) string {
	label := githubPRLabel(s)
	var b strings.Builder
	b.WriteString("The operator is looking at ")
	b.WriteString(label)
	if t := strings.TrimSpace(s.PRTitle); t != "" {
		b.WriteString(` titled "`)
		b.WriteString(t)
		b.WriteString(`"`)
	}
	head := strings.TrimSpace(s.PRBranch)
	base := strings.TrimSpace(s.PRBase)
	if head != "" || base != "" {
		b.WriteString(" (")
		if head != "" {
			b.WriteString("head ")
			b.WriteString(head)
		}
		if head != "" && base != "" {
			b.WriteString(" into ")
		}
		if base != "" {
			b.WriteString(base)
		}
		b.WriteString(")")
	}
	b.WriteString(". Questions, plans, and reviews are about this pull request — use `gh pr view`, `gh pr diff`, and existing review threads. Do not treat `git diff` in the current directory as the PR unless cwd is actually checked out on the PR head.")
	if s.Host {
		b.WriteString(" This chat is attached to the PR, not a Recur worktree of its head; cwd is the project's working copy and may be a different branch.")
	} else {
		b.WriteString(" This chat runs in the Recur workspace for that PR's head; uncommitted local work is not on GitHub until pushed.")
	}
	return b.String()
}

func localPRSubjectPrompt(s chatSurface) string {
	n := s.PRNumber
	title := strings.TrimSpace(s.PRTitle)
	var b strings.Builder
	b.WriteString("The operator is looking at Local PR")
	if n > 0 {
		b.WriteString(" #")
		b.WriteString(strconv.Itoa(n))
	}
	if title != "" {
		b.WriteString(` titled "`)
		b.WriteString(title)
		b.WriteString(`"`)
	}
	b.WriteString(". This is Recur's worktree-diff review, not a GitHub pull request. Questions and reviews default to this PR's changes versus its base.")
	return b.String()
}

func workspaceSubjectPrompt(s chatSurface) string {
	var b strings.Builder
	b.WriteString("The operator is looking at this Recur workspace (the git worktree in cwd). Questions and reviews default to this worktree versus its merge base.")
	if s.PRNumber <= 0 && strings.TrimSpace(s.PRID) == "" {
		return b.String()
	}
	if s.PRSource == string(domain.SourceGitHub) {
		b.WriteString(" ")
		b.WriteString(githubPRLabel(s))
		b.WriteString(" is open for this branch — that is a different surface; only treat it as the subject when they refer to the pull request.")
	} else if s.PRSource == string(domain.SourceLocal) || s.PRNumber > 0 {
		b.WriteString(" Local PR")
		if s.PRNumber > 0 {
			b.WriteString(" #")
			b.WriteString(strconv.Itoa(s.PRNumber))
		}
		b.WriteString(" is open for this workspace — only treat it as the subject when they refer to the pull request.")
	}
	return b.String()
}

func githubPRLabel(s chatSurface) string {
	n := s.PRNumber
	url := strings.TrimSpace(s.PRURL)
	switch {
	case n > 0 && url != "":
		return fmt.Sprintf("GitHub pull request #%d (%s)", n, url)
	case n > 0:
		return fmt.Sprintf("GitHub pull request #%d", n)
	case url != "":
		return "GitHub pull request (" + url + ")"
	default:
		return "this GitHub pull request"
	}
}
