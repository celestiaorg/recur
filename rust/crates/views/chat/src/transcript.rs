//! The transcript blocks (`AgentChat.svelte`'s `{#each blocks}`): engine facts and their
//! wave / plan cards, specialist sessions, the turn footer with its actions, presented card and
//! findings digest, notices, stopped and error rows, user bubbles, assistant replies, step
//! groups, and the inline permission / question cards with the live turn row under them.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, canvas, div, px, relative,
};
use recur_content::Sheet;
use recur_engine::view::chat::{
    AgentMsg, ChatLog, agent_error_detail, agent_error_title, last_assistant_text_in_turn,
};
use recur_engine::view::digest::TurnDigestFinding;
use recur_engine::view::loop_cockpit::{
    PrCompletionCtx, PrRef, loop_completion_wrap, loop_pr_completion, strip_loop_wrap,
};
use recur_engine::view::mentions::strip_attachment_markers;
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{IconName, Theme, icon, motion};
use recur_view_core::format::format_elapsed;

use crate::blocks::{
    Block, BubblePart, bubble_parts, cockpit_start_ts, group_summary, last_presented_in_turn,
};
use crate::chat::{AgentChat, ChatEvent, debug_key};
use crate::flow::{ATOM, FlowStyle, flow};
use crate::html_inline::{InlineStyle, inline_html};
use crate::rows::{accord_detail, finding_row_wrap, findings_list, labeled_note};
use crate::style::{GREEN, ag_color, hsla, label};
use crate::tools::{base_name, format_duration};
use crate::window::{BlockShape, CHAT_TAIL_LIVE, estimate_chat_block_height, reserved_chat_height};

/// `.chat-think-dots`: three 5px amber dots bouncing in turn.
pub(crate) fn think_dots(
    epoch: std::time::Instant,
    window: &mut Window,
    cx: &gpui_kit::App,
) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let e = motion::elapsed(epoch, cx).as_secs_f32();
    let animate = !motion::reduced(cx);
    if animate && !motion::is_frozen(cx) {
        window.request_animation_frame();
    }
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .children((0..3).map(|i| {
            // `chat-think-pulse` 1.05s ease-in-out, delays 0 / .18 / .36s.
            let (opacity, dy) = if animate {
                let local = e - 0.18 * i as f32;
                if local < 0. {
                    (0.35, 0.)
                } else {
                    let p = (local % 1.05) / 1.05;
                    let half = if p < 0.5 { p * 2. } else { (1. - p) * 2. };
                    let k = motion::Easing::EASE_IN_OUT.apply(half);
                    (0.25 + 0.75 * k, -2. * k)
                }
            } else {
                (0.35, 0.)
            };
            div()
                .size(px(5.))
                .relative()
                .top(px(dy))
                .rounded_full()
                .bg(hsla(t.amber.fade(opacity)))
        }))
        .into_any_element()
}

impl AgentChat {
    /// The transcript column's children.
    pub(crate) fn transcript_children(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let t = Theme::of(cx).tokens;
        let msgs = self.transcript();
        let (blocks, keys) = self.blocks(&msgs);
        let windowed = self.window_chat();
        let first_checkpoint = self.first_checkpoint(&msgs, &blocks, cx);
        let mut out = Vec::new();
        if msgs.is_empty() {
            let empty = match &self.empty {
                Some(v) => v.clone().into_any_element(),
                None => text(self.props.empty_text.clone())
                    .id(SharedString::from("chat-empty"))
                    .size(13.)
                    .line_height(LineHeight::Scale(1.55))
                    .color(t.muted)
                    .into_any_element(),
            };
            out.push(
                div()
                    .max_w(px(36. * 13.))
                    .pt(px(8.))
                    .px(px(2.))
                    .pb(px(20.))
                    .child(empty)
                    .into_any_element(),
            );
        }
        let ranges = self.block_ranges.clone();
        let n = blocks.len();
        for (bi, b) in blocks.iter().enumerate() {
            let k = keys[bi].clone();
            let live = !windowed || self.chat_hydrated.contains(&k) || bi + CHAT_TAIL_LIVE >= n;
            let estimate = match b {
                Block::Steps { .. } => estimate_chat_block_height(BlockShape::Steps),
                Block::Msg(i) => {
                    let m = &msgs[*i];
                    estimate_chat_block_height(BlockShape::Msg {
                        me: m.me,
                        turn_end: m.turn_end,
                        body: &m.body,
                    })
                }
            };
            let measured = self.block_heights().get(&k).copied();
            let reserved = reserved_chat_height(live, measured, estimate);
            let body = if live {
                Some(match b {
                    Block::Msg(i) => self.msg_block(&msgs, *i, window, cx),
                    Block::Steps { start, rows } => self.steps_block(
                        &msgs,
                        *start,
                        rows,
                        bi + 1 == n,
                        first_checkpoint,
                        window,
                        cx,
                    ),
                })
            } else {
                None
            };
            // `registerChatBlock` / `recordStubHeight`: where each block sits, and live
            // heights for stubs to reserve.
            let rec = ranges.clone();
            let key = k.clone();
            let probe = canvas(
                move |bounds, _, _| {
                    rec.borrow_mut().insert(
                        key.clone(),
                        (f32::from(bounds.origin.y), f32::from(bounds.size.height)),
                    );
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0();
            out.push(
                div()
                    .id(SharedString::from(format!("cblk-{k}")))
                    .debug_selector(|| format!("{}:{k}", if live { "cblk" } else { "stub" }))
                    .relative()
                    .flex()
                    .flex_col()
                    .when_some(reserved, |d, h| d.min_h(px(h)))
                    .children(body)
                    .child(probe)
                    .into_any_element(),
            );
        }
        for req in self.chat_permissions(cx) {
            let card = self.permission_card(&req, window, cx);
            out.push(div().mt(px(8.)).mb(px(14.)).child(card).into_any_element());
        }
        for q in self.chat_questions(cx) {
            let card = self.question_card_for_user(&q, window, cx);
            out.push(div().mt(px(8.)).mb(px(14.)).child(card).into_any_element());
        }
        if let Some(esc) = self
            .chat_escalation(cx)
            .filter(|_| self.hooks.answer_escalation)
        {
            let card = self.question_card_for_escalation(&esc, window, cx);
            out.push(div().mt(px(8.)).mb(px(14.)).child(card).into_any_element());
        }
        if self.show_turn_elapsed(cx) {
            out.push(self.turn_row(format_elapsed(self.turn_elapsed_sec), true, window, cx));
        } else if !self.props.status_text.is_empty() {
            out.push(self.turn_row(self.props.status_text.clone(), false, window, cx));
        }
        out
    }

    /// `.chat-turn`: the bouncing dots and the elapsed clock or the setup status.
    fn turn_row(
        &self,
        s: String,
        elapsed: bool,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let label_el = label(s, Family::Mono, 11., 400.).tabular().color(t.faint);
        div()
            .flex()
            .when(elapsed, |d| d.debug_selector(|| "chat-turn-elapsed".into()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .mt(px(4.))
                    .mb(px(10.))
                    .py(px(2.))
                    .child(think_dots(self.anim_epoch, window, cx))
                    .child(label_el),
            )
            .into_any_element()
    }

    /// A one-message block.
    fn msg_block(
        &mut self,
        msgs: &[AgentMsg],
        i: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let m = msgs[i].clone();
        if let Some(log) = &m.log {
            return match log.as_ref() {
                ChatLog::Fact { .. } => self.fact_block(&m, msgs, window, cx),
                ChatLog::Session(s) => self.sessions_block(std::slice::from_ref(s), window, cx),
                ChatLog::Sessions { sessions, .. } => self.sessions_block(sessions, window, cx),
            };
        }
        if m.turn_end {
            return self.turn_end_block(msgs, i, window, cx);
        }
        if m.note {
            return self.notice_card(&m, &i.to_string(), cx);
        }
        if m.stopped {
            return self.stopped_row(&m, cx);
        }
        if m.error {
            return self.error_block(&m, i, msgs, cx);
        }
        if m.me {
            return self.user_bubble(&m, i, window, cx);
        }
        if !m.body.is_empty() {
            let key = format!("{}:m{i}", self.props.reset_key);
            // `.chat-row` has no padding: the canvas's edge margins collapse into its 18px.
            let sheet = Some(Sheet {
                collapse_edges: true,
                ..Sheet::chat()
            });
            let body = if m.streaming {
                self.stream(format!("{key}-live"), &m.body, sheet, cx)
            } else {
                self.md(format!("{key}-md"), &m.body, sheet, false, cx)
            };
            let streaming = m.streaming;
            return div()
                .debug_selector(|| {
                    format!(
                        "{}:{}",
                        if streaming { "streaming" } else { "assistant" },
                        debug_key(&m.body)
                    )
                })
                .mb(px(18.))
                .max_w_full()
                .child(body)
                .into_any_element();
        }
        div().into_any_element()
    }

    fn stopped_row(&self, m: &AgentMsg, cx: &Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        div()
            .flex()
            .debug_selector(|| "chat-stopped".into())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .mt(px(4.))
                    .mb(px(12.))
                    .child(icon(IconName::OctagonX).size(px(13.)).color(hsla(t.coral)))
                    .child(
                        label(
                            if m.body.is_empty() {
                                "Stopped by user".to_string()
                            } else {
                                m.body.clone()
                            },
                            Family::Mono,
                            11.,
                            400.,
                        )
                        .line_height(LineHeight::Scale(1.3))
                        .color(t.coral),
                    ),
            )
            .into_any_element()
    }

    /// `.chat-error` with Retry on the trailing failure.
    fn error_block(
        &mut self,
        m: &AgentMsg,
        i: usize,
        msgs: &[AgentMsg],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let title = agent_error_title(&m.body);
        let detail = agent_error_detail(&m.body);
        let can_retry = self.last_retry_error(msgs) == Some(i);
        let bg = t.coral_d.over(t.pane);
        let sel = format!("chat-error:{title}");
        let mut card = recur_ui::style::card(bg, t.coral_line, 8.)
            .debug_selector(|| sel)
            .flex()
            .flex_col()
            .gap(px(8.))
            .mt(px(6.))
            .mb(px(12.))
            .pt(px(11.))
            .px(px(12.))
            .pb(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w_0()
                    .child(
                        icon(IconName::CircleAlert)
                            .size(px(15.))
                            .color(hsla(t.coral)),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            text(title)
                                .id(SharedString::from(format!("err-t-{i}")))
                                .size(13.)
                                .weight(600.)
                                .color(t.text),
                        ),
                    ),
            );
        if !detail.is_empty() && detail != title {
            card = card.child(
                text(detail)
                    .id(SharedString::from(format!("err-b-{i}")))
                    .size(12.5)
                    .line_height(LineHeight::Scale(1.45))
                    .color(t.muted)
                    .pre_wrap()
                    .break_anywhere(),
            );
        }
        if can_retry {
            let amber = t.amber;
            card = card.child(
                div()
                    .flex()
                    .justify_end()
                    .pt(px(2.))
                    .child(recur_ui::tooltip::titled(
                        SharedString::from("chat-retry-tip"),
                        "Retry last message",
                        div()
                            .id("chat-retry")
                            .debug_selector(|| "chat-error-retry".into())
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .h(px(28.))
                            .min_w(px(72.))
                            .px(px(12.))
                            .rounded(px(7.))
                            .border_1()
                            .border_color(hsla(amber))
                            .bg(hsla(amber))
                            .cursor_pointer()
                            .hover(move |s| {
                                s.bg(hsla(amber.brightness(1.08)))
                                    .border_color(hsla(amber.brightness(1.08)))
                            })
                            .active(move |s| s.bg(hsla(amber.brightness(0.94))).top(px(1.)))
                            .child(
                                icon(IconName::Redo2)
                                    .size(px(13.))
                                    .stroke_width(2.2)
                                    .color(hsla(t.on_amber)),
                            )
                            .child(
                                recur_ui::style::button_label("Retry", 12., 600.).color(t.on_amber),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                    )),
            );
        }
        card.into_any_element()
    }

    // ---- facts and sessions ----

    fn fact_block(
        &mut self,
        m: &AgentMsg,
        msgs: &[AgentMsg],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let Some(ChatLog::Fact {
            id,
            text: html,
            is_deviation,
            code,
            name,
            card,
            data,
        }) = m.log.as_deref()
        else {
            return div().into_any_element();
        };
        let code = if code.is_empty() {
            "MA".to_string()
        } else {
            code.clone()
        };
        let you = code == "AR";
        let start = cockpit_start_ts(msgs);
        let ctx = PrCompletionCtx {
            prs: self
                .state
                .read(cx)
                .prs
                .iter()
                .map(|p| PrRef {
                    id: p.id.clone(),
                    number: p.number,
                    workspace_id: p.workspace_id.clone(),
                })
                .collect(),
            workspace_id: self.props.workspace_id.clone(),
            fact_ts: m.ts.unwrap_or(0),
            start_ts: (start > 0).then_some(start),
        };
        let pr_done = loop_pr_completion(html, Some(&ctx));
        let wrap = loop_completion_wrap(html);
        let is_card = !card.is_empty();
        let mut el = div()
            .when(is_card, |d| d.my(px(6.)))
            .when(!is_card, |d| d.my(px(2.)))
            .rounded(px(7.));
        if card == "fanout" && !data.is_empty() {
            return el
                .child(self.fanout_card(id, data, html, window, cx))
                .into_any_element();
        }
        if card == "plan" && !data.is_empty() {
            return el
                .child(self.plan_card(id, data, html, window, cx))
                .into_any_element();
        }
        let fid = SharedString::from(format!(
            "fact-{}-{}",
            self.props.reset_key,
            if id.is_empty() {
                m.ts.unwrap_or(0).to_string()
            } else {
                id.clone()
            }
        ));
        let tint = if you {
            t.text
        } else {
            ag_color(&code, &t).unwrap_or(t.text)
        };
        let meta: AnyElement = match &pr_done {
            Some(done) => {
                let mut row = div()
                    .flex()
                    .flex_wrap()
                    .items_baseline()
                    .gap_x(px(6.))
                    .text_size(px(11.))
                    .font_family(recur_ui::fonts::MONO)
                    .text_color(hsla(t.faint));
                row = row.child(
                    text(if done.duration.is_empty() {
                        "Completed".to_string()
                    } else {
                        format!("Completed in {}", done.duration)
                    })
                    .nowrap(),
                );
                row = row.child(div().opacity(0.55).child(text("·").nowrap()));
                row = match done.pr_id.clone() {
                    Some(pid) => row.child(
                        div()
                            .id(SharedString::from(format!("{fid}-pr")))
                            .cursor_pointer()
                            .text_color(hsla(t.cyan))
                            .hover(|s| s.text_color(hsla(t.text)))
                            .child(text(done.pr_label.clone()).nowrap())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |s, cx| s.open_pr(&pid, cx))
                            })),
                    ),
                    None => row.child(text(done.pr_label.clone()).nowrap()),
                };
                row.into_any_element()
            }
            None => {
                let mut st =
                    InlineStyle::meta(11., if *is_deviation { t.amber } else { t.faint }, &t);
                st.family = Family::Mono;
                st.pre_wrap = true;
                if *is_deviation {
                    st.code_fg = t.amber;
                    st.code_bg = t.amber_d;
                }
                inline_html(
                    SharedString::from(format!("{fid}-meta")),
                    &strip_loop_wrap(html),
                    st,
                )
            }
        };
        let head_kids = vec![
            div()
                .relative()
                .flex_none()
                .size(px(14.))
                .top(px(1.))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    icon(if you {
                        IconName::User
                    } else {
                        IconName::Activity
                    })
                    .size(px(14.))
                    .color(hsla(t.muted)),
                )
                .into_any_element(),
            div()
                .flex_none()
                .max_w(relative(0.55))
                .overflow_hidden()
                .child(
                    text(if name.is_empty() {
                        code.clone()
                    } else {
                        name.clone()
                    })
                    .weight(600.)
                    .color(tint)
                    .truncate(),
                )
                .into_any_element(),
            div().flex_1().min_w_0().child(meta).into_any_element(),
        ];
        el = el.child(
            div()
                .flex()
                .items_baseline()
                .gap(px(7.))
                .w_full()
                .min_w_0()
                .pt(px(3.))
                .pb(px(3.))
                .pl(px(2.))
                .pr(px(6.))
                .rounded(px(6.))
                .text_size(px(12.))
                .text_color(hsla(t.text))
                .children(head_kids),
        );
        if !wrap.is_empty() {
            let mut st = InlineStyle::meta(12.5, t.text, &t);
            st.leading = Some(1.45);
            st.bold_weight = 600.;
            st.bold_color = Some(t.muted);
            el = el.child(div().ml(px(26.)).mr(px(6.)).mb(px(4.)).child(inline_html(
                SharedString::from(format!("{fid}-wrap")),
                &wrap,
                st,
            )));
        }
        el.into_any_element()
    }

    fn sessions_block(
        &mut self,
        sessions: &[recur_engine::view::chat::ChatLogSession],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let Some(render) = self.session_card.clone() else {
            return div().into_any_element();
        };
        recur_ui::style::card(t.pane, t.line, 8.)
            .overflow_hidden()
            .my(px(6.))
            .children(sessions.iter().map(|s| render(s, window, cx)))
            .into_any_element()
    }

    // ---- the turn footer ----

    fn turn_end_block(
        &mut self,
        msgs: &[AgentMsg],
        i: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let m = msgs[i].clone();
        let digest = self.turn_digest_visible(msgs, i);
        let data = self.tool_data.clone();
        let presented = last_presented_in_turn(msgs, i, data.cx());
        let reply = last_assistant_text_in_turn(msgs, i).cloned();
        let copy_key = format!(
            "turn-{}",
            if m.message_id.is_empty() {
                i.to_string()
            } else {
                m.message_id.clone()
            }
        );
        let can_ckpt = !self.props.readonly && !m.message_id.is_empty() && m.checkpoint;
        let id = SharedString::from(format!("te-{}-{i}", self.props.reset_key));
        let link = |id: SharedString,
                    s: String,
                    cx: &mut Context<Self>,
                    f: Box<dyn Fn(&mut Self, &mut Context<Self>)>|
         -> AnyElement {
            let t = Theme::of(cx).tokens;
            let sel = format!("turn-end-link:{s}");
            div()
                .id(id)
                .debug_selector(|| sel)
                .cursor_pointer()
                .text_color(hsla(t.cyan))
                .hover(|s| s.text_color(hsla(t.text)))
                .child(text(s).nowrap())
                .on_click(cx.listener(move |this, _, _, cx| f(this, cx)))
                .into_any_element()
        };
        let sep = || {
            div()
                .opacity(0.55)
                .child(text("·").nowrap())
                .into_any_element()
        };
        let mut row = div()
            .debug_selector(|| "turn-end".into())
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(6.))
            .mt(px(2.))
            .text_size(px(12.))
            .line_height(px(20.))
            .text_color(hsla(t.muted))
            .child(
                text(format!("completed in {}", format_duration(m.duration_ms)))
                    .tabular()
                    .nowrap(),
            );
        let presented_ok = presented
            .as_ref()
            .filter(|p| p.ok && (p.brief.is_some() || !p.target.is_empty()))
            .cloned();
        if let Some(p) = &presented_ok {
            let target = p
                .brief
                .as_ref()
                .map(|b| b.primary.target.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| p.target.clone());
            let title = p
                .brief
                .as_ref()
                .map(|b| b.title.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| p.target.clone());
            row = row.child(sep()).child(link(
                SharedString::from(format!("{id}-open")),
                format!("Open {title}"),
                cx,
                Box::new(move |this, cx| {
                    let target = target.clone();
                    this.state.update(cx, |s, cx| s.route_target(&target, cx));
                }),
            ));
        }
        if !digest.is_empty() {
            let first = digest
                .iter()
                .find(|d| !d.id.is_empty())
                .map(|d| d.id.clone())
                .unwrap_or_default();
            row = row.child(sep()).child(link(
                SharedString::from(format!("{id}-digest")),
                format!(
                    "{} finding{}",
                    digest.len(),
                    if digest.len() == 1 { "" } else { "s" }
                ),
                cx,
                Box::new(move |this, cx| this.jump_to_finding(&first, false, cx)),
            ));
        }
        let acts_shown = reply.is_some()
            || (!self.props.readonly
                && (self.hooks.fork || self.hooks.reset)
                && !m.message_id.is_empty())
            || can_ckpt;
        if acts_shown {
            let mut acts = div().flex().items_center().ml(px(2.));
            if let Some(reply) = &reply {
                let copied = self.copied_bubble.as_deref() == Some(copy_key.as_str());
                let body = strip_attachment_markers(&reply.body);
                let key = copy_key.clone();
                acts = acts.child(self.turn_act(
                    SharedString::from(format!("{id}-copy")),
                    "Copy reply",
                    if copied {
                        IconName::Check
                    } else {
                        IconName::Copy
                    },
                    copied.then_some(2.4),
                    copied,
                    false,
                    cx,
                    Box::new(move |this, _, cx| {
                        if body.is_empty() {
                            return;
                        }
                        this.copy_text(body.clone(), cx);
                        this.mark_copied(None, Some(key.clone()), cx);
                    }),
                ));
            }
            if !self.props.readonly && self.hooks.fork && !m.message_id.is_empty() {
                let mid = m.message_id.clone();
                let on = self.fork_menu.as_ref().is_some_and(|f| f.message_id == mid);
                let anchor = self.fork_anchor.clone();
                let aid = mid.clone();
                acts = acts.child(
                    div()
                        .relative()
                        .flex()
                        .debug_selector(|| "turn-end-fork".into())
                        .child(self.turn_act(
                            SharedString::from(format!("{id}-fork")),
                            "Fork",
                            IconName::Split,
                            None,
                            false,
                            on,
                            cx,
                            Box::new(move |this, window, cx| {
                                let b = anchor.borrow().get(&aid).copied().unwrap_or_default();
                                this.toggle_fork_menu(&aid, b, window, cx);
                            }),
                        ))
                        .child({
                            let rec = self.fork_anchor.clone();
                            canvas(
                                move |b, _, _| {
                                    rec.borrow_mut().insert(mid.clone(), b);
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .inset_0()
                        }),
                );
            }
            if can_ckpt && self.hooks.turn_diff {
                let mid = m.message_id.clone();
                acts = acts.child(self.turn_act(
                    SharedString::from(format!("{id}-diff")),
                    "Changes this turn",
                    IconName::FileDiff,
                    None,
                    false,
                    false,
                    cx,
                    Box::new(move |_, _, cx| cx.emit(ChatEvent::TurnDiff(mid.clone()))),
                ));
            }
            if !self.props.readonly
                && self.hooks.reset
                && !m.message_id.is_empty()
                && i + 1 < msgs.len()
            {
                let mid = m.message_id.clone();
                let restores = msgs[i + 1..].iter().any(|x| x.turn_end && x.checkpoint);
                acts = acts.child(self.turn_act(
                    SharedString::from(format!("{id}-reset")),
                    "Reset to here",
                    IconName::Undo,
                    None,
                    false,
                    false,
                    cx,
                    Box::new(move |_, _, cx| {
                        cx.emit(ChatEvent::Reset {
                            message_id: mid.clone(),
                            restores_files: restores,
                        })
                    }),
                ));
            }
            row = row.child(acts);
        }
        let mut wrap = div().mt(px(2.)).mb(px(12.)).child(row);
        if let Some(p) = &presented_ok {
            let card = self.present_workspace_card(p, &format!("{id}-brief"), window, cx);
            wrap = wrap.child(div().mt(px(8.)).child(card));
        }
        if !digest.is_empty() {
            let list = self.turn_digest(&digest, &id, cx);
            wrap = wrap.child(
                recur_ui::style::card(t.pane, t.line, 6.)
                    .debug_selector(|| "findings-filed".into())
                    .mt(px(8.))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(list),
            );
        }
        wrap.into_any_element()
    }

    /// `.turn-end-act`: a 20px icon button.
    #[allow(clippy::too_many_arguments)]
    fn turn_act(
        &self,
        id: SharedString,
        tip: &str,
        name: IconName,
        stroke: Option<f32>,
        copied: bool,
        on: bool,
        cx: &mut Context<Self>,
        f: Box<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let mut ic = icon(name).size(px(14.));
        if let Some(w) = stroke {
            ic = ic.stroke_width(w);
        }
        recur_ui::tooltip::titled(
            SharedString::from(format!("{id}-tip")),
            tip.to_string(),
            div()
                .id(id)
                .size(px(20.))
                .rounded(px(4.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(hsla(if copied {
                    GREEN
                } else if on {
                    t.text
                } else {
                    t.faint
                }))
                .when(on, |d| d.bg(hsla(t.pane_2)))
                .hover(move |s| {
                    s.bg(hsla(t.pane_2))
                        .text_color(hsla(if copied { GREEN } else { t.text }))
                })
                .child(ic)
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    f(this, window, cx)
                })),
        )
        .into_any_element()
    }

    /// The turn digest accordion (same rows as `list_findings`).
    fn turn_digest(
        &mut self,
        digest: &[TurnDigestFinding],
        id: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let n = digest.len();
        let rows: Vec<AnyElement> = digest
            .iter()
            .enumerate()
            .map(|(fi, f)| {
                let loc = crate::findings_ui::finding_loc_suffix(f.line, f.end_line);
                let loc_label = crate::findings_ui::finding_loc_label(&f.file, f.line, f.end_line);
                let title = crate::findings_ui::finding_title(if f.message.is_empty() {
                    &f.text
                } else {
                    &f.message
                });
                let show_body = !f.text.is_empty()
                    && (f.text != recur_engine::view::js::trim(&f.message)
                        || crate::findings_ui::finding_body_needed(&f.text));
                let accord = if f.id.is_empty() {
                    format!("digest-{fi}")
                } else {
                    f.id.clone()
                };
                let noting = !f.id.is_empty() && self.digest_comment_for == f.id;
                let open = self
                    .finding_accord_open
                    .get(&accord)
                    .copied()
                    .unwrap_or(false)
                    || noting;
                let status_busy =
                    self.digest_status_busy == f.id || self.digest_comment_busy == f.id;
                let notes = self.notes_for_digest(f);
                let loc_tip = loc_for_digest(&f.file, f.line, f.end_line);
                let head = self.accord_head(
                    &accord,
                    open,
                    &f.severity,
                    &title,
                    &loc_label,
                    Some(loc_tip.clone()),
                    div().into_any_element(),
                    &format!("{id}-dh-{fi}"),
                    cx,
                );
                let mut row = div().child(head);
                if open {
                    let mut detail = accord_detail(&t);
                    if !f.file.is_empty() {
                        let el = if self.hooks.open_file {
                            let shown = if loc_label.is_empty() {
                                format!("{}{loc}", base_name(&f.file))
                            } else {
                                loc_label.clone()
                            };
                            let line = (f.line > 0).then_some(f.line);
                            let end = (f.end_line > f.line).then_some(f.end_line);
                            self.loc_button(
                                shown,
                                f.file.clone(),
                                line,
                                end,
                                &format!("{id}-df-{fi}"),
                                cx,
                            )
                        } else {
                            label(loc_tip.clone(), Family::Mono, 11., 400.)
                                .color(t.cyan)
                                .into_any_element()
                        };
                        detail = detail.child(div().line_height(relative(1.2)).child(el));
                    }
                    if show_body {
                        let md = self.md(format!("{id}-dm-{fi}"), &f.text, None, false, cx);
                        detail = detail.child(md);
                    }
                    if !notes.is_empty() {
                        let mut col = div().flex().flex_col().gap(px(2.));
                        for (ni, note) in notes.iter().enumerate() {
                            let mut nrow = div()
                                .id(SharedString::from(format!("{id}-dn-{fi}-{ni}")))
                                .group(SharedString::from(format!("{id}-dng-{fi}-{ni}")))
                                .relative()
                                .pt(px(5.))
                                .pb(px(5.))
                                .pl(px(8.))
                                .pr(px(28.))
                                .rounded(px(6.))
                                .hover(|s| s.bg(hsla(t.pane_2)))
                                .child(labeled_note(
                                    "Note",
                                    &note.body,
                                    12.,
                                    1.45,
                                    (0., 0., 0., 0.),
                                    &t,
                                ));
                            if !f.id.is_empty() && !note.id.is_empty() {
                                let (fid, nid) = (f.id.clone(), note.id.clone());
                                let busy = self.digest_comment_busy == f.id;
                                let g = SharedString::from(format!("{id}-dng-{fi}-{ni}"));
                                nrow = nrow.child(
                                    div()
                                        .absolute()
                                        .right(px(4.))
                                        .top_0()
                                        .bottom_0()
                                        .flex()
                                        .items_center()
                                        .child(recur_ui::tooltip::titled(
                                            SharedString::from(format!("{id}-dd-{fi}-{ni}-tip")),
                                            "Delete note",
                                            div()
                                                .id(SharedString::from(format!(
                                                    "{id}-dd-{fi}-{ni}"
                                                )))
                                                .size(px(22.))
                                                .rounded(px(5.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_color(hsla(t.faint))
                                                .opacity(0.)
                                                .group_hover(g, move |s| {
                                                    s.opacity(if busy { 0.4 } else { 1. })
                                                })
                                                .when(!busy, |d| {
                                                    d.cursor_pointer().hover(|s| {
                                                        s.text_color(hsla(t.coral)).bg(hsla(
                                                            t.pane.mix(
                                                                0.7,
                                                                recur_ui::color::TRANSPARENT,
                                                            ),
                                                        ))
                                                    })
                                                })
                                                .child(icon(IconName::Trash).size(px(13.)))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if !busy {
                                                        this.delete_digest_note(&fid, &nid, cx);
                                                    }
                                                })),
                                        )),
                                );
                            }
                            col = col.child(nrow);
                        }
                        detail = detail.child(col);
                    }
                    if !f.id.is_empty() && !noting {
                        detail = detail.child(self.digest_actions(
                            &f.id,
                            status_busy,
                            &format!("{id}-da-{fi}"),
                            cx,
                        ));
                    }
                    if noting {
                        detail = detail.child(self.digest_compose(&f.id, cx));
                    }
                    row = row.child(detail);
                }
                finding_row_wrap(row.into_any_element(), fi + 1 == n, &t)
            })
            .collect();
        findings_list(SharedString::from(format!("{id}-digest-l")), rows, f32::MAX)
    }

    fn digest_actions(
        &self,
        fid: &str,
        busy: bool,
        idk: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let act = |id: String,
                   ic: IconName,
                   s: &str,
                   color: gpui_kit::Hsla,
                   hover: gpui_kit::Hsla,
                   f: Box<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)>,
                   cx: &mut Context<Self>| {
            div()
                .id(SharedString::from(id))
                .flex()
                .items_center()
                .gap(px(5.))
                .text_size(px(11.5))
                .text_color(color)
                .when(busy, |d| d.opacity(0.45))
                .when(!busy, |d| {
                    d.cursor_pointer().hover(move |s| s.text_color(hover))
                })
                .child(icon(ic).size(px(13.)))
                .child(text(s.to_string()).line_height(LineHeight::Normal).nowrap())
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !busy {
                        f(this, window, cx)
                    }
                }))
        };
        let (a, b, c) = (fid.to_string(), fid.to_string(), fid.to_string());
        div()
            .flex()
            .flex_wrap()
            .gap(px(12.))
            .items_center()
            .child(act(
                format!("{idk}-i"),
                IconName::EyeOff,
                "Ignore",
                hsla(t.muted),
                hsla(t.text),
                Box::new(move |this, _, cx| this.set_digest_finding_status(&a, "ignored", cx)),
                cx,
            ))
            .child(act(
                format!("{idk}-v"),
                IconName::BugOff,
                "Mark invalid",
                hsla(t.coral),
                hsla(t.coral.mix(0.8, t.text)),
                Box::new(move |this, _, cx| this.set_digest_finding_status(&b, "invalid", cx)),
                cx,
            ))
            .child(act(
                format!("{idk}-n"),
                IconName::NotebookPen,
                "Add note",
                hsla(t.cyan),
                hsla(t.text),
                Box::new(move |this, window, cx| this.open_digest_compose(&c, window, cx)),
                cx,
            ))
            .into_any_element()
    }

    /// `.turn-finding-compose`.
    fn digest_compose(&self, fid: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let busy = self.digest_comment_busy == fid;
        let Some(input) = self.digest_comment_input.clone() else {
            return div().into_any_element();
        };
        let empty = recur_engine::view::js::trim(&input.read(cx).value()).is_empty();
        let (f1, f2) = (fid.to_string(), fid.to_string());
        let btn = |id: &'static str, s: &'static str, primary: bool, disabled: bool| {
            let base = div()
                .id(id)
                .h(px(24.))
                .px(px(8.))
                .rounded(px(6.))
                .border_1()
                .border_color(hsla(if primary { t.line_bright } else { t.line }))
                .bg(hsla(if primary { t.pane } else { t.pane_2 }))
                .flex()
                .items_center()
                .text_size(px(11.))
                .text_color(hsla(if primary { t.text } else { t.muted }))
                .child(text(s).line_height(LineHeight::Normal).nowrap());
            if disabled {
                base.opacity(0.4)
            } else {
                base.cursor_pointer().hover(move |s| {
                    if primary {
                        s.border_color(hsla(t.muted))
                    } else {
                        s.text_color(hsla(t.text)).border_color(hsla(t.line_bright))
                    }
                })
            }
        };
        div()
            .flex()
            .flex_col()
            .bg(hsla(t.bg))
            .border_1()
            .border_color(hsla(t.line))
            .rounded(px(10.))
            .overflow_hidden()
            .child(
                div()
                    .id("digest-note-input")
                    .min_h(px(40.))
                    .max_h(px(120.))
                    .pt(px(8.))
                    .px(px(10.))
                    .pb(px(4.))
                    .text_size(px(12.5))
                    .line_height(relative(1.5))
                    .text_color(hsla(t.text))
                    .when(busy, |d| d.opacity(0.55))
                    .on_key_down(
                        cx.listener(move |this, ev: &gpui_kit::KeyDownEvent, _, cx| {
                            let k = &ev.keystroke;
                            if k.key == "enter" && (k.modifiers.platform || k.modifiers.control) {
                                cx.stop_propagation();
                                this.submit_digest_comment(&f1, cx);
                            } else if k.key == "escape" {
                                cx.stop_propagation();
                                this.cancel_digest_compose(cx);
                            }
                        }),
                    )
                    .child(gpui_kit::base::input::Textarea::new(&input)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(2.))
                    .px(px(8.))
                    .pb(px(8.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .text_size(px(11.))
                            .text_color(hsla(t.faint))
                            .child(recur_ui::kbd::kbd("⌘↵").size(11.).color(t.faint))
                            .child(text("to save").nowrap()),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(btn("digest-cancel", "Cancel", false, busy).on_click(
                                cx.listener(|this, _, _, cx| this.cancel_digest_compose(cx)),
                            ))
                            .child(btn("digest-save", "Save", true, busy || empty).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.submit_digest_comment(&f2, cx)
                                }),
                            )),
                    ),
            )
            .into_any_element()
    }

    // ---- steps ----

    #[allow(clippy::too_many_arguments)]
    fn steps_block(
        &mut self,
        msgs: &[AgentMsg],
        start: usize,
        rows: &[usize],
        is_last: bool,
        first_checkpoint: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let mut el = div().mt(px(4.)).mb(px(16.));
        if rows.len() == 1 {
            let r = rows[0];
            return el
                .child(self.step_row(
                    &msgs[r].clone(),
                    &r.to_string(),
                    first_checkpoint,
                    window,
                    cx,
                ))
                .into_any_element();
        }
        let open = crate::glance::live_group_open(
            self.open_groups.get(&start).copied(),
            is_last && first_checkpoint,
        );
        let tail = (!open && self.props.sending && is_last).then(|| *rows.last().expect("rows"));
        let head_rows: Vec<usize> = match tail {
            Some(_) => rows[..rows.len() - 1].to_vec(),
            None => rows.to_vec(),
        };
        let data = self.tool_data.clone();
        let summary = group_summary(msgs, &head_rows, rows, data.cx());
        // `.steps-head` is inline-flex on a 13 / 1.45 line; its baseline is the centered 12px
        // chevron's bottom edge.
        let line = 11.5 * 1.45;
        el = el.child(crate::style::line_box(
            div()
                .id(SharedString::from(format!(
                    "steps-{}-{start}",
                    self.props.reset_key
                )))
                .debug_selector(|| format!("steps-head:{summary}"))
                .flex()
                .items_center()
                .gap(px(6.))
                .py(px(2.))
                .cursor_pointer()
                .text_size(px(11.5))
                .text_color(hsla(t.muted))
                .hover(|s| s.text_color(hsla(t.text)))
                .child(
                    div().flex_none().size(px(12.)).child(
                        icon(IconName::ChevronRight)
                            .size(px(12.))
                            .stroke_width(2.4)
                            .color(hsla(t.faint))
                            .rotate(if open { 90. } else { 0. }),
                    ),
                )
                .child(text(summary).line_height(LineHeight::Scale(1.45)).nowrap())
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_group(start, open, cx))),
            13.,
            1.45,
            2. + (line - 12.) / 2. + 12.,
            line + 4.,
            window,
        ));
        let shown: Vec<usize> = if open {
            rows.to_vec()
        } else {
            tail.into_iter().collect()
        };
        if !shown.is_empty() {
            let kids: Vec<AnyElement> = shown
                .iter()
                .map(|&r| {
                    self.step_row(
                        &msgs[r].clone(),
                        &r.to_string(),
                        first_checkpoint,
                        window,
                        cx,
                    )
                })
                .collect();
            el = el.child(
                div()
                    .mt(px(4.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(kids),
            );
        }
        el.into_any_element()
    }

    // ---- user bubbles ----

    /// The bubble's inline content: text and chips in one flow, or a column of parts when
    /// the message carries images (`.has-att`).
    pub(crate) fn bubble_content(
        &mut self,
        parts: &[BubblePart],
        id: &str,
        has_att: bool,
        trailing_space: bool,
        color: recur_ui::Color,
        backdrop: recur_ui::Color,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let style = FlowStyle {
            family: Family::Ui,
            size: 12.5,
            weight: 400.,
            leading: 1.5,
            color: color.into(),
        };
        if has_att {
            let kids: Vec<AnyElement> = parts
                .iter()
                .enumerate()
                .map(|(pi, p)| match p {
                    BubblePart::Text(s) => flow(
                        SharedString::from(format!("{id}-p{pi}")),
                        s.clone(),
                        Vec::new(),
                        style.clone(),
                    )
                    .into_any_element(),
                    other => match self.bubble_atom(
                        other,
                        SharedString::from(format!("{id}-a{pi}")),
                        backdrop,
                        window,
                        cx,
                    ) {
                        Some(a) => div()
                            .flex()
                            .child(
                                div()
                                    .w(px(a.width))
                                    .h(px(a.height))
                                    .pr(px(2.))
                                    .py(px(2.))
                                    .child(a.el),
                            )
                            .into_any_element(),
                        None => div().into_any_element(),
                    },
                })
                .collect();
            return div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .children(kids)
                .into_any_element();
        }
        let mut content = String::new();
        let mut atoms = Vec::new();
        for (pi, p) in parts.iter().enumerate() {
            match p {
                BubblePart::Text(s) => content.push_str(s),
                other => {
                    if let Some(a) = self.bubble_atom(
                        other,
                        SharedString::from(format!("{id}-a{pi}")),
                        backdrop,
                        window,
                        cx,
                    ) {
                        content.push(ATOM);
                        atoms.push(crate::flow::Atom {
                            el: div()
                                .pt(px(2.))
                                .pr(px(2.))
                                .pb(px(2.))
                                .child(a.el)
                                .into_any_element(),
                            width: a.width,
                            height: a.height,
                        });
                    }
                }
            }
        }
        flow(
            SharedString::from(format!("{id}-flow")),
            content,
            atoms,
            style,
        )
        .fit_content()
        .trailing_space(trailing_space)
        .into_any_element()
    }

    fn user_bubble(
        &mut self,
        m: &AgentMsg,
        i: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let bubble_key = format!("m{i}");
        let plain = strip_attachment_markers(&m.body);
        let can_copy = !plain.is_empty();
        let id = format!("ub-{}-{i}", self.props.reset_key);
        let arts = self.live_artifacts(cx);
        let parts = bubble_parts(m, &|title| {
            recur_engine::view::mentions::unique_artifact_id_for_title(
                title,
                arts.iter().map(|a| (a.id.as_str(), a.title.as_str())),
            )
        });
        let has_att = !m.attachments.is_empty();
        let bg = t.amber_d;
        let content = self.bubble_content(
            &parts,
            &id,
            has_att,
            can_copy,
            t.text,
            bg.over(t.bg),
            window,
            cx,
        );
        let copied = self.copied_bubble.as_deref() == Some(bubble_key.as_str());
        let group = SharedString::from(format!("{id}-g"));
        let sel = format!("me:{}", debug_key(&plain));
        let mut bubble =
            recur_ui::style::card(bg, t.amber.mix(0.16, recur_ui::color::TRANSPARENT), 8.)
                .id(SharedString::from(id.clone()))
                .debug_selector(|| sel)
                .group(group.clone())
                .relative()
                .max_w(px(520.))
                .pt(px(9.))
                .pb(px(9.))
                .px(px(12.))
                .child(content);
        if can_copy {
            let copy_bg = t.pane.mix(0.88, t.amber);
            let copy_border = t.amber.mix(0.35, recur_ui::color::TRANSPARENT);
            let hover_bg = t.pane.mix(0.78, t.amber);
            let hover_border = t.amber.mix(0.5, recur_ui::color::TRANSPARENT);
            let key = bubble_key.clone();
            bubble = bubble.child(
                div()
                    .absolute()
                    .top(px(6.))
                    .right(px(6.))
                    .child(recur_ui::tooltip::titled(
                        SharedString::from(format!("{id}-copy-tip")),
                        "Copy message",
                        recur_ui::style::card(copy_bg, copy_border, 4.)
                            .id(SharedString::from(format!("{id}-copy")))
                            .debug_selector(|| "chat-bubble-copy".into())
                            .size(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_color(hsla(if copied { GREEN } else { t.muted }))
                            .when(!copied, |d| {
                                d.invisible().group_hover(group.clone(), |s| s.visible())
                            })
                            .hover(move |s| {
                                s.bg(hsla(hover_bg))
                                    .border_color(hsla(hover_border))
                                    .text_color(hsla(if copied { GREEN } else { t.text }))
                            })
                            .child(if copied {
                                icon(IconName::Check).size(px(13.)).stroke_width(2.4)
                            } else {
                                icon(IconName::Copy).size(px(13.))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.copy_text(plain.clone(), cx);
                                this.mark_copied(None, Some(key.clone()), cx);
                            })),
                    )),
            );
        }
        div()
            .flex()
            .justify_end()
            .my(px(22.))
            .child(
                div()
                    .max_w(relative(0.85))
                    .flex()
                    .justify_end()
                    .child(bubble),
            )
            .into_any_element()
    }

    /// One queued follow-up above the composer.
    pub(crate) fn queued_bubble(
        &mut self,
        m: &AgentMsg,
        qi: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let id = format!("q-{}-{qi}", self.props.reset_key);
        let arts = self.live_artifacts(cx);
        let parts = bubble_parts(m, &|title| {
            recur_engine::view::mentions::unique_artifact_id_for_title(
                title,
                arts.iter().map(|a| (a.id.as_str(), a.title.as_str())),
            )
        });
        let content = self.bubble_content(&parts, &id, false, false, t.muted, t.pane_2, window, cx);
        let mut head = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_h(px(16.))
            .child(
                label("Queued", Family::Mono, 9.5, 400.)
                    .tracking(0.08)
                    .uppercase()
                    .line_height(LineHeight::Scale(1.))
                    .color(t.faint),
            );
        if !m.pending_id.is_empty() && self.hooks.remove_queued {
            let pid = m.pending_id.clone();
            head = head.child(
                div().ml_auto().child(recur_ui::tooltip::titled(
                    SharedString::from(format!("{id}-x-tip")),
                    "Remove from queue",
                    div()
                        .id(SharedString::from(format!("{id}-x")))
                        .debug_selector(|| format!("queued-x:{}", debug_key(&m.body)))
                        .size(px(16.))
                        .rounded(px(4.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_color(hsla(t.faint))
                        .hover(|s| s.text_color(hsla(t.text)).bg(hsla(t.pane)))
                        .child(
                            text("×")
                                .size(13.)
                                .line_height(LineHeight::Scale(1.))
                                .nowrap(),
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(ChatEvent::RemoveQueued(pid.clone()))
                        })),
                )),
            );
        }
        let sel = format!("queued:{}", debug_key(&m.body));
        div()
            .debug_selector(|| sel)
            .flex()
            .justify_end()
            .max_w_full()
            .child(
                recur_ui::style::card(t.pane_2, t.line_bright, 8.)
                    .max_w(px(520.))
                    .pt(px(8.))
                    .pb(px(9.))
                    .px(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .text_color(hsla(t.muted))
                    .child(head)
                    .child(content),
            )
            .into_any_element()
    }
}

fn loc_for_digest(file: &str, line: i64, end_line: i64) -> String {
    if file.is_empty() {
        return String::new();
    }
    if line <= 0 {
        return file.to_string();
    }
    if end_line > line {
        return format!("{file}:{line}-{end_line}");
    }
    format!("{file}:{line}")
}
