//! Go `artifacts_test.go` (the PR chat host hoist cases are E5's; the
//! artifact move they rely on is tested here).

use std::collections::HashMap;

use recur_agent_tools::CreateArtifactRequest;
use recur_domain::{Artifact, ArtifactKind};

use super::*;
use crate::bus::EngineEvent;
use crate::testing::TestEngine;

fn req(title: &str, kind: &str, content: &str) -> CreateArtifactRequest {
    CreateArtifactRequest {
        title: title.into(),
        kind: kind.into(),
        content: content.into(),
    }
}

#[tokio::test]
async fn create_and_present_events() {
    let te = TestEngine::new(None);
    let a = te
        .engine
        .create_artifact("ws1", "PA", req(" Notes ", "MD", "# hi\n"))
        .unwrap();
    assert!(!a.id.is_empty());
    assert_eq!(a.kind, ArtifactKind::Md);
    assert_eq!(a.title, "Notes");
    assert_eq!(a.path, format!("workspaces/ws1/artifacts/{}.md", a.id));
    assert_eq!(a.created_by, "PA");
    assert!(a.created_at.ends_with('Z'));
    assert_eq!(te.events.count("artifact.created"), 1);
    let meta = std::fs::read_to_string(
        te.data_dir
            .join("workspaces/ws1/artifacts")
            .join(format!("{}.meta.json", a.id)),
    )
    .unwrap();
    assert!(
        meta.starts_with("{\n  \"id\": "),
        "MarshalIndent shape: {meta}"
    );
    assert!(meta.contains("\n  \"createdBy\": \"PA\"\n}"));
    let list = te.engine.list_artifacts("ws1").unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0], a);
    assert_eq!(te.events.count("artifact.presented"), 0);
    assert_eq!(
        te.engine.present_artifact("ws1", &a.id),
        format!("Presented artifact {} (Notes).", a.id)
    );
    let presented = te.events.by_name("artifact.presented");
    assert_eq!(presented.len(), 1);
    assert!(
        matches!(&presented[0], EngineEvent::ArtifactPresented(p) if p.artifact.id == a.id && p.workspace_id == "ws1")
    );
    assert_eq!(
        te.engine.present_artifact("ws1", "missing"),
        "Artifact not found: missing"
    );
    assert_eq!(te.events.count("artifact.presented"), 1);
    assert_eq!(
        te.engine
            .create_artifact("ws1", "PA", req("x", "exe", ""))
            .unwrap_err()
            .0,
        "unsupported artifact kind \"exe\""
    );
    te.close().await;
}

#[tokio::test]
async fn read_artifact() {
    let te = TestEngine::new(None);
    let a = te
        .engine
        .create_artifact("ws1", "PA", req("Notes", "md", "# hi\n"))
        .unwrap();
    assert_eq!(
        te.engine.read_artifact("ws1", &a.id).unwrap(),
        format!(
            r##"{{"content":"# hi\n","id":"{}","kind":"md","path":"{}","title":"Notes"}}"##,
            a.id, a.path
        )
    );
    assert_eq!(te.events.count("artifact.presented"), 0);
    assert_eq!(
        te.engine.read_artifact("ws1", "missing").unwrap_err().0,
        "artifact not found: missing"
    );
    assert_eq!(
        te.engine.read_artifact("ws1", " ").unwrap_err().0,
        "artifact id is required"
    );
    let png = te
        .engine
        .create_artifact(
            "ws1",
            "PA",
            req(
                "Dot",
                "png",
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
            ),
        )
        .unwrap();
    let got = te.engine.read_artifact("ws1", &png.id).unwrap();
    assert!(got.contains(r#""binary":true"#) && !got.contains(r#""content""#));
    let on_disk = std::fs::read(te.data_dir.join(&png.path)).unwrap();
    assert!(on_disk.starts_with(b"\x89PNG"), "decoded bytes on disk");
    assert_eq!(
        te.engine
            .create_artifact("ws1", "PA", req("Bad", "jpeg", "not base64!"))
            .unwrap_err()
            .0,
        "png/jpeg content must be base64: illegal base64 data at input byte 3"
    );
    let body = "x".repeat(MAX_ARTIFACT_BYTES + 1024);
    let big = te
        .engine
        .create_artifact("ws1", "PA", req("Big", "md", &body))
        .unwrap();
    let got: serde_json::Value =
        serde_json::from_str(&te.engine.read_artifact("ws1", &big.id).unwrap()).unwrap();
    assert_eq!(
        got["content"].as_str().unwrap(),
        body,
        "read_artifact is the full body"
    );
    te.close().await;
}

#[tokio::test]
async fn hoist_moves_artifacts_and_remaps_creators() {
    let te = TestEngine::new(None);
    let a = te
        .engine
        .create_artifact("W-host", "PA", req("Notes", "md", "# host\n"))
        .unwrap();
    te.engine
        .hoist_workspace_artifacts(
            "W-host",
            "W-live",
            &HashMap::from([("PA".to_owned(), "PA2".to_owned())]),
        )
        .unwrap();
    let got = te.engine.list_artifacts("W-live").unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(
        (got[0].id.as_str(), got[0].workspace_id.as_str()),
        (a.id.as_str(), "W-live")
    );
    assert_eq!(
        got[0].path,
        format!("workspaces/W-live/artifacts/{}.md", a.id)
    );
    assert_eq!(got[0].created_by, "PA2");
    assert!(te.engine.list_artifacts("W-host").unwrap().is_empty());
    assert!(
        te.events
            .by_name("artifact.created")
            .iter()
            .any(|e| matches!(e, EngineEvent::ArtifactCreated(c) if c.workspace_id == "W-live"))
    );
    te.close().await;
}

#[tokio::test]
async fn hoist_skips_dest_collision() {
    let te = TestEngine::new(None);
    let dest = te
        .engine
        .create_artifact("W-live", "PA", req("Dest", "md", "dest\n"))
        .unwrap();
    let host_dir = recur_store::paths::artifacts_dir(&te.data_dir, "W-host").unwrap();
    let filename = format!("{}.md", dest.id);
    std::fs::write(host_dir.join(&filename), "host\n").unwrap();
    let host_art = Artifact {
        workspace_id: "W-host".into(),
        title: "Host".into(),
        path: format!("workspaces/W-host/artifacts/{filename}"),
        ..dest.clone()
    };
    std::fs::write(
        host_dir.join(format!("{}.meta.json", dest.id)),
        marshal_indent(&host_art).unwrap(),
    )
    .unwrap();
    te.engine
        .hoist_workspace_artifacts("W-host", "W-live", &HashMap::new())
        .unwrap();
    let live = te.engine.list_artifacts("W-live").unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(
        (live[0].id.as_str(), live[0].title.as_str()),
        (dest.id.as_str(), "Dest")
    );
    let host = te.engine.list_artifacts("W-host").unwrap();
    assert_eq!(host.len(), 1);
    assert_eq!(host[0].title, "Host");
    te.close().await;
}

#[test]
fn commit_rolls_back_on_meta_write_failure() {
    let dir = tempfile::tempdir().unwrap();
    let src_file = dir.path().join("src.md");
    let dest_file = dir.path().join("dest.md");
    let src_meta = dir.path().join("src.meta.json");
    let dest_meta = dir.path().join("dest.meta.json");
    std::fs::write(&dest_file, "host\n").unwrap();
    std::fs::write(&src_meta, r#"{"id":"a"}"#).unwrap();
    std::fs::create_dir(&dest_meta).unwrap();
    let err = commit_hoisted_artifact(
        &src_file,
        &dest_file,
        &src_meta,
        &dest_meta,
        &Artifact {
            id: "a".into(),
            title: "Notes".into(),
            kind: ArtifactKind::Md,
            ..Default::default()
        },
    );
    assert!(err.is_err());
    assert!(src_meta.exists(), "source metadata kept");
    assert!(src_file.exists(), "content rolled back");
    assert!(!dest_file.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn hoist_returns_on_move_failure() {
    use std::os::unix::fs::PermissionsExt;
    let te = TestEngine::new(None);
    let a = te
        .engine
        .create_artifact("W-host", "PA", req("Notes", "md", "# host\n"))
        .unwrap();
    let dest_dir = recur_store::paths::artifacts_dir(&te.data_dir, "W-live").unwrap();
    std::fs::set_permissions(&dest_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let res = te
        .engine
        .hoist_workspace_artifacts("W-host", "W-live", &HashMap::new());
    std::fs::set_permissions(&dest_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        res.unwrap_err()
            .0
            .starts_with(&format!("hoist artifact {}: ", a.id))
    );
    let host = te.engine.list_artifacts("W-host").unwrap();
    assert_eq!(host.len(), 1);
    assert_eq!(host[0].id, a.id);
    te.close().await;
}

#[tokio::test]
async fn rename_and_delete() {
    let te = TestEngine::new(None);
    let a = te
        .engine
        .create_artifact("ws1", "PA", req("Notes", "md", "# hi\n"))
        .unwrap();
    assert_eq!(
        te.engine.rename_artifact("ws1", &a.id, "  ").unwrap_err().0,
        "title is required"
    );
    te.engine.rename_artifact("ws1", &a.id, "Renamed").unwrap();
    let list = te.engine.list_artifacts("ws1").unwrap();
    assert_eq!(
        (list[0].title.as_str(), list[0].path.as_str()),
        ("Renamed", a.path.as_str())
    );
    let full = te.engine.artifact_abs_path("ws1", &a.id).unwrap();
    te.engine.delete_artifact("ws1", &a.id).unwrap();
    assert!(te.engine.list_artifacts("ws1").unwrap().is_empty());
    assert!(!full.exists());
    assert_eq!(
        te.engine.delete_artifact("ws1", &a.id).unwrap_err().0,
        format!("artifact not found: {}", a.id)
    );
    te.close().await;
}

#[tokio::test]
async fn listing_never_creates_directories_and_refuses_escapes() {
    let te = TestEngine::new(None);
    assert!(te.engine.list_artifacts("ghost").unwrap().is_empty());
    assert!(!te.data_dir.join("workspaces/ghost").exists());
    let escape = Artifact {
        id: "x".into(),
        workspace_id: "ws1".into(),
        path: "workspaces/other/artifacts/x.md".into(),
        ..Default::default()
    };
    assert_eq!(
        artifact_abs_path(&te.data_dir, &escape).unwrap_err().0,
        "artifact path outside workspace"
    );
    te.close().await;
}

#[test]
fn go_indent_matches_marshal_indent() {
    #[derive(serde::Serialize)]
    struct S {
        a: Vec<i32>,
        b: Vec<i32>,
        c: std::collections::BTreeMap<String, String>,
        d: String,
    }
    let got = marshal_indent(&S {
        a: vec![],
        b: vec![1, 2],
        c: Default::default(),
        d: "x<y\"".into(),
    })
    .unwrap();
    assert_eq!(
        got,
        "{\n  \"a\": [],\n  \"b\": [\n    1,\n    2\n  ],\n  \"c\": {},\n  \"d\": \"x\\u003cy\\\"\"\n}"
    );
}

#[test]
fn rfc3339_nano_trims_zeros() {
    use chrono::TimeZone;
    let t = chrono::Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
    assert_eq!(rfc3339_nano_utc(t), "2026-01-02T03:04:05Z");
    let t = t + chrono::Duration::nanoseconds(120_000_000);
    assert_eq!(rfc3339_nano_utc(t), "2026-01-02T03:04:05.12Z");
}
