//! Hardening tests that drive real walks: a panic inside a turn fails the loop
//! and frees its slot, queued loops drain, concurrent starts are clean, Resume
//! re-dispatches a not-running loop, Shutdown joins a live walk, and
//! `dispatch_mu` serializes drain and Resume storms (no double dispatch,
//! balanced slots). Recover, the not-running Pause/Stop rules and Shutdown over
//! fake walks are in `lifecycle::tests`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use recur_domain::{AutonomyLevel, Group, Workspace, WorkspaceState};
use recur_store::Store;
use tokio_util::sync::CancellationToken;

use crate::codes::PAIR_AGENT_CODE;
use crate::loops::test_support::*;
use crate::publish::terminal;
use crate::testing::{TestEngine, add_project, init_repo, wait_for};

async fn start(t: &TestEngine, proj: &str, task: &str) -> String {
    t.engine
        .start_workspace(proj, task, "", AutonomyLevel::default())
        .await
        .unwrap()
}

fn queued_loop(store: &Store, proj: &str, i: usize) -> String {
    let id = format!("LOOP-Q-{}-{i}", (b'A' + (i % 26) as u8) as char);
    store
        .add_workspace(Workspace {
            id: id.clone(),
            project_id: proj.into(),
            group: Group::Active,
            goal: format!("queued work {i}"),
            state: WorkspaceState::Queued,
            status_label: "Queued".into(),
            phase: "waiting for a slot".into(),
            ..Workspace::default()
        })
        .unwrap();
    id
}

fn paused_loop(store: &Store, proj: &str, id: &str, goal: &str) {
    store
        .add_workspace(Workspace {
            id: id.into(),
            project_id: proj.into(),
            group: Group::Needs,
            goal: goal.into(),
            state: WorkspaceState::Paused,
            status_label: "Paused".into(),
            phase: "interrupted".into(),
            ..Workspace::default()
        })
        .unwrap();
}

/// A promote hook that holds the `dispatch_mu` section open for `d` and
/// records the most callers seen inside it at once.
fn window_widener(
    d: Duration,
) -> (
    impl Fn() + Send + Sync + 'static,
    Arc<Mutex<(usize, usize)>>,
) {
    let state = Arc::new(Mutex::new((0usize, 0usize)));
    let s = Arc::clone(&state);
    let hook = move || {
        {
            let mut g = s.lock();
            g.0 += 1;
            g.1 = g.1.max(g.0);
        }
        std::thread::sleep(d);
        s.lock().0 -= 1;
    };
    (hook, state)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_loop_panic_recovers() {
    let repo = init_repo();
    let panicked = Arc::new(AtomicBool::new(false));
    let p = Arc::clone(&panicked);
    let t = TestEngine::new(Some(AsyncProvider::wrap(move |req, _cx| {
        let p = Arc::clone(&p);
        async move {
            if !p.swap(true, Ordering::SeqCst) {
                panic!("boom in the runner");
            }
            apply_edits(&req.worktree, &[("ok.txt", "fine\n")])
                .map_err(|e| recur_agent::ProviderError::failed(e.to_string()))?;
            Ok(success(&req.agent_id, "done"))
        }
    })));
    let proj = add_project(&t.store, &repo.path);
    let id = start(&t, &proj, "this loop panics").await;
    wait_for(Duration::from_secs(5), "panicking loop to fail", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::Failed)
    })
    .await;
    wait_for(Duration::from_secs(3), "slot released after panic", || {
        t.engine.governor().loops_running() == 0
    })
    .await;
    assert!(t.engine.running_loop_by_id(&id).is_none());

    let id2 = start(&t, &proj, "this loop is healthy").await;
    wait_for(Duration::from_secs(5), "healthy loop after panic", || {
        t.store
            .workspace(&id2)
            .is_some_and(|l| l.state == WorkspaceState::PrOpen)
    })
    .await;
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_queued_loop_drains() {
    let repo = init_repo();
    let gate = CancellationToken::new();
    let t = TestEngine::new(Some(blocking_runner(gate.clone(), &[("drain.txt", "x\n")])));
    t.engine.apply_governor_caps(1, 8).await;
    let proj = add_project(&t.store, &repo.path);
    let a = start(&t, &proj, "loop A").await;
    wait_for(Duration::from_secs(3), "loop A running", || {
        t.engine.governor().loops_running() == 1
    })
    .await;
    let b = start(&t, &proj, "loop B").await;
    assert_eq!(t.store.workspace(&b).unwrap().state, WorkspaceState::Queued);
    gate.cancel();
    for (id, what) in [(&a, "loop A to finish"), (&b, "queued B to run")] {
        wait_for(Duration::from_secs(5), what, || {
            t.store
                .workspace(id)
                .is_some_and(|l| l.state == WorkspaceState::PrOpen)
        })
        .await;
    }
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_concurrent_start_workspace_race_clean() {
    let repo = init_repo();
    let t = TestEngine::new(Some(scripted_with_edits(
        Vec::new(),
        &[("c.txt", "concurrent\n")],
    )));
    t.engine.apply_governor_caps(0, 0).await;
    let proj = add_project(&t.store, &repo.path);
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let (eng, proj) = (Arc::clone(&t.engine), proj.clone());
        tasks.push(tokio::spawn(async move {
            eng.start_workspace(&proj, "concurrent task", "", AutonomyLevel::default())
                .await
        }));
    }
    let mut ids = Vec::new();
    for task in tasks {
        ids.push(task.await.unwrap().unwrap());
    }
    for id in &ids {
        wait_for(Duration::from_secs(10), "concurrent loop to finish", || {
            t.store
                .workspace(id)
                .is_some_and(|l| l.state == WorkspaceState::PrOpen)
        })
        .await;
    }
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_resume_not_running_redispatches() {
    let repo = init_repo();
    let t = TestEngine::new(Some(scripted_with_edits(
        Vec::new(),
        &[("resume.txt", "resumed\n")],
    )));
    let proj = add_project(&t.store, &repo.path);
    paused_loop(&t.store, &proj, "LOOP-PAUSED", "resume me");
    t.engine.resume("LOOP-PAUSED").await;
    wait_for(
        Duration::from_secs(5),
        "resumed loop to reach pr_open",
        || {
            t.store
                .workspace("LOOP-PAUSED")
                .is_some_and(|l| l.state == WorkspaceState::PrOpen)
        },
    )
    .await;
    let l = t.store.workspace("LOOP-PAUSED").unwrap();
    assert!(
        !l.worktree_path.is_empty(),
        "worktree created on re-dispatch"
    );
    assert!(l.diff.add > 0, "{:?}", l.diff);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_shutdown_joins() {
    let repo = init_repo();
    let t = TestEngine::new(Some(ctx_runner()));
    let proj = add_project(&t.store, &repo.path);
    let id = start(&t, &proj, "long running").await;
    wait_for(Duration::from_secs(3), "agent running", || {
        t.engine.governor().agents_running() == 1
    })
    .await;
    tokio::time::timeout(Duration::from_secs(6), t.engine.shutdown())
        .await
        .expect("Shutdown returns within its bound");
    assert_eq!(t.engine.governor().loops_running(), 0);
    assert!(t.engine.running_loop_by_id(&id).is_none());
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn engine_concurrent_drain_no_double_dispatch() {
    let repo = init_repo();
    let gate = CancellationToken::new();
    let runner = Arc::new(CountingRunner {
        gate: Some(gate.clone()),
        ..CountingRunner::default()
    });
    let t = TestEngine::new(Some(runner.provider()));
    t.engine.apply_governor_caps(8, 64).await;
    let (hook, window) = window_widener(Duration::from_millis(40));
    t.engine.set_promote_hook(hook);
    let proj = add_project(&t.store, &repo.path);
    let ids: Vec<String> = (0..12).map(|i| queued_loop(&t.store, &proj, i)).collect();

    const HAMMER: usize = 48;
    let barrier = Arc::new(tokio::sync::Barrier::new(HAMMER));
    let mut tasks = Vec::new();
    for _ in 0..HAMMER {
        let (eng, b) = (Arc::clone(&t.engine), Arc::clone(&barrier));
        tasks.push(tokio::spawn(async move {
            b.wait().await;
            eng.drain_queued().await;
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    wait_for(Duration::from_secs(5), "first wave occupies slots", || {
        t.engine.governor().loops_running() >= 1
    })
    .await;
    gate.cancel();
    for id in &ids {
        wait_for(Duration::from_secs(20), "queued loop to finish", || {
            t.store
                .workspace(id)
                .is_some_and(|l| l.state == WorkspaceState::PrOpen)
        })
        .await;
    }
    assert_eq!(window.lock().1, 1, "the promotion section is serialized");
    let (worst, n) = runner.max_count();
    assert!(n <= 1, "double dispatch: {worst} ran {n} times");
    for id in &ids {
        assert_eq!(
            runner.counts.lock().get(id).copied().unwrap_or(0),
            1,
            "{id} dispatched once"
        );
    }
    wait_for(Duration::from_secs(5), "all slots released", || {
        t.engine.governor().loops_running() == 0
    })
    .await;
    for id in &ids {
        assert!(
            t.engine.running_loop_by_id(id).is_none(),
            "{id} left running"
        );
    }
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn engine_concurrent_resume_single_dispatch() {
    let repo = init_repo();
    let runner = Arc::new(CountingRunner::default());
    let t = TestEngine::new(Some(runner.provider()));
    let (hook, window) = window_widener(Duration::from_millis(40));
    t.engine.set_promote_hook(hook);
    let proj = add_project(&t.store, &repo.path);
    paused_loop(
        &t.store,
        &proj,
        "LOOP-RESUME-RACE",
        "resume me concurrently",
    );

    const CALLERS: usize = 32;
    let barrier = Arc::new(tokio::sync::Barrier::new(CALLERS));
    let mut tasks = Vec::new();
    for _ in 0..CALLERS {
        let (eng, b) = (Arc::clone(&t.engine), Arc::clone(&barrier));
        tasks.push(tokio::spawn(async move {
            b.wait().await;
            eng.resume("LOOP-RESUME-RACE").await;
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(window.lock().1, 1, "Resume re-dispatch is serialized");
    wait_for(Duration::from_secs(5), "resumed loop to finish", || {
        t.store
            .workspace("LOOP-RESUME-RACE")
            .is_some_and(|l| l.state == WorkspaceState::PrOpen)
    })
    .await;
    assert_eq!(
        runner
            .counts
            .lock()
            .get("LOOP-RESUME-RACE")
            .copied()
            .unwrap_or(0),
        1
    );
    wait_for(Duration::from_secs(5), "slot released", || {
        t.engine.governor().loops_running() == 0
    })
    .await;
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_resume_does_not_resurrect_stopped() {
    let repo = init_repo();
    for iter in 0..20 {
        let runner = Arc::new(CountingRunner::default());
        let t = TestEngine::new(Some(runner.provider()));
        let proj = add_project(&t.store, &repo.path);
        t.store
            .add_workspace(Workspace {
                id: "LOOP-STOP-RESUME".into(),
                project_id: proj,
                goal: "race".into(),
                state: WorkspaceState::Paused,
                group: Group::Needs,
                phase: "interrupted".into(),
                ..Workspace::default()
            })
            .unwrap();
        let (e1, e2) = (Arc::clone(&t.engine), Arc::clone(&t.engine));
        let stop = tokio::spawn(async move { e1.stop("LOOP-STOP-RESUME").await });
        let resume = tokio::spawn(async move { e2.resume("LOOP-STOP-RESUME").await });
        stop.await.unwrap();
        resume.await.unwrap();
        wait_for(
            Duration::from_secs(8),
            "loop settles after the race",
            || {
                t.store
                    .workspace("LOOP-STOP-RESUME")
                    .is_some_and(|l| terminal(&l.state))
            },
        )
        .await;
        let (_, n) = runner.max_count();
        assert!(n <= 1, "iter {iter}: double dispatch ({n})");
        wait_for(
            Duration::from_secs(5),
            "slot released after the race",
            || t.engine.governor().loops_running() == 0,
        )
        .await;
        if t.store.workspace("LOOP-STOP-RESUME").unwrap().state == WorkspaceState::Stopped {
            assert!(
                t.engine.running_loop_by_id("LOOP-STOP-RESUME").is_none(),
                "iter {iter}: stopped loop resurrected"
            );
        }
        t.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drain_queued_promotes_next_after_closed_owner() {
    let repo = init_repo();
    let t = TestEngine::new(Some(ConvergeRunner::new(&[0, 0]).provider()));
    t.engine.apply_governor_caps(1, 64).await;
    let proj = add_project(&t.store, &repo.path);
    let b = queued_loop(&t.store, &proj, 0);
    let c = queued_loop(&t.store, &proj, 1);
    t.store.update_workspace(&b, |w| {
        w.run_started = true;
        w.loop_chat_code = PAIR_AGENT_CODE.into();
    });
    t.engine.ensure_pair_agent(&b, PAIR_AGENT_CODE, "");
    let (store, id) = (Arc::clone(&t.store), b.clone());
    t.engine
        .set_promote_hook(move || close_tab(&store, &id, PAIR_AGENT_CODE));
    t.engine.drain_queued().await;
    assert_ne!(
        t.store.workspace(&b).unwrap().state,
        WorkspaceState::Queued,
        "the closed owner leaves the queue"
    );
    wait_for(Duration::from_secs(10), "C leaves the queue", || {
        t.store
            .workspace(&c)
            .is_some_and(|l| l.state != WorkspaceState::Queued)
    })
    .await;
    t.close().await;
}
