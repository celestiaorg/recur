//! Conversation refresh, drafts, comments and reviews.

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

fn snap_ok(
    snap: PrSnapshot,
) -> impl Fn((std::path::PathBuf, i64)) -> std::future::Ready<Result<PrSnapshot, ForgeError>> {
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
    assert_eq!(
        (got.state.as_str(), got.group.as_str()),
        ("Merged", "merged")
    );
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
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "blocked on the snapshot"
    );
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
    assert_eq!(
        (got.state.as_str(), got.group.as_str()),
        ("Merged", "merged")
    );
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
    assert_eq!(
        px.pr("gh-methods").allowed_merge_methods,
        ["squash", "rebase"]
    );
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
    assert!(
        feed_text(&px)
            .iter()
            .any(|t| t == "Convert to draft failed: gh: boom")
    );
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
type Reviews = Arc<Mutex<Vec<(String, String, Vec<PendingLineComment>)>>>;

fn capture_reviews(px: &Px) -> Reviews {
    let got = Arc::new(Mutex::new(Vec::new()));
    let g = Arc::clone(&got);
    px.forge
        .on_submit_review(move |(_, _, kind, body, comments)| {
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
    let (_, ok) = px
        .eng()
        .add_pr_comment("gh-21", "auth.go", 12, "nits", 0, "")
        .await;
    assert!(ok);
    assert!(
        px.forge.calls_to("post_comment").is_empty(),
        "pending until submit"
    );
    let got = px.pr("gh-21");
    assert_eq!(got.pending_comments.len(), 1);
    assert_eq!(
        (
            got.pending_comments[0].file.as_str(),
            got.pending_comments[0].line
        ),
        ("auth.go", 12)
    );
    assert!(got.comments.unwrap_or_default().is_empty());
    let (_, ok) = px
        .eng()
        .submit_review("gh-21", "comment", "nits wrap")
        .await;
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
    let (_, ok) = px
        .eng()
        .add_pr_comment("gh-22", "auth.go", 12, "span", 10, "")
        .await;
    assert!(ok);
    let pc = px.pr("gh-22").pending_comments[0].clone();
    assert_eq!(
        (
            pc.file.as_str(),
            pc.line,
            pc.start_line,
            pc.side.as_str(),
            pc.start_side.as_str()
        ),
        ("auth.go", 12, 10, "RIGHT", "RIGHT")
    );
    px.eng().submit_review("gh-22", "comment", "wrap").await;
    let r = reviews.lock();
    assert_eq!((r[0].2[0].start_line, r[0].2[0].line), (10, 12));
}

#[tokio::test]
async fn add_comment_left_side_and_bad_range() {
    let (px, _repo) = conv(open_pr("gh-l", 30));
    px.eng()
        .add_pr_comment("gh-l", "auth.go", -7, "old side", 12, "")
        .await;
    let pc = px.pr("gh-l").pending_comments[0].clone();
    assert_eq!(
        (
            pc.line,
            pc.side.as_str(),
            pc.start_line,
            pc.start_side.as_str()
        ),
        (7, "LEFT", 0, "")
    );
}

#[tokio::test]
async fn add_comment_persists_finding_id() {
    let (px, _repo) = conv(open_pr("gh-find", 23));
    px.eng()
        .add_pr_comment("gh-find", "auth.go", 12, "nits", 0, "F-1")
        .await;
    assert_eq!(px.pr("gh-find").pending_comments[0].finding_id, "F-1");
}

#[tokio::test]
async fn add_comment_delete_removes_only_matching_draft() {
    let (px, _repo) = conv(open_pr("gh-two", 24));
    let eng = px.eng();
    assert!(
        eng.add_pr_comment("gh-two", "auth.go", 12, "first", 0, "F-1")
            .await
            .1
    );
    assert!(
        eng.add_pr_comment("gh-two", "auth.go", 12, "second", 0, "F-2")
            .await
            .1
    );
    let (removed, ok) = eng
        .add_pr_comment("gh-two", "auth.go", 12, "", 0, "F-1")
        .await;
    assert!(ok);
    assert_eq!(removed.id, "removed");
    let pending = px.pr("gh-two").pending_comments;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].finding_id, "F-2");
    assert!(
        !eng.add_pr_comment("gh-two", "auth.go", 12, "", 0, "F-9")
            .await
            .1
    );
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
    px.te
        .events
        .all()
        .iter()
        .any(|e| matches!(e, EngineEvent::FindingsUpdated(r) if r.workspace_id == ws))
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
    px.eng()
        .add_pr_comment("gh-resolve", "auth.go", 12, "nits", 0, "F-1")
        .await;
    assert!(
        px.eng()
            .submit_review("gh-resolve", "comment", "wrap")
            .await
            .1
    );
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
    px.eng()
        .add_pr_comment("gh-sendback-resolve", "auth.go", 12, "nits", 0, "F-sb")
        .await;
    assert!(
        px.eng()
            .send_back_to_loop("gh-sendback-resolve", &["fix it".into()])
            .await
            .1
    );
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
    px.eng()
        .add_pr_comment("gh-sendback-fail", "auth.go", 12, "nits", 0, "F-sb-fail")
        .await;
    assert!(
        !px.eng()
            .send_back_to_loop("gh-sendback-fail", &["fix it".into()])
            .await
            .1
    );
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
    assert!(
        px.eng()
            .send_back_to_loop("pr-sb-find", &["fix the unwrap".into()])
            .await
            .1
    );
    assert_eq!(
        finding_status(&px, "L-sb-find", "F-local").status,
        "resolved"
    );
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
    px.te
        .store
        .set_finding_status("ws-d", "F-ig", "ignored")
        .unwrap();
    px.te
        .store
        .set_finding_status("ws-d", "F-inv", "invalid")
        .unwrap();
    capture_reviews(&px);
    px.forge.on_snapshot(snap_ok(PrSnapshot {
        state: "OPEN".into(),
        title: "pr".into(),
        ..PrSnapshot::default()
    }));
    assert!(
        px.eng()
            .submit_review("gh-dismiss", "comment", "wrap")
            .await
            .1
    );
    assert_eq!(finding_status(&px, "ws-d", "F-ig").status, "ignored");
    assert_eq!(finding_status(&px, "ws-d", "F-inv").status, "invalid");
    assert_eq!(finding_status(&px, "ws-d", "F-open").status, "resolved");
}

#[tokio::test]
async fn add_comment_pending_only_event() {
    let (px, _repo) = conv(open_pr("pend-1", 1));
    px.eng()
        .add_pr_comment("pend-1", "auth.go", 12, "nits", 0, "")
        .await;
    let ev = px
        .te
        .events
        .all()
        .into_iter()
        .rev()
        .find_map(|e| match e {
            EngineEvent::PrUpdated(u) if u.pr_id == "pend-1" => Some(u),
            _ => None,
        })
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

#[tokio::test]
async fn locked_pr_refuses_conversation_writes() {
    let (px, _repo) = conv(Pr {
        state: "Merged".into(),
        group: "merged".into(),
        comments: Some(vec![PrComment {
            id: "gh-inline-1".into(),
            kind: "comment".into(),
            path: "a.go".into(),
            line: 1,
            ..PrComment::default()
        }]),
        ..open_pr("merged-1", 1)
    });
    let eng = px.eng();
    assert!(
        !eng.add_pr_comment("merged-1", "", 0, "too late", 0, "")
            .await
            .1
    );
    assert!(!eng.submit_review("merged-1", "approve", "lgtm").await.1);
    assert!(
        !eng.reply_pr_comment("merged-1", "gh-inline-1", "too late")
            .await
            .1
    );
    assert!(!eng.update_pr_details("merged-1", "nope", "nope").await.1);
}

fn local_pr(id: &str, comments: Vec<PrComment>) -> Pr {
    Pr {
        id: id.into(),
        number: 10,
        source: PrSource::Local,
        title: "old".into(),
        comments: Some(comments),
        ..Pr::default()
    }
}

#[tokio::test]
async fn update_details_local() {
    let px = Px::new();
    px.add_pr(local_pr(
        "10",
        vec![PrComment {
            id: "gh-description".into(),
            kind: "description".into(),
            body_md: "old body".into(),
            ..PrComment::default()
        }],
    ));
    let (got, ok) = px
        .eng()
        .update_pr_details("10", "new title", "new body")
        .await;
    assert!(ok);
    assert_eq!(got.title, "new title");
    assert_eq!(got.comments.unwrap()[0].body_md, "new body");
    assert!(
        !px.eng().update_pr_details("10", "  ", "x").await.1,
        "a title is required"
    );
}

#[tokio::test]
async fn update_comment_local() {
    let px = Px::new();
    px.add_pr(local_pr(
        "11",
        vec![PrComment {
            id: "c-you".into(),
            author: "You".into(),
            body_html: "old".into(),
            body_md: "old".into(),
            ..PrComment::default()
        }],
    ));
    let (got, ok) = px
        .eng()
        .update_pr_comment("11", "c-you", "edited <b>")
        .await;
    assert!(ok);
    assert_eq!(got.body_md, "edited <b>");
    assert_eq!(got.body_html, "edited &lt;b&gt;");
    assert!(!px.eng().update_pr_comment("11", "missing", "x").await.1);
}

#[tokio::test]
async fn update_details_forge() {
    let (px, _repo) = conv(Pr {
        title: "old".into(),
        author_login: "avery".into(),
        viewer_login: "avery".into(),
        viewer_can_write: true,
        ..open_pr("gh-24", 24)
    });
    let (got, ok) = px
        .eng()
        .update_pr_details("gh-24", "ship it", "please review")
        .await;
    assert!(ok);
    let edits = px.forge.calls_to("edit_pr");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0][1..], ["24", "ship it", "please review"]);
    assert_eq!(got.title, "ship it");

    let (got, ok) = px
        .eng()
        .update_pr_details("gh-24", "ship it 2", "please review")
        .await;
    assert!(ok);
    assert_eq!(
        px.forge.calls_to("edit_pr")[1].len(),
        3,
        "an unchanged body is omitted from the edit"
    );
    assert_eq!(got.title, "ship it 2");

    px.add_pr(Pr {
        title: "other".into(),
        author_login: "loopbot".into(),
        viewer_login: "avery".into(),
        ..open_pr("gh-25", 25)
    });
    assert!(
        !px.eng().update_pr_details("gh-25", "nope", "nope").await.1,
        "a reader cannot edit"
    );
}

#[tokio::test]
async fn update_comment_forge_skips_refresh() {
    let (px, _repo) = conv(Pr {
        viewer_login: "avery".into(),
        comments: Some(vec![PrComment {
            id: "gh-comment-IC_1".into(),
            author: "avery".into(),
            author_login: "avery".into(),
            body_md: "old".into(),
            ..PrComment::default()
        }]),
        ..open_pr("gh-26", 26)
    });
    let (got, ok) = px
        .eng()
        .update_pr_comment("gh-26", "gh-comment-IC_1", "fixed")
        .await;
    assert!(ok);
    assert!(
        px.forge.calls_to("snapshot").is_empty(),
        "persisted locally"
    );
    assert_eq!(
        px.forge.calls_to("edit_comment")[0][2..],
        ["gh-comment-IC_1", "fixed"]
    );
    assert_eq!(got.body_md, "fixed");
}

#[tokio::test]
async fn reply_forge_skips_refresh() {
    let (px, _repo) = conv(Pr {
        url: "https://github.com/acme/app/pull/27".into(),
        viewer_login: "avery".into(),
        comments: Some(vec![PrComment {
            id: "gh-inline-1".into(),
            parent_id: "gh-review-9".into(),
            path: "auth.go".into(),
            line: 12,
            thread_id: "PRRT_1".into(),
            body_md: "hmm".into(),
            ..PrComment::default()
        }]),
        ..open_pr("gh-27", 27)
    });
    px.forge.on_reply(|(_, _, _, body, _)| async move {
        Ok(Some(PrComment {
            id: "gh-inline-999".into(),
            body_md: body,
            path: "auth.go".into(),
            line: 12,
            in_reply_to_id: "gh-inline-1".into(),
            author_login: "avery".into(),
            ..PrComment::default()
        }))
    });
    let (got, ok) = px
        .eng()
        .reply_pr_comment("gh-27", "gh-inline-1", "thread reply")
        .await;
    assert!(ok);
    assert!(px.forge.calls_to("snapshot").is_empty());
    assert_eq!(got.id, "gh-inline-999");
    assert_eq!(got.in_reply_to_id, "gh-inline-1");
    assert_eq!(got.parent_id, "gh-review-9");
    assert_eq!(got.thread_id, "PRRT_1");
    assert!(
        px.pr("gh-27")
            .comments
            .unwrap()
            .iter()
            .any(|c| c.id == "gh-inline-999")
    );
    assert!(px.te.events.has("pr.comment"));
}

#[tokio::test]
async fn reply_local_threads_under_root() {
    let px = Px::new();
    px.add_pr(local_pr(
        "12",
        vec![PrComment {
            id: "c-root".into(),
            author: "You".into(),
            path: "a.go".into(),
            line: 3,
            side: "RIGHT".into(),
            ..PrComment::default()
        }],
    ));
    let (got, ok) = px.eng().reply_pr_comment("12", "c-root", "agreed").await;
    assert!(ok);
    assert_eq!(
        (got.in_reply_to_id.as_str(), got.thread_id.as_str()),
        ("c-root", "c-root")
    );
    assert_eq!((got.path.as_str(), got.line), ("a.go", 3));
    let (_, ok) = px.eng().reply_pr_comment("12", "nope", "x").await;
    assert!(!ok);
}

#[tokio::test]
async fn drop_remembered_comments() {
    let px = Px::new();
    px.eng()
        .remember_dropped_comments("p1", &["gh-inline-1".into(), "gh-inline-2".into()]);
    let got = px.eng().drop_remembered_comments(
        "p1",
        vec![
            comment("gh-inline-1", "", ""),
            comment("gh-inline-3", "", ""),
        ],
    );
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].id, "gh-inline-3");
    px.eng().age_pr_holds(Duration::from_secs(180));
    let got = px
        .eng()
        .drop_remembered_comments("p1", vec![comment("gh-inline-1", "", "")]);
    assert_eq!(got.len(), 1, "an expired hold lets the row back");
}

#[tokio::test]
async fn delete_forge_comment_skips_refresh_and_holds() {
    let (px, _repo) = conv(Pr {
        viewer_login: "avery".into(),
        viewer_can_write: true,
        comments: Some(vec![
            PrComment {
                id: "gh-inline-1".into(),
                author_login: "avery".into(),
                path: "auth.go".into(),
                line: 12,
                body_md: "hmm".into(),
                ..PrComment::default()
            },
            PrComment {
                id: "gh-inline-2".into(),
                in_reply_to_id: "gh-inline-1".into(),
                path: "auth.go".into(),
                line: 12,
                body_md: "reply".into(),
                ..PrComment::default()
            },
        ]),
        ..open_pr("gh-28", 28)
    });
    let (got, ok) = px.eng().delete_pr_comment("gh-28", "gh-inline-1").await;
    assert!(ok);
    assert!(px.forge.calls_to("snapshot").is_empty());
    assert_eq!(px.forge.calls_to("delete_comment")[0][2], "gh-inline-1");
    assert!(
        got.comments.unwrap_or_default().is_empty(),
        "root and replies gone"
    );
    let held = px
        .eng()
        .drop_remembered_comments("gh-28", vec![comment("gh-inline-1", "", "")]);
    assert!(
        held.is_empty(),
        "a stale snapshot cannot revive the deleted row"
    );
}

#[tokio::test]
async fn delete_forge_conversation_comment() {
    let (px, _repo) = conv(Pr {
        viewer_login: "avery".into(),
        viewer_can_write: true,
        comments: Some(vec![
            PrComment {
                id: "gh-comment-IC_1".into(),
                author_login: "avery".into(),
                body_md: "looks good".into(),
                ..PrComment::default()
            },
            PrComment {
                id: "gh-review-PRR_1".into(),
                author_login: "avery".into(),
                body_md: "ship it".into(),
                ..PrComment::default()
            },
        ]),
        ..open_pr("gh-30", 30)
    });
    let (got, ok) = px.eng().delete_pr_comment("gh-30", "gh-comment-IC_1").await;
    assert!(ok);
    assert!(
        got.comments
            .unwrap()
            .iter()
            .all(|c| c.id != "gh-comment-IC_1")
    );
    assert!(
        !px.eng()
            .delete_pr_comment("gh-30", "gh-review-PRR_1")
            .await
            .1,
        "reviews stay"
    );
}

#[tokio::test]
async fn delete_forge_comment_restores_on_failure() {
    let (px, _repo) = conv(Pr {
        viewer_login: "avery".into(),
        viewer_can_write: true,
        created_at: "2026-08-01T12:00:00Z".into(),
        updated_at: "2026-08-01T12:00:00Z".into(),
        comments: Some(vec![PrComment {
            id: "gh-inline-1".into(),
            author_login: "avery".into(),
            path: "auth.go".into(),
            line: 12,
            body_md: "hmm".into(),
            ..PrComment::default()
        }]),
        ..open_pr("gh-29", 29)
    });
    px.forge
        .on_delete_comment(|_| async { Err(ForgeError::failed("denied")) });
    assert!(!px.eng().delete_pr_comment("gh-29", "gh-inline-1").await.1);
    let stored = px.pr("gh-29");
    assert!(
        stored
            .comments
            .unwrap()
            .iter()
            .any(|c| c.id == "gh-inline-1")
    );
    assert_eq!(stored.updated_at, "2026-08-01T12:00:00Z");
    assert!(
        px.eng()
            .drop_remembered_comments("gh-29", vec![comment("gh-inline-1", "", "")])
            .len()
            == 1,
        "the hold is forgotten"
    );
}

#[tokio::test]
async fn delete_local_comment() {
    let px = Px::new();
    px.add_pr(local_pr(
        "13",
        vec![
            PrComment {
                id: "c1".into(),
                author: "You".into(),
                ..PrComment::default()
            },
            PrComment {
                id: "c2".into(),
                author: "You".into(),
                in_reply_to_id: "c1".into(),
                ..PrComment::default()
            },
        ],
    ));
    let (got, ok) = px.eng().delete_pr_comment("13", "c1").await;
    assert!(ok);
    assert!(got.comments.unwrap_or_default().is_empty());
}

#[tokio::test]
async fn apply_remembered_thread_resolved() {
    let px = Px::new();
    px.eng().remember_thread_resolved("p1", "PRRT_1", true);
    let got = px.eng().apply_remembered_thread_resolved(
        "p1",
        vec![
            PrComment {
                id: "gh-inline-1".into(),
                thread_id: "PRRT_1".into(),
                ..PrComment::default()
            },
            PrComment {
                id: "gh-inline-2".into(),
                thread_id: "PRRT_2".into(),
                ..PrComment::default()
            },
        ],
    );
    assert!(got[0].resolved && !got[1].resolved);
}

fn resolvable() -> Pr {
    Pr {
        viewer_login: "avery".into(),
        viewer_can_write: true,
        comments: Some(vec![PrComment {
            id: "gh-inline-1".into(),
            path: "auth.go".into(),
            line: 12,
            thread_id: "PRRT_1".into(),
            body_md: "hmm".into(),
            ..PrComment::default()
        }]),
        ..open_pr("gh-29", 29)
    }
}

#[tokio::test]
async fn set_forge_thread_resolved_skips_refresh() {
    let (px, _repo) = conv(resolvable());
    let (got, ok) = px
        .eng()
        .set_pr_thread_resolved("gh-29", "PRRT_1", true)
        .await;
    assert!(ok);
    assert!(px.forge.calls_to("snapshot").is_empty());
    assert_eq!(
        px.forge.calls_to("set_thread_resolved")[0][1..],
        ["PRRT_1", "true"]
    );
    assert!(got.comments.unwrap()[0].resolved);
    let ev = px
        .te
        .events
        .all()
        .into_iter()
        .rev()
        .find_map(|e| match e {
            EngineEvent::PrUpdated(u) if u.pr_id == "gh-29" => Some(u),
            _ => None,
        })
        .unwrap();
    assert!(ev.pending_only);
}

#[tokio::test]
async fn set_forge_thread_resolved_refuses_row_ids() {
    let (px, _repo) = conv(Pr {
        comments: Some(vec![PrComment {
            id: "gh-inline-5".into(),
            path: "a.go".into(),
            line: 1,
            ..PrComment::default()
        }]),
        ..resolvable()
    });
    let (_, ok) = px
        .eng()
        .set_pr_thread_resolved("gh-29", "gh-inline-5", true)
        .await;
    assert!(
        !ok,
        "a note with no thread node cannot resolve on the forge"
    );
    assert!(px.forge.calls_to("set_thread_resolved").is_empty());
}

#[tokio::test]
async fn refresh_keeps_recent_resolve() {
    let mut pr = resolvable();
    pr.comments.as_mut().unwrap()[0].resolved = true;
    let (px, _repo) = conv(Pr {
        id: "gh-30".into(),
        number: 30,
        ..pr
    });
    px.eng().remember_thread_resolved("gh-30", "PRRT_1", true);
    px.forge.on_snapshot(snap_ok(PrSnapshot {
        state: "OPEN".into(),
        comments: vec![PrComment {
            id: "gh-inline-1".into(),
            path: "auth.go".into(),
            line: 12,
            thread_id: "PRRT_1".into(),
            body_md: "hmm".into(),
            ..PrComment::default()
        }],
        ..PrSnapshot::default()
    }));
    px.eng().refresh_pr_conversation("gh-30").await;
    let cs = px.pr("gh-30").comments.unwrap();
    assert_eq!(cs.len(), 1);
    assert!(cs[0].resolved);
}

#[tokio::test]
async fn set_local_thread_resolved() {
    let px = Px::new();
    px.add_pr(local_pr(
        "14",
        vec![
            PrComment {
                id: "c-root".into(),
                path: "a.go".into(),
                line: 2,
                ..PrComment::default()
            },
            PrComment {
                id: "c-reply".into(),
                in_reply_to_id: "c-root".into(),
                ..PrComment::default()
            },
        ],
    ));
    let (got, ok) = px.eng().set_pr_thread_resolved("14", "c-reply", true).await;
    assert!(ok);
    let cs = got.comments.unwrap();
    assert!(
        cs.iter().all(|c| c.resolved && c.thread_id == "c-root"),
        "{cs:?}"
    );
}

#[tokio::test]
async fn refresh_persists_outdated() {
    let stored = PrComment {
        id: "gh-inline-1".into(),
        path: "b.go".into(),
        line: 9,
        start_line: 7,
        side: "LEFT".into(),
        diff_hunk: "@@ -7,3 +7,3 @@".into(),
        body_md: "stale".into(),
        body_html: "<code>b.go:7-9</code>".into(),
        ..PrComment::default()
    };
    let (px, _repo) = conv(Pr {
        comments: Some(vec![stored.clone()]),
        ..open_pr("gh-31", 31)
    });
    px.forge.on_snapshot(snap_ok(PrSnapshot {
        state: "OPEN".into(),
        comments: vec![PrComment {
            outdated: true,
            ..stored
        }],
        ..PrSnapshot::default()
    }));
    assert!(px.eng().refresh_pr_conversation("gh-31").await);
    let cs = px.pr("gh-31").comments.unwrap();
    assert!(cs[0].outdated);
    assert_eq!(cs[0].line, 9);
}

#[tokio::test]
async fn toggle_reaction_local() {
    let px = Px::new();
    px.add_pr(Pr {
        id: "local-rxn".into(),
        source: PrSource::Local,
        comments: Some(vec![PrComment {
            id: "c1".into(),
            author: "You".into(),
            body_md: "hi".into(),
            ..PrComment::default()
        }]),
        ..Pr::default()
    });
    let (got, ok) = px
        .eng()
        .toggle_pr_comment_reaction("local-rxn", "c1", "+1")
        .await;
    assert!(ok);
    assert_eq!(
        got.comments.unwrap()[0].reactions,
        [recur_domain::PrReaction {
            content: "+1".into(),
            count: 1,
            mine: true
        }]
    );
    let (got, ok) = px
        .eng()
        .toggle_pr_comment_reaction("local-rxn", "c1", "+1")
        .await;
    assert!(ok);
    assert!(got.comments.unwrap()[0].reactions.is_empty());
    assert!(
        !px.eng()
            .toggle_pr_comment_reaction("local-rxn", "c1", "nope")
            .await
            .1
    );
}

#[tokio::test]
async fn toggle_reaction_forge() {
    let (px, _repo) = conv(Pr {
        viewer_login: "avery".into(),
        node_id: "PR_1".into(),
        comments: Some(vec![PrComment {
            id: "gh-comment-IC_1".into(),
            node_id: "IC_1".into(),
            author_login: "sam".into(),
            body_md: "looks good".into(),
            ..PrComment::default()
        }]),
        ..open_pr("gh-rxn", 12)
    });
    let (updated, ok) = px
        .eng()
        .toggle_pr_comment_reaction("gh-rxn", "gh-comment-IC_1", "heart")
        .await;
    assert!(ok);
    assert_eq!(
        px.forge.calls_to("set_reaction")[0][1..],
        ["IC_1", "heart", "true"]
    );
    assert!(recur_domain::reaction_mine(
        &updated.comments.unwrap()[0].reactions,
        "heart"
    ));
    let (updated, ok) = px
        .eng()
        .toggle_pr_comment_reaction("gh-rxn", "gh-comment-IC_1", "heart")
        .await;
    assert!(ok);
    assert_eq!(px.forge.calls_to("set_reaction")[1][3], "false");
    assert!(!recur_domain::reaction_mine(
        &updated.comments.unwrap()[0].reactions,
        "heart"
    ));
}

#[tokio::test]
async fn refresh_keeps_recent_reaction() {
    let heart = vec![recur_domain::PrReaction {
        content: "heart".into(),
        count: 1,
        mine: true,
    }];
    let (px, _repo) = conv(Pr {
        viewer_login: "avery".into(),
        comments: Some(vec![PrComment {
            id: "gh-comment-IC_1".into(),
            body_md: "looks good".into(),
            reactions: heart.clone(),
            ..PrComment::default()
        }]),
        ..open_pr("gh-31", 31)
    });
    px.eng()
        .remember_comment_reactions("gh-31", "gh-comment-IC_1", &heart);
    px.forge.on_snapshot(snap_ok(PrSnapshot {
        state: "OPEN".into(),
        comments: vec![PrComment {
            id: "gh-comment-IC_1".into(),
            body_md: "looks good".into(),
            ..PrComment::default()
        }],
        ..PrSnapshot::default()
    }));
    px.eng().refresh_pr_conversation("gh-31").await;
    let cs = px.pr("gh-31").comments.unwrap();
    assert!(recur_domain::reaction_mine(&cs[0].reactions, "heart"));
}

#[tokio::test]
async fn mentionable_users_through_the_forge() {
    let (px, _repo) = conv(Pr {
        url: "https://github.com/acme/app/pull/40".into(),
        ..open_pr("gh-40", 40)
    });
    px.forge.on_mentionable_users(|(_, _, q)| async move {
        vec![PrParticipant {
            login: format!("{q}-match"),
            ..PrParticipant::default()
        }]
    });
    let got = px.eng().search_pr_mentionable_users("gh-40", "  av ").await;
    assert_eq!(got[0].login, "av-match");
    px.add_pr(local_pr("15", Vec::new()));
    assert!(
        px.eng()
            .search_pr_mentionable_users("15", "av")
            .await
            .is_empty()
    );
}
