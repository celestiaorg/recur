//! install_bundle: lint errors refuse before any approval, every install
//! (even under bypass, even a remembered one) asks the human, and only an
//! approval copies the bundle into the user root.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use recur_agent_tools::Host;
use recur_agent_tools::names::mcp;
use recur_domain::PermissionDecision;
use tokio_util::sync::CancellationToken;

use super::test_support::*;
use crate::host::EngineHost;
use crate::testing::{TestOptions, wait_for};

const GOOD: &str = r##"[bundle]
name = "Mine"
id = "me.mine"

[[command]]
id = "me.mine.hello"
name = "Hello"
body = "#!/bin/sh\necho hi\n"

[[languageServer]]
id = "me.gopls"
languages = ["go"]
command = "sh"
args = ["-c", "gopls serve"]

[[workflow]]
id = "me.workflows.check"
name = "Check"
conductor = "recur.roles.maestro"
entry = "check"

[[workflow.phase]]
id = "check"
uses = "run"
command = "mise run check"
"##;

fn write(root: &str, rel: &str, body: &str) {
    let p = Path::new(root).join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

async fn answer(fx: &Fx, allow: bool, remember: bool) -> serde_json::Value {
    let eng = fx.eng().clone();
    wait_for(Duration::from_secs(5), "permission request", || {
        !eng.pending_permissions().is_empty()
    })
    .await;
    let req = eng.pending_permissions().remove(0);
    assert_eq!(req.tool, mcp::INSTALL_BUNDLE);
    assert!(eng.answer_permission(
        &req.id,
        PermissionDecision {
            allow,
            remember,
            reason: if allow { String::new() } else { "no".into() },
        }
    ));
    serde_json::from_str(&req.input).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn install_bundle_lints_then_requires_approval() {
    let fx = Fx::with(None, TestOptions::default());
    let mut set = fx.te.store.settings();
    set.bypass_permissions = true;
    fx.te.store.set_settings(set);
    let ws = fx.workspace("bundles").await;
    let host = Arc::new(EngineHost::new(Arc::downgrade(fx.eng())));
    let install = |path: &'static str| {
        let (h, id) = (host.clone(), ws.id.clone());
        tokio::spawn(async move {
            h.install_bundle(&CancellationToken::new(), &id, "PA", path)
                .await
        })
    };
    let user = fx.eng().bundle_roots().user_dir.clone();
    let installed = Path::new(&user).join("Mine/bundle.toml");

    write(
        &ws.worktree_path,
        "Bad/bundle.toml",
        "[bundle]\nid = \"x\"\n[[workflow]]\nid = \"x.w\"\nname = \"W\"\nconductor = \"recur.roles.maestro\"\nentry = \"c\"\n[[workflow.phase]]\nid = \"c\"\nuses = \"run\"\n",
    );
    let err = install("Bad").await.unwrap().unwrap_err().0;
    assert!(err.starts_with("lint failed, nothing installed"), "{err}");
    assert!(err.contains("command is required"), "{err}");
    assert!(
        fx.eng().pending_permissions().is_empty(),
        "lint errors never ask"
    );
    assert!(!Path::new(&user).join("Bad").exists());

    let err = install("../outside").await.unwrap().unwrap_err().0;
    assert!(err.contains("not a directory in the worktree"), "{err}");

    write(&ws.worktree_path, "Mine/bundle.toml", GOOD);
    let task = install("Mine");
    wait_for(Duration::from_secs(5), "permission request", || {
        !fx.eng().pending_permissions().is_empty()
    })
    .await;
    let staged = recur_bundles::discover_catalog(fx.eng().bundle_roots()).unwrap();
    assert!(
        !staged.language_servers.iter().any(|l| l.id == "me.gopls")
            && !staged.loops.iter().any(|l| l.id == "me.workflows.check"),
        "the stage is inert while the human decides"
    );
    let shown = answer(&fx, false, false).await;
    assert_eq!(shown["name"], "Mine");
    assert_eq!(shown["replaces"], false);
    assert_eq!(shown["workflows"][0], "me.workflows.check");
    assert_eq!(shown["commands"][0], "me.mine.hello");
    assert_eq!(shown["languageServers"][0], "me.gopls");
    assert_eq!(
        shown["shell"],
        serde_json::json!([
            "command me.mine.hello",
            "workflow me.workflows.check phase check: mise run check",
            "language server me.gopls: sh -c gopls serve"
        ])
    );
    assert_eq!(task.await.unwrap().unwrap_err().0, "permission denied: no");
    assert!(!installed.exists(), "a denial installs nothing");
    assert!(
        std::fs::read_dir(&user).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with('.')),
        "a denial leaves no stage"
    );

    let task = install("Mine");
    answer(&fx, true, true).await;
    let out: serde_json::Value = serde_json::from_str(&task.await.unwrap().unwrap()).unwrap();
    assert_eq!(out["installed"], true);
    assert_eq!(std::fs::read_to_string(&installed).unwrap(), GOOD);

    write(
        &ws.worktree_path,
        "Mine/bundle.toml",
        &GOOD.replace("Check", "Check 2"),
    );
    let task = install("Mine");
    wait_for(Duration::from_secs(5), "permission request", || {
        !fx.eng().pending_permissions().is_empty()
    })
    .await;
    write(&ws.worktree_path, "Mine/bundle.toml", "edited while asking");
    let shown = answer(&fx, true, false).await;
    assert_eq!(
        shown["replaces"], true,
        "overwrites ask again despite remember"
    );
    task.await.unwrap().unwrap();
    assert!(
        std::fs::read_to_string(&installed)
            .unwrap()
            .contains("Check 2")
    );

    let listed: serde_json::Value =
        serde_json::from_str(&host.reload_bundles(&ws.id, "PA").await.unwrap()).unwrap();
    assert!(
        listed["workflows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == "me.workflows.check" && w["source"] == "user"),
        "{listed}"
    );

    write(&ws.worktree_path, "Mine/bundle.toml", GOOD);
    let task = install("Mine");
    wait_for(Duration::from_secs(5), "permission request", || {
        !fx.eng().pending_permissions().is_empty()
    })
    .await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(
        std::fs::read_dir(&user).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with('.')),
        "a dropped install leaves no stage"
    );
    fx.te.close().await;
}
