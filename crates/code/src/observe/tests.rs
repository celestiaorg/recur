//! Port of `observe_test.go`.

use recur_domain::{DiffLine, FileDiff, Finding, Severity, TestResult};
use tokio_util::sync::CancellationToken;

use super::checks::Ctx;
use super::*;
use crate::testutil::{tmp, write};

fn line(new: &str, kind: &str) -> DiffLine {
    DiffLine {
        new: new.into(),
        r#type: kind.into(),
        code_html: "x".into(),
        ..Default::default()
    }
}

fn finding(line: i64, end_line: i64, sev: Severity, msg: &str) -> Finding {
    Finding {
        file: "pkg/a.go".into(),
        line,
        end_line,
        severity: sev,
        message: msg.into(),
        status: "open".into(),
        ..Default::default()
    }
}

#[test]
fn project_findings_onto_diff_single_line() {
    let mut files = vec![FileDiff {
        path: "pkg/a.go".into(),
        lines: Some(vec![line("10", "add"), line("11", "add")]),
        ..Default::default()
    }];
    let findings = [finding(11, 0, Severity::from("high"), "race <x>")];
    assert!(project_findings_onto_diff(&mut files, &findings));
    let lines = files[0].lines.as_ref().unwrap();
    assert!(lines[0].extra_html.is_empty());
    assert_eq!(
        lines[1].extra_html,
        r#"<div class="flag" data-sev="high"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4-4" stroke-linecap="round"/></svg><div><div class="ft">Reviewer flagged this region for human eyes · high</div><div class="fm">race &lt;x&gt;</div></div></div>"#
    );
    // Projecting again does not stack a second flag.
    assert!(!project_findings_onto_diff(&mut files, &findings));
}

#[test]
fn project_findings_onto_diff_range() {
    let mut files = vec![FileDiff {
        path: "pkg/a.go".into(),
        lines: Some(vec![
            line("10", "add"),
            line("11", "add"),
            line("12", "add"),
        ]),
        ..Default::default()
    }];
    let findings = [finding(10, 11, Severity::from("low"), "span")];
    assert!(project_findings_onto_diff(&mut files, &findings));
    let lines = files[0].lines.as_ref().unwrap();
    assert!(!lines[0].extra_html.is_empty() && !lines[1].extra_html.is_empty());
    assert!(
        lines[2].extra_html.is_empty(),
        "line 12 is outside the range"
    );
}

#[test]
fn project_findings_basename_fallback_and_filters() {
    let mut files = vec![FileDiff {
        path: "src/pkg/a.go".into(),
        lines: Some(vec![line("3", "ctx"), line("", "del"), line("4", "hunk")]),
        ..Default::default()
    }];
    let mut closed = finding(3, 0, Severity::default(), "closed");
    closed.status = "resolved".into();
    assert!(!project_findings_onto_diff(&mut files, &[closed]));
    let short = Finding {
        file: "./a.go".into(),
        ..finding(3, 0, Severity::default(), "m")
    };
    assert!(project_findings_onto_diff(&mut files, &[short]));
    let lines = files[0].lines.as_ref().unwrap();
    assert!(lines[0].extra_html.contains(r#"data-sev="medium""#));
    assert!(
        lines[0]
            .extra_html
            .contains("Reviewer flagged this region · medium")
    );
}

#[tokio::test]
async fn collect_metrics_and_impact() {
    let root = tmp();
    // Line 3 is the func def; it must be an "add" line for impact.
    write(
        root.path(),
        "a.go",
        "package p\n\nfunc UniqueWidgetName() {\n\tif true {\n\t\tfor i := 0; i < 3; i++ {\n\t\t\t_ = i\n\t\t}\n\t}\n}\n",
    );
    write(
        root.path(),
        "a_test.go",
        "package p\nfunc TestUniqueWidgetName(t *testing.T) {}\n",
    );
    write(
        root.path(),
        "b/use.go",
        "package b\nvar _ = UniqueWidgetName\n",
    );
    let r = root.path().to_str().unwrap();
    // Recorded metrics and impact for the same tree.
    assert_eq!(
        json(&collect_metrics(r)),
        go(
            r#"{"files":3,"loc":12,"avgComplexity":1.0499999999999998,"maxComplexity":3,"duplication":0,"todoCount":0,"testFiles":1}"#
        )
    );
    let files = vec![FileDiff {
        path: "a.go".into(),
        lines: Some(vec![line("3", "add")]),
        ..Default::default()
    }];
    assert_eq!(
        json(&compute_impact(&CancellationToken::new(), r, &files)),
        go(
            r#"[{"kind":"symbol","name":"UniqueWidgetName","path":"a.go","line":3,"detail":"func definition in diff","changed":true},{"kind":"api","name":"UniqueWidgetName","path":"b/use.go","line":2,"detail":"references changed func","changed":false},{"kind":"test","name":"a_test.go","path":"a_test.go","detail":"likely covers a.go","changed":false},{"kind":"test","name":"a_test.go","path":"./a_test.go","detail":"likely covers a.go","changed":false}]"#
        )
    );
}

fn json<T: serde::Serialize>(v: &T) -> serde_json::Value {
    recur_domain::json::canonical(serde_json::to_value(v).unwrap())
}

fn go(s: &str) -> serde_json::Value {
    serde_json::from_str(s).unwrap()
}

#[test]
fn collect_metrics_includes_major_languages() {
    let root = tmp();
    write(
        root.path(),
        "a.py",
        "def foo():\n    if True:\n        pass\n",
    );
    write(root.path(), "a.rs", "fn foo() { if true {} }\n");
    write(
        root.path(),
        "A.java",
        "class A { void foo() { if (true) {} } }\n",
    );
    write(root.path(), "bindings/gen.ts", "export const x = 1\n");
    assert_eq!(collect_metrics(root.path().to_str().unwrap()).files, 3);
}

#[test]
fn collect_metrics_finds_duplicate_blocks() {
    let root = tmp();
    let block = [
        "func prepareWidgetContext(cfg Config) error {",
        "\tif cfg.Timeout <= 0 {",
        "\t\tcfg.Timeout = defaultWidgetTimeout",
        "\t}",
        "\tclient := newWidgetClient(cfg)",
        "\tif err := client.Connect(); err != nil {",
        "\t\treturn fmt.Errorf(\"widget connect: %w\", err)",
        "\t}",
        "\treturn nil",
        "}",
    ]
    .join("\n");
    write(root.path(), "a.go", format!("package p\n\n{block}\n"));
    write(
        root.path(),
        "b.go",
        format!("package p\n\n{block}\n\nfunc other() {{}}\n"),
    );
    // Recorded metrics for the same tree.
    assert_eq!(
        json(&collect_metrics(root.path().to_str().unwrap())),
        go(
            r#"{"files":2,"loc":23,"avgComplexity":1.0499999999999998,"maxComplexity":3,"duplication":0.5217391304347826,"duplicates":[{"lines":6,"preview":"func prepareWidgetContext(cfg Config) error {","occurrences":[{"path":"a.go","line":3,"end":9,"text":"func prepareWidgetContext(cfg Config) error {"},{"path":"b.go","line":3,"end":9,"text":"func prepareWidgetContext(cfg Config) error {"}]},{"lines":5,"preview":"if cfg.Timeout \u003c= 0 {","occurrences":[{"path":"a.go","line":4,"end":9,"text":"if cfg.Timeout \u003c= 0 {"},{"path":"b.go","line":4,"end":9,"text":"if cfg.Timeout \u003c= 0 {"}]}],"todoCount":0,"testFiles":0}"#
        )
    );
}

#[tokio::test]
async fn analyze_delta() {
    let root = tmp();
    write(root.path(), "a.go", "package p\nfunc A() {}\n");
    let r = root.path().to_str().unwrap();
    let cancel = CancellationToken::new();
    let prev = analyze_root(&cancel, "p", "", r, vec![], None, false).await;
    assert_eq!(prev.summary, "1 files · 2 LOC · 0 hotspots · 0 impact");
    assert!(prev.id.starts_with("A-") && prev.when == "now");
    write(
        root.path(),
        "b.go",
        "package p\nfunc B() {\n\tif true {}\n}\n",
    );
    let next = analyze(
        &cancel,
        AnalyzeOpts {
            project_id: "p".into(),
            root: r.into(),
            previous: Some(prev),
            test_result: Some(TestResult {
                passed: false,
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await;
    let delta = next.delta.expect("delta");
    assert!(delta.loc > 0, "{delta:?}");
    assert_eq!(next.checks.len(), 1);
    assert_eq!(next.checks[0].detail, "Tester reported fail");
}

#[tokio::test]
async fn light_checks_report_agent_result_and_tools() {
    let root = tmp();
    let r = root.path().to_str().unwrap();
    let tr = TestResult {
        passed: true,
        summary: format!("  {}  ", "word ".repeat(40)),
        ..Default::default()
    };
    let checks = run_light_checks(None, r, Some(&tr)).await;
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].detail.chars().count(), 120);
    assert!(checks[0].detail.ends_with('…'));

    // A failing command reports its first output line.
    write(root.path(), "Gemfile", "");
    write(root.path(), "pyproject.toml", "");
    let ok = Ctx::new(None, std::time::Duration::from_secs(5));
    let fail = ok
        .run(r, "Fail", "sh", &["-c", "echo first; echo second; exit 3"])
        .await;
    assert_eq!((fail.detail.as_str(), fail.passed), ("first", false));
    let silent = ok.run(r, "Silent", "sh", &["-c", "exit 2"]).await;
    assert_eq!(silent.detail, "exit status 2");
    let pass = ok
        .run(
            r,
            "Pass",
            "sh",
            &["-c", "printf 'ok\\tpkg\\nok\\tother\\n' >&2"],
        )
        .await;
    assert_eq!((pass.detail.as_str(), pass.passed), ("2 passed", true));
    let missing = ok.run(r, "Missing", "no-such-tool-xyz", &[]).await;
    assert_eq!(
        (missing.detail.as_str(), missing.passed),
        ("skipped (tool unavailable)", true)
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    let killed = Ctx::new(Some(cancel), std::time::Duration::from_secs(5))
        .run(r, "Slow", "sleep", &["5"])
        .await;
    assert_eq!(
        (killed.detail.as_str(), killed.passed),
        ("signal: killed", false)
    );
}

#[test]
fn parse_line_digits() {
    assert_eq!(parse_line(" 42x"), 42);
    assert_eq!(parse_line(""), 0);
    assert_eq!(parse_line("-1"), 0);
}
