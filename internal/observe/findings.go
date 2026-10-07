// Package observe implements continuous / on-demand code observatory analysis:
// checks, metrics, impact, drift, and projecting reviewer findings onto diffs.
package observe

import (
	"fmt"
	"html"
	"strings"

	"recur/internal/domain"
)

// ProjectFindingsOntoDiff attaches risk-flag ExtraHTML for open findings onto
// matching new-side lines. Existing ExtraHTML is preserved (appended after).
func ProjectFindingsOntoDiff(files []domain.FileDiff, findings []domain.Finding) bool {
	byFile := map[string][]domain.Finding{}
	for _, f := range findings {
		if f.Status != "" && f.Status != "open" {
			continue
		}
		if f.File == "" || f.Line <= 0 {
			continue
		}
		path := strings.TrimPrefix(filepathToSlash(f.File), "./")
		byFile[path] = append(byFile[path], f)
	}
	if len(byFile) == 0 {
		return false
	}
	changed := false
	for i := range files {
		path := filepathToSlash(files[i].Path)
		fs := byFile[path]
		if len(fs) == 0 {
			// Also try basename match when agents report short paths.
			base := path
			if j := strings.LastIndexByte(path, '/'); j >= 0 {
				base = path[j+1:]
			}
			for k, list := range byFile {
				if strings.HasSuffix(k, "/"+base) || k == base {
					fs = list
					break
				}
			}
		}
		if len(fs) == 0 {
			continue
		}
		for j := range files[i].Lines {
			l := &files[i].Lines[j]
			if l.Type != "add" && l.Type != "ctx" {
				continue
			}
			lineNo := parseLine(l.New)
			if lineNo <= 0 {
				continue
			}
			var matched []domain.Finding
			for _, f := range fs {
				if f.CoversLine(lineNo) {
					matched = append(matched, f)
				}
			}
			if len(matched) == 0 {
				continue
			}
			flag := FindingFlagHTML(matched)
			if strings.Contains(l.ExtraHTML, `class="flag"`) && strings.Contains(l.ExtraHTML, html.EscapeString(matched[0].Message)) {
				continue
			}
			l.ExtraHTML = flag + l.ExtraHTML
			changed = true
		}
	}
	return changed
}

// FindingFlagHTML renders the PRD risk-flag block for one or more findings.
func FindingFlagHTML(findings []domain.Finding) string {
	if len(findings) == 0 {
		return ""
	}
	var b strings.Builder
	for _, f := range findings {
		sev := string(f.Severity)
		if sev == "" {
			sev = "medium"
		}
		title := "Reviewer flagged this region"
		if sev == "high" || sev == "critical" {
			title = "Reviewer flagged this region for human eyes"
		}
		fmt.Fprintf(&b,
			`<div class="flag" data-sev="%s"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4-4" stroke-linecap="round"/></svg><div><div class="ft">%s · %s</div><div class="fm">%s</div></div></div>`,
			html.EscapeString(sev),
			html.EscapeString(title),
			html.EscapeString(sev),
			html.EscapeString(f.Message),
		)
	}
	return b.String()
}

func parseLine(s string) int {
	s = strings.TrimSpace(s)
	if s == "" {
		return 0
	}
	n := 0
	for _, c := range s {
		if c < '0' || c > '9' {
			break
		}
		n = n*10 + int(c-'0')
	}
	return n
}

func filepathToSlash(p string) string {
	return strings.ReplaceAll(p, "\\", "/")
}
