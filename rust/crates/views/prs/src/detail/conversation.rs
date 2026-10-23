//! The Conversation pane: the rework banner, the description card, the Local PR summary and
//! verify cards, reviewer findings, the timeline (commit and review events, comments with
//! their line-note threads), then the settled notice or the checks card and the composer.

use std::sync::LazyLock;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Context, Div, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, StyledImage, Window, canvas,
    div, img, px,
};
use recur_domain::{Finding, Pr, PrComment, PrSource};
use recur_engine::view::comment_thread::{
    can_delete_pr_comment, can_edit_pr_comment, can_resolve_line_note, comment_source,
    is_timeline_root, line_thread_key, review_children, thread_replies,
};
use recur_forge::github::comment_permalink;
use recur_ui::fonts::Family;
use recur_ui::text::{InlineBox, LineHeight, Span, Text, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::{Color, IconName, Theme, Tokens, icon, rgba};
use recur_view_core::pr::{PrLifecycle, pr_icon_name, pr_lifecycle};
use recur_view_review::avatar::avatar;
use recur_view_review::comment_header::{ReviewDecision, comment_header_verb, review_decision};
use recur_view_review::comment_hunk::{
    ClipOpts, clip_comment_hunk, comment_hunk_path_label, parse_diff_hunk,
};
use recur_view_review::style::note_btn;
use regex::Regex;

use super::PrDetail;
use super::field::Field;
use super::html::html_spans;
use crate::style::{hsla, spin_deg};
use crate::ui::PrTab;

/// `.cmt .ch` and friends: 12.5px on the body's 1.45.
const CH_SIZE: f32 = 12.5;

fn ch_line() -> LineHeight {
    LineHeight::Scale(1.45)
}

/// What a compose box saves into.
#[derive(Clone)]
pub(crate) enum Compose {
    Desc,
    Comment(String),
    Reply(String),
}

impl PrDetail {
    /// `whenStamp(c, cls)`: the time, linked to GitHub when there is a permalink.
    pub(crate) fn when_stamp(
        &self,
        c: &PrComment,
        pr_url: &str,
        tl: bool,
        cx: &App,
    ) -> Option<AnyElement> {
        if c.time.is_empty() {
            return None;
        }
        let t = Theme::of(cx).tokens;
        let href = comment_permalink(pr_url, c);
        let label = text(c.time.clone())
            .mono()
            .size(9.5)
            .line_height(ch_line())
            .color(t.faint)
            .nowrap();
        let _ = tl;
        if href.is_empty() {
            return Some(div().flex_none().child(label).into_any_element());
        }
        Some(
            titled(
                SharedString::from(format!("when-{}", c.id)),
                "Open on GitHub",
                div()
                    .id(SharedString::from(format!("when-a-{}", c.id)))
                    .debug_selector({
                        let id = c.id.clone();
                        move || format!("when:{id}")
                    })
                    .flex_none()
                    .cursor_pointer()
                    .text_color(hsla(t.faint))
                    .hover(move |s| s.text_color(hsla(t.muted)))
                    .child(label)
                    .on_click(move |_, _, cx| recur_content::links::open_external_url(&href, cx)),
            )
            .into_any_element(),
        )
    }

    /// `hasMoreActions`.
    fn has_more(&self, c: &PrComment, can_edit: bool, cx: &App) -> bool {
        if can_edit {
            return true;
        }
        let pr = self.pr(cx);
        if can_delete_pr_comment(pr.as_ref(), Some(c)) {
            return true;
        }
        if !comment_source(Some(c)).is_empty() {
            return true;
        }
        !comment_permalink(pr.as_ref().map_or("", |p| p.url.as_str()), c).is_empty()
    }

    /// `.cmt-more`: the ⋯ trigger for `id`.
    fn more_btn(&mut self, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let open = self.more.id == id;
        let me = cx.entity().downgrade();
        let probe_id = id.to_string();
        let probe = canvas(
            move |b, _, cx| {
                let _ = me.update(cx, |d, _| {
                    d.more.triggers.insert(probe_id.clone(), b);
                    if d.more.id == probe_id {
                        d.more.trigger = Some(b);
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let sid = id.to_string();
        let tid = id.to_string();
        div()
            .flex()
            .flex_none()
            .ml_auto()
            .child(titled(
                SharedString::from(format!("more-t-{id}")),
                "More",
                super::render::cmt_act(format!("more-{id}"), IconName::Ellipsis, open, cx)
                    .debug_selector(move || format!("cmt-more:{sid}"))
                    .child(probe)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |d, _, _, cx| {
                        cx.stop_propagation();
                        d.toggle_more(&tid, cx)
                    })),
            ))
            .into_any_element()
    }

    /// `.finding-note-compose.cmt-edit`: the textarea, the ⌘↵ hint, Cancel and the primary.
    pub(crate) fn compose_box(
        &mut self,
        which: Compose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let (field, key, primary, saving_key, grows): (&Field, String, &str, String, bool) =
            match &which {
                Compose::Desc => (&self.desc, "desc".into(), "Save", "desc".into(), false),
                Compose::Comment(id) => (
                    &self.comment,
                    format!("edit-{id}"),
                    "Save",
                    id.clone(),
                    false,
                ),
                Compose::Reply(id) => (
                    &self.reply,
                    format!("reply-{id}"),
                    "Reply",
                    format!("reply-{id}"),
                    true,
                ),
            };
        let area = field.area.clone();
        let value = area.read(cx).value().to_string();
        let empty = value.trim().is_empty();
        let saving = self.saving == saving_key;
        let primary_disabled = match &which {
            Compose::Desc => saving,
            _ => empty || saving,
        };
        let lh = recur_ui::style::button_label("x", 11., 400.)
            .measure(None, window)
            .2
            .height;
        let backdrop = t.bg;
        let w1 = which.clone();
        let w2 = which.clone();
        let w3 = which.clone();
        let input = div()
            .id(SharedString::from(format!("{key}-input")))
            .debug_selector({
                let key = key.clone();
                move || format!("finding-note-input:{key}")
            })
            .when(grows, |d| d.min_h(px(40.)).max_h(px(120.)))
            .when(!grows, |d| d.h(px(49.5)))
            .overflow_hidden()
            .pt(px(8.))
            .px(px(10.))
            .pb(px(4.))
            .text_size(px(12.5))
            .line_height(px(18.75))
            .text_color(hsla(t.text))
            .when(saving, |d| d.opacity(0.55))
            .key_context("TextEntry")
            .capture_key_down(cx.listener(move |d, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    d.cancel_compose(&w1, cx);
                }
            }))
            .child(gpui_kit::base::input::Textarea::new(&area));
        let hint = div()
            .flex()
            .items_center()
            .gap(px(5.))
            .text_color(hsla(t.faint))
            .child(recur_ui::kbd::kbd("⌘↵").size(11.).color(t.faint))
            .child(text("to save").size(11.).nowrap());
        let cancel = note_btn(
            SharedString::from(format!("{key}-cancel")),
            "Cancel",
            false,
            saving,
            backdrop,
            lh,
            cx,
        )
        .debug_selector(|| "finding-note-btn:Cancel".into())
        .when(!saving, |b| {
            b.on_click(cx.listener(move |d, _, _, cx| d.cancel_compose(&w2, cx)))
        });
        let label = if saving { "Saving…" } else { primary };
        let go = note_btn(
            SharedString::from(format!("{key}-go")),
            label,
            true,
            primary_disabled,
            backdrop,
            lh,
            cx,
        )
        .debug_selector(move || format!("finding-note-btn-primary:{primary}"))
        .when(!primary_disabled, |b| {
            b.on_click(cx.listener(move |d, _, window, cx| d.save_compose(&w3, window, cx)))
        });
        recur_ui::style::card(t.bg, t.line, 10.)
            .mt(px(6.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(input)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(2.))
                    .px(px(8.))
                    .pb(px(8.))
                    .child(hint)
                    .child(
                        div()
                            .ml_auto()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(cancel)
                            .child(go),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn save_compose(&mut self, which: &Compose, window: &mut Window, cx: &mut Context<Self>) {
        match which {
            Compose::Desc => self.save_desc(cx),
            Compose::Comment(id) => self.save_comment(id, cx),
            Compose::Reply(id) => self.submit_reply(id, window, cx),
        }
    }

    fn cancel_compose(&mut self, which: &Compose, cx: &mut Context<Self>) {
        match which {
            Compose::Desc => self.editing_desc = false,
            Compose::Comment(_) => self.editing_comment.clear(),
            Compose::Reply(_) => self.reply_for.clear(),
        }
        cx.notify();
    }

    /// `commentBody(c, locShown)`.
    fn comment_body(
        &mut self,
        c: &PrComment,
        loc_shown: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let t = Theme::of(cx).tokens;
        let mut out = Vec::new();
        if self.editing_comment == c.id {
            out.push(self.compose_box(Compose::Comment(c.id.clone()), window, cx));
            return out;
        }
        if !c.body_md.is_empty() {
            if !loc_shown && !c.body_html.is_empty() {
                out.push(loc_line(&c.body_html, &t).into_any_element());
            }
            let md = self.md(&c.id, &c.body_md, cx);
            out.push(
                div()
                    .debug_selector({
                        let id = c.id.clone();
                        move || format!("cm-md:{id}")
                    })
                    .min_w_0()
                    .text_color(hsla(t.text))
                    .child(md)
                    .into_any_element(),
            );
        } else if !c.body_html.is_empty() && !loc_shown {
            out.push(cm_html(&c.body_html, &t).into_any_element());
        }
        if !c.react_html.is_empty() {
            out.push(react(&c.react_html, &t).into_any_element());
        }
        let r = self.reactions(c, cx);
        out.push(r.into_any_element());
        out
    }

    /// `personAvatar(c)`: the 28px `.cmt .ca`.
    fn person_avatar(c: &PrComment, cx: &App) -> Div {
        avatar(&c.code, &c.avatar_css, &c.avatar_url, 28., 6., 10., cx)
    }

    /// The description card.
    fn description_card(
        &mut self,
        d: &PrComment,
        pr: &Pr,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let more_id = if d.id.is_empty() {
            "gh-description".to_string()
        } else {
            d.id.clone()
        };
        let can_edit = self.can_edit_pr(cx);
        let mut ch = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .mb(px(5.))
            .text_color(hsla(t.muted))
            .child(
                text(d.author.clone())
                    .size(CH_SIZE)
                    .weight(600.)
                    .line_height(ch_line())
                    .color(t.text)
                    .nowrap()
                    .id("desc-cn"),
            )
            .debug_selector({
                let a = d.author.clone();
                move || format!("desc-cn={a}")
            });
        if let Some(w) = self.when_stamp(d, &pr.url, false, cx) {
            ch = ch.child(w);
        }
        if !self.editing_desc && self.has_more(d, can_edit, cx) {
            ch = ch.child(self.more_btn(&more_id, cx));
        }
        let mut cc = div().flex_1().min_w_0().child(ch);
        if self.editing_desc {
            cc = cc.child(self.compose_box(Compose::Desc, window, cx));
        } else if !d.body_md.is_empty() {
            let md = self.md(&format!("desc:{}", d.id), &d.body_md, cx);
            cc = cc.child(div().min_w_0().text_color(hsla(t.text)).child(md));
        } else {
            cc = cc.child(
                text("No description provided.")
                    .size(12.5)
                    .leading(1.55)
                    .color(t.faint)
                    .id("desc-empty"),
            );
        }
        if !self.editing_desc {
            let r = self.reactions(d, cx);
            cc = cc.child(r);
        }
        recur_ui::style::card(t.pane, t.line, 9.)
            .debug_selector(|| "cmt-desc".into())
            .flex()
            .gap(px(12.))
            .py(px(14.))
            .px(px(15.))
            .mb(px(8.))
            .min_w_0()
            .child(Self::person_avatar(d, cx))
            .child(cc)
            .into_any_element()
    }

    /// The `.card` for a Local PR's Maestro summary and focus callout.
    fn summary_card(&mut self, pr: &Pr, desc: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let mut body = card_b();
        if !desc {
            let paras = pr.summary.clone().unwrap_or_default();
            let n = paras.len();
            for (i, para) in paras.iter().enumerate() {
                let last = i + 1 == n;
                if para.kind == "bullets" {
                    let mut ul = div().pl(px(18.)).when(!last, |d| d.mb(px(10.)));
                    for it in &para.items {
                        ul = ul.child(
                            div()
                                .relative()
                                .my(px(3.))
                                .child(
                                    div()
                                        .absolute()
                                        .left(px(-18.))
                                        .w(px(18.))
                                        .flex()
                                        .justify_end()
                                        .pr(px(7.))
                                        .child(text("•").size(12.5).leading(1.6).color(t.muted)),
                                )
                                .child(
                                    rich(it, 12.5, 1.6, t.muted, &t)
                                        .id(SharedString::from(format!("sum-li-{i}-{it}"))),
                                ),
                        );
                    }
                    body = body.child(ul);
                } else {
                    body = body.child(
                        div().when(!last, |d| d.mb(px(10.))).child(
                            rich(&para.html, 12.5, 1.6, t.muted, &t)
                                .id(SharedString::from(format!("sum-p-{i}"))),
                        ),
                    );
                }
            }
        }
        if !pr.focus.title.is_empty() || !pr.focus.body.is_empty() {
            body = body.child(self.focus_callout(pr, cx));
        }
        card(
            card_h(
                IconName::AlignLeft,
                "Summary",
                Some("written by Maestro · review this first"),
                &t,
            ),
            body,
            &t,
        )
        .into_any_element()
    }

    /// `.focus`.
    fn focus_callout(&mut self, pr: &Pr, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let amber = rgba(242, 179, 61, 0.06);
        let mut col = div()
            .flex_1()
            .min_w_0()
            .child(
                text(pr.focus.title.clone())
                    .size(12.)
                    .color(t.text)
                    .id("focus-ft"),
            )
            .child(
                div().mt(px(3.)).child(
                    rich(&pr.focus.body, 10.5, 1.5, t.muted, &t)
                        .family(Family::Mono)
                        .id("focus-ff"),
                ),
            );
        let chips: Vec<_> = pr
            .impact
            .iter()
            .filter(|it| it.changed || it.kind == "test")
            .take(8)
            .cloned()
            .collect();
        if !pr.impact.is_empty() {
            let mut row = div().flex().flex_wrap().gap(px(6.)).my(px(8.));
            for it in chips {
                let label = format!("{}: {}", it.kind, it.name);
                let (path, line) = (it.path.clone(), it.line);
                row = row.child(
                    div()
                        .id(SharedString::from(format!(
                            "impact-{}-{}-{}",
                            it.kind, it.path, it.name
                        )))
                        .debug_selector({
                            let l = label.clone();
                            move || format!("impact-chip:{l}")
                        })
                        .py(px(3.))
                        .px(px(8.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(hsla(t.line))
                        .bg(hsla(t.pane_2))
                        .text_color(hsla(t.text))
                        .cursor_pointer()
                        .hover(move |s| s.border_color(hsla(t.amber)).text_color(hsla(t.amber)))
                        .child(
                            recur_ui::style::button_label(label, 10.5, 400.).family(Family::Mono),
                        )
                        .on_click(
                            cx.listener(move |d, _, _, cx| d.jump_to_impact(&path, line, cx)),
                        ),
                );
            }
            col = col.child(row);
        }
        col = col.child(
            div()
                .id("focus-fl")
                .debug_selector(|| "focus-fl".into())
                .mt(px(6.))
                .cursor_pointer()
                .child(
                    text("jump to the diff →")
                        .mono()
                        .size(9.5)
                        .color(t.amber)
                        .nowrap(),
                )
                .on_click(cx.listener(|d, _, _, cx| d.set_tab(PrTab::Files, cx))),
        );
        div()
            .debug_selector(|| "focus".into())
            .flex()
            .gap(px(10.))
            .py(px(10.))
            .px(px(12.))
            .mt(px(8.))
            .rounded(px(7.))
            .border_1()
            .border_color(hsla(rgba(242, 179, 61, 0.2)))
            .bg(hsla(amber))
            .child(
                div()
                    .flex_none()
                    .mt(px(2.))
                    .text_color(hsla(t.amber))
                    .child(icon(IconName::TriangleAlert).size(px(14.))),
            )
            .child(col)
            .into_any_element()
    }

    /// The Reviewer findings card.
    fn findings_card(
        &mut self,
        listed: &[Finding],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let can_post = self.can_post_findings(cx);
        let postable = self.postable_findings(cx);
        let postable_ids: std::collections::HashSet<String> =
            postable.iter().map(|f| f.id.clone()).collect();
        let posted = Self::posted_ids(cx);
        let lh = recur_ui::style::button_label("x", 11., 400.)
            .measure(None, window)
            .2
            .height;
        let mut end = div().ml_auto().flex().items_center().gap(px(8.)).child(
            text(format!("{} open", listed.len()))
                .mono()
                .size(10.)
                .weight(400.)
                .color(t.faint)
                .nowrap(),
        );
        if can_post && !postable.is_empty() {
            let busy = postable.iter().any(|f| self.posting.contains(&f.id));
            let list = postable.clone();
            end = end.child(
                note_btn("findings-post-all", "Post all", false, busy, t.pane, lh, cx)
                    .debug_selector(|| "button:Post all".into())
                    .when(!busy, |b| {
                        b.on_click(
                            cx.listener(move |d, _, _, cx| d.post_findings(list.clone(), cx)),
                        )
                    }),
            );
        }
        let head = card_h_with(
            IconName::Search,
            "Reviewer findings",
            end.into_any_element(),
            &t,
        );
        let mut body = card_b();
        for f in listed {
            let sev = f.severity.as_str().to_string();
            let sev_color = if matches!(sev.as_str(), "high" | "critical") {
                t.coral
            } else {
                t.amber
            };
            let loc = if f.line > 0 {
                if f.end_line > f.line {
                    format!("{}:{}-{}", f.file, f.line, f.end_line)
                } else {
                    format!("{}:{}", f.file, f.line)
                }
            } else {
                f.file.clone()
            };
            let ff = f.clone();
            let row = div()
                .id(SharedString::from(format!("finding-row-{}", f.id)))
                .debug_selector({
                    let m = f.message.clone();
                    move || format!("finding-row:{m}")
                })
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .py(px(8.))
                .cursor_pointer()
                .hover(move |s| s.bg(hsla(t.pane_2)))
                .child(
                    div().flex_none().w(px(64.)).child(
                        text(sev.clone())
                            .mono()
                            .size(10.)
                            .uppercase()
                            .color(sev_color)
                            .nowrap(),
                    ),
                )
                .child(div().w(px(8.)).flex_none())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(text(f.message.clone()).size(12.).color(t.text).truncate()),
                )
                .child(div().w(px(8.)).flex_none())
                .child(
                    div()
                        .flex_none()
                        .child(text(loc).mono().size(10.5).color(t.faint).nowrap()),
                )
                .on_click(cx.listener(move |d, _, _, cx| d.jump_to_impact(&ff.file, ff.line, cx)));
            let mut actions = div()
                .flex()
                .items_center()
                .gap(px(8.))
                .border_b_1()
                .border_color(hsla(t.line_soft))
                .child(row);
            if can_post && postable_ids.contains(&f.id) {
                let busy = self.posting.contains(&f.id);
                let one = vec![f.clone()];
                actions = actions.child(
                    div().mr(px(2.)).child(titled(
                        SharedString::from(format!("post-one-t-{}", f.id)),
                        "Queue as a GitHub review comment",
                        note_btn(
                            SharedString::from(format!("post-one-{}", f.id)),
                            "Post",
                            false,
                            busy,
                            t.pane,
                            lh,
                            cx,
                        )
                        .debug_selector({
                            let m = f.message.clone();
                            move || format!("button:Post as GitHub comment:{m}")
                        })
                        .when(!busy, |b| {
                            b.on_click(
                                cx.listener(move |d, _, _, cx| d.post_findings(one.clone(), cx)),
                            )
                        }),
                    )),
                );
            } else if posted.contains(&f.id) {
                let busy = self.settling.contains(&f.id);
                let fid = f.id.clone();
                actions = actions.child(
                    div().mr(px(2.)).child(titled(
                        SharedString::from(format!("resolve-t-{}", f.id)),
                        "Mark the finding resolved",
                        note_btn(
                            SharedString::from(format!("resolve-{}", f.id)),
                            "Resolve",
                            false,
                            busy,
                            t.pane,
                            lh,
                            cx,
                        )
                        .debug_selector({
                            let m = f.message.clone();
                            move || format!("button:Resolve finding:{m}")
                        })
                        .when(!busy, |b| {
                            b.on_click(cx.listener(move |d, _, _, cx| {
                                let id = d.pr_id(cx);
                                if !id.is_empty() {
                                    d.settle_posted_findings(&id, Some(vec![fid.clone()]), cx);
                                }
                            }))
                        }),
                    )),
                );
            }
            body = body.child(actions);
        }
        card(head, body, &t)
            .debug_selector(|| "card:Reviewer findings".into())
            .into_any_element()
    }

    /// The Local PR `Verified in browser` card.
    fn verify_card(&mut self, pr: &Pr, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let by = if pr.when.is_empty() {
            "QA agent".to_string()
        } else {
            format!("QA agent · {}", pr.when)
        };
        let mut col = div().flex().flex_col();
        let steps = pr.verify.clone().unwrap_or_default();
        let n = steps.len();
        for (i, v) in steps.iter().enumerate() {
            let shot = shot_url(&v.screenshot);
            let mut row = div()
                .flex()
                .items_center()
                .gap(px(11.))
                .py(px(9.))
                .px(px(15.))
                .when(i + 1 < n, |d| {
                    d.border_b_1().border_color(hsla(t.line_soft))
                })
                .child(
                    div()
                        .flex_none()
                        .size(px(18.))
                        .rounded(px(5.))
                        .bg(hsla(rgba(93, 190, 126, 0.15)))
                        .text_color(hsla(t.emerald))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            icon(if v.passed {
                                IconName::Check
                            } else {
                                IconName::X
                            })
                            .size(px(11.))
                            .stroke_width(3.),
                        ),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        Text::from_spans(html_spans(&v.html, &|tags, s| {
                            if tags.contains(&"b") {
                                s.color(t.text).weight(500.)
                            } else {
                                s
                            }
                        }))
                        .mono()
                        .size(11.5)
                        .color(t.muted)
                        .id(SharedString::from(format!("estep-{i}"))),
                    ),
                );
            let frame = || {
                div()
                    .relative()
                    .flex_none()
                    .w(px(78.))
                    .h(px(48.))
                    .rounded(px(5.))
                    .border_1()
                    .border_color(hsla(t.line))
                    .overflow_hidden()
                    .child(recur_ui::style::css_gradient(
                        135.,
                        (recur_ui::hex(0x1a2230), 0.),
                        (recur_ui::hex(0x0e1318), 1.),
                        5.,
                        1.,
                        t.line,
                    ))
            };
            if shot.is_empty() {
                row = row.child(titled(
                    SharedString::from(format!("shot-{i}")),
                    v.screenshot.clone(),
                    frame()
                        .id(SharedString::from(format!("shot-el-{i}")))
                        .cursor_pointer()
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(text("▸ view").mono().size(8.).color(t.muted).nowrap()),
                        ),
                ));
            } else {
                let href = shot.clone();
                row = row.child(titled(
                    SharedString::from(format!("shot-{i}")),
                    v.screenshot.clone(),
                    frame()
                        .id(SharedString::from(format!("shot-el-{i}")))
                        .cursor_pointer()
                        .child(
                            img(SharedString::from(shot))
                                .absolute()
                                .inset_0()
                                .size_full(),
                        )
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(text("▸ view").mono().size(8.).color(t.muted).nowrap()),
                        )
                        .on_click(move |_, _, cx| {
                            recur_content::links::open_external_url(&href, cx)
                        }),
                ));
            }
            col = col.child(row);
        }
        card(
            card_h(IconName::RefreshCcw, "Verified in browser", Some(&by), &t),
            col,
            &t,
        )
        .debug_selector(|| "card:Verified in browser".into())
        .into_any_element()
    }

    /// One timeline row.
    fn timeline_item(
        &mut self,
        c: &PrComment,
        items: &[PrComment],
        pr: &Pr,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if c.kind == "action" && !c.commit_sha.is_empty() && !is_merge_action(c) {
            return self.commit_event(c, pr, window, cx);
        }
        if c.kind == "action" {
            return self.action_event(c, pr, cx);
        }
        self.comment_row(c, items, pr, window, cx)
    }

    fn commit_event(
        &mut self,
        c: &PrComment,
        pr: &Pr,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let local = self.local_commit_index(&c.commit_sha, cx);
        let title = commit_title(c);
        let ico = tl_ico(&t, None, false).child(if !c.avatar_url.is_empty() {
            img(SharedString::from(c.avatar_url.clone()))
                .size_full()
                .object_fit(gpui_kit::ObjectFit::Cover)
                .into_any_element()
        } else {
            icon(IconName::GitCommit).size(px(14.)).into_any_element()
        });
        let msg_text = text(title.clone())
            .size(12.5)
            .weight(500.)
            .truncate()
            .id(SharedString::from(format!("tl-msg-{}", c.id)));
        let msg: AnyElement = match local {
            Some(i) => titled(
                SharedString::from(format!("tl-msg-t-{}", c.id)),
                "View commit",
                div()
                    .id(SharedString::from(format!("tl-msg-b-{}", c.id)))
                    .min_w_0()
                    .cursor_pointer()
                    .child(msg_text)
                    .on_click(cx.listener(move |d, _, _, cx| d.open_local_commit(i, cx))),
            )
            .into_any_element(),
            None if !c.url.is_empty() => {
                let href = c.url.clone();
                titled(
                    SharedString::from(format!("tl-msg-t-{}", c.id)),
                    "Open commit on GitHub",
                    div()
                        .id(SharedString::from(format!("tl-msg-a-{}", c.id)))
                        .min_w_0()
                        .cursor_pointer()
                        .child(msg_text)
                        .on_click(move |_, _, cx| {
                            recur_content::links::open_external_url(&href, cx)
                        }),
                )
                .into_any_element()
            }
            None => div().min_w_0().child(msg_text).into_any_element(),
        };
        let mut tx = div()
            .flex()
            .items_center()
            .gap_x(px(8.))
            .flex_1()
            .min_w_0()
            .child(div().flex_initial().min_w_0().child(msg));
        if !c.time.is_empty() {
            tx = tx.child(
                div().flex_none().child(
                    text(c.time.clone())
                        .mono()
                        .size(9.5)
                        .color(t.faint)
                        .nowrap(),
                ),
            );
        }
        let sha = short_sha(&c.commit_sha);
        let sha_text = text(sha.clone())
            .mono()
            .size(11.)
            .weight(400.)
            .color(t.faint)
            .nowrap();
        let sha_el: AnyElement = match local {
            Some(i) => titled(
                SharedString::from(format!("tl-sha-t-{}", c.id)),
                "View commit",
                div()
                    .id(SharedString::from(format!("tl-sha-b-{}", c.id)))
                    .debug_selector({
                        let s = sha.clone();
                        move || format!("tl-sha:{s}")
                    })
                    .cursor_pointer()
                    .child(sha_text)
                    .on_click(cx.listener(move |d, _, _, cx| d.open_local_commit(i, cx))),
            )
            .into_any_element(),
            None if !c.url.is_empty() => {
                let href = c.url.clone();
                titled(
                    SharedString::from(format!("tl-sha-t-{}", c.id)),
                    "Open commit on GitHub",
                    div()
                        .id(SharedString::from(format!("tl-sha-a-{}", c.id)))
                        .debug_selector({
                            let s = sha.clone();
                            move || format!("tl-sha:{s}")
                        })
                        .cursor_pointer()
                        .child(sha_text)
                        .on_click(move |_, _, cx| {
                            recur_content::links::open_external_url(&href, cx)
                        }),
                )
                .into_any_element()
            }
            None => div()
                .debug_selector({
                    let s = sha.clone();
                    move || format!("tl-sha:{s}")
                })
                .child(sha_text)
                .into_any_element(),
        };
        let mut meta = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .ml_auto();
        if c.verified {
            meta = meta.child(verified(&t));
        }
        meta = meta
            .child(commit_check(
                &c.check_status,
                &format!("tl-{}", c.id),
                window,
                cx,
            ))
            .child(sha_el);
        tx = tx.child(meta);
        let _ = pr;
        tl_event(&t, false, None)
            .debug_selector({
                let id = c.id.clone();
                move || format!("tl-commit:{id}")
            })
            .child(ico)
            .child(tx)
            .into_any_element()
    }

    fn action_event(&mut self, c: &PrComment, pr: &Pr, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let behalf = action_behalf(c);
        let merge = is_merge_action(c);
        let merge_link = merge_commit_link(c, &pr.url);
        let decision = review_decision(Some(c));
        let ico = tl_ico(&t, Some(decision), merge);
        let ico = match decision {
            ReviewDecision::Approved => {
                ico.child(icon(IconName::Check).size(px(14.)).stroke_width(3.))
            }
            ReviewDecision::Changes => ico.child(icon(IconName::X).size(px(14.)).stroke_width(3.)),
            _ if !c.avatar_url.is_empty() => ico.child(
                img(SharedString::from(c.avatar_url.clone()))
                    .size_full()
                    .object_fit(gpui_kit::ObjectFit::Cover),
            ),
            _ => ico.child(icon(action_icon(c)).size(px(14.))),
        };
        let b = |s: &str| text(s.to_string()).size(12.5).weight(600.).color(t.text);
        let plain = |s: &str| text(s.to_string()).size(12.5).color(t.muted);
        let mut tx = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(8.))
            .gap_y(px(6.))
            .flex_1()
            .min_w_0();
        if let Some((verb, whom)) = behalf {
            let href = comment_permalink(&pr.url, c);
            tx = tx.child(b(&c.author));
            if href.is_empty() {
                tx = tx.child(plain(&verb));
            } else {
                tx = tx.child(titled(
                    SharedString::from(format!("tl-verb-{}", c.id)),
                    "Open on GitHub",
                    div()
                        .id(SharedString::from(format!("tl-verb-a-{}", c.id)))
                        .cursor_pointer()
                        .text_color(hsla(t.muted))
                        .hover(move |s| s.text_color(hsla(t.text)))
                        .child(text(verb.clone()).size(12.5))
                        .on_click(move |_, _, cx| {
                            recur_content::links::open_external_url(&href, cx)
                        }),
                ));
            }
            tx = tx.child(plain("on behalf of")).child(b(&whom));
        } else if let Some((sha, rest, href)) = merge_link {
            tx = tx.child(b(&c.author)).child(plain("merged commit"));
            let sha_text = text(sha.clone())
                .mono()
                .size(11.)
                .weight(400.)
                .color(t.faint)
                .nowrap();
            if href.is_empty() {
                tx = tx.child(
                    div()
                        .debug_selector({
                            let s = sha.clone();
                            move || format!("tl-sha:{s}")
                        })
                        .child(sha_text),
                );
            } else {
                tx = tx.child(titled(
                    SharedString::from(format!("tl-merge-sha-{}", c.id)),
                    "Open commit on GitHub",
                    div()
                        .id(SharedString::from(format!("tl-merge-sha-a-{}", c.id)))
                        .debug_selector({
                            let s = sha.clone();
                            let h = href.clone();
                            move || format!("tl-sha-a:{s}={h}")
                        })
                        .cursor_pointer()
                        .child(sha_text)
                        .on_click(move |_, _, cx| {
                            recur_content::links::open_external_url(&href, cx)
                        }),
                ));
            }
            if !rest.is_empty() {
                tx = tx.child(plain(&rest));
            }
        } else if decision == ReviewDecision::Approved {
            tx = tx
                .child(b(&c.author))
                .child(plain("approved these changes"));
        } else if decision == ReviewDecision::Changes {
            tx = tx.child(b(&c.author)).child(plain("requested changes"));
        } else {
            tx = tx.child(b(&c.author));
            if !c.body_md.is_empty() {
                tx = tx.child(plain(&c.body_md));
            }
        }
        if let Some(w) = self.when_stamp(c, &pr.url, true, cx) {
            tx = tx.child(w);
        }
        let text_all = format!("{} {}", c.author, c.body_md);
        tl_event(&t, merge, Some(decision))
            .debug_selector({
                let id = c.id.clone();
                let d = match decision {
                    ReviewDecision::Approved => ":review-ok",
                    ReviewDecision::Changes => ":review-ask",
                    _ => "",
                };
                move || format!("tl-event:{id}{d}:{text_all}")
            })
            .child(ico)
            .child(tx)
            .into_any_element()
    }

    /// A timeline comment (`.cmt`): avatar with the review badge, header, body, and the line
    /// notes its review submitted.
    fn comment_row(
        &mut self,
        c: &PrComment,
        items: &[PrComment],
        pr: &Pr,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let verb = comment_header_verb(Some(c));
        let decision = review_decision(Some(c));
        let mut face = div()
            .relative()
            .flex_none()
            .size(px(28.))
            .child(Self::person_avatar(c, cx));
        if matches!(decision, ReviewDecision::Approved | ReviewDecision::Changes) {
            let ok = decision == ReviewDecision::Approved;
            face = face.child(titled(
                SharedString::from(format!("badge-{}", c.id)),
                if ok { "Approved" } else { "Changes requested" },
                div()
                    .id(SharedString::from(format!("badge-el-{}", c.id)))
                    .debug_selector(move || {
                        format!("cmt-review-badge:{}", if ok { "ok" } else { "ask" })
                    })
                    .absolute()
                    .right(px(-5.))
                    .bottom(px(-5.))
                    .size(px(16.))
                    .rounded_full()
                    .border_2()
                    .border_color(hsla(t.bg))
                    .bg(hsla(if ok { t.emerald } else { t.coral }))
                    .text_color(hsla(recur_ui::hex(0xffffff)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        icon(if ok { IconName::Check } else { IconName::X })
                            .size(px(12.))
                            .stroke_width(3.),
                    ),
            ));
        }
        let can_edit = can_edit_pr_comment(Some(pr), Some(c));
        let mut ch = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .mb(px(5.))
            .text_color(hsla(t.muted))
            .child(
                text(c.author.clone())
                    .size(CH_SIZE)
                    .weight(600.)
                    .line_height(ch_line())
                    .color(t.text)
                    .nowrap(),
            );
        if !verb.is_empty() {
            ch = ch.child(
                text(verb)
                    .size(CH_SIZE)
                    .line_height(ch_line())
                    .color(t.muted)
                    .nowrap(),
            );
        }
        if let Some(w) = self.when_stamp(c, &pr.url, false, cx) {
            ch = ch.child(w);
        }
        if self.editing_comment != c.id && self.has_more(c, can_edit, cx) {
            ch = ch.child(self.more_btn(&c.id, cx));
        }
        let mut cc = div()
            .flex_1()
            .min_w_0()
            .child(ch)
            .children(self.comment_body(c, false, window, cx));
        let kids: Vec<PrComment> = review_children(items, &c.id).into_iter().cloned().collect();
        if !kids.is_empty() {
            let mut col = div().mt(px(10.)).flex().flex_col().gap(px(12.));
            for child in &kids {
                col = col.child(self.kid(child, items, pr, window, cx));
            }
            cc = cc.child(col);
        }
        let all = format!("{} {}", c.author, comment_source(Some(c)));
        div()
            .debug_selector({
                let id = c.id.clone();
                let d = match decision {
                    ReviewDecision::Approved => ":review-ok",
                    ReviewDecision::Changes => ":review-ask",
                    _ => "",
                };
                move || format!("cmt:{id}{d}:{all}")
            })
            .flex()
            .gap(px(12.))
            .py(px(14.))
            .min_w_0()
            .border_b_1()
            .border_color(hsla(t.line_soft))
            .child(face)
            .child(cc)
            .into_any_element()
    }

    /// `.cmt-kid`: one line-note thread under its review.
    fn kid(
        &mut self,
        child: &PrComment,
        items: &[PrComment],
        pr: &Pr,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let locked = self.conversation_locked(cx);
        let replies: Vec<PrComment> = thread_replies(items, &child.id)
            .into_iter()
            .cloned()
            .collect();
        let hunk = clip_comment_hunk(
            &parse_diff_hunk(&child.diff_hunk),
            &ClipOpts {
                line: child.line,
                side: &child.side,
                start_line: child.start_line,
                start_side: &child.start_side,
                context: None,
            },
        );
        let resolved_open = !child.resolved || self.open_resolved.contains(&child.id);
        let hide_hunk = if child.resolved {
            !self.open_resolved.contains(&child.id)
        } else {
            self.collapsed_ctx.contains(&child.id)
        };
        let bg = if child.resolved { t.pane_2 } else { t.pane };
        let mut kid = div()
            .debug_selector({
                let id = child.id.clone();
                let r = if child.resolved { ":resolved" } else { "" };
                move || format!("cmt-kid:{id}{r}")
            })
            .relative()
            .border_1()
            .border_color(hsla(t.line))
            .rounded(px(8.))
            .overflow_hidden()
            .child(recur_ui::style::inset_background(bg, 8., 1., t.line));
        if !child.path.is_empty() || !hunk.is_empty() {
            let mut ctx = div().flex().flex_col();
            if !child.path.is_empty() {
                let label = comment_hunk_path_label(
                    &child.path,
                    child.line,
                    child.start_line,
                    child.outdated,
                );
                let title = if hide_hunk {
                    "Show context"
                } else {
                    "Hide context"
                };
                let id = child.id.clone();
                let resolved = child.resolved;
                let (path, line) = (
                    child.path.clone(),
                    if child.outdated { 0 } else { child.line },
                );
                let group = SharedString::from(format!("ctx-h-{}", child.id));
                let toggle = {
                    let id = id.clone();
                    cx.listener(move |d, _, _, cx| d.toggle_kid_hunk(&id, resolved, cx))
                };
                let fill_toggle = {
                    let id = id.clone();
                    cx.listener(move |d, _, _, cx| d.toggle_kid_hunk(&id, resolved, cx))
                };
                let chev = icon(IconName::ChevronDown)
                    .size(px(12.))
                    .rotate(if hide_hunk { -90. } else { 0. });
                ctx = ctx.child(
                    div()
                        .id(group.clone())
                        .group(group.clone())
                        .debug_selector(|| "cmt-ctx-h".into())
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .border_b_1()
                        .border_color(hsla(t.line_soft))
                        .child(titled(
                            SharedString::from(format!("ctx-toggle-t-{id}")),
                            title,
                            div()
                                .id(SharedString::from(format!("ctx-toggle-{id}")))
                                .debug_selector(move || {
                                    format!(
                                        "cmt-ctx-toggle:{}",
                                        if hide_hunk { "collapsed" } else { "open" }
                                    )
                                })
                                .flex()
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .size(px(28.))
                                .rounded(px(4.))
                                .cursor_pointer()
                                .text_color(hsla(t.faint))
                                .group_hover(group.clone(), move |s| s.text_color(hsla(t.muted)))
                                .child(chev)
                                .on_click(toggle),
                        ))
                        .child(titled(
                            SharedString::from(format!("ctx-path-t-{id}")),
                            "View in Files changed",
                            div()
                                .id(SharedString::from(format!("ctx-path-{id}")))
                                .debug_selector({
                                    let l = label.clone();
                                    move || format!("cmt-ctx-path={l}")
                                })
                                .flex_none()
                                .max_w(gpui_kit::relative(0.7))
                                .min_w_0()
                                .py(px(6.))
                                .px(px(2.))
                                .cursor_pointer()
                                .text_color(hsla(t.muted))
                                .hover(move |s| s.text_color(hsla(t.text)))
                                .child(text(label.clone()).mono().size(11.).truncate())
                                .on_click(cx.listener(move |d, _, _, cx| {
                                    d.jump_to_impact(&path, line, cx)
                                })),
                        ))
                        .child(titled(
                            SharedString::from(format!("ctx-fill-t-{id}")),
                            title,
                            div()
                                .id(SharedString::from(format!("ctx-fill-{id}")))
                                .debug_selector(|| "cmt-ctx-fill".into())
                                .flex_1()
                                .self_stretch()
                                .min_w(px(8.))
                                .cursor_pointer()
                                .on_click(fill_toggle),
                        )),
                );
            }
            if !hide_hunk && !hunk.is_empty() {
                let mut diff = div()
                    .debug_selector(|| "cmt-ctx-diff".into())
                    .border_b_1()
                    .border_color(hsla(t.line_soft));
                for hl in &hunk {
                    diff = diff.child(hunk_line(hl, cx));
                }
                ctx = ctx.child(diff);
            }
            kid = kid.child(ctx);
        } else if !child.body_html.is_empty() && resolved_open {
            kid = kid.child(
                div()
                    .pt(px(10.))
                    .px(px(12.))
                    .child(loc_line(&child.body_html, &t)),
            );
        }
        if resolved_open {
            kid = kid.child(self.line_note(child, pr, true, window, cx));
            for r in &replies {
                kid = kid.child(self.line_note(r, pr, false, window, cx));
            }
            let mut foot = div().pb(px(10.)).px(px(12.));
            if self.reply_for == child.id && !locked {
                foot = foot.child(self.compose_box(Compose::Reply(child.id.clone()), window, cx));
            } else if !locked {
                let id = child.id.clone();
                foot = foot.child(
                    div()
                        .id(SharedString::from(format!("reply-ph-{}", child.id)))
                        .debug_selector({
                            let id = child.id.clone();
                            move || format!("cmt-reply-ph:{id}")
                        })
                        .w_full()
                        .py(px(7.))
                        .px(px(10.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(hsla(t.line))
                        .bg(hsla(t.pane_2))
                        .text_color(hsla(t.faint))
                        .cursor_text()
                        .hover(move |s| s.text_color(hsla(t.muted)).border_color(hsla(t.muted)))
                        .child(recur_ui::style::button_label("Reply…", 12.5, 400.))
                        .on_click(
                            cx.listener(move |d, _, window, cx| d.open_reply(&id, window, cx)),
                        ),
                );
            }
            if can_resolve_line_note(Some(pr), Some(child)) {
                foot = foot.child(
                    div()
                        .flex()
                        .justify_end()
                        .mt(px(8.))
                        .child(self.resolve_btn(child, window, cx)),
                );
            }
            kid = kid.child(foot);
        } else {
            let mut bar = div()
                .debug_selector({
                    let id = child.id.clone();
                    move || format!("cmt-resolved-bar:{id}")
                })
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.))
                .pt(px(8.))
                .px(px(12.))
                .pb(px(10.))
                .text_color(hsla(t.faint))
                .child(text("Resolved").size(12.5).nowrap());
            if can_resolve_line_note(Some(pr), Some(child)) {
                bar = bar.child(self.resolve_btn(child, window, cx));
            }
            kid = kid.child(bar);
        }
        kid.into_any_element()
    }

    fn toggle_kid_hunk(&mut self, id: &str, resolved: bool, cx: &mut Context<Self>) {
        let set = if resolved {
            &mut self.open_resolved
        } else {
            &mut self.collapsed_ctx
        };
        if !set.remove(id) {
            set.insert(id.to_string());
        }
        cx.notify();
    }

    /// `resolveThreadBtn`.
    fn resolve_btn(
        &mut self,
        c: &PrComment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let busy = self.saving == format!("res-{}", line_thread_key(Some(c)));
        let lh = recur_ui::style::button_label("x", 11., 400.)
            .measure(None, window)
            .2
            .height;
        let label = if c.resolved {
            "Unresolve"
        } else {
            "Resolve conversation"
        };
        let backdrop = if c.resolved { t.pane_2 } else { t.pane };
        let cc = c.clone();
        let to = !c.resolved;
        note_btn(
            SharedString::from(format!("resolve-{}", c.id)),
            label,
            false,
            busy,
            backdrop,
            lh,
            cx,
        )
        .debug_selector(move || format!("cmt-resolve:{label}"))
        .when(!busy, |b| {
            b.on_click(cx.listener(move |d, _, _, cx| d.set_thread_resolved(&cc, to, cx)))
        })
        .into_any_element()
    }

    /// `lineNote(c)`: a note row inside a `.cmt-kid`.
    fn line_note(
        &mut self,
        c: &PrComment,
        pr: &Pr,
        first: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let can_edit = can_edit_pr_comment(Some(pr), Some(c));
        let mut ch = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .mb(px(5.))
            .text_color(hsla(t.muted))
            .child(
                text(c.author.clone())
                    .size(CH_SIZE)
                    .weight(600.)
                    .line_height(ch_line())
                    .color(t.text)
                    .nowrap(),
            );
        if let Some(w) = self.when_stamp(c, &pr.url, false, cx) {
            ch = ch.child(w);
        }
        if self.editing_comment != c.id && self.has_more(c, can_edit, cx) {
            ch = ch.child(self.more_btn(&c.id, cx));
        }
        let cc = div()
            .flex_1()
            .min_w_0()
            .child(ch)
            .children(self.comment_body(c, true, window, cx));
        let all = comment_source(Some(c));
        div()
            .debug_selector({
                let id = c.id.clone();
                move || format!("cmt-kid-note:{id}:{all}")
            })
            .flex()
            .gap(px(10.))
            .pt(px(10.))
            .px(px(12.))
            .pb(px(8.))
            .when(!first, |d| d.border_t_1().border_color(hsla(t.line_soft)))
            .child(avatar(
                &c.code,
                &c.avatar_css,
                &c.avatar_url,
                22.,
                5.,
                8.5,
                cx,
            ))
            .child(cc)
            .into_any_element()
    }

    /// The composer at the end of an open conversation.
    fn composer_box(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let area = self.composer.area.clone();
        let empty = area.read(cx).value().trim().is_empty();
        let disabled = empty || !self.saving.is_empty();
        let input = div()
            .id("composer-input")
            .debug_selector(|| "composer-textarea".into())
            .min_h(px(72.))
            .max_h(px(160.))
            .overflow_hidden()
            .pt(px(10.))
            .px(px(12.))
            .pb(px(6.))
            .text_size(px(12.5))
            .line_height(px(18.75))
            .text_color(hsla(t.text))
            .key_context("TextEntry")
            .child(gpui_kit::base::input::Textarea::new(&area));
        let go = recur_ui::style::primary_button("composer-go", disabled.then_some(t.bg), cx)
            .debug_selector(|| "composer-go:Comment".into())
            .child(recur_ui::style::button_label("Comment", 13., 600.))
            .child(recur_ui::style::button_label("⌘↵", 12., 500.).opacity(0.55))
            .when(!disabled, |b| {
                b.on_click(cx.listener(|d, _, window, cx| d.submit_composer(window, cx)))
            });
        let _ = window;
        // `.composer { margin-top: 4px }` collapses into the checks card's 14px.
        recur_ui::style::card(t.bg, t.line, 10.)
            .debug_selector(|| "composer".into())
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(input)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(2.))
                    .px(px(10.))
                    .pb(px(10.))
                    .child(go),
            )
            .into_any_element()
    }

    /// `.pr-settled`.
    fn settled(pr: &Pr, cx: &App) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let merged = pr_lifecycle(Some(pr)) == Some(PrLifecycle::Merged);
        let (border, bg, ico_bg, ico_fg) = if merged {
            (
                t.indigo.mix(0.45, t.line),
                t.indigo.mix(0.08, t.pane),
                t.indigo,
                recur_ui::hex(0x0a0c10),
            )
        } else {
            (t.muted.mix(0.5, t.line), t.pane, t.pane_2, t.muted)
        };
        let (h, s) = if merged {
            (
                "Pull request successfully merged and closed",
                "You're all set — the branch has been merged.",
            )
        } else {
            (
                "Pull request closed",
                "This pull request was closed without merging.",
            )
        };
        recur_ui::style::card(bg, border, 10.)
            .debug_selector(move || format!("pr-settled:{h}"))
            .flex()
            .items_start()
            .gap(px(14.))
            .mt(px(16.))
            .py(px(16.))
            .px(px(18.))
            .child(
                div()
                    .flex_none()
                    .size(px(36.))
                    .rounded(px(8.))
                    .bg(hsla(ico_bg))
                    .text_color(hsla(ico_fg))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(pr_icon_name(Some(pr))).size(px(18.))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(text(h).size(14.).weight(650.).color(t.text).id("settled-h"))
                    .child(
                        div()
                            .mt(px(4.))
                            .child(text(s).size(12.5).color(t.muted).id("settled-s")),
                    ),
            )
            .into_any_element()
    }

    /// The Conversation pane.
    pub(crate) fn conversation(
        &mut self,
        pr: Option<&Pr>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let mut wrap = div()
            .w_full()
            .max_w(px(840.))
            .mx_auto()
            .p(px(18.))
            .min_w_0()
            .flex()
            .flex_col();
        if let Some(pr) = pr {
            if !pr.updating_base.is_empty() {
                wrap = wrap.child(rework(pr, window, cx));
            }
            if self.conv_pending(cx) {
                wrap = wrap.child(super::skeleton::conv_skel(window, cx));
            } else {
                let items = self.thread_items(cx);
                let desc = items.iter().find(|c| c.kind == "description").cloned();
                if let Some(d) = &desc {
                    wrap = wrap.child(self.description_card(d, pr, window, cx));
                }
                let gh = pr.source == PrSource::GitHub;
                let has_focus = !pr.focus.title.is_empty() || !pr.focus.body.is_empty();
                if !gh && (desc.is_none() || has_focus) {
                    wrap = wrap.child(self.summary_card(pr, desc.is_some(), cx));
                }
                let listed = self.listed_findings(cx);
                if !listed.is_empty() {
                    wrap = wrap.child(self.findings_card(&listed, window, cx));
                }
                if !gh {
                    wrap = wrap.child(self.verify_card(pr, cx));
                }
                let mut thread = div()
                    .debug_selector(|| "thread".into())
                    .pt(px(6.))
                    .flex()
                    .flex_col();
                for c in items.iter().filter(|c| is_timeline_root(Some(c))) {
                    thread = thread.child(self.timeline_item(c, &items, pr, window, cx));
                }
                wrap = wrap.child(thread);
                if self.conversation_locked(cx) {
                    wrap = wrap.child(Self::settled(pr, cx));
                } else {
                    wrap = wrap.child(
                        div()
                            .mt(px(14.))
                            .child(self.checks_card(pr, true, window, cx)),
                    );
                    wrap = wrap.child(self.composer_box(window, cx));
                }
            }
        }
        let _ = t;
        let scroll = self.conv_scroll.clone();
        self.pane("conv", &scroll, wrap, cx)
    }
}

/// `.cmt .cm.loc`: the line note's location line (mono 11, faint, 6px under).
fn loc_line(html: &str, t: &Tokens) -> Div {
    div().mb(px(6.)).child(
        rich_code(html, 11., 1.55, t.faint, None, t)
            .family(Family::Mono)
            .id(SharedString::from(format!("loc-{html}"))),
    )
}

/// `.cmt .cm`: trusted markup on the comment line height.
fn cm_html(html: &str, t: &Tokens) -> Div {
    div().child(
        rich_code(html, 12.5, 1.55, t.faint, None, t).id(SharedString::from(format!("cm-{html}"))),
    )
}

/// `.cmt .react`.
fn react(html: &str, t: &Tokens) -> Div {
    div().flex().child(
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .mt(px(9.))
            .py(px(4.))
            .px(px(10.))
            .rounded(px(6.))
            .border_1()
            .border_color(hsla(rgba(76, 198, 214, 0.2)))
            .bg(hsla(rgba(76, 198, 214, 0.07)))
            .child(
                Text::from_spans(html_spans(html, &|_, s| s))
                    .mono()
                    .size(10.5)
                    .color(t.cyan)
                    .nowrap(),
            ),
    )
}

/// Trusted inline HTML at `size` / `leading`: `<b>` in `--text` 600, `<code>` as the card's
/// code chip.
pub(crate) fn rich(html: &str, size: f32, leading: f32, color: Color, t: &Tokens) -> Text {
    rich_code(html, size, leading, color, Some(t.text), t)
}

/// [`rich`] with the code chip's color (`None` inherits, as `.cmt .cm code` does).
pub(crate) fn rich_code(
    html: &str,
    size: f32,
    leading: f32,
    color: Color,
    code_color: Option<Color>,
    t: &Tokens,
) -> Text {
    let text_c = t.text;
    let code_bg = t.pane_2;
    Text::from_spans(html_spans(html, &move |tags, s: Span| {
        let mut s = s;
        if tags.iter().any(|t| *t == "b" || *t == "strong") {
            s = s.color(text_c).weight(600.);
        }
        if tags.contains(&"code") {
            if let Some(c) = code_color {
                s = s.color(c);
            }
            s = s.mono().size(11.).boxed(InlineBox {
                pad_left: 5.,
                pad_right: 5.,
                pad_top: 1.,
                pad_bottom: 1.,
                background: Some(code_bg.into()),
                radius: 3.,
                ..Default::default()
            });
        }
        s
    }))
    .size(size)
    .leading(leading)
    .color(color)
}

/// `.card`.
pub(crate) fn card(head: Div, body: impl IntoElement, t: &Tokens) -> Div {
    recur_ui::style::card(t.pane, t.line, 9.)
        .mb(px(14.))
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(head)
        .child(body)
}

/// `.card-h` with a trailing `.by`.
pub(crate) fn card_h(ic: IconName, title: &str, by: Option<&str>, t: &Tokens) -> Div {
    let d = card_h_base(ic, title, t);
    match by {
        Some(b) => d.child(
            div().ml_auto().child(
                text(b.to_string())
                    .mono()
                    .size(10.)
                    .weight(400.)
                    .color(t.faint)
                    .nowrap(),
            ),
        ),
        None => d,
    }
}

fn card_h_with(ic: IconName, title: &str, end: AnyElement, t: &Tokens) -> Div {
    card_h_base(ic, title, t).child(end)
}

fn card_h_base(ic: IconName, title: &str, t: &Tokens) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(9.))
        .py(px(11.))
        .px(px(15.))
        .border_b_1()
        .border_color(hsla(t.line))
        .child(
            div()
                .flex_none()
                .text_color(hsla(t.amber))
                .child(icon(ic).size(px(15.))),
        )
        .child(text(title.to_string()).size(12.5).weight(600.).nowrap())
}

/// `.card-b`.
fn card_b() -> Div {
    div().py(px(14.)).px(px(15.))
}

/// `.rework`: the update-with-base banner.
fn rework(pr: &Pr, window: &mut Window, cx: &App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let base = recur_engine::view::pr_merge_action::short_base_name(&pr.base);
    let deg = spin_deg(window, cx);
    div()
        .debug_selector(|| "rework".into())
        .flex()
        .items_center()
        .gap(px(13.))
        .mb(px(14.))
        .py(px(12.))
        .px(px(15.))
        .rounded(px(9.))
        .border_1()
        .border_color(hsla(rgba(242, 179, 61, 0.32)))
        .bg(hsla(rgba(242, 179, 61, 0.08)))
        .child(
            div()
                .flex_none()
                .size(px(32.))
                .rounded(px(8.))
                .bg(hsla(rgba(242, 179, 61, 0.15)))
                .text_color(hsla(t.amber))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(IconName::RefreshCw).size(px(17.)).rotate(deg)),
        )
        .child(
            div()
                .flex_1()
                .child(
                    text(format!("Updating with latest {base}"))
                        .size(12.5)
                        .weight(600.)
                        .color(t.amber)
                        .id("rw-t"),
                )
                .child(
                    div().mt(px(2.)).child(
                        rich(
                            &format!(
                                "Integrating <code>{}</code> into this branch…",
                                html_escape(&base)
                            ),
                            10.5,
                            1.45,
                            t.muted,
                            &t,
                        )
                        .family(Family::Mono)
                        .id("rw-d"),
                    ),
                ),
        )
        .into_any_element()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The `.tl-event` row.
fn tl_event(t: &Tokens, _merge: bool, _decision: Option<ReviewDecision>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .pt(px(8.))
        .pr(px(4.))
        .pb(px(8.))
        .pl(px(6.))
        .text_color(hsla(t.muted))
        .text_size(px(12.5))
        .border_b_1()
        .border_color(hsla(t.line_soft))
}

/// `.tl-ico` (and `.merge`, `.review-ok`, `.review-ask`).
fn tl_ico(t: &Tokens, decision: Option<ReviewDecision>, merge: bool) -> Div {
    let (bg, fg, radius) = match decision {
        Some(ReviewDecision::Approved) => (t.emerald, recur_ui::hex(0xffffff), 11.),
        Some(ReviewDecision::Changes) => (t.coral, recur_ui::hex(0xffffff), 11.),
        _ if merge => (t.indigo.mix(0.22, t.pane_2), t.indigo, 6.),
        _ => (t.pane_2, t.faint, 11.),
    };
    div()
        .flex_none()
        .size(px(22.))
        .rounded(px(radius))
        .bg(hsla(bg))
        .text_color(hsla(fg))
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
}

/// `.tl-verified`.
fn verified(t: &Tokens) -> Div {
    let green = recur_ui::hex(0x3fb950);
    div()
        .flex_none()
        .py(px(1.))
        .px(px(7.))
        .rounded(px(99.))
        .border_1()
        .border_color(hsla(rgba(63, 185, 80, 0.45)))
        .child(text("Verified").mono().size(9.).color(green).nowrap())
        .debug_selector(|| "tl-verified".into())
        .text_color(hsla(t.text))
}

/// `commitCheck(status)`: the 14px check slot (empty without a status).
pub(crate) fn commit_check(status: &str, key: &str, window: &mut Window, cx: &App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let slot = div()
        .flex_none()
        .size(px(14.))
        .flex()
        .items_center()
        .justify_center();
    if status.is_empty() {
        return slot
            .debug_selector(|| "tl-ck:empty".into())
            .into_any_element();
    }
    let (name, color, spin, title) = match status {
        "pass" => (IconName::Check, t.emerald, false, "Checks passed"),
        "fail" => (IconName::X, t.coral, false, "Checks failed"),
        _ if recur_forge::view::is_skipped_check(status, "") => {
            (IconName::CircleSlash, t.muted, false, "Checks pending")
        }
        _ if recur_forge::view::is_pending_check(status) => {
            (IconName::Loader, t.amber, true, "Checks pending")
        }
        _ => (IconName::X, t.coral, false, "Checks pending"),
    };
    let deg = if spin { spin_deg(window, cx) } else { 0. };
    titled(
        SharedString::from(format!("tl-ck-{key}")),
        title,
        slot.id(SharedString::from(format!("tl-ck-el-{key}")))
            .debug_selector(|| "tl-ck:icon".into())
            .text_color(hsla(color))
            .child(icon(name).size(px(14.)).stroke_width(2.4).rotate(deg)),
    )
    .into_any_element()
}

/// A `.cmt-ctx-diff .hline`.
fn hunk_line(hl: &recur_view_review::comment_hunk::HunkLine, cx: &App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let light = recur_view_review::style::light_diff_text(cx);
    let (bg, cd) = match hl.r#type {
        "add" => (
            Some(recur_view_review::style::ADD_BG),
            if light {
                t.text
            } else {
                recur_view_review::style::ADD_CD
            },
        ),
        "del" => (
            Some(recur_view_review::style::DEL_BG),
            if light {
                t.text
            } else {
                recur_view_review::style::DEL_CD
            },
        ),
        _ => (None, t.muted),
    };
    let overlay = if hl.target {
        Some(t.warn.fade(0.16))
    } else if hl.in_range {
        Some(t.cyan.fade(0.10))
    } else {
        None
    };
    let line = 11. * 1.6;
    let ln = |s: &str| {
        div()
            .flex_none()
            .w(px(46.))
            .px(px(8.))
            .border_r_1()
            .border_color(hsla(t.line_soft))
            .flex()
            .justify_end()
            .child(
                text(s.to_string())
                    .mono()
                    .size(10.)
                    .line_height(LineHeight::Px(line))
                    .color(t.faint)
                    .nowrap(),
            )
    };
    let cls = format!(
        "hline:{}{}{}",
        hl.r#type,
        if hl.target { ":target" } else { "" },
        if hl.in_range && !hl.target {
            ":in-range"
        } else {
            ""
        }
    );
    let txt = hl.text.clone();
    div()
        .debug_selector(move || format!("{cls}:{txt}"))
        .relative()
        .flex()
        .when_some(bg, |d, b| d.bg(hsla(b)))
        .when_some(overlay, |d, o| {
            d.child(div().absolute().inset_0().bg(hsla(o)))
        })
        .child(ln(&hl.old))
        .child(ln(&hl.new))
        .child(
            div().flex_1().min_w_0().px(px(10.)).child(
                text(hl.text.clone())
                    .mono()
                    .size(11.)
                    .line_height(LineHeight::Px(line))
                    .color(cd)
                    .pre_wrap()
                    .break_anywhere()
                    .id(SharedString::from(format!(
                        "hl-{}-{}-{}",
                        hl.old, hl.new, hl.text
                    ))),
            ),
        )
        .into_any_element()
}

static COMMIT_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^committed\s+").expect("re"));
static COMMIT_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s+\([0-9a-f]{7,}\)\s*$").expect("re"));
static MERGE_COMMIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^merged commit ([0-9a-f]{7,40})(.*)$").expect("re"));
static BEHALF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(started reviewing|started work|finished reviewing|finished work|stopped work) on behalf of (.+?)(?: due to an error)?$").expect("re")
});

/// `commitTitle`.
pub(crate) fn commit_title(c: &PrComment) -> String {
    let t = COMMIT_PREFIX.replace(&c.body_md, "").to_string();
    COMMIT_SUFFIX.replace(&t, "").to_string()
}

/// `shortSha`.
pub(crate) fn short_sha(sha: &str) -> String {
    if sha.chars().count() > 7 {
        sha.chars().take(7).collect()
    } else {
        sha.to_string()
    }
}

/// `actionIcon`.
pub(crate) fn action_icon(c: &PrComment) -> IconName {
    let t = c.body_md.to_lowercase();
    if t.starts_with("committed") {
        IconName::GitCommit
    } else if t.starts_with("merged") {
        IconName::GitMerge
    } else if (t.contains("deleted the") || t.contains("restored the")) && t.contains("branch") {
        IconName::GitBranch
    } else if t.starts_with("closed") {
        IconName::CircleSlash
    } else if [
        "started review",
        "started work",
        "finished work",
        "finished review",
        "stopped work",
    ]
    .iter()
    .any(|k| t.contains(k))
    {
        IconName::GitBranch
    } else if t.contains("review") {
        IconName::Eye
    } else if t.contains("force-pushed") || t.contains("assigned") {
        IconName::GitBranch
    } else {
        IconName::GitPullRequest
    }
}

/// `isMergeAction`.
pub(crate) fn is_merge_action(c: &PrComment) -> bool {
    c.body_md.to_lowercase().starts_with("merged ")
}

/// `mergeCommitLink`: (short sha, rest, href).
pub(crate) fn merge_commit_link(c: &PrComment, pr_url: &str) -> Option<(String, String, String)> {
    let m = MERGE_COMMIT.captures(&c.body_md)?;
    let sha = m.get(1)?.as_str();
    let rest = m.get(2).map_or("", |r| r.as_str()).trim().to_string();
    let full = if c.commit_sha.trim().is_empty() {
        sha.to_string()
    } else {
        c.commit_sha.trim().to_string()
    };
    Some((
        short_sha(sha),
        rest,
        recur_forge::github::commit_url(pr_url, &full),
    ))
}

/// `actionBehalf`: (verb, whom).
pub(crate) fn action_behalf(c: &PrComment) -> Option<(String, String)> {
    let m = BEHALF.captures(&c.body_md)?;
    Some((
        m.get(1)?.as_str().to_string(),
        m.get(2)?.as_str().to_string(),
    ))
}

/// `shotURL`: an evidence screenshot path as its `/workspaces/…` URL.
pub(crate) fn shot_url(p: &str) -> String {
    p.find("/workspaces/")
        .map_or(String::new(), |i| p[i..].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(body: &str) -> PrComment {
        PrComment {
            body_md: body.into(),
            kind: "action".into(),
            ..Default::default()
        }
    }

    #[test]
    fn timeline_rules() {
        assert_eq!(
            commit_title(&act("committed Fix tests (abc1234)")),
            "Fix tests"
        );
        assert_eq!(short_sha("abcdef123456"), "abcdef1");
        assert_eq!(short_sha("abc"), "abc");
        assert_eq!(action_icon(&act("committed x")), IconName::GitCommit);
        assert_eq!(action_icon(&act("merged commit abc")), IconName::GitMerge);
        assert_eq!(
            action_icon(&act("deleted the x branch")),
            IconName::GitBranch
        );
        assert_eq!(action_icon(&act("closed this")), IconName::CircleSlash);
        assert_eq!(action_icon(&act("requested a review")), IconName::Eye);
        assert_eq!(
            action_icon(&act("force-pushed the branch")),
            IconName::GitBranch
        );
        assert_eq!(action_icon(&act("reopened")), IconName::GitPullRequest);
        assert!(is_merge_action(&act("Merged commit")));
        let mut m = act("merged commit a3f73e2 into main");
        m.commit_sha = "a3f73e2abcdef".into();
        assert_eq!(
            merge_commit_link(&m, "https://github.com/acme/app/pull/1"),
            Some((
                "a3f73e2".into(),
                "into main".into(),
                "https://github.com/acme/app/commit/a3f73e2abcdef".into()
            ))
        );
        assert_eq!(
            action_behalf(&act("started reviewing on behalf of avery due to an error")),
            Some(("started reviewing".into(), "avery".into()))
        );
        assert_eq!(
            shot_url("/x/workspaces/1/screenshots/a.png"),
            "/workspaces/1/screenshots/a.png"
        );
        assert_eq!(shot_url("/x/a.png"), "");
    }
}
