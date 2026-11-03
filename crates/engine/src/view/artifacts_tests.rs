use serde_json::json;

use super::*;

#[test]
fn from_event_accepts_json_tagged_fields() {
    assert_eq!(
        artifact_from_event(&json!({
            "id": "a1", "title": "Report", "kind": "md", "path": "workspaces/w/artifacts/a1.md",
        })),
        Some(ArtifactRef {
            id: "a1".into(),
            title: "Report".into(),
            kind: "md".into(),
            path: "workspaces/w/artifacts/a1.md".into(),
        })
    );
}

#[test]
fn from_event_rejects_exported_go_names_and_missing_fields() {
    assert_eq!(
        artifact_from_event(&json!({
            "ID": "a1", "Title": "Report", "Kind": "HTML", "Path": "workspaces/w/artifacts/a1.html",
        })),
        None
    );
    assert_eq!(artifact_from_event(&Value::Null), None);
    assert_eq!(artifact_from_event(&json!({"title": "x"})), None);
    assert_eq!(artifact_from_event(&json!({"id": "a1"})), None);
    assert_eq!(
        artifact_from_event(&json!({"path": "workspaces/w/artifacts/a1.md"})),
        None
    );
    let titled = artifact_from_event(&json!({"id": " a1 ", "path": "p", "kind": "HTML"})).unwrap();
    assert_eq!(
        (titled.title.as_str(), titled.kind.as_str()),
        ("a1", "html")
    );
}

#[test]
fn unseen_ids_dedupe_and_accumulate() {
    let ids = add_unseen_artifact_id(&UnseenArtifactIds::new(), "ws-b", "art-1");
    let ids = add_unseen_artifact_id(&ids, "ws-b", "art-1");
    assert_eq!(ids["ws-b"], ["art-1"]);
    let ids = add_unseen_artifact_id(&UnseenArtifactIds::new(), "ws-a", "art-1");
    let ids = add_unseen_artifact_id(&ids, "ws-a", "art-2");
    let ids = add_unseen_artifact_id(&ids, "ws-b", "art-1");
    assert_eq!(ids["ws-a"], ["art-1", "art-2"]);
    assert_eq!(ids["ws-b"], ["art-1"]);
    assert_eq!(add_unseen_artifact_id(&ids, " ", "x"), ids);
}

#[test]
fn clear_unseen_clears_one_workspace_only() {
    let ids = UnseenArtifactIds::from([
        ("ws-a".to_owned(), vec!["art-1".to_owned()]),
        ("ws-b".to_owned(), vec!["art-2".to_owned()]),
    ]);
    let ids = clear_unseen_artifact_ids(&ids, "ws-a");
    assert!(ids["ws-a"].is_empty());
    assert_eq!(ids["ws-b"], ["art-2"]);
}

#[test]
fn icon_maps_known_kinds() {
    assert_eq!(artifact_icon("html"), "file-type-html");
    assert_eq!(artifact_icon("md"), "markdown");
    assert_eq!(artifact_icon("JPG"), "photo");
    assert_eq!(artifact_icon("exe"), "file");
}

#[test]
fn scratch_path_ids() {
    assert_eq!(
        artifact_id_from_scratch_path("scratch://artifact-a1.md"),
        "a1"
    );
    assert_eq!(
        artifact_id_from_scratch_path("scratch://artifact-52f1b2e3-0555.html"),
        "52f1b2e3-0555"
    );
    assert_eq!(
        artifact_id_from_scratch_path("scratch://artifact-shot.v2.png"),
        "shot.v2"
    );
    assert_eq!(artifact_id_from_scratch_path(""), "");
    assert_eq!(artifact_id_from_scratch_path("src/lib/artifacts.ts"), "");
    assert_eq!(artifact_id_from_scratch_path("scratch://untitled-1.ts"), "");
}

fn row() -> Value {
    json!({
        "id": "52f1b2e3-0555-471e-bbef-13e9c9b408fb",
        "workspaceId": "recur.bubbly-parfait",
        "title": "Modes as bundle items",
        "kind": "html",
        "path": "workspaces/recur.bubbly-parfait/artifacts/52f1b2e3.html",
        "createdAt": "2026-08-20T04:32:46Z",
        "createdBy": "PA",
    })
}

#[test]
fn parses_the_structured_envelope() {
    let got = parse_list_artifacts_result(&json!({"count": 1, "artifacts": [row()]}).to_string());
    assert_eq!(
        got,
        Some(ListedArtifacts {
            count: 1,
            artifacts: vec![ListedArtifact {
                id: "52f1b2e3-0555-471e-bbef-13e9c9b408fb".into(),
                title: "Modes as bundle items".into(),
                kind: "html".into(),
                path: "workspaces/recur.bubbly-parfait/artifacts/52f1b2e3.html".into(),
                created_at: "2026-08-20T04:32:46Z".into(),
                created_by: "PA".into(),
            }],
        })
    );
}

#[test]
fn accepts_a_prose_prefix() {
    let got = parse_list_artifacts_result(&format!(
        "Listed artifacts\n{}",
        json!({"artifacts": [row()]})
    ))
    .unwrap();
    assert_eq!(got.count, 1);
    assert_eq!(got.artifacts[0].title, "Modes as bundle items");
}

#[test]
fn rejects_sliced_and_unrelated_payloads() {
    let raw = json!({"count": 2, "artifacts": [row(), row()]}).to_string();
    assert_eq!(parse_list_artifacts_result(&raw[..40]), None);
    assert_eq!(
        parse_list_artifacts_result(&json!({"status": "ok"}).to_string()),
        None
    );
    assert_eq!(parse_list_artifacts_result(""), None);
}

#[test]
fn go_nil_slice_is_an_empty_list() {
    assert_eq!(
        parse_list_artifacts_result(&json!({"count": 0, "artifacts": null}).to_string()),
        Some(ListedArtifacts::default())
    );
}

#[test]
fn can_open_requires_id_and_workspaces_path() {
    let r = row();
    let id = r["id"].as_str().unwrap();
    let path = r["path"].as_str().unwrap();
    assert!(artifact_can_open(id, path));
    assert!(artifact_can_open(id, &format!("/{path}")));
    assert!(!artifact_can_open(id, ""));
    assert!(!artifact_can_open(id, "artifacts/x.html"));
    assert!(!artifact_can_open("", path));
}

#[test]
fn resolve_does_not_invent_rows() {
    assert_eq!(
        resolve_list_artifacts(&json!({"artifacts": [row()]}).to_string(), true),
        None
    );
    assert_eq!(
        resolve_list_artifacts(r#"{"count":2,"artifacts":[{"#, false),
        None
    );
}

#[test]
fn kind_tally_orders_known_kinds_and_keeps_extras() {
    assert_eq!(
        artifact_kind_tally(["md", "html", "html", "mystery", ""]),
        vec![
            ArtifactKindTally {
                kind: "html".into(),
                count: 2
            },
            ArtifactKindTally {
                kind: "md".into(),
                count: 1
            },
            ArtifactKindTally {
                kind: "mystery".into(),
                count: 1
            },
        ]
    );
}

#[test]
fn when_is_a_compact_age_with_a_full_stamp() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-08-20T05:32:46Z")
        .unwrap()
        .timestamp_millis();
    let got = artifact_when("2026-08-20T04:32:46Z", now).unwrap();
    assert_eq!(got.rel, "1h");
    assert!(got.full.contains("2026"));
    assert_eq!(artifact_when("", now), None);
}

#[test]
fn relative_age_steps() {
    let now = 10_000_000_000_000;
    let ago = |s: i64| format_relative_age(now - s * 1000, now).unwrap().rel;
    assert_eq!(ago(0), "just now");
    assert_eq!(ago(59), "just now");
    assert_eq!(ago(60), "1m");
    assert_eq!(ago(3600 * 5), "5h");
    assert_eq!(ago(86400 * 13), "13d");
    assert_eq!(ago(86400 * 14), "2w");
    assert_eq!(ago(86400 * 30), "1mo");
    assert_eq!(ago(86400 * 365), "1y");
    assert_eq!(format_relative_age(0, now), None);
}
