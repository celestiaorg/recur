//! The cards the transcript embeds: permission and question prompts, plan and fan-out wave
//! cards, specialist sessions and workspace briefs.

use recur_ui::guard::Guarded;
use std::collections::HashMap;

use gpui_kit::{AnyElement, AppContext as _, Context, Entity, IntoElement, Window};
use recur_domain::{Escalation, PermissionRequest, UserQuestion};

use crate::chat::{AgentChat, ChatEvent};

pub mod brief;
pub mod fanout;
pub mod permission;
pub mod plan;
pub mod question;
pub mod session;

use fanout::FanOutCard;
use permission::PermissionCard;
use question::{QuestionAnswered, QuestionCard, QuestionLike};

/// Card entities kept across frames, keyed by what they show.
#[derive(Default)]
pub struct CardViews {
    permissions: HashMap<String, Entity<PermissionCard>>,
    questions: HashMap<String, Entity<QuestionCard>>,
    escalation: Option<(String, Entity<QuestionCard>)>,
    fanouts: HashMap<String, Entity<FanOutCard>>,
}

impl CardViews {
    /// Drops cards for requests and questions that are gone.
    pub(crate) fn retain(&mut self, perms: &[String], questions: &[String]) {
        self.permissions.retain(|k, _| perms.contains(k));
        self.questions.retain(|k, _| questions.contains(k));
    }
}

impl AgentChat {
    pub(crate) fn permission_card(
        &mut self,
        req: &PermissionRequest,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.clone();
        let card = self
            .cards
            .permissions
            .entry(req.id.clone())
            .or_insert_with(|| cx.new(|_| PermissionCard::new(req.clone(), true, state)))
            .clone();
        if card.read(cx).request != *req {
            let r = req.clone();
            card.update(cx, |c, cx| c.set_request(r, cx));
        }
        card.into_any_element()
    }

    pub(crate) fn question_card_for_user(
        &mut self,
        q: &UserQuestion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.clone();
        let like = QuestionLike {
            id: q.id.clone(),
            question: q.question.clone(),
            context: q.context.clone(),
            options: q.options.clone(),
        };
        self.cards
            .questions
            .entry(q.id.clone())
            .or_insert_with(|| cx.new(|cx| QuestionCard::new(like, None, false, state, window, cx)))
            .clone()
            .into_any_element()
    }

    pub(crate) fn question_card_for_escalation(
        &mut self,
        e: &Escalation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self
            .cards
            .escalation
            .as_ref()
            .is_none_or(|(id, _)| id != &e.id)
        {
            let state = self.state.clone();
            let like = QuestionLike {
                id: e.id.clone(),
                question: e.question.clone(),
                context: e.context.clone(),
                options: e.options.clone().unwrap_or_default(),
            };
            let card = cx.new(|cx| {
                QuestionCard::new(
                    like,
                    Some("Maestro needs a decision".into()),
                    true,
                    state,
                    window,
                    cx,
                )
            });
            let sub = cx.guarded_subscribe(&card, |_, _, a: &QuestionAnswered, cx| {
                cx.emit(ChatEvent::AnswerEscalation {
                    choice: a.choice.clone(),
                    text: a.text.clone(),
                })
            });
            self._subs.push(sub);
            self.cards.escalation = Some((e.id.clone(), card));
        }
        self.cards
            .escalation
            .as_ref()
            .map(|(_, c)| c.clone().into_any_element())
            .unwrap_or_else(|| gpui_kit::div().into_any_element())
    }

    pub(crate) fn fanout_card(
        &mut self,
        fact_id: &str,
        data: &str,
        fallback: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let vcx = recur_view_core::ViewContext {
            state: self.state.clone(),
            services: self.services.clone(),
        };
        let key = if fact_id.is_empty() {
            data.to_string()
        } else {
            fact_id.to_string()
        };
        let created = !self.cards.fanouts.contains_key(&key);
        let card = self
            .cards
            .fanouts
            .entry(key)
            .or_insert_with(|| {
                cx.new(|cx| FanOutCard::new(vcx, data.to_string(), fallback.to_string(), cx))
            })
            .clone();
        let fs = self.props.font_size;
        card.update(cx, |c, _| c.font_size = fs);
        if created {
            let sub = cx.guarded_subscribe(&card, |_, _, ev: &ChatEvent, cx| cx.emit(ev.clone()));
            self._subs.push(sub);
        } else {
            let (d, f) = (data.to_string(), fallback.to_string());
            card.update(cx, |c, cx| c.set_data(d, f, cx));
        }
        card.into_any_element()
    }
}
