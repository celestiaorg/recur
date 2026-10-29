//! The engine's forge glue: base mapping, forge merges and the merge queue,
//! check refresh, the branch-rules cache, PR creation and head pushes.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use recur_domain::{Check, MergeMethod, Pr, PrSource, Project, Workspace};
use recur_forge::{ChecksReport, ForgeError, MergeOutcome, PrSnapshot};
use tokio_util::sync::CancellationToken;

use super::support::*;
use crate::bus::EngineEvent;
use crate::testing::{init_repo, wait_for};

#[tokio::test]
async fn pr_base_strips_remote_tracking() {
    let px = Px::new();
    let (_keep, clone) = clone_tracking("main", "main");
    let eng = px.eng();
    assert_eq!(eng.forge_pr_base(&clone, "origin/main").await, "main");
    assert_eq!(eng.forge_pr_base(&clone, "main").await, "main");
    assert_eq!(eng.forge_pr_base(&clone, "  origin/main  ").await, "main");
}

#[tokio::test]
async fn pr_base_slashy_tracking_ref() {
    let px = Px::new();
    let (_keep, clone) = clone_tracking("origin/release", "origin/release");
    assert_eq!(
        px.eng()
            .forge_pr_base(&clone, "origin/origin/release")
            .await,
        "origin/release"
    );
}

fn feed(px: &Px) -> Vec<String> {
    px.te
        .events
        .all()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::WorkspaceEvent(w) => Some(w.event.text),
            _ => None,
        })
        .collect()
}

/// A forge PR on a real worktree of project `acme`.
async fn merge_fx(id: &str, number: i64) -> (Px, crate::testing::TempRepo, Workspace) {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    let w = px.eng().create_workspace("acme", id, "").await.unwrap();
    px.add_pr(Pr {
        id: id.into(),
        workspace_id: w.id.clone(),
        title: id.into(),
        state: "In review".into(),
        group: "you".into(),
        ..gh_pr(number, "")
    });
    (px, repo, w)
}

#[tokio::test]
async fn forge_merge_marks_merged() {
    let (px, _repo, w) = merge_fx("gh-merge-fast", 88).await;
    let snapped = CancellationToken::new();
    let s2 = snapped.clone();
    px.forge.on_snapshot(move |_| {
        s2.cancel();
        async {
            Ok(PrSnapshot {
                state: "MERGED".into(),
                title: "fast".into(),
                ..PrSnapshot::default()
            })
        }
    });
    let (merged, ok) = px
        .eng()
        .merge_pr("gh-merge-fast", &MergeMethod::Squash)
        .await;
    assert!(ok);
    let calls = px.forge.calls_to("merge");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0][1..], ["gh-merge-fast", "squash"]);
    assert_eq!(
        (merged.state.as_str(), merged.group.as_str()),
        ("Merged", "merged")
    );
    assert_eq!(px.ws(&w.id).state, recur_domain::WorkspaceState::Merged);
    tokio::time::timeout(Duration::from_secs(2), snapped.cancelled())
        .await
        .expect("the conversation refresh started after the merge");
    assert!(
        feed(&px)
            .iter()
            .any(|t| t == "Squash and merge: GitHub PR <code>#88</code> via gh."),
        "{:?}",
        feed(&px)
    );
}

#[tokio::test]
async fn forge_merge_queued_leaves_pr_open() {
    let (px, _repo, _w) = merge_fx("gh-merge-queue", 20).await;
    let snapped = CancellationToken::new();
    let s2 = snapped.clone();
    px.forge.on_snapshot(move |_| {
        s2.cancel();
        async {
            Ok(PrSnapshot {
                state: "OPEN".into(),
                title: "queued".into(),
                ..PrSnapshot::default()
            })
        }
    });
    px.forge.on_merge(|_| async { Ok(MergeOutcome::Queued) });
    let (got, ok) = px
        .eng()
        .merge_pr("gh-merge-queue", &MergeMethod::Squash)
        .await;
    assert!(ok && got.merge_queue_enabled && got.merge_queued);
    assert!(got.state != "Merged" && got.group != "merged");
    tokio::time::timeout(Duration::from_secs(2), snapped.cancelled())
        .await
        .expect("the conversation refresh started after the enqueue");
    assert!(
        feed(&px)
            .iter()
            .any(|t| t == "Added GitHub PR <code>#20</code> to the merge queue.")
    );
}

#[tokio::test]
async fn forge_merge_refuses_disallowed_method_and_reports_failure() {
    let (px, _repo, _w) = merge_fx("gh-m", 21).await;
    px.te
        .store
        .update_pr("gh-m", |p| p.allowed_merge_methods = vec!["merge".into()]);
    let (_, ok) = px.eng().merge_pr("gh-m", &MergeMethod::Squash).await;
    assert!(!ok);
    assert!(px.forge.calls_to("merge").is_empty());
    assert!(
        feed(&px)
            .iter()
            .any(|t| t == "GitHub doesn't allow Squash and merge on this repository.")
    );

    px.forge
        .on_merge(|_| async { Err(ForgeError::failed("HTTP 401: Bad credentials")) });
    let (_, ok) = px.eng().merge_pr("gh-m", &MergeMethod::Merge).await;
    assert!(!ok);
    assert!(
        feed(&px)
            .iter()
            .any(|t| t.starts_with("Can't merge on GitHub: the gh CLI is not authenticated"))
    );
}

#[tokio::test]
async fn merge_refuses_closed_and_is_idempotent_when_merged() {
    let (px, _repo, _w) = merge_fx("gh-c", 22).await;
    px.te.store.update_pr("gh-c", |p| p.state = "Closed".into());
    assert!(!px.eng().merge_pr("gh-c", &MergeMethod::Merge).await.1);
    assert!(
        feed(&px)
            .iter()
            .any(|t| t == "Merge refused: this GitHub PR is closed.")
    );
    px.te.store.update_pr("gh-c", |p| p.state = "Merged".into());
    assert!(px.eng().merge_pr("gh-c", &MergeMethod::Merge).await.1);
    assert!(px.forge.calls_to("merge").is_empty());
}

#[tokio::test]
async fn refresh_checks_uses_forge_rollup() {
    let (px, _repo, w) = merge_fx("gh-11", 11).await;
    px.add_pr(Pr {
        id: "10".into(),
        workspace_id: w.id.clone(),
        number: 10,
        source: PrSource::Local,
        checks: Some(vec![Check {
            name: "Independent review".into(),
            passed: true,
            ..Check::default()
        }]),
        checks_ok: true,
        ..Pr::default()
    });
    px.te.store.update_pr("gh-11", |p| {
        p.checks = Some(vec![Check {
            name: "Independent review".into(),
            passed: true,
            ..Check::default()
        }]);
        p.checks_ok = true;
    });
    px.forge.on_checks(|_| async {
        Ok(ChecksReport {
            checks: vec![Check {
                name: "test".into(),
                detail: "CI".into(),
                status: "fail".into(),
                ..Check::default()
            }],
            ok: false,
        })
    });
    px.eng().refresh_pr_checks("10").await;
    assert_eq!(px.pr("10").checks.unwrap()[0].name, "Independent review");
    px.eng().refresh_pr_checks("gh-11").await;
    assert_eq!(px.forge.calls_to("checks")[0][1], "11");
    let got = px.pr("gh-11");
    assert!(!got.checks_ok);
    assert_eq!(got.checks.unwrap()[0].name, "test");
    px.eng().refresh_pr_checks("missing").await;
    assert_eq!(px.forge.calls_to("checks").len(), 1);
}

#[tokio::test]
async fn refresh_checks_skips_merged() {
    let (px, _repo, _w) = merge_fx("gh-12", 12).await;
    px.te.store.update_pr("gh-12", |p| {
        p.state = "Merged".into();
        p.group = "merged".into();
        p.checks = Some(vec![Check {
            name: "stale".into(),
            passed: true,
            ..Check::default()
        }]);
    });
    px.eng().refresh_pr_checks("gh-12").await;
    assert!(px.forge.calls_to("checks").is_empty());
    assert_eq!(px.pr("gh-12").checks.unwrap()[0].name, "stale");
}

#[tokio::test]
async fn forge_pr_checks_degrade_on_error() {
    let px = Px::new();
    px.forge
        .on_checks(|_| async { Err(ForgeError::failed("network down")) });
    let (checks, ok) = px.eng().forge_pr_checks(&PrSource::GitHub, "/tmp", 1).await;
    assert!(ok && checks.is_empty());
    let (checks, ok) = px
        .eng()
        .forge_pr_checks(&PrSource::Other("gl".into()), "/tmp", 1)
        .await;
    assert!(ok && checks.is_empty(), "no forge for the source");
}

fn rules_project(px: &Px, repo: &str) -> Project {
    let p = Project {
        id: "acme".into(),
        name: "acme".into(),
        repo_path: repo.into(),
        base_branch: "main".into(),
        ..Project::default()
    };
    px.te.store.add_project(p.clone()).unwrap();
    px.forge.on_remote_repo(|(_, remote)| async move {
        match remote.as_str() {
            "origin" => Ok("acme/app".to_owned()),
            "fork" => Ok("acme/fork".to_owned()),
            "upstream" => Ok("acme/parent".to_owned()),
            _ => Err(ForgeError::failed("no such remote")),
        }
    });
    p
}

#[tokio::test]
async fn branch_rules_use_the_store() {
    let px = Px::new();
    let repo = init_repo();
    let p = rules_project(&px, &repo.path);
    px.te
        .store
        .set_github_branch_require_pr("acme", "acme/app", "main", true);
    assert!(px.eng().branch_requires_pr(&p, "origin", "main").await);
    assert!(
        px.forge.calls_to("branch_requires_pr").is_empty(),
        "a fresh store hit does not probe"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn branch_rules_stale_refresh_is_async() {
    let px = Px::new();
    let repo = init_repo();
    let p = rules_project(&px, &repo.path);
    px.te
        .store
        .set_github_branch_require_pr("acme", "acme/app", "main", false);
    px.eng().set_branch_rules_ttl(Duration::ZERO);
    let started = CancellationToken::new();
    let block = CancellationToken::new();
    let (s2, b2) = (started.clone(), block.clone());
    px.forge.on_branch_requires_pr(move |_| {
        let (s, b) = (s2.clone(), b2.clone());
        async move {
            s.cancel();
            b.cancelled().await;
            Some(true)
        }
    });
    let start = std::time::Instant::now();
    assert!(
        !px.eng().branch_requires_pr(&p, "origin", "main").await,
        "the stale value answers"
    );
    assert!(start.elapsed() < Duration::from_millis(100));
    tokio::time::timeout(Duration::from_secs(2), started.cancelled())
        .await
        .expect("a background refresh");
    block.cancel();
    wait_for(Duration::from_secs(2), "stored refresh and event", || {
        px.te
            .store
            .github_branch_require_pr("acme", "acme/app", "main")
            .is_some_and(|(r, _)| r)
            && px.te.events.has("github.branch_rules")
    })
    .await;
}

#[tokio::test]
async fn branch_rules_unknown_fails_closed() {
    let px = Px::new();
    let repo = init_repo();
    let p = rules_project(&px, &repo.path);
    assert!(px.eng().branch_requires_pr(&p, "origin", "main").await);
    assert!(
        px.te
            .store
            .github_branch_require_pr("acme", "acme/app", "main")
            .is_none()
    );
}

#[tokio::test]
async fn branch_rules_persist_known_probe() {
    let px = Px::new();
    let repo = init_repo();
    let p = rules_project(&px, &repo.path);
    let n = Arc::new(AtomicUsize::new(0));
    let n2 = Arc::clone(&n);
    px.forge.on_branch_requires_pr(move |_| {
        n2.fetch_add(1, Ordering::SeqCst);
        async { Some(false) }
    });
    assert!(!px.eng().branch_requires_pr(&p, "origin", "main").await);
    assert_eq!(
        px.te
            .store
            .github_branch_require_pr("acme", "acme/app", "main")
            .map(|(r, _)| r),
        Some(false)
    );
    assert!(!px.eng().branch_requires_pr(&p, "origin", "main").await);
    assert_eq!(n.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn branch_rules_scope_cache_by_repo() {
    let px = Px::new();
    let repo = init_repo();
    let p = rules_project(&px, &repo.path);
    px.forge
        .on_branch_requires_pr(
            |(_, owner_repo, _)| async move { Some(owner_repo == "acme/parent") },
        );
    assert!(!px.eng().branch_requires_pr(&p, "fork", "main").await);
    assert!(px.eng().branch_requires_pr(&p, "upstream", "main").await);
    let store = &px.te.store;
    assert_eq!(
        store
            .github_branch_require_pr("acme", "acme/fork", "main")
            .map(|x| x.0),
        Some(false)
    );
    assert_eq!(
        store
            .github_branch_require_pr("acme", "acme/parent", "main")
            .map(|x| x.0),
        Some(true)
    );
    assert_eq!(px.forge.calls_to("branch_requires_pr").len(), 2);
    assert!(
        px.eng().branch_requires_pr(&p, "", "main").await,
        "missing inputs fail closed"
    );
}

#[tokio::test]
async fn create_forge_pr_maps_base_and_reports_fallback() {
    let px = Px::new();
    let (_keep, clone) = clone_tracking("main", "main");
    let proj = Project {
        id: "acme".into(),
        repo_path: clone.clone(),
        ..Project::default()
    };
    px.forge.on_create_pr(|(c,)| async move {
        if c.base == "main" {
            Ok(31)
        } else {
            Err(format!("bad base {}", c.base))
        }
    });
    let got = px
        .eng()
        .create_forge_pr(&proj, &clone, "feat", "origin/main", "Title", "Body", true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((got.number, got.pr_id.as_str()), (31, "gh-acme-31"));
    assert_eq!(got.source, PrSource::GitHub);
    assert_eq!(px.forge.calls_to("create_pr")[0][6], "true", "draft");
    px.forge
        .on_create_pr(|_| async { Err("gh pr create failed: boom".to_owned()) });
    let err = px
        .eng()
        .create_forge_pr(&proj, &clone, "feat", "main", "Title", "Body", false)
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(
        err,
        ("GitHub".to_owned(), "gh pr create failed: boom".to_owned())
    );

    let none = crate::testing::TestEngine::new(None);
    assert!(
        none.engine
            .create_forge_pr(&proj, &clone, "feat", "main", "T", "B", false)
            .await
            .is_none(),
        "no forge: stay Local"
    );
}

#[tokio::test]
async fn push_forge_pr_head_reports_the_forge_error() {
    let px = Px::new();
    let pr = gh_pr(5, "feat");
    assert!(
        px.eng()
            .push_forge_pr_head("/w", "/r", &pr, "feat", true)
            .await
            .is_ok()
    );
    assert_eq!(
        px.forge.calls_to("push_head")[0][2..],
        ["gh-acme-5", "feat", "true"]
    );
    px.forge
        .on_push_head(|_| async { Err(ForgeError::failed("rejected")) });
    assert_eq!(
        px.eng()
            .push_forge_pr_head("/w", "/r", &pr, "feat", false)
            .await,
        Err(("GitHub".to_owned(), "rejected".to_owned()))
    );
}

/// The live round trip: a scripted loop on a forge-mode project opens a
/// real GitHub PR in a throwaway private repo, then merges it. Needs an
/// authenticated `gh` (`RECUR_GH=1 cargo test -- --ignored`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "real GitHub: RECUR_GH=1 cargo test -- --ignored github_merge_round_trip"]
async fn github_merge_round_trip() {
    if std::env::var("RECUR_GH").as_deref() != Ok("1") {
        eprintln!("set RECUR_GH=1 (with gh installed and authenticated) to run the round trip");
        return;
    }
    let gh = Arc::new(recur_forge::github::GitHub::new());
    if !recur_forge::Forge::available(gh.as_ref()).await {
        eprintln!("gh not available or not authenticated");
        return;
    }
    let repo = init_repo();
    let name = format!("recur-gh-e2e-{}", chrono::Utc::now().timestamp());
    let run = |args: &[&str]| {
        std::process::Command::new("gh")
            .args(args)
            .current_dir(&repo.path)
            .output()
            .expect("spawn gh")
    };
    let out = run(&[
        "repo",
        "create",
        &name,
        "--private",
        "--source=.",
        "--remote=origin",
        "--push",
    ]);
    assert!(
        out.status.success(),
        "gh repo create: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let owner = String::from_utf8_lossy(&run(&["api", "user", "-q", ".login"]).stdout)
        .trim()
        .to_owned();
    let full = format!("{owner}/{name}");
    struct Cleanup(String, String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let ok = std::process::Command::new("gh")
                .args(["repo", "delete", &self.0, "--yes"])
                .current_dir(&self.1)
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                eprintln!(
                    "NOTE: delete the throwaway repo by hand: gh repo delete {} --yes",
                    self.0
                );
            }
        }
    }
    let _cleanup = Cleanup(full.clone(), repo.path.clone());

    let runner = crate::loops::test_support::ConvergeRunner::new(&[0]);
    let te = crate::testing::TestEngine::with(crate::testing::TestOptions {
        provider: Some(runner.provider()),
        forges: vec![gh],
        ..crate::testing::TestOptions::default()
    });
    te.store
        .add_project(Project {
            id: "ghproj".into(),
            name: "ghproj".into(),
            repo_path: repo.path.clone(),
            base_branch: "main".into(),
            pr_on_github: true,
            ..Project::default()
        })
        .unwrap();
    let id = te
        .engine
        .start_workspace(
            "ghproj",
            "Add a real GitHub PR feature",
            "",
            Default::default(),
        )
        .await
        .unwrap();
    wait_for(Duration::from_secs(90), "pr_open", || {
        te.store.workspace(&id).is_some_and(|l| {
            matches!(
                l.state,
                recur_domain::WorkspaceState::PrOpen | recur_domain::WorkspaceState::Failed
            )
        })
    })
    .await;
    let pr = te.store.pr_by_workspace(&id).expect("a PR");
    assert_eq!(
        pr.source,
        PrSource::GitHub,
        "feed: {:?}",
        te.store.feed(&id)
    );
    assert!(pr.number > 0);
    let state = |n: i64| {
        String::from_utf8_lossy(
            &run(&[
                "pr",
                "view",
                &n.to_string(),
                "--json",
                "state",
                "-q",
                ".state",
            ])
            .stdout,
        )
        .trim()
        .to_owned()
    };
    assert_eq!(state(pr.number), "OPEN");
    let (merged, ok) = te.engine.merge_pr(&pr.id, &MergeMethod::default()).await;
    assert!(ok && merged.state == "Merged");
    assert_eq!(state(pr.number), "MERGED");
}
