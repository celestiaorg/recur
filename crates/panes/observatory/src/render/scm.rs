//! The commit composer under the review's uncommitted file list: the message box, the AI
//! generate button, and Commit (⌘↵).

use recur_ui::guard::Guarded;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_ui::text::{TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::{IconName, Theme, icon};
use recur_view_review::style::hsla;

use crate::obs::Observatory;

impl Observatory {
    /// `.obs-scm-foot`: the commit message composer.
    pub(crate) fn render_commit_foot(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let staged = self.staged_files.len();
        let dirty = self.working_dirty_count();
        let busy = self.commit_busy;
        let msg_busy = self.commit_msg_busy;
        let msg = self.inputs.commit_msg(cx);
        recur_ui::style::style_textarea(&self.inputs.commit, t.faint, cx);
        let gen_disabled = msg_busy || staged == 0;
        let gen_title = if msg_busy {
            "Generating…".to_string()
        } else if staged > 0 {
            "Generate commit message with AI".to_string()
        } else {
            "Check files to generate a message".to_string()
        };
        let pulse = (msg_busy && !recur_ui::motion::reduced(cx))
            .then(|| crate::render::pulse_opacity(self.anim_epoch, window, cx));
        let gen_color = if msg_busy { t.amber } else { t.faint };
        let gen_btn =
            titled(
                "scm-gen-t",
                gen_title,
                div()
                    .id("scm-gen")
                    .debug_selector(|| "scm-gen".into())
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(24.))
                    .rounded(px(5.))
                    .text_color(hsla(gen_color))
                    .when(gen_disabled && !msg_busy, |d| d.opacity(0.35))
                    .when_some(pulse, |d, k| d.opacity(k))
                    .when(!gen_disabled, |d| {
                        d.cursor_pointer()
                            .hover(move |s| s.text_color(hsla(t.text)).bg(hsla(t.pane_2)))
                            .on_click(cx.guarded_listener(|o, _, window, cx| {
                                o.generate_commit_msg(window, cx)
                            }))
                    })
                    .child(icon(IconName::Sparkle).size(px(13.))),
            );
        let can_commit = !busy && (staged > 0 || dirty > 0) && !msg.trim().is_empty();
        let files = |n: usize| format!("{n} file{}", if n == 1 { "" } else { "s" });
        let title = if staged > 0 {
            format!("Commit {} checked", files(staged))
        } else {
            format!("Commit all {}", files(dirty))
        };
        let label = if busy { "Committing…" } else { "Commit" };
        let go_bg = if can_commit {
            t.text
        } else {
            recur_ui::style::faded(t.text, t.text, 0.35, t.bg)
        };
        let go_fg = if can_commit {
            t.bg
        } else {
            recur_ui::style::faded(t.bg, t.text, 0.35, t.bg)
        };
        let go = titled(
            "scm-commit-t",
            title,
            div()
                .id("scm-commit")
                .debug_selector(|| "scm-commit".into())
                .ml_auto()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(5.))
                .h(px(24.))
                .px(px(8.))
                .rounded(px(6.))
                .bg(hsla(go_bg))
                .text_color(hsla(go_fg))
                .child(text(label).size(11.).weight(600.).nowrap())
                .child(
                    recur_ui::kbd::kbd("⌘↵")
                        .size(11.)
                        .weight(500.)
                        .opacity(0.55),
                )
                .when(can_commit, |d| {
                    d.cursor_pointer().on_click(
                        cx.guarded_listener(|o, _, window, cx| o.human_commit(window, cx)),
                    )
                }),
        );
        let field = div()
            .min_h(px(56.))
            .max_h(px(120.))
            .pt(px(8.))
            .px(px(10.))
            .pb(px(4.))
            .text_size(px(12.5))
            .line_height(gpui_kit::relative(1.5))
            .when(busy, |d| d.opacity(0.55))
            .child(self.inputs.commit_view.element(18.75, cx));
        let compose = recur_ui::style::card(t.bg, t.line, 10.)
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(field)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(2.))
                    .px(px(8.))
                    .pb(px(8.))
                    .child(gen_btn)
                    .child(go),
            );
        self.commit_fit
            .content(
                div()
                    .debug_selector(|| "obs-scm-foot".into())
                    .flex_none()
                    .pt(px(8.))
                    .px(px(10.))
                    .pb(px(10.))
                    .border_t_1()
                    .border_color(hsla(t.line))
                    .bg(hsla(t.pane))
                    .child(compose),
            )
            .into_any_element()
    }
}
