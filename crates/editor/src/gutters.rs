//! Recur's state-driven gutters: the SCM change bars against the git base
//! (`cm-diff-gutter.ts`) and reviewer findings (`cm-findings-gutter.ts`).

use std::collections::BTreeMap;
use std::rc::Rc;

use crate::change::MapMode;
use crate::diff::line_hunks;
use crate::state::{Effect, Transaction};
use crate::text::Text;

/// Idle delay before the diff bars are recomputed after an edit (they map
/// through edits meanwhile).
pub const DIFF_RECOMPUTE_MS: u64 = 80;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffHintKind {
    Add,
    Mod,
    Del,
}

impl DiffHintKind {
    pub fn title(self) -> &'static str {
        match self {
            DiffHintKind::Add => "Added versus base",
            DiffHintKind::Mod => "Modified versus base",
            DiffHintKind::Del => "Deleted lines versus base",
        }
    }
}

fn set_kind(
    by_line: &mut BTreeMap<usize, DiffHintKind>,
    line: usize,
    kind: DiffHintKind,
    max_line: usize,
) {
    if line < 1 || line > max_line {
        return;
    }
    match by_line.get(&line) {
        None => {
            by_line.insert(line, kind);
        }
        Some(prev) if *prev != kind => {
            by_line.insert(line, DiffHintKind::Mod);
        }
        _ => {}
    }
}

/// `diffMarksFromDocs`: per-line add/mod/del bars for `current` against
/// `base` (1-based lines, ascending).
pub fn diff_marks_from_docs(base: &str, current: &str) -> Vec<(usize, DiffHintKind)> {
    let a = crate::text::normalize_newlines(base);
    let b = crate::text::normalize_newlines(current);
    if a == b {
        return Vec::new();
    }
    let b_line_count = b.split('\n').count().max(1);
    if a.is_empty() {
        return (1..=b_line_count).map(|l| (l, DiffHintKind::Add)).collect();
    }
    if b.is_empty() {
        return vec![(1, DiffHintKind::Del)];
    }
    let mut by_line = BTreeMap::new();
    for h in line_hunks(&a, &b) {
        let insert = h.from_a == h.to_a;
        let del = h.from_b == h.to_b;
        if insert && !del {
            for i in h.from_b..h.to_b {
                set_kind(&mut by_line, i + 1, DiffHintKind::Add, b_line_count);
            }
            continue;
        }
        if del && !insert {
            set_kind(
                &mut by_line,
                (h.from_b + 1).clamp(1, b_line_count),
                DiffHintKind::Del,
                b_line_count,
            );
            continue;
        }
        for i in h.from_b..h.to_b {
            set_kind(&mut by_line, i + 1, DiffHintKind::Mod, b_line_count);
        }
    }
    by_line.into_iter().collect()
}

/// A reviewer finding to mark (`EditorFindingMark`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindingMark {
    pub id: String,
    pub line: usize,
    pub end_line: Option<usize>,
    pub severity: String,
    pub title: String,
}

/// A finding gutter icon: the most severe finding on the line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindingHit {
    pub id: String,
    pub severity: String,
    pub title: String,
}

fn finding_sev_rank(s: &str) -> u8 {
    match s.to_lowercase().as_str() {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

/// `normSev`: the class suffix for a finding severity.
pub fn norm_finding_severity(s: &str) -> &'static str {
    match s.to_lowercase().as_str() {
        "critical" => "critical",
        "high" => "high",
        "medium" => "medium",
        "low" => "low",
        _ => "info",
    }
}

/// `findingSevIcon`.
pub fn finding_sev_icon(s: &str) -> &'static str {
    match s.to_lowercase().as_str() {
        "critical" => "sev-critical",
        "high" => "sev-high",
        "medium" => "sev-medium",
        "info" => "sev-info",
        _ => "sev-low",
    }
}

fn finding_hits(doc: &Text, marks: &[FindingMark]) -> BTreeMap<usize, FindingHit> {
    let lines = doc.lines();
    let mut by_line: BTreeMap<usize, (FindingHit, usize)> = BTreeMap::new();
    for m in marks {
        if m.id.is_empty() || m.line < 1 {
            continue;
        }
        let start = m.line.min(lines);
        let end = match m.end_line {
            Some(e) if e > m.line => e.min(lines),
            _ => start,
        };
        let severity = if m.severity.is_empty() {
            "info".to_string()
        } else {
            m.severity.clone()
        };
        let title = if m.title.is_empty() {
            "Finding".to_string()
        } else {
            m.title.clone()
        };
        for ln in start..=end {
            match by_line.get_mut(&ln) {
                None => {
                    by_line.insert(
                        ln,
                        (
                            FindingHit {
                                id: m.id.clone(),
                                severity: severity.clone(),
                                title: title.clone(),
                            },
                            1,
                        ),
                    );
                }
                Some((hit, count)) => {
                    *count += 1;
                    if finding_sev_rank(&m.severity) > finding_sev_rank(&hit.severity) {
                        *hit = FindingHit {
                            id: m.id.clone(),
                            severity: severity.clone(),
                            title: title.clone(),
                        };
                    }
                }
            }
        }
    }
    by_line
        .into_iter()
        .map(|(ln, (mut hit, count))| {
            if count > 1 {
                hit.title = format!("{} (+{} more)", hit.title, count - 1);
            }
            (doc.line(ln).from, hit)
        })
        .collect()
}

#[derive(Clone, Debug)]
pub enum GutterEffect {
    /// `setDiffBase`: the git-base text (or none).
    SetDiffBase(Option<Rc<str>>),
    /// `recomputeDiffMarks`: the idle recompute after edits.
    RecomputeDiff,
    /// `setFindingsMarks`.
    SetFindings(Rc<[FindingMark]>),
}

/// Diff bars (keyed by position, mapped through edits) and finding icons
/// (keyed by line start, rebuilt from line numbers on edits).
#[derive(Clone, Debug, Default)]
pub struct GutterState {
    pub diff_base: Option<Rc<str>>,
    pub diff_marks: Rc<Vec<(usize, DiffHintKind)>>,
    findings: Rc<[FindingMark]>,
    pub finding_hits: Rc<BTreeMap<usize, FindingHit>>,
}

fn build_diff_marks(doc: &Text, base: Option<&str>) -> Vec<(usize, DiffHintKind)> {
    let Some(base) = base else { return Vec::new() };
    diff_marks_from_docs(base, &doc.to_string())
        .into_iter()
        .filter(|(l, _)| *l >= 1 && *l <= doc.lines())
        .map(|(l, k)| (doc.line(l).from, k))
        .collect()
}

impl GutterState {
    pub(crate) fn update(&self, tr: &Transaction, doc: &Text) -> GutterState {
        let mut next = self.clone();
        let mut base: Option<Option<Rc<str>>> = None;
        let mut recompute = false;
        let mut findings: Option<Rc<[FindingMark]>> = None;
        for e in &tr.effects {
            match e {
                Effect::Gutter(GutterEffect::SetDiffBase(b)) => base = Some(b.clone()),
                Effect::Gutter(GutterEffect::RecomputeDiff) => recompute = true,
                Effect::Gutter(GutterEffect::SetFindings(f)) => findings = Some(f.clone()),
                _ => {}
            }
        }
        if let Some(b) = base {
            next.diff_marks = Rc::new(build_diff_marks(doc, b.as_deref()));
            next.diff_base = b;
        } else if recompute {
            next.diff_marks = Rc::new(build_diff_marks(doc, next.diff_base.as_deref()));
        } else if tr.doc_changed() && !self.diff_marks.is_empty() {
            let changes = tr.changes.desc();
            next.diff_marks = Rc::new(
                self.diff_marks
                    .iter()
                    .filter_map(|&(p, k)| {
                        changes
                            .map_pos_mode(p, -1, MapMode::TrackDel)
                            .map(|p| (p, k))
                    })
                    .collect(),
            );
        }
        if let Some(f) = findings {
            next.finding_hits = Rc::new(finding_hits(doc, &f));
            next.findings = f;
        } else if tr.doc_changed() && !self.findings.is_empty() {
            next.finding_hits = Rc::new(finding_hits(doc, &self.findings));
        }
        next
    }

    /// The findings column collapses to zero width without markers.
    pub fn findings_visible(&self) -> bool {
        !self.finding_hits.is_empty()
    }

    /// Finding id for a gutter click on the line starting at `line_from`.
    pub fn finding_at(&self, line_from: usize) -> Option<&str> {
        self.finding_hits.get(&line_from).map(|h| h.id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::Change;
    use crate::state::{EditorState, StateConfig, TransactionSpec};

    #[test]
    fn diff_marks() {
        assert!(diff_marks_from_docs("a\nb", "a\nb").is_empty());
        assert_eq!(
            diff_marks_from_docs("", "x\ny"),
            vec![(1, DiffHintKind::Add), (2, DiffHintKind::Add)]
        );
        assert_eq!(diff_marks_from_docs("x", ""), vec![(1, DiffHintKind::Del)]);
        assert_eq!(
            diff_marks_from_docs("a\nb\nc\nd\n", "a\nB\nc\nd\ne\n"),
            vec![(2, DiffHintKind::Mod), (5, DiffHintKind::Add)]
        );
        assert_eq!(
            diff_marks_from_docs("a\nb\nc\n", "a\nc\n"),
            vec![(2, DiffHintKind::Del)]
        );
        assert_eq!(diff_marks_from_docs("a\r\nb", "a\nb"), vec![]);
    }

    #[test]
    fn state_maps_bars_and_rebuilds_findings() {
        let s = EditorState::create("a\nB\nc\n", StateConfig::default());
        let s = s
            .update(
                TransactionSpec::default()
                    .effect(Effect::Gutter(GutterEffect::SetDiffBase(Some(
                        "a\nb\nc\n".into(),
                    ))))
                    .effect(Effect::Gutter(GutterEffect::SetFindings(
                        vec![
                            FindingMark {
                                id: "f1".into(),
                                line: 1,
                                end_line: Some(2),
                                severity: "low".into(),
                                title: "Low".into(),
                            },
                            FindingMark {
                                id: "f2".into(),
                                line: 2,
                                end_line: None,
                                severity: "high".into(),
                                title: "High".into(),
                            },
                        ]
                        .into(),
                    ))),
            )
            .state()
            .clone();
        assert_eq!(*s.gutters.diff_marks, vec![(2, DiffHintKind::Mod)]);
        assert_eq!(s.gutters.finding_at(2), Some("f2"));
        assert_eq!(s.gutters.finding_hits[&2].title, "High (+1 more)");
        let s = s
            .update(TransactionSpec::default().changes(Change::insert(0, "z\n")))
            .state()
            .clone();
        assert_eq!(*s.gutters.diff_marks, vec![(4, DiffHintKind::Mod)]);
        assert_eq!(s.gutters.finding_at(0), Some("f1"));
        assert_eq!(s.gutters.finding_at(2), Some("f2"));
        let s = s
            .update(TransactionSpec::default().effect(Effect::Gutter(GutterEffect::RecomputeDiff)))
            .state()
            .clone();
        assert_eq!(
            *s.gutters.diff_marks,
            vec![(0, DiffHintKind::Add), (4, DiffHintKind::Mod)]
        );
    }
}
