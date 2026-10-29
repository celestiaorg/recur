//! One OpenAI-compatible endpoint's name, base URL, key, extra
//! headers, models with their picker controls, and removal.

use recur_ui::guard::Guarded;
use std::collections::{HashMap, HashSet};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Div, Entity, EventEmitter, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, div, px,
};
use recur_domain::{CompatibleFlagDim, CompatibleFlagOption, CompatibleModel, CompatibleProvider};
use recur_services::Services;
use recur_ui::fonts::{Family, weight};
use recur_ui::style::{card, flex_share};
use recur_ui::text::{LineHeight, Span, Text, TextStyled, span, text};
use recur_ui::widgets::{Select, SelectEvent, SelectOption, SelectSize, TriggerStyle};
use recur_ui::{IconName, icon};

use crate::field::{Field, FieldEvent, FieldOpts};
use crate::rules::{
    COMPATIBLE_WIRES, flag_kind, persistable_flags, persistable_headers, persistable_models,
    unused_key,
};
use crate::style::{
    InpStyle, Row, RowsCtx, code, copy, ctrl, danger_btn, field_msg, hsla, inp, rm_btn, rows, tbtn,
    tbtn_icon, tbtn_label, tokens,
};

/// What the card asks its host to do.
#[derive(Clone, Debug, PartialEq)]
pub enum CardEvent {
    /// `onChange(next)`.
    Change(CompatibleProvider),
    Test,
    Remove,
}

/// `CONTROL_PRESETS`.
const CONTROL_PRESETS: [(&str, &str); 4] = [
    ("", "None"),
    ("effort", "Effort"),
    ("openai", "Effort, verbosity"),
    ("custom", "Custom…"),
];

pub struct CompatibleProviderCard {
    provider: CompatibleProvider,
    testing: bool,
    test_msg: Option<(bool, String)>,
    services: Services,
    key_visible: bool,
    header_drafts: Vec<(String, String)>,
    drafts: Vec<CompatibleModel>,
    customizing: HashSet<usize>,
    fields: HashMap<String, Field>,
    selects: HashMap<String, Entity<Select>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<CardEvent> for CompatibleProviderCard {}

impl CompatibleProviderCard {
    pub fn new(
        provider: CompatibleProvider,
        services: Services,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Self {
        CompatibleProviderCard {
            provider,
            testing: false,
            test_msg: None,
            services,
            key_visible: false,
            header_drafts: Vec::new(),
            drafts: Vec::new(),
            customizing: HashSet::new(),
            fields: HashMap::new(),
            selects: HashMap::new(),
            _subs: Vec::new(),
        }
    }

    /// The props.
    pub fn set(
        &mut self,
        provider: CompatibleProvider,
        testing: bool,
        test_msg: Option<(bool, String)>,
        cx: &mut Context<Self>,
    ) {
        self.provider = provider;
        self.testing = testing;
        self.test_msg = test_msg;
        cx.notify();
    }

    fn emit_set(&self, f: impl FnOnce(&mut CompatibleProvider), cx: &mut Context<Self>) {
        let mut next = self.provider.clone();
        f(&mut next);
        cx.emit(CardEvent::Change(next));
    }

    // ---- headers ----

    fn header_saved(&self) -> Vec<(String, String)> {
        self.provider
            .extra_headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn header_rows(&self) -> Vec<(String, String)> {
        let mut r = self.header_saved();
        r.extend(self.header_drafts.iter().cloned());
        r
    }

    /// `setHeader`.
    fn set_header(
        &mut self,
        i: usize,
        key: Option<String>,
        value: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let saved = self.header_saved();
        let n = saved.len();
        let patch =
            |(k, v): (String, String)| (key.clone().unwrap_or(k), value.clone().unwrap_or(v));
        if i < n {
            let next: Vec<_> = saved
                .into_iter()
                .enumerate()
                .map(|(j, r)| if j == i { patch(r) } else { r })
                .collect();
            let h = persistable_headers(&next);
            self.emit_set(|p| p.extra_headers = h, cx);
            return;
        }
        let next: Vec<_> = std::mem::take(&mut self.header_drafts)
            .into_iter()
            .enumerate()
            .map(|(j, r)| if j + n == i { patch(r) } else { r })
            .collect();
        let ready: Vec<_> = next
            .iter()
            .filter(|r| !r.0.trim().is_empty())
            .cloned()
            .collect();
        self.header_drafts = next.into_iter().filter(|r| r.0.trim().is_empty()).collect();
        if !ready.is_empty() {
            let mut all = saved;
            all.extend(ready);
            let h = persistable_headers(&all);
            self.emit_set(|p| p.extra_headers = h, cx);
        }
        cx.notify();
    }

    fn remove_header(&mut self, i: usize, cx: &mut Context<Self>) {
        let saved = self.header_saved();
        let n = saved.len();
        if i < n {
            let next: Vec<_> = saved
                .into_iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, r)| r)
                .collect();
            let h = persistable_headers(&next);
            self.emit_set(|p| p.extra_headers = h, cx);
            return;
        }
        if i - n < self.header_drafts.len() {
            self.header_drafts.remove(i - n);
        }
        cx.notify();
    }

    // ---- models ----

    /// `rows`: saved then drafts, or one blank row.
    fn rows(&self) -> Vec<CompatibleModel> {
        if self.provider.models.is_empty() && self.drafts.is_empty() {
            return vec![CompatibleModel::default()];
        }
        let mut r = self.provider.models.clone();
        r.extend(self.drafts.iter().cloned());
        r
    }

    /// `setModel`: saved rows save when some id remains; draft rows join the saved list once
    /// they have an id.
    fn set_model(&mut self, i: usize, f: impl Fn(&mut CompatibleModel), cx: &mut Context<Self>) {
        let saved = self.provider.models.clone();
        let n = saved.len();
        if i < n {
            let next: Vec<CompatibleModel> = saved
                .into_iter()
                .enumerate()
                .map(|(j, mut m)| {
                    if j == i {
                        f(&mut m);
                    }
                    m
                })
                .collect();
            let next = persistable_models(&next);
            if !next.is_empty() {
                self.emit_set(|p| p.models = next, cx);
            }
            return;
        }
        let next: Vec<CompatibleModel> = std::mem::take(&mut self.drafts)
            .into_iter()
            .enumerate()
            .map(|(j, mut m)| {
                if j + n == i {
                    f(&mut m);
                }
                m
            })
            .collect();
        let ready = persistable_models(&next);
        self.drafts = next
            .into_iter()
            .filter(|m| m.id.trim().is_empty())
            .collect();
        if !ready.is_empty() {
            let mut all = saved;
            all.extend(ready);
            self.emit_set(|p| p.models = all, cx);
        }
        cx.notify();
    }

    fn controls_kind(&self, m: &CompatibleModel, i: usize) -> &'static str {
        let kind = flag_kind(m);
        if !kind.is_empty() && self.customizing.contains(&i) {
            "custom"
        } else {
            kind
        }
    }

    /// `applyFlags`: Custom… opens the editor (seeding an Effort control); a preset loads its
    /// controls from the backend.
    fn apply_flags(&mut self, i: usize, kind: String, cx: &mut Context<Self>) {
        self.customizing.remove(&i);
        if kind == "custom" {
            self.customizing.insert(i);
            let cur = self
                .rows()
                .get(i)
                .map(|m| m.flags.clone())
                .unwrap_or_default();
            if !cur.is_empty() {
                cx.notify();
                return;
            }
            self.set_model(
                i,
                |m| {
                    m.flags = vec![CompatibleFlagDim {
                        id: "effort".into(),
                        label: "Effort".into(),
                        wire: "reasoning.effort".into(),
                        options: Vec::new(),
                    }]
                },
                cx,
            );
            return;
        }
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let flags = if kind.is_empty() {
                Vec::new()
            } else {
                services
                    .settings
                    .compatible_flag_preset(&kind)
                    .await
                    .unwrap_or_default()
            };
            let _ = this.update(cx, |this, cx| {
                this.set_model(i, |m| m.flags = flags.clone(), cx)
            });
        })
        .detach();
    }

    fn set_flag(
        &mut self,
        i: usize,
        di: usize,
        f: impl Fn(&mut CompatibleFlagDim),
        cx: &mut Context<Self>,
    ) {
        let Some(m) = self.rows().get(i).cloned() else {
            return;
        };
        let mut flags = m.flags;
        if let Some(d) = flags.get_mut(di) {
            f(d);
        }
        let flags = persistable_flags(&flags);
        self.set_model(i, |m| m.flags = flags.clone(), cx);
    }

    fn add_flag(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(m) = self.rows().get(i).cloned() else {
            return;
        };
        let id = unused_key(m.flags.iter().map(|f| f.id.as_str()), "flag");
        let mut flags = m.flags;
        flags.push(CompatibleFlagDim {
            id,
            label: "New control".into(),
            wire: String::new(),
            options: Vec::new(),
        });
        self.set_model(i, |m| m.flags = flags.clone(), cx);
    }

    fn remove_flag(&mut self, i: usize, di: usize, cx: &mut Context<Self>) {
        let Some(m) = self.rows().get(i).cloned() else {
            return;
        };
        let kept: Vec<_> = m
            .flags
            .into_iter()
            .enumerate()
            .filter(|(j, _)| *j != di)
            .map(|(_, d)| d)
            .collect();
        let flags = persistable_flags(&kept);
        self.set_model(i, |m| m.flags = flags.clone(), cx);
    }

    /// `setOption`: a default pick clears the others.
    fn set_option(
        &mut self,
        i: usize,
        di: usize,
        oi: usize,
        f: impl Fn(&mut CompatibleFlagOption),
        cx: &mut Context<Self>,
    ) {
        let Some(dim) = self.rows().get(i).and_then(|m| m.flags.get(di)).cloned() else {
            return;
        };
        let mut options = dim.options;
        if let Some(o) = options.get_mut(oi) {
            f(o);
        }
        if options.get(oi).is_some_and(|o| o.default) {
            for (j, o) in options.iter_mut().enumerate() {
                if j != oi {
                    o.default = false;
                }
            }
        }
        self.set_flag(i, di, |d| d.options = options.clone(), cx);
    }

    fn add_option(&mut self, i: usize, di: usize, cx: &mut Context<Self>) {
        let Some(dim) = self.rows().get(i).and_then(|m| m.flags.get(di)).cloned() else {
            return;
        };
        let value = unused_key(dim.options.iter().map(|o| o.value.as_str()), "value");
        let mut options = dim.options;
        options.push(CompatibleFlagOption {
            label: value.clone(),
            value,
            default: false,
        });
        self.set_flag(i, di, |d| d.options = options.clone(), cx);
    }

    fn remove_option(&mut self, i: usize, di: usize, oi: usize, cx: &mut Context<Self>) {
        let Some(dim) = self.rows().get(i).and_then(|m| m.flags.get(di)).cloned() else {
            return;
        };
        let options: Vec<_> = dim
            .options
            .into_iter()
            .enumerate()
            .filter(|(j, _)| *j != oi)
            .map(|(_, o)| o)
            .collect();
        self.set_flag(i, di, |d| d.options = options.clone(), cx);
    }

    fn remove_model(&mut self, i: usize, cx: &mut Context<Self>) {
        self.customizing = self
            .customizing
            .iter()
            .filter(|j| **j != i)
            .map(|j| if *j > i { j - 1 } else { *j })
            .collect();
        let saved = self.provider.models.clone();
        let n = saved.len();
        if i < n {
            let next: Vec<_> = saved
                .into_iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, m)| m)
                .collect();
            if !next.is_empty() {
                self.emit_set(|p| p.models = next, cx);
            }
            return;
        }
        if i - n < self.drafts.len() {
            self.drafts.remove(i - n);
        }
        cx.notify();
    }

    // ---- fields ----

    fn bound(
        &mut self,
        key: &str,
        v: &str,
        opts: FieldOpts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<gpui_kit::base::input::InputState> {
        if !self.fields.contains_key(key) {
            let f = Field::new(key.to_string(), opts, Self::field_event, window, cx);
            self.fields.insert(key.to_string(), f);
        }
        let f = self.fields.get_mut(key).expect("just inserted");
        f.bind(v, window, cx);
        recur_ui::style::style_text_input(&f.input, cx);
        f.input.clone()
    }

    fn field_event(
        &mut self,
        key: &str,
        ev: FieldEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            FieldEvent::Focus => {
                if let Some(f) = self.fields.get_mut(key) {
                    f.focused(cx);
                }
                return;
            }
            FieldEvent::Change => {
                if !self.fields.get_mut(key).is_some_and(|f| f.take_change(cx)) {
                    return;
                }
            }
            _ => return,
        }
        let v = self
            .fields
            .get(key)
            .map(|f| f.value(cx))
            .unwrap_or_default();
        let parts: Vec<&str> = key.split(':').collect();
        let idx = |k: usize| {
            parts
                .get(k)
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0)
        };
        match parts.first().copied().unwrap_or("") {
            "name" => self.emit_set(|p| p.name = v, cx),
            "url" => self.emit_set(|p| p.base_url = v, cx),
            "key" => self.emit_set(|p| p.api_key = v, cx),
            "hk" => self.set_header(idx(1), Some(v), None, cx),
            "hv" => self.set_header(idx(1), None, Some(v), cx),
            "mid" => self.set_model(
                idx(1),
                move |m| {
                    if m.label.is_empty() {
                        m.label = v.clone();
                    }
                    m.id = v.clone();
                },
                cx,
            ),
            "mlabel" => self.set_model(idx(1), move |m| m.label = v.clone(), cx),
            "mctx" => {
                let n = crate::rules::number_input_value(&v);
                let w = if n > 0. {
                    crate::rules::js_round(n) as i64
                } else {
                    0
                };
                self.set_model(idx(1), move |m| m.context_window = w, cx)
            }
            "fl" => self.set_flag(idx(1), idx(2), move |d| d.label = v.clone(), cx),
            "ov" => self.set_option(
                idx(1),
                idx(2),
                idx(3),
                move |o| {
                    if o.label.is_empty() {
                        o.label = v.clone();
                    }
                    o.value = v.clone();
                },
                cx,
            ),
            "ol" => self.set_option(idx(1), idx(2), idx(3), move |o| o.label = v.clone(), cx),
            _ => {}
        }
        cx.notify();
    }

    fn select(
        &mut self,
        key: &str,
        build: impl FnOnce(&mut Window, &mut Context<Select>) -> Select,
        on: impl Fn(&mut Self, String, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Select> {
        if let Some(s) = self.selects.get(key) {
            return s.clone();
        }
        let sel = cx.new(|cx| build(window, cx));
        self._subs.push(cx.guarded_subscribe(
            &sel,
            move |this: &mut Self, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(v) = ev {
                    on(this, v.to_string(), cx);
                }
            },
        ));
        self.selects.insert(key.to_string(), sel.clone());
        sel
    }
}

/// An `.inp` cell with its spec handle.
fn cell(
    label: String,
    input: &Entity<gpui_kit::base::input::InputState>,
    placeholder: &str,
    style: InpStyle,
    window: &Window,
    cx: &App,
) -> Div {
    div()
        .flex()
        .min_w_0()
        .debug_selector(move || format!("inp:{label}"))
        .child(inp(
            input,
            &placeholder.to_string().into(),
            style,
            window,
            cx,
        ))
}

/// `.compat-cols`: 11px `--muted`.
fn cols(s: &str, cx: &App) -> Text {
    text(s.to_string())
        .size(11.)
        .leading(1.45)
        .color(tokens(cx).muted)
        .nowrap()
}

impl CompatibleProviderCard {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = tokens(cx);
        let p = self.provider.clone();
        let id = p.id.clone();

        // Display name, base URL, key.
        let name = self.bound("name", &p.name, FieldOpts::default(), window, cx);
        let url = self.bound("url", &p.base_url, FieldOpts::default(), window, cx);
        let key = self.bound("key", &p.api_key, FieldOpts::default(), window, cx);
        if let Some(f) = self.fields.get("key") {
            f.set_masked(!self.key_visible, window, cx);
        }
        let visible = self.key_visible;
        let mut key_row = Row::stack()
            .child(copy(
                &format!("{id}-key"),
                "API key",
                Some(vec![
                    span("Sent as "),
                    code("Authorization: Bearer", cx),
                    span(". Optional for local servers."),
                ]),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_stretch()
                    .gap(px(8.))
                    .w_full()
                    .child(
                        cell("API key".into(), &key, "sk-…", InpStyle::BASE, window, cx).flex_1(),
                    )
                    .child(
                        tbtn("compat-eye", p.api_key.is_empty(), cx)
                            .h_auto()
                            .debug_selector(move || {
                                format!(
                                    "btn:{}",
                                    if visible {
                                        "Hide API key"
                                    } else {
                                        "Show API key"
                                    }
                                )
                            })
                            .child(tbtn_icon(if visible {
                                IconName::EyeOff
                            } else {
                                IconName::Eye
                            }))
                            .when(!p.api_key.is_empty(), |d| {
                                d.on_click(cx.guarded_listener(|this, _, _, cx| {
                                    this.key_visible = !this.key_visible;
                                    cx.notify();
                                }))
                            }),
                    )
                    .child(
                        tbtn_label(
                            "compat-test",
                            "Test",
                            self.testing || p.base_url.is_empty(),
                            cx,
                        )
                        .h_auto()
                        .debug_selector(|| "btn:Test".into())
                        .when(
                            !(self.testing || p.base_url.is_empty()),
                            |d| {
                                d.on_click(
                                    cx.guarded_listener(|_, _, _, cx| cx.emit(CardEvent::Test)),
                                )
                            },
                        ),
                    ),
            );
        if self.testing {
            key_row = key_row.child(field_msg("compat-msg", "Testing…", None, cx));
        } else if let Some((ok, msg)) = &self.test_msg {
            key_row = key_row.child(field_msg("compat-msg", msg, Some(*ok), cx));
        }

        // Extra headers.
        let mut headers = div().flex().flex_col().gap(px(8.)).w_full();
        for (i, (k, v)) in self.header_rows().into_iter().enumerate() {
            let ki = self.bound(&format!("hk:{i}"), &k, FieldOpts::default(), window, cx);
            let vi = self.bound(&format!("hv:{i}"), &v, FieldOpts::default(), window, cx);
            headers =
                headers.child(
                    div()
                        .flex()
                        .items_stretch()
                        .gap(px(6.))
                        .child(flex_share(
                            1.,
                            cell(
                                "Header key".into(),
                                &ki,
                                "Key",
                                InpStyle::COMPAT,
                                window,
                                cx,
                            )
                            .flex_1(),
                        ))
                        .child(flex_share(
                            1.2,
                            cell(
                                "Header value".into(),
                                &vi,
                                "Value",
                                InpStyle::COMPAT,
                                window,
                                cx,
                            )
                            .flex_1(),
                        ))
                        .child(
                            div().flex().items_center().child(
                                rm_btn(
                                    SharedString::from(format!("hrm-{i}")),
                                    IconName::Trash,
                                    false,
                                    cx,
                                )
                                .debug_selector(move || format!("btn:Remove header {i}"))
                                .on_click(cx.guarded_listener(move |this, _, _, cx| {
                                    this.remove_header(i, cx)
                                })),
                            ),
                        ),
                );
        }
        headers = headers.child(
            div().flex().gap(px(8.)).mt(px(4.)).child(
                tbtn("compat-add-header", false, cx)
                    .debug_selector(|| "btn:Add header".into())
                    .child(tbtn_icon(IconName::Plus))
                    .child(recur_ui::style::button_label(
                        "Add header",
                        12.,
                        weight::MEDIUM,
                    ))
                    .on_click(cx.guarded_listener(|this, _, _, cx| {
                        this.header_drafts.push((String::new(), String::new()));
                        cx.notify();
                    })),
            ),
        );

        // Models.
        let rows_now = self.rows();
        let saved_n = p.models.len();
        let n_rows = rows_now.len();
        // Spec handles: `{row}:{column}` on each column (`compat-cols` heads, `compat-model:{i}`).
        let model_grid = |row: String,
                          a: AnyElement,
                          b: AnyElement,
                          c: AnyElement,
                          d: AnyElement,
                          e: AnyElement| {
            let at = |n: usize| {
                let sel = format!("{row}:{n}");
                move || sel.clone()
            };
            div()
                .flex()
                .items_stretch()
                .gap(px(6.))
                .child(flex_share(
                    1.2,
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .debug_selector(at(0))
                        .child(a),
                ))
                .child(flex_share(
                    1.,
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .debug_selector(at(1))
                        .child(b),
                ))
                .child(
                    div()
                        .flex_none()
                        .w(px(88.))
                        .flex()
                        .debug_selector(at(2))
                        .child(c),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(176.))
                        .flex()
                        .flex_col()
                        .debug_selector(at(3))
                        .child(d),
                )
                .child(div().flex_none().w(px(28.)).flex().items_center().child(e))
        };
        let mut models = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .w_full()
            .child(model_grid(
                "compat-cols".into(),
                cols("Model id", cx).into_any_element(),
                cols("Label", cx).into_any_element(),
                recur_ui::tooltip::titled(
                    "compat-ctx-tip",
                    "Shown in the chat context meter. Not sent to the server.",
                    cols("Context window", cx),
                )
                .into_any_element(),
                cols("Picker controls", cx).into_any_element(),
                div().into_any_element(),
            ));
        for (i, m) in rows_now.iter().enumerate() {
            let kind = self.controls_kind(m, i);
            let mid = self.bound(&format!("mid:{i}"), &m.id, FieldOpts::default(), window, cx);
            let ml = self.bound(
                &format!("mlabel:{i}"),
                &m.label,
                FieldOpts::default(),
                window,
                cx,
            );
            let ctx_v = if m.context_window > 0 {
                m.context_window.to_string()
            } else {
                String::new()
            };
            let mc = self.bound(
                &format!("mctx:{i}"),
                &ctx_v,
                FieldOpts::default(),
                window,
                cx,
            );
            let sel = self.select(
                &format!("ctl:{i}"),
                |window, cx| {
                    Select::new(
                        CONTROL_PRESETS
                            .iter()
                            .map(|(v, l)| SelectOption::new(*v, *l))
                            .collect(),
                        window,
                        cx,
                    )
                    .label("Picker controls")
                    .trigger_style(TriggerStyle {
                        height: Some(36.),
                        ..Default::default()
                    })
                },
                move |this, v, cx| this.apply_flags(i, v, cx),
                window,
                cx,
            );
            sel.update(cx, |s, cx| {
                if s.value() != kind {
                    s.set_value(kind, cx)
                }
            });
            let disabled = n_rows <= 1 || (i < saved_n && saved_n <= 1);
            let mut block = div().flex().flex_col().gap(px(8.)).child(model_grid(
                format!("compat-model:{i}"),
                cell(
                    "Model id".into(),
                    &mid,
                    "model-id",
                    InpStyle::COMPAT,
                    window,
                    cx,
                )
                .flex_1()
                .into_any_element(),
                cell(
                    "Model label".into(),
                    &ml,
                    "Label",
                    InpStyle::COMPAT,
                    window,
                    cx,
                )
                .flex_1()
                .into_any_element(),
                cell(
                    "Context window tokens".into(),
                    &mc,
                    "128000",
                    InpStyle::COMPAT,
                    window,
                    cx,
                )
                .flex_1()
                .into_any_element(),
                sel.into_any_element(),
                rm_btn(
                    SharedString::from(format!("mrm-{i}")),
                    IconName::Trash,
                    disabled,
                    cx,
                )
                .debug_selector(move || {
                    format!("btn:Remove model {i}{}", if disabled { ":off" } else { "" })
                })
                .when(!disabled, |d| {
                    d.on_click(cx.guarded_listener(move |this, _, _, cx| this.remove_model(i, cx)))
                })
                .into_any_element(),
            ));
            if !kind.is_empty() && kind != "custom" {
                block = block.child(self.flag_summary(m, window, cx));
            } else if kind == "custom" {
                block = block.child(self.flag_editor(i, m, window, cx));
            }
            models = models.child(block);
        }
        models = models.child(
            div().flex().gap(px(8.)).mt(px(4.)).child(
                tbtn("compat-add-model", false, cx)
                    .debug_selector(|| "btn:Add model".into())
                    .child(tbtn_icon(IconName::Plus))
                    .child(recur_ui::style::button_label(
                        "Add model",
                        12.,
                        weight::MEDIUM,
                    ))
                    .on_click(cx.guarded_listener(|this, _, _, cx| {
                        this.drafts.push(CompatibleModel::default());
                        cx.notify();
                    })),
            ),
        );

        let ctx_rows = RowsCtx {
            sides: true,
            in_group: true,
            more: false,
        };
        rows(
            ctx_rows,
            vec![
                Row::stack()
                    .child(copy(
                        &format!("{id}-name"),
                        "Display name",
                        Some(vec![
                            span("Shown in model pickers. Slug "),
                            code(id.clone(), cx),
                            span(" prefixes model refs."),
                        ]),
                        cx,
                    ))
                    .child(
                        div().flex().w_full().child(
                            cell(
                                "Provider display name".into(),
                                &name,
                                "",
                                InpStyle::BASE,
                                window,
                                cx,
                            )
                            .flex_1(),
                        ),
                    ),
                Row::stack()
                    .child(copy(
                        &format!("{id}-url"),
                        "Base URL",
                        Some(vec![
                            span("Recur POSTs "),
                            code("{base}/v1/responses", cx),
                            span(". No trailing slash."),
                        ]),
                        cx,
                    ))
                    .child(
                        div().flex().w_full().child(
                            cell(
                                "Base URL".into(),
                                &url,
                                "https://api.openai.com",
                                InpStyle::BASE,
                                window,
                                cx,
                            )
                            .flex_1(),
                        ),
                    ),
                key_row,
                Row::stack()
                    .child(copy(
                        &format!("{id}-headers"),
                        "Extra headers",
                        Some(vec![span(
                            "Optional. Sent on every request to this endpoint.",
                        )]),
                        cx,
                    ))
                    .child(headers),
                Row::stack()
                    .child(copy(
                        &format!("{id}-models"),
                        "Models",
                        Some(vec![
                            span("Each id is stored as "),
                            code(format!("{id}/id"), cx),
                            span(". Slash in the id is allowed (OpenRouter)."),
                        ]),
                        cx,
                    ))
                    .child(models),
                Row::new()
                    .child(copy(
                        &format!("{id}-remove"),
                        "Remove provider",
                        Some(vec![span(
                            "Drops this endpoint and its models from pickers.",
                        )]),
                        cx,
                    ))
                    .child(
                        ctrl().child(
                            danger_btn("compat-remove", false, false, cx)
                                .debug_selector(|| "btn:Remove".into())
                                .hover(move |s| s.border_color(hsla(t.line_bright)))
                                .child(recur_ui::style::button_label("Remove", 12., weight::MEDIUM))
                                .on_click(
                                    cx.guarded_listener(|_, _, _, cx| cx.emit(CardEvent::Remove)),
                                ),
                        ),
                    ),
            ],
            window,
            cx,
        )
    }
}

impl Render for CompatibleProviderCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

impl CompatibleProviderCard {
    /// `.compat-flag-summary`: what a preset adds to the picker, one line per control.
    fn flag_summary(&self, m: &CompatibleModel, window: &Window, cx: &App) -> AnyElement {
        let t = tokens(cx);
        let courier = Family::Stack("Courier".into(), Vec::new());
        let lead = if m.flags.len() == 1 {
            "this dropdown"
        } else {
            "these dropdowns"
        };
        let names: Vec<Text> = m
            .flags
            .iter()
            .map(|d| {
                text(if d.label.is_empty() {
                    d.id.clone()
                } else {
                    d.label.clone()
                })
                .size(12.)
                .leading(1.45)
                .color(t.text)
                .nowrap()
            })
            .collect();
        let wire_spans = |d: &CompatibleFlagDim| -> Vec<Span> {
            if d.wire.is_empty() {
                vec![span("not sent")]
            } else {
                vec![span("sends "), span(d.wire.clone()).family(courier.clone())]
            }
        };
        let w_of = |tx: &Text| tx.measure(None, window).0;
        let name_w = names.iter().map(w_of).fold(0., f32::max);
        let wire_w = m
            .flags
            .iter()
            .map(|d| {
                w_of(
                    &Text::from_spans(wire_spans(d))
                        .size(12.)
                        .leading(1.45)
                        .nowrap(),
                )
            })
            .fold(0., f32::max);
        let mut grid = card(recur_ui::color::TRANSPARENT, t.line, 6.)
            .debug_selector(|| "compat-flag-summary".into())
            .flex()
            .flex_col()
            .gap(px(4.))
            .py(px(8.))
            .px(px(12.))
            .child(
                div().mb(px(2.)).child(
                    text(format!(
                        "Adds {lead} to the composer's model picker. Choose Custom… to edit."
                    ))
                    .id(SharedString::from(format!("sum-lead-{}", m.id)))
                    .size(12.)
                    .leading(1.45)
                    .color(t.muted),
                ),
            );
        for (di, (d, name)) in m.flags.iter().zip(names).enumerate() {
            let mut opts: Vec<Span> = Vec::new();
            for (oi, o) in d.options.iter().enumerate() {
                if oi > 0 {
                    opts.push(span(", "));
                }
                let mut s = span(format!(
                    "{}{}",
                    if o.label.is_empty() {
                        &o.value
                    } else {
                        &o.label
                    },
                    if o.default { " (default)" } else { "" }
                ));
                if o.default {
                    s = s.color(t.text);
                }
                opts.push(s);
            }
            let shown: String = opts.iter().map(|s| s.text.as_ref()).collect();
            grid = grid.child(
                div()
                    .flex()
                    .gap(px(16.))
                    .debug_selector(move || format!("compat-flag-opts:{shown}"))
                    .child(div().flex_none().w(px(name_w)).child(name))
                    .child(
                        div().flex_1().min_w_0().child(
                            Text::from_spans(opts)
                                .id(SharedString::from(format!("sum-opts-{}-{di}", m.id)))
                                .size(12.)
                                .leading(1.45)
                                .color(t.muted),
                        ),
                    )
                    .child(
                        div().flex_none().w(px(wire_w)).child(
                            Text::from_spans(wire_spans(d))
                                .size(12.)
                                .leading(1.45)
                                .color(t.muted)
                                .nowrap(),
                        ),
                    ),
            );
        }
        grid.into_any_element()
    }

    /// `.compat-flags`: the custom control editor.
    fn flag_editor(
        &mut self,
        i: usize,
        m: &CompatibleModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = tokens(cx);
        let mut col = div().flex().flex_col().gap(px(8.)).pl(px(12.)).child(
            crate::style::desc_text(
                SharedString::from(format!("flags-desc-{i}")),
                vec![span(
                    "Each control is a dropdown in the composer's model picker. The chosen value is sent as the request field.",
                )],
                cx,
            ),
        );
        for (di, d) in m.flags.iter().enumerate() {
            let label = self.bound(
                &format!("fl:{i}:{di}"),
                &d.label,
                FieldOpts::default(),
                window,
                cx,
            );
            let wire = self.select(
                &format!("wire:{i}:{di}"),
                |window, cx| {
                    let mut opts = vec![SelectOption::new("", "(not sent)")];
                    opts.extend(COMPATIBLE_WIRES.iter().map(|w| SelectOption::new(*w, *w)));
                    Select::new(opts, window, cx)
                        .label("Request field")
                        .size(SelectSize::Sm)
                        .trigger_style(TriggerStyle {
                            mono_font: true,
                            ..Default::default()
                        })
                },
                move |this, v, cx| this.set_flag(i, di, move |d| d.wire = v.clone(), cx),
                window,
                cx,
            );
            let w = d.wire.clone();
            wire.update(cx, |s, cx| {
                if *s.value() != w {
                    s.set_value(w, cx)
                }
            });
            let mut opts_grid = div().flex().flex_col().gap(px(4.)).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().flex_none().w(px(52.)).child(cols("Default", cx)))
                    .child(div().flex_1().min_w_0().child(cols("Value sent", cx)))
                    .child(div().flex_1().min_w_0().child(cols("Shown as", cx)))
                    .child(div().flex_none().w(px(28.))),
            );
            for (oi, o) in d.options.iter().enumerate() {
                let vi = self.bound(
                    &format!("ov:{i}:{di}:{oi}"),
                    &o.value,
                    FieldOpts::default(),
                    window,
                    cx,
                );
                let li = self.bound(
                    &format!("ol:{i}:{di}:{oi}"),
                    &o.label,
                    FieldOpts::default(),
                    window,
                    cx,
                );
                let on = o.default;
                let nm = if o.label.is_empty() {
                    o.value.clone()
                } else {
                    o.label.clone()
                };
                let dot = div()
                    .id(SharedString::from(format!("def-{i}-{di}-{oi}")))
                    .debug_selector(move || format!("compat-default:{di}:Default {nm}:{on}"))
                    .size(px(20.))
                    .rounded_full()
                    .border(px(1.))
                    .border_color(hsla(if on { t.amber } else { t.line_bright }))
                    .when(on, |d| d.bg(hsla(t.amber)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(hsla(if on {
                        t.bg
                    } else {
                        recur_ui::color::TRANSPARENT
                    }))
                    .when(!on, |d| d.hover(move |s| s.text_color(hsla(t.muted))))
                    .child(icon(IconName::Check).stroke_width(3.).size(px(12.)))
                    .on_click(cx.guarded_listener(move |this, _, _, cx| {
                        this.set_option(i, di, oi, move |o| o.default = !on, cx)
                    }));
                opts_grid = opts_grid.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .flex_none()
                                .w(px(52.))
                                .flex()
                                .justify_center()
                                .child(dot),
                        )
                        .child(flex_share(
                            1.,
                            cell(
                                "Option value".into(),
                                &vi,
                                "value",
                                InpStyle::FLAG,
                                window,
                                cx,
                            )
                            .flex_1(),
                        ))
                        .child(flex_share(
                            1.,
                            cell(
                                "Option label".into(),
                                &li,
                                "Label",
                                InpStyle::FLAG,
                                window,
                                cx,
                            )
                            .flex_1(),
                        ))
                        .child(
                            rm_btn(
                                SharedString::from(format!("orm-{i}-{di}-{oi}")),
                                IconName::Trash,
                                false,
                                cx,
                            )
                            .debug_selector(move || format!("btn:Remove option {oi}"))
                            .on_click(cx.guarded_listener(
                                move |this, _, _, cx| this.remove_option(i, di, oi, cx),
                            )),
                        ),
                );
            }
            let flag_card = card(recur_ui::color::TRANSPARENT, t.line, 6.)
                .debug_selector(move || format!("compat-flag-card:{di}"))
                .flex()
                .flex_col()
                .gap(px(8.))
                .p(px(10.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(flex_share(
                            1.,
                            cell(
                                "Control name".into(),
                                &label,
                                "Control name",
                                InpStyle {
                                    family_ui: true,
                                    weight: weight::SEMIBOLD,
                                    ..InpStyle::FLAG
                                },
                                window,
                                cx,
                            )
                            .flex_1(),
                        ))
                        .child(div().flex_none().child(cols("sent as", cx)))
                        .child(flex_share(1., div().flex_1().min_w_0().child(wire)))
                        .child(
                            rm_btn(
                                SharedString::from(format!("frm-{i}-{di}")),
                                IconName::Trash,
                                false,
                                cx,
                            )
                            .debug_selector(move || format!("btn:Remove control {di}"))
                            .on_click(cx.guarded_listener(
                                move |this, _, _, cx| this.remove_flag(i, di, cx),
                            )),
                        ),
                )
                .child(opts_grid)
                .child(
                    div().flex().child(
                        tbtn(SharedString::from(format!("addopt-{i}-{di}")), false, cx)
                            .debug_selector(move || format!("btn:Add option {di}"))
                            .child(tbtn_icon(IconName::Plus))
                            .child(recur_ui::style::button_label(
                                "Add option",
                                12.,
                                weight::MEDIUM,
                            ))
                            .on_click(cx.guarded_listener(move |this, _, _, cx| {
                                this.add_option(i, di, cx)
                            })),
                    ),
                );
            col = col.child(flag_card);
        }
        col = col.child(
            div().flex().child(
                tbtn(SharedString::from(format!("addflag-{i}")), false, cx)
                    .debug_selector(|| "btn:Add control".into())
                    .child(tbtn_icon(IconName::Plus))
                    .child(recur_ui::style::button_label(
                        "Add control",
                        12.,
                        weight::MEDIUM,
                    ))
                    .on_click(cx.guarded_listener(move |this, _, _, cx| this.add_flag(i, cx))),
            ),
        );
        let _ = LineHeight::Normal;
        col.into_any_element()
    }
}
