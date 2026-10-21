//! The terminal emulator state the UI renders: alacritty's VT engine behind xterm.js
//! semantics.
//!
//! The Svelte pane ran xterm.js 6; programs saw xterm.js's replies and the user saw xterm.js's
//! selection, search, links and colours. [`Screen`] keeps those observable behaviours: a
//! [`scanner`] answers and tracks the reports xterm.js owns (device attributes, mode reports,
//! DECRQSS, mouse protocols, cursor style) before bytes reach alacritty, and the selection,
//! search, link and colour modules are ports of the xterm.js code paths.

mod buffer;
mod links;
mod move_to_cell;
mod overline;
mod render;
mod scanner;
mod search;
mod selection;
mod theme;

use std::sync::Arc;
use std::time::Instant;

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection as Probe, SelectionType};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{self, Handler, NamedColor, Processor, StdSyncHandler};
use parking_lot::Mutex;

use crate::input::{MouseEncoding, MouseProtocol};
use buffer::Buffer;
pub use links::{Link, LinkKind};
pub use render::{BgRect, CursorDraw, CursorShape, Frame, Glyph, Underline, UnderlineStyle};
use scanner::{Csi, Item, Scanner};
pub use search::{Decorations, HIGHLIGHT_LIMIT, InvalidRegex, SearchOptions, SearchResults};
pub use selection::{
    ALT_CLICK_MOVE_CURSOR_MS, DRAG_SCROLL_INTERVAL_MS, MouseUp, SelectionMode, WORD_SEPARATOR,
    drag_scroll_amount,
};
pub use theme::{Palette, Rgba, Theme, default_ansi};

/// xterm.js scrollback in Recur's pane.
pub const SCROLLBACK: usize = 10_000;
/// The xterm.js build the Svelte app shipped, reported by XTVERSION.
pub const XTERM_VERSION: &str = "6.1.0-beta.304";

/// Something the screen produced while parsing output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// OSC 0 / OSC 2 title (`onTitleChange`).
    Title(String),
    /// A terminal report for the PTY (not user input).
    Reply(String),
    /// BEL (`onBell`; Recur's pane did not react to it).
    Bell,
}

/// The cursor style programs select with DECSCUSR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorStyle {
    Block,
    Underline,
    Bar,
}

#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<TermEvent>>>);

impl EventListener for Events {
    fn send_event(&self, event: TermEvent) {
        self.0.lock().push(event);
    }
}

#[derive(Clone, Copy)]
struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// The xterm.js state alacritty does not model the same way.
#[derive(Debug, Clone)]
struct Xterm {
    mouse_protocol: MouseProtocol,
    mouse_encoding: MouseEncoding,
    send_focus: bool,
    color_scheme_updates: bool,
    reverse_wraparound: bool,
    synchronized_output: bool,
    /// `options.cursorBlink`, which DECSET 12 rewrites. Recur's pane opens with it on.
    option_cursor_blink: bool,
    /// DECSCUSR override (`decPrivateModes.cursorStyle` / `cursorBlink`).
    decscusr: Option<(CursorStyle, bool)>,
    /// DECSTBM per buffer, 0-based inclusive (normal, alternate).
    scroll_region: [Option<(usize, usize)>; 2],
}

impl Default for Xterm {
    fn default() -> Self {
        Self {
            mouse_protocol: MouseProtocol::None,
            mouse_encoding: MouseEncoding::Default,
            send_focus: false,
            color_scheme_updates: false,
            reverse_wraparound: false,
            synchronized_output: false,
            option_cursor_blink: true,
            decscusr: None,
            scroll_region: [None, None],
        }
    }
}

impl Xterm {
    /// `CoreService.reset()`: DEC private modes back to defaults (DECSTR and RIS).
    fn reset_dec_modes(&mut self) {
        self.send_focus = false;
        self.color_scheme_updates = false;
        self.reverse_wraparound = false;
        self.synchronized_output = false;
        self.decscusr = None;
    }
}

pub struct Screen {
    term: Term<Events>,
    parser: Processor<StdSyncHandler>,
    events: Events,
    scanner: Scanner,
    palette: Palette,
    xt: Xterm,
    selection: selection::Selection,
    search: search::Search,
    /// Set once the cursor has been shown (first focus or keystroke), like xterm's
    /// `isCursorInitialized`.
    cursor_initialized: bool,
    /// Bumped whenever content, scroll position or selection changes, for renderers.
    generation: u64,
    /// Rows trimmed off the top of the scrollback by the last write.
    last_trim: usize,
    search_refresh_due: Option<Instant>,
}

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Self {
        let cols = cols.max(2);
        let rows = rows.max(1);
        let events = Events::default();
        let config = Config {
            scrolling_history: SCROLLBACK,
            default_cursor_style: ansi::CursorStyle {
                shape: ansi::CursorShape::Block,
                blinking: true,
            },
            kitty_keyboard: false,
            osc52: alacritty_terminal::term::Osc52::Disabled,
            ..Config::default()
        };
        let term = Term::new(config, &Size { cols, rows }, events.clone());
        Self {
            term,
            parser: Processor::new(),
            events,
            scanner: Scanner::new(),
            palette: Palette::default(),
            xt: Xterm::default(),
            selection: selection::Selection::default(),
            search: search::Search::default(),
            cursor_initialized: false,
            generation: 0,
            last_trim: 0,
            search_refresh_due: None,
        }
    }

    pub fn cols(&self) -> usize {
        self.term.columns()
    }

    pub fn rows(&self) -> usize {
        self.term.screen_lines()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn buf(&self) -> Buffer<'_> {
        Buffer::new(&self.term)
    }

    fn bump(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// Parses PTY output. Returns titles, bells and the reports the PTY must receive.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Output> {
        let mut out = Vec::new();
        for item in self.scanner.feed(bytes) {
            match item {
                Item::Forward(b) => self.advance(&b),
                Item::Esc { final_byte, raw } => {
                    if final_byte == b'c' {
                        self.full_reset();
                    }
                    self.advance(&raw);
                }
                Item::Csi(csi) => self.csi(csi, &mut out),
                Item::Decrqss(payload) => out.push(Output::Reply(self.decrqss(&payload))),
            }
        }
        self.drain_events(&mut out);
        self.bump();
        if self.search.cached_term().is_some()
            && self
                .search
                .last_options()
                .is_some_and(|o| o.decorations.is_some())
        {
            self.search_refresh_due = Some(Instant::now() + std::time::Duration::from_millis(200));
        }
        out
    }

    /// Advances the VT parser, tracking how many scrollback rows fell off the top so selection
    /// and search highlights stay on their text (xterm's `onTrim`).
    fn advance(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        // A two-cell probe parked on the newest scrollback row rides alacritty's selection
        // rotation; how far it moved is how many rows scrolled into history.
        let hist_before = self.term.history_size();
        let probing = !self.term.mode().contains(TermMode::ALT_SCREEN) && hist_before > 0;
        if probing {
            let mut probe = Probe::new(
                SelectionType::Simple,
                Point::new(Line(-1), Column(0)),
                Side::Left,
            );
            probe.update(Point::new(Line(-1), Column(1)), Side::Right);
            self.term.selection = Some(probe);
        }
        self.parser
            .advance(&mut overline::Overlined(&mut self.term), bytes);
        let mut trimmed = 0usize;
        if probing && !self.term.mode().contains(TermMode::ALT_SCREEN) {
            let hist_after = self.term.history_size();
            match self
                .term
                .selection
                .take()
                .and_then(|s| s.to_range(&self.term))
            {
                Some(range) => {
                    let scrolled = (-1 - range.start.line.0).max(0) as usize;
                    trimmed = scrolled.saturating_sub(hist_after.saturating_sub(hist_before));
                }
                // Scrollback cleared (ED 3), or the probe row itself scrolled out of history.
                None if hist_after < hist_before => trimmed = hist_before - hist_after,
                None => trimmed = hist_before + self.rows(),
            }
        }
        self.term.selection = None;
        if trimmed > 0 {
            self.last_trim += trimmed;
            if self.selection.model.handle_trim(trimmed as i64) {
                self.bump();
            }
            self.search.handle_trim(trimmed as i64);
        }
    }

    fn drain_events(&mut self, out: &mut Vec<Output>) {
        let events = std::mem::take(&mut *self.events.0.lock());
        for ev in events {
            match ev {
                TermEvent::Title(t) => out.push(Output::Title(t)),
                TermEvent::Bell => out.push(Output::Bell),
                TermEvent::PtyWrite(s) => out.push(Output::Reply(s)),
                // OSC 4 queries are answered from the palette; Recur's pane swallowed OSC 10/11/12
                // queries (the PTY side answers those).
                TermEvent::ColorRequest(index, _) if index < 256 => {
                    let c = self.term.colors()[index]
                        .map_or(self.palette.ansi[index], |c| Rgba::rgb(c.r, c.g, c.b));
                    out.push(Output::Reply(format!(
                        "\x1b]4;{index};rgb:{:02x}{0:02x}/{:02x}{1:02x}/{:02x}{2:02x}\x1b\\",
                        c.r(),
                        c.g(),
                        c.b()
                    )));
                }
                _ => {}
            }
        }
    }

    fn full_reset(&mut self) {
        let blink = self.xt.option_cursor_blink;
        self.xt = Xterm {
            option_cursor_blink: blink,
            ..Xterm::default()
        };
        self.selection.clear();
        self.search.clear_decorations(true);
    }

    fn reply(out: &mut Vec<Output>, s: String) {
        out.push(Output::Reply(s));
    }

    fn csi(&mut self, csi: Csi, out: &mut Vec<Output>) {
        let inter = csi.intermediates.as_slice();
        match (csi.private, inter, csi.final_byte) {
            (None, [], b'c') => {
                if csi.param(0) == 0 {
                    Self::reply(out, "\x1b[?1;2c".into());
                }
            }
            (Some(b'>'), [], b'c') => {
                if csi.param(0) == 0 {
                    Self::reply(out, "\x1b[>0;276;0c".into());
                }
            }
            (Some(b'>'), [], b'q') => {
                if csi.param(0) == 0 {
                    Self::reply(out, format!("\x1bP>|xterm.js({XTERM_VERSION})\x1b\\"));
                }
            }
            (None, [], b'n') => match csi.param(0) {
                5 => Self::reply(out, "\x1b[0n".into()),
                6 => {
                    let (x, y) = self.cursor_report();
                    Self::reply(out, format!("\x1b[{y};{x}R"));
                }
                _ => {}
            },
            (Some(b'?'), [], b'n') => match csi.param(0) {
                6 => {
                    let (x, y) = self.cursor_report();
                    Self::reply(out, format!("\x1b[?{y};{x}R"));
                }
                996 => Self::reply(out, self.color_scheme_report()),
                _ => {}
            },
            (p, [b'$'], b'p') if p.is_none() || p == Some(b'?') => {
                Self::reply(out, self.decrpm(csi.param(0), p.is_none()));
            }
            // windowOptions are all disabled in xterm.js by default (reports, resize, title stack).
            (None, [], b't') => {}
            // Kitty keyboard protocol is off in Recur's pane.
            (Some(b'?' | b'=' | b'>' | b'<'), [], b'u') => {}
            (Some(b'?'), [], b'h' | b'l') => {
                let set = csi.final_byte == b'h';
                for &p in &csi.params {
                    self.dec_mode(p, set);
                }
                self.advance(&csi.raw);
            }
            (None, [b' '], b'q') => {
                self.xt.decscusr = match csi.param(0) {
                    0 => None,
                    1 => Some((CursorStyle::Block, true)),
                    2 => Some((CursorStyle::Block, false)),
                    3 => Some((CursorStyle::Underline, true)),
                    4 => Some((CursorStyle::Underline, false)),
                    5 => Some((CursorStyle::Bar, true)),
                    6 => Some((CursorStyle::Bar, false)),
                    _ => self.xt.decscusr,
                };
                self.advance(&csi.raw);
            }
            (None, [b'!'], b'p') => {
                // DECSTR: xterm.js's soft reset, expressed to alacritty as the resets it implies.
                self.xt.reset_dec_modes();
                self.xt.scroll_region[self.buffer_index()] = None;
                self.advance(b"\x1b[?25h\x1b[0m\x1b[r\x1b[?6l\x1b[?1l\x1b>\x1b[4l\x1b[?7h\x1b[?2004l\x1b[?1004l\x1b(B");
            }
            (None, [], b'J') if csi.param(0) == 2 && !self.alt_screen() => {
                // xterm.js erases the screen in place (scrollOnEraseInDisplay is off); alacritty
                // would push it into the scrollback.
                let point = self.term.grid().cursor.point;
                self.term.grid_mut().cursor.point = Point::new(Line(0), Column(0));
                self.term.clear_screen(ansi::ClearMode::Below);
                self.term.grid_mut().cursor.point = point;
            }
            (None, [], b'm') => {
                self.advance(&csi.raw);
                match overline::sgr_effect(&csi.raw[2..csi.raw.len() - 1]) {
                    Some(true) => self.advance(overline::ON),
                    Some(false) => self.advance(overline::OFF),
                    None => {}
                }
            }
            (None, [], b'r') => {
                let rows = self.rows() as i64;
                let top = csi.param(0).max(1);
                let bottom = match csi.param(1) {
                    0 => rows,
                    b => b.min(rows),
                };
                if bottom > top {
                    self.xt.scroll_region[self.buffer_index()] =
                        Some((top as usize - 1, bottom as usize - 1));
                }
                self.advance(&csi.raw);
            }
            _ => self.advance(&csi.raw),
        }
    }

    fn buffer_index(&self) -> usize {
        usize::from(self.term.mode().contains(TermMode::ALT_SCREEN))
    }

    fn dec_mode(&mut self, p: i64, set: bool) {
        let xt = &mut self.xt;
        match p {
            9 => {
                xt.mouse_protocol = if set {
                    MouseProtocol::X10
                } else {
                    MouseProtocol::None
                }
            }
            1000 => {
                xt.mouse_protocol = if set {
                    MouseProtocol::Vt200
                } else {
                    MouseProtocol::None
                }
            }
            1002 => {
                xt.mouse_protocol = if set {
                    MouseProtocol::Drag
                } else {
                    MouseProtocol::None
                }
            }
            1003 => {
                xt.mouse_protocol = if set {
                    MouseProtocol::Any
                } else {
                    MouseProtocol::None
                }
            }
            1004 => xt.send_focus = set,
            1006 => {
                xt.mouse_encoding = if set {
                    MouseEncoding::Sgr
                } else {
                    MouseEncoding::Default
                }
            }
            1016 => {
                xt.mouse_encoding = if set {
                    MouseEncoding::SgrPixels
                } else {
                    MouseEncoding::Default
                }
            }
            12 => xt.option_cursor_blink = set,
            45 => xt.reverse_wraparound = set,
            2026 => xt.synchronized_output = set,
            2031 => xt.color_scheme_updates = set,
            _ => {}
        }
        // Selection is disabled (and cleared) while mouse events are active.
        if matches!(p, 9 | 1000 | 1002 | 1003) && self.xt.mouse_protocol != MouseProtocol::None {
            self.selection.clear();
        }
    }

    fn cursor_report(&self) -> (usize, usize) {
        let p = self.term.grid().cursor.point;
        (p.column.0 + 1, p.line.0 as usize + 1)
    }

    fn color_scheme_report(&self) -> String {
        format!("\x1b[?997;{}n", if self.palette.is_dark() { 1 } else { 2 })
    }

    fn decrpm(&self, p: i64, ansi: bool) -> String {
        const NOT_RECOGNIZED: u8 = 0;
        let b2v = |v: bool| if v { 1 } else { 2 };
        let mode = *self.term.mode();
        let has = |m: TermMode| b2v(mode.contains(m));
        let v = if ansi {
            match p {
                2 => 4,
                4 => has(TermMode::INSERT),
                12 => 3,
                20 => 2,
                _ => NOT_RECOGNIZED,
            }
        } else {
            let xt = &self.xt;
            match p {
                1 => has(TermMode::APP_CURSOR),
                3 => NOT_RECOGNIZED,
                6 => has(TermMode::ORIGIN),
                7 => has(TermMode::LINE_WRAP),
                8 => 3,
                9 => b2v(xt.mouse_protocol == MouseProtocol::X10),
                12 => b2v(xt.option_cursor_blink),
                25 => has(TermMode::SHOW_CURSOR),
                45 => b2v(xt.reverse_wraparound),
                66 => has(TermMode::APP_KEYPAD),
                67 => 4,
                1000 => b2v(xt.mouse_protocol == MouseProtocol::Vt200),
                1002 => b2v(xt.mouse_protocol == MouseProtocol::Drag),
                1003 => b2v(xt.mouse_protocol == MouseProtocol::Any),
                1004 => b2v(xt.send_focus),
                1005 | 1015 => 4,
                1006 => b2v(xt.mouse_encoding == MouseEncoding::Sgr),
                1016 => b2v(xt.mouse_encoding == MouseEncoding::SgrPixels),
                1048 => 1,
                47 | 1047 | 1049 => has(TermMode::ALT_SCREEN),
                2004 => has(TermMode::BRACKETED_PASTE),
                2026 => b2v(xt.synchronized_output),
                _ => NOT_RECOGNIZED,
            }
        };
        format!("\x1b[{}{p};{v}$y", if ansi { "" } else { "?" })
    }

    fn decrqss(&self, data: &[u8]) -> String {
        let reply = |s: String| format!("\x1b{s}\x1b\\");
        match data {
            b"\"q" => reply("P1$r0\"q".into()),
            b"\"p" => reply("P1$r61;1\"p".into()),
            b"r" => {
                let (top, bottom) =
                    self.xt.scroll_region[self.buffer_index()].unwrap_or((0, self.rows() - 1));
                reply(format!("P1$r{};{}r", top + 1, bottom + 1))
            }
            b"m" => reply("P1$r0m".into()),
            // xterm.js reports the configured option (block, blink on), not DECSCUSR.
            b" q" => reply(format!(
                "P1$r{} q",
                2 - u8::from(self.xt.option_cursor_blink)
            )),
            _ => reply("P0$r".into()),
        }
    }

    /// Resizes the grid (reflowing like the emulator does) and resets scroll regions.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(2), rows.max(1));
        if cols == self.cols() && rows == self.rows() {
            return;
        }
        let rows_changed = rows != self.rows();
        self.term.resize(Size { cols, rows });
        self.xt.scroll_region = [None, None];
        if rows_changed {
            self.selection.clear();
        }
        self.bump();
    }

    /// Replaces the colour palette (a theme switch). OSC 4/10/11/12 overrides reset with it, and
    /// apps that asked for DECSET 2031 get a colour-scheme notification.
    pub fn set_palette(&mut self, palette: Palette) -> Vec<Output> {
        if palette == self.palette {
            return Vec::new();
        }
        self.palette = palette;
        for i in 0..alacritty_terminal::term::color::COUNT {
            self.term.reset_color(i);
        }
        self.bump();
        if self.xt.color_scheme_updates {
            vec![Output::Reply(self.color_scheme_report())]
        } else {
            Vec::new()
        }
    }

    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// The effective foreground, background and cursor (palette with OSC overrides).
    fn special(&self, named: NamedColor, fallback: Rgba) -> Rgba {
        self.term.colors()[named].map_or(fallback, |c| Rgba::rgb(c.r, c.g, c.b))
    }

    /// Theme foreground with any OSC 10 override (link underlines use it).
    pub fn foreground(&self) -> Rgba {
        self.special(NamedColor::Foreground, self.palette.foreground)
    }

    /// xterm.js `term.clear()`: the prompt row becomes the first row and everything else,
    /// scrollback included, is dropped.
    pub fn clear(&mut self) {
        let point = self.term.grid().cursor.point;
        let history = self.term.history_size();
        if history == 0 && point.line.0 == 0 {
            return;
        }
        let row = self.term.grid()[point.line].clone();
        self.term.clear_screen(ansi::ClearMode::All);
        self.term.clear_screen(ansi::ClearMode::Saved);
        self.term.grid_mut()[Line(0)] = row;
        self.term.grid_mut().cursor.point.line = Line(0);
        self.term.scroll_display(Scroll::Bottom);
        self.selection.clear();
        self.search.clear_decorations(true);
        self.bump();
    }

    pub fn mouse_protocol(&self) -> MouseProtocol {
        self.xt.mouse_protocol
    }

    pub fn mouse_encoding(&self) -> MouseEncoding {
        self.xt.mouse_encoding
    }

    pub fn mouse_events_active(&self) -> bool {
        self.xt.mouse_protocol != MouseProtocol::None
    }

    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    pub fn app_cursor(&self) -> bool {
        self.term.mode().contains(TermMode::APP_CURSOR)
    }

    pub fn alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    pub fn cursor_hidden(&self) -> bool {
        !self.term.mode().contains(TermMode::SHOW_CURSOR)
    }

    /// Cursor style and whether it blinks (`decPrivateModes ?? options`).
    pub fn cursor_style(&self) -> (CursorStyle, bool) {
        self.xt
            .decscusr
            .unwrap_or((CursorStyle::Block, self.xt.option_cursor_blink))
    }

    /// Cursor cell, viewport-relative, if it is on screen.
    pub fn cursor_in_viewport(&self) -> Option<(usize, usize)> {
        let p = self.term.grid().cursor.point;
        let y = p.line.0 + self.term.grid().display_offset() as i32;
        (y >= 0 && (y as usize) < self.rows())
            .then(|| (p.column.0.min(self.cols() - 1), y as usize))
    }

    /// `_showCursor`: the cursor appears after the first focus or keystroke.
    pub fn show_cursor(&mut self) {
        if !self.cursor_initialized {
            self.cursor_initialized = true;
            self.bump();
        }
    }

    pub fn cursor_initialized(&self) -> bool {
        self.cursor_initialized
    }

    /// The focus report to send when focus changes and the app asked for DECSET 1004.
    pub fn focus_report(&self, focused: bool) -> Option<&'static str> {
        self.xt
            .send_focus
            .then_some(if focused { "\x1b[I" } else { "\x1b[O" })
    }

    // ---- viewport ----

    /// xterm `ydisp`: first buffer row in the viewport.
    pub fn viewport_top(&self) -> usize {
        self.buf().ydisp()
    }

    /// xterm `ybase`: scrollback rows above the screen.
    pub fn scrollback_len(&self) -> usize {
        self.term.history_size()
    }

    pub fn total_rows(&self) -> usize {
        self.buf().len()
    }

    pub fn at_bottom(&self) -> bool {
        self.term.grid().display_offset() == 0
    }

    /// Scrolls the viewport by `lines` (positive = towards newer output).
    pub fn scroll_lines(&mut self, lines: i64) {
        if lines == 0 {
            return;
        }
        self.term.scroll_display(Scroll::Delta(-lines as i32));
        self.bump();
    }

    /// Scrolls so `row` is the viewport's top row.
    pub fn scroll_to_row(&mut self, row: usize) {
        let ydisp = self.viewport_top() as i64;
        self.scroll_lines(row as i64 - ydisp);
    }

    pub fn scroll_to_bottom(&mut self) {
        if !self.at_bottom() {
            self.term.scroll_display(Scroll::Bottom);
            self.bump();
        }
    }

    /// `triggerDataEvent(..., wasUserInput)`: user input scrolls to the prompt and clears the
    /// selection.
    pub fn user_input(&mut self) {
        self.scroll_to_bottom();
        if self.selection.has_selection(&self.buf()) {
            self.selection.clear();
            self.bump();
        }
        self.show_cursor();
    }

    // ---- selection ----

    pub fn has_selection(&self) -> bool {
        self.selection.has_selection(&self.buf())
    }

    pub fn selection_text(&self) -> String {
        self.selection.text(&self.buf())
    }

    pub fn selection_mode(&self) -> SelectionMode {
        self.selection.mode()
    }

    pub fn selection_dragging(&self) -> bool {
        self.selection.is_dragging()
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear();
        self.bump();
    }

    pub fn select_all(&mut self) {
        self.selection.select_all();
        self.bump();
    }

    /// `[start, end)` of the selection in absolute rows, if any.
    pub fn selection_range(&self) -> Option<((i64, i64), (i64, i64))> {
        let buf = self.buf();
        self.selection
            .range(&buf)
            .filter(|_| self.selection.has_selection(&buf))
            .map(|(s, e)| ((s[0], s[1]), (e[0], e[1])))
    }

    fn link_selection(&self, x1: i64, y1: i64) -> Option<([i64; 2], i64)> {
        let buf = self.buf();
        links::link_at(&buf, x1, y1).map(|l| {
            (
                [l.start.0 - 1, l.start.1 - 1],
                l.range_length(buf.cols() as i64),
            )
        })
    }

    /// Primary-button press on the grid. `sel` is the 1-based selection cell
    /// ([`crate::input::cell_coords`] with `selection = true`) and `link` the 1-based hover cell.
    pub fn selection_mouse_down(
        &mut self,
        sel: (i64, i64),
        link: (i64, i64),
        click_count: usize,
        shift: bool,
        column: bool,
    ) {
        let coords = [sel.0 - 1, sel.1 - 1 + self.viewport_top() as i64];
        let link = (click_count == 2)
            .then(|| self.link_selection(link.0, link.1))
            .flatten();
        let buf = Buffer::new(&self.term);
        self.selection
            .mouse_down(&buf, coords, click_count, shift, column, link);
        self.bump();
    }

    /// Drag with the button held. `offset_y` is the pointer's y relative to the grid top,
    /// `grid_height` the grid's pixel height (for drag-scrolling).
    pub fn selection_mouse_move(&mut self, sel: (i64, i64), offset_y: f64, grid_height: f64) {
        let coords = [sel.0 - 1, sel.1 - 1 + self.viewport_top() as i64];
        let amount = drag_scroll_amount(offset_y, grid_height);
        let buf = Buffer::new(&self.term);
        if self.selection.mouse_move(&buf, coords, amount) {
            self.bump();
        }
    }

    /// One drag-scroll tick (every [`DRAG_SCROLL_INTERVAL_MS`] while dragging).
    pub fn selection_drag_tick(&mut self) {
        let amount = self.selection.drag_scroll();
        if amount == 0 {
            return;
        }
        self.scroll_lines(amount);
        let buf = Buffer::new(&self.term);
        self.selection.after_drag_scroll(&buf);
        self.bump();
    }

    /// Button release. `cell` is the 1-based non-selection cell under the pointer.
    pub fn selection_mouse_up(&mut self, elapsed_ms: u64, alt: bool, cell: (i64, i64)) -> MouseUp {
        let app_cursor = self.app_cursor();
        let buf = Buffer::new(&self.term);
        let res = self.selection.mouse_up(
            &buf,
            elapsed_ms,
            alt,
            Some([cell.0 - 1, cell.1 - 1]),
            app_cursor,
        );
        self.bump();
        res
    }

    /// Right-click word selection (`rightClickSelectsWord`, on for macOS).
    pub fn right_click_select(&mut self, sel: (i64, i64), link: (i64, i64)) {
        let coords = [sel.0 - 1, sel.1 - 1 + self.viewport_top() as i64];
        let link = self.link_selection(link.0, link.1);
        let buf = Buffer::new(&self.term);
        if self.selection.right_click_select(&buf, coords, link) {
            self.bump();
        }
    }

    // ---- search ----

    /// `findNext` / `findPrevious`. Returns whether a match was selected and, with decorations,
    /// the results event.
    pub fn find(
        &mut self,
        term: &str,
        opts: &SearchOptions,
        forward: bool,
    ) -> Result<(bool, Option<SearchResults>), InvalidRegex> {
        self.find_inner(term, opts, forward, false)
    }

    fn find_inner(
        &mut self,
        term: &str,
        opts: &SearchOptions,
        forward: bool,
        no_scroll: bool,
    ) -> Result<(bool, Option<SearchResults>), InvalidRegex> {
        let buf = Buffer::new(&self.term);
        let res = self
            .search
            .find(&buf, &mut self.selection, term, opts, forward, no_scroll);
        self.bump();
        match res {
            Ok((found, scroll, results)) => {
                if let Some(lines) = scroll {
                    self.scroll_lines(lines);
                }
                Ok((found, results))
            }
            Err(e) => {
                self.search.clear_decorations(false);
                Err(e)
            }
        }
    }

    pub fn clear_search(&mut self) {
        self.search.clear_decorations(false);
        self.search_refresh_due = None;
        self.bump();
    }

    /// When output changed while a search was decorating, xterm re-runs it 200ms later
    /// (`findPrevious` incremental, no scroll) so counts and highlights follow the text.
    pub fn search_refresh_due(&self) -> Option<Instant> {
        self.search_refresh_due
    }

    pub fn run_search_refresh(
        &mut self,
    ) -> Option<Result<(bool, Option<SearchResults>), InvalidRegex>> {
        self.search_refresh_due = None;
        let term = self.search.cached_term()?.to_string();
        let mut opts = self.search.last_options()?.clone();
        opts.decorations?;
        opts.incremental = true;
        self.search.clear_cached_term();
        Some(self.find_inner(&term, &opts, false, true))
    }

    /// Schedules the same refresh after a resize (`onResize`).
    fn schedule_search_refresh(&mut self) {
        if self.search.cached_term().is_some()
            && self
                .search
                .last_options()
                .is_some_and(|o| o.decorations.is_some())
        {
            self.search_refresh_due = Some(Instant::now() + std::time::Duration::from_millis(200));
        }
    }

    /// Resizes and schedules a search refresh, as the addon's `onResize` hook does.
    pub fn resize_and_refresh(&mut self, cols: usize, rows: usize) {
        let before = (self.cols(), self.rows());
        self.resize(cols, rows);
        if before != (self.cols(), self.rows()) {
            self.schedule_search_refresh();
        }
    }

    // ---- links ----

    /// The link under 1-based viewport cell (`x`, `y`).
    pub fn link_at(&self, x: i64, y: i64) -> Option<Link> {
        let buf = self.buf();
        links::link_at(&buf, x, y + buf.ydisp() as i64)
    }

    /// Text of absolute row `row` (for tests and accessibility).
    pub fn row_text(&self, row: usize, trim_right: bool) -> String {
        self.buf().translate(row, trim_right, 0, None)
    }

    /// Viewport row text.
    pub fn viewport_text(&self) -> Vec<String> {
        let top = self.viewport_top();
        (0..self.rows())
            .map(|y| self.row_text(top + y, true))
            .collect()
    }

    /// Trimmed rows since the last call (for renderers caching per-row state).
    pub fn take_trimmed(&mut self) -> usize {
        std::mem::take(&mut self.last_trim)
    }

    /// Synchronized-output (DECSET 2026) deadline, after which buffered output is flushed.
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.parser.sync_timeout().sync_timeout()
    }

    pub fn flush_sync(&mut self) {
        if self.parser.sync_timeout().sync_timeout().is_some() {
            self.parser
                .stop_sync(&mut overline::Overlined(&mut self.term));
            self.bump();
        }
    }

    /// The renderable frame: resolved colours, glyphs and cursor, as xterm.js's WebGL renderer
    /// computes them. `focused` picks active/inactive selection and cursor styles;
    /// `cursor_blink_on` is the blink phase.
    pub fn frame(&self, focused: bool, cursor_blink_on: bool) -> Frame {
        render::frame(self, focused, cursor_blink_on)
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
