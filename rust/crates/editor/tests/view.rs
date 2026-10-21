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
    let mut options = EditorOptions::new(path, doc);
    options.kind = kind;
    mount_with(cx, options)
}

fn mount_with(cx: &mut TestAppContext, options: EditorOptions) -> Harness<'_> {
    cx.update(gpui_kit::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
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

#[gpui::test]
fn emacs_chains_and_words(cx: &mut TestAppContext) {
    let mut options = EditorOptions::new("a.txt", "one two three");
    options.prefs.keymap = recur_editor::prefs::EditorKeymap::Emacs;
    let mut h = mount_with(cx, options);
    h.cx.simulate_keystrokes("alt-f");
    h.settle();
    assert_eq!(h.sel(), (3, 3));
    h.cx.simulate_keystrokes("ctrl-x h");
    h.settle();
    assert_eq!(h.sel(), (0, 13));
    h.cx.simulate_keystrokes("ctrl-g");
    h.settle();
    assert_eq!(h.sel(), (13, 13));
    h.cx.simulate_keystrokes("ctrl-a");
    h.cx.simulate_keystrokes("alt-d");
    h.settle();
    assert_eq!(h.doc(), " two three");
    h.cx.simulate_keystrokes("ctrl-y");
    h.settle();
    assert_eq!(h.doc(), "one two three");
}

fn mount_vim<'a>(cx: &'a mut TestAppContext, doc: &str) -> Harness<'a> {
    let mut options = EditorOptions::new("a.txt", doc);
    options.prefs.keymap = recur_editor::prefs::EditorKeymap::Vim;
    mount_with(cx, options)
}

#[gpui::test]
fn vim_normal_mode_edits_and_repeats(cx: &mut TestAppContext) {
    let mut h = mount_vim(cx, "one two\nthree\nfour");
    h.cx.simulate_keystrokes("x");
    h.settle();
    assert_eq!(h.doc(), "ne two\nthree\nfour");
    h.cx.simulate_keystrokes("d w");
    h.settle();
    assert_eq!(h.doc(), "two\nthree\nfour");
    h.cx.simulate_keystrokes("u");
    h.settle();
    assert_eq!(h.doc(), "ne two\nthree\nfour");
    h.cx.simulate_keystrokes("j d d");
    h.settle();
    assert_eq!(h.doc(), "ne two\nfour");
    h.cx.simulate_keystrokes("p");
    h.settle();
    assert_eq!(h.doc(), "ne two\nfour\nthree");
    h.cx.simulate_keystrokes("g g i");
    h.cx.simulate_input("o");
    // The first Escape closes the pending completion (the editor keymap
    // runs before vim), the second leaves insert mode.
    h.cx.simulate_keystrokes("escape escape");
    h.settle();
    assert_eq!(h.doc(), "one two\nfour\nthree");
    h.cx.simulate_keystrokes("j .");
    h.settle();
    assert_eq!(h.doc(), "one two\nofour\nthree");
    h.cx.simulate_keystrokes("j shift-a");
    h.cx.simulate_input("!");
    h.cx.simulate_keystrokes("escape escape");
    h.settle();
    assert_eq!(h.doc(), "one two\nofour\nthree!");
}

#[gpui::test]
fn vim_visual_line_and_registers(cx: &mut TestAppContext) {
    let mut h = mount_vim(cx, "a\nb\nc\nd");
    h.cx.simulate_keystrokes("shift-v j y");
    h.settle();
    assert_eq!(h.sel(), (0, 0));
    h.cx.simulate_keystrokes("shift-g p");
    h.settle();
    assert_eq!(h.doc(), "a\nb\nc\nd\na\nb");
    // `l` may reach the line break in visual mode, but the selection end
    // clamps to the line end, so only "a" goes.
    h.cx.simulate_keystrokes("g g v l d");
    h.settle();
    assert_eq!(h.doc(), "\nb\nc\nd\na\nb");
}

#[gpui::test]
fn vim_search_and_ex_commands(cx: &mut TestAppContext) {
    let mut h = mount_vim(cx, "foo bar\nbar foo\nbaz");
    h.cx.simulate_keystrokes("/");
    h.settle();
    h.cx.simulate_input("bar");
    h.cx.simulate_keystrokes("enter");
    h.settle();
    assert_eq!(h.sel(), (4, 4));
    h.cx.simulate_keystrokes("n");
    h.settle();
    assert_eq!(h.sel(), (8, 8));
    h.cx.simulate_keystrokes(":");
    h.settle();
    h.cx.simulate_input("%s/bar/qux/g");
    h.cx.simulate_keystrokes("enter");
    h.settle();
    assert_eq!(h.doc(), "foo qux\nqux foo\nbaz");
    h.cx.simulate_keystrokes(":");
    h.settle();
    h.cx.simulate_input("2");
    h.cx.simulate_keystrokes("enter");
    h.settle();
    assert_eq!(h.sel(), (8, 8));
    h.cx.simulate_keystrokes(":");
    h.settle();
    h.cx.simulate_input("g/foo/d");
    h.cx.simulate_keystrokes("enter");
    h.settle();
    assert_eq!(h.doc(), "baz");
}

#[gpui::test]
fn vim_substitute_confirm_prompt(cx: &mut TestAppContext) {
    let mut h = mount_vim(cx, "a a a");
    h.cx.simulate_keystrokes(":");
    h.settle();
    h.cx.simulate_input("s/a/b/gc");
    h.cx.simulate_keystrokes("enter");
    h.settle();
    h.cx.simulate_keystrokes("y n y");
    h.settle();
    assert_eq!(h.doc(), "b a b");
    h.cx.simulate_keystrokes("u");
    h.settle();
    assert_eq!(h.doc(), "a a a");
}

#[gpui::test]
fn vim_macros_text_objects_and_blocks(cx: &mut TestAppContext) {
    let mut h = mount_vim(cx, "f(a, b) x\nc\nb\na");
    h.cx.simulate_keystrokes("f a d i (");
    h.settle();
    assert_eq!(h.doc(), "f() x\nc\nb\na");
    h.cx.simulate_keystrokes("j q a shift-a");
    h.cx.simulate_input(";");
    h.cx.simulate_keystrokes("escape escape j q");
    h.settle();
    assert_eq!(h.doc(), "f() x\nc;\nb\na");
    h.cx.simulate_keystrokes("@ a");
    h.settle();
    assert_eq!(h.doc(), "f() x\nc;\nb;\na");
    h.cx.simulate_keystrokes("m k g g ' k");
    h.settle();
    assert_eq!(h.sel(), (12, 12));
    h.cx.simulate_keystrokes(":");
    h.settle();
    h.cx.simulate_input("2,4sort");
    h.cx.simulate_keystrokes("enter");
    h.settle();
    assert_eq!(h.doc(), "f() x\na\nb;\nc;");
    h.cx.simulate_keystrokes(":");
    h.settle();
    h.cx.simulate_input("2,3normal Ax");
    h.cx.simulate_keystrokes("enter");
    h.settle();
    assert_eq!(h.doc(), "f() x\nax\nb;x\nc;");
    // On macOS the editor keymap's Ctrl-v (page down) runs before vim, as
    // in Recur, so the block uses vim's `<C-q>`.
    h.cx.simulate_keystrokes("g g ctrl-q j j shift-i");
    h.cx.simulate_input("-");
    h.cx.simulate_keystrokes("escape escape");
    h.settle();
    assert_eq!(h.doc(), "-f() x\n-ax\n-b;x\nc;");
    h.cx.simulate_keystrokes("g g w *");
    h.settle();
    assert_eq!(h.sel(), (5, 5));
}
