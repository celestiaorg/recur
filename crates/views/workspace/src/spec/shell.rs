//! The shell with every view registered, as the desktop mounts it: the workspace titlebar's
//! target branch and git chrome, Remove and the chat dock. Titlebar buttons carry `{id}` and
//! `{id}={label}` (`{id}={label}:off:{title}` while disabled); Selects carry
//! `sel:{aria label}={value}` (`:off` while disabled).

use std::sync::Arc;
use std::time::Duration;

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, px};
use gpui_kit::{Bounds, Focusable, Pixels};
use parking_lot::Mutex;
use recur_app::{Shell, ShellOptions};
use recur_services::mock::{MockBridge, MockOptions, Release, Reply, Seed};
use recur_ui::widgets::{ToastKind, ToastStore};
use recur_view_core::{AppState, LocalStorage, ViewContext, ViewName, ViewRegistry};
use serde_json::{Value, json};

use super::harness::install_prompt_runner;

const SETTLED: &str = "LOOP-2224";
const CODING: &str = "LOOP-2229";
const ACTIVE: &str = "LOOP-2231";

pub struct S<'a> {
    pub state: Entity<AppState>,
    pub bridge: Arc<MockBridge>,
    pub cx: &'a mut VisualTestContext,
}

pub fn boot<'a>(
    cx: &'a mut TestAppContext,
    seed: impl FnOnce(&mut Seed),
    opts: MockOptions,
    storage: impl FnOnce(&mut LocalStorage),
) -> S<'a> {
    let mut registry = ViewRegistry::default();
    recur_view_chat::register(&mut registry);
    recur_view_review::register(&mut registry);
    recur_view_observatory::register(&mut registry);
    crate::register(&mut registry);
    cx.update(|cx| {
        recur_app::init(&registry, cx);
        recur_ui::motion::freeze(None, cx);
        recur_content::markdown::set_markdown_render_inline(true);
        install_prompt_runner(cx);
    });
    let mut sd = recur_services::mock::fixtures::seed();
    seed(&mut sd);
    let bridge = MockBridge::new(sd, opts);
    let mut st = LocalStorage::in_memory();
    storage(&mut st);
    let services = bridge.services();
    let state = cx.update(|cx| AppState::install(services.clone(), st, cx));
    let ctx = ViewContext {
        state: state.clone(),
        services,
    };
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        Shell::new(ctx, registry, ShellOptions::default(), window, cx)
    });
    cx.simulate_resize(gpui::size(px(1280.), px(800.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.activate_window();
        let f = shell.focus_handle(cx);
        window.focus(&f, cx);
    });
    let mut s = S { state, bridge, cx };
    s.settle();
    s
}

fn plain(cx: &mut TestAppContext, opts: MockOptions) -> S<'_> {
    boot(cx, |_| {}, opts, |_| {})
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

impl S<'_> {
    /// Runs to idle, delivering frames (and their `on_next_frame` work) as the platform would.
    pub fn settle(&mut self) {
        for _ in 0..4 {
            self.cx.run_until_parked();
            self.cx.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.refresh();
            });
        }
        self.cx.run_until_parked();
    }

    pub fn bounds(&mut self, sel: &str) -> Option<Bounds<Pixels>> {
        self.settle();
        self.cx.debug_bounds(leak(sel))
    }

    pub fn has(&mut self, sel: &str) -> bool {
        self.bounds(sel).is_some()
    }

    pub fn click(&mut self, sel: &str) {
        let p = self
            .bounds(sel)
            .unwrap_or_else(|| panic!("no element {sel}"))
            .center();
        self.cx.simulate_mouse_move(p, None, Modifiers::none());
        self.cx.simulate_click(p, Modifiers::none());
        self.settle();
    }

    pub fn keys(&mut self, keys: &str) {
        self.cx.simulate_keystrokes(keys);
        self.settle();
    }

    pub fn wait(&mut self, ms: u64) {
        self.cx.executor().advance_clock(Duration::from_millis(ms));
        self.settle();
    }

    /// Lets frames pass until `f` holds (a resolver's `await started`).
    pub fn until(&mut self, f: impl Fn() -> bool) {
        for _ in 0..20 {
            if f() {
                return;
            }
            self.wait(16);
        }
    }

    /// `openWorkspaceByGoal`: the sidebar row.
    pub fn open(&mut self, id: &str) {
        self.click(&format!("hit:{id}"));
    }

    /// The project's working-copy row (`.rail button.tmain`).
    pub fn open_main(&mut self, project: &str) {
        self.click(&format!("main:{project}"));
        assert!(self.has("tb-back"));
    }

    pub fn view(&mut self) -> ViewName {
        self.state.read_with(self.cx, |s, _| s.view)
    }

    pub fn wire(&mut self, name: &str, data: Value) {
        self.bridge.dispatch_wire(name, data).unwrap();
        self.settle();
    }

    /// Still listed (not archived) on the backend.
    pub fn present(&mut self, id: &str) -> bool {
        self.bridge
            .with_seed(|s| s.loop_by_id(id).is_some_and(|l| l["archived"] != true))
    }

    pub fn toasted(&mut self, kind: ToastKind, text: &str) -> bool {
        self.cx.update(|_, cx| {
            ToastStore::global(cx)
                .read(cx)
                .toasts()
                .iter()
                .any(|t| t.kind == kind && t.text.contains(text))
        })
    }

    pub fn success(&mut self, text: &str) -> bool {
        self.toasted(ToastKind::Success, text)
    }
}

/// Replies held while `hold` is on (a resolver's `await gate`), answered on `release`.
#[derive(Clone, Default)]
struct Gate(Arc<Mutex<Held>>);

/// Whether replies are held, and the held ones with their answers.
type Held = (bool, Vec<(Release, Value)>);

impl Gate {
    fn hold(&self) {
        self.0.lock().0 = true;
    }

    fn reply(&self, v: Value) -> Reply {
        let mut g = self.0.lock();
        if !g.0 {
            return Reply::value(v);
        }
        let (r, release) = Reply::deferred();
        g.1.push((release, v));
        r
    }

    fn release(&self) {
        let held: Vec<_> = {
            let mut g = self.0.lock();
            g.0 = false;
            g.1.drain(..).collect()
        };
        for (r, v) in held {
            r.send(Reply::value(v));
        }
    }
}

/// `WorkspaceLandState` and `FetchWorkingCopyUpstream` both answer `land`.
fn land(land: Value) -> MockOptions {
    let l2 = land.clone();
    MockOptions::default()
        .resolver("WorkspaceLandState", move |_, _, _| {
            Reply::value(land.clone())
        })
        .resolver("FetchWorkingCopyUpstream", move |_, _, _| {
            Reply::value(l2.clone())
        })
}

/// A land the test edits as it goes (`({ ...land })`).
fn land_cell(land: Arc<Mutex<Value>>) -> MockOptions {
    let l2 = land.clone();
    MockOptions::default()
        .resolver("WorkspaceLandState", move |_, _, _| {
            Reply::value(land.lock().clone())
        })
        .resolver("FetchWorkingCopyUpstream", move |_, _, _| {
            Reply::value(l2.lock().clone())
        })
}

fn edit_ws(s: &mut Seed, id: &str, f: impl Fn(&mut Value)) {
    if let Some(o) = s.find_loop(|l| l["id"] == id) {
        f(s.obj_mut(o));
    }
    if let Some(w) = s.loop_by_id_mut(id) {
        f(w);
    }
}

struct GhPr {
    id: &'static str,
    number: i64,
    source: &'static str,
    state: &'static str,
    group: &'static str,
}

fn gh(id: &'static str, number: i64, source: &'static str) -> GhPr {
    GhPr {
        id,
        number,
        source,
        state: "In review",
        group: "you",
    }
}

/// `githubWorkspaceSeed`: storefront opens PRs on GitHub and LOOP-2229 is hand-driven, with
/// an optional PR linked to it.
fn github_seed(pr: Option<GhPr>) -> impl FnOnce(&mut Seed) {
    move |s| {
        if let Some(p) = s.projects.iter_mut().find(|p| p["id"] == "storefront") {
            p["prOnGithub"] = json!(true);
        }
        edit_ws(s, CODING, |w| {
            w["type"] = json!("agent");
            w["runStarted"] = json!(false);
            w["state"] = json!("");
        });
        if let Some(pr) = pr {
            s.insert_pr(pr_record(pr.id, pr.number, pr.source, pr.state, pr.group));
        }
    }
}

fn pr_record(id: &str, number: i64, source: &str, state: &str, group: &str) -> Value {
    json!({
        "id": id, "workspaceId": CODING, "group": group, "number": number, "source": source,
        "branch": "recur/pg16", "base": "main", "title": "Migrate analytics to Postgres 16",
        "state": state, "diff": {"add": 1, "del": 0}, "fileCount": 1, "checksOk": true,
        "when": "now", "commits": [], "files": [], "comments": [],
    })
}

fn seed_ws(id: &str) -> Value {
    recur_services::mock::fixtures::base_seed()
        .loop_by_id(id)
        .cloned()
        .unwrap_or(Value::Null)
}

fn str_of(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
}

const PICKER: &str = "sel:Target branch";

// ---- workspace target branch ----

#[gpui::test]
async fn loop_workspaces_show_name_and_branch_not_the_id(cx: &mut TestAppContext) {
    let mut s = plain(cx, MockOptions::default());
    s.open(CODING);
    let w = seed_ws(CODING);
    let (short, branch) = (str_of(&w, "short"), str_of(&w, "branch"));
    assert!(!branch.is_empty());
    let crumb = format!("crumb=/ storefront / {short} {branch}");
    assert!(s.has(&crumb), "{crumb}");
    assert!(!crumb.contains(CODING));
    assert!(s.has("crumb-base"));
}

#[gpui::test]
async fn titlebar_offers_a_target_branch_picker_that_can_change_after_creation(
    cx: &mut TestAppContext,
) {
    let mut s = plain(cx, MockOptions::default());
    s.open(CODING);
    // Seeded loops inherit the project default (main) when baseBranch is unset.
    assert!(s.has(&format!("{PICKER}=main")));
    s.click("crumb-base");
    assert!(s.has("sel-menu"));
    assert!(s.has("sel-opt:develop"));
    s.click("sel-opt:develop");
    assert!(s.success("Target branch is now develop"));
    assert!(s.has(&format!("{PICKER}=develop")));
}

fn branches_for(seed: &Seed, project: &str) -> Value {
    let base = seed
        .projects
        .iter()
        .find(|p| p["id"] == project)
        .map(|p| str_of(p, "baseBranch"))
        .unwrap_or_default();
    if base.is_empty() {
        return json!([]);
    }
    let mut names = vec![base, "origin/main".to_string(), "develop".to_string()];
    names.dedup();
    let remote = names
        .iter()
        .filter(|n| n.starts_with("origin/"))
        .map(|n| json!({"name": n, "group": "origin"}));
    let local = names
        .iter()
        .filter(|n| !n.starts_with("origin/"))
        .map(|n| json!({"name": n, "group": "Local"}));
    Value::Array(remote.chain(local).collect())
}

#[gpui::test]
async fn target_branch_picker_does_not_refetch_on_workspace_updated(cx: &mut TestAppContext) {
    let calls = Arc::new(Mutex::new(0usize));
    let c = calls.clone();
    let opts = MockOptions::default().resolver("ListBranches", move |a, s, _| {
        *c.lock() += 1;
        Reply::value(branches_for(s, a[0].as_str().unwrap_or_default()))
    });
    let mut s = plain(cx, opts);
    s.open(CODING);
    assert!(s.has(&format!("{PICKER}=main")), "enabled, on main");
    let after_load = *calls.lock();
    s.wire(
        "workspace.updated",
        json!({"workspaceId": CODING, "diff": {"add": 4, "del": 1}}),
    );
    s.wire(
        "workspace.event",
        json!({"workspaceId": CODING, "event": {"text": "edited a file"}}),
    );
    let add = s.state.read_with(s.cx, |st, _| {
        st.workspaces
            .iter()
            .find(|w| w.id == CODING)
            .map(|w| w.diff.add)
    });
    assert_eq!(add, Some(4), "the store applied workspace.updated");
    assert!(s.has(&format!("{PICKER}=main")));
    assert_eq!(
        *calls.lock(),
        after_load,
        "ListBranches must not re-run when workspace.updated replaces the row"
    );
}

#[gpui::test]
async fn target_branch_picker_stays_disabled_until_the_new_projects_branches_land(
    cx: &mut TestAppContext,
) {
    let gate = Gate::default();
    let g = gate.clone();
    let opts = MockOptions::default().resolver("ListBranches", move |a, _, _| {
        if a[0] == "payments" {
            return g.reply(
                json!([{"name": "release", "group": "Local"}, {"name": "main", "group": "Local"}]),
            );
        }
        Reply::value(json!([
            {"name": "main", "group": "Local"},
            {"name": "develop", "group": "Local"},
            {"name": "feature/auth", "group": "Local"},
        ]))
    });
    let mut s = plain(cx, opts);
    s.open(CODING);
    assert!(s.has(&format!("{PICKER}=main")));
    s.click("crumb-base");
    assert!(s.has("sel-opt:feature/auth"));
    s.keys("escape");
    assert!(!s.has("sel-menu"));

    gate.hold();
    s.open("LOOP-2218");
    assert!(
        s.has(&format!("{PICKER}=main:off")),
        "disabled while payments loads"
    );
    gate.release();
    s.settle();
    assert!(s.has(&format!("{PICKER}=main")));
    s.click("crumb-base");
    assert!(s.has("sel-opt:release"));
    assert!(!s.has("sel-opt:feature/auth"));
}

#[gpui::test]
async fn switching_workspaces_in_the_same_project_reloads_branch_choices(cx: &mut TestAppContext) {
    let extra = Arc::new(Mutex::new(Vec::<Value>::new()));
    let calls = Arc::new(Mutex::new(0usize));
    let (e, c) = (extra.clone(), calls.clone());
    let opts = MockOptions::default().resolver("ListBranches", move |_, _, _| {
        *c.lock() += 1;
        let mut list = vec![
            json!({"name": "main", "group": "Local"}),
            json!({"name": "develop", "group": "Local"}),
        ];
        list.extend(e.lock().iter().cloned());
        Reply::value(Value::Array(list))
    });
    let mut s = plain(cx, opts);
    s.open(CODING);
    assert!(s.has(&format!("{PICKER}=main")));
    let after_load = *calls.lock();
    *extra.lock() = vec![json!({"name": "feature/new-base", "group": "Local"})];
    s.open("LOOP-2235");
    assert!(s.has(&format!("{PICKER}=main")));
    assert!(
        *calls.lock() > after_load,
        "ListBranches must re-run when switching workspaces in the same project"
    );
    s.click("crumb-base");
    assert!(s.has("sel-opt:feature/new-base"));
}

#[gpui::test]
async fn the_main_working_copy_has_no_target_branch_picker(cx: &mut TestAppContext) {
    let mut s = plain(cx, MockOptions::default());
    s.open_main("storefront");
    assert!(!s.has("crumb-base"));
}

// ---- main working copy titlebar git actions ----

fn main_land(behind: i64, ahead: i64, dirty: bool) -> Value {
    json!({
        "behindBase": behind, "aheadBase": ahead, "dirty": dirty, "unmerged": false,
        "base": "origin/main", "remote": "origin",
    })
}

#[gpui::test]
async fn shows_commit_when_the_checkout_is_dirty(cx: &mut TestAppContext) {
    let mut s = plain(cx, land(main_land(0, 0, true)));
    s.open_main("storefront");
    assert!(s.has("tb-commit=Commit"));
    assert!(!s.has("tb-remove"));
}

#[gpui::test]
async fn recalls_commit_disabled_until_git_status_lands_after_a_switch(cx: &mut TestAppContext) {
    let gate = Gate::default();
    let land_for = {
        let gate = gate.clone();
        move |a: &[Value], s: &mut Seed| {
            let id = a[0].as_str().unwrap_or_default();
            let storefront_main = s
                .loop_by_id(id)
                .is_some_and(|w| w["isMain"] == true && w["projectId"] == "storefront");
            gate.reply(main_land(0, 0, storefront_main))
        }
    };
    let l2 = land_for.clone();
    let opts = MockOptions::default()
        .resolver("WorkspaceLandState", move |a, s, _| land_for(a, s))
        .resolver("FetchWorkingCopyUpstream", move |a, s, _| l2(a, s));
    let mut s = plain(cx, opts);
    s.open_main("storefront");
    assert!(s.has("tb-commit=Commit"), "visible and enabled");

    s.open_main("payments");
    assert!(!s.has("tb-commit"));

    gate.hold();
    s.open_main("storefront");
    assert!(
        s.has("tb-commit=Commit:off:Checking git status…"),
        "recalled from the snapshot, disabled while git answers"
    );
    gate.release();
    s.settle();
    assert!(s.has("tb-commit=Commit"));
}

#[gpui::test]
async fn shows_update_from_the_tracking_branch_when_the_remote_is_ahead(cx: &mut TestAppContext) {
    let mut s = plain(cx, land(main_land(2, 0, false)));
    s.open_main("storefront");
    assert!(s.has("tb-update=Update from origin/main"));
}

fn fs_changed_clears_update(s: &mut S, land: &Arc<Mutex<Value>>, id: &str, label: &str) {
    assert!(s.has(&format!("tb-update={label}")));
    land.lock()["behindBase"] = json!(0);
    s.wire("workspace.fs_changed", json!({"workspaceId": id}));
    s.wait(100);
    assert!(!s.has("tb-update"));
}

#[gpui::test]
async fn clears_update_after_an_external_git_pull(cx: &mut TestAppContext) {
    let cell = Arc::new(Mutex::new(main_land(2, 0, false)));
    let mut s = plain(cx, land_cell(cell.clone()));
    s.open_main("storefront");
    let id = s
        .state
        .read_with(s.cx, |st, _| st.selected_workspace_id.clone());
    fs_changed_clears_update(&mut s, &cell, &id, "Update from origin/main");
}

#[gpui::test]
async fn shows_push_to_the_remote_when_local_is_ahead_and_clean(cx: &mut TestAppContext) {
    let mut s = plain(cx, land(main_land(0, 1, false)));
    s.open_main("storefront");
    assert!(s.has("tb-push=Push to origin"));
    s.click("tb-push=Push to origin");
    assert!(s.success("Pushed to origin"));
}

#[gpui::test]
async fn shows_commit_and_push_together_when_ahead_with_leftover_dirty_files(
    cx: &mut TestAppContext,
) {
    let mut s = plain(cx, land(main_land(0, 1, true)));
    s.open_main("storefront");
    assert!(s.has("tb-commit=Commit"));
    assert!(s.has("tb-push=Push to origin"));
    s.click("tb-push=Push to origin");
    assert!(s.success("Pushed to origin"));
}

#[gpui::test]
async fn update_from_origin_offers_merge_and_rebase(cx: &mut TestAppContext) {
    let mut s = plain(cx, land(main_land(2, 0, false)));
    s.open_main("storefront");
    assert!(s.has("tb-update=Update from origin/main"));
    s.click("tb-update-chev");
    assert!(s.has("update-merge"));
    assert!(s.has("update-rebase"));
    s.click("update-rebase");
    assert!(s.has("tb-update=Rebase onto origin/main"));
}

#[gpui::test]
async fn shows_commit_and_update_together_when_behind_with_leftover_dirty_files(
    cx: &mut TestAppContext,
) {
    let mut s = plain(cx, land(main_land(2, 0, true)));
    s.open_main("storefront");
    assert!(s.has("tb-commit=Commit"));
    assert!(s.has("tb-update=Update from origin/main"));
}

// ---- workspace GitHub PR titlebar ----

fn ws_land(extra: Value) -> Value {
    let mut l =
        json!({"behindBase": 0, "aheadBase": 1, "dirty": false, "unmerged": false, "base": "main"});
    if let (Some(l), Some(e)) = (l.as_object_mut(), extra.as_object()) {
        l.extend(e.clone());
    }
    l
}

fn gh_land(ahead_remote: i64, behind_remote: i64) -> Value {
    ws_land(json!({
        "prId": "gh-91", "prNumber": 91, "prSource": "gh",
        "aheadRemote": ahead_remote, "behindRemote": behind_remote,
    }))
}

fn gh_boot(cx: &mut TestAppContext, pr: Option<GhPr>, opts: MockOptions) -> S<'_> {
    let mut s = boot(cx, github_seed(pr), opts, |_| {});
    s.open(CODING);
    s
}

#[gpui::test]
async fn workspace_behind_its_base_offers_merge_rebase_update_and_auto_pushes_the_github_pr(
    cx: &mut TestAppContext,
) {
    let l = ws_land(json!({
        "behindBase": 2, "prId": "gh-91", "prNumber": 91, "prSource": "gh",
        "aheadRemote": 0, "behindRemote": 0,
    }));
    let opts = land(l).resolver("UpdateWorkspaceWithBase", |_, _, _| {
        Reply::value(json!({
            "status": "updated", "workspace": {}, "base": "main", "pushedPR": 91, "forcePushed": true,
        }))
    });
    let mut s = gh_boot(cx, Some(gh("gh-91", 91, "gh")), opts);
    assert!(s.has("tb-update=Update with main"));
    assert!(s.has("tb-view-pr=View PR #91"));
    s.click("tb-update-chev");
    assert!(s.has("update-merge"));
    s.click("update-rebase");
    assert!(s.has("tb-update=Rebase onto main"));
    s.click("tb-update=Rebase onto main");
    assert!(s.success("Updated with latest main and force-pushed to PR #91"));
}

#[gpui::test]
async fn clears_update_with_base_after_an_external_git_pull(cx: &mut TestAppContext) {
    let cell = Arc::new(Mutex::new(ws_land(json!({"behindBase": 2}))));
    let mut s = gh_boot(cx, None, land_cell(cell.clone()));
    fs_changed_clears_update(&mut s, &cell, CODING, "Update with main");
}

#[gpui::test]
async fn shows_open_pr_before_a_pull_request_exists(cx: &mut TestAppContext) {
    let mut s = gh_boot(cx, None, land(ws_land(json!({}))));
    assert!(s.has("tb-open-pr=Open PR"));
    assert!(s.has("tb-open-pr-chev"));
    assert!(!s.has("tb-view-pr"));
    assert!(!s.has("tb-push-pr"));
    assert!(s.has("tb-merge"));
}

fn remote_land(base: &str, require_pr: bool) -> Value {
    ws_land(json!({"base": base, "remote": "origin", "requirePR": require_pr}))
}

#[gpui::test]
async fn hides_merge_when_github_requires_a_pull_request_on_the_remote_target(
    cx: &mut TestAppContext,
) {
    let mut s = gh_boot(cx, None, land(remote_land("origin/main", true)));
    assert!(!s.has("tb-merge"));
    assert!(s.has("tb-open-pr=Open PR"));
}

#[gpui::test]
async fn shows_merge_on_a_github_pr_remote_when_github_allows_a_direct_land(
    cx: &mut TestAppContext,
) {
    let mut s = gh_boot(cx, None, land(remote_land("origin/main", false)));
    assert!(s.has("tb-merge"));
    assert!(s.has("tb-open-pr=Open PR"));
}

#[gpui::test]
async fn hides_merge_when_a_background_github_branch_rules_probe_starts_requiring_a_pr(
    cx: &mut TestAppContext,
) {
    let mut s = gh_boot(cx, None, land(remote_land("origin/main", false)));
    assert!(s.has("tb-merge"));
    s.wire(
        "github.branch_rules",
        json!({"projectId": "storefront", "branch": "main", "requirePR": true}),
    );
    assert!(!s.has("tb-merge"));
}

#[gpui::test]
async fn hides_merge_for_a_slash_containing_base_branch_when_github_requires_a_pr(
    cx: &mut TestAppContext,
) {
    let mut s = gh_boot(cx, None, land(remote_land("origin/release/1.0", false)));
    assert!(s.has("tb-merge"));
    s.wire(
        "github.branch_rules",
        json!({"projectId": "storefront", "branch": "1.0", "remote": "origin", "requirePR": true}),
    );
    assert!(s.has("tb-merge"));
    s.wire(
        "github.branch_rules",
        json!({"projectId": "storefront", "branch": "release/1.0", "remote": "origin", "requirePR": true}),
    );
    assert!(!s.has("tb-merge"));
}

fn rules_main(s: &mut S) {
    s.wire(
        "github.branch_rules",
        json!({"projectId": "storefront", "branch": "main", "remote": "origin", "requirePR": true}),
    );
}

#[gpui::test]
async fn keeps_a_branch_rules_event_when_an_in_flight_land_returns_a_stale_require_pr(
    cx: &mut TestAppContext,
) {
    let stale = remote_land("origin/main", false);
    let held: Arc<Mutex<Vec<Release>>> = Default::default();
    let (st, h) = (stale.clone(), held.clone());
    let opts = MockOptions::default()
        .resolver("WorkspaceLandState", move |_, _, _| {
            Reply::value(st.clone())
        })
        .resolver("FetchWorkingCopyUpstream", move |_, _, _| {
            let (r, release) = Reply::deferred();
            h.lock().push(release);
            r
        });
    let mut s = gh_boot(cx, None, opts);
    assert!(s.has("tb-merge"));
    s.until(|| !held.lock().is_empty());
    assert!(!held.lock().is_empty(), "the upstream fetch started");
    rules_main(&mut s);
    assert!(!s.has("tb-merge"));
    for r in held.lock().drain(..) {
        r.send(Reply::value(stale.clone()));
    }
    s.settle();
    assert!(!s.has("tb-merge"));
}

#[gpui::test]
async fn keeps_a_branch_rules_event_that_arrives_before_the_first_land_resolves(
    cx: &mut TestAppContext,
) {
    let stale = remote_land("origin/main", false);
    let held: Arc<Mutex<Vec<Release>>> = Default::default();
    let (h1, h2) = (held.clone(), held.clone());
    let opts = MockOptions::default()
        .resolver("WorkspaceLandState", move |_, _, _| {
            let (r, release) = Reply::deferred();
            h1.lock().push(release);
            r
        })
        .resolver("FetchWorkingCopyUpstream", move |_, _, _| {
            let (r, release) = Reply::deferred();
            h2.lock().push(release);
            r
        });
    let mut s = gh_boot(cx, None, opts);
    s.until(|| held.lock().len() >= 2);
    assert!(
        held.lock().len() >= 2,
        "land and upstream fetch both started"
    );
    rules_main(&mut s);
    for r in held.lock().drain(..) {
        r.send(Reply::value(stale.clone()));
    }
    s.settle();
    assert!(s.has("tb-open-pr=Open PR"), "enabled");
    assert!(!s.has("tb-merge"));
}

#[gpui::test]
async fn open_pr_split_offers_open_draft_pr(cx: &mut TestAppContext) {
    let mut s = gh_boot(cx, None, land(ws_land(json!({}))));
    s.click("tb-open-pr-chev");
    assert!(s.has("open-draft-pr-item"));
    s.click("open-draft-pr-item");
    assert!(!s.has("open-draft-pr-item"));
}

#[gpui::test]
async fn shows_view_pr_after_a_pull_request_is_open(cx: &mut TestAppContext) {
    let mut s = gh_boot(cx, Some(gh("gh-91", 91, "gh")), land(gh_land(0, 0)));
    assert!(!s.has("tb-open-pr"));
    s.click("tb-view-pr=View PR #91");
    assert_eq!(s.view(), ViewName::PrDetail);
    s.click("tb-view-workspace=View workspace");
    assert_eq!(s.view(), ViewName::Detail);
    assert!(s.has("v-detail"));
    assert!(s.has("tb-view-pr=View PR #91"));
}

#[gpui::test]
async fn shows_view_draft_when_land_pr_draft_is_set(cx: &mut TestAppContext) {
    let mut l = gh_land(0, 0);
    l["prDraft"] = json!(true);
    let mut s = gh_boot(cx, Some(gh("gh-91", 91, "gh")), land(l));
    assert!(s.has("tb-view-pr=View draft #91"));
    assert!(!s.has("tb-view-pr=View PR #91"));
    // The dotted draft glyph, not the solid PR icon.
    let pr = recur_domain::Pr {
        draft: true,
        ..Default::default()
    };
    assert_eq!(
        recur_view_core::pr::pr_icon_name(Some(&pr)),
        recur_ui::IconName::GitPullRequestDraft
    );
}

#[gpui::test]
async fn keeps_view_pr_after_the_github_pr_is_merged(cx: &mut TestAppContext) {
    let seed = |s: &mut Seed| {
        github_seed(Some(GhPr {
            state: "Merged",
            group: "merged",
            ..gh("gh-91", 91, "gh")
        }))(s);
        edit_ws(s, CODING, |w| w["state"] = json!("merged"));
        let leftover = s.alloc(pr_record("4", 4, "local", "In review", "you"));
        let mut ids = vec![leftover];
        ids.extend(s.pr_ids().iter().copied());
        s.set_prs(ids);
        s.set_pr_by_id("4", leftover);
    };
    let l =
        json!({"behindBase": 0, "aheadBase": 0, "dirty": false, "unmerged": false, "base": "main"});
    let mut s = boot(cx, seed, land(l), |_| {});
    s.open(CODING);
    assert!(s.has("tb-view-pr=View PR #91"));
    assert!(!s.has("tb-open-pr"));
}

#[gpui::test]
async fn shows_push_updates_next_to_view_pr_when_head_is_ahead_of_the_github_head(
    cx: &mut TestAppContext,
) {
    let opts =
        land(gh_land(2, 0)).resolver("OpenWorkspacePR", |_, _, _| Reply::value(json!("gh-91")));
    let mut s = gh_boot(cx, Some(gh("gh-91", 91, "gh")), opts);
    assert!(!s.has("tb-open-pr"));
    assert!(s.has("tb-view-pr=View PR #91"));
    assert!(s.has("tb-push-pr=Push updates"));
    assert!(!s.has("tb-pull-pr"));
    assert!(!s.has("tb-force-push-pr"));
    s.click("tb-push-pr=Push updates");
    assert!(s.success("Pushed to PR #91"));
}

#[gpui::test]
async fn shows_pull_updates_next_to_view_pr_when_github_is_ahead_of_head(cx: &mut TestAppContext) {
    let opts = land(gh_land(0, 2)).resolver("PullWorkspacePR", |_, _, _| {
        Reply::value(json!({"status": "updated", "workspace": {}, "base": "origin/recur/pg16"}))
    });
    let mut s = gh_boot(cx, Some(gh("gh-91", 91, "gh")), opts);
    assert!(!s.has("tb-open-pr"));
    assert!(s.has("tb-view-pr=View PR #91"));
    assert!(s.has("tb-pull-pr=Pull updates"));
    assert!(!s.has("tb-push-pr"));
    assert!(!s.has("tb-force-push-pr"));
    s.click("tb-pull-pr=Pull updates");
    assert!(s.success("Pulled updates from PR #91"));
}

#[gpui::test]
async fn hides_open_pr_when_the_workspace_has_no_worktree(cx: &mut TestAppContext) {
    let seed = |s: &mut Seed| {
        github_seed(None)(s);
        edit_ws(s, CODING, |w| {
            w["worktreePath"] = json!("");
            w["phase"] = json!("failed");
            w["lastError"] = json!("allocate worktree: bad base");
            w["statusLabel"] = json!("Couldn't create");
        });
    };
    let mut s = boot(cx, seed, land(ws_land(json!({}))), |_| {});
    s.open(CODING);
    assert!(!s.has("tb-open-pr"));
}

#[gpui::test]
async fn shows_view_pr_and_open_pr_when_the_existing_pr_is_only_local(cx: &mut TestAppContext) {
    let l = ws_land(json!({
        "prId": "7", "prNumber": 7, "prSource": "local", "aheadRemote": 0, "behindRemote": 0,
    }));
    let mut s = gh_boot(cx, Some(gh("7", 7, "local")), land(l));
    assert!(s.has("tb-open-pr=Open PR"));
    assert!(s.has("tb-view-pr=View PR #7"));
}

#[gpui::test]
async fn shows_pull_updates_and_force_push_when_the_github_head_has_diverged(
    cx: &mut TestAppContext,
) {
    let opts = land(gh_land(2, 2))
        .resolver("OpenWorkspacePR", |_, _, _| Reply::value(json!("gh-91")))
        .resolver("PullWorkspacePR", |_, _, _| {
            Reply::value(json!({"status": "updated", "workspace": {}, "base": "origin/recur/pg16"}))
        });
    let mut s = gh_boot(cx, Some(gh("gh-91", 91, "gh")), opts);
    assert!(s.has("tb-view-pr=View PR #91"));
    assert!(s.has("tb-pull-pr=Pull updates"));
    assert!(s.has("tb-force-push-pr=Force push"));
    assert!(!s.has("tb-push-pr"));
    s.click("tb-force-push-pr=Force push");
    assert!(s.success("Force-pushed to PR #91"));
}

#[gpui::test]
async fn hides_pull_push_force_push_when_the_github_pr_is_already_in_sync(cx: &mut TestAppContext) {
    let mut s = gh_boot(cx, Some(gh("gh-91", 91, "gh")), land(gh_land(0, 0)));
    assert!(s.has("tb-view-pr=View PR #91"));
    assert!(!s.has("tb-pull-pr"));
    assert!(!s.has("tb-push-pr"));
    assert!(!s.has("tb-force-push-pr"));
}

// ---- remove ----

fn agent(s: &mut S, ws: &str, status: &str) {
    s.wire(
        "agent.updated",
        json!({"workspaceId": ws, "agentId": format!("{ws}-MA"), "status": status}),
    );
}

#[gpui::test]
async fn a_settled_loop_offers_remove_a_running_loop_shows_it_disabled(cx: &mut TestAppContext) {
    let mut s = plain(cx, MockOptions::default());
    s.open(SETTLED);
    assert!(s.has("tb-remove"));
    s.click("tb-back");
    s.open(CODING);
    assert!(s.has("tb-remove"));
    s.click("tb-remove");
    s.click("tb-remove");
    assert!(s.present(CODING), "a disabled Remove never confirms");
    assert_eq!(s.view(), ViewName::Detail);
}

#[gpui::test]
async fn an_armed_remove_disarms_when_an_agent_turn_starts(cx: &mut TestAppContext) {
    let mut s = plain(cx, MockOptions::default());
    s.open(SETTLED);
    s.click("tb-remove");
    agent(&mut s, SETTLED, "working");
    s.click("tb-remove");
    assert!(s.present(SETTLED), "disabled while the turn runs");
    agent(&mut s, SETTLED, "idle");
    s.click("tb-remove");
    assert!(
        s.present(SETTLED),
        "the turn disarmed it: this click only arms again"
    );
}

#[gpui::test]
async fn remove_requires_a_second_click_then_hides_the_loop_and_returns_to_inbox(
    cx: &mut TestAppContext,
) {
    let mut s = plain(cx, MockOptions::default());
    s.open(SETTLED);
    s.click("tb-remove");
    assert!(s.present(SETTLED));
    s.click("tb-remove");
    assert!(s.success("Workspace removed"));
    assert_eq!(s.view(), ViewName::Briefing);
    assert!(!s.present(SETTLED));
    assert!(!s.has(&format!("hit:{SETTLED}")));
}

#[gpui::test]
async fn remove_confirms_on_a_double_click(cx: &mut TestAppContext) {
    let mut s = plain(cx, MockOptions::default());
    s.open(SETTLED);
    let p = s.bounds("tb-remove").expect("remove").center();
    s.cx.simulate_mouse_move(p, None, Modifiers::none());
    s.cx.simulate_click(p, Modifiers::none());
    s.cx.simulate_click(p, Modifiers::none());
    s.settle();
    assert!(s.success("Workspace removed"));
    assert_eq!(s.view(), ViewName::Briefing);
    assert!(!s.present(SETTLED));
    assert!(!s.has(&format!("hit:{SETTLED}")));
}

// ---- chat dock ----

#[gpui::test]
async fn hiding_the_left_panel_still_works_after_visiting_the_linked_pr(cx: &mut TestAppContext) {
    let opts = MockOptions::default().resolver("ListLoopAgents", |_, _, _| Reply::value(json!([])));
    let mut s = plain(cx, opts);
    s.open(ACTIVE);
    assert!(s.has("obs-left"));
    s.click("tb-view-pr=View PR #482");
    assert_eq!(s.view(), ViewName::PrDetail);
    s.click("tb-view-workspace=View workspace");
    assert_eq!(s.view(), ViewName::Detail);
    assert!(s.has("obs-left"));
    s.click("obs-left-toggle");
    assert!(!s.has("obs-left"));
    s.click("obs-left-toggle");
    assert!(s.has("obs-left"));
}

// ---- escalated loops: the question is an inline card in the Loop chat ----

const ESCALATED: &str = "LOOP-2218";

fn answers(calls: &Arc<Mutex<Vec<Vec<Value>>>>) -> MockOptions {
    let log = calls.clone();
    MockOptions::default().resolver("AnswerEscalation", move |a, _, _| {
        log.lock().push(a.to_vec());
        Reply::value(json!({}))
    })
}

fn me(s: &mut S, body: &str) -> bool {
    s.has(&format!("me:{}", recur_view_chat::chat::debug_key(body)))
}

#[gpui::test]
async fn opens_an_escalated_loop_into_loop_chat_with_an_inline_question(cx: &mut TestAppContext) {
    let calls: Arc<Mutex<Vec<Vec<Value>>>> = Arc::default();
    let mut s = plain(cx, answers(&calls));
    s.open(ESCALATED);
    assert!(s.has("ask-kicker:Maestro needs a decision"));
    s.click("ask-opt:Token bucket with refill");
    assert!(
        !s.has("ask-kicker:Maestro needs a decision"),
        "the card closes"
    );
    let c = calls.lock().clone();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0][0], ESCALATED);
    assert!(
        c[0].iter().any(|v| v == "Token bucket with refill"),
        "{c:?}"
    );
    assert!(me(&mut s, "Token bucket with refill"));
}

#[gpui::test]
async fn plan_approval_notes_option_waits_for_the_text_box(cx: &mut TestAppContext) {
    let calls: Arc<Mutex<Vec<Vec<Value>>>> = Arc::default();
    let opts = answers(&calls).resolver("GetEscalation", |_, _, _| {
        Reply::value(json!({
            "id": "esc-plan", "workspaceId": ESCALATED, "status": "open",
            "question": "Review the plan before I start coding.",
            "context": "1. Create go.mod\n2. Add the game loop",
            "options": [
                {"text": "Approve and proceed", "tag": "recommended", "desc": "Start coding to this plan as written."},
                {"text": "Proceed with my notes", "tag": "notes", "desc": "Add guidance in the text box — I fold it into the plan and start."}
            ]
        }))
    });
    let mut s = plain(cx, opts);
    s.open(ESCALATED);
    s.click("ask-opt:Proceed with my notes");
    assert!(
        s.has("ask-opt-sel:Proceed with my notes"),
        "the notes option stays selected"
    );
    assert!(s.has("ask-answer:off"), "Answer waits for the notes");
    assert!(calls.lock().is_empty());
    for ch in "use ncurses for the board".chars() {
        s.cx.simulate_input(&ch.to_string());
    }
    s.settle();
    s.click("ask-answer:on");
    let c = calls.lock().clone();
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(c[0].iter().any(|v| v == "Proceed with my notes"), "{c:?}");
    assert!(
        c[0].iter().any(|v| v == "use ncurses for the board"),
        "{c:?}"
    );
    assert!(me(
        &mut s,
        "Proceed with my notes — use ncurses for the board"
    ));
}

#[gpui::test]
async fn opens_a_working_loop_into_the_unified_workspace_view(cx: &mut TestAppContext) {
    let mut s = plain(cx, MockOptions::default());
    s.open(ACTIVE);
    assert!(s.has("v-detail") && s.has("obs-left"));
    assert!(
        s.has("tab-chip:chat-tab-MA:on") || s.has("tab-chip:chat-tab-MA"),
        "the dock leads with the Loop tab"
    );
}
