//! The terminal emulator state the UI renders, on alacritty's VT engine.
//!
//! alacritty parses output, answers terminal queries, tracks modes and the cursor style, and
//! owns selection and regex search. [`Screen`] adds what Recur's pane needs on top: replies to
//! colour queries from the live theme palette, overline (SGR 53, which alacritty does not
//! model), link detection, find highlights and the renderable [`Frame`].
//! Exact xterm.js replies and edge-case behaviour are not a goal.

mod links;
mod move_to_cell;
mod overline;
mod render;
mod search;
mod theme;

use std::sync::Arc;
use std::time::Instant;

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{self, Handler, NamedColor, Processor, StdSyncHandler};
use parking_lot::Mutex;

use crate::input::{MouseEncoding, MouseProtocol};
pub use alacritty_terminal::index::Side;
pub use links::Link;
pub use render::{BgRect, CursorDraw, CursorShape, Frame, Glyph, Underline, UnderlineStyle};
use search::Search;
pub use search::{Decorations, HIGHLIGHT_LIMIT, InvalidRegex, SearchOptions, SearchResults};
pub use theme::{Palette, Rgba, Theme, default_ansi};

/// Scrollback lines in Recur's pane.
pub const SCROLLBACK: usize = 10_000;
/// Interval between drag-scroll steps while the mouse is held outside the grid.
pub const DRAG_SCROLL_INTERVAL_MS: u64 = 50;
/// A click shorter than this with Alt held moves the prompt cursor.
const ALT_CLICK_MOVE_CURSOR_MS: u64 = 500;
/// How long after output or a resize the find count is recounted.
const SEARCH_REFRESH: std::time::Duration = std::time::Duration::from_millis(200);

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
    search: Option<Search>,
    search_refresh_due: Option<Instant>,
    urls: Option<RegexSearch>,
    /// A mouse selection drag is in progress, scrolling this many lines per tick.
    dragging: Option<i64>,
    /// Set once the cursor has been shown (first focus or keystroke).
    cursor_initialized: bool,
    /// Bumped whenever content, scroll position or selection changes, for renderers.
    generation: u64,
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
            search: None,
            search_refresh_due: None,
            urls: None,
            dragging: None,
            cursor_initialized: false,
            generation: 0,
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
        self.schedule_search_refresh();
        out
    }

    fn advance(&mut self, bytes: &[u8]) {
        self.parser
            .advance(&mut overline::Overlined(&mut self.term), bytes);
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

    /// Resizes the grid, reflowing its text.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(2), rows.max(1));
        if cols == self.cols() && rows == self.rows() {
            return;
        }
        self.term.resize(Size { cols, rows });
        self.schedule_search_refresh();
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

    /// Clear Terminal: the prompt row becomes the first row and everything else, scrollback
    /// included, is dropped.
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
        self.term.selection = None;
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

    /// The cursor appears after the first focus or keystroke.
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

    /// Scrollback lines above the viewport (the scrollbar position, in rows).
    pub fn scroll_position(&self) -> usize {
        self.term.history_size() - self.term.grid().display_offset()
    }

    pub fn total_rows(&self) -> usize {
        self.term.total_lines()
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

    pub fn scroll_to_bottom(&mut self) {
        if !self.at_bottom() {
            self.term.scroll_display(Scroll::Bottom);
            self.bump();
        }
    }

    /// User input scrolls to the prompt and clears the selection.
    pub fn user_input(&mut self) {
        self.scroll_to_bottom();
        if self.has_selection() {
            self.clear_selection();
        }
        self.show_cursor();
    }

    /// The grid point of 0-based viewport cell (`col`, `row`).
    pub fn viewport_point(&self, col: usize, row: usize) -> Point {
        let line = row.min(self.rows() - 1) as i32 - self.term.grid().display_offset() as i32;
        Point::new(Line(line), Column(col.min(self.cols() - 1)))
    }

    /// Viewport row text, right-trimmed.
    pub fn viewport_text(&self) -> Vec<String> {
        (0..self.rows())
            .map(|y| {
                let start = self.viewport_point(0, y);
                let end = Point::new(start.line, self.term.last_column());
                self.term
                    .bounds_to_string(start, end)
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    // ---- selection ----

    pub fn has_selection(&self) -> bool {
        self.term.selection.as_ref().is_some_and(|s| !s.is_empty())
    }

    pub fn selection_text(&self) -> String {
        self.term.selection_to_string().unwrap_or_default()
    }

    pub fn selection_dragging(&self) -> bool {
        self.dragging.is_some()
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
        self.bump();
    }

    fn select(
        &mut self,
        ty: SelectionType,
        start: Point,
        start_side: Side,
        end: Point,
        end_side: Side,
    ) {
        let mut selection = Selection::new(ty, start, start_side);
        selection.update(end, end_side);
        self.term.selection = Some(selection);
        self.bump();
    }

    /// Selects the link at `point`, or the word there.
    fn select_word(&mut self, point: Point) {
        match self.link_at(point) {
            Some(l) => self.select(
                SelectionType::Simple,
                l.start,
                Side::Left,
                l.end,
                Side::Right,
            ),
            None => self.select(
                SelectionType::Semantic,
                point,
                Side::Left,
                point,
                Side::Right,
            ),
        }
    }

    /// Primary-button press on 0-based viewport `cell`, on its `side`. Double-click selects a
    /// word (or link), triple-click a line; `column` starts a block selection.
    pub fn selection_mouse_down(
        &mut self,
        cell: (usize, usize),
        side: Side,
        click_count: usize,
        shift: bool,
        column: bool,
    ) {
        let point = self.viewport_point(cell.0, cell.1);
        self.dragging = Some(0);
        if shift && let Some(selection) = &mut self.term.selection {
            selection.update(point, side);
            self.bump();
            return;
        }
        match click_count {
            1 if column => self.select(SelectionType::Block, point, side, point, side),
            1 => self.select(SelectionType::Simple, point, side, point, side),
            2 => self.select_word(point),
            3 => self.select(SelectionType::Lines, point, side, point, side),
            _ => {}
        }
    }

    /// Drag with the button held. `offset_y` is the pointer's y relative to the grid top,
    /// `grid_height` the grid's pixel height (for drag-scrolling).
    pub fn selection_mouse_move(
        &mut self,
        cell: (usize, usize),
        side: Side,
        offset_y: f64,
        grid_height: f64,
    ) {
        self.dragging = Some(drag_scroll_amount(offset_y, grid_height));
        let point = self.viewport_point(cell.0, cell.1);
        if let Some(selection) = &mut self.term.selection {
            selection.update(point, side);
            self.bump();
        }
    }

    /// One drag-scroll tick (every [`DRAG_SCROLL_INTERVAL_MS`] while dragging).
    pub fn selection_drag_tick(&mut self) {
        let Some(amount) = self.dragging.filter(|&a| a != 0) else {
            return;
        };
        self.scroll_lines(amount);
        let (point, side) = if amount > 0 {
            (self.viewport_point(self.cols(), self.rows()), Side::Right)
        } else {
            (self.viewport_point(0, 0), Side::Left)
        };
        if let Some(selection) = &mut self.term.selection {
            selection.update(point, side);
        }
    }

    /// Button release on 0-based viewport `cell`. A quick Alt-click at the prompt returns the
    /// arrow keys that move the cursor there.
    pub fn selection_mouse_up(
        &mut self,
        elapsed_ms: u64,
        alt: bool,
        cell: (usize, usize),
    ) -> Option<String> {
        self.dragging = None;
        (alt && elapsed_ms < ALT_CLICK_MOVE_CURSOR_MS
            && self.at_bottom()
            && self.selection_text().chars().count() <= 1)
            .then(|| {
                move_to_cell::sequence(
                    self.term.grid().cursor.point,
                    self.viewport_point(cell.0, cell.1),
                    self.cols(),
                    self.alt_screen(),
                    self.app_cursor(),
                )
            })
    }

    /// Right-click selects the word (or link) under the pointer, unless it is already selected.
    pub fn right_click_select(&mut self, cell: (usize, usize)) {
        let point = self.viewport_point(cell.0, cell.1);
        let range = self
            .term
            .selection
            .as_ref()
            .and_then(|s| s.to_range(&self.term));
        if !range.is_some_and(|r| r.contains(point)) {
            self.select_word(point);
        }
    }

    // ---- search ----

    /// Selects the next (`forward`) or previous match and scrolls it into view.
    pub fn find(
        &mut self,
        query: &str,
        opts: &SearchOptions,
        forward: bool,
    ) -> Result<SearchResults, InvalidRegex> {
        if query.is_empty() {
            self.clear_search();
            self.clear_selection();
            return Ok(SearchResults::default());
        }
        if self
            .search
            .as_ref()
            .is_none_or(|s| s.query != query || s.opts != *opts)
        {
            self.search = None;
            self.search = Some(Search::new(query, opts)?);
        }
        let term = &self.term;
        let range = term.selection.as_ref().and_then(|s| s.to_range(term));
        let (origin, direction, side) = match range {
            Some(r) if opts.incremental => (r.start, Direction::Right, Side::Left),
            Some(r) if forward => (
                r.start.add(term, Boundary::None, 1),
                Direction::Right,
                Side::Left,
            ),
            Some(r) => (
                r.end.sub(term, Boundary::None, 1),
                Direction::Left,
                Side::Right,
            ),
            None if forward => (
                Point::new(term.topmost_line(), Column(0)),
                Direction::Right,
                Side::Left,
            ),
            None => (
                Point::new(term.bottommost_line(), term.last_column()),
                Direction::Left,
                Side::Right,
            ),
        };
        let regex = &mut self.search.as_mut().expect("compiled above").regex;
        match term.search_next(regex, origin, direction, side, None) {
            Some(m) => {
                self.select(
                    SelectionType::Simple,
                    *m.start(),
                    Side::Left,
                    *m.end(),
                    Side::Right,
                );
                self.term.scroll_to_point(*m.start());
            }
            None => self.clear_selection(),
        }
        Ok(self.search_results())
    }

    /// The match count and which one is selected.
    fn search_results(&mut self) -> SearchResults {
        let Some(search) = &mut self.search else {
            return SearchResults::default();
        };
        let term = &self.term;
        let selected = term.selection.as_ref().and_then(|s| s.to_range(term));
        let start = Point::new(term.topmost_line(), Column(0));
        let end = Point::new(term.bottommost_line(), term.last_column());
        let mut results = SearchResults::default();
        for m in RegexIter::new(start, end, Direction::Right, term, &mut search.regex)
            .take(HIGHLIGHT_LIMIT)
        {
            results.count += 1;
            if selected
                .as_ref()
                .is_some_and(|r| r.start == *m.start() && r.end == *m.end())
            {
                results.index = results.count;
            }
        }
        results
    }

    /// The matches on screen, for highlighting.
    fn visible_matches(&mut self) -> Vec<Match> {
        let Some(search) = &mut self.search else {
            return Vec::new();
        };
        let term = &self.term;
        let top = Line(-(term.grid().display_offset() as i32));
        let start = term.line_search_left(Point::new(top, Column(0)));
        let bottom = Point::new(top + (term.screen_lines() - 1), term.last_column());
        let end = term.line_search_right(bottom);
        RegexIter::new(start, end, Direction::Right, term, &mut search.regex).collect()
    }

    pub fn clear_search(&mut self) {
        self.search = None;
        self.search_refresh_due = None;
        self.bump();
    }

    /// When output or a resize changes the text under an open search, the count is redone a
    /// moment later.
    pub fn search_refresh_due(&self) -> Option<Instant> {
        self.search_refresh_due
    }

    fn schedule_search_refresh(&mut self) {
        if self.search.is_some() {
            self.search_refresh_due = Some(Instant::now() + SEARCH_REFRESH);
        }
    }

    pub fn run_search_refresh(&mut self) -> Option<SearchResults> {
        self.search_refresh_due = None;
        self.search.is_some().then(|| self.search_results())
    }

    // ---- links ----

    /// The link covering grid `point` (see [`Self::viewport_point`]), if the point still exists.
    pub fn link_at(&mut self, point: Point) -> Option<Link> {
        let term = &self.term;
        if point.line < term.topmost_line()
            || point.line > term.bottommost_line()
            || point.column > term.last_column()
        {
            return None;
        }
        let urls = self
            .urls
            .get_or_insert_with(|| RegexSearch::new(links::URL).expect("valid URL pattern"));
        links::link_at(&self.term, urls, point)
    }

    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
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

    /// The renderable frame: resolved colours, glyphs and cursor. `focused` picks
    /// active/inactive selection and cursor styles; `cursor_blink_on` is the blink phase.
    pub fn frame(&mut self, focused: bool, cursor_blink_on: bool) -> Frame {
        let matches = self.visible_matches();
        render::frame(self, &matches, focused, cursor_blink_on)
    }
}

/// Lines to drag-scroll per tick: `offset` is the pointer's y relative to the grid top,
/// `height` the grid's height.
fn drag_scroll_amount(offset: f64, height: f64) -> i64 {
    const MAX_THRESHOLD: f64 = 50.0;
    const MAX_SPEED: f64 = 15.0;
    if (0.0..=height).contains(&offset) {
        return 0;
    }
    let past = if offset > height {
        offset - height
    } else {
        offset
    };
    let t = past.clamp(-MAX_THRESHOLD, MAX_THRESHOLD) / MAX_THRESHOLD;
    (t.signum() + (t * (MAX_SPEED - 1.0)).round()) as i64
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
