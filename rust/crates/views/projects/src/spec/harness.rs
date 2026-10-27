//! The shell with every view registered, as the desktop mounts it, on the mock bridge over the
//! e2e seed, with selector-driven input (`debug_selector` keys) for the briefing, schedules and
//! project spec ports. A
//! reload builds a fresh app (`AppState`, shell and module state) over the same bridge and
//! local storage.

use std::sync::Arc;
use std::time::Duration;

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, point, px};
use gpui_kit::{
    AppContext as _, Bounds, Context, Focusable, IntoElement, ParentElement, Pixels, Render,
    Styled, Window, div,
};
use recur_app::{Shell, ShellOptions};
use recur_services::mock::{MockBridge, MockOptions, Seed};
use recur_ui::widgets::{ToastKind, ToastStore};
use recur_view_core::{AppState, LocalStorage, ViewContext, ViewName, ViewRegistry};
use serde_json::Value;

pub fn registry() -> ViewRegistry {
    let mut r = ViewRegistry::default();
    recur_view_chat::register(&mut r);
    recur_view_review::register(&mut r);
    recur_view_observatory::register(&mut r);
    recur_view_workspace::register(&mut r);
    recur_view_prs::register(&mut r);
    recur_view_settings::register(&mut r);
    recur_view_home::register(&mut r);
    recur_view_briefing::register(&mut r);
    crate::register(&mut r);
    recur_view_palettes::register(&mut r);
    r
}

pub struct Root {
    pub shell: Entity<Shell>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.shell.clone())
    }
}

pub struct S<'a> {
    pub state: Entity<AppState>,
    pub bridge: Arc<MockBridge>,
    pub root: Entity<Root>,
    pub cx: &'a mut VisualTestContext,
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// The module state a page load starts without.
fn fresh_modules(cx: &mut gpui_kit::App) {
    recur_view_briefing::issues::reset_saved_view(cx);
    cx.set_global(recur_view_widgets::schedule_expanded::ScheduleExpanded::default());
}

pub fn boot<'a>(
    cx: &'a mut TestAppContext,
    seed: impl FnOnce(&mut Seed),
    opts: MockOptions,
    storage: LocalStorage,
) -> S<'a> {
    let registry = registry();
    cx.update(|cx| {
        recur_app::init(&registry, cx);
        recur_ui::motion::freeze(None, cx);
        fresh_modules(cx);
    });
    let mut sd = recur_services::mock::fixtures::seed();
    seed(&mut sd);
    let bridge = MockBridge::new(sd, opts);
    let services = bridge.services();
    let state = cx.update(|cx| AppState::install(services.clone(), storage, cx));
    let ctx = ViewContext {
        state: state.clone(),
        services,
    };
    let (root, cx) = cx.add_window_view(move |window, cx| {
        let shell = cx.new(|cx| Shell::new(ctx, registry, ShellOptions::default(), window, cx));
        Root { shell }
    });
    // playwright.config.ts's viewport.
    cx.simulate_resize(gpui::size(px(1380.), px(880.)));
    cx.run_until_parked();
    let mut s = S {
        state,
        bridge,
        root,
        cx,
    };
    s.focus_shell();
    s.settle();
    s
}

pub fn plain(cx: &mut TestAppContext) -> S<'_> {
    boot(
        cx,
        |_| {},
        MockOptions::default(),
        LocalStorage::in_memory(),
    )
}

pub fn with(cx: &mut TestAppContext, seed: impl FnOnce(&mut Seed), opts: MockOptions) -> S<'_> {
    boot(cx, seed, opts, LocalStorage::in_memory())
}

impl S<'_> {
    fn focus_shell(&mut self) {
        let shell = self.root.read_with(self.cx, |r, _| r.shell.clone());
        self.cx.update(|window, cx| {
            window.activate_window();
            let f = shell.focus_handle(cx);
            window.focus(&f, cx);
        });
    }

    /// `reloadApp`: a fresh app over the same backend and local storage.
    pub fn reload(&mut self) {
        let services = self.bridge.services();
        let old = self.state.clone();
        let root = self.root.clone();
        let state = self.cx.update(|window, cx| {
            let mut st = LocalStorage::in_memory();
            let prev = old.read(cx).storage();
            let pairs: Vec<(String, String)> = prev
                .keys()
                .map(|k| (k.to_string(), prev.get(k).unwrap_or_default().to_string()))
                .collect();
            for (k, v) in pairs {
                st.set(&k, v);
            }
            fresh_modules(cx);
            cx.set_global(recur_view_chat::strip::ChatStripCache::default());
            cx.set_global(recur_view_chat::heap::ChatHeapCache::default());
            cx.set_global(recur_view_chat::transcript_ui::TranscriptUiCache::default());
            cx.set_global(recur_view_chat::hoist::HoistState::default());
            let state = AppState::install(services.clone(), st, cx);
            let ctx = ViewContext {
                state: state.clone(),
                services,
            };
            let shell =
                cx.new(|cx| Shell::new(ctx, registry(), ShellOptions::default(), window, cx));
            root.update(cx, |r, cx| {
                r.shell = shell;
                cx.notify();
            });
            state
        });
        self.state = state;
        self.focus_shell();
        self.settle();
    }

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

    pub fn hover(&mut self, sel: &str) {
        self.reveal(sel);
        let p = self
            .bounds(sel)
            .unwrap_or_else(|| panic!("no element {sel}"))
            .center();
        self.cx.simulate_mouse_move(p, None, Modifiers::none());
        self.settle();
    }

    pub fn click(&mut self, sel: &str) {
        self.click_with(sel, Modifiers::none());
    }

    pub fn click_with(&mut self, sel: &str, m: Modifiers) {
        self.reveal(sel);
        let p = self
            .bounds(sel)
            .unwrap_or_else(|| panic!("no element {sel}"))
            .center();
        self.click_at(p, m);
    }

    pub fn click_at(&mut self, p: gpui::Point<Pixels>, m: Modifiers) {
        self.cx.simulate_mouse_move(p, None, m);
        self.settle();
        self.cx.simulate_click(p, m);
        self.settle();
    }

    pub fn right_click(&mut self, sel: &str) {
        let p = self
            .bounds(sel)
            .unwrap_or_else(|| panic!("no element {sel}"))
            .center();
        self.cx.simulate_mouse_move(p, None, Modifiers::none());
        self.cx.simulate_event(gpui::MouseDownEvent {
            button: gpui::MouseButton::Right,
            position: p,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        });
        self.cx.simulate_event(gpui::MouseUpEvent {
            button: gpui::MouseButton::Right,
            position: p,
            modifiers: Modifiers::none(),
            click_count: 1,
        });
        self.settle();
    }

    /// Clicks `sel`, focusing the field inside it, selects all and types `s` (`fill`).
    pub fn fill(&mut self, sel: &str, s: &str) {
        self.click(sel);
        self.cx.simulate_keystrokes("secondary-a backspace");
        self.type_text(s);
    }

    /// Types into the focused field, one character at a time.
    pub fn type_text(&mut self, s: &str) {
        for ch in s.chars() {
            if ch == '\n' {
                self.cx.simulate_keystrokes("shift-enter");
            } else {
                self.cx.simulate_input(&ch.to_string());
            }
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

    pub fn go(&mut self, v: ViewName) {
        self.state.update(self.cx, |s, cx| s.go(v, None, cx));
        self.settle();
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

    pub fn storage(&mut self, key: &str) -> Option<String> {
        self.state
            .read_with(self.cx, |s, _| s.storage().get(key).map(str::to_string))
    }
}
