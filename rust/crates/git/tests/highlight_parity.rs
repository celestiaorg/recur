//! Highlighting matches the Go app byte for byte. The fixtures come from Go
//! `internal/git` `highlightLine` and chroma `lexers.Match` run over chroma's
//! lexer test inputs and a set of edge cases.

use recur_domain::{DiffLine, FileDiff};
use serde_json::Value;

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('\'', "&#39;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&#34;")
}

#[test]
fn highlight_matches_go_highlight_line() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/highlight_fixture.json")).expect("fixture");
    let mut failures = Vec::new();
    for case in &cases {
        let path = case["path"].as_str().unwrap_or_default();
        let code = case["code"].as_str().unwrap_or_default();
        let want = case["html"].as_str().unwrap_or_default();
        let mut files = vec![FileDiff {
            path: path.to_string(),
            lines: Some(vec![DiffLine {
                r#type: "add".into(),
                code_html: escape(code),
                ..Default::default()
            }]),
            ..Default::default()
        }];
        recur_git::highlight_files(&mut files);
        let got = files[0]
            .lines
            .as_ref()
            .map(|l| l[0].code_html.clone())
            .unwrap_or_default();
        if got != want {
            failures.push(format!("{path}: {code:?}\n  got:  {got}\n  want: {want}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} lines differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

#[test]
fn lexer_name_matches_go_match() {
    let names: serde_json::Map<String, Value> =
        serde_json::from_str(include_str!("fixtures/lexer_names.json")).expect("fixture");
    let mut failures = Vec::new();
    for (path, want) in &names {
        let want = want.as_str().unwrap_or_default();
        let got = recur_git::lexer_name(path);
        if got != want {
            failures.push(format!("{path}: got {got:?}, want {want:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} names differ:\n{}",
        failures.len(),
        names.len(),
        failures.join("\n")
    );
}
