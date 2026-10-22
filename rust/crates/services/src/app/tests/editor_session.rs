//! `editor_session_test.go`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use recur_domain::Workspace;
use recur_engine::EngineEvent;
use recur_engine::bus::WorkspaceRef;

use super::{Fx, write};
use crate::api::{
    EditorBuffer, EditorFileView, EditorScratchDoc, EditorSession, EditorSessionMeta,
    EditorSessionPatch,
};
use crate::app::editor_session::EDITOR_SESSION_FILE;

const WS: &str = "WS-1";

async fn fixture() -> Fx {
    let fx = Fx::new().await;
    fx.store()
        .add_workspace(Workspace {
            id: WS.into(),
            project_id: "p".into(),
            ..Default::default()
        })
        .unwrap();
    fx
}

fn session_file(fx: &Fx, ws: &str) -> PathBuf {
    fx.root
        .join("workspaces")
        .join(ws)
        .join(EDITOR_SESSION_FILE)
}

fn meta_patch(open: &[&str], selected: &str) -> EditorSessionPatch {
    EditorSessionPatch {
        meta: Some(meta(open, selected)),
        ..Default::default()
    }
}

fn meta(open: &[&str], selected: &str) -> EditorSessionMeta {
    EditorSessionMeta {
        open_files: (!open.is_empty()).then(|| open.iter().map(|s| s.to_string()).collect()),
        selected_path: selected.into(),
        ..Default::default()
    }
}

fn buf(draft: &str, hash: &str) -> EditorBuffer {
    EditorBuffer {
        draft: draft.into(),
        base_hash: hash.into(),
    }
}

fn map<V>(entries: impl IntoIterator<Item = (&'static str, V)>) -> BTreeMap<String, V> {
    entries
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect()
}

impl Fx {
    async fn update(&self, ws: &str, patch: EditorSessionPatch) {
        self.s
            .editor_session
            .update_editor_session(ws, &patch)
            .await
            .unwrap();
    }

    async fn session(&self, ws: &str) -> Option<EditorSession> {
        self.s.editor_session.get_editor_session(ws).await.unwrap()
    }

    /// Go's fresh `EditorSessionService`: the file on disk, not the cache.
    async fn session_from_disk(&self, ws: &str) -> Option<EditorSession> {
        self.svc.editor.forget(ws);
        self.session(ws).await
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_session_round_trip() {
    let fx = fixture().await;
    assert_eq!(fx.session(WS).await, None);
    let mut patch = meta_patch(&["src/main.go", "src/other.go"], "src/main.go");
    patch.buffers = map([("src/main.go", buf("package main // edited\n", "abc123"))]);
    patch.scratch_docs = map([(
        "scratch://1-notes.txt",
        EditorScratchDoc {
            title: "Notes".into(),
            content: "todo\n".into(),
            lang: "txt".into(),
            ..Default::default()
        },
    )]);
    let view = EditorFileView {
        view_mode: "preview".into(),
        line: 12,
        col: 4,
        top_line: 10,
        scroll_top: 80,
        preview_scroll_top: 240,
        ..Default::default()
    };
    patch.meta.as_mut().unwrap().file_views = map([("src/main.go", view.clone())]);
    fx.update(WS, patch).await;

    let sess = fx.session(WS).await.unwrap();
    assert_eq!(
        sess.buffers["src/main.go"],
        buf("package main // edited\n", "abc123")
    );
    assert_eq!(sess.scratch_docs["scratch://1-notes.txt"].content, "todo\n");
    assert_eq!(sess.open_files.as_ref().map(Vec::len), Some(2));
    assert_eq!(sess.selected_path, "src/main.go");
    assert_eq!(sess.file_views["src/main.go"], view);

    let fresh = fx.session_from_disk(WS).await.unwrap();
    assert_eq!(
        fresh.buffers["src/main.go"].draft,
        "package main // edited\n"
    );
    assert_eq!(fresh.file_views["src/main.go"], view);
    assert!(session_file(&fx, WS).exists());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_session_patch_merge() {
    let fx = fixture().await;
    let mut first = meta_patch(&["a.go", "b.go"], "a.go");
    first.buffers = map([("a.go", buf("A1", "ha")), ("b.go", buf("B1", "hb"))]);
    fx.update(WS, first).await;

    fx.update(
        WS,
        EditorSessionPatch {
            buffers: map([("a.go", buf("A2", "ha"))]),
            ..Default::default()
        },
    )
    .await;
    let sess = fx.session(WS).await.unwrap();
    assert_eq!(
        (
            sess.buffers["a.go"].draft.as_str(),
            sess.buffers["b.go"].draft.as_str()
        ),
        ("A2", "B1")
    );
    assert_eq!(sess.selected_path, "a.go", "meta clobbered");

    let mut m = meta(&["a.go", "b.go"], "b.go");
    m.file_views = map([(
        "b.go",
        EditorFileView {
            view_mode: "diff".into(),
            line: 9,
            col: 1,
            ..Default::default()
        },
    )]);
    fx.update(
        WS,
        EditorSessionPatch {
            meta: Some(m),
            ..Default::default()
        },
    )
    .await;
    let sess = fx.session(WS).await.unwrap();
    assert_eq!(sess.selected_path, "b.go");
    assert_eq!(
        (
            sess.file_views["b.go"].view_mode.as_str(),
            sess.file_views["b.go"].line
        ),
        ("diff", 9)
    );
    assert!(!sess.file_views.contains_key("a.go"), "stale fileView kept");

    fx.update(
        WS,
        EditorSessionPatch {
            drop_buffers: vec!["a.go".into()],
            ..Default::default()
        },
    )
    .await;
    let sess = fx.session(WS).await.unwrap();
    assert!(!sess.buffers.contains_key("a.go"));
    assert_eq!(sess.buffers["b.go"].draft, "B1");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_session_syntax_maps_round_trip() {
    let fx = fixture().await;
    let mut patch = meta_patch(&["src/main.go"], "src/main.go");
    let m = patch.meta.as_mut().unwrap();
    m.syntax_by_path = Some(map([("src/main.go", String::new())]));
    m.syntax_by_file_type = Some(map([("*.conf", "source.yaml".to_owned())]));
    fx.update(WS, patch).await;
    let check = |sess: &EditorSession| {
        assert_eq!(
            sess.syntax_by_path.get("src/main.go").map(String::as_str),
            Some("")
        );
        assert_eq!(sess.syntax_by_file_type["*.conf"], "source.yaml");
    };
    check(&fx.session(WS).await.unwrap());

    fx.update(
        WS,
        EditorSessionPatch {
            buffers: map([("src/main.go", buf("x", "h"))]),
            ..Default::default()
        },
    )
    .await;
    check(&fx.session(WS).await.unwrap());

    fx.update(WS, meta_patch(&["src/other.go"], "src/other.go"))
        .await;
    check(&fx.session(WS).await.unwrap());
    check(&fx.session_from_disk(WS).await.unwrap());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_session_syntax_only_is_kept() {
    let fx = fixture().await;
    let mut seed = meta_patch(&["a.go"], "a.go");
    seed.meta.as_mut().unwrap().syntax_by_path = Some(map([("a.go", "source.yaml".to_owned())]));
    seed.buffers = map([("a.go", buf("X", "h"))]);
    fx.update(WS, seed).await;

    let mut empty_tabs = meta_patch(&[], "");
    empty_tabs.meta.as_mut().unwrap().syntax_by_path =
        Some(map([("a.go", "source.yaml".to_owned())]));
    empty_tabs.drop_buffers = vec!["a.go".into()];
    fx.update(WS, empty_tabs).await;
    let sess = fx.session(WS).await.expect("syntax-only session kept");
    assert!(sess.open_files.unwrap_or_default().is_empty() && sess.buffers.is_empty());
    assert_eq!(sess.syntax_by_path["a.go"], "source.yaml");
    assert!(session_file(&fx, WS).exists());

    let mut gone = meta_patch(&[], "");
    let m = gone.meta.as_mut().unwrap();
    m.syntax_by_path = Some(BTreeMap::new());
    m.syntax_by_file_type = Some(BTreeMap::new());
    fx.update(WS, gone).await;
    assert_eq!(fx.session(WS).await, None);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_session_review_only_is_kept() {
    let fx = fixture().await;
    let mut review = meta_patch(&[], "");
    review.meta.as_mut().unwrap().main_surface = "review".into();
    fx.update(WS, review).await;
    assert_eq!(fx.session(WS).await.unwrap().main_surface, "review");
    assert!(session_file(&fx, WS).exists());
    assert_eq!(
        fx.session_from_disk(WS).await.unwrap().main_surface,
        "review"
    );
    fx.update(WS, meta_patch(&[], "")).await;
    assert_eq!(
        fx.session(WS).await,
        None,
        "editor-only empty session is gone"
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_session_pane_chrome_only_is_kept() {
    let fx = fixture().await;
    let pane = |left: &str, right: &str, open: Option<bool>| {
        let mut p = meta_patch(&[], "");
        let m = p.meta.as_mut().unwrap();
        m.left_kind = left.into();
        m.right_kind = right.into();
        m.left_open = open;
        m.right_open = open;
        p
    };
    fx.update(WS, pane("findings", "scm", Some(false))).await;
    let sess = fx.session(WS).await.unwrap();
    assert_eq!(
        (sess.left_kind.as_str(), sess.right_kind.as_str()),
        ("findings", "scm")
    );
    assert_eq!(
        (sess.left_open, sess.right_open),
        (Some(false), Some(false))
    );
    assert!(session_file(&fx, WS).exists());
    let fresh = fx.session_from_disk(WS).await.unwrap();
    assert_eq!(
        (fresh.left_kind.as_str(), fresh.right_kind.as_str()),
        ("findings", "scm")
    );

    fx.update(WS, pane("maestro", "files", Some(true))).await;
    assert_eq!(fx.session(WS).await, None, "default chrome is empty");
    fx.update(WS, pane("nope", "", None)).await;
    assert_eq!(
        fx.session(WS).await,
        None,
        "an unknown kind must not pin a session"
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_session_empty_merge_deletes() {
    let fx = fixture().await;
    let mut patch = meta_patch(&["a.go"], "a.go");
    patch.buffers = map([("a.go", buf("X", "h"))]);
    fx.update(WS, patch).await;
    let mut empty = meta_patch(&[], "");
    empty.drop_buffers = vec!["a.go".into()];
    fx.update(WS, empty).await;
    assert_eq!(fx.session(WS).await, None);
    assert!(!session_file(&fx, WS).exists());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_session_clear() {
    let fx = fixture().await;
    fx.update(WS, meta_patch(&["a.go"], "a.go")).await;
    let es = &fx.s.editor_session;
    es.clear_editor_session(WS).await.unwrap();
    assert_eq!(fx.session(WS).await, None);
    assert!(!session_file(&fx, WS).exists());
    es.clear_editor_session(WS).await.unwrap();
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_session_unknown_workspace_refused() {
    let fx = fixture().await;
    assert!(
        fx.s.editor_session
            .update_editor_session("nope", &meta_patch(&["a.go"], "a.go"))
            .await
            .is_err()
    );
    assert!(
        !fx.root.join("workspaces/nope").exists(),
        "phantom workspace dir"
    );
    assert_eq!(fx.session("nope").await, None);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_session_corrupt_file_is_absent() {
    let fx = fixture().await;
    write(session_file(&fx, WS), "not gzip");
    assert_eq!(fx.session(WS).await, None);
    fx.update(WS, meta_patch(&["a.go"], "a.go")).await;
    assert_eq!(fx.session(WS).await.unwrap().selected_path, "a.go");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_session_concurrent_patches() {
    let fx = fixture().await;
    fx.update(WS, meta_patch(&["a.go"], "a.go")).await;
    let patches = (0..8u8).map(|i| {
        let patch = EditorSessionPatch {
            buffers: BTreeMap::from([(format!("{}.txt", (b'a' + i) as char), buf("body", "h"))]),
            ..Default::default()
        };
        let s = fx.s.clone();
        async move { s.editor_session.update_editor_session(WS, &patch).await }
    });
    for r in futures::future::join_all(patches).await {
        r.unwrap();
    }
    assert_eq!(fx.session(WS).await.unwrap().buffers.len(), 8);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_session_does_not_allow_undropped_abs_path() {
    let fx = fixture().await;
    let abs = format!("{}/export-111.csv", fx.temp());
    write(&abs, "a,b\n1,2\n");
    fx.update(WS, meta_patch(&[&abs, "src/main.go"], &abs))
        .await;
    assert!(fx.session_from_disk(WS).await.is_some());
    let e = recur_code::prepare_external(&abs).unwrap_err();
    assert!(e.to_string().contains("path not allowed"), "{e}");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_session_clear_drops_cache() {
    let fx = fixture().await;
    fx.update(WS, meta_patch(&["a.go"], "a.go")).await;
    fx.s.editor_session.clear_editor_session(WS).await.unwrap();
    assert!(!fx.svc.editor.cached(WS));
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_session_dropped_on_workspace_deleted() {
    let fx = Fx::new().await;
    fx.store()
        .add_workspace(Workspace {
            id: "WS-del".into(),
            project_id: "p".into(),
            ..Default::default()
        })
        .unwrap();
    fx.update("WS-del", meta_patch(&["a.go"], "a.go")).await;
    assert!(fx.svc.editor.cached("WS-del"));
    fx.svc
        .engine
        .bus
        .publish(EngineEvent::WorkspaceDeleted(WorkspaceRef {
            workspace_id: "WS-del".into(),
        }));
    assert!(!fx.svc.editor.cached("WS-del"));
    fx.close().await;
}
