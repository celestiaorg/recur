//! The Terminals route: a [`TerminalDock`] over the shells of no workspace
//! ([`GLOBAL_TERMINALS`]), while Settings turns the page on. The dock mounts on the first
//! visit and stays while hidden, so its shells keep streaming; turning the page off drops it
//! (the shells keep running and the next dock adopts them).

use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, Subscription, Window, div,
};
use recur_services::api::GLOBAL_TERMINALS;
use recur_ui::guard::Guarded;
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
                    if matches!(ev, ShellEvent::MenuAction(a) if a.as_ref() == "view:terminals") {
                        this.sync(window, cx);
                        this.focus(window, cx);
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
            let sub = cx.guarded_subscribe_in(&d, window, |_, d, _: &DockEvent, window, cx| {
                d.update(cx, |d, cx| {
                    if d.focus.is_focused(window) {
                        d.focus_active(window, cx);
                    }
                });
            });
            self.dock = Some((d, sub));
            cx.notify();
        }
        if on && !self.on {
            self.focus(window, cx);
        }
        self.on = on;
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
