//! The composer toolbar (the `toolbar` snippet): the chat mode select, then the loop / policy
//! pickers (Loop mode, the MA tab), the model pickers, or their loading skeleton, and the
//! Simplify toggle. `.wsv-mode` and `.wsv-simplify` follow the `chat-in` container's width.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, Hsla, InteractiveElement, IntoElement,
    ParentElement, Render, StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window,
    div, px,
};
use recur_engine::view::loops::{enabled_loops, enabled_policies};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::select::{Select, SelectEvent, SelectOption, TriggerStyle};
use recur_ui::{IconName, Theme, icon, motion};
use recur_view_chat::pickers::{LoopPolicyEvent, LoopPolicyPickers, ModelPickers, PickerEvent};
use recur_view_core::modes::ChatMode;
use recur_view_core::prefs::{CodePanelKey, PanelKey, PanelSide};

use crate::view::WorkspaceView;

/// What the toolbar shows after the mode select.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pickers {
    Loop,
    Models,
    Pending,
    None,
}

pub struct DockToolbar {
    view: WeakEntity<WorkspaceView>,
    mode: Entity<Select>,
    loops: Entity<LoopPolicyPickers>,
    models: Entity<ModelPickers>,
    pickers: Pickers,
    simplify_on: bool,
    simplify_disabled: bool,
    /// `chat-in` content width ≤ 640px / ≤ 460px.
    narrow: (bool, bool),
    mode_key: Option<(Vec<ChatMode>, ChatMode, bool, bool)>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

fn hsla(c: recur_ui::Color) -> Hsla {
    c.into()
}

impl DockToolbar {
    pub(crate) fn new(view: WeakEntity<WorkspaceView>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mode = cx.new(|cx| Select::new(Vec::new(), window, cx).label("Chat mode").ghost().up());
        let loops = cx.new(|cx| LoopPolicyPickers::new(true, true, true, window, cx));
        let models = cx.new(|cx| ModelPickers::new(true, true, window, cx));
        let subs = vec![
            cx.subscribe(&mode, |t, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(v) = ev {
                    let m = ChatMode::from_key(v);
                    let _ = t.view.update(cx, |v, cx| v.on_chat_mode_change(m, cx).detach());
                }
            }),
            cx.subscribe(&loops, |t, _, ev: &LoopPolicyEvent, cx| {
                let _ = t.view.update(cx, |v, cx| {
                    let (l, p) = match ev {
                        LoopPolicyEvent::Loop(id) => (id.clone(), v.dock_policy_id(cx)),
                        LoopPolicyEvent::Policy(id) => (v.dock_loop_id(cx), id.clone()),
                    };
                    v.persist_loop_policy(&l, &p, cx).detach();
                });
            }),
            cx.subscribe(&models, |t, _, ev: &PickerEvent, cx| {
                let _ = t.view.update(cx, |v, cx| match ev {
                    PickerEvent::Model(r, f) => v.on_chat_model_change(r.clone(), f.clone(), cx),
                    PickerEvent::Flags(f) => v.on_chat_flags_change(f.clone(), cx),
                });
            }),
        ];
        DockToolbar {
            view,
            mode,
            loops,
            models,
            pickers: Pickers::None,
            simplify_on: false,
            simplify_disabled: false,
            narrow: (false, false),
            mode_key: None,
            focus: cx.focus_handle().tab_stop(true),
            _subs: subs,
        }
    }

    /// The props from the view: options, values, disabled states and the container width.
    pub(crate) fn sync(&mut self, v: &WorkspaceView, _: &mut Window, cx: &mut Context<Self>) {
        let t = Theme::of(cx).tokens;
        let busy = v.composer_busy(cx);
        let ma = v.active_code() == "MA";
        let (main, pr) = (v.is_main(cx), v.is_pr_host(cx));
        let (opts, value) = if ma {
            (ChatMode::options(false, false), ChatMode::Loop)
        } else {
            (ChatMode::options(main || pr, pr), v.chat_mode(cx))
        };
        // `@container chat-in`: the dock's content width, less `.chat-dock` and `.chat-in` box.
        let chat_in = {
            let s = v.state.read(cx);
            let side = s
                .panels
                .sides
                .get(&CodePanelKey::Maestro)
                .copied()
                .unwrap_or(PanelSide::Left);
            let w = s.panels.size(if side == PanelSide::Right {
                PanelKey::ObsSide
            } else {
                PanelKey::MaestroDock
            });
            (w - 1. - 20.).min(780.) - 2. - 24.
        };
        let narrow = (chat_in <= 640., chat_in <= 460.);
        let key = (opts.clone(), value, busy, narrow.1);
        if self.mode_key.as_ref() != Some(&key) {
            self.mode_key = Some(key);
            let options = opts
                .iter()
                .map(|m| {
                    SelectOption::new(m.key(), m.label())
                        .icon(m.icon())
                        .tint(m.tint(&t))
                        .hint(m.hint())
                })
                .collect();
            let style = if narrow.1 {
                TriggerStyle {
                    hide_value: true,
                    pad_x: Some(4.),
                    gap: Some(2.),
                    ..Default::default()
                }
            } else {
                TriggerStyle::default()
            };
            self.mode.update(cx, |s, cx| {
                s.set_options(options, cx);
                s.set_value(value.key(), cx);
                s.set_disabled(busy.then_some(t.pane), cx);
                s.set_trigger_style(style, cx);
            });
        }
        let pickers = if v.show_loop_pickers(cx) {
            Pickers::Loop
        } else if ma {
            Pickers::None
        } else if v.pickers_ready() {
            Pickers::Models
        } else {
            Pickers::Pending
        };
        match pickers {
            Pickers::Loop => {
                let settings = v.state.read(cx).settings.clone();
                let loops = enabled_loops(&v.catalog_loops, settings.as_ref());
                let policies = enabled_policies(&v.catalog_policies, settings.as_ref());
                let (l, p) = (v.dock_loop_id(cx), v.dock_policy_id(cx));
                let pinned = v.loop_pickers_pinned(cx);
                self.loops.update(cx, |lp, cx| lp.set(&l, &p, &loops, &policies, pinned, t.pane, cx));
            }
            Pickers::Models => {
                let (model, flags) = v.dock_model(cx);
                let models = v.catalog_models.clone();
                self.models
                    .update(cx, |mp, cx| mp.set(models, &model, &flags, busy.then_some(t.pane), cx));
            }
            Pickers::Pending | Pickers::None => {}
        }
        let simplify_on = v.simplify_on(cx);
        let simplify_disabled = busy || v.state.read(cx).settings.is_none();
        if (pickers, simplify_on, simplify_disabled, narrow)
            != (self.pickers, self.simplify_on, self.simplify_disabled, self.narrow)
        {
            self.pickers = pickers;
            self.simplify_on = simplify_on;
            self.simplify_disabled = simplify_disabled;
            self.narrow = narrow;
            cx.notify();
        }
    }
}

impl Render for DockToolbar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        let mut row = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .child(div().flex_none().child(self.mode.clone()));
        row = match self.pickers {
            Pickers::Loop => row.child(self.loops.clone()),
            Pickers::Models => row.child(self.models.clone()),
            // `.model-pickers-pending`: reserve the dock's space while the catalog loads.
            Pickers::Pending => row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(24.))
                    .px(px(6.))
                    .min_w(px(118.))
                    .child(div().h(px(10.)).w(px(72.)).rounded(px(4.)).bg(hsla(t.pane_2)))
                    .child(
                        div()
                            .h(px(10.))
                            .w(px(44.))
                            .rounded(px(4.))
                            .bg(hsla(t.pane_2.fade(0.75))),
                    ),
            ),
            Pickers::None => row,
        };
        // `.wsv-simplify { transition: color .15s ease, background .15s ease }`.
        let h = motion::hover(
            "wsv-simplify-hover",
            Duration::from_millis(150),
            motion::Easing::EASE,
            window,
            cx,
        );
        let (on, disabled) = (self.simplify_on, self.simplify_disabled);
        let k = if disabled { 0. } else { h.k() };
        let lit = if on { 1. } else { k };
        let mut fg = t.text.mix(lit, t.muted);
        let mut bg = t.pane_2.fade(lit);
        // `.wsv-simplify:disabled { opacity: .45 }` over the composer's `--pane`.
        if disabled {
            let fill = bg.over(t.pane);
            fg = recur_ui::style::faded(fg, fill, 0.45, t.pane);
            bg = recur_ui::style::faded(bg, bg, 0.45, t.pane);
        }
        let mut btn = div()
            .id("wsv-simplify")
            .debug_selector(|| "wsv-simplify".into())
            .track_focus(&self.focus)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .h(px(24.))
            .px(px(if self.narrow.0 { 4. } else { 6. }))
            .rounded(px(6.))
            .bg(hsla(bg))
            .text_color(hsla(fg))
            .child(icon(IconName::Feather).size(px(12.)))
            .when(!self.narrow.0, |d| {
                d.child(text("Simplify").size(12.).color(fg).line_height(LineHeight::Normal))
            });
        if recur_ui::style::is_focus_visible(&self.focus, window) {
            btn = btn.child(recur_ui::style::outline(2., 2., 6., 0., t.amber));
        }
        if !disabled {
            let w = self.view.clone();
            btn = btn
                .cursor_pointer()
                .on_hover(h.listener())
                .on_click(move |_, _, cx| {
                    let _ = w.update(cx, |v, cx| v.toggle_simplify(cx));
                });
        }
        row.child(titled(
            "wsv-simplify-t",
            if on { "Simplify on" } else { "Simplify off" },
            div().id("wsv-simplify-wrap").child(btn),
        ))
    }
}
