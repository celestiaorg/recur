//! The route in the shell over the fake backend's busy-month report.

use gpui::{Modifiers, TestAppContext, px};
use recur_app::{Shell, ShellOptions};
use recur_services::fake::{Fake, Overrides, Seed};
use recur_view_core::{AppState, LocalStorage, ViewContext, ViewName, ViewRegistry};

#[gpui::test]
fn renders_the_fake_report(cx: &mut TestAppContext) {
    let mut registry = ViewRegistry::default();
    crate::register(&mut registry);
    cx.update(|cx| {
        recur_app::init(&registry, cx);
        recur_ui::motion::freeze(cx);
    });
    let services = Fake::new(Seed::e2e(), Overrides::default()).services();
    let state = cx.update(|cx| AppState::install(services.clone(), LocalStorage::in_memory(), cx));
    let ctx = ViewContext {
        state: state.clone(),
        services,
    };
    let (_, cx) = cx.add_window_view(move |window, cx| {
        Shell::new(ctx, registry, ShellOptions::default(), window, cx)
    });
    cx.simulate_resize(gpui::size(px(1380.), px(880.)));
    state.update(cx, |s, cx| s.go(ViewName::Insights, None, cx));
    cx.run_until_parked();
    for sel in [
        "stat:Agent hours=236h",
        "stat:Tasks=813",
        "stat:Work merged=67",
        "h2:Models",
        "insights-heatmap",
        "seg:Month:true",
    ] {
        assert!(cx.debug_bounds(sel).is_some(), "{sel} not drawn");
    }
    assert!(cx.debug_bounds("insights-loading").is_none());
}

#[gpui::test]
fn the_range_label_jumps_to_an_earlier_period(cx: &mut TestAppContext) {
    let mut registry = ViewRegistry::default();
    crate::register(&mut registry);
    cx.update(|cx| {
        recur_app::init(&registry, cx);
        recur_ui::motion::freeze(cx);
    });
    let services = Fake::new(Seed::e2e(), Overrides::default()).services();
    let state = cx.update(|cx| AppState::install(services.clone(), LocalStorage::in_memory(), cx));
    let ctx = ViewContext {
        state: state.clone(),
        services,
    };
    let (_, cx) = cx.add_window_view(move |window, cx| {
        Shell::new(ctx, registry, ShellOptions::default(), window, cx)
    });
    cx.simulate_resize(gpui::size(px(1380.), px(880.)));
    state.update(cx, |s, cx| s.go(ViewName::Insights, None, cx));
    cx.run_until_parked();
    let months = crate::Period::Month.recent(chrono::Local::now().date_naive());
    let (now, back) = (
        crate::Period::Month.label_for(months[0]),
        crate::Period::Month.label_for(months[3]),
    );
    click(cx, &format!("sel:Range={now}"));
    click(cx, &format!("sel-opt:{back}"));
    let moved = format!("insights-range={back}").leak();
    assert!(
        cx.debug_bounds(moved).is_some(),
        "range did not move to {back}"
    );
}

fn click(cx: &mut gpui::VisualTestContext, sel: &str) {
    let sel: &'static str = sel.to_owned().leak();
    let b = cx
        .debug_bounds(sel)
        .unwrap_or_else(|| panic!("{sel} not drawn"));
    cx.simulate_click(b.center(), Modifiers::none());
    cx.run_until_parked();
}
