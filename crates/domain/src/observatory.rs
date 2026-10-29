//! The code-observatory surface: file tree, search, symbols, metrics and
//! impact around loops and PRs (outcome-first, not a full IDE).

use serde::{Deserialize, Serialize};

use crate::Check;
use crate::serde_util::{is_false, is_zero, is_zero_f64};

/// One node in a project/worktree file tree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TreeEntry {
    pub name: String,
    /// Repo-relative, forward slashes.
    pub path: String,
    pub dir: bool,
    /// m/a/d/u: git worktree status (D46).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub status: String,
    /// git check-ignore (D-explorer-ignore-mute).
    #[serde(skip_serializing_if = "is_false")]
    pub ignored: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TreeEntry>,
}

/// One discovered Agent Skill (SKILL.md) for the composer `/` menu and host
/// progressive disclosure. Wire form is /Name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentSkill {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// workspace | user.
    pub scope: String,
    /// The absolute SKILL.md location.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// False when frontmatter sets user-invocable: false (hidden from the
    /// `/` menu; the model can still activate it).
    pub user_invocable: bool,
}

/// A file for the observatory preview/editor. Images set Mime and leave
/// Content empty; small text is inlined in Content, larger text is streamed
/// separately (Content empty).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileContent {
    pub path: String,
    pub language: String,
    /// Raw text when inlined; empty when streamed.
    pub content: String,
    pub size: i64,
    /// True when path is under a loop worktree.
    pub writable: bool,
    /// e.g. image/png; the body is not in Content.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mime: String,
    /// Unix milliseconds, for cheap live-refresh.
    #[serde(skip_serializing_if = "is_zero")]
    pub mtime: i64,
}

/// One ripgrep-style match.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchHit {
    pub path: String,
    pub line: i64,
    pub col: i64,
    pub text: String,
    pub preview: String,
}

/// One page of search hits plus the reasons the page may not be the whole
/// truth.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchResult {
    /// `None` serializes as `null`.
    pub hits: Option<Vec<SearchHit>>,
    /// Files skipped for exceeding the per-file size cap.
    pub skipped_large: i64,
    /// Hit limit reached; more matches may exist.
    pub truncated: bool,
}

/// One ranked path from SearchFiles (@-mentions / Quick Open).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileSearchHit {
    pub path: String,
    pub score: i64,
    pub dir: bool,
}

/// One destination in the workspace "Open In" picker. Kind is "app"; Icon is
/// a PNG data URL of the OS app icon when available.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenInTarget {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub icon: String,
}

/// SearchFiles output: top hits plus index readiness.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileSearchResult {
    /// `None` serializes as `null`.
    pub hits: Option<Vec<FileSearchHit>>,
    pub building: bool,
    pub ready: bool,
}

/// A top-level symbol in a file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutlineSymbol {
    pub name: String,
    /// func/type/class/const/var/method/export.
    pub kind: String,
    pub line: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub end: i64,
}

/// A definition site for a named symbol.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SymbolDef {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub line: i64,
}

/// A reference (or definition) site for a symbol name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SymbolRef {
    pub path: String,
    pub line: i64,
    pub text: String,
    /// True when this site is a definition.
    pub def: bool,
}

/// One observatory health reading for a root (project or worktree).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MetricSnapshot {
    pub files: i64,
    pub loc: i64,
    pub avg_complexity: f64,
    pub max_complexity: i64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hotspots: Vec<Hotspot>,
    /// 0..1 fraction of LOC covered by clone blocks.
    pub duplication: f64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub duplicates: Vec<DupClone>,
    pub todo_count: i64,
    pub test_files: i64,
    /// 0 when unknown.
    #[serde(skip_serializing_if = "is_zero_f64")]
    pub coverage_pct: f64,
}

/// A high-complexity or oversized file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotspot {
    pub path: String,
    pub loc: i64,
    pub complexity: i64,
    pub reason: String,
}

/// One occurrence of a duplicated block.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DupLoc {
    pub path: String,
    /// Start (1-based).
    pub line: i64,
    /// Inclusive.
    pub end: i64,
    /// First-line preview.
    pub text: String,
}

/// A block of code that appears in multiple places.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DupClone {
    /// Block length.
    pub lines: i64,
    /// Representative snippet.
    pub preview: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub occurrences: Vec<DupLoc>,
}

/// One API/behavior/test affected by a change set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImpactItem {
    /// api / symbol / test / file.
    pub kind: String,
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub line: i64,
    pub detail: String,
    /// True when the item itself was edited.
    pub changed: bool,
}

/// A persisted observatory pass over a project or loop worktree.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AnalysisRun {
    pub id: String,
    pub project_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
    /// Absolute path analyzed.
    pub root: String,
    /// Relative label, e.g. "now".
    pub when: String,
    /// Unix ms.
    pub ts: i64,
    pub metrics: MetricSnapshot,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub impact: Vec<ImpactItem>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<Check>,
    pub summary: String,
    /// Delta vs the previous analysis for the same project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<MetricDelta>,
}

/// A before→after change in key health numbers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MetricDelta {
    pub loc: i64,
    pub max_complexity: i64,
    pub duplication: f64,
    pub todo_count: i64,
    pub hotspots: i64,
}

/// The Tester role's last report_result for a loop.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TestResult {
    pub passed: bool,
    pub summary: String,
    pub when: String,
}

/// The outcome of a human (or staged) commit in a loop worktree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorktreeCommitResult {
    pub hash: String,
    pub committed: bool,
    pub additions: i64,
    pub deletions: i64,
}

/// Advanced project search / replace-in-files options.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchOpts {
    pub regex: bool,
    pub case_sensitive: bool,
    /// Optional include glob (e.g. "*.go").
    pub glob: String,
    /// Max hits (0 = default).
    pub limit: i64,
    /// Skip the first N hits for pagination.
    pub offset: i64,
}

/// A replace-in-files pass summary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReplaceResult {
    pub files_changed: i64,
    pub replacements: i64,
}
