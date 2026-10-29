//! `DateTimeField` in a window: fields focus on click and take keys, and a date input's
//! calendar popover (WebKitGTK's picker) opens on click, selects days by click and arrow keys,
//! and gives the keyboard back to its field on Escape.

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div,
};

use crate::color::hex;
use crate::logic::date_time::Kind;
use crate::widgets::{DateTimeField, DateTimeStyle};

struct Root {
    field: Entity<DateTimeField>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p_4().child(self.field.clone())
    }
}

fn open(cx: &mut TestAppContext) -> (Entity<DateTimeField>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::init(cx);
    });
    let mut field = None;
    let (_, vcx) = cx.add_window_view(|_, cx| {
        let f = cx.new(|cx| {
            let style = DateTimeStyle {
                color: hex(0xaaaaaa),
                size: 12.,
                tabular: false,
                pad_x: 6.,
                height: 24.,
                dark: true,
            };
            let mut f = DateTimeField::with_pattern(
                "dt",
                Kind::Date,
                "M/d/y",
                ["AM".into(), "PM".into()],
                style,
                cx,
            )
            .with_popover();
            f.set_value("2026-10-27", cx);
            f
        });
        field = Some(f.clone());
        Root { field: f }
    });
    vcx.run_until_parked();
    (field.expect("field"), vcx)
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let at = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
        .center();
    cx.simulate_click(at, gpui_kit::Modifiers::none());
    cx.run_until_parked();
}

#[gpui_kit::test]
fn the_calendar_popover_picks_days_and_returns_the_keyboard(cx: &mut TestAppContext) {
    let (field, cx) = open(cx);
    let value = |cx: &mut VisualTestContext| field.read_with(cx, |f, _| f.value());

    click(cx, "dt:field1");
    assert!(
        cx.debug_bounds("dt-calendar").is_some(),
        "a click opens the popover"
    );
    cx.simulate_keystrokes("up");
    assert_eq!(
        value(cx),
        "2026-10-20",
        "the popover has the keys: up is a week back"
    );

    click(cx, "dt-calendar-d2026-10-15");
    assert_eq!(value(cx), "2026-10-15");
    cx.simulate_keystrokes("right down");
    assert_eq!(
        value(cx),
        "2026-10-23",
        "arrows move the selection and choose it"
    );
    click(cx, "dt-calendar-month-next");
    assert_eq!(
        value(cx),
        "2026-10-23",
        "the month arrows only change what shows"
    );
    assert!(cx.debug_bounds("dt-calendar-d2026-11-23").is_some());

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds("dt-calendar").is_none(), "Escape closes it");
    let back = field.read_with(cx, |f, _| f.focus_handles()[1].clone());
    assert!(
        cx.update(|window, _| back.is_focused(window)),
        "the day field has the keys again"
    );
    cx.simulate_keystrokes("down");
    assert_eq!(value(cx), "2026-10-22", "down steps the day field");
}
