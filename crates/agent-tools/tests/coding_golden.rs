//! Snapshot of the coding-tool cases (`golden/coding_calls.json`) in a fixture
//! worktree. Paths are normalized to `$WT`. Edit the fixture by hand when the
//! behavior changes on purpose.

mod common;

use std::sync::Arc;

use recur_agent_tools::{Call, HostError, PermissionVerdict, Permitter, TodoStore, Tool};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    tool: String,
    args: String,
    bypass: bool,
    session: String,
    result: String,
    error: String,
}

/// The generator's `denyAll` permitter.
struct DenyAll;

#[async_trait::async_trait]
impl Permitter for DenyAll {
    async fn request_permission(
        &self,
        _cancel: &CancellationToken,
        _ws: &str,
        _agent: &str,
        tool: &str,
        input: &str,
    ) -> Result<PermissionVerdict, HostError> {
        match input {
            r#"{"command":"echo blank"}"# => Ok(PermissionVerdict::default()),
            r#"{"command":"echo err"}"# => Err(HostError::new("broker down")),
            r#"{"command":"echo ok"}"# => Ok(PermissionVerdict {
                allow: true,
                reason: String::new(),
            }),
            _ => Ok(PermissionVerdict {
                allow: false,
                reason: format!("no {tool}"),
            }),
        }
    }
}

fn tool(name: &str) -> Option<Arc<dyn Tool>> {
    recur_agent_tools::coding_tools()
        .into_iter()
        .find(|t| t.name() == name)
}

#[tokio::test]
async fn coding_tools_match_go() {
    // bash -lc sources login profiles; an empty HOME keeps outputs portable,
    // as in the generator. This binary runs no other test.
    let home = tempfile::tempdir().unwrap();
    #[allow(unsafe_code)]
    // SAFETY: the only test in this binary; no other thread reads the env yet.
    unsafe {
        std::env::set_var("HOME", home.path());
    }

    let wt_dir = tempfile::tempdir().unwrap();
    let wt = wt_dir.path().to_str().unwrap().to_string();
    let files: &[(&str, &[u8])] = &[
        ("a.go", b"hello world\n"),
        ("dup.txt", b"aaa\n"),
        ("nl.txt", b"one\ntwo\nthree"),
        ("empty.txt", b""),
        ("bin.dat", b"ab\x00cd"),
        ("latin1.txt", b"caf\xe9 \xe2\x82\n"),
        ("sub/deep.txt", b"deep\n"),
    ];
    for (name, body) in files {
        let p = wt_dir.path().join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    std::fs::write(
        wt_dir.path().join("long.txt"),
        format!("{}\nshort\n", "é".repeat(2005)),
    )
    .unwrap();
    std::fs::write(wt_dir.path().join("many.txt"), "x\n".repeat(2100)).unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), wt_dir.path().join("outlink")).unwrap();
    std::os::unix::fs::symlink("sub", wt_dir.path().join("inlink")).unwrap();
    let eval = std::fs::canonicalize(&wt)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let norm = |s: &str| s.replace(&eval, "$WT").replace(&wt, "$WT");

    let cases: Vec<Case> = serde_json::from_str(include_str!("golden/coding_calls.json")).unwrap();
    let todos = Arc::new(TodoStore::default());
    let mut failures = Vec::new();
    let mut ran = 0;
    for c in &cases {
        let t = tool(&c.tool).unwrap_or_else(|| panic!("no tool {}", c.tool));
        ran += 1;
        let call = Call {
            worktree: wt.clone(),
            args: c.args.clone(),
            bypass_perms: c.bypass,
            permitter: Some(Arc::new(DenyAll)),
            todos: (c.session != "nostore").then(|| todos.clone()),
            session_id: if c.session == "nostore" {
                "s1".into()
            } else {
                c.session.clone()
            },
            ..Default::default()
        };
        let (result, error) = match t.execute(call).await {
            Ok(r) => (norm(&r), String::new()),
            Err(e) => (String::new(), norm(&e.0)),
        };
        // Output alongside a context error: the error wins.
        let want_result = if c.error.is_empty() {
            c.result.as_str()
        } else {
            ""
        };
        if !common::same_result(&result, want_result) || !common::same_error(&error, &c.error) {
            failures.push(format!(
                "{} {}\n  want result={:?} error={:?}\n  got  result={:?} error={:?}",
                c.tool, c.args, want_result, c.error, result, error
            ));
        }
    }
    assert_eq!(ran, 82, "replayed cases");
    assert!(
        failures.is_empty(),
        "{} of {ran} cases differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
