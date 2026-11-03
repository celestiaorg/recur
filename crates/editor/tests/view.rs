//! The editor view end to end: typing, the keymap, panels, mouse
//! selection, scrolling and the merge view, against GPUI's test platform.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use recur_editor::change::Change;
use recur_editor::highlight::{FILL_IDLE_DELAY_MS, SharedTextMate, TextMate};
use recur_editor::state::TransactionSpec;
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
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_ui::init(cx);
    });
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
    h.cx.simulate_keystrokes("secondary-z");
    h.settle();
    assert_eq!(h.doc(), "f() {}");
}

#[gpui::test]
fn dirty_flag_and_save(cx: &mut TestAppContext) {
    let mut h = mount(cx, "a.txt", "hello", EditorKind::Code);
    h.cx.simulate_keystrokes("secondary-right");
    h.cx.simulate_input("!");
    h.settle();
    assert!(h.view.read_with(h.cx, |v, _| v.is_dirty()));
    h.cx.simulate_keystrokes("secondary-s");
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
    h.cx.simulate_keystrokes("secondary-f");
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
fn find_panel_survives_reload_and_closes_on_file_switch(cx: &mut TestAppContext) {
    let mut h = mount(cx, "a.txt", "one", EditorKind::Code);
    h.cx.simulate_keystrokes("secondary-f");
    h.settle();
    let panel = |h: &mut Harness| {
        h.view.update_in(h.cx, |v, window, cx| {
            (v.is_search_open(), v.panel_has_focus(window, cx))
        })
    };
    h.view.update_in(h.cx, |v, window, cx| {
        v.external_doc_change("two", window, cx)
    });
    h.settle();
    assert_eq!(panel(&mut h), (true, true));
    h.view.update_in(h.cx, |v, window, cx| {
        v.mount_path("b.txt", "three", None, window, cx)
    });
    h.settle();
    assert_eq!(panel(&mut h), (false, false));
}

#[gpui::test]
fn go_to_line_dialog(cx: &mut TestAppContext) {
    let mut h = mount(cx, "a.txt", "a\nb\nc\nd", EditorKind::Code);
    h.cx.simulate_keystrokes("secondary-alt-g");
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
    h.cx.simulate_keystrokes("secondary-d secondary-d");
    h.settle();
    h.cx.simulate_input("X");
    h.settle();
    assert_eq!(h.doc(), "X\nX");
}

#[gpui::test]
fn completion_offers_buffer_words(cx: &mut TestAppContext) {
    let mut h = mount(cx, "a.txt", "alphabet\n", EditorKind::Code);
    // Document end: ⌘↓ is CodeMirror's macOS binding, Ctrl+End elsewhere.
    h.cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-down"
    } else {
        "ctrl-end"
    });
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
    h.cx.simulate_keystrokes("secondary-alt-z");
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
// Linux: Ctrl+G is CodeMirror's Mod-g (find next), which the editor keymap runs before emacs.
#[cfg_attr(not(target_os = "macos"), ignore)]
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

#[gpui::test]
fn vim_brackets_indent_and_display_lines(cx: &mut TestAppContext) {
    let mut h = mount_vim(cx, "f(a) \"q\" x\nb");
    h.cx.simulate_keystrokes("%");
    h.settle();
    assert_eq!(h.sel(), (3, 3));
    h.cx.simulate_keystrokes("%");
    h.settle();
    assert_eq!(h.sel(), (1, 1));
    h.cx.simulate_keystrokes("f q c i \"");
    h.cx.simulate_input("z");
    h.cx.simulate_keystrokes("escape escape");
    h.settle();
    assert_eq!(h.doc(), "f(a) \"z\" x\nb");
    h.cx.simulate_keystrokes("g $");
    h.settle();
    assert_eq!(h.sel(), (9, 9));
    h.cx.simulate_keystrokes("g 0");
    h.settle();
    assert_eq!(h.sel(), (0, 0));
    h.cx.simulate_keystrokes("t ) ;");
    h.settle();
    assert_eq!(h.sel(), (2, 2));
    h.cx.simulate_keystrokes("> > j > >");
    h.settle();
    assert_eq!(h.doc(), "    f(a) \"z\" x\n    b");
    h.cx.simulate_keystrokes("< <");
    h.settle();
    assert_eq!(h.doc(), "    f(a) \"z\" x\nb");
    h.cx.simulate_keystrokes("k = =");
    h.settle();
    assert_eq!(h.doc(), "f(a) \"z\" x\nb");
    h.cx.simulate_keystrokes("shift-o");
    h.cx.simulate_input("t");
    h.cx.simulate_keystrokes("escape escape");
    h.settle();
    assert_eq!(h.doc(), "t\nf(a) \"z\" x\nb");
    h.cx.simulate_keystrokes("j shift-r");
    h.cx.simulate_input("gh");
    h.cx.simulate_keystrokes("backspace escape escape");
    h.settle();
    assert_eq!(h.doc(), "t\nga) \"z\" x\nb");
}

#[gpui::test]
fn find_field_takes_typed_keys(cx: &mut TestAppContext) {
    let mut h = mount(cx, "a.txt", "fmt x fmt", EditorKind::Code);
    h.cx.simulate_keystrokes("secondary-f");
    h.settle();
    h.cx.simulate_keystrokes("f m t");
    h.settle();
    let q = h
        .view
        .read_with(h.cx, |v, _| v.state().search.query.search.clone());
    assert_eq!(q, "fmt");
}

fn go_textmate() -> SharedTextMate {
    let bundles = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bundles");
    let grammar =
        std::fs::read_to_string(bundles.join("Languages/syntaxes/go.tmLanguage.json")).unwrap();
    let theme =
        std::fs::read_to_string(bundles.join("Recur/themes/charcoal.tmTheme.json")).unwrap();
    let tm: SharedTextMate = Rc::new(RefCell::new(TextMate::default()));
    {
        let mut t = tm.borrow_mut();
        t.catalog.add_grammar(
            recur_textmate::GrammarInfo {
                scope_name: "source.go".into(),
                file_types: Some(vec!["go".into()]),
                ..Default::default()
            },
            Rc::new(move || Some(grammar.clone())),
        );
        t.catalog.set_active_theme(Rc::new(
            recur_textmate::parse_theme_json(&theme, "charcoal").unwrap(),
        ));
    }
    tm
}

#[gpui::test]
fn a_large_edit_refills_on_the_background_executor(cx: &mut TestAppContext) {
    let tm = go_textmate();
    let lines = 3000;
    let doc: String = (0..lines)
        .map(|i| format!("func f{i}() int {{ return {i} }}\n"))
        .collect();
    let mut options = EditorOptions::new("big.go", &doc);
    options.textmate = Some(tm.clone());
    let mut h = mount_with(cx, options);
    h.settle();
    let total = lines + 1;
    let sweep = |tm: &SharedTextMate| {
        let t = tm.borrow();
        let e = t.catalog.engine();
        let id = e.doc_ids().into_iter().next().unwrap();
        (e.filled_through(&id), e.tokens_for_line(&id, total - 1))
    };
    let (filled, before) = sweep(&tm);
    assert_eq!(filled, total);

    // Opening a block comment at the top changes the state of every line.
    h.view.update_in(h.cx, |v, window, cx| {
        v.dispatch(
            TransactionSpec::default().changes(Change::insert(0, "/*")),
            window,
            cx,
        )
    });
    h.cx.run_until_parked();
    let (viewport, _) = sweep(&tm);
    assert!(
        viewport < total / 2,
        "the edit tokenized {viewport} lines inline"
    );

    // The fill task snapshots a job and hands it to the background executor
    // without tokenizing on the foreground. (`advance_clock` would run every
    // task, so move the clock alone and step.)
    let executor = h.cx.executor();
    let scheduler = executor.dispatcher().as_test().unwrap().scheduler().clone();
    scheduler
        .clock()
        .advance(Duration::from_millis(FILL_IDLE_DELAY_MS));
    while scheduler.pending_task_counts().1 == 0 {
        assert!(
            executor.tick(),
            "the fill never reached the background executor"
        );
    }
    assert_eq!(sweep(&tm).0, viewport);
    while scheduler.tick_background_only() {}
    assert_eq!(sweep(&tm).0, viewport, "results wait for the UI thread");

    h.settle();
    let (filled, after) = sweep(&tm);
    assert_eq!(filled, total);
    assert_ne!(after, before, "the last line is inside the comment now");
}

fn diag(from: usize, to: usize, message: &str) -> recur_editor::lint::Diagnostic {
    recur_editor::lint::Diagnostic {
        from,
        to,
        severity: recur_editor::lint::Severity::Error,
        message: message.into(),
        source: None,
        mark_class: None,
        actions: Vec::new(),
    }
}

impl Harness<'_> {
    /// Rest the pointer on `pos` until the hover tooltip shows.
    fn hover(&mut self, pos: usize) -> gpui::Bounds<gpui::Pixels> {
        let p = self.point_at(pos);
        self.cx
            .simulate_mouse_move(p, None::<MouseButton>, Modifiers::none());
        self.cx
            .executor()
            .advance_clock(Duration::from_millis(recur_editor::lsp::HOVER_TIME_MS + 50));
        self.cx.run_until_parked();
        self.view
            .read_with(self.cx, |v, _| v.hover_bounds())
            .expect("hover shown")
    }
}

/// A doc whose first line carries a short and a long (scrolling) lint.
fn hover_doc(cx: &mut TestAppContext) -> Harness<'_> {
    let doc: String =
        "alpha beta\n".to_string() + &(0..200).map(|i| format!("line {i}\n")).collect::<String>();
    let long: String = (0..60).map(|i| format!("detail {i}\n")).collect();
    let mut h = mount(cx, "a.txt", &doc, EditorKind::Code);
    h.view.update_in(h.cx, |v, window, cx| {
        v.set_diagnostics(
            vec![diag(0, 5, "short"), diag(6, 10, long.trim_end())],
            window,
            cx,
        )
    });
    h.settle();
    h
}

#[gpui::test]
fn hover_lands_in_place_on_its_first_frame(cx: &mut TestAppContext) {
    let mut h = hover_doc(cx);
    let first = h.hover(1);
    let away = h.point_at(40);
    h.cx.simulate_mouse_move(away, None::<MouseButton>, Modifiers::none());
    h.cx.run_until_parked();
    assert!(h.view.read_with(h.cx, |v, _| v.hover_bounds()).is_none());

    // Every frame the view draws after this records where the hover went.
    let seen = Rc::new(RefCell::new(Vec::new()));
    let log = seen.clone();
    let view = h.view.clone();
    let _sub = h.cx.update(|_, cx| {
        cx.observe(&view, move |v, cx| {
            log.borrow_mut().push(v.read(cx).hover_bounds())
        })
    });
    let second = h.hover(7);
    assert_ne!(first.size, second.size);
    let anchor = h
        .view
        .update_in(h.cx, |v, window, _| v.coords_at_pos(6, 1, window))
        .unwrap();
    assert_eq!(second.origin.y, anchor.bottom());
    for b in seen.borrow().iter().flatten() {
        assert_eq!(*b, second, "painted somewhere else first");
    }
}

#[gpui::test]
fn hover_takes_its_own_clicks_and_copies(cx: &mut TestAppContext) {
    let mut h = hover_doc(cx);
    let before = h.sel();
    h.hover(1);
    let body = h.cx.debug_bounds("cm-tooltip-hover-body").expect("body");
    let y = body.top() + px(14.);
    let (a, b) = (
        point(body.left() + px(16.), y),
        point(body.right() - px(8.), y),
    );
    h.cx.simulate_mouse_move(a, None::<MouseButton>, Modifiers::none());
    h.cx.simulate_mouse_down(a, MouseButton::Left, Modifiers::none());
    h.cx.simulate_mouse_move(b, Some(MouseButton::Left), Modifiers::none());
    h.cx.simulate_mouse_up(b, MouseButton::Left, Modifiers::none());
    h.cx.run_until_parked();
    assert_eq!(h.sel(), before);
    assert!(h.view.read_with(h.cx, |v, _| v.hover_bounds()).is_some());
    h.cx.simulate_keystrokes("secondary-c");
    let copied = h.cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(copied.as_deref(), Some("hort"));
}

#[gpui::test]
fn wheel_over_hover_scrolls_it(cx: &mut TestAppContext) {
    let mut h = hover_doc(cx);
    let hover = h.hover(7);
    let top = h.cx.debug_bounds("cm-tooltip-hover-body").unwrap().top();
    h.cx.simulate_event(gpui::ScrollWheelEvent {
        position: hover.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-100.))),
        ..Default::default()
    });
    h.cx.run_until_parked();
    assert_eq!(h.view.read_with(h.cx, |v, _| v.scroll_top()), 0.0);
    assert!(h.view.read_with(h.cx, |v, _| v.hover_bounds()).is_some());
    let scrolled = h.cx.debug_bounds("cm-tooltip-hover-body").unwrap().top();
    assert!(scrolled < top - px(50.), "{scrolled:?} vs {top:?}");
}
