//! `#v-prdetail`: the header (`.prh`), the split with the agent dock (`.prd-agent`) beside
//! the tab column (`.prd-col`), the tab strip (`.prtabs`), and the panes.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Context, Div, Focusable, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement,
    Styled, Window, canvas, div, px,
};
use recur_domain::{Pr, PrSource};
use recur_engine::view::pr_workspace::pr_agent_is_running;
use recur_forge::view::{RollupKind, checks_rollup, pr_attention_labels};
use recur_ui::fonts::Family;
use recur_ui::guard::Guarded;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::{Color, IconName, Theme, icon};
use recur_view_core::pr::{PrLifecycle, pr_lifecycle};
use recur_view_review::scroll_fade::{SCROLL_FADE_SLACK, is_scrolled_from_top};

use super::PrDetail;
use crate::chrome::{chrome_color, status_label};
use crate::style::{dot, hsla, pulse_opacity, spin_deg};
use crate::ui::PrTab;
use recur_view_widgets::people::{people_stack, popout_for, pr_people};

/// `.prh { background: rgba(10, 12, 16, .85) }` (`rgba(247, 248, 250, .88)` on light themes)
/// over the view's `--bg`.
pub(crate) fn prh_bg(cx: &App) -> Color {
    let theme = Theme::of(cx);
    let t = theme.tokens;
    let over = if matches!(theme.id.key(), "paper" | "catppuccin-latte") {
        recur_ui::rgba(247, 248, 250, 0.88)
    } else {
        recur_ui::rgba(10, 12, 16, 0.85)
    };
    over.over(t.bg)
}

/// `.cmt-act`: a 22px faint icon button.
pub(crate) fn cmt_act(
    id: impl Into<SharedString>,
    name: IconName,
    open: bool,
    cx: &App,
) -> gpui_kit::Stateful<Div> {
    let t = Theme::of(cx).tokens;
    div()
        .id(gpui_kit::ElementId::Name(id.into()))
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(22.))
        .rounded(px(6.))
        .cursor_pointer()
        .text_color(hsla(if open { t.text } else { t.faint }))
        .when(open, |d| d.bg(hsla(t.pane_2)))
        .hover(move |s| s.text_color(hsla(t.text)).bg(hsla(t.pane_2)))
        .child(icon(name).size(px(13.)))
}

impl PrDetail {
    /// `.prh`.
    fn header(
        &mut self,
        pr: Option<&Pr>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let prh = div()
            .debug_selector(|| "prh".into())
            .relative()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(14.))
            .gap_y(px(10.))
            .py(px(12.))
            .px(px(18.))
            .bg(hsla(prh_bg(cx)))
            .border_b_1()
            .border_color(hsla(t.line));
        let Some(pr) = pr else {
            // Skeleton until the PR is in the store.
            let skel = |w: f32, h: f32, r: f32, window: &mut Window| {
                recur_ui::style::skeleton(window, cx)
                    .w(px(w))
                    .h(px(h))
                    .rounded(px(r))
                    .flex_none()
            };
            return prh
                .child(
                    div()
                        .flex_grow(1.)
                        .flex_shrink(1.)
                        .flex_basis(px(224.))
                        .min_w_0()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(10.))
                        .child(skel(88., 22., 99., window))
                        .child(skel(51.2, 16., 5., window))
                        .child(
                            recur_ui::style::skeleton(window, cx)
                                .flex_1()
                                .max_w(px(352.))
                                .h(px(18.)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(skel(72., 20., 99., window))
                        .child(skel(144., 20., 6., window)),
                )
                .into_any_element();
        };
        let life = pr_lifecycle(Some(pr));
        let updating = !pr.updating_base.is_empty();
        let gh = pr.source == PrSource::GitHub;
        // `.statepill`
        let pill = if updating {
            let o = pulse_opacity(1.6, window, cx);
            statepill(t.amber_d, t.amber, dot(7., t.amber).opacity(o), "Updating…")
        } else {
            let chrome = chrome_color(Some(pr), &t);
            statepill(
                chrome.fade(0.14),
                chrome,
                dot(7., chrome),
                &status_label(pr),
            )
        };
        let label = status_label(pr);
        let pill = pill.debug_selector(move || format!("statepill={label}"));
        // `.num`
        let src_title = if gh { "GitHub PR" } else { "Local PR" };
        let num = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .text_color(hsla(t.faint))
            .child(titled(
                "prh-src",
                src_title,
                div()
                    .id("prh-src-el")
                    .debug_selector(move || format!("prh-src={src_title}"))
                    .relative()
                    .top(px(-1.))
                    .flex()
                    .items_center()
                    .child(
                        icon(if gh {
                            IconName::Github
                        } else {
                            IconName::HardDrive
                        })
                        .size(px(18.)),
                    ),
            ))
            .child(
                text(format!("#{}", pr.number))
                    .mono()
                    .size(15.)
                    .weight(400.)
                    .tracking_px(-0.17)
                    .line_height(LineHeight::Scale(1.))
                    .nowrap(),
            );
        let mut prh_t = div()
            .debug_selector(|| "prh-t".into())
            .flex_grow(1.)
            .flex_shrink(1.)
            .flex_basis(px(224.))
            .min_w_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(10.))
            .text_size(px(17.))
            .font_weight(gpui_kit::FontWeight(600.))
            .child(pill)
            .child(num);
        if self.editing_title {
            let saving = self.saving == "title";
            let backdrop = prh_bg(cx);
            let lh = recur_ui::style::button_label("x", 11., 400.)
                .measure(None, window)
                .2
                .height;
            let empty = self.title_input.read(cx).value().trim().is_empty();
            let focused = self
                .title_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window);
            let border = if focused {
                t.amber.mix(0.55, t.line)
            } else {
                t.line
            };
            prh_t = prh_t
                .child(
                    div()
                        .debug_selector(|| "prh-title-input".into())
                        .flex_1()
                        .min_w_0()
                        .map(|d| {
                            super::field::press_focuses(
                                d,
                                self.title_input.read(cx).focus_handle(cx),
                            )
                        })
                        .key_context("TextEntry")
                        .on_key_down(cx.guarded_listener(|d, ev: &KeyDownEvent, _, cx| {
                            if ev.keystroke.key == "enter" {
                                cx.stop_propagation();
                                d.save_title(cx);
                            }
                        }))
                        .child(
                            recur_ui::style::card(t.bg, border, 8.)
                                .py(px(5.))
                                .px(px(10.))
                                .when(saving, |d| d.opacity(0.7))
                                .child(recur_ui::style::text_field(
                                    &self.title_input,
                                    &SharedString::from("Title"),
                                    t.faint,
                                    Family::Ui,
                                    16.,
                                    window,
                                    cx,
                                )),
                        ),
                )
                .child(
                    recur_view_review::style::note_btn(
                        "prh-title-cancel",
                        "Cancel",
                        false,
                        saving,
                        backdrop,
                        lh,
                        cx,
                    )
                    .debug_selector(|| "prh-title-cancel".into())
                    .when(!saving, |b| {
                        b.on_click(cx.guarded_listener(|d, _, _, cx| {
                            d.editing_title = false;
                            cx.notify();
                        }))
                    }),
                )
                .child(
                    recur_view_review::style::note_btn(
                        "prh-title-save",
                        if saving { "Saving…" } else { "Save" },
                        true,
                        empty || saving,
                        backdrop,
                        lh,
                        cx,
                    )
                    .debug_selector(|| "prh-title-save".into())
                    .when(!empty && !saving, |b| {
                        b.on_click(cx.guarded_listener(|d, _, _, cx| d.save_title(cx)))
                    }),
                );
        } else {
            let title = pr.title.clone();
            prh_t = prh_t.child(
                div()
                    .min_w_0()
                    .child(
                        text(title.clone())
                            .size(17.)
                            .weight(600.)
                            .tracking(-0.01)
                            .truncate()
                            .id("prh-title"),
                    )
                    .debug_selector(move || format!("prh-title={title}")),
            );
            if self.can_edit_pr(cx) {
                prh_t = prh_t.child(
                    div().ml(px(2.)).child(titled(
                        "prh-edit-title-t",
                        "Edit title",
                        cmt_act("prh-edit-title", IconName::Pencil, false, cx)
                            .debug_selector(|| "cmt-act:Edit title".into())
                            .on_click(cx.guarded_listener(|d, _, window, cx| {
                                d.start_edit_title(window, cx)
                            })),
                    )),
                );
            }
        }
        // `.prh-side`
        let people = pr_people(pr);
        let me = cx.entity().downgrade();
        let people_bounds = self.people_bounds.clone();
        let mut side = div()
            .debug_selector(|| "prh-side".into())
            .flex()
            .flex_initial()
            .max_w_full()
            .min_w_0()
            .items_center()
            .gap(px(8.))
            .text_color(hsla(t.muted))
            .child(
                div()
                    .relative()
                    .flex_none()
                    .child(people_stack(
                        "prh",
                        &people,
                        t.pane,
                        move |on, _, cx| {
                            let on = *on;
                            let _ = me.update(cx, |d, cx| {
                                if d.hovered_people != on {
                                    d.hovered_people = on;
                                    cx.notify();
                                }
                            });
                        },
                        cx,
                    ))
                    .child(
                        canvas(move |b, _, _| people_bounds.set(Some(b)), |_, _, _, _| {})
                            .absolute()
                            .inset_0(),
                    ),
            );
        if !matches!(life, Some(PrLifecycle::Merged | PrLifecycle::Closed)) {
            for line in pr_attention_labels(pr) {
                let l2 = line.clone();
                side = side.child(
                    pill2(None, &line, cx).debug_selector(move || format!("pr-attn-pill={l2}")),
                );
            }
        }
        let branch = format!("{} → {}", pr.branch, pr.base);
        side = side.child(titled(
            "prh-branch",
            branch.clone(),
            pill2(Some(IconName::GitLine), &branch, cx).id("prh-branch-el"),
        ));
        let head_bounds = self.head_bounds.clone();
        prh.child(prh_t)
            .child(side)
            .child(
                canvas(move |b, _, _| head_bounds.set(Some(b)), |_, _, _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .into_any_element()
    }

    /// `.prtabs`.
    fn tabs(&mut self, pr: Option<&Pr>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let tab = self.tab(cx);
        let agent_ws = self.agent_ws(cx);
        let dock_open = self.state().read(cx).pr_agent_dock_open;
        let focus_agent = self.ui.read(cx).focus_agent;
        let lit = agent_ws.is_empty() || !dock_open || !focus_agent;
        let mut bar = div()
            .debug_selector(|| "prtabs".into())
            .flex()
            .flex_none()
            .items_stretch()
            .gap(px(2.))
            .h(px(40.))
            .px(px(8.))
            .bg(hsla(t.pane))
            .border_b_1()
            .border_color(hsla(t.line));
        if !agent_ws.is_empty() && !dock_open {
            let running = {
                let s = self.state().read(cx);
                pr_agent_is_running(pr, &s.workspaces, &s.workspaces_running)
            };
            let (title, label) = if running {
                (
                    "Show left panel — agent running (⌥⌘B)",
                    "Show left panel. Agent running",
                )
            } else {
                ("Show left panel (⌥⌘B)", "Show left panel")
            };
            let o = if running {
                pulse_opacity(2.4, window, cx)
            } else {
                1.
            };
            let color = if running { t.amber } else { t.faint };
            let hover = if running { t.amber } else { t.text };
            let ring = recur_ui::style::is_focus_visible(&self.toggle_focus, window);
            bar = bar.child(
                div().flex().items_center().child(titled(
                    "prd-chat-toggle-t",
                    title,
                    div()
                        .id("prd-chat-toggle")
                        .debug_selector(move || {
                            format!("button:{label}{}", if running { ":running" } else { "" })
                        })
                        .track_focus(&self.toggle_focus)
                        .relative()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(28.))
                        .rounded(px(5.))
                        .cursor_pointer()
                        .text_color(hsla(color))
                        .hover(move |s| s.text_color(hsla(hover)).bg(hsla(t.pane_2)))
                        .child(
                            div()
                                .opacity(o)
                                .child(icon(IconName::PanelLeft).size(px(16.))),
                        )
                        .when(ring, |d| d.child(recur_ui::style::focus_ring(4., 0., cx)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.guarded_listener(|d, _, window, cx| {
                                window.focus(&d.toggle_focus, cx);
                            }),
                        )
                        .on_click(
                            cx.guarded_listener(|d, _, window, cx| d.show_agent_dock(window, cx)),
                        ),
                )),
            );
        }
        let pending = (
            self.conv_pending(cx),
            self.commits_pending(cx),
            self.checks_pending(cx),
        );
        let items = self.render_items(cx);
        let desc = items.iter().any(|c| c.kind == "description");
        let timeline = items
            .iter()
            .filter(|c| recur_engine::view::comment_thread::is_timeline_root(Some(c)))
            .count();
        let conv_n = timeline + usize::from(desc);
        let commits_n = pr.and_then(|p| p.commits.as_ref()).map_or(0, Vec::len);
        let files_n = pr.map_or(0, |p| {
            let n = p.files.as_ref().map_or(0, Vec::len);
            if n > 0 { n as i64 } else { p.file_count }
        });
        let checks = pr.and_then(|p| p.checks.clone()).unwrap_or_default();
        let roll = checks_rollup(&checks, pr.map(|p| p.checks_ok), pr.map(|p| p.has_checks));
        let passed = checks.iter().filter(|c| c.passed).count();
        let (roll_color, roll_icon) = rollup_look(roll, &t);
        let chip = |label: String,
                    color: Option<Color>,
                    pending: bool,
                    window: &mut Window,
                    cx: &mut Context<Self>|
         -> AnyElement {
            if pending {
                return recur_ui::style::skeleton(window, cx)
                    .flex_none()
                    .w(px(16.))
                    .min_h(px(11.))
                    .h(px(11.))
                    .rounded(px(10.))
                    .into_any_element();
            }
            div()
                .flex_none()
                .px(px(6.))
                .rounded(px(10.))
                .bg(hsla(t.pane_2))
                .child(
                    text(label)
                        .mono()
                        .size(9.5)
                        .line_height(LineHeight::Px(15.))
                        .color(color.unwrap_or(t.faint))
                        .nowrap(),
                )
                .into_any_element()
        };
        let mk = |me: &mut Self,
                  key: PrTab,
                  name: &'static str,
                  lead: AnyElement,
                  count: AnyElement,
                  cx: &mut Context<Self>| {
            let on = tab == key;
            let fg = if on { t.text } else { t.muted };
            let border = if on && lit {
                t.amber
            } else if on {
                t.line
            } else {
                recur_ui::color::TRANSPARENT
            };
            let _ = me;
            div()
                .id(SharedString::from(format!("prtab-{name}")))
                .debug_selector(move || {
                    format!(
                        "prtab:{name}:{}{}",
                        if on { "on" } else { "off" },
                        if on && lit { ":lit" } else { "" }
                    )
                })
                .flex()
                .items_center()
                .gap(px(6.))
                .h_full()
                .px(px(10.))
                .border_b_2()
                .border_color(hsla(border))
                .cursor_pointer()
                .text_color(hsla(fg))
                .hover(move |s| s.text_color(hsla(t.text)))
                .child(lead)
                .child(recur_ui::style::button_label(name, 12., 500.).family(Family::Ui))
                .child(count)
                .on_click(cx.guarded_listener(move |d, _, _, cx| d.set_tab(key, cx)))
        };
        let conv_chip = chip(conv_n.to_string(), None, pending.0, window, cx);
        bar = bar.child(mk(
            self,
            PrTab::Conv,
            "Conversation",
            icon(IconName::Message).size(px(14.)).into_any_element(),
            conv_chip,
            cx,
        ));
        let commits_chip = chip(commits_n.to_string(), None, pending.1, window, cx);
        bar = bar.child(mk(
            self,
            PrTab::Commits,
            "Commits",
            icon(IconName::GitCommit).size(px(14.)).into_any_element(),
            commits_chip,
            cx,
        ));
        let files_chip = chip(files_n.to_string(), None, false, window, cx);
        bar = bar.child(mk(
            self,
            PrTab::Files,
            "Files changed",
            icon(IconName::FileDiff).size(px(14.)).into_any_element(),
            files_chip,
            cx,
        ));
        let (kind, color, name) = if pending.2 {
            ("none", t.faint, IconName::Clock)
        } else {
            (rollup_kind(roll), roll_color, roll_icon)
        };
        let deg = if !pending.2 && roll == RollupKind::Pending {
            spin_deg(window, cx)
        } else {
            0.
        };
        let lead = div()
            .debug_selector(move || format!("prtab-check={kind}"))
            .flex_none()
            .text_color(hsla(color))
            .child(icon(name).size(px(14.)).rotate(deg))
            .into_any_element();
        let checks_color = if roll == RollupKind::None {
            None
        } else {
            Some(roll_color)
        };
        let checks_chip = chip(
            format!("{passed}/{}", checks.len()),
            checks_color,
            pending.2,
            window,
            cx,
        );
        bar = bar.child(mk(self, PrTab::Checks, "Checks", lead, checks_chip, cx));
        bar.into_any_element()
    }

    /// `showPRAgentDock`: open the dock and focus its hide toggle.
    fn show_agent_dock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state().update(cx, |s, cx| {
            s.pr_agent_dock_open = true;
            cx.notify();
        });
        window.defer(cx, |window, cx| {
            if let Some(f) = dock_toggle_focus(cx) {
                window.focus(&f, cx);
            }
        });
    }

    /// A tab's scroll pane (`.prpane` with `fadeScrollTop`).
    pub(crate) fn pane(
        &self,
        id: &'static str,
        scroll: &ScrollHandle,
        content: impl IntoElement,
        cx: &App,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let top = -f32::from(scroll.offset().y);
        let fade = is_scrolled_from_top(top, SCROLL_FADE_SLACK);
        let area = recur_ui::widgets::scroll_area(SharedString::from(format!("prpane-{id}")))
            .track(scroll)
            .child(content);
        div()
            .debug_selector(move || format!("prpane:{id}{}", if fade { ":fade-top" } else { "" }))
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .bg(hsla(t.bg))
            .child(area)
            .when(fade, |d| d.child(top_fade(cx)))
            .into_any_element()
    }
}

/// `.fade-top`'s mask: the top 40px dissolve into `--bg`.
pub(crate) fn top_fade(cx: &App) -> Div {
    let t = Theme::of(cx).tokens;
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(px(40.))
        .child(recur_ui::style::gradient_fill(
            180.,
            (t.bg, 0.),
            (t.bg.fade(0.), 1.),
        ))
}

fn statepill(bg: Color, fg: Color, d: Div, label: &str) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .py(px(4.))
        .px(px(11.))
        .rounded(px(20.))
        .bg(hsla(bg))
        .child(d)
        // `.prh-t`'s `letter-spacing: -.01em` (of 17px) inherits as an absolute length.
        .child(
            text(label.to_string())
                .mono()
                .size(11.)
                .weight(500.)
                .tracking_px(-0.17)
                .color(fg)
                .nowrap(),
        )
}

/// `.prh-side .pill2`.
fn pill2(ic: Option<IconName>, label: &str, cx: &App) -> Div {
    let t = Theme::of(cx).tokens;
    recur_ui::style::card(t.pane, t.line, 6.)
        .flex()
        .items_center()
        .gap(px(6.))
        .min_w_0()
        .py(px(3.))
        .px(px(9.))
        .text_color(hsla(t.text))
        .when_some(ic, |d, i| {
            d.child(div().flex_none().child(icon(i).size(px(11.))))
        })
        .child(
            div()
                .min_w_0()
                .child(text(label.to_string()).mono().size(10.5).truncate()),
        )
}

pub(crate) fn rollup_look(r: RollupKind, t: &recur_ui::Tokens) -> (Color, IconName) {
    match r {
        RollupKind::Fail => (t.coral, IconName::X),
        RollupKind::Pending => (t.amber, IconName::Loader),
        RollupKind::Pass => (t.emerald, IconName::Check),
        RollupKind::None => (t.faint, IconName::Check),
    }
}

pub(crate) fn rollup_kind(r: RollupKind) -> &'static str {
    match r {
        RollupKind::Fail => "fail",
        RollupKind::Pending => "pending",
        RollupKind::Pass => "pass",
        RollupKind::None => "none",
    }
}

/// The agent dock's hide toggle, once the dock has rendered.
pub(crate) fn dock_toggle_focus(cx: &App) -> Option<gpui_kit::FocusHandle> {
    let (dock, host) = recur_view_core::pr_dock::dock(cx)?;
    (host.toggle_focus)(&dock, cx)
}

impl PrDetail {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let pr = self.render_pr(cx);
        // `prDetailFocus`: the dock holds focus.
        let agent = self.dock_focus.contains_focused(window, cx);
        if agent != self.ui.read(cx).focus_agent {
            self.ui.update(cx, |u, cx| {
                u.focus_agent = agent;
                cx.notify();
            });
        }
        // `setPRAgentDockOpen`'s focus restore: the toggle that hides the dock hands focus to
        // the one that shows it, and back.
        let open = !self.agent_ws(cx).is_empty() && self.state().read(cx).pr_agent_dock_open;
        if open != self.dock_was_open {
            if !open && self.focus_in_dock {
                window.focus(&self.toggle_focus, cx);
            }
            if open && self.focus_on_toggle {
                window.defer(cx, |window, cx| {
                    if let Some(f) = dock_toggle_focus(cx) {
                        window.focus(&f, cx);
                    }
                });
            }
            self.dock_was_open = open;
        }
        self.focus_in_dock = open && self.dock_focus.contains_focused(window, cx);
        self.focus_on_toggle = self.toggle_focus.is_focused(window);
        // Files changed focuses its stack unless a field has focus.
        let tab_now = self.tab(cx);
        if self.shown_tab != Some(tab_now) {
            self.shown_tab = Some(tab_now);
            if tab_now == PrTab::Files {
                let focus = self.stack.focus.clone();
                window.defer(cx, move |window, cx| {
                    let typing = window.focused(cx).is_some_and(|_| {
                        window
                            .context_stack()
                            .iter()
                            .any(|c| c.contains("TextEntry") || c.contains("Input"))
                    });
                    if !typing {
                        window.focus(&focus, cx);
                    }
                });
            }
        }
        let header = self.header(pr.as_deref(), window, cx);
        let tabs = self.tabs(pr.as_deref(), window, cx);
        let tab = self.tab(cx);
        let body = match tab {
            PrTab::Conv => self.conversation(pr.as_deref(), window, cx),
            PrTab::Commits => self.commits_pane(pr.as_deref(), window, cx),
            PrTab::Files => self.files_pane(pr.as_deref(), window, cx),
            PrTab::Checks => self.checks_pane(pr.as_deref(), window, cx),
        };
        let mut split = div()
            .debug_selector(|| "prd-split".into())
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_row()
            .overflow_hidden();
        let agent_ws = self.agent_ws(cx);
        if !agent_ws.is_empty()
            && self.state().read(cx).pr_agent_dock_open
            && let Some(dock) = self.agent_dock(window, cx)
        {
            split = split.child(
                div()
                    .id("prd-agent")
                    .debug_selector(|| "prd-agent".into())
                    .track_focus(&self.dock_focus)
                    .flex_none()
                    .min_w(px(260.))
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .bg(hsla(t.bg))
                    .child(dock),
            );
        }
        let col = div()
            .id("prd-col")
            .debug_selector(|| "prd-col".into())
            .track_focus(&self.col_focus)
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(tabs)
            .child(recur_ui::layout_root::layout_root(
                "prd-body",
                gpui_kit::StyleRefinement::default()
                    .flex_1()
                    .min_h_0()
                    .min_w_0(),
                div().size_full().flex().flex_col().child(body),
            ))
            .children(self.more_menu(window, cx));
        split = split.child(col);
        let pop = if self.hovered_people
            && let (Some(p), Some(b), Some(h)) = (
                pr.as_deref(),
                self.people_bounds.get(),
                self.head_bounds.get(),
            ) {
            let people = pr_people(p);
            (!people.is_empty()).then(|| {
                let left = f32::from(b.origin.x) - f32::from(h.origin.x);
                let top = f32::from(b.bottom()) - f32::from(h.origin.y) + 6.;
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .child(popout_for("prh", &people, cx))
            })
        } else {
            None
        };
        div()
            .id("v-prdetail")
            .debug_selector(|| "v-prdetail".into())
            .key_context("PrDetail")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(|d, _, window, cx| {
                    // `onPRChromeMouseDown`: park focus on the column so the tabs light up,
                    // unless the press started a text selection (which focuses its scope).
                    if !d.dock_focus.contains_focused(window, cx) && !window.default_prevented() {
                        window.focus(&d.col_focus, cx);
                    }
                }),
            )
            .child(header)
            .child(split)
            // The open line notes' text boxes: the card stack may be a cached view, so they
            // draw here, after it (see `recur_view_review::note_portal`).
            .children(self.stack.pane.portals())
            .children(self.commit_stack.pane.portals())
            .children(pop)
    }
}

impl Render for PrDetail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

impl Focusable for PrDetail {
    fn focus_handle(&self, _: &App) -> gpui_kit::FocusHandle {
        self.col_focus.clone()
    }
}

impl PrDetail {
    /// The PR's agent dock: the bound workspace view's Observatory renders its left aside here.
    fn agent_dock(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (dock, host) = recur_view_core::pr_dock::dock(cx)?;
        let (render, observe) = (host.render, host.observe);
        self.watch_dock(&dock, observe, cx);
        render(&dock, window, cx)
    }

    /// Re-render when the dock's Observatory changes (its elements render inside this view).
    fn watch_dock(
        &mut self,
        dock: &gpui_kit::AnyView,
        observe: recur_view_core::pr_dock::ObserveDock,
        cx: &mut Context<Self>,
    ) {
        let id = dock.entity_id();
        if self.watched == Some(id) {
            return;
        }
        self.watched = Some(id);
        let me = cx.entity().downgrade();
        let notify = Box::new(move |cx: &mut App| {
            let _ = me.update(cx, |_, cx| cx.notify());
        });
        self.watch = observe(dock, notify, cx);
    }

    /// `.cmt-more-menu`: Open in GitHub, Copy link, Quote reply, Edit, Delete.
    pub(crate) fn more_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.more.id.is_empty() {
            return None;
        }
        let trigger = self.more.trigger?;
        let t = Theme::of(cx).tokens;
        let pr = self.pr(cx);
        let target = self.more_target(cx);
        let locked = self.conversation_locked(cx);
        let can_edit = match &target {
            Some(c) if c.kind == "description" => self.can_edit_pr(cx),
            Some(c) => {
                recur_engine::view::comment_thread::can_edit_pr_comment(pr.as_ref(), Some(c))
            }
            None => false,
        };
        let can_delete = target.as_ref().is_some_and(|c| {
            recur_engine::view::comment_thread::can_delete_pr_comment(pr.as_ref(), Some(c))
        });
        let can_quote = !locked
            && target.as_ref().is_some_and(|c| {
                !recur_engine::view::comment_thread::comment_source(Some(c)).is_empty()
            });
        let link = self.more_permalink(cx);
        let vp = window.viewport_size();
        let (top, right) = recur_ui::logic::menu_space::more_menu_fixed_box(
            Some(f32::from(trigger.origin.y)),
            f32::from(trigger.bottom()),
            f32::from(trigger.right()),
            self.more.menu.map_or(0., |b| f32::from(b.size.height)),
            (f32::from(vp.width), f32::from(vp.height)),
        );
        let item = |id: &'static str,
                    name: IconName,
                    label: &'static str,
                    danger: bool,
                    action: super::MoreAction,
                    cx: &mut Context<Self>| {
            let fg = if danger { t.coral } else { t.text };
            let ic = if danger { t.coral } else { t.faint };
            div()
                .id(id)
                .debug_selector(move || format!("menuitem:{label}"))
                .flex()
                .items_center()
                .gap(px(8.))
                .w_full()
                .py(px(7.))
                .px(px(10.))
                .rounded(px(5.))
                .cursor_pointer()
                .hover(move |s| s.bg(hsla(t.pane_2)))
                .child(icon(name).size(px(13.)).color(hsla(ic)))
                .child(recur_ui::style::button_label(label, 12.5, 500.).color(fg))
                .on_click(
                    cx.guarded_listener(move |d, _, window, cx| d.more_action(action, window, cx)),
                )
                .into_any_element()
        };
        let mut items = Vec::new();
        if !link.is_empty() {
            items.push(item(
                "pr-more-gh",
                IconName::Github,
                "Open in GitHub",
                false,
                super::MoreAction::OpenGitHub,
                cx,
            ));
            items.push(item(
                "pr-more-copy",
                IconName::Copy,
                "Copy link",
                false,
                super::MoreAction::CopyLink,
                cx,
            ));
        }
        if can_quote {
            items.push(item(
                "pr-more-quote",
                IconName::Quote,
                "Quote reply",
                false,
                super::MoreAction::Quote,
                cx,
            ));
        }
        if can_edit {
            items.push(item(
                "pr-more-edit",
                IconName::Pencil,
                "Edit",
                false,
                super::MoreAction::Edit,
                cx,
            ));
        }
        if can_delete {
            items.push(item(
                "pr-more-del",
                IconName::Trash,
                "Delete",
                true,
                super::MoreAction::Delete,
                cx,
            ));
        }
        let me = cx.entity().downgrade();
        let probe = canvas(
            move |b, _, cx| {
                let _ = me.update(cx, |d, cx| {
                    if d.more.menu != Some(b) {
                        d.more.menu = Some(b);
                        cx.notify();
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let card = recur_ui::style::card(t.rail, t.line, 8.)
            .debug_selector(|| "cmt-more-menu".into())
            .min_w(px(168.))
            .p(px(4.))
            .flex()
            .flex_col()
            .children(items)
            .child(probe)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        let panel = recur_ui::shadow::with_shadow(
            card,
            &[recur_ui::shadow::css_box_shadow(
                0.,
                6.,
                18.,
                0.,
                recur_ui::rgba(0, 0, 0, 0.3),
            )],
            8.,
        );
        let left_of = f32::from(vp.width) - right;
        let w = self.more.menu.map_or(168., |b| f32::from(b.size.width));
        let me = cx.entity().downgrade();
        let listeners = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let m1 = me.clone();
                window.on_mouse_event(move |e: &gpui_kit::MouseDownEvent, phase, _, cx| {
                    if phase != gpui_kit::DispatchPhase::Capture {
                        return;
                    }
                    let _ = m1.update(cx, |d, cx| {
                        if d.more.menu.is_some_and(|m| m.contains(&e.position)) {
                            return;
                        }
                        if d.more
                            .triggers
                            .get(&d.more.id)
                            .is_some_and(|b| b.contains(&e.position))
                        {
                            return;
                        }
                        d.close_more(cx);
                    });
                });
                let m2 = me.clone();
                window.on_key_event(move |e: &KeyDownEvent, phase, _, cx| {
                    if phase != gpui_kit::DispatchPhase::Capture || e.keystroke.key != "escape" {
                        return;
                    }
                    let _ = m2.update(cx, |d, cx| {
                        if !d.more.id.is_empty() {
                            cx.stop_propagation();
                            d.close_more(cx);
                        }
                    });
                });
            },
        )
        .absolute()
        .size_0();
        Some(
            div()
                .child(listeners)
                .child(
                    gpui_kit::deferred(recur_ui::style::anchored_at(left_of - w, top, panel))
                        .with_priority(40),
                )
                .into_any_element(),
        )
    }
}
