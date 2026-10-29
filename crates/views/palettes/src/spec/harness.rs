//! The shell with the desktop's views and the palettes, on the mock bridge over the e2e seed,
//! driven by `debug_selector` keys.

use std::sync::Arc;
use std::time::Duration;

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, px};
use gpui_kit::{
    AppContext as _, Bounds, Context, Focusable, IntoElement, ParentElement, Pixels, Render,
    Styled, Window, div,
};
use parking_lot::Mutex;
use recur_app::{Shell, ShellOptions};
use recur_services::mock::{MockBridge, MockOptions, Reply, Seed};
use recur_ui::widgets::{ToastKind, ToastStore};
use recur_view_core::{AppState, LocalStorage, ViewContext, ViewName, ViewRegistry};
use serde_json::{Value, json};

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
    pub cx: &'a mut VisualTestContext,
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

pub fn boot(cx: &mut TestAppContext, seed: impl FnOnce(&mut Seed), opts: MockOptions) -> S<'_> {
    let registry = registry();
    cx.update(|cx| {
        recur_app::init(&registry, cx);
        recur_ui::motion::freeze(None, cx);
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
    let (root, cx) = cx.add_window_view(move |window, cx| {
        let shell = cx.new(|cx| Shell::new(ctx, registry, ShellOptions::default(), window, cx));
        Root { shell }
    });
    // The specs' window size.
    cx.simulate_resize(gpui::size(px(1380.), px(880.)));
    cx.run_until_parked();
    let shell = root.read_with(cx, |r, _| r.shell.clone());
    cx.update(|window, cx| {
        window.activate_window();
        let f = shell.focus_handle(cx);
        window.focus(&f, cx);
    });
    let mut s = S { state, cx };
    s.settle();
    s
}

pub fn plain(cx: &mut TestAppContext) -> S<'_> {
    boot(cx, |_| {}, MockOptions::default())
}

/// The storefront project with these `[[run]]` commands (`seedWithRuns`).
pub fn with_runs(runs: &[(&str, &str)]) -> impl FnOnce(&mut Seed) {
    let runs: Vec<Value> = runs
        .iter()
        .map(|(n, c)| json!({"name": n, "cmd": c}))
        .collect();
    move |s: &mut Seed| {
        if let Some(p) = s.projects.iter_mut().find(|p| p["id"] == "storefront") {
            p["runCommands"] = json!(runs);
        }
    }
}

/// One recorded bound call: its method name and arguments.
pub type Call = (String, Vec<Value>);

/// Bound calls by name, recorded.
#[derive(Clone, Default)]
pub struct Calls(pub Arc<Mutex<Vec<Call>>>);

impl Calls {
    pub fn named(&self, name: &str) -> Vec<Vec<Value>> {
        self.0
            .lock()
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, a)| a.clone())
            .collect()
    }

    /// Record `name`, answering `reply`.
    pub fn record(&self, opts: MockOptions, name: &'static str, reply: Value) -> MockOptions {
        let calls = self.0.clone();
        opts.resolver(name, move |a, _, _| {
            calls.lock().push((name.into(), a.to_vec()));
            Reply::value(reply.clone())
        })
    }

    /// `terminalResolvers`: a fresh session per OpenTerminal, recorded writes and interrupts.
    pub fn terminals(&self, opts: MockOptions) -> MockOptions {
        let calls = self.0.clone();
        let n = Arc::new(Mutex::new(0u32));
        let opts = opts.resolver("OpenTerminal", move |a, _, _| {
            calls.lock().push(("OpenTerminal".into(), a.to_vec()));
            let mut n = n.lock();
            *n += 1;
            Reply::value(json!({
                "id": format!("sess-{n}"), "workspaceId": a.first().cloned().unwrap_or_default(),
                "cwd": "/tmp/wt", "shell": "/bin/zsh", "title": "zsh",
                "createdAt": "2026-01-01T00:00:00Z",
            }))
        });
        let opts = self.record(opts, "AttachTerminal", json!(""));
        let opts = self.record(opts, "WriteTerminal", Value::Null);
        let opts = self.record(opts, "InterruptTerminal", Value::Null);
        self.record(opts, "ResizeTerminal", Value::Null)
    }
}

impl S<'_> {
    /// Runs to idle, delivering frames as the platform would.
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

    pub fn hover(&mut self, sel: &str) {
        let p = self
            .bounds(sel)
            .unwrap_or_else(|| panic!("no element {sel}"))
            .center();
        // Two moves: the hover gate arms on real movement.
        self.cx
            .simulate_mouse_move(p - gpui::point(px(10.), px(0.)), None, Modifiers::none());
        self.settle();
        self.cx.simulate_mouse_move(p, None, Modifiers::none());
        self.settle();
    }

    pub fn click(&mut self, sel: &str) {
        let p = self
            .bounds(sel)
            .unwrap_or_else(|| panic!("no element {sel}"))
            .center();
        self.cx.simulate_mouse_move(p, None, Modifiers::none());
        self.settle();
        self.cx.simulate_click(p, Modifiers::none());
        self.settle();
    }

    /// Clicks a palette row, selected (`…:sel`) or not.
    pub fn click_row(&mut self, sel: &str) {
        let on = format!("{sel}:sel");
        if self.has(&on) {
            self.click(&on);
        } else {
            self.click(sel);
        }
    }

    /// Types into the focused field, one character at a time.
    pub fn type_text(&mut self, s: &str) {
        for ch in s.chars() {
            self.cx.simulate_input(&ch.to_string());
            self.cx.run_until_parked();
        }
        self.settle();
    }

    /// `fill`: select all in the focused field and type `s`.
    pub fn fill(&mut self, s: &str) {
        self.cx.simulate_keystrokes("secondary-a backspace");
        self.type_text(s);
    }

    pub fn keys(&mut self, keys: &str) {
        self.cx.simulate_keystrokes(keys);
        self.settle();
    }

    pub fn wait(&mut self, ms: u64) {
        self.cx.executor().advance_clock(Duration::from_millis(ms));
        self.settle();
    }

    pub fn until(&mut self, f: impl Fn(&mut Self) -> bool) -> bool {
        for _ in 0..40 {
            if f(self) {
                return true;
            }
            self.wait(50);
        }
        f(self)
    }

    pub fn read<T>(&mut self, f: impl FnOnce(&AppState) -> T) -> T {
        self.state.read_with(self.cx, |s, _| f(s))
    }

    pub fn view(&mut self) -> ViewName {
        self.read(|s| s.view)
    }

    pub fn open_workspace(&mut self, id: &str) {
        self.state
            .update(self.cx, |s, cx| s.open_workspace(id, false, cx));
        self.wait(300);
    }

    /// The window's focused element is a palette's query field.
    pub fn input_focused(&mut self) -> bool {
        self.cx
            .update(|window, _| window.context_stack().iter().any(|c| c.contains("Input")))
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

    pub fn storage(&mut self, key: &str) -> Option<String> {
        self.read(|s| s.storage().get(key).map(str::to_string))
    }

    /// Rows of the palette with prefix `prefix` (`pal-row:` …), top to bottom, with `:sel`.
    pub fn rows(&mut self, prefix: &str, labels: &[&str]) -> Vec<String> {
        let mut found: Vec<(f32, String)> = Vec::new();
        for l in labels {
            for suffix in ["", ":sel"] {
                let sel = format!("{prefix}{l}{suffix}");
                if let Some(b) = self.bounds(&sel) {
                    found.push((f32::from(b.origin.y), format!("{l}{suffix}")));
                }
            }
        }
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        found.into_iter().map(|(_, l)| l).collect()
    }
}
