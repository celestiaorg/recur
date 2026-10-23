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
    // playwright.config.ts's viewport.
    boot_sized(cx, seed, opts, 1380., 880.)
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

pub fn plain_with(cx: &mut TestAppContext, seed: impl FnOnce(&mut Seed)) -> S<'_> {
    boot(cx, seed, MockOptions::default())
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

    /// Scrolls an off-screen `sel` to the middle of the window, as Playwright scrolls a target
    /// into view before acting on it.
    pub fn reveal(&mut self, sel: &str) {
        for _ in 0..12 {
            let Some(b) = self.bounds(sel) else { return };
            let vh = f32::from(self.cx.update(|w, _| w.viewport_size().height));
            if f32::from(b.origin.y) >= 0. && f32::from(b.bottom()) <= vh {
                return;
            }
            let dy = vh / 2. - f32::from(b.center().y);
            self.cx.simulate_event(gpui::ScrollWheelEvent {
                position: point(b.center().x, px(vh / 2.)),
                delta: gpui::ScrollDelta::Pixels(point(px(0.), px(dy))),
                ..Default::default()
            });
            self.settle();
            if self.bounds(sel).is_some_and(|n| n.origin.y == b.origin.y) {
                return;
            }
        }
    }

    /// Scrolls `sel` fully inside the `pane` that clips it, vertically and then sideways
    /// (a tab strip scrolls under the wheel at the tab's row).
    pub fn reveal_within(&mut self, sel: &str, pane: &str) {
        for _ in 0..12 {
            let (Some(b), Some(p)) = (self.bounds(sel), self.bounds(pane)) else { return };
            let (dx, dy, at) = if b.origin.y < p.origin.y || b.bottom() > p.bottom() {
                (0., f32::from(p.center().y) - f32::from(b.center().y), p.center())
            } else if b.origin.x < p.origin.x || b.right() > p.right() {
                let dx = f32::from(p.center().x) - f32::from(b.center().x);
                (dx, 0., point(p.center().x, b.center().y))
            } else {
                return;
            };
            self.cx.simulate_event(gpui::ScrollWheelEvent {
                position: at,
                delta: gpui::ScrollDelta::Pixels(point(px(dx), px(dy))),
                ..Default::default()
            });
            self.settle();
            if self.bounds(sel).is_some_and(|n| n.origin == b.origin) {
                return;
            }
        }
    }

    /// Clicks a dock chat tab, scrolling the strip to it first.
    pub fn click_chip(&mut self, code: &str) {
        let chip = self.chip(code).unwrap_or_else(|| panic!("no chat tab {code}"));
        self.reveal_within(&chip, "prd-agent");
        let chip = self.chip(code).expect(code);
        self.click(&chip);
    }

    pub fn click(&mut self, sel: &str) {
        self.reveal(sel);
        let p = self
            .bounds(sel)
            .unwrap_or_else(|| panic!("no element {sel}"))
            .center();
        self.cx.simulate_mouse_move(p, None, Modifiers::none());
        self.cx.simulate_click(p, Modifiers::none());
        self.settle();
    }

    pub fn hover(&mut self, sel: &str) {
        self.reveal(sel);
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
        self.reveal(from);
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

impl S<'_> {
    /// A PR detail tab by its label (`Conversation`, `Commits`, `Files changed`, `Checks`).
    pub fn tab(&mut self, name: &str) {
        for state in ["on:lit", "on", "off"] {
            let sel = format!("prtab:{name}:{state}");
            if self.has(&sel) {
                self.click(&sel);
                return;
            }
        }
        panic!("no tab {name}");
    }

    /// `prReviewDraft`.
    pub fn review_draft(&mut self) -> String {
        self.cx.update(|_, cx| crate::ui::ui(cx).read(cx).review_draft.clone())
    }

    /// The open typeahead's logins or emoji names.
    pub fn suggestions(&mut self, kind: recur_view_chat::suggest::SuggestKind) -> Vec<String> {
        use recur_view_chat::suggest::{SuggestItem, SuggestStore};
        self.cx.update(|_, cx| {
            let store = SuggestStore::global(cx);
            store
                .read(cx)
                .slot(kind)
                .map(|s| {
                    s.items
                        .iter()
                        .map(|i| match i {
                            SuggestItem::Person(p) => p.login.clone(),
                            SuggestItem::Emoji(h) => h.name.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
    }
}

/// A shared findings list behind `ListPRFindings` / `SetFindingStatus`, counting settles.
#[derive(Clone, Default)]
pub struct Findings {
    pub list: Arc<parking_lot::Mutex<Vec<Value>>>,
    pub settled: Arc<std::sync::atomic::AtomicUsize>,
}

impl Findings {
    pub fn new(list: Vec<Value>) -> Self {
        Findings {
            list: Arc::new(parking_lot::Mutex::new(list)),
            ..Default::default()
        }
    }

    pub fn settled(&self) -> usize {
        self.settled.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// `ListPRFindings` answers the open ones; `SetFindingStatus` records the status, failing
    /// the first `fail_first` calls.
    pub fn options(&self, fail_first: usize) -> MockOptions {
        let (list, list2, settled) = (self.list.clone(), self.list.clone(), self.settled.clone());
        MockOptions::default()
            .resolver("ListPRFindings", move |_, _, _| {
                let open: Vec<Value> = list.lock().iter().filter(|f| f["status"] == "open").cloned().collect();
                recur_services::mock::Reply::value(Value::Array(open))
            })
            .resolver("SetFindingStatus", move |a, _, _| {
                let n = settled.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if n <= fail_first {
                    return recur_services::mock::Reply::error("nope");
                }
                let id = a.get(1).and_then(Value::as_str).unwrap_or("").to_string();
                let status = a.get(2).and_then(Value::as_str).unwrap_or("").to_string();
                let mut l = list2.lock();
                let f = l.iter_mut().find(|f| f["id"] == id.as_str());
                let out = match f {
                    Some(f) => {
                        f["status"] = json!(status);
                        f.clone()
                    }
                    None => json!({ "id": id, "status": status }),
                };
                recur_services::mock::Reply::value(out)
            })
    }
}

/// `f-race` on `useOAuthSession.ts:2`.
pub fn race_finding(workspace: &str) -> Value {
    json!({
        "id": "f-race", "workspaceId": workspace, "file": "src/hooks/useOAuthSession.ts", "line": 2,
        "message": "token refresh can race across two tabs", "status": "open", "severity": "high",
    })
}

/// PR #482's seeded comments plus a submitted inline note matching `f-race`.
pub fn comments_with_race_note(id: &str) -> Value {
    let mut c = recur_services::mock::fixtures::seed().pr_by_id("482").unwrap()["comments"].clone();
    c.as_array_mut().unwrap().push(json!({
        "id": id, "code": "AR", "author": "Avery Rourke", "authorLogin": "avery", "time": "1m",
        "bodyMd": "token refresh can race across two tabs", "parentId": "gh-review-99",
        "path": "src/hooks/useOAuthSession.ts", "line": 2, "side": "RIGHT",
    }));
    c
}

impl S<'_> {
    /// The workspace view PR detail's agent dock shows (the shell's bound workspace).
    pub fn dock(&mut self) -> Option<Entity<recur_view_workspace::view::WorkspaceView>> {
        self.cx.update(|_, cx| {
            recur_view_workspace::detail::WorkspaceDetail::current(cx).and_then(|d| d.read(cx).bound_view(cx))
        })
    }

    pub fn dock_ws(&mut self) -> String {
        let v = self.dock().expect("dock");
        self.cx.update(|_, cx| v.read(cx).workspace_id().to_string())
    }

    pub fn dock_codes(&mut self) -> Vec<String> {
        let v = self.dock().expect("dock");
        self.cx.update(|_, cx| v.read(cx).tab_codes())
    }

    pub fn dock_active(&mut self) -> String {
        let v = self.dock().expect("dock");
        self.cx.update(|_, cx| v.read(cx).active_code())
    }

    /// `fill` the dock's composer and press Send.
    pub fn dock_send(&mut self, text: &str) {
        let v = self.dock().expect("dock");
        let t = text.to_string();
        self.cx.update(|window, cx| {
            let chat = v.read(cx).chat().clone();
            chat.update(cx, |c, cx| {
                c.set_draft(t, Vec::new(), cx);
                c.focus_composer(window, cx);
            });
        });
        self.settle();
        self.click("chat-send");
    }

    /// A user row in the shown transcript.
    pub fn me(&mut self, body: &str) -> bool {
        self.has(&format!("me:{}", recur_view_chat::chat::debug_key(body)))
    }

    /// The chat tab chip for `code`, as shown (`:on` while active).
    pub fn chip(&mut self, code: &str) -> Option<String> {
        let base = format!("tab-chip:chat-tab-{code}");
        let on = format!("{base}:on");
        if self.has(&on) {
            Some(on)
        } else if self.has(&base) {
            Some(base)
        } else {
            None
        }
    }

    pub fn attention(&mut self) -> recur_services::mock::ChatAttention {
        self.bridge.attention()
    }

    /// The PR nav's running and unseen counts, as the sidebar computes them.
    pub fn pr_nav(&mut self) -> (usize, i64) {
        self.state.read_with(self.cx, |s, _| {
            (
                recur_view_core::pr::pr_chat_host_running_count(&s.workspaces, &s.workspaces_running),
                recur_view_core::pr::pr_chat_host_unseen_count(&s.workspaces, &s.attention.by_workspace),
            )
        })
    }
}
