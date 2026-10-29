//! Differential check against vscode-textmate 9.3.2 + vscode-oniguruma 2.0.1
//! on real files with the shipped grammars. Fixtures come from
//! `fixtures/differential/generate.cjs` (run from `frontend/`): scope lists
//! per token, plus tokenizeLine2 metadata and the colour map under Charcoal.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use recur_textmate::{
    GrammarSource, LoadGrammarError, RawGrammar, RawTheme, RawThemeSetting, Registry, StateStack,
    ThemeScope, parse_raw_grammar,
};
use serde_json::Value;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Bundles(HashMap<String, PathBuf>);

impl Bundles {
    fn load() -> Bundles {
        let mut map = HashMap::new();
        for b in std::fs::read_dir(repo().join("bundles")).unwrap().flatten() {
            let Ok(entries) = std::fs::read_dir(b.path().join("syntaxes")) else {
                continue;
            };
            for e in entries.flatten() {
                let v: Value =
                    serde_json::from_str(&std::fs::read_to_string(e.path()).unwrap()).unwrap();
                map.insert(v["scopeName"].as_str().unwrap().to_string(), e.path());
            }
        }
        Bundles(map)
    }
}

impl GrammarSource for Bundles {
    fn load_grammar(&mut self, scope: &str) -> Result<Option<RawGrammar>, LoadGrammarError> {
        let Some(p) = self.0.get(scope) else {
            return Ok(None);
        };
        Ok(Some(
            parse_raw_grammar(&std::fs::read_to_string(p).unwrap(), p.to_str()).unwrap(),
        ))
    }
    fn injections(&mut self, _: &str) -> Option<Vec<String>> {
        None
    }
}

fn charcoal() -> RawTheme {
    let t: Value = serde_json::from_str(
        &std::fs::read_to_string(repo().join("bundles/Recur/themes/charcoal.tmTheme.json"))
            .unwrap(),
    )
    .unwrap();
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let mut settings = vec![RawThemeSetting {
        foreground: s(&t["colors"], "editor.foreground"),
        background: s(&t["colors"], "editor.background"),
        ..RawThemeSetting::default()
    }];
    for tc in t["tokenColors"].as_array().unwrap() {
        let scope = match &tc["scope"] {
            Value::String(x) if !x.is_empty() => ThemeScope::One(x.clone()),
            Value::Array(a) => ThemeScope::Many(
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
            ),
            _ => continue,
        };
        settings.push(RawThemeSetting {
            name: s(tc, "name"),
            scope,
            font_style: s(&tc["settings"], "fontStyle"),
            foreground: s(&tc["settings"], "foreground"),
            background: s(&tc["settings"], "background"),
        });
    }
    RawTheme {
        name: Some("charcoal".into()),
        settings,
    }
}

#[test]
fn matches_vscode_textmate_on_real_files() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/differential");
    let mut source = Bundles::load();
    let mut plain = Registry::new(None);
    let theme = charcoal();
    let mut themed = Registry::new(Some(&theme));
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        if entry.path().extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let fx: Value =
            serde_json::from_str(&std::fs::read_to_string(entry.path()).unwrap()).unwrap();
        let scope = fx["scope"].as_str().unwrap();
        let g = plain.load_grammar(scope, &mut source).unwrap().unwrap();
        let tg = themed.load_grammar(scope, &mut source).unwrap().unwrap();

        let mut stack: Option<StateStack> = None;
        for case in fx["lines"].as_array().unwrap() {
            let line = case["line"].as_str().unwrap();
            let r = g.tokenize_line(line, stack.as_ref()).unwrap();
            let actual: Vec<(String, Vec<String>)> = r
                .tokens
                .iter()
                .map(|t| {
                    (
                        line[t.start_index..t.end_index.min(line.len())].to_string(),
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
                        t[0].as_str().unwrap().to_string(),
                        t[1].as_array()
                            .unwrap()
                            .iter()
                            .map(|s| s.as_str().unwrap().to_string())
                            .collect(),
                    )
                })
                .collect();
            assert_eq!(actual, expected, "{scope}: {line:?}");
            stack = Some(r.rule_stack);
            checked += 1;
        }

        let mut stack: Option<StateStack> = None;
        for (case, line) in fx["binary"]
            .as_array()
            .unwrap()
            .iter()
            .zip(fx["lines"].as_array().unwrap())
        {
            let line = line["line"].as_str().unwrap();
            let r = tg.tokenize_line2(line, stack.as_ref()).unwrap();
            let mut actual = Vec::new();
            for i in (0..r.tokens.len()).step_by(2) {
                let start = r.tokens[i] as usize;
                let end = if i + 2 < r.tokens.len() {
                    r.tokens[i + 2] as usize
                } else {
                    line.len()
                };
                actual.push((
                    line[start.min(line.len())..end.min(line.len())].to_string(),
                    r.tokens[i + 1],
                ));
            }
            let expected: Vec<(String, u32)> = case
                .as_array()
                .unwrap()
                .iter()
                .map(|t| {
                    (
                        t[0].as_str().unwrap().to_string(),
                        t[1].as_u64().unwrap() as u32,
                    )
                })
                .collect();
            assert_eq!(actual, expected, "{scope} (themed): {line:?}");
            stack = Some(r.rule_stack);
        }
        let map: Vec<String> = fx["colorMap"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap_or("").to_string())
            .collect();
        let ours = themed.color_map();
        assert_eq!(
            ours[..map.len().min(ours.len())],
            map[..map.len().min(ours.len())],
            "{scope} colour map"
        );
    }
    assert!(checked > 1000, "only {checked} lines");
}
