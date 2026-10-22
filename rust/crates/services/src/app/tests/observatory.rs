//! `observatory_test.go`.

use recur_domain::{FINDING_SOURCE_USER, Finding, FindingStatus, Project, Severity, Workspace};

use super::{Fx, write};

#[tokio::test(flavor = "multi_thread")]
async fn code_and_observatory_services() {
    let fx = Fx::new().await;
    let root = fx.temp();
    write(format!("{root}/main.go"), "package main\nfunc main() {}\n");
    fx.store()
        .add_project(Project {
            id: "p-obs".into(),
            name: "Obs".into(),
            repo_path: root,
            base_branch: "main".into(),
            ..Default::default()
        })
        .unwrap();
    let code = &fx.s.code;
    assert!(!code.list_tree("p-obs", "", "").await.unwrap().is_empty());
    assert!(!code.read_file("p-obs", "", "main.go").await.unwrap().content.is_empty());
    assert!(!code.outline("p-obs", "", "main.go").await.unwrap().is_empty());

    fx.store()
        .add_workspace(Workspace {
            id: "w-obs".into(),
            project_id: "p-obs".into(),
            goal: "Obs loop".into(),
            ..Default::default()
        })
        .unwrap();
    fx.store().add_finding(Finding {
        id: "f1".into(),
        workspace_id: "w-obs".into(),
        severity: Severity::Medium,
        message: "example".into(),
        status: "open".into(),
        ..Default::default()
    });
    let list = fx.s.observatory.list_findings("w-obs").await.unwrap();
    assert_eq!(list.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["f1"]);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn create_finding() {
    let fx = Fx::new().await;
    fx.store()
        .add_workspace(Workspace {
            id: "w-note".into(),
            project_id: "p".into(),
            goal: "g".into(),
            ..Default::default()
        })
        .unwrap();
    let obs = &fx.s.observatory;
    let got = obs
        .create_finding("w-note", "src/main.go", 12, "  check the timeout  ", 0)
        .await
        .unwrap();
    assert_eq!(
        (got.file.as_str(), got.line, got.end_line, got.message.as_str()),
        ("src/main.go", 12, 0, "check the timeout")
    );
    assert_eq!(got.status, FindingStatus::Open.as_str());
    assert_eq!(got.severity, Severity::Info);
    assert_eq!(got.source, FINDING_SOURCE_USER);
    let list = obs.list_findings("w-note").await.unwrap();
    assert_eq!(list.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), [got.id.as_str()]);

    for (name, ws, file, line, msg) in [
        ("empty message", "w-note", "a.go", 1, "  "),
        ("empty file", "w-note", "", 1, "x"),
        ("negative line", "w-note", "a.go", -1, "x"),
        ("missing workspace", "nope", "a.go", 1, "x"),
    ] {
        assert!(obs.create_finding(ws, file, line, msg, 0).await.is_err(), "{name}");
    }
    let span = obs.create_finding("w-note", "src/main.go", 10, "this span", 14).await.unwrap();
    assert_eq!((span.line, span.end_line), (10, 14));
    let same = obs.create_finding("w-note", "src/main.go", 8, "same line", 8).await.unwrap();
    assert_eq!(same.end_line, 0);
    for (name, line, end) in [("negative end", 1, -1), ("end before start", 10, 5), ("end without start", 0, 5)] {
        assert!(obs.create_finding("w-note", "a.go", line, "x", end).await.is_err(), "{name}");
    }
    fx.close().await;
}
