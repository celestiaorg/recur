//! A window with one `AgentChat` on the mock bridge, for specs of the chat's own rendering and
//! composer. The chat sits in a plain container; routing (send, the
//! queue, Stop, tabs) is the workspace view's and is specced there. Elements are found by
//! their debug selectors.

use std::sync::Arc;
use std::time::Duration;

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, px};
use gpui_kit::{
    AppContext as _, Bounds, Context, IntoElement, ParentElement, Pixels, Render, Styled, Window,
    div,
};
use recur_engine::view::chat::AgentMsg;
use recur_services::mock::{MockBridge, MockOptions};
use recur_view_core::{AppState, LocalStorage, ViewContext};
use serde_json::Value;

use crate::chat::{AgentChat, ChatHooks, ChatProps, debug_key};

pub const WS: &str = "LOOP-2231";
pub const PROJECT: &str = "storefront";

pub fn root_path() -> String {
    format!("/mock/worktrees/{WS}")
}

/// The chat in a column.
pub struct Host {
    pub chat: Entity<AgentChat>,
    pub services: recur_services::Services,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex().flex_col().child(self.chat.clone())
    }
}

pub struct H<'a> {
    pub bridge: Arc<MockBridge>,
    pub host: Entity<Host>,
    pub cx: &'a mut VisualTestContext,
}

pub fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_ui::init(cx);
        recur_content::markdown::set_markdown_render_inline(true);
    });
}

/// Workspace chat props for the seed's first active workspace.
pub fn ws_props() -> ChatProps {
    ChatProps {
        workspace_id: WS.into(),
        project_id: PROJECT.into(),
        root_path: root_path(),
        agent_code: "MA".into(),
        reset_key: format!("{WS}\tMA"),
        ..Default::default()
    }
}

pub fn ws_hooks() -> ChatHooks {
    ChatHooks {
        send: true,
        stop: true,
        retry: true,
        remove_queued: true,
        cycle_mode: true,
        open_file: true,
        open_artifact: true,
        fork: true,
        fork_to_workspace: true,
        prompt_action: true,
        ..Default::default()
    }
}

pub fn boot<'a>(cx: &'a mut TestAppContext, opts: MockOptions, props: ChatProps, hooks: ChatHooks) -> H<'a> {
    init(cx);
    let bridge = MockBridge::fresh(opts);
    let services = bridge.services();
    let state = cx.update(|cx| AppState::install(services.clone(), LocalStorage::in_memory(), cx));
    let vcx = ViewContext {
        state,
        services: services.clone(),
    };
    let (host, vcx_cx) = cx.add_window_view(move |window, cx| {
        let chat = cx.new(|cx| AgentChat::new(&vcx, props, window, cx));
        chat.update(cx, |c, cx| c.set_hooks(hooks, cx));
        Host { chat, services }
    });
    let mut h = H {
        bridge,
        host,
        cx: vcx_cx,
    };
    h.cx.update(|window, _| window.activate_window());
    h.cx.simulate_resize(gpui::size(px(560.), px(760.)));
    h.settle();
    h
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

impl H<'_> {
    pub fn settle(&mut self) {
        for _ in 0..3 {
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

    pub fn assistant(&mut self, body: &str) -> bool {
        self.has(&format!("assistant:{}", debug_key(body)))
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

    pub fn focus_composer(&mut self) {
        let chat = self.chat();
        self.cx
            .update(|window, cx| chat.update(cx, |c, cx| c.focus_composer(window, cx)));
        self.settle();
    }

    /// Type into the focused composer, one character at a time like `pressSequentially`.
    pub fn type_text(&mut self, s: &str) {
        for ch in s.chars() {
            self.cx.simulate_input(&ch.to_string());
            self.cx.run_until_parked();
        }
        self.settle();
    }

    /// `fill`: replace the draft.
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

    pub fn read<R>(&mut self, f: impl FnOnce(&AgentChat) -> R) -> R {
        let chat = self.chat();
        chat.read_with(self.cx, |c, _| f(c))
    }

    pub fn chat(&mut self) -> Entity<AgentChat> {
        self.host.read_with(self.cx, |h, _| h.chat.clone())
    }

    /// Show `msgs` as the transcript.
    pub fn set_messages(&mut self, msgs: Vec<AgentMsg>) {
        let chat = self.chat();
        self.cx.update(|_, cx| {
            chat.update(cx, |c, cx| {
                let mut p = c.props().clone();
                p.messages = msgs;
                c.set_props(p, cx)
            })
        });
        self.settle();
    }
}

/// Tracks the wire calls a resolver sees.
pub type Calls = Arc<parking_lot::Mutex<Vec<Vec<Value>>>>;

pub fn calls() -> Calls {
    Default::default()
}
