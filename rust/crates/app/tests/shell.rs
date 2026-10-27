//! The shell over the mock bridge: ports of `frontend/e2e/shell.spec.ts`, `a11y-keys.spec.ts`
//! and `toasts.spec.ts` scenarios that live in the shell and sidebar. Elements are found by
//! their debug selectors (`row:<id>`, `ph:<project>`, `on:<id>`, `ct:<key>`, …).

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    Bounds, Entity, Focusable, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext,
    point, px,
};
use recur_app::gallery::scene_seed;
use recur_app::{Shell, ShellOptions};
use recur_services::mock::{MockBridge, MockOptions, Reply};
use recur_ui::widgets::ToastStore;
use recur_view_core::{AppState, LocalStorage, ViewContext, ViewName, ViewRegistry};
use serde_json::{Value, json};

struct H<'a> {
    state: Entity<AppState>,
    bridge: Arc<MockBridge>,
    cx: &'a mut VisualTestContext,
}

fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_ui::init(cx);
    });
}

fn bridge(patch: Value, opts: MockOptions) -> Arc<MockBridge> {
    MockBridge::new(scene_seed(&patch), opts)
}

fn open(
    cx: &mut TestAppContext,
    bridge: Arc<MockBridge>,
    storage: LocalStorage,
) -> (Entity<AppState>, &mut VisualTestContext) {
    let services = bridge.services();
    let state = cx.update(|cx| AppState::install(services.clone(), storage, cx));
    let ctx = ViewContext {
        state: state.clone(),
        services,
    };
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        Shell::new(
            ctx,
            ViewRegistry::default(),
            ShellOptions::default(),
            window,
            cx,
        )
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

fn boot(cx: &mut TestAppContext, bridge: Arc<MockBridge>, storage: LocalStorage) -> H<'_> {
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

    fn seed_project(&self, id: &str) -> Value {
        self.bridge.with_seed(|s| {
            s.projects
                .iter()
                .find(|p| p["id"] == id)
                .cloned()
                .unwrap_or(Value::Null)
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
        bridge(json!({}), MockOptions::default()),
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
        bridge(json!({}), MockOptions::default()),
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
        bridge(patch, MockOptions::default()),
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
    let opts = MockOptions::default().resolver("ListPendingQuestions", |_, _, _| {
        Reply::value(
            json!([{"id": "q-host", "workspaceId": "ws-pr-host", "agentCode": "MA",
            "question": "Ship it?", "askedAgo": "1m"}]),
        )
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
    let b = bridge(json!({}), MockOptions::default());
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
    let b = bridge(json!({}), MockOptions::default());
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
        bridge(json!({}), MockOptions::default()),
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
        bridge(json!({}), MockOptions::default()),
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
        bridge(json!({}), MockOptions::default()),
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
    let archived = h.bridge.with_seed(|s| {
        s.loop_by_id(PR_OPEN)
            .map(|l| l["archived"] == true)
            .unwrap_or(false)
    });
    assert!(archived);
}

#[gpui_kit::test]
fn hover_remove_stays_disabled_on_a_running_workspace(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), MockOptions::default()),
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
        bridge(json!({}), MockOptions::default()),
        LocalStorage::in_memory(),
    );
    assert!(h.has("main:storefront"), "open project shows its rows");
    assert!(!h.has("main:marketing"), "closed project is collapsed");
    h.click("ph:storefront");
    assert!(!h.has("main:storefront"));
    assert_eq!(h.seed_project("storefront")["open"], false);
    h.click("ph:storefront");
    assert!(h.has("main:storefront"));
    assert_eq!(h.seed_project("storefront")["open"], true);
}

#[gpui_kit::test]
fn right_click_hides_a_project_and_the_eye_toggle_reveals_it(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), MockOptions::default()),
        LocalStorage::in_memory(),
    );
    assert!(!h.has("sec-hidden"));
    h.right_click("ph:marketing");
    assert!(h.has("rail-move-menu"));
    h.click("rail-move-hide");
    assert!(!h.has("rail-move-menu"));
    assert!(!h.has("ph:marketing"));
    assert_eq!(h.seed_project("marketing")["hidden"], true);
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
        bridge(patch, MockOptions::default()),
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
        bridge(json!({}), MockOptions::default()),
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
        bridge(json!({}), MockOptions::default()),
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
        bridge(json!({}), MockOptions::default()),
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
        bridge(json!({}), MockOptions::default()),
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
    assert_eq!(h.seed_project("storefront")["open"], true);
}

#[gpui_kit::test]
fn esc_from_workspace_detail_returns_to_the_briefing(cx: &mut TestAppContext) {
    let mut h = boot(
        cx,
        bridge(json!({}), MockOptions::default()),
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
        bridge(json!({}), MockOptions::default()),
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
        bridge(patch, MockOptions::default()),
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
        bridge(patch, MockOptions::default()),
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
        bridge(json!({}), MockOptions::default()),
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
    let creates: Arc<parking_lot::Mutex<Vec<Vec<Value>>>> = Arc::default();
    let log = creates.clone();
    let opts = MockOptions::default().resolver("CreateWorkspace", move |a, _, _| {
        log.lock().push(a.to_vec());
        Reply::value(json!({
            "id": "WS-NEW", "projectId": a[0], "type": "solo", "goal": "New Workspace",
            "short": "New Workspace", "statusLabel": "Creating", "phase": "creating",
            "branch": "recur/new", "baseBranch": "main",
        }))
    });
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
        bridge(json!({}), MockOptions::default()),
        LocalStorage::in_memory(),
    );
    h.cx.update(|window, cx| window.blur(cx));
    h.settle();
    h.keys("secondary-k");
    assert!(h.read(|s| s.overlays.palette));
}

#[gpui_kit::test]
fn a_failed_save_shows_an_error_toast_that_auto_dismisses(cx: &mut TestAppContext) {
    let opts = MockOptions::default().resolver("UpdateProjectRunConfig", |_, _, _| {
        Reply::error("disk is read-only")
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
        bridge(patch, MockOptions::default()),
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
    assert_eq!(h.seed_project("storefront")["groupId"], "g-a");
    let order: Vec<String> = h.bridge.with_seed(|s| {
        s.projects
            .iter()
            .map(|p| p["id"].as_str().unwrap_or_default().to_string())
            .collect()
    });
    assert_eq!(
        &order[..2],
        ["storefront", "payments"],
        "dropped above payments"
    );
    assert!(h.has("ph:storefront"));
}

/// Navigate → Reload Bundles (`lib/bundle-reload.ts` and App.svelte's toasts): the catalog is
/// re-read, bundle-derived state rebuilds off the epoch, and skipped bundles are reported.
#[gpui::test]
fn reload_bundles_rereads_the_catalog_and_reports_skipped_bundles(cx: &mut TestAppContext) {
    let calls = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let skipped = Arc::new(parking_lot::Mutex::new(json!([])));
    let (c1, c2, s2) = (calls.clone(), calls.clone(), skipped.clone());
    let opts = MockOptions::default()
        .resolver("InvalidateCatalog", move |_, _, _| {
            c1.lock().push("InvalidateCatalog".into());
            Reply::null()
        })
        .resolver("ListSkippedBundles", move |_, _, _| {
            c2.lock().push("ListSkippedBundles".into());
            Reply::value(s2.lock().clone())
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
    *skipped.lock() =
        json!([{"name": "broken", "reason": "bad toml"}, {"name": "old", "reason": "x"}]);
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
    let (u1, u2) = (usage.clone(), usage);
    let opts = MockOptions::default()
        .resolver("GetProviderUsage", move |_, _, _| Reply::value(u1.clone()))
        .resolver("RefreshProviderUsage", move |_, _, _| Reply::value(u2.clone()));
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
        bridge(json!({}), MockOptions::default()),
        LocalStorage::in_memory(),
    );
    h.hover("main:storefront");
    assert!(h.has("main:storefront"));
    for id in SEED_LOOPS {
        assert!(!h.has(&format!("rm:{id}")) && !h.has(&format!("pr:{id}")));
    }
}
