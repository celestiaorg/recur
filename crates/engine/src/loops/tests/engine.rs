//! Engine loop tests: the scripted end-to-end walk (engine commit, events,
//! durable feed, reopen), the git-identity refusal, governor queueing and the
//! caps drain, Pause/Resume/Stop, per-loop autonomy, and the env-gated real
//! Claude smoke.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use recur_domain::{AutonomyLevel, Group, WorkspaceState};
use recur_store::Store;
use tokio_util::sync::CancellationToken;

use crate::bus::EngineEvent;
use crate::loops::test_support::*;
use crate::testing::{TestEngine, add_project, git, git_out, init_repo, wait_for};

/// One Write tool call, then a success result.
fn fixture_lines(session: &str, file: &str, text: &str, cost: &str) -> Vec<String> {
    vec![
        format!(
            r#"{{"type":"system","subtype":"init","session_id":"{session}","model":"claude-sonnet-4-6","mcp_servers":[]}}"#
        ),
        format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"t1","name":"Write","input":{{"file_path":"{file}"}}}}]}}}}"#
        ),
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":false}]}}"#.into(),
        format!(
            r#"{{"type":"result","subtype":"success","session_id":"{session}","duration_ms":1000,"is_error":false,"num_turns":1,"result":"{text}","total_cost_usd":{cost}}}"#
        ),
    ]
}

fn feature_runner(session: &str) -> Arc<dyn recur_agent::Provider> {
    scripted_with_edits(
        fixture_lines(session, "feature.txt", "Implemented the feature.", "0.25"),
        &[("feature.txt", "added by the coder\n")],
    )
}

async fn start(t: &TestEngine, proj: &str, task: &str) -> String {
    t.engine
        .start_workspace(proj, task, "", AutonomyLevel::default())
        .await
        .unwrap()
}

fn saw_sequence(states: &[WorkspaceState], want: &[WorkspaceState]) -> bool {
    let mut i = 0;
    for s in states {
        if i < want.len() && *s == want[i] {
            i += 1;
        }
    }
    i == want.len()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_end_to_end_scripted() {
    let repo = init_repo();
    let t = TestEngine::new(Some(feature_runner("sess-e2e")));
    let proj = add_project(&t.store, &repo.path);
    let id = start(&t, &proj, "Add a feature file").await;
    assert!(!id.is_empty());
    wait_for(Duration::from_secs(5), "loop to reach pr_open", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::PrOpen)
    })
    .await;
    wait_for(Duration::from_secs(5), "walk to exit", || {
        t.engine.running_loop_by_id(&id).is_none()
    })
    .await;
    let l = t.store.workspace(&id).unwrap();
    assert_eq!(l.group, Group::Ready);
    assert!(l.diff.add > 0, "{:?}", l.diff);
    let wt = &l.worktree_path;
    assert!(!wt.is_empty());
    let log = git_out(wt, &["log", "--oneline"]).unwrap();
    assert!(log.contains("Add a feature file"), "{log}");
    let author = git_out(wt, &["log", "-1", "--format=%an <%ae>"]).unwrap();
    assert_eq!(author.trim(), "test <test@example.com>");
    assert!(Path::new(wt).join("feature.txt").exists());

    for name in [
        "workspace.updated",
        "workspace.event",
        "agent.step",
        "resource.updated",
    ] {
        assert!(t.events.has(name), "no {name} event");
    }
    let states: Vec<WorkspaceState> = t
        .events
        .by_name("workspace.updated")
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::WorkspaceUpdated(u) if u.workspace_id == id => Some(u.state),
            _ => None,
        })
        .collect();
    assert!(
        saw_sequence(
            &states,
            &[
                WorkspaceState::Planning,
                WorkspaceState::Coding,
                WorkspaceState::PrOpen
            ]
        ),
        "{states:?}"
    );
    assert!(
        !t.store
            .events_for_loop(&id)
            .unwrap()
            .unwrap_or_default()
            .is_empty(),
        "durable events persisted"
    );
    assert!(
        !t.engine.reconstruct_feed(&id).is_empty(),
        "reconstruct_feed returned nothing despite persisted events"
    );

    t.store.close().unwrap();
    let st2 = Arc::new(Store::open(&t.data_dir.join("recur.db")).unwrap());
    assert_eq!(st2.workspace(&id).unwrap().state, WorkspaceState::PrOpen);
    assert!(
        !st2.events_for_loop(&id)
            .unwrap()
            .unwrap_or_default()
            .is_empty()
    );
    let eng2 = t.reopen_on(Arc::clone(&st2), None);
    assert!(
        !eng2.reconstruct_feed(&id).is_empty(),
        "feed not reconstructable from disk after reopen"
    );
    st2.close().unwrap();
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_commit_requires_git_identity() {
    let repo = init_repo();
    // Repo-local empty identity masks any global one without touching the
    // process environment.
    git(&repo.path, &["config", "user.useConfigOnly", "true"]);
    git(&repo.path, &["config", "user.name", ""]);
    git(&repo.path, &["config", "user.email", ""]);
    let t = TestEngine::new(Some(feature_runner("sess-noid")));
    let proj = add_project(&t.store, &repo.path);
    let id = start(&t, &proj, "Add a feature file").await;
    wait_for(
        Duration::from_secs(5),
        "loop to fail without git identity",
        || {
            t.store
                .workspace(&id)
                .is_some_and(|l| l.state == WorkspaceState::Failed)
        },
    )
    .await;
    let l = t.store.workspace(&id).unwrap();
    assert!(
        l.last_error.contains("engine commit identity"),
        "{}",
        l.last_error
    );
    let log = git_out(&repo.path, &["log", "--oneline"]).unwrap();
    assert!(!log.contains("Add a feature file"), "{log}");
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_governor_queues_third_loop() {
    let repo = init_repo();
    let gate = CancellationToken::new();
    let t = TestEngine::new(Some(blocking_runner(gate.clone(), &[])));
    let mut set = t.store.settings();
    set.max_loops = 2;
    t.store.set_settings(set);
    t.engine.apply_governor_caps(2, 8).await;
    let proj = add_project(&t.store, &repo.path);
    start(&t, &proj, "task one").await;
    start(&t, &proj, "task two").await;
    wait_for(Duration::from_secs(3), "two loops running", || {
        t.engine.governor().loops_running() == 2
    })
    .await;
    let id3 = start(&t, &proj, "task three").await;
    assert_eq!(
        t.store.workspace(&id3).unwrap().state,
        WorkspaceState::Queued
    );
    gate.cancel();
    wait_for(Duration::from_secs(5), "loops 1 and 2 to finish", || {
        t.engine.governor().loops_running() == 0
    })
    .await;
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apply_governor_caps_drains_queued_loops() {
    let repo = init_repo();
    let gate = CancellationToken::new();
    let t = TestEngine::new(Some(blocking_runner(gate.clone(), &[])));
    t.engine.apply_governor_caps(1, 8).await;
    let proj = add_project(&t.store, &repo.path);
    start(&t, &proj, "loop A").await;
    wait_for(Duration::from_secs(3), "loop A running", || {
        t.engine.governor().loops_running() == 1
    })
    .await;
    let b = start(&t, &proj, "loop B").await;
    let c = start(&t, &proj, "loop C").await;
    for id in [&b, &c] {
        assert_eq!(
            t.store.workspace(id).unwrap().state,
            WorkspaceState::Queued,
            "{id}"
        );
    }
    t.engine.apply_governor_caps(3, 8).await;
    wait_for(Duration::from_secs(3), "queued loops to drain", || {
        t.engine.governor().loops_running() == 3
    })
    .await;
    gate.cancel();
    wait_for(Duration::from_secs(5), "all loops to finish", || {
        t.engine.governor().loops_running() == 0
    })
    .await;
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_stop_cancels_promptly() {
    let repo = init_repo();
    let t = TestEngine::new(Some(ctx_runner()));
    let proj = add_project(&t.store, &repo.path);
    let id = start(&t, &proj, "long task").await;
    wait_for(Duration::from_secs(3), "agent running", || {
        t.engine.governor().agents_running() == 1
    })
    .await;
    let began = Instant::now();
    t.engine.stop(&id).await;
    wait_for(Duration::from_secs(3), "loop to stop", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::Stopped)
    })
    .await;
    assert!(
        began.elapsed() < Duration::from_secs(2),
        "{:?}",
        began.elapsed()
    );
    wait_for(Duration::from_secs(2), "agent slot released", || {
        t.engine.governor().agents_running() == 0
    })
    .await;
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_pause_then_resume_completes() {
    let repo = init_repo();
    let gate = CancellationToken::new();
    let t = TestEngine::new(Some(blocking_runner(gate.clone(), &[("p.txt", "x\n")])));
    let proj = add_project(&t.store, &repo.path);
    let id = start(&t, &proj, "pausable").await;
    wait_for(Duration::from_secs(3), "agent running", || {
        t.engine.governor().agents_running() == 1
    })
    .await;
    t.engine.pause(&id);
    gate.cancel();
    wait_for(Duration::from_secs(4), "loop paused", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::Paused)
    })
    .await;
    t.engine.resume(&id).await;
    wait_for(Duration::from_secs(4), "loop resumes to pr_open", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::PrOpen)
    })
    .await;
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_workspace_stamps_autonomy() {
    let repo = init_repo();
    let t = TestEngine::new(Some(scripted_with_edits(
        fixture_lines("sess-autonomy", "feature.txt", "done", "0"),
        &[("feature.txt", "ok\n")],
    )));
    let proj = add_project(&t.store, &repo.path);
    let autonomy = |id: &str| t.store.workspace(id).unwrap().autonomy;
    let id_default = start(&t, &proj, "policy default").await;
    assert_eq!(autonomy(&id_default), AutonomyLevel::Balanced);
    let id_cautious = t
        .engine
        .start_workspace(&proj, "cautious override", "", AutonomyLevel::Cautious)
        .await
        .unwrap();
    assert_eq!(autonomy(&id_cautious), AutonomyLevel::Cautious);
    let mut p = t.store.policy();
    p.autonomy = AutonomyLevel::Aggressive;
    t.store.set_policy(p);
    let id_aggr = start(&t, &proj, "new policy default").await;
    assert_eq!(autonomy(&id_aggr), AutonomyLevel::Aggressive);
    assert_eq!(autonomy(&id_default), AutonomyLevel::Balanced);
    assert_eq!(autonomy(&id_cautious), AutonomyLevel::Cautious);
    t.close().await;
}

/// One real Coder turn through the production Claude provider. Gated on
/// `RECUR_E2E=1`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "real claude: RECUR_E2E=1 cargo test -- --ignored real_claude_smoke"]
async fn real_claude_smoke() {
    if std::env::var("RECUR_E2E").as_deref() != Ok("1") {
        eprintln!("set RECUR_E2E=1 to run the real claude smoke test");
        return;
    }
    let repo = init_repo();
    let t = TestEngine::new(Some(Arc::new(recur_provider_claude::ClaudeProvider::new(
        recur_provider_claude::ClaudeConfig::default(),
    ))));
    let proj = add_project(&t.store, &repo.path);
    let id = start(
        &t,
        &proj,
        "Create a file named HELLO.txt containing the word hi",
    )
    .await;
    wait_for(Duration::from_secs(5 * 60), "real loop to finish", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| matches!(l.state, WorkspaceState::PrOpen | WorkspaceState::Failed))
    })
    .await;
    let l = t.store.workspace(&id).unwrap();
    assert_eq!(l.state, WorkspaceState::PrOpen);
    let log = git_out(&l.worktree_path, &["log", "--oneline"]).unwrap_or_default();
    assert!(log.contains("HELLO.txt"), "{log}");
    t.close().await;
}
