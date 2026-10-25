//! `StopPalette.svelte` (⌘⇧R: pick a running command to stop) and `CommandPrompts.svelte`
//! (a bundle command's `prompts` form before it runs).

use recur_ui::guard::Guarded;
use recur_view_core::{ObserveState as _, Topics};
use std::collections::BTreeMap;

use gpui_kit::base::input::InputState;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, Focusable, InteractiveElement,
    IntoElement, KeystrokeEvent, MouseButton, MouseMoveEvent, ParentElement, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};
use recur_ui::fonts::Family;
use recur_ui::kbd::kbd;
use recur_ui::shadow::with_shadow;
use recur_ui::style::{
    PALETTE_SHADOW, command_palette_panel, esc_key, footer_key, palette_empty, palette_footer,
    palette_input_row, palette_row, palette_scrim, style_text_input, text_field,
};
use recur_ui::text::{LineHeight, Text, TextStyled, span, text};
use recur_ui::widgets::select::{Select, SelectEvent, SelectOption};
use recur_ui::{IconName, Theme, hex, icon};
use recur_view_core::AppState;
use recur_view_core::commands::CommandPrompt;
use recur_view_core::run::workspace_runs;

use crate::style::hsla;
use recur_ui::logic::hover_gate::HoverGate;

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
                                        text(format!("{}", i + 1))
                                            .mono()
                                            .size(11.)
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandPromptsEvent {
    /// `onResolve(values)`.
    Resolve(BTreeMap<String, String>),
    Cancel,
}

enum Field {
    Text(Entity<InputState>),
    Choice(Entity<Select>, SharedString),
}

/// `CommandPrompts.svelte`: open it with [`CommandPrompts::open`]; it emits
/// [`CommandPromptsEvent`].
pub struct CommandPrompts {
    title: String,
    prompts: Vec<CommandPrompt>,
    fields: Vec<Field>,
    is_open: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<CommandPromptsEvent> for CommandPrompts {}

impl CommandPrompts {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let me = cx.entity().downgrade();
        let sub = cx.intercept_keystrokes(move |ev, window, cx| {
            let Some(me) = me.upgrade() else { return };
            if me.update(cx, |p, cx| p.on_key(ev, window, cx)) {
                cx.stop_propagation();
            }
        });
        CommandPrompts {
            title: "Command".into(),
            prompts: Vec::new(),
            fields: Vec::new(),
            is_open: false,
            _subs: vec![sub],
        }
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    /// Opens on `prompts`, each field at its default, and focuses the first.
    pub fn open(
        &mut self,
        title: &str,
        prompts: Vec<CommandPrompt>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.title = if title.is_empty() {
            "Command".into()
        } else {
            title.to_string()
        };
        let mut subs = self._subs.drain(..1).collect::<Vec<_>>();
        self.fields = prompts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if p.r#type == "choice" && !p.choices.is_empty() {
                    let opts = p
                        .choices
                        .iter()
                        .map(|c| SelectOption::new(c.clone(), c.clone()))
                        .collect();
                    let select = cx.new(|cx| {
                        let mut s = Select::new(opts, window, cx);
                        s.set_value(p.default.clone(), cx);
                        s
                    });
                    subs.push(cx.guarded_subscribe(
                        &select,
                        move |this, _, ev: &SelectEvent, cx| {
                            if let (SelectEvent::Change(v), Some(Field::Choice(_, cur))) =
                                (ev, this.fields.get_mut(i))
                            {
                                *cur = v.clone();
                                cx.notify();
                            }
                        },
                    ));
                    Field::Choice(select, p.default.clone().into())
                } else {
                    let masked = p.r#type == "password";
                    let default = p.default.clone();
                    Field::Text(cx.new(|cx| {
                        let mut s = InputState::new(window, cx).masked(masked);
                        s.set_value(default, window, cx);
                        s
                    }))
                }
            })
            .collect();
        self._subs = subs;
        self.prompts = prompts;
        self.is_open = true;
        if let Some(Field::Text(first)) = self.fields.first() {
            let focus = first.focus_handle(cx);
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
        }
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.is_open = false;
        cx.notify();
    }

    fn values(&self, cx: &gpui_kit::App) -> BTreeMap<String, String> {
        self.prompts
            .iter()
            .zip(&self.fields)
            .map(|(p, f)| {
                let v = match f {
                    Field::Text(i) => i.read(cx).value().to_string(),
                    Field::Choice(_, v) => v.to_string(),
                };
                (p.name.clone(), v)
            })
            .collect()
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let values = self.values(cx);
        cx.emit(CommandPromptsEvent::Resolve(values));
    }

    fn on_key(&mut self, ev: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.is_open {
            return false;
        }
        let inside = self.fields.iter().any(|f| match f {
            Field::Text(i) => i.focus_handle(cx).is_focused(window),
            Field::Choice(s, _) => s.focus_handle(cx).contains_focused(window, cx),
        });
        if !inside {
            return false;
        }
        match ev.keystroke.key.as_str() {
            "escape" => {
                cx.emit(CommandPromptsEvent::Cancel);
                true
            }
            "enter" => {
                self.submit(cx);
                true
            }
            _ => false,
        }
    }
}

impl CommandPrompts {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        if !self.is_open {
            return div().into_any_element();
        }
        let t = Theme::of(cx).tokens;
        // `var(--border, #2a2f38)`: `--border` is not a theme token.
        let border = hex(0x2a2f38);
        for f in &self.fields {
            if let Field::Text(i) = f {
                style_text_input(i, cx);
            }
        }
        let empty: SharedString = SharedString::default();
        let fields = self.prompts.iter().zip(&self.fields).map(|(p, f)| {
            let label = if p.label.is_empty() {
                p.name.clone()
            } else {
                p.label.clone()
            };
            let control: AnyElement = match f {
                Field::Text(i) => recur_ui::style::card(t.pane_2, border, 6.)
                    .flex()
                    .py(px(7.))
                    .px(px(9.))
                    .child(text_field(i, &empty, t.faint, Family::Ui, 13., window, cx))
                    .into_any_element(),
                Field::Choice(s, _) => s.clone().into_any_element(),
            };
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    text(label)
                        .size(11.)
                        .color(t.muted)
                        .uppercase()
                        .tracking(0.04)
                        .line_height(LineHeight::Normal),
                )
                .child(control)
        });
        let head = palette_input_row(false, cx)
            .child(icon(IconName::Settings).size(px(16.)).color(hsla(t.faint)))
            .child(
                div().flex_1().min_w_0().child(
                    text(self.title.clone())
                        .size(13.)
                        .weight(600.)
                        .color(t.text)
                        .nowrap(),
                ),
            )
            .child(esc_key(false, cx));
        let body = div()
            .py(px(12.))
            .px(px(14.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .children(fields);
        let amber = t.amber;
        let actions = div()
            .flex()
            .justify_end()
            .items_center()
            .gap(px(10.))
            .pt(px(10.))
            .px(px(14.))
            .pb(px(12.))
            .border_t_1()
            .border_color(hsla(border))
            .child(
                div()
                    .id("cmd-cancel")
                    .cursor_pointer()
                    .child(text("Cancel").color(t.text).line_height(LineHeight::Normal))
                    .on_click(
                        cx.guarded_listener(|_, _, _, cx| cx.emit(CommandPromptsEvent::Cancel)),
                    ),
            )
            .child(
                div()
                    .id("cmd-run")
                    .cursor_pointer()
                    .bg(hsla(amber))
                    .rounded(px(6.))
                    .py(px(6.))
                    .px(px(12.))
                    .child(
                        text("Run")
                            .weight(600.)
                            .color(t.on_amber)
                            .line_height(LineHeight::Normal),
                    )
                    .on_click(cx.guarded_listener(|p, _, _, cx| p.submit(cx))),
            );
        let vh = f32::from(window.viewport_size().height);
        let panel = recur_ui::style::palette_panel(window, cx)
            .max_h(px(vh * 0.64))
            .child(head)
            .child(body)
            .child(actions);
        let width = 420f32.min(f32::from(window.viewport_size().width) * 0.92);
        let panel = with_shadow(panel, &PALETTE_SHADOW, 13.)
            .id("cmd-prompts")
            .w(px(width))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        palette_scrim(window, panel)
            .id("cmd-prompts-bg")
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(|_, _, _, cx| cx.emit(CommandPromptsEvent::Cancel)),
            )
            .into_any_element()
    }
}

impl Render for CommandPrompts {
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
