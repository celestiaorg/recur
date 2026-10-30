//! The shell with every view registered, as the desktop mounts it, on the typed service fakes over
//! the e2e seed, with selector-driven input (`debug_selector` keys) for the Settings spec ports.

use std::sync::Arc;
use std::time::Duration;

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, point, px};
use gpui_kit::{Bounds, Focusable, Pixels};
use recur_app::{Shell, ShellOptions};
use recur_domain::Settings;
use recur_services::AppEvent;
use recur_services::fake::{Fake, Overrides, Seed};
use recur_ui::widgets::{ToastKind, ToastStore};
use recur_view_core::{AppState, LocalStorage, ViewContext, ViewName, ViewRegistry};

/// The registry the desktop builds, as far as these specs need it.
pub fn registry() -> ViewRegistry {
    let mut r = ViewRegistry::default();
    recur_view_chat::register(&mut r);
    recur_view_review::register(&mut r);
    recur_view_observatory::register(&mut r);
    recur_view_workspace::register(&mut r);
    recur_view_prs::register(&mut r);
    crate::register(&mut r);
    r
}

pub struct S<'a> {
    pub state: Entity<AppState>,
    pub fake: Arc<Fake>,
    pub cx: &'a mut VisualTestContext,
}

pub fn boot<'a>(
    cx: &'a mut TestAppContext,
    seed: impl FnOnce(&mut Seed),
    opts: Overrides,
    storage: LocalStorage,
) -> S<'a> {
    let registry = registry();
    cx.update(|cx| {
        recur_app::init(&registry, cx);
        recur_ui::motion::freeze(cx);
    });
    let mut sd = Seed::e2e();
    seed(&mut sd);
    let fake = Fake::new(sd, opts);
    let services = fake.services();
    let state = cx.update(|cx| AppState::install(services.clone(), storage, cx));
    let ctx = ViewContext {
        state: state.clone(),
        services,
    };
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        Shell::new(ctx, registry, ShellOptions::default(), window, cx)
    });
    // The specs' window size.
    cx.simulate_resize(gpui::size(px(1380.), px(880.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.activate_window();
        let f = shell.focus_handle(cx);
        window.focus(&f, cx);
    });
    let mut s = S { state, fake, cx };
    s.settle();
    s
}

pub fn plain(cx: &mut TestAppContext) -> S<'_> {
    boot(
        cx,
        |_| {},
        Overrides::default(),
        LocalStorage::in_memory(),
    )
}

pub fn with(cx: &mut TestAppContext, seed: impl FnOnce(&mut Seed), opts: Overrides) -> S<'_> {
    boot(cx, seed, opts, LocalStorage::in_memory())
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

    /// Clicks `sel`, focusing the field inside it, selects all and types `s` (`fill`).
    pub fn fill(&mut self, sel: &str, s: &str) {
        self.click(sel);
        self.cx.simulate_keystrokes("secondary-a backspace");
        self.type_text(s);
    }

    /// Types into the focused field, one character at a time.
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

    pub fn toasted(&mut self, kind: ToastKind, text: &str) -> bool {
        self.cx.update(|_, cx| {
            ToastStore::global(cx)
                .read(cx)
                .toasts()
                .iter()
                .any(|t| t.kind == kind && t.text.contains(text))
        })
    }

    pub fn dispatch(&mut self, event: AppEvent) {
        self.fake.dispatch(event);
        self.settle();
    }

    /// The settings the fake holds (what `set_settings` saved).
    pub fn saved(&mut self) -> Settings {
        self.fake.with_seed(|s| s.settings.clone())
    }

    pub fn storage(&mut self, key: &str) -> Option<String> {
        self.state
            .read_with(self.cx, |s, _| s.storage().get(key).map(str::to_string))
    }

    /// The app chrome theme key painted now (`data-theme`, `charcoal` for none).
    pub fn theme(&mut self) -> &'static str {
        self.cx.update(|_, cx| recur_ui::Theme::of(cx).id.key())
    }

    /// `emulateMedia({ colorScheme })`.
    pub fn system_dark(&mut self, dark: bool) {
        self.state
            .update(self.cx, |s, cx| s.set_native_dark(dark, cx));
        self.settle();
    }

    /// `openSettings`: the rail foot's Settings button.
    pub fn open_settings(&mut self) {
        self.click("foot-settings");
        assert_eq!(self.view(), ViewName::Settings);
        assert!(self.has("v-settings:on"));
    }

    /// `openSection`: a nav item, then its heading.
    pub fn section(&mut self, label: &str) {
        let on = format!("set-nav:{label}:on");
        if !self.has(&on) {
            self.click(&format!("set-nav:{label}"));
        }
        assert!(self.has(&on), "{label} not selected");
        assert!(self.has(&format!("set-head={label}")), "{label} heading");
    }

    /// Opens the select labelled `label` showing `shown`, then picks `option`.
    pub fn pick(&mut self, label: &str, shown: &str, option: &str) {
        self.click(&format!("sel:{label}={shown}"));
        self.click(&format!("sel-opt:{option}"));
    }
}
