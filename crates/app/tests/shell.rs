//! The shell over the typed service fakes: shell, keyboard-accessibility and toast scenarios that
//! live in the shell and sidebar. Elements are found by
//! their debug selectors (`row:<id>`, `ph:<project>`, `on:<id>`, `ct:<key>`, …).

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    Bounds, Entity, Focusable, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext,
    point, px,
};
use recur_app::{Shell, ShellOptions};
use recur_domain::Workspace;
use recur_services::api::{BundleSkipInfo, Snapshot};
use recur_services::fake::{Fake, Overrides, Seed};
use serde::Serialize;
use serde::de::DeserializeOwned;
use recur_ui::widgets::ToastStore;
use recur_view_core::{AppState, LocalStorage, ViewContext, ViewName, ViewRegistry};
use serde_json::{Value, json};

struct H<'a> {
    state: Entity<AppState>,
    bridge: Arc<Fake>,
    cx: &'a mut VisualTestContext,
}

fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_ui::init(cx);
    });
}

fn bridge(patch: Value, opts: Overrides) -> Arc<Fake> {
    Fake::new(patched_seed(&patch), opts)
}

/// `v` with the fields `over` names replaced (`{ ...v, ...over }`).
fn patched<T: Serialize + DeserializeOwned>(v: &T, over: &Value) -> T {
    let mut base = serde_json::to_value(v).expect("seed row serializes");
    if let (Some(a), Some(b)) = (base.as_object_mut(), over.as_object()) {
        for (k, x) in b {
            a.insert(k.clone(), x.clone());
        }
    }
    serde_json::from_value(base).expect("patched seed row")
}

/// Upsert `rows` by `id`, merging fields into an existing row.
fn upsert<T: Serialize + DeserializeOwned + Default>(
    list: &mut Vec<T>,
    rows: &Value,
    id: impl Fn(&T) -> &str,
) {
    for row in rows.as_array().into_iter().flatten() {
        let key = row.get("id").and_then(Value::as_str).unwrap_or_default();
        match list.iter_mut().find(|x| id(x) == key) {
            Some(x) => *x = patched(x, row),
            None => list.push(patched(&T::default(), row)),
        }
    }
}

/// The e2e seed with `patch` applied: `projects` and `loops` upsert rows by id;
/// `projectGroups` replaces; `settings` merges.
fn patched_seed(patch: &Value) -> Seed {
    let mut s = Seed::e2e();
    if let Some(rows) = patch.get("projects") {
        upsert(&mut s.projects, rows, |p| &p.id);
    }
    if let Some(rows) = patch.get("loops") {
        upsert(&mut s.workspaces, rows, |w| &w.id);
        for w in s.workspaces.iter_mut().filter(|w| w.worktree_path.is_empty()) {
            w.worktree_path = format!("/mock/worktrees/{}", w.id);
        }
    }
    if let Some(v) = patch.get("projectGroups") {
        s.project_groups = serde_json::from_value(v.clone()).expect("project groups");
    }
    if let Some(v) = patch.get("settings") {
        s.settings = patched(&s.settings, v);
    }
    s
}

fn open(
    cx: &mut TestAppContext,
    bridge: Arc<Fake>,
    storage: LocalStorage,
) -> (Entity<AppState>, &mut VisualTestContext) {
    open_with(cx, bridge, storage, ViewRegistry::default())
}

fn open_with(
    cx: &mut TestAppContext,
    bridge: Arc<Fake>,
    storage: LocalStorage,
    registry: ViewRegistry,
) -> (Entity<AppState>, &mut VisualTestContext) {
    let services = bridge.services();
    let state = cx.update(|cx| AppState::install(services.clone(), storage, cx));
    let ctx = ViewContext {
        state: state.clone(),
        services,
    };
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        Shell::new(ctx, registry, ShellOptions::default(), window, cx)
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.activate_window();
        let f = shell.focus_handle(cx);
        window.focus(&f, cx);
    });
    cx.run_until_parked();
    (state, cx)
}

fn boot(cx: &mut TestAppContext, bridge: Arc<Fake>, storage: LocalStorage) -> H<'_> {
    init(cx);
    let (state, cx) = open(cx, bridge.clone(), storage);
    let mut h = H { state, bridge, cx };
    h.settle();
    h
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

impl H<'_> {
    fn settle(&mut self) {
        self.cx.run_until_parked();
        self.cx.update(|window, _| window.refresh());
        self.cx.run_until_parked();
    }

    fn bounds(&mut self, sel: &str) -> Option<Bounds<Pixels>> {
        self.settle();
        self.cx.debug_bounds(leak(sel))
    }

    fn has(&mut self, sel: &str) -> bool {
        self.bounds(sel).is_some()
    }

    fn at(&mut self, sel: &str) -> gpui_kit::Point<Pixels> {
        self.bounds(sel)
            .unwrap_or_else(|| panic!("no element {sel}"))
            .center()
    }

    fn click(&mut self, sel: &str) {
        let p = self.at(sel);
        self.cx.simulate_mouse_move(p, None, Modifiers::none());
        self.cx.simulate_click(p, Modifiers::none());
        self.settle();
    }

    fn right_click(&mut self, sel: &str) {
        let p = self.at(sel);
        self.cx.simulate_mouse_move(p, None, Modifiers::none());
        self.cx
            .simulate_mouse_down(p, MouseButton::Right, Modifiers::none());
        self.cx
            .simulate_mouse_up(p, MouseButton::Right, Modifiers::none());
        self.settle();
    }

    fn hover(&mut self, sel: &str) {
        let p = self.at(sel);
        self.move_to(f32::from(p.x), f32::from(p.y));
    }

    fn move_to(&mut self, x: f32, y: f32) {
        self.cx
            .simulate_mouse_move(point(px(x), px(y)), None, Modifiers::none());
        self.settle();
    }

    fn keys(&mut self, keys: &str) {
        self.cx.simulate_keystrokes(keys);
        self.settle();
    }

    fn wait(&mut self, ms: u64) {
        self.cx.executor().advance_clock(Duration::from_millis(ms));
        self.settle();
    }

    fn read<R>(&mut self, f: impl FnOnce(&AppState) -> R) -> R {
        self.state.read_with(self.cx, |s, _| f(s))
    }

    fn view(&mut self) -> ViewName {
        self.read(|s| s.view)
    }

    fn seed_project(&self, id: &str) -> recur_domain::Project {
        self.bridge.with_seed(|s| {
            s.projects
                .iter()
                .find(|p| p.id == id)
                .cloned()
                .unwrap_or_default()
        })
    }
}

const ESCALATED: &str = "LOOP-2218";
const ACTIVE: &str = "LOOP-2231";
const CODING: &str = "LOOP-2229";
const PR_OPEN: &str = "LOOP-2224";
const SEED_LOOPS: [&str; 5] = [ESCALATED, ACTIVE, CODING, "LOOP-2235", PR_OPEN];

#[gpui_kit::test]
fn boots_into_the_briefing_with_seeded_rows(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    assert_eq!(h.view(), ViewName::Briefing);
    for id in SEED_LOOPS {
        assert!(h.has(&format!("row:{id}")), "{id} in the tree");
        assert!(!h.has(&format!("on:{id}")), "{id} not selected");
    }
}

#[gpui_kit::test]
fn sidebar_flags_a_workspace_waiting_on_the_user(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    assert!(h.has(&format!("ct:row:{ESCALATED}")));
    assert!(h.has("foot-needs"));
    assert!(!h.has(&format!("ct:row:{CODING}")));
}

#[gpui_kit::test]
fn retryable_failed_and_interrupted_workspaces_need_you(cx: &mut TestAppContext) {
    let patch = json!({"loops": [
        {"id": ESCALATED, "state": "failed", "group": "needs", "statusLabel": "Failed"},
        {"id": "LOOP-cut", "projectId": "payments", "group": "needs", "state": "paused",
         "phase": "interrupted", "goal": "Interrupted recover", "short": "Interrupted recover",
         "archived": false, "isMain": false, "diff": {"add": 0, "del": 0}},
    ]});
    let mut h = boot(
        cx,
        bridge(patch, Overrides::default()),
        LocalStorage::in_memory(),
    );
    assert!(h.has(&format!("ct:row:{ESCALATED}")));
    assert!(h.has("ct:row:LOOP-cut"));
    assert!(h.has("foot-needs"));
}

#[gpui_kit::test]
fn collapsed_group_does_not_badge_a_hidden_pr_chat_host(cx: &mut TestAppContext) {
    let patch = json!({
        "projectGroups": [{"id": "g-sites", "name": "Sites", "open": false,
            "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"}],
        "projects": [{"id": "marketing", "groupId": "g-sites"}],
        "loops": [{"id": "ws-pr-host", "projectId": "marketing", "hostPrId": "pr-host",
            "type": "solo", "goal": "PR chat", "short": "PR chat", "archived": false,
            "isMain": false, "state": "paused", "diff": {"add": 0, "del": 0}}],
    });
    let opts = Overrides::default().list_pending_questions(|_, ()| {
        Ok(serde_json::from_value(
            json!([{"id": "q-host", "workspaceId": "ws-pr-host", "agentCode": "MA",
            "question": "Ship it?", "askedAgo": "1m"}]),
        )
        .unwrap())
    });
    let mut h = boot(cx, bridge(patch, opts), LocalStorage::in_memory());
    assert!(h.has("gh:g-sites"));
    assert!(!h.has("ph:marketing"), "collapsed group hides its projects");
    assert!(!h.has("ct:gh:g-sites"));
    h.click("gh:g-sites");
    assert!(h.has("ph:marketing"));
    assert!(!h.has("ct:ph:marketing"));
    assert!(!h.has("row:ws-pr-host"));
}

#[gpui_kit::test]
fn restores_the_last_open_workspace_after_a_restart(cx: &mut TestAppContext) {
    init(cx);
    let b = bridge(json!({}), Overrides::default());
    {
        let (state, vcx) = open(cx, b.clone(), LocalStorage::in_memory());
        let mut h = H {
            state,
            bridge: b.clone(),
            cx: vcx,
        };
        h.click(&format!("hit:{ACTIVE}"));
        assert_eq!(h.view(), ViewName::Detail);
        assert!(h.has(&format!("on:{ACTIVE}")));
        h.wait(1500);
    }
    let (state, vcx) = open(cx, b.clone(), LocalStorage::in_memory());
    let mut h = H {
        state,
        bridge: b,
        cx: vcx,
    };
    assert_eq!(h.view(), ViewName::Detail);
    assert_eq!(h.read(|s| s.selected_workspace_id.clone()), ACTIVE);
    assert!(h.has(&format!("on:{ACTIVE}")));
}

#[gpui_kit::test]
fn stays_on_the_briefing_after_a_restart_if_that_was_the_last_view(cx: &mut TestAppContext) {
    init(cx);
    let b = bridge(json!({}), Overrides::default());
    {
        let (state, vcx) = open(cx, b.clone(), LocalStorage::in_memory());
        let mut h = H {
            state,
            bridge: b.clone(),
            cx: vcx,
        };
        h.click(&format!("hit:{ACTIVE}"));
        h.click("nav-inbox");
        assert_eq!(h.view(), ViewName::Briefing);
        h.wait(1500);
    }
    let (state, vcx) = open(cx, b.clone(), LocalStorage::in_memory());
    let mut h = H {
        state,
        bridge: b,
        cx: vcx,
    };
    assert_eq!(h.view(), ViewName::Briefing);
    assert!(!h.has(&format!("on:{ACTIVE}")));
}

#[gpui_kit::test]
fn inbox_and_pr_dashboard_do_not_keep_the_row_selected(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    let on = format!("on:{ACTIVE}");
    h.click(&format!("hit:{ACTIVE}"));
    assert!(h.has(&on));
    h.click("nav-inbox");
    assert_eq!(h.view(), ViewName::Briefing);
    assert!(!h.has(&on));
    h.click(&format!("hit:{ACTIVE}"));
    assert!(h.has(&on));
    h.click("nav-pr");
    assert_eq!(h.view(), ViewName::Pr);
    assert!(!h.has(&on));
}

#[gpui_kit::test]
fn hover_shows_view_pr_and_remove(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    assert!(!h.has(&format!("rm:{PR_OPEN}")));
    h.hover(&format!("row:{PR_OPEN}"));
    assert!(h.has(&format!("pr:{PR_OPEN}")));
    assert!(h.has(&format!("rm:{PR_OPEN}")));
    h.hover(&format!("row:{CODING}"));
    assert!(!h.has(&format!("pr:{CODING}")), "no PR, no View PR");
    assert!(h.has(&format!("rm:{CODING}")));
    assert!(!h.has(&format!("rm:{PR_OPEN}")));
}

#[gpui_kit::test]
fn hover_remove_archives_a_settled_workspace_after_confirm(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    h.hover(&format!("row:{PR_OPEN}"));
    h.click(&format!("rm:{PR_OPEN}"));
    assert!(
        h.has(&format!("row:{PR_OPEN}")),
        "the first click only arms"
    );
    h.click(&format!("rm:{PR_OPEN}"));
    assert!(!h.has(&format!("row:{PR_OPEN}")));
    let archived = h
        .bridge
        .with_seed(|s| s.workspace(PR_OPEN).is_some_and(|w| w.archived));
    assert!(archived);
}

#[gpui_kit::test]
fn hover_remove_stays_disabled_on_a_running_workspace(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    h.hover(&format!("row:{CODING}"));
    h.click(&format!("rm:{CODING}"));
    h.click(&format!("rm:{CODING}"));
    assert!(h.has(&format!("row:{CODING}")));
    assert!(
        !h.read(|s| s.rail_peek_locked),
        "a disabled Remove never arms"
    );
}

#[gpui_kit::test]
fn project_tree_expand_collapse_follows_open_and_toggles(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    assert!(h.has("main:storefront"), "open project shows its rows");
    assert!(!h.has("main:marketing"), "closed project is collapsed");
    h.click("ph:storefront");
    assert!(!h.has("main:storefront"));
    assert!(!h.seed_project("storefront").open);
    h.click("ph:storefront");
    assert!(h.has("main:storefront"));
    assert!(h.seed_project("storefront").open);
}

#[gpui_kit::test]
fn right_click_hides_a_project_and_the_eye_toggle_reveals_it(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    assert!(!h.has("sec-hidden"));
    h.right_click("ph:marketing");
    assert!(h.has("rail-move-menu"));
    h.click("rail-move-hide");
    assert!(!h.has("rail-move-menu"));
    assert!(!h.has("ph:marketing"));
    assert!(h.seed_project("marketing").hidden);
    h.click("sec-hidden");
    assert!(h.read(|s| s.show_hidden_projects()));
    assert!(h.has("ph:marketing"));
}

#[gpui_kit::test]
fn boots_with_saved_hidden_projects_and_show_hidden(cx: &mut TestAppContext) {
    let patch = json!({
        "projects": [{"id": "marketing", "hidden": true}],
        "settings": {"showHiddenProjects": true},
    });
    let mut h = boot(
        cx,
        bridge(patch, Overrides::default()),
        LocalStorage::in_memory(),
    );
    assert!(h.has("ph:marketing"));
    assert!(h.has("sec-hidden"));
    h.click("sec-hidden");
    assert!(!h.has("ph:marketing"));
}

#[gpui_kit::test]
fn collapsed_sidebar_flies_out_from_the_left_edge(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    h.click("tb-rail");
    assert!(h.read(|s| s.rail_collapsed));
    h.wait(300);
    assert!(!h.has("rail"), "the docked rail slid away");
    assert_eq!(h.view(), ViewName::Briefing);

    let body_top = f32::from(h.at("tb-rail").y) + 60.;
    h.move_to(4., body_top + 80.);
    h.wait(120);
    assert!(h.read(|s| s.rail_peek));
    assert!(h.has("nav-inbox"));
    // The flyout slides in over 160ms; Playwright clicks a row once it has stopped moving.
    h.wait(160);

    h.click(&format!("hit:{ACTIVE}"));
    assert_eq!(h.view(), ViewName::Detail);
    assert!(
        h.read(|s| s.rail_peek),
        "the flyout stays while the pointer is on it"
    );

    h.move_to(700., body_top + 120.);
    h.wait(400);
    assert!(!h.read(|s| s.rail_peek));
}

#[gpui_kit::test]
fn collapsed_flyout_dismisses_with_escape_without_leaving_the_view(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    h.click(&format!("hit:{ACTIVE}"));
    h.click("tb-rail");
    let body_top = f32::from(h.at("tb-rail").y) + 60.;
    h.move_to(4., body_top + 80.);
    h.wait(120);
    assert!(h.read(|s| s.rail_peek));
    h.keys("escape");
    assert!(!h.read(|s| s.rail_peek));
    assert_eq!(h.view(), ViewName::Detail);
}

#[gpui_kit::test]
fn alt_cmd_arrows_cycle_workspaces_in_sidebar_order(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    let rows: Vec<(String, f32)> = SEED_LOOPS
        .iter()
        .filter_map(|id| {
            h.bounds(&format!("row:{id}"))
                .map(|b| (id.to_string(), f32::from(b.top())))
        })
        .collect();
    let mut rows = rows;
    rows.sort_by(|a, b| a.1.total_cmp(&b.1));
    let (first, second) = (rows[0].0.clone(), rows[1].0.clone());
    h.click(&format!("hit:{first}"));
    assert!(h.has(&format!("on:{first}")));
    h.keys("alt-secondary-down");
    assert!(h.has(&format!("on:{second}")));
    assert!(!h.has(&format!("on:{first}")));
    h.keys("alt-secondary-up");
    assert!(h.has(&format!("on:{first}")));
}

#[gpui_kit::test]
fn alt_cmd_arrows_peek_a_collapsed_project_and_recollapse_unless_you_dwell(
    cx: &mut TestAppContext,
) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    h.click("ph:storefront");
    assert!(!h.has("main:storefront"));
    h.click("main:payments");
    h.wait(50);
    assert!(h.has("on:main:payments"));

    h.keys("alt-secondary-up");
    h.keys("alt-secondary-down");
    assert!(h.has("on:main:payments"));
    assert!(
        !h.has("main:storefront"),
        "a pass-through peek re-collapses"
    );

    h.keys("alt-secondary-up");
    assert!(h.has("main:storefront"), "landing peeks the project open");
    h.wait(1200);
    h.keys("alt-secondary-down");
    assert!(h.has("main:storefront"), "a dwell keeps it open");
    assert!(h.seed_project("storefront").open);
}

#[gpui_kit::test]
fn esc_from_workspace_detail_returns_to_the_briefing(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    h.click(&format!("hit:{ACTIVE}"));
    assert_eq!(h.view(), ViewName::Detail);
    h.keys("escape");
    assert_eq!(h.view(), ViewName::Briefing);
}

#[gpui_kit::test]
fn esc_from_pr_detail_returns_to_the_pr_list(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    let state = h.state.clone();
    state.update(h.cx, |s, cx| s.open_pr("482", cx));
    h.settle();
    assert_eq!(h.view(), ViewName::PrDetail);
    h.keys("escape");
    assert_eq!(h.view(), ViewName::Pr);
}

#[gpui_kit::test]
fn esc_while_typing_does_not_navigate_away(cx: &mut TestAppContext) {
    let patch = json!({
        "projectGroups": [{"id": "g-a", "name": "Clients", "open": true}],
        "projects": [{"id": "storefront", "groupId": "g-a"}],
    });
    let mut h = boot(
        cx,
        bridge(patch, Overrides::default()),
        LocalStorage::in_memory(),
    );
    h.click(&format!("hit:{ACTIVE}"));
    h.hover("gh:g-a");
    h.click("gh-rename:g-a");
    assert!(h.has("gh-input:g-a"));
    h.keys("escape");
    assert!(!h.has("gh-input:g-a"), "Escape cancels the rename");
    assert_eq!(h.view(), ViewName::Detail);
}

#[gpui_kit::test]
fn rename_commits_on_enter(cx: &mut TestAppContext) {
    let patch = json!({
        "projectGroups": [{"id": "g-a", "name": "Clients", "open": true}],
        "projects": [{"id": "storefront", "groupId": "g-a"}],
    });
    let mut h = boot(
        cx,
        bridge(patch, Overrides::default()),
        LocalStorage::in_memory(),
    );
    h.hover("gh:g-a");
    h.click("gh-rename:g-a");
    h.keys("secondary-a");
    h.cx.simulate_input("Agency");
    h.keys("enter");
    assert!(!h.has("gh-input:g-a"));
    let name = h.read(|s| s.project_groups[0].name.clone());
    assert_eq!(name, "Agency");
}

#[gpui_kit::test]
fn add_project_is_not_left_focused_after_launch(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    // A focused Add project would open the project form on Enter.
    h.keys("enter");
    assert_eq!(h.view(), ViewName::Briefing);
    h.keys("space");
    assert_eq!(h.view(), ViewName::Briefing);
}

#[gpui_kit::test]
fn shift_cmd_n_creates_a_blank_workspace_without_the_modal(cx: &mut TestAppContext) {
    let creates: Arc<parking_lot::Mutex<Vec<Vec<String>>>> = Arc::default();
    let log = creates.clone();
    let opts = Overrides::default().create_workspace(
        move |_, (project_id, name, base, mode, prompt, autonomy, _)| {
            log.lock()
                .push(vec![project_id.clone(), name, base, mode, prompt, autonomy]);
            Ok(Workspace {
                id: "WS-NEW".into(),
                project_id,
                r#type: "solo".into(),
                goal: "New Workspace".into(),
                short: "New Workspace".into(),
                status_label: "Creating".into(),
                phase: "creating".into(),
                branch: "recur/new".into(),
                base_branch: "main".into(),
                ..Default::default()
            })
        },
    );
    let mut h = boot(cx, bridge(json!({}), opts), LocalStorage::in_memory());
    h.keys("shift-secondary-n");
    assert!(!h.read(|s| s.overlays.new_task));
    assert_eq!(h.view(), ViewName::Briefing);
    {
        let creates = creates.lock();
        assert_eq!(creates.len(), 1);
        assert_eq!(creates[0][3], "agent");
        assert_eq!(creates[0][1], "");
        assert_eq!(creates[0][4], "");
    }
    h.click("hit:WS-NEW");
    assert_eq!(h.view(), ViewName::Detail);
}

#[gpui_kit::test]
fn window_shortcuts_work_with_nothing_focused(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    h.cx.update(|window, cx| window.blur(cx));
    h.settle();
    h.keys("secondary-k");
    assert!(h.read(|s| s.overlays.palette));
}

#[gpui_kit::test]
fn a_failed_save_shows_an_error_toast_that_auto_dismisses(cx: &mut TestAppContext) {
    let opts = Overrides::default().update_project_run_config(|_, _| {
        Err(recur_services::Error::new("disk is read-only"))
    });
    let mut h = boot(cx, bridge(json!({}), opts), LocalStorage::in_memory());
    h.click("ph:storefront");
    let texts = |h: &mut H| {
        h.cx.update(|_, cx| {
            ToastStore::global(cx)
                .read(cx)
                .toasts()
                .iter()
                .map(|t| t.text.to_string())
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(
        texts(&mut h),
        ["Could not save project view: disk is read-only"]
    );
    assert!(h.has("main:storefront"), "the toggle rolls back");
    h.wait(7_500);
    assert_eq!(texts(&mut h).len(), 1, "errors linger");
    h.wait(1_000);
    assert!(texts(&mut h).is_empty());
}

#[gpui_kit::test]
fn dragging_a_project_into_a_group_moves_it_there(cx: &mut TestAppContext) {
    let patch = json!({
        "projectGroups": [{"id": "g-a", "name": "Clients", "open": true}],
        "projects": [{"id": "payments", "groupId": "g-a"}],
    });
    let mut h = boot(
        cx,
        bridge(patch, Overrides::default()),
        LocalStorage::in_memory(),
    );
    let from = h.at("ph:storefront");
    let to = h.at("ph:payments");
    h.cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    for i in 1..=12 {
        let t = i as f32 / 12.;
        let p = point(
            from.x + (to.x - from.x) * t,
            from.y + (to.y - from.y) * t + px(4.),
        );
        h.cx.simulate_mouse_move(p, MouseButton::Left, Modifiers::none());
        h.settle();
    }
    h.cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    h.wait(300);
    assert_eq!(h.seed_project("storefront").group_id, "g-a");
    let order: Vec<String> = h
        .bridge
        .with_seed(|s| s.projects.iter().map(|p| p.id.clone()).collect());
    assert_eq!(
        &order[..2],
        ["storefront", "payments"],
        "dropped above payments"
    );
    assert!(h.has("ph:storefront"));
}

/// Navigate → Reload Bundles and its toasts: the catalog is
/// re-read, bundle-derived state rebuilds off the epoch, and skipped bundles are reported.
#[gpui::test]
fn reload_bundles_rereads_the_catalog_and_reports_skipped_bundles(cx: &mut TestAppContext) {
    let calls = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let skipped = Arc::new(parking_lot::Mutex::new(Vec::<BundleSkipInfo>::new()));
    let (c1, c2, s2) = (calls.clone(), calls.clone(), skipped.clone());
    let opts = Overrides::default()
        .invalidate_catalog(move |_, ()| {
            c1.lock().push("InvalidateCatalog".into());
            Ok(())
        })
        .list_skipped_bundles(move |_, ()| {
            c2.lock().push("ListSkippedBundles".into());
            Ok(s2.lock().clone())
        });
    let mut h = boot(cx, bridge(json!({}), opts), LocalStorage::in_memory());
    let toast = |h: &mut H, kind: recur_ui::widgets::ToastKind, text: &str| {
        h.cx.update(|_, cx| {
            ToastStore::global(cx)
                .read(cx)
                .toasts()
                .iter()
                .any(|t| t.kind == kind && t.text == text)
        })
    };
    let epoch = h.read(|s| s.bundles_epoch);
    h.state
        .update(h.cx, |s, cx| s.dispatch_menu_action("reload-bundles", cx));
    h.settle();
    assert_eq!(*calls.lock(), ["InvalidateCatalog", "ListSkippedBundles"]);
    assert_eq!(h.read(|s| s.bundles_epoch), epoch + 1);
    assert!(toast(
        &mut h,
        recur_ui::widgets::ToastKind::Success,
        "Bundles reloaded"
    ));
    *skipped.lock() = serde_json::from_value(
        json!([{"name": "broken", "reason": "bad toml"}, {"name": "old", "reason": "x"}]),
    )
    .unwrap();
    h.state
        .update(h.cx, |s, cx| s.dispatch_menu_action("reload-bundles", cx));
    h.settle();
    assert_eq!(h.read(|s| s.bundles_epoch), epoch + 2);
    assert!(toast(
        &mut h,
        recur_ui::widgets::ToastKind::Info,
        "Bundles reloaded, 2 skipped (see Settings → Bundles)"
    ));
}

#[gpui_kit::test]
fn sidebar_usage_meter_opens_a_popover_with_quota_bars(cx: &mut TestAppContext) {
    let usage = json!({
        "fetchedAt": 1767225600000_i64, "stale": false,
        "providers": [
            {"id": "claude-code", "name": "Claude", "email": "dev@example.com",
             "source": "Claude Code login", "sourceId": "claude_login",
             "windows": [
                {"id": "five_hour", "label": "5-hour", "usedPct": 12},
                {"id": "seven_day", "label": "7-day", "usedPct": 34},
                {"id": "seven_day_fable", "label": "Fable (7-day)", "usedPct": 41}]},
            {"id": "codex", "name": "Codex", "email": "codex@example.com",
             "source": "ChatGPT login", "sourceId": "chatgpt",
             "windows": [
                {"id": "five_hour", "label": "5-hour", "usedPct": 100},
                {"id": "seven_day", "label": "7-day", "usedPct": 16},
                {"id": "billing", "label": "Monthly", "usedPct": 100, "detail": "500 of 500 credits"}]},
            {"id": "cursor", "name": "Cursor", "error": "No Cursor usage session", "hint": "Connect in Settings."}
        ]
    });
    let usage: Snapshot = serde_json::from_value(usage).unwrap();
    let (u1, u2) = (usage.clone(), usage);
    let opts = Overrides::default()
        .get_provider_usage(move |_, ()| Ok(u1.clone()))
        .refresh_provider_usage(move |_, ()| Ok(u2.clone()));
    let mut h = boot(cx, bridge(json!({}), opts), LocalStorage::in_memory());
    assert!(h.has("use-chip:claude-code") && h.has("use-chip:codex"));
    assert!(!h.has("use-chip:cursor"), "two chips");
    h.click("use-btn");
    assert!(h.has("use-pop"));
    for sel in [
        "use-prov:claude-code",
        "use-win:claude-code:7-day:34.0%",
        "use-win:claude-code:Fable (7-day):41.0%",
        "use-prov:codex",
        "use-win:codex:Monthly:100.0%",
        "use-detail:codex:500 of 500 credits",
        "use-cursor-link",
    ] {
        assert!(h.has(sel), "missing {sel}");
    }
}

#[gpui_kit::test]
fn working_copy_row_has_no_hover_actions(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    h.hover("main:storefront");
    assert!(h.has("main:storefront"));
    for id in SEED_LOOPS {
        assert!(!h.has(&format!("rm:{id}")) && !h.has(&format!("pr:{id}")));
    }
}

#[gpui_kit::test]
fn sidebar_shows_live_pr_count(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    let n = h.bridge.with_seed(|s| s.prs.len());
    assert!(n > 0);
    assert!(
        h.has(&format!("ct:nav-pr:{n}")),
        "the count follows the store"
    );
}

#[gpui_kit::test]
fn dragging_a_project_onto_a_group_highlights_the_whole_group(cx: &mut TestAppContext) {
    let patch = json!({
        "projectGroups": [
            {"id": "g-apps", "name": "Apps", "open": true},
            {"id": "g-sites", "name": "Sites", "open": true}
        ],
        "projects": [{"id": "payments", "groupId": "g-apps"}, {"id": "marketing", "groupId": "g-sites"}],
    });
    let mut h = boot(
        cx,
        bridge(patch, Overrides::default()),
        LocalStorage::in_memory(),
    );
    let from = h.at("ph:storefront");
    let to = h.at("gh:g-apps");
    h.cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    for i in 1..=12 {
        let t = i as f32 / 12.;
        let p = point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
        h.cx.simulate_mouse_move(p, MouseButton::Left, Modifiers::none());
        h.settle();
    }
    assert!(h.has("drop-target:g-apps"));
    assert!(!h.has("drop-target:g-sites"));
    assert!(h.has("drag-ph"), "the lifted project leaves a dashed slot");
    h.cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    h.wait(300);
}

/// Whether `id` is selected and its row shows at least 80% inside the tree's scrollport.
fn shown_in_tree(h: &mut H, id: &str) -> bool {
    if !h.has(&format!("on:{id}")) {
        return false;
    }
    let (Some(port), Some(row)) = (h.bounds("rail-tree"), h.bounds(&format!("row:{id}"))) else {
        return false;
    };
    let overlap = f32::from(row.bottom().min(port.bottom()) - row.top().max(port.top()));
    overlap >= 0.8 * f32::from(row.size.height)
}

fn storefront_rows(h: &mut H) -> Vec<String> {
    let mut rows: Vec<(String, f32)> = ["LOOP-2231", "LOOP-2229", "LOOP-2235", "LOOP-2224"]
        .iter()
        .filter_map(|id| {
            h.bounds(&format!("row:{id}"))
                .map(|b| (id.to_string(), f32::from(b.top())))
        })
        .collect();
    rows.sort_by(|a, b| a.1.total_cmp(&b.1));
    rows.into_iter().map(|r| r.0).collect()
}

#[gpui_kit::test]
fn alt_cmd_arrows_scroll_the_selected_workspace_into_the_sidebar_tree(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    let rows = storefront_rows(&mut h);
    assert!(rows.len() > 2);
    h.cx.simulate_resize(gpui::size(px(1280.), px(400.)));
    h.settle();
    h.click(&format!("hit:{}", rows[0]));
    for _ in 1..rows.len() {
        h.keys("alt-secondary-down");
    }
    let last = rows.last().unwrap();
    assert!(h.has(&format!("on:{last}")));
    assert!(
        shown_in_tree(&mut h, last),
        "the selected row is scrolled into view"
    );
}

#[gpui_kit::test]
fn opening_a_workspace_elsewhere_scrolls_it_into_the_sidebar_tree(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    let last = storefront_rows(&mut h).pop().unwrap();
    h.cx.simulate_resize(gpui::size(px(1280.), px(400.)));
    h.settle();
    h.state
        .update(h.cx, |s, cx| s.open_workspace(&last, false, cx));
    h.settle();
    assert!(
        shown_in_tree(&mut h, &last),
        "the opened row is scrolled into view"
    );
}

/// A modal on the New Task slot whose scrim closes it on a press, as the real ones do.
struct ScrimModal {
    state: Entity<AppState>,
    _sub: gpui_kit::Subscription,
}

impl gpui_kit::Render for ScrimModal {
    fn render(
        &mut self,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{InteractiveElement as _, IntoElement as _, Styled as _};
        if !self.state.read(cx).overlays.new_task {
            return gpui_kit::div().into_any_element();
        }
        let state = self.state.clone();
        recur_ui::style::modal_scrim(window, gpui_kit::div().w(px(200.)).h(px(100.)))
            .id("scrim-modal")
            .occlude()
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                state.update(cx, |s, cx| s.close_new_task(cx))
            })
            .into_any_element()
    }
}

/// An open modal covers the page's resize handles: a press on the sidebar's edge lands on the
/// scrim (closing the modal) instead of dragging the rail under it.
#[gpui_kit::test]
fn resize_handles_under_a_modal_take_no_pointer(cx: &mut TestAppContext) {
    use gpui_kit::AppContext as _;
    use recur_ui::guard::Guarded as _;
    use recur_view_core::prefs::PanelKey;
    use recur_view_core::view::OverlaySlot;
    init(cx);
    let mut registry = ViewRegistry::default();
    registry.overlay(OverlaySlot::NewTask, |vcx, _, cx| {
        let state = vcx.state.clone();
        cx.new(|cx| ScrimModal {
            _sub: cx.guarded_observe(&state, |_, _, cx| cx.notify()),
            state,
        })
        .into()
    });
    let bridge = bridge(json!({}), Overrides::default());
    let (state, cx) = open_with(cx, bridge.clone(), LocalStorage::in_memory(), registry);
    let mut h = H { state, bridge, cx };
    h.settle();
    let rail = h.bounds("rail").expect("rail");
    let width = h.read(|s| s.panels.size(PanelKey::Rail));
    h.state.update(h.cx, |s, cx| s.open_new_task("", None, cx));
    h.settle();
    let edge = point(rail.right() - px(1.), rail.center().y);
    let past = point(edge.x + px(80.), edge.y);
    h.cx.simulate_mouse_move(edge, None, Modifiers::none());
    h.cx.simulate_mouse_down(edge, MouseButton::Left, Modifiers::none());
    h.cx.simulate_mouse_move(past, Some(MouseButton::Left), Modifiers::none());
    h.cx.simulate_mouse_up(past, MouseButton::Left, Modifiers::none());
    h.settle();
    assert_eq!(h.read(|s| s.panels.size(PanelKey::Rail)), width);
    assert!(!h.read(|s| s.overlays.new_task), "the scrim took the press");
}

/// ⌃` toggles the terminal drawer and ⌃⇧` maximizes it. GPUI reports ⌃⇧` as ⌃~ (Shift
/// folded into the key), which toggled the drawer instead.
#[gpui_kit::test]
fn ctrl_shift_backtick_maximizes_the_terminal(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), Overrides::default()),
        LocalStorage::in_memory(),
    );
    h.click(&format!("hit:{ACTIVE}"));
    assert_eq!(h.view(), ViewName::Detail);
    h.keys("ctrl-`");
    assert!(h.read(|s| s.dock.is_open(ACTIVE)));
    assert!(!h.read(|s| s.dock.is_maximized(ACTIVE)));
    h.keys("ctrl-~");
    assert!(
        h.read(|s| s.dock.is_open(ACTIVE)),
        "maximize keeps the drawer"
    );
    assert!(h.read(|s| s.dock.is_maximized(ACTIVE)));
    h.keys("ctrl-~");
    assert!(!h.read(|s| s.dock.is_maximized(ACTIVE)));
}

struct TabProbe {
    focus: gpui_kit::FocusHandle,
    tabs: usize,
}

impl gpui_kit::Render for TabProbe {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{InteractiveElement, Styled};
        use recur_ui::guard::Guarded;
        gpui_kit::div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(
                cx.guarded_listener(|this, e: &gpui_kit::KeyDownEvent, _, cx| {
                    if e.keystroke.key == "tab" {
                        this.tabs += 1;
                        cx.stop_propagation();
                    }
                }),
            )
    }
}

/// The window's gpui-base `Root` binds Tab to focus traversal, which ran before the focused
/// terminal, editor or composer saw the key.
#[gpui_kit::test]
fn tab_reaches_the_focused_element_under_the_window_root(cx: &mut TestAppContext) {
    cx.update(|cx| recur_app::init(&ViewRegistry::default(), cx));
    let probe = gpui_kit::AppContext::new(cx, |cx| TabProbe {
        focus: cx.focus_handle(),
        tabs: 0,
    });
    let root_probe = probe.clone();
    let (_, cx) =
        cx.add_window_view(move |window, cx| gpui_kit::base::Root::new(root_probe, window, cx));
    cx.update(|window, cx| {
        let f = probe.read(cx).focus.clone();
        window.focus(&f, cx);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("tab shift-tab");
    assert_eq!(probe.read_with(cx, |p, _| p.tabs), 2);
}
