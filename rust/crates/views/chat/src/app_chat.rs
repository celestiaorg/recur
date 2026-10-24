//! `AppChat.svelte` ("Ask Recur"): the app-surface chat with its own tab strip, the ask
//! chips on an empty transcript, and the model pickers. Transcripts and live events are the
//! `""` workspace's; the host shows or hides the panel and handles [`AppChatEvent::Hide`].

use recur_ui::guard::Guarded;
use std::collections::{BTreeMap, HashMap};

use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, FocusHandle, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Render, StatefulInteractiveElement, Styled, Subscription, Task,
    WeakEntity, Window, div, px,
};
use recur_agent::context_usage::{self, ContextUsage};
use recur_domain::WorkspaceChatTab;
use recur_engine::view::chat::{
    AgentMsg, StreamKind, StreamOpts, add_tool, append_stream, apply_compact, apply_fallback,
    apply_notice, apply_stream, attach_tool_result, drop_live_after_footer, end_turn,
    map_chat_items, merge_persisted_with_live, note_thinking_tokens, patch_tool_input,
};
use recur_services::Services;
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::widgets::tab_chip::{ChipIcon, tab_chip};
use recur_ui::widgets::tab_strip::{tab_strip, tab_strip_action};
use recur_ui::{IconName, Theme, icon, motion};
use recur_view_core::models::{CatalogModel, FlagMap, flags_for_model, normalize_flags};
use recur_view_core::modes::{ChatMode, resolved_default_for_mode};
use recur_view_core::{AppState, ViewContext, app_chat_agent_id};

use crate::chat::{AgentChat, ChatEvent, ChatHooks, ChatProps, ChatSurfaceKind, now_ms};
use crate::pickers::{ModelPickers, PickerEvent};
use crate::session_events::{SessionSignal, StreamEvt, session_event};
use crate::style::{hsla, label};

pub const APP: &str = "APP";
pub const ACTIVE_KEY: &str = "recur.askActiveTab";
pub const ASK_CHIPS: [&str; 3] = [
    "What needs me?",
    "Where did I leave off?",
    "Which workspace should I open next?",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppChatEvent {
    /// The header's Hide chat panel button (`setInboxChatOpen(false)`).
    Hide,
}

/// The tab's label (`tab.title || 'New chat'`).
pub fn tab_label(tab: &WorkspaceChatTab) -> &str {
    if tab.title.is_empty() {
        "New chat"
    } else {
        &tab.title
    }
}

/// The backend's first-message title stamp, mirrored so the chip labels at once.
pub fn stamp_title(tabs: &mut [WorkspaceChatTab], code: &str, sent: &str) {
    for t in tabs.iter_mut().filter(|t| t.code == code) {
        if (t.title.is_empty() || t.title == "New chat")
            && let Some(line) = sent.lines().map(str::trim).find(|l| !l.is_empty())
        {
            t.title = line.to_string();
        }
    }
}

pub struct AppChat {
    state: Entity<AppState>,
    services: Services,
    visible: bool,
    tabs: Vec<WorkspaceChatTab>,
    active: String,
    messages: Vec<AgentMsg>,
    sending_by: HashMap<String, bool>,
    turn_started_at: i64,
    stream_seq: HashMap<String, i64>,
    context_usage: Option<ContextUsage>,
    catalog: Vec<CatalogModel>,
    catalog_rev: Option<u64>,
    model: String,
    flags: FlagMap,
    /// Bumped per transcript load so a stale reply for a previous tab is dropped.
    load_gen: u64,
    /// The focus target `focus` stands for, last reported (`SetAppChatFocus`).
    reported_focus: Option<(bool, String)>,
    chat: Entity<AgentChat>,
    pickers: Entity<ModelPickers>,
    focus: FocusHandle,
    props_dirty: bool,
    pickers_key: Option<(usize, String, FlagMap, bool)>,
    _events: Task<()>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<AppChatEvent> for AppChat {}

impl AppChat {
    pub fn new(
        vcx: &ViewContext,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = vcx.state.clone();
        let active = state
            .read(cx)
            .storage()
            .get(ACTIVE_KEY)
            .filter(|s| !s.is_empty())
            .unwrap_or(APP)
            .to_string();
        let catalog = state.read(cx).peek_model_catalog().to_vec();
        let chat =
            cx.new(|cx| AgentChat::new(vcx, Self::chat_props_for(&active, visible), window, cx));
        chat.update(cx, |c, cx| {
            c.set_hooks(
                ChatHooks {
                    send: true,
                    stop: true,
                    ..Default::default()
                },
                cx,
            )
        });
        let pickers = cx.new(|cx| ModelPickers::new(true, true, window, cx));
        let me = cx.entity().downgrade();
        let empty = cx.new(|_| AskEmpty { app: me });
        chat.update(cx, |c, cx| c.set_empty(Some(empty.into()), cx));
        let focus = cx.focus_handle();
        let subs = vec![
            cx.guarded_subscribe_in(
                &chat,
                window,
                |this, chat, ev: &ChatEvent, _, cx| match ev {
                    ChatEvent::Send { .. } => {
                        let input = chat.read(cx).input().to_string();
                        chat.update(cx, |c, cx| c.set_draft(String::new(), Vec::new(), cx));
                        this.send_text(&input, cx);
                    }
                    ChatEvent::Stop => this.stop(cx),
                    _ => {}
                },
            ),
            cx.guarded_subscribe(&pickers, |this, _, ev: &PickerEvent, cx| match ev {
                PickerEvent::Model(r, f) => this.remember(r.clone(), f.clone(), cx),
                PickerEvent::Flags(f) => {
                    let m = this.model.clone();
                    this.remember(m, f.clone(), cx)
                }
            }),
            cx.guarded_observe(&state, |this, _, cx| {
                this.sync_catalog(cx);
                this.props_dirty = true;
                cx.notify();
            }),
            cx.on_focus_in(&focus, window, |_, _, cx| cx.notify()),
            cx.on_focus_out(&focus, window, |_, _, _, cx| cx.notify()),
            cx.on_release(|this: &mut Self, cx| {
                let services = this.services.clone();
                let code = this.active.clone();
                cx.background_spawn(async move {
                    let _ = services.briefing.set_app_chat_focus(false, &code).await;
                })
                .detach();
            }),
        ];
        let mut events = vcx.services.subscribe();
        let _events = cx.spawn(async move |this, cx| {
            while let Some(ev) = events.recv().await {
                let Some(ev) = session_event(&ev) else {
                    continue;
                };
                if this
                    .update(cx, |a, cx| {
                        a.on_session(ev.workspace_id, ev.code, ev.signal, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut this = AppChat {
            state,
            services: vcx.services.clone(),
            visible,
            tabs: vec![WorkspaceChatTab {
                code: APP.into(),
                title: "Ask Recur".into(),
                ..Default::default()
            }],
            active,
            messages: Vec::new(),
            sending_by: HashMap::new(),
            turn_started_at: 0,
            stream_seq: HashMap::new(),
            context_usage: None,
            catalog,
            catalog_rev: None,
            model: String::new(),
            flags: FlagMap::new(),
            load_gen: 0,
            reported_focus: None,
            chat,
            pickers,
            focus,
            props_dirty: true,
            pickers_key: None,
            _events,
            _subs: subs,
        };
        this.sync_catalog(cx);
        this.refresh_tabs(cx);
        this.load_active(cx);
        this.report_focus(cx);
        this
    }

    fn chat_props_for(active: &str, visible: bool) -> ChatProps {
        ChatProps {
            surface: ChatSurfaceKind::App,
            agent_code: active.to_string(),
            reset_key: format!("app:{active}"),
            compact: true,
            empty_text: String::new(),
            placeholder: "Ask what needs you, or where you left off…".into(),
            shell_active: visible,
            ..Default::default()
        }
    }

    pub fn chat(&self) -> &Entity<AgentChat> {
        &self.chat
    }

    pub fn active(&self) -> &str {
        &self.active
    }

    pub fn tabs(&self) -> &[WorkspaceChatTab] {
        &self.tabs
    }

    pub fn messages(&self) -> &[AgentMsg] {
        &self.messages
    }

    /// `visible`: the host's panel is showing.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if visible != self.visible {
            self.visible = visible;
            self.props_dirty = true;
            self.report_focus(cx);
            cx.notify();
        }
    }

    /// The strip's selected tab is lit (the chat has focus while shown).
    pub fn lit(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        self.visible && self.focus.contains_focused(window, cx)
    }

    /// A tab's turn is running (`aria-busy`).
    pub fn tab_busy(&self, code: &str, cx: &gpui_kit::App) -> bool {
        self.sending_by.get(code).copied().unwrap_or(false)
            || self
                .state
                .read(cx)
                .agents_running
                .contains(&app_chat_agent_id(code))
    }

    fn sending(&self, cx: &gpui_kit::App) -> bool {
        self.tab_busy(&self.active, cx)
    }

    /// `SetAppChatFocus(visible, active)`, releasing the previous tab first.
    fn report_focus(&mut self, cx: &mut Context<Self>) {
        let next = (self.visible, self.active.clone());
        if self.reported_focus.as_ref() == Some(&next) {
            return;
        }
        let prev = self.reported_focus.replace(next.clone());
        let services = self.services.clone();
        let refresh = cx.spawn(async move |this, cx| {
            if let Some((true, code)) = prev
                && code != next.1
            {
                let _ = services.briefing.set_app_chat_focus(false, &code).await;
            }
            let _ = services.briefing.set_app_chat_focus(next.0, &next.1).await;
            let _ = this.update(cx, |a, cx| a.refresh_tabs(cx));
        });
        refresh.detach();
    }

    fn refresh_tabs(&mut self, cx: &mut Context<Self>) {
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let Ok(list) = services.briefing.list_app_chats().await else {
                return;
            };
            if list.is_empty() {
                return;
            }
            let _ = this.update(cx, |a, cx| {
                a.tabs = list;
                if !a.tabs.iter().any(|t| t.code == a.active) {
                    a.set_active(APP.into(), cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sync_catalog(&mut self, cx: &mut Context<Self>) {
        let rev = self.state.read(cx).model_catalog_rev();
        if self.catalog_rev == Some(rev) {
            return;
        }
        self.catalog_rev = Some(rev);
        let load = self.state.update(cx, |s, cx| s.load_model_catalog(cx));
        cx.spawn(async move |this, cx| {
            let models = load.await;
            let _ = this.update(cx, |a, cx| {
                if a.catalog_rev == Some(rev) {
                    a.catalog = models;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn set_active(&mut self, code: String, cx: &mut Context<Self>) {
        if code == self.active {
            return;
        }
        self.active = code;
        self.state.update(cx, |s, _| {
            s.storage_mut().set(ACTIVE_KEY, self.active.clone())
        });
        self.load_active(cx);
        self.report_focus(cx);
        self.props_dirty = true;
        cx.notify();
    }

    /// The active tab's transcript and model selection.
    fn load_active(&mut self, cx: &mut Context<Self>) {
        self.load_gen += 1;
        let load_gen = self.load_gen;
        self.stream_seq.clear();
        let code = self.active.clone();
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let items = services.briefing.app_chat_items(&code).await;
            let _ = this.update(cx, |a, cx| {
                let Ok(items) = items else { return };
                if a.load_gen != load_gen || code != a.active {
                    return;
                }
                let mapped = map_chat_items(&items);
                a.messages = if a.sending_by.get(&code).copied().unwrap_or(false) {
                    merge_persisted_with_live(&mapped, &a.messages)
                } else {
                    mapped
                };
                a.context_usage = context_usage::from_chat_items(&items);
                a.props_dirty = true;
                cx.notify();
            });
            let sel = services.briefing.app_chat_selection(&code).await;
            let _ = this.update(cx, |a, cx| {
                let Ok(sel) = sel else { return };
                if a.load_gen != load_gen || code != a.active {
                    return;
                }
                let d = resolved_default_for_mode(
                    "ask",
                    a.state.read(cx).settings.as_ref(),
                    &a.catalog,
                );
                a.model = if sel.model.is_empty() {
                    d.model
                } else {
                    sel.model
                };
                let cur = if sel.flags.is_empty() {
                    d.flags
                } else {
                    sel.flags
                };
                a.flags = normalize_flags(&flags_for_model(&a.catalog, &a.model), Some(&cur));
                cx.notify();
            });
        })
        .detach();
    }

    fn live_turn_closed(&self, cx: &gpui_kit::App) -> bool {
        drop_live_after_footer(&self.messages, self.turn_started_at, self.sending(cx))
    }

    fn apply_live_stream(&mut self, kind: StreamKind, evt: &StreamEvt, cx: &gpui_kit::App) {
        if self.live_turn_closed(cx) {
            return;
        }
        let tag = if kind == StreamKind::Text {
            "text"
        } else {
            "thinking"
        };
        let seq_key = if evt.parent_id.is_empty() {
            tag.to_string()
        } else {
            format!("{tag}:{}", evt.parent_id)
        };
        if evt.seq > 0 {
            let prev = self.stream_seq.get(&seq_key).copied().unwrap_or(0);
            if evt.seq <= prev {
                return;
            }
            self.stream_seq.insert(seq_key, evt.seq);
        }
        let opts = StreamOpts {
            done: evt.done,
            parent_id: evt.parent_id.clone(),
            uuid: evt.uuid.clone(),
        };
        if !evt.parent_id.is_empty() || evt.replace || evt.done {
            self.messages = apply_stream(&self.messages, kind, &evt.text, &opts);
        } else if !evt.text.is_empty() {
            self.messages = append_stream(&self.messages, kind, &evt.text);
        }
    }

    #[cfg(test)]
    pub(crate) fn on_session_for_test(
        &mut self,
        ws: &str,
        code: &str,
        signal: SessionSignal,
        cx: &mut Context<Self>,
    ) {
        self.on_session(ws.to_string(), code.to_string(), signal, cx);
    }

    fn on_session(
        &mut self,
        ws: String,
        code: String,
        signal: SessionSignal,
        cx: &mut Context<Self>,
    ) {
        if let SessionSignal::TurnEnd(turn) = &signal {
            if !ws.is_empty() || !self.tabs.iter().any(|t| t.code == code) {
                return;
            }
            self.sending_by.insert(code.clone(), false);
            if code != self.active {
                self.refresh_tabs(cx);
                return;
            }
            self.messages = end_turn(&self.messages, turn.duration_ms, now_ms());
            if let Some(u) = &turn.usage {
                self.context_usage = Some(u.clone());
            }
            let services = self.services.clone();
            let c = code.clone();
            cx.spawn(async move |this, cx| {
                let Ok(items) = services.briefing.app_chat_items(&c).await else {
                    return;
                };
                let _ = this.update(cx, |a, cx| {
                    if c != a.active {
                        return;
                    }
                    a.messages = merge_persisted_with_live(&map_chat_items(&items), &a.messages);
                    a.context_usage =
                        context_usage::from_chat_items(&items).or(a.context_usage.take());
                    a.props_dirty = true;
                    cx.notify();
                });
            })
            .detach();
            self.refresh_tabs(cx);
            self.props_dirty = true;
            cx.notify();
            return;
        }
        if !ws.is_empty() || code != self.active {
            return;
        }
        match signal {
            SessionSignal::Delta(evt) => self.apply_live_stream(StreamKind::Text, &evt, cx),
            SessionSignal::Thinking(evt) => {
                if !evt.text.is_empty() || evt.replace || evt.done {
                    self.apply_live_stream(StreamKind::Thinking, &evt, cx)
                } else {
                    self.messages =
                        note_thinking_tokens(&self.messages, evt.tokens, &evt.uuid, &evt.parent_id);
                }
            }
            SessionSignal::Step(step) => {
                if self.live_turn_closed(cx) {
                    return;
                }
                self.messages = add_tool(&self.messages, &step);
            }
            SessionSignal::ToolResult {
                id,
                result,
                is_error,
                extras,
            } => {
                self.messages = attach_tool_result(
                    &self.messages,
                    &id,
                    &result,
                    is_error,
                    &extras.unwrap_or_default(),
                );
            }
            SessionSignal::ToolInput { id, extras } => {
                self.messages = patch_tool_input(&self.messages, &id, &extras);
            }
            SessionSignal::Notice(text) => self.messages = apply_notice(&self.messages, &text),
            SessionSignal::Fallback(evt) => self.messages = apply_fallback(&self.messages, &evt),
            SessionSignal::Compact(evt) => self.messages = apply_compact(&self.messages, &evt),
            _ => return,
        }
        self.props_dirty = true;
        cx.notify();
    }

    pub(crate) fn select_tab(&mut self, code: &str, cx: &mut Context<Self>) {
        if code == self.active {
            return;
        }
        for t in self.tabs.iter_mut().filter(|t| t.code == code) {
            t.unseen = false;
        }
        self.set_active(code.to_string(), cx);
    }

    pub(crate) fn new_tab(&mut self, cx: &mut Context<Self>) {
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let Ok(tab) = services.briefing.new_app_chat().await else {
                return;
            };
            if tab.code.is_empty() {
                return;
            }
            let _ = this.update(cx, |a, cx| {
                let code = tab.code.clone();
                a.tabs.push(tab);
                a.set_active(code, cx);
            });
        })
        .detach();
    }

    pub(crate) fn close_tab(&mut self, code: &str, cx: &mut Context<Self>) {
        let services = self.services.clone();
        let code = code.to_string();
        cx.spawn(async move |this, cx| {
            let Ok(list) = services.briefing.close_app_chat(&code).await else {
                return;
            };
            let _ = this.update(cx, |a, cx| {
                if !list.is_empty() {
                    a.tabs = list;
                }
                if a.active != code {
                    cx.notify();
                    return;
                }
                if code == APP {
                    // Same code, fresh session: clear and re-pull the empty transcript.
                    a.messages.clear();
                    a.context_usage = None;
                    a.props_dirty = true;
                    a.load_active(cx);
                } else {
                    a.set_active(APP.into(), cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn remember(&mut self, model: String, flags: FlagMap, cx: &mut Context<Self>) {
        self.model = model.clone();
        self.flags = flags.clone();
        let services = self.services.clone();
        let code = self.active.clone();
        cx.background_spawn(async move {
            let flags: BTreeMap<String, String> = flags.into_iter().collect();
            let _ = services
                .briefing
                .remember_app_chat_selection(&code, &model, &flags)
                .await;
        })
        .detach();
        cx.notify();
    }

    /// `send()`: the optimistic bubble, the title stamp, then `SendAppMessage`.
    pub fn send_text(&mut self, input: &str, cx: &mut Context<Self>) {
        let text = input.trim().to_string();
        if text.is_empty() || self.sending(cx) {
            return;
        }
        let code = self.active.clone();
        self.sending_by.insert(code.clone(), true);
        self.turn_started_at = now_ms();
        self.messages.push(AgentMsg {
            me: true,
            body: text.clone(),
            optimistic: true,
            ts: Some(now_ms()),
            ..Default::default()
        });
        stamp_title(&mut self.tabs, &code, &text);
        self.props_dirty = true;
        cx.notify();
        let services = self.services.clone();
        let model = self.model.clone();
        let flags: BTreeMap<String, String> = self.flags.clone().into_iter().collect();
        cx.spawn(async move |this, cx| {
            let _ = services
                .briefing
                .send_app_message(&code, &text, &model, &flags)
                .await;
            let _ = this.update(cx, |a, cx| {
                a.sending_by.insert(code, false);
                a.props_dirty = true;
                cx.notify();
            });
        })
        .detach();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        let services = self.services.clone();
        let code = self.active.clone();
        cx.background_spawn(async move {
            let _ = services.briefing.cancel_app_message(&code).await;
        })
        .detach();
    }

    fn sync_children(&mut self, cx: &mut Context<Self>) {
        let sending = self.sending(cx);
        if self.props_dirty {
            self.props_dirty = false;
            let mut props = Self::chat_props_for(&self.active, self.visible);
            props.messages = self.messages.clone();
            props.sending = sending;
            props.turn_started_at = self.turn_started_at;
            props.context_usage = self.context_usage.clone();
            self.chat.update(cx, |c, cx| c.set_props(props, cx));
        }
        let show = !self.catalog.is_empty() && !self.model.is_empty();
        let key = (
            self.catalog.len(),
            self.model.clone(),
            self.flags.clone(),
            sending,
        );
        if show && self.pickers_key.as_ref() != Some(&key) {
            self.pickers_key = Some(key);
            let backdrop = sending.then(|| Theme::of(cx).tokens.pane);
            let (models, model, flags) =
                (self.catalog.clone(), self.model.clone(), self.flags.clone());
            self.pickers
                .update(cx, |p, cx| p.set(models, &model, &flags, backdrop, cx));
            let view = self.pickers.clone().into();
            self.chat.update(cx, |c, cx| c.set_toolbar(Some(view), cx));
        } else if !show && self.pickers_key.take().is_some() {
            self.chat.update(cx, |c, cx| c.set_toolbar(None, cx));
        }
    }
}

impl AppChat {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        self.sync_children(cx);
        let t = Theme::of(cx).tokens;
        let lit = self.visible && self.focus.contains_focused(window, cx);
        let ask = ChatMode::Ask;
        let tint = hsla(ask.tint(&t));
        let mut strip = tab_strip("app-chat-tabs", "Ask Recur chats")
            .flush(true)
            .active_key(self.active.clone());
        for tab in &self.tabs {
            let code = tab.code.clone();
            let label = tab_label(tab).to_string();
            let busy = self.tab_busy(&code, cx);
            let unseen = tab.unseen && self.active != code;
            let title = format!(
                "{label} — Ask mode{}{}",
                if busy { " — running" } else { "" },
                if unseen { " — new response" } else { "" }
            );
            let close = format!(
                "Close {}",
                if tab.title.is_empty() {
                    "chat"
                } else {
                    &tab.title
                }
            );
            let (sel, cls) = (code.clone(), code.clone());
            let me = cx.entity().downgrade();
            let me2 = me.clone();
            strip = strip.tab(
                code.clone(),
                tab_chip(
                    gpui_kit::SharedString::from(format!("ask-tab-{code}")),
                    label,
                )
                .title(title)
                .icon(ChipIcon::App(ask.icon(), Some(tint)))
                .active(self.active == code)
                .lit(lit)
                .busy(busy)
                .unseen(unseen)
                .close_label(close)
                .on_click(move |_, _, cx| {
                    let _ = me.update(cx, |a, cx| a.select_tab(&sel, cx));
                })
                .on_close(move |_, _, cx| {
                    let _ = me2.update(cx, |a, cx| a.close_tab(&cls, cx));
                }),
            );
        }
        let new_btn = recur_ui::tooltip::titled(
            "ask-new-tip",
            "New chat",
            // `.wsv-chat-new { color: var(--muted) }`; `.tab-strip-act svg` is 15px.
            tab_strip_action("ask-new", false, false, cx)
                .debug_selector(|| "ask-new".into())
                .text_color(hsla(Theme::of(cx).tokens.muted))
                .child(icon(IconName::Plus).size(px(15.)))
                .on_click(cx.guarded_listener(|a, _, _, cx| a.new_tab(cx))),
        );
        let strip = strip.trailing(new_btn);
        let toggle = recur_ui::tooltip::titled(
            "ask-hide-tip",
            "Hide chat panel",
            div()
                .id("ask-hide")
                .debug_selector(|| "ask-hide".into())
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(28.))
                .rounded(px(5.))
                .text_color(hsla(t.text))
                .cursor_pointer()
                .hover(|s| s.bg(hsla(t.pane_2)))
                .child(icon(IconName::PanelLeft).size(px(16.)))
                .on_click(cx.guarded_listener(|_, _, _, cx| cx.emit(AppChatEvent::Hide))),
        );
        let head = div()
            .flex()
            .flex_none()
            .items_stretch()
            .gap(px(2.))
            .h(px(40.))
            .pl(px(8.))
            .pr(px(4.))
            .min_w_0()
            .border_b_1()
            .border_color(hsla(t.line))
            .bg(hsla(t.pane))
            .child(div().flex().items_center().child(toggle))
            .child(div().flex_1().min_w_0().flex().child(strip));
        let focus = self.focus.clone();
        div()
            .id("app-chat")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                if !focus.contains_focused(window, cx) {
                    window.focus(&focus, cx);
                }
            })
            .child(head)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(self.chat.clone()),
            )
    }
}

impl Render for AppChat {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// The `empty` snippet: the blurb and the ask chips.
struct AskEmpty {
    app: WeakEntity<AppChat>,
}

impl AskEmpty {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let hover_bg = t.text.mix(0.06, t.pane_2);
        let chips: Vec<_> = ASK_CHIPS
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let app = self.app.clone();
                // `.ask-chips button { transition: border-color .12s, background .12s }`.
                let h = motion::hover(
                    ("ask-chip-hover", i),
                    std::time::Duration::from_millis(120),
                    motion::Easing::EASE,
                    window,
                    cx,
                );
                div()
                    .id(("ask-chip", i))
                    .debug_selector(|| format!("ask-chip:{c}"))
                    .border_1()
                    .border_color(hsla(t.line_bright.mix(h.k(), t.line)))
                    .bg(hsla(hover_bg.mix(h.k(), t.pane_2)))
                    .text_color(hsla(t.text))
                    .rounded(px(999.))
                    .px(px(12.))
                    .py(px(6.))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .on_hover(h.listener())
                    .child(label(*c, Family::Ui, 12., 400.).line_height(LineHeight::Normal))
                    .on_click(move |_, _, cx| {
                        let _ = app.update(cx, |a, cx| a.send_text(c, cx));
                    })
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .text_color(hsla(t.muted))
            .text_size(px(13.))
            .child(
                text("Ask about Now and Next across your workspaces.")
                    .size(13.)
                    .line_height(LineHeight::Scale(1.55))
                    .color(t.muted),
            )
            .child(div().flex().flex_wrap().gap(px(8.)).children(chips))
    }
}

impl Render for AskEmpty {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_stamp_and_label() {
        let mut tabs = vec![
            WorkspaceChatTab {
                code: "APP".into(),
                title: "New chat".into(),
                ..Default::default()
            },
            WorkspaceChatTab {
                code: "AP2".into(),
                title: "Kept".into(),
                ..Default::default()
            },
        ];
        stamp_title(&mut tabs, "APP", "\n  first line \nsecond");
        stamp_title(&mut tabs, "AP2", "ignored");
        assert_eq!(tabs[0].title, "first line");
        assert_eq!(tabs[1].title, "Kept");
        tabs[0].title.clear();
        assert_eq!(tab_label(&tabs[0]), "New chat");
    }
}
