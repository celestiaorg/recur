//! Governor tests.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::Governor;

#[test]
fn loop_cap_queues_third() {
    let g = Governor::new(2, 8);
    assert!(g.try_acquire_loop(), "first loop slot should be available");
    assert!(g.try_acquire_loop(), "second loop slot should be available");
    assert!(
        !g.try_acquire_loop(),
        "third loop slot must NOT be available (cap=2)"
    );
    assert_eq!(g.loops_running(), 2);
    g.release_loop();
    assert!(g.try_acquire_loop(), "a slot should be free after release");
}

#[tokio::test]
async fn agent_cap_blocks_then_releases() {
    let g = Arc::new(Governor::new(4, 1));
    let done = CancellationToken::new();
    assert!(
        g.acquire_agent(&done).await,
        "first agent slot should acquire"
    );
    assert_eq!(g.agents_running(), 1);

    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let g2 = Arc::clone(&g);
    let d2 = done.clone();
    tokio::spawn(async move {
        if g2.acquire_agent(&d2).await {
            g2.release_agent();
        }
        let _ = tx.send(());
    });

    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut rx)
            .await
            .is_err(),
        "second agent acquire should block while cap=1 is full"
    );
    g.release_agent();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), rx)
            .await
            .is_ok(),
        "second agent acquire did not proceed after release"
    );
}

#[tokio::test]
async fn agent_acquire_cancels() {
    let g = Governor::new(4, 1);
    let done = CancellationToken::new();
    assert!(g.acquire_agent(&done).await, "first acquire should succeed");
    done.cancel();
    assert!(
        !g.acquire_agent(&done).await,
        "acquire should fail when done is closed and no slot is free"
    );
    g.release_agent();
}

#[tokio::test]
async fn agent_acquire_wakes_on_cancel() {
    let g = Arc::new(Governor::new(4, 1));
    let done = CancellationToken::new();
    assert!(g.acquire_agent(&CancellationToken::new()).await);
    let g2 = Arc::clone(&g);
    let d2 = done.clone();
    let waiter = tokio::spawn(async move { g2.acquire_agent(&d2).await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(g.agents_waiting(), 1);
    done.cancel();
    let got = tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("waiter must wake on cancel")
        .expect("join");
    assert!(!got);
    assert_eq!(g.agents_waiting(), 0);
}

#[test]
fn set_caps_raises_loop_cap() {
    let g = Governor::new(1, 8);
    assert!(g.try_acquire_loop(), "first slot");
    assert!(!g.try_acquire_loop(), "cap=1 should be full");
    g.set_caps(2, 8);
    assert!(
        g.try_acquire_loop(),
        "raising the loop cap must free a slot without a restart"
    );
}

#[tokio::test]
async fn set_caps_wakes_agent() {
    let g = Arc::new(Governor::new(4, 1));
    let done = CancellationToken::new();
    assert!(g.acquire_agent(&done).await, "first agent slot");

    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let g2 = Arc::clone(&g);
    let d2 = done.clone();
    tokio::spawn(async move {
        let ok = g2.acquire_agent(&d2).await;
        if ok {
            g2.release_agent();
        }
        let _ = tx.send(ok);
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut rx)
            .await
            .is_err(),
        "second acquire should block at cap=1"
    );
    g.set_caps(4, 2);
    let ok = tokio::time::timeout(Duration::from_secs(1), rx)
        .await
        .expect("waiter did not proceed after set_caps")
        .expect("sender");
    assert!(ok, "raising the agent cap should wake the waiter");
    g.release_agent();
}

#[test]
fn set_caps_lower_does_not_preempt() {
    let g = Governor::new(2, 8);
    assert!(g.try_acquire_loop() && g.try_acquire_loop());
    g.set_caps(1, 8);
    assert!(
        !g.try_acquire_loop(),
        "lowering the cap must not free a slot while two loops are in flight"
    );
    assert_eq!(g.loops_running(), 2, "in-flight work is not preempted");
    g.release_loop();
    assert!(
        !g.try_acquire_loop(),
        "still at cap 1 with one loop running"
    );
    g.release_loop();
    assert!(
        g.try_acquire_loop(),
        "a slot should free once in-flight work drops under the new cap"
    );
}

#[test]
fn unlimited_caps_and_free_loops() {
    let g = Governor::new(0, 0);
    for _ in 0..(recur_domain::DEFAULT_MAX_LOOPS + 1) {
        assert!(g.try_acquire_loop());
    }
    assert_eq!(g.free_loops(), 1 << 20);
    let g = Governor::new(3, 1);
    assert!(g.try_acquire_loop());
    assert_eq!(g.free_loops(), 2);
}
