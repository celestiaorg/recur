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

fn decorations() -> SearchOptions {
    SearchOptions {
        decorations: Some(Decorations {
            match_background: Rgba::rgb(1, 2, 3),
            active_match_background: Rgba::rgb(4, 5, 6),
        }),
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
fn answers_device_attributes_like_xterm_js() {
    let mut s = screen(80, 24);
    assert_eq!(replies(&s.feed(b"\x1b[c")), vec!["\x1b[?1;2c"]);
    assert_eq!(replies(&s.feed(b"\x1b[0c")), vec!["\x1b[?1;2c"]);
    assert!(replies(&s.feed(b"\x1b[1c")).is_empty());
    assert_eq!(replies(&s.feed(b"\x1b[>c")), vec!["\x1b[>0;276;0c"]);
    assert_eq!(
        replies(&s.feed(b"\x1b[>q")),
        vec![format!("\x1bP>|xterm.js({XTERM_VERSION})\x1b\\")]
    );
    assert_eq!(replies(&s.feed(b"\x1b[5n")), vec!["\x1b[0n"]);
    s.feed(b"ab\r\ncd");
    assert_eq!(replies(&s.feed(b"\x1b[6n")), vec!["\x1b[2;3R"]);
    assert_eq!(replies(&s.feed(b"\x1b[?6n")), vec!["\x1b[?2;3R"]);
}

#[test]
fn window_ops_and_kitty_queries_are_ignored() {
    let mut s = screen(80, 24);
    assert!(replies(&s.feed(b"\x1b[18t\x1b[14t\x1b[22;0t\x1b[?u\x1b[>1u")).is_empty());
}

#[test]
fn reports_modes_with_decrpm() {
    let mut s = screen(80, 24);
    assert_eq!(replies(&s.feed(b"\x1b[?2004$p")), vec!["\x1b[?2004;2$y"]);
    s.feed(b"\x1b[?2004h\x1b[?1000h\x1b[?1006h");
    assert_eq!(replies(&s.feed(b"\x1b[?2004$p")), vec!["\x1b[?2004;1$y"]);
    assert_eq!(replies(&s.feed(b"\x1b[?1000$p")), vec!["\x1b[?1000;1$y"]);
    assert_eq!(replies(&s.feed(b"\x1b[?1006$p")), vec!["\x1b[?1006;1$y"]);
    assert_eq!(
        replies(&s.feed(b"\x1b[?12$p")),
        vec!["\x1b[?12;1$y"],
        "Recur opens with cursorBlink on"
    );
    assert_eq!(replies(&s.feed(b"\x1b[?1005$p")), vec!["\x1b[?1005;4$y"]);
    assert_eq!(replies(&s.feed(b"\x1b[?9999$p")), vec!["\x1b[?9999;0$y"]);
    assert_eq!(replies(&s.feed(b"\x1b[4$p")), vec!["\x1b[4;2$y"]);
    assert_eq!(replies(&s.feed(b"\x1b[20$p")), vec!["\x1b[20;2$y"]);
}

#[test]
fn decrqss_reports() {
    let mut s = screen(80, 24);
    assert_eq!(
        replies(&s.feed(b"\x1bP$q q\x1b\\")),
        vec!["\x1bP1$r1 q\x1b\\"]
    );
    assert_eq!(
        replies(&s.feed(b"\x1bP$qm\x1b\\")),
        vec!["\x1bP1$r0m\x1b\\"]
    );
    assert_eq!(
        replies(&s.feed(b"\x1bP$qr\x1b\\")),
        vec!["\x1bP1$r1;24r\x1b\\"]
    );
    s.feed(b"\x1b[3;10r");
    assert_eq!(
        replies(&s.feed(b"\x1bP$qr\x1b\\")),
        vec!["\x1bP1$r3;10r\x1b\\"]
    );
    assert_eq!(replies(&s.feed(b"\x1bP$qz\x1b\\")), vec!["\x1bP0$r\x1b\\"]);
}

#[test]
fn tracks_xterm_mouse_protocols() {
    let mut s = screen(80, 24);
    assert_eq!(
        (s.mouse_protocol(), s.mouse_encoding()),
        (MouseProtocol::None, MouseEncoding::Default)
    );
    s.feed(b"\x1b[?1002h\x1b[?1006h");
    assert_eq!(
        (s.mouse_protocol(), s.mouse_encoding()),
        (MouseProtocol::Drag, MouseEncoding::Sgr)
    );
    s.feed(b"\x1b[?1003;1016h");
    assert_eq!(
        (s.mouse_protocol(), s.mouse_encoding()),
        (MouseProtocol::Any, MouseEncoding::SgrPixels)
    );
    s.feed(b"\x1b[?1003l");
    assert_eq!(s.mouse_protocol(), MouseProtocol::None);
    s.feed(b"\x1b[?9h\x1b[?1004h");
    assert_eq!(s.mouse_protocol(), MouseProtocol::X10);
    assert_eq!(s.focus_report(true), Some("\x1b[I"));
    s.feed(b"\x1bc");
    assert_eq!(
        (s.mouse_protocol(), s.mouse_encoding(), s.focus_report(true)),
        (MouseProtocol::None, MouseEncoding::Default, None)
    );
}

#[test]
fn cursor_style_follows_decscusr_and_options() {
    let mut s = screen(80, 24);
    assert_eq!(s.cursor_style(), (CursorStyle::Block, true));
    s.feed(b"\x1b[6 q");
    assert_eq!(s.cursor_style(), (CursorStyle::Bar, false));
    s.feed(b"\x1b[3 q");
    assert_eq!(s.cursor_style(), (CursorStyle::Underline, true));
    s.feed(b"\x1b[0 q\x1b[?12l");
    assert_eq!(s.cursor_style(), (CursorStyle::Block, false));
    s.feed(b"\x1b[5 q\x1b[!p");
    assert_eq!(
        s.cursor_style(),
        (CursorStyle::Block, false),
        "DECSTR drops DECSCUSR but keeps the option"
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
fn answers_osc4_queries_from_the_palette() {
    let mut s = screen(80, 24);
    assert_eq!(
        replies(&s.feed(b"\x1b]4;1;?\x07")),
        vec!["\x1b]4;1;rgb:cccc/0000/0000\x1b\\"]
    );
    s.feed(b"\x1b]4;1;rgb:12/34/56\x07");
    assert_eq!(
        replies(&s.feed(b"\x1b]4;1;?\x07")),
        vec!["\x1b]4;1;rgb:1212/3434/5656\x1b\\"]
    );
    assert!(
        replies(&s.feed(b"\x1b]11;?\x07")).is_empty(),
        "OSC 11 queries are swallowed"
    );
}

#[test]
fn color_scheme_query_and_updates() {
    let mut s = screen(80, 24);
    assert_eq!(replies(&s.feed(b"\x1b[?996n")), vec!["\x1b[?997;1n"]);
    s.feed(b"\x1b[?2031h");
    let light = Palette::from_theme(&Theme {
        background: Some("#ffffff".into()),
        foreground: Some("#000000".into()),
        ..Default::default()
    });
    assert_eq!(replies(&s.set_palette(light)), vec!["\x1b[?997;2n"]);
}

#[test]
fn erase_display_does_not_push_the_screen_into_scrollback() {
    let mut s = screen(20, 3);
    s.feed(b"one\r\ntwo\r\nthree\r\nfour");
    assert_eq!(s.scrollback_len(), 1);
    s.feed(b"\x1b[2J");
    assert_eq!(
        s.scrollback_len(),
        1,
        "xterm.js scrollOnEraseInDisplay is off"
    );
    assert_eq!(s.viewport_text(), vec!["", "", ""]);
    assert_eq!(s.row_text(0, true), "one");
}

#[test]
fn clear_keeps_only_the_prompt_row() {
    let mut s = screen(20, 3);
    s.feed(b"a\r\nb\r\nc\r\nd\r\n$ ls");
    assert!(s.scrollback_len() > 0);
    s.clear();
    assert_eq!(s.scrollback_len(), 0);
    assert_eq!(s.viewport_text(), vec!["$ ls", "", ""]);
    assert_eq!(s.cursor_in_viewport(), Some((4, 0)));
}

fn sel(x: i64, y: i64) -> (i64, i64) {
    (x, y)
}

#[test]
fn drag_selects_and_copies_text() {
    let mut s = screen(20, 3);
    s.feed(b"hello world\r\nsecond line");
    // Selection cells are 1-based with the half-cell rounding applied by the caller.
    s.selection_mouse_down(sel(1, 1), sel(1, 1), 1, false, false);
    s.selection_mouse_move(sel(6, 2), 20.0, 54.0);
    s.selection_mouse_up(300, false, sel(6, 2));
    assert!(s.has_selection());
    assert_eq!(s.selection_text(), "hello world\nsecon");
    s.user_input();
    assert!(!s.has_selection(), "typing clears the selection");
}

#[test]
fn double_click_selects_a_word_and_triple_click_a_line() {
    let mut s = screen(30, 3);
    s.feed(b"cargo test --package recur");
    s.selection_mouse_down(sel(9, 1), sel(9, 1), 2, false, false);
    assert_eq!(s.selection_text(), "test");
    s.selection_mouse_down(sel(15, 1), sel(15, 1), 2, false, false);
    assert_eq!(s.selection_text(), "--package");
    s.selection_mouse_down(sel(2, 1), sel(2, 1), 3, false, false);
    assert_eq!(s.selection_text(), "cargo test --package recur");
}

#[test]
fn word_selection_follows_wrapped_rows() {
    let mut s = screen(10, 3);
    s.feed(b"xx abcdefghijklm yy");
    s.selection_mouse_down(sel(6, 1), sel(6, 1), 2, false, false);
    assert_eq!(s.selection_text(), "abcdefghijklm");
}

#[test]
fn selection_text_joins_wrapped_rows() {
    let mut s = screen(5, 4);
    s.feed(b"abcdefgh\r\nij");
    s.select_all();
    assert_eq!(s.selection_text(), "abcdefgh\nij\n");
}

#[test]
fn shift_click_extends_and_alt_click_moves_the_cursor() {
    let mut s = screen(20, 3);
    s.feed(b"$ echo hi");
    s.selection_mouse_down(sel(1, 1), sel(1, 1), 1, false, false);
    s.selection_mouse_up(10, false, sel(1, 1));
    s.selection_mouse_down(sel(7, 1), sel(7, 1), 1, true, false);
    s.selection_mouse_up(10, false, sel(7, 1));
    assert_eq!(s.selection_text(), "$ echo");
    s.clear_selection();
    s.selection_mouse_down(sel(3, 1), sel(3, 1), 1, false, false);
    let up = s.selection_mouse_up(100, true, sel(3, 1));
    assert_eq!(up, MouseUp::MoveCursor("\x1b[D".repeat(7)));
}

#[test]
fn column_selection() {
    let mut s = screen(10, 3);
    s.feed(b"abcdef\r\nghijkl\r\nmnopqr");
    s.selection_mouse_down(sel(2, 1), sel(2, 1), 1, false, true);
    s.selection_mouse_move(sel(4, 3), 30.0, 54.0);
    assert_eq!(s.selection_mode(), SelectionMode::Column);
    assert_eq!(s.selection_text(), "bc\nhi\nno");
}

#[test]
fn right_click_selects_the_word_unless_inside_the_selection() {
    let mut s = screen(30, 3);
    s.feed(b"open http://localhost:3000 now");
    s.right_click_select(sel(2, 1), sel(2, 1));
    assert_eq!(s.selection_text(), "open");
    s.right_click_select(sel(3, 1), sel(3, 1));
    assert_eq!(
        s.selection_text(),
        "open",
        "click inside keeps the selection"
    );
    s.right_click_select(sel(10, 1), sel(10, 1));
    assert_eq!(
        s.selection_text(),
        "http://localhost:3000",
        "a link selects as a whole"
    );
}

#[test]
fn selection_follows_text_when_scrollback_trims() {
    let mut s = screen(20, 3);
    for i in 0..SCROLLBACK + 10 {
        s.feed(format!("line {i}\r\n").as_bytes());
    }
    assert_eq!(s.scrollback_len(), SCROLLBACK);
    let row = s.total_rows() - 3;
    s.scroll_to_row(row - 2);
    // Select a row in the viewport.
    s.selection_mouse_down(sel(1, 1), sel(1, 1), 3, false, false);
    let before = s.selection_text();
    s.feed(b"more\r\nmore\r\n");
    assert_eq!(
        s.selection_text(),
        before,
        "selection stays on its text after trimming"
    );
}

#[test]
fn search_counts_matches_and_steps_through_them() {
    let mut s = screen(40, 5);
    s.feed(b"hello world hello\r\n");
    let opts = decorations();
    let (found, res) = s.find("hello", &opts, true).unwrap();
    assert!(found);
    assert_eq!(res, Some(SearchResults { index: 0, count: 2 }));
    assert_eq!(s.selection_text(), "hello");
    let (_, res) = s.find("hello", &opts, true).unwrap();
    assert_eq!(res, Some(SearchResults { index: 1, count: 2 }));
    let (_, res) = s.find("hello", &opts, true).unwrap();
    assert_eq!(
        res,
        Some(SearchResults { index: 0, count: 2 }),
        "wraps around"
    );
    let (_, res) = s.find("hello", &opts, false).unwrap();
    assert_eq!(
        res,
        Some(SearchResults { index: 1, count: 2 }),
        "previous wraps backwards"
    );
}

#[test]
fn search_case_regex_and_whole_word() {
    let mut s = screen(40, 5);
    s.feed(b"Hello world hello helloo\r\n");
    let cs = SearchOptions {
        case_sensitive: true,
        ..decorations()
    };
    assert_eq!(
        s.find("hello", &cs, true).unwrap().1,
        Some(SearchResults { index: 0, count: 2 })
    );
    s.clear_search();
    let ww = SearchOptions {
        whole_word: true,
        ..decorations()
    };
    assert_eq!(
        s.find("hello", &ww, true).unwrap().1.map(|r| r.count),
        Some(2)
    );
    s.clear_search();
    let re = SearchOptions {
        regex: true,
        ..decorations()
    };
    assert_eq!(
        s.find("hel+o+", &re, true).unwrap().1.map(|r| r.count),
        Some(3)
    );
    s.clear_search();
    assert_eq!(
        s.find("(?<=world )hello", &re, true)
            .unwrap()
            .1
            .map(|r| r.count),
        Some(1),
        "ECMAScript lookbehind"
    );
    assert!(s.find("(unclosed", &re, true).is_err());
}

#[test]
fn search_without_matches_and_empty_terms() {
    let mut s = screen(40, 5);
    s.feed(b"abc\r\n");
    assert_eq!(
        s.find("zzz", &decorations(), true).unwrap(),
        (
            false,
            Some(SearchResults {
                index: -1,
                count: 0
            })
        )
    );
    assert_eq!(
        s.find("", &decorations(), true).unwrap(),
        (
            false,
            Some(SearchResults {
                index: -1,
                count: 0
            })
        )
    );
    assert!(!s.has_selection());
}

#[test]
fn search_highlight_limit() {
    let mut s = screen(80, 24);
    for _ in 0..30 {
        s.feed("ab ".repeat(40).as_bytes());
    }
    let (_, res) = s.find("ab", &decorations(), true).unwrap();
    assert_eq!(res.map(|r| r.count), Some(HIGHLIGHT_LIMIT));
}

#[test]
fn search_refreshes_after_output() {
    let mut s = screen(40, 5);
    s.feed(b"x hit\r\n");
    s.find("hit", &decorations(), true).unwrap();
    assert!(s.search_refresh_due().is_none());
    s.feed(b"another hit\r\n");
    assert!(s.search_refresh_due().is_some());
    let (_, res) = s.run_search_refresh().unwrap().unwrap();
    assert_eq!(
        res,
        Some(SearchResults { index: 0, count: 2 }),
        "keeps the current match selected"
    );
}

#[test]
fn search_scrolls_matches_into_view() {
    let mut s = screen(20, 3);
    s.feed(b"needle\r\n");
    for i in 0..30 {
        s.feed(format!("filler {i}\r\n").as_bytes());
    }
    assert!(s.at_bottom());
    s.find("needle", &decorations(), true).unwrap();
    assert!(!s.at_bottom());
    assert_eq!(s.viewport_top(), 0);
}

#[test]
fn detects_web_links_and_osc8_hyperlinks() {
    let mut s = screen(60, 3);
    s.feed(b"see https://example.com/a?b=1, then \x1b]8;;https://recur.dev/x\x1b\\docs\x1b]8;;\x1b\\ end");
    let web = s.link_at(6, 1).unwrap();
    assert_eq!(
        (web.kind, web.text.as_str(), web.start, web.end),
        (LinkKind::Web, "https://example.com/a?b=1", (5, 1), (29, 1))
    );
    let osc = s.link_at(38, 1).unwrap();
    assert_eq!(
        (osc.kind, osc.text.as_str()),
        (LinkKind::Hyperlink, "https://recur.dev/x")
    );
    assert!(s.link_at(2, 1).is_none());
}

#[test]
fn web_links_span_wrapped_rows() {
    let mut s = screen(12, 3);
    s.feed(b"x https://example.com/path");
    let link = s.link_at(3, 1).unwrap();
    assert_eq!(link.text, "https://example.com/path");
    assert_eq!((link.start, link.end), ((3, 1), (2, 3)));
    assert!(
        s.link_at(1, 2).is_some(),
        "the continuation row hit-tests too"
    );
}

fn find_glyph(f: &Frame, col: usize, row: usize) -> &Glyph {
    f.glyphs
        .iter()
        .find(|g| g.col == col && g.row == row)
        .unwrap()
}

#[test]
fn frame_resolves_xterm_colours() {
    let mut s = screen(20, 3);
    let theme = Theme {
        background: Some("#141414".into()),
        foreground: Some("#e6e4e1".into()),
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
    s.find("abc", &decorations(), true).unwrap();
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
    s.select_all();
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
