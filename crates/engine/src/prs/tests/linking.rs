//! Workspace ↔ PR linking, create-from-PR and reconciling missing PRs.

use std::collections::HashSet;
use std::time::Duration;

use recur_domain::{AgentRole, Pr, PrSource, Workspace, WorkspaceLandState, WorkspaceState};
use recur_forge::{PrListState, PrStateProbe};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::support::*;
use crate::bus::EngineEvent;
use crate::loops::test_support::CaptureScript;
use crate::prs::from_pr::checkout_pr_head_name;
use crate::testing::{git, init_repo};
use crate::turn::test_support::maestro_fixture;
use crate::workspace::create::PrHeadCheckoutSpec;

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

#[tokio::test]
async fn link_unlinked_attaches_existing_workspace() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    px.add_pr(gh_pr(12, "feat/oauth"));
    px.add_ws(ws("ws-oauth", "feat/oauth"));
    px.eng().link_unlinked_prs().await;
    assert_eq!(px.pr("gh-acme-12").workspace_id, "ws-oauth");
}

#[tokio::test]
async fn sync_links_existing_workspace_without_a_repo() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme-not-a-repo");
    px.add_pr(gh_pr(12, "feat/oauth"));
    px.add_ws(ws("ws-oauth", "feat/oauth"));
    px.eng().sync_prs().await;
    assert_eq!(px.pr("gh-acme-12").workspace_id, "ws-oauth");
}

#[tokio::test]
async fn matching_branch_skips_project_base() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    px.add_ws(ws("ws-main", "main"));
    assert!(
        px.eng()
            .workspace_matching_pr_branch("acme", "main")
            .await
            .is_none()
    );
}

#[tokio::test]
async fn matching_branch_via_upstream() {
    let px = Px::new();
    let (_keep, clone) = clone_tracking("feat/oauth", "local-oauth");
    px.add_project("acme", &clone);
    px.add_ws(Workspace {
        worktree_path: clone.clone(),
        ..ws("ws-oauth", "local-oauth")
    });
    for head in ["feat/oauth", "local-oauth"] {
        let got = px.eng().workspace_matching_pr_branch("acme", head).await;
        assert_eq!(got.map(|w| w.id).as_deref(), Some("ws-oauth"), "{head}");
    }
}

#[tokio::test]
async fn matching_branch_skips_tracked_project_base() {
    let px = Px::new();
    let (_keep, clone) = clone_tracking("main", "work");
    px.add_project("acme", &clone);
    px.add_ws(Workspace {
        worktree_path: clone.clone(),
        ..ws("ws-work", "work")
    });
    assert!(
        px.eng()
            .workspace_matching_pr_branch("acme", "main")
            .await
            .is_none()
    );
}

#[tokio::test]
async fn matching_branch_prefers_local_name() {
    let px = Px::new();
    let (_keep, clone) = clone_tracking("feat/oauth", "local-oauth");
    px.add_project("acme", &clone);
    px.add_ws(Workspace {
        worktree_path: clone.clone(),
        ..ws("ws-track", "local-oauth")
    });
    px.add_ws(ws("ws-exact", "feat/oauth"));
    let got = px
        .eng()
        .workspace_matching_pr_branch("acme", "feat/oauth")
        .await;
    assert_eq!(got.map(|w| w.id).as_deref(), Some("ws-exact"));
}

#[tokio::test]
async fn link_unlinked_via_upstream() {
    let px = Px::new();
    let (_keep, clone) = clone_tracking("feat/oauth", "local-oauth");
    px.add_project("acme", &clone);
    px.add_pr(gh_pr(12, "feat/oauth"));
    px.add_ws(Workspace {
        worktree_path: clone.clone(),
        ..ws("ws-oauth", "local-oauth")
    });
    px.eng().link_unlinked_prs().await;
    assert_eq!(px.pr("gh-acme-12").workspace_id, "ws-oauth");
}

/// A branch re-pointed with `git branch -u` outside Recur links to the
/// replacement PR on the next sweep; the open PR wins over the closed one
/// still linked to the workspace.
#[tokio::test]
async fn sweep_links_upstream_set_later() {
    let px = Px::new();
    let (_keep, clone) = clone_tracking("recur/tab-close", "me/roasted-risotto");
    git(&clone, &["branch", "--unset-upstream"]);
    px.add_project("acme", &clone);
    px.add_ws(Workspace {
        worktree_path: clone.clone(),
        ..ws("ws-r", "me/roasted-risotto")
    });
    px.add_pr(Pr {
        workspace_id: "ws-r".into(),
        state: "Closed".into(),
        group: "merged".into(),
        ..gh_pr(9, "recur/tab-close")
    });
    px.forge.on_list_prs(|(_, state, _)| async move {
        if state != PrListState::Open {
            return Ok(Vec::new());
        }
        let mut r = row(13, "Tab close menu", "recur/tab-close");
        r.author_login.clear();
        r.updated_at = chrono::Utc::now().to_rfc3339();
        Ok(vec![r])
    });
    px.eng().sweep_prs().await;
    assert_eq!(px.pr("gh-acme-13").workspace_id, "", "before upstream");

    git(&clone, &["branch", "-u", "origin/recur/tab-close"]);
    px.eng().sweep_prs().await;
    assert_eq!(px.pr("gh-acme-13").workspace_id, "ws-r");
    assert_eq!(
        px.pr("gh-acme-9").workspace_id,
        "ws-r",
        "the closed PR keeps its link"
    );
    let mut land = WorkspaceLandState::default();
    px.eng()
        .attach_open_pr_land(&mut land, "ws-r", "", None)
        .await;
    assert_eq!(land.pr_number, 13);
    let linked = px.te.events.by_name("pr.updated").into_iter().any(|e| {
        matches!(e, EngineEvent::PrUpdated(u) if u.pr_id == "gh-acme-13" && u.workspace_id == "ws-r")
    });
    assert!(linked, "pr.updated carries #13's workspace link");
}

#[tokio::test]
async fn link_workspace_to_matching_prs_via_upstream() {
    let px = Px::new();
    let (_keep, clone) = clone_tracking("feat/oauth", "local-oauth");
    px.add_project("acme", &clone);
    px.add_pr(gh_pr(12, "feat/oauth"));
    let w = px.add_ws(Workspace {
        worktree_path: clone.clone(),
        ..ws("ws-oauth", "local-oauth")
    });
    px.eng().link_workspace_to_matching_prs(&w).await;
    assert_eq!(px.pr("gh-acme-12").workspace_id, "ws-oauth");
}

#[tokio::test]
async fn create_from_pr_links_tracking_workspace() {
    let px = Px::new();
    let (_keep, clone) = clone_tracking("feat/oauth", "local-oauth");
    px.add_project("acme", &clone);
    px.add_ws(Workspace {
        worktree_path: clone.clone(),
        ..ws("ws-oauth", "local-oauth")
    });
    px.add_pr(gh_pr(42, "feat/oauth"));
    let got = px
        .eng()
        .create_workspace_from_pr("gh-acme-42")
        .await
        .unwrap();
    assert_eq!(got.id, "ws-oauth");
    assert_eq!(px.pr("gh-acme-42").workspace_id, "ws-oauth");
}

#[tokio::test]
async fn apply_relinks_archived_workspace() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    px.add_ws(Workspace {
        archived: true,
        ..ws("ws-old", "feat/oauth")
    });
    px.add_ws(ws("ws-new", "feat/oauth"));
    px.add_pr(Pr {
        workspace_id: "ws-old".into(),
        ..gh_pr(12, "feat/oauth")
    });
    let mut r = row(12, "OAuth", "feat/oauth");
    r.author_login.clear();
    let (_, created) = px
        .eng()
        .apply_listed_pr(px.forge.as_ref(), "acme", &r, "avery", chrono::Utc::now())
        .await;
    assert!(!created);
    assert_eq!(px.pr("gh-acme-12").workspace_id, "ws-new");
}

#[tokio::test]
async fn create_from_pr_links_existing() {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    let w = px
        .eng()
        .create_workspace("acme", "existing", "main")
        .await
        .unwrap();
    px.add_pr(gh_pr(42, &w.branch));
    let got = px
        .eng()
        .create_workspace_from_pr("gh-acme-42")
        .await
        .unwrap();
    assert_eq!(got.id, w.id);
    assert_eq!(px.pr("gh-acme-42").workspace_id, w.id);
}

#[tokio::test]
async fn create_from_pr_hoists_host_chat_when_linking_existing() {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    px.add_pr(gh_pr(42, "feat/oauth"));
    let host = px.eng().ensure_pr_chat_host("gh-acme-42").await.unwrap();
    let sess = px
        .te
        .store
        .session_for_loop_agent(&host.id, "PA")
        .expect("a PA session on the host");
    px.add_ws(Workspace {
        worktree_path: repo.path.clone(),
        ..ws("ws-oauth", "feat/oauth")
    });
    let got = px
        .eng()
        .create_workspace_from_pr("gh-acme-42")
        .await
        .unwrap();
    assert_eq!(got.id, "ws-oauth");
    let moved = px
        .te
        .store
        .session_for_loop_agent("ws-oauth", "PA")
        .unwrap();
    assert_eq!(moved.id, sess.id);
    assert!(px.te.store.session_for_loop_agent(&host.id, "PA").is_none());
    assert!(px.te.store.agent("ws-oauth", "PA").is_some());
    assert!(px.ws(&host.id).archived, "host archived after linking");
}

#[tokio::test]
async fn send_follows_hoisted_host_alias() {
    let script = CaptureScript::new(|call| maestro_fixture(&format!("hoist-send-{call}"), "ok"));
    let px = Px::with(Some(script.provider()));
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    px.add_pr(Pr {
        id: "gh-acme-42s".into(),
        ..gh_pr(42, "feat/oauth")
    });
    let host = px.eng().ensure_pr_chat_host("gh-acme-42s").await.unwrap();
    px.add_ws(Workspace {
        worktree_path: repo.path.clone(),
        ..ws("ws-oauth", "feat/oauth")
    });
    let got = px
        .eng()
        .create_workspace_from_pr("gh-acme-42s")
        .await
        .unwrap();
    assert_eq!(got.id, "ws-oauth");
    let reply = px
        .eng()
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
    assert!(
        !reply.contains("can't find") && !reply.contains("Create a workspace"),
        "send after hoist still targeted the archived host: {reply}"
    );
    assert!(reply.contains("ok"), "{reply}");
    let spec = script
        .with_role(AgentRole::Coder)
        .pop()
        .expect("a coder turn");
    assert_eq!(spec.workspace_id, "ws-oauth");
    assert_eq!(spec.agent_id, "ws-oauth-PA");
}

#[tokio::test]
async fn create_from_pr_returns_linked() {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    let w = px
        .eng()
        .create_workspace("acme", "linked", "main")
        .await
        .unwrap();
    px.add_pr(Pr {
        workspace_id: w.id.clone(),
        group: "you".into(),
        ..gh_pr(7, "")
    });
    let got = px
        .eng()
        .create_workspace_from_pr("gh-acme-7")
        .await
        .unwrap();
    assert_eq!(got.id, w.id);
}

#[tokio::test]
async fn create_from_pr_uses_remote_tracking_base() {
    let px = Px::new();
    let (_keep, clone) = clone_with_pull_head(42, "feat/oauth");
    px.add_project("acme", &clone);
    px.add_pr(gh_pr(42, "feat/oauth"));
    let got = px
        .eng()
        .create_workspace_from_pr("gh-acme-42")
        .await
        .unwrap();
    assert_eq!(got.base_branch, "origin/main");
    assert_eq!(
        px.pr("gh-acme-42").base,
        "main",
        "the PR keeps the forge's name"
    );
    wait_settled(&px, &got.id).await;
}

#[tokio::test]
async fn create_from_pr_local_only_keeps_short_base() {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    px.add_pr(gh_pr(7, "feat"));
    let got = px
        .eng()
        .create_workspace_from_pr("gh-acme-7")
        .await
        .unwrap();
    assert_eq!(got.base_branch, "main");
    wait_settled(&px, &got.id).await;
}

/// Lets a background allocate finish before the temp repo goes away.
async fn wait_settled(px: &Px, id: &str) {
    crate::testing::wait_for(Duration::from_secs(10), "allocate", || {
        !px.eng().creating_in_flight(id)
    })
    .await;
}

#[tokio::test]
async fn local_base_prefers_project_remote() {
    let px = Px::new();
    let (_keep, clone) = clone_with_upstream_fork();
    px.te
        .store
        .add_project(recur_domain::Project {
            id: "acme".into(),
            name: "acme".into(),
            repo_path: clone.clone(),
            base_branch: "upstream/main".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        px.eng().local_base_from_forge("acme", "main").await,
        "upstream/main"
    );
}

#[tokio::test]
async fn local_base_keeps_project_remote_when_unfetched() {
    let px = Px::new();
    let (_keep, clone) = clone_with_upstream_fork();
    git(&clone, &["checkout", "-b", "release"]);
    git(&clone, &["push", "origin", "release"]);
    git(&clone, &["checkout", "main"]);
    git(&clone, &["branch", "-D", "release"]);
    px.te
        .store
        .add_project(recur_domain::Project {
            id: "acme".into(),
            name: "acme".into(),
            repo_path: clone.clone(),
            base_branch: "upstream/main".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        px.eng().local_base_from_forge("acme", "release").await,
        "upstream/release"
    );
}

#[tokio::test]
async fn local_base_slashy_forge_branch() {
    let px = Px::new();
    let (_keep, clone) = clone_tracking("origin/release", "origin/release");
    px.add_project("acme", &clone);
    assert_eq!(
        px.eng()
            .local_base_from_forge("acme", "origin/release")
            .await,
        "origin/origin/release"
    );
}

#[tokio::test]
async fn pr_repo_path_uses_project_id() {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    let pr = Pr {
        id: "orphan".into(),
        project_id: "acme".into(),
        number: 1,
        source: PrSource::GitHub,
        ..Pr::default()
    };
    assert_eq!(px.eng().pr_repo_path(&pr), repo.path);
}

#[tokio::test]
async fn imported_pr_id() {
    let px = Px::new();
    assert_eq!(
        recur_forge::Forge::imported_pr_id(px.forge.as_ref(), "acme", 12),
        "gh-acme-12"
    );
}

#[tokio::test]
async fn find_forge_pr_url_is_project_scoped() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    px.add_project("beta", "/tmp/beta");
    px.add_pr(gh_pr(12, ""));
    let url = "https://github.com/acme/app/pull/12";
    assert!(
        px.eng()
            .find_forge_pr(px.forge.as_ref(), "beta", 12, url)
            .is_none()
    );
    assert_eq!(
        px.eng()
            .find_forge_pr(px.forge.as_ref(), "acme", 12, url)
            .map(|p| p.id)
            .as_deref(),
        Some("gh-acme-12")
    );
}

fn probe(
    state: &'static str,
    updated: &'static str,
) -> impl Fn(
    (std::path::PathBuf, i64),
) -> std::future::Ready<Result<PrStateProbe, recur_forge::ForgeError>> {
    move |_| {
        std::future::ready(Ok(PrStateProbe {
            state: state.into(),
            updated_at: updated.into(),
        }))
    }
}

#[tokio::test]
async fn reconcile_closes_unseen() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    px.add_pr(Pr {
        title: "Stale".into(),
        ..gh_pr(99, "feat/x")
    });
    px.forge
        .on_pr_state(probe("CLOSED", "2026-08-28T15:04:00Z"));
    px.eng()
        .reconcile_missing_prs(
            px.forge.as_ref(),
            "acme",
            "/tmp/acme",
            &HashSet::new(),
            false,
            deadline(),
        )
        .await;
    let got = px.pr("gh-acme-99");
    assert_eq!(
        (got.state.as_str(), got.group.as_str()),
        ("Closed", "merged")
    );
    assert_eq!(got.updated_at, "2026-08-28T15:04:00Z");
}

#[tokio::test]
async fn reconcile_marks_merged() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    px.add_pr(Pr {
        id: "local-418".into(),
        group: "you".into(),
        state: "In review".into(),
        ..gh_pr(418, "feat/x")
    });
    px.forge.on_pr_state(probe("MERGED", ""));
    px.eng()
        .reconcile_missing_prs(
            px.forge.as_ref(),
            "acme",
            "/tmp/acme",
            &HashSet::new(),
            false,
            deadline(),
        )
        .await;
    assert_eq!(px.pr("local-418").state, "Merged");
}

#[tokio::test]
async fn reconcile_marks_merged_live_workspace() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    px.add_ws(ws("ws-live", "feat/x"));
    let (frozen, merged_at) = ("2026-08-01T12:00:00Z", "2026-08-20T09:00:00Z");
    px.add_pr(Pr {
        id: "gh-live-50".into(),
        workspace_id: "ws-live".into(),
        group: "you".into(),
        state: "In review".into(),
        created_at: frozen.into(),
        updated_at: frozen.into(),
        ..gh_pr(50, "feat/x")
    });
    px.forge
        .on_pr_state(probe("MERGED", "2026-08-20T09:00:00Z"));
    px.eng()
        .reconcile_missing_prs(
            px.forge.as_ref(),
            "acme",
            "/tmp/acme",
            &HashSet::new(),
            false,
            deadline(),
        )
        .await;
    let got = px.pr("gh-live-50");
    assert_eq!(
        (got.state.as_str(), got.group.as_str()),
        ("Merged", "merged")
    );
    assert_eq!(got.updated_at, merged_at);
    assert_eq!(px.ws("ws-live").state, WorkspaceState::Merged);
}

#[tokio::test]
async fn reconcile_skips_when_incomplete() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    px.add_pr(gh_pr(99, "feat/x"));
    px.eng()
        .reconcile_missing_prs(
            px.forge.as_ref(),
            "acme",
            "/tmp/acme",
            &HashSet::new(),
            true,
            deadline(),
        )
        .await;
    assert_eq!(px.pr("gh-acme-99").state, "Open");
    assert!(px.forge.calls_to("pr_state").is_empty());
}

#[test]
fn checkout_head_name_skips_base_and_forks() {
    assert_eq!(
        checkout_pr_head_name("feat/oauth", "main", "main", false),
        "feat/oauth"
    );
    assert_eq!(checkout_pr_head_name("main", "develop", "main", false), "");
    assert_eq!(checkout_pr_head_name("main", "main", "main", false), "");
    assert_eq!(
        checkout_pr_head_name("feat/oauth", "main", "main", true),
        ""
    );
}

#[tokio::test]
async fn link_unlinked_skips_merged() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    px.add_pr(Pr {
        title: "Old".into(),
        group: "merged".into(),
        state: "Merged".into(),
        ..gh_pr(12, "feat/x")
    });
    px.add_ws(ws("ws-new", "feat/x"));
    px.eng().link_unlinked_prs().await;
    assert_eq!(px.pr("gh-acme-12").workspace_id, "");
}

#[tokio::test]
async fn create_from_pr_reuses_in_flight() {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    px.add_pr(gh_pr(42, "feat/oauth"));
    let rec = px
        .eng()
        .persist_creating_workspace("acme", "OAuth", "main")
        .await
        .unwrap();
    px.eng().pr_head_checkout.lock().insert(
        rec.id.clone(),
        PrHeadCheckoutSpec {
            number: 42,
            branch: "feat/oauth".into(),
            pr_id: "gh-acme-42".into(),
            ..PrHeadCheckoutSpec::default()
        },
    );
    let got = px
        .eng()
        .create_workspace_from_pr("gh-acme-42")
        .await
        .unwrap();
    assert_eq!(got.id, rec.id);
    let n = px
        .te
        .store
        .workspaces()
        .into_iter()
        .filter(|w| !w.archived && w.project_id == "acme")
        .count();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn create_from_pr_checks_out_named_head() {
    let px = Px::new();
    let (_keep, clone) = clone_with_pull_head(42, "feat/oauth");
    px.add_project("acme", &clone);
    px.add_pr(gh_pr(42, "feat/oauth"));
    let rec = px
        .eng()
        .persist_creating_workspace("acme", "OAuth", "main")
        .await
        .unwrap();
    px.eng().pr_head_checkout.lock().insert(
        rec.id.clone(),
        PrHeadCheckoutSpec {
            number: 42,
            branch: "feat/oauth".into(),
            pr_id: "gh-acme-42".into(),
            cross_repo: false,
            head: Some(recur_forge::Forge::head_ref(px.forge.as_ref(), 42)),
        },
    );
    px.eng()
        .allocate_workspace_worktree(&CancellationToken::new(), &rec)
        .await
        .unwrap();
    let w = px.ws(&rec.id);
    assert_eq!(w.branch, "feat/oauth");
    assert!(!w.worktree_path.is_empty());
    assert_eq!(px.pr("gh-acme-42").workspace_id, rec.id);
}

#[tokio::test]
async fn unset_source_is_not_a_forge_pr() {
    let px = Px::new();
    px.add_project("acme", "/tmp/acme");
    px.add_pr(Pr {
        source: PrSource::default(),
        ..gh_pr(12, "feat/oauth")
    });
    px.add_ws(ws("ws-oauth", "feat/oauth"));
    px.eng().link_unlinked_prs().await;
    assert_eq!(
        px.pr("gh-acme-12").workspace_id,
        "",
        "only forge PRs are linked"
    );
    assert!(px.eng().pr_forge(&px.pr("gh-acme-12")).is_none());
    assert_eq!(px.eng().pr_source_label(&PrSource::default()), "Local");
    assert_eq!(px.eng().pr_source_label(&PrSource::GitHub), "GitHub");
}
