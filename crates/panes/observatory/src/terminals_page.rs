//! The Terminals route: a [`TerminalDock`] over the shells of no workspace
//! ([`GLOBAL_TERMINALS`]), while Settings turns the page on. The dock mounts on the first
//! visit and stays while hidden, so its shells keep streaming; turning the page off drops it
//! (the shells keep running and the next dock adopts them).

use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, Subscription, Window, div,
};
use recur_services::api::GLOBAL_TERMINALS;
use recur_services::events::MenuState;
use recur_ui::guard::Guarded;
use recur_view_core::menu::menu_state_idle;
use recur_view_core::state::ShellEvent;
use recur_view_core::{ObserveState as _, Topics, ViewContext, ViewName};

use crate::dock::{DockEvent, DockProps, TerminalDock};

pub struct TerminalsPage {
    vcx: ViewContext,
    dock: Option<(Entity<TerminalDock>, Subscription)>,
    on: bool,
    _subs: Vec<Subscription>,
}

impl TerminalsPage {
    pub fn new(vcx: &ViewContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs = vec![
            cx.observe_state_in(&vcx.state, Topics::ALL, window, |this, _, window, cx| {
                this.sync(window, cx)
            }),
            // ⌥⌘5 and Navigate → Terminals focus the shell, also when the page is already up.
            cx.guarded_subscribe_in(
                &vcx.state,
                window,
                |this, _, ev: &ShellEvent, window, cx| {
                    let ShellEvent::MenuAction(a) = ev else {
                        return;
                    };
                    if a.as_ref() == "view:terminals" {
                        this.sync(window, cx);
                        this.focus(window, cx);
                    } else if this.on {
                        this.on_menu_action(a, window, cx);
                    }
                },
            ),
        ];
        let mut me = TerminalsPage {
            vcx: vcx.clone(),
            dock: None,
            on: false,
            _subs: subs,
        };
        me.sync(window, cx);
        me
    }

    pub fn dock(&self) -> Option<&Entity<TerminalDock>> {
        self.dock.as_ref().map(|(d, _)| d)
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.vcx.state.read(cx);
        let page = s.terminal_prefs.page;
        let on = page && s.view == ViewName::Terminals;
        if !page && self.dock.take().is_some() {
            cx.notify();
        }
        if on && self.dock.is_none() {
            let props = DockProps {
                lit: true,
                gpu: true,
                ..Default::default()
            };
            let state = self.vcx.state.clone();
            let d = cx.new(|cx| TerminalDock::new(state, GLOBAL_TERMINALS, props, window, cx));
            // The first shell mounts after the page took focus: hand it to the shell.
            let sub = cx.guarded_subscribe_in(&d, window, |this, d, _: &DockEvent, window, cx| {
                d.update(cx, |d, cx| {
                    if d.focus.is_focused(window) {
                        d.focus_active(window, cx);
                    }
                });
                if this.on {
                    this.push_menu_state(cx);
                }
            });
            self.dock = Some((d, sub));
            cx.notify();
        }
        if on && !self.on {
            self.focus(window, cx);
        }
        if on {
            self.push_menu_state(cx);
        } else if self.on && cx.default_global::<crate::Owners>().menu == GLOBAL_TERMINALS {
            cx.default_global::<crate::Owners>().menu.clear();
            self.vcx
                .state
                .update(cx, |s, cx| s.set_menu_state(menu_state_idle(), cx));
        }
        self.on = on;
    }

    /// The menubar's tab items drive this page's dock, as they do the Observatory's.
    fn push_menu_state(&mut self, cx: &mut Context<Self>) {
        let (tabs, active) = self.dock().map_or((0, false), |d| {
            let d = d.read(cx);
            (d.tabs().len(), !d.active_id().is_empty())
        });
        cx.default_global::<crate::Owners>().menu = GLOBAL_TERMINALS.into();
        let patch = MenuState {
            new_tab: Some(true),
            close_tab: Some(active),
            prev_tab: Some(tabs > 1),
            next_tab: Some(tabs > 1),
            ..menu_state_idle()
        };
        self.vcx
            .state
            .update(cx, |s, cx| s.set_menu_state(patch, cx));
    }

    fn on_menu_action(&mut self, action: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.dock().cloned() else {
            return;
        };
        d.update(cx, |d, cx| match action {
            "new-tab" => d.new_tab(window, cx),
            "close-tab" => d.close_active_tab(window, cx),
            "prev-tab" => d.rotate_tab(-1, window, cx),
            "next-tab" => d.rotate_tab(1, window, cx),
            _ => {}
        });
    }

    fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((d, _)) = &self.dock {
            d.update(cx, |d, cx| d.focus_active(window, cx));
        }
    }
}

impl TerminalsPage {
    fn render_view(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .debug_selector(|| "v-terminals".into())
            .size_full()
            .flex()
            .flex_col()
            .children(self.dock().cloned())
    }
}

impl Render for TerminalsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}
