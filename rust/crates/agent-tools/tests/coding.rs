//! Port of `coding_test.go`, plus Bash cancellation and WaitDelay.

use std::sync::Arc;
use std::time::{Duration, Instant};

use recur_agent_tools::coding::bash::BashTool;
use recur_agent_tools::{
    Call, HostError, PermissionVerdict, Permitter, Registry, TodoStore, Tool, advertised_name,
};
use tokio_util::sync::CancellationToken;

fn registry() -> Registry {
    Registry::new(recur_agent_tools::coding_tools())
}

fn call(wt: &str, args: &str) -> Call {
    Call {
        worktree: wt.into(),
        args: args.into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn read_edit_write_sandbox() {
    let wt = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let w = wt.path().to_str().unwrap();
    std::fs::write(wt.path().join("a.go"), "hello world\n").unwrap();
    let reg = registry();

    let read = reg.get("Read").unwrap();
    let got = read
        .execute(call(w, r#"{"file_path":"a.go"}"#))
        .await
        .unwrap();
    assert!(got.contains("hello world"), "{got}");
    let escape = format!(r#"{{"file_path":"{}/x"}}"#, outside.path().display());
    assert!(read.execute(call(w, &escape)).await.is_err());

    let edit = reg.get("Edit").unwrap();
    edit.execute(call(
        w,
        r#"{"file_path":"a.go","old_string":"hello","new_string":"hi"}"#,
    ))
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(wt.path().join("a.go")).unwrap(),
        "hi world\n"
    );
    std::fs::write(wt.path().join("dup.txt"), "aaa\n").unwrap();
    assert!(
        edit.execute(call(
            w,
            r#"{"file_path":"dup.txt","old_string":"a","new_string":"A"}"#
        ))
        .await
        .is_err()
    );

    let write = reg.get("Write").unwrap();
    write
        .execute(call(w, r#"{"file_path":"b.txt","content":"new"}"#))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(wt.path().join("b.txt")).unwrap(),
        "new"
    );
    assert!(
        write
            .execute(call(w, r#"{"file_path":"../nope.txt","content":"x"}"#))
            .await
            .is_err()
    );
    assert!(
        write
            .execute(call(w, r#"{"file_path":"wipe.go","contents":"x"}"#))
            .await
            .is_err(),
        "typo'd contents must not wipe the file"
    );
    write
        .execute(call(w, r#"{"file_path":"empty.txt","content":""}"#))
        .await
        .unwrap();
}

#[tokio::test]
async fn grep_and_glob() {
    let wt = tempfile::tempdir().unwrap();
    let w = wt.path().to_str().unwrap();
    std::fs::write(wt.path().join("a.go"), "func Alpha()\n").unwrap();
    std::fs::create_dir_all(wt.path().join("sub")).unwrap();
    std::fs::write(wt.path().join("sub/b.txt"), "func Alpha()\n").unwrap();
    let reg = registry();
    let got = reg
        .get("Grep")
        .unwrap()
        .execute(call(w, r#"{"pattern":"Alpha","glob":"*.go"}"#))
        .await
        .unwrap();
    assert!(got.contains("a.go:1:func Alpha()"), "{got}");
    let got = reg
        .get("Glob")
        .unwrap()
        .execute(call(w, r#"{"pattern":"*.go"}"#))
        .await
        .unwrap();
    assert!(got.contains("a.go"), "{got}");
}

#[tokio::test]
async fn grep_and_glob_ripgrep() {
    let Ok(_) = recur_code::rg::locate(&recur_store::paths::data_dir().unwrap_or_default()) else {
        eprintln!("rg not found");
        return;
    };
    let wt = tempfile::tempdir().unwrap();
    let w = wt.path().to_str().unwrap();
    let st = std::process::Command::new("git")
        .args(["init", "-q", w])
        .status()
        .unwrap();
    assert!(st.success());
    for (name, body) in [
        (".gitignore", "ignored.go\n"),
        ("ignored.go", "func Alpha()\n"),
        (".github/ci.yml", "Alpha: true\n"),
        ("sub/deep/b.go", "one\nfunc Alpha()\nthree\n"),
        ("sub/deep/notes.txt", "Alpha\n"),
        ("sub/deep/regexp.txt", "a(\n"),
    ] {
        let p = wt.path().join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    let reg = registry();
    let grep = reg.get("Grep").unwrap();
    let run = |args: &'static str| {
        let grep = grep.clone();
        let w = w.to_string();
        async move { grep.execute(call(&w, args)).await.unwrap() }
    };
    let got = run(r#"{"pattern":"Alpha"}"#).await;
    assert!(
        got.contains(".github/ci.yml:1:Alpha: true") && !got.contains("ignored.go"),
        "{got}"
    );
    let got = run(r#"{"pattern":"Alpha","glob":"*.go"}"#).await;
    assert!(
        got.contains("sub/deep/b.go") && !got.contains("ignored.go"),
        "{got}"
    );
    assert_eq!(run(r#"{"pattern":"alpha"}"#).await, "No matches.");
    let got = run(r#"{"pattern":"alpha","-i":true,"path":"sub","glob":"deep/*.go"}"#).await;
    assert_eq!(got.trim(), "sub/deep/b.go:2:func Alpha()");
    let got = run(r#"{"pattern":"Alpha","path":"sub/deep/b.go","-C":1}"#).await;
    assert!(
        got.contains("sub/deep/b.go-1-one") && got.contains("sub/deep/b.go-3-three"),
        "{got}"
    );
    let got = run(r#"{"pattern":"Alpha","path":"sub","output_mode":"files_with_matches"}"#).await;
    assert!(
        got.contains("sub/deep/notes.txt") && !got.contains(':'),
        "{got}"
    );
    let err = grep
        .execute(call(w, r#"{"pattern":"a("}"#))
        .await
        .unwrap_err();
    assert!(err.0.contains("regex parse error"), "{err}");

    let glob = reg.get("Glob").unwrap();
    let got = glob
        .execute(call(w, r#"{"pattern":"*.go"}"#))
        .await
        .unwrap();
    assert!(
        got.contains("sub/deep/b.go") && !got.contains("ignored.go"),
        "{got}"
    );
    let got = glob
        .execute(call(w, r#"{"pattern":"deep/*.txt","path":"sub"}"#))
        .await
        .unwrap();
    assert!(got.contains("sub/deep/notes.txt"), "{got}");
}

struct DenyPermits;

#[async_trait::async_trait]
impl Permitter for DenyPermits {
    async fn request_permission(
        &self,
        _cancel: &CancellationToken,
        _ws: &str,
        _agent: &str,
        _tool: &str,
        _input: &str,
    ) -> Result<PermissionVerdict, HostError> {
        Ok(PermissionVerdict {
            allow: false,
            reason: "no".into(),
        })
    }
}

#[tokio::test]
async fn bash_and_todo() {
    let wt = tempfile::tempdir().unwrap();
    let w = wt.path().to_str().unwrap();
    let reg = registry();
    let bash = reg.get("Bash").unwrap();
    let got = bash
        .execute(Call {
            bypass_perms: true,
            ..call(w, r#"{"command":"echo hi"}"#)
        })
        .await
        .unwrap();
    assert!(got.contains("hi"), "{got}");
    assert!(
        bash.execute(Call {
            permitter: Some(Arc::new(DenyPermits)),
            ..call(w, r#"{"command":"echo x"}"#)
        })
        .await
        .is_err()
    );

    let todo = reg.get("TodoWrite").unwrap();
    let got = todo
        .execute(Call {
            session_id: "s1".into(),
            todos: Some(Arc::new(TodoStore::default())),
            args: r#"{"todos":[{"id":"1","content":"do","status":"pending"}]}"#.into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(got.contains(r#""id":"1""#), "{got}");
}

#[tokio::test]
async fn bash_cancel_kills_the_process_group() {
    let wt = tempfile::tempdir().unwrap();
    let marker = wt.path().join("survived");
    let cancel = CancellationToken::new();
    let cmd = format!(
        r#"{{"command":"(sleep 2; touch {}) & sleep 30"}}"#,
        marker.display()
    );
    let c = Call {
        bypass_perms: true,
        cancel: cancel.clone(),
        ..call(wt.path().to_str().unwrap(), &cmd)
    };
    let started = Instant::now();
    let task = tokio::spawn(async move { BashTool.execute(c).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel.cancel();
    let err = task.await.unwrap().unwrap_err();
    assert_eq!(err.0, "context canceled");
    assert!(started.elapsed() < Duration::from_secs(5));
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(!marker.exists(), "background child outlived the group kill");
}

#[tokio::test]
async fn bash_wait_delay_closes_pipes_held_by_a_detached_child() {
    let wt = tempfile::tempdir().unwrap();
    // A child in its own session keeps stdout open after bash exits.
    let c = Call {
        bypass_perms: true,
        ..call(
            wt.path().to_str().unwrap(),
            r#"{"command":"echo start; perl -e 'use POSIX; setsid(); sleep 5' &"}"#,
        )
    };
    let started = Instant::now();
    let got = BashTool.execute(c).await.unwrap();
    // Login profiles may add stderr between the two.
    assert!(got.starts_with("start\n"), "{got}");
    assert!(
        got.ends_with("\nexit: exec: WaitDelay expired before I/O complete"),
        "{got}"
    );
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(2) && took < Duration::from_secs(4),
        "{took:?}"
    );
}

#[test]
fn registry_allowlist() {
    let reg = registry();
    let got = reg.list(Some(&[
        "Read".to_string(),
        "mcp__recur__ask_user".to_string(),
    ]));
    assert_eq!(got.iter().map(|t| t.name()).collect::<Vec<_>>(), ["Read"]);
    assert_eq!(
        advertised_name(got[0].as_ref(), &["Read".to_string()]),
        "Read"
    );
}

#[test]
fn registry_resolves_prefixed_names_and_advertises_them() {
    let reg = Registry::new(recur_agent_tools::host_tools());
    assert_eq!(reg.get("mcp__recur__ask_user").unwrap().name(), "ask_user");
    let allowed = ["mcp__recur__ask_user".to_string()];
    let listed = reg.list(Some(&allowed));
    assert_eq!(
        listed.iter().map(|t| t.name()).collect::<Vec<_>>(),
        ["ask_user"]
    );
    assert_eq!(
        advertised_name(listed[0].as_ref(), &allowed),
        "mcp__recur__ask_user"
    );
    assert_eq!(reg.list(None).len(), 42);
    assert!(recur_agent_tools::halt_after("mcp__recur__ask_user", ""));
    assert!(recur_agent_tools::halt_after(
        "switch_mode",
        "Switched; remounting"
    ));
    assert!(!recur_agent_tools::halt_after("switch_mode", "Refused"));
    assert!(!recur_agent_tools::halt_after("done", "remounting"));
}
