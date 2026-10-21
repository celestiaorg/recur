//! [`TerminalView`]: one terminal surface bound to one PTY session (`TerminalPane.svelte`).
//!
//! Mounting attaches and repaints from the replay the session kept while nothing listened;
//! disposing detaches without killing the shell. Output is parsed as it arrives and acked in
//! 32KB batches once parsed, which is what releases the session's flow control.

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
    MouseReport, MouseReporter, cell_coords, evaluate_key, paste_text,
};
use recur_term::screen::{Link, LinkKind, MouseUp, Output, Palette, Screen, SearchOptions};

use crate::backend::TerminalBackend;
use crate::blink::CursorBlink;
use crate::element::TerminalElement;
use crate::font::{CellMetrics, TERM_FIT_SCROLLBAR_WIDTH, TerminalFont, term_fit_size};
use crate::park::Park;
use crate::scrollbar::{PIXELS_PER_LINE, Scrollbar};
use crate::theme::{ThemeContext, search_decorations};

gpui_kit::actions!(terminal, [Copy, Paste]);

/// Key context of a focused terminal (for app keymaps).
pub const KEY_CONTEXT: &str = "Terminal";
/// Batches acks so a fast stream doesn't make a call per chunk; well under the session's 256KB
/// high-water mark.
pub const ACK_THRESHOLD: usize = 32 * 1024;
/// `pushResize` debounce: every SIGWINCH makes full-screen programs repaint.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(100);
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

    /// The scrollbar column: 14px at the content box's right edge, grid height.
    pub fn scrollbar_bounds(&self) -> Bounds<Pixels> {
        Bounds::new(
            Point::new(
                self.content.right() - px(TERM_FIT_SCROLLBAR_WIDTH),
                self.origin.y,
            ),
            gpui_kit::size(px(TERM_FIT_SCROLLBAR_WIDTH), px(self.grid_height())),
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

    /// 1-based cell under `p` (xterm `getCoords`).
    pub fn cell(&self, p: Point<Pixels>, selection: bool) -> (i64, i64) {
        let (x, y) = self.local(p);
        cell_coords(
            x as f64,
            y as f64,
            self.cell_w() as f64,
            self.cell_h() as f64,
            self.cols,
            self.rows,
            selection,
        )
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
    resize_task: Option<Task<()>>,
    attach_task: Option<Task<()>>,
    tick_task: Option<(Instant, Task<()>)>,
    drag_task: Option<Task<()>>,
    pub(crate) blink: Option<CursorBlink>,
    pub(crate) scrollbar: Scrollbar,
    /// `Viewport._latestYDisp`: the row the scrollbar last scrolled to.
    latest_ydisp: Option<usize>,
    pub(crate) hovered_link: Option<Link>,
    mouse_down_link: Option<Link>,
    mouse_down_at: Instant,
    /// A mouse button reported to the app while held (for drag / release reports).
    reported_button: Option<TermButton>,
    reporter: MouseReporter,
    pub(crate) slider_hovered: bool,
    pub(crate) mouse_position: Option<Point<Pixels>>,
    pub(crate) composition: Option<Composition>,
    last_find_key: Option<(bool, bool, bool)>,
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
            resize_task: None,
            attach_task: None,
            tick_task: None,
            drag_task: None,
            blink: None,
            scrollbar: Scrollbar::default(),
            latest_ydisp: None,
            hovered_link: None,
            mouse_down_link: None,
            mouse_down_at: Instant::now(),
            reported_button: None,
            reporter: MouseReporter::default(),
            slider_hovered: false,
            mouse_position: None,
            composition: None,
            last_find_key: None,
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
        if let Ok(bytes) = replay
            && !bytes.is_empty()
        {
            self.feed(&bytes, cx);
        }
        self.replayed = true;
        for chunk in std::mem::take(&mut self.queued) {
            self.feed(&chunk, cx);
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
        self.feed(bytes, cx);
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

    fn feed(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        let cursor = self.screen.cursor_in_viewport();
        for out in self.screen.feed(bytes) {
            self.handle_output(out, cx);
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
        let now = Instant::now();
        let (_, blinks) = self.screen.cursor_style();
        match (&mut self.blink, blinks) {
            (None, true) => self.blink = Some(CursorBlink::new(now, self.focused)),
            (Some(_), false) => self.blink = None,
            (Some(b), true) if self.screen.cursor_in_viewport() != cursor_before => b.restart(now),
            _ => {}
        }
        if let Some(link) = &self.hovered_link
            && self
                .screen
                .link_at(
                    link.start.0,
                    link.start.1 - self.screen.viewport_top() as i64,
                )
                .as_ref()
                != Some(link)
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
        // A collapsed or hidden container measures 0x0; fitting to that would shrink the PTY.
        if w > 0.0 && h > 0.0 {
            let (cols, rows) = term_fit_size(
                w,
                h,
                metrics.css_cell_width(),
                metrics.css_cell_height(),
                TERM_FIT_SCROLLBAR_WIDTH,
            );
            if (cols, rows) != self.size() {
                self.screen.resize_and_refresh(cols, rows);
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
        };
        self.layout = Some(layout);
        if let Some(replay) = self.pending_replay.take() {
            self.replayed(replay, cx);
        }
        if self.latest_ydisp == Some(self.screen.viewport_top()) {
            self.scrollbar.set_dimensions(
                layout.grid_height(),
                layout.cell_h() * self.screen.total_rows() as f32,
            );
        } else {
            self.sync_scrollbar();
        }
        layout
    }

    /// `Viewport._sync`: scrollbar dimensions and position from the buffer.
    fn sync_scrollbar(&mut self) {
        let Some(l) = self.layout else { return };
        let ydisp = self.screen.viewport_top();
        self.scrollbar.sync(
            l.grid_height(),
            l.cell_h() * self.screen.total_rows() as f32,
            ydisp,
            l.cell_h(),
        );
        self.latest_ydisp = Some(ydisp);
    }

    // ---- timers ----

    fn next_deadline(&self, now: Instant) -> Option<Instant> {
        [
            self.blink
                .as_ref()
                .and_then(CursorBlink::next_deadline)
                .filter(|_| self.focused && self.active),
            self.scrollbar.next_deadline(now),
            self.screen.search_refresh_due(),
            self.screen.sync_deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn schedule_tick(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let Some(deadline) = self.next_deadline(now) else {
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
        let now = Instant::now();
        if let Some(b) = &mut self.blink {
            b.tick(now);
        }
        self.scrollbar.tick(now);
        if self.screen.sync_deadline().is_some_and(|d| d <= now) {
            self.screen.flush_sync();
        }
        if self.screen.search_refresh_due().is_some_and(|d| d <= now)
            && let Some(res) = self.screen.run_search_refresh()
        {
            self.emit_find_results(res, cx);
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
        let now = Instant::now();
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
        for out in self.screen.set_palette(palette) {
            self.handle_output(out, cx);
        }
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
        self.sync_scrollbar();
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
            decorations: Some(search_decorations(&recur_ui::Theme::of(cx).tokens)),
        };
        // The addon never re-highlights the same term when only options change.
        let key = (o.case_sensitive, o.regex, o.whole_word);
        if self.last_find_key != Some(key) {
            self.screen.clear_search();
        }
        self.last_find_key = Some(key);
        let res = self.screen.find(query, &o, forward);
        let found = matches!(res, Ok((true, _)));
        self.emit_find_results(res, cx);
        self.sync_scrollbar();
        self.schedule_tick(cx);
        cx.notify();
        found
    }

    fn emit_find_results(
        &mut self,
        res: Result<
            (bool, Option<recur_term::screen::SearchResults>),
            recur_term::screen::InvalidRegex,
        >,
        cx: &mut Context<Self>,
    ) {
        match res {
            Ok((_, Some(r))) => {
                let index = if r.index >= 0 {
                    r.index as usize + 1
                } else {
                    usize::from(r.count > 0)
                };
                cx.emit(TerminalEvent::FindResults {
                    index,
                    total: r.count,
                    valid: true,
                });
            }
            Ok((_, None)) => {}
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
                self.sync_scrollbar();
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
            x: x.floor() as i64,
            y: y.floor() as i64,
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
        let now = Instant::now();
        if l.scrollbar_bounds().contains(&pos)
            && self.scrollbar.is_needed()
            && button == MouseButton::Left
        {
            let offset = f32::from(pos.y - l.origin.y);
            if let Some(top) = self.scrollbar.pointer_down(offset, now) {
                self.scroll_to_px(top);
            }
            self.schedule_tick(cx);
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
        let sel = l.cell(pos, true);
        let cell = l.cell(pos, false);
        let link_cell = (cell.0, cell.1 + self.screen.viewport_top() as i64);
        match button {
            MouseButton::Left if !self.mouse_events_active() || force => {
                let shift = !self.mouse_events_active() && m.shift;
                self.screen.selection_mouse_down(
                    sel,
                    link_cell,
                    click_count,
                    shift,
                    self.should_column_select(m.alt),
                );
                self.start_drag_scroll(cx);
                self.write_primary_selection(cx);
            }
            MouseButton::Right if self.is_mac => {
                self.screen.right_click_select(sel, link_cell);
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
        let now = Instant::now();
        self.mouse_position = Some(pos);
        if self.scrollbar.is_dragging() {
            if let Some(top) = self
                .scrollbar
                .pointer_move(f32::from(pos.y - l.origin.y), now)
            {
                self.scroll_to_px(top);
            }
            cx.notify();
            return;
        }
        let over = hovered && f32::from(pos.y - l.origin.y) >= 0.0;
        self.scrollbar.set_mouse_over(over, now);
        let slider_hover = hovered
            && l.scrollbar_bounds().contains(&pos)
            && self.scrollbar.slider().is_some_and(|s| {
                let y = f32::from(pos.y - l.origin.y);
                y >= s.position && y <= s.position + s.size
            });
        let mut dirty = slider_hover != self.slider_hovered;
        self.slider_hovered = slider_hover;
        if self.screen.selection_dragging() {
            let offset = f32::from(pos.y - l.origin.y) as f64;
            self.screen
                .selection_mouse_move(l.cell(pos, true), offset, l.grid_height() as f64);
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
        let link = if hovered && l.contains_grid(pos) && !l.scrollbar_bounds().contains(&pos) {
            let (x, y) = l.cell(pos, false);
            self.screen.link_at(x, y)
        } else {
            None
        };
        if link != self.hovered_link {
            self.hovered_link = link;
            dirty = true;
        }
        self.schedule_tick(cx);
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
        let now = Instant::now();
        if self.scrollbar.is_dragging() {
            self.scrollbar.pointer_up(now);
            self.schedule_tick(cx);
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
            if let MouseUp::MoveCursor(seq) =
                self.screen
                    .selection_mouse_up(elapsed, m.alt, l.cell(pos, false))
            {
                self.send_input(seq, cx);
            }
        }
        // Linkifier: a click that starts and ends on the same link activates it.
        if let (Some(down), Some(cur)) = (self.mouse_down_link.take(), self.hovered_link.clone())
            && down == cur
            && l.contains_grid(pos)
        {
            let (x, y) = l.cell(pos, false);
            if self.screen.link_at(x, y).as_ref() == Some(&cur) {
                self.activate_link(&cur, cx);
            }
        }
        cx.notify();
    }

    fn activate_link(&mut self, link: &Link, cx: &mut Context<Self>) {
        match link.kind {
            // openExternalUrl: absolute http(s) only, in the OS browser.
            LinkKind::Web => {
                if is_external_http_url(&link.text) {
                    cx.open_url(link.text.trim());
                }
            }
            // xterm's default OSC 8 handler asked `confirm()`, which Recur's webview never showed,
            // so these links never opened.
            LinkKind::Hyperlink => {}
        }
    }

    pub(crate) fn scroll_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let Some(l) = self.layout else { return };
        let now = Instant::now();
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
        if self.latest_ydisp != Some(self.screen.viewport_top()) {
            self.sync_scrollbar();
        }
        if let Some(top) = self.scrollbar.wheel(dy, pixels, m.alt, now) {
            self.scroll_to_px(top);
            cx.stop_propagation();
        }
        self.schedule_tick(cx);
        cx.notify();
    }

    /// `Viewport._handleScroll`: whole rows from a pixel scroll position.
    fn scroll_to_px(&mut self, top: f32) {
        let Some(l) = self.layout else { return };
        let row = (top / l.cell_h()).round().max(0.0) as usize;
        let diff = row as i64 - self.screen.viewport_top() as i64;
        if diff != 0 {
            self.screen.scroll_lines(diff);
        }
        self.latest_ydisp = Some(self.screen.viewport_top());
    }

    pub(crate) fn mouse_exited(&mut self, cx: &mut Context<Self>) {
        self.mouse_position = None;
        self.scrollbar.set_mouse_over(false, Instant::now());
        if self.hovered_link.take().is_some() || self.slider_hovered {
            self.slider_hovered = false;
        }
        self.schedule_tick(cx);
        cx.notify();
    }

    /// Restart the cursor blink on any mouse press in the window (xterm listens on the document).
    pub(crate) fn window_mouse_down(&mut self) {
        if let Some(b) = &mut self.blink {
            b.restart(Instant::now());
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
                        let top = v.screen.viewport_top();
                        v.screen.selection_drag_tick();
                        if v.screen.viewport_top() != top {
                            v.sync_scrollbar();
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

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_focus(window, cx);
        let pane = div()
            .id(gpui_kit::ElementId::Name(
                format!("term-pane-{}", self.session_id).into(),
            ))
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste_action))
            .on_drop(cx.listener(|v, drag: &WorkspaceFileDrag, window, cx| {
                v.drop_paths(&drag.0, window, cx)
            }))
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

impl gpui_kit::EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        adjusted: &mut Option<std::ops::Range<usize>>,
        _w: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let text: Vec<u16> = self.composition.as_ref()?.text.encode_utf16().collect();
        let range = range.start.min(text.len())..range.end.min(text.len());
        *adjusted = Some(range.clone());
        Some(String::from_utf16_lossy(&text[range]))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled: bool,
        _w: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let len = self
            .composition
            .as_ref()
            .map_or(0, |c| c.text.encode_utf16().count());
        Some(UTF16Selection {
            range: len..len,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _w: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        self.composition
            .as_ref()
            .map(|c| 0..c.text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _w: &mut Window, cx: &mut Context<Self>) {
        self.unmark(cx);
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_text(text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        new_text: &str,
        _sel: Option<std::ops::Range<usize>>,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_marked_text(new_text, cx);
    }

    fn bounds_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        _element: Bounds<Pixels>,
        _w: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.cursor_bounds()
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _w: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
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
