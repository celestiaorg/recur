//! Port of Go `feedproject_test.go`: the reload projection reproduces the
//! live feed of a converging loop, and stored provider tool names render in
//! the common vocabulary.

use std::sync::Arc;
use std::time::Duration;

use recur_domain::{
    AgentRole, AutonomyLevel, ContentBlock, FEED_KIND_OUTCOME, FEED_KIND_TOOL, FeedItem, Message,
    PROVIDER_CURSOR, Session, WorkspaceState,
};

use crate::loops::specialists::step_kind;
use crate::loops::test_support::{ConvergeRunner, StepEvidence, verify_dev_project};
use crate::testing::{TestEngine, TestOptions, add_project, init_repo, wait_for};

fn key(it: &FeedItem) -> String {
    format!(
        "{}\0{}\0{}\0{}",
        it.agent_code, it.kind, it.text, it.tool_result
    )
}

fn multiset(items: &[FeedItem]) -> Vec<String> {
    let mut out: Vec<String> = items.iter().map(key).collect();
    out.sort();
    out
}

fn kind_count(items: &[FeedItem], kind: &str) -> usize {
    items.iter().filter(|it| it.kind == kind).count()
}

/// Multiset, not order: a scripted run fires many events in one
/// millisecond, so the merge's tie-break is not meaningful here. No row may
/// be lost, invented or misattributed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_feed_projection_matches_the_live_feed() {
    let repo = init_repo();
    let runner = Arc::new(ConvergeRunner {
        findings_per_iter: vec![2, 0],
        evidence_always: vec![
            StepEvidence {
                step: "Booted the app and opened /",
                shot: "/shots/01.png",
                passed: true,
            },
            StepEvidence {
                step: "Exercised the feature",
                shot: "/shots/02.png",
                passed: true,
            },
        ],
        ..ConvergeRunner::default()
    });
    let t = TestEngine::new(Some(runner.provider()));
    t.install_fixture_pack();
    let Some(proj) = verify_dev_project(&t.store, &repo.path) else {
        eprintln!("python3 not available; verify needs a bootable dev server");
        return;
    };
    let id = t
        .engine
        .start_workspace_with(
            &proj,
            "Add a verifiable, reviewable feature",
            "",
            AutonomyLevel::default(),
            "test.loops.verify",
            "",
        )
        .await
        .unwrap();
    wait_for(Duration::from_secs(30), "loop to reach pr_open", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::PrOpen)
    })
    .await;
    let live = t.store.feed(&id).unwrap_or_default();
    assert!(!live.is_empty(), "live feed is empty");
    let projected = t.engine.reconstruct_feed(&id);
    let (l, p) = (multiset(&live), multiset(&projected));
    assert_eq!(l.len(), p.len(), "feed row count");
    for (i, (a, b)) in l.iter().zip(&p).enumerate() {
        assert_eq!(a, b, "feed projection diverges at {i}");
    }
    for kind in [FEED_KIND_TOOL, FEED_KIND_OUTCOME] {
        assert!(kind_count(&live, kind) > 0, "live feed had no {kind} rows");
        assert_eq!(
            kind_count(&projected, kind),
            kind_count(&live, kind),
            "{kind}"
        );
    }
    t.close().await;
}

/// A stored Cursor `delete` + `{"path":…}` renders as `Delete: /wt/out.txt`
/// on reload, as it did live, not as JSON.
#[tokio::test]
async fn reconstruct_feed_canonicalizes_a_cursor_delete() {
    let home = tempfile::tempdir().unwrap();
    let cursor: Arc<dyn recur_agent::Provider> = Arc::new(
        recur_provider_cursor::CursorProvider::new(recur_provider_cursor::CursorConfig {
            data_dir: home.path().to_path_buf(),
            home: Some(home.path().to_path_buf()),
            ..recur_provider_cursor::CursorConfig::default()
        }),
    );
    let t = TestEngine::with(TestOptions {
        registered: Some(vec![cursor]),
        ..TestOptions::default()
    });
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .create_workspace(&proj, "feed-delete", "")
        .await
        .unwrap();
    let sess = t
        .store
        .create_session(Session {
            workspace_id: ws.id.clone(),
            kind: AgentRole::Coder.as_str().into(),
            provider: PROVIDER_CURSOR.into(),
            config_json: r#"{"code":"CO","name":"Coder"}"#.into(),
            ..Session::default()
        })
        .unwrap();
    t.store
        .append_message(
            &sess.id,
            Message {
                role: "assistant".into(),
                provider: PROVIDER_CURSOR.into(),
                content: Some(vec![ContentBlock::tool_use(
                    "t1",
                    "c1",
                    "delete",
                    r#"{"path":"/wt/out.txt"}"#,
                )]),
                ..Message::default()
            },
        )
        .unwrap();
    let feed = t.engine.reconstruct_feed(&ws.id);
    let row = feed
        .iter()
        .find(|it| it.kind == FEED_KIND_TOOL)
        .unwrap_or_else(|| panic!("no tool row in {feed:?}"));
    assert!(!row.text.contains('{'), "reload dumped JSON: {}", row.text);
    assert!(
        row.text.contains("Delete") && row.text.contains("/wt/out.txt"),
        "{}",
        row.text
    );
    t.close().await;
}

#[test]
fn a_delete_step_is_an_edit() {
    assert_eq!(step_kind("Delete"), "edit");
}
