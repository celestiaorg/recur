//! `SettingsGroup.svelte` / `SettingsGroups.svelte`: settings subsection cards (`.set-subsec*`
//! in `_settings.scss`), with the optional override switch that collapses the body.

use crate::fonts::weight;
use crate::style::toggle;
use crate::text::{Span, Text, TextStyled, span, text, text_stack};
use crate::theme::Theme;
use gpui_kit::{
    AnyElement, App, ClickEvent, Div, ElementId, Hsla, InteractiveElement, IntoElement,
    ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px, relative,
};
use std::rc::Rc;

type OnOverride = Rc<dyn Fn(bool, &mut Window, &mut App)>;

struct Override {
    on: bool,
    disabled: bool,
    label: SharedString,
    on_change: Option<OnOverride>,
}

/// `<SettingsGroup title blurb override …>`.
#[derive(IntoElement)]
pub struct SettingsGroup {
    id: ElementId,
    title: SharedString,
    blurb: Vec<Span>,
    over: Option<Override>,
    nested: bool,
    children: Vec<AnyElement>,
}

pub fn settings_group(id: impl Into<ElementId>, title: impl Into<SharedString>) -> SettingsGroup {
    SettingsGroup {
        id: id.into(),
        title: title.into(),
        blurb: Vec::new(),
        over: None,
        nested: false,
        children: Vec::new(),
    }
}

/// `.set-subhead-blurb code`: mono 11px inside a blurb.
pub fn blurb_code(code: impl Into<SharedString>) -> Span {
    span(code).mono().size(11.)
}

impl SettingsGroup {
    /// Plain blurb text.
    pub fn blurb(mut self, blurb: impl Into<SharedString>) -> Self {
        self.blurb = vec![span(blurb)];
        self
    }

    /// Rich blurb (`blurbContent`): text runs, e.g. with [`blurb_code`].
    pub fn blurb_spans(mut self, spans: Vec<Span>) -> Self {
        self.blurb = spans;
        self
    }

    /// The header switch: the body shows only while `on`. `label` is the switch's accessible
    /// name (`overrideLabel`, default `Override <title>`).
    pub fn override_toggle(
        mut self,
        on: bool,
        disabled: bool,
        label: Option<SharedString>,
        on_change: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        let label = label.unwrap_or_else(|| format!("Override {}", self.title).into());
        self.over = Some(Override {
            on,
            disabled,
            label,
            on_change: Some(Rc::new(on_change)),
        });
        self
    }

    /// `.set-subsec-nested`: a group inside another group's body.
    pub fn nested(mut self) -> Self {
        self.nested = true;
        self
    }
}

impl ParentElement for SettingsGroup {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for SettingsGroup {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        let show_body = self.over.as_ref().is_none_or(|o| o.on) && !self.children.is_empty();
        let collapsed = self.over.as_ref().is_some_and(|o| !o.on);

        let title = text(self.title.clone())
            .mono()
            .size(12.)
            .weight(weight::SEMIBOLD_PLUS)
            .tracking(0.05)
            .uppercase()
            .color(t.muted);
        let mut blocks: Vec<(f32, Text)> = vec![(0., title)];
        if !self.blurb.is_empty() {
            blocks.push((
                3.,
                Text::from_spans(self.blurb)
                    .ui()
                    .size(12.)
                    .weight(weight::REGULAR)
                    .leading(1.4)
                    .color(t.faint),
            ));
        }
        let copy = text_stack(blocks);

        let (head_pad, head_bg, body_pad) = if self.nested {
            ((10., 12.), t.pane_2.fade(0.7), (2., 12., 6.))
        } else {
            ((12., 14.), t.pane_2.fade(0.55), (2., 14., 8.))
        };
        let mut head = div()
            .flex()
            .py(px(head_pad.0))
            .px(px(head_pad.1))
            .bg(head_bg)
            .when(!collapsed, |d| {
                d.border_b(px(1.)).border_color(Hsla::from(t.line_soft))
            });
        head = if let Some(o) = self.over {
            let on = o.on;
            let mut switch = toggle((self.id.clone(), "override"), on, o.disabled, cx)
                .role(gpui_kit::accesskit::Role::Switch)
                .aria_label(o.label.clone());
            if !o.disabled
                && let Some(cb) = o.on_change.clone()
            {
                switch = switch.on_click(move |_: &ClickEvent, w, cx| cb(!on, w, cx));
            }
            head.flex_row()
                .items_center()
                .gap(px(14.))
                .child(div().flex_1().min_w_0().child(copy))
                .child(div().flex_none().flex().items_center().child(switch))
        } else {
            head.flex_col().child(copy)
        };

        let bg = if self.nested {
            t.pane_2.mix(0.4, t.bg)
        } else {
            t.pane.mix(0.7, t.bg)
        };
        div()
            .id(self.id)
            .flex()
            .flex_col()
            .border(px(1.))
            .border_color(Hsla::from(t.line_soft))
            .rounded(px(if self.nested { 8. } else { 10. }))
            .bg(bg)
            .overflow_hidden()
            .child(head)
            .when(show_body, |d| {
                d.child(
                    div()
                        .pt(px(body_pad.0))
                        .px(px(body_pad.1))
                        .pb(px(body_pad.2))
                        .children(self.children),
                )
            })
    }
}

/// `<SettingsGroups>`: the column the groups sit in (gap 16, full width).
pub fn settings_groups() -> Div {
    div().flex().flex_col().gap(px(16.)).w(relative(1.))
}
