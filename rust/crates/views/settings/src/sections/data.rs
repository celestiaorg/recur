//! Data: the process log and clearing every project, workspace and pull request.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_ui::fonts::weight;
use recur_ui::style::button_label;
use recur_ui::text::{TextStyled, span, text};
use recur_ui::widgets::ToastKind;
use recur_ui::{IconName, icon};
use recur_view_core::{ViewName, call, toast};

use crate::style::{Row, RowsCtx, copy, ctrl, danger_btn, rows, tbtn_label, tokens};
use crate::view::SettingsView;

impl SettingsView {
    pub(crate) fn data_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = tokens(cx);
        let logs = if self.logs_path.is_empty() {
            "Process log for crashes and agent failures.".to_string()
        } else {
            self.logs_path.clone()
        };
        let clear_ctrl = if !self.confirming_clear {
            danger_btn("clear-all", false, false, cx)
                .debug_selector(|| "btn:Clear all data".into())
                .hover(move |s| s.border_color(Hsla::from(t.line_bright)))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.confirming_clear = true;
                    cx.notify();
                }))
                .child(icon(IconName::Trash).size(px(13.)))
                .child(button_label("Clear all data", 12., weight::MEDIUM))
                .into_any_element()
        } else {
            let clearing = self.clearing;
            div()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(8.))
                .child(
                    text("This can’t be undone.")
                        .size(12.)
                        .weight(weight::MEDIUM)
                        .leading(1.45)
                        .color(t.coral),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            danger_btn("clear-yes", true, clearing, cx)
                                .debug_selector(|| "btn:Yes, clear all".into())
                                .when(!clearing, |d| {
                                    d.on_click(cx.listener(|this, _, _, cx| this.clear_all(cx)))
                                })
                                .child(button_label(
                                    if clearing { "Clearing…" } else { "Yes, clear all" },
                                    12.,
                                    weight::MEDIUM,
                                )),
                        )
                        .child(
                            tbtn_label("clear-cancel", "Cancel", clearing, cx)
                                .debug_selector(|| "btn:Cancel".into())
                                .when(!clearing, |d| {
                                    d.on_click(cx.listener(|this, _, _, cx| {
                                        this.confirming_clear = false;
                                        cx.notify();
                                    }))
                                }),
                        ),
                )
                .into_any_element()
        };
        rows(
            RowsCtx {
                sides: false,
                in_group: false,
                more: false,
            },
            vec![
                Row::new()
                    .child(copy("logs", "Logs", Some(vec![span(logs)]), cx))
                    .child(ctrl().child(
                        tbtn_label("reveal-logs", "Reveal in Finder", false, cx)
                            .debug_selector(|| "btn:Reveal in Finder".into())
                            .on_click(cx.listener(|this, _, _, cx| this.reveal_logs(cx))),
                    )),
                Row::new()
                    .danger()
                    .child(copy(
                        "clear",
                        "Clear all data",
                        Some(vec![span(
                            "Deletes all projects, workspaces, and pull requests. Your settings are kept.",
                        )]),
                        cx,
                    ))
                    .child(ctrl().child(clear_ctrl)),
            ],
            window,
            cx,
        )
        .into_any_element()
    }

    /// The Data effect: where the log lives.
    pub(crate) fn load_logs_path(&mut self, cx: &mut Context<Self>) {
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let p = services.settings.logs_path().await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.logs_path = p;
                cx.notify();
            });
        })
        .detach();
    }

    fn reveal_logs(&mut self, cx: &mut Context<Self>) {
        let services = self.vcx.services.clone();
        cx.spawn(async move |_, cx| {
            call(cx, services.settings.reveal_logs(), "Could not reveal logs").await;
        })
        .detach();
    }

    /// `clearAll`: clear, reload the stores, then Get started.
    fn clear_all(&mut self, cx: &mut Context<Self>) {
        self.clearing = true;
        cx.notify();
        let services = self.vcx.services.clone();
        let state = self.vcx.state.clone();
        cx.spawn(async move |this, cx| {
            let ok = call(cx, services.settings.clear_all_data(), "Could not clear data")
                .await
                .is_some();
            if ok {
                let load = cx.update(|cx| state.update(cx, |s, cx| s.load_initial(cx)));
                let _ = load.await;
                let _ = this.update(cx, |this, cx| {
                    this.confirming_clear = false;
                    toast(ToastKind::Success, "All data cleared", cx);
                    state.update(cx, |s, cx| s.go(ViewName::Welcome, None, cx));
                });
            }
            let _ = this.update(cx, |this, cx| {
                this.clearing = false;
                cx.notify();
            });
        })
        .detach();
    }
}
