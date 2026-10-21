//! The editor view end to end: typing, the keymap, panels, mouse
//! selection, scrolling and the merge view, against GPUI's test platform.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use recur_editor::view::{EditorEvent, EditorKind, EditorOptions, EditorView};

struct Harness<'a> {
    view: Entity<EditorView>,
    events: Rc<RefCell<Vec<EditorEvent>>>,
    cx: &'a mut VisualTestContext,
}

fn mount<'a>(cx: &'a mut TestAppContext, path: &str, doc: &str, kind: EditorKind) -> Harness<'a> {
    cx.update(gpui_kit::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    let mut options = EditorOptions::new(path, doc);
    options.kind = kind;
    let (view, cx) = cx.add_window_view(move |window, cx| {
        let view = EditorView::new(options, window, cx);
        let sink = sink.clone();
        cx.subscribe_self(move |_, e: &EditorEvent, _| sink.borrow_mut().push(e.clone()))
            .detach();
        view
    });
    cx.run_until_parked();
    cx.update(|window, _| window.activate_window());
    view.update_in(cx, |v, window, cx| v.focus(window, cx));
    cx.run_until_parked();
    Harness { view, events, cx }
}

impl Harness<'_> {
    fn settle(&mut self) {
        self.cx.run_until_parked();
        self.cx.executor().advance_clock(Duration::from_millis(300));
        self.cx.run_until_parked();
    }

    fn doc(&mut self) -> String {
        self.view.read_with(self.cx, |v, _| v.peek_doc())
    }

    fn sel(&mut self) -> (usize, usize) {
        self.view.read_with(self.cx, |v, _| {
            let m = v.state().selection.main();
            (m.anchor(), m.head())
        })
    }

    fn point_at(&mut self, pos: usize) -> gpui::Point<gpui::Pixels> {
        self.view.update_in(self.cx, |v, window, _| {
            let b = v.coords_at_pos(pos, 1, window).expect("visible");
            point(b.origin.x + px(1.), b.origin.y + b.size.height / 2.)
        })
    }
}

#[gpui::test]
fn typing_closes_brackets_and_indents(cx: &mut TestAppContext) {
    let mut h = mount(cx, "main.go", "", EditorKind::Code);
    h.cx.simulate_input("f(");
    h.settle();
    assert_eq!(h.doc(), "f()");
    h.cx.simulate_input(")");
    h.settle();
    assert_eq!(h.doc(), "f()");
    assert_eq!(h.sel(), (3, 3));
    h.cx.simulate_input(" {");
    h.cx.simulate_keystrokes("enter");
    h.settle();
    assert_eq!(h.doc(), "f() {\n\t\n}");
    h.cx.simulate_keystrokes("cmd-z");
    h.settle();
    assert_eq!(h.doc(), "f() {}");
}

#[gpui::test]
fn dirty_flag_and_save(cx: &mut TestAppContext) {
    let mut h = mount(cx, "a.txt", "hello", EditorKind::Code);
    h.cx.simulate_keystrokes("cmd-right");
    h.cx.simulate_input("!");
    h.settle();
    assert!(h.view.read_with(h.cx, |v, _| v.is_dirty()));
    h.cx.simulate_keystrokes("cmd-s");
    h.settle();
    let events = h.events.borrow().clone();
    assert!(events.contains(&EditorEvent::DirtyChanged(true)));
    assert!(events.contains(&EditorEvent::Save));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, EditorEvent::DocChanged(d) if d == "hello!"))
    );
}

#[gpui::test]
fn find_panel_selects_matches(cx: &mut TestAppContext) {
    let mut h = mount(cx, "a.txt", "one two one", EditorKind::Code);
    h.cx.simulate_keystrokes("cmd-f");
    h.settle();
    assert!(h.view.read_with(h.cx, |v, _| v.is_search_open()));
    h.cx.simulate_input("one");
    h.settle();
    h.cx.simulate_keystrokes("enter");
    h.settle();
    assert_eq!(h.sel(), (0, 3));
    h.cx.simulate_keystrokes("enter");
    h.settle();
    assert_eq!(h.sel(), (8, 11));
    h.cx.simulate_keystrokes("escape");
    h.settle();
    assert!(!h.view.read_with(h.cx, |v, _| v.is_search_open()));
    let events = h.events.borrow().clone();
    assert!(events.contains(&EditorEvent::SearchOpenChanged(true)));
    assert!(events.contains(&EditorEvent::SearchOpenChanged(false)));
}

#[gpui::test]
fn go_to_line_dialog(cx: &mut TestAppContext) {
    let mut h = mount(cx, "a.txt", "a\nb\nc\nd", EditorKind::Code);
    h.cx.simulate_keystrokes("cmd-alt-g");
    h.settle();
    h.cx.simulate_input("3");
    h.cx.simulate_keystrokes("enter");
    h.settle();
    assert_eq!(h.sel(), (4, 4));
}

#[gpui::test]
fn clicks_place_and_select(cx: &mut TestAppContext) {
    let mut h = mount(cx, "a.txt", "alpha beta\ngamma", EditorKind::Code);
    let p = h.point_at(7);
    h.cx.simulate_click(p, Modifiers::none());
    h.settle();
    assert_eq!(h.sel(), (7, 7));
    let p = h.point_at(2);
    h.cx.simulate_mouse_down(p, MouseButton::Left, Modifiers::none());
    let q = h.point_at(13);
    h.cx.simulate_mouse_move(q, Some(MouseButton::Left), Modifiers::none());
    h.cx.simulate_mouse_up(q, MouseButton::Left, Modifiers::none());
    h.settle();
    assert_eq!(h.sel(), (2, 13));
}

#[gpui::test]
fn multi_cursor_typing(cx: &mut TestAppContext) {
    let mut h = mount(cx, "a.txt", "ab\nab", EditorKind::Code);
    h.cx.simulate_keystrokes("cmd-d cmd-d");
    h.settle();
    h.cx.simulate_input("X");
    h.settle();
    assert_eq!(h.doc(), "X\nX");
}

#[gpui::test]
fn completion_offers_buffer_words(cx: &mut TestAppContext) {
    let mut h = mount(cx, "a.txt", "alphabet\n", EditorKind::Code);
    h.cx.simulate_keystrokes("cmd-down");
    h.cx.simulate_input("alp");
    h.settle();
    assert!(
        h.view
            .read_with(h.cx, |v, _| v.state().completion.open.is_some())
    );
    h.cx.simulate_keystrokes("enter");
    h.settle();
    assert_eq!(h.doc(), "alphabet\nalphabet");
}

#[gpui::test]
fn merge_view_reverts_a_hunk(cx: &mut TestAppContext) {
    let original = "a\nb\nc\n";
    let mut h = mount(
        cx,
        "a.txt",
        "a\nB\nc\n",
        EditorKind::Merge {
            original: original.into(),
        },
    );
    h.cx.simulate_keystrokes("down");
    h.settle();
    h.cx.simulate_keystrokes("cmd-alt-z");
    h.settle();
    assert_eq!(h.doc(), original);
}

#[gpui::test]
fn wheel_scrolls(cx: &mut TestAppContext) {
    let doc: String = (0..400).map(|i| format!("line {i}\n")).collect();
    let mut h = mount(cx, "a.txt", &doc, EditorKind::Code);
    let p = h.point_at(0);
    h.cx.simulate_event(gpui::ScrollWheelEvent {
        position: p,
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-300.))),
        ..Default::default()
    });
    h.settle();
    assert!(h.view.read_with(h.cx, |v, _| v.scroll_top()) > 200.0);
}
