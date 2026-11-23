//! The Insights page: the stat row, the agent hours heatmap, where the time went, models,
//! commands and the key/value lists, as plain sections under small uppercase headings.

use std::rc::Rc;

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, Context, Div, Entity, Hsla, InteractiveElement, IntoElement, ParentElement, Render,
    ScrollHandle, SharedString, Styled, Window, canvas, div, point, px, relative, size,
};
use recur_services::api::{CommandStats, InsightsReport, ModelStats};
use recur_ui::guard::Guarded;
use recur_ui::text::{LineHeight, Text, TextStyled, span, text};
use recur_ui::tooltip::titled;
use recur_ui::{Color, Theme, Tokens};

use crate::{Insights, Loaded, Period};

const HOUR: i64 = 3_600_000;
/// The heatmap's 15px cells, 3px apart.
const CELL: f32 = 15.;
const STEP: f32 = 18.;
const WEEKS: usize = 53;
const MODEL_ROWS: usize = 4;
const COMMAND_ROWS: usize = 6;
/// A failure rate at or past this many percent shows coral.
const HIGH_FAILURE: f64 = 5.;
const DASH: &str = "—";

fn h(c: Color) -> Hsla {
    c.into()
}

/// `1,204`.
pub fn count(n: i64) -> String {
    let s = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

fn one_decimal(x: f64) -> String {
    let s = format!("{x:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

/// Agent hours: `236h`, `10.6h`, `0.5h`.
pub fn hours(ms: i64) -> String {
    let h = ms as f64 / HOUR as f64;
    if h >= 100. {
        format!("{}h", count(h.round() as i64))
    } else {
        format!("{}h", one_decimal(h))
    }
}

/// A duration: `9h 12m`, `1h 01m`, `45m`, `1m 52s`, `22s`, `0.1s`.
pub fn dur(ms: i64) -> String {
    let ms = ms.max(0);
    if ms < 1_000 {
        return format!("{}s", one_decimal(ms as f64 / 1_000.));
    }
    let s = (ms + 500) / 1_000;
    let (hh, mm, ss) = (s / 3_600, s / 60 % 60, s % 60);
    match (hh, mm, ss) {
        (0, 0, ss) => format!("{ss}s"),
        (0, mm, 0) => format!("{mm}m"),
        (0, mm, ss) => format!("{mm}m {ss:02}s"),
        (hh, 0, _) => format!("{hh}h"),
        (hh, mm, _) => format!("{hh}h {mm:02}m"),
    }
}

/// Token and line counts: `4.8B`, `18M`, `198K`, `640`.
pub fn compact(n: i64) -> String {
    let v = n as f64;
    let unit = |d: f64, u: &str| {
        let x = v / d;
        if x < 10. {
            format!("{}{u}", one_decimal(x))
        } else {
            format!("{}{u}", x.round())
        }
    };
    match n {
        1_000_000_000.. => unit(1e9, "B"),
        1_000_000.. => unit(1e6, "M"),
        1_000.. => unit(1e3, "K"),
        _ => n.to_string(),
    }
}

/// `n` of `of` as a percent: one decimal under 10 (`3.1%`, `2%`), whole past it (`31%`).
pub fn pct(n: i64, of: i64) -> Option<(String, f64)> {
    (of > 0).then(|| {
        let p = n as f64 * 100. / of as f64;
        let s = if p < 10. {
            one_decimal(p)
        } else {
            format!("{}", p.round())
        };
        (format!("{s}%"), p)
    })
}

/// One heatmap cell: its week column, its day row (Sunday first) and its level (0 to 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeatCell {
    pub col: u8,
    pub row: u8,
    pub level: u8,
}

/// What the heatmap shows of `report.days`, derived once per report.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Heat {
    pub cells: Rc<[HeatCell]>,
    /// Each cell's tooltip, `Nov 18: 24h`.
    pub tips: Vec<SharedString>,
    /// Month labels by week column.
    pub months: Vec<(usize, String)>,
    pub total_ms: i64,
    /// The first day with agent time.
    pub since: Option<NaiveDate>,
    /// The longest run of days with agent time.
    pub streak: usize,
    pub busiest: Option<(NaiveDate, i64)>,
    /// The median day with agent time.
    pub typical_ms: Option<i64>,
}

impl Heat {
    pub fn of(days: &[recur_services::api::DayHours]) -> Heat {
        let days: Vec<(NaiveDate, i64)> = days
            .iter()
            .filter_map(|d| Some((d.date.parse().ok()?, d.agent_ms)))
            .collect();
        let Some(&(last, _)) = days.last() else {
            return Heat::default();
        };
        let first = last
            - Duration::days(last.weekday().num_days_from_sunday().into())
            - Duration::weeks(WEEKS as i64 - 1);
        let shown: Vec<_> = days.into_iter().filter(|(d, _)| *d >= first).collect();
        let max = shown.iter().map(|d| d.1).max().unwrap_or(0).max(1);
        let mut cells = Vec::with_capacity(shown.len());
        let mut tips = Vec::with_capacity(shown.len());
        let (mut run, mut streak) = (0, 0);
        let mut busy: Vec<i64> = Vec::new();
        let mut busiest: Option<(NaiveDate, i64)> = None;
        for &(d, ms) in &shown {
            let n = (d - first).num_days() as usize;
            let f = ms as f64 / max as f64;
            let level = [0., 0.25, 0.5, 0.75]
                .iter()
                .filter(|edge| f > **edge)
                .count() as u8;
            cells.push(HeatCell {
                col: (n / 7) as u8,
                row: (n % 7) as u8,
                level,
            });
            let day = d.format("%b %-d");
            tips.push(if ms > 0 {
                format!("{day}: {}", hours(ms)).into()
            } else {
                format!("{day}: no agents").into()
            });
            run = if ms > 0 { run + 1 } else { 0 };
            streak = streak.max(run);
            if ms > 0 {
                busy.push(ms);
                if busiest.is_none_or(|b| ms > b.1) {
                    busiest = Some((d, ms));
                }
            }
        }
        let mut months: Vec<(usize, String)> = Vec::new();
        for col in 0..WEEKS {
            let sunday = first + Duration::weeks(col as i64);
            if months.is_empty() || sunday.day() <= 7 {
                months.push((col, sunday.format("%b").to_string()));
            }
        }
        if months.len() > 1 && months[1].0 - months[0].0 < 3 {
            months.remove(0);
        }
        busy.sort_unstable();
        Heat {
            cells: cells.into(),
            tips,
            months,
            total_ms: shown.iter().map(|d| d.1).sum(),
            since: shown.iter().find(|d| d.1 > 0).map(|d| d.0),
            streak,
            busiest,
            typical_ms: busy.get(busy.len() / 2).copied(),
        }
    }
}

/// The page under the route, drawn as a cached view: it renders when the report or range
/// changes, not on shell frames.
pub struct InsightsBody {
    model: Entity<Insights>,
    scroll: ScrollHandle,
}

impl InsightsBody {
    pub fn new(model: Entity<Insights>, cx: &mut Context<Self>) -> Self {
        cx.guarded_observe(&model, |_, _, cx| cx.notify()).detach();
        InsightsBody {
            model,
            scroll: ScrollHandle::new(),
        }
    }

    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let m = self.model.read(cx);
        let main = div()
            .w_full()
            .max_w(px(1040.))
            .px(px(28.))
            .pt(px(30.))
            .pb(px(80.))
            .flex()
            .flex_col()
            .text_color(h(t.text));
        if let Some(p) = m.loaded.as_ref().and_then(|l| l.report.preparing) {
            return preparing(p, &t);
        }
        let main = match &m.loaded {
            None => main.child(
                div()
                    .debug_selector(|| "insights-loading".into())
                    .py(px(24.))
                    .child(text("Loading…").size(12.5).color(t.faint)),
            ),
            Some(l) => main.children(sections(l, m.period, m.start, &t)),
        };
        div()
            .size_full()
            .flex()
            .bg(h(t.bg))
            .child(
                recur_ui::widgets::scroll_area("insights-scroll")
                    .track(&self.scroll)
                    .child(div().w_full().flex().justify_center().child(main)),
            )
            .into_any_element()
    }
}

impl Render for InsightsBody {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, |this, window, cx| {
            recur_ui::style::cached_root(this.render_view(window, cx))
        })
    }
}

/// Shown in place of the page while older history is indexed.
fn preparing(p: f32, t: &Tokens) -> AnyElement {
    div()
        .debug_selector(|| "insights-preparing".into())
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.))
        .bg(h(t.bg))
        .child(
            text("Preparing Insights")
                .size(15.)
                .weight(600.)
                .color(t.text),
        )
        .child(
            text("Indexing your agent history. This takes about a minute and only happens once.")
                .size(13.5)
                .color(t.faint),
        )
        .child(
            div()
                .mt(px(8.))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .w(px(240.))
                        .h(px(4.))
                        .rounded(px(2.))
                        .bg(h(bar_gray(t)))
                        .child(bar(p.into(), t.muted, 4.)),
                )
                .child(
                    text(format!("{}%", (p * 100.).round()))
                        .size(12.)
                        .tabular()
                        .color(t.faint),
                ),
        )
        .into_any_element()
}

fn sections(l: &Loaded, period: Period, start: NaiveDate, t: &Tokens) -> Vec<AnyElement> {
    let r = &l.report;
    let empty = r.agent_ms == 0 && r.tasks.total == 0;
    let mut out = vec![
        stat_row(r, empty, period, start, t),
        section(t).child(heatmap(&l.heat, t)).into_any_element(),
    ];
    if empty {
        out.push(
            section(t)
                .child(
                    text(format!(
                        "No agent time {}.",
                        period.span_words(start, today())
                    ))
                    .size(13.5)
                    .color(t.faint),
                )
                .into_any_element(),
        );
        return out;
    }
    out.extend([
        two_cols(where_hours_went(r, t), time_lost(r, period, start, t), t),
        two_cols(by_hour(r, t), by_weekday(r, t), t),
        section(t).child(models(&r.models, t)).into_any_element(),
        section(t)
            .child(commands(&r.commands, t))
            .into_any_element(),
        section(t).child(kv_grid(r, t)).into_any_element(),
        div()
            .mt(px(44.))
            .child(
                Text::from_spans(vec![
                    span("How this is counted. ").color(t.muted).weight(500.),
                    span(
                        "Agent hours count each agent separately, so five agents busy for an \
                         hour is 5h. Command time runs from a call to its result. Read from this \
                         machine's Recur data; nothing is sent anywhere.",
                    ),
                ])
                .size(12.)
                .leading(1.5)
                .color(t.faint),
            )
            .into_any_element(),
    ]);
    out
}

fn today() -> NaiveDate {
    Local::now().date_naive()
}

fn section(_: &Tokens) -> Div {
    div().mt(px(44.)).flex().flex_col()
}

fn two_cols(a: Div, b: Div, t: &Tokens) -> AnyElement {
    section(t)
        .flex_row()
        .flex_wrap()
        .gap_x(px(56.))
        .gap_y(px(44.))
        .child(a.flex_1().min_w(px(340.)))
        .child(b.flex_1().min_w(px(340.)))
        .into_any_element()
}

/// `h2`: the 12px uppercase heading with its faint note on the right.
fn heading(label: &'static str, note: impl Into<SharedString>, t: &Tokens) -> Div {
    div()
        .debug_selector(move || format!("h2:{label}"))
        .flex()
        .items_center()
        .gap(px(10.))
        .mb(px(12.))
        .child(
            text(label)
                .size(12.)
                .weight(600.)
                .tracking(0.06)
                .uppercase()
                .color(t.muted)
                .nowrap(),
        )
        .child(
            div().ml_auto().child(
                text(note.into())
                    .size(12.)
                    .tabular()
                    .color(t.faint)
                    .nowrap(),
            ),
        )
}

/// Body text: 13.5px, tabular figures.
fn cell(s: impl Into<SharedString>, t: &Tokens) -> Text {
    text(s.into())
        .size(13.5)
        .leading(1.45)
        .tabular()
        .color(t.text)
        .nowrap()
}

fn right(w: f32, child: impl IntoElement) -> Div {
    div().w(px(w)).flex_none().flex().justify_end().child(child)
}

/// `--bar`: the neutral bar gray.
fn bar_gray(t: &Tokens) -> Color {
    t.faint.mix(0.82, t.bg)
}

fn bar(frac: f64, color: Color, height: f32) -> Div {
    div()
        .w(relative(frac.clamp(0., 1.) as f32))
        .h(px(height))
        .rounded(px(height / 2.))
        .bg(h(color))
}

fn stat_row(
    r: &InsightsReport,
    empty: bool,
    period: Period,
    start: NaiveDate,
    t: &Tokens,
) -> AnyElement {
    let sub = |spans: Vec<recur_ui::text::Span>| {
        Text::from_spans(spans)
            .size(12.)
            .leading(1.45)
            .tabular()
            .color(t.faint)
            .line_clamp(2)
    };
    let plain = |s: String| sub(vec![span(s)]);
    let change = if r.prev_agent_ms > 0 {
        let d = (r.agent_ms - r.prev_agent_ms) as f64 * 100. / r.prev_agent_ms as f64;
        let (arrow, color) = if d >= 0. {
            ("↑", t.emerald)
        } else {
            ("↓", t.coral)
        };
        sub(vec![
            span(format!("{arrow} {}%", d.abs().round())).color(color),
            span(format!(" vs {}", period.previous_name(start))),
        ])
    } else {
        plain(DASH.into())
    };
    let peak_at = Local
        .timestamp_millis_opt(r.peak_at_ms)
        .single()
        .filter(|_| r.peak_agents > 0)
        .map(|at| {
            let f = if period == Period::Year {
                "%b %-d, %H:%M"
            } else {
                "%a %-d, %H:%M"
            };
            at.format(f).to_string()
        })
        .unwrap_or_else(|| DASH.into());
    let wait = r
        .asks
        .typical_wait_ms
        .map(dur)
        .unwrap_or_else(|| DASH.into());
    let m = &r.merged;
    let stats: [(&'static str, String, &str, Text); 5] = [
        (
            "Agent hours",
            count((r.agent_ms as f64 / HOUR as f64).round() as i64),
            "h",
            change,
        ),
        (
            "Tasks",
            count(r.tasks.total),
            "",
            plain(format!(
                "{} finished · {} failed",
                count(r.tasks.finished),
                count(r.tasks.failed)
            )),
        ),
        ("Most at once", count(r.peak_agents), "", plain(peak_at)),
        (
            "Needed you",
            count(r.asks.count),
            "×",
            plain(format!("typical wait {wait}")),
        ),
        (
            "Work merged",
            count(m.total),
            "",
            plain(format!(
                "{} by PR · {} direct · {} open",
                count(m.by_pr),
                count(m.direct),
                count(m.open_prs)
            )),
        ),
    ];
    div()
        .flex()
        .children(
            stats
                .into_iter()
                .enumerate()
                .map(|(i, (label, v, unit, s))| {
                    let (v, unit, s) = if empty {
                        (DASH.to_string(), "", plain(DASH.into()))
                    } else {
                        (v, unit, s)
                    };
                    let sel = format!("stat:{label}={v}{unit}");
                    div()
                        .debug_selector(move || sel.clone())
                        .flex_auto()
                        .py(px(2.))
                        .px(px(18.))
                        .when(i == 0, |d| d.pl_0())
                        .when(i > 0, |d| d.border_l_1().border_color(h(t.line)))
                        .child(text(label).size(12.).leading(1.45).color(t.muted).nowrap())
                        .child(
                            Text::from_spans(vec![
                                span(v).size(34.).weight(600.).tracking_px(-0.5),
                                span(unit).size(17.).weight(500.).color(t.muted),
                            ])
                            .leading(1.15)
                            .tabular()
                            .color(t.text)
                            .nowrap(),
                        )
                        .child(s)
                }),
        )
        .into_any_element()
}

fn level_colors(t: &Tokens) -> [Hsla; 5] {
    [
        h(t.line_soft),
        h(t.amber.mix(0.25, t.bg)),
        h(t.amber.mix(0.45, t.bg)),
        h(t.amber.mix(0.7, t.bg)),
        h(t.amber),
    ]
}

fn heatmap(heat: &Heat, t: &Tokens) -> Div {
    let colors = level_colors(t);
    let grid_w = WEEKS as f32 * STEP - (STEP - CELL);
    let grid_h = 7. * STEP - (STEP - CELL);
    let small = |s: &'static str| {
        text(s)
            .size(10.)
            .line_height(LineHeight::Px(CELL))
            .color(t.faint)
    };
    let days = div()
        .w(px(28.))
        .flex_none()
        .pt(px(18.))
        .flex()
        .flex_col()
        .children(
            ["", "Mon", "", "Wed", "", "Fri", ""]
                .map(|d| div().flex_1().flex().items_center().child(small(d))),
        );
    let months = div()
        .relative()
        .h(px(14.))
        .mb(px(4.))
        .children(heat.months.iter().map(|(col, m)| {
            div()
                .absolute()
                .left(relative(*col as f32 * STEP / grid_w))
                .child(
                    text(m.clone())
                        .size(11.)
                        .leading(1.2)
                        .color(t.faint)
                        .nowrap(),
                )
        }));
    let cells = heat.cells.clone();
    let paint = canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let k = f32::from(bounds.size.width) / grid_w;
            window.paint_layer(bounds, |window| {
                for c in cells.iter() {
                    let o = bounds.origin
                        + point(px(c.col as f32 * STEP * k), px(c.row as f32 * STEP * k));
                    window.paint_quad(
                        gpui_kit::fill(
                            gpui_kit::Bounds::new(o, size(px(CELL * k), px(CELL * k))),
                            colors[c.level as usize],
                        )
                        .corner_radii(px(3. * k)),
                    );
                }
            });
        },
    )
    .absolute()
    .inset_0();
    let tips = heat
        .cells
        .iter()
        .zip(&heat.tips)
        .enumerate()
        .map(|(i, (c, tip))| {
            titled(
                ("hm", i),
                tip.clone(),
                div()
                    .absolute()
                    .left(relative(c.col as f32 * STEP / grid_w))
                    .top(relative(c.row as f32 * STEP / grid_h))
                    .w(relative(CELL / grid_w))
                    .h(relative(CELL / grid_h)),
            )
        });
    let grid = div()
        .debug_selector(|| "insights-heatmap".into())
        .relative()
        .w_full()
        .max_w(px(grid_w))
        .aspect_ratio(grid_w / grid_h)
        .child(paint)
        .children(tips);
    let b = |s: String| span(s).color(t.text).weight(500.);
    let foot = |spans: Vec<recur_ui::text::Span>| {
        Text::from_spans(spans)
            .size(12.)
            .tabular()
            .color(t.muted)
            .nowrap()
    };
    let total = foot(vec![
        b(format!(
            "{}h",
            count((heat.total_ms as f64 / HOUR as f64).round() as i64)
        )),
        span(
            heat.since
                .map(|d| format!(" since {}", d.format("%b %-d")))
                .unwrap_or_default(),
        ),
    ]);
    let streak = foot(vec![
        span("Longest streak "),
        b(match heat.streak {
            0 => DASH.into(),
            1 => "1 day".into(),
            n => format!("{n} days"),
        }),
    ]);
    let busiest = foot(match heat.busiest {
        Some((d, ms)) => vec![
            span("Busiest "),
            b(d.format("%b %-d").to_string()),
            span(format!(", {}", hours(ms))),
        ],
        None => vec![span("Busiest "), b(DASH.into())],
    });
    let typical = foot(vec![
        span("Typical day "),
        b(heat.typical_ms.map(hours).unwrap_or_else(|| DASH.into())),
    ]);
    let legend = div()
        .ml_auto()
        .flex()
        .items_center()
        .gap(px(3.))
        .child(
            div()
                .mr(px(2.))
                .child(text("Less").size(12.).color(t.faint)),
        )
        .children(colors.map(|c| div().size(px(11.)).rounded(px(2.)).bg(c)))
        .child(
            div()
                .ml(px(2.))
                .child(text("More").size(12.).color(t.faint)),
        );
    div()
        .child(heading("Agent hours", "last 12 months", t))
        .child(
            div().flex().gap(px(6.)).child(days).child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(months)
                    .child(grid),
            ),
        )
        .child(
            div()
                .mt(px(12.))
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x(px(24.))
                .gap_y(px(6.))
                .child(total)
                .child(streak)
                .child(busiest)
                .child(typical)
                .child(legend),
        )
}

fn where_hours_went(r: &InsightsReport, t: &Tokens) -> Div {
    let s = &r.split;
    let parts = [
        ("Running commands", s.commands_ms, t.text.mix(0.88, t.muted)),
        ("Model thinking", s.model_ms, t.muted.mix(0.6, t.bg)),
        ("Waiting on you", s.waiting_ms, t.indigo),
    ];
    let total = parts.iter().map(|p| p.1).sum::<i64>().max(1);
    let shown: Vec<_> = parts.iter().filter(|p| p.1 > 0).collect();
    let n = shown.len();
    let stack =
        div()
            .flex()
            .h(px(12.))
            .gap(px(2.))
            .mb(px(12.))
            .children(shown.iter().enumerate().map(|(i, p)| {
                div()
                    .min_w(px(2.))
                    .w(relative(p.1 as f32 / total as f32))
                    .h_full()
                    .bg(h(p.2))
                    .when(i == 0, |d| d.rounded_l(px(3.)))
                    .when(i + 1 == n, |d| d.rounded_r(px(3.)))
            }));
    div()
        .child(heading(
            "Where the hours went",
            format!("of {}", hours(r.agent_ms)),
            t,
        ))
        .child(stack)
        .children(parts.iter().map(|(label, ms, color)| {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .py(px(5.))
                .child(
                    div()
                        .flex_none()
                        .size(px(10.))
                        .rounded(px(2.))
                        .bg(h(*color)),
                )
                .child(div().flex_1().min_w_0().child(cell(*label, t)))
                .child(right(
                    48.,
                    cell(
                        format!("{}%", (*ms as f64 * 100. / total as f64).round()),
                        t,
                    )
                    .color(t.muted),
                ))
                .child(right(52., cell(hours(*ms), t)))
        }))
}

fn time_lost(r: &InsightsReport, period: Period, start: NaiveDate, t: &Tokens) -> Div {
    let total: i64 = r.lost.iter().map(|l| l.ms).sum();
    let max = r.lost.iter().map(|l| l.ms).max().unwrap_or(0).max(1);
    let mut rows: Vec<_> = r.lost.iter().collect();
    rows.sort_by_key(|l| std::cmp::Reverse(l.ms));
    let note = if total > 0 {
        format!("{} {}", dur(total), period.span_words(start, today()))
    } else {
        String::new()
    };
    div()
        .child(heading("Time lost", note, t))
        .when(rows.is_empty(), |d| d.child(cell("None", t).color(t.faint)))
        .children(rows.into_iter().enumerate().map(|(i, l)| {
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .py(px(5.))
                .child(
                    div()
                        .w(px(200.))
                        .min_w_0()
                        .overflow_hidden()
                        .child(cell(l.kind.label(), t).truncate()),
                )
                .child(div().flex_1().min_w_0().child(bar(
                    l.ms as f64 / max as f64,
                    if i == 0 { t.amber } else { bar_gray(t) },
                    8.,
                )))
                .child(right(64., cell(dur(l.ms), t)))
        }))
}

/// Bars from 0 to the max, the busiest in the accent, with axis labels under them.
fn histogram(
    values: &[i64],
    gap: f32,
    tips: impl Fn(usize, i64) -> String,
    axis: Div,
    t: &Tokens,
) -> Div {
    let max = values.iter().copied().max().unwrap_or(0);
    let busiest = values.iter().position(|v| *v == max && max > 0);
    div()
        .child(div().flex().items_end().gap(px(gap)).h(px(96.)).children(
            values.iter().enumerate().map(|(i, v)| {
                let frac = if max > 0 { *v as f32 / max as f32 } else { 0. };
                titled(
                    SharedString::from(format!("hist-{gap}-{i}")),
                    tips(i, *v),
                    div().flex_1().h_full().flex().items_end().child(
                        div().w_full().h(relative(frac)).rounded_t(px(2.)).bg(h(
                            if busiest == Some(i) {
                                t.amber
                            } else {
                                bar_gray(t)
                            },
                        )),
                    ),
                )
            }),
        ))
        .child(axis.mt(px(5.)))
}

fn axis(labels: &[&'static str], t: &Tokens) -> Div {
    div().flex().children(
        labels
            .iter()
            .map(|l| text(*l).size(11.).leading(1.3).color(t.faint)),
    )
}

fn by_hour(r: &InsightsReport, t: &Tokens) -> Div {
    let max = r.by_hour.iter().copied().max().unwrap_or(0);
    let note = r
        .by_hour
        .iter()
        .position(|v| *v == max && max > 0)
        .map(|i| format!("busiest {i:02}:00 to {:02}:00", (i + 1) % 24))
        .unwrap_or_default();
    div()
        .child(heading("By hour of day", note, t))
        .child(histogram(
            &r.by_hour,
            3.,
            |i, v| format!("{i:02}:00 · {}", hours(v)),
            axis(&["0", "6", "12", "18", "23"], t).justify_between(),
            t,
        ))
}

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

fn by_weekday(r: &InsightsReport, t: &Tokens) -> Div {
    div()
        .child(heading("By weekday", "agent hours per day, average", t))
        .child(histogram(
            &r.by_weekday,
            10.,
            |i, v| format!("{} · {}", WEEKDAYS[i], hours(v)),
            axis(&WEEKDAYS, t).justify_around(),
            t,
        ))
}

/// The table's header row: the first column left, the rest right-aligned at their widths.
fn header(first: &'static str, first_w: f32, cols: &[(&'static str, f32)], t: &Tokens) -> Div {
    let th = |s: &'static str| text(s).size(12.).leading(1.45).color(t.faint).nowrap();
    div()
        .flex()
        .items_center()
        .pb(px(6.))
        .child(div().w(px(first_w)).flex_none().child(th(first)))
        .child(div().flex_1())
        .children(cols.iter().map(|(s, w)| right(*w, th(s))))
}

fn row(t: &Tokens) -> Div {
    div()
        .flex()
        .items_center()
        .py(px(6.))
        .border_t_1()
        .border_color(h(t.line_soft))
}

fn bar_cell(frac: f64, color: Color) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .pl(px(14.))
        .pr(px(24.))
        .child(bar(frac, color, 7.))
}

/// Up to `keep` rows, and the rest folded into one when more than one is left.
fn split<T>(v: &[T], keep: usize) -> (&[T], &[T]) {
    if v.len() > keep + 1 {
        v.split_at(keep)
    } else {
        (v, &[])
    }
}

fn model_colors(t: &Tokens) -> [Color; 6] {
    [t.amber, t.cyan, t.git_m, t.coral, t.indigo, t.emerald]
}

const MODEL_COLS: [(&str, f32); 6] = [
    ("Hours", 72.),
    ("Runs", 64.),
    ("Typical task", 96.),
    ("Needed you", 96.),
    ("Failed cmds", 96.),
    ("Tokens in", 104.),
];

fn models(models: &[ModelStats], t: &Tokens) -> Div {
    let (top, rest) = split(models, MODEL_ROWS);
    let max = models.iter().map(|m| m.agent_ms).max().unwrap_or(0).max(1) as f64;
    let palette = model_colors(t);
    let faint = |s: &str| cell(s.to_string(), t).color(t.faint);
    let w = |i: usize| MODEL_COLS[i].1;
    let mut table = div()
        .child(heading(
            "Models",
            format!("{} models · by agent time", count(models.len() as i64)),
            t,
        ))
        .child(header("Model", 170., &MODEL_COLS, t));
    for (i, m) in top.iter().enumerate() {
        let color = palette[i % palette.len()];
        let failed = pct(m.failed_commands, m.commands);
        table = table.child(
            row(t)
                .child(
                    div()
                        .w(px(170.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().flex_none().size(px(8.)).rounded_full().bg(h(color)))
                        .child(cell(m.model.clone(), t)),
                )
                .child(bar_cell(m.agent_ms as f64 / max, color.fade(0.85)))
                .child(right(w(0), cell(hours(m.agent_ms), t)))
                .child(right(w(1), cell(count(m.runs), t)))
                .child(right(
                    w(2),
                    m.typical_task_ms
                        .map_or(faint("too few"), |ms| cell(dur(ms), t)),
                ))
                .child(right(
                    w(3),
                    m.ms_per_ask
                        .map_or(faint("too few"), |ms| cell(format!("1 per {}", dur(ms)), t)),
                ))
                .child(right(
                    w(4),
                    match failed {
                        Some((s, p)) if p >= HIGH_FAILURE => cell(s, t).color(t.coral),
                        Some((s, _)) => cell(s, t),
                        None => faint(DASH),
                    },
                ))
                .child(right(
                    w(5),
                    m.tokens_in
                        .map_or(faint("not reported"), |n| cell(compact(n), t)),
                )),
        );
    }
    if !rest.is_empty() {
        let ms: i64 = rest.iter().map(|m| m.agent_ms).sum();
        let tokens: i64 = rest.iter().filter_map(|m| m.tokens_in).sum();
        table = table.child(
            row(t)
                .child(
                    div()
                        .w(px(170.))
                        .flex_none()
                        .child(faint(&format!("{} more", rest.len()))),
                )
                .child(bar_cell(ms as f64 / max, bar_gray(t)))
                .child(right(w(0), cell(hours(ms), t)))
                .child(right(
                    w(1),
                    cell(count(rest.iter().map(|m| m.runs).sum()), t),
                ))
                .child(right(w(2) + w(3) + w(4), div()))
                .child(right(
                    w(5),
                    if tokens > 0 {
                        cell(compact(tokens), t)
                    } else {
                        faint(DASH)
                    },
                )),
        );
    }
    table
}

const COMMAND_COLS: [(&str, f32); 5] = [
    ("Time", 72.),
    ("Calls", 72.),
    ("Typical", 80.),
    ("Slowest", 80.),
    ("Failed", 112.),
];

fn commands(cmds: &[CommandStats], t: &Tokens) -> Div {
    let (top, rest) = split(cmds, COMMAND_ROWS);
    let max = cmds.iter().map(|c| c.total_ms).max().unwrap_or(0).max(1) as f64;
    let calls: i64 = cmds.iter().map(|c| c.calls).sum();
    let total: i64 = cmds.iter().map(|c| c.total_ms).sum();
    let faint = |s: String| cell(s, t).color(t.faint);
    let w = |i: usize| COMMAND_COLS[i].1;
    let mut table = div()
        .child(heading(
            "Commands",
            format!("{} calls · {}", count(calls), hours(total)),
            t,
        ))
        .child(header("Command", 170., &COMMAND_COLS, t));
    for c in top {
        let failed = match (pct(c.failed, c.calls), pct(c.no_match, c.calls)) {
            (_, Some((s, _))) if c.failed == 0 => faint(format!("{s} no match")),
            (Some((s, p)), _) if p >= HIGH_FAILURE => cell(s, t).color(t.coral),
            (Some((s, p)), _) if p < 1. => faint(s),
            (Some((s, _)), _) => cell(s, t),
            (None, _) => faint(DASH.into()),
        };
        table = table.child(
            row(t)
                .child(div().w(px(170.)).flex_none().child(cell(c.name.clone(), t)))
                .child(bar_cell(c.total_ms as f64 / max, bar_gray(t)))
                .child(right(w(0), cell(hours(c.total_ms), t)))
                .child(right(w(1), cell(count(c.calls), t)))
                .child(right(w(2), cell(dur(c.p50_ms), t)))
                .child(right(w(3), cell(dur(c.max_ms), t)))
                .child(right(w(4), failed)),
        );
    }
    if !rest.is_empty() {
        let ms: i64 = rest.iter().map(|c| c.total_ms).sum();
        table = table.child(
            row(t)
                .child(
                    div()
                        .w(px(170.))
                        .flex_none()
                        .child(faint(format!("{} more", rest.len()))),
                )
                .child(bar_cell(ms as f64 / max, bar_gray(t)))
                .child(right(w(0), cell(hours(ms), t)))
                .child(right(
                    w(1),
                    cell(count(rest.iter().map(|c| c.calls).sum()), t),
                ))
                .child(right(w(2) + w(3) + w(4), div())),
        );
    }
    table
}

fn kv_grid(r: &InsightsReport, t: &Tokens) -> Div {
    let v = |s: String| cell(s, t);
    let n = |x: i64| v(count(x));
    let (tk, hf, cx, ch) = (&r.tasks, &r.handoffs, &r.context, &r.changes);
    let maybe = |p: Option<(String, f64)>| v(p.map(|p| p.0).unwrap_or_else(|| DASH.into()));
    let groups: [(&'static str, Vec<(&'static str, Text)>); 6] = [
        (
            "Tasks",
            vec![
                ("From you", n(tk.from_you)),
                ("From another agent", n(tk.from_agent)),
                ("From schedules", n(tk.from_schedule)),
                ("Projects", n(tk.projects)),
            ],
        ),
        (
            "How they ended",
            vec![
                ("Finished", n(tk.finished)),
                (
                    "Failed",
                    n(tk.failed).color(if tk.failed > 0 { t.coral } else { t.text }),
                ),
                ("Cancelled / stopped", n(tk.stopped)),
                ("Running", n(tk.running)),
            ],
        ),
        (
            "Handoffs",
            vec![
                ("Coder → reviewer", n(hf.to_reviewer)),
                ("Chat → child workspace", n(hf.to_child)),
                (
                    "Review loops settled",
                    maybe(pct(hf.review_settled, hf.review_loops)),
                ),
                ("Didn't finish", n(hf.unfinished)),
            ],
        ),
        (
            "Context",
            vec![
                ("Compactions", n(cx.compactions)),
                ("Turns past 90% full", n(cx.turns_over_90)),
                (
                    "Largest context",
                    v(if cx.largest > 0 {
                        compact(cx.largest)
                    } else {
                        DASH.into()
                    }),
                ),
            ],
        ),
        (
            "Tokens",
            vec![
                ("In", v(compact(r.tokens.input))),
                ("From cache", maybe(pct(r.tokens.cached, r.tokens.input))),
                ("Out", v(compact(r.tokens.output))),
            ],
        ),
        (
            "Changes",
            vec![
                (
                    "Lines",
                    Text::from_spans(vec![
                        span(format!("+{}", compact(ch.added))).color(t.emerald),
                        span(" "),
                        span(format!("−{}", compact(ch.removed))).color(t.coral),
                    ])
                    .size(13.5)
                    .leading(1.45)
                    .tabular()
                    .nowrap(),
                ),
                ("Files changed", n(ch.files)),
                (
                    "Workspaces merged",
                    Text::from_spans(vec![
                        span(count(r.merged.total)).color(t.text),
                        span(format!(" ({} by PR)", count(r.merged.by_pr))).color(t.muted),
                    ])
                    .size(13.5)
                    .leading(1.45)
                    .tabular()
                    .nowrap(),
                ),
            ],
        ),
    ];
    let mut groups = groups.into_iter();
    let mut grid = div().flex().flex_col().gap(px(28.));
    for _ in 0..2 {
        grid = grid.child(
            div()
                .flex()
                .gap(px(56.))
                .children(groups.by_ref().take(3).map(|(title, rows)| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div().mb(px(8.)).child(
                                text(title)
                                    .size(12.)
                                    .weight(600.)
                                    .tracking(0.06)
                                    .uppercase()
                                    .color(t.muted)
                                    .nowrap(),
                            ),
                        )
                        .children(rows.into_iter().enumerate().map(|(i, (k, val))| {
                            div()
                                .flex()
                                .justify_between()
                                .py(px(4.))
                                .when(i > 0, |d| d.border_t_1().border_color(h(t.line_soft)))
                                .child(cell(k, t).color(t.muted))
                                .child(val)
                        }))
                })),
        );
    }
    grid
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_services::api::DayHours;

    #[test]
    fn numbers() {
        assert_eq!(count(29_840), "29,840");
        assert_eq!(count(813), "813");
        assert_eq!(count(1_412_000), "1,412,000");
        assert_eq!(hours(236 * HOUR), "236h");
        assert_eq!(hours(10 * HOUR + 36 * 60_000), "10.6h");
        assert_eq!(hours(14 * HOUR), "14h");
        assert_eq!(dur(9 * HOUR + 12 * 60_000), "9h 12m");
        assert_eq!(dur(HOUR + 60_000), "1h 01m");
        assert_eq!(dur(45 * 60_000), "45m");
        assert_eq!(dur(112_000), "1m 52s");
        assert_eq!(dur(22_000), "22s");
        assert_eq!(dur(100), "0.1s");
        assert_eq!(compact(4_800_000_000), "4.8B");
        assert_eq!(compact(18_000_000), "18M");
        assert_eq!(compact(198_000), "198K");
        assert_eq!(pct(304, 9_812).unwrap().0, "3.1%");
        assert_eq!(pct(2_133, 6_880).unwrap().0, "31%");
        assert_eq!(pct(1, 0), None);
    }

    #[test]
    fn heat_lays_weeks_out_sunday_first() {
        let start = NaiveDate::from_ymd_opt(2025, 11, 14).unwrap();
        let days: Vec<DayHours> = (0..371)
            .map(|i| DayHours {
                date: (start + Duration::days(i)).format("%Y-%m-%d").to_string(),
                agent_ms: if (300..341).contains(&i) {
                    (i % 5 + 1) * HOUR
                } else {
                    0
                },
            })
            .collect();
        let heat = Heat::of(&days);
        let last = heat.cells.last().unwrap();
        assert_eq!(last.col as usize, WEEKS - 1);
        // 2026-11-19 is a Thursday.
        assert_eq!(last.row, 4);
        assert_eq!(heat.streak, 41);
        assert_eq!(
            heat.total_ms,
            (0..41).map(|i| ((300 + i) % 5 + 1) * HOUR).sum::<i64>()
        );
        assert_eq!(heat.cells.iter().filter(|c| c.level == 4).count(), 16);
        assert!(heat.tips.iter().any(|t| t.as_ref() == "Nov 19: no agents"));
        assert!(heat.months.iter().all(|m| m.1.len() == 3));
    }
}
