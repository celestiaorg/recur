//! WebKitGTK's date picker (`WebDateTimePickerGtk`): a `GtkCalendar` in an arrowless
//! `GtkPopover` under the input. Month and year switch with their arrows; a click or an arrow
//! key selects a day (`day-selected`, which sets the input's value); Escape or a click outside
//! closes it. Drawn after GTK 4's Adwaita calendar.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Div, FocusHandle, Hsla, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::color::{Color, hex};
use crate::logic::date_time::days_in_month;
use crate::text::{TextStyled, text};

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
/// Adwaita's `@accent_bg_color`.
const ACCENT: Color = hex(0x3584e4);

/// The calendar's state: the month shown and the selected day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Calendar {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

/// Day of the week of the 1st, 0 = Sunday (Zeller's congruence, Gregorian).
fn first_weekday(year: i32, month: u32) -> u32 {
    let (y, m) = if month < 3 {
        (year - 1, month + 12)
    } else {
        (year, month)
    };
    let k = y.rem_euclid(100);
    let j = y.div_euclid(100);
    let h = (1 + (13 * (m as i32 + 1)) / 5 + k + k / 4 + j / 4 + 5 * j).rem_euclid(7);
    ((h + 6) % 7) as u32
}

impl Calendar {
    pub fn new(year: i32, month: u32, day: u32) -> Self {
        Self { year, month, day }
    }

    /// The 6 by 7 grid from the Sunday on or before the 1st: (year, month, day) per cell.
    pub fn grid(&self) -> Vec<(i32, u32, u32)> {
        let lead = first_weekday(self.year, self.month);
        let (py, pm) = shift(self.year, self.month, -1);
        let prev_len = days_in_month(py, pm);
        (0..42)
            .map(|i| {
                let n = i as i64 - i64::from(lead) + 1;
                if n < 1 {
                    (py, pm, (i64::from(prev_len) + n) as u32)
                } else if n > i64::from(days_in_month(self.year, self.month)) {
                    let (ny, nm) = shift(self.year, self.month, 1);
                    (
                        ny,
                        nm,
                        (n - i64::from(days_in_month(self.year, self.month))) as u32,
                    )
                } else {
                    (self.year, self.month, n as u32)
                }
            })
            .collect()
    }

    /// The month arrows: the day clamps to the new month's length.
    pub fn shift_month(&mut self, by: i32) {
        (self.year, self.month) = shift(self.year, self.month, by);
        self.day = self.day.min(days_in_month(self.year, self.month));
    }

    /// Arrow keys move the selection by `days` across month ends.
    pub fn move_days(&mut self, days: i32) {
        let mut d = self.day as i32 + days;
        while d < 1 {
            self.shift_month(-1);
            d += days_in_month(self.year, self.month) as i32;
        }
        while d > days_in_month(self.year, self.month) as i32 {
            d -= days_in_month(self.year, self.month) as i32;
            self.shift_month(1);
        }
        self.day = d as u32;
    }

    pub fn value(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

fn shift(year: i32, month: u32, by: i32) -> (i32, u32) {
    let m = year * 12 + month as i32 - 1 + by;
    (m.div_euclid(12), (m.rem_euclid(12) + 1) as u32)
}

/// What a click or key in the calendar did.
pub enum CalendarAction {
    /// The shown month or the selection moved; `select` when a day was chosen.
    Changed {
        select: bool,
    },
    Close,
}

/// The popover's content: header switchers, weekday row and day grid, in the GTK theme's
/// light or dark popover colors. `on` receives every action with the new state.
pub fn render(
    id: &str,
    cal: Calendar,
    today: (i32, u32, u32),
    dark: bool,
    focus: &FocusHandle,
    on: impl Fn(Calendar, CalendarAction, &mut Window, &mut App) + Clone + 'static,
) -> AnyElement {
    let (bg, fg) = if dark {
        (hex(0x383838), hex(0xffffff))
    } else {
        (hex(0xffffff), hex(0x000000).with_alpha(0.8))
    };
    let dim = fg.with_alpha(fg.a * 0.45);
    let arrow = |name: &str, label: &'static str, cal: Calendar, by: i32| {
        let on = on.clone();
        let sel = format!("{id}-{name}");
        div()
            .id(SharedString::from(sel.clone()))
            .debug_selector(move || sel.clone())
            .px(px(6.))
            .rounded(px(6.))
            .cursor_pointer()
            .child(text(label).size(13.).color(fg))
            .on_click(move |_, window, cx| {
                let mut c = cal;
                c.shift_month(by);
                on(c, CalendarAction::Changed { select: false }, window, cx);
            })
    };
    let switcher = |name: &str, label: String, step: i32| -> Div {
        div()
            .flex()
            .items_center()
            .child(arrow(&format!("{name}-prev"), "‹", cal, -step))
            .child(
                div()
                    .min_w(px(if step == 1 { 76. } else { 40. }))
                    .flex()
                    .justify_center()
                    .child(text(label).size(13.).color(fg)),
            )
            .child(arrow(&format!("{name}-next"), "›", cal, step))
    };
    let header = div()
        .flex()
        .justify_between()
        .pb(px(6.))
        .child(switcher(
            "month",
            MONTHS[(cal.month - 1) as usize].to_string(),
            1,
        ))
        .child(switcher("year", cal.year.to_string(), 12));
    let mut grid = div().flex().flex_col();
    let mut row = div().flex();
    for w in WEEKDAYS {
        row = row.child(cell().child(text(w).size(11.).color(dim)));
    }
    grid = grid.child(row);
    for week in cal.grid().chunks(7) {
        let mut row = div().flex();
        for &(y, m, d) in week {
            let selected = (y, m, d) == (cal.year, cal.month, cal.day);
            let inside = m == cal.month;
            let is_today = (y, m, d) == today;
            let on = on.clone();
            let sel = format!("{id}-d{y}-{m}-{d}");
            row = row.child(
                cell()
                    .id(SharedString::from(sel.clone()))
                    .debug_selector(move || sel.clone())
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(selected, |c| c.bg(Hsla::from(ACCENT)))
                    .child({
                        let t = text(d.to_string()).size(13.).color(if selected {
                            hex(0xffffff)
                        } else if inside {
                            fg
                        } else {
                            dim
                        });
                        if is_today {
                            t.weight(crate::fonts::weight::BOLD)
                        } else {
                            t
                        }
                    })
                    .on_click(move |_, window, cx| {
                        on(
                            Calendar::new(y, m, d),
                            CalendarAction::Changed { select: true },
                            window,
                            cx,
                        )
                    }),
            );
        }
        grid = grid.child(row);
    }
    let keys = on.clone();
    let outside = on.clone();
    div()
        .id(SharedString::from(id.to_string()))
        .debug_selector({
            let id = id.to_string();
            move || id.clone()
        })
        .track_focus(focus)
        .key_context("TextEntry")
        .p(px(8.))
        .rounded(px(12.))
        .bg(Hsla::from(bg))
        .border(px(1.))
        .border_color(Hsla::from(hex(0x000000).with_alpha(0.14)))
        .shadow_lg()
        .child(header)
        .child(grid)
        .on_key_down(move |e: &KeyDownEvent, window, cx| {
            let mut c = cal;
            let action = match e.keystroke.key.as_str() {
                "left" => Some(-1),
                "right" => Some(1),
                "up" => Some(-7),
                "down" => Some(7),
                "escape" => {
                    cx.stop_propagation();
                    keys(c, CalendarAction::Close, window, cx);
                    return;
                }
                _ => None,
            };
            if let Some(days) = action {
                c.move_days(days);
                cx.stop_propagation();
                keys(c, CalendarAction::Changed { select: true }, window, cx);
            }
        })
        .on_mouse_down_out(move |_, window, cx| outside(cal, CalendarAction::Close, window, cx))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .into_any_element()
}

fn cell() -> Div {
    div()
        .w(px(30.))
        .h(px(26.))
        .flex()
        .items_center()
        .justify_center()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grids_start_on_the_sunday_before_the_first() {
        let g = Calendar::new(2026, 10, 27).grid();
        assert_eq!(g.len(), 42);
        // October 1, 2026 is a Thursday.
        assert_eq!(
            &g[..5],
            &[
                (2026, 9, 27),
                (2026, 9, 28),
                (2026, 9, 29),
                (2026, 9, 30),
                (2026, 10, 1)
            ]
        );
        assert_eq!(g[41], (2026, 11, 7));
        assert_eq!(first_weekday(2024, 2), 4, "February 1, 2024 is a Thursday");
    }

    #[test]
    fn month_arrows_clamp_and_keys_cross_month_ends() {
        let mut c = Calendar::new(2026, 1, 31);
        c.shift_month(1);
        assert_eq!(c, Calendar::new(2026, 2, 28));
        c.shift_month(-12);
        assert_eq!(c, Calendar::new(2025, 2, 28));
        let mut c = Calendar::new(2026, 12, 30);
        c.move_days(7);
        assert_eq!(c.value(), "2027-01-06");
        c.move_days(-7);
        assert_eq!(c.value(), "2026-12-30");
    }
}
