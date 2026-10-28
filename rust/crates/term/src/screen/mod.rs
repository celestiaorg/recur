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
pub use search::{Decorations, HIGHLIGHT_LIMIT, InvalidRegex, SearchOptions, SearchResults};
pub use selection::{
    ALT_CLICK_MOVE_CURSOR_MS, DRAG_SCROLL_INTERVAL_MS, MouseUp, SelectionMode, WORD_SEPARATOR,
    drag_scroll_amount,
};
pub use theme::{Palette, Rgba, Theme, default_ansi};

/// xterm.js scrollback in Recur's pane.
pub const SCROLLBACK: usize = 10_000;

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

pub struct Screen {
    term: Term<Events>,
    parser: Processor<StdSyncHandler>,
    events: Events,
    /// An unfinished `CSI` at the end of the last feed, held until the next one so overline
    /// SGRs are seen whole.
    sgr_tail: Vec<u8>,
    palette: Palette,
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
            sgr_tail: Vec::new(),
            palette: Palette::default(),
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
        let mut data = std::mem::take(&mut self.sgr_tail);
        data.extend_from_slice(bytes);
        let (mut done, mut from) = (0, 0);
        loop {
            match overline::next_sgr(&data, from) {
                Ok(Some(body)) => {
                    if let Some(on) = overline::sgr_effect(&data[body.clone()]) {
                        self.advance(&data[done..=body.end]);
                        self.advance(if on { overline::ON } else { overline::OFF });
                        done = body.end + 1;
                    }
                    from = body.end + 1;
                }
                Ok(None) => break,
                Err(esc) => {
                    self.sgr_tail = data.split_off(esc);
                    break;
                }
            }
        }
        self.advance(&data[done..]);
        let mut out = Vec::new();
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
                // OSC 4 / 10 / 11 / 12 queries, answered from the live palette.
                TermEvent::ColorRequest(index, reply) => {
                    let fallback = match index {
                        0..256 => self.palette.ansi[index],
                        256 => self.palette.foreground,
                        257 => self.palette.background,
                        _ => self.palette.cursor,
                    };
                    let c =
                        self.term.colors()[index].map_or(fallback, |c| Rgba::rgb(c.r, c.g, c.b));
                    out.push(Output::Reply(reply(ansi::Rgb {
                        r: c.r(),
                        g: c.g(),
                        b: c.b(),
                    })));
                }
                _ => {}
            }
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
        if rows_changed {
            self.selection.clear();
        }
        self.bump();
    }

    /// Replaces the colour palette (a theme switch). OSC 4/10/11/12 overrides reset with it.
    pub fn set_palette(&mut self, palette: Palette) {
        if palette == self.palette {
            return;
        }
        self.palette = palette;
        for i in 0..alacritty_terminal::term::color::COUNT {
            self.term.reset_color(i);
        }
        self.bump();
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
        let mode = self.term.mode();
        if mode.contains(TermMode::MOUSE_MOTION) {
            MouseProtocol::Any
        } else if mode.contains(TermMode::MOUSE_DRAG) {
            MouseProtocol::Drag
        } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            MouseProtocol::Vt200
        } else {
            MouseProtocol::None
        }
    }

    pub fn mouse_encoding(&self) -> MouseEncoding {
        if self.term.mode().contains(TermMode::SGR_MOUSE) {
            MouseEncoding::Sgr
        } else {
            MouseEncoding::Default
        }
    }

    pub fn mouse_events_active(&self) -> bool {
        self.term.mode().intersects(TermMode::MOUSE_MODE)
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

    pub fn cursor_style(&self) -> ansi::CursorStyle {
        self.term.cursor_style()
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
        self.term
            .mode()
            .contains(TermMode::FOCUS_IN_OUT)
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
