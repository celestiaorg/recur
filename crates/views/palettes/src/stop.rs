//! The Stop palette (⌘⇧R: pick a running command to stop).

use recur_ui::guard::Guarded;
use recur_view_core::{ObserveState as _, Topics};
use std::collections::BTreeMap;

use gpui_kit::base::input::InputState;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, Focusable, InteractiveElement, IntoElement,
    KeystrokeEvent, MouseButton, MouseMoveEvent, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};
use recur_ui::fonts::Family;
use recur_ui::kbd::kbd;
use recur_ui::shadow::with_shadow;
use recur_ui::style::{
    PALETTE_SHADOW, command_palette_panel, footer_key, palette_empty, palette_footer,
    palette_input_row, palette_row, palette_scrim, style_text_input, text_field,
};
use recur_ui::text::{LineHeight, Text, TextStyled, span, text};
use recur_ui::{IconName, Theme, icon};
use recur_view_core::AppState;
use recur_view_core::run::workspace_runs;

use recur_ui::logic::hover_gate::HoverGate;
use recur_ui::set::hsla;

/// A Stop row: one running command, or `all` when several are going.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopRow {
    pub id: String,
    pub name: String,
    pub cmd: String,
    pub all: bool,
}

/// `all`: the running commands with their command lines, then Stop all when there are several.
pub fn stop_rows(running: &[String], cmds: &BTreeMap<String, String>) -> Vec<StopRow> {
    let mut rows: Vec<StopRow> = running
        .iter()
        .map(|n| StopRow {
            id: n.clone(),
            name: n.clone(),
            cmd: cmds.get(n).cloned().unwrap_or_default(),
            all: false,
        })
        .collect();
    if running.len() > 1 {
        rows.push(StopRow {
            id: "__all__".into(),
            name: "all".into(),
            cmd: running.join(", "),
            all: true,
        });
    }
    rows
}

/// `flat`: rows whose name, command or `stop <name>` contains the query.
pub fn filter_stop_rows(rows: &[StopRow], q: &str) -> Vec<StopRow> {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        return rows.to_vec();
    }
    rows.iter()
        .filter(|r| {
            r.name.to_lowercase().contains(&q)
                || r.cmd.to_lowercase().contains(&q)
                || format!("stop {}", r.name).to_lowercase().contains(&q)
        })
        .cloned()
        .collect()
}

/// `paletteRowDigit(e, { shiftDigits: true })`.
fn row_digit(ev: &KeystrokeEvent) -> Option<usize> {
    let k = &ev.keystroke;
    if k.modifiers.platform || k.modifiers.control || k.modifiers.alt {
        return None;
    }
    match k.key.as_str() {
        d @ ("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9") => d.parse().ok(),
        _ => None,
    }
}

pub struct StopPalette {
    state: Entity<AppState>,
    input: Entity<InputState>,
    sel: usize,
    hover: HoverGate,
    scroll: ScrollHandle,
    was_open: bool,
    _subs: Vec<Subscription>,
}

impl StopPalette {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let me = cx.entity().downgrade();
        let subs = vec![
            cx.observe_state_in(&state, Topics::ALL, window, |this, _, window, cx| {
                this.sync_open(window, cx)
            }),
            cx.guarded_subscribe(
                &input,
                |this, _, _: &gpui_kit::base::input::InputEvent, cx| {
                    this.clamp_sel(cx);
                    cx.notify();
                },
            ),
            cx.intercept_keystrokes(move |ev, window, cx| {
                let Some(me) = me.upgrade() else { return };
                if me.update(cx, |p, cx| p.on_key(ev, window, cx)) {
                    cx.stop_propagation();
                }
            }),
        ];
        StopPalette {
            state,
            input,
            sel: 0,
            hover: HoverGate::default(),
            scroll: ScrollHandle::new(),
            was_open: false,
            _subs: subs,
        }
    }

    fn open(&self, cx: &gpui_kit::App) -> bool {
        self.state.read(cx).overlays.stop_menu
    }

    fn close(&self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.overlays.stop_menu = false;
            cx.notify();
        });
    }

    fn rows(&self, cx: &gpui_kit::App) -> Vec<StopRow> {
        let s = self.state.read(cx);
        let ws = &s.selected_workspace_id;
        let running: Vec<String> = s.run.running_runs(ws).into_iter().map(|(n, _)| n).collect();
        let cmds: BTreeMap<String, String> = workspace_runs(ws, &s.workspaces, &s.projects)
            .into_iter()
            .map(|r| (r.name, r.cmd))
            .collect();
        stop_rows(&running, &cmds)
    }

    fn flat(&self, cx: &gpui_kit::App) -> Vec<StopRow> {
        filter_stop_rows(&self.rows(cx), &self.input.read(cx).value())
    }

    fn clamp_sel(&mut self, cx: &gpui_kit::App) {
        let n = self.flat(cx).len();
        if self.sel >= n {
            self.sel = n.saturating_sub(1);
        }
    }

    fn sync_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.open(cx);
        // A closed palette has nothing to follow: the store changes many times a second.
        if !open && !self.was_open {
            return;
        }
        if open && !self.was_open {
            self.input.update(cx, |s, cx| s.set_value("", window, cx));
            self.sel = 0;
            self.hover.reset();
            let focus = self.input.focus_handle(cx);
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
        }
        // The hidden field would keep focus; WebKit blurs a field that stops rendering.
        if !open && self.was_open && self.input.focus_handle(cx).is_focused(window) {
            window.blur(cx);
        }
        self.was_open = open;
        if open && self.rows(cx).is_empty() {
            self.close(cx);
        }
        self.clamp_sel(cx);
        cx.notify();
    }

    fn pick(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(r) = self.flat(cx).get(i).cloned() else {
            return;
        };
        let ws = self.state.read(cx).selected_workspace_id.clone();
        self.state.update(cx, |s, cx| {
            s.overlays.stop_menu = false;
            if r.all {
                s.request_stop_all(&ws, cx);
            } else {
                s.request_stop_run(&ws, &r.name, cx);
            }
            cx.notify();
        });
    }

    fn on_key(&mut self, ev: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.open(cx) || !self.input.focus_handle(cx).is_focused(window) {
            return false;
        }
        let k = &ev.keystroke;
        let m = k.modifiers;
        let n = self.flat(cx).len();
        match k.key.as_str() {
            "down" | "up" if !(m.platform || m.control || m.alt) => {
                self.sel = if k.key == "down" {
                    (self.sel + 1).min(n.saturating_sub(1))
                } else {
                    self.sel.saturating_sub(1)
                };
                self.scroll.scroll_to_item(self.sel);
                cx.notify();
                true
            }
            "enter" => {
                self.pick(self.sel, cx);
                true
            }
            "escape" => {
                self.close(cx);
                true
            }
            _ if self.input.read(cx).value().trim().is_empty() => match row_digit(ev) {
                Some(d) if d - 1 < n => {
                    self.pick(d - 1, cx);
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }
}

impl StopPalette {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        if !self.open(cx) {
            return div().into_any_element();
        }
        style_text_input(&self.input, cx);
        let t = Theme::of(cx).tokens;
        let flat = self.flat(cx);
        let filtered = !self.input.read(cx).value().trim().is_empty();
        let placeholder: SharedString = "Stop…".into();
        let head = palette_input_row(true, cx)
            .child(
                icon(IconName::SquareFill)
                    .size(px(14.))
                    .color(hsla(t.faint)),
            )
            .child(div().flex_1().min_w_0().flex().p(px(1.)).child(text_field(
                &self.input,
                &placeholder,
                t.faint,
                Family::Ui,
                13.,
                window,
                cx,
            )))
            .child(
                div()
                    .flex_none()
                    .border_1()
                    .border_color(hsla(t.line))
                    .rounded(px(4.))
                    .py(px(2.))
                    .px(px(6.))
                    .child(kbd("⌘⇧R").size(9.5).color(t.faint)),
            );
        let list: AnyElement = if flat.is_empty() {
            palette_empty(
                if filtered {
                    "No matching processes"
                } else {
                    "Nothing running"
                },
                true,
                cx,
            )
            .into_any_element()
        } else {
            let rows =
                flat.iter().enumerate().map(|(i, r)| {
                    let on = i == self.sel;
                    let sel = format!("stop-{}", r.id);
                    palette_row(SharedString::from(sel.clone()), on, true, cx)
                        .debug_selector(|| sel)
                        .gap(px(10.))
                        .child(
                            text(format!("Stop {}", r.name))
                                .size(12.5)
                                .line_height(LineHeight::Scale(1.3))
                                .color(t.text)
                                .nowrap(),
                        )
                        .when(!r.cmd.is_empty(), |d| {
                            d.child(div().flex_none().ml(px(8.)).min_w_0().child(
                                text(r.cmd.clone()).mono().size(10.).color(t.faint).nowrap(),
                            ))
                        })
                        .when(!filtered && i < 9, |d| {
                            let (bg, border, fg) = if on {
                                (t.pane, t.line_bright, t.text)
                            } else {
                                (t.pane_2, t.line, t.muted)
                            };
                            d.child(
                                div()
                                    .ml_auto()
                                    .flex_none()
                                    .min_w(px(18.))
                                    .flex()
                                    .justify_center()
                                    .bg(hsla(bg))
                                    .border_1()
                                    .border_color(hsla(border))
                                    .rounded(px(4.))
                                    .py(px(1.))
                                    .px(px(5.))
                                    .child(
                                        // `.pk`: weight 500, `.02em`.
                                        text(format!("{}", i + 1))
                                            .mono()
                                            .size(11.)
                                            .weight(recur_ui::fonts::weight::MEDIUM)
                                            .tracking(0.02)
                                            .color(fg)
                                            .nowrap(),
                                    ),
                            )
                        })
                        .on_mouse_move(cx.guarded_listener(move |p, ev: &MouseMoveEvent, _, cx| {
                            if p.hover
                                .allows(f32::from(ev.position.x), f32::from(ev.position.y))
                                && p.sel != i
                            {
                                p.sel = i;
                                cx.notify();
                            }
                        }))
                        .on_click(cx.guarded_listener(move |p, _, _, cx| p.pick(i, cx)))
                        .into_any_element()
                });
            div()
                .id("stop-list")
                .track_scroll(&self.scroll)
                .overflow_y_scroll()
                .min_h_0()
                .p(px(4.))
                .flex()
                .flex_col()
                .children(rows)
                .into_any_element()
        };
        // GPUI clips rectangles: the footer rounds itself inside the 10px panel.
        let foot = palette_footer(true, cx)
            .rounded_b(px(9.))
            .flex_wrap()
            .gap_x(px(14.))
            .gap_y(px(8.))
            .child(Text::from_spans(vec![
                footer_key("↑↓", cx),
                span("navigate"),
            ]))
            .child(Text::from_spans(vec![footer_key("1–9", cx), span("stop")]))
            .child(Text::from_spans(vec![footer_key("↵", cx), span("stop")]))
            .child(Text::from_spans(vec![footer_key("esc", cx), span("close")]));
        let vh = f32::from(window.viewport_size().height);
        let panel = command_palette_panel(cx)
            .w_full()
            .max_h(px(vh * 0.56))
            .child(head)
            .child(list)
            .child(foot);
        let width = 420f32.min(f32::from(window.viewport_size().width) * 0.92);
        let panel = with_shadow(panel, &PALETTE_SHADOW, 10.)
            .id("stop-palette")
            .w(px(width))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        palette_scrim(window, panel)
            .id("stop-palette-bg")
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(|p, _, _, cx| p.close(cx)),
            )
            .into_any_element()
    }
}

impl Render for StopPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_rows_and_filter() {
        let cmds = BTreeMap::from([("dev".to_string(), "pnpm dev".to_string())]);
        let one = stop_rows(&["dev".into()], &cmds);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].cmd, "pnpm dev");
        let two = stop_rows(&["dev".into(), "test".into()], &cmds);
        assert_eq!(two.last().unwrap().cmd, "dev, test");
        assert!(two.last().unwrap().all);
        assert_eq!(filter_stop_rows(&two, "PNPM").len(), 1);
        assert_eq!(
            filter_stop_rows(&two, "stop t")
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            ["test"]
        );
        assert_eq!(filter_stop_rows(&two, " ").len(), 3);
    }
}
