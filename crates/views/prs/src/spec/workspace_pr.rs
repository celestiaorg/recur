//! PR detail's Pull / Push / Force push over
//! the linked workspace's land state.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::TestAppContext;
use parking_lot::Mutex;
use recur_services::mock::{MockOptions, Reply, Seed};
use recur_ui::widgets::ToastKind;
use recur_view_core::ViewName;
use serde_json::{Value, json};

use super::harness::*;

const CODING: &str = "LOOP-2229";

fn land() -> Value {
    json!({
        "behindBase": 0, "aheadBase": 1, "dirty": false, "unmerged": false, "base": "main",
        "prId": "gh-91", "prNumber": 91, "prSource": "gh", "aheadRemote": 2, "behindRemote": 0,
    })
}

/// `githubWorkspaceSeed`: storefront opens PRs on GitHub, LOOP-2229 is hand-driven with PR #91.
fn github_seed(state: &'static str) -> impl FnOnce(&mut Seed) {
    move |s| {
        if let Some(p) = s.projects.iter_mut().find(|p| p["id"] == "storefront") {
            p["prOnGithub"] = json!(true);
        }
        let edit = |w: &mut Value| {
            w["type"] = json!("agent");
            w["runStarted"] = json!(false);
            w["state"] = json!("");
        };
        if let Some(o) = s.loop_obj(CODING) {
            edit(s.obj_mut(o));
        }
        if let Some(w) = s.loop_by_id_mut(CODING) {
            edit(w);
        }
        s.insert_pr(json!({
            "id": "gh-91", "workspaceId": CODING, "group": "you", "number": 91, "source": "gh",
            "branch": "recur/pg16", "base": "main", "title": "Migrate analytics to Postgres 16",
            "state": state, "diff": {"add": 1, "del": 0}, "fileCount": 1, "checksOk": true,
            "when": "now", "commits": [], "files": [], "comments": [],
        }));
    }
}

fn land_opts() -> MockOptions {
    MockOptions::default()
        .resolver("WorkspaceLandState", |_, _, _| Reply::value(land()))
        .resolver("FetchWorkingCopyUpstream", |_, _, _| Reply::value(land()))
}

fn open_pr_view(s: &mut S) {
    s.state
        .update(s.cx, |st, cx| st.open_workspace(CODING, false, cx));
    s.settle();
    s.click("tb-view-pr");
    assert_eq!(s.view(), ViewName::PrDetail);
}

#[gpui::test]
fn pr_view_shows_push_updates_when_head_is_ahead_of_the_github_head(cx: &mut TestAppContext) {
    let mut s = boot(
        cx,
        github_seed("In review"),
        land_opts().resolver("OpenWorkspacePR", |_, _, _| Reply::value(json!("gh-91"))),
    );
    open_pr_view(&mut s);
    assert!(s.until(|s| s.has("pr-act:pr-push=Push updates")));
    assert!(s.has("pr-act:pr-merge=Merge"));
    assert!(!s.has("pr-act:pr-pull=Pull updates"));
    assert!(!s.has("pr-act:pr-force-push=Force push"));
    s.click("pr-act:pr-push=Push updates");
    assert!(s.until(|s| s.toasted(ToastKind::Success, "Pushed to PR #91")));
}

#[gpui::test]
fn pr_view_hides_pull_push_force_push_when_the_github_pr_is_closed(cx: &mut TestAppContext) {
    let mut s = boot(cx, github_seed("Closed"), land_opts());
    open_pr_view(&mut s);
    s.wait(200);
    for sel in [
        "pr-act:pr-push=Push updates",
        "pr-act:pr-pull=Pull updates",
        "pr-act:pr-force-push=Force push",
    ] {
        assert!(!s.has(sel) && !s.has(&format!("{sel}:off")), "{sel}");
    }
}

#[gpui::test]
fn pr_view_disables_merge_while_push_updates_is_in_flight(cx: &mut TestAppContext) {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut s = boot(
        cx,
        github_seed("In review"),
        land_opts().resolver("OpenWorkspacePR", move |_, _, _| {
            let (reply, release) = Reply::deferred();
            let _ = tx.send(release);
            reply
        }),
    );
    open_pr_view(&mut s);
    assert!(s.until(|s| s.has("pr-act:pr-push=Push updates")));
    assert!(s.has("pr-act:pr-merge=Merge"));
    s.click("pr-act:pr-push=Push updates");
    assert!(s.has("pr-act:pr-push=Pushing…:off"));
    assert!(s.has("pr-act:pr-merge=Merge:off"));
    rx.try_recv()
        .expect("pushing")
        .send(Reply::value(json!("gh-91")));
    s.settle();
    assert!(s.until(|s| s.toasted(ToastKind::Success, "Pushed to PR #91")));
}

#[gpui::test]
fn pr_view_recalls_push_disabled_until_git_status_lands(cx: &mut TestAppContext) {
    let hold = Arc::new(AtomicBool::new(false));
    let held: Arc<Mutex<Vec<recur_services::mock::Release>>> = Arc::default();
    let answer = |hold: Arc<AtomicBool>, held: Arc<Mutex<Vec<recur_services::mock::Release>>>| {
        move |_: &[Value], _: &mut Seed, _: &recur_services::mock::Table| {
            if !hold.load(Ordering::SeqCst) {
                return Reply::value(land());
            }
            let (reply, release) = Reply::deferred();
            held.lock().push(release);
            reply
        }
    };
    let opts = MockOptions::default()
        .resolver("WorkspaceLandState", answer(hold.clone(), held.clone()))
        .resolver(
            "FetchWorkingCopyUpstream",
            answer(hold.clone(), held.clone()),
        );
    let mut s = boot(cx, github_seed("In review"), opts);
    open_pr_view(&mut s);
    assert!(s.until(|s| s.has("pr-act:pr-push=Push updates")));

    s.click("tb-view-workspace");
    assert_eq!(s.view(), ViewName::Detail);
    assert!(s.has("tb-view-pr"));

    hold.store(true, Ordering::SeqCst);
    s.click("tb-view-pr");
    // The recalled snapshot shows Push, disabled while git answers ("Checking git status…").
    assert!(s.has("pr-act:pr-push=Push updates:off"));
    for r in held.lock().drain(..) {
        r.send(Reply::value(land()));
    }
    s.settle();
    assert!(s.until(|s| s.has("pr-act:pr-push=Push updates")));
}
