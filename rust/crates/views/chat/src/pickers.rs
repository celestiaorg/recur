//! The composer's pickers: `ModelPickers.svelte` (the base model Select plus
//! `ModelFlagsSelect.svelte`, one pick per flag dimension in a menu that stays open) and
//! `LoopPolicyPickers.svelte` (loop + policy Selects with the policy help popover).

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
    div, px,
};
use recur_engine::view::loops::{LoopInfo, PolicyInfo, loop_select_options, policy_select_options};
use recur_services::api::FlagDimension;
use recur_ui::text::{Span, Text, TextStyled, span};
use recur_ui::widgets::select::{Select, SelectEvent, SelectOption, TriggerStyle};
use recur_ui::{IconName, Theme, icon};
use recur_view_core::models::{CatalogModel, FlagMap, flags_for_model, model_options_for, normalize_flags};

use crate::style::hsla;

const SEP: char = '\u{1f}';

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
        return if v == "true" { d.label.clone() } else { String::new() };
    }
    d.options
        .iter()
        .find(|o| o.value == v)
        .map_or_else(|| v.to_string(), |o| o.label.clone())
}

/// The closed trigger: tokens joined by ` · `, or `—`.
pub fn flags_summary(dims: &[FlagDimension], selected: &FlagMap) -> String {
    let toks: Vec<String> = dims
        .iter()
        .map(|d| summary_token(d, selected.get(&d.id).map(String::as_str)))
        .filter(|t| !t.is_empty())
        .collect();
    if toks.is_empty() { "—".into() } else { toks.join(" · ") }
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
                let label = if o.default { format!("{} (default)", o.label) } else { o.label.clone() };
                SelectOption::new(format!("{}{SEP}{}", d.id, o.value), label).group(d.label.clone())
            })
        })
        .collect()
}

fn flag_values(selected: &FlagMap) -> Vec<SharedString> {
    selected.iter().map(|(k, v)| format!("{k}{SEP}{v}").into()).collect()
}

impl ModelFlagsSelect {
    pub fn new(label: &str, ghost: bool, up: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let select = cx.new(|cx| {
            let mut s = Select::new(Vec::new(), window, cx).label(label.to_string()).multiple();
            if ghost {
                s = s.ghost();
            }
            if up {
                s = s.up();
            }
            s
        });
        let sub = cx.subscribe(&select, |this: &mut Self, _, ev: &SelectEvent, cx| {
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

    pub fn set(&mut self, dims: Vec<FlagDimension>, value: &FlagMap, disabled_over: Option<recur_ui::Color>, cx: &mut Context<Self>) {
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
        self.dims = dims;
        self.selected = selected;
        cx.notify();
    }
}

impl Render for ModelFlagsSelect {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().flex_none().child(self.select.clone())
    }
}

/// `ModelPickers`.
pub struct ModelPickers {
    models: Vec<CatalogModel>,
    model: String,
    flags: FlagMap,
    model_select: Entity<Select>,
    flags_select: Entity<ModelFlagsSelect>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<PickerEvent> for ModelPickers {}

impl ModelPickers {
    pub fn new(ghost: bool, up: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_labels("Model", "Model flags", ghost, up, window, cx)
    }

    pub fn with_labels(model_label: &str, flags_label: &str, ghost: bool, up: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model_select = cx.new(|cx| {
            let mut s = Select::new(Vec::new(), window, cx).label(model_label.to_string()).searchable();
            if ghost {
                s = s.ghost();
            }
            if up {
                s = s.up();
            }
            s
        });
        let flags_select = cx.new(|cx| ModelFlagsSelect::new(flags_label, ghost, up, window, cx));
        let subs = vec![
            cx.subscribe(&model_select, |this: &mut Self, _, ev: &SelectEvent, cx| {
                let SelectEvent::Change(r) = ev else { return };
                let next = normalize_flags(&flags_for_model(&this.models, r), None);
                cx.emit(PickerEvent::Model(r.to_string(), next));
            }),
            cx.subscribe(&flags_select, |_, _, f: &FlagMap, cx| cx.emit(PickerEvent::Flags(f.clone()))),
        ];
        ModelPickers {
            models: Vec::new(),
            model: String::new(),
            flags: FlagMap::new(),
            model_select,
            flags_select,
            _subs: subs,
        }
    }

    /// The props: catalog, current model and flags, disabled (faded over `backdrop`).
    pub fn set(&mut self, models: Vec<CatalogModel>, model: &str, flags: &FlagMap, disabled_over: Option<recur_ui::Color>, cx: &mut Context<Self>) {
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
        let m = model.to_string();
        self.model_select.update(cx, |s, cx| {
            s.set_options(options, cx);
            s.set_value(m, cx);
            s.set_disabled(disabled_over, cx);
        });
        let dims = flags_for_model(&models, model);
        let f = flags.clone();
        self.flags_select.update(cx, |s, cx| s.set(dims, &f, disabled_over, cx));
        self.models = models;
        self.model = model.to_string();
        self.flags = flags.clone();
        cx.notify();
    }
}

impl Render for ModelPickers {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_dims = !flags_for_model(&self.models, &self.model).is_empty();
        let vw = f32::from(window.viewport_size().width);
        let max = 320f32.min(0.52 * vw);
        let _ = cx;
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .min_w_0()
            .flex_initial()
            .child(div().flex_auto().min_w(px(4.5 * 16.)).max_w(px(max)).child(self.model_select.clone()))
            .when(has_dims, |d| d.child(div().flex_none().max_w(px(max)).child(self.flags_select.clone())))
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
        open_icon_color: Some(t.muted),
        auto_width: true,
        ..Default::default()
    }
}

fn select_options(list: Vec<recur_engine::view::loops::SelectOption>) -> Vec<SelectOption> {
    list.into_iter()
        .map(|o| {
            let label = if o.label.is_empty() { o.value.clone() } else { o.label };
            let mut s = SelectOption::new(o.value, label);
            if !o.hint.is_empty() {
                s = s.hint(o.hint);
            }
            s
        })
        .collect()
}

impl LoopPolicyPickers {
    pub fn new(ghost: bool, up: bool, show_tip: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let t = Theme::of(cx).tokens;
        let mk = |label: &str, policy: bool, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut s = Select::new(Vec::new(), window, cx).label(label.to_string()).trigger_style(autonomy_trigger(&t));
                if ghost {
                    s = s.ghost();
                }
                if up {
                    s = s.up();
                }
                if policy {
                    s = s.trigger_icon(IconName::Shield);
                }
                s
            })
        };
        let loop_select = mk("Loop", false, window, cx);
        let policy_select = mk("Policy", true, window, cx);
        let subs = vec![
            cx.subscribe(&loop_select, |_, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(v) = ev {
                    cx.emit(LoopPolicyEvent::Loop(v.to_string()));
                }
            }),
            cx.subscribe(&policy_select, |_, _, ev: &SelectEvent, cx| {
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
        cx.notify();
    }
}

impl Render for LoopPolicyPickers {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .on_click(cx.listener(|p, _, _, cx| {
                        p.tip_open = !p.tip_open;
                        cx.notify();
                    })),
            );
            if open {
                let b = |s: &str| span(s.to_string()).weight(700.).color(t.text);
                let line = |head: &str, rest: &str| -> Text {
                    Text::from_spans(vec![b(head), span(rest.to_string())])
                };
                let bullet = |head: &str, rest: &str| {
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(recur_ui::text("•").nowrap())
                        .child(div().flex_1().min_w_0().child(line(head, rest).id(SharedString::from(format!("tip-{head}")))))
                };
                let intro: Vec<Span> = vec![b("Policy"), span(" is how hard this loop pushes before it asks you.")];
                let pop = recur_ui::style::popover(cx)
                    .child(Text::from_spans(intro).id("tip-intro"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .pl(px(18.))
                            .child(bullet("Cautious", " — every finding; escalate after 2 rounds"))
                            .child(bullet("Balanced", " — medium and up; escalate after 4"))
                            .child(bullet("Aggressive", " — ship-blockers only; proceed after 6")),
                    );
                tip = tip.child(
                    div()
                        .absolute()
                        .left_0()
                        .bottom(gpui_kit::relative(1.))
                        .pb(px(8.))
                        .child(recur_ui::shadow::with_shadow(pop, &recur_ui::style::POPOVER_SHADOW, 9.)),
                );
            }
            row = row.child(tip);
        }
        row
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

    // ModelFlagsSelect summary tokens.
    #[test]
    fn summary_tokens() {
        let fast = dim("fast", "Fast", &[("true", "On", false), ("false", "Off", true)]);
        let effort = dim("effort", "Effort", &[("low", "Low", false), ("high", "High", true)]);
        let mut sel = FlagMap::new();
        sel.insert("fast".into(), "false".into());
        sel.insert("effort".into(), "high".into());
        assert_eq!(flags_summary(&[fast.clone(), effort.clone()], &sel), "High");
        sel.insert("fast".into(), "true".into());
        assert_eq!(flags_summary(&[fast.clone(), effort], &sel), "Fast · High");
        assert_eq!(flags_summary(&[fast], &FlagMap::new()), "—");
    }
}
