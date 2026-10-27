//! `QuickOpen.svelte`: Go to file (⌘P, ranked by the backend file index, `path:line`) and Go
//! to symbol in the open file (⌘⇧O), as an overlay over the Observatory.

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement,
    IntoElement, ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Task, Window, deferred, div, px,
};
use recur_domain::OutlineSymbol;
use recur_services::AppEvent;
use recur_ui::guard::Guarded;
use recur_ui::logic::hover_gate::HoverGate;
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{Theme, rgba};
use recur_view_core::AppState;
use recur_view_review::style::hsla;
use recur_view_widgets::file_index::{FileSearchHit, map_file_search_hits};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QuickOpenMode {
    #[default]
    File,
    Symbol,
}

/// What a pick asks the Observatory to do (`onPickFile`, `onPickSymbol`).
#[derive(Clone, Debug, PartialEq)]
pub enum QuickOpenEvent {
    PickFile { path: String, line: Option<usize> },
    PickSymbol { name: String, line: usize },
    Closed,
}

/// One row: a file path or a symbol.
#[derive(Clone, Debug, PartialEq)]
enum Row {
    File(FileSearchHit),
    Symbol(OutlineSymbol),
}

pub struct QuickOpen {
    state: Entity<AppState>,
    open: bool,
    mode: QuickOpenMode,
    project_id: String,
    workspace_id: String,
    symbols: Vec<OutlineSymbol>,
    input: Entity<InputState>,
    sel: usize,
    file_hits: Vec<FileSearchHit>,
    file_loading: bool,
    search: Option<Task<()>>,
    search_gen: u64,
    hover: HoverGate,
    list: ScrollHandle,
    reveal_sel: bool,
    _events: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<QuickOpenEvent> for QuickOpen {}

/// `pathQuery` / `parseLine`: `path:12` splits off the line.
pub fn split_line(q: &str) -> (&str, Option<usize>) {
    if let Some(i) = q.rfind(':')
        && i > 0
    {
        let tail = &q[i + 1..];
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
            return (&q[..i], tail.parse().ok());
        }
    }
    (q, None)
}

impl QuickOpen {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        recur_ui::style::style_text_input(&input, cx);
        let subs = vec![
            cx.guarded_subscribe_in(&input, window, |q, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    q.sel = 0;
                    q.query_changed(cx);
                }
            }),
        ];
        let mut events = state.read(cx).services().subscribe();
        let ev_task = cx.spawn(async move |this, cx| {
            while let Some(ev) = events.recv().await {
                let AppEvent::WorkspaceFileIndex(f) = ev else {
                    continue;
                };
                let ok = this.update(cx, |q, cx| {
                    if q.open && f.workspace_id == q.workspace_id {
                        q.query_changed(cx);
                    }
                });
                if ok.is_err() {
                    break;
                }
            }
        });
        QuickOpen {
            state,
            open: false,
            mode: QuickOpenMode::File,
            project_id: String::new(),
            workspace_id: String::new(),
            symbols: Vec::new(),
            input,
            sel: 0,
            file_hits: Vec::new(),
            file_loading: false,
            search: None,
            search_gen: 0,
            hover: HoverGate::default(),
            list: ScrollHandle::new(),
            reveal_sel: false,
            _events: Some(ev_task),
            _subs: subs,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn mode(&self) -> QuickOpenMode {
        self.mode
    }

    /// The host's props (`projectId`, `workspaceId`, `symbols`).
    pub fn set_props(
        &mut self,
        project_id: &str,
        workspace_id: &str,
        symbols: &[OutlineSymbol],
        cx: &mut Context<Self>,
    ) {
        if self.project_id == project_id
            && self.workspace_id == workspace_id
            && self.symbols == symbols
        {
            return;
        }
        self.project_id = project_id.into();
        self.workspace_id = workspace_id.into();
        self.symbols = symbols.to_vec();
        cx.notify();
    }

    /// `open = true` with a mode: empty query, first row, focus.
    pub fn show(&mut self, mode: QuickOpenMode, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = mode;
        self.open = true;
        self.sel = 0;
        self.hover.reset();
        self.input.update(cx, |s, cx| s.set_value("", window, cx));
        let input = self.input.clone();
        window.defer(cx, move |window, cx| {
            input.update(cx, |s, cx| s.focus(window, cx))
        });
        self.state
            .update(cx, |s, cx| s.set_chat_occluder("quick-open", true, cx));
        if mode == QuickOpenMode::File {
            self.query_changed(cx);
        } else {
            self.file_hits.clear();
        }
        cx.notify();
    }

    /// Keys are in the overlay's field.
    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.open
            && gpui_kit::Focusable::focus_handle(self.input.read(cx), cx)
                .contains_focused(window, cx)
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.state
            .update(cx, |s, cx| s.set_chat_occluder("quick-open", false, cx));
        cx.emit(QuickOpenEvent::Closed);
        cx.notify();
    }

    fn query(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// The file search for the current query (newest wins).
    fn query_changed(&mut self, cx: &mut Context<Self>) {
        self.hover.reset();
        if !self.open
            || self.mode != QuickOpenMode::File
            || self.project_id.is_empty()
            || self.workspace_id.is_empty()
        {
            if self.mode != QuickOpenMode::File {
                self.file_hits.clear();
            }
            cx.notify();
            return;
        }
        let q = self.query(cx);
        let path_q = split_line(&q).0.to_string();
        self.search_gen += 1;
        let token = self.search_gen;
        self.file_loading = self.file_hits.is_empty();
        let svc = self.state.read(cx).services().clone();
        let (p, w) = (self.project_id.clone(), self.workspace_id.clone());
        self.search = Some(cx.spawn(async move |this, cx| {
            let res = svc.code.search_files(&p, &w, &path_q, 50, true).await;
            let _ = this.update(cx, |q, cx| {
                if token != q.search_gen {
                    return;
                }
                match res {
                    Ok(r) => {
                        let hits = map_file_search_hits(r.hits.as_deref().unwrap_or_default());
                        let any = !hits.is_empty();
                        q.file_hits = hits.into_iter().filter(|h| !h.dir).collect();
                        q.file_loading = r.building && !any;
                    }
                    Err(err) => {
                        recur_view_core::toast(
                            recur_ui::widgets::ToastKind::Error,
                            format!(
                                "Search files failed: {}",
                                recur_view_core::bridge_error_message(&err)
                            ),
                            cx,
                        );
                        q.file_hits.clear();
                        q.file_loading = false;
                    }
                }
                q.clamp_sel();
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn rows(&self, cx: &App) -> Vec<Row> {
        match self.mode {
            QuickOpenMode::Symbol => {
                let q = self.query(cx);
                let syms: Vec<recur_textmate::OutlineSymbol> = self
                    .symbols
                    .iter()
                    .map(|s| recur_textmate::OutlineSymbol {
                        name: s.name.clone(),
                        kind: s.kind.clone(),
                        line: s.line.max(0) as usize,
                        end: (s.end > 0).then_some(s.end as usize),
                    })
                    .collect();
                recur_textmate::filter_symbols(&syms, &q, 40, |labels, q, limit| {
                    let prepared = recur_ui::logic::fuzzy::prepare_fuzzy(
                        &(0..labels.len()).collect::<Vec<_>>(),
                        |i| labels[*i].clone(),
                    );
                    recur_ui::logic::fuzzy::fuzzy_filter_prepared(&prepared, q, limit)
                        .into_iter()
                        .map(|h| (h.item, h.score))
                        .collect()
                })
                .into_iter()
                .map(|h| {
                    Row::Symbol(OutlineSymbol {
                        name: h.item.name.clone(),
                        kind: h.item.kind.clone(),
                        line: h.item.line as i64,
                        end: h.item.end.unwrap_or(0) as i64,
                    })
                })
                .collect()
            }
            QuickOpenMode::File => self.file_hits.iter().cloned().map(Row::File).collect(),
        }
    }

    fn clamp_sel(&mut self) {
        let n = match self.mode {
            QuickOpenMode::File => self.file_hits.len(),
            QuickOpenMode::Symbol => usize::MAX,
        };
        if n != usize::MAX && self.sel >= n {
            self.sel = n.saturating_sub(1);
        }
    }

    fn run(&mut self, i: usize, cx: &mut Context<Self>) {
        let rows = self.rows(cx);
        let Some(row) = rows.get(i).cloned() else {
            return;
        };
        let q = self.query(cx);
        self.close(cx);
        match row {
            Row::Symbol(s) => cx.emit(QuickOpenEvent::PickSymbol {
                name: s.name,
                line: s.line.max(0) as usize,
            }),
            Row::File(h) => cx.emit(QuickOpenEvent::PickFile {
                path: h.item,
                line: split_line(&q).1,
            }),
        }
    }

    fn on_key(&mut self, e: &gpui_kit::KeyDownEvent, cx: &mut Context<Self>) {
        let m = e.keystroke.modifiers;
        if m.platform || m.control || m.alt {
            return;
        }
        let n = self.rows(cx).len();
        match e.keystroke.key.as_str() {
            "down" => {
                self.hover.reset();
                self.sel = (self.sel + 1).min(n.saturating_sub(1));
                self.reveal_sel = true;
            }
            "up" => {
                self.hover.reset();
                self.sel = self.sel.saturating_sub(1);
                self.reveal_sel = true;
            }
            "enter" => {
                let i = self.sel;
                self.run(i, cx);
                cx.stop_propagation();
                return;
            }
            "escape" => {
                self.close(cx);
                cx.stop_propagation();
                return;
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl QuickOpen {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        if !self.open {
            return div().into_any_element();
        }
        let t = Theme::of(cx).tokens;
        let rows = self.rows(cx);
        if self.sel >= rows.len() {
            self.sel = rows.len().saturating_sub(1);
        }
        if self.reveal_sel {
            self.reveal_sel = false;
            self.list.scroll_to_item(self.sel);
        }
        let placeholder = match self.mode {
            QuickOpenMode::Symbol => "Go to symbol in file…",
            QuickOpenMode::File => "Go to file…",
        };
        let q = self.query(cx);
        let field = recur_ui::style::text_field(
            &self.input,
            &SharedString::from(placeholder),
            recur_ui::style::UA_PLACEHOLDER,
            recur_ui::fonts::Family::Mono,
            13.,
            window,
            cx,
        );
        let input_row = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(10.))
            .border_b_1()
            .border_color(hsla(t.line))
            .child(div().flex_1().min_w_0().child(field))
            .child(text("Esc").size(13.).nowrap());
        let empty_label = if !q.trim().is_empty() {
            "No matches"
        } else if self.mode == QuickOpenMode::Symbol {
            "No symbols in this file"
        } else {
            "Type to filter files"
        };
        let loading = self.mode == QuickOpenMode::File && self.file_loading && rows.is_empty();
        let mut list = div().flex().flex_col();
        if loading {
            list = list.child(qo_empty("Loading files…", &t));
        } else if rows.is_empty() {
            list = list.child(qo_empty(empty_label, &t));
        } else {
            for (i, r) in rows.iter().enumerate() {
                let on = i == self.sel;
                let mut row = div()
                    .id(SharedString::from(format!("qo-{i}")))
                    .debug_selector(move || format!("qo-row:{i}"))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .w_full()
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_color(hsla(t.text))
                    .when(on, |d| d.bg(hsla(t.amber_d)))
                    .on_mouse_move(cx.guarded_listener(
                        move |q, e: &gpui_kit::MouseMoveEvent, _, cx| {
                            if q.hover
                                .allows(f32::from(e.position.x), f32::from(e.position.y))
                                && q.sel != i
                            {
                                q.sel = i;
                                cx.notify();
                            }
                        },
                    ))
                    .on_click(cx.guarded_listener(move |q, _, _, cx| q.run(i, cx)));
                row = match r {
                    Row::Symbol(s) => row
                        .child(
                            div().w(px(52.)).flex_none().child(
                                text(s.kind.clone())
                                    .mono()
                                    .size(10.)
                                    .line_height(LineHeight::Normal)
                                    .uppercase()
                                    .color(t.faint)
                                    .nowrap(),
                            ),
                        )
                        .child(
                            div().flex_1().min_w_0().child(
                                text(s.name.clone())
                                    .mono()
                                    .size(12.)
                                    .line_height(LineHeight::Normal)
                                    .truncate(),
                            ),
                        )
                        .child(
                            text(s.line.to_string())
                                .mono()
                                .size(11.)
                                .line_height(LineHeight::Normal)
                                .color(t.faint)
                                .nowrap(),
                        ),
                    Row::File(h) => row.child(
                        div().flex_1().min_w_0().child(
                            text(h.item.clone())
                                .mono()
                                .size(12.)
                                .line_height(LineHeight::Normal)
                                .truncate(),
                        ),
                    ),
                };
                list = list.child(row);
            }
        }
        let panel = div()
            .debug_selector(|| "quick-open".into())
            .w_full()
            .flex()
            .flex_col()
            .child(input_row)
            .child(
                div().max_h(px(360.)).child(
                    recur_ui::widgets::scroll_area("qo-list")
                        .track(&self.list)
                        .child(div().p(px(4.)).child(list)),
                ),
            );
        let card = recur_ui::style::card(t.pane, t.line_bright, 10.)
            .overflow_hidden()
            .child(panel);
        let card = with_shadow(
            card,
            &[css_box_shadow(0., 18., 48., 0., rgba(0, 0, 0, 0.35))],
            10.,
        );
        let vh = f32::from(window.viewport_size().height);
        deferred(
            div()
                .id("qo-back")
                .absolute()
                .inset_0()
                .bg(hsla(t.bg.fade(0.55)))
                .flex()
                .flex_col()
                .items_center()
                .child(recur_ui::exact::vgap(vh * 0.12, window))
                .child(
                    div()
                        .id("qo")
                        .w(px(560.))
                        .max_w(gpui_kit::relative(0.92))
                        .occlude()
                        .on_key_down(cx.guarded_listener(|q, e: &gpui_kit::KeyDownEvent, _, cx| {
                            q.on_key(e, cx)
                        }))
                        .child(card),
                )
                .on_mouse_down(
                    gpui_kit::MouseButton::Left,
                    cx.guarded_listener(|q, _, _, cx| q.close(cx)),
                ),
        )
        .with_priority(80)
        .into_any_element()
    }
}

impl Render for QuickOpen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

fn qo_empty(s: &'static str, t: &recur_ui::Tokens) -> AnyElement {
    div()
        .p(px(16.))
        .flex()
        .justify_center()
        .child(text(s).size(12.).color(t.faint))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_suffix_splits_off() {
        assert_eq!(split_line("src/a.go:12"), ("src/a.go", Some(12)));
        assert_eq!(split_line("src/a.go:"), ("src/a.go:", None));
        assert_eq!(split_line(":12"), (":12", None));
        assert_eq!(split_line("a:b"), ("a:b", None));
    }
}
