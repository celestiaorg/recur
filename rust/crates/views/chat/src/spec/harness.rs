//! A window with one `AgentChat` on the mock bridge, and `Host`: a test double of the
//! workspace view's chat contract (`WorkspaceView.svelte`'s send, follow-up queue, steer,
//! Stop, Retry and tab switching) so the chat.spec scenarios run against the chat as the
//! real host will drive it. Elements are found by their debug selectors.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use gpui_kit::{AppContext as _, Bounds, Context, IntoElement, ParentElement, Pixels, Render, Styled, Subscription, Window, div};
use recur_engine::view::chat::{
    AgentMsg, StreamKind, StreamOpts, apply_stream, drop_live_after_footer, end_turn, map_chat_items,
    merge_persisted_with_live, should_ignore_stale_idle, should_refresh_turn_stamp,
    should_skip_idle_transcript_reset,
};
use recur_services::Services;
use recur_services::mock::{MockBridge, MockOptions};
use recur_view_core::{AppState, LocalStorage, ViewContext};
use serde_json::Value;

use crate::chat::{AgentChat, ChatEvent, ChatHooks, ChatProps, debug_key, now_ms};

pub const WS: &str = "LOOP-2231";
pub const PROJECT: &str = "storefront";

pub fn root_path() -> String {
    format!("/mock/worktrees/{WS}")
}

/// One chat tab as the workspace view keeps it.
#[derive(Default, Clone)]
pub struct Tab {
    pub msgs: Vec<AgentMsg>,
    pub sending: bool,
    pub turn_started_at: i64,
    /// Queued follow-ups: pending id and wire text.
    pub pending: Vec<(String, String)>,
    pub draft: String,
    pub can_steer: bool,
    /// The engine reports the tab's agent working (`agentsRunning`).
    pub running: bool,
    /// The stamp was replaced over a footer: the next idle may be the previous turn's.
    pub stale_idle_armed: bool,
    /// The latest send; an older one finishing does not clear `sending`.
    pub send_gen: u64,
}

pub struct Host {
    pub chat: Entity<AgentChat>,
    pub events: Rc<RefCell<Vec<ChatEvent>>>,
    pub services: Services,
    pub base: ChatProps,
    pub hooks: ChatHooks,
    /// Act as the workspace view (send / queue / stop / retry); off: only record events.
    pub sim: bool,
    pub tabs: BTreeMap<String, Tab>,
    pub active: String,
    /// `!settings.queueFollowUps`.
    pub steer_by_default: bool,
    seq: u64,
    _sub: Subscription,
}

impl Host {
    pub fn tab(&mut self) -> &mut Tab {
        let code = self.active.clone();
        self.tabs.entry(code).or_default()
    }

    pub fn props(&self) -> ChatProps {
        let tab = self.tabs.get(&self.active).cloned().unwrap_or_default();
        ChatProps {
            messages: tab.msgs,
            sending: tab.sending || tab.running,
            can_stop: tab.sending || tab.running,
            can_steer: tab.can_steer,
            steer_follow_up: self.steer_by_default,
            turn_started_at: tab.turn_started_at,
            agent_code: self.active.clone(),
            reset_key: format!("{}:{}", self.base.workspace_id, self.active),
            ..self.base.clone()
        }
    }

    pub fn push(&mut self, cx: &mut Context<Self>) {
        let p = self.props();
        self.chat.update(cx, |c, cx| c.set_props(p, cx));
        cx.notify();
    }

    /// Switch tabs: park the draft, show the other tab.
    pub fn switch(&mut self, code: &str, cx: &mut Context<Self>) {
        let draft = self.chat.read(cx).input().to_string();
        self.tab().draft = draft;
        self.active = code.to_string();
        let draft = self.tab().draft.clone();
        self.push(cx);
        self.chat.update(cx, |c, cx| c.set_draft(draft, Vec::new(), cx));
    }

    fn on_event(&mut self, ev: &ChatEvent, cx: &mut Context<Self>) {
        self.events.borrow_mut().push(ev.clone());
        if !self.sim {
            return;
        }
        match ev {
            ChatEvent::Send { alt } => self.send(*alt, cx),
            ChatEvent::Stop => self.stop(cx),
            ChatEvent::Retry => self.dispatch("\u{2060}".into(), "\u{2060}".into(), false, cx),
            ChatEvent::RemoveQueued(id) => {
                let tab = self.tab();
                tab.pending.retain(|(p, _)| p != id);
                tab.msgs.retain(|m| m.pending_id != *id);
                self.push(cx);
            }
            _ => {}
        }
    }

    fn send(&mut self, alt: bool, cx: &mut Context<Self>) {
        let text = self.chat.read(cx).input().to_string();
        let steer_default = self.steer_by_default;
        let tab = self.tab();
        if text.trim().is_empty() && tab.pending.is_empty() {
            return;
        }
        let busy = tab.sending;
        let can_steer = tab.can_steer;
        self.chat.update(cx, |c, cx| c.set_draft(String::new(), Vec::new(), cx));
        if busy {
            if can_steer && steer_default != alt {
                self.dispatch(text.clone(), String::new(), true, cx);
                return;
            }
            self.seq += 1;
            let id = format!("pending-{}", self.seq);
            let tab = self.tab();
            tab.msgs.push(AgentMsg {
                me: true,
                body: text.clone(),
                queued: true,
                pending_id: id.clone(),
                ts: Some(now_ms()),
                ..Default::default()
            });
            tab.pending.push((id, text));
            self.push(cx);
            return;
        }
        if text.trim().is_empty() {
            self.flush(cx);
            return;
        }
        self.dispatch(text, String::new(), false, cx);
    }

    /// Fold every queued follow-up into one user turn once the tab is idle.
    fn flush(&mut self, cx: &mut Context<Self>) {
        let tab = self.tab();
        if tab.sending || tab.pending.is_empty() {
            return;
        }
        let text = tab.pending.drain(..).map(|(_, t)| t).collect::<Vec<_>>().join("\n\n");
        tab.msgs.retain(|m| !m.queued);
        self.dispatch(text, String::new(), false, cx);
    }

    fn dispatch(&mut self, text: String, display: String, steer: bool, cx: &mut Context<Self>) {
        let code = self.active.clone();
        let invisible = display == "\u{2060}";
        let tab = self.tab();
        if !invisible {
            tab.msgs.push(AgentMsg {
                me: true,
                body: if display.is_empty() { text.clone() } else { display.clone() },
                optimistic: true,
                ts: Some(now_ms()),
                ..Default::default()
            });
        }
        if !steer {
            tab.sending = true;
            tab.turn_started_at = now_ms();
            tab.send_gen += 1;
        }
        let send_gen = tab.send_gen;
        self.push(cx);
        let services = self.services.clone();
        let ws = self.base.workspace_id.clone();
        cx.spawn(async move |this, cx| {
            let _ = services
                .workspaces
                .send_workspace_message(&ws, &code, &text, "", &BTreeMap::new(), &[], "", &display, false)
                .await;
            if steer {
                return;
            }
            let items = services.workspaces.workspace_chat_items(&ws, &code).await.unwrap_or_default();
            let _ = this.update(cx, |h, cx| {
                let tab = h.tabs.entry(code.clone()).or_default();
                let latest = tab.send_gen == send_gen;
                if latest {
                    tab.sending = false;
                }
                let queued: Vec<AgentMsg> = tab.msgs.iter().filter(|m| m.queued).cloned().collect();
                tab.msgs = if latest { map_chat_items(&items) } else { merge_persisted_with_live(&map_chat_items(&items), &tab.msgs) };
                tab.msgs.retain(|m| !m.queued);
                tab.msgs.extend(queued);
                if h.active == code {
                    h.push(cx);
                    h.flush(cx);
                }
            });
        })
        .detach();
    }

    /// A `session.delta` snapshot for the active tab.
    pub fn live_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let tab = self.tab();
        if drop_live_after_footer(&tab.msgs, tab.turn_started_at, tab.sending || tab.running) {
            return;
        }
        tab.msgs = apply_stream(&tab.msgs, StreamKind::Text, text, &StreamOpts::default());
        self.push(cx);
    }

    /// `session.turn_end`.
    pub fn turn_end(&mut self, duration_ms: i64, cx: &mut Context<Self>) {
        let tab = self.tab();
        tab.msgs = end_turn(&tab.msgs, duration_ms, now_ms());
        self.push(cx);
    }

    /// `agent.updated` working: a new turn re-stamps the clock over a footer.
    pub fn working(&mut self, cx: &mut Context<Self>) {
        let tab = self.tab();
        tab.running = true;
        if should_refresh_turn_stamp(&tab.msgs, tab.turn_started_at) {
            tab.stale_idle_armed = tab.turn_started_at > 0;
            let footer = tab.msgs.iter().rev().find(|m| m.turn_end).and_then(|m| m.ts).unwrap_or(0);
            tab.turn_started_at = now_ms().max(footer);
        }
        self.push(cx);
    }

    /// `agent.updated` idle: drop the sticky send, settle from the durable transcript unless
    /// this idle is the reordered previous turn's, then flush the queue.
    pub fn idle(&mut self, cx: &mut Context<Self>) {
        let code = self.active.clone();
        let tab = self.tab();
        tab.running = false;
        tab.sending = false;
        let skip = should_skip_idle_transcript_reset(&tab.msgs, tab.turn_started_at, tab.stale_idle_armed);
        tab.stale_idle_armed = false;
        if skip {
            self.push(cx);
            self.flush(cx);
            return;
        }
        if !should_ignore_stale_idle(&tab.msgs, tab.turn_started_at) {
            tab.turn_started_at = 0;
        }
        self.push(cx);
        self.flush(cx);
        let services = self.services.clone();
        let ws = self.base.workspace_id.clone();
        cx.spawn(async move |this, cx| {
            let items = services.workspaces.workspace_chat_items(&ws, &code).await.unwrap_or_default();
            let _ = this.update(cx, |h, cx| {
                let tab = h.tabs.entry(code.clone()).or_default();
                let queued: Vec<AgentMsg> = tab.msgs.iter().filter(|m| m.queued).cloned().collect();
                tab.msgs = map_chat_items(&items);
                tab.msgs.extend(queued);
                if h.active == code {
                    h.push(cx);
                }
            });
        })
        .detach();
    }

    /// Unmount and remount the chat (a panel toggle), as the host re-creates it.
    pub fn remount(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let vcx = ViewContext {
            state: self.chat.read(cx).state.clone(),
            services: self.services.clone(),
        };
        let props = self.props();
        let hooks = self.hooks;
        self.chat = cx.new(|cx| AgentChat::new(&vcx, props, window, cx));
        self.chat.update(cx, |c, cx| c.set_hooks(hooks, cx));
        self._sub = cx.subscribe(&self.chat, |h: &mut Host, _, e: &ChatEvent, cx| h.on_event(e, cx));
        cx.notify();
    }

    /// Stop: cancel the active tab's turn and hand queued follow-ups back to the composer.
    fn stop(&mut self, cx: &mut Context<Self>) {
        let code = self.active.clone();
        let tab = self.tab();
        let restored = tab.pending.drain(..).map(|(_, t)| t).collect::<Vec<_>>();
        tab.msgs.retain(|m| !m.queued);
        if !restored.is_empty() {
            let draft = restored.join("\n\n");
            self.chat.update(cx, |c, cx| c.set_draft(draft, Vec::new(), cx));
        }
        self.push(cx);
        let services = self.services.clone();
        let ws = self.base.workspace_id.clone();
        cx.background_spawn(async move {
            let _ = services.workspaces.cancel_workspace_message(&ws, &code).await;
        })
        .detach();
    }
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex().flex_col().child(self.chat.clone())
    }
}

pub struct H<'a> {
    pub state: Entity<AppState>,
    pub bridge: Arc<MockBridge>,
    pub host: Entity<Host>,
    pub chat: Entity<AgentChat>,
    pub events: Rc<RefCell<Vec<ChatEvent>>>,
    pub cx: &'a mut VisualTestContext,
}

pub fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_ui::init(cx);
    });
}

/// Workspace chat props for the seed's first active workspace.
pub fn ws_props() -> ChatProps {
    ChatProps {
        workspace_id: WS.into(),
        project_id: PROJECT.into(),
        root_path: root_path(),
        agent_code: "MA".into(),
        reset_key: format!("{WS}:MA"),
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

/// A transcript as `WorkspaceChatItems` returns it.
pub fn items(v: Value) -> Vec<AgentMsg> {
    let items: Vec<recur_domain::ChatItem> = serde_json::from_value(v).expect("chat items");
    map_chat_items(&items)
}

pub fn boot<'a>(
    cx: &'a mut TestAppContext,
    opts: MockOptions,
    base: ChatProps,
    hooks: ChatHooks,
    sim: bool,
) -> H<'a> {
    boot_with(cx, MockBridge::fresh(opts), LocalStorage::in_memory(), base, hooks, sim)
}

pub fn boot_with<'a>(
    cx: &'a mut TestAppContext,
    bridge: Arc<MockBridge>,
    storage: LocalStorage,
    base: ChatProps,
    hooks: ChatHooks,
    sim: bool,
) -> H<'a> {
    init(cx);
    let services = bridge.services();
    let state = cx.update(|cx| AppState::install(services.clone(), storage, cx));
    let vcx = ViewContext {
        state: state.clone(),
        services: services.clone(),
    };
    let events: Rc<RefCell<Vec<ChatEvent>>> = Default::default();
    let ev = events.clone();
    let (host, vcx_cx) = cx.add_window_view(move |window, cx| {
        let active = base.agent_code.clone();
        let props = ChatProps {
            reset_key: format!("{}:{active}", base.workspace_id),
            ..base.clone()
        };
        let chat = cx.new(|cx| AgentChat::new(&vcx, props, window, cx));
        chat.update(cx, |c, cx| c.set_hooks(hooks, cx));
        let _sub = cx.subscribe(&chat, |h: &mut Host, _, e: &ChatEvent, cx| h.on_event(e, cx));
        let mut tabs = BTreeMap::new();
        tabs.insert(active.clone(), Tab { can_steer: active != "MA", ..Default::default() });
        Host {
            chat,
            events: ev,
            services: vcx.services.clone(),
            base,
            hooks,
            sim,
            tabs,
            active,
            steer_by_default: false,
            seq: 0,
            _sub,
        }
    });
    let chat = host.read_with(vcx_cx, |h, _| h.chat.clone());
    let mut h = H {
        state,
        bridge,
        host,
        chat,
        events,
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

    pub fn me(&mut self, body: &str) -> bool {
        self.has(&format!("me:{}", debug_key(body)))
    }

    pub fn assistant(&mut self, body: &str) -> bool {
        self.has(&format!("assistant:{}", debug_key(body)))
    }

    pub fn click(&mut self, sel: &str) {
        let p = self.bounds(sel).unwrap_or_else(|| panic!("no element {sel}")).center();
        self.cx.simulate_mouse_move(p, None, Modifiers::none());
        self.cx.simulate_click(p, Modifiers::none());
        self.settle();
    }

    pub fn click_mods(&mut self, sel: &str, mods: Modifiers) {
        let p = self.bounds(sel).unwrap_or_else(|| panic!("no element {sel}")).center();
        self.cx.simulate_mouse_move(p, None, mods);
        self.cx.simulate_mouse_down(p, MouseButton::Left, mods);
        self.cx.simulate_mouse_up(p, MouseButton::Left, mods);
        self.settle();
    }

    pub fn hover(&mut self, sel: &str) {
        let p = self.bounds(sel).unwrap_or_else(|| panic!("no element {sel}")).center();
        self.cx.simulate_mouse_move(point(p.x + px(1.), p.y), None, Modifiers::none());
        self.cx.simulate_mouse_move(p, None, Modifiers::none());
        self.settle();
    }

    pub fn focus_composer(&mut self) {
        let chat = self.chat.clone();
        self.cx.update(|window, cx| chat.update(cx, |c, cx| c.focus_composer(window, cx)));
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
        let chat = self.chat.clone();
        let s = s.to_string();
        self.cx.update(|_, cx| chat.update(cx, |c, cx| c.set_draft(s, Vec::new(), cx)));
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

    /// The composer's lines (chips show as U+FFFC).
    pub fn lines(&mut self) -> Vec<String> {
        self.read(|c| c.doc.text.split('\n').map(str::to_string).collect())
    }

    pub fn input(&mut self) -> String {
        self.chat.read_with(self.cx, |c, _| c.input().to_string())
    }

    pub fn read<R>(&mut self, f: impl FnOnce(&AgentChat) -> R) -> R {
        self.chat.read_with(self.cx, |c, _| f(c))
    }

    /// Remount the chat; the handle follows the new entity.
    pub fn remount(&mut self) {
        let host = self.host.clone();
        self.cx.update(|window, cx| host.update(cx, |h, cx| h.remount(window, cx)));
        self.chat = self.host.read_with(self.cx, |h, _| h.chat.clone());
        self.settle();
    }

    pub fn host<R>(&mut self, f: impl FnOnce(&mut Host, &mut Context<Host>) -> R) -> R {
        let host = self.host.clone();
        let r = self.cx.update(|_, cx| host.update(cx, f));
        self.settle();
        r
    }

    pub fn set_messages(&mut self, msgs: Vec<AgentMsg>) {
        self.host(|h, cx| {
            h.tab().msgs = msgs;
            h.push(cx);
        });
    }

    pub fn events(&self) -> Vec<ChatEvent> {
        self.events.borrow().clone()
    }

    /// Distance from the scroller's bottom.
    pub fn dist_from_bottom(&mut self) -> f32 {
        self.settle();
        self.read(|c| {
            let max = f32::from(c.scroll.max_offset().y);
            max + f32::from(c.scroll.offset().y)
        })
    }

    pub fn scroll_top(&mut self) -> f32 {
        self.read(|c| -f32::from(c.scroll.offset().y))
    }

    pub fn max_scroll(&mut self) -> f32 {
        self.read(|c| f32::from(c.scroll.max_offset().y))
    }

    /// Scroll the transcript to `top` the way a wheel would, and let the chat record it.
    pub fn scroll_to(&mut self, top: f32) {
        let chat = self.chat.clone();
        self.cx.update(|_, cx| {
            chat.update(cx, |c, cx| {
                c.scroll.set_offset(point(px(0.), px(-top)));
                cx.notify();
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

/// A per-code transcript store the Send / ChatItems resolvers share.
pub type Store = Arc<parking_lot::Mutex<HashMap<String, Vec<Value>>>>;
