//! Top panels: Recur's find/replace panel (`cm-search-panel.ts`), the
//! go-to-line dialog (`cm-goto-line.ts`) and CodeMirror's stock dialogs the
//! LSP client opens (rename prompt, error messages), styled per
//! `_codemirror.scss`, `.obs-icon-btn` and CodeMirror's base theme.

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, BoxShadow, Context, Entity, Focusable, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled, Subscription,
    Window, div, linear_color_stop, linear_gradient, point, px,
};
use recur_ui::fonts::Family;
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::style::{button_label, card, text_field};
use recur_ui::tooltip::titled;
use recur_ui::{IconName, TextStyled, Theme, icon, rgba, text};

use crate::bindings::Cmd;
use crate::keymap::KeyEvent;
use crate::search::SearchPanel;
use crate::view::EditorView;

/// The body line height the panels inherit (`body { line-height: 1.45 }`).
const BODY_LEADING: f32 = 1.45;

fn h(c: impl Into<Hsla>) -> Hsla {
    c.into()
}

/// The find panel's fields and model.
pub(crate) struct SearchUi {
    pub model: SearchPanel,
    pub find: Entity<InputState>,
    pub replace: Entity<InputState>,
    _subs: Vec<Subscription>,
}

impl SearchUi {
    pub fn find_focused(&self, window: &Window, cx: &App) -> bool {
        self.find.read(cx).focus_handle(cx).is_focused(window)
    }

    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.find_focused(window, cx) || self.replace.read(cx).focus_handle(cx).is_focused(window)
    }

    /// Focus the find field and select its text (`mount` / `openSearchPanel`).
    pub fn focus_find(&self, window: &mut Window, cx: &mut App) {
        self.find.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
    }
}

/// The rename prompt's field.
pub(crate) struct RenamePrompt {
    pub input: Entity<InputState>,
    _subs: Vec<Subscription>,
}

/// A dialog panel (`showDialog`), newest first in the panel stack.
pub(crate) enum Dialog {
    /// `recurGotoLine`.
    Goto {
        id: u64,
        input: Entity<InputState>,
        _subs: Vec<Subscription>,
    },
    /// `reportError`.
    Message { id: u64, text: String },
}

impl Dialog {
    fn id(&self) -> u64 {
        match self {
            Dialog::Goto { id, .. } | Dialog::Message { id, .. } => *id,
        }
    }

    fn input(&self) -> Option<&Entity<InputState>> {
        match self {
            Dialog::Goto { input, .. } => Some(input),
            Dialog::Message { .. } => None,
        }
    }
}

impl EditorView {
    fn new_input(
        &self,
        value: &str,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let value = value.to_string();
        let placeholder = SharedString::from(placeholder.to_string());
        let input = recur_ui::style::text_input(&placeholder, window, cx);
        input.update(cx, |s, cx| s.set_value(value, window, cx));
        recur_ui::style::style_text_input(&input, cx);
        input
    }

    /// Create the find panel's UI once the panel opens; the find field is
    /// focused and selected.
    pub(crate) fn ensure_search_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_none() {
            let model = SearchPanel::new(&self.state);
            let find = self.new_input(&model.search, "Find", window, cx);
            let replace = self.new_input(&model.replace, "Replace", window, cx);
            let subs =
                vec![
                    cx.subscribe_in(
                        &find,
                        window,
                        |v, input, ev: &InputEvent, window, cx| match ev {
                            InputEvent::Change => {
                                let value = input.read(cx).value().to_string();
                                if let Some(s) = v.search.as_mut()
                                    && s.model.search != value
                                {
                                    s.model.search = value;
                                    v.commit_search(window, cx);
                                }
                            }
                            InputEvent::PressEnter { shift, .. } => {
                                v.run_cmd(
                                    if *shift {
                                        Cmd::FindPrevious
                                    } else {
                                        Cmd::FindNext
                                    },
                                    window,
                                    cx,
                                );
                            }
                            InputEvent::Focus | InputEvent::Blur => cx.notify(),
                        },
                    ),
                    cx.subscribe_in(&replace, window, |v, input, ev: &InputEvent, window, cx| {
                        match ev {
                            InputEvent::Change => {
                                let value = input.read(cx).value().to_string();
                                if let Some(s) = v.search.as_mut()
                                    && s.model.replace != value
                                {
                                    s.model.replace = value;
                                    v.commit_search(window, cx);
                                }
                            }
                            InputEvent::PressEnter { .. } => {
                                let state = v.state.clone();
                                let run = crate::search::replace_next(&state);
                                v.run_search_run(run, window, cx);
                            }
                            InputEvent::Focus | InputEvent::Blur => cx.notify(),
                        }
                    }),
                ];
            self.search = Some(SearchUi {
                model,
                find,
                replace,
                _subs: subs,
            });
        }
        if let Some(s) = self.search.as_ref() {
            s.focus_find(window, cx);
        }
    }

    /// `commitQuery`.
    fn commit_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(spec) = self.search.as_mut().and_then(|s| s.model.commit()) else {
            if let Some(s) = self.search.as_mut() {
                s.model.refresh_count(&self.state, false);
            }
            cx.notify();
            return;
        };
        self.dispatch(spec, window, cx);
        if let Some(s) = self.search.as_mut() {
            s.model.refresh_count(&self.state, false);
        }
        cx.notify();
    }

    /// Mirror an external query change into the fields (`setQuery`).
    pub(crate) fn sync_search_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.search.as_ref() else {
            return;
        };
        let (search, replace) = (s.model.search.clone(), s.model.replace.clone());
        let (find, rep) = (s.find.clone(), s.replace.clone());
        if find.read(cx).value() != search.as_str() {
            find.update(cx, |i, cx| i.set_value(search, window, cx));
        }
        if rep.read(cx).value() != replace.as_str() {
            rep.update(cx, |i, cx| i.set_value(replace, window, cx));
        }
    }

    pub(crate) fn run_search_run(
        &mut self,
        run: crate::search::SearchRun,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match run {
            crate::search::SearchRun::Dispatch(spec) => {
                self.dispatch(spec, window, cx);
                true
            }
            crate::search::SearchRun::Failed => false,
            crate::search::SearchRun::OpenPanel => self.run_cmd(Cmd::OpenSearchPanel, window, cx),
        }
    }

    fn toggle_search_button(&mut self, which: u8, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.search.as_mut() {
            match which {
                0 => s.model.case_sensitive = !s.model.case_sensitive,
                1 => s.model.regexp = !s.model.regexp,
                _ => s.model.whole_word = !s.model.whole_word,
            }
        }
        self.commit_search(window, cx);
    }

    /// Keys inside the find panel: `search-panel` scope, then Enter.
    fn search_panel_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = KeyEvent::from_keystroke(&ev.keystroke, self.platform);
        if self.run_keymap(&key, "search-panel", window, cx) {
            cx.stop_propagation();
        }
    }

    fn next_dialog_id(&mut self) -> u64 {
        self.dialog_seq += 1;
        self.dialog_seq
    }

    fn dialog_input_subs(
        &mut self,
        id: u64,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Subscription> {
        vec![cx.subscribe_in(
            input,
            window,
            move |v, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => v.submit_dialog(id, window, cx),
                InputEvent::Focus | InputEvent::Blur => cx.notify(),
                InputEvent::Change => {}
            },
        )]
    }

    /// `recurGotoLine` (⌥⌘G).
    pub(crate) fn open_goto(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.next_dialog_id();
        let value = crate::search::goto_line_initial(&self.state);
        let input = self.new_input(&value, "Line", window, cx);
        let subs = self.dialog_input_subs(id, &input, window, cx);
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.dialogs.insert(
            0,
            Dialog::Goto {
                id,
                input,
                _subs: subs,
            },
        );
        cx.notify();
    }

    /// `renameWithWorkspaceLoad`'s `window.prompt('New name', word)`.
    pub(crate) fn open_rename(&mut self, word: &str, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.new_input(word, "", window, cx);
        let subs = vec![cx.subscribe_in(
            &input,
            window,
            |v, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => v.finish_rename(true, window, cx),
                InputEvent::Focus | InputEvent::Blur | InputEvent::Change => cx.notify(),
            },
        )];
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.rename_prompt = Some(RenamePrompt { input, _subs: subs });
        cx.notify();
    }

    /// The prompt closed: OK with a non-empty name renames.
    fn finish_rename(&mut self, ok: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.rename_prompt.take() else {
            return;
        };
        let name = p.input.read(cx).value().to_string();
        self.focus(window, cx);
        cx.notify();
        if ok && !name.is_empty() {
            self.do_rename(&name, window, cx);
        }
    }

    /// `reportError`: a top message dialog.
    pub fn report_lsp_error(
        &mut self,
        message: &str,
        err: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_dialog_id();
        self.dialogs.insert(
            0,
            Dialog::Message {
                id,
                text: format!("{message}: {err}"),
            },
        );
        cx.notify();
    }

    /// `done(form)`.
    fn submit_dialog(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.dialogs.iter().position(|d| d.id() == id) else {
            return;
        };
        let value = self.dialogs[i]
            .input()
            .map(|input| input.read(cx).value().to_string());
        self.close_dialog(id, window, cx);
        if let Some(raw) = value {
            let state = self.state.clone();
            let spec = crate::search::goto_line_submit(&state, &raw);
            self.run_spec(spec, window, cx);
        }
    }

    /// `done(null)`: close, refocusing the editor if focus was inside.
    fn close_dialog(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.dialogs.iter().position(|d| d.id() == id) else {
            return;
        };
        let had_focus = self.dialogs[i]
            .input()
            .is_some_and(|input| input.read(cx).focus_handle(cx).is_focused(window));
        self.dialogs.remove(i);
        if had_focus {
            self.focus(window, cx);
        }
        cx.notify();
    }

    fn dialog_key(
        &mut self,
        id: u64,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ev.keystroke.key == "escape" {
            cx.stop_propagation();
            self.close_dialog(id, window, cx);
        }
    }

    // ---- rendering ----

    /// Track the hovered control for styles a card background can't take
    /// through GPUI's hover refinement.
    fn hover_tracked(
        &self,
        id: &'static str,
        e: gpui_kit::Stateful<gpui_kit::Div>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        e.on_hover(cx.listener(move |v, hovered: &bool, _, cx| {
            let next = if *hovered {
                Some(id)
            } else if v.hovered_control == Some(id) {
                None
            } else {
                v.hovered_control
            };
            if next != v.hovered_control {
                v.hovered_control = next;
                cx.notify();
            }
        }))
    }

    /// `.obs-icon-btn` (26×26, 15px icon), `.on` for pressed toggles, with
    /// its `title`.
    fn icon_button(
        &self,
        id: &'static str,
        title: &'static str,
        name: IconName,
        on: Option<bool>,
        on_click: impl Fn(&mut EditorView, &mut Window, &mut Context<EditorView>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let active = on == Some(true);
        let hovered = self.hovered_control == Some(id);
        let (fg, bg) = if active {
            (t.amber, Some(t.amber_d))
        } else if hovered {
            (t.text, Some(t.pane_2))
        } else {
            (t.faint, None)
        };
        let b = div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(26.))
            .rounded(px(5.))
            .text_color(h(fg))
            .when_some(bg, |d, bg| d.bg(h(bg)))
            .child(icon(name).size(px(15.)).color(h(fg)))
            .on_click(cx.listener(move |v, _, window, cx| on_click(v, window, cx)));
        titled(id, title, self.hover_tracked(id, b, cx)).into_any_element()
    }

    /// `.cm-recur-search-textbtn`, with its `title`.
    fn text_button(
        &self,
        id: &'static str,
        label: &'static str,
        title: &'static str,
        on_click: impl Fn(&mut EditorView, &mut Window, &mut Context<EditorView>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let light = Theme::of(cx).is_light();
        let hovered = self.hovered_control == Some(id);
        let (fg, border, bg) = if hovered {
            (t.text, t.line_bright, t.pane_2)
        } else {
            (t.muted, t.line, if light { t.pane } else { t.bg })
        };
        let b = card(bg, border, 5.)
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .h(px(26.))
            .px(px(8.))
            .child(button_label(label, 11., 500.).ui().color(h(fg)))
            .on_click(cx.listener(move |v, _, window, cx| on_click(v, window, cx)));
        titled(id, title, self.hover_tracked(id, b, cx)).into_any_element()
    }

    /// `.cm-recur-search-input`.
    fn input_field(
        &self,
        input: &Entity<InputState>,
        placeholder: &'static str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        recur_ui::style::style_text_input(input, cx);
        let theme = Theme::of(cx);
        let t = theme.tokens;
        let focused = input.read(cx).focus_handle(cx).is_focused(window);
        let border = if focused {
            t.amber.mix(0.55, t.line)
        } else {
            t.line
        };
        let bg = if theme.is_light() { t.pane } else { t.bg };
        card(bg, border, 6.)
            .h(px(28.))
            .px(px(9.))
            .flex()
            .items_center()
            .when(focused, |d| {
                d.shadow(vec![BoxShadow {
                    color: h(t.amber_d),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(2.),
                    inset: false,
                }])
            })
            .child(text_field(
                input,
                &SharedString::from(placeholder),
                t.faint,
                Family::Mono,
                12.,
                window,
                cx,
            ))
    }

    /// `.cm-recur-search-glyph`.
    fn glyph(&self, name: IconName, cx: &App) -> gpui_kit::Div {
        let faint = Theme::of(cx).tokens.faint;
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(26.))
            .child(icon(name).size(px(15.)).color(h(faint)))
    }

    fn render_search(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let s = self.search.as_ref()?;
        let t = Theme::of(cx).tokens;
        let label = s.model.count.label();
        let none = s.model.count == crate::search::SearchCount::None;
        let (case, regexp, word) = (s.model.case_sensitive, s.model.regexp, s.model.whole_word);
        let (find, replace) = (s.find.clone(), s.replace.clone());
        let row = |d: gpui_kit::Div| d.flex().items_center().gap(px(6.)).pl(px(10.)).pr(px(8.));
        let mut find_row = row(div())
            .min_h(px(36.))
            .pt(px(5.))
            .pb(px(5.))
            .child(self.glyph(IconName::Search, cx))
            .child(
                self.input_field(&find, "Find", window, cx)
                    .flex_1()
                    .min_w(px(80.)),
            );
        if !label.is_empty() {
            let count = text(label)
                .mono()
                .size(10.5)
                .leading(BODY_LEADING)
                .color(h(if none { t.coral } else { t.muted }))
                .nowrap();
            find_row = find_row.child(
                div()
                    .flex()
                    .flex_none()
                    .justify_end()
                    .px(px(2.))
                    .when(!none, |d| d.min_w(px(10.5 * 4.5)))
                    .child(count),
            );
        }
        let find_row = find_row
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(2.))
                    .child(self.icon_button(
                        "cm-search-prev",
                        "Previous match (⇧⌘G)",
                        IconName::ChevronLeft,
                        None,
                        |v, window, cx| {
                            v.run_cmd(Cmd::FindPrevious, window, cx);
                        },
                        cx,
                    ))
                    .child(self.icon_button(
                        "cm-search-next",
                        "Next match (⌘G)",
                        IconName::ChevronRight,
                        None,
                        |v, window, cx| {
                            v.run_cmd(Cmd::FindNext, window, cx);
                        },
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(2.))
                    .child(self.icon_button(
                        "cm-search-case",
                        "Match case (⌥⌘C)",
                        IconName::CaseSensitive,
                        Some(case),
                        |v, window, cx| v.toggle_search_button(0, window, cx),
                        cx,
                    ))
                    .child(self.icon_button(
                        "cm-search-regex",
                        "Regular expression (⌥⌘R)",
                        IconName::Regex,
                        Some(regexp),
                        |v, window, cx| v.toggle_search_button(1, window, cx),
                        cx,
                    ))
                    .child(self.icon_button(
                        "cm-search-word",
                        "Whole word (⌥⌘W)",
                        IconName::WholeWord,
                        Some(word),
                        |v, window, cx| v.toggle_search_button(2, window, cx),
                        cx,
                    )),
            )
            .child(self.text_button(
                "cm-search-all",
                "All",
                "Select all matches",
                |v, window, cx| {
                    let state = v.state.clone();
                    let run = crate::search::select_matches(&state);
                    v.run_search_run(run, window, cx);
                },
                cx,
            ))
            .child(div().ml(px(2.)).child(self.icon_button(
                "cm-search-close",
                "Close",
                IconName::X,
                None,
                |v, window, cx| {
                    v.run_cmd(Cmd::CloseSearchPanel, window, cx);
                },
                cx,
            )));
        let mut panel = div()
            .flex()
            .flex_col()
            .text_color(h(t.text))
            .capture_key_down(cx.listener(Self::search_panel_key))
            .child(find_row);
        if SearchPanel::has_replace(&self.state) {
            panel = panel.child(
                row(div())
                    .pb(px(8.))
                    .child(self.glyph(IconName::Replace, cx))
                    .child(
                        self.input_field(&replace, "Replace", window, cx)
                            .flex_1()
                            .min_w(px(80.)),
                    )
                    .child(self.text_button(
                        "cm-replace-one",
                        "Replace",
                        "Replace",
                        |v, window, cx| {
                            let state = v.state.clone();
                            let run = crate::search::replace_next(&state);
                            v.run_search_run(run, window, cx);
                        },
                        cx,
                    ))
                    .child(self.text_button(
                        "cm-replace-all",
                        "Replace all",
                        "Replace all",
                        |v, window, cx| {
                            let state = v.state.clone();
                            let run = crate::search::replace_all(&state);
                            v.run_search_run(run, window, cx);
                        },
                        cx,
                    )),
            );
        }
        Some(panel.into_any_element())
    }

    fn render_goto(
        &self,
        id: u64,
        input: &Entity<InputState>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .min_h(px(36.))
            .pt(px(5.))
            .pb(px(5.))
            .pl(px(10.))
            .pr(px(8.))
            .text_color(h(t.text))
            .capture_key_down(
                cx.listener(move |v, ev: &KeyDownEvent, window, cx| {
                    v.dialog_key(id, ev, window, cx)
                }),
            )
            .child(self.glyph(IconName::Hash, cx))
            .child(
                div().flex_none().child(
                    text("Go to line")
                        .mono()
                        .size(11.)
                        .weight(600.)
                        .tracking(0.02)
                        .leading(BODY_LEADING)
                        .color(h(t.muted))
                        .nowrap(),
                ),
            )
            .child(
                self.input_field(input, "Line", window, cx)
                    .flex_shrink_1()
                    .w(px(140.))
                    .min_w(px(72.))
                    .max_w(px(180.)),
            )
            .child(div().flex_none().mr_auto().child(self.text_button(
                "cm-goto-go",
                "Go",
                "Go",
                move |v, window, cx| v.submit_dialog(id, window, cx),
                cx,
            )))
            .child(self.icon_button(
                "cm-goto-close",
                "Close (Escape)",
                IconName::X,
                None,
                move |v, window, cx| v.close_dialog(id, window, cx),
                cx,
            ))
            .into_any_element()
    }

    /// CodeMirror's stock `.cm-button`.
    fn stock_button(
        &self,
        id: SharedString,
        label: &'static str,
        font_size: f32,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let (top, bottom, border) = self.theme.stock_button();
        div()
            .id(id)
            .flex_none()
            .py(px(0.2 * font_size))
            .px(px(font_size))
            .rounded(px(1.))
            .border_1()
            .border_color(h(border))
            .bg(linear_gradient(
                180.,
                linear_color_stop(top, 0.),
                linear_color_stop(bottom, 1.),
            ))
            .child(button_label(label, font_size, 400.).ui())
    }

    /// The rename prompt: a window-modal card with the field and
    /// Cancel / OK, standing in for the webview's `window.prompt`.
    pub(crate) fn render_rename_prompt(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = self.rename_prompt.as_ref()?;
        let theme = Theme::of(cx);
        let t = theme.tokens;
        let shadow = if theme.is_light() {
            css_box_shadow(0., 12., 28., 0., rgba(0, 0, 0, 0.12))
        } else {
            css_box_shadow(0., 12., 32., 0., rgba(0, 0, 0, 0.45))
        };
        let input = p.input.clone();
        let body = card(t.pane, t.line_bright, 8.)
            .w(px(320.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(12.))
            .text_color(h(t.text))
            .capture_key_down(cx.listener(|v, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    v.finish_rename(false, window, cx);
                }
            }))
            .child(text("New name").ui().size(13.).leading(BODY_LEADING))
            .child(self.input_field(&input, "", window, cx))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .child(self.text_button(
                        "cm-rename-cancel",
                        "Cancel",
                        "Cancel",
                        |v, window, cx| v.finish_rename(false, window, cx),
                        cx,
                    ))
                    .child(self.text_button(
                        "cm-rename-ok",
                        "OK",
                        "OK",
                        |v, window, cx| v.finish_rename(true, window, cx),
                        cx,
                    )),
            );
        Some(
            div()
                .absolute()
                .top(px(16.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(with_shadow(body, &[shadow], 8.))
                .into_any_element(),
        )
    }

    /// `.cm-dialog-close` (×, 14px, top-right).
    fn dialog_close(
        &self,
        id: u64,
        ent: Entity<Self>,
        top: f32,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div()
            .id(SharedString::from(format!("cm-dialog-close-{id}")))
            .absolute()
            .top(px(top))
            .right(px(4.))
            .child(button_label("×", 14., 400.).ui())
            .on_click(move |_, window, cx| ent.update(cx, |v, cx| v.close_dialog(id, window, cx)))
    }

    fn render_message(&self, id: u64, message: &str, cx: &mut Context<Self>) -> AnyElement {
        let fs = self.metrics.font_size;
        let (ok, close) = (cx.entity(), cx.entity());
        div()
            .relative()
            .pt(px(2.))
            .pr(px(19.))
            .pb(px(4.))
            .pl(px(6.))
            .child(
                text(format!("{message} "))
                    .ui()
                    .size(fs)
                    .leading(BODY_LEADING),
            )
            .child(
                div().flex().child(
                    self.stock_button(format!("cm-message-ok-{id}").into(), "OK", fs * 0.7)
                        .on_click(move |_, window, cx| {
                            ok.update(cx, |v, cx| v.close_dialog(id, window, cx))
                        }),
                ),
            )
            .child(self.dialog_close(id, close, 3.0))
            .into_any_element()
    }

    /// The top panel stack (`.cm-panels-top`): dialogs (newest first), then
    /// the find panel.
    pub(crate) fn render_panels(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.dialogs.is_empty() && self.search.is_none() {
            return None;
        }
        let t = Theme::of(cx).tokens;
        let mut stack = div()
            .flex()
            .flex_col()
            .flex_none()
            .bg(h(t.pane))
            .text_color(h(t.text))
            .border_b_1()
            .border_color(h(t.line));
        let dialogs: Vec<(u64, Option<Entity<InputState>>, Option<String>)> = self
            .dialogs
            .iter()
            .map(|d| match d {
                Dialog::Goto { id, input, .. } => (*id, Some(input.clone()), None),
                Dialog::Message { id, text } => (*id, None, Some(text.clone())),
            })
            .collect();
        for (id, input, message) in dialogs {
            stack = stack.child(match (input, message) {
                (Some(input), _) => self.render_goto(id, &input, window, cx),
                (None, Some(m)) => self.render_message(id, &m, cx),
                (None, None) => div().into_any_element(),
            });
        }
        if let Some(search) = self.render_search(window, cx) {
            stack = stack.child(search);
        }
        Some(stack.into_any_element())
    }

    /// Restyle open panel inputs after a theme change.
    pub(crate) fn restyle_panel_inputs(&self, cx: &mut Context<Self>) {
        let mut inputs: Vec<Entity<InputState>> = self
            .dialogs
            .iter()
            .filter_map(|d| d.input().cloned())
            .collect();
        inputs.extend(self.rename_prompt.as_ref().map(|p| p.input.clone()));
        if let Some(s) = self.search.as_ref() {
            inputs.push(s.find.clone());
            inputs.push(s.replace.clone());
        }
        for i in inputs {
            recur_ui::style::style_text_input(&i, cx);
        }
    }

    /// Whether keyboard focus is in a panel field (not the content).
    pub fn panel_has_focus(&self, window: &Window, cx: &App) -> bool {
        self.search
            .as_ref()
            .is_some_and(|s| s.contains_focus(window, cx))
            || self.dialogs.iter().any(|d| {
                d.input()
                    .is_some_and(|i| i.read(cx).focus_handle(cx).is_focused(window))
            })
    }

    /// The gutter or chip `title` under the pointer, as an invisible
    /// titled box for the app's `TitleTooltip`.
    pub(crate) fn render_title_target(&self) -> Option<AnyElement> {
        let (title, b) = self.title_seen.clone()?;
        let id = SharedString::from(format!(
            "cm-title-{}-{}",
            f32::from(b.origin.x),
            f32::from(b.origin.y)
        ));
        let target = div().w(b.size.width).h(b.size.height);
        Some(
            gpui_kit::deferred(recur_ui::style::anchored_at(
                f32::from(b.origin.x),
                f32::from(b.origin.y),
                titled(id, title, target),
            ))
            .into_any_element(),
        )
    }
}
