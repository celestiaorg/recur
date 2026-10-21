//! The pool cases of Go `engine/provider_test.go`, against a fake bridge
//! installed in a temp data dir.

use std::time::Duration;

use recur_domain::Settings;

use super::*;
use crate::testserver::{wait_for, write_fake_cursor_install};

struct Fixture {
    data: tempfile::TempDir,
    _home: tempfile::TempDir,
    pool: Arc<Pool>,
}

fn fixture(install: bool) -> Fixture {
    let data = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    if install {
        write_fake_cursor_install(data.path());
    }
    let pool = Arc::new(Pool::new(
        data.path().to_path_buf(),
        data.path().join("logs"),
        Some(home.path().to_path_buf()),
    ));
    Fixture {
        data,
        _home: home,
        pool,
    }
}

fn keyed() -> Settings {
    Settings {
        cursor_api_key: "sk-test".into(),
        ..Default::default()
    }
}

async fn ensure(f: &Fixture, set: &Settings, wt: &Path) -> Result<Reservation, PoolError> {
    f.pool.ensure_runner(&CancellationToken::new(), set, wt).await
}

fn runner(busy: bool) -> Arc<CursorRunner> {
    let r = CursorRunner::new(Bridge::bound("http://127.0.0.1:9", "", None), "");
    if busy {
        r.in_use.store(1, Ordering::SeqCst);
    }
    Arc::new(r)
}

fn entry(r: Arc<CursorRunner>, age: u64) -> Entry {
    Entry {
        bridge: None,
        runner: Some(r),
        last_used: Instant::now() + Duration::from_secs(age),
    }
}

#[tokio::test]
async fn without_key_fails() {
    let f = fixture(true);
    let wt = tempfile::tempdir().unwrap();
    let err = ensure(&f, &Settings::default(), wt.path()).await.err().unwrap();
    assert!(err.0.contains("Connect"), "{err:?}");
}

#[tokio::test]
async fn disabled_fails() {
    let f = fixture(true);
    let wt = tempfile::tempdir().unwrap();
    let set = Settings {
        disabled_providers: vec![PROVIDER_CURSOR.into()],
        ..keyed()
    };
    let err = ensure(&f, &set, wt.path()).await.err().unwrap();
    assert_eq!(err.0, "Cursor is turned off in Settings → Providers");
}

#[tokio::test]
async fn missing_bridge_fails() {
    let f = fixture(false);
    let wt = tempfile::tempdir().unwrap();
    let err = ensure(&f, &keyed(), wt.path()).await.err().unwrap();
    assert!(err.0.starts_with("cursor bridge not found at "), "{err:?}");
}

#[tokio::test]
async fn rejects_unpatchable_sdk() {
    let f = fixture(true);
    std::fs::remove_dir_all(f.data.path().join("cursor-bridge/node_modules")).unwrap();
    let wt = tempfile::tempdir().unwrap();
    let err = ensure(&f, &keyed(), wt.path()).await.err().unwrap();
    assert!(err.0.contains("reinstall"), "{err:?}");
}

#[tokio::test]
async fn requires_worktree() {
    let f = fixture(true);
    for wt in ["", "/", "  "] {
        let err = ensure(&f, &keyed(), Path::new(wt)).await.err().unwrap();
        assert_eq!(err.0, "cursor bridge: worktree path is required (refusing to start at /)", "{wt:?}");
    }
    let err = ensure(&f, &keyed(), &f.data.path().join("missing")).await.err().unwrap();
    assert!(err.0.ends_with("is unavailable"), "{err:?}");
}

/// The self-heal guarantee: a dead Node bridge is replaced, never handed out
/// with a client dialing its dead port.
#[tokio::test]
async fn restarts_dead_bridge() {
    let f = fixture(true);
    let wt = tempfile::tempdir().unwrap();
    let r1 = ensure(&f, &keyed(), wt.path()).await.unwrap();
    let b1 = r1.runner.bridge.clone();
    drop(r1);
    assert!(b1.alive());
    let old_pid = b1.discovery().pid;
    assert_ne!(old_pid, 0, "the fake bridge advertises its pid");
    let _ = rustix::process::kill_process(rustix::process::Pid::from_raw(old_pid as i32).unwrap(), rustix::process::Signal::KILL);
    wait_for(Duration::from_secs(5), "bridge reaped", || !b1.alive()).await;

    let r2 = ensure(&f, &keyed(), wt.path()).await.unwrap();
    assert!(r2.runner.bridge.alive());
    assert_ne!(r2.runner.bridge.discovery().pid, old_pid, "reused the dead process");
    assert!(!Arc::ptr_eq(&r2.runner.bridge, &b1));
    drop(r2);
    f.pool.close().await;
}

/// Two worktrees get two bridges, each bound to its worktree; the same
/// worktree reuses its warm process.
#[tokio::test]
async fn pools_bridges_by_worktree() {
    let f = fixture(true);
    let (wt1, wt2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let r1 = ensure(&f, &keyed(), wt1.path()).await.unwrap();
    let r2 = ensure(&f, &keyed(), wt2.path()).await.unwrap();
    assert!(!Arc::ptr_eq(&r1.runner.bridge, &r2.runner.bridge));
    assert_eq!(r1.runner.bridge.workspace(), wt1.path());
    assert_eq!(r2.runner.bridge.workspace(), wt2.path());
    assert_eq!(r1.runner.bridge.discovery().workspace_ref, wt1.path().to_string_lossy());
    let r1b = ensure(&f, &keyed(), wt1.path()).await.unwrap();
    assert!(Arc::ptr_eq(&r1b.runner.bridge, &r1.runner.bridge));
    assert_eq!(r1.runner.in_use.load(Ordering::SeqCst), 2);
    drop((r1, r1b, r2));
    f.pool.close().await;
}

#[tokio::test]
async fn key_change_tears_the_pool_down() {
    let f = fixture(true);
    let wt = tempfile::tempdir().unwrap();
    let r1 = ensure(&f, &keyed(), wt.path()).await.unwrap();
    let b1 = r1.runner.bridge.clone();
    drop(r1);
    let rotated = Settings {
        cursor_api_key: "sk-new".into(),
        cursor_strict_config: true,
        ..Default::default()
    };
    let r2 = ensure(&f, &rotated, wt.path()).await.unwrap();
    assert!(!b1.alive(), "the old key's bridge must be closed");
    assert!(!Arc::ptr_eq(&r2.runner.bridge, &b1));
    assert!(r2.runner.strict_config.load(Ordering::SeqCst));
    drop(r2);
    f.pool.close().await;
}

#[tokio::test]
async fn evict_skips_in_use_bridges() {
    let mut p = PoolState::default();
    p.entries.insert("/busy".into(), entry(runner(true), 1));
    p.entries.insert("/idle".into(), entry(runner(false), 2));
    evict_locked(&mut p, 1).await;
    assert!(p.entries.contains_key(Path::new("/busy")));
    assert!(!p.entries.contains_key(Path::new("/idle")));
}

#[tokio::test]
async fn evict_keeps_all_busy_over_cap() {
    let mut p = PoolState::default();
    p.entries.insert("/a".into(), entry(runner(true), 1));
    p.entries.insert("/b".into(), entry(runner(true), 2));
    evict_locked(&mut p, 1).await;
    assert_eq!(p.entries.len(), 2);
}

#[tokio::test]
async fn reserved_handout_survives_cold_start_eviction() {
    let fill = |handed: Arc<CursorRunner>| {
        let mut p = PoolState::default();
        p.entries.insert("/handed".into(), entry(handed, 0));
        for i in 1..MAX_CURSOR_BRIDGES {
            p.entries.insert(PathBuf::from(format!("/{i}")), entry(runner(true), i as u64));
        }
        p
    };
    let mut p = fill(runner(true));
    evict_locked(&mut p, MAX_CURSOR_BRIDGES - 1).await;
    assert!(p.entries.contains_key(Path::new("/handed")), "evicted the reserved handout");
    let mut p2 = fill(runner(false));
    evict_locked(&mut p2, MAX_CURSOR_BRIDGES - 1).await;
    assert!(!p2.entries.contains_key(Path::new("/handed")), "an unreserved handout is the idle victim");
}

#[tokio::test]
async fn ensure_reserves_against_eviction() {
    let f = fixture(true);
    let wt = tempfile::tempdir().unwrap();
    let r = ensure(&f, &keyed(), wt.path()).await.unwrap();
    assert_eq!(r.runner.in_use.load(Ordering::SeqCst), 1, "reserved before the lock drops");
    {
        let mut p = f.pool.state.lock().await;
        for i in 0..MAX_CURSOR_BRIDGES - 1 {
            p.entries.insert(PathBuf::from(format!("/busy-{i}")), entry(runner(true), i as u64 + 1));
        }
    }
    let wt2 = tempfile::tempdir().unwrap();
    let r2 = ensure(&f, &keyed(), wt2.path()).await.unwrap();
    assert!(r.runner.bridge.alive(), "the reserved runner was evicted by a later cold start");
    assert!(f.pool.state.lock().await.entries.contains_key(wt.path()));
    drop((r, r2));
    f.pool.close().await;
}

#[tokio::test]
async fn release_trims_overflow_when_idle() {
    let pool = Arc::new(Pool::new(PathBuf::new(), PathBuf::new(), None));
    let n = MAX_CURSOR_BRIDGES + 4;
    let mut handouts = Vec::new();
    {
        let mut p = pool.state.lock().await;
        for i in 0..n {
            let r = runner(false);
            p.entries.insert(PathBuf::from(format!("/{i}")), entry(r.clone(), i as u64 + 1));
            handouts.push(Reservation::new(pool.clone(), r));
        }
    }
    let first = handouts.remove(0);
    drop(first);
    wait_for(Duration::from_secs(2), "idle overflow trimmed", || {
        pool.state.try_lock().is_ok_and(|p| p.entries.len() == n - 1)
    })
    .await;
    drop(handouts);
    wait_for(Duration::from_secs(2), "trim to the cap", || {
        pool.state.try_lock().is_ok_and(|p| p.entries.len() <= MAX_CURSOR_BRIDGES)
    })
    .await;
}

#[test]
fn release_counts_down_once() {
    let r = runner(false);
    r.in_use.store(2, Ordering::SeqCst);
    assert!(!release(&r));
    assert!(release(&r), "the last handout");
    assert!(!release(&r), "never below zero");
    assert_eq!(r.in_use.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn close_bridges_defers_while_busy() {
    let pool = Pool::new(PathBuf::new(), PathBuf::new(), None);
    pool.state.lock().await.entries.insert("/tmp/wt".into(), entry(runner(true), 0));
    pool.close_bridges().await;
    {
        let p = pool.state.lock().await;
        assert!(p.entries.contains_key(Path::new("/tmp/wt")), "dropped the pool mid-turn");
        assert!(p.close_when_idle);
    }
    pool.close_if_idle_requested(true).await;
    let p = pool.state.lock().await;
    assert!(p.entries.is_empty() && !p.close_when_idle);
}

#[tokio::test]
async fn close_bridges_immediate_when_idle() {
    let pool = Pool::new(PathBuf::new(), PathBuf::new(), None);
    pool.state.lock().await.entries.insert("/tmp/wt".into(), entry(runner(false), 0));
    pool.close_bridges().await;
    assert!(pool.state.lock().await.entries.is_empty());
}

#[tokio::test]
async fn cancel_deferred_close() {
    let pool = Pool::new(PathBuf::new(), PathBuf::new(), None);
    pool.state.lock().await.entries.insert("/tmp/wt".into(), entry(runner(true), 0));
    pool.close_bridges().await;
    pool.cancel_close_when_idle().await;
    pool.close_if_idle_requested(true).await;
    assert!(pool.state.lock().await.entries.contains_key(Path::new("/tmp/wt")));
    pool.close_if_idle_requested(false).await;
}

#[tokio::test]
async fn invalidate_drops_one_or_all() {
    let pool = Pool::new(PathBuf::new(), PathBuf::new(), None);
    let wt = tempfile::tempdir().unwrap();
    {
        let mut p = pool.state.lock().await;
        p.entries.insert(wt.path().to_path_buf(), entry(runner(false), 0));
        p.entries.insert("/gone/already".into(), entry(runner(false), 0));
        p.entries.insert("/other".into(), entry(runner(false), 0));
    }
    pool.invalidate(wt.path()).await;
    pool.invalidate(Path::new("/gone/./already")).await;
    assert_eq!(pool.state.lock().await.entries.len(), 1);
    pool.invalidate(Path::new("")).await;
    assert!(pool.state.lock().await.entries.is_empty());
}

#[test]
fn worktree_log_key_is_stable() {
    assert_eq!(worktree_log_key(Path::new("/tmp/wt")), "ced5e8d7d14397e6");
    assert_eq!(worktree_log_key(Path::new("/tmp/wt")), worktree_log_key(Path::new("/tmp/wt")));
    assert_ne!(worktree_log_key(Path::new("/tmp/wt")), worktree_log_key(Path::new("/tmp/wt2")));
}
