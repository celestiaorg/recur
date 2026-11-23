//! Insights: how agents spent their time over a week, month or year, from
//! `InsightsService::insights`.
//!
//! # Embedding
//!
//! Call [`register`]. The route's range controls lead the titlebar's right side; both read the
//! one [`Insights`] model per app. The granularity is kept in `insights-view.json`; the range
//! always opens on the current period.

mod page;
#[cfg(test)]
mod spec;

use chrono::{Datelike, Duration, Local, Months, NaiveDate, TimeZone};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, AppContext as _, Context, Entity, Global, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Render, StatefulInteractiveElement, Styled, Subscription, Task, Window, div, px,
};
use recur_services::Services;
use recur_services::api::InsightsReport;
use recur_ui::guard::Guarded;
use recur_ui::widgets::select::{Select, SelectEvent, SelectOption, TriggerStyle};
use recur_ui::{IconName, Theme, icon};
use recur_view_core::{
    AppState, ObserveState as _, TitlebarParts, Topics, ViewContext, ViewName, ViewRegistry,
};
use recur_view_widgets::list_chrome::{filter_seg, seg_count_btn};

pub use page::{Heat, InsightsBody};

/// Registers the Insights route and its titlebar range controls.
pub fn register(r: &mut ViewRegistry) {
    r.view(&[ViewName::Insights], |vcx: &ViewContext, window, cx| {
        cx.new(|cx| InsightsPage::new(vcx, window, cx)).into()
    })
    .titlebar(|vcx, window, cx| TitlebarParts {
        left: None,
        right: Some(cx.new(|cx| InsightsTitlebar::new(vcx, window, cx)).into()),
    });
}

/// The range granularity. Ranges follow local calendar boundaries; weeks start on Monday.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Period {
    Week,
    #[default]
    Month,
    Year,
}

impl Period {
    pub const ALL: [Period; 3] = [Period::Week, Period::Month, Period::Year];

    pub fn label(self) -> &'static str {
        match self {
            Period::Week => "Week",
            Period::Month => "Month",
            Period::Year => "Year",
        }
    }

    /// The saved form in `insights-view.json`.
    pub fn key(self) -> &'static str {
        match self {
            Period::Week => "week",
            Period::Month => "month",
            Period::Year => "year",
        }
    }

    pub fn from_key(key: &str) -> Option<Period> {
        Period::ALL.into_iter().find(|p| p.key() == key)
    }

    /// The first day of the period holding `d`.
    pub fn start_of(self, d: NaiveDate) -> NaiveDate {
        match self {
            Period::Week => d - Duration::days(d.weekday().num_days_from_monday().into()),
            Period::Month => d.with_day(1).unwrap_or(d),
            Period::Year => d.with_ordinal(1).unwrap_or(d),
        }
    }

    /// The start of the period after (`step` 1) or before (`step` -1) the one at `start`.
    pub fn step(self, start: NaiveDate, step: i32) -> NaiveDate {
        let months = match self {
            Period::Week => return start + Duration::days(7 * i64::from(step)),
            Period::Month => 1,
            Period::Year => 12,
        };
        let m = Months::new(months);
        if step > 0 {
            start.checked_add_months(m)
        } else {
            start.checked_sub_months(m)
        }
        .unwrap_or(start)
    }

    /// "Nov 16 to 22", "Oct 26 to Nov 1", "November 2026", "2026".
    pub fn label_for(self, start: NaiveDate) -> String {
        match self {
            Period::Week => {
                let end = start + Duration::days(6);
                if end.month() == start.month() {
                    format!("{} to {}", start.format("%b %-d"), end.day())
                } else {
                    format!("{} to {}", start.format("%b %-d"), end.format("%b %-d"))
                }
            }
            Period::Month => start.format("%B %Y").to_string(),
            Period::Year => start.year().to_string(),
        }
    }

    /// The starts of the periods a jump can pick, newest first: the 12 weeks or months, or the
    /// 5 years, up to the one holding `today`.
    pub fn recent(self, today: NaiveDate) -> Vec<NaiveDate> {
        let n = if self == Period::Year { 5 } else { 12 };
        std::iter::successors(Some(self.start_of(today)), |&s| Some(self.step(s, -1)))
            .take(n)
            .collect()
    }

    /// What the comparison is against: "last week", "October", "2025".
    pub fn previous_name(self, start: NaiveDate) -> String {
        match self {
            Period::Week => "last week".into(),
            Period::Month => self.step(start, -1).format("%B").to_string(),
            Period::Year => (start.year() - 1).to_string(),
        }
    }

    /// "this week", "this month", "this year" for the current period, else "in {label}".
    pub fn span_words(self, start: NaiveDate, today: NaiveDate) -> String {
        if self.start_of(today) == start {
            format!("this {}", self.label().to_lowercase())
        } else {
            format!("in {}", self.label_for(start))
        }
    }
}

/// Unix ms of local midnight on `d` (1am where midnight falls in a DST gap).
pub fn local_ms(d: NaiveDate) -> i64 {
    [0, 1]
        .into_iter()
        .find_map(|h| {
            Local
                .from_local_datetime(&d.and_hms_opt(h, 0, 0)?)
                .earliest()
        })
        .map(|t| t.timestamp_millis())
        .unwrap_or_default()
}

const PREPARING_POLL: std::time::Duration = std::time::Duration::from_secs(2);

fn today() -> NaiveDate {
    Local::now().date_naive()
}

/// A loaded report and what the view derives from it once.
pub struct Loaded {
    pub report: InsightsReport,
    pub heat: Heat,
}

/// The range and its report, shared by the page and its titlebar controls.
pub struct Insights {
    services: Services,
    pub period: Period,
    pub start: NaiveDate,
    /// `None` while the range's report loads.
    pub loaded: Option<Loaded>,
    load: Option<Task<()>>,
}

struct InsightsGlobal(Entity<Insights>);

impl Global for InsightsGlobal {}

impl Insights {
    /// The app's model, created on first use, switching to the saved granularity once it loads.
    pub fn of(services: &Services, cx: &mut App) -> Entity<Insights> {
        if let Some(g) = cx.try_global::<InsightsGlobal>() {
            return g.0.clone();
        }
        let period = Period::default();
        let e = cx.new(|_| Insights {
            services: services.clone(),
            period,
            start: period.start_of(today()),
            loaded: None,
            load: None,
        });
        cx.set_global(InsightsGlobal(e.clone()));
        let shell = services.shell.clone();
        let model = e.downgrade();
        cx.spawn(async move |cx| {
            let saved = shell.get_insights_period().await.unwrap_or_default();
            if let Some(p) = Period::from_key(&saved) {
                let _ = model.update(cx, |m, cx| m.set_range(p, p.start_of(today()), cx));
            }
        })
        .detach();
        e
    }

    /// Loads the current range's report off the UI thread; the shown one stays until it lands.
    /// While the history is still being indexed it asks again every couple of seconds.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let (prev, from, to) = (
            local_ms(self.period.step(self.start, -1)),
            local_ms(self.start),
            local_ms(self.period.step(self.start, 1)),
        );
        let svc = self.services.insights.clone();
        self.load = Some(cx.spawn(async move |this, cx| {
            loop {
                let report = recur_view_core::call(
                    cx,
                    svc.insights(prev, from, to),
                    "Could not load insights",
                )
                .await
                .unwrap_or_default();
                let preparing = report.preparing.is_some();
                let heat = Heat::of(&report.days);
                if this
                    .update(cx, |m, cx| {
                        m.loaded = Some(Loaded { report, heat });
                        cx.notify();
                    })
                    .is_err()
                    || !preparing
                {
                    return;
                }
                cx.background_executor().timer(PREPARING_POLL).await;
            }
        }));
    }

    pub fn set_range(&mut self, period: Period, start: NaiveDate, cx: &mut Context<Self>) {
        if (period, start) == (self.period, self.start) {
            return;
        }
        self.period = period;
        self.start = start;
        self.loaded = None;
        cx.notify();
        self.reload(cx);
    }

    /// Switch granularity around the shown range (today when it holds today, else its first
    /// day) and save it.
    pub fn set_period(&mut self, period: Period, cx: &mut Context<Self>) {
        let shell = self.services.shell.clone();
        cx.background_spawn(async move { shell.set_insights_period(period.key()).await })
            .detach();
        let last = self.period.step(self.start, 1) - Duration::days(1);
        let anchor = today().min(last);
        self.set_range(period, period.start_of(anchor), cx);
    }

    pub fn can_step_forward(&self) -> bool {
        self.period.step(self.start, 1) <= today()
    }

    pub fn step(&mut self, step: i32, cx: &mut Context<Self>) {
        if step > 0 && !self.can_step_forward() {
            return;
        }
        let start = self.period.step(self.start, step);
        self.set_range(self.period, start, cx);
    }
}

/// The route: reloads the report on each visit and draws the page as a cached view, so shell
/// frames replay it instead of rebuilding a few hundred elements.
pub struct InsightsPage {
    state: Entity<AppState>,
    model: Entity<Insights>,
    body: Entity<InsightsBody>,
    on: bool,
    _subs: Vec<Subscription>,
}

impl InsightsPage {
    pub fn new(vcx: &ViewContext, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = Insights::of(&vcx.services, cx);
        let body = cx.new(|cx| InsightsBody::new(model.clone(), cx));
        let subs = vec![cx.observe_state(&vcx.state, Topics::NONE, |this, _, cx| this.sync(cx))];
        let mut me = InsightsPage {
            state: vcx.state.clone(),
            model,
            body,
            on: false,
            _subs: subs,
        };
        me.sync(cx);
        me
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let on = self.state.read(cx).view == ViewName::Insights;
        if on && !self.on {
            self.model.update(cx, |m, cx| m.reload(cx));
        }
        self.on = on;
    }
}

impl Render for InsightsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, |this, _, _| {
            recur_ui::style::cached_view(
                &this.body,
                gpui_kit::StyleRefinement::default().size_full(),
            )
        })
    }
}

/// Week / Month / Year, ‹ › and the range label, leading `.tb-right`.
pub struct InsightsTitlebar {
    model: Entity<Insights>,
    range: Entity<Select>,
}

impl InsightsTitlebar {
    pub fn new(vcx: &ViewContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = Insights::of(&vcx.services, cx);
        let text = Theme::of(cx).tokens.text;
        let range = cx.new(|cx| {
            Select::new(Vec::new(), window, cx)
                .label("Range")
                .ghost()
                .trigger_style(TriggerStyle {
                    height: Some(24.),
                    ui_font: true,
                    font_size: Some(12.5),
                    weight: Some(500.),
                    color: Some(text),
                    no_outline: true,
                    ..Default::default()
                })
        });
        cx.guarded_observe(&model, |this, _, cx| {
            this.sync_range(cx);
            cx.notify()
        })
        .detach();
        cx.guarded_subscribe(&range, |this, _, ev: &SelectEvent, cx| {
            if let SelectEvent::Change(v) = ev
                && let Ok(start) = NaiveDate::parse_from_str(v, "%Y-%m-%d")
            {
                this.model
                    .update(cx, |m, cx| m.set_range(m.period, start, cx));
            }
        })
        .detach();
        let mut this = InsightsTitlebar { model, range };
        this.sync_range(cx);
        this
    }

    /// The jump list for the shown granularity, with the shown range selected.
    fn sync_range(&mut self, cx: &mut Context<Self>) {
        let m = self.model.read(cx);
        let (period, start) = (m.period, m.start);
        let key = |d: NaiveDate| d.format("%Y-%m-%d").to_string();
        let mut starts = period.recent(today());
        if !starts.contains(&start) {
            starts.push(start);
        }
        let options = starts
            .into_iter()
            .map(|d| SelectOption::new(key(d), period.label_for(d)))
            .collect();
        self.range.update(cx, |s, cx| {
            s.set_options(options, cx);
            s.set_value(key(start), cx);
        });
    }

    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let m = self.model.read(cx);
        let (period, label, forward) =
            (m.period, m.period.label_for(m.start), m.can_step_forward());
        let model = self.model.clone();
        let seg = Period::ALL.into_iter().fold(filter_seg(cx), |seg, p| {
            let model = model.clone();
            seg.child(no_drag(
                seg_count_btn(p.label(), p.label(), None, p == period, cx)
                    .on_click(move |_, _, cx| model.update(cx, |m, cx| m.set_period(p, cx))),
            ))
        });
        let arrow = |id: &'static str, name: IconName, step: i32, enabled: bool| {
            let model = model.clone();
            no_drag(
                recur_ui::style::titlebar_icon_button(id, false, cx)
                    .debug_selector(move || id.into())
                    .h(px(24.))
                    .when(!enabled, |b| {
                        b.cursor_default().text_color(t.faint.mix(0.4, t.titlebar))
                    })
                    .when(enabled, |b| {
                        b.on_click(move |_, _, cx| model.update(cx, |m, cx| m.step(step, cx)))
                    })
                    .child(icon(name).size(px(14.))),
            )
        };
        let sel = format!("insights-range={label}");
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .child(div().mr(px(8.)).child(seg))
            .child(arrow("insights-prev", IconName::ChevronLeft, -1, true))
            .child(no_drag(
                div()
                    .debug_selector(move || sel.clone())
                    .min_w(px(104.))
                    .flex()
                    .justify_center()
                    .child(self.range.clone()),
            ))
            .child(arrow("insights-next", IconName::ChevronRight, 1, forward))
    }
}

impl Render for InsightsTitlebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// `-webkit-app-region: no-drag`: a press here does not move the window.
fn no_drag<E: InteractiveElement>(el: E) -> E {
    el.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn periods_follow_the_calendar() {
        let fri = d(2026, 11, 20);
        assert_eq!(Period::Week.start_of(fri), d(2026, 11, 16));
        assert_eq!(Period::Month.start_of(fri), d(2026, 11, 1));
        assert_eq!(Period::Year.start_of(fri), d(2026, 1, 1));
        assert_eq!(Period::Month.step(d(2026, 1, 1), -1), d(2025, 12, 1));
        assert_eq!(Period::Year.step(d(2026, 1, 1), 1), d(2027, 1, 1));
        assert_eq!(Period::Week.step(d(2026, 11, 16), -1), d(2026, 11, 9));
    }

    #[test]
    fn labels() {
        assert_eq!(Period::Week.label_for(d(2026, 11, 16)), "Nov 16 to 22");
        assert_eq!(Period::Week.label_for(d(2026, 10, 26)), "Oct 26 to Nov 1");
        assert_eq!(Period::Month.label_for(d(2026, 11, 1)), "November 2026");
        assert_eq!(Period::Year.label_for(d(2026, 1, 1)), "2026");
        for p in Period::ALL {
            assert_eq!(Period::from_key(p.key()), Some(p));
        }
        assert_eq!(Period::from_key(""), None);
        assert_eq!(Period::Month.previous_name(d(2026, 11, 1)), "October");
        assert_eq!(Period::Year.previous_name(d(2026, 1, 1)), "2025");
        let today = d(2026, 11, 20);
        assert_eq!(
            Period::Month.span_words(d(2026, 11, 1), today),
            "this month"
        );
        assert_eq!(
            Period::Month.span_words(d(2026, 10, 1), today),
            "in October 2026"
        );
    }
}
