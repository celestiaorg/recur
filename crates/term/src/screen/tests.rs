use super::*;
use crate::input::{MouseEncoding, MouseProtocol};

fn screen(cols: usize, rows: usize) -> Screen {
    Screen::new(cols, rows)
}

fn replies(out: &[Output]) -> Vec<String> {
    out.iter()
        .filter_map(|o| {
            if let Output::Reply(r) = o {
                Some(r.clone())
            } else {
                None
            }
        })
        .collect()
}

fn opts() -> SearchOptions {
    SearchOptions {
        decorations: Decorations {
            match_background: Rgba::rgb(1, 2, 3),
            active_match_background: Rgba::rgb(4, 5, 6),
        },
        ..Default::default()
    }
}

#[test]
fn prints_text_and_wraps() {
    let mut s = screen(10, 3);
    s.feed(b"hello\r\nworld 1234567890");
    assert_eq!(s.viewport_text(), vec!["hello", "world 1234", "567890"]);
}

#[test]
fn alacritty_answers_status_queries() {
    let mut s = screen(80, 24);
    assert_eq!(replies(&s.feed(b"\x1b[c")), vec!["\x1b[?6c"]);
    s.feed(b"ab\r\ncd");
    assert_eq!(replies(&s.feed(b"\x1b[6n")), vec!["\x1b[2;3R"]);
}

#[test]
fn mouse_and_focus_modes_come_from_the_terminal() {
    let mut s = screen(80, 24);
    assert_eq!(
        (s.mouse_protocol(), s.mouse_encoding()),
        (MouseProtocol::None, MouseEncoding::Default)
    );
    s.feed(b"\x1b[?1002h\x1b[?1006h\x1b[?1004h");
    assert_eq!(
        (s.mouse_protocol(), s.mouse_encoding()),
        (MouseProtocol::Drag, MouseEncoding::Sgr)
    );
    assert_eq!(s.focus_report(true), Some("\x1b[I"));
    s.feed(b"\x1bc");
    assert_eq!(
        (s.mouse_protocol(), s.focus_report(true)),
        (MouseProtocol::None, None)
    );
}

#[test]
fn emits_titles_and_bells() {
    let mut s = screen(80, 24);
    let out = s.feed(b"\x1b]0;zsh: ~/recur\x07\x1b]2;npm run dev\x1b\\\x07");
    assert!(out.contains(&Output::Title("zsh: ~/recur".into())));
    assert!(out.contains(&Output::Title("npm run dev".into())));
    assert!(out.contains(&Output::Bell));
}

#[test]
fn colour_queries_answer_from_the_live_palette() {
    let mut s = screen(80, 24);
    assert_eq!(
        replies(&s.feed(b"\x1b]4;1;?\x07")),
        vec!["\x1b]4;1;rgb:cccc/0000/0000\x07"]
    );
    s.set_palette(Palette::from_theme(&Theme {
        background: Some("#ffffff".into()),
        ..Default::default()
    }));
    assert_eq!(
        replies(&s.feed(b"\x1b]11;?\x1b\\")),
        vec!["\x1b]11;rgb:ffff/ffff/ffff\x1b\\"]
    );
    let c = s.palette().cursor;
    assert_eq!(
        replies(&s.feed(b"\x1b]12;?\x07")),
        vec![format!(
            "\x1b]12;rgb:{0:02x}{0:02x}/{1:02x}{1:02x}/{2:02x}{2:02x}\x07",
            c.r(),
            c.g(),
            c.b()
        )]
    );
    s.feed(b"\x1b]11;rgb:12/34/56\x07");
    assert_eq!(
        replies(&s.feed(b"\x1b]11;?\x07")),
        vec!["\x1b]11;rgb:1212/3434/5656\x07"]
    );
}

#[test]
fn clear_keeps_only_the_prompt_row() {
    let mut s = screen(20, 3);
    s.feed(b"a\r\nb\r\nc\r\nd\r\n$ ls");
    assert!(s.total_rows() > 3);
    s.clear();
    assert_eq!(s.total_rows(), 3);
    assert_eq!(s.viewport_text(), vec!["$ ls", "", ""]);
    assert_eq!(s.cursor_in_viewport(), Some((4, 0)));
}

#[test]
fn drag_selects_and_copies_text() {
    let mut s = screen(20, 3);
    s.feed(b"hello world\r\nsecond line");
    s.selection_mouse_down((0, 0), Side::Left, 1, false, false);
    s.selection_mouse_move((4, 1), Side::Right, 20.0, 54.0);
    assert_eq!(s.selection_mouse_up(300, false, (4, 1)), None);
    assert_eq!(s.selection_text(), "hello world\nsecon");
    s.user_input();
    assert!(!s.has_selection(), "typing clears the selection");
}

#[test]
fn double_click_selects_a_word_and_triple_click_a_line() {
    let mut s = screen(30, 3);
    s.feed(b"cargo test --package recur");
    s.selection_mouse_down((8, 0), Side::Left, 2, false, false);
    assert_eq!(s.selection_text(), "test");
    s.selection_mouse_down((2, 0), Side::Left, 3, false, false);
    assert_eq!(s.selection_text(), "cargo test --package recur\n");
}

#[test]
fn shift_click_extends_and_alt_click_moves_the_cursor() {
    let mut s = screen(20, 3);
    s.feed(b"$ echo hi");
    s.selection_mouse_down((0, 0), Side::Left, 1, false, false);
    s.selection_mouse_up(10, false, (0, 0));
    s.selection_mouse_down((5, 0), Side::Right, 1, true, false);
    s.selection_mouse_up(10, false, (5, 0));
    assert_eq!(s.selection_text(), "$ echo");
    s.clear_selection();
    s.selection_mouse_down((2, 0), Side::Left, 1, false, false);
    assert_eq!(
        s.selection_mouse_up(100, true, (2, 0)),
        Some("\x1b[D".repeat(7))
    );
}

#[test]
fn column_selection() {
    let mut s = screen(10, 3);
    s.feed(b"abcdef\r\nghijkl\r\nmnopqr");
    s.selection_mouse_down((1, 0), Side::Left, 1, false, true);
    s.selection_mouse_move((2, 2), Side::Right, 30.0, 54.0);
    assert_eq!(s.selection_text(), "bc\nhi\nno");
}

#[test]
fn right_click_selects_the_word_or_link_unless_inside_the_selection() {
    let mut s = screen(30, 3);
    s.feed(b"open http://localhost:3000 now");
    s.right_click_select((1, 0));
    assert_eq!(s.selection_text(), "open");
    s.right_click_select((2, 0));
    assert_eq!(s.selection_text(), "open", "click inside keeps it");
    s.right_click_select((9, 0));
    assert_eq!(s.selection_text(), "http://localhost:3000");
}

#[test]
fn find_steps_through_matches_and_counts_them() {
    let mut s = screen(40, 5);
    s.feed(b"hello world hello\r\n");
    let o = opts();
    assert_eq!(
        s.find("hello", &o, true).unwrap(),
        SearchResults { index: 1, count: 2 }
    );
    assert_eq!(s.selection_text(), "hello");
    assert_eq!(s.find("hello", &o, true).unwrap().index, 2);
    assert_eq!(s.find("hello", &o, true).unwrap().index, 1, "wraps around");
    assert_eq!(
        s.find("hello", &o, false).unwrap().index,
        2,
        "and backwards"
    );
}

#[test]
fn find_case_whole_word_and_regex() {
    let mut s = screen(40, 5);
    s.feed(b"Hello world hello helloo\r\n");
    let mut count = |q: &str, o: SearchOptions| s.find(q, &o, true).unwrap().count;
    assert_eq!(count("hello", opts()), 3);
    let case_sensitive = SearchOptions {
        case_sensitive: true,
        ..opts()
    };
    assert_eq!(count("hello", case_sensitive), 2);
    let whole_word = SearchOptions {
        whole_word: true,
        ..opts()
    };
    assert_eq!(count("hello", whole_word), 2);
    let regex = SearchOptions {
        regex: true,
        ..opts()
    };
    assert_eq!(count("hel+o+", regex.clone()), 3);
    assert_eq!(count("o.", opts()), 0, "literal unless regex");
    assert!(s.find("(unclosed", &regex, true).is_err());
}

#[test]
fn find_without_matches_or_query_selects_nothing() {
    let mut s = screen(40, 5);
    s.feed(b"abc\r\n");
    assert_eq!(
        s.find("zzz", &opts(), true).unwrap(),
        SearchResults::default()
    );
    assert_eq!(s.find("", &opts(), true).unwrap(), SearchResults::default());
    assert!(!s.has_selection());
}

#[test]
fn find_count_stops_at_the_highlight_limit() {
    let mut s = screen(80, 24);
    for _ in 0..30 {
        s.feed("ab ".repeat(40).as_bytes());
    }
    assert_eq!(s.find("ab", &opts(), true).unwrap().count, HIGHLIGHT_LIMIT);
}

#[test]
fn find_recounts_after_output() {
    let mut s = screen(40, 5);
    s.feed(b"x hit\r\n");
    s.find("hit", &opts(), true).unwrap();
    assert!(s.search_refresh_due().is_none());
    s.feed(b"another hit\r\n");
    assert!(s.search_refresh_due().is_some());
    assert_eq!(
        s.run_search_refresh(),
        Some(SearchResults { index: 1, count: 2 })
    );
}

#[test]
fn find_scrolls_matches_into_view() {
    let mut s = screen(20, 3);
    s.feed(b"needle\r\n");
    for i in 0..30 {
        s.feed(format!("filler {i}\r\n").as_bytes());
    }
    s.find("needle", &opts(), true).unwrap();
    assert!(s.viewport_text().iter().any(|l| l == "needle"));
}

#[test]
fn detects_web_links_and_osc8_hyperlinks() {
    let mut s = screen(60, 3);
    s.feed(b"see https://example.com/a?b=1, then \x1b]8;;https://recur.dev/x\x1b\\docs\x1b]8;;\x1b\\ end");
    let web = s.link_at(s.viewport_point(5, 0)).unwrap();
    assert_eq!(
        (web.text.as_str(), web.start.column.0, web.end.column.0),
        ("https://example.com/a?b=1", 4, 28)
    );
    let osc = s.link_at(s.viewport_point(37, 0)).unwrap();
    assert_eq!(
        (osc.text.as_str(), osc.start.column.0, osc.end.column.0),
        ("https://recur.dev/x", 36, 39)
    );
    assert!(s.link_at(s.viewport_point(1, 0)).is_none());
}

#[test]
fn web_links_span_wrapped_rows() {
    let mut s = screen(12, 3);
    s.feed(b"x https://example.com/path");
    let link = s.link_at(s.viewport_point(2, 0)).unwrap();
    assert_eq!(link.text, "https://example.com/path");
    assert_eq!(
        s.link_at(s.viewport_point(0, 1)),
        Some(link),
        "the continuation row hit-tests too"
    );
    s.feed(b"\r\n\r\n\r\n\r\n\x1b[3J");
    let gone = Point::new(Line(-2), Column(2));
    assert_eq!(s.link_at(gone), None, "a point that scrolled away");
}

fn find_glyph(f: &Frame, col: usize, row: usize) -> &Glyph {
    f.glyphs
        .iter()
        .find(|g| g.col == col && g.row == row)
        .unwrap()
}

#[test]
fn frame_resolves_colours() {
    let mut s = screen(20, 3);
    let theme = Theme {
        background: Some("#141414".into()),
        foreground: Some("#e6e4e1".into()),
        bright_red: Some("#ff5555".into()),
        ..Default::default()
    };
    s.set_palette(Palette::from_theme(&theme));
    s.feed(b"\x1b[1;31mR\x1b[0m\x1b[2mD\x1b[0m\x1b[7mI\x1b[0m\x1b[44mB\x1b[0m\x1b[3mi");
    let f = s.frame(true, true);
    assert_eq!(f.background, Rgba::rgb(0x14, 0x14, 0x14));
    assert_eq!(
        find_glyph(&f, 0, 0).fg,
        s.palette().ansi[9],
        "bold red draws bright"
    );
    assert_eq!(
        find_glyph(&f, 1, 0).fg,
        Rgba::new(0xe6, 0xe4, 0xe1, 128),
        "dim is half opacity"
    );
    assert_eq!(
        find_glyph(&f, 2, 0).fg,
        Rgba::rgb(0x14, 0x14, 0x14),
        "inverse text takes the background"
    );
    let rect = |col: usize| {
        f.rects
            .iter()
            .find(|r| r.row == 0 && r.col <= col && col < r.col + r.len)
            .map(|r| r.color)
    };
    assert_eq!(
        rect(2),
        Some(Rgba::rgb(0xe6, 0xe4, 0xe1)),
        "inverse cell fills with the foreground"
    );
    assert_eq!(rect(3), Some(s.palette().ansi[4]));
    assert_eq!(
        rect(4),
        Some(Rgba::rgb(0x14, 0x14, 0x14)),
        "italic default-bg cells get an opaque rect"
    );
    assert_eq!(rect(1), Some(Rgba::rgb(0x14, 0x14, 0x14)), "dim cells too");
    assert_eq!(rect(0), None);
}

#[test]
fn frame_selection_and_search_layers() {
    let mut s = screen(20, 3);
    s.feed(b"abc abc");
    s.find("abc", &opts(), true).unwrap();
    let f = s.frame(true, true);
    let rect = |col: usize| {
        f.rects
            .iter()
            .find(|r| r.row == 0 && r.col <= col && col < r.col + r.len)
            .map(|r| r.color)
    };
    assert_eq!(
        rect(0),
        Some(Rgba::rgb(4, 5, 6)),
        "active match on top of the selection"
    );
    assert_eq!(
        rect(4),
        Some(Rgba::rgb(1, 2, 3)),
        "other matches use the match colour"
    );
    s.clear_search();
    s.selection_mouse_down((0, 0), Side::Left, 3, false, false);
    let f = s.frame(false, true);
    let p = s.palette().clone();
    assert!(
        f.rects
            .iter()
            .all(|r| r.color == p.selection_inactive_background_opaque)
    );
}

#[test]
fn frame_cursor_shapes() {
    let mut s = screen(10, 2);
    s.feed(b"ab");
    let f = s.frame(true, true);
    assert!(f.cursor.is_none(), "cursor waits for focus or input");
    s.show_cursor();
    let f = s.frame(true, true);
    let r = f.rects.iter().find(|r| r.col == 2).unwrap();
    assert_eq!(
        r.color,
        s.palette().cursor,
        "focused block cursor fills its cell"
    );
    let f = s.frame(false, true);
    assert_eq!(
        f.cursor.map(|c| (c.col, c.shape)),
        Some((2, CursorShape::Outline))
    );
    let f = s.frame(true, false);
    assert!(
        f.rects.iter().all(|r| r.col != 2),
        "blink phase off hides the block"
    );
    s.feed(b"\x1b[6 q");
    assert_eq!(
        s.frame(true, false).cursor.map(|c| c.shape),
        Some(CursorShape::Bar),
        "steady bar ignores the blink phase"
    );
    s.feed(b"\x1b[?25l");
    assert!(s.frame(true, true).cursor.is_none());
}

/// Viewport rows with `o` for an overlined cell and `-` otherwise.
fn overlines(s: &mut Screen) -> Vec<String> {
    let f = s.frame(false, false);
    (0..s.rows())
        .map(|y| {
            (0..s.cols())
                .map(|x| {
                    let on = f
                        .glyphs
                        .iter()
                        .any(|g| g.row == y && g.col == x && g.overline);
                    if on { 'o' } else { '-' }
                })
                .collect::<String>()
                .trim_end_matches('-')
                .to_string()
        })
        .collect()
}

#[test]
fn overline_follows_sgr_53_and_55() {
    let mut s = screen(12, 2);
    s.feed(b"a\x1b[53mbc\x1b[55md\x1b[1;53me\x1b[0mf");
    assert_eq!(overlines(&mut s), vec!["-oo-o", ""]);
    s.feed(b"\r\n\x1b[53;0mx\x1b[0;53my\x1b[38;5;53mz\x1b[m\x1b[38;5;53mw\x1b[48;2;1;53;55mv");
    assert_eq!(overlines(&mut s)[1], "-oo");
    assert_eq!(s.viewport_text()[1], "xyzwv");
}

#[test]
fn overlined_blanks_render_and_keep_default_colours() {
    let mut s = screen(10, 1);
    let theme = Theme {
        background: Some("#141414".into()),
        foreground: Some("#e6e4e1".into()),
        red: Some("#ff5555".into()),
        ..Default::default()
    };
    s.set_palette(Palette::from_theme(&theme));
    s.feed(b"\x1b[53m \x1b[31mr\x1b[39md\x1b[2mm\x1b[22;7mi\x1b[0m");
    let f = s.frame(false, false);
    let blank = find_glyph(&f, 0, 0);
    assert!(blank.overline && blank.text == " ");
    assert_eq!(blank.fg, Rgba::rgb(0xe6, 0xe4, 0xe1));
    assert_eq!(find_glyph(&f, 1, 0).fg, s.palette().ansi[1]);
    assert_eq!(
        find_glyph(&f, 2, 0).fg,
        Rgba::rgb(0xe6, 0xe4, 0xe1),
        "SGR 39 under overline is still the default foreground"
    );
    assert_eq!(find_glyph(&f, 3, 0).fg, Rgba::new(0xe6, 0xe4, 0xe1, 128));
    assert_eq!(
        find_glyph(&f, 4, 0).fg,
        Rgba::rgb(0x14, 0x14, 0x14),
        "inverse overlines in the swapped colour"
    );
    let rect = f.rects.iter().find(|r| r.col == 0).map(|r| r.color);
    assert_eq!(
        rect,
        Some(Rgba::rgb(0x14, 0x14, 0x14)),
        "overline is a background flag, so the cell gets an opaque rect"
    );
    s.show_cursor();
    s.feed(b"\x1b[53m\x1b[2D");
    let f = s.frame(true, true);
    assert!(
        !find_glyph(&f, 3, 0).overline,
        "the block cursor cell drops every attribute"
    );
}

#[test]
fn overline_survives_scrollback() {
    let mut s = screen(8, 3);
    s.feed(b"\x1b[53mtop\x1b[55m line\r\n");
    for i in 0..20 {
        s.feed(format!("row {i}\r\n").as_bytes());
    }
    assert!(s.scroll_position() > 0);
    s.scroll_lines(-(SCROLLBACK as i64));
    assert_eq!(s.viewport_text()[0], "top line");
    assert_eq!(overlines(&mut s)[0], "ooo");
    s.scroll_to_bottom();
    assert!(overlines(&mut s).iter().all(|r| r.is_empty()));
}

#[test]
fn overline_survives_reflow() {
    let mut s = screen(10, 4);
    s.feed(b"ab\x1b[53mcdef\x1b[55mgh");
    s.resize(4, 4);
    s.scroll_lines(-(SCROLLBACK as i64));
    assert_eq!(s.viewport_text()[..2], ["abcd", "efgh"]);
    assert_eq!(overlines(&mut s)[..2], ["--oo", "oo"]);
    s.resize(10, 4);
    assert_eq!(s.viewport_text()[0], "abcdefgh");
    assert_eq!(overlines(&mut s)[0], "--oooo");
}

#[test]
fn overlined_trailing_blanks_survive_reflow() {
    let mut s = screen(10, 3);
    s.feed(b"abcdef\x1b[53m   \x1b[55m\r\n");
    s.resize(5, 3);
    s.scroll_lines(-(SCROLLBACK as i64));
    assert_eq!(s.viewport_text()[..2], ["abcde", "f"]);
    assert_eq!(overlines(&mut s)[..2], ["", "-ooo"]);
    s.resize(10, 3);
    assert_eq!(overlines(&mut s)[0], "------ooo");
}

#[test]
fn overline_on_the_alternate_screen() {
    let mut s = screen(10, 3);
    s.feed(b"\x1b[53mmain\x1b[55m\x1b[?1049h");
    assert!(overlines(&mut s).iter().all(|r| r.is_empty()));
    s.feed(b"\x1b[H\x1b[53malt");
    assert_eq!(overlines(&mut s)[0], "ooo");
    s.feed(b"\x1b[?1049l");
    assert_eq!(s.viewport_text()[0], "main");
    assert_eq!(overlines(&mut s)[0], "oooo");
    s.feed(b"\r\n\x1b[53m\x1b[?1049hx\x1b[?1049l");
    s.feed(b"\x1b[?1049h");
    assert_eq!(
        overlines(&mut s)[0],
        "",
        "the alternate screen starts clear each time"
    );
}

#[test]
fn erased_and_inserted_cells_drop_overline() {
    let mut s = screen(10, 3);
    s.feed(b"\x1b[53mabcdef\x1b[3D\x1b[K");
    assert_eq!(overlines(&mut s)[0], "ooo", "EL with overline on");
    s.feed(b"\r\x1b[2C\x1b[2X");
    assert_eq!(overlines(&mut s)[0], "oo", "ECH");
    s.feed(b"\r\x1b[2@");
    assert_eq!(overlines(&mut s)[0], "--oo", "ICH inserts plain blanks");
    s.feed(b"\r\x1b[P");
    assert_eq!(overlines(&mut s)[0], "-oo", "DCH shifts overlined cells");
    s.feed(b"\r\n\x1b[53mxyz\r\x1b[2K");
    assert_eq!(overlines(&mut s)[1], "", "EL 2");
    s.feed(b"\x1b[53mxyz\x1b[2J");
    assert!(overlines(&mut s).iter().all(|r| r.is_empty()), "ED 2");
}

#[test]
fn overline_is_saved_restored_and_reset() {
    let mut s = screen(10, 3);
    s.feed(b"\x1b[53m\x1b7\x1b[55ma\x1b8b");
    assert_eq!(overlines(&mut s)[0], "o");
    s.feed(b"\x1b[53m\x1bcd");
    assert_eq!(s.viewport_text()[0], "d");
    assert_eq!(overlines(&mut s)[0], "", "RIS");
}

#[test]
fn overline_keeps_its_place_in_synchronized_output() {
    let mut s = screen(10, 3);
    s.feed(b"\x1b[?2026h\x1b[53ma\x1b[55mb\x1b[53mc");
    s.feed(b"\x1b[0md\x1b[?2026l");
    assert_eq!(s.viewport_text()[0], "abcd");
    assert_eq!(overlines(&mut s)[0], "o-o");
}

#[test]
fn title_stack_requests_do_not_toggle_overline() {
    let mut s = screen(10, 3);
    s.feed(b"\x1b[22t\x1b[22;0ta\x1b[53m\x1b[23tb");
    assert_eq!(overlines(&mut s)[0], "-o");
}

#[test]
fn overline_handler_delegates_everything_else() {
    // Each case exercises handler calls the overline wrapper forwards; the screen must end up
    // exactly where the bare emulator does. (Calls alacritty ignores have nothing to observe.)
    let cases: &[&[u8]] = &[
        b"\x1b]2;title\x07\x07",
        b"\x1b]2;a\x07\x1b[22t\x1b]2;b\x07\x1b[23t",
        b"\x1b[2 qx",
        b"\x1b]50;CursorShape=2\x07",
        b"\x1b[3;4Ha\x1b[5db\x1b[9Gc",
        b"abcdef\r\x1b[2@",
        b"\x1b[5;5Ha\x1b[2Ab\x1b[3Bc\x1b[4Cd\x1b[2De\x1b[Ef\x1b[2Fg",
        b"a\tb\x08c\rd\ne\x1bEf\x1a",
        b"\x1b[20ha\nb\x0bc\x0cd",
        b"\x1b[3g\x1b[4G\x1bH\r\tx\x1b[2Iy\x1b[Zz",
        b"1\r\n2\r\n3\r\n4\x1b[2S\x1b[1T\x1b[2;1H\x1b[L\x1b[3;1H\x1b[M",
        b"abcdef\r\x1b[2C\x1b[2X",
        b"abcdef\r\x1b[2C\x1b[P\x1b[3C\x1b[1K",
        b"top\r\n\x1b7\x1b[5;5Hx\x1b8y\x1b[1J\x1b[3;3H\x1b[0J",
        b"\x1b[2;4r\x1b[2;1Ha\x1bM\x1bMb\x1b[r",
        b"xyz\r\x1b[4hab",
        b"\x1b[4hab\r\x1b[4lX\x1b[?7l\x1b[1;29Habc\x1b[?7h\x1b[?6h\x1b[?25l",
        b"\x1b=",
        b"\x1b=\x1b>",
        b"\x1b)0\x0elqk\x0flqk",
        b"\x1b]4;1;rgb:10/20/30\x1b\\\x1b]4;2;rgb:40/50/60\x1b\\\x1b]104;2\x1b\\",
        b"\x1b]4;1;?\x1b\\",
        b"\x1b#8",
        b"\x1b]8;id=a;http://x\x1b\\link\x1b]8;;\x1b\\",
        b"\x1b[>4;1m\x1b[?4m",
        b"ab\x1bcz",
    ];
    for case in cases {
        let mut s = screen(30, 8);
        let out = s.feed(case);
        let events = Events::default();
        let mut bare = Term::new(
            Config {
                scrolling_history: SCROLLBACK,
                default_cursor_style: ansi::CursorStyle {
                    shape: ansi::CursorShape::Block,
                    blinking: true,
                },
                kitty_keyboard: false,
                osc52: alacritty_terminal::term::Osc52::Disabled,
                ..Config::default()
            },
            &Size { cols: 30, rows: 8 },
            events.clone(),
        );
        let mut p: Processor<StdSyncHandler> = Processor::new();
        p.advance(&mut bare, case);
        let cells = |t: &Term<Events>| {
            let g = t.grid();
            (0..8)
                .flat_map(|y| {
                    (0..30).map(move |x| {
                        let c = &g[Line(y)][Column(x)];
                        (c.c, c.fg, c.bg, c.flags, c.hyperlink(), c.underline_color())
                    })
                })
                .collect::<Vec<_>>()
        };
        let name = String::from_utf8_lossy(case);
        assert_eq!(cells(&s.term), cells(&bare), "{name:?}");
        assert_eq!(
            s.term.grid().cursor.point,
            bare.grid().cursor.point,
            "{name:?}"
        );
        assert_eq!(*s.term.mode(), *bare.mode(), "{name:?}");
        assert_eq!(s.term.cursor_style(), bare.cursor_style(), "{name:?}");
        for i in 0..alacritty_terminal::term::color::COUNT {
            assert_eq!(s.term.colors()[i], bare.colors()[i], "{name:?} colour {i}");
        }
        // Titles, bells and replies; colour queries as the palette index they asked for.
        let bare_out: Vec<String> = events
            .0
            .lock()
            .iter()
            .filter_map(|e| match e {
                TermEvent::Title(t) => Some(format!("title {t}")),
                TermEvent::Bell => Some("bell".into()),
                TermEvent::PtyWrite(r) => Some(format!("reply {r:?}")),
                TermEvent::ColorRequest(i, _) => Some(format!("colour {i}")),
                _ => None,
            })
            .collect();
        let screen_out: Vec<String> = out
            .into_iter()
            .map(|o| match o {
                Output::Title(t) => format!("title {t}"),
                Output::Bell => "bell".into(),
                Output::Reply(r) => match r.strip_prefix("\x1b]4;") {
                    Some(rest) => format!("colour {}", rest.split(';').next().unwrap_or("")),
                    None => format!("reply {r:?}"),
                },
            })
            .collect();
        assert_eq!(screen_out, bare_out, "{name:?}");
    }
}

#[test]
fn frame_lifts_faint_text_to_minimum_contrast() {
    let mut s = screen(20, 3);
    let theme = Theme {
        background: Some("#ffffff".into()),
        foreground: Some("#1f2328".into()),
        bright_black: Some("#d0d0d0".into()),
        ..Default::default()
    };
    s.set_palette(Palette::from_theme(&theme));
    s.feed(b"\x1b[90mf\x1b[0mt\x1b[90;7mi");
    let f = s.frame(true, true);
    let white = Rgba::rgb(0xff, 0xff, 0xff);
    assert!(
        find_glyph(&f, 0, 0).fg.contrast(white) >= 4.5,
        "faint grey darkens"
    );
    assert_eq!(
        find_glyph(&f, 1, 0).fg,
        Rgba::rgb(0x1f, 0x23, 0x28),
        "readable text is untouched"
    );
    assert!(
        find_glyph(&f, 2, 0)
            .fg
            .contrast(Rgba::rgb(0xd0, 0xd0, 0xd0))
            >= 4.5,
        "inverse text contrasts with its faint fill"
    );
}
