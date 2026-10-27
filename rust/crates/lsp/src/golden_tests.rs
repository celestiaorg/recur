//! Replays `tests/golden/go_lsp.json` and `go_managed.json`, recorded from
//! the Go `lsp` and `lsp/managed` packages, and drives a real session
//! through the manager.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_bundles::LanguageServer;
use serde_json::{Value, json};

use crate::managed;
use crate::*;

fn golden() -> Value {
    serde_json::from_str(include_str!("../tests/golden/go_lsp.json")).unwrap()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|s| s.as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn specs_json(specs: &[ServerSpec]) -> Value {
    let opt = |v: &[String]| if v.is_empty() { Value::Null } else { json!(v) };
    Value::Array(
        specs
            .iter()
            .map(|s| {
                json!({"key": s.key, "languages": opt(&s.languages), "command": s.command, "args": opt(&s.args),
                    "managed": s.managed, "fileGlob": opt(&s.file_glob), "env": opt(&s.env), "source": s.source,
                    "bundleName": s.bundle_name})
            })
            .collect(),
    )
}

#[test]
fn builtin_registry_matches_go() {
    assert_eq!(
        specs_json(&ServerRegistry::default().all_servers()),
        golden()["builtin"]
    );
}

#[test]
fn lookups_match_go() {
    let g = golden();
    let bundle: Vec<LanguageServer> = g["bundle"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| LanguageServer {
            id: b["id"].as_str().unwrap().into(),
            languages: strs(&b["languages"]),
            file_glob: strs(&b["fileGlob"]),
            command: b["command"].as_str().unwrap().into(),
            args: strs(&b["args"]),
            managed: b["managed"].as_str().unwrap_or_default().into(),
            env: strs(&b["env"]),
            bundle_name: b["bundleName"].as_str().unwrap_or_default().into(),
            ..Default::default()
        })
        .collect();
    for setup in g["lookups"].as_array().unwrap() {
        let r = ServerRegistry::default();
        if setup["setup"] != "builtin" {
            r.set_bundle_servers(&bundle);
        }
        r.set_disabled_servers(&strs(&setup["disabled"]));
        assert_eq!(
            specs_json(&r.all_servers()),
            setup["all"],
            "{}",
            setup["setup"]
        );
        for c in setup["cases"].as_array().unwrap() {
            let got = r.lookup_all(c["lang"].as_str().unwrap(), c["rel"].as_str().unwrap());
            let want = if c["out"].is_null() {
                json!([])
            } else {
                c["out"].clone()
            };
            assert_eq!(specs_json(&got), want, "{} {c}", setup["setup"]);
        }
    }
}

#[test]
fn uris_match_go() {
    let g = golden();
    for c in g["pathToURI"].as_array().unwrap() {
        assert_eq!(
            path_to_uri(c["path"].as_str().unwrap()),
            c["uri"].as_str().unwrap()
        );
    }
    for c in g["uriToPath"].as_array().unwrap() {
        let uri = c["uri"].as_str().unwrap();
        match uri_to_path(uri) {
            Ok(p) => {
                assert!(!c["err"].as_bool().unwrap(), "{uri}: want error, got {p}");
                assert_eq!(p, c["path"].as_str().unwrap(), "{uri}");
            }
            Err(e) => assert!(c["err"].as_bool().unwrap(), "{uri}: {e}"),
        }
    }
}

#[test]
fn frames_match_go_bytes() {
    let mut got = frame::frame(br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
    got.extend(frame::frame("é".as_bytes()));
    assert_eq!(
        String::from_utf8(got).unwrap(),
        golden()["frames"].as_str().unwrap()
    );
}

#[test]
fn session_info_json_matches_go() {
    let infos = [
        SessionInfo {
            id: "id".into(),
            server_key: "gopls".into(),
            root_uri: "file:///r".into(),
            root_path: "/r".into(),
            available: true,
            ..Default::default()
        },
        SessionInfo {
            reason: "no language server registered for x".into(),
            ..Default::default()
        },
        SessionInfo {
            id: "t".into(),
            server_key: "typescript-language-server".into(),
            available: true,
            vue_plugin_path: "/p".into(),
            ..Default::default()
        },
    ];
    assert_eq!(
        serde_json::to_value(infos).unwrap(),
        golden()["sessionInfo"]
    );
}

#[test]
fn managed_pins_match_go() {
    let g: Value = serde_json::from_str(include_str!("../tests/golden/go_managed.json")).unwrap();
    let r = &g["releases"];
    for (got, key) in [
        (managed::GOPLS_RELEASE, "gopls"),
        (managed::GOPLS_MODULE, "goplsModule"),
        (managed::MARKSMAN_RELEASE, "marksman"),
        (managed::RUFF_RELEASE, "ruff"),
        (managed::RUST_ANALYZER_RELEASE, "rustAnalyzer"),
        (managed::TAPLO_RELEASE, "taplo"),
        (
            managed::TYPESCRIPT_LANGUAGE_SERVER_RELEASE,
            "typescriptLanguageServer",
        ),
        (managed::TYPESCRIPT_RELEASE, "typescript"),
        (managed::BASED_PYRIGHT_RELEASE, "basedpyright"),
        (managed::YAML_LANGUAGE_SERVER_RELEASE, "yaml"),
        (managed::BASH_LANGUAGE_SERVER_RELEASE, "bash"),
        (managed::SVELTE_LANGUAGE_SERVER_RELEASE, "svelte"),
        (managed::VUE_LANGUAGE_SERVER_RELEASE, "vue"),
        (
            managed::VSCODE_LANGSERVERS_EXTRACTED_RELEASE,
            "vscodeLangserversExtracted",
        ),
    ] {
        assert_eq!(got, r[key], "{key}");
    }
    type Digest = fn(&str) -> Option<&'static str>;
    let tables: [(&str, Digest); 4] = [
        ("marksman", managed::marksman_sha256),
        ("ruff", managed::ruff_sha256),
        ("rustAnalyzer", managed::rust_analyzer_sha256),
        ("taplo", managed::taplo_sha256),
    ];
    for (key, sha) in tables {
        for (asset, digest) in g["sha256"][key].as_object().unwrap() {
            assert_eq!(sha(asset), digest.as_str(), "{key} {asset}");
        }
    }
    let p = managed::host_platform();
    let assets: [(&str, Result<&str, String>); 4] = [
        ("marksman", managed::marksman_asset(&p)),
        ("ruff", managed::ruff_asset(&p)),
        ("rustAnalyzer", managed::rust_analyzer_asset(&p)),
        ("taplo", managed::taplo_asset(&p)),
    ];
    // `hostAssets` was recorded on an arm64 Mac.
    for (key, got) in assets {
        let got = got
            .map(str::to_owned)
            .unwrap_or_else(|e| format!("error: {e}"));
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            assert_eq!(got, g["hostAssets"][key].as_str().unwrap(), "{key}");
        }
    }
    let env = &g["env"];
    for (got, key) in [
        (managed::ENV_GOPLS, "gopls"),
        (managed::ENV_RUST_ANALYZER, "rustAnalyzer"),
        (managed::ENV_MARKSMAN, "marksman"),
        (managed::ENV_TYPESCRIPT, "typescript"),
        (managed::ENV_BASED_PYRIGHT, "basedpyright"),
        (managed::ENV_RUFF, "ruff"),
        (managed::ENV_TAPLO, "taplo"),
        (managed::ENV_JSON, "json"),
        (managed::ENV_HTML, "html"),
        (managed::ENV_CSS, "css"),
        (managed::ENV_YAML, "yaml"),
        (managed::ENV_BASH, "bash"),
        (managed::ENV_SVELTE, "svelte"),
        (managed::ENV_VUE, "vue"),
    ] {
        assert_eq!(got, env[key], "{key}");
    }
    assert_eq!(
        managed::managed_bin_dir("/data", "gopls").to_string_lossy(),
        g["binDir"].as_str().unwrap()
    );
}

/// `cat` echoes framed messages back, which is enough of a server to drive
/// ensure → write → message → close → exit through the manager.
#[tokio::test]
async fn manager_runs_a_session_end_to_end() {
    let events: Arc<Mutex<Vec<LspEvent>>> = Arc::default();
    let log = Arc::clone(&events);
    let mgr = Manager::new(Some(Arc::new(move |ev| log.lock().push(ev))), "");
    mgr.registry().set_bundle_servers(&[LanguageServer {
        id: "echo".into(),
        languages: vec!["echo".into()],
        command: "cat".into(),
        bundle_name: "Echo".into(),
        ..Default::default()
    }]);
    let root = tempfile::tempdir().unwrap();
    let root_s = root.path().to_string_lossy().into_owned();
    std::fs::write(root.path().join("a b.echo"), "").unwrap();
    let res = service::ensure_lsp(&mgr, "W1", &root_s, "echo", "a b.echo")
        .await
        .unwrap();
    assert!(res.session.available, "{res:?}");
    assert_eq!(res.session.server_key, "echo");
    assert_eq!(res.language_id, "echo");
    let canon = canon_path(&root_s);
    assert_eq!(res.session.root_path, canon);
    assert_eq!(res.session.root_uri, path_to_uri(&canon));
    assert_eq!(res.file_uri, format!("{}/a%20b.echo", path_to_uri(&canon)));

    let again = mgr.ensure("W1", &root_s, "echo", "").await.unwrap();
    assert_eq!(again.id, res.session.id, "the live session is reused");

    service::write_lsp(
        &mgr,
        &res.session.id,
        r#"{"jsonrpc":"2.0","method":"ping"}"#,
    )
    .await
    .unwrap();
    for _ in 0..200 {
        if !events.lock().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        events.lock()[0],
        LspEvent::Message {
            id: res.session.id.clone(),
            message: r#"{"jsonrpc":"2.0","method":"ping"}"#.into()
        }
    );
    assert_eq!(
        service::write_lsp(&mgr, " ", "{}").await.unwrap_err(),
        "missing lsp session id"
    );
    assert_eq!(
        mgr.write("nope", "{}").await.unwrap_err(),
        "unknown lsp session"
    );

    mgr.close_workspace("W1");
    assert!(mgr.session_ids().is_empty());
    for _ in 0..300 {
        if events
            .lock()
            .iter()
            .any(|e| matches!(e, LspEvent::Exit { .. }))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        events.lock().last(),
        Some(&LspEvent::Exit {
            id: res.session.id.clone()
        })
    );
}

#[tokio::test]
async fn ensure_soft_degrades() {
    let mgr = Manager::new(None, "");
    let res = service::ensure_lsp(&mgr, "W", "/tmp", "cobol", "")
        .await
        .unwrap();
    assert!(!res.session.available);
    assert_eq!(
        res.session.reason,
        "no language server registered for cobol"
    );
    mgr.registry().set_bundle_servers(&[LanguageServer {
        id: "ghost".into(),
        languages: vec!["ghost".into()],
        command: "definitely-not-a-binary-recur".into(),
        ..Default::default()
    }]);
    let res = service::ensure_lsp(&mgr, "W", "/tmp", "ghost", "")
        .await
        .unwrap();
    assert_eq!(
        res.session.reason,
        "definitely-not-a-binary-recur not found on PATH"
    );
    assert_eq!(res.session.server_key, "ghost");
    assert!(
        service::ensure_lsp(&mgr, "W", "/tmp", "ghost", "../x")
            .await
            .is_err()
    );
}

#[test]
fn list_language_servers_shape() {
    let mgr = Manager::new(None, "");
    let list = service::list_language_servers(&mgr);
    assert_eq!(list.len(), builtin_servers().len());
    let gopls = serde_json::to_value(&list[0]).unwrap();
    assert_eq!(gopls["id"], "gopls");
    assert_eq!(gopls["source"], "builtin");
    assert!(gopls.get("managed").is_none() && gopls.get("bundleName").is_none());
    assert_eq!(gopls["languages"][0], "go");
}

#[test]
fn safe_join_rules() {
    assert_eq!(service::safe_join("/r", "").unwrap(), "/r");
    assert_eq!(service::safe_join("/r", "/a/b").unwrap(), "/r/a/b");
    assert_eq!(service::safe_join("/r/", "./a").unwrap(), "/r/a");
    assert_eq!(
        service::safe_join("/r", "a/../b").unwrap_err(),
        "path escapes root"
    );
}
