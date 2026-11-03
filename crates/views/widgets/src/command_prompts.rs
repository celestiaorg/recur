//! A bundle command's `prompts` form before it runs.

use std::collections::BTreeMap;

use gpui_kit::base::input::InputState;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, Focusable, InteractiveElement,
    IntoElement, KeystrokeEvent, MouseButton, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};
use recur_ui::fonts::Family;
use recur_ui::guard::Guarded;
use recur_ui::set::hsla;
use recur_ui::shadow::with_shadow;
use recur_ui::style::{
    PALETTE_SHADOW, esc_key, palette_input_row, palette_scrim, style_text_input, text_field,
};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::widgets::select::{Select, SelectEvent, SelectOption};
use recur_ui::{IconName, Theme, hex, icon};
use recur_view_core::commands::CommandPrompt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandPromptsEvent {
    /// `onResolve(values)`.
    Resolve(BTreeMap<String, String>),
    Cancel,
}

enum Field {
    Text(Entity<InputState>),
    Choice(Entity<Select>, SharedString),
}

/// The prompts form: open it with [`CommandPrompts::open`]; it emits
/// [`CommandPromptsEvent`].
pub struct CommandPrompts {
    title: String,
    prompts: Vec<CommandPrompt>,
    fields: Vec<Field>,
    is_open: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<CommandPromptsEvent> for CommandPrompts {}

impl CommandPrompts {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let me = cx.entity().downgrade();
        let sub = cx.intercept_keystrokes(move |ev, window, cx| {
            let Some(me) = me.upgrade() else { return };
            if me.update(cx, |p, cx| p.on_key(ev, window, cx)) {
                cx.stop_propagation();
            }
        });
        CommandPrompts {
            title: "Command".into(),
            prompts: Vec::new(),
            fields: Vec::new(),
            is_open: false,
            _subs: vec![sub],
        }
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    /// Opens on `prompts`, each field at its default, and focuses the first.
    pub fn open(
        &mut self,
        title: &str,
        prompts: Vec<CommandPrompt>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.title = if title.is_empty() {
            "Command".into()
        } else {
            title.to_string()
        };
        let mut subs = self._subs.drain(..1).collect::<Vec<_>>();
        self.fields = prompts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if p.r#type == "choice" && !p.choices.is_empty() {
                    let opts = p
                        .choices
                        .iter()
                        .map(|c| SelectOption::new(c.clone(), c.clone()))
                        .collect();
                    let select = cx.new(|cx| {
                        let mut s = Select::new(opts, window, cx);
                        s.set_value(p.default.clone(), cx);
                        s
                    });
                    subs.push(cx.guarded_subscribe(
                        &select,
                        move |this, _, ev: &SelectEvent, cx| {
                            if let (SelectEvent::Change(v), Some(Field::Choice(_, cur))) =
                                (ev, this.fields.get_mut(i))
                            {
                                *cur = v.clone();
                                cx.notify();
                            }
                        },
                    ));
                    Field::Choice(select, p.default.clone().into())
                } else {
                    let masked = p.r#type == "password";
                    let default = p.default.clone();
                    Field::Text(cx.new(|cx| {
                        let mut s = InputState::new(window, cx).masked(masked);
                        s.set_value(default, window, cx);
                        s
                    }))
                }
            })
            .collect();
        self._subs = subs;
        self.prompts = prompts;
        self.is_open = true;
        if let Some(Field::Text(first)) = self.fields.first() {
            let focus = first.focus_handle(cx);
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
        }
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.is_open = false;
        cx.notify();
    }

    fn values(&self, cx: &gpui_kit::App) -> BTreeMap<String, String> {
        self.prompts
            .iter()
            .zip(&self.fields)
            .map(|(p, f)| {
                let v = match f {
                    Field::Text(i) => i.read(cx).value().to_string(),
                    Field::Choice(_, v) => v.to_string(),
                };
                (p.name.clone(), v)
            })
            .collect()
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let values = self.values(cx);
        cx.emit(CommandPromptsEvent::Resolve(values));
    }

    fn on_key(&mut self, ev: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.is_open {
            return false;
        }
        let inside = self.fields.iter().any(|f| match f {
            Field::Text(i) => i.focus_handle(cx).is_focused(window),
            Field::Choice(s, _) => s.focus_handle(cx).contains_focused(window, cx),
        });
        if !inside {
            return false;
        }
        match ev.keystroke.key.as_str() {
            "escape" => {
                cx.emit(CommandPromptsEvent::Cancel);
                true
            }
            "enter" => {
                self.submit(cx);
                true
            }
            _ => false,
        }
    }
}

impl CommandPrompts {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        if !self.is_open {
            return div().into_any_element();
        }
        let t = Theme::of(cx).tokens;
        // `var(--border, #2a2f38)`: `--border` is not a theme token.
        let border = hex(0x2a2f38);
        for f in &self.fields {
            if let Field::Text(i) = f {
                style_text_input(i, cx);
            }
        }
        let empty: SharedString = SharedString::default();
        let fields = self.prompts.iter().zip(&self.fields).map(|(p, f)| {
            let label = if p.label.is_empty() {
                p.name.clone()
            } else {
                p.label.clone()
            };
            let control: AnyElement = match f {
                Field::Text(i) => recur_ui::style::card(t.pane_2, border, 6.)
                    .flex()
                    .py(px(7.))
                    .px(px(9.))
                    .child(text_field(i, &empty, t.faint, Family::Ui, 13., window, cx))
                    .into_any_element(),
                Field::Choice(s, _) => s.clone().into_any_element(),
            };
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    text(label)
                        .size(11.)
                        .color(t.muted)
                        .uppercase()
                        .tracking(0.04)
                        .line_height(LineHeight::Normal),
                )
                .child(control)
        });
        let head = palette_input_row(false, cx)
            .child(icon(IconName::Settings).size(px(16.)).color(hsla(t.faint)))
            .child(
                div().flex_1().min_w_0().child(
                    text(self.title.clone())
                        .size(13.)
                        .weight(600.)
                        .color(t.text)
                        .nowrap(),
                ),
            )
            .child(esc_key(false, cx));
        let body = div()
            .py(px(12.))
            .px(px(14.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .children(fields);
        let amber = t.amber;
        let actions = div()
            .flex()
            .justify_end()
            .items_center()
            .gap(px(10.))
            .pt(px(10.))
            .px(px(14.))
            .pb(px(12.))
            .border_t_1()
            .border_color(hsla(border))
            .child(
                div()
                    .id("cmd-cancel")
                    .cursor_pointer()
                    .child(text("Cancel").color(t.text).line_height(LineHeight::Normal))
                    .on_click(
                        cx.guarded_listener(|_, _, _, cx| cx.emit(CommandPromptsEvent::Cancel)),
                    ),
            )
            .child(
                div()
                    .id("cmd-run")
                    .cursor_pointer()
                    .bg(hsla(amber))
                    .rounded(px(6.))
                    .py(px(6.))
                    .px(px(12.))
                    .child(
                        text("Run")
                            .weight(600.)
                            .color(t.on_amber)
                            .line_height(LineHeight::Normal),
                    )
                    .on_click(cx.guarded_listener(|p, _, _, cx| p.submit(cx))),
            );
        let vh = f32::from(window.viewport_size().height);
        let panel = recur_ui::style::palette_panel(window, cx)
            .max_h(px(vh * 0.64))
            .child(head)
            .child(body)
            .child(actions);
        let width = 420f32.min(f32::from(window.viewport_size().width) * 0.92);
        let panel = with_shadow(panel, &PALETTE_SHADOW, 13.)
            .id("cmd-prompts")
            .w(px(width))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        palette_scrim(window, panel)
            .id("cmd-prompts-bg")
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(|_, _, _, cx| cx.emit(CommandPromptsEvent::Cancel)),
            )
            .into_any_element()
    }
}

impl Render for CommandPrompts {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}
