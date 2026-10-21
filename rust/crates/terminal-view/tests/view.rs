//! Pane behaviour against a recording backend: the TerminalPane half of
//! `frontend/e2e/terminal-dock.spec.ts` and `components/TerminalDock.test.ts`.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Entity, Modifiers, MouseButton, Point, ScrollDelta, ScrollWheelEvent, TestAppContext,
    TouchPhase, VisualTestContext, point, px,
};
use parking_lot::Mutex;
use recur_terminal_view::{
    ACK_THRESHOLD, FindOptions, TerminalBackend, TerminalEvent, TerminalFont, TerminalOptions,
    TerminalView, ThemeContext,
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Attach,
    Detach,
    Ack(usize),
    Write(String),
    Resize(u16, u16),
    ClearReplay,
}

#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<Call>>,
    replay: Vec<u8>,
}

impl Recorder {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().clone()
    }

    fn writes(&self) -> String {
        self.calls()
            .into_iter()
            .filter_map(|c| {
                if let Call::Write(s) = c {
                    Some(s)
                } else {
                    None
                }
            })
            .collect()
    }

    fn clear(&self) {
        self.calls.lock().clear();
    }
}

impl TerminalBackend for Recorder {
    fn attach(&self, _id: &str) -> Result<Vec<u8>, String> {
        self.calls.lock().push(Call::Attach);
        Ok(self.replay.clone())
    }

    fn detach(&self, _id: &str) {
        self.calls.lock().push(Call::Detach);
    }

    fn ack(&self, _id: &str, n: usize) {
        self.calls.lock().push(Call::Ack(n));
    }

    fn write(&self, _id: &str, data: &[u8]) {
        self.calls
            .lock()
            .push(Call::Write(String::from_utf8_lossy(data).into_owned()));
    }

    fn resize(&self, _id: &str, cols: u16, rows: u16) {
        self.calls.lock().push(Call::Resize(cols, rows));
    }

    fn clear_replay(&self, _id: &str) {
        self.calls.lock().push(Call::ClearReplay);
    }
}

struct Harness<'a> {
    view: Entity<TerminalView>,
    rec: Arc<Recorder>,
    events: Arc<Mutex<Vec<TerminalEvent>>>,
    cx: &'a mut VisualTestContext,
}

fn mount<'a>(cx: &'a mut TestAppContext, replay: &[u8]) -> Harness<'a> {
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_terminal_view::init(cx);
    });
    let rec = Arc::new(Recorder {
        replay: replay.to_vec(),
        ..Default::default()
    });
    let backend = rec.clone();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| {
        let view = TerminalView::new(
            TerminalOptions {
                session_id: "sess-1".into(),
                workspace_id: "ws-1".into(),
                active: true,
                frosted: false,
                font: TerminalFont::default(),
                theme: ThemeContext {
                    app_theme: "default".into(),
                    ..Default::default()
                },
                workspace_root: "/tmp/wt".into(),
                dock_focus: None,
            },
            backend,
            window,
            cx,
        );
        let sink = sink.clone();
        cx.subscribe_self(move |_, e: &TerminalEvent, _| sink.lock().push(e.clone()))
            .detach();
        view
    });
    cx.run_until_parked();
    cx.update(|window, _| window.activate_window());
    view.update_in(cx, |v, window, cx| v.focus(window, cx));
    cx.run_until_parked();
    Harness {
        view,
        rec,
        events,
        cx,
    }
}

impl Harness<'_> {
    fn settle(&mut self) {
        self.cx.run_until_parked();
        self.cx.executor().advance_clock(Duration::from_millis(250));
        self.cx.run_until_parked();
    }

    fn output(&mut self, text: &str) {
        let bytes = text.as_bytes().to_vec();
        self.cx.update(|_, cx| {
            recur_terminal_view::route_event(
                &recur_term::Event::Data {
                    id: "sess-1".into(),
                    data: bytes,
                },
                cx,
            )
        });
        self.settle();
    }

    fn viewport(&mut self) -> Vec<String> {
        self.view
            .read_with(self.cx, |v, _| v.screen().viewport_text())
    }

    /// Window position of the centre of 0-based cell (col, row).
    fn cell(&mut self, col: usize, row: usize) -> Point<gpui::Pixels> {
        let (origin, (w, h)) = self.view.read_with(self.cx, |v, _| {
            (v.grid_origin().unwrap(), v.cell_size().unwrap())
        });
        point(
            origin.x + px((col as f32 + 0.5) * w),
            origin.y + px((row as f32 + 0.5) * h),
        )
    }
}

#[gpui::test]
fn attaches_on_mount_and_repaints_the_replay(cx: &mut TestAppContext) {
    let mut h = mount(cx, b"replayed-scrollback\r\n");
    h.settle();
    assert_eq!(h.rec.calls().first(), Some(&Call::Attach));
    assert_eq!(h.viewport()[0], "replayed-scrollback");
    // The replay is not acked: the session never counted it.
    assert!(!h.rec.calls().iter().any(|c| matches!(c, Call::Ack(_))));
}

#[gpui::test]
fn opens_the_pty_at_the_measured_size(cx: &mut TestAppContext) {
    let mut h = mount(cx, b"");
    h.settle();
    let (cols, rows) = h.view.read_with(h.cx, |v, _| v.size());
    assert!(cols >= 20 && rows > 3, "fit to the window: {cols}x{rows}");
    assert!(
        h.rec
            .calls()
            .contains(&Call::Resize(cols as u16, rows as u16))
    );
}

#[gpui::test]
fn streams_output_and_acks_in_32kb_batches(cx: &mut TestAppContext) {
    let mut h = mount(cx, b"");
    h.settle();
    h.output(&"x".repeat(40 * 1024));
    let acks: Vec<usize> = h
        .rec
        .calls()
        .into_iter()
        .filter_map(|c| if let Call::Ack(n) = c { Some(n) } else { None })
        .collect();
    assert_eq!(acks.len(), 1);
    assert!(acks[0] >= ACK_THRESHOLD);
}

#[gpui::test]
fn output_before_the_replay_waits_for_it(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_terminal_view::init(cx);
    });
    let rec = Arc::new(Recorder {
        replay: b"old\r\n".to_vec(),
        ..Default::default()
    });
    let backend = rec.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| {
        TerminalView::new(
            TerminalOptions {
                session_id: "s".into(),
                workspace_id: "w".into(),
                active: true,
                frosted: false,
                font: TerminalFont::default(),
                theme: ThemeContext::default(),
                workspace_root: String::new(),
                dock_focus: None,
            },
            backend,
            window,
            cx,
        )
    });
    // Data that lands before the attach resolves is queued behind the replay.
    view.update(cx, |v, cx| v.push_output(b"new\r\n", cx));
    cx.run_until_parked();
    let text = view.read_with(cx, |v, _| v.screen().viewport_text());
    assert_eq!(&text[..2], ["old", "new"]);
}

#[gpui::test]
fn typing_writes_to_the_pty(cx: &mut TestAppContext) {
    let mut h = mount(cx, b"");
    h.settle();
    h.rec.clear();
    h.cx.simulate_input("ls");
    h.cx.simulate_keystrokes("enter");
    h.cx.simulate_keystrokes("ctrl-c up");
    h.settle();
    assert_eq!(h.rec.writes(), "ls\r\x03\x1b[A");
}

#[gpui::test]
fn shell_chords_stay_with_the_shell(cx: &mut TestAppContext) {
    let mut h = mount(cx, b"");
    h.settle();
    h.rec.clear();
    // Ctrl+T / Ctrl+W / Ctrl+F are readline keys: the pane sends them.
    h.cx.simulate_keystrokes("ctrl-t ctrl-w ctrl-f");
    h.settle();
    assert_eq!(h.rec.writes(), "\x14\x17\x06");
    h.rec.clear();
    // ⌘ chords and ⌃` belong to the app.
    h.cx.simulate_keystrokes("cmd-t ctrl-`");
    h.settle();
    assert_eq!(h.rec.writes(), "");
}

#[gpui::test]
fn paste_uses_bracketed_paste_when_asked(cx: &mut TestAppContext) {
    let mut h = mount(cx, b"");
    h.settle();
    h.rec.clear();
    h.view.update(h.cx, |v, cx| v.paste("a\nb", cx));
    h.output("\x1b[?2004h");
    h.view.update(h.cx, |v, cx| v.paste("c", cx));
    h.settle();
    assert_eq!(h.rec.writes(), "a\rb\x1b[200~c\x1b[201~");
}

#[gpui::test]
fn find_counts_matches_and_toggles_case(cx: &mut TestAppContext) {
    let mut h = mount(cx, b"Hello world hello\r\n");
    h.settle();
    let opts = FindOptions {
        incremental: true,
        ..Default::default()
    };
    h.view.update(h.cx, |v, cx| v.find_next("hello", opts, cx));
    h.settle();
    assert_eq!(
        h.events.lock().last(),
        Some(&TerminalEvent::FindResults {
            index: 1,
            total: 2,
            valid: true
        })
    );
    let cs = FindOptions {
        case_sensitive: true,
        ..opts
    };
    h.view.update(h.cx, |v, cx| v.find_next("hello", cs, cx));
    h.settle();
    assert_eq!(
        h.events.lock().last(),
        Some(&TerminalEvent::FindResults {
            index: 1,
            total: 1,
            valid: true
        })
    );
    let re = FindOptions {
        regexp: true,
        ..opts
    };
    h.view.update(h.cx, |v, cx| v.find_next("(", re, cx));
    h.settle();
    assert_eq!(
        h.events.lock().last(),
        Some(&TerminalEvent::FindResults {
            index: 0,
            total: 0,
            valid: false
        })
    );
    assert_eq!(recur_terminal_view::find_count_label(1, 2, false), "1 / 2");
    assert_eq!(
        recur_terminal_view::find_count_label(0, 0, false),
        "No results"
    );
    assert_eq!(
        recur_terminal_view::find_count_label(3, 999, true),
        "3 / 999+"
    );
}

#[gpui::test]
fn exit_and_titles_reach_the_dock(cx: &mut TestAppContext) {
    let mut h = mount(cx, b"");
    h.settle();
    h.output("\x1b]0;npm run dev\x07");
    h.cx.update(|_, cx| {
        recur_terminal_view::route_event(
            &recur_term::Event::Exit {
                id: "sess-1".into(),
                workspace_id: "ws-1".into(),
                exit_code: 127,
            },
            cx,
        )
    });
    h.settle();
    let events = h.events.lock().clone();
    assert!(events.contains(&TerminalEvent::Title("npm run dev".into())));
    assert!(events.contains(&TerminalEvent::Exit(127)));
}

#[gpui::test]
fn dropping_explorer_paths_types_them_quoted(cx: &mut TestAppContext) {
    let mut h = mount(cx, b"");
    h.settle();
    h.rec.clear();
    h.view.update_in(h.cx, |v, window, cx| {
        v.drop_paths(
            &["src/main.go".into(), "My File.ts".into(), "src/".into()],
            window,
            cx,
        )
    });
    h.settle();
    assert_eq!(
        h.rec.writes(),
        "/tmp/wt/src/main.go '/tmp/wt/My File.ts' /tmp/wt/src"
    );
    assert_eq!(
        recur_terminal_view::quote_for_shell("it's.ts"),
        "'it'\\''s.ts'"
    );
    assert_eq!(
        recur_terminal_view::terminal_drop_text(&["src/main.go".into(), "README.md".into()], ""),
        "src/main.go README.md"
    );
}

#[gpui::test]
fn clear_wipes_the_screen_and_the_replay(cx: &mut TestAppContext) {
    let mut h = mount(cx, b"");
    h.settle();
    h.output("one\r\ntwo\r\n$ ");
    h.view.update(h.cx, |v, cx| v.clear(cx));
    h.settle();
    assert!(h.rec.calls().contains(&Call::ClearReplay));
    assert_eq!(h.viewport()[0], "$");
}

#[gpui::test]
fn dispose_detaches_and_stops_routing(cx: &mut TestAppContext) {
    let mut h = mount(cx, b"");
    h.settle();
    h.view.update(h.cx, |v, cx| v.dispose(cx));
    h.settle();
    assert!(h.rec.calls().contains(&Call::Detach));
    h.output("after\r\n");
    assert_ne!(h.viewport()[0], "after");
}

#[gpui::test]
fn parking_keeps_the_session_attached(cx: &mut TestAppContext) {
    let mut h = mount(cx, b"");
    h.settle();
    let view = h.view.clone();
    h.cx.update(|_, cx| recur_terminal_view::park(view, cx));
    h.output("while parked\r\n");
    assert_eq!(
        h.viewport()[0],
        "while parked",
        "a parked view keeps its screen live"
    );
    assert!(
        !h.rec.calls().contains(&Call::Detach),
        "parking does not detach"
    );
    let taken =
        h.cx.update(|_, cx| recur_terminal_view::take_parked("sess-1", cx));
    assert!(taken.is_some());
    // A shell that exits while parked releases its view.
    let view = h.view.clone();
    h.cx.update(|_, cx| recur_terminal_view::park(view, cx));
    h.cx.update(|_, cx| {
        recur_terminal_view::route_event(
            &recur_term::Event::Exit {
                id: "sess-1".into(),
                workspace_id: "ws-1".into(),
                exit_code: 0,
            },
            cx,
        )
    });
    h.settle();
    assert!(h.rec.calls().contains(&Call::Detach));
    assert!(
        h.cx.update(|_, cx| recur_terminal_view::take_parked("sess-1", cx))
            .is_none()
    );
}

#[gpui::test]
fn focus_reports_follow_decset_1004(cx: &mut TestAppContext) {
    let mut h = mount(cx, b"");
    h.settle();
    h.output("\x1b[?1004h");
    assert!(
        h.view.read_with(h.cx, |v, _| v.is_focused()),
        "mounted pane has focus"
    );
    h.rec.clear();
    h.cx.deactivate_window();
    h.settle();
    assert_eq!(h.rec.writes(), "\x1b[O");
}

#[gpui::test]
fn device_attribute_queries_are_answered(cx: &mut TestAppContext) {
    let mut h = mount(cx, b"");
    h.settle();
    h.rec.clear();
    h.output("\x1b[c");
    assert_eq!(h.rec.writes(), "\x1b[?1;2c");
}

#[gpui::test]
fn wheel_scrolls_the_scrollback(cx: &mut TestAppContext) {
    let mut h = mount(cx, b"");
    h.settle();
    let rows = h.view.read_with(h.cx, |v, _| v.size().1);
    for i in 0..rows * 3 {
        h.output(&format!("line {i}\r\n"));
    }
    assert!(h.view.read_with(h.cx, |v, _| v.screen().at_bottom()));
    let pos = h.cell(2, 2);
    h.cx.simulate_mouse_move(pos, None, Modifiers::none());
    h.cx.simulate_event(ScrollWheelEvent {
        position: pos,
        delta: ScrollDelta::Lines(point(0., 3.)),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    h.settle();
    assert!(
        !h.view.read_with(h.cx, |v, _| v.screen().at_bottom()),
        "wheel up scrolls into history"
    );
    h.rec.clear();
    h.cx.simulate_input("x");
    h.settle();
    assert!(
        h.view.read_with(h.cx, |v, _| v.screen().at_bottom()),
        "typing returns to the prompt"
    );
}

#[gpui::test]
fn mouse_reports_in_sgr_mode(cx: &mut TestAppContext) {
    let mut h = mount(cx, b"");
    h.settle();
    h.output("\x1b[?1000h\x1b[?1006h");
    h.rec.clear();
    let pos = h.cell(4, 2);
    h.cx.simulate_mouse_move(pos, None, Modifiers::none());
    h.cx.simulate_mouse_down(pos, MouseButton::Left, Modifiers::none());
    h.cx.simulate_mouse_up(pos, MouseButton::Left, Modifiers::none());
    h.settle();
    assert_eq!(h.rec.writes(), "\x1b[<0;5;3M\x1b[<0;5;3m");
}

#[gpui::test]
fn drag_selects_and_copies(cx: &mut TestAppContext) {
    let mut h = mount(cx, b"");
    h.settle();
    h.output("hello world");
    let (a, b) = (h.cell(0, 0), h.cell(4, 0));
    let a = point(a.x - px(3.), a.y);
    let b = point(b.x + px(3.), b.y);
    h.cx.simulate_mouse_move(a, None, Modifiers::none());
    h.cx.simulate_mouse_down(a, MouseButton::Left, Modifiers::none());
    h.cx.simulate_mouse_move(b, MouseButton::Left, Modifiers::none());
    h.cx.simulate_mouse_up(b, MouseButton::Left, Modifiers::none());
    h.settle();
    assert_eq!(h.view.read_with(h.cx, |v, _| v.selection_text()), "hello");
}
