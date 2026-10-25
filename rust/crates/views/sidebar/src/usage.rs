//! The sidebar footer's provider usage meter (`ProviderUsageMeter.svelte`) and its rules
//! (`lib/providerUsage.ts`): one ring chip per provider with session quota, and a hover /
//! click popover with every window, reset time and Cursor model bucket.

use recur_ui::guard::Guarded;
use recur_view_core::{ObserveState as _, Topics};
use std::time::{Duration, Instant};

use gpui_kit::{
    AnyElement, App, Bounds, Context, Entity, FocusHandle, Hsla, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    Task, Window, canvas, deferred, div, prelude::FluentBuilder, px,
};
use recur_services::api::{ProviderQuota, Snapshot};
use recur_ui::color::Color;
use recur_ui::fonts::weight;
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{Theme, color::rgba, provider_icon, svg_markup};
use recur_view_core::AppState;
use recur_view_core::format::format_compact_count;

/// Extra / credit balances are overage, not the session chip.
pub fn is_chip_window(w: &recur_services::api::Window) -> bool {
    w.id != "extra" && w.used_pct.is_finite()
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChipRing {
    pub id: String,
    pub label: String,
    pub pct: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderChip {
    pub id: String,
    pub name: String,
    pub rings: Vec<ChipRing>,
    pub headline: f64,
}

const CHIP_ORDER: [&str; 3] = ["claude-code", "cursor", "codex"];

fn ring_rank(id: &str) -> u8 {
    match id {
        "five_hour" => 0,
        "seven_day" => 1,
        _ if id.starts_with("seven_day_") => 2,
        "billing" => 3,
        _ => 4,
    }
}

/// Cursor usage is Recur Connect only: a row with no account is hidden.
pub fn cursor_usage_visible(p: &ProviderQuota) -> bool {
    p.id != "cursor" || !p.email.is_empty() || !p.plan.is_empty() || p.source_id == "cursor_connect"
}

/// Providers off in Settings stay out of the meter and popover.
pub fn usage_provider_on(id: &str, disabled: &[String]) -> bool {
    !id.is_empty() && !disabled.iter().any(|d| d == id)
}

fn providers(s: Option<&Snapshot>) -> &[ProviderQuota] {
    s.and_then(|s| s.providers.as_deref()).unwrap_or(&[])
}

/// `providerChips`: one chip per provider that has session quota.
pub fn provider_chips(s: Option<&Snapshot>, disabled: &[String]) -> Vec<ProviderChip> {
    let mut out: Vec<ProviderChip> = providers(s)
        .iter()
        .filter(|p| usage_provider_on(&p.id, disabled) && cursor_usage_visible(p))
        .filter_map(|p| {
            let mut rings: Vec<ChipRing> = p
                .windows
                .iter()
                .filter(|w| is_chip_window(w))
                .map(|w| ChipRing {
                    id: w.id.clone(),
                    label: w.label.clone(),
                    pct: w.used_pct,
                })
                .collect();
            rings.sort_by(|a, b| {
                ring_rank(&a.id)
                    .cmp(&ring_rank(&b.id))
                    .then_with(|| a.id.cmp(&b.id))
            });
            (!rings.is_empty()).then(|| ProviderChip {
                id: p.id.clone(),
                name: p.name.clone(),
                headline: rings
                    .iter()
                    .map(|r| r.pct)
                    .fold(f64::NEG_INFINITY, f64::max),
                rings,
            })
        })
        .collect();
    let rank = |id: &str| CHIP_ORDER.iter().position(|x| *x == id).unwrap_or(99);
    out.sort_by_key(|c| rank(&c.id));
    out
}

pub fn snapshot_has_data(s: Option<&Snapshot>, disabled: &[String]) -> bool {
    providers(s).iter().any(|p| {
        usage_provider_on(&p.id, disabled)
            && (!p.windows.is_empty()
                || !p.error.is_empty()
                || p.connecting
                || !p.hint.is_empty()
                || !p.source_id.is_empty())
    })
}

/// True when an enabled Claude row can recover through `claude auth status`.
pub fn snapshot_has_provider_error(s: Option<&Snapshot>, disabled: &[String]) -> bool {
    providers(s).iter().any(|p| {
        usage_provider_on(&p.id, disabled)
            && p.id == "claude-code"
            && (p.error == "Claude Code login expired" || p.error == "Not signed in to Claude Code")
    })
}

/// `formatResetIn`.
pub fn format_reset_in(ms: i64, now: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    let sec = ((ms - now) as f64 / 1000.).round().max(0.) as i64;
    if sec < 60 {
        "resets now".into()
    } else if sec < 3600 {
        format!("resets in {}m", sec / 60)
    } else if sec < 86400 {
        format!("resets in {}h", sec / 3600)
    } else {
        format!("resets in {}d", sec / 86400)
    }
}

/// `formatUsagePct`: one decimal like the dashboard.
pub fn format_usage_pct(n: f64) -> String {
    if !n.is_finite() {
        return "0.0%".into();
    }
    format!("{:.1}%", (n * 10.).round() / 10.)
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelBucket {
    pub id: &'static str,
    pub label: &'static str,
    pub tokens: i64,
    pub used_pct: f64,
    pub models: Vec<recur_services::api::ModelRow>,
}

/// `modelBuckets`: Cursor's aggregation grouped the way the dashboard does.
pub fn model_buckets(models: &[recur_services::api::ModelRow]) -> [ModelBucket; 2] {
    let mut cursor = ModelBucket {
        id: "cursor",
        label: "Cursor Models",
        tokens: 0,
        used_pct: 0.,
        models: Vec::new(),
    };
    let mut other = ModelBucket {
        id: "other",
        label: "Other Models",
        tokens: 0,
        used_pct: 0.,
        models: Vec::new(),
    };
    for m in models {
        let b = if m.bucket == "other" {
            &mut other
        } else {
            &mut cursor
        };
        b.tokens += m.tokens;
        b.used_pct += m.used_pct;
        b.models.push(m.clone());
    }
    [cursor, other]
}

/// `ringColor`.
pub fn ring_color(pct: f64, cx: &App) -> Color {
    let t = Theme::of(cx).tokens;
    if pct >= 90. {
        t.coral
    } else if pct >= 70. {
        t.amber
    } else {
        t.muted
    }
}

pub fn can_disconnect_cursor(s: Option<&Snapshot>) -> bool {
    providers(s)
        .iter()
        .find(|p| p.id == "cursor")
        .is_some_and(|p| p.source_id == "cursor_connect")
}

const RING_SIZE: f32 = 16.;
const HOVER_OPEN: Duration = Duration::from_millis(160);
const HOVER_CLOSE: Duration = Duration::from_millis(120);
/// `.use-fill { transition: stroke-dashoffset .25s ease }`.
const RING_TWEEN: Duration = Duration::from_millis(250);

/// One ring's stroke width, radius and circumference (`ringGeom`).
fn ring_geom(count: usize, i: usize) -> (f32, f32, f32) {
    let n = count.max(1) as f32;
    let sw = if count <= 1 { 2.2 } else { 1.55 };
    let gap = 0.7;
    let r = RING_SIZE / 2. - sw / 2. - (n - 1. - i as f32) * (sw + gap);
    (sw, r, 2. * std::f32::consts::PI * r)
}

/// The rings as SVG markup, colors resolved for the theme.
fn rings_svg(rings: &[(f64, Color)], line: Color) -> String {
    let c = RING_SIZE / 2.;
    let mut body = String::new();
    for (i, (pct, color)) in rings.iter().enumerate() {
        let (sw, r, circ) = ring_geom(rings.len(), i);
        let offset = circ * (1. - (pct / 100.).min(1.) as f32);
        body.push_str(&format!(
            r#"<circle cx="{c}" cy="{c}" r="{r}" fill="none" stroke="{}" stroke-width="{sw}"/><circle cx="{c}" cy="{c}" r="{r}" fill="none" stroke="{}" stroke-width="{sw}" stroke-linecap="round" stroke-dasharray="{circ}" stroke-dashoffset="{offset}" transform="rotate(-90 {c} {c})"/>"#,
            line.to_css(),
            color.to_css(),
        ));
    }
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{RING_SIZE}" height="{RING_SIZE}" viewBox="0 0 {RING_SIZE} {RING_SIZE}">{body}</svg>"#
    )
}

/// The meter (`.use-meter`): the chip button and its popover.
pub struct UsageMeter {
    state: Entity<AppState>,
    open: bool,
    hover_timer: Option<Task<()>>,
    pop_max: f32,
    bounds: Option<Bounds<Pixels>>,
    focus: FocusHandle,
    /// Shown ring percents and when each began tweening toward its value.
    rings: Vec<(String, f64, f64, Instant)>,
}

impl UsageMeter {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe_state(&state, Topics::USAGE, |_, _, cx| cx.notify())
            .detach();
        // `onMount`: fill the meter after first paint.
        state.update(cx, |s, cx| s.prefetch_provider_usage(cx).detach());
        Self {
            state,
            open: false,
            hover_timer: None,
            pop_max: 0.,
            bounds: None,
            focus: cx.focus_handle(),
            rings: Vec::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// `fitPop`: the popover may grow up to 12px below the window top.
    fn fit_pop(&mut self) {
        if let Some(b) = self.bounds {
            self.pop_max = (f32::from(b.origin.y) - 12.).floor().max(160.);
        }
    }

    fn on_enter(&mut self, cx: &mut Context<Self>) {
        self.hover_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_OPEN).await;
            let _ = this.update(cx, |m, cx| {
                m.open = true;
                m.fit_pop();
                cx.notify();
            });
        }));
    }

    fn on_leave(&mut self, cx: &mut Context<Self>) {
        self.hover_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_CLOSE).await;
            let _ = this.update(cx, |m, cx| {
                m.open = false;
                cx.notify();
            });
        }));
    }

    /// A click: hover already opens without fetching, so a click while closed (or with no
    /// snapshot yet) opens and collects; a recoverable Claude error forces a fresh collect.
    fn toggle(&mut self, cx: &mut Context<Self>) {
        self.hover_timer = None;
        let (has_snap, error) = {
            let s = self.state.read(cx);
            let disabled = s
                .settings
                .as_ref()
                .map(|x| x.disabled_providers.clone())
                .unwrap_or_default();
            (
                s.provider_usage.is_some(),
                snapshot_has_provider_error(s.provider_usage.as_ref(), &disabled),
            )
        };
        if !self.open || !has_snap {
            self.open = true;
            self.fit_pop();
            self.state.update(cx, |s, cx| {
                if error {
                    s.refresh_provider_usage(cx).detach();
                } else {
                    s.prefetch_provider_usage(cx).detach();
                }
            });
        } else {
            self.open = false;
        }
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            cx.notify();
        }
    }

    /// A ring's shown percent, tweened toward `pct`.
    fn tweened(&mut self, key: &str, pct: f64, cx: &App) -> f64 {
        let now = Instant::now();
        let slot = match self.rings.iter_mut().find(|r| r.0 == key) {
            Some(slot) => slot,
            None => {
                self.rings.push((key.to_string(), pct, pct, now));
                return pct;
            }
        };
        let p = if recur_ui::motion::reduced(cx) {
            1.
        } else {
            recur_ui::motion::eased(slot.3, RING_TWEEN, recur_ui::motion::Easing::EASE, cx) as f64
        };
        let shown = slot.1 + (slot.2 - slot.1) * p;
        if slot.2 != pct {
            *slot = (key.to_string(), shown, pct, now);
            return shown;
        }
        shown
    }

    fn popover(
        &self,
        rows: &[ProviderQuota],
        snap: Option<&Snapshot>,
        disabled: &[String],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let stale = snap.is_some_and(|s| s.stale);
        let has = snapshot_has_data(snap, disabled);
        let small = |s: &str, color: Color| text(s.to_string()).size(12.).leading(1.4).color(color);
        let mut body = div().flex().flex_col();
        body = body.child(
            div()
                .flex()
                .items_baseline()
                .justify_between()
                .gap(px(10.))
                .mb(px(8.))
                .child(
                    text("Usage")
                        .size(12.)
                        .weight(weight::SEMIBOLD)
                        .color(t.text),
                )
                .when(stale, |d| {
                    d.child(text("Last known").mono().size(11.).color(t.muted))
                }),
        );
        if snap.is_none() {
            body = body.child(small("Loading usage…", t.muted));
        } else if !has {
            body = body.child(small(
                "No plan quotas yet. Sign in to Claude Code, Codex (ChatGPT), or Connect Cursor.",
                t.muted,
            ));
        } else {
            let mut list = div().flex().flex_col().gap(px(14.));
            for (i, p) in rows.iter().enumerate() {
                let mut prov = div().flex().flex_col();
                if i > 0 {
                    prov = prov
                        .pt(px(14.))
                        .border_t(px(1.))
                        .border_color(Hsla::from(t.line));
                }
                let plan = if p.email.is_empty() {
                    &p.plan
                } else {
                    &p.email
                };
                prov = prov.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(7.))
                                .min_w_0()
                                .child(provider_icon(&p.id).size(px(13.)).color(t.text))
                                .child(
                                    text(p.name.clone())
                                        .size(12.)
                                        .weight(weight::SEMIBOLD)
                                        .color(t.text),
                                ),
                        )
                        .when(!plan.is_empty(), |d| {
                            d.child(
                                div().min_w_0().max_w(gpui_kit::relative(0.58)).child(
                                    text(plan.clone())
                                        .mono()
                                        .size(10.)
                                        .color(t.muted)
                                        .truncate()
                                        .align(recur_ui::text::Align::Right),
                                ),
                            )
                        }),
                );
                if p.connecting {
                    prov = prov.child(
                        div()
                            .mt(px(2.))
                            .child(small("Waiting for browser sign-in…", t.faint)),
                    );
                } else if p.id == "cursor"
                    && !can_disconnect_cursor(snap)
                    && (!p.error.is_empty() || !cursor_usage_visible(p))
                {
                    let state = self.state.clone();
                    prov = prov.child(
                        div()
                            .id("use-cursor-link")
                            .mt(px(8.))
                            .cursor_pointer()
                            .on_click(cx.guarded_listener(move |m, _, _, cx| {
                                m.open = false;
                                state.update(cx, |s, cx| s.open_settings(Some("agent"), cx));
                                cx.notify();
                            }))
                            .child(recur_ui::style::lnk("Connect Cursor in Settings", cx)),
                    );
                } else if !p.error.is_empty() {
                    prov = prov.child(div().mt(px(4.)).child(small(&p.error, t.coral)));
                    if !p.hint.is_empty() {
                        prov = prov.child(div().mt(px(2.)).child(small(&p.hint, t.faint)));
                    }
                } else if p.windows.is_empty() && !p.hint.is_empty() {
                    prov = prov.child(div().mt(px(2.)).child(small(&p.hint, t.faint)));
                } else {
                    for w in &p.windows {
                        let extra_detail = w.id == "extra" && !w.detail.is_empty();
                        let value = if extra_detail {
                            w.detail.clone()
                        } else {
                            format_usage_pct(w.used_pct)
                        };
                        let mut win = div().mt(px(6.)).child(
                            div()
                                .flex()
                                .justify_between()
                                .gap(px(10.))
                                .child(text(w.label.clone()).size(12.).color(t.muted))
                                .child(text(value).mono().size(11.).color(t.text)),
                        );
                        if w.detail.is_empty() || w.id != "extra" {
                            let fill = ring_color(w.used_pct, cx);
                            win = win.child(
                                div()
                                    .mt(px(4.))
                                    .h(px(4.))
                                    .rounded(px(2.))
                                    .bg(t.line)
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded(px(2.))
                                            .w(gpui_kit::relative(
                                                (w.used_pct.min(100.) / 100.) as f32,
                                            ))
                                            .bg(fill),
                                    ),
                            );
                        }
                        if !w.detail.is_empty() && w.id != "extra" {
                            win = win.child(div().mt(px(2.)).child(small(&w.detail, t.faint)));
                        }
                        if w.resets_at != 0 {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as i64)
                                .unwrap_or_default();
                            win = win.child(
                                div()
                                    .mt(px(2.))
                                    .child(small(&format_reset_in(w.resets_at, now), t.faint)),
                            );
                        }
                        prov = prov.child(win);
                    }
                    if !p.models.is_empty() {
                        let mut models = div().mt(px(6.)).flex().flex_col().gap(px(3.));
                        for (bi, b) in model_buckets(&p.models).into_iter().enumerate() {
                            models = models.child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .gap(px(10.))
                                    .when(bi > 0, |d| d.mt(px(6.)))
                                    .child(
                                        text(b.label)
                                            .size(12.)
                                            .weight(weight::SEMIBOLD)
                                            .color(t.text),
                                    )
                                    .child(
                                        text(format!(
                                            "{} · {}",
                                            format_compact_count(b.tokens as f64),
                                            format_usage_pct(b.used_pct)
                                        ))
                                        .mono()
                                        .size(11.)
                                        .color(t.text),
                                    ),
                            );
                            for m in b.models {
                                models = models.child(
                                    div()
                                        .flex()
                                        .justify_between()
                                        .gap(px(10.))
                                        .child(
                                            div().min_w_0().child(
                                                text(m.name.clone())
                                                    .size(12.)
                                                    .color(t.muted)
                                                    .truncate(),
                                            ),
                                        )
                                        .child(
                                            text(format!(
                                                "{} · {}",
                                                format_compact_count(m.tokens as f64),
                                                format_usage_pct(m.used_pct)
                                            ))
                                            .mono()
                                            .size(11.)
                                            .color(t.text),
                                        ),
                                );
                            }
                        }
                        prov = prov.child(models);
                    }
                }
                list = list.child(prov);
            }
            body = body.child(list);
        }
        let max_h = if self.pop_max > 0. {
            self.pop_max
        } else {
            10_000.
        };
        deferred(
            div()
                .absolute()
                .bottom(px(26. + 8.))
                .left_0()
                .child(with_shadow(
                    recur_ui::style::card(t.pane_2, t.line_bright, 9.)
                        .id("use-pop")
                        .w(px(300.))
                        .max_h(px(max_h))
                        .overflow_y_scroll()
                        .py(px(11.))
                        .px(px(12.))
                        .occlude()
                        .child(body),
                    &[css_box_shadow(0., 12., 32., 0., rgba(0, 0, 0, 0.45))],
                    9.,
                )),
        )
        .with_priority(2)
        .into_any_element()
    }
}

impl UsageMeter {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let (snap, disabled) = {
            let s = self.state.read(cx);
            (
                s.provider_usage.clone(),
                s.settings
                    .as_ref()
                    .map(|x| x.disabled_providers.clone())
                    .unwrap_or_default(),
            )
        };
        let chips = provider_chips(snap.as_ref(), &disabled);
        let rows: Vec<ProviderQuota> = providers(snap.as_ref())
            .iter()
            .filter(|p| usage_provider_on(&p.id, &disabled))
            .cloned()
            .collect();
        let label: SharedString = if !chips.is_empty() {
            chips
                .iter()
                .map(|c| {
                    let bits: Vec<String> = c
                        .rings
                        .iter()
                        .map(|r| format!("{} {}%", r.label, r.pct.round()))
                        .collect();
                    if bits.is_empty() {
                        format!("{} usage", c.name)
                    } else {
                        format!("{} usage: {}", c.name, bits.join(", "))
                    }
                })
                .collect::<Vec<_>>()
                .join(". ")
                .into()
        } else if snap.is_none() {
            "Provider usage — loading".into()
        } else {
            "Provider usage — no plan data yet".into()
        };
        let open = self.open;
        let mut btn = div()
            .id("use-btn")
            .track_focus(&self.focus)
            .role(gpui_kit::Role::Button)
            .aria_label(label)
            .aria_expanded(open)
            .flex()
            .items_center()
            .gap(px(7.))
            .h(px(26.))
            .px(px(5.))
            .rounded(px(6.))
            .cursor_pointer()
            .text_color(if open { t.muted } else { t.faint })
            .when(open, |d| d.bg(t.pane))
            .hover(move |s| s.bg(t.pane).text_color(Hsla::from(t.muted)))
            .on_click(cx.guarded_listener(|m, _, _, cx| m.toggle(cx)));
        if recur_ui::style::is_focus_visible(&self.focus, window) {
            // `.use-btn:focus-visible { box-shadow: 0 0 0 2px var(--amber-d) }`.
            btn = btn.shadow(vec![recur_ui::style::css_shadow(0., 0., 0., 2., t.amber_d)]);
        }
        if chips.is_empty() {
            btn = btn.child(
                div().min_w(px(16.)).flex().justify_center().child(
                    text("—")
                        .mono()
                        .size(10.)
                        .line_height(LineHeight::Scale(1.)),
                ),
            );
        } else {
            let mut animating = false;
            for chip in &chips {
                let rings: Vec<(f64, Color)> = chip
                    .rings
                    .iter()
                    .map(|r| {
                        let key = format!("{}:{}", chip.id, r.id);
                        let shown = self.tweened(&key, r.pct, cx);
                        animating |= (shown - r.pct).abs() > f64::EPSILON;
                        (shown, ring_color(r.pct, cx))
                    })
                    .collect();
                btn = btn.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.))
                        .child(provider_icon(&chip.id).size(px(11.)).color(t.faint))
                        // `.rail-foot-bar svg { width: 14px }` outranks the 16px attributes.
                        .child(svg_markup(rings_svg(&rings, t.line)).size(px(14.))),
                );
            }
            if animating && !recur_ui::motion::is_frozen(cx) {
                window.request_animation_frame();
            }
        }
        let state_bounds = cx.entity();
        div()
            .id("use-meter")
            .relative()
            .flex_none()
            .flex()
            .items_center()
            .h(px(26.))
            .on_hover(cx.guarded_listener(|m, hovered: &bool, _, cx| {
                if *hovered {
                    m.on_enter(cx);
                } else {
                    m.on_leave(cx);
                }
            }))
            .on_mouse_down_out(cx.guarded_listener(|m, _, _, cx| m.close(cx)))
            .on_key_down(
                cx.guarded_listener(|m, ev: &gpui_kit::KeyDownEvent, _, cx| {
                    if ev.keystroke.key == "escape" && m.open {
                        m.close(cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .child(
                canvas(
                    move |bounds, _, cx| {
                        state_bounds.update(cx, |m, _| m.bounds = Some(bounds));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(btn)
            .when(open, |d| {
                d.child(self.popover(&rows, snap.as_ref(), &disabled, cx))
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    }
}

impl Render for UsageMeter {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_services::api::{ModelRow, Window as Win};

    fn w(id: &str, label: &str, pct: f64) -> Win {
        Win {
            id: id.into(),
            label: label.into(),
            used_pct: pct,
            ..Win::default()
        }
    }

    fn snap(providers: Vec<ProviderQuota>) -> Snapshot {
        Snapshot {
            providers: Some(providers),
            ..Snapshot::default()
        }
    }

    fn p(id: &str, f: impl FnOnce(&mut ProviderQuota)) -> ProviderQuota {
        let mut q = ProviderQuota {
            id: id.into(),
            name: id.into(),
            ..ProviderQuota::default()
        };
        f(&mut q);
        q
    }

    #[test]
    fn chips_stack_session_rings_and_skip_extra() {
        let s = snap(vec![
            p("claude-code", |q| {
                q.windows = vec![
                    w("five_hour", "5-hour", 0.),
                    w("seven_day", "7-day", 34.),
                    w("seven_day_fable", "Fable (7-day)", 12.),
                    w("extra", "Extra", 100.),
                ]
            }),
            p("cursor", |q| {
                q.email = "dev@example.com".into();
                q.windows = vec![w("billing", "Included usage", 27.4)];
            }),
        ]);
        let chips = provider_chips(Some(&s), &[]);
        assert_eq!(
            chips.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["claude-code", "cursor"]
        );
        assert_eq!(
            chips[0]
                .rings
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["five_hour", "seven_day", "seven_day_fable"]
        );
        assert_eq!(chips[0].headline, 34.);
        assert_eq!(chips[1].headline, 27.4);
        let codex = snap(vec![p("codex", |q| {
            q.windows = vec![
                w("five_hour", "5-hour", 100.),
                w("seven_day", "7-day", 16.),
                w("billing", "Monthly", 100.),
                w("extra", "Credits", 0.),
            ]
        })]);
        let chips = provider_chips(Some(&codex), &[]);
        assert_eq!(chips[0].rings.len(), 3);
        assert_eq!(chips[0].rings[2].id, "billing");
        let off = provider_chips(Some(&s), &["cursor".into()]);
        assert_eq!(off.len(), 1);
    }

    #[test]
    fn cursor_visibility_and_data() {
        assert!(!cursor_usage_visible(&p("cursor", |_| {})));
        assert!(cursor_usage_visible(
            &p("cursor", |q| q.email = "a@b.c".into())
        ));
        assert!(cursor_usage_visible(
            &p("cursor", |q| q.source_id = "cursor_connect".into())
        ));
        assert!(cursor_usage_visible(&p("claude-code", |_| {})));
        assert!(snapshot_has_data(
            Some(&snap(vec![p("cursor", |q| q.error = "no".into())])),
            &[]
        ));
        let hinted = snap(vec![p("cursor", |q| {
            q.source_id = "cursor_connect".into();
            q.hint = "off".into();
        })]);
        assert!(snapshot_has_data(Some(&hinted), &[]));
        assert!(!snapshot_has_data(Some(&snap(vec![])), &[]));
        assert!(!snapshot_has_data(Some(&hinted), &["cursor".into()]));
    }

    #[test]
    fn provider_error() {
        assert!(!snapshot_has_provider_error(None, &[]));
        let err = |id: &str, e: &str| snap(vec![p(id, |q| q.error = e.into())]);
        assert!(snapshot_has_provider_error(
            Some(&err("claude-code", "Claude Code login expired")),
            &[]
        ));
        assert!(!snapshot_has_provider_error(
            Some(&err("cursor", "No Cursor usage session")),
            &[]
        ));
        assert!(!snapshot_has_provider_error(
            Some(&err("codex", "Not signed in to Codex")),
            &[]
        ));
        assert!(!snapshot_has_provider_error(
            Some(&err("claude-code", "Claude Code login expired")),
            &["claude-code".into()]
        ));
        assert!(!snapshot_has_provider_error(
            Some(&err("claude-code", "Usage endpoint is rate limited")),
            &[]
        ));
    }

    #[test]
    fn formatting() {
        let now = 1_787_227_200_000;
        assert_eq!(format_reset_in(now + 90_000, now), "resets in 1m");
        assert_eq!(format_reset_in(now + 3 * 3_600_000, now), "resets in 3h");
        assert_eq!(format_usage_pct(27.14), "27.1%");
        assert_eq!(format_usage_pct(0.), "0.0%");
        assert!(can_disconnect_cursor(Some(&snap(vec![p("cursor", |q| {
            q.source_id = "cursor_connect".into()
        })]))));
        assert!(!can_disconnect_cursor(Some(&snap(vec![p(
            "cursor",
            |_| {}
        )]))));
    }

    #[test]
    fn buckets() {
        let m = |name: &str, tokens: i64, pct: f64, bucket: &str| ModelRow {
            name: name.into(),
            tokens,
            used_pct: pct,
            bucket: bucket.into(),
        };
        let [cursor, other] = model_buckets(&[
            m("cursor-grok-4.6-high-fast", 590_200_000, 22.7, "cursor"),
            m("claude-4.6-opus", 12_000, 1.1, "other"),
            m("composer-2.5-fast", 4_300_000, 0.1, "cursor"),
        ]);
        assert_eq!(cursor.label, "Cursor Models");
        assert_eq!(cursor.tokens, 594_500_000);
        assert!((cursor.used_pct - 22.8).abs() < 1e-9);
        assert_eq!(cursor.models.len(), 2);
        assert_eq!(other.label, "Other Models");
        assert_eq!(other.tokens, 12_000);
    }
}
