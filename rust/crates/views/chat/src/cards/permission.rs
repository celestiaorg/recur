//! `PermissionCard.svelte`: one live "may I run this tool?" card, inline in a transcript
//! (`compact`) or as the floating corner panel.

use recur_ui::guard::Guarded;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_agent_tools::display::permission_tools::permission_tool_label;
use recur_domain::PermissionRequest;
use recur_ui::fonts::Family;
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::style::flex_share;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{IconName, Theme, icon, motion, rgba};
use recur_view_core::AppState;

use crate::style::{INK, hsla, label};

/// The floating card's "Open workspace" link.
#[derive(Clone, Debug, PartialEq)]
pub enum PermissionCardEvent {
    OpenWhere,
}

pub struct PermissionCard {
    pub request: PermissionRequest,
    pub compact: bool,
    pub where_label: String,
    pub queued: usize,
    show_detail: bool,
    state: Entity<AppState>,
    mounted: Instant,
}

impl EventEmitter<PermissionCardEvent> for PermissionCard {}

/// The engine summary minus the friendly label it leads with.
pub fn detail_line(req: &PermissionRequest) -> String {
    let label = permission_tool_label(&req.tool);
    let s = recur_engine::view::js::trim(&req.summary).to_string();
    if s.is_empty() || s == label || s == req.tool {
        return String::new();
    }
    if !req.tool.is_empty() && s.starts_with(&req.tool) {
        let rest = &s[req.tool.len()..];
        let rest = rest.strip_prefix(':').unwrap_or(rest);
        return recur_engine::view::js::trim(rest).to_string();
    }
    if let Some(r) = s.strip_prefix(&format!("{label}: ")) {
        return recur_engine::view::js::trim(r).to_string();
    }
    if s.starts_with(&format!("{label} (")) {
        return recur_engine::view::js::trim(&s[label.len() + 1..]).to_string();
    }
    s
}

/// `JSON.stringify(JSON.parse(input), null, 2)`, else the raw input.
pub fn args_json(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    match serde_json::from_str::<serde_json::Value>(input) {
        Ok(v) => crate::tools::js_pretty(&v),
        Err(_) => input.to_string(),
    }
}

impl PermissionCard {
    pub fn new(request: PermissionRequest, compact: bool, state: Entity<AppState>) -> Self {
        PermissionCard {
            request,
            compact,
            where_label: String::new(),
            queued: 0,
            show_detail: false,
            state,
            mounted: Instant::now(),
        }
    }

    pub fn set_request(&mut self, request: PermissionRequest, cx: &mut Context<Self>) {
        if request.id != self.request.id {
            self.show_detail = false;
            self.mounted = Instant::now();
        }
        self.request = request;
        cx.notify();
    }

    pub fn set_where(&mut self, label: String, queued: usize, cx: &mut Context<Self>) {
        self.where_label = label;
        self.queued = queued;
        cx.notify();
    }

    fn answer(&self, allow: bool, reason: &str, remember: bool, cx: &mut Context<Self>) {
        let id = self.request.id.clone();
        let reason = reason.to_string();
        self.state.update(cx, |s, cx| {
            s.answer_permission(&id, allow, &reason, remember, cx)
        });
    }
}

impl PermissionCard {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let compact = self.compact;
        let tool_label = permission_tool_label(&self.request.tool);
        let detail = detail_line(&self.request);
        let args = args_json(&self.request.input);
        // `toastin .16s ease-out` (inline) / `permin .2s` settle (float); transforms dropped.
        let (dur, ease) = if compact {
            (Duration::from_millis(160), motion::Easing::EASE_OUT)
        } else {
            (Duration::from_millis(200), motion::Easing::SETTLE)
        };
        let k = motion::eased(self.mounted, dur, ease, cx);
        if k < 1. && !motion::is_frozen(cx) {
            window.request_animation_frame();
        }
        let id = SharedString::from(format!("perm-{}", self.request.id));
        let ico = icon(IconName::TriangleAlert)
            .size(px(if compact { 13. } else { 14. }))
            .color(hsla(t.amber));
        let kicker = if compact {
            label("Permission needed", Family::Ui, 12.5, 600.).color(t.text)
        } else {
            label("Permission needed", Family::Mono, 10., 600.)
                .tracking(0.08)
                .uppercase()
                .color(t.muted)
        };
        let mut head_text = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_w_0()
            .flex_1()
            .child(kicker);
        if !compact && self.queued > 0 {
            head_text = head_text.child(
                div().ml_auto().child(
                    recur_ui::style::card(
                        t.amber_d,
                        t.amber.mix(0.28, recur_ui::color::TRANSPARENT),
                        999.,
                    )
                    .px(px(7.))
                    .py(px(2.))
                    .child(
                        label(format!("+{} waiting", self.queued), Family::Mono, 10., 400.)
                            .color(t.amber),
                    ),
                ),
            );
        }
        let head = div()
            .flex()
            .items_center()
            .gap(px(7.))
            .when(!compact, |d| d.pt(px(12.)).pr(px(14.)).pl(px(16.)))
            .child(ico)
            .child(head_text);
        let tool = if compact {
            text(tool_label.clone())
                .mono()
                .size(11.5)
                .weight(500.)
                .line_height(LineHeight::Scale(1.25))
                .color(t.amber)
        } else {
            text(tool_label.clone())
                .size(14.5)
                .weight(600.)
                .tracking(-0.01)
                .line_height(LineHeight::Scale(1.25))
                .color(t.text)
        };
        let mut main = div()
            .flex()
            .flex_col()
            .gap(px(5.))
            .when(!compact, |d| d.pt(px(10.)).pr(px(14.)).pl(px(16.)))
            .child(tool.id(SharedString::from(format!("{id}-tool"))));
        if !detail.is_empty() {
            let sum = text(detail)
                .id(SharedString::from(format!("{id}-sum")))
                .size(12.5)
                .line_height(LineHeight::Scale(1.45))
                .color(if compact { t.text } else { t.muted })
                .break_anywhere();
            main = main.child(if compact {
                div()
                    .id(SharedString::from(format!("{id}-sum-s")))
                    .child(sum)
            } else {
                let vh = f32::from(window.viewport_size().height);
                div()
                    .id(SharedString::from(format!("{id}-sum-s")))
                    .max_h(px(0.18 * vh))
                    .overflow_y_scroll()
                    .child(sum)
            });
        }
        if !compact && !self.where_label.is_empty() {
            main = main.child(
                div().flex().child(recur_ui::tooltip::titled(
                    SharedString::from(format!("{id}-where-tip")),
                    "Open workspace",
                    recur_ui::style::card(t.pane_2, t.line, 6.)
                        .id(SharedString::from(format!("{id}-where")))
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .mt(px(4.))
                        .pt(px(3.))
                        .pb(px(3.))
                        .pl(px(7.))
                        .pr(px(8.))
                        .max_w_full()
                        .cursor_pointer()
                        .text_color(hsla(t.muted))
                        .hover(|s| s.text_color(hsla(t.text)).border_color(hsla(t.line_bright)))
                        .child(
                            icon(IconName::GitPullRequest)
                                .size(px(12.))
                                .color(hsla(t.muted.fade(0.85))),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .child(text(self.where_label.clone()).size(11.).truncate()),
                        )
                        .child(
                            icon(IconName::ChevronRight)
                                .size(px(11.))
                                .color(hsla(t.muted.fade(0.55))),
                        )
                        .on_click(cx.guarded_listener(|_, _, _, cx| {
                            cx.emit(PermissionCardEvent::OpenWhere)
                        })),
                )),
            );
        }
        let mut card = div().flex().flex_col();
        card = card.child(head).child(main);
        if !args.is_empty() {
            let open = self.show_detail;
            let toggle = div()
                .id(SharedString::from(format!("{id}-toggle")))
                .flex()
                .items_center()
                .gap(px(4.))
                .cursor_pointer()
                .text_color(hsla(t.faint))
                .hover(|s| s.text_color(hsla(t.text)))
                .child(
                    icon(IconName::ChevronRight)
                        .size(px(11.))
                        .rotate(if open { 90. } else { 0. })
                        .color(hsla(t.faint.fade(0.7))),
                )
                .child(
                    text(if open {
                        "Hide arguments"
                    } else {
                        "Show arguments"
                    })
                    .mono()
                    .size(10.5)
                    .line_height(LineHeight::Normal)
                    .nowrap(),
                )
                .on_click(cx.guarded_listener(|c, _, _, cx| {
                    c.show_detail = !c.show_detail;
                    cx.notify();
                }));
            let mut box_ = div()
                .flex()
                .flex_col()
                .when(compact, |d| d.mt(px(8.)))
                .when(!compact, |d| {
                    d.mt(px(10.))
                        .mr(px(14.))
                        .ml(px(16.))
                        .pt(px(8.))
                        .border_t_1()
                        .border_color(hsla(t.line_soft))
                })
                .child({
                    // `.perm-args`: the inline-flex toggle on a 12.5 / 1.45 line. Its first item,
                    // the centered 11px chevron, has no baseline: its bottom edge is the box's.
                    let (_, h) = crate::style::text_baseline(
                        text("x").mono().size(10.5).line_height(LineHeight::Normal),
                        window,
                    );
                    let toggle_h = h.max(11.);
                    crate::style::line_box(
                        toggle,
                        12.5,
                        1.45,
                        (toggle_h + 11.) / 2.,
                        toggle_h,
                        window,
                    )
                });
            if open {
                let vh = f32::from(window.viewport_size().height);
                box_ = box_.child(
                    recur_ui::style::card(t.bg, t.line, 7.)
                        .id(SharedString::from(format!("{id}-detail")))
                        .mt(px(6.))
                        .pt(px(8.))
                        .pb(px(8.))
                        .px(px(9.))
                        .max_h(px(0.22 * vh))
                        .overflow_y_scroll()
                        .child(
                            text(args)
                                .id(SharedString::from(format!("{id}-args")))
                                .mono()
                                .size(10.5)
                                .color(t.muted)
                                .pre_wrap()
                                .break_anywhere(),
                        ),
                );
            }
            card = card.child(box_);
        }
        let btn = |key: &str, s: &'static str, kind: u8, cx: &mut Context<Self>| {
            // kind: 0 deny, 1 once, 2 go.
            let (fg, border, bg) = match kind {
                0 => (
                    t.coral,
                    t.coral.mix(0.35, t.line),
                    recur_ui::color::TRANSPARENT,
                ),
                2 => (t.on_amber, t.amber, t.amber),
                _ => (t.text, t.line_bright, recur_ui::color::TRANSPARENT),
            };
            let pad_x = if kind == 0 {
                11.
            } else if compact {
                8.
            } else {
                9.
            };
            let pad_y = if compact { 6. } else { 7. };
            let amber = t.amber;
            let el = div()
                .id(SharedString::from(format!("{id}-{key}")))
                .flex()
                .items_center()
                .justify_center()
                .px(px(pad_x))
                .py(px(pad_y))
                .rounded(px(if compact { 6. } else { 7. }))
                .border_1()
                .border_color(hsla(border))
                .bg(hsla(bg))
                .cursor_pointer()
                .child(
                    recur_ui::style::button_label(s, 11.5, if kind == 2 { 600. } else { 400. })
                        .color(fg),
                );
            let el = match kind {
                0 => el.hover(move |s| {
                    s.bg(hsla(t.coral))
                        .border_color(hsla(t.coral))
                        .text_color(hsla(INK))
                }),
                2 => el.hover(move |s| {
                    s.bg(hsla(amber.brightness(1.07)))
                        .border_color(hsla(amber.brightness(1.07)))
                }),
                _ => el.hover(move |s| s.border_color(hsla(t.text)).bg(hsla(t.pane_2))),
            };
            el.on_click(cx.guarded_listener(move |c, _, _, cx| match kind {
                0 => c.answer(false, "denied by the human", false, cx),
                1 => c.answer(true, "", false, cx),
                _ => c.answer(true, "", true, cx),
            }))
        };
        let (deny, once, go) = (
            btn("deny", "Deny", 0, cx),
            btn("once", "Allow once", 1, cx),
            btn("go", "Allow for this turn", 2, cx),
        );
        let acts = if compact {
            div()
                .flex()
                .mt(px(11.))
                .gap(px(7.))
                .child(flex_share(1., deny.flex_1()))
                .child(flex_share(1., once.flex_1()))
                .child(flex_share(1., go.flex_1()))
        } else {
            // `grid-template-columns: auto 1fr 1.35fr`.
            div()
                .flex()
                .gap(px(6.))
                .mt(px(4.))
                .pt(px(12.))
                .pr(px(14.))
                .pb(px(13.))
                .pl(px(16.))
                .bg(hsla(t.bg.mix(0.55, recur_ui::color::TRANSPARENT)))
                .border_t_1()
                .border_color(hsla(t.line_soft))
                .child(deny.flex_none())
                .child(flex_share(1., once.flex_1()))
                .child(flex_share(1.35, go.flex_1()))
        };
        card = card.child(acts);
        if compact {
            let bg = t.amber.mix(0.06, t.pane);
            recur_ui::style::card(bg, t.amber.mix(0.45, t.line), 9.)
                .w_full()
                .max_w(px(36. * 16.))
                .pt(px(11.))
                .pb(px(11.))
                .px(px(12.))
                .text_size(px(12.5))
                .opacity(k)
                .child(card)
                .into_any_element()
        } else {
            let shadows = [
                css_box_shadow(0., 18., 48., -16., rgba(0, 0, 0, 0.72)),
                css_box_shadow(
                    0.,
                    0.,
                    0.,
                    1.,
                    t.amber.mix(0.18, recur_ui::color::TRANSPARENT),
                ),
            ];
            let bar = div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(3.))
                .child(recur_ui::style::gradient_fill(
                    180.,
                    (t.amber, 0.),
                    (t.amber.mix(0.35, recur_ui::color::TRANSPARENT), 1.),
                ));
            with_shadow(
                recur_ui::style::card(t.pane, t.line_bright, 12.)
                    .overflow_hidden()
                    .text_size(px(12.5))
                    .child(card)
                    .child(bar),
                &shadows,
                12.,
            )
            .opacity(k)
            .into_any_element()
        }
    }
}

impl Render for PermissionCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(tool: &str, summary: &str) -> PermissionRequest {
        PermissionRequest {
            tool: tool.into(),
            summary: summary.into(),
            ..Default::default()
        }
    }

    #[test]
    fn detail_line_peels_the_label() {
        assert_eq!(detail_line(&req("Bash", "Bash: rm -rf x")), "rm -rf x");
        assert_eq!(detail_line(&req("Bash", "Bash")), "");
        let label = permission_tool_label("mcp__recur__list_findings");
        assert_eq!(
            detail_line(&req("mcp__recur__list_findings", &format!("{label}: open"))),
            "open"
        );
        assert_eq!(detail_line(&req("Write", "edits a.go")), "edits a.go");
        assert_eq!(args_json("{\"a\":1}"), "{\n  \"a\": 1\n}");
        assert_eq!(args_json("not json"), "not json");
    }
}
