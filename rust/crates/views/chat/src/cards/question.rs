//! `QuestionCard.svelte`: the inline ask_user / loop-escalation answer card. Complete options
//! submit on click; options tagged `notes` select first and Answer sends the choice with the
//! typed notes (the loop's "Proceed with my notes" plan approval).

use std::time::{Duration, Instant};

use gpui_kit::base::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
    div, px,
};
use recur_content::{MarkdownClass, MarkdownView, Sheet};
use recur_domain::EscalationOption;
use recur_engine::view::chat::option_choice_text;
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{IconName, Theme, icon, motion};
use recur_view_core::AppState;
use recur_view_core::chat::AskUserAnswer;

use crate::style::{hsla, label};

/// The question as the card shows it (`QuestionLike`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestionLike {
    pub id: String,
    pub question: String,
    pub context: String,
    pub options: Vec<EscalationOption>,
}

/// An answer the host handles itself (`onAnswer`: the loop escalation).
#[derive(Clone, Debug, PartialEq)]
pub struct QuestionAnswered {
    pub choice: String,
    pub text: String,
}

pub struct QuestionCard {
    pub question: QuestionLike,
    pub kicker: String,
    /// The host answers (`onAnswer`) instead of `answerQuestion`.
    pub host_answers: bool,
    state: Entity<AppState>,
    input: Entity<InputState>,
    notes: Entity<TextareaState>,
    notes_opt: Option<String>,
    question_md: Entity<MarkdownView>,
    context_md: Entity<MarkdownView>,
    option_md: Vec<Entity<MarkdownView>>,
    mounted: Instant,
    _subs: Vec<Subscription>,
}

impl EventEmitter<QuestionAnswered> for QuestionCard {}

/// `font: inherit` from `.ask-q`: 12.5px, 600, `-0.01em`, the `.perm-tool` 1.25 line, no
/// paragraph margins.
fn question_sheet() -> Sheet {
    Sheet {
        size: 12.5,
        leading: 1.25,
        weight: 600.,
        tracking: -0.01,
        p_margin_bottom: 0.,
        ..Sheet::chat()
    }
}

/// `.ask-opt-text`: 12px 550 inside a `<button>` (line-height normal), no paragraph margins.
fn option_sheet(leading: f32) -> Sheet {
    Sheet {
        size: 12.,
        leading,
        weight: 550.,
        p_margin_bottom: 0.,
        ..Sheet::chat()
    }
}

/// `.perm.inline .perm-sum.chat-canvas`: 12.5 / 1.55.
fn context_sheet() -> Sheet {
    Sheet {
        size: 12.5,
        leading: 1.55,
        ..Sheet::chat()
    }
}

impl QuestionCard {
    pub fn new(
        question: QuestionLike,
        kicker: Option<String>,
        host_answers: bool,
        state: Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = if question.options.is_empty() {
            "Your answer…"
        } else {
            "Or type your own answer…"
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        recur_ui::style::style_text_input(&input, cx);
        let notes = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(3)
                .placeholder("Add your notes…")
        });
        let mut subs =
            vec![
                cx.subscribe(&input, |c: &mut Self, _, ev: &InputEvent, cx| match ev {
                    InputEvent::PressEnter { shift: false, .. } => c.answer_free(cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                }),
            ];
        subs.push(cx.observe(&notes, |_, _, cx| cx.notify()));
        // Enter (no Shift) in the notes field submits, as the textarea's keydown did.
        let me = cx.entity().downgrade();
        subs.push(cx.intercept_keystrokes(move |ev, window, cx| {
            let k = &ev.keystroke;
            if k.key != "enter" || k.modifiers.modified() {
                return;
            }
            let Some(card) = me.upgrade() else { return };
            let focused = card
                .read(cx)
                .notes
                .read(cx)
                .focus_handle(cx)
                .is_focused(window);
            let has_text =
                !recur_engine::view::js::trim(&card.read(cx).notes.read(cx).value()).is_empty();
            if focused && card.read(cx).notes_opt.is_some() && has_text {
                cx.stop_propagation();
                card.update(cx, |c, cx| c.answer_free(cx));
            }
        }));
        let question_md = cx.new(|cx| {
            let mut v = MarkdownView::new(
                question.question.clone(),
                MarkdownClass::ChatCanvas,
                false,
                cx,
            );
            v.set_sheet(question_sheet(), cx);
            v
        });
        let context_md = cx.new(|cx| {
            let mut v = MarkdownView::new(
                question.context.trim().to_string(),
                MarkdownClass::ChatCanvas,
                false,
                cx,
            );
            v.set_sheet(context_sheet(), cx);
            v
        });
        let (_, _, normal) = text("x")
            .size(12.)
            .weight(550.)
            .line_height(LineHeight::Normal)
            .measure(None, window);
        let leading = normal.height / 12.;
        let option_md = question
            .options
            .iter()
            .map(|o| {
                cx.new(|cx| {
                    let mut v =
                        MarkdownView::new(o.text.clone(), MarkdownClass::ChatCanvas, false, cx);
                    v.set_sheet(option_sheet(leading), cx);
                    v
                })
            })
            .collect();
        QuestionCard {
            question,
            kicker: kicker.unwrap_or_else(|| "Question for you".into()),
            host_answers,
            state,
            input,
            notes,
            notes_opt: None,
            question_md,
            context_md,
            option_md,
            mounted: Instant::now(),
            _subs: subs,
        }
    }

    fn submit(&mut self, choice: &str, text: String, cx: &mut Context<Self>) {
        let sent = option_choice_text(choice);
        if self.host_answers {
            cx.emit(QuestionAnswered { choice: sent, text });
            return;
        }
        let id = self.question.id.clone();
        self.state
            .update(cx, |s, cx| {
                s.answer_question(
                    &id,
                    AskUserAnswer {
                        choice: sent,
                        free_text: text,
                        attachments: Vec::new(),
                    },
                    cx,
                )
            })
            .detach();
    }

    fn pick(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(opt) = self.question.options.get(i).cloned() else {
            return;
        };
        if opt.tag == "notes" {
            self.notes_opt = Some(opt.text.clone());
            self.notes.update(cx, |n, cx| n.focus(window, cx));
            cx.notify();
            return;
        }
        self.notes_opt = None;
        let text = recur_engine::view::js::trim(&self.input.read(cx).value()).to_string();
        self.submit(&opt.text, text, cx);
    }

    fn free_text(&self, cx: &gpui_kit::App) -> String {
        let raw = if self.notes_opt.is_some() {
            self.notes.read(cx).value()
        } else {
            self.input.read(cx).value()
        };
        recur_engine::view::js::trim(&raw).to_string()
    }

    fn answer_free(&mut self, cx: &mut Context<Self>) {
        let text = self.free_text(cx);
        if text.is_empty() {
            return;
        }
        let choice = self.notes_opt.clone().unwrap_or_default();
        self.submit(&choice, text, cx);
    }
}

impl Render for QuestionCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        let k = motion::eased(
            self.mounted,
            Duration::from_millis(160),
            motion::Easing::EASE_OUT,
            cx,
        );
        if k < 1. && !motion::is_frozen(cx) {
            window.request_animation_frame();
        }
        let id = SharedString::from(format!("ask-{}", self.question.id));
        let can_answer = !self.free_text(cx).is_empty();
        let notes_pending = self.notes_opt.is_some();
        let mut main = div().flex().flex_col().gap(px(5.)).child(
            div()
                .text_color(hsla(t.text))
                .child(self.question_md.clone()),
        );
        if !self.question.context.trim().is_empty() {
            let vh = f32::from(window.viewport_size().height);
            main = main.child(
                div()
                    .id(SharedString::from(format!("{id}-ctx")))
                    .max_h(px((0.6 * vh).min(560.)))
                    .overflow_y_scroll()
                    .child(self.context_md.clone()),
            );
        }
        let mut card = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .child(
                        icon(IconName::MessageQuestion)
                            .size(px(13.))
                            .color(hsla(t.amber)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .min_w_0()
                            .flex_1()
                            .child(
                                label(self.kicker.clone(), Family::Ui, 12.5, 600.).color(t.text),
                            ),
                    ),
            )
            .child(main);
        if !self.question.options.is_empty() {
            let mut opts = div().flex().flex_col().gap(px(5.)).mt(px(9.));
            for (i, opt) in self.question.options.clone().iter().enumerate() {
                let sel =
                    opt.tag == "notes" && self.notes_opt.as_deref() == Some(opt.text.as_str());
                let tip = if opt.desc.is_empty() {
                    opt.text.clone()
                } else {
                    opt.desc.clone()
                };
                let (border, bg) = if sel {
                    (t.amber, t.amber.mix(0.1, t.pane))
                } else {
                    (t.line_bright, recur_ui::color::TRANSPARENT)
                };
                let mut btn = div()
                    .id(SharedString::from(format!("{id}-opt-{i}")))
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(1.))
                    .w_full()
                    .px(px(8.))
                    // `.perm.inline .perm-btn` (6px 8px) wins over the component's 5px 8px.
                    .py(px(6.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(hsla(border))
                    .bg(hsla(bg))
                    .cursor_pointer()
                    .text_color(hsla(t.text))
                    .when(!sel, |d| {
                        d.hover(|s| s.border_color(hsla(t.text)).bg(hsla(t.pane_2)))
                    })
                    .child(div().w_full().child(self.option_md[i].clone()))
                    .on_click(cx.listener(move |c, _, window, cx| c.pick(i, window, cx)));
                if !opt.desc.is_empty() {
                    btn = btn.child(
                        text(opt.desc.clone())
                            .id(SharedString::from(format!("{id}-od-{i}")))
                            .size(11.)
                            .line_height(LineHeight::Scale(1.35))
                            .color(t.muted),
                    );
                }
                opts = opts.child(recur_ui::tooltip::titled(
                    SharedString::from(format!("{id}-ot-{i}")),
                    tip,
                    btn,
                ));
            }
            card = card.child(opts);
        }
        let focused_input = if notes_pending {
            self.notes.read(cx).focus_handle(cx).is_focused(window)
        } else {
            self.input.read(cx).focus_handle(cx).is_focused(window)
        };
        let field_border = if focused_input {
            t.amber.mix(0.55, t.line)
        } else {
            t.line_bright
        };
        let field = if notes_pending {
            let (_, _, lm) = text("x")
                .size(12.)
                .line_height(LineHeight::Scale(1.4))
                .measure(None, window);
            recur_ui::style::card(t.bg, field_border, 6.)
                .flex_1()
                .min_w_0()
                .min_h(px(4.2 * 12. + 12. + 2.))
                .px(px(8.))
                .py(px(6.))
                .text_size(px(12.))
                .line_height(px(lm.height))
                .text_color(hsla(t.text))
                .child(Textarea::new(&self.notes))
                .into_any_element()
        } else {
            let ph = SharedString::from(if self.question.options.is_empty() {
                "Your answer…"
            } else {
                "Or type your own answer…"
            });
            recur_ui::style::card(t.bg, field_border, 6.)
                .flex_1()
                .min_w_0()
                .flex()
                .px(px(8.))
                .py(px(6.))
                // `font: inherit` hands the input the card's 1.45: a 17.4px line, text centered.
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h(px(12. * 1.45))
                        .flex()
                        .items_center()
                        .child(recur_ui::style::text_field(
                            &self.input,
                            &ph,
                            recur_ui::style::UA_PLACEHOLDER,
                            Family::Ui,
                            12.,
                            window,
                            cx,
                        )),
                )
                .into_any_element()
        };
        let amber = t.amber;
        let card_bg = t.amber.mix(0.06, t.pane);
        // `:disabled { opacity: .45 }` fades the button as one layer.
        let (fill, label_fg) = if can_answer {
            (amber, t.on_amber)
        } else {
            use recur_ui::style::faded;
            (
                faded(amber, recur_ui::color::TRANSPARENT, 0.45, card_bg),
                faded(t.on_amber, amber, 0.45, card_bg),
            )
        };
        let answer = div()
            .id(SharedString::from(format!("{id}-answer")))
            .flex_none()
            .self_end()
            .px(px(12.))
            .py(px(6.))
            .rounded(px(6.))
            .border_1()
            .border_color(hsla(fill))
            .bg(hsla(fill))
            .child(recur_ui::style::button_label("Answer", 11.5, 600.).color(label_fg))
            .when(can_answer, |d| {
                d.cursor_pointer()
                    .hover(move |s| {
                        s.bg(hsla(amber.brightness(1.07)))
                            .border_color(hsla(amber.brightness(1.07)))
                    })
                    .on_click(cx.listener(|c, _, _, cx| c.answer_free(cx)))
            });
        card = card.child(
            div()
                .flex()
                .gap(px(6.))
                .when(!notes_pending, |d| d.items_center())
                .mt(px(9.))
                .child(field)
                .child(answer),
        );
        recur_ui::style::card(card_bg, t.amber.mix(0.45, t.line), 9.)
            .w_full()
            .max_w(px(36. * 16.))
            .pt(px(11.))
            .pb(px(11.))
            .px(px(12.))
            .text_size(px(12.5))
            .opacity(k)
            .child(card)
    }
}
