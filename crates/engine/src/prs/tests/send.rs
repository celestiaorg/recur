//! Sends on PR surfaces.

use recur_domain::{AgentRole, Pr, Workspace};

use super::support::*;
use crate::codes::CHAT_RETRY_DISPLAY_SENTINEL;
use crate::loops::test_support::CaptureScript;
use crate::testing::init_repo;
use crate::turn::test_support::maestro_fixture;

fn capture(prefix: &'static str, reply: &'static str) -> std::sync::Arc<CaptureScript> {
    CaptureScript::new(move |call| maestro_fixture(&format!("{prefix}-{call}"), reply))
}

const WIRE: &str = "⟦recur-surface⟧{\"surface\":\"github_pr\",\"prId\":\"gh-acme-8\",\"prNumber\":8}⟦/recur-surface⟧\nplease review";

#[tokio::test]
async fn pr_chat_host_injects_forge_surface() {
    let script = capture("rev", "Reviewed.");
    let px = Px::with(Some(script.provider()));
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    px.add_pr(Pr {
        title: "Docs".into(),
        ..gh_pr(3, "docs")
    });
    let host = px.eng().ensure_pr_chat_host("gh-acme-3").await.unwrap();
    px.eng()
        .send_workspace_message(
            &host.id,
            "PA",
            "what changed?",
            "sonnet",
            None,
            &[],
            "review",
            "",
        )
        .await;
    let specs = script.with_role(AgentRole::Coder);
    let spec = specs.first().expect("a review turn");
    assert!(
        spec.turn_append_prompt.contains("GitHub pull request #3"),
        "{}",
        spec.turn_append_prompt
    );
    assert!(
        spec.turn_append_prompt.contains("working copy"),
        "{}",
        spec.turn_append_prompt
    );
    assert!(!spec.prompt.contains("recur-surface"));
}

async fn linked_pr8(px: &Px) -> (crate::testing::TempRepo, Workspace) {
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    let w = px
        .eng()
        .create_workspace("acme", "linked", "main")
        .await
        .unwrap();
    px.add_pr(Pr {
        workspace_id: w.id.clone(),
        title: "Already linked".into(),
        group: "you".into(),
        state: "In review".into(),
        ..gh_pr(8, "feat")
    });
    (repo, w)
}

#[tokio::test]
async fn peels_forge_pr_surface_marker() {
    let script = capture("surf", "ok");
    let px = Px::with(Some(script.provider()));
    let (_repo, w) = linked_pr8(&px).await;
    px.eng()
        .send_workspace_message(&w.id, "PA", WIRE, "sonnet", None, &[], "review", "")
        .await;
    let spec = script
        .with_role(AgentRole::Coder)
        .into_iter()
        .next()
        .expect("a review turn");
    assert_eq!(spec.prompt, "please review");
    assert!(
        spec.turn_append_prompt.contains("GitHub pull request #8"),
        "{}",
        spec.turn_append_prompt
    );
    assert!(!spec.turn_append_prompt.contains("different surface"));
}

#[tokio::test]
async fn retry_keeps_forge_pr_surface() {
    let script = capture("retry", "ok");
    let px = Px::with(Some(script.provider()));
    let (_repo, w) = linked_pr8(&px).await;
    let eng = px.eng();
    eng.send_workspace_message(&w.id, "PA", WIRE, "sonnet", None, &[], "review", "")
        .await;
    eng.send_workspace_message(
        &w.id,
        "PA",
        "ignored",
        "sonnet",
        None,
        &[],
        "review",
        CHAT_RETRY_DISPLAY_SENTINEL,
    )
    .await;
    let specs = script.with_role(AgentRole::Coder);
    assert!(specs.len() >= 2, "retry specs = {}", specs.len());
    let got = &specs[1].turn_append_prompt;
    assert!(got.contains("GitHub pull request #8"), "{got}");
    assert!(!got.contains("this Recur workspace"), "{got}");
}

#[tokio::test]
async fn hoist_rekeys_last_chat_surface_for_retry() {
    let script = capture("hoist-surf", "ok");
    let px = Px::with(Some(script.provider()));
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    let w = px
        .eng()
        .create_workspace("acme", "linked", "main")
        .await
        .unwrap();
    let st = &px.te.store;
    let live = st
        .create_session(recur_domain::Session {
            workspace_id: w.id.clone(),
            kind: "pair".into(),
            provider: recur_domain::PROVIDER_CLAUDE_CODE.into(),
            config_json: r#"{"code":"PA"}"#.into(),
            ..Default::default()
        })
        .unwrap();
    st.append_message(
        &live.id,
        recur_domain::Message {
            role: "user".into(),
            content: Some(vec![recur_domain::ContentBlock::text("live chat")]),
            ..Default::default()
        },
    )
    .unwrap();
    st.upsert_agent(
        &w.id,
        recur_domain::Agent {
            id: format!("{}-PA", w.id),
            workspace_id: w.id.clone(),
            code: "PA".into(),
            session_id: "prov-live".into(),
            ..Default::default()
        },
    );
    px.add_pr(Pr {
        id: "gh-acme-8h".into(),
        title: "Already linked".into(),
        group: "you".into(),
        state: "In review".into(),
        ..gh_pr(8, "feat")
    });
    px.add_ws(Workspace {
        id: "W-host-surf".into(),
        project_id: "acme".into(),
        goal: "Already linked".into(),
        host_pr_id: "gh-acme-8h".into(),
        worktree_path: repo.path.clone(),
        r#type: recur_domain::WORKSPACE_SOLO.into(),
        ..Workspace::default()
    });
    st.create_session(recur_domain::Session {
        workspace_id: "W-host-surf".into(),
        kind: "pair".into(),
        provider: recur_domain::PROVIDER_CLAUDE_CODE.into(),
        config_json: r#"{"code":"PA"}"#.into(),
        ..Default::default()
    })
    .unwrap();
    st.upsert_agent(
        "W-host-surf",
        recur_domain::Agent {
            id: "a-pa".into(),
            code: "PA".into(),
            name: "Agent".into(),
            workspace_id: "W-host-surf".into(),
            ..Default::default()
        },
    );
    px.eng()
        .send_workspace_message(
            "W-host-surf",
            "PA",
            "what changed?",
            "sonnet",
            None,
            &[],
            "review",
            "",
        )
        .await;
    // Link after the host turn: an idle host whose PR is already linked is
    // archived on unlock, so the send above would never run.
    st.update_pr("gh-acme-8h", |p| p.workspace_id = w.id.clone());
    px.eng()
        .hoist_pr_chat_host("W-host-surf", &w.id)
        .await
        .unwrap();
    {
        let surfaces = px.eng().last_chat_surfaces.lock();
        assert!(surfaces.contains_key(&format!("{}\x00PA2", w.id)));
        assert!(!surfaces.contains_key("W-host-surf\x00PA"));
    }
    px.eng()
        .send_workspace_message(
            &w.id,
            "PA2",
            "ignored",
            "sonnet",
            None,
            &[],
            "review",
            CHAT_RETRY_DISPLAY_SENTINEL,
        )
        .await;
    let specs = script.with_role(AgentRole::Coder);
    assert!(specs.len() >= 2);
    let got = &specs.last().unwrap().turn_append_prompt;
    assert!(got.contains("GitHub pull request #8"), "{got}");
    assert!(!got.contains("this Recur workspace"), "{got}");
}
