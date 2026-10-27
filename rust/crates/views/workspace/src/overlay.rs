//! The worktree's setup states: the `mainOverlay` snippet that frosts the editor while the
//! workspace is created (or failed to be), and `.wsv-strip`, the setup-failed bar above the
//! workbench once a tree exists.

use recur_ui::guard::Guarded;
use std::time::Instant;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Entity, Hsla, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_ui::fonts::Family;
use recur_ui::style::{Tbtn, TbtnSize, button_label, tbtn};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{IconName, Theme, icon, motion};
use recur_view_core::AppState;
use recur_view_core::call;

use crate::view::WorkspaceView;

fn hsla(c: recur_ui::Color) -> Hsla {
    c.into()
}

/// `.spin`: `rotate(360deg)` over 1.5s, linear, unless motion is reduced.
fn spinner(started: Instant, window: &mut Window, cx: &mut gpui_kit::App) -> impl IntoElement {
    let t = Theme::of(cx).tokens;
    let deg = if motion::reduced(cx) {
        0.
    } else {
        let e = motion::elapsed(started, cx).as_secs_f32();
        if !motion::is_frozen(cx) {
            window.request_animation_frame();
        }
        (e / 1.5).fract() * 360.
    };
    icon(IconName::Loader)
        .size(px(22.))
        .color(hsla(t.amber))
        .rotate(deg)
}

fn remove_workspace(state: &Entity<AppState>, id: &str, cx: &mut gpui_kit::App) {
    let id = id.to_string();
    state
        .update(cx, |s, cx| s.remove_workspace(&id, cx))
        .detach();
}

pub struct MainOverlay {
    state: Entity<AppState>,
    ws_id: String,
    started: Instant,
}

impl MainOverlay {
    pub(crate) fn new(state: Entity<AppState>, ws_id: String, cx: &gpui_kit::App) -> Self {
        MainOverlay {
            state,
            ws_id,
            started: recur_ui::motion::now(cx),
        }
    }
}

impl MainOverlay {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let Some(ws) = self.state.read(cx).workspace(&self.ws_id).cloned() else {
            return div().into_any_element();
        };
        let setup_failed = ws.last_error.starts_with("setup command failed");
        let copy = |title: &'static str, body: String| {
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .max_w(px(36. * 16.))
                .min_w_0()
                .child(
                    text(title)
                        .size(13.)
                        .weight(600.)
                        .color(t.text)
                        .line_height(LineHeight::Normal)
                        .align(recur_ui::text::Align::Center),
                )
                .child(
                    text(body)
                        .size(12.)
                        .line_height(LineHeight::Scale(1.45))
                        .color(t.faint)
                        .align(recur_ui::text::Align::Center),
                )
        };
        let mut root = div()
            .id("wsv-creating")
            .debug_selector(|| "wsv-creating".into())
            .flex_1()
            .w_full()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(14.))
            .py(px(32.))
            .px(px(24.))
            .text_color(hsla(t.muted));
        if !ws.last_error.is_empty() {
            let body = if setup_failed {
                "The project's setup command exited non-zero. Fix it in the project settings, then remove this workspace and create it again."
            } else {
                "Remove this workspace and create it again."
            };
            let (state, id, project) = (self.state.clone(), ws.id.clone(), ws.project_id.clone());
            let state2 = state.clone();
            let mut acts = div().flex().flex_wrap().gap(px(8.)).justify_center();
            if setup_failed {
                acts = acts.child(
                    tbtn("wsv-edit-setup", Tbtn::Default, TbtnSize::BASE, None, cx)
                        .child(button_label("Edit setup command", 12., 500.))
                        .on_click(move |_, _, cx| {
                            state.update(cx, |s, cx| s.open_project(&project, cx))
                        }),
                );
            }
            acts = acts.child(
                tbtn("wsv-remove-ws", Tbtn::Default, TbtnSize::BASE, None, cx)
                    .child(button_label("Remove workspace", 12., 500.))
                    .on_click(move |_, _, cx| remove_workspace(&state2, &id, cx)),
            );
            root = root
                .child(
                    icon(IconName::CircleAlert)
                        .size(px(22.))
                        .color(hsla(t.amber)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .max_w(px(36. * 16.))
                        .min_w_0()
                        .child(
                            text("Couldn’t create the workspace")
                                .size(13.)
                                .weight(600.)
                                .color(t.text)
                                .line_height(LineHeight::Normal)
                                .align(recur_ui::text::Align::Center),
                        )
                        .child(err_block(&ws.last_error, false, cx))
                        .child(
                            text(body)
                                .size(12.)
                                .line_height(LineHeight::Scale(1.45))
                                .color(t.faint)
                                .align(recur_ui::text::Align::Center),
                        )
                        .child(acts),
                );
        } else if ws.phase == "waiting for a slot" {
            root = root.child(spinner(self.started, window, cx)).child(copy(
                "Waiting for a loop slot…",
                "Every loop slot is busy; this starts when one frees.".into(),
            ));
        } else {
            root = root.child(spinner(self.started, window, cx)).child(copy(
                "Creating workspace…",
                "Setting up the worktree.".into(),
            ));
        }
        root.into_any_element()
    }
}

impl Render for MainOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// `.wsv-creating-err`: the error output, mono, wrapping (or one clipped line in the strip).
fn err_block(err: &str, clip: bool, cx: &gpui_kit::App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let body = text(err.to_string())
        .family(Family::Mono)
        .size(11.)
        .line_height(LineHeight::Scale(1.45))
        .color(t.text);
    if clip {
        return div()
            .flex_1()
            .min_w(px(8. * 16.))
            .overflow_hidden()
            .child(body.nowrap().truncate())
            .into_any_element();
    }
    div()
        .id("wsv-creating-err")
        .max_h(px(12. * 16.))
        .overflow_y_scroll()
        .child(body.pre_wrap().break_anywhere())
        .into_any_element()
}

/// `.wsv-strip` while a workspace with a tree failed to create (and has not engaged a run).
pub(crate) fn failed_strip(
    v: &mut WorkspaceView,
    cx: &mut Context<WorkspaceView>,
) -> Option<AnyElement> {
    let ws = v.ws(cx)?.clone();
    if !v.create_failed(cx) || v.has_run(cx) || ws.worktree_path.is_empty() {
        return None;
    }
    let t = Theme::of(cx).tokens;
    let setup_failed = v.setup_failed(cx);
    let creating = v.creating(cx);
    let err = ws.last_error.clone();
    let nl = err.find('\n');
    let multi = nl.is_some();
    let open = v.setup_err_open;
    let mut strip = div()
        .id("wsv-strip")
        .debug_selector(|| "wsv-strip".into())
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(10.))
        .py(px(6.))
        .px(px(14.))
        .border_b_1()
        .border_color(hsla(t.line))
        .text_size(px(11.5))
        .child(
            text(if setup_failed {
                "Setup Failed"
            } else {
                "Couldn't Create"
            })
            .size(11.5)
            .weight(700.)
            .color(t.amber)
            .line_height(LineHeight::Normal)
            .nowrap(),
        );
    if !err.is_empty() && (!multi || !open) {
        let line = nl.map_or(err.as_str(), |i| &err[..i]);
        strip = strip.child(err_block(line, true, cx));
    }
    if multi {
        strip = strip.child(
            tbtn("wsv-err-toggle", Tbtn::Default, TbtnSize::BASE, None, cx)
                .child(button_label(
                    if open { "Hide output" } else { "Show output" },
                    12.,
                    500.,
                ))
                .on_click(cx.guarded_listener(|v, _, _, cx| {
                    v.setup_err_open = !v.setup_err_open;
                    v.touch(cx);
                })),
        );
    }
    strip = strip.child(div().flex_1());
    let mut acts = div().flex().flex_wrap().gap(px(8.)).justify_end();
    if setup_failed {
        let id = ws.id.clone();
        acts = acts.child(
            tbtn(
                "wsv-retry-setup",
                Tbtn::Default,
                TbtnSize::BASE,
                creating.then_some(t.bg),
                cx,
            )
            .child(button_label("Retry setup", 12., 500.))
            .when(!creating, |b| {
                b.on_click(cx.guarded_listener(move |v, _, _, cx| {
                    let (svc, id) = (v.services.clone(), id.clone());
                    cx.spawn(async move |_, cx| {
                        call(
                            cx,
                            svc.workspaces.retry_workspace_setup(&id),
                            "Could not retry setup",
                        )
                        .await;
                    })
                    .detach();
                }))
            }),
        );
        let project = ws.project_id.clone();
        acts = acts.child(
            tbtn(
                "wsv-edit-setup-strip",
                Tbtn::Default,
                TbtnSize::BASE,
                None,
                cx,
            )
            .child(button_label("Edit setup command", 12., 500.))
            .on_click(cx.guarded_listener(move |v, _, _, cx| {
                v.state.update(cx, |s, cx| s.open_project(&project, cx))
            })),
        );
    }
    let id = ws.id.clone();
    acts = acts.child(
        tbtn("wsv-remove-strip", Tbtn::Default, TbtnSize::BASE, None, cx)
            .child(button_label("Remove", 12., 500.))
            .on_click(cx.guarded_listener(move |v, _, _, cx| remove_workspace(&v.state, &id, cx))),
    );
    strip = strip.child(acts);
    if !err.is_empty() && multi && open {
        strip = strip.child(div().w_full().child(err_block(&err, false, cx)));
    }
    Some(strip.into_any_element())
}
