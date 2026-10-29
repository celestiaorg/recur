//! The Observatory in a window as the workspace view mounts it (LOOP-2231, workspace mode,
//! the agent chat in the left dock), over the e2e mock bridge, with selector-driven input
//! (`debug_selector` keys).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use gpui_kit::{
    AnyView, AppContext as _, Bounds, Context, InteractiveElement, IntoElement, ParentElement,
    Pixels, Render, Styled, Subscription, Window, div,
};
use recur_services::mock::{MockBridge, MockOptions};
use recur_view_core::{AppState, LocalStorage};

use crate::obs::{Observatory, ObservatoryEvent, ObservatoryProps, ObservatorySlots};

pub const WS: &str = "LOOP-2231";
pub const PROJECT: &str = "storefront";

pub fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_ui::init(cx);
        recur_terminal_view::init(cx);
        // Finite animations (the maximize FLIP) settle at once, as in the parity captures.
        recur_ui::motion::freeze(None, cx);
        cx.set_global(recur_view_review::store::ReviewStore::default());
    });
}

/// The agent chat the workspace view puts in the left dock.
pub struct AgentStub;

impl Render for AgentStub {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().debug_selector(|| "agent-chat".into()).size_full()
    }
}

/// The workspace view's side: owns the chat dock's open flag (`onToggleLeft`) and records
/// every event.
pub struct Host {
    pub obs: Entity<Observatory>,
    _sub: Subscription,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::style::window_root(
            recur_ui::style::body(cx)
                .size_full()
                .flex()
                .flex_col()
                .child(self.obs.clone()),
        )
    }
}

pub struct H<'a> {
    pub state: Entity<AppState>,
    pub obs: Entity<Observatory>,
    pub bridge: Arc<MockBridge>,
    pub events: Rc<RefCell<Vec<ObservatoryEvent>>>,
    pub chat_open: Rc<RefCell<std::collections::HashMap<String, bool>>>,
    pub cx: &'a mut VisualTestContext,
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// The workspace view's props for LOOP-2231.
pub fn workspace_props() -> ObservatoryProps {
    ObservatoryProps {
        project_id: PROJECT.into(),
        workspace_id: WS.into(),
        loop_scoped: true,
        left_open: Some(true),
        left_title: "Agent".into(),
        agent_tab_count: 1,
        agent_tabs: true,
        can_fix_findings: true,
        can_add_editor_mention: true,
        shell_active: true,
        ..Default::default()
    }
}

/// Mount the Observatory over `opts` (on top of the e2e resolvers), with `storage` set before
/// it boots.
pub fn mount<'a>(
    cx: &'a mut TestAppContext,
    opts: MockOptions,
    storage: impl FnOnce(&mut LocalStorage),
    props: impl FnOnce(&mut ObservatoryProps),
) -> H<'a> {
    mount_seeded(cx, |_| {}, opts, storage, props)
}

/// [`mount`] over the e2e seed edited by `seed`.
pub fn mount_seeded<'a>(
    cx: &'a mut TestAppContext,
    seed: impl FnOnce(&mut recur_services::mock::Seed),
    opts: MockOptions,
    storage: impl FnOnce(&mut LocalStorage),
    props: impl FnOnce(&mut ObservatoryProps),
) -> H<'a> {
    init(cx);
    let mut sd = recur_services::mock::fixtures::base_seed().clone();
    seed(&mut sd);
    let bridge = MockBridge::new(sd, opts);
    let mut st = LocalStorage::in_memory();
    storage(&mut st);
    let services = bridge.services();
    let state = cx.update(|cx| AppState::install(services, st, cx));
    let mut p = workspace_props();
    props(&mut p);
    let s2 = state.clone();
    let events: Rc<RefCell<Vec<ObservatoryEvent>>> = Rc::default();
    let ev2 = events.clone();
    let (host, vcx) = cx.add_window_view(|window, cx| {
        s2.update(cx, |s, cx| {
            s.view = recur_view_core::ViewName::Detail;
            s.wire_events(cx);
            s.load_initial(cx).detach();
        });
        let slots = ObservatorySlots {
            left_panel: Some(AnyView::from(cx.new(|_| AgentStub))),
            ..Default::default()
        };
        let obs = cx.new(|cx| Observatory::new(s2.clone(), p, slots, window, cx));
        // Specs let a panic in the callback fail the test.
        #[allow(clippy::disallowed_methods)]
        let sub = cx.subscribe_in(
            &obs,
            window,
            move |_, obs, ev: &ObservatoryEvent, window, cx| {
                ev2.borrow_mut().push(ev.clone());
                if let ObservatoryEvent::ToggleLeft(open) = ev {
                    let open = *open;
                    obs.update(cx, |o, cx| {
                        let mut p = o.props().clone();
                        p.left_open = Some(open);
                        o.set_props(p, window, cx);
                    });
                }
            },
        );
        Host { obs, _sub: sub }
    });
    let obs = host.read_with(vcx, |h, _| h.obs.clone());
    vcx.simulate_resize(gpui::size(px(1280.), px(800.)));
    let mut h = H {
        state,
        obs,
        bridge,
        events,
        chat_open: Rc::default(),
        cx: vcx,
    };
    h.settle();
    h
}

/// The backend's editor-session store (`GetEditorSession` / `UpdateEditorSession`): meta
/// replaces wholesale; buffers and scratch docs upsert by path.
#[derive(Clone, Default)]
pub struct SessionStore(pub Arc<parking_lot::Mutex<serde_json::Map<String, serde_json::Value>>>);

impl SessionStore {
    pub fn install(&self, opts: MockOptions) -> MockOptions {
        let (get, put) = (self.0.clone(), self.0.clone());
        opts.resolver("GetEditorSession", move |a, _, _| {
            let ws = a.first().and_then(serde_json::Value::as_str).unwrap_or("");
            recur_services::mock::Reply::value(get.lock().get(ws).cloned().unwrap_or_default())
        })
        .resolver("UpdateEditorSession", move |a, _, _| {
            let ws = a
                .first()
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let patch = a.get(1).cloned().unwrap_or_default();
            let mut all = put.lock();
            let entry = all.entry(ws).or_insert_with(|| serde_json::json!({}));
            if let Some(meta) = patch.get("meta").and_then(serde_json::Value::as_object) {
                let keep_buffers = entry.get("buffers").cloned();
                let keep_scratch = entry.get("scratchDocs").cloned();
                *entry = serde_json::Value::Object(meta.clone());
                if let Some(b) = keep_buffers {
                    entry["buffers"] = b;
                }
                if let Some(d) = keep_scratch {
                    entry["scratchDocs"] = d;
                }
            }
            for key in ["buffers", "scratchDocs"] {
                if let Some(m) = patch.get(key).and_then(serde_json::Value::as_object) {
                    if !entry.get(key).is_some_and(serde_json::Value::is_object) {
                        entry[key] = serde_json::json!({});
                    }
                    for (k, v) in m {
                        entry[key][k] = v.clone();
                    }
                }
            }
            for (key, drop) in [
                ("buffers", "dropBuffers"),
                ("scratchDocs", "dropScratchDocs"),
            ] {
                if let Some(list) = patch.get(drop).and_then(serde_json::Value::as_array)
                    && let Some(m) = entry
                        .get_mut(key)
                        .and_then(serde_json::Value::as_object_mut)
                {
                    for p in list.iter().filter_map(serde_json::Value::as_str) {
                        m.remove(p);
                    }
                }
            }
            recur_services::mock::Reply::null()
        })
        .resolver("ClearEditorSession", {
            let all = self.0.clone();
            move |a, _, _| {
                let ws = a.first().and_then(serde_json::Value::as_str).unwrap_or("");
                all.lock().remove(ws);
                recur_services::mock::Reply::null()
            }
        })
    }
}

impl H<'_> {
    /// Quit: write the pending editor session (the quit journal's flush). Remount in a
    /// second app context for the restart.
    pub fn quit(self) {
        let obs = self.obs.clone();
        self.cx
            .update(|window, cx| obs.update(cx, |o, cx| o.page_hide(window, cx)));
        self.cx
            .update(|_, cx| crate::session_host::flush_editor_session_now(cx));
        self.cx.run_until_parked();
    }

    pub fn settle(&mut self) {
        for _ in 0..4 {
            self.cx.run_until_parked();
            self.cx.update(|window, _| window.refresh());
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

    fn center(&mut self, sel: &str) -> gpui::Point<Pixels> {
        self.bounds(sel)
            .unwrap_or_else(|| panic!("no element {sel}"))
            .center()
    }

    pub fn click(&mut self, sel: &str) {
        let p = self.center(sel);
        self.click_at(p, Modifiers::none());
    }

    pub fn click_at(&mut self, p: gpui::Point<Pixels>, mods: Modifiers) {
        self.cx.simulate_mouse_move(p, None, mods);
        self.cx.simulate_click(p, mods);
        self.settle();
    }

    pub fn dblclick(&mut self, sel: &str) {
        let p = self.center(sel);
        self.cx.simulate_mouse_move(p, None, Modifiers::none());
        self.cx.simulate_event(gpui::MouseDownEvent {
            position: p,
            button: MouseButton::Left,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        });
        self.cx.simulate_event(gpui::MouseUpEvent {
            position: p,
            button: MouseButton::Left,
            modifiers: Modifiers::none(),
            click_count: 1,
        });
        self.cx.simulate_event(gpui::MouseDownEvent {
            position: p,
            button: MouseButton::Left,
            modifiers: Modifiers::none(),
            click_count: 2,
            first_mouse: false,
        });
        self.cx.simulate_event(gpui::MouseUpEvent {
            position: p,
            button: MouseButton::Left,
            modifiers: Modifiers::none(),
            click_count: 2,
        });
        self.settle();
    }

    pub fn right_click(&mut self, sel: &str) {
        let p = self.center(sel);
        self.cx.simulate_mouse_move(p, None, Modifiers::none());
        self.cx
            .simulate_mouse_down(p, MouseButton::Right, Modifiers::none());
        self.cx
            .simulate_mouse_up(p, MouseButton::Right, Modifiers::none());
        self.settle();
    }

    pub fn hover(&mut self, sel: &str) {
        let p = self.center(sel);
        self.cx
            .simulate_mouse_move(point(p.x + px(1.), p.y), None, Modifiers::none());
        self.cx.simulate_mouse_move(p, None, Modifiers::none());
        self.settle();
    }

    /// Press on `from`, move to `to`, release (`dragTo`).
    pub fn drag(&mut self, from: &str, to: &str) {
        let a = self.center(from);
        let b = self.center(to);
        self.drag_points(a, b);
    }

    pub fn drag_points(&mut self, a: gpui::Point<Pixels>, b: gpui::Point<Pixels>) {
        self.cx.simulate_mouse_move(a, None, Modifiers::none());
        self.cx
            .simulate_mouse_down(a, MouseButton::Left, Modifiers::none());
        self.settle();
        let steps = 6;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let p = point(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
            self.cx
                .simulate_mouse_move(p, Some(MouseButton::Left), Modifiers::none());
            self.settle();
        }
        self.cx
            .simulate_mouse_up(b, MouseButton::Left, Modifiers::none());
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

    pub fn keys(&mut self, keys: &str) {
        self.cx.simulate_keystrokes(keys);
        self.settle();
    }

    pub fn wait(&mut self, ms: u64) {
        self.cx.executor().advance_clock(Duration::from_millis(ms));
        self.settle();
    }

    pub fn read<R>(&mut self, f: impl FnOnce(&Observatory, &gpui_kit::App) -> R) -> R {
        let obs = self.obs.clone();
        obs.read_with(self.cx, f)
    }

    pub fn update<R>(
        &mut self,
        f: impl FnOnce(&mut Observatory, &mut Window, &mut Context<Observatory>) -> R,
    ) -> R {
        let obs = self.obs.clone();
        let r = self
            .cx
            .update(|window, cx| obs.update(cx, |o, cx| f(o, window, cx)));
        self.settle();
        r
    }

    pub fn state_update(&mut self, f: impl FnOnce(&mut AppState, &mut Context<AppState>)) {
        let s = self.state.clone();
        self.cx.update(|_, cx| s.update(cx, f));
        self.settle();
    }

    /// The worktree watcher saw a change (`workspace.fs_changed`).
    pub fn fs_changed(&mut self) {
        self.bridge
            .dispatch(recur_services::AppEvent::WorkspaceFsChanged(
                recur_services::events::WorkspaceFsChanged {
                    workspace_id: WS.into(),
                    ..Default::default()
                },
            ));
        self.settle();
    }

    /// A backend event by its wire name (`dispatchWailsEvent`).
    pub fn event(&mut self, name: &str, data: serde_json::Value) {
        self.bridge.dispatch_wire(name, data).expect("wire event");
        self.settle();
    }

    /// Switch the host to another workspace (or back). Each workspace view keeps its own chat
    /// dock open flag (`chatOpen`).
    pub fn switch_to(&mut self, project: &str, workspace: &str) {
        let (p, w) = (project.to_string(), workspace.to_string());
        let dock = self.chat_open.clone();
        self.update(move |o, window, cx| {
            let mut props = o.props().clone();
            dock.borrow_mut()
                .insert(props.workspace_id.clone(), props.left_open.unwrap_or(true));
            props.left_open = Some(dock.borrow().get(&w).copied().unwrap_or(true));
            props.project_id = p;
            props.workspace_id = w;
            o.set_props(props, window, cx);
        });
        self.wait(100);
    }

    /// The tab strip's visible span of the active chip, in px.
    pub fn active_chip_visible_width(&mut self) -> f32 {
        let strip = self.bounds("obs-tb-center").expect("tab strip");
        let path = self.selected();
        let chip = self
            .bounds(&format!("tab-chip:file-tab:{path}:on"))
            .expect("active chip");
        let l = chip.left().max(strip.left());
        let r = chip.right().min(strip.right());
        f32::from(r - l)
    }

    /// The toasts on screen: kind and text.
    pub fn toasts(&mut self) -> Vec<(recur_ui::widgets::ToastKind, String)> {
        self.cx.update(|_, cx| {
            let store = recur_ui::widgets::ToastStore::global(cx);
            store
                .read(cx)
                .toasts()
                .iter()
                .map(|t| (t.kind, t.text.to_string()))
                .collect()
        })
    }

    pub fn has_toast(&mut self, kind: recur_ui::widgets::ToastKind, needle: &str) -> bool {
        self.toasts()
            .iter()
            .any(|(k, t)| *k == kind && t.contains(needle))
    }

    /// The split editor's text.
    pub fn split_doc(&mut self) -> String {
        let obs = self.obs.clone();
        self.cx.update(|_, cx| {
            obs.read(cx)
                .split_editor
                .as_ref()
                .map(|e| e.read(cx).peek_doc())
                .unwrap_or_default()
        })
    }

    /// The main editor's text.
    pub fn editor_doc(&mut self) -> String {
        let obs = self.obs.clone();
        self.cx.update(|_, cx| {
            obs.read(cx)
                .main_editor
                .as_ref()
                .map(|e| e.read(cx).peek_doc())
                .unwrap_or_default()
        })
    }

    /// Focus the main editor (a click in the code).
    pub fn focus_editor(&mut self) {
        let obs = self.obs.clone();
        self.cx.update(|window, cx| {
            if let Some(e) = obs.read(cx).main_editor.clone() {
                let f = gpui_kit::Focusable::focus_handle(e.read(cx), cx);
                window.focus(&f, cx);
            }
        });
        self.settle();
    }

    /// A tab chip for `path` exists (active or not).
    pub fn has_tab(&mut self, path: &str) -> bool {
        self.has(&format!("tab-chip:file-tab:{path}"))
            || self.has(&format!("tab-chip:file-tab:{path}:on"))
    }

    /// The active tab chip is `path`.
    pub fn active_tab(&mut self, path: &str) -> bool {
        self.has(&format!("tab-chip:file-tab:{path}:on"))
    }

    pub fn open_files(&mut self) -> Vec<String> {
        self.read(|o, _| o.open_files.clone())
    }

    pub fn selected(&mut self) -> String {
        self.read(|o, _| o.selected_path.clone())
    }
}

/// Every call's arguments, recorded by a resolver (`calls.push({ name, args })`).
pub type Calls = Arc<parking_lot::Mutex<Vec<Vec<serde_json::Value>>>>;

/// A resolver that records its calls and answers `reply`.
pub fn record(
    calls: &Calls,
    reply: impl Fn(&[serde_json::Value]) -> recur_services::mock::Reply + Send + Sync + 'static,
) -> impl Fn(
    &[serde_json::Value],
    &mut recur_services::mock::Seed,
    &recur_services::mock::Table,
) -> recur_services::mock::Reply
+ Send
+ Sync
+ 'static {
    let calls = calls.clone();
    move |a, _, _| {
        calls.lock().push(a.to_vec());
        reply(a)
    }
}
