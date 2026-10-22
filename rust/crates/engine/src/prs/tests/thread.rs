//! Conversation refresh, drafts, comments and reviews (Go
//! `github_thread_test.go` engine cases).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use recur_domain::{
    ATTENTION_REVIEW_REQUESTED, Commit, FINDING_ACTOR_ENGINE, Finding, FindingStatus,
    PendingLineComment, Pr, PrComment, PrParticipant, PrSource, Workspace, WorkspaceState,
};
use recur_forge::{CommitAuthor, CommitMeta, ForgeError, PrSnapshot};
use tokio_util::sync::CancellationToken;

use super::support::*;
use crate::bus::EngineEvent;
use crate::prs::PR_CONVERSATION_MIN_INTERVAL;
use crate::testing::{TempRepo, init_repo, wait_for};

/// A project on a real repo (the forge's cwd) and one forge PR on it.
fn conv(pr: Pr) -> (Px, TempRepo) {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    px.add_pr(pr);
    (px, repo)
}

fn open_pr(id: &str, number: i64) -> Pr {
    Pr {
        id: id.into(),
        title: "pr".into(),
        state: "In review".into(),
        group: "you".into(),
        url: String::new(),
        ..gh_pr(number, "")
    }
}

fn snap_ok(snap: PrSnapshot) -> impl Fn((std::path::PathBuf, i64)) -> std::future::Ready<Result<PrSnapshot, ForgeError>> {
    move |_| std::future::ready(Ok(snap.clone()))
}

fn comment(id: &str, author: &str, html: &str) -> PrComment {
    PrComment {
        id: id.into(),
        author: author.into(),
        body_html: html.into(),
        ..PrComment::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refresh_does_not_unmerge_in_flight() {
    let (px, _repo) = conv(Pr {
        id: "gh-inflight".into(),
        title: "old".into(),
        comments: Some(vec![comment("c-1", "avery", "before")]),
        ..open_pr("gh-inflight", 90)
    });
    let entered = CancellationToken::new();
    let release = CancellationToken::new();
    let (e2, r2) = (entered.clone(), release.clone());
    px.forge.on_snapshot(move |_| {
        let (e, r) = (e2.clone(), r2.clone());
        async move {
            e.cancel();
            r.cancelled().await;
            Ok(PrSnapshot {
                title: "from gh".into(),
                state: "OPEN".into(),
                review_decision: "APPROVED".into(),
                comments: vec![comment("gh-comment-1", "avery", "please fix")],
                ..PrSnapshot::default()
            })
        }
    });
    let e = Arc::clone(px.eng());
    let refresh = tokio::spawn(async move { e.refresh_pr_conversation("gh-inflight").await });
    tokio::time::timeout(Duration::from_secs(2), entered.cancelled())
        .await
        .expect("snapshot called");
    px.eng().mark_merged("gh-inflight", "").await;
    release.cancel();
    assert!(refresh.await.unwrap(), "changed");
    let got = px.pr("gh-inflight");
    assert_eq!((got.state.as_str(), got.group.as_str()), ("Merged", "merged"));
}

#[tokio::test]
async fn refresh_reopens_closed_pr() {
    let (px, _repo) = conv(Pr {
        title: "closed then opened".into(),
        state: "Closed".into(),
        group: "merged".into(),
        comments: Some(vec![comment("c-1", "avery", "before")]),
        ..open_pr("gh-reopen", 21)
    });
    px.forge.on_snapshot(snap_ok(PrSnapshot {
        title: "closed then opened".into(),
        state: "OPEN".into(),
        comments: vec![
            comment("c-1", "avery", "before"),
            PrComment {
                kind: "action".into(),
                ..comment("gh-reopened", "avery", "reopened this")
            },
        ],
        ..PrSnapshot::default()
    }));
    assert!(px.eng().refresh_pr_conversation("gh-reopen").await);
    let got = px.pr("gh-reopen");
    assert_eq!((got.state.as_str(), got.group.as_str()), ("Open", "open"));
    let cs = got.comments.unwrap();
    assert_eq!(cs.len(), 2);
    assert_eq!(cs[1].id, "gh-reopened");
}

#[tokio::test]
async fn refresh_mirrors_the_forge() {
    let (px, _repo) = conv(Pr {
        title: "old title".into(),
        comments: Some(vec![comment("c-agent", "Coder", "working")]),
        commits: Some(vec![Commit {
            hash: "3f1d02b".into(),
            author: "Josh".into(),
            author_role: "Author".into(),
            ..Commit::default()
        }]),
        ..open_pr("gh-20", 20)
    });
    px.forge.on_snapshot(snap_ok(PrSnapshot {
        title: "new title".into(),
        state: "OPEN".into(),
        url: "https://github.com/acme/app/pull/20".into(),
        created_at: "2026-08-01T09:00:00Z".into(),
        updated_at: "2026-08-21T15:00:00Z".into(),
        review_decision: "CHANGES_REQUESTED".into(),
        comments: vec![comment("gh-comment-1", "avery", "please fix")],
        commit_authors: HashMap::from([(
            "3f1d02babcdef".to_owned(),
            CommitAuthor {
                name: "mycodecrafting".into(),
                avatar_url: "https://github.com/mycodecrafting.png?size=56".into(),
            },
        )]),
        commit_meta: HashMap::from([(
            "3f1d02babcdef".to_owned(),
            CommitMeta {
                verified: true,
                check_status: "pass".into(),
                url: String::new(),
            },
        )]),
        ..PrSnapshot::default()
    }));
    px.eng().refresh_pr_conversation("gh-20").await;
    let got = px.pr("gh-20");
    assert_eq!(got.title, "new title");
    assert_eq!(got.url, "https://github.com/acme/app/pull/20");
    assert_eq!(got.state, "Changes requested");
    let ids: Vec<String> = got.comments.unwrap().into_iter().map(|c| c.id).collect();
    assert_eq!(ids, ["gh-comment-1", "c-agent"]);
    let commits = got.commits.unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].author, "mycodecrafting");
    assert!(commits[0].verified);
    assert_eq!(commits[0].check_status, "pass");
    assert_eq!(got.created_at, "2026-08-01T09:00:00Z");
    assert_eq!(got.updated_at, "2026-08-21T15:00:00Z");
}

#[tokio::test]
async fn refresh_keeps_older_comments_when_truncated() {
    let existing: Vec<PrComment> = (1..=101)
        .map(|i| PrComment {
            id: format!("gh-comment-{i}"),
            body_md: format!("c{i}"),
            ..PrComment::default()
        })
        .collect();
    let from_forge = existing[1..].to_vec();
    let (px, _repo) = conv(Pr {
        title: "busy".into(),
        comments: Some(existing),
        ..open_pr("gh-101", 101)
    });
    px.forge.on_snapshot(snap_ok(PrSnapshot {
        comments: from_forge,
        comments_truncated: true,
        ..PrSnapshot::default()
    }));
    px.eng().refresh_pr_conversation("gh-101").await;
    let cs = px.pr("gh-101").comments.unwrap();
    assert_eq!(cs.len(), 101);
    assert!(cs.iter().any(|c| c.id == "gh-comment-1"));
}

#[tokio::test]
async fn refresh_stores_review_request() {
    let (px, _repo) = conv(Pr {
        title: "old".into(),
        state: "Open".into(),
        group: "open".into(),
        author_login: "sam".into(),
        viewer_login: "avery".into(),
        ..open_pr("gh-21", 21)
    });
    px.forge.on_snapshot(snap_ok(PrSnapshot {
        title: "Please review".into(),
        state: "OPEN".into(),
        url: "https://github.com/acme/app/pull/21".into(),
        author_login: "sam".into(),
        viewer_login: "avery".into(),
        review_decision: "REVIEW_REQUIRED".into(),
        review_requests: vec![PrParticipant {
            login: "avery".into(),
            ..PrParticipant::default()
        }],
        ..PrSnapshot::default()
    }));
    px.eng().refresh_pr_conversation("gh-21").await;
    let got = px.pr("gh-21");
    assert!(got.review_requested);
    assert_eq!(got.group, "you");
    assert_eq!(got.attention_reasons, [ATTENTION_REVIEW_REQUESTED]);
}

#[test]
fn conversation_min_interval_lets_open_pr_refresh() {
    assert!(PR_CONVERSATION_MIN_INTERVAL <= Duration::from_secs(20));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn schedule_refresh_does_not_block_and_single_flights() {
    let (px, _repo) = conv(Pr {
        title: "old".into(),
        ..open_pr("gh-sched", 21)
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = CancellationToken::new();
    let release = CancellationToken::new();
    let (c2, e2, r2) = (Arc::clone(&calls), entered.clone(), release.clone());
    px.forge.on_snapshot(move |_| {
        c2.fetch_add(1, Ordering::SeqCst);
        let (e, r) = (e2.clone(), r2.clone());
        async move {
            e.cancel();
            r.cancelled().await;
            Ok(PrSnapshot {
                title: "from gh".into(),
                state: "OPEN".into(),
                ..PrSnapshot::default()
            })
        }
    });
    let started = std::time::Instant::now();
    px.eng().schedule_refresh_pr_conversation("gh-sched");
    px.eng().schedule_refresh_pr_conversation("gh-sched");
    assert!(started.elapsed() < Duration::from_millis(200), "blocked on the snapshot");
    tokio::time::timeout(Duration::from_secs(2), entered.cancelled())
        .await
        .expect("snapshot called");
    px.eng().schedule_refresh_pr_conversation("gh-sched");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "single-flight");
    release.cancel();
    wait_for(Duration::from_secs(2), "refreshed title", || {
        px.pr("gh-sched").title == "from gh"
    })
    .await;
    wait_for(Duration::from_secs(2), "flight done", || {
        px.eng().prs.conv_busy.lock().is_empty()
    })
    .await;
    px.eng().schedule_refresh_pr_conversation("gh-sched");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "throttled");
}

#[tokio::test]
async fn refresh_stores_merge_status() {
    let (px, _repo) = conv(open_pr("gh-merge", 33));
    px.forge.on_snapshot(snap_ok(PrSnapshot {
        title: "pr".into(),
        state: "OPEN".into(),
        review_decision: "APPROVED".into(),
        mergeable: "MERGEABLE".into(),
        merge_state_status: "BLOCKED".into(),
        is_draft: true,
        head_sha: "abc123def".into(),
        ..PrSnapshot::default()
    }));
    px.eng().refresh_pr_conversation("gh-merge").await;
    let got = px.pr("gh-merge");
    assert_eq!(got.mergeable, "MERGEABLE");
    assert_eq!(got.merge_state_status, "BLOCKED");
    assert_eq!(got.review_decision, "APPROVED");
    assert!(got.draft);
    assert_eq!(got.head_sha, "abc123def");
}

#[tokio::test]
async fn refresh_preserves_merged_updated_at() {
    let (frozen, merged_at) = ("2026-08-01T12:00:00Z", "2026-08-20T09:00:00Z");
    let (px, _repo) = conv(Pr {
        created_at: frozen.into(),
        updated_at: frozen.into(),
        ..open_pr("gh-merged-at", 77)
    });
    px.forge.on_snapshot(snap_ok(PrSnapshot {
        title: "pr".into(),
        state: "MERGED".into(),
        created_at: frozen.into(),
        updated_at: merged_at.into(),
        ..PrSnapshot::default()
    }));
    assert!(px.eng().refresh_pr_conversation("gh-merged-at").await);
    let got = px.pr("gh-merged-at");
    assert_eq!((got.state.as_str(), got.group.as_str()), ("Merged", "merged"));
    assert_eq!(got.updated_at, merged_at);
}

#[tokio::test]
async fn refresh_stores_allowed_merge_methods() {
    let (px, _repo) = conv(open_pr("gh-methods", 55));
    px.forge.on_snapshot(snap_ok(PrSnapshot {
        title: "pr".into(),
        state: "OPEN".into(),
        allowed_merge_methods: vec!["squash".into(), "rebase".into()],
        ..PrSnapshot::default()
    }));
    px.eng().refresh_pr_conversation("gh-methods").await;
    assert_eq!(px.pr("gh-methods").allowed_merge_methods, ["squash", "rebase"]);
}

#[tokio::test]
async fn ready_pr_marks_draft_ready() {
    let (px, _repo) = conv(Pr {
        draft: true,
        ..open_pr("gh-ready", 44)
    });
    let (got, ok) = px.eng().ready_pr("gh-ready").await;
    assert!(ok);
    assert_eq!(px.forge.calls_to("set_draft").len(), 1);
    assert_eq!(px.forge.calls_to("set_draft")[0][2], "false");
    assert!(!got.draft);
}

#[tokio::test]
async fn set_draft_converts_to_draft() {
    let (px, _repo) = conv(open_pr("gh-to-draft", 45));
    let (got, ok) = px.eng().set_pr_draft("gh-to-draft", true).await;
    assert!(ok);
    assert_eq!(px.forge.calls_to("set_draft")[0][2], "true");
    assert!(got.draft);
}

#[tokio::test]
async fn set_draft_local_persists_flag() {
    let (px, _repo) = conv(Pr {
        source: PrSource::Local,
        ..open_pr("local-draft", 0)
    });
    let (got, ok) = px.eng().set_pr_draft("local-draft", true).await;
    assert!(ok && got.draft);
    assert!(px.forge.calls_to("set_draft").is_empty());
}

#[tokio::test]
async fn set_draft_refuses_settled_and_reports_failures() {
    let (px, _repo) = conv(open_pr("gh-d", 46));
    px.forge
        .on_set_draft(|_| async { Err(ForgeError::failed("gh: boom")) });
    let (_, ok) = px.eng().set_pr_draft("gh-d", true).await;
    assert!(!ok);
    assert!(feed_text(&px).iter().any(|t| t == "Convert to draft failed: gh: boom"));
    px.te.store.update_pr("gh-d", |p| p.state = "Merged".into());
    let (_, ok) = px.eng().set_pr_draft("gh-d", true).await;
    assert!(!ok);
}

/// Every workspace feed line published.
fn feed_text(px: &Px) -> Vec<String> {
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

/// Captures the line notes each submitted review carried.
fn capture_reviews(px: &Px) -> Arc<Mutex<Vec<(String, String, Vec<PendingLineComment>)>>> {
    let got = Arc::new(Mutex::new(Vec::new()));
    let g = Arc::clone(&got);
    px.forge.on_submit_review(move |(_, _, kind, body, comments)| {
        g.lock().push((kind, body, comments));
        async { Ok(()) }
    });
    got
}

#[tokio::test]
async fn add_comment_line_note_pends_until_review() {
    let (px, _repo) = conv(open_pr("gh-21", 21));
    let reviews = capture_reviews(&px);
    px.forge.on_snapshot(snap_ok(PrSnapshot {
        state: "OPEN".into(),
        comments: vec![comment("gh-inline-1", "you", "nits")],
        ..PrSnapshot::default()
    }));
    let (_, ok) = px.eng().add_pr_comment("gh-21", "auth.go", 12, "nits", 0, "").await;
    assert!(ok);
    assert!(px.forge.calls_to("post_comment").is_empty(), "pending until submit");
    let got = px.pr("gh-21");
    assert_eq!(got.pending_comments.len(), 1);
    assert_eq!((got.pending_comments[0].file.as_str(), got.pending_comments[0].line), ("auth.go", 12));
    assert!(got.comments.unwrap_or_default().is_empty());
    let (_, ok) = px.eng().submit_review("gh-21", "comment", "nits wrap").await;
    assert!(ok);
    let r = reviews.lock();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].2.len(), 1);
    assert_eq!(r[0].2[0].body, "nits");
}

#[tokio::test]
async fn add_comment_multiline_pending() {
    let (px, _repo) = conv(open_pr("gh-22", 22));
    let reviews = capture_reviews(&px);
    let (_, ok) = px.eng().add_pr_comment("gh-22", "auth.go", 12, "span", 10, "").await;
    assert!(ok);
    let pc = px.pr("gh-22").pending_comments[0].clone();
    assert_eq!(
        (pc.file.as_str(), pc.line, pc.start_line, pc.side.as_str(), pc.start_side.as_str()),
        ("auth.go", 12, 10, "RIGHT", "RIGHT")
    );
    px.eng().submit_review("gh-22", "comment", "wrap").await;
    let r = reviews.lock();
    assert_eq!((r[0].2[0].start_line, r[0].2[0].line), (10, 12));
}

#[tokio::test]
async fn add_comment_left_side_and_bad_range() {
    let (px, _repo) = conv(open_pr("gh-l", 30));
    px.eng().add_pr_comment("gh-l", "auth.go", -7, "old side", 12, "").await;
    let pc = px.pr("gh-l").pending_comments[0].clone();
    assert_eq!((pc.line, pc.side.as_str(), pc.start_line, pc.start_side.as_str()), (7, "LEFT", 0, ""));
}

#[tokio::test]
async fn add_comment_persists_finding_id() {
    let (px, _repo) = conv(open_pr("gh-find", 23));
    px.eng().add_pr_comment("gh-find", "auth.go", 12, "nits", 0, "F-1").await;
    assert_eq!(px.pr("gh-find").pending_comments[0].finding_id, "F-1");
}

#[tokio::test]
async fn add_comment_delete_removes_only_matching_draft() {
    let (px, _repo) = conv(open_pr("gh-two", 24));
    let eng = px.eng();
    assert!(eng.add_pr_comment("gh-two", "auth.go", 12, "first", 0, "F-1").await.1);
    assert!(eng.add_pr_comment("gh-two", "auth.go", 12, "second", 0, "F-2").await.1);
    let (removed, ok) = eng.add_pr_comment("gh-two", "auth.go", 12, "", 0, "F-1").await;
    assert!(ok);
    assert_eq!(removed.id, "removed");
    let pending = px.pr("gh-two").pending_comments;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].finding_id, "F-2");
    assert!(!eng.add_pr_comment("gh-two", "auth.go", 12, "", 0, "F-9").await.1);
}

/// A workspace on the project plus one open finding per id.
fn findings_ws(px: &Px, id: &str, ids: &[&str]) {
    px.add_ws(Workspace {
        id: id.into(),
        project_id: "acme".into(),
        ..Workspace::default()
    });
    for (i, f) in ids.iter().enumerate() {
        px.te.store.add_finding(Finding {
            id: (*f).into(),
            workspace_id: id.into(),
            file: "auth.go".into(),
            line: 12 + i as i64 * 8,
            severity: "high".into(),
            message: "nits".into(),
            status: FindingStatus::Open.as_str().into(),
            ..Finding::default()
        });
    }
}

fn finding_status(px: &Px, ws: &str, id: &str) -> Finding {
    px.te
        .store
        .findings(ws)
        .into_iter()
        .find(|f| f.id == id)
        .expect("finding")
}

fn saw_findings_updated(px: &Px, ws: &str) -> bool {
    px.te.events.all().iter().any(|e| {
        matches!(e, EngineEvent::FindingsUpdated(r) if r.workspace_id == ws)
    })
}

#[tokio::test]
async fn submit_review_resolves_pending_findings() {
    let (px, _repo) = conv(Pr {
        workspace_id: "ws-r".into(),
        ..open_pr("gh-resolve", 25)
    });
    findings_ws(&px, "ws-r", &["F-1"]);
    capture_reviews(&px);
    px.forge.on_snapshot(snap_ok(PrSnapshot {
        state: "OPEN".into(),
        title: "pr".into(),
        ..PrSnapshot::default()
    }));
    px.eng().add_pr_comment("gh-resolve", "auth.go", 12, "nits", 0, "F-1").await;
    assert!(px.eng().submit_review("gh-resolve", "comment", "wrap").await.1);
    assert_eq!(finding_status(&px, "ws-r", "F-1").status, "resolved");
    assert!(saw_findings_updated(&px, "ws-r"));
}

#[tokio::test]
async fn send_back_resolves_pending_findings() {
    let (px, _repo) = conv(Pr {
        workspace_id: "ws-sb".into(),
        ..open_pr("gh-sendback-resolve", 26)
    });
    findings_ws(&px, "ws-sb", &["F-sb"]);
    capture_reviews(&px);
    px.eng().add_pr_comment("gh-sendback-resolve", "auth.go", 12, "nits", 0, "F-sb").await;
    assert!(px.eng().send_back_to_loop("gh-sendback-resolve", &["fix it".into()]).await.1);
    assert!(px.pr("gh-sendback-resolve").pending_comments.is_empty());
    let f = finding_status(&px, "ws-sb", "F-sb");
    assert_eq!(f.status, "resolved");
    assert_eq!(f.updated_by, FINDING_ACTOR_ENGINE);
    assert!(saw_findings_updated(&px, "ws-sb"));
}

#[tokio::test]
async fn send_back_preserves_pending_on_forge_failure() {
    let (px, _repo) = conv(Pr {
        workspace_id: "ws-sbf".into(),
        ..open_pr("gh-sendback-fail", 27)
    });
    findings_ws(&px, "ws-sbf", &["F-sb-fail"]);
    px.forge
        .on_submit_review(|_| async { Err(ForgeError::failed("network down")) });
    px.eng().add_pr_comment("gh-sendback-fail", "auth.go", 12, "nits", 0, "F-sb-fail").await;
    assert!(!px.eng().send_back_to_loop("gh-sendback-fail", &["fix it".into()]).await.1);
    let pending = px.pr("gh-sendback-fail").pending_comments;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].finding_id, "F-sb-fail");
    assert_eq!(finding_status(&px, "ws-sbf", "F-sb-fail").status, "open");
}

#[tokio::test]
async fn send_back_resolves_local_pending_findings() {
    let px = Px::new();
    px.add_ws(Workspace {
        id: "L-sb-find".into(),
        project_id: "P".into(),
        goal: "Add OAuth".into(),
        state: WorkspaceState::PrOpen,
        run_started: true,
        loop_id: "recur.loops.pr".into(),
        ..Workspace::default()
    });
    px.te.store.add_finding(Finding {
        id: "F-local".into(),
        workspace_id: "L-sb-find".into(),
        file: "auth.go".into(),
        line: 8,
        severity: "medium".into(),
        message: "unwrap".into(),
        status: "open".into(),
        ..Finding::default()
    });
    px.add_pr(Pr {
        id: "pr-sb-find".into(),
        workspace_id: "L-sb-find".into(),
        number: 12,
        title: "Add OAuth".into(),
        state: "In review".into(),
        group: "you".into(),
        pending_comments: vec![PendingLineComment {
            file: "auth.go".into(),
            line: 8,
            body: "unwrap this error".into(),
            finding_id: "F-local".into(),
            ..PendingLineComment::default()
        }],
        ..Pr::default()
    });
    assert!(px.eng().send_back_to_loop("pr-sb-find", &["fix the unwrap".into()]).await.1);
    assert_eq!(finding_status(&px, "L-sb-find", "F-local").status, "resolved");
}

#[tokio::test]
async fn submit_review_leaves_dismissed_pending_findings() {
    let (px, _repo) = conv(Pr {
        workspace_id: "ws-d".into(),
        pending_comments: ["F-ig", "F-inv", "F-open"]
            .iter()
            .enumerate()
            .map(|(i, f)| PendingLineComment {
                file: "auth.go".into(),
                line: 12 + i as i64 * 8,
                body: format!("{f} draft"),
                finding_id: (*f).into(),
                ..PendingLineComment::default()
            })
            .collect(),
        ..open_pr("gh-dismiss", 27)
    });
    findings_ws(&px, "ws-d", &["F-ig", "F-inv", "F-open"]);
    px.te.store.set_finding_status("ws-d", "F-ig", "ignored").unwrap();
    px.te.store.set_finding_status("ws-d", "F-inv", "invalid").unwrap();
    capture_reviews(&px);
    px.forge.on_snapshot(snap_ok(PrSnapshot {
        state: "OPEN".into(),
        title: "pr".into(),
        ..PrSnapshot::default()
    }));
    assert!(px.eng().submit_review("gh-dismiss", "comment", "wrap").await.1);
    assert_eq!(finding_status(&px, "ws-d", "F-ig").status, "ignored");
    assert_eq!(finding_status(&px, "ws-d", "F-inv").status, "invalid");
    assert_eq!(finding_status(&px, "ws-d", "F-open").status, "resolved");
}

#[tokio::test]
async fn add_comment_pending_only_event() {
    let (px, _repo) = conv(open_pr("pend-1", 1));
    px.eng().add_pr_comment("pend-1", "auth.go", 12, "nits", 0, "").await;
    let ev = px
        .te
        .events
        .all()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::PrUpdated(u) if u.pr_id == "pend-1" => Some(u),
            _ => None,
        })
        .last()
        .expect("pr.updated");
    assert!(ev.pending_only);
    let pending = ev.pending_comments.unwrap();
    assert_eq!((pending[0].file.as_str(), pending[0].line), ("auth.go", 12));
}

#[tokio::test]
async fn submit_review_forge_posts() {
    let (px, _repo) = conv(open_pr("gh-22", 22));
    let reviews = capture_reviews(&px);
    px.forge.on_snapshot(snap_ok(PrSnapshot {
        state: "OPEN".into(),
        review_decision: "APPROVED".into(),
        title: "pr".into(),
        ..PrSnapshot::default()
    }));
    let (updated, ok) = px.eng().submit_review("gh-22", "approve", "ship it").await;
    assert!(ok);
    let r = reviews.lock();
    assert_eq!((r[0].0.as_str(), r[0].1.as_str()), ("approve", "ship it"));
    assert_eq!(updated.state, "Approved");
}

#[tokio::test]
async fn submit_review_forge_rejects_own_pr() {
    let (px, _repo) = conv(Pr {
        author_login: "avery".into(),
        viewer_login: "avery".into(),
        viewer_can_write: true,
        ..open_pr("gh-23", 23)
    });
    let (_, ok) = px.eng().submit_review("gh-23", "approve", "ship it").await;
    assert!(!ok);
    assert!(px.forge.calls_to("submit_review").is_empty());
}

#[tokio::test]
async fn add_comment_forge_failure() {
    let (px, _repo) = conv(open_pr("gh-23", 23));
    px.forge
        .on_post_comment(|_| async { Err(ForgeError::failed("network down")) });
    let (_, ok) = px.eng().add_pr_comment("gh-23", "", 0, "hi", 0, "").await;
    assert!(!ok);
    assert!(
        feed_text(&px)
            .iter()
            .any(|t| t == "Could not post the comment to GitHub: network down")
    );
}

#[tokio::test]
async fn add_comment_forge_posts_conversation_note() {
    let (px, _repo) = conv(open_pr("gh-24", 24));
    px.forge.on_snapshot(snap_ok(PrSnapshot {
        state: "OPEN".into(),
        comments: vec![comment("gh-comment-9", "you", "hi")],
        ..PrSnapshot::default()
    }));
    let (c, ok) = px.eng().add_pr_comment("gh-24", "", 0, "hi", 0, "").await;
    assert!(ok);
    assert_eq!(c.id, "gh-comment-9", "the refreshed thread's last row");
    assert_eq!(px.forge.calls_to("post_comment").len(), 1);
}
