//! Port of `fileindex_test.go`.

use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::{Notify, mpsc};

use super::*;
use crate::testutil::{tmp, write};

const NO_RG: &str = "/nonexistent/rg-binary";

fn ready_signal() -> (Publish, mpsc::UnboundedReceiver<String>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let publish: Publish = Arc::new(move |ws: &str, snap: Snapshot| {
        if snap.ready && !snap.building {
            let _ = tx.send(ws.to_string());
        }
    });
    (publish, rx)
}

async fn wait_for(rx: &mut mpsc::UnboundedReceiver<String>, ws: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(got)) if got == ws => return,
            Ok(Some(_)) => {}
            _ => panic!("timeout waiting for {ws} rebuild"),
        }
    }
}

async fn eventually(mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

fn root_str(d: &tempfile::TempDir) -> String {
    d.path().to_str().unwrap().to_string()
}

#[test]
fn file_index_upsert_remove() {
    let idx = FileIndex::new("/r");
    idx.upsert("validators/decode-hyperlane-message.sh");
    idx.upsert("validators/hyperlane_validator2.sh");
    idx.upsert("README.md");
    let snap = idx.snapshot();
    assert_eq!(snap.paths.len(), 3);
    assert!(snap.bytes > 0);

    idx.remove("README.md");
    let snap = idx.snapshot();
    assert_eq!(snap.paths.len(), 2);
    assert!(
        snap.paths
            .contains(&"validators/hyperlane_validator2.sh".into())
    );
    assert!(
        snap.paths
            .contains(&"validators/decode-hyperlane-message.sh".into())
    );
    assert_eq!(snap.paths.len(), snap.basenames.len());

    let before = idx.snapshot().bytes;
    idx.upsert("./validators/hyperlane_validator2.sh");
    assert_eq!(idx.snapshot().paths.len(), 2, "upsert of a known path");
    assert_eq!(idx.snapshot().bytes, before);
    idx.upsert("node_modules/x.js");
    idx.upsert("img.png");
    idx.upsert("../out.txt");
    assert_eq!(idx.snapshot().paths.len(), 2, "skipped paths");
}

#[test]
fn file_index_apply_fs_change() {
    let root = tmp();
    write(
        root.path(),
        "validators/hyperlane_validator2.sh",
        "#!/bin/sh\n",
    );
    let idx = FileIndex::new(&root_str(&root));
    idx.apply_fs_change(&root_str(&root), "validators/hyperlane_validator2.sh");
    assert_eq!(idx.snapshot().paths, ["validators/hyperlane_validator2.sh"]);
    fs_remove(root.path().join("validators/hyperlane_validator2.sh"));
    idx.apply_fs_change(&root_str(&root), "validators/hyperlane_validator2.sh");
    assert!(idx.snapshot().paths.is_empty());
}

fn fs_remove(p: std::path::PathBuf) {
    std::fs::remove_file(p).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn hub_rebuild_walk_fallback_includes_validators() {
    let root = tmp();
    write(root.path(), "validators/decode-hyperlane-message.sh", "x");
    write(root.path(), "validators/hyperlane_validator2.sh", "x");
    write(root.path(), "hypothesis.txt", "x");
    let (publish, mut rx) = ready_signal();
    let data = tmp();
    let hub = IndexHub::new(&root_str(&data), Some(publish));
    hub.set_rg_path(NO_RG);
    hub.refresh("ws1", &root_str(&root));
    wait_for(&mut rx, "ws1").await;
    let paths = hub.ensure("ws1", &root_str(&root)).paths;
    for want in [
        "validators/hyperlane_validator2.sh",
        "validators/decode-hyperlane-message.sh",
        "hypothesis.txt",
    ] {
        assert!(
            paths.iter().any(|p| p == want),
            "paths = {paths:?}, missing {want}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hub_apply_changes_marks_dirty_instead_of_rebuild() {
    let root = tmp();
    write(root.path(), "a.txt", "x");
    let data = tmp();
    let hub = IndexHub::new(&root_str(&data), None);
    hub.set_rg_path(NO_RG);
    let (publish, mut rx) = ready_signal();
    hub.set_publish(Some(publish));
    hub.refresh("ws1", &root_str(&root));
    wait_for(&mut rx, "ws1").await;

    hub.apply_changes("ws1", &root_str(&root), &[], true);
    let snap = hub.index("ws1").unwrap().snapshot();
    assert!(snap.dirty, "rescan should mark dirty");
    assert!(!snap.building, "rescan must not start a rebuild");

    // Cold workspace: file-system noise must not create an index.
    hub.apply_changes("ws-cold", &root_str(&root), &["a.txt".into()], false);
    assert_eq!(hub.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn hub_refresh_if_dirty_skips_clean_and_cold() {
    let root = tmp();
    write(root.path(), "a.txt", "x");
    let data = tmp();
    let hub = IndexHub::new(&root_str(&data), None);
    hub.set_rg_path(NO_RG);
    let (publish, mut rx) = ready_signal();
    hub.set_publish(Some(publish));
    hub.refresh("ws1", &root_str(&root));
    wait_for(&mut rx, "ws1").await;

    let builds = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&builds);
    hub.set_publish(Some(Arc::new(move |_: &str, snap: Snapshot| {
        if snap.ready && !snap.building {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    })));
    hub.refresh_if_dirty("ws1", &root_str(&root));
    assert_eq!(
        builds.load(Ordering::SeqCst),
        0,
        "clean refresh_if_dirty rebuilt"
    );
    let cold = hub.refresh_if_dirty("ws-cold", &root_str(&root));
    assert!(!cold.ready && hub.len() == 1, "cold should no-op: {cold:?}");

    hub.mark_dirty("ws1");
    hub.refresh_if_dirty("ws1", &root_str(&root));
    let idx = hub.index("ws1").unwrap();
    assert!(
        eventually(|| {
            let s = idx.snapshot();
            !s.dirty && s.ready
        })
        .await,
        "dirty refresh_if_dirty did not clear dirty"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hub_byte_budget_evicts_lru_not_pinned() {
    let data = tmp();
    let hub = IndexHub::new(&root_str(&data), None);
    hub.set_rg_path(NO_RG);
    let mut roots = Vec::new();
    for id in ["ws-old", "ws-pin"] {
        let root = tmp();
        for i in 0..40u8 {
            write(
                root.path(),
                &format!(
                    "file-with-a-fairly-long-name-{}.txt",
                    (b'a' + i % 26) as char
                ),
                "x",
            );
        }
        let (publish, mut rx) = ready_signal();
        hub.set_publish(Some(publish));
        hub.refresh(id, &root_str(&root));
        wait_for(&mut rx, id).await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        roots.push(root);
    }
    hub.set_pinned("ws-pin");
    // A tiny budget forces eviction of the colder unpinned index.
    hub.set_max_bytes(1);
    assert_eq!(hub.len(), 1);
    assert!(
        hub.index("ws-pin").is_some(),
        "pinned workspace was evicted"
    );
    assert!(
        hub.index("ws-old").is_none(),
        "LRU unpinned should be evicted"
    );
}

fn gated_resolve(calls: Arc<AtomicUsize>, started: Arc<Notify>, release: Arc<Notify>) -> ResolveRg {
    Arc::new(move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        started.notify_one();
        let release = Arc::clone(&release);
        Box::pin(async move {
            release.notified().await;
            NO_RG.to_string()
        })
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn file_index_rebuild_coalesces() {
    let root = tmp();
    write(root.path(), "a.txt", "x");
    let idx = Arc::new(FileIndex::new(&root_str(&root)));
    let (calls, started, release) = (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(Notify::new()),
        Arc::new(Notify::new()),
    );
    let resolve = gated_resolve(
        Arc::clone(&calls),
        Arc::clone(&started),
        Arc::clone(&release),
    );
    let rt = Handle::current();
    idx.rebuild(
        &rt,
        Some(resolve.clone()),
        None,
        false,
        false,
        vec![],
        ExcludeSet::default(),
    );
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .expect("first rebuild did not start");
    // A non-force rebuild while building must not resolve again.
    idx.rebuild(
        &rt,
        Some(resolve),
        None,
        false,
        false,
        vec![],
        ExcludeSet::default(),
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "coalesced");

    let data = tmp();
    let hub = IndexHub::new(&root_str(&data), None);
    hub.set_rg_path(NO_RG);
    hub.insert_index("ws1", Arc::clone(&idx));
    let snap = hub.ensure("ws1", &root_str(&root));
    assert!(snap.building, "ensure should return the in-flight snapshot");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "ensure restarted the build"
    );
    release.notify_one();
    assert!(
        eventually(|| idx.snapshot().ready).await,
        "coalesced rebuild never finished"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn file_index_finish_rebuild_keeps_dirty_on_race() {
    let root = tmp();
    write(root.path(), "a.txt", "x");
    let idx = Arc::new(FileIndex::new(&root_str(&root)));
    let (calls, started, release) = (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(Notify::new()),
        Arc::new(Notify::new()),
    );
    let resolve = gated_resolve(calls, Arc::clone(&started), Arc::clone(&release));
    idx.rebuild(
        &Handle::current(),
        Some(resolve),
        None,
        false,
        true,
        vec![],
        ExcludeSet::default(),
    );
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .expect("rebuild did not start");
    // A mid-walk signal must survive the walk finishing.
    idx.mark_dirty();
    write(root.path(), "b.txt", "x");
    idx.apply_fs_change(&root_str(&root), "b.txt");
    release.notify_one();
    assert!(
        eventually(|| {
            let s = idx.snapshot();
            s.ready && !s.building
        })
        .await,
        "rebuild never finished"
    );
    assert!(
        idx.snapshot().dirty,
        "finish_rebuild cleared a mid-walk dirty"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn file_index_rebuild_outlives_its_caller() {
    let root = tmp();
    write(root.path(), "a.txt", "x");
    let idx = Arc::new(FileIndex::new(&root_str(&root)));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let on_update: OnUpdate = Arc::new(move |snap: Snapshot| {
        if snap.ready && !snap.building {
            let _ = tx.send(());
        }
    });
    let caller = {
        let idx = Arc::clone(&idx);
        tokio::spawn(async move {
            let resolve: ResolveRg = Arc::new(|_| Box::pin(async { NO_RG.to_string() }));
            idx.rebuild(
                &Handle::current(),
                Some(resolve),
                Some(on_update),
                false,
                true,
                vec![],
                ExcludeSet::default(),
            );
        })
    };
    // The RPC that asked for the build returns (and is dropped) right away.
    caller.await.unwrap();
    let done = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
    assert!(matches!(done, Ok(Some(()))), "rebuild died with its caller");
    assert_eq!(idx.snapshot().paths, ["a.txt"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn hub_apply_changes_splice() {
    let root = tmp();
    write(root.path(), "a.txt", "x");
    let data = tmp();
    let hub = IndexHub::new(&root_str(&data), None);
    hub.set_rg_path(NO_RG);
    let (publish, mut rx) = ready_signal();
    hub.set_publish(Some(publish));
    hub.refresh("ws1", &root_str(&root));
    wait_for(&mut rx, "ws1").await;

    write(root.path(), "b.txt", "x");
    hub.apply_changes("ws1", &root_str(&root), &["b.txt".into()], false);
    let paths = hub.ensure("ws1", &root_str(&root)).paths;
    assert!(
        paths.contains(&"a.txt".into()) && paths.contains(&"b.txt".into()),
        "after splice: {paths:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hub_set_exclude_filters_listing() {
    let root = tmp();
    for name in ["src.go", "dist/bundle.js", ".DS_Store"] {
        write(root.path(), name, "x");
    }
    let data = tmp();
    let hub = IndexHub::new(&root_str(&data), None);
    hub.set_rg_path(NO_RG);
    hub.set_exclude(&["dist/**".into(), "**/.DS_Store".into()]);
    let (publish, mut rx) = ready_signal();
    hub.set_publish(Some(publish));
    hub.refresh("ws1", &root_str(&root));
    wait_for(&mut rx, "ws1").await;
    assert_eq!(hub.ensure("ws1", &root_str(&root)).paths, ["src.go"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn hub_search_ranks_and_reports_readiness() {
    let root = tmp();
    write(root.path(), "pkg/UserController.go", "x");
    write(root.path(), "pkg/other.go", "x");
    let data = tmp();
    let hub = IndexHub::new(&root_str(&data), None);
    hub.set_rg_path(NO_RG);
    let (publish, mut rx) = ready_signal();
    hub.set_publish(Some(publish));
    let first = hub.search(
        &CancellationToken::new(),
        "ws1",
        &root_str(&root),
        "uc",
        10,
        true,
    );
    assert!(first.building || first.ready);
    wait_for(&mut rx, "ws1").await;
    let res = hub.search(
        &CancellationToken::new(),
        "ws1",
        &root_str(&root),
        "uc",
        10,
        true,
    );
    assert!(res.ready && !res.building);
    let hits = res.hits.unwrap();
    assert_eq!(hits[0].path, "pkg/UserController.go");
    let mixed = hub.search(
        &CancellationToken::new(),
        "ws1",
        &root_str(&root),
        "pkg",
        10,
        false,
    );
    assert!(
        mixed
            .hits
            .unwrap()
            .iter()
            .any(|h| h.dir && h.path == "pkg/")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hub_debounces_splice_publishes() {
    let root = tmp();
    write(root.path(), "a.txt", "x");
    let data = tmp();
    let hub = IndexHub::new(&root_str(&data), None);
    hub.set_rg_path(NO_RG);
    let (publish, mut rx) = ready_signal();
    hub.set_publish(Some(publish));
    hub.refresh("ws1", &root_str(&root));
    wait_for(&mut rx, "ws1").await;

    let (tx, mut seen) = mpsc::unbounded_channel();
    hub.set_publish(Some(Arc::new(move |ws: &str, _| {
        let _ = tx.send(ws.to_string());
    })));
    for name in ["b.txt", "c.txt", "d.txt"] {
        write(root.path(), name, "x");
        hub.apply_changes("ws1", &root_str(&root), &[name.into()], false);
    }
    let got = tokio::time::timeout(Duration::from_secs(2), seen.recv())
        .await
        .expect("debounced publish");
    assert_eq!(got.as_deref(), Some("ws1"));
    tokio::time::sleep(PUBLISH_DEBOUNCE * 2).await;
    assert!(seen.try_recv().is_err(), "a burst publishes once");
}

#[test]
fn normalize_and_skip_rules() {
    assert_eq!(normalize_rel(" ./a/b "), "a/b");
    assert_eq!(normalize_rel("."), "");
    assert_eq!(normalize_rel("../x"), "");
    assert!(should_skip_indexed(".git/config"));
    assert!(should_skip_indexed("a/node_modules/x/y.js"));
    assert!(should_skip_indexed("logo.PNG"));
    assert!(!should_skip_indexed(".github/ci.yml"));
}
