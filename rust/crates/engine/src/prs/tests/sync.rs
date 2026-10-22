//! The PR sweep and list-row import (Go `github_prs_test.go` sync and
//! `applyListedGitHubPR` cases).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use recur_domain::{ATTENTION_REVIEW_REQUESTED, Check, FileDiff, Pr, PrParticipant};
use recur_forge::model::check_rows_kind;
use recur_forge::{ChecksReport, ForgeError, PrListState, PrSnapshot};

use super::support::*;
use crate::bus::EngineEvent;
use crate::testing::init_repo;

fn who(login: &str) -> PrParticipant {
    PrParticipant {
        login: login.into(),
        ..PrParticipant::default()
    }
}

#[tokio::test]
async fn sync_imports_and_dedupes() {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    px.forge.on_login(|()| async { "avery".to_owned() });
    let now = Utc::now().to_rfc3339();
    px.forge.on_list_prs(move |(_, state, _)| {
        let now = now.clone();
        async move {
            if state == PrListState::Merged {
                let mut done = row(9, "Done", "done");
                done.state = "MERGED".into();
                done.updated_at = now;
                return Ok(vec![done]);
            }
            let mut review = row(12, "Please review", "feat/oauth");
            review.review_decision = "REVIEW_REQUIRED".into();
            review.review_requests = vec![who("avery")];
            review.additions = 4;
            review.deletions = 1;
            review.changed_files = 2;
            review.updated_at = now.clone();
            let mut other = row(13, "Unrelated open", "chore/docs");
            other.author_login = "pat".into();
            other.updated_at = now;
            Ok(vec![review, other])
        }
    });
    let got = px.eng().sync_prs().await;
    let (mut you, mut open, mut merged) = (0, 0, 0);
    for p in got.iter().filter(|p| p.project_id == "acme") {
        match p.group.as_str() {
            "you" => {
                you += 1;
                assert_eq!(p.number, 12);
                assert!(p.review_requested);
                assert_eq!(p.attention_reasons, [ATTENTION_REVIEW_REQUESTED]);
            }
            "open" if p.state != "Merged" && p.state != "Closed" => open += 1,
            "merged" => merged += 1,
            _ => {}
        }
    }
    assert_eq!((you, open, merged), (1, 1, 1));
}

#[tokio::test]
async fn sync_lists_projects_concurrently() {
    let px = Px::new();
    let (a, b) = (init_repo(), init_repo());
    px.add_project("a", &a.path);
    px.add_project("b", &b.path);
    let seen = Arc::new(Mutex::new(std::collections::HashSet::new()));
    let both = tokio_util::sync::CancellationToken::new();
    let (s2, b2) = (Arc::clone(&seen), both.clone());
    px.forge.on_list_prs(move |(repo, state, _)| {
        let (seen, both) = (Arc::clone(&s2), b2.clone());
        async move {
            if state != PrListState::Open {
                return Ok(Vec::new());
            }
            {
                let mut s = seen.lock();
                s.insert(repo);
                if s.len() == 2 {
                    both.cancel();
                }
            }
            tokio::select! {
                _ = both.cancelled() => Ok(Vec::new()),
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    Err(ForgeError::failed("projects listed sequentially"))
                }
            }
        }
    });
    let start = std::time::Instant::now();
    px.eng().sync_prs().await;
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "listed one at a time"
    );
}

#[tokio::test]
async fn sync_streams_slim_project_rows() {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    px.add_pr(Pr {
        files: Some(vec![FileDiff {
            path: "big.go".into(),
            ..FileDiff::default()
        }]),
        ..gh_pr(12, "feat")
    });
    px.forge.on_list_prs(|(_, state, _)| async move {
        if state != PrListState::Open {
            return Ok(Vec::new());
        }
        Ok(vec![row(12, "Fresh title", "feat")])
    });
    px.eng().sync_prs().await;
    let evs = px.te.events.by_name("prs.synced");
    assert_eq!(evs.len(), 1);
    let EngineEvent::PrsSynced(ev) = &evs[0] else {
        panic!("not prs.synced");
    };
    let rows = ev.prs.clone().unwrap_or_default();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "Fresh title");
    assert!(rows[0].files.is_none(), "rows are slim");
    assert_eq!(px.pr("gh-acme-12").files.unwrap().len(), 1);
}

#[tokio::test]
async fn sync_ttl_keeps_store() {
    let px = Px::new();
    px.add_pr(Pr {
        id: "keep".into(),
        title: "Keep".into(),
        ..gh_pr(1, "")
    });
    px.eng().set_pr_sync_min_interval(Duration::from_secs(3600));
    px.eng().prs.sync.lock().at = Some(std::time::Instant::now());
    let got = px.eng().sync_prs().await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].id, "keep");
    assert!(px.forge.calls_to("list_prs").is_empty());
}

#[tokio::test]
async fn sync_waits_for_in_flight_sweep() {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    let release = tokio_util::sync::CancellationToken::new();
    let r2 = release.clone();
    px.forge.on_list_prs(move |_| {
        let r = r2.clone();
        async move {
            r.cancelled().await;
            Ok(Vec::new())
        }
    });
    let e = Arc::clone(px.eng());
    let first = tokio::spawn(async move { e.sync_prs().await });
    crate::testing::wait_for(Duration::from_secs(2), "first sweep listing", || {
        px.forge.calls_to("list_prs").len() == 2
    })
    .await;
    let e = Arc::clone(px.eng());
    let second = tokio::spawn(async move { e.sync_prs().await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !second.is_finished(),
        "a concurrent caller waits for the sweep"
    );
    release.cancel();
    first.await.unwrap();
    second.await.unwrap();
    assert_eq!(px.forge.calls_to("list_prs").len(), 2, "single-flight");
}

#[tokio::test]
async fn apply_reconciles_stale_checks() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    px.add_pr(Pr {
        checks: Some(vec![Check {
            name: "build".into(),
            status: "pending".into(),
            url: "https://github.com/acme/app/runs/1".into(),
            ..Check::default()
        }]),
        checks_ok: true,
        has_checks: true,
        ..gh_pr(12, "feat/oauth")
    });
    let fail = with_checks(
        row(12, "OAuth", "feat/oauth"),
        vec![listed("build", "fail")],
    );
    let (_, created) = px
        .eng()
        .apply_listed_pr(px.forge.as_ref(), "acme", &fail, "avery", Utc::now())
        .await;
    assert!(!created);
    let got = px.pr("gh-acme-12");
    assert!(!got.checks_ok);
    let checks = got.checks.unwrap();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].status, "fail");
    assert!(!checks[0].passed);

    let pass = with_checks(
        row(12, "OAuth", "feat/oauth"),
        vec![listed("build", "pass")],
    );
    px.eng()
        .apply_listed_pr(px.forge.as_ref(), "acme", &pass, "avery", Utc::now())
        .await;
    let got = px.pr("gh-acme-12");
    assert!(got.checks_ok);
    assert_eq!(got.checks.unwrap()[0].status, "pass");

    px.te.store.update_pr("gh-acme-12", |p| {
        p.checks = Some(vec![Check {
            name: "build".into(),
            passed: true,
            status: "pass".into(),
            url: "https://github.com/acme/app/runs/1".into(),
            ..Check::default()
        }]);
        p.checks_ok = true;
        p.has_checks = true;
    });
    px.eng()
        .apply_listed_pr(px.forge.as_ref(), "acme", &pass, "avery", Utc::now())
        .await;
    assert_eq!(
        px.pr("gh-acme-12").checks.unwrap()[0].url,
        "https://github.com/acme/app/runs/1",
        "a same-kind list sync keeps the GetPR url"
    );
}

#[tokio::test]
async fn apply_keeps_unreported_pending_checks() {
    let px = Px::new();
    px.forge
        .on_checks(|_| async { Err(ForgeError::failed("graphql unavailable")) });
    px.add_project("acme", "/tmp/acme");
    px.add_pr(Pr {
        checks: Some(vec![
            Check {
                name: "lint".into(),
                passed: true,
                status: "pass".into(),
                url: "https://github.com/acme/app/runs/1".into(),
                ..Check::default()
            },
            Check {
                name: "CI Success".into(),
                status: "pending".into(),
                ..Check::default()
            },
        ]),
        checks_ok: true,
        has_checks: true,
        ..gh_pr(12, "feat/oauth")
    });
    let r = with_checks(row(12, "OAuth", "feat/oauth"), vec![listed("lint", "pass")]);
    let (_, created) = px
        .eng()
        .apply_listed_pr(px.forge.as_ref(), "acme", &r, "avery", Utc::now())
        .await;
    assert!(!created);
    let checks = px.pr("gh-acme-12").checks.unwrap();
    assert_eq!(check_rows_kind(&checks), "pending");
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0].name, "lint");
    assert_eq!(checks[0].url, "https://github.com/acme/app/runs/1");
    assert_eq!(checks[1].name, "CI Success");
    assert_eq!(checks[1].status, "pending");
}

#[tokio::test]
async fn apply_retires_queued_workflow_placeholder() {
    let px = Px::new();
    let fetched = Arc::new(AtomicUsize::new(0));
    let f2 = Arc::clone(&fetched);
    px.forge.on_checks(move |_| {
        f2.fetch_add(1, Ordering::SeqCst);
        async {
            Ok(ChecksReport {
                checks: vec![listed("publish", "pass")],
                ok: true,
            })
        }
    });
    px.add_project("acme", "/tmp/acme");
    px.add_pr(Pr {
        checks: Some(vec![Check {
            name: "Release".into(),
            status: "pending".into(),
            ..Check::default()
        }]),
        checks_ok: true,
        has_checks: true,
        ..gh_pr(12, "feat/oauth")
    });
    let r = with_checks(
        row(12, "OAuth", "feat/oauth"),
        vec![listed("publish", "pass")],
    );
    let (_, created) = px
        .eng()
        .apply_listed_pr(px.forge.as_ref(), "acme", &r, "avery", Utc::now())
        .await;
    assert!(!created);
    assert_eq!(fetched.load(Ordering::SeqCst), 1);
    let checks = px.pr("gh-acme-12").checks.unwrap();
    assert_eq!(check_rows_kind(&checks), "pass");
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].name, "publish");

    px.eng()
        .apply_listed_pr(px.forge.as_ref(), "acme", &r, "avery", Utc::now())
        .await;
    assert_eq!(
        fetched.load(Ordering::SeqCst),
        1,
        "repeat sync does not refetch"
    );
    let checks = px.pr("gh-acme-12").checks.unwrap();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].name, "publish");
}

#[tokio::test]
async fn apply_reopens_closed_pr() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    px.add_pr(Pr {
        state: "Closed".into(),
        group: "merged".into(),
        ..gh_pr(12, "feat/oauth")
    });
    let (id, created) = px
        .eng()
        .apply_listed_pr(
            px.forge.as_ref(),
            "acme",
            &row(12, "OAuth", "feat/oauth"),
            "avery",
            Utc::now(),
        )
        .await;
    assert!(!created);
    assert_eq!(id, "gh-acme-12");
    let got = px.pr(&id);
    assert_eq!((got.state.as_str(), got.group.as_str()), ("Open", "open"));
}

#[tokio::test]
async fn apply_refreshes_queued_pr() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    px.add_pr(Pr {
        merge_queue_enabled: true,
        merge_queued: true,
        ..gh_pr(12, "feat/oauth")
    });
    px.forge.on_snapshot(|_| async {
        Ok(PrSnapshot {
            state: "OPEN".into(),
            title: "OAuth".into(),
            url: "https://github.com/acme/app/pull/12".into(),
            merge_queue_enabled: true,
            ..PrSnapshot::default()
        })
    });
    px.eng()
        .apply_listed_pr(
            px.forge.as_ref(),
            "acme",
            &row(12, "OAuth", "feat/oauth"),
            "avery",
            Utc::now(),
        )
        .await;
    crate::testing::wait_for(Duration::from_secs(2), "queued snapshot refresh", || {
        !px.forge.calls_to("snapshot").is_empty() && px.eng().prs.conv_busy.lock().is_empty()
    })
    .await;
    assert!(
        !px.pr("gh-acme-12").merge_queued,
        "the snapshot reported the PR left the queue"
    );
}

#[tokio::test]
async fn apply_links_matching_workspace() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    px.add_ws(ws("ws-oauth", "feat/oauth"));
    let (id, created) = px
        .eng()
        .apply_listed_pr(
            px.forge.as_ref(),
            "acme",
            &row(12, "OAuth", "refs/heads/feat/oauth"),
            "avery",
            Utc::now(),
        )
        .await;
    assert!(created && !id.is_empty());
    assert_eq!(px.pr(&id).workspace_id, "ws-oauth");
}

#[tokio::test]
async fn apply_stores_created_at() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    let (opened, touched) = ("2026-01-15T12:00:00Z", "2026-08-21T12:00:00Z");
    let mut r = row(12, "OAuth", "feat/oauth");
    r.created_at = opened.into();
    r.updated_at = touched.into();
    let (id, created) = px
        .eng()
        .apply_listed_pr(px.forge.as_ref(), "acme", &r, "avery", Utc::now())
        .await;
    assert!(created);
    let got = px.pr(&id);
    assert_eq!(got.created_at, opened);
    assert_eq!(got.updated_at, touched);
    let mut r2 = row(12, "OAuth v2", "feat/oauth");
    r2.created_at = opened.into();
    let (_, created) = px
        .eng()
        .apply_listed_pr(px.forge.as_ref(), "acme", &r2, "avery", Utc::now())
        .await;
    assert!(!created);
    assert_eq!(px.pr(&id).created_at, opened);
}

#[tokio::test]
async fn apply_stores_participants() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    let mut r = row(12, "OAuth", "feat/oauth");
    r.participants = vec![who("sam"), who("pat"), who("avery")];
    let (id, created) = px
        .eng()
        .apply_listed_pr(px.forge.as_ref(), "acme", &r, "avery", Utc::now())
        .await;
    assert!(created);
    let logins: Vec<String> = px
        .pr(&id)
        .participants
        .into_iter()
        .map(|p| p.login)
        .collect();
    assert_eq!(logins, ["sam", "pat", "avery"]);
}

#[tokio::test]
async fn apply_links_workspace_on_later_sync() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    let r = row(12, "OAuth", "feat/oauth");
    let (id, created) = px
        .eng()
        .apply_listed_pr(px.forge.as_ref(), "acme", &r, "avery", Utc::now())
        .await;
    assert!(created);
    assert_eq!(px.pr(&id).workspace_id, "");
    px.add_ws(ws("ws-oauth", "feat/oauth"));
    let (_, created) = px
        .eng()
        .apply_listed_pr(px.forge.as_ref(), "acme", &r, "avery", Utc::now())
        .await;
    assert!(!created);
    assert_eq!(px.pr(&id).workspace_id, "ws-oauth");
}

#[tokio::test]
async fn apply_keeps_local_behind_base() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    px.add_ws(recur_domain::Workspace {
        worktree_path: "/tmp/acme-oauth".into(),
        ..ws("ws-oauth", "feat/oauth")
    });
    px.add_pr(Pr {
        workspace_id: "ws-oauth".into(),
        behind_base: 3,
        author_login: "sam".into(),
        ..gh_pr(12, "feat/oauth")
    });
    let mut r = row(12, "OAuth", "feat/oauth");
    r.merge_state_status = "CLEAN".into();
    let (_, created) = px
        .eng()
        .apply_listed_pr(px.forge.as_ref(), "acme", &r, "avery", Utc::now())
        .await;
    assert!(!created);
    assert_eq!(
        px.pr("gh-acme-12").behind_base,
        3,
        "CLEAN must not clobber git's count"
    );

    px.add_pr(Pr {
        title: "Docs".into(),
        behind_base: 3,
        author_login: "sam".into(),
        ..gh_pr(13, "docs")
    });
    let mut r = row(13, "Docs", "docs");
    r.merge_state_status = "BEHIND".into();
    let (_, created) = px
        .eng()
        .apply_listed_pr(px.forge.as_ref(), "acme", &r, "avery", Utc::now())
        .await;
    assert!(!created);
    assert_eq!(px.pr("gh-acme-13").behind_base, 1);
}

#[tokio::test]
async fn apply_tracks_conflicts() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    px.add_pr(Pr {
        author_login: "sam".into(),
        mergeable: "MERGEABLE".into(),
        merge_state_status: "CLEAN".into(),
        ..gh_pr(12, "feat/oauth")
    });
    let apply = |mergeable: &'static str, status: &'static str| {
        let px = &px;
        async move {
            let mut r = row(12, "OAuth", "feat/oauth");
            r.mergeable = mergeable.into();
            r.merge_state_status = status.into();
            px.eng()
                .apply_listed_pr(px.forge.as_ref(), "acme", &r, "avery", Utc::now())
                .await;
            px.pr("gh-acme-12")
        }
    };
    let got = apply("CONFLICTING", "DIRTY").await;
    assert_eq!(
        (got.mergeable.as_str(), got.merge_state_status.as_str()),
        ("CONFLICTING", "DIRTY")
    );
    let got = apply("UNKNOWN", "UNKNOWN").await;
    assert_eq!(
        (got.mergeable.as_str(), got.merge_state_status.as_str()),
        ("CONFLICTING", "DIRTY"),
        "UNKNOWN keeps the known conflict"
    );
}
