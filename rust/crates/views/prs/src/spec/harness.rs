//! The shell with every view registered, as the desktop mounts it, on the mock bridge over the
//! e2e seed, with selector-driven input (`debug_selector` keys) for the `pr-review.spec.ts`
//! and `activity.spec.ts` ports.

use std::sync::Arc;
use std::time::Duration;

use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use gpui_kit::{Bounds, Focusable, Pixels};
use recur_app::{Shell, ShellOptions};
use recur_domain::Pr;
use recur_services::mock::{MockBridge, MockOptions, Seed};
use recur_ui::widgets::{ToastKind, ToastStore};
use recur_view_core::{AppState, LocalStorage, ViewContext, ViewName};
use serde_json::{Value, json};

/// The seed's richest PR (commits, files, diffs, thread).
pub const RICH: &str = "482";

pub struct S<'a> {
    pub state: Entity<AppState>,
    pub bridge: Arc<MockBridge>,
    pub cx: &'a mut VisualTestContext,
}

pub fn boot<'a>(
    cx: &'a mut TestAppContext,
    seed: impl FnOnce(&mut Seed),
    opts: MockOptions,
) -> S<'a> {
    boot_sized(cx, seed, opts, 1280., 800.)
}

pub fn boot_sized<'a>(
    cx: &'a mut TestAppContext,
    seed: impl FnOnce(&mut Seed),
    opts: MockOptions,
    w: f32,
    h: f32,
) -> S<'a> {
    let registry = crate::gallery::registry();
    cx.update(|cx| {
        recur_app::init(&registry, cx);
        recur_ui::motion::freeze(None, cx);
        recur_content::markdown::set_markdown_render_inline(true);
        crate::ui::reset(cx);
    });
    let mut sd = recur_services::mock::fixtures::seed();
    seed(&mut sd);
    let bridge = MockBridge::new(sd, opts);
    let services = bridge.services();
    let state = cx.update(|cx| AppState::install(services.clone(), LocalStorage::in_memory(), cx));
    let ctx = ViewContext {
        state: state.clone(),
        services,
    };
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        Shell::new(ctx, registry, ShellOptions::default(), window, cx)
    });
    cx.simulate_resize(gpui::size(px(w), px(h)));
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

pub fn plain(cx: &mut TestAppContext) -> S<'_> {
    boot(cx, |_| {}, MockOptions::default())
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// `patchRichPR`: merge `over` into PR #482 in both `prs` and `prById`.
pub fn patch_pr(s: &mut Seed, id: &str, over: Value) {
    let merge = |v: &mut Value| {
        if let (Some(a), Some(b)) = (v.as_object_mut(), over.as_object()) {
            for (k, x) in b {
                a.insert(k.clone(), x.clone());
            }
        }
    };
    let ids: Vec<_> = s.pr_ids().to_vec();
    for o in ids {
        if s.obj(o)["id"] == id {
            merge(s.obj_mut(o));
        }
    }
    if let Some(v) = s.pr_by_id_mut(id) {
        merge(v);
    }
}

/// A list row in the e2e shape (`row(over)`).
pub fn pr_row(over: Value) -> Value {
    let mut base = json!({
        "projectId": "storefront", "workspaceId": "", "group": "open", "source": "gh",
        "branch": "feat/x", "base": "main", "state": "In review", "diff": {"add": 1, "del": 0},
        "fileCount": 1, "checksOk": true, "when": "1h",
        "comments": [], "commits": [], "files": [], "checks": [],
    });
    if let (Some(a), Some(b)) = (base.as_object_mut(), over.as_object()) {
        for (k, x) in b {
            a.insert(k.clone(), x.clone());
        }
    }
    base
}

/// `addHostPR`: a GitHub PR with no workspace and its hidden chat host.
pub fn add_host_pr(s: &mut Seed, id: &str, number: i64, title: &str) -> String {
    s.insert_pr(pr_row(json!({
        "id": id, "number": number, "title": title, "branch": format!("feat/{id}"),
    })));
    let host = format!("WS-HOST-{id}");
    s.insert_loop(json!({
        "id": host, "projectId": "storefront", "type": "solo", "hostPrId": id, "goal": title,
        "short": title, "statusLabel": "Review", "phase": "editing", "worktreePath": "/tmp/repo",
        "baseBranch": "main", "archived": false,
    }));
    host
}

/// `addIdlePRWorkspace`.
pub fn add_idle_pr_workspace(s: &mut Seed, pr_id: &str) -> String {
    let id = format!("WS-IDLE-{pr_id}");
    s.insert_loop(json!({
        "id": id, "projectId": "storefront", "type": "solo", "goal": "Idle PR workspace",
        "short": "Idle PR workspace", "statusLabel": "Idle", "phase": "editing",
        "worktreePath": "/tmp/from-pr", "baseBranch": "main", "hostPrId": "",
    }));
    patch_pr(s, pr_id, json!({ "workspaceId": id }));
    id
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

    pub fn hover(&mut self, sel: &str) {
        let p = self
            .bounds(sel)
            .unwrap_or_else(|| panic!("no element {sel}"))
            .center();
        self.cx
            .simulate_mouse_move(point(p.x + px(1.), p.y), None, Modifiers::none());
        self.cx.simulate_mouse_move(p, None, Modifiers::none());
        self.settle();
    }

    /// Press on `from`, move to `to`, release (`dragTo`).
    pub fn drag(&mut self, from: &str, to: &str) {
        let a = self.bounds(from).expect(from).center();
        let b = self.bounds(to).expect(to).center();
        self.cx.simulate_mouse_move(a, None, Modifiers::none());
        self.cx
            .simulate_mouse_down(a, MouseButton::Left, Modifiers::none());
        self.settle();
        for i in 1..=4 {
            let t = i as f32 / 4.;
            let p = point(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
            self.cx
                .simulate_mouse_move(p, Some(MouseButton::Left), Modifiers::none());
            self.settle();
        }
        self.cx
            .simulate_mouse_up(b, MouseButton::Left, Modifiers::none());
        self.settle();
    }

    pub fn keys(&mut self, keys: &str) {
        self.cx.simulate_keystrokes(keys);
        self.settle();
    }

    /// Type into the focused field, one character at a time.
    pub fn type_text(&mut self, s: &str) {
        for ch in s.chars() {
            self.cx.simulate_input(&ch.to_string());
            self.cx.run_until_parked();
        }
        self.settle();
    }

    pub fn wait(&mut self, ms: u64) {
        self.cx.executor().advance_clock(Duration::from_millis(ms));
        self.settle();
    }

    /// Lets frames pass until `f` holds.
    pub fn until(&mut self, f: impl Fn(&mut Self) -> bool) -> bool {
        for _ in 0..40 {
            if f(self) {
                return true;
            }
            self.wait(50);
        }
        f(self)
    }

    pub fn view(&mut self) -> ViewName {
        self.state.read_with(self.cx, |s, _| s.view)
    }

    pub fn pr(&mut self, id: &str) -> Pr {
        self.state
            .read_with(self.cx, |s, _| s.pr(id).cloned())
            .expect("pr")
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

    pub fn wire(&mut self, name: &str, data: Value) {
        self.bridge.dispatch_wire(name, data).unwrap();
        self.settle();
    }

    /// The sidebar's Pull requests nav.
    pub fn prs_nav(&mut self) {
        self.click("nav-pr");
        assert_eq!(self.view(), ViewName::Pr);
    }

    /// A lifecycle segment (`Open`, `Draft`, `Merged`, `Closed`, `All`).
    pub fn seg(&mut self, label: &str) {
        let sel = if self.has(&format!("seg:{label}:true")) {
            format!("seg:{label}:true")
        } else {
            format!("seg:{label}:false")
        };
        self.click(&sel);
    }

    /// Pull requests → All → the row.
    pub fn open_pr(&mut self, id: &str) {
        self.prs_nav();
        self.seg("All");
        self.click(&format!("prrow:{id}"));
        assert_eq!(self.view(), ViewName::PrDetail);
    }
}
