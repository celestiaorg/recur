//! The titlebar (`.titlebar` in App.svelte, `_base.scss`): the traffic-light spacer, the
//! sidebar toggle, the breadcrumb and back buttons, the dev instance chip, and the per-view
//! chrome on the right.

use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, Hsla, InteractiveElement, IntoElement,
    MouseButton, ParentElement, SharedString, StatefulInteractiveElement, Styled, Task, Window,
    div, prelude::FluentBuilder, px,
};
use recur_ui::fonts::{Family, weight};
use recur_ui::style::{self, Tbtn, TbtnSize, tbtn};
use recur_ui::text::{InlineBox, Span, Text, TextStyled, span, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::decode_text::decode_frame;
use recur_ui::widgets::{ToastKind, armed_button};
use recur_ui::{IconName, Theme, icon};
use recur_view_core::modes::{workspace_can_remove, workspace_shows_remove, ws_main_blocked};
use recur_view_core::run::{run_label, workspace_dev_cmd, workspace_run};
use recur_view_core::{AppState, CrumbStyle, ViewName, toast};

use crate::chrome::{no_drag, titlebar_mouse_down};
use crate::shell::Shell;

/// `.titlebar { height: 52px }`: the macOS toolbar height the traffic lights center in.
pub const TITLEBAR_HEIGHT: f32 = 52.;
/// `.tb-traffic-clear`: room for the native traffic lights.
const TRAFFIC_CLEAR_WIDTH: f32 = 68.;

/// Whether the titlebar leaves room for the native traffic lights (macOS).
pub const TRAFFIC_LIGHTS: bool = cfg!(target_os = "macos");

/// DecodeText for the live workspace name in the breadcrumb: scramble, then resolve left to
/// right when the name changes; switching workspaces snaps.
#[derive(Default)]
pub(crate) struct CrumbName {
    display: SharedString,
    decoding: bool,
    seen: SharedString,
    seen_key: SharedString,
    timer: Option<Task<()>>,
}

/// State behind the titlebar's dynamic chrome.
#[derive(Default)]
pub(crate) struct TitlebarState {
    pub name: CrumbName,
    /// The dirty / unmerged work Remove would discard (`WorkspaceDiscardRisk`).
    discard_risk: String,
    discard_seq: u64,
    discard_for: (String, bool),
}

impl Shell {
    /// Store changes the titlebar follows: the discard risk and the land chrome.
    pub(crate) fn titlebar_state_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = {
            let s = self.state.read(cx);
            let id = s.selected_workspace_id.clone();
            let can = s.view == ViewName::Detail
                && workspace_can_remove(s.workspace(&id), s.workspaces_running.contains(&id));
            (id, can)
        };
        if self.titlebar.discard_for != key {
            self.titlebar.discard_for = key;
            self.refresh_discard_risk(cx);
        }
        self.land_state_changed(window, cx);
    }

    /// `refreshDiscardRisk`: the confirm copy for Remove when the worktree has uncommitted or
    /// unmerged work.
    pub(crate) fn refresh_discard_risk(&mut self, cx: &mut Context<Self>) {
        let (id, can) = {
            let s = self.state.read(cx);
            let id = s.selected_workspace_id.clone();
            let can = s.view == ViewName::Detail
                && workspace_can_remove(s.workspace(&id), s.workspaces_running.contains(&id));
            (id, can)
        };
        self.titlebar.discard_seq += 1;
        if id.is_empty() || !can {
            self.titlebar.discard_risk.clear();
            return;
        }
        let seq = self.titlebar.discard_seq;
        let services = self.state.read(cx).services().clone();
        cx.spawn(async move |this, cx| {
            let risk = services.workspaces.workspace_discard_risk(&id).await;
            let _ = this.update(cx, |shell, cx| {
                if shell.titlebar.discard_seq == seq {
                    shell.titlebar.discard_risk = risk.unwrap_or_default();
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

impl CrumbName {
    /// The name to paint for `text` of the entity `key` (DecodeText semantics).
    fn sync(
        &mut self,
        next: SharedString,
        key: SharedString,
        reduced: bool,
        cx: &mut Context<Shell>,
    ) -> SharedString {
        let snap = |s: &mut CrumbName| {
            s.timer = None;
            s.decoding = false;
            s.display = next.clone();
            s.seen = next.clone();
            s.seen_key = key.clone();
        };
        if self.seen.is_empty() || (!key.is_empty() && key != self.seen_key) {
            snap(self);
            return self.display.clone();
        }
        if self.seen == next {
            return self.display.clone();
        }
        self.seen = next.clone();
        self.timer = None;
        if next.is_empty() || reduced {
            self.decoding = false;
            self.display = next.clone();
            return self.display.clone();
        }
        self.decoding = true;
        let target = next.clone();
        self.timer = Some(cx.spawn(async move |this, cx| {
            let mut seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0x9e37_79b9, |d| d.as_nanos() as u64)
                | 1;
            let mut random = move || {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed as usize
            };
            let mut tick = 0;
            loop {
                cx.background_executor()
                    .timer(recur_ui::widgets::decode_text::TICK)
                    .await;
                tick += 1;
                let frame = decode_frame(&target, tick, &mut random);
                let done = frame.is_none();
                let alive = this.update(cx, |shell, cx| {
                    let n = &mut shell.titlebar.name;
                    n.display = frame.map_or_else(|| target.clone(), Into::into);
                    if done {
                        n.decoding = false;
                        n.timer = None;
                    }
                    cx.notify();
                });
                if done || alive.is_err() {
                    break;
                }
            }
        }));
        self.display.clone()
    }
}

/// `copySelectedWorkspacePath`: copy the on-screen workspace's folder to the clipboard.
pub fn copy_selected_workspace_path(state: &Entity<AppState>, cx: &mut App) {
    let ws = state.read(cx).selected_workspace().cloned();
    if ws_main_blocked(ws.as_ref()) {
        return;
    }
    let services = state.read(cx).services().clone();
    cx.spawn(async move |cx| {
        let mut path = ws
            .as_ref()
            .map(|w| w.worktree_path.trim().to_string())
            .unwrap_or_default();
        if path.is_empty()
            && let Some(w) = &ws
        {
            path = services
                .code
                .abs_path(&w.project_id, &w.id, "")
                .await
                .map(|p| p.trim().to_string())
                .unwrap_or_default();
        }
        cx.update(|cx| {
            if path.is_empty() {
                toast(ToastKind::Error, "This workspace has no folder on disk", cx);
                return;
            }
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(path));
            toast(ToastKind::Info, "Copied path", cx);
        });
    })
    .detach();
}

/// The crumb's mono 11.5px text, with `<b>`, `.on` and `.crumb-branch` runs.
fn crumb_span(seg: &recur_view_core::CrumbSeg, cx: &App) -> Span {
    let t = Theme::of(cx).tokens;
    let s = span(seg.text.clone());
    match seg.style {
        CrumbStyle::Plain => s,
        CrumbStyle::Bold => s.color(t.muted).weight(weight::MEDIUM),
        CrumbStyle::On => s.color(t.text),
        CrumbStyle::BoldOn => s.color(t.text).weight(weight::MEDIUM),
        CrumbStyle::Branch => s.color(t.muted).size(10.5).boxed(InlineBox {
            margin_left: 8.,
            border: 1.,
            border_color: t.line.into(),
            pad_top: 1.,
            pad_right: 6.,
            pad_bottom: 1.,
            pad_left: 6.,
            radius: 5.,
            ..Default::default()
        }),
    }
}

/// `.crumb`: mono 11.5px `--faint`, truncating inside `.tb-left`.
fn crumb(spans: Vec<Span>, cx: &App) -> gpui_kit::Div {
    let t = Theme::of(cx).tokens;
    div().flex_initial().min_w_0().overflow_hidden().child(
        Text::from_spans(spans)
            .mono()
            .size(11.5)
            .color(t.faint)
            .truncate(),
    )
}

/// `.tb-back`: the mono back link.
fn back_button(
    id: &'static str,
    label: &'static str,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Theme::of(cx).tokens;
    no_drag(
        div()
            .id(id)
            .debug_selector(move || id.into())
            .flex_none()
            .cursor_pointer()
            .text_color(t.muted)
            .hover(move |s| s.text_color(Hsla::from(t.text)))
            .child(style::button_label(label, 11.5, weight::REGULAR).family(Family::Mono)),
    )
}

/// `.tb-sub`: mono 11px `--faint` counts beside the crumb.
fn tb_sub(label: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    let t = Theme::of(cx).tokens;
    div()
        .flex_none()
        .child(text(label).mono().size(11.).color(t.faint).nowrap())
}

impl Shell {
    pub(crate) fn render_titlebar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let state = self.state.clone();
        let (view, rail_collapsed, label) = {
            let s = state.read(cx);
            (s.view, s.rail_collapsed, s.instance_label.clone())
        };
        let left = self.titlebar_left(view, rail_collapsed, window, cx);
        let right = self.titlebar_right(view, window, cx);
        div()
            .id("titlebar")
            .relative()
            .flex_none()
            .h(px(TITLEBAR_HEIGHT))
            .flex()
            .items_center()
            .gap(px(13.))
            .px(px(14.))
            .bg(t.titlebar)
            .border_b(px(1.))
            .border_color(Hsla::from(t.line))
            .on_mouse_down(MouseButton::Left, |ev, window, cx| {
                titlebar_mouse_down(ev, true, window);
                cx.stop_propagation();
            })
            .child(left)
            .when_some(
                (!label.is_empty()).then(|| instance_chip(&label, cx)),
                |d, chip| d.child(chip),
            )
            .child(right)
            .into_any_element()
    }

    fn titlebar_left(
        &mut self,
        view: ViewName,
        rail_collapsed: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let state = self.state.clone();
        let mut left = div()
            .id("tb-left")
            .flex_1()
            .flex()
            .items_center()
            .gap(px(10.))
            .min_w_0()
            .overflow_hidden()
            .p(px(6.))
            .m(px(-6.));
        if TRAFFIC_LIGHTS {
            // Drag-to-move, but no zoom: the traffic lights sit here.
            left = left.child(
                div()
                    .id("tb-traffic-clear")
                    .flex_none()
                    .w(px(TRAFFIC_CLEAR_WIDTH))
                    .h_full()
                    .on_mouse_down(MouseButton::Left, |ev, window, cx| {
                        titlebar_mouse_down(ev, false, window);
                        cx.stop_propagation();
                    }),
            );
        }
        let title = if rail_collapsed {
            "Show sidebar (⌘B)"
        } else {
            "Hide sidebar (⌘B)"
        };
        let rail = no_drag(style::rail_toggle("tb-rail", !rail_collapsed, cx))
            .debug_selector(|| "tb-rail".into())
            .on_click({
                let state = state.clone();
                move |_, _, cx| state.update(cx, |s, cx| s.toggle_rail(cx))
            })
            .child(icon(IconName::PanelLeft).size(px(16.)));
        left = left
            .child(titled("tb-rail-tip", title, rail))
            .child(div().flex_none().w(px(1.)).h(px(16.)).bg(t.line));

        let s = state.read(cx);
        match view {
            ViewName::Detail => {
                let back = back_button("tb-back", "← inbox", cx).on_click({
                    let state = state.clone();
                    move |_, _, cx| state.update(cx, |s, cx| s.go(ViewName::Briefing, None, cx))
                });
                left = left.child(back);
                if let Some(ws) = s.selected_workspace().cloned() {
                    let name: SharedString = [&ws.short, &ws.branch, &ws.id]
                        .into_iter()
                        .find(|x| !x.is_empty())
                        .cloned()
                        .unwrap_or_default()
                        .into();
                    let reduced = recur_ui::motion::reduced(cx) || recur_ui::motion::is_frozen(cx);
                    let shown = self
                        .titlebar
                        .name
                        .sync(name, ws.id.clone().into(), reduced, cx);
                    let decoding = self.titlebar.name.decoding;
                    let plain = format!(
                        "/ {} / {shown}{}",
                        ws.project_id,
                        if ws.branch.is_empty() {
                            String::new()
                        } else {
                            format!(" {}", ws.branch)
                        }
                    );
                    let mut spans = vec![
                        span("/ "),
                        span(ws.project_id.clone())
                            .color(t.muted)
                            .weight(weight::MEDIUM),
                        span(" / "),
                    ];
                    // `.decoding { opacity: .85 }` on the name.
                    let on = if decoding { t.text.fade(0.85) } else { t.text };
                    spans.push(span(shown).color(on));
                    if !ws.branch.is_empty() {
                        spans.push(span(" "));
                        spans.push(crumb_span(
                            &recur_view_core::CrumbSeg {
                                text: ws.branch.clone(),
                                style: CrumbStyle::Branch,
                            },
                            cx,
                        ));
                    }
                    left = left
                        .child(crumb(spans, cx).debug_selector(move || format!("crumb={plain}")));
                    if !ws.is_main {
                        left = left.child(no_drag(
                            div()
                                .id("crumb-arrow")
                                .flex_none()
                                .mx(px(2.))
                                .ml(px(4.))
                                .child(text("→").size(11.).color(t.faint).nowrap()),
                        ));
                        left = left.child(self.base_select());
                    }
                } else {
                    let mut spans = vec![span("/ ")];
                    spans.extend(s.crumb.iter().map(|c| crumb_span(c, cx)));
                    left = left.child(crumb(spans, cx));
                }
            }
            ViewName::PrDetail => {
                let back = back_button("tb-back", "← pull requests", cx).on_click({
                    let state = state.clone();
                    move |_, _, cx| state.update(cx, |s, cx| s.go(ViewName::Pr, None, cx))
                });
                let num = s
                    .pr(&s.selected_pr_id)
                    .map(|p| p.number.to_string())
                    .unwrap_or_else(|| s.selected_pr_id.clone());
                left = left.child(back).child(crumb(
                    vec![
                        span("/ "),
                        span(format!("#{num}")).color(t.text).weight(weight::MEDIUM),
                    ],
                    cx,
                ));
            }
            ViewName::Project => {
                let spans = match s.project(&s.project_form_id) {
                    Some(p) => vec![
                        span("Project").color(t.muted).weight(weight::MEDIUM),
                        span(" / "),
                        span(p.name.clone()).color(t.text),
                    ],
                    None => vec![span("New project").color(t.text).weight(weight::MEDIUM)],
                };
                left = left.child(crumb(spans, cx));
            }
            _ => {
                let spans = s.crumb.iter().map(|c| crumb_span(c, cx)).collect();
                left = left.child(crumb(spans, cx));
                match view {
                    ViewName::Briefing => left = left.child(inbox_counts(s, cx)),
                    ViewName::Schedules => left = left.child(schedule_counts(s, cx)),
                    _ => {}
                }
            }
        }
        if let (Some(part), _) = self.titlebar_parts(view) {
            left = left.child(part);
        }
        left.into_any_element()
    }

    fn titlebar_right(
        &mut self,
        view: ViewName,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.clone();
        let mut right = div()
            .id("tb-right")
            .flex_none()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(6.));
        let s = state.read(cx);
        if view == ViewName::Briefing {
            let unread = s.inbox.iter().filter(|i| i.unread).count();
            if unread > 0 {
                right = right.child(no_drag(
                    tbtn("tb-mark-read", Tbtn::Default, TbtnSize::TITLEBAR, None, cx)
                        .on_click({
                            let state = state.clone();
                            move |_, _, cx| mark_all_inbox_read(&state, cx)
                        })
                        .child(style::button_label("Mark all read", 11.5, weight::MEDIUM)),
                ));
            }
        }
        let detail_ws = (view == ViewName::Detail)
            .then(|| s.selected_workspace().cloned())
            .flatten();
        if let Some(ws) = detail_ws.as_ref().filter(|w| !ws_main_blocked(Some(w))) {
            let open = s.overlays.open_in;
            let _ = ws;
            right = right
                .child(titled(
                    "tb-open-in-tip",
                    "Open in… (⌘O)",
                    no_drag(style::titlebar_icon_button("tb-open-in", open, cx))
                        .on_click({
                            let state = state.clone();
                            move |_, _, cx| state.update(cx, |s, cx| s.toggle_open_in(cx))
                        })
                        .child(icon(IconName::QueuePopOut).size(px(14.))),
                ))
                .child(titled(
                    "tb-copy-path-tip",
                    "Copy path",
                    no_drag(style::titlebar_icon_button("tb-copy-path", false, cx))
                        .on_click({
                            let state = state.clone();
                            move |_, _, cx| copy_selected_workspace_path(&state, cx)
                        })
                        .child(icon(IconName::Copy).size(px(14.))),
                ));
        }
        if let Some(ws) = detail_ws.as_ref() {
            let can_run = !ws_main_blocked(Some(ws))
                && !workspace_dev_cmd(&ws.id, &s.workspaces, &s.projects).is_empty();
            if can_run {
                right = right.children(self.run_splits(ws, cx));
            }
        }
        right = right.children(self.land_chrome(view, window, cx));
        if let (_, Some(part)) = self.titlebar_parts(view) {
            right = right.child(part);
        }
        let s = state.read(cx);
        if view == ViewName::Detail
            && let Some(ws) = s.selected_workspace()
            && workspace_shows_remove(Some(ws))
        {
            let can = workspace_can_remove(Some(ws), s.workspaces_running.contains(&ws.id));
            let risk = &self.titlebar.discard_risk;
            let confirm = if risk.is_empty() {
                "Confirm remove".to_string()
            } else {
                format!("Confirm remove — {risk}")
            };
            let id = ws.id.clone();
            let backdrop = Theme::of(cx).tokens.titlebar;
            let mut btn = armed_button(
                SharedString::from(format!("tb-remove-{id}")),
                "Remove",
                confirm,
                {
                    let state = state.clone();
                    move |_, cx| {
                        let id = id.clone();
                        state.update(cx, |s, cx| {
                            if workspace_can_remove(
                                s.workspace(&id),
                                s.workspaces_running.contains(&id),
                            ) {
                                s.remove_workspace(&id, cx).detach();
                            }
                        });
                    }
                },
            )
            .size(TbtnSize::TITLEBAR)
            .on_arm({
                let shell = cx.entity();
                move |_, cx| shell.update(cx, |this, cx| this.refresh_discard_risk(cx))
            });
            if !can {
                btn = btn
                    .disabled(Some(backdrop))
                    .title("Stop the workspace first");
            }
            right = right.child(no_drag(
                div()
                    .id("tb-remove-wrap")
                    .debug_selector(|| "tb-remove".into())
                    .child(btn),
            ));
        }
        right.into_any_element()
    }

    /// `.run-split` / `.stop-split`: Run the pinned command (click) with the picker on the
    /// chevron, and Stop while anything runs.
    fn run_splits(
        &mut self,
        ws: &recur_domain::Workspace,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let state = self.state.clone();
        let s = state.read(cx);
        let t = Theme::of(cx).tokens;
        let default = workspace_run(&ws.id, &s.workspaces, &s.projects);
        let run_label_text = run_label(
            "Run",
            default
                .as_ref()
                .map(|r| r.name.as_str())
                .filter(|n| !n.is_empty())
                .unwrap_or("dev"),
        );
        let running = s.run.running_runs(&ws.id);
        let stop_label = if running.len() == 1 {
            run_label("Stop", &running[0].0)
        } else {
            "Stop".to_string()
        };
        let run_open = s.overlays.run_menu;
        let stop_open = s.overlays.stop_menu;
        let id = ws.id.clone();
        let mut out = Vec::new();
        let chev_on = if run_open { Tbtn::On } else { Tbtn::Default };
        let run = div()
            .id("run-split")
            .relative()
            .flex()
            .items_center()
            .child(titled(
                "tb-run-tip",
                format!("{run_label_text} (click) · picker (⌘R)"),
                no_drag(
                    tbtn("tb-run", Tbtn::Default, TbtnSize::TITLEBAR, None, cx)
                        .rounded_r(px(0.))
                        .on_click({
                            let (state, id) = (state.clone(), id.clone());
                            move |_, _, cx| {
                                state.update(cx, |s, cx| {
                                    s.overlays.run_menu = false;
                                    s.request_run_dev(&id, None, cx);
                                })
                            }
                        })
                        .child(style::button_label(
                            run_label_text.clone(),
                            11.5,
                            weight::MEDIUM,
                        )),
                ),
            ))
            .child(no_drag(
                tbtn("tb-run-chev", chev_on, TbtnSize::TITLEBAR, None, cx)
                    .rounded_l(px(0.))
                    .border_l(px(0.))
                    .px(px(5.))
                    .on_click({
                        let (state, id) = (state.clone(), id.clone());
                        move |_, _, cx| {
                            state.update(cx, |s, cx| {
                                s.open_run_menu(&id, cx);
                            });
                        }
                    })
                    .child(icon(IconName::ChevronDown).size(px(13.))),
            ));
        out.push(run.into_any_element());
        let _ = t;
        if !running.is_empty() {
            let single = running.len() == 1;
            let stop_title = if single {
                format!("{stop_label} (⌘⇧R)")
            } else {
                "Stop (⌘⇧R)".to_string()
            };
            let first = running[0].0.clone();
            let mut stop = div()
                .id("stop-split")
                .relative()
                .flex()
                .items_center()
                .child(titled(
                    "tb-stop-tip",
                    stop_title,
                    no_drag(
                        tbtn("tb-stop", Tbtn::On, TbtnSize::TITLEBAR, None, cx)
                            .when(!single, |b| b.rounded_r(px(0.)))
                            .on_click({
                                let (state, id) = (state.clone(), id.clone());
                                move |_, _, cx| {
                                    state.update(cx, |s, cx| {
                                        if single {
                                            s.request_stop_run(&id, &first, cx);
                                        } else {
                                            s.open_stop_menu(&id, cx);
                                        }
                                    })
                                }
                            })
                            .child(style::button_label(
                                stop_label.clone(),
                                11.5,
                                weight::MEDIUM,
                            )),
                    ),
                ));
            if !single {
                stop = stop.child(no_drag(
                    tbtn("tb-stop-chev", Tbtn::On, TbtnSize::TITLEBAR, None, cx)
                        .rounded_l(px(0.))
                        .border_l(px(0.))
                        .px(px(5.))
                        .on_click({
                            let (state, id) = (state.clone(), id.clone());
                            move |_, _, cx| {
                                state.update(cx, |s, cx| {
                                    s.open_stop_menu(&id, cx);
                                })
                            }
                        })
                        .child(icon(IconName::ChevronDown).size(px(13.))),
                ));
            }
            let _ = stop_open;
            out.push(stop.into_any_element());
        }
        out
    }
}

/// `.tb-instance`: the dogfood `dev` chip with the branch.
fn instance_chip(label: &str, cx: &App) -> impl IntoElement {
    let t = Theme::of(cx).tokens;
    let border = t.warn.mix(0.45, t.line);
    let bg = t.warn.mix(0.18, t.titlebar);
    let k = t.warn.mix(0.7, t.muted);
    let title = format!("Dev instance {label}");
    titled(
        "tb-instance-tip",
        title,
        no_drag(
            div()
                .id("tb-instance")
                .flex_initial()
                .min_w_0()
                .max_w(px(280.))
                .py(px(2.))
                .px(px(8.))
                .border(px(1.))
                .border_color(Hsla::from(border))
                .rounded(px(5.))
                .bg(bg)
                .overflow_hidden()
                .child(
                    Text::from_spans(vec![
                        span("dev")
                            .size(9.)
                            .tracking(0.08)
                            .uppercase()
                            .color(k)
                            .boxed(InlineBox {
                                margin_right: 6.,
                                ..Default::default()
                            }),
                        span(label.to_string()),
                    ])
                    .mono()
                    .size(11.)
                    .color(t.warn)
                    .truncate(),
                ),
        ),
    )
}

/// InboxTitlebar `counts`: "N need you · M unread" or "all caught up".
fn inbox_counts(s: &AppState, cx: &App) -> impl IntoElement {
    let unread = s.inbox.iter().filter(|i| i.unread).count();
    let needs = s.briefing.counts.needs;
    let mut parts = Vec::new();
    if needs > 0 {
        parts.push(format!(
            "{needs} {} you",
            if needs == 1 { "needs" } else { "need" }
        ));
    }
    parts.push(if unread > 0 {
        format!("{unread} unread")
    } else {
        "all caught up".into()
    });
    tb_sub(parts.join(" · "), cx)
}

/// SchedulesTitlebar `counts`: active, paused and the next run.
fn schedule_counts(s: &AppState, cx: &App) -> impl IntoElement {
    let active: Vec<_> = s.schedules.iter().filter(|x| !x.paused).collect();
    let paused = s.schedules.len() - active.len();
    let next = active
        .iter()
        .map(|x| x.next_run_at)
        .filter(|n| *n > 0)
        .min();
    let mut parts = vec![format!("{} active", active.len())];
    if paused > 0 {
        parts.push(format!("{paused} paused"));
    }
    if let Some(ms) = next {
        parts.push(format!("next {}", format_medium_short(ms)));
    }
    tb_sub(parts.join(" · "), cx)
}

/// `Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' })` in en-US:
/// "Oct 21, 2026, 3:04 PM", in local time.
pub fn format_medium_short(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ms).single() {
        Some(d) => d.format("%b %-d, %Y, %-I:%M %p").to_string(),
        None => String::new(),
    }
}

/// `markAllInboxRead`.
fn mark_all_inbox_read(state: &Entity<AppState>, cx: &mut App) {
    let services = state.read(cx).services().clone();
    cx.background_spawn(async move {
        let _ = services.inbox.mark_all_inbox_read().await;
    })
    .detach();
    state.update(cx, |s, cx| {
        for i in s.inbox.iter_mut() {
            i.unread = false;
        }
        cx.notify();
    });
}
