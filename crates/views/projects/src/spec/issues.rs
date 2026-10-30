//! The Issues page: the saved view's session cache and disk copy, the
//! refresh on `issuesStale`, filters and the link menu.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::TestAppContext;
use recur_domain::PrParticipant;
use recur_services::api::{AssignedIssue, IssuesView};
use recur_services::fake::Overrides;
use recur_view_core::ViewName;

use super::harness::with;

fn issues() -> Vec<AssignedIssue> {
    vec![
        AssignedIssue {
            source: "linear".into(),
            id: "lin-1".into(),
            r#ref: "ENG-412".into(),
            title: "Rate-limit the public API per key".into(),
            url: "https://linear.app/acme/issue/ENG-412".into(),
            created_at: "2024-03-01T10:00:00Z".into(),
            updated_at: "2024-06-02T10:00:00Z".into(),
            group: "progress".into(),
            state: "In Progress".into(),
            state_color: "f2c94c".into(),
            state_position: 2.,
            priority: 1,
            priority_label: "Urgent".into(),
            project: "Payments".into(),
            labels: Some(Vec::new()),
            comments: 0,
            assignees: Some(vec![PrParticipant {
                login: "jd".into(),
                ..Default::default()
            }]),
            project_id: "payments".into(),
            linear_project: "Payments".into(),
            linear_team: "ENG".into(),
        },
        AssignedIssue {
            source: "github".into(),
            id: "gh-88".into(),
            r#ref: "acme/storefront#88".into(),
            title: "Checkout button overlaps the cart drawer".into(),
            url: "https://github.com/acme/storefront/issues/88".into(),
            created_at: "2024-02-11T10:00:00Z".into(),
            updated_at: "2024-05-20T10:00:00Z".into(),
            group: "todo".into(),
            state: "Open".into(),
            project: "acme/storefront".into(),
            labels: Some(Vec::new()),
            comments: 1,
            assignees: Some(Vec::new()),
            project_id: "storefront".into(),
            ..Default::default()
        },
    ]
}

struct Counts {
    list: Arc<AtomicUsize>,
    get_view: Arc<AtomicUsize>,
    saved: Arc<parking_lot::Mutex<Vec<IssuesView>>>,
}

fn opts() -> (Overrides, Counts) {
    let c = Counts {
        list: Arc::default(),
        get_view: Arc::default(),
        saved: Arc::default(),
    };
    let (l, g, s, s2) = (
        c.list.clone(),
        c.get_view.clone(),
        c.saved.clone(),
        c.saved.clone(),
    );
    let o = Overrides::default()
        .list_assigned_issues(move |_, ()| {
            l.fetch_add(1, Ordering::SeqCst);
            Ok(issues())
        })
        .get_issues_view(move |_, ()| {
            g.fetch_add(1, Ordering::SeqCst);
            Ok(s2.lock().last().cloned().unwrap_or_default())
        })
        .set_issues_view(move |_, v| {
            s.lock().push(v);
            Ok(())
        });
    (o, c)
}

#[gpui::test]
fn issues_load_on_entry_and_refresh_on_issues_stale(cx: &mut TestAppContext) {
    let (o, c) = opts();
    let mut s = with(cx, |_| {}, o);
    s.click("nav-issues");
    assert_eq!(s.view(), ViewName::Issues);
    assert!(s.until(|s| s.has("iss-row:Rate-limit the public API per key")));
    assert!(s.has("briefing-h:In Progress"));
    assert!(s.has("briefing-h:Open"));
    let n = c.list.load(Ordering::SeqCst);
    s.click("iss-refresh");
    assert!(s.until(|_| c.list.load(Ordering::SeqCst) > n));
    let n = c.list.load(Ordering::SeqCst);
    // Leaving unmounts the list; coming back loads again.
    s.click("nav-inbox");
    assert!(!s.has("v-issues"));
    s.click("nav-issues");
    assert!(s.until(|_| c.list.load(Ordering::SeqCst) > n));
}

#[gpui::test]
fn the_view_is_saved_to_disk_and_kept_for_the_session(cx: &mut TestAppContext) {
    let (o, c) = opts();
    let mut s = with(cx, |_| {}, o);
    s.click("nav-issues");
    assert!(s.until(|s| s.has("iss-layout:list:true")));
    assert_eq!(c.get_view.load(Ordering::SeqCst), 1);
    s.click("iss-layout:board:false");
    assert!(s.has("iss-board"));
    assert!(s.has("iss-card:Checkout button overlaps the cart drawer"));
    assert_eq!(c.saved.lock().last().unwrap().layout, "board");
    s.click("seg:Mine:false");
    assert!(!s.has("iss-card:Checkout button overlaps the cart drawer"));
    assert_eq!(c.saved.lock().last().unwrap().owner, "mine");
    // Back on the route the session copy applies at once, without asking the disk.
    s.click("nav-inbox");
    s.click("nav-issues");
    assert!(s.has("iss-layout:board:true"));
    assert_eq!(c.get_view.load(Ordering::SeqCst), 1);
    // A reload starts from `issues-view.json`.
    s.reload();
    s.click("nav-issues");
    assert!(s.until(|s| s.has("iss-layout:board:true")));
    assert!(s.has("seg:Mine:true"));
    assert_eq!(c.get_view.load(Ordering::SeqCst), 2);
}

#[gpui::test]
fn create_workspace_prefills_new_workspace_with_the_issue(cx: &mut TestAppContext) {
    let (o, _) = opts();
    let mut s = with(cx, |_| {}, o);
    s.click("nav-issues");
    assert!(s.until(|s| s.has("iss-start:https://linear.app/acme/issue/ENG-412=Create workspace")));
    s.click("iss-start:https://linear.app/acme/issue/ENG-412=Create workspace");
    let (open, repo) = s.state.read_with(s.cx, |st, _| {
        (st.overlays.new_task, st.new_task_repo_id.clone())
    });
    assert!(open);
    assert!(repo.is_empty() || repo == "payments");
    assert!(s.has("v-newtask"));
}

#[gpui::test]
fn the_context_menu_links_an_issue_to_a_workspace(cx: &mut TestAppContext) {
    let (o, _) = opts();
    let mut s = with(cx, |_| {}, o);
    s.click("nav-issues");
    assert!(s.until(|s| s.has("iss-row:Checkout button overlaps the cart drawer")));
    s.right_click("iss-row:Checkout button overlaps the cart drawer");
    assert!(s.has("iss-ctx"));
    let target = s.state.read_with(s.cx, |st, _| {
        st.workspaces
            .iter()
            .find(|w| !w.archived && !w.is_main && w.project_id == "storefront")
            .map(|w| {
                let name = if !w.short.is_empty() {
                    w.short.clone()
                } else if !w.branch.is_empty() {
                    w.branch.clone()
                } else {
                    w.id.clone()
                };
                (w.id.clone(), name)
            })
    });
    let (id, name) = target.expect("a storefront workspace");
    s.click(&format!("iss-ctx-ws:{name}:false"));
    assert!(!s.has("iss-ctx"));
    assert!(s.until(|s| s.state.read_with(s.cx, |st, _| {
        st.workspace(&id)
            .and_then(|w| w.issue.as_ref())
            .is_some_and(|r| r.id == "gh-88")
    })));
    s.right_click("iss-row:Checkout button overlaps the cart drawer");
    assert!(s.has("iss-ctx:View workspace"));
    s.keys("escape");
    assert!(!s.has("iss-ctx"));
    s.go(ViewName::Briefing);
}
