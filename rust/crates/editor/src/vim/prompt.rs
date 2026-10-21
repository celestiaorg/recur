//! Vim's bottom panel (`.cm-vim-panel`): the `/` `?` `:` prompts and the
//! substitute confirm prompt (`showPrompt` / `openDialog`), notifications
//! (`showConfirm` / `openNotification`), the "recording @x" message, and
//! the virtual prompt mappings type into (`sendKeyToPrompt`).

use std::cell::Cell;
use std::time::Duration;

use gpui_kit::base::input::{Input, InputEditorStyle, InputEvent, InputState, Position};
use gpui_kit::{
    AnyElement, AppContext, Context, Entity, Focusable, Hsla, InteractiveElement, IntoElement,
    Keystroke, MouseButton, ParentElement, StatefulInteractiveElement, Styled, Subscription,
    Window, div, px,
};
use recur_ui::fonts::Family;
use recur_ui::text::WhiteSpace;
use recur_ui::{Text, TextStyled, Theme, span, text};

use super::cm::{Cm, ScrollInfo};
use super::search::VimRegex;
use super::state::{OptValue, with_global};
use crate::keymap::KeyEvent;
use crate::view::EditorView;

/// The prompt's prefix.
#[derive(Clone)]
pub(crate) enum Prefix {
    Text(String),
    /// `replace with <strong>…</strong> (y/n/a/q/l)`.
    Replace(String),
}

/// The prompt's trailing description.
#[derive(Clone, Copy)]
pub(crate) enum Desc {
    /// The search prompt's clickable pcre label.
    Pcre,
}

/// What the prompt's handlers do (`onClose` / `onKeyDown` / `onKeyUp`).
pub(crate) enum PromptKind {
    Search {
        forward: bool,
        to_jumplist: bool,
        original_query: Option<VimRegex>,
        original_scroll: ScrollInfo,
        last_query: String,
    },
    Ex,
    Replace(Box<super::ex::Replace>),
}

/// `PromptOptions`.
pub(crate) struct PromptOptions {
    pub prefix: Prefix,
    pub desc: Option<Desc>,
    pub value: Option<String>,
    pub kind: PromptKind,
}

/// `cm.state.dialog`.
pub(crate) enum Dialog {
    Prompt {
        id: u64,
        input: Entity<InputState>,
        prefix: Prefix,
        desc: Option<Desc>,
        /// Taken while a handler runs.
        opts: Option<Box<PromptOptions>>,
        _subs: Vec<Subscription>,
    },
    Notification {
        id: u64,
        text: String,
        long: bool,
    },
    Message {
        id: u64,
        text: String,
    },
}

impl Dialog {
    fn id(&self) -> u64 {
        match self {
            Dialog::Prompt { id, .. }
            | Dialog::Notification { id, .. }
            | Dialog::Message { id, .. } => *id,
        }
    }
}

thread_local! {
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
    /// The "recording @x" dialog (`macroModeState.onRecordingDone`).
    static RECORDING: Cell<Option<u64>> = const { Cell::new(None) };
}

fn next_id() -> u64 {
    NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    })
}

/// The `close(value)` a prompt handler gets: set the value or close.
pub(crate) struct PromptCtl {
    /// `None` for the virtual prompt.
    id: Option<u64>,
    pub value: String,
    /// `selectionEnd`, in characters.
    pub caret: usize,
    pub closed: bool,
}

impl PromptCtl {
    pub fn close(&mut self, cm: &mut Cm) {
        if self.closed {
            return;
        }
        self.closed = true;
        if let Some(id) = self.id {
            hide_prompt(cm, id);
        }
    }

    pub fn set(&mut self, cm: &mut Cm, value: &str) {
        if self.closed {
            return;
        }
        self.value = value.to_string();
        if let Some(input) = self.input(cm) {
            let v = value.to_string();
            let window = &mut *cm.window;
            input.update(cm.cx, |s, cx| s.set_value(v, window, cx));
        }
    }

    pub fn set_caret(&mut self, cm: &mut Cm, caret: usize) {
        self.caret = caret;
        if let Some(input) = self.input(cm) {
            let window = &mut *cm.window;
            input.update(cm.cx, |s, cx| {
                s.set_cursor_position(Position::new(0, caret as u32), window, cx)
            });
        }
    }

    fn input(&self, cm: &Cm) -> Option<Entity<InputState>> {
        let id = self.id?;
        match cm.view.vim.as_ref()?.dialog.as_ref()? {
            Dialog::Prompt { id: d, input, .. } if *d == id => Some(input.clone()),
            _ => None,
        }
    }
}

/// `showDialog`: replace the panel content, refocusing the editor if the
/// old dialog had focus.
fn show_dialog(cm: &mut Cm, dialog: Dialog) {
    let Some(v) = cm.view.vim.as_mut() else {
        return;
    };
    let old = v.dialog.replace(dialog);
    let had_focus = match &old {
        Some(Dialog::Prompt { input, .. }) => {
            input.read(cm.cx).focus_handle(cm.cx).is_focused(cm.window)
        }
        _ => false,
    };
    drop(old);
    if had_focus {
        cm.focus();
    }
    cm.cx.notify();
}

/// `hideDialog`: clear the panel if `id` is still showing.
fn hide_dialog(cm: &mut Cm, id: u64) {
    let Some(v) = cm.view.vim.as_mut() else {
        return;
    };
    if v.dialog.as_ref().is_some_and(|d| d.id() == id) {
        v.dialog = None;
        cm.cx.notify();
    }
}

/// An `openDialog` close: hide, then focus the editor when nothing else is up.
fn hide_prompt(cm: &mut Cm, id: u64) {
    hide_dialog(cm, id);
    if cm.view.vim.as_ref().is_some_and(|v| v.dialog.is_none()) {
        cm.focus();
    }
}

/// `openNotification`.
fn open_notification(cm: &mut Cm, text: &str, long: bool, duration_ms: u64) {
    let id = next_id();
    show_dialog(
        cm,
        Dialog::Notification {
            id,
            text: text.to_string(),
            long,
        },
    );
    let task = (duration_ms > 0).then(|| {
        cm.cx.spawn_in(cm.window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(duration_ms))
                .await;
            this.update_in(cx, |view, window, cx| {
                let mut cm = Cm::new(view, window, cx);
                hide_dialog(&mut cm, id);
            })
            .ok();
        })
    });
    if let Some(v) = cm.view.vim.as_mut() {
        v.notification_timer = task;
    }
}

/// `showConfirm`: a red message for 15s, or until the next key (`long`).
pub(crate) fn show_confirm(cm: &mut Cm, text: &str, long: bool) {
    if long {
        close_notification(cm);
        open_notification(cm, text, true, 0);
    } else {
        open_notification(cm, text, false, 15000);
    }
}

/// `cm.state.closeVimNotification()`.
pub(crate) fn close_notification(cm: &mut Cm) {
    let id = match cm.view.vim.as_ref().and_then(|v| v.dialog.as_ref()) {
        Some(Dialog::Notification { id, long: true, .. }) => *id,
        _ => return,
    };
    hide_dialog(cm, id);
}

/// `openDialog(template, fn, {bottom: true})` for "recording @x".
pub(crate) fn open_message(cm: &mut Cm, text: &str) {
    let id = next_id();
    if let Some(v) = cm.view.vim.as_mut() {
        v.notification_timer = None;
    }
    show_dialog(
        cm,
        Dialog::Message {
            id,
            text: text.to_string(),
        },
    );
    RECORDING.with(|r| r.set(Some(id)));
}

/// `onRecordingDone()`.
pub(crate) fn close_message(cm: &mut Cm) {
    if let Some(id) = RECORDING.with(|r| r.take()) {
        hide_prompt(cm, id);
    }
}

/// `showPrompt`: queue it while a mapping runs, else open the panel.
pub(crate) fn show_prompt(cm: &mut Cm, mut opts: PromptOptions) {
    if with_global(|g| !g.key_to_key_stack.is_empty()) {
        opts.value.get_or_insert_with(String::new);
        with_global(|g| g.virtual_prompt = Some(opts));
        return;
    }
    if let Some(v) = cm.view.vim.as_mut() {
        v.notification_timer = None;
    }
    let id = next_id();
    let value = opts.value.clone().filter(|v| !v.is_empty());
    let style = input_style(cm.cx);
    let input = {
        let window = &mut *cm.window;
        cm.cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_editor_style(style);
            if let Some(v) = value {
                s.set_value(v, window, cx);
            }
            s
        })
    };
    let weak = cm.cx.entity().downgrade();
    let focus = input.read(cm.cx).focus_handle(cm.cx).clone();
    let intercept = cm.cx.intercept_keystrokes(move |ev, window, cx| {
        if !focus.is_focused(window) {
            return;
        }
        let Some(view) = weak.upgrade() else { return };
        let stop = view.update(cx, |v, cx| {
            prompt_key_down(v, id, &ev.keystroke, window, cx)
        });
        if stop {
            cx.stop_propagation();
        }
    });
    let blur = cm.cx.subscribe_in(
        &input,
        cm.window,
        move |_, input, ev: &InputEvent, window, cx| {
            if let InputEvent::Blur = ev {
                let input = input.clone();
                cx.defer_in(window, move |view, window, cx| {
                    if view.vim.is_none() || input.read(cx).focus_handle(cx).is_focused(window) {
                        return;
                    }
                    let current = view
                        .vim
                        .as_ref()
                        .and_then(|v| v.dialog.as_ref())
                        .is_some_and(|d| d.id() == id);
                    if current {
                        let mut cm = Cm::new(view, window, cx);
                        hide_prompt(&mut cm, id);
                    }
                });
            }
        },
    );
    let prefix = opts.prefix.clone();
    let desc = opts.desc;
    show_dialog(
        cm,
        Dialog::Prompt {
            id,
            input: input.clone(),
            prefix,
            desc,
            opts: Some(Box::new(opts)),
            _subs: vec![intercept, blur],
        },
    );
    let window = &mut *cm.window;
    input.update(cm.cx, |s, cx| s.focus(window, cx));
}

/// The input's UA look: `FieldText` ink (Recur sets no `color-scheme`, so
/// form controls stay light-scheme black) on the panel's background.
fn input_style(cx: &gpui_kit::App) -> InputEditorStyle {
    let (_, _, field) = crate::theme::vim_panel_colors();
    let field: Hsla = field.into();
    InputEditorStyle {
        foreground: field,
        muted_foreground: field,
        background: Theme::of(cx).tokens.pane.into(),
        border: Hsla::transparent_black(),
        selection: Theme::of(cx).tokens.amber.fade(0.3).into(),
        caret: field,
        ..InputEditorStyle::default()
    }
}

/// Restyle an open prompt after a theme change.
pub(crate) fn restyle(view: &EditorView, cx: &mut Context<EditorView>) {
    if let Some(Dialog::Prompt { input, .. }) = view.vim.as_ref().and_then(|v| v.dialog.as_ref()) {
        let style = input_style(cx);
        input.update(cx, |s, _| s.set_editor_style(style));
    }
}

/// CSS `monospace` as WebKit resolves it (the default fixed font).
pub(crate) fn generic_monospace() -> Family {
    if cfg!(target_os = "macos") {
        Family::Stack("Courier".into(), vec!["monospace".into()])
    } else {
        Family::Stack("DejaVu Sans Mono".into(), vec!["monospace".into()])
    }
}

fn is_cancel(name: Option<&str>, value: &str) -> bool {
    matches!(name, Some("<Esc>") | Some("<C-c>") | Some("<C-[>"))
        || (name == Some("<BS>") && value.is_empty())
}

/// `onKeyDown`; returns (handler returned true, `e_stop`).
fn on_key_down(
    cm: &mut Cm,
    opts: &mut PromptOptions,
    name: Option<&str>,
    ctl: &mut PromptCtl,
) -> (bool, bool) {
    match &mut opts.kind {
        PromptKind::Search {
            original_query,
            original_scroll,
            ..
        } => {
            if is_cancel(name, &ctl.value) {
                let value = ctl.value.clone();
                super::search::search_prompt_cancel(
                    cm,
                    &value,
                    original_query.as_ref(),
                    *original_scroll,
                );
                ctl.close(cm);
                cm.focus();
                (false, true)
            } else if matches!(name, Some("<Up>") | Some("<Down>")) {
                (false, true)
            } else if name == Some("<C-u>") {
                ctl.set(cm, "");
                (false, true)
            } else {
                (false, false)
            }
        }
        PromptKind::Ex => {
            let mut stop = false;
            if is_cancel(name, &ctl.value) {
                with_global(|g| {
                    g.ex_history.push_input(&ctl.value);
                    g.ex_history.reset();
                });
                stop = true;
                super::clear_input_state(cm);
                super::search::clear_search_highlight(cm);
                ctl.close(cm);
                cm.focus();
            }
            if matches!(name, Some("<Up>") | Some("<Down>")) {
                stop = true;
                let up = name == Some("<Up>");
                let offset = ctl.caret;
                let input =
                    with_global(|g| g.ex_history.next_match(&ctl.value, up)).unwrap_or_default();
                ctl.set(cm, &input);
                if offset > 0 {
                    ctl.set_caret(cm, offset.min(input.chars().count()));
                }
            } else if name == Some("<C-u>") {
                stop = true;
                ctl.set(cm, "");
            } else if name.is_some_and(|n| n != "<Left>" && n != "<Right>") {
                with_global(|g| g.ex_history.reset());
            }
            (false, stop)
        }
        PromptKind::Replace(r) => {
            r.key(cm, name, ctl);
            (true, true)
        }
    }
}

/// `onKeyUp`.
fn on_key_up(cm: &mut Cm, opts: &mut PromptOptions, name: Option<&str>, ctl: &mut PromptCtl) {
    match &mut opts.kind {
        PromptKind::Search {
            forward,
            original_scroll,
            last_query,
            ..
        } => {
            let mut query = ctl.value.clone();
            if matches!(name, Some("<Up>") | Some("<Down>")) {
                let up = name == Some("<Up>");
                let offset = ctl.caret;
                query =
                    with_global(|g| g.search_history.next_match(&query, up)).unwrap_or_default();
                ctl.set(cm, &query);
                if offset > 0 {
                    ctl.set_caret(cm, offset.min(query.chars().count()));
                }
            } else if name.is_some_and(|n| n != "<Left>" && n != "<Right>") {
                with_global(|g| g.search_history.reset());
            }
            *last_query = query.clone();
            super::search::search_prompt_change(cm, &query, *forward, *original_scroll);
        }
        PromptKind::Ex => {
            let value = ctl.value.clone();
            super::ex::ex_prompt_key_up(cm, &value);
        }
        PromptKind::Replace(_) => {}
    }
}

/// `onClose(value)` (the dialog's Enter callback).
fn on_close(cm: &mut Cm, opts: PromptOptions, value: &str) {
    match opts.kind {
        PromptKind::Search {
            to_jumplist,
            original_scroll,
            ..
        } => super::search::search_prompt_close(cm, value, to_jumplist, original_scroll),
        PromptKind::Ex => super::ex::ex_prompt_close(cm, value),
        PromptKind::Replace(_) => {}
    }
}

/// The prompt's value and caret (characters), if `id` is showing.
fn prompt_state(cm: &Cm, id: u64) -> Option<(String, usize)> {
    match cm.view.vim.as_ref()?.dialog.as_ref()? {
        Dialog::Prompt { id: d, input, .. } if *d == id => {
            let s = input.read(cm.cx);
            let value = s.value().to_string();
            let caret = value[..s.cursor().min(value.len())].chars().count();
            Some((value, caret))
        }
        _ => None,
    }
}

fn take_opts(cm: &mut Cm, id: u64) -> Option<PromptOptions> {
    match cm.view.vim.as_mut()?.dialog.as_mut()? {
        Dialog::Prompt { id: d, opts, .. } if *d == id => opts.take().map(|o| *o),
        _ => None,
    }
}

fn restore_opts(cm: &mut Cm, id: u64, o: PromptOptions) {
    if let Some(Dialog::Prompt { id: d, opts, .. }) =
        cm.view.vim.as_mut().and_then(|v| v.dialog.as_mut())
        && *d == id
    {
        *opts = Some(Box::new(o));
    }
}

/// The prompt input's keydown, ahead of the input's own bindings
/// (`openDialog`'s keydown listener).
pub(crate) fn prompt_key_down(
    view: &mut EditorView,
    id: u64,
    ks: &Keystroke,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) -> bool {
    let key = KeyEvent::from_keystroke(ks, view.platform);
    let name = super::event_key_name(view, &key, ks.key_char.as_deref(), false);
    let mut cm = Cm::new(view, window, cx);
    let Some((value, caret)) = prompt_state(&cm, id) else {
        return false;
    };
    let Some(mut opts) = take_opts(&mut cm, id) else {
        return false;
    };
    let mut ctl = PromptCtl {
        id: Some(id),
        value,
        caret,
        closed: false,
    };
    let (returned, mut stop) = on_key_down(&mut cm, &mut opts, name.as_deref(), &mut ctl);
    if returned {
        if !ctl.closed {
            restore_opts(&mut cm, id, opts);
        }
        return stop;
    }
    let enter = ks.key == "enter";
    let escape = ks.key == "escape";
    let opts = if enter && !ctl.closed {
        let value = ctl.value.clone();
        on_close(&mut cm, opts, &value);
        None
    } else {
        Some(opts)
    };
    if enter || escape {
        stop = true;
        ctl.close(&mut cm);
    }
    if let Some(o) = opts
        && !ctl.closed
    {
        restore_opts(&mut cm, id, o);
    }
    stop
}

/// The prompt input's keyup.
pub(crate) fn prompt_key_up(
    view: &mut EditorView,
    id: u64,
    ks: &Keystroke,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    let key = KeyEvent::from_keystroke(ks, view.platform);
    let name = super::event_key_name(view, &key, ks.key_char.as_deref(), false);
    let mut cm = Cm::new(view, window, cx);
    let Some((value, caret)) = prompt_state(&cm, id) else {
        return;
    };
    let Some(mut opts) = take_opts(&mut cm, id) else {
        return;
    };
    let mut ctl = PromptCtl {
        id: Some(id),
        value,
        caret,
        closed: false,
    };
    on_key_up(&mut cm, &mut opts, name.as_deref(), &mut ctl);
    if !ctl.closed {
        restore_opts(&mut cm, id, opts);
    }
}

/// `sendKeyToPrompt`: a mapping types into the queued prompt.
pub(crate) fn send_key_to_prompt(cm: &mut Cm, key: &str) {
    let Some(mut opts) = with_global(|g| g.virtual_prompt.take()) else {
        return;
    };
    let mut key = key.to_string();
    if key.starts_with('<') {
        let lower = key.to_lowercase();
        let inner = if lower.chars().count() >= 2 {
            lower[1..lower.len() - 1].to_string()
        } else {
            String::new()
        };
        let last = inner.rsplit('-').next().unwrap_or("").to_string();
        if last == "lt" {
            key = "<".into();
        } else if last == "space" {
            key = " ".into();
        } else if last == "cr" {
            key = "\n".into();
        } else if let Some(cm_key) = super::vim_to_cm_key(&last) {
            let value = opts.value.clone().unwrap_or_default();
            let is_mac = cm.view.platform == crate::keymap::Platform::Mac;
            let name = super::keymap::vim_key_from_event(
                cm_key,
                None,
                false,
                false,
                false,
                false,
                is_mac,
                true,
                |_| false,
                &super::Langmap::default(),
            );
            let caret = value.chars().count();
            let mut ctl = PromptCtl {
                id: None,
                value,
                caret,
                closed: false,
            };
            on_key_down(cm, &mut opts, name.as_deref(), &mut ctl);
            if !ctl.closed {
                on_key_up(cm, &mut opts, name.as_deref(), &mut ctl);
            }
            if !ctl.closed {
                opts.value = Some(ctl.value);
                with_global(|g| g.virtual_prompt = Some(opts));
            }
            return;
        }
    }
    if key == "\n" {
        let value = opts.value.clone().unwrap_or_default();
        on_close(cm, opts, &value);
    } else {
        let mut v = opts.value.take().unwrap_or_default();
        v.push_str(&key);
        opts.value = Some(v);
        with_global(|g| g.virtual_prompt = Some(opts));
    }
}

/// The body line height the panel inherits.
const PANEL_LEADING: f32 = 1.45;
/// The prompt `<input>`'s UA font size (`-webkit-small-control`).
const INPUT_FONT: f32 = 11.;

/// The pcre label (`pcreLabel`).
fn pcre_label() -> &'static str {
    if with_global(|g| g.pcre()) {
        "(JavaScript regexp: set pcre)"
    } else {
        "(Vim regexp: set nopcre)"
    }
}

/// The label's mousedown: toggle `pcre` and re-run the incremental search.
fn toggle_pcre(view: &mut EditorView, id: u64, window: &mut Window, cx: &mut Context<EditorView>) {
    let pcre = with_global(|g| g.pcre());
    with_global(|g| {
        if let Some(o) = g.options.get_mut("pcre") {
            o.value = Some(OptValue::Bool(!pcre));
        }
    });
    let mut cm = Cm::new(view, window, cx);
    let Some(opts) = take_opts(&mut cm, id) else {
        return;
    };
    if let PromptKind::Search {
        forward,
        original_scroll,
        last_query,
        ..
    } = &opts.kind
    {
        let (q, f, s) = (last_query.clone(), *forward, *original_scroll);
        super::search::search_prompt_change(&mut cm, &q, f, s);
    }
    restore_opts(&mut cm, id, opts);
    cm.cx.notify();
}

/// `white-space: pre` tab stops (8 columns) for the monospace panel.
fn expand_tabs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let mut col = 0;
        for c in line.chars() {
            if c == '\t' {
                let n = 8 - col % 8;
                out.extend(std::iter::repeat_n(' ', n));
                col += n;
            } else {
                out.push(c);
                col += 1;
            }
        }
    }
    out
}

/// The bottom panel (`.cm-panels-bottom > .cm-vim-panel`): `monospace`
/// at the editor's font size, `min-height: 1.3em`, padding 0 10px.
pub(crate) fn render_panel(view: &EditorView, cx: &mut Context<EditorView>) -> Option<AnyElement> {
    let dialog = view.vim.as_ref()?.dialog.as_ref()?;
    let t = Theme::of(cx).tokens;
    let fs = view.metrics.font_size;
    let (desc_ink, message_ink, _) = crate::theme::vim_panel_colors();
    let mono = generic_monospace();
    let line = |s: String| text(s).family(mono.clone()).size(fs).leading(PANEL_LEADING);
    let content: AnyElement = match dialog {
        Dialog::Prompt {
            id,
            input,
            prefix,
            desc,
            ..
        } => {
            let id = *id;
            let prefix_el: AnyElement = match prefix {
                Prefix::Text(s) => line(s.clone())
                    .white_space(WhiteSpace::PreWrap)
                    .into_any_element(),
                Prefix::Replace(with) => Text::from_spans(vec![
                    span("replace with "),
                    span(with.clone()).weight(700.),
                    span(" (y/n/a/q/l)"),
                ])
                .family(mono.clone())
                .size(fs)
                .leading(PANEL_LEADING)
                .white_space(WhiteSpace::PreWrap)
                .into_any_element(),
            };
            let (field_family, field_size) = (
                mono.font(400., false, Default::default()).family,
                INPUT_FONT,
            );
            let field = div()
                .flex()
                .flex_1()
                .items_center()
                .min_w(px(0.))
                .font_family(field_family)
                .text_size(px(field_size))
                .child(Input::new(input));
            let ent = cx.entity();
            let mut row = div()
                .flex()
                .flex_1()
                .capture_key_up(
                    cx.listener(move |v, ev: &gpui_kit::KeyUpEvent, window, cx| {
                        prompt_key_up(v, id, &ev.keystroke, window, cx)
                    }),
                )
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .child(div().flex_none().child(prefix_el))
                        .child(field),
                );
            if let Some(Desc::Pcre) = desc {
                row = row.child(
                    div()
                        .id(("vim-pcre", id as usize))
                        .flex_none()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            window.prevent_default();
                            ent.update(cx, |v, cx| toggle_pcre(v, id, window, cx));
                        })
                        .child(
                            line(pcre_label().into())
                                .color(Hsla::from(desc_ink))
                                .nowrap(),
                        ),
                );
            }
            row.into_any_element()
        }
        Dialog::Notification {
            id,
            text: msg,
            long,
        } => {
            let id = *id;
            let ent = cx.entity();
            let msg = line(expand_tabs(msg))
                .color(Hsla::from(message_ink))
                .white_space(WhiteSpace::PreWrap);
            let body = if *long {
                div()
                    .child(msg)
                    .child(line("Press ENTER or type command to continue".into()))
            } else {
                div().child(msg)
            };
            body.id(("vim-notification", id as usize))
                .on_click(move |_, window, cx| {
                    ent.update(cx, |v, cx| {
                        let mut cm = Cm::new(v, window, cx);
                        hide_dialog(&mut cm, id);
                    })
                })
                .into_any_element()
        }
        Dialog::Message { text: msg, .. } => line(msg.clone()).into_any_element(),
    };
    let panel = div()
        .flex()
        .min_h(px(fs * 1.3))
        .px(px(10.))
        .child(div().flex().flex_1().child(content));
    Some(
        div()
            .flex_none()
            .w_full()
            .border_t_1()
            .border_color(Hsla::from(t.line))
            .bg(Hsla::from(t.pane))
            .text_color(Hsla::from(t.text))
            .child(panel)
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_expand_to_eight_columns() {
        assert_eq!(expand_tabs("mark\tline\tcol"), "mark    line    col");
        assert_eq!(expand_tabs("a\tb\nab\tc"), "a       b\nab      c");
    }
}
