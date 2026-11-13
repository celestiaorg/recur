//! The composer's pickers: the model pickers (the base model Select plus a flags menu with one
//! pick per flag dimension that stays open) and the loop pickers (loop + policy Selects with
//! the policy help popover).

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement, IntoElement, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, canvas, deferred, div, px,
};
use recur_agent::catalog::HarnessChoice;
use recur_domain::{base_model_ref, compose_harness_ref, harness_of};
use recur_engine::view::loops::{LoopInfo, PolicyInfo, loop_select_options, policy_select_options};
use recur_services::api::FlagDimension;
use recur_ui::guard::Guarded;
use recur_ui::logic::menu_space::{MENU_GAP_PX, clamp_menu_left, menu_fixed_box, menu_prefer_up};
use recur_ui::style;
use recur_ui::text::{Span, Text, TextStyled, span};
use recur_ui::widgets::select::{Select, SelectEvent, SelectOption, TriggerStyle};
use recur_ui::{IconName, Theme, icon};
use recur_view_core::models::{
    CatalogModel, FlagMap, flags_for_model, model_options_for, normalize_flags, short_model_label,
};

use crate::style::hsla;

const SEP: char = '\u{1f}';
/// The Run in popup's width.
const HARNESS_POPUP_W: f32 = 248.;

/// `isBoolDim`: two options, `true` and `false`.
pub fn is_bool_dim(d: &FlagDimension) -> bool {
    d.options.len() == 2
        && d.options.iter().any(|o| o.value == "true")
        && d.options.iter().any(|o| o.value == "false")
}

/// `summaryToken`: a bool dimension shows its name when on and nothing when off; others show
/// the option label.
pub fn summary_token(d: &FlagDimension, v: Option<&str>) -> String {
    let Some(v) = v.filter(|v| !v.is_empty()) else {
        return String::new();
    };
    if is_bool_dim(d) {
        return if v == "true" {
            d.label.clone()
        } else {
            String::new()
        };
    }
    d.options
        .iter()
        .find(|o| o.value == v)
        .map_or_else(|| v.to_string(), |o| o.label.clone())
}

/// The closed trigger: tokens joined by ` · `, or `-`.
pub fn flags_summary(dims: &[FlagDimension], selected: &FlagMap) -> String {
    let toks: Vec<String> = dims
        .iter()
        .map(|d| summary_token(d, selected.get(&d.id).map(String::as_str)))
        .filter(|t| !t.is_empty())
        .collect();
    if toks.is_empty() {
        "-".into()
    } else {
        toks.join(" · ")
    }
}

/// What a picker reports.
#[derive(Clone, Debug, PartialEq)]
pub enum PickerEvent {
    /// `onModelChange(ref, flags)`: flags reset to the new model's defaults.
    Model(String, FlagMap),
    /// `onFlagsChange(flags)`.
    Flags(FlagMap),
}

/// `ModelFlagsSelect`.
pub struct ModelFlagsSelect {
    dims: Vec<FlagDimension>,
    selected: FlagMap,
    select: Entity<Select>,
    _sub: Subscription,
}

impl EventEmitter<FlagMap> for ModelFlagsSelect {}

fn flag_options(dims: &[FlagDimension]) -> Vec<SelectOption> {
    dims.iter()
        .flat_map(|d| {
            d.options.iter().map(move |o| {
                let label = if o.default {
                    format!("{} (default)", o.label)
                } else {
                    o.label.clone()
                };
                SelectOption::new(format!("{}{SEP}{}", d.id, o.value), label).group(d.label.clone())
            })
        })
        .collect()
}

fn flag_values(selected: &FlagMap) -> Vec<SharedString> {
    selected
        .iter()
        .map(|(k, v)| format!("{k}{SEP}{v}").into())
        .collect()
}

impl ModelFlagsSelect {
    pub fn new(
        label: &str,
        ghost: bool,
        up: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let select = cx.new(|cx| {
            let mut s = Select::new(Vec::new(), window, cx)
                .label(label.to_string())
                .multiple();
            if ghost {
                s = s.ghost();
            }
            if up {
                s = s.up();
            }
            s
        });
        let sub = cx.guarded_subscribe(&select, |this: &mut Self, _, ev: &SelectEvent, cx| {
            let SelectEvent::Values(vs) = ev else { return };
            let had = flag_values(&this.selected);
            let mut next = this.selected.clone();
            for v in vs.iter().filter(|v| !had.contains(v)) {
                if let Some((dim, val)) = v.split_once(SEP) {
                    next.insert(dim.to_string(), val.to_string());
                }
            }
            // A pick of the selected option keeps it (one selection per dimension).
            let values = flag_values(&next);
            this.select.update(cx, |s, cx| s.set_values(values, cx));
            if next != this.selected {
                cx.emit(next);
            }
        });
        ModelFlagsSelect {
            dims: Vec::new(),
            selected: FlagMap::new(),
            select,
            _sub: sub,
        }
    }

    pub fn set(
        &mut self,
        dims: Vec<FlagDimension>,
        value: &FlagMap,
        disabled_over: Option<recur_ui::Color>,
        cx: &mut Context<Self>,
    ) {
        let selected = normalize_flags(&dims, Some(value));
        let options = flag_options(&dims);
        let summary = flags_summary(&dims, &selected);
        let values = flag_values(&selected);
        self.select.update(cx, |s, cx| {
            s.set_options(options, cx);
            s.set_values(values, cx);
            s.set_display(Some(summary.into()), cx);
            s.set_disabled(disabled_over, cx);
        });
        // The dock's host pushes this from its render: only a change redraws.
        if self.dims != dims || self.selected != selected {
            self.dims = dims;
            self.selected = selected;
            cx.notify();
        }
    }
}

impl ModelFlagsSelect {
    fn render_view(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement + use<> {
        div().flex_none().child(self.select.clone())
    }
}

impl Render for ModelFlagsSelect {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// A model was picked and Recur is asking which harness should run it.
#[derive(Clone, Debug)]
struct PendingHarness {
    base: String,
    label: String,
    harnesses: Vec<HarnessChoice>,
}

fn harnesses_for(models: &[CatalogModel], base: &str) -> Vec<HarnessChoice> {
    models
        .iter()
        .find(|m| m.r#ref == base)
        .map(|m| m.harnesses.clone())
        .unwrap_or_default()
}

/// What picking a model does next.
#[derive(Debug, PartialEq, Eq)]
enum HarnessStep {
    /// Open the Run in popup: more than one harness can run the model, or one
    /// cannot because of the model source and the user should see why.
    Ask,
    /// Select the model in this harness ("" is the source's own) directly.
    Pick(String),
}

fn harness_step(harnesses: &[HarnessChoice]) -> HarnessStep {
    let eligible: Vec<&HarnessChoice> = harnesses.iter().filter(|h| !h.unavailable).collect();
    let explains = harnesses
        .iter()
        .any(|h| h.unavailable && !h.not_installed && !h.reason.is_empty());
    if eligible.len() > 1 || (explains && !eligible.is_empty()) {
        return HarnessStep::Ask;
    }
    HarnessStep::Pick(eligible.first().map(|h| h.id.clone()).unwrap_or_default())
}

fn harness_label(harnesses: &[HarnessChoice], id: &str) -> String {
    harnesses
        .iter()
        .find(|h| h.id == id)
        .map(|h| h.label.clone())
        .unwrap_or_else(|| id.to_owned())
}

/// The closed trigger: `Model · Harness` once a non-default harness is chosen.
fn display_label(models: &[CatalogModel], model: &str) -> Option<SharedString> {
    let base = base_model_ref(model);
    let harness = harness_of(model);
    if harness.is_empty() {
        return None;
    }
    let label = models
        .iter()
        .find(|m| m.r#ref == base)
        .map(|m| m.label.clone())
        .unwrap_or_else(|| short_model_label(base));
    let harnesses = harnesses_for(models, base);
    Some(format!("{} · {}", label, harness_label(&harnesses, harness)).into())
}

/// `ModelPickers`.
pub struct ModelPickers {
    models: Vec<CatalogModel>,
    model: String,
    flags: FlagMap,
    pending_harness: Option<PendingHarness>,
    /// Open the Run in popup upward, like the model Select's `up`.
    up: bool,
    /// The model trigger's box, recorded at paint like `Select`'s, to fit the popup to the window.
    trigger_bounds: Option<Bounds<Pixels>>,
    model_select: Entity<Select>,
    flags_select: Entity<ModelFlagsSelect>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<PickerEvent> for ModelPickers {}

impl ModelPickers {
    pub fn new(ghost: bool, up: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_labels("Model", "Model flags", ghost, up, window, cx)
    }

    pub fn with_labels(
        model_label: &str,
        flags_label: &str,
        ghost: bool,
        up: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let model_select = cx.new(|cx| {
            // Inside `.model-pickers` the menu is `.sel-menu-model` (`portalMenuMods`).
            let mut s = Select::new(Vec::new(), window, cx)
                .label(model_label.to_string())
                .searchable()
                .menu_variant(recur_ui::logic::menu_space::MenuVariant::Model);
            if ghost {
                s = s.ghost();
            }
            if up {
                s = s.up();
            }
            s
        });
        let flags_select = cx.new(|cx| ModelFlagsSelect::new(flags_label, ghost, up, window, cx));
        // `.flags-menu` in `.model-pickers` is `.sel-menu-model` too (min 240px).
        flags_select.update(cx, |f, cx| {
            f.select.update(cx, |s, cx| {
                s.set_menu_variant(recur_ui::logic::menu_space::MenuVariant::Model, cx)
            })
        });
        let subs = vec![
            cx.guarded_subscribe(&model_select, |this: &mut Self, _, ev: &SelectEvent, cx| {
                let SelectEvent::Change(r) = ev else { return };
                let base = r.to_string();
                let harnesses = harnesses_for(&this.models, &base);
                let harness = match harness_step(&harnesses) {
                    HarnessStep::Pick(h) => h,
                    HarnessStep::Ask => {
                        let label = this
                            .models
                            .iter()
                            .find(|m| m.r#ref == base)
                            .map(|m| m.label.clone())
                            .unwrap_or_else(|| short_model_label(&base));
                        this.pending_harness = Some(PendingHarness {
                            base,
                            label,
                            harnesses,
                        });
                        cx.notify();
                        return;
                    }
                };
                let model = compose_harness_ref(&harness, &base);
                let next = normalize_flags(&flags_for_model(&this.models, &model), None);
                this.pending_harness = None;
                cx.emit(PickerEvent::Model(model, next));
            }),
            cx.guarded_subscribe(&flags_select, |_, _, f: &FlagMap, cx| {
                cx.emit(PickerEvent::Flags(f.clone()))
            }),
        ];
        ModelPickers {
            models: Vec::new(),
            model: String::new(),
            flags: FlagMap::new(),
            pending_harness: None,
            up,
            trigger_bounds: None,
            model_select,
            flags_select,
            _subs: subs,
        }
    }

    fn pick_harness(&mut self, harness: &str, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_harness.take() else {
            return;
        };
        let model = compose_harness_ref(harness, &pending.base);
        let next = normalize_flags(&flags_for_model(&self.models, &model), None);
        let display = display_label(&self.models, &model);
        let base = base_model_ref(&model).to_owned();
        self.model_select.update(cx, |s, cx| {
            s.set_value(base, cx);
            s.set_display(display, cx);
        });
        self.model = model.clone();
        self.flags = next.clone();
        cx.emit(PickerEvent::Model(model, next));
        cx.notify();
    }

    fn cancel_harness(&mut self, cx: &mut Context<Self>) {
        self.pending_harness = None;
        let model = self.model.clone();
        let display = display_label(&self.models, &model);
        self.model_select.update(cx, |s, cx| {
            s.set_value(base_model_ref(&model).to_owned(), cx);
            s.set_display(display, cx);
        });
        cx.notify();
    }

    fn harness_popup(
        &self,
        pending: &PendingHarness,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let t = Theme::of(cx).tokens;
        // Placed like `Select::menu`: the roomier side of the trigger, kept inside the window.
        let trigger = self.trigger_bounds?;
        let vp = window.viewport_size();
        let (vw, vh) = (f32::from(vp.width), f32::from(vp.height));
        let rect = (
            f32::from(trigger.top()),
            f32::from(trigger.bottom()),
            f32::from(trigger.left()),
            HARNESS_POPUP_W,
        );
        let up = self.up || menu_prefer_up(rect.0, rect.1, vh);
        let mbox = menu_fixed_box(rect, up, HARNESS_POPUP_W, (vw, vh));
        let left = clamp_menu_left(mbox.left, HARNESS_POPUP_W, vw);
        let mut panel = style::menu_panel(cx)
            .debug_selector(|| "harness-menu".into())
            .w(px(HARNESS_POPUP_W));
        panel = panel.child(
            div()
                .px(px(10.))
                .pt(px(6.))
                .pb(px(4.))
                .text_size(px(11.))
                .text_color(t.muted)
                .child(format!("Run {} in", pending.label)),
        );
        for h in &pending.harnesses {
            if h.unavailable {
                panel = panel.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .px(px(10.))
                        .py(px(7.))
                        .rounded(px(5.))
                        .text_size(px(12.))
                        .text_color(t.muted)
                        .child(h.label.clone())
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(t.faint)
                                .child(h.reason.clone()),
                        ),
                );
                continue;
            }
            let id = h.id.clone();
            panel = panel.child(
                div()
                    .id(SharedString::from(format!("harness-{id}")))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(5.))
                    .text_size(px(12.5))
                    .text_color(t.text)
                    .cursor_pointer()
                    .hover(|s| s.bg(hsla(t.pane)))
                    .child(h.label.clone())
                    .on_click(cx.guarded_listener(
                        move |this: &mut Self, _: &gpui_kit::ClickEvent, _, cx| {
                            this.pick_harness(&id, cx);
                        },
                    )),
            );
        }
        panel = panel.child(
            div()
                .id("harness-cancel")
                .mt(px(4.))
                .border_t(px(1.))
                .border_color(hsla(t.line))
                .pt(px(4.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(5.))
                .text_size(px(12.))
                .text_color(t.muted)
                .cursor_pointer()
                .hover(|s| s.bg(hsla(t.pane)))
                .child("Cancel")
                .on_click(cx.guarded_listener(
                    |this: &mut Self, _: &gpui_kit::ClickEvent, _, cx| this.cancel_harness(cx),
                )),
        );
        let below = mbox.top.is_some();
        Some(
            deferred(style::attached(
                below,
                MENU_GAP_PX,
                left - rect.2,
                recur_ui::shadow::with_shadow(panel, &style::MENU_SHADOW, 8.),
            ))
            .with_priority(210)
            .into_any_element(),
        )
    }

    /// Restyle both triggers for the context they sit in (Settings' `.model-pickers.chip`
    /// inherits the global `.chip` mono font).
    pub fn set_trigger_style(&mut self, t: TriggerStyle, cx: &mut Context<Self>) {
        let t2 = t.clone();
        self.model_select
            .update(cx, |s, cx| s.set_trigger_style(t, cx));
        self.flags_select.update(cx, |f, cx| {
            f.select.update(cx, |s, cx| s.set_trigger_style(t2, cx))
        });
    }

    /// A menu is open (`.sel.open`).
    pub fn is_open(&self, cx: &App) -> bool {
        self.model_select.read(cx).is_open() || self.flags_select.read(cx).select.read(cx).is_open()
    }

    /// The Tab stops, in order: the model select, then the flags select when it shows.
    pub fn focus_handles(&self, cx: &App) -> Vec<FocusHandle> {
        let mut v = vec![self.model_select.focus_handle(cx)];
        if !flags_for_model(&self.models, &self.model).is_empty() {
            v.push(self.flags_select.read(cx).select.focus_handle(cx));
        }
        v
    }

    /// The props: catalog, current model and flags, disabled (faded over `backdrop`).
    pub fn set(
        &mut self,
        models: Vec<CatalogModel>,
        model: &str,
        flags: &FlagMap,
        disabled_over: Option<recur_ui::Color>,
        cx: &mut Context<Self>,
    ) {
        let options: Vec<SelectOption> = model_options_for(&models, model)
            .into_iter()
            .map(|o| {
                let mut s = SelectOption::new(o.value, o.label).group(o.group);
                if !o.provider.is_empty() {
                    s = s.provider(o.provider);
                }
                s
            })
            .collect();
        let base = base_model_ref(model).to_owned();
        let display = display_label(&models, model);
        self.model_select.update(cx, |s, cx| {
            s.set_options(options, cx);
            s.set_value(base, cx);
            s.set_display(display, cx);
            s.set_disabled(disabled_over, cx);
        });
        let dims = flags_for_model(&models, model);
        let f = flags.clone();
        self.flags_select
            .update(cx, |s, cx| s.set(dims, &f, disabled_over, cx));
        if self.models != models || self.model != model || self.flags != *flags {
            self.models = models;
            self.model = model.to_string();
            self.flags = flags.clone();
            cx.notify();
        }
    }
}

impl ModelPickers {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let has_dims = !flags_for_model(&self.models, &self.model).is_empty();
        let popup = self
            .pending_harness
            .clone()
            .and_then(|p| self.harness_popup(&p, window, cx));
        let entity = cx.entity();
        let vw = f32::from(window.viewport_size().width);
        let max = 320f32.min(0.52 * vw);
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .min_w_0()
            .flex_initial()
            .child(
                div()
                    .relative()
                    .flex_auto()
                    .min_w(px(4.5 * 16.))
                    .max_w(px(max))
                    .child(self.model_select.clone())
                    .child(
                        canvas(
                            // The popup places itself from the model trigger's box.
                            move |bounds, _, cx| {
                                entity.update(cx, |this, _| this.trigger_bounds = Some(bounds))
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
                    .when_some(popup, |d, p| d.child(p)),
            )
            .when(has_dims, |d| {
                d.child(
                    div()
                        .flex_none()
                        .max_w(px(max))
                        .child(self.flags_select.clone()),
                )
            })
    }
}

impl Render for ModelPickers {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// `LoopPolicyPickers`.
pub struct LoopPolicyPickers {
    loop_select: Entity<Select>,
    policy_select: Entity<Select>,
    show_tip: bool,
    tip_open: bool,
    _subs: Vec<Subscription>,
}

/// A loop or policy was picked.
#[derive(Clone, Debug, PartialEq)]
pub enum LoopPolicyEvent {
    Loop(String),
    Policy(String),
}

impl EventEmitter<LoopPolicyEvent> for LoopPolicyPickers {}

/// `.nt-autonomy .sel-btn`.
fn autonomy_trigger(t: &recur_ui::Tokens) -> TriggerStyle {
    TriggerStyle {
        height: Some(28.),
        pad_x: Some(9.),
        radius: Some(7.),
        gap: Some(6.),
        font_size: Some(12.5),
        ui_font: true,
        bg: Some(recur_ui::color::TRANSPARENT),
        border: Some(recur_ui::color::TRANSPARENT),
        color: Some(t.muted),
        hover_bg: Some(t.pane_2),
        hover_border: Some(recur_ui::color::TRANSPARENT),
        hover_color: Some(t.text),
        open_bg: Some(t.pane_2),
        open_border: Some(recur_ui::color::TRANSPARENT),
        open_color: Some(t.text),
        no_ring: true,
        icon_color: Some(t.amber),
        open_icon_color: Some(t.amber),
        open_chevron_color: Some(t.muted),
        auto_width: true,
        ..Default::default()
    }
}

fn select_options(list: Vec<recur_engine::view::loops::SelectOption>) -> Vec<SelectOption> {
    list.into_iter()
        .map(|o| {
            let label = if o.label.is_empty() {
                o.value.clone()
            } else {
                o.label
            };
            let mut s = SelectOption::new(o.value, label);
            if !o.hint.is_empty() {
                s = s.hint(o.hint);
            }
            s
        })
        .collect()
}

impl LoopPolicyPickers {
    pub fn new(
        ghost: bool,
        up: bool,
        show_tip: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let t = Theme::of(cx).tokens;
        let mk = |label: &str, policy: bool, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                // `.sel.ghost .sel-btn` outranks `.nt-autonomy .sel-btn`: a ghost pair keeps
                // only the amber icon, which `.nt-autonomy .sel.open .sel-ico` (later than
                // `.sel.ghost.open .sel-ico`) keeps amber while open; the chevron goes muted.
                let style = if ghost {
                    TriggerStyle {
                        icon_color: Some(t.amber),
                        open_icon_color: Some(t.amber),
                        open_chevron_color: Some(t.muted),
                        ..Default::default()
                    }
                } else {
                    autonomy_trigger(&t)
                };
                let mut s = Select::new(Vec::new(), window, cx)
                    .label(label.to_string())
                    .trigger_style(style);
                if ghost {
                    s = s.ghost();
                }
                if up {
                    s = s.up();
                }
                if policy {
                    s = s.trigger_icon(IconName::Shield);
                }
                // Inside `.nt-autonomy` the menu is `.sel-menu-compact` (`portalMenuMods`).
                s.menu_variant(recur_ui::logic::menu_space::MenuVariant::Compact)
            })
        };
        let loop_select = mk("Loop", false, window, cx);
        let policy_select = mk("Policy", true, window, cx);
        let subs = vec![
            cx.guarded_subscribe(&loop_select, |_, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(v) = ev {
                    cx.emit(LoopPolicyEvent::Loop(v.to_string()));
                }
            }),
            cx.guarded_subscribe(&policy_select, |_, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(v) = ev {
                    cx.emit(LoopPolicyEvent::Policy(v.to_string()));
                }
            }),
        ];
        LoopPolicyPickers {
            loop_select,
            policy_select,
            show_tip,
            tip_open: false,
            _subs: subs,
        }
    }

    /// A menu is open (`.sel.open`).
    pub fn is_open(&self, cx: &App) -> bool {
        self.loop_select.read(cx).is_open() || self.policy_select.read(cx).is_open()
    }

    /// The Tab stops: the loop and policy selects.
    pub fn focus_handles(&self, cx: &App) -> Vec<FocusHandle> {
        vec![
            self.loop_select.focus_handle(cx),
            self.policy_select.focus_handle(cx),
        ]
    }

    /// `.nt-modal .sel-btn.kb-focus` on both triggers (see `TriggerStyle::kb_ring`).
    pub fn set_kb_ring(&mut self, on: bool, cx: &mut Context<Self>) {
        for s in [&self.loop_select, &self.policy_select] {
            s.update(cx, |s, cx| {
                let mut t = s.trigger().clone();
                if t.kb_ring != on {
                    t.kb_ring = on;
                    t.no_outline = on;
                    s.set_trigger_style(t, cx);
                }
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set(
        &mut self,
        loop_id: &str,
        policy_id: &str,
        loops: &[LoopInfo],
        policies: &[PolicyInfo],
        pinned: bool,
        backdrop: recur_ui::Color,
        cx: &mut Context<Self>,
    ) {
        let lo = select_options(loop_select_options(loops));
        let po = select_options(policy_select_options(policies));
        let (l, p) = (loop_id.to_string(), policy_id.to_string());
        let (ld, pd) = (pinned || lo.is_empty(), pinned || po.is_empty());
        self.loop_select.update(cx, |s, cx| {
            s.set_options(lo, cx);
            s.set_value(l, cx);
            s.set_disabled(ld.then_some(backdrop), cx);
        });
        self.policy_select.update(cx, |s, cx| {
            s.set_options(po, cx);
            s.set_value(p, cx);
            s.set_disabled(pd.then_some(backdrop), cx);
        });
    }
}

impl LoopPolicyPickers {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let mut row = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .flex_none()
            .child(self.loop_select.clone())
            .child(self.policy_select.clone());
        if self.show_tip {
            let open = self.tip_open;
            let mut tip = div().relative().flex_none().child(
                div()
                    .id("nt-help")
                    .w(px(26.))
                    .h(px(28.))
                    .rounded(px(7.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(hsla(if open { t.muted } else { t.faint }))
                    .when(open, |d| d.bg(hsla(t.pane_2)))
                    .hover(|s| s.bg(hsla(t.pane_2)).text_color(hsla(t.muted)))
                    .child(icon(IconName::Info).size(px(14.)))
                    .on_click(cx.guarded_listener(|p, _, _, cx| {
                        p.tip_open = !p.tip_open;
                        cx.notify();
                    })),
            );
            if open {
                // `.nt-tip-pop`: `<b>` at 600 in `--text`; the `<ul>` 8px down with a 1.1em
                // indent and `disc` markers, items 3px apart.
                let b = |s: &str| span(s.to_string()).weight(600.).color(t.text);
                let line = |head: &str, rest: &str| -> Text {
                    Text::from_spans(vec![b(head), span(rest.to_string())])
                        .size(12.)
                        .leading(1.45)
                };
                let item = |head: &str, rest: &str, window: &Window| {
                    div()
                        .relative()
                        .child(recur_content::render::block::disc_marker(
                            12., 1.45, t.muted, window,
                        ))
                        .child(line(head, rest).id(SharedString::from(format!("tip-{head}"))))
                };
                let intro: Vec<Span> = vec![
                    b("Policy"),
                    span(" is how hard this loop pushes before it asks you."),
                ];
                let gap = |h: f32| div().flex_none().h(px(h));
                let pop = recur_ui::style::popover(cx)
                    .child(
                        Text::from_spans(intro)
                            .size(12.)
                            .leading(1.45)
                            .id("tip-intro"),
                    )
                    .child(
                        div()
                            .mt(px(8.))
                            .mb(px(3.))
                            .flex()
                            .child(div().flex_none().w(px(13.2)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(item(
                                        "Cautious",
                                        ": every finding; escalate after 2 rounds",
                                        window,
                                    ))
                                    .child(gap(3.))
                                    .child(item(
                                        "Balanced",
                                        ": medium and up; escalate after 4",
                                        window,
                                    ))
                                    .child(gap(3.))
                                    .child(item(
                                        "Aggressive",
                                        ": ship-blockers only; proceed after 6",
                                        window,
                                    )),
                            ),
                    );
                tip = tip.child(recur_ui::style::deferred_clipped(
                    div()
                        .absolute()
                        .left_0()
                        .bottom(gpui_kit::relative(1.))
                        .pb(px(8.))
                        .child(recur_ui::shadow::with_shadow(
                            pop,
                            &recur_ui::style::POPOVER_SHADOW,
                            9.,
                        )),
                    200,
                ));
            }
            row = row.child(tip);
        }
        row
    }
}

impl Render for LoopPolicyPickers {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_services::api::FlagOption;

    fn dim(id: &str, label: &str, opts: &[(&str, &str, bool)]) -> FlagDimension {
        FlagDimension {
            id: id.into(),
            label: label.into(),
            options: opts
                .iter()
                .map(|(v, l, d)| FlagOption {
                    value: v.to_string(),
                    label: l.to_string(),
                    default: *d,
                })
                .collect(),
        }
    }

    fn harness(id: &str, unavailable: bool, not_installed: bool) -> HarnessChoice {
        HarnessChoice {
            id: id.into(),
            label: id.into(),
            unavailable,
            reason: if unavailable {
                "why".into()
            } else {
                String::new()
            },
            not_installed,
        }
    }

    #[test]
    fn harness_step_cases() {
        assert_eq!(harness_step(&[]), HarnessStep::Pick(String::new()));
        // Both run the model: ask.
        assert_eq!(
            harness_step(&[harness("", false, false), harness("opencode", false, false)]),
            HarnessStep::Ask
        );
        // OpenCode is only missing here: pick Recur without a popup.
        assert_eq!(
            harness_step(&[harness("", false, false), harness("opencode", true, true)]),
            HarnessStep::Pick(String::new())
        );
        // The endpoint cannot run in OpenCode: ask, so the reason shows.
        assert_eq!(
            harness_step(&[harness("", false, false), harness("opencode", true, false)]),
            HarnessStep::Ask
        );
        // Only OpenCode can run it: pick it directly.
        assert_eq!(
            harness_step(&[harness("", true, true), harness("opencode", false, false)]),
            HarnessStep::Pick("opencode".into())
        );
    }

    // ModelFlagsSelect summary tokens.
    #[test]
    fn summary_tokens() {
        let fast = dim(
            "fast",
            "Fast",
            &[("true", "On", false), ("false", "Off", true)],
        );
        let effort = dim(
            "effort",
            "Effort",
            &[("low", "Low", false), ("high", "High", true)],
        );
        let mut sel = FlagMap::new();
        sel.insert("fast".into(), "false".into());
        sel.insert("effort".into(), "high".into());
        assert_eq!(flags_summary(&[fast.clone(), effort.clone()], &sel), "High");
        sel.insert("fast".into(), "true".into());
        assert_eq!(flags_summary(&[fast.clone(), effort], &sel), "Fast · High");
        assert_eq!(flags_summary(&[fast], &FlagMap::new()), "-");
    }
}
