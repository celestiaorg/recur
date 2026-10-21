//! Diagnostics: `@codemirror/lint`'s state (underline marks and point
//! widgets, hover lookup) as Recur uses it through `setDiagnostics`, plus
//! Recur's severity-icon gutter (`cm-lint-gutter.ts`).
//!
//! Recur binds neither `lintKeymap` nor a linter source, so the lint panel,
//! `nextDiagnostic` and the stock lint gutter are unreachable there and are
//! not ported.

use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;

use gpui_kit::{App, Window};

use crate::change::MapMode;
use crate::state::{Effect, Transaction};
use crate::text::Text;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Hint,
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Severity::Hint => "hint",
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

pub type ActionFn = Rc<dyn Fn(usize, usize, &mut Window, &mut App)>;

/// A button on a diagnostic ("Fix", "Fix with AI"); `apply` gets the
/// diagnostic's current range.
#[derive(Clone)]
pub struct DiagnosticAction {
    pub name: String,
    pub mark_class: Option<String>,
    pub apply: ActionFn,
}

impl fmt::Debug for DiagnosticAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiagnosticAction")
            .field("name", &self.name)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub from: usize,
    pub to: usize,
    pub severity: Severity,
    pub message: String,
    pub source: Option<String>,
    pub mark_class: Option<String>,
    pub actions: Vec<DiagnosticAction>,
}

#[derive(Clone, Debug)]
pub enum LintEffect {
    SetDiagnostics(Vec<Rc<Diagnostic>>),
}

/// A decorated stretch: an underline (`cm-lintRange-<sev>`) or, for
/// diagnostics covering nothing but line breaks, a point widget
/// (`cm-lintPoint-<sev>`).
#[derive(Clone, Debug)]
pub struct LintRange {
    pub from: usize,
    pub to: usize,
    pub severity: Severity,
    pub widget: bool,
    pub mark_class: String,
    pub inclusive_end: bool,
    pub diagnostics: Vec<Rc<Diagnostic>>,
}

/// A line's gutter icon: the most severe diagnostic's severity and message,
/// with "(+N more)" when there are several.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LintGutterHit {
    pub severity: Severity,
    pub title: String,
}

#[derive(Clone, Debug, Default)]
pub struct LintState {
    pub ranges: Rc<Vec<LintRange>>,
    /// Recur's gutter source: every diagnostic with its mapped range.
    gutter_source: Rc<Vec<(usize, usize, Rc<Diagnostic>)>>,
    /// Gutter hits keyed by line start.
    pub gutter: Rc<BTreeMap<usize, LintGutterHit>>,
}

fn max_severity(diags: &[Rc<Diagnostic>]) -> Severity {
    diags
        .iter()
        .map(|d| d.severity)
        .max()
        .unwrap_or(Severity::Hint)
        .max(Severity::Hint)
}

/// `LintState.init`: split overlapping diagnostics into decorated ranges.
fn build_ranges(doc: &Text, diagnostics: &[Rc<Diagnostic>]) -> Vec<LintRange> {
    let mut sorted = diagnostics.to_vec();
    sorted.sort_by(|a, b| a.from.cmp(&b.from).then(a.to.cmp(&b.to)));
    let doc_len = doc.len();
    let mut out = Vec::new();
    let mut active: Vec<Rc<Diagnostic>> = Vec::new();
    let mut pos = 0;
    let mut i = 0;
    loop {
        let next = sorted.get(i).cloned();
        if next.is_none() && active.is_empty() {
            break;
        }
        let from;
        let mut to;
        if !active.is_empty() {
            from = pos;
            let limit = match &next {
                Some(n) if n.from > from => n.from,
                _ => usize::MAX,
            };
            to = active.iter().map(|d| d.to).fold(limit, usize::min);
        } else {
            let n = next.unwrap_or_else(|| sorted[i].clone());
            from = n.from;
            if from > doc_len {
                break;
            }
            to = n.to;
            active.push(n);
            i += 1;
        }
        while i < sorted.len() {
            let n = &sorted[i];
            if n.from == from && (n.to > n.from || n.to == from) {
                active.push(n.clone());
                i += 1;
                to = to.min(n.to);
            } else {
                to = to.min(n.from);
                break;
            }
        }
        to = to.min(doc_len);
        let mut widget = false;
        if active
            .iter()
            .any(|d| d.from == from && (d.to == to || to == doc_len))
        {
            widget = from == to;
            if !widget && to - from < 10 {
                widget = doc.slice(from, to).bytes().all(|b| b == b'\n');
            }
        }
        let severity = max_severity(&active);
        let mark_class: String = active
            .iter()
            .filter_map(|d| d.mark_class.as_deref())
            .map(|c| format!(" {c}"))
            .collect();
        out.push(LintRange {
            from,
            to,
            severity,
            widget,
            mark_class: if widget { String::new() } else { mark_class },
            inclusive_end: !widget && active.iter().any(|a| a.to > to),
            diagnostics: active.clone(),
        });
        pos = to;
        if pos == doc_len {
            break;
        }
        active.retain(|d| d.to > pos);
    }
    out
}

fn map_ranges(ranges: &[LintRange], tr: &Transaction) -> Vec<LintRange> {
    let changes = tr.changes.desc();
    ranges
        .iter()
        .filter_map(|r| {
            if r.widget {
                let at = changes.map_pos_mode(r.from, -1, MapMode::TrackDel)?;
                return Some(LintRange {
                    from: at,
                    to: at,
                    ..r.clone()
                });
            }
            let from = changes.map_pos(r.from, 1);
            let to = changes.map_pos(r.to, if r.inclusive_end { 1 } else { -1 });
            if from > to || (from == to && !r.inclusive_end) {
                return None;
            }
            Some(LintRange {
                from,
                to,
                ..r.clone()
            })
        })
        .collect()
}

/// `hitsByLine` from `cm-lint-gutter.ts`.
fn gutter_hits(
    doc: &Text,
    source: &[(usize, usize, Rc<Diagnostic>)],
) -> BTreeMap<usize, LintGutterHit> {
    let mut by_line: BTreeMap<usize, (Severity, String, usize)> = BTreeMap::new();
    for (from, _, d) in source {
        if *from > doc.len() {
            continue;
        }
        let line = doc.line_at(*from).from;
        let message = if d.message.is_empty() {
            "Issue"
        } else {
            &d.message
        };
        let msg = message.split_whitespace().collect::<Vec<_>>().join(" ");
        match by_line.get_mut(&line) {
            None => {
                by_line.insert(line, (d.severity, msg, 1));
            }
            Some(cur) => {
                cur.2 += 1;
                if d.severity > cur.0 {
                    cur.0 = d.severity;
                    cur.1 = msg;
                }
            }
        }
    }
    by_line
        .into_iter()
        .map(|(line, (severity, title, count))| {
            let title = if count > 1 {
                format!("{title} (+{} more)", count - 1)
            } else {
                title
            };
            (line, LintGutterHit { severity, title })
        })
        .collect()
}

impl LintState {
    pub(crate) fn update(&self, tr: &Transaction) -> LintState {
        let mut next = self.clone();
        let mut set = None;
        for e in &tr.effects {
            if let Effect::Lint(LintEffect::SetDiagnostics(d)) = e {
                set = Some(d);
            }
        }
        let doc = tr.new_doc();
        if let Some(diags) = set {
            next.ranges = Rc::new(build_ranges(doc, diags));
            next.gutter_source = Rc::new(diags.iter().map(|d| (d.from, d.to, d.clone())).collect());
            next.gutter = Rc::new(gutter_hits(doc, &next.gutter_source));
            return next;
        }
        if tr.doc_changed() {
            if !self.ranges.is_empty() {
                next.ranges = Rc::new(map_ranges(&self.ranges, tr));
            }
            if !self.gutter_source.is_empty() {
                next.gutter_source = Rc::new(
                    self.gutter_source
                        .iter()
                        .map(|(f, t, d)| {
                            (
                                tr.changes.map_pos(*f, 1),
                                tr.changes.map_pos(*t, -1),
                                d.clone(),
                            )
                        })
                        .collect(),
                );
                next.gutter = Rc::new(gutter_hits(doc, &next.gutter_source));
            }
        }
        next
    }

    /// `diagnosticCount`: decorated ranges.
    pub fn count(&self) -> usize {
        self.ranges.len()
    }

    /// Whether the gutter column shows (it collapses to zero width when
    /// there are no markers).
    pub fn gutter_visible(&self) -> bool {
        !self.gutter.is_empty()
    }

    /// `lintTooltip`: the diagnostics under `pos` approached from `side`,
    /// with the range the tooltip anchors to.
    pub fn tooltip_at(&self, pos: usize, side: i32) -> Option<(usize, usize, Vec<Rc<Diagnostic>>)> {
        let lo = pos.saturating_sub(usize::from(side < 0));
        let hi = pos + usize::from(side > 0);
        self.ranges
            .iter()
            .filter(|r| r.to >= lo && r.from <= hi)
            .find(|r| {
                pos >= r.from
                    && pos <= r.to
                    && (r.from == r.to || ((pos > r.from || side > 0) && (pos < r.to || side < 0)))
            })
            .map(|r| (r.from, r.to, r.diagnostics.clone()))
    }

    /// `findDiagnostic`: the current extent of `diagnostic`.
    pub fn find(&self, diagnostic: &Rc<Diagnostic>) -> Option<(usize, usize)> {
        let mut found: Option<(usize, usize)> = None;
        for r in self.ranges.iter() {
            if !r.diagnostics.iter().any(|d| Rc::ptr_eq(d, diagnostic)) {
                continue;
            }
            match found {
                None => found = Some((r.from, r.to)),
                Some((f, _)) => found = Some((f, r.to)),
            }
        }
        found
    }
}

/// `hideTooltip`: the lint hover closes on new diagnostics or an edit
/// touching its line(s).
pub fn hide_lint_tooltip(tr: &Transaction, from: usize, to: usize) -> bool {
    let line = tr.start_state.doc.line_at(from);
    tr.effects
        .iter()
        .any(|e| matches!(e, Effect::Lint(LintEffect::SetDiagnostics(_))))
        || tr.changes.touches_range(line.from, line.to.max(to)) != crate::change::Touch::No
}

/// `assignKeys`: the access key letter for each action.
pub fn assign_keys(actions: &[DiagnosticAction]) -> Vec<Option<char>> {
    let mut assigned: Vec<Option<char>> = Vec::new();
    for a in actions {
        let key = a.name.chars().find(|c| {
            c.is_ascii_alphabetic() && !assigned.iter().flatten().any(|k| k.eq_ignore_ascii_case(c))
        });
        assigned.push(key);
    }
    assigned
}

/// Builds a set-diagnostics transaction spec (`setDiagnostics`).
pub fn set_diagnostics(diagnostics: Vec<Diagnostic>) -> crate::state::TransactionSpec {
    crate::state::TransactionSpec::default().effect(Effect::Lint(LintEffect::SetDiagnostics(
        diagnostics.into_iter().map(Rc::new).collect(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::Change;
    use crate::state::{EditorState, StateConfig, TransactionSpec};

    fn diag(from: usize, to: usize, severity: Severity, message: &str) -> Diagnostic {
        Diagnostic {
            from,
            to,
            severity,
            message: message.into(),
            source: None,
            mark_class: None,
            actions: Vec::new(),
        }
    }

    fn with(doc: &str, diags: Vec<Diagnostic>) -> EditorState {
        let s = EditorState::create(doc, StateConfig::default());
        s.update(set_diagnostics(diags)).state().clone()
    }

    #[test]
    fn overlapping_diagnostics_split_into_ranges() {
        let s = with(
            "abcdefghij\nxyz",
            vec![
                diag(0, 6, Severity::Warning, "w"),
                diag(3, 9, Severity::Error, "e"),
            ],
        );
        let r: Vec<_> = s
            .lint
            .ranges
            .iter()
            .map(|r| (r.from, r.to, r.severity, r.diagnostics.len()))
            .collect();
        assert_eq!(
            r,
            vec![
                (0, 3, Severity::Warning, 1),
                (3, 6, Severity::Error, 2),
                (6, 9, Severity::Error, 1)
            ]
        );
        assert!(s.lint.ranges[0].inclusive_end);
    }

    #[test]
    fn line_break_only_ranges_become_widgets() {
        let s = with(
            "ab\n\ncd",
            vec![
                diag(2, 4, Severity::Info, "i"),
                diag(5, 5, Severity::Hint, "h"),
            ],
        );
        assert!(s.lint.ranges.iter().all(|r| r.widget));
        assert_eq!(s.lint.ranges.len(), 2);
    }

    #[test]
    fn ranges_map_through_edits_and_gutter_groups_by_line() {
        let s = with(
            "let x = 1\nlet y = 2",
            vec![
                diag(4, 5, Severity::Warning, "unused   x"),
                diag(0, 3, Severity::Error, "bad"),
                diag(14, 15, Severity::Info, "info"),
            ],
        );
        assert_eq!(
            s.lint.gutter.get(&0),
            Some(&LintGutterHit {
                severity: Severity::Error,
                title: "bad (+1 more)".into()
            })
        );
        let s2 = s
            .update(TransactionSpec::default().changes(Change::insert(0, "// \n")))
            .state()
            .clone();
        assert_eq!((s2.lint.ranges[0].from, s2.lint.ranges[0].to), (4, 7));
        assert!(s2.lint.gutter.contains_key(&4));
        assert!(s2.lint.gutter.contains_key(&14));
        let s3 = s
            .update(TransactionSpec::default().changes(Change::delete(4, 5)))
            .state()
            .clone();
        assert_eq!(s3.lint.ranges.len(), 2);
        let d = s.lint.ranges[1].diagnostics[0].clone();
        assert_eq!(s.lint.find(&d), Some((4, 5)));
    }

    #[test]
    fn tooltip_lookup_respects_side() {
        let s = with("abcdef", vec![diag(2, 4, Severity::Error, "e")]);
        assert!(s.lint.tooltip_at(2, 1).is_some());
        assert!(s.lint.tooltip_at(2, -1).is_none());
        assert!(s.lint.tooltip_at(4, -1).is_some());
        assert!(s.lint.tooltip_at(4, 1).is_none());
        assert!(s.lint.tooltip_at(3, 1).is_some());
    }

    #[test]
    fn access_keys() {
        let noop: ActionFn = Rc::new(|_, _, _, _| {});
        let a = |n: &str| DiagnosticAction {
            name: n.into(),
            mark_class: None,
            apply: noop.clone(),
        };
        assert_eq!(
            assign_keys(&[a("Fix"), a("Fix with AI")]),
            vec![Some('F'), Some('i')]
        );
    }
}
