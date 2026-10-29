//! The real services and the mock bridge agree: the mock answers from the
//! e2e seed, which is a dump of the same demo-seeded store the real services
//! open here, so every read returns the same JSON and a missing id fails on
//! both. One divergence comes from the mock itself and is pinned
//! here so a view handles both: GetPR's commits carry diffs only on the mock
//! (the real service leaves them to GetCommitDiff). GetEscalation for a loop
//! with none answers an empty escalation on both. GetSettings normalizes the
//! branch prefix only on the real side.
#![cfg(feature = "mock")]

use std::sync::Arc;

use recur_engine::GitSetup;
use recur_services::mock::{MockBridge, MockOptions};
use recur_services::{AppConfig, AppOptions, AppServices, Services};
use recur_store::Store;
use serde::Serialize;
use serde_json::Value;

async fn real(root: &std::path::Path) -> Arc<AppServices> {
    let config = AppConfig::under(root);
    std::fs::create_dir_all(&config.worktrees_dir).unwrap();
    AppServices::new(
        Arc::new(Store::new_in_memory()),
        config.clone(),
        AppOptions {
            pin: Some(None),
            forges: Some(Vec::new()),
            trackers: Some(Vec::new()),
            git: Some(GitSetup {
                worktree_root: config.worktrees_dir.to_string_lossy().into_owned(),
                ..GitSetup::default()
            }),
        },
    )
    .await
}

fn json<T: Serialize>(v: recur_services::Result<T>) -> Result<Value, String> {
    v.map(|v| serde_json::to_value(v).unwrap())
        .map_err(|e| e.message)
}

/// Paths where the two differ, `a.b[2].c` style.
fn diff(path: &str, a: &Value, b: &Value, out: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for k in x.keys().chain(y.keys().filter(|k| !x.contains_key(*k))) {
                diff(
                    &format!("{path}.{k}"),
                    x.get(k).unwrap_or(&Value::Null),
                    y.get(k).unwrap_or(&Value::Null),
                    out,
                );
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                diff(&format!("{path}[{i}]"), p, q, out);
            }
        }
        _ if a != b => out.push(format!("{path}: real={a} mock={b}")),
        _ => {}
    }
}

async fn calls(s: &Services) -> Vec<(String, Result<Value, String>)> {
    let ws = "LOOP-2231";
    let mut out: Vec<(String, Result<Value, String>)> = Vec::new();
    for id in [
        "LOOP-2218",
        "LOOP-2224",
        "LOOP-2229",
        "LOOP-2231",
        "LOOP-2235",
    ] {
        out.push((
            format!("ListLoopAgents({id})"),
            json(s.agents.list_loop_agents(id).await),
        ));
        out.push((
            format!("GetWorkspaceFeed({id})"),
            json(s.workspaces.get_workspace_feed(id).await),
        ));
    }
    for id in ["471", "479", "482"] {
        out.push((format!("GetPR({id})"), json(s.prs.get_pr(id).await)));
    }
    out.extend(
        [
            ("ListProjects", json(s.projects.list_projects().await)),
            ("ListWorkspaces", json(s.workspaces.list_workspaces().await)),
            ("GetWorkspace", json(s.workspaces.get_workspace(ws).await)),
            (
                "GetEscalation",
                json(s.escalation.get_escalation("LOOP-2218").await),
            ),
            ("ListPRs", json(s.prs.list_prs().await)),
            ("GetInbox", json(s.inbox.get_inbox().await)),
            ("GetPolicy", json(s.policy.get_policy().await)),
            ("GetSettings", json(s.settings.get_settings().await)),
            ("ListFindings", json(s.observatory.list_findings(ws).await)),
            ("GetResource", json(s.settings.get_resource().await)),
            (
                "GetWorkspace(missing)",
                json(s.workspaces.get_workspace("nope").await),
            ),
            ("GetPR(missing)", json(s.prs.get_pr("nope").await)),
        ]
        .map(|(n, v)| (n.to_owned(), v)),
    );
    out
}

/// The two stamps the e2e seed carries: seeddump marks every project a git
/// checkout (the demo paths are not real repos) and the mock gives each
/// loop a `/mock/worktrees/<id>` path.
fn stamp_like_seed(v: &mut Value) {
    match v {
        Value::Array(rows) => rows.iter_mut().for_each(stamp_like_seed),
        Value::Object(o) => {
            if o.contains_key("repoPath") {
                o.insert("isGit".into(), Value::Bool(true));
            }
            if o.contains_key("projectId")
                && o.contains_key("worktreePath")
                && o["worktreePath"] == ""
                && let Some(id) = o.get("id").and_then(Value::as_str)
            {
                let path = format!("/mock/worktrees/{id}");
                o.insert("worktreePath".into(), Value::String(path));
            }
        }
        _ => {}
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn real_and_mock_agree_on_reads() {
    let dir = tempfile::tempdir().unwrap();
    let real = real(dir.path()).await;
    let mock = MockBridge::fresh(MockOptions::default());
    let (r, m) = (calls(&real.services()).await, calls(&mock.services()).await);
    let mut out = Vec::new();
    for ((name, a), (_, b)) in r.iter().zip(&m) {
        match (a, b) {
            (Ok(a), Ok(b)) => {
                let mut a = a.clone();
                stamp_like_seed(&mut a);
                let mut b = b.clone();
                if name.starts_with("GetPR(") {
                    lazy_commit_diffs(&mut b);
                }
                if name == "GetSettings" {
                    settings_like_real(&mut a, &mut b);
                }
                diff(name, &a, &b, &mut out);
            }
            (Err(_), Err(_)) => {}
            _ => out.push(format!("{name}: real={a:?} mock={b:?}")),
        }
    }
    assert!(out.is_empty(), "{}", out.join("\n"));

    let pr = real.services().prs.get_pr("482").await.unwrap();
    let commits = pr.commits.unwrap_or_default();
    assert!(
        !commits.is_empty()
            && commits
                .iter()
                .all(|c| c.files.as_ref().is_none_or(Vec::is_empty))
    );
    let ws = "LOOP-2231";
    let esc = real.services().escalation.get_escalation(ws).await.unwrap();
    assert!(esc.id.is_empty(), "{esc:?}");
    let esc = mock.services().escalation.get_escalation(ws).await.unwrap();
    assert!(esc.id.is_empty(), "{esc:?}");
    real.shutdown().await;
}

/// The demo seed reads this machine's `~/.codex` login (seeddump runs with
/// an empty HOME), and GetSettings returns the normalized branch prefix
/// where the mock returns the stored (unset) one.
fn settings_like_real(real: &mut Value, mock: &mut Value) {
    for v in [&mut *real, &mut *mock] {
        if let Some(o) = v.as_object_mut() {
            o.remove("codexChatGPTAuth");
        }
    }
    if mock["branchNamePrefix"].is_null() {
        mock["branchNamePrefix"] = Value::String(recur_domain::DEFAULT_BRANCH_NAME_PREFIX.into());
    }
}

/// The real GetPR leaves commit diffs to GetCommitDiff.
fn lazy_commit_diffs(pr: &mut Value) {
    if let Some(Value::Array(commits)) = pr.get_mut("commits") {
        for c in commits.iter_mut().filter_map(Value::as_object_mut) {
            c.insert("files".into(), Value::Null);
        }
    }
}
