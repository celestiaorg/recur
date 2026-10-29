//! `CompatibleProviderModal.svelte`: add an OpenAI-compatible endpoint from a preset.

use recur_ui::guard::Guarded;
use std::collections::HashMap;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, Context, DispatchPhase, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, canvas, div, px,
};
use recur_domain::{CompatibleModel, CompatibleProvider};
use recur_services::Services;
use recur_ui::fonts::weight;
use recur_ui::shadow::with_shadow;
use recur_ui::style::{MODAL_SHADOW, button_label, modal_panel, primary_button};
use recur_ui::text::{TextStyled, text};

use crate::field::{Field, FieldEvent, FieldOpts};
use crate::rules::{COMPATIBLE_PRESETS, CompatiblePreset, compatible_slug_error};
use crate::style::{InpStyle, field_msg, hsla, inp, tokens};

/// What the modal asks its host to do.
#[derive(Clone, Debug, PartialEq)]
pub enum ModalEvent {
    Close,
    /// `onAdd(provider)`.
    Add(CompatibleProvider),
}

/// `.titlebar`'s height: the part of the window the scrim leaves to the titlebar.
const TITLEBAR_H: f32 = 52.;

/// The modal's fields, in tab order.
const FIELDS: [(&str, &str, &str); 5] = [
    ("slug", "Provider slug", "openai"),
    ("name", "Provider display name", "OpenAI"),
    ("url", "Base URL", "https://api.openai.com"),
    ("model-id", "First model id", "model-id"),
    ("model-label", "First model label", "Label"),
];

/// A Tab stop: a preset button, a field, or the submit button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Preset(usize),
    Field(usize),
    Submit,
}

pub struct CompatibleProviderModal {
    focus: FocusHandle,
    taken: Vec<String>,
    services: Services,
    preset: &'static str,
    fields: HashMap<String, Field>,
    preset_focus: Vec<FocusHandle>,
    submit_focus: FocusHandle,
    /// The control Tab last moved to (`.kb-focus`), cleared by a pointer press.
    kb: Option<Stop>,
    error: String,
    saving: bool,
    hover_x: bool,
}

impl EventEmitter<ModalEvent> for CompatibleProviderModal {}

impl Focusable for CompatibleProviderModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl CompatibleProviderModal {
    pub fn new(
        taken: Vec<String>,
        services: Services,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut m = CompatibleProviderModal {
            focus: cx.focus_handle(),
            taken,
            services,
            preset: "openai",
            fields: HashMap::new(),
            preset_focus: COMPATIBLE_PRESETS
                .iter()
                .map(|_| cx.focus_handle().tab_stop(true))
                .collect(),
            submit_focus: cx.focus_handle().tab_stop(true),
            kb: None,
            error: String::new(),
            saving: false,
            hover_x: false,
        };
        for (k, _, _) in FIELDS {
            let f = Field::new(
                k.to_string(),
                FieldOpts::default(),
                Self::field_event,
                window,
                cx,
            );
            m.fields.insert(k.to_string(), f);
        }
        m.apply_values(&COMPATIBLE_PRESETS[0], "openai");
        m
    }

    fn apply_values(&mut self, p: &CompatiblePreset, slug: &str) {
        for (k, v) in [
            ("slug", slug),
            ("name", p.name),
            ("url", p.base_url),
            ("model-id", p.model_id),
            ("model-label", p.model_label),
        ] {
            if let Some(f) = self.fields.get_mut(k) {
                f.rewrite(v);
            }
        }
    }

    /// `applyPreset`.
    fn apply_preset(&mut self, id: &'static str, cx: &mut Context<Self>) {
        let p = COMPATIBLE_PRESETS
            .iter()
            .find(|p| p.id == id)
            .copied()
            .unwrap_or(COMPATIBLE_PRESETS[3]);
        self.preset = p.id;
        let slug = if p.id == "custom" { "" } else { p.id };
        self.apply_values(&p, slug);
        self.error.clear();
        cx.notify();
    }

    /// `slugEl.focus()` once the modal opens.
    pub fn initial_focus(&self, cx: &App) -> FocusHandle {
        self.handle(Stop::Field(0), cx)
    }

    fn value(&self, k: &str, cx: &App) -> String {
        self.fields.get(k).map(|f| f.value(cx)).unwrap_or_default()
    }

    /// `canSave`.
    fn can_save(&self, cx: &App) -> bool {
        !self.value("slug", cx).trim().is_empty()
            && !self.value("url", cx).trim().is_empty()
            && !self.value("model-id", cx).trim().is_empty()
            && !self.saving
    }

    fn field_event(&mut self, key: &str, ev: FieldEvent, _: &mut Window, cx: &mut Context<Self>) {
        if ev == FieldEvent::Focus
            && let Some(f) = self.fields.get_mut(key)
        {
            f.focused(cx);
        }
        cx.notify();
    }

    /// `submit`: validate the slug and the first model, fetch the preset's controls, add.
    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let id = self.value("slug", cx).trim().to_lowercase();
        let err = compatible_slug_error(&id, &self.taken);
        if !err.is_empty() {
            self.error = err;
            cx.notify();
            return;
        }
        let mid = self.value("model-id", cx).trim().to_string();
        let url = self.value("url", cx).trim().to_string();
        if url.is_empty() || mid.is_empty() {
            self.error = "Base URL and at least one model id are required.".into();
            cx.notify();
            return;
        }
        self.saving = true;
        self.error.clear();
        cx.notify();
        let name = self.value("name", cx).trim().to_string();
        let label = self.value("model-label", cx).trim().to_string();
        let preset_flags = COMPATIBLE_PRESETS
            .iter()
            .find(|p| p.id == self.preset)
            .map(|p| p.flags)
            .unwrap_or("");
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let flags = if preset_flags.is_empty() {
                Vec::new()
            } else {
                services
                    .settings
                    .compatible_flag_preset(preset_flags)
                    .await
                    .unwrap_or_default()
            };
            let p = CompatibleProvider {
                name: if name.is_empty() { id.clone() } else { name },
                id,
                base_url: url,
                models: vec![CompatibleModel {
                    label: if label.is_empty() { mid.clone() } else { label },
                    id: mid,
                    flags,
                    ..Default::default()
                }],
                ..Default::default()
            };
            let _ = this.update(cx, |this, cx| {
                cx.emit(ModalEvent::Add(p));
                this.saving = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn stops(&self, cx: &App) -> Vec<Stop> {
        let mut s: Vec<Stop> = (0..COMPATIBLE_PRESETS.len()).map(Stop::Preset).collect();
        s.extend((0..FIELDS.len()).map(Stop::Field));
        if self.can_save(cx) {
            s.push(Stop::Submit);
        }
        s
    }

    fn handle(&self, s: Stop, cx: &App) -> FocusHandle {
        match s {
            Stop::Preset(i) => self.preset_focus[i].clone(),
            Stop::Field(i) => self
                .fields
                .get(FIELDS[i].0)
                .map(|f| gpui_kit::Focusable::focus_handle(f.input.read(cx), cx))
                .unwrap_or_else(|| self.focus.clone()),
            Stop::Submit => self.submit_focus.clone(),
        }
    }

    /// `cycleDialogTab`.
    fn cycle(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let stops = self.stops(cx);
        let at = stops
            .iter()
            .position(|s| self.handle(*s, cx).is_focused(window));
        let next = match at {
            None if back => stops.len() - 1,
            None => 0,
            Some(0) if back => stops.len() - 1,
            Some(i) if back => i - 1,
            Some(i) if i + 1 == stops.len() => 0,
            Some(i) => i + 1,
        };
        let Some(s) = stops.get(next).copied() else {
            return;
        };
        self.kb = (s == Stop::Submit).then_some(s);
        window.focus(&self.handle(s, cx), cx);
        cx.notify();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let m = &k.modifiers;
        if k.key == "tab" && !m.platform && !m.control && !m.alt {
            cx.stop_propagation();
            self.cycle(m.shift, window, cx);
            return;
        }
        if k.key == "enter" && (m.platform || m.control) {
            cx.stop_propagation();
            self.submit(cx);
            return;
        }
        if k.key == "escape" {
            cx.stop_propagation();
            cx.emit(ModalEvent::Close);
        }
    }
}

impl CompatibleProviderModal {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = tokens(cx);
        let mut inputs: Vec<Entity<gpui_kit::base::input::InputState>> = Vec::new();
        for (k, _, _) in FIELDS {
            if let Some(f) = self.fields.get_mut(k) {
                f.flush(window, cx);
                recur_ui::style::style_text_input(&f.input, cx);
                inputs.push(f.input.clone());
            }
        }
        let lbl = |s: &str| {
            text(s.to_string())
                .size(13.)
                .leading(1.45)
                .color(t.text)
                .nowrap()
        };
        let row = |label: &str, i: usize, window: &Window, cx: &App| {
            let (_, aria, placeholder) = FIELDS[i];
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .min_w_0()
                .child(lbl(label))
                .child(
                    div()
                        .flex()
                        .debug_selector(move || format!("inp:{aria}"))
                        .child(inp(
                            &inputs[i],
                            &placeholder.into(),
                            InpStyle::BASE,
                            window,
                            cx,
                        )),
                )
        };
        let mut presets = div().flex().flex_wrap().gap(px(6.)).w_full();
        for (i, p) in COMPATIBLE_PRESETS.iter().enumerate() {
            let on = self.preset == p.id;
            let id = p.id;
            let name = p.name;
            let ring = recur_ui::style::is_focus_visible(&self.preset_focus[i], window);
            presets = presets.child(
                recur_ui::style::tbtn(
                    SharedString::from(format!("preset-{id}")),
                    if on {
                        recur_ui::style::Tbtn::On
                    } else {
                        recur_ui::style::Tbtn::Default
                    },
                    recur_ui::style::TbtnSize::BASE,
                    None,
                    cx,
                )
                .relative()
                .track_focus(&self.preset_focus[i])
                .debug_selector(move || format!("preset:{name}{}", if on { ":on" } else { "" }))
                .on_click(cx.guarded_listener(move |m, _, _, cx| m.apply_preset(id, cx)))
                .on_key_down(cx.guarded_listener(move |m, e: &KeyDownEvent, _, cx| {
                    if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        m.apply_preset(id, cx);
                    }
                }))
                .child(button_label(name, 12., weight::MEDIUM))
                .when(ring, |d| d.child(recur_ui::style::focus_ring(4., 1., cx))),
            );
        }
        let can_save = self.can_save(cx);
        let saving = self.saving;
        let mut go = primary_button("compat-go", (!can_save).then_some(t.pane), cx)
            .debug_selector(move || format!("nt-go:{}", if can_save { "on" } else { "off" }))
            .relative()
            .track_focus(&self.submit_focus);
        go = if saving {
            go.child(button_label("Adding…", 13., weight::SEMIBOLD))
        } else {
            go.child(button_label("Add provider", 13., weight::SEMIBOLD))
                .child(recur_ui::kbd::kbd("⌘↵").size(12.).opacity(0.55))
        };
        if can_save {
            go = go
                .on_click(cx.guarded_listener(|m, _, _, cx| m.submit(cx)))
                .on_key_down(cx.guarded_listener(|m, e: &KeyDownEvent, _, cx| {
                    if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        m.submit(cx);
                    }
                }));
        }
        if self.kb == Some(Stop::Submit) && self.submit_focus.is_focused(window) {
            go = go.child(recur_ui::style::outline(2., 2., 8., 0., t.amber));
        }
        let x = div()
            .id("compat-x")
            .debug_selector(|| "compat-x".into())
            .ml_auto()
            .flex_none()
            .cursor_pointer()
            .on_hover(cx.guarded_listener(|m, h: &bool, _, cx| {
                m.hover_x = *h;
                cx.notify();
            }))
            .on_click(cx.guarded_listener(|_, _, _, cx| cx.emit(ModalEvent::Close)))
            .child(recur_ui::style::nt_esc_hint(self.hover_x, window, cx));
        let head = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .pt(px(12.))
            .px(px(14.))
            .min_h(px(44.))
            .child(
                div().pt(px(2.)).px(px(2.)).child(
                    text("Add provider")
                        .size(14.)
                        .weight(weight::SEMIBOLD)
                        .leading(1.45)
                        .color(t.text),
                ),
            )
            .child(x);
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(10.))
            .px(px(16.))
            .pb(px(4.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .min_w_0()
                    .child(lbl("Preset"))
                    .child(presets),
            )
            .child(row("Slug", 0, window, cx))
            .child(row("Display name", 1, window, cx))
            .child(row("Base URL", 2, window, cx))
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .min_w_0()
                    .child(recur_ui::style::flex_share(
                        1.,
                        row("Model id", 3, window, cx).flex_1(),
                    ))
                    .child(recur_ui::style::flex_share(
                        1.,
                        row("Label", 4, window, cx).flex_1(),
                    )),
            );
        if !self.error.is_empty() {
            body = body.child(field_msg(
                "compat-err",
                &self.error.clone(),
                Some(false),
                cx,
            ));
        }
        let foot = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .pt(px(10.))
            .px(px(14.))
            .pb(px(14.))
            .min_w_0()
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
            .debug_selector(|| "compat-modal".into())
            .child(head)
            .child(body)
            .child(foot);
        let width = 440f32.min(f32::from(window.viewport_size().width) - 32.);
        let panel = with_shadow(panel, &MODAL_SHADOW, 16.)
            .id("compat-modal")
            .w(px(width))
            .occlude()
            // `clearKeyboardFocus(modal)` on pointer down.
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(|m, _, _, cx| {
                    m.kb = None;
                    cx.stop_propagation();
                    cx.notify();
                }),
            );
        // `.nt-bg` sits inside `.body`, so the titlebar paints over its top 52px; the panel
        // still hangs 22vh below the window top.
        let vh = f32::from(window.viewport_size().height);
        let drop = recur_ui::exact::lu(0.22 * vh) - TITLEBAR_H;
        div()
            .absolute()
            .top(px(TITLEBAR_H))
            .left_0()
            .right_0()
            .bottom_0()
            .bg(hsla(recur_ui::rgba(5, 7, 10, 0.55)))
            .flex()
            .flex_col()
            .items_center()
            .px(px(16.))
            .pb(px(24.))
            .child(recur_ui::exact::vgap(drop.max(0.), window))
            .child(div().flex_none().flex().flex_col().child(panel))
            .id("v-compat-provider")
            .debug_selector(|| "v-compat-provider".into())
            .track_focus(&self.focus)
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(|_, _, _, cx| cx.emit(ModalEvent::Close)),
            )
            .child(keys)
    }
}

impl Render for CompatibleProviderModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}
