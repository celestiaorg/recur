//! `FanOutCard.svelte`: one wave of the board. Each child's live state comes from the workspace
//! store; a live row opens the child, a settled row expands into the child's full loop
//! transcript (conductor turns, specialist cards and engine facts merged as its loop tab showed
//! them), read-only. A Plan-panel link can ask for a child (`childTranscriptRequest`).

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Task,
    Window, div, px,
};
use recur_domain::{Finding, Workspace};
use recur_engine::view::chat::{AgentMsg, map_chat_items};
use recur_engine::view::loop_cockpit::{FeedSess, is_pair_chat_code, merge_loop_cockpit};
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{IconName, Theme, icon, motion};
use recur_view_core::ViewContext;
use recur_view_core::modes::{ChatMode, workspace_is_running};
use serde::Deserialize;

use crate::cards::plan::running_dot;
use crate::cards::session::{SessionCard, SessionCardProps, steps_slot};
use crate::chat::{AgentChat, ChatEvent, ChatProps, SessionCardRenderer};
use crate::style::{hsla, label};

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct FanOutChild {
    pub id: String,
    pub title: String,
    pub steps: Vec<String>,
    pub mode: String,
    pub state: String,
    pub note: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct FanOutWave {
    pub wave: i64,
    pub title: Option<String>,
    pub after: Vec<String>,
    pub children: Vec<FanOutChild>,
    pub done: bool,
}

/// `JSON.parse(data)` with a `children` array.
pub fn parse_wave(data: &str) -> Option<FanOutWave> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    v.get("children")?.as_array()?;
    serde_json::from_value(v).ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKey {
    Running,
    Reviewing,
    Needs,
    Merged,
    Failed,
    Conflict,
    Queued,
    Done,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RowState {
    pub key: RowKey,
    pub label: String,
    pub live: bool,
}

fn rs(key: RowKey, label: impl Into<String>, live: bool) -> RowState {
    RowState {
        key,
        label: label.into(),
        live,
    }
}

/// `rowState`.
pub fn row_state(
    c: &FanOutChild,
    ws: Option<&Workspace>,
    running: &std::collections::HashSet<String>,
) -> RowState {
    let fin = c.state.to_lowercase();
    if fin.starts_with("conflict") {
        return rs(RowKey::Conflict, c.state.clone(), false);
    }
    if fin.starts_with("failed") || fin.starts_with("stopped") {
        return rs(RowKey::Failed, c.state.clone(), false);
    }
    if fin == "closed" {
        return rs(RowKey::Done, "closed", false);
    }
    if fin == "merged" || ws.is_some_and(|w| w.state.as_str() == "merged" || w.archived) {
        return rs(RowKey::Merged, "merged", false);
    }
    let Some(ws) = ws else {
        return rs(
            RowKey::Queued,
            if c.state.is_empty() {
                "starting".into()
            } else {
                c.state.clone()
            },
            false,
        );
    };
    let is_running = workspace_is_running(ws, running);
    let state = ws.state.as_str();
    if is_running && state != "escalated" {
        let phase = if !c.steps.is_empty()
            && !ws.phase.is_empty()
            && ws.phase != "idle"
            && ws.phase != "starting"
        {
            ws.phase.clone()
        } else {
            "working".into()
        };
        return rs(RowKey::Running, phase, true);
    }
    if c.steps.is_empty() && !["failed", "stopped", "escalated"].contains(&state) {
        let finished = c.state == "settled" || ws.phase == "idle";
        return rs(
            RowKey::Done,
            if finished { "finished" } else { "idle" },
            false,
        );
    }
    let or = |a: &str, b: &str, c: &str| -> String {
        if !a.is_empty() {
            a.into()
        } else if !b.is_empty() {
            b.into()
        } else {
            c.into()
        }
    };
    match state {
        "escalated" => rs(RowKey::Needs, "needs you", false),
        "reviewing" => rs(RowKey::Reviewing, "reviewing", is_running),
        "coding" | "planning" => rs(
            RowKey::Running,
            or(&ws.phase, &ws.status_label, "working"),
            is_running,
        ),
        "queued" => rs(RowKey::Queued, "queued", false),
        "failed" | "stopped" => rs(RowKey::Failed, state, false),
        "paused" => rs(
            RowKey::Done,
            if ws.phase == "artifact" || ws.phase == "idle" {
                "finished"
            } else {
                "paused"
            },
            false,
        ),
        _ => rs(RowKey::Running, or(&ws.status_label, state, ""), is_running),
    }
}

/// `summary`: `N merged` / `M of N merged`.
pub fn wave_summary(states: &[RowState]) -> String {
    let n = states.len();
    let merged = states.iter().filter(|s| s.key == RowKey::Merged).count();
    if merged == n {
        format!("{n} merged")
    } else {
        format!("{merged} of {n} merged")
    }
}

struct Transcript {
    loading: bool,
    messages: Vec<AgentMsg>,
    findings: Vec<Finding>,
    chat: Option<Entity<AgentChat>>,
}

pub struct FanOutCard {
    vcx: ViewContext,
    data: String,
    fallback: String,
    open_ids: BTreeMap<String, bool>,
    transcripts: HashMap<String, Transcript>,
    loads: HashMap<String, Task<()>>,
    flash: Option<(String, Instant)>,
    /// The host chat's inherited font size, for the child transcripts.
    pub(crate) font_size: f32,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ChatEvent> for FanOutCard {}

impl FanOutCard {
    pub fn new(vcx: ViewContext, data: String, fallback: String, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&vcx.state, |c: &mut Self, _, cx| {
            c.take_child_request(cx);
            cx.notify();
        });
        let mut c = FanOutCard {
            vcx,
            data,
            fallback,
            open_ids: BTreeMap::new(),
            transcripts: HashMap::new(),
            loads: HashMap::new(),
            flash: None,
            font_size: 13.,
            _subs: vec![sub],
        };
        c.take_child_request(cx);
        c
    }

    pub fn set_data(&mut self, data: String, fallback: String, cx: &mut Context<Self>) {
        if data != self.data || fallback != self.fallback {
            self.data = data;
            self.fallback = fallback;
            cx.notify();
        }
    }

    fn wave(&self) -> Option<FanOutWave> {
        parse_wave(&self.data)
    }

    fn workspace<'a>(&self, id: &str, cx: &'a gpui_kit::App) -> Option<&'a Workspace> {
        self.vcx
            .state
            .read(cx)
            .workspaces
            .iter()
            .find(|w| w.id == id)
    }

    fn is_openable(&self, c: &FanOutChild, cx: &gpui_kit::App) -> bool {
        self.workspace(&c.id, cx).is_some_and(|w| !w.archived)
    }

    /// A Plan-panel link asked for one of our children: expand it and flash the row.
    fn take_child_request(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self
            .vcx
            .state
            .read(cx)
            .child_transcript_request()
            .map(str::to_string)
        else {
            return;
        };
        let Some(child) = self
            .wave()
            .and_then(|w| w.children.into_iter().find(|c| c.id == id))
        else {
            return;
        };
        self.vcx
            .state
            .update(cx, |s, _| s.clear_child_transcript_request());
        self.flash = Some((id.clone(), Instant::now()));
        self.open_ids.insert(id, true);
        self.load_transcript(&child, cx);
        cx.notify();
    }

    fn load_transcript(&mut self, c: &FanOutChild, cx: &mut Context<Self>) {
        let id = c.id.clone();
        if self.transcripts.contains_key(&id) {
            return;
        }
        self.transcripts.insert(
            id.clone(),
            Transcript {
                loading: true,
                messages: Vec::new(),
                findings: Vec::new(),
                chat: None,
            },
        );
        let ws = self.workspace(&id, cx).cloned();
        let owner = if c.steps.is_empty() {
            "PA".to_string()
        } else {
            ws.as_ref()
                .map(|w| w.loop_chat_code.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "MA".into())
        };
        let goal = ws.as_ref().map(|w| w.goal.clone()).unwrap_or_default();
        let services = self.vcx.services.clone();
        let key = id.clone();
        self.loads.insert(
            id,
            cx.spawn(async move |this, cx| {
                let (items, sv, fx, findings) = futures::join!(
                    services.workspaces.workspace_chat_items(&key, &owner),
                    services.workspaces.workspace_sessions(&key, &owner, ""),
                    services.workspaces.workspace_facts(&key),
                    services.observatory.list_findings(&key),
                );
                let conductor = map_chat_items(&items.unwrap_or_default());
                let sessions: Vec<FeedSess> = sv
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|s| !is_pair_chat_code(&s.code))
                    .map(|s| FeedSess {
                        messages: map_chat_items(s.items.as_deref().unwrap_or_default()),
                        id: s.id,
                        code: s.code,
                        name: s.name,
                        kind: s.kind,
                        ts: s.created_at,
                        live_ts: None,
                    })
                    .collect();
                let messages =
                    merge_loop_cockpit(&conductor, &sessions, &fx.unwrap_or_default(), &[], &goal);
                let _ = this.update(cx, |c, cx| {
                    c.transcripts.insert(
                        key.clone(),
                        Transcript {
                            loading: false,
                            messages,
                            findings: findings.unwrap_or_default(),
                            chat: None,
                        },
                    );
                    c.loads.remove(&key);
                    cx.notify();
                });
            }),
        );
    }

    fn on_row(&mut self, c: &FanOutChild, cx: &mut Context<Self>) {
        if self.is_openable(c, cx) {
            let id = c.id.clone();
            self.vcx
                .state
                .update(cx, |s, cx| s.open_workspace(&id, false, cx));
            return;
        }
        let next = !self.open_ids.get(&c.id).copied().unwrap_or(false);
        self.open_ids.insert(c.id.clone(), next);
        if next {
            self.load_transcript(c, cx);
        }
        cx.notify();
    }

    /// The read-only chat for a child's transcript, with specialist cards.
    fn child_chat(
        &mut self,
        child: &FanOutChild,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<AgentChat>> {
        let root_path = self
            .workspace(&child.id, cx)
            .map(|w| w.worktree_path.clone())
            .unwrap_or_default();
        let t = self.transcripts.get_mut(&child.id)?;
        if let Some(c) = &t.chat {
            return Some(c.clone());
        }
        let vcx = self.vcx.clone();
        let props = ChatProps {
            messages: t.messages.clone(),
            readonly: true,
            workspace_id: child.id.clone(),
            empty_text: "No transcript was recorded for this child.".into(),
            session_body: true,
            session_max: 360.,
            font_size: self.font_size,
            ..ChatProps::default()
        };
        let findings = t.findings.clone();
        let ws_id = child.id.clone();
        let font_size = self.font_size;
        let chat = cx.new(|cx| {
            let mut c = AgentChat::new(&vcx, props, window, cx);
            let vcx2 = vcx.clone();
            let cards: std::rc::Rc<std::cell::RefCell<HashMap<String, Entity<SessionCard>>>> =
                Default::default();
            let renderer: SessionCardRenderer = std::rc::Rc::new(move |log, first, _, cx| {
                let key = if log.id.is_empty() {
                    log.code.clone()
                } else {
                    log.id.clone()
                };
                let props = SessionCardProps {
                    code: log.code.clone(),
                    name: log.name.clone(),
                    messages: log.messages.clone(),
                    live: false,
                    presentation_active: true,
                    root_path: root_path.clone(),
                    workspace_id: ws_id.clone(),
                    findings: findings.clone(),
                    open_file: false,
                    first,
                    font_size,
                };
                let card = cards
                    .borrow_mut()
                    .entry(key)
                    .or_insert_with(|| cx.new(|_| SessionCard::new(vcx2.clone(), props.clone())))
                    .clone();
                card.update(cx, |c, cx| c.set_props(props, cx));
                card.into_any_element()
            });
            c.set_session_card(Some(renderer), cx);
            c
        });
        t.chat = Some(chat.clone());
        Some(chat)
    }
}

impl Render for FanOutCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        let Some(wave) = self.wave() else {
            let mut st = crate::html_inline::InlineStyle::meta(11., t.faint, &t);
            st.family = Family::Mono;
            return div()
                .flex()
                .items_baseline()
                .gap(px(7.))
                .pt(px(3.))
                .pb(px(3.))
                .pl(px(2.))
                .pr(px(6.))
                .text_size(px(12.))
                .child(div().flex_1().min_w_0().overflow_hidden().child(
                    crate::html_inline::inline_html("fanout-fallback".into(), &self.fallback, st),
                ))
                .into_any_element();
        };
        let running = self.vcx.state.read(cx).workspaces_running.clone();
        let states: Vec<RowState> = wave
            .children
            .iter()
            .map(|c| row_state(c, self.workspace(&c.id, cx), &running))
            .collect();
        let n = wave.children.len();
        let mut head = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .py(px(6.))
            .text_size(px(12.))
            .text_color(hsla(t.muted))
            .border_b_1()
            .border_color(hsla(t.line))
            .child(
                div()
                    .size(px(14.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(IconName::GitFork).size(px(14.)).color(hsla(t.amber))),
            )
            .child(
                label(
                    wave.title
                        .clone()
                        .unwrap_or_else(|| format!("Wave {}", wave.wave)),
                    Family::Ui,
                    12.,
                    600.,
                )
                .color(t.text),
            );
        if !wave.after.is_empty() {
            head = head.child(recur_ui::tooltip::titled(
                SharedString::from(format!("fanout-{}-after", wave.wave)),
                "Started when these steps landed",
                label(
                    format!("after {}", wave.after.join(", ")),
                    Family::Ui,
                    12.,
                    400.,
                )
                .color(t.muted),
            ));
        }
        head = head
            .child(
                label(
                    format!("{n} child workspace{}", if n == 1 { "" } else { "s" }),
                    Family::Ui,
                    12.,
                    400.,
                )
                .color(t.muted),
            )
            .child(
                div()
                    .ml_auto()
                    .child(label(wave_summary(&states), Family::Ui, 11., 400.).color(t.muted)),
            );
        let mut card = recur_ui::style::card(t.pane, t.line, 8.)
            .overflow_hidden()
            .child(head);
        let flash = self.flash.clone();
        for (ci, c) in wave.children.iter().enumerate() {
            let st = &states[ci];
            let openable = self.is_openable(c, cx);
            let expanded = self.open_ids.get(&c.id).copied().unwrap_or(false);
            let id = SharedString::from(format!("fanout-row-{}", c.id));
            let group = SharedString::from(format!("{id}-g"));
            let mut chips = div().flex().gap(px(4.));
            let tstep = |s: String, fg: recur_ui::Color, bg: recur_ui::Color| {
                div()
                    .bg(hsla(bg))
                    .rounded(px(4.))
                    .px(px(5.))
                    .py(px(2.))
                    .child(
                        label(s, Family::Mono, 10., 400.)
                            .line_height(LineHeight::Normal)
                            .color(fg),
                    )
            };
            if !c.mode.is_empty() {
                let mm = ChatMode::from_key(&c.mode);
                chips = chips.child(tstep(
                    mm.label().into(),
                    mm.tint(&t),
                    mm.tint(&t).mix(0.14, recur_ui::color::TRANSPARENT),
                ));
            }
            for s in &c.steps {
                chips = chips.child(tstep(s.clone(), t.faint, t.pane_2));
            }
            let mut name = div().flex_1().min_w_0().overflow_hidden().child(
                text(c.title.clone())
                    .color(t.text)
                    .line_height(LineHeight::Normal)
                    .truncate(),
            );
            if c.steps.is_empty() && !c.note.is_empty() {
                name = name.child(
                    text(c.note.clone())
                        .id(SharedString::from(format!("{id}-note")))
                        .size(11.)
                        .color(t.faint),
                );
            }
            let color = match st.key {
                RowKey::Merged => t.indigo,
                RowKey::Failed | RowKey::Conflict => t.coral,
                RowKey::Needs => t.amber,
                _ => t.muted,
            };
            let mut state = div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(11.))
                .text_color(hsla(color));
            state = if st.live {
                state.child(
                    div()
                        .size(px(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(running_dot(&t)),
                )
            } else {
                match st.key {
                    RowKey::Merged => state.child(icon(IconName::GitMerge).size(px(12.))),
                    RowKey::Needs => state.child(icon(IconName::CircleAlert).size(px(12.))),
                    RowKey::Failed | RowKey::Conflict => {
                        state.child(icon(IconName::OctagonX).size(px(12.)))
                    }
                    _ => state,
                }
            };
            state = state.child(
                text(st.label.clone())
                    .line_height(LineHeight::Normal)
                    .nowrap(),
            );
            let tip = if openable {
                format!("Open {}", c.id)
            } else if !c.note.is_empty() {
                format!("{} — click for the transcript", c.note)
            } else {
                "Show the child transcript".into()
            };
            // `fanout-flash`: amber-d through 30%, fading out by 1.4s.
            let flash_bg = flash
                .as_ref()
                .filter(|(fid, _)| fid == &c.id)
                .map(|(_, at)| {
                    let p = motion::progress(*at, Duration::from_millis(1400), cx);
                    if p < 1. && !motion::is_frozen(cx) {
                        window.request_animation_frame();
                    }
                    let k = if p <= 0.3 {
                        1.
                    } else {
                        1. - motion::Easing::EASE_OUT.apply((p - 0.3) / 0.7)
                    };
                    t.amber_d.fade(k)
                });
            let child = c.clone();
            let row = div()
                .id(id.clone())
                .group(group.clone())
                .flex()
                .items_center()
                .gap(px(10.))
                .w_full()
                .px(px(10.))
                .py(px(5.))
                .text_size(px(12.))
                .when(ci > 0, |d| d.border_t_1().border_color(hsla(t.line)))
                .when_some(flash_bg, |d, bg| d.bg(hsla(bg)))
                .cursor_pointer()
                .hover(|s| s.bg(hsla(t.pane_2)))
                .child(steps_slot(
                    &group,
                    chips.into_any_element(),
                    !openable,
                    expanded,
                    &t,
                ))
                .child(name)
                .child(state)
                .on_click(cx.listener(move |c, _, _, cx| c.on_row(&child, cx)));
            card = card.child(recur_ui::tooltip::titled(
                SharedString::from(format!("{id}-tip")),
                tip,
                row,
            ));
            if expanded && !openable {
                let body: AnyElement = match self.transcripts.get(&c.id) {
                    Some(tr) if !tr.loading && !tr.messages.is_empty() => {
                        let chat = self.child_chat(c, window, cx);
                        // `.fanout-transcript .fanout-body`: no card, capped at 360px.
                        div()
                            .max_h(px(360.))
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .children(chat)
                            .into_any_element()
                    }
                    Some(tr) if !tr.loading => {
                        empty_note("No transcript was recorded for this child.", &t)
                    }
                    _ => empty_note("Loading transcript…", &t),
                };
                card = card.child(
                    div()
                        .relative()
                        .pt(px(1.))
                        .child(recur_ui::style::dashed_top_border(1., t.line))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(body),
                );
            }
        }
        card.into_any_element()
    }
}

/// `.fanout-transcript-empty` (unstyled: the card's 13px text).
fn empty_note(s: &str, t: &recur_ui::Tokens) -> AnyElement {
    div()
        .child(text(s.to_string()).size(13.).color(t.text))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child(state: &str, steps: &[&str]) -> FanOutChild {
        FanOutChild {
            id: "c1".into(),
            state: state.into(),
            steps: steps.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn row_states_follow_the_store_and_the_final_outcome() {
        let none = std::collections::HashSet::new();
        assert_eq!(
            row_state(&child("conflict in a.go", &[]), None, &none).key,
            RowKey::Conflict
        );
        assert_eq!(
            row_state(&child("", &["S1"]), None, &none).label,
            "starting"
        );
        let mut ws = Workspace {
            id: "c1".into(),
            ..Default::default()
        };
        ws.state = recur_domain::WorkspaceState::Escalated;
        assert_eq!(
            row_state(&child("", &["S1"]), Some(&ws), &none).key,
            RowKey::Needs
        );
        let running: std::collections::HashSet<String> = ["c1".to_string()].into();
        ws.state = recur_domain::WorkspaceState::Coding;
        ws.phase = "editing".into();
        let r = row_state(&child("", &["S1"]), Some(&ws), &running);
        assert!(r.live && r.label == "editing");
        let merged = rs(RowKey::Merged, "merged", false);
        assert_eq!(wave_summary(&[merged.clone(), merged]), "2 merged");
        assert!(parse_wave("{\"wave\":1}").is_none());
    }
}
