//! The PR review modal: Approve / Request changes with the pending line comments that post
//! with the review.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, AppContext as _, Context, DispatchPhase, Entity, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, canvas, div,
    px,
};
use recur_domain::PendingLineComment;
use recur_ui::exact::vgap;
use recur_ui::fonts::Family;
use recur_ui::fonts::weight;
use recur_ui::guard::Guarded;
use recur_ui::shadow::with_shadow;
use recur_ui::style::{self, MODAL_SHADOW, button_label, modal_panel, modal_scrim, primary_button};
use recur_ui::text::{TextStyled, strut_metrics, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::{Select, SelectEvent, SelectOption, ToastKind, TriggerStyle};
use recur_ui::{IconName, LineHeight, Theme, icon};
use recur_view_core::state::PrDiffJump;
use recur_view_core::{AppState, PrReviewKind, ViewContext, ViewName, call, toast};
use recur_view_core::{ObserveState as _, Topics};
use recur_view_review::comment_hunk::comment_hunk_path_label;
use recur_view_review::diff_file::remove_pending_comment;
use recur_view_widgets::suggest::{SuggestKind, SuggestStore};

use crate::detail::field::Field;
use crate::style::hsla;
use crate::ui::{PrTab, PrUi, ui};

fn kind_value(k: PrReviewKind) -> &'static str {
    match k {
        PrReviewKind::Approve => "approve",
        PrReviewKind::Request => "request",
    }
}

/// A Tab stop inside the dialog (`dialogFocusables`, in document order).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    Kind,
    Loc(usize),
    Del(usize),
    Body,
    Submit,
}

pub struct PrReviewModal {
    state: Entity<AppState>,
    ui: Entity<PrUi>,
    kind_select: Entity<Select>,
    body: Field,
    review_rev: u64,
    submitting: bool,
    /// The kind the modal last opened for (focus the body once per opening).
    shown: Option<PrReviewKind>,
    /// `.kb-focus`: the control last reached with Tab (only pickers and submit draw a ring).
    kb: Option<Stop>,
    hover_x: bool,
    hover_chip: Option<String>,
    submit_focus: FocusHandle,
    loc_focus: Vec<FocusHandle>,
    del_focus: Vec<FocusHandle>,
    _subs: Vec<Subscription>,
}

impl PrReviewModal {
    pub fn new(vcx: &ViewContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = vcx.state.clone();
        let ui = ui(cx);
        let t = Theme::of(cx).tokens;
        let options = vec![
            SelectOption::new("approve", "Approve").icon(IconName::Check),
            SelectOption::new("request", "Request changes").icon(IconName::OctagonX),
        ];
        // `.nt-pickers .sel-btn`.
        let kind_select = cx.new(|cx| {
            Select::new(options, window, cx)
                .label("Review type")
                .with_value("approve")
                .trigger_style(TriggerStyle {
                    height: Some(30.),
                    pad_x: Some(8.),
                    radius: Some(7.),
                    gap: Some(7.),
                    font_size: Some(13.),
                    ui_font: true,
                    bg: Some(t.pane.fade(0.)),
                    border: Some(t.pane.fade(0.)),
                    color: Some(t.text),
                    hover_bg: Some(t.pane_2),
                    hover_border: Some(t.pane_2.fade(0.)),
                    open_bg: Some(t.pane_2),
                    open_border: Some(t.pane_2.fade(0.)),
                    no_ring: true,
                    icon_color: Some(t.muted),
                    open_icon_color: Some(t.muted),
                    value_auto: true,
                    auto_width: true,
                    max_width: Some(240f32.min(f32::from(window.viewport_size().width) * 0.46)),
                    no_outline: true,
                    ..Default::default()
                })
        });
        let body = Field::new(
            &state,
            "Leave a comment (optional)…",
            (5, 5),
            |m: &mut Self, cx| {
                let v = m.body.value(cx);
                m.ui.update(cx, |u, _| u.review_draft = v);
            },
            window,
            cx,
        )
        .on_submit(|m: &mut Self, _, cx| m.submit(cx), cx);
        let subs = vec![
            cx.guarded_subscribe(&kind_select, |m, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(v) = ev {
                    let kind = match v.as_ref() {
                        "approve" => Some(PrReviewKind::Approve),
                        "request" => Some(PrReviewKind::Request),
                        _ => None,
                    };
                    if let Some(k) = kind {
                        m.state.update(cx, |s, cx| s.open_pr_review(k, cx));
                    }
                }
            }),
            cx.observe_state_in(&state, Topics::ALL, window, |m, _, window, cx| {
                m.state_changed(window, cx)
            }),
            cx.guarded_observe_in(&ui, window, |m, _, window, cx| m.ui_changed(window, cx)),
        ];
        PrReviewModal {
            state,
            ui,
            kind_select,
            body,
            review_rev: 0,
            submitting: false,
            shown: None,
            kb: None,
            hover_x: false,
            hover_chip: None,
            submit_focus: cx.focus_handle(),
            loc_focus: Vec::new(),
            del_focus: Vec::new(),
            _subs: subs,
        }
    }

    fn kind(&self, cx: &App) -> Option<PrReviewKind> {
        self.state.read(cx).overlays.pr_review
    }

    fn pending(&self, cx: &App) -> Vec<PendingLineComment> {
        let s = self.state.read(cx);
        s.pr(&s.selected_pr_id)
            .map(|p| p.pending_comments.clone())
            .unwrap_or_default()
    }

    fn can_submit(&self, cx: &App) -> bool {
        self.kind(cx) == Some(PrReviewKind::Approve)
            || !self.ui.read(cx).review_draft.trim().is_empty()
            || !self.pending(cx).is_empty()
    }

    fn state_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (kind, view) = {
            let s = self.state.read(cx);
            (s.overlays.pr_review, s.view)
        };
        // Leaving PR detail closes the modal.
        if kind.is_some() && view != ViewName::PrDetail {
            self.state.update(cx, |s, cx| s.close_pr_review(cx));
            return;
        }
        if let Some(k) = kind {
            let v = kind_value(k);
            if self.kind_select.read(cx).value().as_ref() != v {
                self.kind_select.update(cx, |s, cx| s.set_value(v, cx));
            }
            let placeholder = if k == PrReviewKind::Approve {
                "Leave a comment (optional)…"
            } else {
                "What should change?"
            };
            self.body
                .area
                .update(cx, |a, cx| a.set_placeholder(placeholder, window, cx));
        }
        if kind.is_none() {
            self.submitting = false;
            self.kb = None;
        } else if self.shown.is_none() {
            // `setTimeout(() => bodyEl?.focus(), 0)` on open.
            let area = self.body.area.clone();
            window.defer(cx, move |window, cx| {
                area.update(cx, |a, cx| a.focus(window, cx))
            });
        }
        self.shown = kind;
        let n = self.pending(cx).len();
        while self.loc_focus.len() < n {
            self.loc_focus.push(cx.focus_handle());
            self.del_focus.push(cx.focus_handle());
        }
        cx.notify();
    }

    fn ui_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (rev, draft) = {
            let u = self.ui.read(cx);
            (u.review_rev, u.review_draft.clone())
        };
        if rev != self.review_rev {
            self.review_rev = rev;
            if self.body.value(cx) != draft {
                self.body.set(draft, window, cx);
            }
        }
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.close_pr_review(cx));
    }

    fn jump_to_pending(&mut self, pc: &PendingLineComment, cx: &mut Context<Self>) {
        self.close(cx);
        self.ui.update(cx, |u, cx| u.set_tab(PrTab::Files, cx));
        let jump = PrDiffJump {
            file: pc.file.clone(),
            line: pc.line.max(0) as u32,
        };
        self.state.update(cx, |s, cx| {
            s.pr_diff_jump = Some(jump);
            cx.notify();
        });
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(kind) = self.kind(cx) else { return };
        let pr_id = self.state.read(cx).selected_pr_id.clone();
        if self.submitting || !self.can_submit(cx) || pr_id.is_empty() {
            return;
        }
        self.submitting = true;
        cx.notify();
        let approved = kind == PrReviewKind::Approve;
        let body = self.ui.read(cx).review_draft.clone();
        let services = self.state.read(cx).services().clone();
        let (state, ui) = (self.state.clone(), self.ui.clone());
        cx.spawn(async move |me, cx| {
            let fail = if approved {
                "Approve failed"
            } else {
                "Request changes failed"
            };
            let updated = call(
                cx,
                services.prs.submit_review(&pr_id, kind_value(kind), &body),
                fail,
            )
            .await;
            let _ = me.update(cx, |m, cx| {
                m.submitting = false;
                cx.notify();
            });
            let Some(updated) = updated else { return };
            state.update(cx, |s, cx| {
                s.update_pr(&updated.id.clone(), |p| *p = updated, cx)
            });
            ui.update(cx, |u, cx| u.set_review_draft(String::new(), cx));
            state.update(cx, |s, cx| s.close_pr_review(cx));
            let msg = if approved {
                "Review approved"
            } else {
                "Changes requested"
            };
            cx.update(|cx| toast(ToastKind::Success, msg, cx));
        })
        .detach();
    }

    /// `dialogFocusables(modal)`, in document order.
    fn stops(&self, cx: &App) -> Vec<Stop> {
        let mut out = vec![Stop::Kind];
        for i in 0..self.pending(cx).len() {
            out.push(Stop::Loc(i));
            out.push(Stop::Del(i));
        }
        out.push(Stop::Body);
        if !self.submitting && self.can_submit(cx) {
            out.push(Stop::Submit);
        }
        out
    }

    fn handle(&self, stop: Stop, cx: &App) -> FocusHandle {
        match stop {
            Stop::Kind => self.kind_select.read(cx).focus_handle(cx),
            Stop::Loc(i) => self.loc_focus[i].clone(),
            Stop::Del(i) => self.del_focus[i].clone(),
            Stop::Body => self.body.area.read(cx).focus_handle(cx),
            Stop::Submit => self.submit_focus.clone(),
        }
    }

    /// `cycleDialogTab`.
    fn cycle(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let stops = self.stops(cx);
        let at = stops
            .iter()
            .position(|s| self.handle(*s, cx).contains_focused(window, cx));
        let next = match at {
            None if back => stops.len() - 1,
            None => 0,
            Some(0) if back => stops.len() - 1,
            Some(i) if back => i - 1,
            Some(i) if i == stops.len() - 1 => 0,
            Some(i) => i + 1,
        };
        let stop = stops[next];
        // `focusKeyboard`: rings only on the picker and submit.
        self.kb = matches!(stop, Stop::Kind | Stop::Submit).then_some(stop);
        window.focus(&self.handle(stop, cx), cx);
        cx.notify();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let suggesting = {
            let store = SuggestStore::global(cx);
            let s = store.read(cx);
            s.slot(SuggestKind::Emoji).is_some() || s.slot(SuggestKind::Mention).is_some()
        };
        let k = &ev.keystroke;
        let m = &k.modifiers;
        if k.key == "tab" && !m.platform && !m.control && !m.alt {
            // The emoji / mention typeahead owns Tab.
            if suggesting {
                return;
            }
            cx.stop_propagation();
            self.cycle(m.shift, window, cx);
            return;
        }
        if k.key != "escape" || self.kind_select.read(cx).is_open() || suggesting {
            return;
        }
        cx.stop_propagation();
        self.close(cx);
    }
}

/// `.nt-x .esc`: an inline span in the close button, so its box is the 9.5px system font's
/// content area plus padding and border, set on the button's 11px Inter strut by baseline.
fn esc_hint(hovered: bool, window: &Window, cx: &App) -> gpui_kit::Div {
    let t = Theme::of(cx).tokens;
    let strut = strut_metrics(&Family::Ui, 11., LineHeight::Normal, window);
    let m = strut_metrics(&Family::System, 9.5, LineHeight::Normal, window);
    let content = m.ascent + m.descent;
    let top = strut.baseline - m.ascent - 3.;
    div().h(px(strut.height)).child(
        div()
            .relative()
            .top(px(top))
            .border(px(1.))
            .border_color(hsla(if hovered { t.line_bright } else { t.line }))
            .rounded(px(4.))
            .py(px(2.))
            .px(px(6.))
            .child(
                recur_ui::kbd::kbd("esc")
                    .size(9.5)
                    .line_height(LineHeight::Px(content))
                    .color(if hovered { t.muted } else { t.faint }),
            ),
    )
}

impl PrReviewModal {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let Some(kind) = self.kind(cx) else {
            return div().into_any_element();
        };
        let t = Theme::of(cx).tokens;
        recur_view_review::style::style_textarea(&self.body.area, cx);
        let approve = kind == PrReviewKind::Approve;
        let title = if approve {
            "Approve pull request"
        } else {
            "Request changes"
        };
        let pending = self.pending(cx);
        let can_submit = self.can_submit(cx);
        let kb_on = |s: Stop, window: &Window, cx: &App| {
            self.kb == Some(s) && self.handle(s, cx).is_focused(window)
        };

        // `.nt-head`.
        let kind_ring = kb_on(Stop::Kind, window, cx);
        let picker = div()
            .relative()
            .flex_none()
            .child(self.kind_select.clone())
            .when(kind_ring, |d| {
                d.child(style::outline(2., 2., 7., 1., t.amber))
            });
        let x = div()
            .id("pr-rev-x")
            .debug_selector(|| "pr-rev-x".into())
            .ml_auto()
            .flex_none()
            .cursor_pointer()
            .on_hover(cx.guarded_listener(|m, h: &bool, _, cx| {
                m.hover_x = *h;
                cx.notify();
            }))
            .on_click(cx.guarded_listener(|m, _, _, cx| m.close(cx)))
            .child(esc_hint(self.hover_x, window, cx));
        let head = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .pt(px(12.))
            .px(px(14.))
            .min_h(px(44.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .min_w_0()
                    .flex_1()
                    .child(picker),
            )
            .child(x);

        // `.nt-body`.
        let mut body = div().relative().flex().flex_col().min_w_0();
        if !pending.is_empty() {
            let mut list = div()
                .id("pr-rev-pending")
                .debug_selector(|| "pr-rev-pending".into())
                .pt(px(14.))
                .px(px(18.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .max_h(px(168.))
                .overflow_y_scroll();
            let strut = strut_metrics(&Family::Ui, 13., LineHeight::Scale(1.45), window);
            let loc_m = strut_metrics(&Family::Mono, 11., LineHeight::Normal, window);
            let above = strut.baseline.max(loc_m.baseline);
            let below = (strut.height - strut.baseline).max(loc_m.height - loc_m.baseline);
            for (i, pc) in pending.iter().enumerate() {
                let label = comment_hunk_path_label(&pc.file, pc.line, pc.start_line, false);
                let (loc_f, del_f) = (self.loc_focus[i].clone(), self.del_focus[i].clone());
                let shown = self.hover_chip.as_deref() == Some(pc.id.as_str())
                    || loc_f.is_focused(window)
                    || del_f.is_focused(window);
                let loc_ring = style::is_focus_visible(&loc_f, window);
                let del_ring = style::is_focus_visible(&del_f, window);
                let pj = pc.clone();
                let loc = div()
                    .id(SharedString::from(format!("pr-rev-loc-{}", pc.id)))
                    .debug_selector({
                        let l = label.clone();
                        move || format!("pr-rev-chip-loc={l}")
                    })
                    .track_focus(&loc_f)
                    .relative()
                    .max_w_full()
                    .cursor_pointer()
                    .group("pr-rev-loc")
                    .on_click(cx.guarded_listener(move |m, _, _, cx| m.jump_to_pending(&pj, cx)))
                    .child(
                        div().group_hover("pr-rev-loc", |s| s.underline()).child(
                            text(label)
                                .mono()
                                .size(11.)
                                .line_height(LineHeight::Normal)
                                .color(t.amber),
                        ),
                    )
                    .when(loc_ring, |d| d.child(style::focus_ring(4., 0., cx)));
                let loc = titled(
                    SharedString::from(format!("pr-rev-loc-t-{}", pc.id)),
                    "View in Files changed",
                    loc,
                );
                // `display: inline`: the button sits on the body's strut, baseline to baseline.
                let line = div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .child(vgap(above - loc_m.baseline, window))
                    .child(loc)
                    .child(vgap(below - (loc_m.height - loc_m.baseline), window));
                let main = div().flex_1().min_w_0().child(line).child(
                    div().mt(px(3.)).child(
                        text(pc.body.clone())
                            .size(12.5)
                            .line_height(LineHeight::Scale(1.45))
                            .color(t.text)
                            .pre_wrap()
                            .line_clamp(3),
                    ),
                );
                let pd = pc.clone();
                let state = self.state.clone();
                let del = div()
                    .id(SharedString::from(format!("pr-rev-del-{}", pc.id)))
                    .debug_selector({
                        let id = pc.id.clone();
                        move || format!("pr-rev-del:{id}")
                    })
                    .track_focus(&del_f)
                    .relative()
                    .flex_none()
                    .ml_auto()
                    .mt(px(1.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(22.))
                    .rounded(px(5.))
                    .text_color(hsla(t.faint))
                    .cursor_pointer()
                    .when(!shown, |d| d.opacity(0.))
                    .hover(move |s| s.text_color(hsla(t.coral)).bg(hsla(t.pane.fade(0.7))))
                    .on_click(move |_, _, cx| remove_pending_comment(&state, pd.clone(), cx))
                    .child(icon(IconName::Trash).size(px(13.)))
                    .when(del_ring, |d| d.child(style::focus_ring(4., 0., cx)));
                let del = titled(
                    SharedString::from(format!("pr-rev-del-t-{}", pc.id)),
                    "Remove pending comment",
                    del,
                );
                let id = pc.id.clone();
                list = list.child(
                    div()
                        .id(SharedString::from(format!("pr-rev-chip-{}", pc.id)))
                        .flex()
                        .items_start()
                        .gap(px(8.))
                        .py(px(8.))
                        .px(px(10.))
                        .bg(hsla(t.bg))
                        .rounded(px(10.))
                        .on_hover(cx.guarded_listener(move |m, h: &bool, _, cx| {
                            m.hover_chip = h.then(|| id.clone());
                            cx.notify();
                        }))
                        .child(main)
                        .child(del),
                );
            }
            body = body.child(list);
        }
        let area = div()
            .id("pr-rev-body")
            .debug_selector(|| "pr-rev-body".into())
            .min_h(px(148.))
            .pt(px(18.))
            .px(px(18.))
            .pb(px(12.))
            .text_size(px(14.))
            .line_height(px(14. * 1.55))
            .text_color(hsla(t.text))
            .map(|d| {
                crate::detail::field::press_focuses(d, self.body.area.read(cx).focus_handle(cx))
            })
            .key_context("TextEntry")
            .child(self.body.element(14. * 1.55, cx));
        body = body.child(area);

        // `.nt-foot`.
        let disabled = self.submitting || !can_submit;
        let label = if approve {
            "Approve"
        } else {
            "Request changes"
        };
        let mut go = primary_button("pr-rev-go", disabled.then_some(t.pane), cx)
            .debug_selector(move || {
                format!("pr-rev-go={label}{}", if disabled { ":off" } else { "" })
            })
            .relative()
            .track_focus(&self.submit_focus);
        go = if self.submitting {
            go.child(button_label("Submitting…", 13., weight::SEMIBOLD))
        } else {
            go.child(button_label(label, 13., weight::SEMIBOLD))
                .child(recur_ui::kbd::kbd("⌘↵").size(12.).opacity(0.55))
        };
        if !disabled {
            go = go.on_click(cx.guarded_listener(|m, _, _, cx| m.submit(cx)));
        }
        if kb_on(Stop::Submit, window, cx) {
            go = go.child(style::outline(2., 2., 8., 0., t.amber));
        }
        let n = pending.len();
        let foot = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .pt(px(10.))
            .px(px(14.))
            .pb(px(14.))
            .min_w_0()
            .when(n > 0, |d| {
                d.child(
                    text(format!(
                        "{n} line comment{} will post with this review",
                        if n == 1 { "" } else { "s" }
                    ))
                    .size(12.)
                    .color(t.muted)
                    .nowrap(),
                )
            })
            .child(go);

        let me = cx.entity().downgrade();
        let keys = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let me = me.clone();
                window.on_key_event(move |ev: &KeyDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let _ = me.update(cx, |m, cx| m.on_key(ev, window, cx));
                });
            },
        )
        .absolute()
        .size_0();
        let panel = modal_panel(cx)
            .w_full()
            .debug_selector(move || format!("pr-rev-modal={title}"))
            .child(head)
            .child(body)
            .child(foot);
        let width = 640f32.min(f32::from(window.viewport_size().width) - 32.);
        let panel = with_shadow(panel, &MODAL_SHADOW, 16.)
            .id("pr-rev-modal")
            .w(px(width))
            .occlude()
            // `clearKeyboardFocus(modal)` on pointer down.
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(|m, _: &MouseDownEvent, _, cx| {
                    m.kb = None;
                    cx.stop_propagation();
                    cx.notify();
                }),
            );
        modal_scrim(window, panel)
            .id("v-pr-review")
            .debug_selector(|| "v-pr-review".into())
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(|m, _, _, cx| m.close(cx)),
            )
            .child(keys)
            .into_any_element()
    }
}

impl Render for PrReviewModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}
