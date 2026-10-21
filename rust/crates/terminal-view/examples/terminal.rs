//! Opens a login shell in a terminal pane: `cargo run -p recur-terminal-view --example terminal`.
//!
//! ⌘F (Ctrl+Shift+F elsewhere) opens the find bar; Enter / Shift+Enter step, Escape closes.

use std::sync::Arc;

use futures::StreamExt;
use gpui_kit::{
    App, AppContext, Bounds, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, Styled, Window, WindowBounds, WindowOptions, div, px,
    size,
};
use recur_terminal_view::{
    FindBarEvent, TerminalEvent, TerminalFindBar, TerminalFont, TerminalOptions, TerminalView,
    ThemeContext, init, route_event,
};
use recur_ui::Theme;
use recur_ui::style::{body, window_root};
use recur_ui::tooltip::TitleTooltip;

struct Demo {
    focus: FocusHandle,
    terminal: Entity<TerminalView>,
    find: Option<Entity<TerminalFindBar>>,
    tooltip: Entity<TitleTooltip>,
    _subs: Vec<gpui_kit::Subscription>,
}

impl Demo {
    fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = e.keystroke.modifiers;
        let chord = if cfg!(target_os = "macos") {
            m.platform && !m.shift
        } else {
            m.control && m.shift
        };
        if chord && !m.alt && e.keystroke.key == "f" {
            cx.stop_propagation();
            let bar = match &self.find {
                Some(bar) => bar.clone(),
                None => {
                    let bar = cx.new(|cx| TerminalFindBar::new(window, cx));
                    self._subs
                        .push(cx.subscribe_in(&bar, window, Self::find_event));
                    self.find = Some(bar.clone());
                    bar
                }
            };
            bar.update(cx, |b, cx| b.focus_and_select(window, cx));
            cx.notify();
        }
    }

    fn find_event(
        &mut self,
        bar: &Entity<TerminalFindBar>,
        e: &FindBarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (query, mut opts) = bar.read_with(cx, |b, cx| (b.query(cx), b.options()));
        match e {
            FindBarEvent::Changed => {
                opts.incremental = true;
                if query.is_empty() {
                    self.terminal.update(cx, |t, cx| t.clear_search(cx));
                } else {
                    self.terminal
                        .update(cx, |t, cx| t.find_next(&query, opts, cx));
                }
            }
            FindBarEvent::Next if !query.is_empty() => {
                self.terminal
                    .update(cx, |t, cx| t.find_next(&query, opts, cx));
            }
            FindBarEvent::Previous if !query.is_empty() => {
                self.terminal
                    .update(cx, |t, cx| t.find_previous(&query, opts, cx));
            }
            FindBarEvent::Close => {
                self.find = None;
                self.terminal.update(cx, |t, cx| {
                    t.clear_search(cx);
                    t.focus(window, cx);
                });
                cx.notify();
            }
            _ => {}
        }
    }
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        window_root(
            body(cx)
                .track_focus(&self.focus)
                .capture_key_down(cx.listener(Self::key_down))
                .size_full()
                .flex()
                .flex_col()
                .bg(t.bg)
                .children(self.find.clone())
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .child(self.terminal.clone()),
                )
                .child(self.tooltip.clone()),
        )
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(recur_ui::Assets)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            recur_ui::init(cx);
            init(cx);

            let (tx, mut rx) = futures::channel::mpsc::unbounded::<recur_term::Event>();
            let manager = recur_term::Manager::new(move |ev| {
                let _ = tx.unbounded_send(ev);
            });
            cx.spawn(async move |cx| {
                while let Some(ev) = rx.next().await {
                    let quit = matches!(ev, recur_term::Event::Exit { .. });
                    cx.update(|cx| {
                        route_event(&ev, cx);
                        if quit {
                            cx.quit();
                        }
                    });
                }
            })
            .detach();

            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "/".into());
            let info = match manager.open("example", &cwd, 100, 30, &[]) {
                Ok(info) => info,
                Err(e) => {
                    eprintln!("{e}");
                    cx.quit();
                    return;
                }
            };
            let bounds = Bounds::centered(None, size(px(900.), px(560.)), cx);
            let opened = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                move |window, cx| {
                    let terminal = cx.new(|cx| {
                        TerminalView::new(
                            TerminalOptions {
                                session_id: info.id.clone(),
                                workspace_id: "example".into(),
                                active: true,
                                frosted: false,
                                font: TerminalFont::default(),
                                theme: ThemeContext::default(),
                                workspace_root: cwd.clone(),
                                dock_focus: None,
                            },
                            Arc::new(manager.clone()),
                            window,
                            cx,
                        )
                    });
                    terminal.update(cx, |t, cx| t.focus(window, cx));
                    cx.new(|cx| {
                        let subs = vec![cx.subscribe(
                            &terminal,
                            |demo: &mut Demo, _, e: &TerminalEvent, cx| match e {
                                TerminalEvent::Title(t) => eprintln!("title: {t}"),
                                TerminalEvent::FindResults {
                                    index,
                                    total,
                                    valid,
                                } => {
                                    if let Some(bar) = &demo.find {
                                        bar.update(cx, |b, cx| {
                                            b.set_results(*index, *total, *valid, cx)
                                        });
                                    }
                                }
                                TerminalEvent::Exit(code) => eprintln!("exited ({code})"),
                            },
                        )];
                        Demo {
                            focus: cx.focus_handle(),
                            terminal,
                            find: None,
                            tooltip: TitleTooltip::new(cx),
                            _subs: subs,
                        }
                    })
                },
            );
            if let Err(e) = opened {
                eprintln!("open window: {e}");
                cx.quit();
            }
        });
}
