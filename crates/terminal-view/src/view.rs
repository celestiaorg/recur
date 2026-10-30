//! [`TerminalView`]: one terminal surface bound to one PTY session.
//!
//! Mounting attaches and repaints from the replay the session kept while nothing listened;
//! disposing detaches without killing the shell. Output is parsed as it arrives and acked in
//! 32KB batches once parsed, which is what releases the session's flow control.

use recur_ui::guard::Guarded;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc;
use gpui_kit::{
    App, Bounds, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, Global,
    InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, MouseButton, ParentElement, Pixels,
    Point, Render, ScrollDelta, ScrollWheelEvent, Styled, Subscription, Task, UTF16Selection,
    WeakEntity, Window, div, px,
};
use recur_term::input::{
    Encoded, KeyAction, KeyPress, MouseAction, MouseButton as TermButton, MouseProtocol,
    MouseReport, MouseReporter, evaluate_key, paste_text,
};
use recur_term::screen::{
    InvalidRegex, Link, Output, Palette, Screen, SearchOptions, SearchResults, Side,
};
use recur_ui::widgets::scrollbar::{
    OVERLAY_BOX, Overlay, classic_thumb, overlay_knob, overlay_scroll,
};

use crate::backend::TerminalBackend;
use crate::blink::CursorBlink;
use crate::element::TerminalElement;
use crate::font::{CellMetrics, TerminalFont, term_fit_size};
use crate::park::Park;
use crate::theme::{ThemeContext, search_decorations};

gpui_kit::actions!(terminal, [Copy, Paste]);

/// Key context of a focused terminal (for app keymaps).
pub const KEY_CONTEXT: &str = "Terminal";
/// Batches acks so a fast stream doesn't make a call per chunk; well under the session's 256KB
/// high-water mark.
pub const ACK_THRESHOLD: usize = 32 * 1024;
/// `pushResize` debounce: every SIGWINCH makes full-screen programs repaint.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(100);
/// WebKit's `Scrollbar::pixelsPerLineStep`: line wheel deltas reported to apps as pixels.
const PIXELS_PER_LINE: f32 = 40.0;
/// The pane's `.term-pane` padding: 6px top and bottom, 8px left.
pub const PANE_PADDING_TOP: f32 = 6.0;
pub const PANE_PADDING_BOTTOM: f32 = 6.0;
pub const PANE_PADDING_LEFT: f32 = 8.0;

/// Registers the terminal key bindings and globals. Call once at startup.
pub fn init(cx: &mut App) {
    // macOS routes ⌘C / ⌘V through the Edit menu to the terminal's copy and paste handlers.
    // Elsewhere the dock owns Ctrl+Shift+C / V and calls `selection_text` / `paste`.
    if cfg!(target_os = "macos") {
        cx.bind_keys([
            KeyBinding::new("cmd-c", Copy, Some(KEY_CONTEXT)),
            KeyBinding::new("cmd-v", Paste, Some(KEY_CONTEXT)),
        ]);
    }
    cx.set_global(Router::default());
    cx.set_global(TerminalPark::default());
}

/// Routes session events to the view showing each session (`wireTerminalEvents`).
#[derive(Default)]
pub struct Router {
    views: HashMap<String, WeakEntity<TerminalView>>,
}

impl Global for Router {}

/// Delivers a session event (`term.data`, `term.exit`) to its view. The app's event bridge calls
/// this for every [`recur_term::Event`]; busy/idle are for the dock and are ignored here.
pub fn route_event(event: &recur_term::Event, cx: &mut App) {
    let id = event.session_id().to_string();
    let Some(view) = cx
        .try_global::<Router>()
        .and_then(|r| r.views.get(&id))
        .and_then(WeakEntity::upgrade)
    else {
        return;
    };
    match event {
        recur_term::Event::Data { data, .. } => view.update(cx, |v, cx| v.push_output(data, cx)),
        recur_term::Event::Exit { exit_code, .. } => {
            let code = *exit_code;
            view.update(cx, |v, cx| v.session_exited(code, cx));
            // A parked pane has no listeners left: release it or it lives until evicted.
            cx.defer(move |cx| {
                if let Some(parked) = take_parked(&id, cx) {
                    parked.update(cx, |v, cx| v.dispose(cx));
                }
            });
        }
        recur_term::Event::Busy { .. } | recur_term::Event::Idle { .. } => {}
    }
}

/// Views kept alive across dock unmounts (see [`crate::park`]).
#[derive(Default)]
pub struct TerminalPark(pub Park<Entity<TerminalView>>);

impl Global for TerminalPark {}

fn park_mut(cx: &mut App) -> &mut Park<Entity<TerminalView>> {
    &mut cx.default_global::<TerminalPark>().0
}

/// Parks `view` (keeping it attached), disposing any views evicted past the budget.
pub fn park(view: Entity<TerminalView>, cx: &mut App) {
    let (id, ws) = {
        let v = view.read(cx);
        (v.session_id.clone(), v.workspace_id.clone())
    };
    let evicted = park_mut(cx).stash(&id, view, &ws);
    for (_, v) in evicted {
        v.update(cx, |v, cx| v.dispose(cx));
    }
}

/// Takes a parked view back for `session_id`, if any.
pub fn take_parked(session_id: &str, cx: &mut App) -> Option<Entity<TerminalView>> {
    park_mut(cx).take(session_id)
}

/// Marks a session for real disposal (close / kill) and disposes its parked view if any.
pub fn discard_parked(session_id: &str, cx: &mut App) {
    if let Some(v) = park_mut(cx).discard(session_id) {
        v.update(cx, |v, cx| v.dispose(cx));
    }
}

/// Whether the unmounting pane must dispose instead of park (consumes the flag).
pub fn should_dispose(session_id: &str, cx: &mut App) -> bool {
    park_mut(cx).should_dispose(session_id)
}

/// Disposes parked views whose workspace is gone.
pub fn prune_parked(keep_workspace_ids: &HashSet<String>, cx: &mut App) {
    let dropped = park_mut(cx).prune(keep_workspace_ids);
    for (_, v) in dropped {
        v.update(cx, |v, cx| v.dispose(cx));
    }
}

/// What a pane reports to its dock (`onTitle`, `onExit`, `onFindResults`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    /// OSC 0 / 2 title.
    Title(String),
    /// The shell exited with this status.
    Exit(i32),
    /// Find results: 1-based active index (0 when none), total highlighted, and whether the query
    /// was valid.
    FindResults {
        index: usize,
        total: usize,
        valid: bool,
    },
}

/// Find options from the dock's find bar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FindOptions {
    pub case_sensitive: bool,
    pub regexp: bool,
    pub whole_word: bool,
    pub incremental: bool,
}

/// Paths dragged from the workspace explorer (`application/x-recur-workspace-file`).
/// simplify: lives here until the explorer port defines the shared drag payload in recur-ui.
#[derive(Debug, Clone)]
pub struct WorkspaceFileDrag(pub Vec<String>);

/// How a pane is mounted.
#[derive(Clone)]
pub struct TerminalOptions {
    pub session_id: String,
    pub workspace_id: String,
    /// Hidden panes stay mounted (and attached) so background tabs keep their screen.
    pub active: bool,
    /// Transparent cells, for the maximized dock on a dark palette.
    pub frosted: bool,
    pub font: TerminalFont,
    pub theme: ThemeContext,
    /// The workspace worktree, for resolving dropped explorer paths.
    pub workspace_root: String,
    /// The dock's focus handle: a pane takes keys on mount or activation only when the dock
    /// already owns them (`termDockOwnsFocus`).
    pub dock_focus: Option<FocusHandle>,
}

/// Grid placement from the last layout, window coordinates.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GridLayout {
    pub content: Bounds<Pixels>,
    pub origin: Point<Pixels>,
    pub metrics: CellMetrics,
    pub cols: usize,
    pub rows: usize,
    /// "Show scroll bars: Always": the classic bar in its own column instead of the overlay.
    pub legacy: bool,
}

impl GridLayout {
    pub fn cell_w(&self) -> f32 {
        self.metrics.css_cell_width()
    }

    pub fn cell_h(&self) -> f32 {
        self.metrics.css_cell_height()
    }

    pub fn grid_height(&self) -> f32 {
        (self.rows as f32 * self.metrics.cell_height / self.metrics.dpr).round()
    }

    /// The scroller's box at the content box's right edge, grid height.
    pub fn bar_bounds(&self) -> Bounds<Pixels> {
        let w = if self.legacy {
            recur_ui::theme::SCROLLBAR_SIZE
        } else {
            px(OVERLAY_BOX)
        };
        Bounds::new(
            Point::new(self.content.right() - w, self.origin.y),
            gpui_kit::size(w, px(self.grid_height())),
        )
    }

    fn local(&self, p: Point<Pixels>) -> (f32, f32) {
        (
            f32::from(p.x - self.origin.x),
            f32::from(p.y - self.origin.y),
        )
    }

    pub fn contains_grid(&self, p: Point<Pixels>) -> bool {
        let (x, y) = self.local(p);
        x >= 0.0 && y >= 0.0 && x < f32::from(self.content.size.width) && y < self.grid_height()
    }

    /// 0-based viewport cell under `p`, clamped to the grid.
    pub fn cell(&self, p: Point<Pixels>) -> (usize, usize) {
        let (x, y) = self.local(p);
        let at = |v: f32, size: f32, n: usize| ((v / size).max(0.0) as usize).min(n.max(1) - 1);
        (
            at(x, self.cell_w(), self.cols),
            at(y, self.cell_h(), self.rows),
        )
    }

    /// Which half of its cell `p` is over, for selection.
    pub fn side(&self, p: Point<Pixels>) -> Side {
        let x = self.local(p).0 / self.cell_w();
        if x >= self.cols as f32 || (x > 0.0 && x.fract() >= 0.5) {
            Side::Right
        } else {
            Side::Left
        }
    }
}

/// An IME composition in progress (marked text).
#[derive(Debug, Clone, Default)]
pub(crate) struct Composition {
    pub text: String,
}

/// One terminal surface bound to one session.
pub struct TerminalView {
    pub(crate) session_id: String,
    pub(crate) workspace_id: String,
    backend: Arc<dyn TerminalBackend>,
    pub(crate) screen: Screen,
    writer: Option<mpsc::UnboundedSender<Vec<u8>>>,
    pub(crate) focus: FocusHandle,
    pub(crate) focused: bool,
    pub(crate) active: bool,
    pub(crate) frosted: bool,
    pub(crate) font: TerminalFont,
    pub(crate) theme: ThemeContext,
    workspace_root: String,
    dock_focus: Option<FocusHandle>,
    pub(crate) layout: Option<GridLayout>,
    replayed: bool,
    /// A replay that arrived before the first layout: xterm.js refits before attaching, so the
    /// replay lands on the fitted grid.
    pending_replay: Option<Result<Vec<u8>, String>>,
    queued: Vec<Vec<u8>>,
    unacked: usize,
    disposed: bool,
    last_size: (usize, usize),
    size_held: bool,
    resize_task: Option<Task<()>>,
    attach_task: Option<Task<()>>,
    tick_task: Option<(Instant, Task<()>)>,
    drag_task: Option<Task<()>>,
    pub(crate) blink: Option<CursorBlink>,
    pub(crate) bar: Overlay,
    /// Dragging the scrollbar knob: the pointer's offset into it.
    pub(crate) knob_grab: Option<f32>,
    pub(crate) knob_hovered: bool,
    /// Wheel rows not yet scrolled, so precise deltas add up.
    wheel_rows: f32,
    pub(crate) hovered_link: Option<Link>,
    mouse_down_link: Option<Link>,
    mouse_down_at: Instant,
    /// A mouse button reported to the app while held (for drag / release reports).
    reported_button: Option<TermButton>,
    reporter: MouseReporter,
    pub(crate) mouse_position: Option<Point<Pixels>>,
    pub(crate) composition: Option<Composition>,
    is_mac: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TerminalView {
    pub fn new(
        opts: TerminalOptions,
        backend: Arc<dyn TerminalBackend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let subscriptions = vec![
            cx.on_focus(&focus, window, |v, window, cx| v.sync_focus(window, cx)),
            cx.on_blur(&focus, window, |v, window, cx| v.sync_focus(window, cx)),
            cx.observe_window_activation(window, |v, window, cx| v.sync_focus(window, cx)),
            cx.observe_global::<recur_ui::Theme>(|v, cx| v.push_theme(cx)),
        ];
        let (tx, mut rx) = mpsc::unbounded::<Vec<u8>>();
        let (write_backend, write_id) = (backend.clone(), opts.session_id.clone());
        // Writes stay in order on one background task; a PTY write can block when the shell is
        // not reading.
        cx.background_executor()
            .spawn(async move {
                while let Some(bytes) = rx.next().await {
                    write_backend.write(&write_id, &bytes);
                }
            })
            .detach();
        let me = cx.weak_entity();
        cx.default_global::<Router>()
            .views
            .insert(opts.session_id.clone(), me);

        let mut view = Self {
            session_id: opts.session_id,
            workspace_id: opts.workspace_id,
            backend,
            screen: Screen::new(80, 24),
            writer: Some(tx),
            focus,
            focused: false,
            active: opts.active,
            frosted: opts.frosted,
            font: opts.font,
            theme: opts.theme,
            workspace_root: opts.workspace_root,
            dock_focus: opts.dock_focus,
            layout: None,
            replayed: false,
            pending_replay: None,
            queued: Vec::new(),
            unacked: 0,
            disposed: false,
            last_size: (0, 0),
            size_held: false,
            resize_task: None,
            attach_task: None,
            tick_task: None,
            drag_task: None,
            blink: None,
            bar: Overlay::default(),
            knob_grab: None,
            knob_hovered: false,
            wheel_rows: 0.0,
            hovered_link: None,
            mouse_down_link: None,
            mouse_down_at: cx.background_executor().now(),
            reported_button: None,
            reporter: MouseReporter::default(),
            mouse_position: None,
            composition: None,
            is_mac: cfg!(target_os = "macos"),
            _subscriptions: subscriptions,
        };
        view.push_theme(cx);
        view.attach(cx);
        if view.active {
            cx.defer_in(window, |v, window, cx| {
                v.try_focus_if_dock_owns_keys(window, cx)
            });
        }
        view
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    /// Grid size `(cols, rows)`.
    pub fn size(&self) -> (usize, usize) {
        (self.screen.cols(), self.screen.rows())
    }

    /// The grid's top-left in window coordinates, after the first layout.
    pub fn grid_origin(&self) -> Option<Point<Pixels>> {
        self.layout.map(|l| l.origin)
    }

    /// Cell size in CSS pixels, after the first layout.
    pub fn cell_size(&self) -> Option<(f32, f32)> {
        self.layout.map(|l| (l.cell_w(), l.cell_h()))
    }

    pub fn is_disposed(&self) -> bool {
        self.disposed
    }

    /// Marks the macOS input conventions (Option composes, ⌥-click forces selection, right-click
    /// selects a word). Defaults to the build target.
    pub fn set_mac(&mut self, mac: bool) {
        self.is_mac = mac;
    }

    fn attach(&mut self, cx: &mut Context<Self>) {
        let (backend, id) = (self.backend.clone(), self.session_id.clone());
        self.attach_task = Some(cx.spawn(async move |this, cx| {
            let replay = cx
                .background_executor()
                .spawn(async move { backend.attach(&id) })
                .await;
            let _ = this.update(cx, |v, cx| v.replayed(replay, cx));
        }));
    }

    /// Chunks can land between subscribing and the replay arriving, so they wait until the
    /// replay is on screen. The replay itself is not acked: the session never counted it.
    fn replayed(&mut self, replay: Result<Vec<u8>, String>, cx: &mut Context<Self>) {
        if self.disposed {
            return;
        }
        if self.layout.is_none() {
            self.pending_replay = Some(replay);
            cx.notify();
            return;
        }
        // The replay's queries were asked of an earlier screen; answering them now would type
        // stale replies into the shell.
        if let Ok(bytes) = replay
            && !bytes.is_empty()
        {
            self.feed(&bytes, false, cx);
        }
        self.replayed = true;
        for chunk in std::mem::take(&mut self.queued) {
            self.feed(&chunk, true, cx);
            self.count_ack(chunk.len(), cx);
        }
        // The PTY was opened at a guessed size; push the measured one now.
        let size = self.size();
        self.push_resize(size, cx);
        cx.notify();
    }

    /// A `term.data` chunk.
    pub fn push_output(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        if self.disposed {
            return;
        }
        if !self.replayed {
            self.queued.push(bytes.to_vec());
            return;
        }
        self.feed(bytes, true, cx);
        self.count_ack(bytes.len(), cx);
    }

    fn count_ack(&mut self, n: usize, cx: &mut Context<Self>) {
        self.unacked += n;
        if self.unacked < ACK_THRESHOLD {
            return;
        }
        let n = std::mem::take(&mut self.unacked);
        let (backend, id) = (self.backend.clone(), self.session_id.clone());
        cx.background_executor()
            .spawn(async move { backend.ack(&id, n) })
            .detach();
    }

    fn feed(&mut self, bytes: &[u8], reply: bool, cx: &mut Context<Self>) {
        let cursor = self.screen.cursor_in_viewport();
        for out in self.screen.feed(bytes) {
            if reply || !matches!(out, Output::Reply(_)) {
                self.handle_output(out, cx);
            }
        }
        self.after_change(cursor, cx);
    }

    fn handle_output(&mut self, out: Output, cx: &mut Context<Self>) {
        match out {
            Output::Title(t) => cx.emit(TerminalEvent::Title(t)),
            Output::Reply(r) => self.write(r.into_bytes()),
            // xterm.js 6 has no bell sound or visual bell, and the pane did not handle `onBell`.
            Output::Bell => {}
        }
    }

    /// Cursor blink and timers after the screen changed.
    fn after_change(&mut self, cursor_before: Option<(usize, usize)>, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        match (&mut self.blink, self.screen.cursor_style().blinking) {
            (None, true) => self.blink = Some(CursorBlink::new(now, self.focused)),
            (Some(_), false) => self.blink = None,
            (Some(b), true) if self.screen.cursor_in_viewport() != cursor_before => b.restart(now),
            _ => {}
        }
        if let Some(link) = self.hovered_link.clone()
            && self.screen.link_at(link.start) != Some(link)
        {
            self.hovered_link = None;
        }
        self.schedule_tick(cx);
        cx.notify();
    }

    fn write(&self, bytes: Vec<u8>) {
        if let Some(w) = &self.writer {
            let _ = w.unbounded_send(bytes);
        }
    }

    /// User input (`triggerDataEvent(data, true)`): scrolls to the prompt, clears the selection.
    fn send_input(&mut self, data: String, cx: &mut Context<Self>) {
        let cursor = self.screen.cursor_in_viewport();
        self.screen.user_input();
        self.write(data.into_bytes());
        self.after_change(cursor, cx);
    }

    /// The shell exited; the dock relabels the tab and keeps it readable.
    pub fn session_exited(&mut self, code: i32, cx: &mut Context<Self>) {
        cx.emit(TerminalEvent::Exit(code));
    }

    /// `pushResize`: tells the PTY its geometry, debounced and deduplicated.
    fn push_resize(&mut self, size: (usize, usize), cx: &mut Context<Self>) {
        if size == self.last_size {
            return;
        }
        self.last_size = size;
        let (backend, id) = (self.backend.clone(), self.session_id.clone());
        self.resize_task = Some(cx.spawn(async move |_, cx| {
            cx.background_executor().timer(RESIZE_DEBOUNCE).await;
            cx.background_executor()
                .spawn(async move { backend.resize(&id, size.0 as u16, size.1 as u16) })
                .await;
        }));
    }

    /// Keep the grid at its size while the pane's box animates; release refits it.
    pub fn hold_size(&mut self, hold: bool, cx: &mut Context<Self>) {
        if self.size_held != hold {
            self.size_held = hold;
            cx.notify();
        }
    }

    /// Layout: refits the grid to the content box (`refit`) and pins it to the bottom, so leftover
    /// sub-row pixels sit at the top and the last row stays fully visible.
    pub(crate) fn fit(
        &mut self,
        content: Bounds<Pixels>,
        metrics: CellMetrics,
        cx: &mut Context<Self>,
    ) -> GridLayout {
        let (w, h) = (
            f32::from(content.size.width),
            f32::from(content.size.height),
        );
        let legacy = recur_ui::widgets::scrollbar::legacy(cx);
        // A collapsed or hidden container measures 0x0; fitting to that would shrink the PTY.
        if w > 0.0 && h > 0.0 && !self.size_held {
            let (cols, rows) = term_fit_size(
                w,
                h,
                metrics.css_cell_width(),
                metrics.css_cell_height(),
                recur_ui::widgets::scrollbar::layout_size(cx),
            );
            if (cols, rows) != self.size() {
                self.screen.resize(cols, rows);
                self.reporter.reset();
                self.schedule_tick(cx);
                if self.replayed {
                    self.push_resize((cols, rows), cx);
                }
                cx.notify();
            }
        }
        let (cols, rows) = self.size();
        let grid_h = (rows as f32 * metrics.cell_height / metrics.dpr).round();
        let origin = Point::new(content.left(), content.bottom() - px(grid_h));
        let layout = GridLayout {
            content,
            origin,
            metrics,
            cols,
            rows,
            legacy,
        };
        self.layout = Some(layout);
        if let Some(replay) = self.pending_replay.take() {
            self.replayed(replay, cx);
        }
        layout
    }

    /// The scrollbar knob as (length, offset from the grid top) in pixels, while there is
    /// scrollback.
    pub(crate) fn knob(&self, l: &GridLayout) -> Option<(f32, f32)> {
        let (pos, total, visible) = self.bar_rows(l);
        let knob = if l.legacy {
            classic_thumb(pos, total, visible, l.grid_height())
        } else {
            overlay_knob(pos, total, visible, l.grid_height())
        };
        (total > visible && knob.0 > 0.0).then_some(knob)
    }

    /// The scroll position, content and viewport, in rows.
    fn bar_rows(&self, l: &GridLayout) -> (f32, f32, f32) {
        (
            self.screen.scroll_position() as f32,
            self.screen.total_rows() as f32,
            l.rows as f32,
        )
    }

    /// The scrollbar under `p`: whether it is on the knob. The overlay takes the pointer only
    /// while it shows.
    fn bar_hit(&self, l: &GridLayout, p: Point<Pixels>, now: Instant) -> Option<bool> {
        let (len, at) = self.knob(l)?;
        if !l.bar_bounds().contains(&p) || !(l.legacy || self.bar.visible(now)) {
            return None;
        }
        let y = f32::from(p.y - l.origin.y);
        Some(y >= at && y < at + len)
    }

    /// Scrolls so the knob starts at `at` px from the grid top.
    fn drag_knob(&mut self, l: &GridLayout, at: f32) {
        let (pos, total, visible) = self.bar_rows(l);
        let row = if l.legacy {
            let (len, _) = classic_thumb(0.0, total, visible, l.grid_height());
            (at / (l.grid_height() - len).max(1.0)).clamp(0.0, 1.0) * (total - visible)
        } else {
            overlay_scroll(at, total, visible, l.grid_height())
        };
        self.screen.scroll_lines(row.round() as i64 - pos as i64);
    }

    // ---- timers ----

    fn next_deadline(&self) -> Option<Instant> {
        [
            self.blink
                .as_ref()
                .and_then(CursorBlink::next_deadline)
                .filter(|_| self.focused && self.active),
            self.screen.search_refresh_due(),
            self.screen.sync_deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn schedule_tick(&mut self, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        let Some(deadline) = self.next_deadline() else {
            self.tick_task = None;
            return;
        };
        if self.tick_task.as_ref().is_some_and(|(d, _)| *d == deadline) {
            return;
        }
        let delay = deadline.saturating_duration_since(now);
        self.tick_task = Some((
            deadline,
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
                let _ = this.update(cx, |v, cx| v.tick(cx));
            }),
        ));
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        self.tick_task = None;
        let now = cx.background_executor().now();
        if let Some(b) = &mut self.blink {
            b.tick(now);
        }
        if self.screen.sync_deadline().is_some_and(|d| d <= now) {
            self.screen.flush_sync();
        }
        if self.screen.search_refresh_due().is_some_and(|d| d <= now)
            && let Some(res) = self.screen.run_search_refresh()
        {
            self.emit_find_results(Ok(res), cx);
        }
        self.schedule_tick(cx);
        cx.notify();
    }

    // ---- focus ----

    fn sync_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.focus.is_focused(window) && window.is_window_active();
        if focused == self.focused {
            return;
        }
        self.focused = focused;
        let now = cx.background_executor().now();
        if focused {
            self.screen.show_cursor();
            if let Some(b) = &mut self.blink {
                b.resume(now);
            }
        } else if let Some(b) = &mut self.blink {
            b.pause();
        }
        if let Some(r) = self.screen.focus_report(focused) {
            self.write(r.as_bytes().to_vec());
        }
        self.schedule_tick(cx);
        cx.notify();
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    /// Takes keys only when the dock already owns them, so a remount after a workspace switch
    /// doesn't steal focus from the editor or agent.
    pub fn try_focus_if_dock_owns_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active
            && let Some(dock) = &self.dock_focus
            && dock.contains_focused(window, cx)
        {
            window.focus(&self.focus, cx);
        }
    }

    // ---- props ----

    /// Becoming visible after a tab switch re-measures, then takes keys if the dock owns them.
    pub fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            cx.defer_in(window, |v, window, cx| {
                v.try_focus_if_dock_owns_keys(window, cx)
            });
        }
        cx.notify();
    }

    pub fn set_frosted(&mut self, frosted: bool, cx: &mut Context<Self>) {
        if self.frosted != frosted {
            self.frosted = frosted;
            self.push_theme(cx);
        }
    }

    /// Live-follows font changes from Settings without a remount.
    pub fn set_font(&mut self, font: TerminalFont, cx: &mut Context<Self>) {
        if self.font != font {
            self.font = font;
            cx.notify();
        }
    }

    /// Terminal theme or catalog changed (app theme switches arrive through `recur_ui::Theme`).
    pub fn set_theme(&mut self, theme: ThemeContext, cx: &mut Context<Self>) {
        if self.theme != theme {
            self.theme = theme;
            self.push_theme(cx);
        }
    }

    pub fn set_workspace_root(&mut self, root: String) {
        self.workspace_root = root;
    }

    pub fn set_dock_focus(&mut self, dock: Option<FocusHandle>) {
        self.dock_focus = dock;
    }

    fn push_theme(&mut self, cx: &mut Context<Self>) {
        let app = recur_ui::Theme::of(cx).id;
        let palette = Palette::from_theme(&self.theme.resolve(app, self.frosted));
        self.screen.set_palette(palette);
        cx.notify();
    }

    // ---- pane API used by the dock ----

    /// Selected text (the dock's Ctrl+Shift+C off macOS).
    pub fn selection_text(&self) -> String {
        self.screen.selection_text()
    }

    /// Paste as the user typing it (bracketed when the shell asks).
    pub fn paste(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = paste_text(text, self.screen.bracketed_paste());
        self.send_input(text, cx);
    }

    /// Clears the screen and the session's replay, so re-attaching stays clear.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.screen.clear();
        let (backend, id) = (self.backend.clone(), self.session_id.clone());
        cx.background_executor()
            .spawn(async move { backend.clear_replay(&id) })
            .detach();
        cx.notify();
    }

    pub fn find_next(&mut self, query: &str, opts: FindOptions, cx: &mut Context<Self>) -> bool {
        self.run_find(query, opts, true, cx)
    }

    pub fn find_previous(
        &mut self,
        query: &str,
        opts: FindOptions,
        cx: &mut Context<Self>,
    ) -> bool {
        self.run_find(query, opts, false, cx)
    }

    pub fn clear_search(&mut self, cx: &mut Context<Self>) {
        self.screen.clear_search();
        cx.notify();
    }

    fn run_find(
        &mut self,
        query: &str,
        opts: FindOptions,
        forward: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let o = SearchOptions {
            case_sensitive: opts.case_sensitive,
            regex: opts.regexp,
            whole_word: opts.whole_word,
            incremental: opts.incremental,
            decorations: search_decorations(&recur_ui::Theme::of(cx).tokens),
        };
        let res = self.screen.find(query, &o, forward);
        let found = matches!(res, Ok(r) if r.count > 0);
        self.emit_find_results(res, cx);
        self.schedule_tick(cx);
        cx.notify();
        found
    }

    fn emit_find_results(
        &mut self,
        res: Result<SearchResults, InvalidRegex>,
        cx: &mut Context<Self>,
    ) {
        match res {
            Ok(r) => cx.emit(TerminalEvent::FindResults {
                index: r.index.max(usize::from(r.count > 0)),
                total: r.count,
                valid: true,
            }),
            Err(_) => {
                self.screen.clear_search();
                cx.emit(TerminalEvent::FindResults {
                    index: 0,
                    total: 0,
                    valid: false,
                });
            }
        }
    }

    /// Writes dropped explorer paths, quoted for the shell, then takes focus.
    pub fn drop_paths(&mut self, paths: &[String], window: &mut Window, cx: &mut Context<Self>) {
        let text = terminal_drop_text(paths, &self.workspace_root);
        if text.is_empty() {
            return;
        }
        self.write(text.into_bytes());
        window.focus(&self.focus, cx);
    }

    /// Detaches without killing the shell and stops routing output here (`killWidget`).
    pub fn dispose(&mut self, cx: &mut Context<Self>) {
        if self.disposed {
            return;
        }
        self.disposed = true;
        self.writer = None;
        self.tick_task = None;
        self.drag_task = None;
        let me = cx.weak_entity();
        if let Some(r) = cx
            .try_global::<Router>()
            .map(|r| r.views.get(&self.session_id).is_some_and(|w| w == &me))
            && r
        {
            cx.global_mut::<Router>().views.remove(&self.session_id);
        }
        let (backend, id) = (self.backend.clone(), self.session_id.clone());
        cx.background_executor()
            .spawn(async move { backend.detach(&id) })
            .detach();
    }

    // ---- keyboard ----

    pub(crate) fn key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.composition.is_some() {
            return;
        }
        let k = &event.keystroke;
        let press = KeyPress {
            key: &k.key,
            key_char: k.key_char.as_deref(),
            ctrl: k.modifiers.control,
            alt: k.modifiers.alt,
            shift: k.modifiers.shift,
            meta: k.modifiers.platform,
        };
        match evaluate_key(&press, self.screen.app_cursor(), self.is_mac, false) {
            KeyAction::Send(s) => {
                cx.stop_propagation();
                self.send_input(s, cx);
            }
            action @ (KeyAction::PageUp | KeyAction::PageDown) => {
                cx.stop_propagation();
                let page = self.screen.rows() as i64 - 1;
                self.screen.scroll_lines(if action == KeyAction::PageUp {
                    -page
                } else {
                    page
                });
                cx.notify();
            }
            KeyAction::Text | KeyAction::Ignore => {}
        }
    }

    pub(crate) fn copy(&mut self, _: &Copy, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.screen.has_selection() {
            cx.propagate();
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(self.screen.selection_text()));
    }

    pub(crate) fn paste_action(&mut self, _: &Paste, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            self.paste(&text, cx);
        }
    }

    // ---- IME ----

    pub(crate) fn commit_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.composition = None;
        if !text.is_empty() {
            self.send_input(text.to_string(), cx);
        }
        cx.notify();
    }

    pub(crate) fn set_marked_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.composition = (!text.is_empty()).then(|| Composition {
            text: text.to_string(),
        });
        cx.notify();
    }

    pub(crate) fn unmark(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.composition.take() {
            self.commit_text(&c.text, cx);
        }
    }

    /// The cursor cell in window coordinates, for placing the IME candidate window.
    pub(crate) fn cursor_bounds(&self) -> Option<Bounds<Pixels>> {
        let l = self.layout?;
        let (x, y) = self.screen.cursor_in_viewport()?;
        Some(Bounds::new(
            Point::new(
                l.origin.x + px(x as f32 * l.cell_w()),
                l.origin.y + px(y as f32 * l.cell_h()),
            ),
            gpui_kit::size(px(l.cell_w()), px(l.cell_h())),
        ))
    }

    // ---- mouse ----

    fn mouse_events_active(&self) -> bool {
        self.screen.mouse_events_active()
    }

    /// `shouldForceSelection`: ⌥ on macOS (`macOptionClickForcesSelection`), Shift elsewhere.
    fn should_force_selection(&self, alt: bool, shift: bool) -> bool {
        if self.is_mac { alt } else { shift }
    }

    /// `shouldColumnSelect`: Alt-drag column selection, except on macOS where ⌥ forces selection.
    pub(crate) fn should_column_select(&self, alt: bool) -> bool {
        alt && !self.is_mac
    }

    fn report(
        &mut self,
        pos: Point<Pixels>,
        button: TermButton,
        action: MouseAction,
        m: gpui_kit::Modifiers,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(l) = self.layout else { return false };
        let (x, y) = l.local(pos);
        let w = l.cols as f32 * l.cell_w();
        let h = l.grid_height();
        let (x, y) = (
            x.clamp(0.0, (w - 1.0).max(0.0)),
            y.clamp(0.0, (h - 1.0).max(0.0)),
        );
        let e = MouseReport {
            col: (x / l.cell_w()).floor() as i64,
            row: (y / l.cell_h()).floor() as i64,
            button,
            action,
            ctrl: m.control,
            alt: m.alt,
            shift: m.shift,
        };
        match self.reporter.report(
            e,
            l.cols,
            l.rows,
            self.screen.mouse_protocol(),
            self.screen.mouse_encoding(),
        ) {
            Some(Encoded::Data(s)) => {
                self.send_input(s, cx);
                true
            }
            // Recur's pane never subscribed to xterm's `onBinary`, so legacy reports were dropped.
            Some(Encoded::Binary(_)) => true,
            None => false,
        }
    }

    pub(crate) fn mouse_down(
        &mut self,
        pos: Point<Pixels>,
        button: MouseButton,
        click_count: usize,
        m: gpui_kit::Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(l) = self.layout else { return };
        let now = cx.background_executor().now();
        if button == MouseButton::Left
            && let Some(on_knob) = self.bar_hit(&l, pos, now)
        {
            self.bar.press(true, now);
            let (_, at) = self.knob(&l).unwrap_or_default();
            let y = f32::from(pos.y - l.origin.y);
            if on_knob {
                self.knob_grab = Some(y - at);
            } else {
                let page = l.rows as i64 - 1;
                self.screen.scroll_lines(if y < at { -page } else { page });
            }
            cx.notify();
            return;
        }
        window.focus(&self.focus, cx);
        self.mouse_down_at = now;
        let force = self.should_force_selection(m.alt, m.shift);
        let tb = match button {
            MouseButton::Left => TermButton::Left,
            MouseButton::Middle => TermButton::Middle,
            MouseButton::Right => TermButton::Right,
            _ => TermButton::None,
        };
        if self.mouse_events_active() && !force && self.report(pos, tb, MouseAction::Down, m, cx) {
            self.reported_button = Some(tb);
        }
        self.mouse_down_link = self.hovered_link.clone();
        let cell = l.cell(pos);
        match button {
            MouseButton::Left if !self.mouse_events_active() || force => {
                let shift = !self.mouse_events_active() && m.shift;
                self.screen.selection_mouse_down(
                    cell,
                    l.side(pos),
                    click_count,
                    shift,
                    self.should_column_select(m.alt),
                );
                self.start_drag_scroll(cx);
                self.write_primary_selection(cx);
            }
            MouseButton::Right if self.is_mac => {
                self.screen.right_click_select(cell);
                self.write_primary_selection(cx);
            }
            #[cfg(target_os = "linux")]
            MouseButton::Middle => {
                if let Some(text) = cx.read_from_primary().and_then(|c| c.text()) {
                    self.paste(&text, cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    pub(crate) fn mouse_move(
        &mut self,
        pos: Point<Pixels>,
        pressed: Option<MouseButton>,
        m: gpui_kit::Modifiers,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(l) = self.layout else { return };
        let now = cx.background_executor().now();
        self.mouse_position = Some(pos);
        if let Some(grab) = self.knob_grab {
            self.drag_knob(&l, f32::from(pos.y - l.origin.y) - grab);
            cx.notify();
            return;
        }
        if hovered {
            self.bar.pointer_moved(now);
        }
        let bar = hovered.then(|| self.bar_hit(&l, pos, now)).flatten();
        let knob = bar == Some(true);
        let mut dirty = self.bar.hover(bar.is_some(), now) || knob != self.knob_hovered;
        self.knob_hovered = knob;
        if self.screen.selection_dragging() {
            let offset = f32::from(pos.y - l.origin.y) as f64;
            self.screen.selection_mouse_move(
                l.cell(pos),
                l.side(pos),
                offset,
                l.grid_height() as f64,
            );
            self.write_primary_selection(cx);
            dirty = true;
        }
        if self.mouse_events_active() {
            let protocol = self.screen.mouse_protocol();
            match (self.reported_button, pressed) {
                (Some(b), Some(_)) if protocol.wants_drag() => {
                    self.report(pos, b, MouseAction::Move, m, cx);
                }
                (_, None) if protocol == MouseProtocol::Any && l.contains_grid(pos) => {
                    self.report(pos, TermButton::None, MouseAction::Move, m, cx);
                }
                _ => {}
            }
        }
        // Linkifier hover.
        let link = if hovered && l.contains_grid(pos) && bar.is_none() {
            let (x, y) = l.cell(pos);
            let point = self.screen.viewport_point(x, y);
            self.screen.link_at(point)
        } else {
            None
        };
        if link != self.hovered_link {
            self.hovered_link = link;
            dirty = true;
        }
        if dirty {
            cx.notify();
        }
    }

    pub(crate) fn mouse_up(
        &mut self,
        pos: Point<Pixels>,
        button: MouseButton,
        m: gpui_kit::Modifiers,
        cx: &mut Context<Self>,
    ) {
        let Some(l) = self.layout else { return };
        let now = cx.background_executor().now();
        self.bar.press(false, now);
        if self.knob_grab.take().is_some() {
            cx.notify();
            return;
        }
        if let Some(b) = self.reported_button.take()
            && self.screen.mouse_protocol().wants_up()
        {
            self.report(pos, b, MouseAction::Up, m, cx);
        }
        if button == MouseButton::Left && self.screen.selection_dragging() {
            self.drag_task = None;
            let elapsed = now
                .saturating_duration_since(self.mouse_down_at)
                .as_millis() as u64;
            if let Some(seq) = self.screen.selection_mouse_up(elapsed, m.alt, l.cell(pos)) {
                self.send_input(seq, cx);
            }
        }
        // Linkifier: a click that starts and ends on the same link activates it.
        if let (Some(down), Some(cur)) = (self.mouse_down_link.take(), self.hovered_link.clone())
            && down == cur
            && l.contains_grid(pos)
        {
            let (x, y) = l.cell(pos);
            let point = self.screen.viewport_point(x, y);
            // Absolute http(s) only, in the OS browser.
            if self.screen.link_at(point).as_ref() == Some(&cur) && is_external_http_url(&cur.text)
            {
                cx.open_url(cur.text.trim());
            }
        }
        cx.notify();
    }

    pub(crate) fn scroll_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let Some(l) = self.layout else { return };
        let now = cx.background_executor().now();
        let (dy, pixels) = match event.delta {
            ScrollDelta::Pixels(p) => (-f32::from(p.y), true),
            ScrollDelta::Lines(p) => (-p.y, false),
        };
        // DOM deltaY in pixels (WebKit turns wheel lines into 40px steps).
        let dom_px = if pixels { dy } else { dy * PIXELS_PER_LINE };
        let m = event.modifiers;
        if self.screen.mouse_protocol().wants_wheel() {
            let lines = self.reporter.consume_wheel(
                dom_px as f64,
                true,
                l.cell_h() as f64,
                l.rows,
                m.alt || m.control || m.shift,
                m.shift,
            );
            if lines != 0 {
                let action = if dom_px < 0.0 {
                    MouseAction::Up
                } else {
                    MouseAction::Down
                };
                self.report(event.position, TermButton::Wheel, action, m, cx);
            }
            cx.stop_propagation();
            return;
        }
        if self.screen.alt_screen() {
            // No scrollback: wheel becomes arrow keys so vim and less scroll anyway.
            let lines = self.reporter.consume_wheel(
                dom_px as f64,
                true,
                l.cell_h() as f64,
                l.rows,
                m.alt || m.control || m.shift,
                m.shift,
            );
            if lines != 0 {
                let seq = format!(
                    "\x1b{}{}",
                    if self.screen.app_cursor() { 'O' } else { '[' },
                    if dom_px < 0.0 { 'A' } else { 'B' }
                );
                self.send_input(seq, cx);
            }
            cx.stop_propagation();
            return;
        }
        let rows = if pixels { dy / l.cell_h() } else { dy };
        let fast = if m.alt {
            recur_term::input::FAST_SCROLL_SENSITIVITY as f32
        } else {
            1.0
        };
        self.wheel_rows += rows * fast;
        let lines = self.wheel_rows.trunc();
        self.wheel_rows -= lines;
        let before = self.screen.scroll_position();
        self.screen.scroll_lines(lines as i64);
        if self.screen.scroll_position() != before {
            self.bar.scrolled(now);
            cx.stop_propagation();
            cx.notify();
        }
    }

    pub(crate) fn mouse_exited(&mut self, cx: &mut Context<Self>) {
        self.mouse_position = None;
        self.bar.hover(false, cx.background_executor().now());
        self.hovered_link = None;
        self.knob_hovered = false;
        cx.notify();
    }

    /// Restart the cursor blink on any mouse press in the window (xterm listens on the document).
    pub(crate) fn window_mouse_down(&mut self, now: Instant) {
        if let Some(b) = &mut self.blink {
            b.restart(now);
        }
    }

    fn start_drag_scroll(&mut self, cx: &mut Context<Self>) {
        let interval = Duration::from_millis(recur_term::screen::DRAG_SCROLL_INTERVAL_MS);
        self.drag_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(interval).await;
                let go = this
                    .update(cx, |v, cx| {
                        if !v.screen.selection_dragging() {
                            return false;
                        }
                        let top = v.screen.scroll_position();
                        v.screen.selection_drag_tick();
                        if v.screen.scroll_position() != top {
                            v.write_primary_selection(cx);
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !go {
                    break;
                }
            }
        }));
    }

    /// Linux middle-click paste reads what the last mouse selection put on PRIMARY.
    fn write_primary_selection(&self, cx: &mut Context<Self>) {
        #[cfg(target_os = "linux")]
        if self.screen.has_selection() {
            cx.write_to_primary(ClipboardItem::new_string(self.screen.selection_text()));
        }
        #[cfg(not(target_os = "linux"))]
        let _ = cx;
    }
}

impl TerminalView {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        self.sync_focus(window, cx);
        let pane = div()
            .id(gpui_kit::ElementId::Name(
                format!("term-pane-{}", self.session_id).into(),
            ))
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .on_key_down(cx.guarded_listener(Self::key_down))
            .on_action(cx.guarded_listener(Self::copy))
            .on_action(cx.guarded_listener(Self::paste_action))
            .on_drop(
                cx.guarded_listener(|v, drag: &WorkspaceFileDrag, window, cx| {
                    v.drop_paths(&drag.0, window, cx)
                }),
            )
            .size_full()
            .min_h_0()
            .pt(px(PANE_PADDING_TOP))
            .pb(px(PANE_PADDING_BOTTOM))
            .pl(px(PANE_PADDING_LEFT))
            .flex()
            .flex_col()
            .justify_end()
            .child(TerminalElement::new(cx.entity()));
        // Hidden tabs stay mounted so their shells keep streaming into a live screen.
        if self.active {
            pane
        } else {
            pane.invisible().absolute().top_0().left_0()
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

impl gpui_kit::EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        adjusted: &mut Option<std::ops::Range<usize>>,
        _w: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        recur_ui::guard::catch("terminal text_for_range", || {
            let text: Vec<u16> = self.composition.as_ref()?.text.encode_utf16().collect();
            let end = range.end.min(text.len());
            let range = range.start.min(end)..end;
            *adjusted = Some(range.clone());
            Some(String::from_utf16_lossy(&text[range]))
        })
        .flatten()
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled: bool,
        _w: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        recur_ui::guard::catch("terminal selected_text_range", || {
            let len = self
                .composition
                .as_ref()
                .map_or(0, |c| c.text.encode_utf16().count());
            Some(UTF16Selection {
                range: len..len,
                reversed: false,
            })
        })
        .flatten()
    }

    fn marked_text_range(
        &self,
        _w: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        recur_ui::guard::catch("terminal marked_text_range", || {
            self.composition
                .as_ref()
                .map(|c| 0..c.text.encode_utf16().count())
        })
        .flatten()
    }

    fn unmark_text(&mut self, _w: &mut Window, cx: &mut Context<Self>) {
        recur_ui::guard::catch("terminal unmark_text", || {
            self.unmark(cx);
        });
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        recur_ui::guard::catch("terminal replace_text_in_range", || {
            self.commit_text(text, cx);
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        new_text: &str,
        _sel: Option<std::ops::Range<usize>>,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        recur_ui::guard::catch("terminal replace_and_mark_text_in_range", || {
            self.set_marked_text(new_text, cx);
        });
    }

    fn bounds_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        _element: Bounds<Pixels>,
        _w: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        recur_ui::guard::catch("terminal bounds_for_range", || self.cursor_bounds()).flatten()
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _w: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        recur_ui::guard::catch("terminal character_index_for_point", || None).flatten()
    }
}

/// `isExternalHttpUrl`: absolute http(s) only.
pub fn is_external_http_url(href: &str) -> bool {
    let raw = href.trim();
    !raw.is_empty()
        && url_scheme(raw)
            .is_some_and(|s| s.eq_ignore_ascii_case("http") || s.eq_ignore_ascii_case("https"))
}

fn url_scheme(s: &str) -> Option<&str> {
    let (scheme, rest) = s.split_once(':')?;
    (rest.starts_with("//")
        && !scheme.is_empty()
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)))
    .then_some(scheme)
}

/// `quoteForShell`: safe paths as-is, everything else single-quoted.
pub fn quote_for_shell(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    if path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_./:@%+=,-".contains(c))
    {
        return path.to_string();
    }
    format!("'{}'", path.replace('\'', "'\\''"))
}

fn is_abs_filesystem_path(path: &str) -> bool {
    let p = path.trim().replace('\\', "/");
    if p.is_empty() || p.starts_with("scratch://") {
        return false;
    }
    let b = p.as_bytes();
    p.starts_with('/')
        || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/')
}

/// `resolveExplorerDropPath`: workspace-relative explorer paths become absolute.
pub fn resolve_explorer_drop_path(rel: &str, root: &str) -> String {
    let p = rel.trim().replace('\\', "/");
    let p = p.trim_end_matches('/');
    if p.is_empty() {
        return String::new();
    }
    if is_abs_filesystem_path(p) {
        return p.to_string();
    }
    let r = root.trim().replace('\\', "/");
    let r = r.trim_end_matches('/');
    if r.is_empty() {
        return p.to_string();
    }
    format!("{r}/{}", p.trim_start_matches('/'))
}

/// `terminalDropText`: dropped paths, quoted and space-separated.
pub fn terminal_drop_text(paths: &[String], root: &str) -> String {
    paths
        .iter()
        .map(|p| {
            let p = if root.is_empty() {
                let t = p.trim_end_matches('/');
                if t.is_empty() {
                    p.clone()
                } else {
                    t.to_string()
                }
            } else {
                resolve_explorer_drop_path(p, root)
            };
            quote_for_shell(&p)
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The find count label (`previewFindCountLabel`): "No results", "3 / 12", or "1 / 999+".
pub fn find_count_label(index: usize, total: usize, capped: bool) -> String {
    if total == 0 {
        return "No results".into();
    }
    if capped {
        format!("{index} / {}+", recur_term::screen::HIGHLIGHT_LIMIT)
    } else {
        format!("{index} / {total}")
    }
}
