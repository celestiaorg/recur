//! vscode-textmate's own tokenization suites (`test-cases/first-mate`,
//! `test-cases/suite1`, copied from v9.3.2 under its MIT license), run the
//! way `src/tests/tokenization.test.ts` runs them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use recur_textmate::{
    GrammarSource, LoadGrammarError, RawGrammar, Registry, StateStack, parse_raw_grammar,
};
use serde_json::Value;

struct Fixtures {
    grammars: HashMap<String, PathBuf>,
    injections: Option<Vec<String>>,
    root: String,
}

impl GrammarSource for Fixtures {
    fn load_grammar(&mut self, scope: &str) -> Result<Option<RawGrammar>, LoadGrammarError> {
        let Some(path) = self.grammars.get(scope) else {
            return Ok(None);
        };
        let content = std::fs::read_to_string(path).map_err(|e| LoadGrammarError::Source {
            scope: scope.into(),
            message: e.to_string(),
        })?;
        parse_raw_grammar(&content, path.to_str())
            .map(Some)
            .map_err(|e| LoadGrammarError::Source {
                scope: scope.into(),
                message: e.to_string(),
            })
    }

    fn injections(&mut self, scope: &str) -> Option<Vec<String>> {
        if scope == self.root {
            return self.injections.clone();
        }
        None
    }
}

fn run_suite(rel: &str) -> (Vec<String>, usize) {
    let location = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/vscode-textmate")
        .join(rel);
    let dir = location.parent().unwrap().to_path_buf();
    let tests: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(&location).unwrap()).unwrap();
    let mut failures = Vec::new();
    let mut compared = 0;
    for test in tests {
        let desc = test["desc"].as_str().unwrap_or("").to_string();
        let mut grammars = HashMap::new();
        let mut root = test["grammarScopeName"].as_str().map(str::to_string);
        for g in test["grammars"].as_array().unwrap() {
            let rel = g.as_str().unwrap();
            let path = dir.join(rel);
            let raw =
                parse_raw_grammar(&std::fs::read_to_string(&path).unwrap(), Some(rel)).unwrap();
            if root.is_none() && Some(rel) == test["grammarPath"].as_str() {
                root = Some(raw.scope_name.clone());
            }
            grammars.insert(raw.scope_name.clone(), path);
        }
        let root = root.expect("test names a grammar");
        let injections = test["grammarInjections"]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect());
        let mut source = Fixtures {
            grammars,
            injections,
            root: root.clone(),
        };
        let mut registry = Registry::new(None);
        let grammar = registry
            .load_grammar(&root, &mut source)
            .unwrap()
            .expect("grammar loads");
        let mut prev: Option<StateStack> = None;
        for case in test["lines"].as_array().unwrap() {
            let line = case["line"].as_str().unwrap();
            let r = grammar.tokenize_line(line, prev.as_ref()).unwrap();
            let actual: Vec<(String, Vec<String>)> = r
                .tokens
                .iter()
                .map(|t| {
                    (
                        line.get(t.start_index..t.end_index.min(line.len()))
                            .unwrap_or("")
                            .to_string(),
                        t.scopes.clone(),
                    )
                })
                .collect();
            let expected: Vec<(String, Vec<String>)> = case["tokens"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| {
                    (
                        t["value"].as_str().unwrap().to_string(),
                        t["scopes"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|s| s.as_str().unwrap().to_string())
                            .collect(),
                    )
                })
                .filter(|(v, _)| line.is_empty() || !v.is_empty())
                .collect();
            if actual != expected {
                failures.push(format!(
                    "{desc}: line {line:?}\n  actual:   {actual:?}\n  expected: {expected:?}"
                ));
                break;
            }
            compared += 1;
            prev = Some(r.rule_stack);
        }
    }
    (failures, compared)
}

#[test]
fn first_mate() {
    let (f, compared) = run_suite("first-mate/tests.json");
    assert!(f.is_empty(), "{} failures:\n{}", f.len(), f.join("\n"));
    assert!(compared == 97, "only {compared} lines compared");
}

#[test]
fn suite1() {
    let (f, compared) = run_suite("suite1/tests.json");
    assert!(f.is_empty(), "{} failures:\n{}", f.len(), f.join("\n"));
    assert!(compared == 49, "only {compared} lines compared");
}

#[test]
fn suite1_while() {
    let (f, compared) = run_suite("suite1/whileTests.json");
    assert!(f.is_empty(), "{} failures:\n{}", f.len(), f.join("\n"));
    assert!(compared == 18, "only {compared} lines compared");
}
