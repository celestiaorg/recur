//! `services_test.go`.

use std::sync::Arc;
use std::time::Duration;

use recur_domain::{
    AutonomyLevel, Commit, FileDiff, Group, Pr, PrSource, Project, ProjectGroup, WorkspaceState,
    reaction_mine,
};
use recur_engine::bus::{EngineEvent, WorkspaceRef};
use recur_provider_claude::testing::NdjsonScript;

use super::{Fx, FxOpts, git, write};
use crate::AppEvent;

fn unread(fx_inbox: &[recur_domain::InboxItem]) -> usize {
    fx_inbox.iter().filter(|i| i.unread).count()
}

#[tokio::test(flavor = "multi_thread")]
async fn seed_data_shape() {
    let fx = Fx::new().await;
    assert_eq!(fx.s.workspaces.list_workspaces().await.unwrap().len(), 5);
    assert_eq!(fx.s.projects.list_projects().await.unwrap().len(), 3);
    assert_eq!(fx.s.prs.list_prs().await.unwrap().len(), 3);
    let esc = fx.s.escalation.get_escalation("LOOP-2218").await.unwrap();
    assert_eq!(esc.status, "open");
    assert_eq!(esc.options.as_ref().map_or(0, Vec::len), 3);
    let l = fx.s.workspaces.get_workspace("LOOP-2231").await.unwrap();
    let conv = l.convergence.unwrap();
    assert_eq!(conv.len(), 3);
    assert_eq!(conv[2].findings, 0);
    let pr = fx.s.prs.get_pr("482").await.unwrap();
    assert_eq!(pr.number, 482);
    assert_eq!(fx.s.prs.get_pr_files("482").await.unwrap().len(), 3);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn get_pr_omits_commit_diffs() {
    let fx = Fx::new().await;
    let files = vec![FileDiff {
        path: "a.go".into(),
        status: "m".into(),
        ..Default::default()
    }];
    fx.store()
        .update_pr("482", |p| {
            p.commits = Some(vec![Commit {
                hash: "abc1234".into(),
                files: Some(files),
                ..Default::default()
            }])
        })
        .unwrap();
    let pr = fx.s.prs.get_pr("482").await.unwrap();
    let commits = pr.commits.unwrap();
    assert_eq!(commits.len(), 1);
    assert!(
        commits[0].files.is_none(),
        "GetPR must drop per-commit files"
    );
    let c = fx.s.prs.get_commit_diff("482", "abc1234").await.unwrap();
    assert_eq!(c.files.unwrap().len(), 1);
    fx.close().await;
}

/// The forge diff starts with GetPR, not after its checks fetch: Files
/// changed would otherwise wait on the slowest of them first.
#[tokio::test(flavor = "multi_thread")]
async fn get_pr_starts_the_diff_before_the_checks_return() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let forge = Arc::new(recur_forge::testing::FakeForge::new());
    let diffing = Arc::new(AtomicBool::new(false));
    let seen = Arc::new(AtomicBool::new(false));
    let (d, d2, s) = (diffing.clone(), diffing.clone(), seen.clone());
    forge
        .on_diff(move |_| {
            d.store(true, Ordering::SeqCst);
            async { Ok(String::new()) }
        })
        .on_checks(move |_| {
            let (d, s) = (d2.clone(), s.clone());
            async move {
                for _ in 0..100 {
                    if d.load(Ordering::SeqCst) {
                        s.store(true, Ordering::SeqCst);
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Ok(recur_forge::ChecksReport::default())
            }
        });
    let fx = super::Fx::with(super::FxOpts {
        forges: Some(vec![forge.clone() as Arc<dyn recur_forge::Forge>]),
        ..super::FxOpts::default()
    })
    .await;
    fx.store().update_pr("482", |p| p.files = None).unwrap();
    fx.s.prs.get_pr("482").await.unwrap();
    assert_eq!(forge.calls_to("checks").len(), 1, "{:?}", forge.calls());
    assert!(seen.load(Ordering::SeqCst), "the diff waited on checks");
    fx.close().await;
}

/// Go `TestBusFanOut`: every subscriber sees a bridged engine event, in
/// order.
#[tokio::test(flavor = "multi_thread")]
async fn bus_fan_out() {
    let fx = Fx::new().await;
    let mut a = fx.s.subscribe();
    let mut b = fx.s.subscribe();
    fx.svc
        .engine
        .bus
        .publish(EngineEvent::WorkspaceDeleted(WorkspaceRef::new("W-1")));
    for rx in [&mut a, &mut b] {
        match rx.recv().await.unwrap() {
            AppEvent::WorkspaceDeleted(r) => assert_eq!(r.workspace_id, "W-1"),
            other => panic!("unexpected {other:?}"),
        }
    }
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn answer_escalation_resumes_loop() {
    let fx = Fx::new().await;
    let before = fx.s.settings.get_resource().await.unwrap().needs_you;
    let e =
        fx.s.escalation
            .answer_escalation("LOOP-2218", "Token bucket with refill", "", "")
            .await
            .unwrap();
    assert_eq!(e.status, "answered");
    assert_eq!(e.answer, "Token bucket with refill");
    let l = fx.s.workspaces.get_workspace("LOOP-2218").await.unwrap();
    assert_eq!(l.group, Group::Active, "loop must leave needs-you");
    // The seeded demo loop has no worktree: it rests paused, not a phantom
    // coding state with nothing running.
    assert_eq!(l.state, WorkspaceState::Paused);
    let after = fx.s.settings.get_resource().await.unwrap().needs_you;
    assert_eq!(after, before - 1);
    assert!(fx.names().contains(&"escalation.resolved"));
    assert_eq!(fx.svc.engine.governor().loops_running(), 0);

    let again =
        fx.s.escalation
            .answer_escalation("LOOP-2218", "Token bucket", "100 req/min per key", "")
            .await;
    assert!(again.is_err(), "a second answer is a no-op error");
    assert_eq!(fx.svc.engine.governor().loops_running(), 0);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn send_back_to_loop_is_conductor_turn() {
    let fx = Fx::new().await;
    let repo = fx.temp();
    fx.s.projects
        .update_project_run_config(&Project {
            id: "storefront".into(),
            name: "storefront".into(),
            repo_path: repo,
            base_branch: "main".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let before =
        fx.s.prs
            .get_pr("482")
            .await
            .unwrap()
            .comments
            .map_or(0, |c| c.len());
    let p =
        fx.s.prs
            .send_back_to_loop("482", &["fix the secret".into()])
            .await
            .unwrap();
    assert_eq!((p.state.as_str(), p.group.as_str()), ("In review", "you"));
    let got = fx.s.prs.get_pr("482").await.unwrap();
    assert_eq!(got.state, "In review");
    let comments = got.comments.unwrap_or_default();
    assert!(comments.len() > before, "send-back note appended");
    assert!(
        comments
            .iter()
            .any(|c| c.body_md.contains("fix the secret") || c.body_html.contains("fix the secret"))
    );
    assert!(fx.names().contains(&"pr.updated"));
    assert_eq!(fx.svc.engine.governor().loops_running(), 0);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_review_and_comment() {
    let fx = Fx::new().await;
    fx.store()
        .add_pr(Pr {
            id: "10".into(),
            number: 10,
            source: PrSource::Local,
            title: "local".into(),
            state: "In review".into(),
            group: "you".into(),
            ..Default::default()
        })
        .unwrap();
    let prs = &fx.s.prs;
    let before = prs
        .get_pr("10")
        .await
        .unwrap()
        .comments
        .map_or(0, |c| c.len());
    prs.add_pr_comment(
        "10",
        "src/auth/oauth.ts",
        12,
        "please confirm the secrets client init order",
        0,
        "",
    )
    .await
    .unwrap();
    let after = prs.get_pr("10").await.unwrap();
    assert_eq!(
        after.comments.map_or(0, |c| c.len()),
        before,
        "line note stays pending"
    );
    assert_eq!(after.pending_comments.len(), 1);
    assert_eq!(after.pending_comments[0].line, 12);

    prs.submit_review("10", "approve", "looks good")
        .await
        .unwrap();
    let got = prs.get_pr("10").await.unwrap();
    assert!(got.pending_comments.is_empty());
    let comments = got.comments.unwrap_or_default();
    assert_eq!(comments.len(), before + 2);
    let review_id = comments[before..]
        .iter()
        .find(|c| c.parent_id.is_empty() && c.body_html.contains("looks good"))
        .map(|c| c.id.clone())
        .unwrap_or_default();
    let child_parent = comments[before..]
        .iter()
        .find(|c| !c.parent_id.is_empty())
        .map(|c| c.parent_id.clone())
        .unwrap_or_default();
    assert!(!review_id.is_empty() && child_parent == review_id);
    let line_id = comments
        .iter()
        .find(|c| c.parent_id == review_id && c.is_line_note())
        .map(|c| c.id.clone())
        .expect("line note");
    let reply = prs
        .reply_pr_comment("10", &line_id, "thread reply")
        .await
        .unwrap();
    assert_eq!(reply.in_reply_to_id, line_id);
    prs.set_pr_thread_resolved("10", &line_id, true)
        .await
        .unwrap();
    let marked = prs.get_pr("10").await.unwrap();
    assert!(
        marked
            .comments
            .unwrap_or_default()
            .iter()
            .filter(|c| c.id == line_id)
            .all(|c| c.resolved)
    );
    prs.delete_pr_comment("10", &line_id).await.unwrap();
    let after_del = prs.get_pr("10").await.unwrap();
    assert!(
        after_del
            .comments
            .unwrap_or_default()
            .iter()
            .all(|c| c.id != line_id)
    );

    prs.add_pr_comment("10", "", 0, "plain conversation note", 0, "")
        .await
        .unwrap();
    prs.update_pr_details("10", "local retitled", "new description")
        .await
        .unwrap();
    let named = prs.get_pr("10").await.unwrap();
    assert_eq!(named.title, "local retitled");
    let you = named
        .comments
        .unwrap_or_default()
        .into_iter()
        .find(|c| c.author == "You" && c.kind != "description")
        .map(|c| c.id)
        .expect("a You comment");
    prs.update_pr_comment("10", &you, "edited conversation note")
        .await
        .unwrap();
    let reacted = prs
        .toggle_pr_comment_reaction("10", &you, "+1")
        .await
        .unwrap();
    assert!(
        reacted
            .comments
            .unwrap_or_default()
            .iter()
            .any(|c| c.id == you && reaction_mine(&c.reactions, "+1"))
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closed_pr_rejects_comments() {
    let fx = Fx::new().await;
    fx.store()
        .add_pr(Pr {
            id: "471".into(),
            number: 471,
            source: PrSource::GitHub,
            title: "merged".into(),
            state: "Merged".into(),
            group: "merged".into(),
            ..Default::default()
        })
        .unwrap();
    let prs = &fx.s.prs;
    let e = prs
        .add_pr_comment("471", "", 0, "too late", 0, "")
        .await
        .unwrap_err();
    assert!(e.message.contains("closed"), "{e}");
    let e = prs
        .submit_review("471", "approve", "lgtm")
        .await
        .unwrap_err();
    assert!(e.message.contains("closed"), "{e}");
    let e = prs
        .reply_pr_comment("471", "c1", "too late")
        .await
        .unwrap_err();
    assert!(e.message.contains("closed"), "{e}");
    assert!(prs.update_pr_details("471", "nope", "nope").await.is_err());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn inbox_mark_read() {
    let fx = Fx::new().await;
    assert_eq!(unread(&fx.s.inbox.get_inbox().await.unwrap()), 4);
    assert_eq!(fx.s.inbox.mark_inbox_read("ib1").await.unwrap(), 3);
    assert_eq!(fx.s.inbox.mark_all_inbox_read().await.unwrap(), 0);
    assert!(fx.names().contains(&"inbox.updated"));
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn policy_toggle_round_trip() {
    let fx = Fx::new().await;
    let pol = fx.s.policy.get_policy().await.unwrap();
    assert_eq!(pol.autonomy, AutonomyLevel::Balanced);
    let mut pol2 = pol.clone();
    pol2.autonomy = AutonomyLevel::Aggressive;
    let got = fx.s.policy.set_policy(&pol2).await.unwrap();
    assert_eq!(got.autonomy, AutonomyLevel::Aggressive);
    assert_eq!(
        fx.s.policy.get_policy().await.unwrap().autonomy,
        AutonomyLevel::Aggressive
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn add_project_assigns_slug_and_defaults() {
    let fx = Fx::new().await;
    let before = fx.s.projects.list_projects().await.unwrap().len();
    let p =
        fx.s.projects
            .add_project(&Project {
                name: "My New App".into(),
                repo_path: "~/code/my-new-app".into(),
                base_branch: "main".into(),
                ..Default::default()
            })
            .await
            .unwrap();
    assert_eq!(p.id, "my-new-app");
    assert_eq!(p.pic, "M");
    assert!(!p.color.is_empty());
    assert!(p.open, "new projects default to expanded");
    assert_eq!(
        fx.s.projects.list_projects().await.unwrap().len(),
        before + 1
    );
    let p2 =
        fx.s.projects
            .add_project(&Project {
                name: "My New App".into(),
                ..Default::default()
            })
            .await
            .unwrap();
    assert_eq!(p2.id, "my-new-app-2");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn preview_copy_files() {
    let fx = Fx::new().await;
    let repo = fx.temp();
    write(format!("{repo}/.env"), "A=1\n");
    write(format!("{repo}/apps/.env.local"), "B=2\n");
    let p =
        fx.s.projects
            .add_project(&Project {
                name: "copy-preview".into(),
                repo_path: repo,
                base_branch: "main".into(),
                ..Default::default()
            })
            .await
            .unwrap();
    let got =
        fx.s.projects
            .preview_copy_files(&p.id, ".env*")
            .await
            .unwrap();
    assert!(got.error.is_empty(), "{}", got.error);
    assert_eq!(got.count, 2, "{:?}", got.files);
    let missing =
        fx.s.projects
            .preview_copy_files("nope", ".env*")
            .await
            .unwrap();
    assert!(!missing.error.is_empty());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn project_group_assign_and_delete() {
    let fx = Fx::new().await;
    let g =
        fx.s.projects
            .add_project_group(&ProjectGroup {
                name: "Side projects".into(),
                ..Default::default()
            })
            .await
            .unwrap();
    assert!(uuid::Uuid::parse_str(&g.id).is_ok() && g.open, "{g:?}");
    assert_eq!(g.name, "Side projects");
    let projs = fx.s.projects.list_projects().await.unwrap();
    let saved =
        fx.s.projects
            .set_project_group(&projs[0].id, &g.id)
            .await
            .unwrap();
    assert_eq!(saved.group_id, g.id);
    assert!(
        fx.s.projects
            .set_project_group(&projs[0].id, "missing")
            .await
            .is_err()
    );
    fx.s.projects.delete_project_group(&g.id).await.unwrap();
    let got = fx.s.projects.get_project(&projs[0].id).await.unwrap();
    assert!(got.group_id.is_empty());
    fx.close().await;
}

/// Go `TestStartWorkspaceServiceEndToEnd`: StartWorkspace over a scripted
/// provider runs to a ready PR with a live feed.
#[tokio::test(flavor = "multi_thread")]
async fn start_workspace_service_end_to_end() {
    let provider = NdjsonScript::new([
        r#"{"type":"system","subtype":"init","session_id":"s1","model":"sonnet","mcp_servers":[]}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Write","input":{"file_path":"svc.txt"}}]}}"#,
        r#"{"type":"result","subtype":"success","session_id":"s1","duration_ms":10,"is_error":false,"num_turns":1,"result":"ok","total_cost_usd":0}"#,
    ])
    .with_edits([("svc.txt".to_owned(), "from coder\n".to_owned())]);
    let fx = Fx::with(FxOpts {
        file_store: true,
        provider: Some(Arc::new(provider)),
        ..Default::default()
    })
    .await;
    // Rail #1 (verify_browser) would gate pr_open behind a browser verify
    // this plumbing test has no run config for.
    let mut p = fx.store().policy();
    for r in p.rails.iter_mut().flatten() {
        if r.key == "verify_browser" {
            r.on = false;
        }
    }
    fx.store().set_policy(p);
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let id =
        fx.s.workspaces
            .start_workspace(&pid, "Add a service file", "", "")
            .await
            .unwrap();
    assert!(!id.is_empty());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut state = WorkspaceState::default();
    while tokio::time::Instant::now() < deadline {
        state = fx.s.workspaces.get_workspace(&id).await.unwrap().state;
        if state == WorkspaceState::PrOpen {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(state, WorkspaceState::PrOpen);
    assert!(
        !fx.s
            .workspaces
            .get_workspace_feed(&id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        fx.s.workspaces
            .list_workspaces()
            .await
            .unwrap()
            .iter()
            .any(|w| w.id == id)
    );
    fx.s.workspaces.stop_loop(&id).await.unwrap();
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn getters_error_on_missing() {
    let fx = Fx::new().await;
    let bogus = "does-not-exist";
    assert!(fx.s.projects.get_project(bogus).await.is_err());
    assert!(fx.s.workspaces.get_workspace(bogus).await.is_err());
    let esc = fx.s.escalation.get_escalation(bogus).await.unwrap();
    assert!(
        esc.id.is_empty() && esc.status.is_empty(),
        "missing escalation = {esc:?}, want empty value"
    );
    assert!(fx.s.agents.get_agent_trace(bogus, "CO").await.is_err());
    assert!(fx.s.prs.get_pr(bogus).await.is_err());
    assert!(fx.s.prs.get_commit_diff("482", bogus).await.is_err());
    assert!(
        fx.s.prs
            .add_pr_comment(bogus, "", 0, "hi", 0, "")
            .await
            .is_err()
    );
    assert!(
        fx.s.prs
            .submit_review(bogus, "comment", "hi")
            .await
            .is_err()
    );
    assert!(fx.s.prs.update_pr_details(bogus, "t", "b").await.is_err());
    assert!(fx.s.prs.update_pr_comment(bogus, "c1", "hi").await.is_err());
    assert!(
        fx.s.prs
            .send_back_to_loop(bogus, &["x".into()])
            .await
            .is_err()
    );
    assert!(fx.s.prs.merge_pr(bogus, "").await.is_err());
    // Go's %q in the not-found text.
    assert_eq!(
        fx.s.projects.get_project(bogus).await.unwrap_err().message,
        "project \"does-not-exist\" not found"
    );
    assert_eq!(
        fx.s.workspaces
            .get_workspace(bogus)
            .await
            .unwrap_err()
            .message,
        "loop \"does-not-exist\" not found"
    );
    fx.close().await;
}

fn commit_file(repo: &str, name: &str, body: &str, msg: &str) -> String {
    write(format!("{repo}/{name}"), body);
    git(repo, &["add", name]);
    git(repo, &["commit", "-m", msg]);
    super::git_out(repo, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_owned()
}

async fn point_pr_482(fx: &Fx, repo: &str, worktree: &str, full: &str, msg: &str) {
    fx.s.projects
        .update_project_run_config(&Project {
            id: "storefront".into(),
            name: "storefront".into(),
            repo_path: repo.into(),
            base_branch: "main".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    fx.store()
        .update_workspace("LOOP-2231", |w| w.worktree_path = worktree.into())
        .unwrap();
    let (full, msg) = (full.to_owned(), msg.to_owned());
    fx.store()
        .update_pr("482", |p| {
            p.base = "main".into();
            p.commits = Some(vec![Commit {
                hash: full[..7].into(),
                sha: full,
                message: msg,
                ..Default::default()
            }]);
        })
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn get_commit_diff_fills_lazy_files() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let full = commit_file(&repo, "feat.txt", "hello\n", "add feat");
    point_pr_482(&fx, &repo, &repo, &full, "add feat").await;
    let got = fx.s.prs.get_commit_diff("482", &full[..7]).await.unwrap();
    assert!(got.files.unwrap().iter().any(|f| f.path == "feat.txt"));
    let again = fx.s.prs.get_commit_diff("482", &full).await.unwrap();
    assert!(!again.files.unwrap().is_empty());
    let pr = fx.store().pr("482").unwrap();
    assert!(
        !pr.commits.unwrap()[0]
            .files
            .clone()
            .unwrap_or_default()
            .is_empty()
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn get_commit_diff_falls_back_to_repo() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let full = commit_file(&repo, "feat.txt", "hello\n", "add feat");
    point_pr_482(&fx, &repo, "", &full, "add feat").await;
    let got = fx.s.prs.get_commit_diff("482", &full[..7]).await.unwrap();
    assert!(got.files.unwrap().iter().any(|f| f.path == "feat.txt"));

    let stale = fx.temp();
    point_pr_482(&fx, &repo, &stale, &full, "add feat").await;
    let again = fx.s.prs.get_commit_diff("482", &full[..7]).await.unwrap();
    assert!(again.files.unwrap().iter().any(|f| f.path == "feat.txt"));
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn get_commit_diff_caches_empty() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    git(&repo, &["commit", "--allow-empty", "-m", "empty"]);
    let full = super::git_out(&repo, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_owned();
    point_pr_482(&fx, &repo, &repo, &full, "empty").await;
    let got = fx.s.prs.get_commit_diff("482", &full[..7]).await.unwrap();
    assert!(got.files.is_some(), "an empty diff is cached as []");
    fx.store()
        .update_workspace("LOOP-2231", |w| w.worktree_path.clear())
        .unwrap();
    let elsewhere = fx.temp();
    fx.s.projects
        .update_project_run_config(&Project {
            id: "storefront".into(),
            name: "storefront".into(),
            repo_path: elsewhere,
            base_branch: "main".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let again = fx.s.prs.get_commit_diff("482", &full[..7]).await.unwrap();
    assert!(again.files.is_some(), "cached empty diff was lost");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn start_workspace_unknown_project_errors() {
    let fx = Fx::new().await;
    let res =
        fx.s.workspaces
            .start_workspace("no-such-project", "do something", "", "")
            .await;
    assert!(res.is_err());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_and_delete_workspace_service() {
    let fx = Fx::new().await;
    let removed = fx.s.workspaces.remove_workspace("LOOP-2224").await.unwrap();
    assert!(removed.archived);
    // Remove's background teardown holds the id until it finishes.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let res = loop {
        let r = fx.s.workspaces.delete_workspace("LOOP-2224").await;
        match &r {
            Err(e)
                if e.message.contains("being removed")
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            _ => break r,
        }
    };
    res.unwrap();
    assert!(fx.s.workspaces.get_workspace("LOOP-2224").await.is_err());
    assert!(fx.s.workspaces.remove_workspace("LOOP-2231").await.is_err());
    assert!(fx.s.workspaces.delete_workspace("LOOP-2231").await.is_err());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_logs_path() {
    let fx = Fx::new().await;
    let got = fx.s.settings.logs_path().await.unwrap();
    assert_eq!(
        got,
        recur_store::applog::app_log_path(&fx.root)
            .to_string_lossy()
            .into_owned()
    );
    fx.close().await;
}

fn fake_cursor_bridge(fx: &Fx, sdk_version: &str) {
    use std::os::unix::fs::PermissionsExt;
    let root = fx.svc.data_dir().join("cursor-bridge");
    std::fs::create_dir_all(root.join("bin")).unwrap();
    let bin = root.join("bin/cursor-sdk-bridge");
    std::fs::write(&bin, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        root.join("manifest.json"),
        format!(r#"{{"sdkVersion":"{sdk_version}"}}"#),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn check_cursor_bridge_notifies_when_behind_pin() {
    let fx = Fx::new().await;
    fake_cursor_bridge(&fx, "0.0.1");
    fx.svc.check_cursor_bridge();
    let n = fx.events.by_name("notify");
    let last = n.last().expect("a notify");
    assert!(
        last.payload_json()
            .contains("Cursor bridge update available"),
        "{}",
        last.payload_json()
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn check_cursor_bridge_silent_when_pinned_or_missing() {
    let fx = Fx::new().await;
    fx.svc.check_cursor_bridge();
    fake_cursor_bridge(&fx, recur_provider_cursor::PINNED_SDK_VERSION);
    fx.svc.check_cursor_bridge();
    assert!(fx.events.by_name("notify").is_empty());
    fx.close().await;
}
