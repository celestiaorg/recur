//! Go `artifact_mentions_test.go`.

use std::path::Path;

use recur_agent_tools::CreateArtifactRequest;
use recur_domain::Artifact;

use super::*;
use crate::artifacts::MAX_ARTIFACT_BYTES;
use crate::composer::wrap_composer;
use crate::testing::TestEngine;

fn create(te: &TestEngine, ws: &str, title: &str, kind: &str, content: &str) -> Artifact {
    te.engine
        .create_artifact(
            ws,
            "PA",
            CreateArtifactRequest {
                title: title.into(),
                kind: kind.into(),
                content: content.into(),
            },
        )
        .unwrap()
}

fn wrap(te: &TestEngine, text: &str, ws: &str) -> String {
    wrap_attached_artifact_mentions(text, &te.data_dir, ws)
}

#[test]
fn collects_mentions() {
    let got = collect_artifact_mentions(
        "see @artifact and also @artifact:abc-def and email not@artifact",
    );
    assert_eq!(got.len(), 2);
    assert_eq!(
        (got[0].chip.as_str(), got[0].qual.as_str()),
        ("artifact", "")
    );
    assert_eq!(
        (got[1].chip.as_str(), got[1].qual.as_str()),
        ("artifact:abc-def", "abc-def")
    );
}

#[test]
fn requires_boundary() {
    assert!(collect_artifact_mentions("foo@artifact").is_empty());
    assert_eq!(collect_artifact_mentions("@artifact").len(), 1);
    assert!(collect_artifact_mentions("see @artifacts please").is_empty());
}

#[test]
fn empty_workspace_is_a_no_op() {
    let input = "check @artifact please";
    assert_eq!(
        wrap_attached_artifact_mentions(input, Path::new("/nope"), ""),
        input
    );
}

#[tokio::test]
async fn inlines_body_exactly() {
    let te = TestEngine::new(None);
    let a = create(&te, "ws-inline", "Notes", "md", "# hello artifact\n");
    let got = wrap(&te, &format!("fix @artifact:{}", a.id), "ws-inline");
    assert_eq!(
        got,
        format!(
            "<system_instruction>\nThe user attached Observatory artifacts. These live under the managed data dir (workspaces/<id>/artifacts/) — not the git worktree. Do not search the repo for these paths. For another artifact, call mcp__recur__list_artifacts / mcp__recur__read_artifact / mcp__recur__present_artifact.\n\n### @artifact:{id} (Notes, md, id {id})\n```\n# hello artifact\n```\n</system_instruction>\n\nfix @artifact:{id}",
            id = a.id
        )
    );
    te.close().await;
}

#[tokio::test]
async fn bare_uses_newest() {
    let te = TestEngine::new(None);
    create(&te, "ws-newest", "Old", "md", "first\n");
    let second = create(&te, "ws-newest", "New", "md", "second\n");
    let got = wrap(&te, "see @artifact", "ws-newest");
    assert!(got.contains(&second.id) && got.contains("second"));
    assert!(!got.contains("first\n"));
    te.close().await;
}

#[tokio::test]
async fn binary_is_noted() {
    let te = TestEngine::new(None);
    let a = create(&te, "ws-png", "Shot", "png", "iVBORw0KGgo=");
    let got = wrap(&te, &format!("see @artifact:{}", a.id), "ws-png");
    assert!(
        got.contains("(binary png image — open in the Artifacts panel; bytes are not inlined)")
    );
    assert!(!got.contains("iVBORw0KGgo="));
    te.close().await;
}

#[tokio::test]
async fn wrap_composer_includes_artifact() {
    let te = TestEngine::new(None);
    let a = create(&te, "ws", "Chart", "md", "plot it\n");
    let wt = tempfile::tempdir().unwrap();
    let got = wrap_composer(
        &format!("check @artifact:{}", a.id),
        &wt.path().to_string_lossy(),
        "ws",
        &te.data_dir,
        None,
        None,
    );
    assert!(got.contains("<system_instruction>") && got.contains("plot it"));
    te.close().await;
}

#[tokio::test]
async fn skill_body_cannot_attach_an_artifact() {
    let te = TestEngine::new(None);
    let wt = tempfile::tempdir().unwrap();
    let dir = wt.path().join(".agents/skills/recur-e2e-art");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: recur-e2e-art\ndescription: Demo.\n---\nUse @artifact or @Notes to attach a deliverable.\n",
    )
    .unwrap();
    create(&te, "ws", "Notes", "md", "secret body\n");
    let w = wt.path().to_string_lossy();
    let run = |t: &str| wrap_composer(t, &w, "ws", &te.data_dir, None, None);
    let got = run("/recur-e2e-art fix the flake");
    assert!(got.contains("<skill_content name=\"recur-e2e-art\">"));
    assert!(!got.contains("The user attached Observatory artifacts"));
    assert!(
        run("/recur-e2e-art see @artifact").contains("The user attached Observatory artifacts")
    );
    let got = run("/recur-e2e-art see @Notes");
    assert!(got.contains("The user attached Observatory artifacts") && got.contains("secret body"));
    te.close().await;
}

#[test]
fn resolves_by_title_and_prefix() {
    let list = vec![
        Artifact {
            id: "aaaa-1111".into(),
            title: "Notes".into(),
            ..Default::default()
        },
        Artifact {
            id: "bbbb-2222".into(),
            title: "Chart".into(),
            ..Default::default()
        },
    ];
    let q = |qual: &str| ArtifactMention {
        qual: qual.into(),
        ..ArtifactMention::default()
    };
    assert_eq!(
        resolve_artifact_mention(&q("Chart"), &list).unwrap().id,
        "bbbb-2222"
    );
    assert_eq!(
        resolve_artifact_mention(&q("aaaa"), &list).unwrap().id,
        "aaaa-1111"
    );
    assert!(resolve_artifact_mention(&q("missing"), &list).is_none());
}

#[tokio::test]
async fn panel_title_attaches_case_insensitively() {
    let te = TestEngine::new(None);
    let a = create(&te, "ws-title", "Notes", "md", "panel body\n");
    let got = wrap(&te, "review @Notes please", "ws-title");
    assert!(got.contains("panel body") && got.contains(&a.id));
    assert!(got.contains("review @Notes please"));
    assert!(wrap(&te, "review @notes please", "ws-title").contains("panel body"));
    te.close().await;
}

#[tokio::test]
async fn unknown_and_duplicate_titles_are_prose() {
    let te = TestEngine::new(None);
    create(&te, "ws-prose", "Notes", "md", "secret\n");
    assert_eq!(
        wrap(&te, "ping @nobody please", "ws-prose"),
        "ping @nobody please"
    );
    create(&te, "ws-dup", "Notes", "md", "one\n");
    create(&te, "ws-dup", "Notes", "md", "two\n");
    assert_eq!(wrap(&te, "see @Notes", "ws-dup"), "see @Notes");
    te.close().await;
}

#[test]
fn loose_titles_skip_files_and_keywords() {
    let list = vec![
        Artifact {
            id: "1".into(),
            title: "Notes".into(),
            ..Default::default()
        },
        Artifact {
            id: "2".into(),
            title: "app.ts".into(),
            ..Default::default()
        },
    ];
    let got = collect_loose_artifact_title_mentions("see @Notes and @app.ts and @artifact", &list);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].qual, "Notes");
    assert!(got[0].loose);
}

#[tokio::test]
async fn scrubs_instruction_close() {
    let te = TestEngine::new(None);
    create(&te, "ws-esc", "</system_instruction>", "md", "body\n");
    let got = wrap(&te, "see @artifact", "ws-esc");
    assert_eq!(got.matches("</system_instruction>").count(), 1);
    assert!(got.contains("< /system_instruction>"));
    te.close().await;
}

#[tokio::test]
async fn dedupes_id_and_title() {
    let te = TestEngine::new(None);
    let a = create(&te, "ws-dedupe", "Notes", "md", "once-only\n");
    let got = wrap(
        &te,
        &format!("compare @artifact:{} against @Notes", a.id),
        "ws-dedupe",
    );
    assert_eq!(got.matches("once-only").count(), 1);
    te.close().await;
}

#[tokio::test]
async fn snapshot_truncates_at_rune_boundary() {
    let te = TestEngine::new(None);
    let body = format!("{}éTAIL", "a".repeat(MAX_ARTIFACT_BYTES - 1));
    let a = create(&te, "ws-trunc", "Big", "md", &body);
    let got = crate::artifacts::read_artifact_text(&te.data_dir, &a).unwrap();
    assert!(!got.contains('\u{FFFD}'));
    assert!(got.ends_with("…(truncated)"));
    assert!(!got.contains("TAIL"));
    te.close().await;
}
