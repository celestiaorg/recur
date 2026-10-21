//! Top panels: Recur's find/replace panel (`cm-search-panel.ts`), the
//! go-to-line dialog (`cm-goto-line.ts`) and CodeMirror's stock dialogs the
//! LSP client opens (rename prompt, error messages), styled per
//! `_codemirror.scss` and CodeMirror's base theme.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::input::{Input, InputEditorStyle, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, AnyView, App, AppContext, BoxShadow, Context, Entity, Focusable, FontWeight, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Rgba, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, linear_color_stop,
    linear_gradient, point, px,
};

use crate::bindings::Cmd;
use crate::icons::{self, IconDef};
use crate::keymap::KeyEvent;
use crate::search::SearchPanel;
use crate::tokens::{AppTokens, mono_font, ui_font, with_font};
use crate::view::EditorView;

/// Builds the app's themed title tip (`TitleTooltip.svelte`) for a
/// control's `title`.
pub type TooltipBuilder = Rc<dyn Fn(SharedString, &mut Window, &mut App) -> AnyView>;

/// `SHOW_DELAY_MS` in `title-tooltip.ts`.
const TITLE_SHOW_DELAY: Duration = Duration::from_millis(160);

fn h(c: Rgba) -> Hsla {
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

fn input_style(t: &AppTokens) -> InputEditorStyle {
    InputEditorStyle {
        foreground: h(t.text),
        muted_foreground: h(t.faint),
        background: h(if t.light { t.pane } else { t.bg }),
        border: h(t.line),
        selection: h(t.amber_d),
        caret: h(t.text),
        ..InputEditorStyle::default()
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
        let style = input_style(&self.theme.tokens);
        let value = value.to_string();
        let placeholder = placeholder.to_string();
        cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder(placeholder);
            s.set_editor_style(style);
            s.set_value(value, window, cx);
            s
        })
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

    fn title_tip<E: StatefulInteractiveElement>(&self, e: E, title: &'static str) -> E {
        match self.tooltip_builder.clone() {
            Some(tip) => {
                let mut e = e.tooltip(move |window, cx| tip(title.into(), window, cx));
                e.interactivity().tooltip_show_delay(TITLE_SHOW_DELAY);
                e
            }
            None => e,
        }
    }

    /// `.obs-icon-btn` (26×26, 15px icon), `.on` for pressed toggles.
    fn icon_button(
        &self,
        id: &'static str,
        title: &'static str,
        icon: IconDef,
        on: Option<bool>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let t = self.theme.tokens;
        let active = on == Some(true);
        let b = div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(26.))
            .rounded(px(5.))
            .text_color(h(if active { t.amber } else { t.faint }))
            .when(active, |d| d.bg(h(t.amber_d)))
            .when(!active, |d| {
                d.hover(move |s| s.text_color(h(t.text)).bg(h(t.pane_2)))
            })
            .child(icon.element(15.));
        self.title_tip(b, title)
    }

    /// `.cm-recur-search-textbtn`.
    fn text_button(
        &self,
        id: &'static str,
        label: &'static str,
        title: &'static str,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let t = self.theme.tokens;
        let b = with_font(div(), &ui_font())
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .h(px(26.))
            .px(px(8.))
            .border_1()
            .border_color(h(t.line))
            .rounded(px(5.))
            .bg(h(if t.light { t.pane } else { t.bg }))
            .text_color(h(t.muted))
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .hover(move |s| {
                s.text_color(h(t.text))
                    .border_color(h(t.line_bright))
                    .bg(h(t.pane_2))
            })
            .child(label);
        self.title_tip(b, title)
    }

    /// `.cm-recur-search-input`.
    fn input_field(&self, input: &Entity<InputState>, window: &Window, cx: &App) -> gpui_kit::Div {
        let t = self.theme.tokens;
        let focused = input.read(cx).focus_handle(cx).is_focused(window);
        let border = if focused {
            AppTokens::mix(t.amber, 0.55, t.line)
        } else {
            t.line
        };
        with_font(div(), &mono_font())
            .h(px(28.))
            .px(px(9.))
            .flex()
            .items_center()
            .bg(h(if t.light { t.pane } else { t.bg }))
            .border_1()
            .border_color(h(border))
            .rounded(px(6.))
            .when(focused, |d| {
                d.shadow(vec![BoxShadow {
                    color: h(t.amber_d),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(2.),
                    inset: false,
                }])
            })
            .text_size(px(12.))
            .text_color(h(t.text))
            .child(Input::new(input))
    }

    fn glyph(&self, icon: IconDef) -> gpui_kit::Div {
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(26.))
            .text_color(h(self.theme.tokens.faint))
            .child(icon.element(15.))
    }

    fn render_search(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let s = self.search.as_ref()?;
        let t = self.theme.tokens;
        let label = s.model.count.label();
        let none = s.model.count == crate::search::SearchCount::None;
        let ent = cx.entity();
        let row = |d: gpui_kit::Div| d.flex().items_center().gap(px(6.)).pl(px(10.)).pr(px(8.));
        let mut find_row = row(div())
            .min_h(px(36.))
            .pt(px(5.))
            .pb(px(5.))
            .child(self.glyph(icons::SEARCH))
            .child(
                self.input_field(&s.find, window, cx)
                    .flex_1()
                    .min_w(px(80.)),
            );
        if !label.is_empty() {
            find_row = find_row.child(
                with_font(div(), &mono_font())
                    .flex_none()
                    .text_size(px(10.5))
                    .whitespace_nowrap()
                    .px(px(2.))
                    .text_color(h(if none { t.coral } else { t.muted }))
                    .when(!none, |d| d.min_w(px(10.5 * 4.5)).flex().justify_end())
                    .child(label),
            );
        }
        let e = |ent: &Entity<Self>| ent.clone();
        let (e1, e2, e3, e4, e5, e6, e7) = (
            e(&ent),
            e(&ent),
            e(&ent),
            e(&ent),
            e(&ent),
            e(&ent),
            e(&ent),
        );
        let find_row = find_row
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        self.icon_button(
                            "cm-search-prev",
                            "Previous match (⇧⌘G)",
                            icons::CHEVRON_LEFT,
                            None,
                        )
                        .on_click(move |_, window, cx| {
                            e1.update(cx, |v, cx| {
                                v.run_cmd(Cmd::FindPrevious, window, cx);
                            })
                        }),
                    )
                    .child(
                        self.icon_button(
                            "cm-search-next",
                            "Next match (⌘G)",
                            icons::CHEVRON_RIGHT,
                            None,
                        )
                        .on_click(move |_, window, cx| {
                            e2.update(cx, |v, cx| {
                                v.run_cmd(Cmd::FindNext, window, cx);
                            })
                        }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        self.icon_button(
                            "cm-search-case",
                            "Match case (⌥⌘C)",
                            icons::CASE_SENSITIVE,
                            Some(s.model.case_sensitive),
                        )
                        .on_click(move |_, window, cx| {
                            e3.update(cx, |v, cx| v.toggle_search_button(0, window, cx))
                        }),
                    )
                    .child(
                        self.icon_button(
                            "cm-search-regex",
                            "Regular expression (⌥⌘R)",
                            icons::REGEX,
                            Some(s.model.regexp),
                        )
                        .on_click(move |_, window, cx| {
                            e4.update(cx, |v, cx| v.toggle_search_button(1, window, cx))
                        }),
                    )
                    .child(
                        self.icon_button(
                            "cm-search-word",
                            "Whole word (⌥⌘W)",
                            icons::WHOLE_WORD,
                            Some(s.model.whole_word),
                        )
                        .on_click(move |_, window, cx| {
                            e5.update(cx, |v, cx| v.toggle_search_button(2, window, cx))
                        }),
                    ),
            )
            .child(
                self.text_button("cm-search-all", "All", "Select all matches")
                    .on_click(move |_, window, cx| {
                        e6.update(cx, |v, cx| {
                            let state = v.state.clone();
                            let run = crate::search::select_matches(&state);
                            v.run_search_run(run, window, cx);
                        })
                    }),
            )
            .child(
                div().ml(px(2.)).child(
                    self.icon_button("cm-search-close", "Close", icons::X, None)
                        .on_click(move |_, window, cx| {
                            e7.update(cx, |v, cx| {
                                v.run_cmd(Cmd::CloseSearchPanel, window, cx);
                            })
                        }),
                ),
            );
        let mut panel = with_font(div(), &ui_font())
            .flex()
            .flex_col()
            .text_color(h(t.text))
            .capture_key_down(cx.listener(Self::search_panel_key))
            .child(find_row);
        if SearchPanel::has_replace(&self.state) {
            let (r1, r2) = (ent.clone(), ent);
            panel = panel.child(
                row(div())
                    .pb(px(8.))
                    .child(self.glyph(icons::REPLACE))
                    .child(
                        self.input_field(&s.replace, window, cx)
                            .flex_1()
                            .min_w(px(80.)),
                    )
                    .child(
                        self.text_button("cm-replace-one", "Replace", "Replace")
                            .on_click(move |_, window, cx| {
                                r1.update(cx, |v, cx| {
                                    let state = v.state.clone();
                                    let run = crate::search::replace_next(&state);
                                    v.run_search_run(run, window, cx);
                                })
                            }),
                    )
                    .child(
                        self.text_button("cm-replace-all", "Replace all", "Replace all")
                            .on_click(move |_, window, cx| {
                                r2.update(cx, |v, cx| {
                                    let state = v.state.clone();
                                    let run = crate::search::replace_all(&state);
                                    v.run_search_run(run, window, cx);
                                })
                            }),
                    ),
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
        let t = self.theme.tokens;
        let (go, close) = (cx.entity(), cx.entity());
        with_font(div(), &ui_font())
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
            .child(self.glyph(icons::HASH))
            .child(
                with_font(div(), &mono_font())
                    .flex_none()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(h(t.muted))
                    .whitespace_nowrap()
                    .child("Go to line"),
            )
            .child(
                self.input_field(input, window, cx)
                    .flex_shrink_1()
                    .w(px(140.))
                    .min_w(px(72.))
                    .max_w(px(180.)),
            )
            .child(
                div().flex_none().mr_auto().child(
                    self.text_button("cm-goto-go", "Go", "Go")
                        .on_click(move |_, window, cx| {
                            go.update(cx, |v, cx| v.submit_dialog(id, window, cx))
                        }),
                ),
            )
            .child(
                self.icon_button("cm-goto-close", "Close (Escape)", icons::X, None)
                    .on_click(move |_, window, cx| {
                        close.update(cx, |v, cx| v.close_dialog(id, window, cx))
                    }),
            )
            .into_any_element()
    }

    /// CodeMirror's stock `.cm-button` / `.cm-textfield` chrome.
    fn stock_button(
        &self,
        id: SharedString,
        label: &'static str,
        font_size: f32,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let dark = self.theme.dark;
        let (top, bottom) = if dark {
            (0x393939, 0x111111)
        } else {
            (0xeff1f5, 0xd9d9df)
        };
        div()
            .id(id)
            .flex_none()
            .text_size(px(font_size))
            .py(px(0.2 * font_size))
            .px(px(font_size))
            .rounded(px(1.))
            .border_1()
            .border_color(h(gpui_kit::rgb(0x888888)))
            .bg(linear_gradient(
                180.,
                linear_color_stop(gpui_kit::rgb(top), 0.),
                linear_color_stop(gpui_kit::rgb(bottom), 1.),
            ))
            .child(label)
    }

    /// The rename prompt: a window-modal card with the field and
    /// Cancel / OK, standing in for the webview's `window.prompt`.
    pub(crate) fn render_rename_prompt(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = self.rename_prompt.as_ref()?;
        let t = self.theme.tokens;
        let (cancel, ok) = (cx.entity(), cx.entity());
        Some(
            with_font(div(), &ui_font())
                .absolute()
                .top(px(16.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .w(px(320.))
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .p(px(12.))
                        .bg(h(t.pane))
                        .border_1()
                        .border_color(h(t.line_bright))
                        .rounded(px(8.))
                        .shadow(vec![BoxShadow {
                            color: gpui_kit::hsla(0., 0., 0., if t.light { 0.12 } else { 0.45 }),
                            offset: point(px(0.), px(12.)),
                            blur_radius: px(if t.light { 28. } else { 32. }),
                            spread_radius: px(0.),
                            inset: false,
                        }])
                        .text_size(px(13.))
                        .text_color(h(t.text))
                        .capture_key_down(cx.listener(|v, ev: &KeyDownEvent, window, cx| {
                            if ev.keystroke.key == "escape" {
                                cx.stop_propagation();
                                v.finish_rename(false, window, cx);
                            }
                        }))
                        .child("New name")
                        .child(self.input_field(&p.input, window, cx))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(6.))
                                .child(
                                    self.text_button("cm-rename-cancel", "Cancel", "Cancel")
                                        .on_click(move |_, window, cx| {
                                            cancel.update(cx, |v, cx| {
                                                v.finish_rename(false, window, cx)
                                            })
                                        }),
                                )
                                .child(self.text_button("cm-rename-ok", "OK", "OK").on_click(
                                    move |_, window, cx| {
                                        ok.update(cx, |v, cx| v.finish_rename(true, window, cx))
                                    },
                                )),
                        ),
                )
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
            .text_size(px(14.))
            .child("×")
            .on_click(move |_, window, cx| ent.update(cx, |v, cx| v.close_dialog(id, window, cx)))
    }

    fn render_message(&self, id: u64, text: &str, cx: &mut Context<Self>) -> AnyElement {
        let fs = self.metrics.font_size;
        let (ok, close) = (cx.entity(), cx.entity());
        div()
            .relative()
            .pt(px(2.))
            .pr(px(19.))
            .pb(px(4.))
            .pl(px(6.))
            .text_size(px(fs))
            .child(format!("{text} "))
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
        let t = self.theme.tokens;
        let mut stack = with_font(div(), &ui_font())
            .flex()
            .flex_col()
            .flex_none()
            .bg(h(t.pane))
            .text_color(h(t.text))
            .text_size(px(self.metrics.font_size))
            .line_height(gpui_kit::relative(1.45))
            .border_b_1()
            .border_color(h(t.line));
        for d in &self.dialogs {
            stack = stack.child(match d {
                Dialog::Goto { id, input, .. } => self.render_goto(*id, input, window, cx),
                Dialog::Message { id, text } => self.render_message(*id, text, cx),
            });
        }
        if let Some(search) = self.render_search(window, cx) {
            stack = stack.child(search);
        }
        Some(stack.into_any_element())
    }

    /// Restyle open panel inputs after a theme change.
    pub(crate) fn restyle_panel_inputs(&self, cx: &mut Context<Self>) {
        let style = input_style(&self.theme.tokens);
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
            let style = style.clone();
            i.update(cx, |s, _| s.set_editor_style(style));
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

    /// Install the app's title tooltip builder for panel buttons.
    pub fn set_tooltip_builder(&mut self, tooltip: Option<TooltipBuilder>) {
        self.tooltip_builder = tooltip;
    }
}
