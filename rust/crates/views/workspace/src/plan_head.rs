//! The plan pane's head (`planHead`): Details / Attachments / Todos tabs, and the plan's
//! Approve, Execute in chat and Run as loop actions.

use std::time::Duration;

use gpui_kit::{
    Context, Entity, Hsla, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, WeakEntity, Window, div, px,
};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::tab_chip::{ChipIcon, tab_chip};
use recur_ui::widgets::tab_strip::tab_strip;
use recur_ui::{IconName, Theme, motion};

use crate::plan_panel::{PlanPanel, PlanTab};
use crate::view::WorkspaceView;

pub struct PlanHead {
    view: WeakEntity<WorkspaceView>,
    panel: Entity<PlanPanel>,
}

fn hsla(c: recur_ui::Color) -> Hsla {
    c.into()
}

impl PlanHead {
    pub(crate) fn new(view: WeakEntity<WorkspaceView>, panel: Entity<PlanPanel>) -> Self {
        PlanHead { view, panel }
    }
}

impl Render for PlanHead {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        let Some(view) = self.view.upgrade() else {
            return div().into_any_element();
        };
        let (plan, has_run) = {
            let v = view.read(cx);
            (v.ws(cx).and_then(|w| w.plan.clone()), v.has_run(cx))
        };
        let (tab, editing) = {
            let p = self.panel.read(cx);
            (p.tab, p.editing)
        };
        let att_count = plan.as_ref().map_or(0, |p| p.attachments.len());
        let todos = self.panel.read(cx).todos_badge(cx);
        let mut strip = tab_strip("wsv-plan-tabs", "Plan")
            .flush(true)
            .active_key(SharedString::from(tab.key()));
        for (pt, label, ic, badge) in [
            (PlanTab::Details, "Details", IconName::Notebook, 0),
            (PlanTab::Attachments, "Attachments", IconName::Grid, att_count),
            (PlanTab::Todos, "Todos", IconName::ListTodo, todos),
        ] {
            let panel = self.panel.clone();
            strip = strip.tab(
                SharedString::from(pt.key()),
                tab_chip(SharedString::from(format!("plan-tab-{}", pt.key())), label)
                    .icon(ChipIcon::App(ic, None))
                    .badge(badge as u32)
                    .active(tab == pt)
                    .lit(true)
                    .on_click(move |_, _, cx| panel.update(cx, |p, cx| p.set_tab(pt, cx))),
            );
        }
        if !editing && let Some(plan) = plan {
            if !plan.approved {
                strip = strip.trailing(self.approve("wsv-plan-approve", "Approve", "Approve this plan", window, cx, |p, cx| {
                    p.approve_plan(cx)
                }));
            } else {
                // `.wsv-plan-act`: ghost text button.
                let panel = self.panel.clone();
                strip = strip.trailing(titled(
                    "wsv-plan-execute-t",
                    "Hand the plan to an Agent-mode chat",
                    div()
                        .id("wsv-plan-execute")
                        .debug_selector(|| "wsv-plan-execute".into())
                        .flex()
                        .flex_none()
                        .items_center()
                        .h(px(22.))
                        .px(px(8.))
                        .rounded(px(5.))
                        .text_color(hsla(t.muted))
                        .cursor_pointer()
                        .hover(move |s| s.text_color(hsla(t.text)).bg(hsla(t.pane_2)))
                        .child(text("Execute in chat").size(11.).weight(500.).line_height(LineHeight::Normal).nowrap())
                        .on_click(move |_, _, cx| panel.update(cx, |p, cx| p.execute_in_chat(cx))),
                ));
                if !has_run {
                    strip = strip.trailing(self.approve(
                        "wsv-plan-run",
                        "Run as loop",
                        "Engage the autopilot with this plan",
                        window,
                        cx,
                        |p, cx| p.run_as_loop(cx),
                    ));
                }
            }
        }
        div().flex_1().min_w_0().flex().items_stretch().child(strip).into_any_element()
    }
}

impl PlanHead {
    /// `.wsv-plan-approve`: the emerald action, lightening toward white on hover.
    fn approve(
        &self,
        id: &'static str,
        label: &'static str,
        title: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        act: fn(&mut PlanPanel, &mut Context<PlanPanel>),
    ) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        let h = motion::hover(SharedString::from(format!("{id}-hover")), Duration::ZERO, motion::Easing::EASE, window, cx);
        let hi = t.emerald.mix_oklab(0.88, recur_ui::hex(0xffffff));
        let fill = t.emerald.mix(1. - h.k(), hi);
        let panel = self.panel.clone();
        titled(
            SharedString::from(format!("{id}-t")),
            title,
            div()
                .id(id)
                .debug_selector(move || id.into())
                .flex()
                .flex_none()
                .items_center()
                .h(px(22.))
                .px(px(8.))
                .border_1()
                .border_color(hsla(fill))
                .rounded(px(5.))
                .bg(hsla(fill))
                .cursor_pointer()
                .on_hover(h.listener())
                .child(
                    text(label)
                        .size(11.)
                        .weight(550.)
                        .color(recur_ui::hex(0x0A0C10))
                        .line_height(LineHeight::Normal)
                        .nowrap(),
                )
                .on_click(move |_, _, cx| panel.update(cx, |p, cx| act(p, cx))),
        )
    }
}
