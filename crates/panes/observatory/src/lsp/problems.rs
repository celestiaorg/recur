//! LSP diagnostics surfaced in the Problems panel: one store for the
//! app, keyed by session and document URI.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use gpui_kit::{App, Global};
use recur_code::collate::locale_compare;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProblemSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

impl ProblemSeverity {
    /// LSP `DiagnosticSeverity` (1 error, 2 warning, 3 info, else hint).
    pub fn from_lsp(sev: Option<i64>) -> Self {
        match sev {
            Some(1) => ProblemSeverity::Error,
            Some(2) => ProblemSeverity::Warning,
            Some(3) => ProblemSeverity::Info,
            _ => ProblemSeverity::Hint,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            ProblemSeverity::Error => "error",
            ProblemSeverity::Warning => "warning",
            ProblemSeverity::Info => "info",
            ProblemSeverity::Hint => "hint",
        }
    }
}

/// A diagnostic code: a string or a number on the wire.
#[derive(Clone, Debug, PartialEq)]
pub enum ProblemCode {
    Str(String),
    Num(f64),
}

impl ProblemCode {
    pub fn from_json(v: &serde_json::Value) -> Option<Self> {
        match v {
            serde_json::Value::String(s) => Some(ProblemCode::Str(s.clone())),
            serde_json::Value::Number(n) => n.as_f64().map(ProblemCode::Num),
            _ => None,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        match self {
            ProblemCode::Str(s) => serde_json::Value::String(s.clone()),
            ProblemCode::Num(n) => serde_json::json!(n),
        }
    }

    /// `String(code)`.
    pub fn display(&self) -> String {
        match self {
            ProblemCode::Str(s) => s.clone(),
            ProblemCode::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", *n as i64),
            ProblemCode::Num(n) => n.to_string(),
        }
    }
}

/// One diagnostic (`LspProblem`).
#[derive(Clone, Debug, PartialEq)]
pub struct LspProblem {
    pub uri: String,
    /// Repo-relative path when known.
    pub path: String,
    pub severity: ProblemSeverity,
    pub message: String,
    /// 1-based.
    pub line: i64,
    pub character: i64,
    pub end_line: i64,
    pub end_character: i64,
    pub source: Option<String>,
    pub code: Option<ProblemCode>,
    pub session_id: Option<String>,
    pub workspace_id: Option<String>,
}

/// `lspProblemsForWorkspace`.
pub fn problems_for_workspace(list: &[LspProblem], workspace_id: &str) -> Vec<LspProblem> {
    if workspace_id.is_empty() {
        return list.to_vec();
    }
    list.iter()
        .filter(|p| p.workspace_id.as_deref() == Some(workspace_id))
        .cloned()
        .collect()
}

/// `countLspProblems`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProblemCounts {
    pub errors: usize,
    pub warnings: usize,
    pub total: usize,
}

pub fn count_problems(list: &[LspProblem]) -> ProblemCounts {
    let mut c = ProblemCounts {
        total: list.len(),
        ..Default::default()
    };
    for p in list {
        match p.severity {
            ProblemSeverity::Error => c.errors += 1,
            ProblemSeverity::Warning => c.warnings += 1,
            _ => {}
        }
    }
    c
}

/// Per path: counts and the worst severity (`LspPathProblemFlag`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathProblemFlag {
    pub errors: usize,
    pub warnings: usize,
    pub count: usize,
    /// `Error`, `Warning` or `Info`.
    pub severity: ProblemSeverity,
}

/// `groupLspProblemsByPath`.
pub fn group_by_path(list: &[LspProblem]) -> BTreeMap<String, PathProblemFlag> {
    let mut m: BTreeMap<String, PathProblemFlag> = BTreeMap::new();
    for p in list {
        if p.path.is_empty() {
            continue;
        }
        let cur = m.entry(p.path.clone()).or_insert(PathProblemFlag {
            errors: 0,
            warnings: 0,
            count: 0,
            severity: ProblemSeverity::Info,
        });
        cur.count += 1;
        match p.severity {
            ProblemSeverity::Error => {
                cur.errors += 1;
                cur.severity = ProblemSeverity::Error;
            }
            ProblemSeverity::Warning => {
                cur.warnings += 1;
                if cur.severity != ProblemSeverity::Error {
                    cur.severity = ProblemSeverity::Warning;
                }
            }
            _ => {}
        }
    }
    m
}

/// `lspProblemArgs`: the `/fix-problem` arguments.
pub fn problem_args(p: &LspProblem) -> String {
    let code = p
        .code
        .as_ref()
        .map(ProblemCode::display)
        .unwrap_or_default();
    let from: Vec<&str> = [p.source.as_deref().unwrap_or(""), code.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    let from = from.join(" ");
    let msg = p.message.split_whitespace().collect::<Vec<_>>().join(" ");
    format!(
        "{}:{}:{} {}{}: {}",
        p.path,
        p.line,
        p.character + 1,
        p.severity.key(),
        if from.is_empty() {
            String::new()
        } else {
            format!(" ({from})")
        },
        msg
    )
}

/// The app's diagnostics (`byUri`): `session\0uri` → problems. A clone shares the problem
/// lists, so a snapshot costs one map of handles.
#[derive(Clone, Default)]
pub struct LspProblems {
    by_key: BTreeMap<String, Rc<Vec<LspProblem>>>,
    epoch: u64,
    /// `all()` for the current epoch: language servers publish file by file, and every
    /// publish wakes each observer.
    sorted: RefCell<Option<Rc<Vec<LspProblem>>>>,
}

impl Global for LspProblems {}

impl LspProblems {
    /// `setLspProblemsForUri`.
    pub fn set_for_uri(&mut self, uri: &str, problems: Vec<LspProblem>, session_key: &str) {
        let key = format!("{session_key}\0{uri}");
        if problems.is_empty() {
            if self.by_key.remove(&key).is_none() {
                return;
            }
        } else {
            self.by_key.insert(key, Rc::new(problems));
        }
        self.bump();
    }

    fn bump(&mut self) {
        self.epoch += 1;
        self.sorted.take();
    }

    /// `clearLspProblems`.
    pub fn clear(&mut self) {
        if !self.by_key.is_empty() {
            self.by_key.clear();
            self.bump();
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// `lspProblems`: every problem, by path, then line, then column.
    pub fn all(&self) -> Rc<Vec<LspProblem>> {
        self.sorted
            .borrow_mut()
            .get_or_insert_with(|| {
                let mut all: Vec<LspProblem> = self
                    .by_key
                    .values()
                    .flat_map(|v| v.iter())
                    .cloned()
                    .collect();
                // Collate each distinct path once, not once per comparison.
                let mut paths: Vec<String> = all.iter().map(|p| p.path.clone()).collect();
                paths.sort_unstable();
                paths.dedup();
                paths.sort_by(|a, b| locale_compare(a, b));
                let mut rank: HashMap<String, usize> = HashMap::with_capacity(paths.len());
                let mut r = 0;
                for (i, p) in paths.iter().enumerate() {
                    if i > 0 && !locale_compare(&paths[i - 1], p).is_eq() {
                        r = i;
                    }
                    rank.insert(p.clone(), r);
                }
                all.sort_by_cached_key(|p| (rank[&p.path], p.line, p.character));
                Rc::new(all)
            })
            .clone()
    }

    /// The problems on one document, by line, then column.
    pub fn for_uri(&self, uri: &str) -> Vec<LspProblem> {
        let mut out: Vec<LspProblem> = self
            .by_key
            .values()
            .flat_map(|v| v.iter())
            .filter(|p| p.uri == uri)
            .cloned()
            .collect();
        out.sort_by_key(|p| (p.line, p.character));
        out
    }

    pub fn global(cx: &App) -> Option<&LspProblems> {
        cx.try_global::<LspProblems>()
    }
}

/// Mutate the app's problem store (installing it on first use).
pub fn update_problems(cx: &mut App, f: impl FnOnce(&mut LspProblems)) {
    f(cx.default_global::<LspProblems>());
}

/// `peekLspProblems`.
pub fn peek_problems(cx: &App) -> Rc<Vec<LspProblem>> {
    LspProblems::global(cx)
        .map(LspProblems::all)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(uri: &str, path: &str, message: &str) -> LspProblem {
        LspProblem {
            uri: uri.into(),
            path: path.into(),
            severity: ProblemSeverity::Error,
            message: message.into(),
            line: 1,
            character: 0,
            end_line: 1,
            end_character: 1,
            source: None,
            code: None,
            session_id: None,
            workspace_id: None,
        }
    }

    #[test]
    fn all_sorts_by_collated_path_then_position_and_follows_changes() {
        let at = |uri: &str, path: &str, line: i64, character: i64| LspProblem {
            line,
            character,
            ..problem(uri, path, "")
        };
        let mut store = LspProblems::default();
        store.set_for_uri(
            "u:b",
            vec![at("u:b", "b.rs", 3, 0), at("u:b", "b.rs", 1, 4)],
            "s1",
        );
        store.set_for_uri("u:B", vec![at("u:B", "B.rs", 2, 0)], "s1");
        store.set_for_uri("u:a", vec![at("u:a", "a.rs", 9, 0)], "s2");
        store.set_for_uri("u:b", vec![at("u:b", "b.rs", 1, 0)], "s2");
        let mut want: Vec<LspProblem> = store
            .by_key
            .values()
            .flat_map(|v| v.iter())
            .cloned()
            .collect();
        want.sort_by(|a, b| {
            locale_compare(&a.path, &b.path)
                .then(a.line.cmp(&b.line))
                .then(a.character.cmp(&b.character))
        });
        assert_eq!(*store.all(), want);
        let b: Vec<(i64, i64)> = store
            .for_uri("u:b")
            .iter()
            .map(|p| (p.line, p.character))
            .collect();
        assert_eq!(b, [(1, 0), (1, 4), (3, 0)]);
        store.set_for_uri("u:a", vec![], "s2");
        assert!(store.all().iter().all(|p| p.path != "a.rs"));
        assert_eq!(store.all().len(), want.len() - 1);
    }

    #[test]
    fn restricts_list_counts_and_badges_to_one_workspace() {
        let mut a = problem("file:///a/x.go", "x.go", "A");
        a.workspace_id = Some("ws-a".into());
        a.session_id = Some("s1".into());
        let mut b = problem("file:///b/x.go", "x.go", "B");
        b.workspace_id = Some("ws-b".into());
        b.session_id = Some("s2".into());
        b.severity = ProblemSeverity::Warning;
        let all = vec![a, b.clone()];
        let mine = problems_for_workspace(&all, "ws-b");
        assert_eq!(mine, vec![b]);
        assert_eq!(
            count_problems(&mine),
            ProblemCounts {
                errors: 0,
                warnings: 1,
                total: 1
            }
        );
        assert_eq!(
            group_by_path(&mine).get("x.go"),
            Some(&PathProblemFlag {
                errors: 0,
                warnings: 1,
                count: 1,
                severity: ProblemSeverity::Warning
            })
        );
        assert_eq!(problems_for_workspace(&all, ""), all);
    }

    #[test]
    fn args_name_location_source_code_and_message() {
        let mut p = problem("file:///a/x.go", "pkg/x.go", "undefined: foo");
        p.line = 12;
        p.character = 4;
        p.source = Some("compiler".into());
        p.code = Some(ProblemCode::Str("UndeclaredName".into()));
        assert_eq!(
            problem_args(&p),
            "pkg/x.go:12:5 error (compiler UndeclaredName): undefined: foo"
        );
    }

    #[test]
    fn args_omit_missing_source_and_flatten_multiline_messages() {
        let p = problem("file:///a/x.go", "x.go", "m\n  more");
        assert_eq!(problem_args(&p), "x.go:1:1 error: m more");
    }

    #[test]
    fn store_keys_by_session_and_sorts_by_path_then_line() {
        let mut s = LspProblems::default();
        let mut late = problem("file:///r/b.go", "b.go", "late");
        late.line = 9;
        let mut early = problem("file:///r/b.go", "b.go", "early");
        early.line = 2;
        s.set_for_uri("file:///r/b.go", vec![late, early], "s1");
        s.set_for_uri(
            "file:///r/a.go",
            vec![problem("file:///r/a.go", "a.go", "a")],
            "s1",
        );
        let msgs: Vec<String> = s.all().iter().map(|p| p.message.clone()).collect();
        assert_eq!(msgs, ["a", "early", "late"]);
        let e = s.epoch();
        s.set_for_uri("file:///r/a.go", vec![], "s2");
        assert_eq!(s.epoch(), e, "clearing an unknown key is a no-op");
        s.set_for_uri("file:///r/a.go", vec![], "s1");
        assert_eq!(s.all().len(), 2);
    }
}
