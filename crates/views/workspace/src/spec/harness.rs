//! The detail route in a window as the shell mounts it (the WorkspaceDetail over the e2e seed
//! on the typed service fakes, a workspace opened), with selector-driven input.

use std::sync::Arc;
use std::time::Duration;

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, point, px};
use gpui_kit::{
    AppContext as _, Bounds, Context, IntoElement, ParentElement, Pixels, Render, Styled, Window,
};
use recur_domain::ChatItem;
use recur_engine::view::chat::AgentMsg;
use recur_services::AppEvent;
use recur_services::fake::{Fake, Overrides, Seed};
use recur_view_chat::chat::{AgentChat, debug_key};
use recur_view_core::{AppState, LocalStorage, ViewContext};

use crate::detail::WorkspaceDetail;
use crate::view::WorkspaceView;

pub const WS: &str = "LOOP-2231";

pub fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_ui::init(cx);
        recur_terminal_view::init(cx);
        recur_ui::motion::freeze(cx);
        recur_content::markdown::set_markdown_render_inline(true);
        cx.set_global(recur_view_review::store::ReviewStore::default());
    });
}

pub struct Root {
    pub detail: Entity<WorkspaceDetail>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::style::window_root(
            recur_ui::style::body(cx)
                .size_full()
                .flex()
                .flex_col()
                .child(self.detail.clone()),
        )
    }
}

pub struct H<'a> {
    pub state: Entity<AppState>,
    pub root: Entity<Root>,
    pub detail: Entity<WorkspaceDetail>,
    pub fake: Arc<Fake>,
    pub cx: &'a mut VisualTestContext,
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// Boot over the e2e seed (edited by `seed`), `opts` on top of the fake's defaults, local
/// storage preset by `storage`, then open `ws`.
pub fn boot_with<'a>(
    cx: &'a mut TestAppContext,
    seed: impl FnOnce(&mut Seed),
    opts: Overrides,
    storage: impl FnOnce(&mut LocalStorage),
    ws: &str,
) -> H<'a> {
    init(cx);
    let mut sd = Seed::e2e();
    seed(&mut sd);
    let fake = Fake::new(sd, opts);
    let mut st = LocalStorage::in_memory();
    storage(&mut st);
    let services = fake.services();
    let state = cx.update(|cx| AppState::install(services.clone(), st, cx));
    let vcx = ViewContext {
        state: state.clone(),
        services,
    };
    let s2 = state.clone();
    let (root, vcx_cx) = cx.add_window_view(move |window, cx| {
        s2.update(cx, |s, cx| {
            s.wire_events(cx);
            s.load_initial(cx).detach();
        });
        let detail = cx.new(|cx| WorkspaceDetail::new(&vcx, window, cx));
        Root { detail }
    });
    let detail = root.read_with(vcx_cx, |r, _| r.detail.clone());
    vcx_cx.simulate_resize(gpui::size(px(1280.), px(800.)));
    vcx_cx.update(|_, cx| install_prompt_runner(cx));
    let mut h = H {
        state,
        root,
        detail,
        fake,
        cx: vcx_cx,
    };
    h.settle();
    h.open(ws);
    h
}

pub fn boot(cx: &mut TestAppContext, opts: Overrides) -> H<'_> {
    boot_with(cx, |_| {}, opts, |_| {}, WS)
}

/// Runs the bundles' compiled TypeScript prompt modules on a JavaScriptCore worker, as the
/// desktop's macro workers do (`exports.default(ctx)` → `{prompt, display, …}`).
pub fn install_prompt_runner(cx: &mut gpui_kit::App) {
    use futures::FutureExt as _;
    const RAW_PROMPT_BOOT: &str =
        "async function run({ code, ctx }) { return load(code, 'prompt')(ctx); }";
    let runner: recur_view_chat::prompt_actions::PromptModuleRunner =
        std::rc::Rc::new(|code, ctx, chat_code, _name, _timeout| {
            async move {
                let msg = serde_json::json!({
                    "code": code,
                    "ctx": {
                        "workspaceId": ctx.workspace_id,
                        "projectId": ctx.project_id,
                        "worktree": ctx.worktree,
                        "args": ctx.args,
                        "surface": ctx.surface,
                        "chatCode": chat_code,
                    },
                });
                // Blocks so the worker's reply never wakes the test scheduler from its thread.
                let out = futures::executor::block_on(recur_bundles::js::run_once(
                    "prompt",
                    RAW_PROMPT_BOOT,
                    &msg,
                    std::time::Duration::from_secs(10),
                ));
                let mut r = out.result?;
                if let Some(o) = r.as_object_mut() {
                    o.remove("ok");
                }
                serde_json::from_value(r).map_err(|e| e.to_string())
            }
            .boxed_local()
        });
    recur_view_chat::prompt_actions::PromptCatalog::set_runners(Some(runner), None, cx);
}

impl H<'_> {
    /// A reload: a fresh `AppState` and detail route over the same backend and local storage,
    /// with the module caches (strip, heap, transcript UI, hoist) cleared as a page load does.
    pub fn reload(&mut self, ws: &str) {
        let services = self.fake.services();
        let state = self.state.clone();
        let root = self.root.clone();
        let new_state = self.cx.update(|window, cx| {
            let mut st = LocalStorage::in_memory();
            let old = state.read(cx).storage();
            let pairs: Vec<(String, String)> = old
                .keys()
                .map(|k| (k.to_string(), old.get(k).unwrap_or_default().to_string()))
                .collect();
            for (k, v) in pairs {
                st.set(&k, v);
            }
            cx.set_global(recur_view_chat::strip::ChatStripCache::default());
            cx.set_global(recur_view_chat::heap::ChatHeapCache::default());
            cx.set_global(recur_view_chat::transcript_ui::TranscriptUiCache::default());
            cx.set_global(recur_view_chat::hoist::HoistState::default());
            let new_state = AppState::install(services.clone(), st, cx);
            new_state.update(cx, |s, cx| {
                s.wire_events(cx);
                s.load_initial(cx).detach();
            });
            let vcx = ViewContext {
                state: new_state.clone(),
                services,
            };
            let detail = cx.new(|cx| WorkspaceDetail::new(&vcx, window, cx));
            root.update(cx, |r, cx| {
                r.detail = detail;
                cx.notify();
            });
            new_state
        });
        self.state = new_state;
        self.detail = self.root.read_with(self.cx, |r, _| r.detail.clone());
        self.settle();
        self.open(ws);
    }

    pub fn settle(&mut self) {
        for _ in 0..4 {
            self.cx.run_until_parked();
            self.cx.update(|window, _| window.refresh());
        }
        self.cx.run_until_parked();
    }

    pub fn open(&mut self, ws: &str) {
        let id = ws.to_string();
        self.state
            .update(self.cx, |s, cx| s.open_workspace(&id, false, cx));
        self.settle();
    }

    /// The bound workspace view.
    pub fn view(&mut self) -> Entity<WorkspaceView> {
        let d = self.detail.clone();
        self.cx
            .update(|_, cx| d.read(cx).bound_view(cx))
            .expect("a bound workspace view")
    }

    pub fn chat(&mut self) -> Entity<AgentChat> {
        let v = self.view();
        v.read_with(self.cx, |v, _| v.chat().clone())
    }

    pub fn with<R>(
        &mut self,
        f: impl FnOnce(&mut WorkspaceView, &mut Context<WorkspaceView>) -> R,
    ) -> R {
        let v = self.view();
        let r = self.cx.update(|_, cx| v.update(cx, f));
        self.settle();
        r
    }

    pub fn read<R>(&mut self, f: impl FnOnce(&WorkspaceView, &gpui_kit::App) -> R) -> R {
        let v = self.view();
        self.cx.update(|_, cx| f(v.read(cx), cx))
    }

    pub fn bounds(&mut self, sel: &str) -> Option<Bounds<Pixels>> {
        self.settle();
        self.cx.debug_bounds(leak(sel))
    }

    pub fn has(&mut self, sel: &str) -> bool {
        self.bounds(sel).is_some()
    }

    pub fn me(&mut self, body: &str) -> bool {
        self.has(&format!("me:{}", debug_key(body)))
    }

    pub fn assistant(&mut self, body: &str) -> bool {
        self.has(&format!("assistant:{}", debug_key(body)))
    }

    pub fn click(&mut self, sel: &str) {
        let p = self
            .bounds(sel)
            .unwrap_or_else(|| panic!("no element {sel}"))
            .center();
        self.click_at(p, Modifiers::none());
    }

    pub fn click_at(&mut self, p: gpui::Point<Pixels>, mods: Modifiers) {
        self.cx.simulate_mouse_move(p, None, mods);
        self.cx.simulate_click(p, mods);
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

    pub fn focus_composer(&mut self) {
        let chat = self.chat();
        self.cx
            .update(|window, cx| chat.update(cx, |c, cx| c.focus_composer(window, cx)));
        self.settle();
    }

    /// Type into the focused composer one character at a time (`pressSequentially`).
    pub fn type_text(&mut self, s: &str) {
        for ch in s.chars() {
            self.cx.simulate_input(&ch.to_string());
            self.cx.run_until_parked();
        }
        self.settle();
    }

    /// `fill`: replace the draft and focus the composer.
    pub fn fill(&mut self, s: &str) {
        let chat = self.chat();
        let s = s.to_string();
        self.cx
            .update(|_, cx| chat.update(cx, |c, cx| c.set_draft(s, Vec::new(), cx)));
        self.focus_composer();
    }

    pub fn keys(&mut self, keys: &str) {
        self.cx.simulate_keystrokes(keys);
        self.settle();
    }

    pub fn wait(&mut self, ms: u64) {
        self.cx.executor().advance_clock(Duration::from_millis(ms));
        self.settle();
    }

    pub fn input(&mut self) -> String {
        let chat = self.chat();
        chat.read_with(self.cx, |c, _| c.input().to_string())
    }

    pub fn active(&mut self) -> String {
        self.read(|v, _| v.active_code())
    }

    pub fn codes(&mut self) -> Vec<String> {
        self.read(|v, _| v.tabs().iter().map(|t| t.code.clone()).collect())
    }

    /// The transcript the chat shows.
    pub fn messages(&mut self) -> Vec<AgentMsg> {
        let chat = self.chat();
        chat.read_with(self.cx, |c, _| c.props().messages.clone())
    }

    pub fn bodies(&mut self) -> Vec<String> {
        self.messages().into_iter().map(|m| m.body).collect()
    }

    /// Select a chat tab through the strip.
    pub fn select_tab(&mut self, code: &str) {
        let sel = self
            .chip(code)
            .unwrap_or_else(|| panic!("no chat tab {code}"));
        self.click(&sel);
    }

    /// The chip's selector (`tab-chip:chat-tab-<code>`, `:on` while active), when shown.
    pub fn chip(&mut self, code: &str) -> Option<String> {
        let base = format!("tab-chip:chat-tab-{code}");
        let on = format!("{base}:on");
        if self.has(&base) {
            Some(base)
        } else if self.has(&on) {
            Some(on)
        } else {
            None
        }
    }

    /// Push a backend event.
    pub fn dispatch(&mut self, event: AppEvent) {
        self.fake.dispatch(event);
        self.settle();
    }

    /// Run the action of the newest toast whose button reads `label`.
    pub fn toast_action(&mut self, label: &str) {
        let run = self.cx.update(|_, cx| {
            recur_ui::widgets::ToastStore::global(cx)
                .read(cx)
                .toasts()
                .iter()
                .rev()
                .find_map(|t| {
                    t.action
                        .as_ref()
                        .filter(|a| a.label.as_ref() == label)
                        .map(|a| a.run.clone())
                })
        });
        let run = run.unwrap_or_else(|| panic!("no toast action {label}"));
        self.cx.update(|window, cx| run(window, cx));
        self.settle();
    }

    pub fn toasts(&mut self) -> Vec<String> {
        self.cx.update(|_, cx| {
            recur_ui::widgets::ToastStore::global(cx)
                .read(cx)
                .toasts()
                .iter()
                .map(|t| t.text.to_string())
                .collect()
        })
    }
}

/// Tracks the calls an override sees, arguments as strings.
pub type Calls = Arc<parking_lot::Mutex<Vec<Vec<String>>>>;

pub fn calls() -> Calls {
    Default::default()
}

/// A per-code transcript store shared by the send / chat items overrides.
pub type Store = Arc<parking_lot::Mutex<std::collections::HashMap<String, Vec<ChatItem>>>>;

/// A fixture written as JSON.
pub fn de<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> T {
    serde_json::from_value(v).expect("fixture matches its type")
}

pub fn item(role: &str, kind: &str, text: impl Into<String>) -> ChatItem {
    ChatItem {
        role: role.into(),
        kind: kind.into(),
        text: text.into(),
        ..Default::default()
    }
}

pub fn user(text: impl Into<String>) -> ChatItem {
    item("user", "text", text)
}

pub fn assistant(text: impl Into<String>) -> ChatItem {
    item("assistant", "text", text)
}

pub fn turn(duration_ms: i64) -> ChatItem {
    ChatItem {
        duration_ms,
        ..item("assistant", "turn", "")
    }
}
