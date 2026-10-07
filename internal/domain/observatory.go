package domain

// Observatory types — the code-observatory / ADE surface. Recur stays
// outcome-first: these types power navigation, impact, and metrics around loops
// and PRs, not a full IDE.

// TreeEntry is one node in a project/worktree file tree.
type TreeEntry struct {
	Name     string      `json:"name"`
	Path     string      `json:"path"` // repo-relative, forward slashes
	Dir      bool        `json:"dir"`
	Status   string      `json:"status,omitempty"`  // m/a/d/u — git worktree status (D46)
	Ignored  bool        `json:"ignored,omitempty"` // git check-ignore (D-explorer-ignore-mute)
	Children []TreeEntry `json:"children,omitempty"`
}

// AgentSkill is one discovered Agent Skill (SKILL.md) for the composer `/`
// menu and host progressive disclosure (catalog + slash activation).
// Scope is "workspace" (worktree) or "user" (home dirs). Wire form is /Name.
// Path is the absolute SKILL.md location (for model file-read activation).
type AgentSkill struct {
	Name        string `json:"name"`
	Description string `json:"description,omitempty"`
	Scope       string `json:"scope"` // workspace | user
	Path        string `json:"path,omitempty"`
	// UserInvocable is false when frontmatter sets user-invocable: false —
	// hidden from the composer `/` menu; model can still activate_skill.
	UserInvocable bool `json:"userInvocable"`
}

// FileContent is a file for the observatory preview/editor. For common image
// types, Mime is set (e.g. "image/png") and Content is empty — the webview
// loads the bytes from /workspace-file. Text small enough to inline over the
// Wails bridge fills Content; larger text also leaves it empty and uses that
// streaming route so JSON-parsing a huge payload cannot freeze the UI.
//
// Content is raw text only: the editor is CodeMirror, which tokenizes and
// splits lines itself. We deliberately do NOT ship a server-rendered HTML
// highlight or a pre-split []string of lines — for a large file those were
// two extra full-size copies (plus a whole-file chroma pass) marshalled over
// the Wails bridge on every open, and nothing read them.
type FileContent struct {
	Path     string `json:"path"`
	Language string `json:"language"`
	Content  string `json:"content"` // raw text when inlined; empty when streamed
	Size     int    `json:"size"`
	Writable bool   `json:"writable"`        // true when path is under a loop worktree
	Mime     string `json:"mime,omitempty"`  // e.g. image/png — body is not in Content
	Mtime    int64  `json:"mtime,omitempty"` // unix milliseconds, for cheap live-refresh
}

// SearchHit is one ripgrep-style match.
type SearchHit struct {
	Path    string `json:"path"`
	Line    int    `json:"line"`
	Col     int    `json:"col"`
	Text    string `json:"text"`
	Preview string `json:"preview"`
}

// SearchResult is one page of search hits plus the reasons the page may not be
// the whole truth. SkippedLarge exists because the walk silently ignores files
// over the per-file byte cap: without surfacing it, a search over a repo with a
// big generated file reports "no matches" and the user has no way to tell that
// from "we didn't look".
type SearchResult struct {
	Hits         []SearchHit `json:"hits"`
	SkippedLarge int         `json:"skippedLarge"` // files skipped for exceeding the per-file size cap
	Truncated    bool        `json:"truncated"`    // hit limit reached — more matches may exist
}

// OutlineSymbol is a top-level symbol in a file (function, type, class, etc.).
type OutlineSymbol struct {
	Name string `json:"name"`
	Kind string `json:"kind"` // func/type/class/const/var/method/export
	Line int    `json:"line"`
	End  int    `json:"end,omitempty"`
}

// SymbolDef is a definition site for a named symbol.
type SymbolDef struct {
	Name string `json:"name"`
	Kind string `json:"kind"`
	Path string `json:"path"`
	Line int    `json:"line"`
}

// SymbolRef is a reference (or definition) site for a symbol name.
type SymbolRef struct {
	Path string `json:"path"`
	Line int    `json:"line"`
	Text string `json:"text"`
	Def  bool   `json:"def"` // true when this site is a definition
}

// MetricSnapshot is one observatory health reading for a root (project or worktree).
type MetricSnapshot struct {
	Files         int        `json:"files"`
	LOC           int        `json:"loc"`
	AvgComplexity float64    `json:"avgComplexity"`
	MaxComplexity int        `json:"maxComplexity"`
	Hotspots      []Hotspot  `json:"hotspots,omitempty"`
	Duplication   float64    `json:"duplication"` // 0..1 fraction of LOC covered by clone blocks
	Duplicates    []DupClone `json:"duplicates,omitempty"`
	TodoCount     int        `json:"todoCount"`
	TestFiles     int        `json:"testFiles"`
	CoveragePct   float64    `json:"coveragePct,omitempty"` // 0 when unknown
}

// Hotspot is a high-complexity or oversized file.
type Hotspot struct {
	Path       string `json:"path"`
	LOC        int    `json:"loc"`
	Complexity int    `json:"complexity"`
	Reason     string `json:"reason"`
}

// DupLoc is one occurrence of a duplicated block.
type DupLoc struct {
	Path string `json:"path"`
	Line int    `json:"line"` // start (1-based)
	End  int    `json:"end"`  // inclusive
	Text string `json:"text"` // first-line preview
}

// DupClone is a block of code that appears in multiple places.
type DupClone struct {
	Lines       int      `json:"lines"`   // block length
	Preview     string   `json:"preview"` // representative snippet
	Occurrences []DupLoc `json:"occurrences,omitempty"`
}

// ImpactItem describes one API/behavior/test affected by a change set.
type ImpactItem struct {
	Kind    string `json:"kind"` // api / symbol / test / file
	Name    string `json:"name"`
	Path    string `json:"path"`
	Line    int    `json:"line,omitempty"`
	Detail  string `json:"detail"`
	Changed bool   `json:"changed"` // true when the item itself was edited
}

// AnalysisRun is a persisted observatory pass over a project or loop worktree.
type AnalysisRun struct {
	ID          string         `json:"id"`
	ProjectID   string         `json:"projectId"`
	WorkspaceID string         `json:"workspaceId,omitempty"`
	Root        string         `json:"root"` // absolute path analyzed
	When        string         `json:"when"` // relative label, e.g. "now"
	TS          int64          `json:"ts"`   // unix ms
	Metrics     MetricSnapshot `json:"metrics"`
	Impact      []ImpactItem   `json:"impact,omitempty"`
	Checks      []Check        `json:"checks,omitempty"`
	Summary     string         `json:"summary"`
	// Delta vs the previous analysis for the same project (observatory continuity).
	Delta *MetricDelta `json:"delta,omitempty"`
}

// MetricDelta is a before→after change in key health numbers.
type MetricDelta struct {
	LOC           int     `json:"loc"`
	MaxComplexity int     `json:"maxComplexity"`
	Duplication   float64 `json:"duplication"`
	TodoCount     int     `json:"todoCount"`
	Hotspots      int     `json:"hotspots"`
}

// TestResult is the Tester role's last report_result for a loop.
type TestResult struct {
	Passed  bool   `json:"passed"`
	Summary string `json:"summary"`
	When    string `json:"when"`
}

// WorktreeCommitResult is the outcome of a human (or staged) commit in a loop worktree.
type WorktreeCommitResult struct {
	Hash      string `json:"hash"`
	Committed bool   `json:"committed"`
	Additions int    `json:"additions"`
	Deletions int    `json:"deletions"`
}

// SearchOpts controls advanced project search / replace-in-files.
type SearchOpts struct {
	Regex         bool   `json:"regex"`
	CaseSensitive bool   `json:"caseSensitive"`
	Glob          string `json:"glob"`   // optional include glob (e.g. "*.go")
	Limit         int    `json:"limit"`  // max hits (0 = default)
	Offset        int    `json:"offset"` // skip first N hits for pagination
}

// ReplaceResult summarizes a replace-in-files pass.
type ReplaceResult struct {
	FilesChanged int `json:"filesChanged"`
	Replacements int `json:"replacements"`
}
