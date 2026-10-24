//! `RunPalette.svelte`: the named-run picker (⌘R, View → Run…, the titlebar chevron). Same
//! keys as Open in: filter, ↑↓, 1–9, ↵ runs, ⌘↩ or the pin makes a run the project default.

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Focusable, InteractiveElement, IntoElement,
    KeystrokeEvent, MouseMoveEvent, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};
use recur_domain::RunCommand;
use recur_ui::IconName;
use recur_ui::logic::hover_gate::HoverGate;
use recur_ui::style::{palette_empty, palette_row, palette_sub, style_text_input};
use recur_view_core::AppState;
use recur_view_core::run::{
    filter_runs, order_runs, project_for_workspace, workspace_run, workspace_runs,
};

use crate::chrome::{self, Kind, cmd_or_ctrl, plain, row_digit, step};
use crate::open_in::stretch_sub;

pub struct RunPalette {
    state: Entity<AppState>,
    input: Entity<InputState>,
    sel: usize,
    hover: HoverGate,
    pin_hover: Option<String>,
    scroll: ScrollHandle,
    revealed: Option<(usize, Vec<String>)>,
    last_query: String,
    was_open: bool,
    _subs: Vec<Subscription>,
}

impl RunPalette {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let me = cx.entity().downgrade();
        let subs = vec![
            cx.observe_in(&state, window, |this, _, window, cx| {
                this.sync_open(window, cx)
            }),
            cx.subscribe(&input, |this, _, _: &InputEvent, cx| {
                this.query_changed(cx);
                cx.notify();
            }),
            cx.intercept_keystrokes(move |ev, window, cx| {
                let Some(me) = me.upgrade() else { return };
                if me.update(cx, |p, cx| p.on_key(ev, window, cx)) {
                    cx.stop_propagation();
                }
            }),
        ];
        RunPalette {
            state,
            input,
            sel: 0,
            hover: HoverGate::default(),
            pin_hover: None,
            scroll: ScrollHandle::new(),
            revealed: None,
            last_query: String::new(),
            was_open: false,
            _subs: subs,
        }
    }

    fn open(&self, cx: &App) -> bool {
        self.state.read(cx).overlays.run_menu
    }

    /// `pin`: the project's default run.
    fn pin(&self, cx: &App) -> String {
        let s = self.state.read(cx);
        workspace_run(&s.selected_workspace_id, &s.workspaces, &s.projects)
            .map(|r| r.name)
            .unwrap_or_default()
    }

    fn query(&self, cx: &App) -> String {
        self.input.read(cx).value().trim().to_lowercase()
    }

    fn flat(&self, cx: &App) -> Vec<RunCommand> {
        let s = self.state.read(cx);
        let all = order_runs(
            workspace_runs(&s.selected_workspace_id, &s.workspaces, &s.projects),
            &self.pin(cx),
        );
        filter_runs(&all, &self.query(cx))
    }

    fn clamp_sel(&mut self, cx: &App) {
        let n = self.flat(cx).len();
        if self.sel >= n {
            self.sel = n.saturating_sub(1);
        }
    }

    fn sync_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.open(cx);
        if open && !self.was_open {
            self.input.update(cx, |s, cx| s.set_value("", window, cx));
            self.sel = 0;
            self.hover.reset();
            let focus = self.input.focus_handle(cx);
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
        }
        if !open && self.was_open {
            chrome::release_focus(&self.input, window, cx);
        }
        self.was_open = open;
        self.clamp_sel(cx);
        cx.notify();
    }

    /// A new query starts over at the first row with the hover gate re-armed.
    fn query_changed(&mut self, cx: &App) {
        let q = self.query(cx);
        if q != self.last_query {
            self.last_query = q;
            self.sel = 0;
            self.hover.reset();
        }
        self.clamp_sel(cx);
    }

    fn close(&self, cx: &mut App) {
        self.state
            .update(cx, |s, cx| s.close_overlay(|o| o.run_menu = false, cx));
    }

    fn make_default(&mut self, name: &str, cx: &mut Context<Self>) {
        let project = {
            let s = self.state.read(cx);
            project_for_workspace(&s.selected_workspace_id, &s.workspaces, &s.projects).cloned()
        };
        let Some(project) = project.filter(|_| !name.is_empty()) else {
            return;
        };
        let pinned = self
            .state
            .update(cx, |s, cx| s.pin_project_run(&project, name, cx));
        cx.spawn(async move |this, cx| {
            if pinned.await {
                let _ = this.update(cx, |p, cx| {
                    p.sel = 0;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn run(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(r) = self.flat(cx).get(i).cloned() else {
            return;
        };
        self.state.update(cx, |s, cx| {
            s.overlays.run_menu = false;
            let id = s.selected_workspace_id.clone();
            s.request_run_dev(&id, Some(r), cx);
            cx.notify();
        });
    }

    fn on_key(&mut self, ev: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.open(cx) || !self.input.focus_handle(cx).is_focused(window) {
            return false;
        }
        let k = &ev.keystroke;
        let n = self.flat(cx).len();
        match k.key.as_str() {
            "down" | "up" => {
                if !plain(k) {
                    return false;
                }
                self.sel = step(self.sel, n, k.key == "down");
                cx.notify();
                true
            }
            "enter" if cmd_or_ctrl(k) => {
                if let Some(r) = self.flat(cx).get(self.sel) {
                    let name = r.name.clone();
                    self.make_default(&name, cx);
                }
                true
            }
            "enter" => {
                self.run(self.sel, cx);
                true
            }
            "escape" => {
                self.close(cx);
                true
            }
            _ if self.input.read(cx).value().trim().is_empty() => match row_digit(k) {
                Some(d) if d - 1 < n => {
                    self.run(d - 1, cx);
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }
}

impl Render for RunPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open(cx) {
            return div().into_any_element();
        }
        style_text_input(&self.input, cx);
        let flat = self.flat(cx);
        let pin = self.pin(cx);
        let query = self.query(cx);
        let ws = self.state.read(cx).selected_workspace_id.clone();
        let placeholder: SharedString = "Run…".into();
        let head = chrome::head(
            Kind::OpenIn,
            IconName::Play,
            &self.input,
            &placeholder,
            "⌘R",
            window,
            cx,
        );
        let mut rows: Vec<AnyElement> = Vec::new();
        if flat.is_empty() {
            let msg = if query.is_empty() {
                "No run commands"
            } else {
                "No matching commands"
            };
            rows.push(palette_empty(msg, true, cx).into_any_element());
        } else {
            for (idx, r) in flat.iter().enumerate() {
                let on = idx == self.sel;
                let name = r.name.clone();
                let is_pin = r.name == pin;
                let running = self.state.read(cx).run.is_run_running(&ws, &r.name);
                let sel_name = name.clone();
                let mut row =
                    palette_row(SharedString::from(format!("run-row:{name}")), on, true, cx)
                        .gap(px(10.))
                        .debug_selector(move || {
                            format!("run-row:{sel_name}{}", if on { ":sel" } else { "" })
                        })
                        .child(chrome::label(
                            SharedString::from(format!("run-pl:{name}")),
                            r.name.clone(),
                            Kind::OpenIn,
                            cx,
                        ))
                        .child(stretch_sub(r.cmd.clone(), cx));
                if is_pin {
                    row = row.child(palette_sub("default", cx));
                }
                if running {
                    row = row.child(palette_sub("running", cx));
                }
                let hovered = self.pin_hover.as_deref() == Some(name.as_str());
                let (pin_name, hover_name) = (name.clone(), name.clone());
                row = row.child(
                    chrome::pin(
                        SharedString::from(format!("run-pin:{name}")),
                        is_pin,
                        on,
                        hovered,
                        cx,
                    )
                    .debug_selector({
                        let name = name.clone();
                        move || format!("run-pin:{name}")
                    })
                    .on_hover(cx.listener(move |p, h: &bool, _, cx| {
                        p.pin_hover = h.then(|| hover_name.clone());
                        cx.notify();
                    }))
                    .on_click(cx.listener(move |p, _, _, cx| {
                        cx.stop_propagation();
                        p.make_default(&pin_name, cx);
                    })),
                );
                if query.is_empty() && idx < 9 {
                    row = row.child(chrome::digit_key(idx + 1, on, true, cx));
                }
                rows.push(
                    row.on_mouse_move(cx.listener(move |p, ev: &MouseMoveEvent, _, cx| {
                        if p.hover
                            .allows(f32::from(ev.position.x), f32::from(ev.position.y))
                            && p.sel != idx
                        {
                            p.sel = idx;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |p, _, _, cx| p.run(idx, cx)))
                    .into_any_element(),
                );
            }
        }
        let key = (self.sel, flat.iter().map(|r| r.name.clone()).collect());
        if self.revealed.as_ref() != Some(&key) {
            if !flat.is_empty() {
                self.scroll.scroll_to_item(self.sel);
            }
            self.revealed = Some(key);
        }
        let list = chrome::list("run-list", Kind::OpenIn, &self.scroll, rows);
        let foot = chrome::footer(
            Kind::OpenIn,
            &[
                ("↑↓", "navigate"),
                ("1–9", "run"),
                ("↵", "run"),
                ("⌘↩", "default"),
                ("esc", "close"),
            ],
            cx,
        );
        let state = self.state.clone();
        chrome::frame(
            Kind::OpenIn,
            Kind::OpenIn.max_vh(),
            "run-palette",
            vec![
                head.into_any_element(),
                list.into_any_element(),
                foot.into_any_element(),
            ],
            move |_, cx| state.update(cx, |s, cx| s.close_overlay(|o| o.run_menu = false, cx)),
            window,
            cx,
        )
    }
}
