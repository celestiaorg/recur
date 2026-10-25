//! `LineNoteThread.svelte`: a posted line-note thread inside a diff (`.icmt-thread`): the root
//! note and its replies with reactions, reply / edit composers with the emoji and `@mention`
//! typeaheads, the ⋯ menu (Open in GitHub, Copy link, Quote reply, Edit, Delete) and resolve.

use recur_ui::guard::Guarded;
use recur_view_core::{ObserveState as _, Topics};
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::base::input::TextareaState;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, ClipboardItem, Context, DispatchPhase, Entity,
    Focusable, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled, Subscription,
    Window, canvas, deferred, div, img, px,
};
use recur_content::{MarkdownClass, MarkdownEvent, MarkdownView, Sheet};
use recur_domain::{Pr, PrComment};
use recur_engine::view::comment_thread::{
    PENDING_REPLY_PREFIX, ThreadEditorDraft, can_delete_pr_comment, can_edit_pr_comment,
    can_react_pr_comment, can_resolve_line_note, comment_source, is_dropped_comment,
    settle_posted_reply, thread_replies,
};
use recur_engine::view::js;
use recur_forge::github::comment_permalink;
use recur_ui::isolated::IsolatedArea;
use recur_ui::logic::menu_space::{more_menu_fixed_box, rects_overlap};
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::ToastKind;
use recur_ui::{IconName, Theme, icon, rgba};
use recur_view_chat::suggest::{SuggestAttach, attach_emoji, attach_user_mentions};
use recur_view_core::pr::{PrLifecycle, pr_lifecycle};
use recur_view_core::{AppState, call, toast};

use crate::autosize::PR_REPLY_AUTOSIZE;
use crate::avatar::avatar;
use crate::reactions::CommentReactions;
use crate::store::{ReviewStore, set_pr_thread_resolved};
use crate::style::{hsla, note_btn};

/// `prConversationLocked`: merged and closed PRs take no new comments.
pub fn pr_conversation_locked(pr: Option<&Pr>) -> bool {
    matches!(
        pr_lifecycle(pr),
        Some(PrLifecycle::Merged | PrLifecycle::Closed)
    )
}

/// `.cmt-kid .cm-md`: 12.5px on a 1.55 line inside `.chat-canvas`.
pub fn note_sheet() -> Sheet {
    Sheet {
        size: 12.5,
        leading: 1.55,
        ..Sheet::chat()
    }
}

/// Time since the epoch in ms (`Date.now()`).
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// `{ ...c, ...updated }`: the fields the service sent override the row's.
pub fn spread(c: &PrComment, updated: &PrComment) -> PrComment {
    let (Ok(serde_json::Value::Object(mut base)), Ok(serde_json::Value::Object(over))) =
        (serde_json::to_value(c), serde_json::to_value(updated))
    else {
        return updated.clone();
    };
    for (k, v) in over {
        base.insert(k, v);
    }
    serde_json::from_value(serde_json::Value::Object(base)).unwrap_or_else(|_| updated.clone())
}

type Area = Entity<TextareaState>;

struct Composer {
    area: Area,
    /// The textarea as an isolated field: a key renders it, not the thread or the stack.
    isolated: IsolatedArea,
    _attach: (
        Entity<SuggestAttach<gpui_kit::base::input::TextareaMode>>,
        Entity<SuggestAttach<gpui_kit::base::input::TextareaMode>>,
    ),
}

pub struct LineNoteThread {
    state: Entity<AppState>,
    root: PrComment,
    items: Vec<PrComment>,
    pr: Option<Pr>,
    reply_open: bool,
    reply: Option<Composer>,
    resolving: bool,
    more_id: String,
    more_trigger: Option<Bounds<Pixels>>,
    more_menu: Option<Bounds<Pixels>>,
    triggers: HashMap<String, Bounds<Pixels>>,
    editing_id: String,
    edit: Option<Composer>,
    saving: String,
    notes: HashMap<String, Entity<MarkdownView>>,
    reactions: HashMap<String, Entity<CommentReactions>>,
    /// The scroll pane that clips this thread (`overflowClipAncestor`), for the ⋯ menu.
    clip: Option<Rc<Cell<Option<Bounds<Pixels>>>>>,
    _subs: Vec<Subscription>,
}

impl LineNoteThread {
    pub fn new(
        state: Entity<AppState>,
        root: PrComment,
        items: Vec<PrComment>,
        pr: Option<Pr>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let held = ReviewStore::get(cx)
            .and_then(|s| s.drafts.get(&root.id).cloned())
            .unwrap_or_default();
        let mut subs = vec![cx.on_release(|this: &mut Self, cx| this.save_draft(cx))];
        subs.push(
            cx.observe_state(&state, Topics::NONE, |this: &mut Self, state, cx| {
                if this.more_id.is_empty() {
                    return;
                }
                if state.read(cx).app_overlay_open() {
                    this.close_more(cx);
                }
            }),
        );
        let mut me = LineNoteThread {
            state,
            root,
            items,
            pr,
            reply_open: held.reply_open,
            reply: None,
            resolving: false,
            more_id: String::new(),
            more_trigger: None,
            more_menu: None,
            triggers: HashMap::new(),
            editing_id: held.editing_id.clone(),
            edit: None,
            saving: String::new(),
            notes: HashMap::new(),
            reactions: HashMap::new(),
            clip: None,
            _subs: subs,
        };
        if held.reply_open || !held.reply.is_empty() {
            me.reply = Some(me.composer("Reply…", &held.reply, window, cx));
        }
        if !held.editing_id.is_empty() {
            me.edit = Some(me.composer("Edit comment", &held.edit, window, cx));
        }
        me
    }

    /// New props (`root`, `items`, `pr`).
    pub fn set(
        &mut self,
        root: PrComment,
        items: Vec<PrComment>,
        pr: Option<Pr>,
        cx: &mut Context<Self>,
    ) {
        if self.root != root || self.items != items || self.pr != pr {
            self.root = root;
            self.items = items;
            self.pr = pr;
            cx.notify();
        }
    }

    /// The scroll pane whose bounds clip the ⋯ trigger.
    pub fn set_clip(&mut self, clip: Option<Rc<Cell<Option<Bounds<Pixels>>>>>) {
        self.clip = clip;
    }

    pub fn root(&self) -> &PrComment {
        &self.root
    }

    pub fn reply_open(&self) -> bool {
        self.reply_open
    }

    pub fn editing_id(&self) -> &str {
        &self.editing_id
    }

    pub fn more_open(&self) -> &str {
        &self.more_id
    }

    /// The reply draft, for tests and hosts.
    pub fn reply_text(&self, cx: &App) -> String {
        self.reply
            .as_ref()
            .map_or(String::new(), |c| c.area.read(cx).value().to_string())
    }

    pub fn edit_text(&self, cx: &App) -> String {
        self.edit
            .as_ref()
            .map_or(String::new(), |c| c.area.read(cx).value().to_string())
    }

    fn save_draft(&mut self, cx: &mut App) {
        let reply = self
            .reply
            .as_ref()
            .map_or(String::new(), |c| c.area.read(cx).value().to_string());
        let edit = self
            .edit
            .as_ref()
            .map_or(String::new(), |c| c.area.read(cx).value().to_string());
        let draft = ThreadEditorDraft {
            reply_open: self.reply_open,
            reply,
            editing_id: self.editing_id.clone(),
            edit,
        };
        let id = self.root.id.clone();
        ReviewStore::global_mut(cx).drafts.set(&id, draft);
    }

    fn composer(
        &self,
        placeholder: &str,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Composer {
        let placeholder = SharedString::from(placeholder.to_string());
        let value = value.to_string();
        let area = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 6)
                .placeholder(placeholder)
                .default_value(value)
        });
        crate::style::style_textarea(&area, cx);
        let emoji = attach_emoji(&area, None, window, cx);
        let mentions = attach_user_mentions(&area, self.state.clone(), None, window, cx);
        Composer {
            isolated: IsolatedArea::new(&area, (1, 6), cx),
            area,
            _attach: (emoji, mentions),
        }
    }

    fn can_comment(&self) -> bool {
        !pr_conversation_locked(self.pr.as_ref())
    }

    fn replies(&self) -> Vec<PrComment> {
        thread_replies(&self.items, &self.root.id)
            .into_iter()
            .cloned()
            .collect()
    }

    fn has_more_actions(&self, c: &PrComment) -> bool {
        if can_edit_pr_comment(self.pr.as_ref(), Some(c))
            || can_delete_pr_comment(self.pr.as_ref(), Some(c))
        {
            return true;
        }
        if !comment_source(Some(c)).is_empty() {
            return true;
        }
        !self.permalink(Some(c)).is_empty()
    }

    fn permalink(&self, c: Option<&PrComment>) -> String {
        let Some(c) = c else { return String::new() };
        comment_permalink(self.pr.as_ref().map_or("", |p| p.url.as_str()), c)
    }

    fn focus_reply(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(r) = &self.reply {
            r.area.update(cx, |a, cx| a.focus(window, cx));
        }
    }

    pub fn open_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_comment() {
            return;
        }
        self.reply_open = true;
        if self.reply.is_none() {
            self.reply = Some(self.composer("Reply…", "", window, cx));
        }
        self.focus_reply(window, cx);
        cx.notify();
    }

    fn close_reply(&mut self, cx: &mut Context<Self>) {
        self.reply_open = false;
        cx.notify();
    }

    fn reply_value(&self, cx: &App) -> String {
        self.reply_text(cx)
    }

    fn patch_comments(&self, pr_id: &str, f: impl FnOnce(&mut Vec<PrComment>), cx: &mut App) {
        self.state.update(cx, |s, cx| {
            s.update_pr(
                pr_id,
                |p| {
                    let mut list = p.comments.take().unwrap_or_default();
                    f(&mut list);
                    p.comments = Some(list);
                },
                cx,
            )
        });
    }

    pub fn submit_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = js::trim(&self.reply_value(cx)).to_string();
        if body.is_empty() || !self.can_comment() {
            return;
        }
        let pr_id = self.state.read(cx).selected_pr_id.clone();
        let login = self
            .pr
            .as_ref()
            .map_or(String::new(), |p| p.viewer_login.clone());
        let author = if login.is_empty() {
            "You".to_string()
        } else {
            login.clone()
        };
        let root = &self.root;
        let pending = PrComment {
            id: format!("{PENDING_REPLY_PREFIX}{}", now_ms()),
            code: author.chars().take(2).collect::<String>().to_uppercase(),
            author: author.clone(),
            author_login: login,
            time: "now".into(),
            body_md: body.clone(),
            verb: "commented".into(),
            kind: "comment".into(),
            in_reply_to_id: root.id.clone(),
            parent_id: root.parent_id.clone(),
            path: root.path.clone(),
            line: root.line,
            side: root.side.clone(),
            thread_id: if root.thread_id.is_empty() {
                root.id.clone()
            } else {
                root.thread_id.clone()
            },
            resolved: root.resolved,
            ..Default::default()
        };
        let pending_id = pending.id.clone();
        self.patch_comments(&pr_id, |list| list.push(pending), cx);
        self.reply_open = false;
        if let Some(r) = &self.reply {
            r.area.update(cx, |a, cx| a.set_value("", window, cx));
        }
        cx.notify();
        let services = self.state.read(cx).services().clone();
        let root_id = self.root.id.clone();
        cx.spawn(async move |this, cx| {
            let posted = call(
                cx,
                services.prs.reply_pr_comment(&pr_id, &root_id, &body),
                "Could not reply",
            )
            .await;
            let _ = this.update(cx, |t, cx| match posted.filter(|p| !p.id.is_empty()) {
                Some(posted) => {
                    t.patch_comments(
                        &pr_id,
                        |list| {
                            *list =
                                settle_posted_reply(std::mem::take(list), &pending_id, Some(posted))
                        },
                        cx,
                    );
                    toast(ToastKind::Success, "Reply posted", cx);
                }
                None => t.patch_comments(
                    &pr_id,
                    |list| *list = settle_posted_reply(std::mem::take(list), &pending_id, None),
                    cx,
                ),
            });
        })
        .detach();
    }

    pub fn set_resolved(&mut self, resolved: bool, cx: &mut Context<Self>) {
        if self.resolving {
            return;
        }
        self.resolving = true;
        cx.notify();
        let root = self.root.clone();
        let task = set_pr_thread_resolved(&self.state, &root, resolved, cx);
        cx.spawn(async move |this, cx| {
            task.await;
            let _ = this.update(cx, |t, cx| {
                t.resolving = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn close_more(&mut self, cx: &mut Context<Self>) {
        if !self.more_id.is_empty() {
            self.more_id.clear();
            self.more_trigger = None;
            self.more_menu = None;
            cx.notify();
        }
    }

    pub fn toggle_more(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.more_id == id {
            self.close_more(cx);
            return;
        }
        self.more_id = id.to_string();
        self.more_trigger = self.triggers.get(id).copied();
        cx.notify();
    }

    fn more_target(&self) -> Option<PrComment> {
        if self.more_id.is_empty() {
            return None;
        }
        if self.root.id == self.more_id {
            return Some(self.root.clone());
        }
        self.replies().into_iter().find(|c| c.id == self.more_id)
    }

    pub fn quote_reply(&mut self, c: &PrComment, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_comment() {
            return;
        }
        let src = comment_source(Some(c));
        if src.is_empty() {
            return;
        }
        let quoted = src
            .split('\n')
            .map(|l| format!("> {l}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n\n";
        let prev = if self.reply_open {
            self.reply_value(cx)
        } else {
            String::new()
        };
        if self.reply.is_none() {
            self.reply = Some(self.composer("Reply…", "", window, cx));
        }
        if let Some(r) = &self.reply {
            let v = quoted + &prev;
            r.area.update(cx, |a, cx| a.set_value(v, window, cx));
        }
        self.reply_open = true;
        self.focus_reply(window, cx);
        cx.notify();
    }

    pub fn start_edit(&mut self, c: &PrComment, window: &mut Window, cx: &mut Context<Self>) {
        self.editing_id = c.id.clone();
        let src = comment_source(Some(c));
        match &self.edit {
            Some(e) => e.area.update(cx, |a, cx| a.set_value(src, window, cx)),
            None => self.edit = Some(self.composer("Edit comment", &src, window, cx)),
        }
        cx.notify();
    }

    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        self.editing_id.clear();
        cx.notify();
    }

    pub fn save_edit(&mut self, id: &str, cx: &mut Context<Self>) {
        let body = js::trim(&self.edit_text(cx)).to_string();
        if body.is_empty() || !self.saving.is_empty() {
            return;
        }
        self.saving = id.to_string();
        cx.notify();
        let services = self.state.read(cx).services().clone();
        let pr_id = self.state.read(cx).selected_pr_id.clone();
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let updated = call(
                cx,
                services.prs.update_pr_comment(&pr_id, &id, &body),
                "Could not update comment",
            )
            .await;
            let _ = this.update(cx, |t, cx| {
                t.saving.clear();
                cx.notify();
                let Some(updated) = updated.filter(|u| !u.id.is_empty()) else {
                    return;
                };
                t.patch_comments(
                    &pr_id,
                    |list| {
                        for c in list.iter_mut().filter(|c| c.id == id) {
                            let mut next = spread(c, &updated);
                            next.body_md = body.clone();
                            *c = next;
                        }
                    },
                    cx,
                );
                t.editing_id.clear();
                toast(ToastKind::Success, "Comment updated", cx);
            });
        })
        .detach();
    }

    pub fn delete_comment(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.saving.is_empty() || id.is_empty() {
            return;
        }
        let pr_id = self.state.read(cx).selected_pr_id.clone();
        let from_pr: Vec<PrComment> = self
            .state
            .read(cx)
            .pr(&pr_id)
            .and_then(|p| p.comments.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|c| is_dropped_comment(c, id))
            .collect();
        if from_pr.is_empty() {
            return;
        }
        self.saving = format!("del-{id}");
        let idc = id.to_string();
        self.patch_comments(
            &pr_id,
            |list| list.retain(|c| !is_dropped_comment(c, &idc)),
            cx,
        );
        if self.editing_id == id {
            self.editing_id.clear();
        }
        cx.notify();
        let services = self.state.read(cx).services().clone();
        cx.spawn(async move |this, cx| {
            let updated = call(
                cx,
                services.prs.delete_pr_comment(&pr_id, &idc),
                "Could not delete comment",
            )
            .await;
            let _ = this.update(cx, |t, cx| {
                t.saving.clear();
                cx.notify();
                let Some(updated) = updated.filter(|p| !p.id.is_empty()) else {
                    t.patch_comments(&pr_id, |list| list.extend(from_pr), cx);
                    return;
                };
                t.state.update(cx, |s, cx| {
                    let uid = updated.id.clone();
                    s.update_pr(&uid, |p| *p = updated, cx)
                });
                toast(ToastKind::Success, "Comment deleted", cx);
            });
        })
        .detach();
    }

    fn more_action(&mut self, action: MoreAction, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.more_target();
        let link = self.permalink(target.as_ref());
        self.close_more(cx);
        match action {
            MoreAction::OpenGitHub => {
                if !link.is_empty() {
                    recur_content::links::open_external_url(&link, cx);
                }
            }
            MoreAction::CopyLink => {
                if !link.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(link));
                    toast(ToastKind::Info, "Copied link", cx);
                }
            }
            MoreAction::Quote => {
                if let Some(c) = target {
                    self.quote_reply(&c, window, cx);
                }
            }
            MoreAction::Edit => {
                if let Some(c) = target {
                    self.start_edit(&c, window, cx);
                }
            }
            MoreAction::Delete => {
                if let Some(c) = target {
                    self.delete_comment(&c.id, cx);
                }
            }
        }
    }

    fn note_md(&mut self, c: &PrComment, cx: &mut Context<Self>) -> Entity<MarkdownView> {
        let body = comment_source(Some(c));
        if let Some(v) = self.notes.get(&c.id) {
            v.update(cx, |v, cx| v.set_text(body, cx));
            return v.clone();
        }
        let v = cx.new(|cx| {
            let mut v = MarkdownView::new(body, MarkdownClass::ChatCanvas, false, cx);
            v.set_sheet(note_sheet(), cx);
            v
        });
        let state = self.state.clone();
        let sub = cx.guarded_subscribe(&v, move |_, _, ev: &MarkdownEvent, cx| match ev {
            MarkdownEvent::LightboxOpen(open) => {
                let open = *open;
                state.update(cx, |s, cx| {
                    s.set_chat_occluder("attachment-lightbox", open, cx)
                });
            }
            MarkdownEvent::Toast { error, message } => {
                let kind = if *error {
                    ToastKind::Error
                } else {
                    ToastKind::Info
                };
                toast(kind, message.clone(), cx);
            }
            MarkdownEvent::TextChanged => {}
        });
        self._subs.push(sub);
        self.notes.insert(c.id.clone(), v.clone());
        v
    }

    fn reactions_for(&mut self, c: &PrComment, cx: &mut Context<Self>) -> Entity<CommentReactions> {
        let can = can_react_pr_comment(self.pr.as_ref(), Some(c));
        if let Some(r) = self.reactions.get(&c.id) {
            r.update(cx, |r, cx| r.set(c.clone(), can, cx));
            return r.clone();
        }
        let state = self.state.clone();
        let r = cx.new(|_| CommentReactions::new(state, c.clone(), can));
        self.reactions.insert(c.id.clone(), r.clone());
        r
    }

    fn compose_box(
        &self,
        which: Which,
        c_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let composer = match which {
            Which::Reply => self.reply.as_ref(),
            Which::Edit => self.edit.as_ref(),
        };
        let Some(composer) = composer else {
            return div().into_any_element();
        };
        let area = composer.area.clone();
        let value = area.read(cx).value().to_string();
        let empty = js::trim(&value).is_empty();
        let saving_this = which == Which::Edit && self.saving == c_id;
        let line = 12.5 * 1.5;
        let c_id2 = c_id.to_string();
        let input = div()
            .id(SharedString::from(format!(
                "{}-{}-input",
                which.key(),
                self.root.id
            )))
            .debug_selector(|| format!("note-input:{}", which.key()))
            .min_h(px(PR_REPLY_AUTOSIZE.min_px))
            .max_h(px(PR_REPLY_AUTOSIZE.max_px))
            .overflow_hidden()
            .pt(px(8.))
            .px(px(10.))
            .pb(px(4.))
            .text_size(px(12.5))
            .line_height(px(line))
            .text_color(hsla(t.text))
            .when(saving_this, |d| d.opacity(0.55))
            .key_context("TextEntry")
            .on_key_down(
                cx.guarded_listener(move |this, ev: &KeyDownEvent, window, cx| {
                    let k = &ev.keystroke;
                    let cmd = k.modifiers.platform || k.modifiers.control;
                    if k.key == "enter" && cmd {
                        cx.stop_propagation();
                        match which {
                            Which::Reply => this.submit_reply(window, cx),
                            Which::Edit => this.save_edit(&c_id2, cx),
                        }
                    } else if k.key == "escape" {
                        cx.stop_propagation();
                        match which {
                            Which::Reply => this.close_reply(cx),
                            Which::Edit => this.cancel_edit(cx),
                        }
                    }
                }),
            )
            .child(composer.isolated.element(line, cx));
        let hint = div()
            .flex()
            .items_center()
            .gap(px(5.))
            .text_color(hsla(t.faint))
            .child(recur_ui::kbd::kbd("⌘↵").size(11.).color(t.faint))
            .child(text("to save").size(11.).nowrap());
        let backdrop = t.bg;
        let lh = recur_ui::style::button_label("x", 11., 400.)
            .measure(None, window)
            .2
            .height;
        let (cancel, primary) = match which {
            Which::Reply => (
                note_btn(
                    SharedString::from(format!("reply-cancel-{}", self.root.id)),
                    "Cancel",
                    false,
                    false,
                    backdrop,
                    lh,
                    cx,
                )
                .debug_selector(|| "note-cancel".into())
                .on_click(cx.guarded_listener(|this, _, _, cx| this.close_reply(cx))),
                note_btn(
                    SharedString::from(format!("reply-go-{}", self.root.id)),
                    "Reply",
                    true,
                    empty,
                    backdrop,
                    lh,
                    cx,
                )
                .debug_selector(|| "note-reply".into())
                .when(!empty, |d| {
                    d.on_click(
                        cx.guarded_listener(|this, _, window, cx| this.submit_reply(window, cx)),
                    )
                }),
            ),
            Which::Edit => {
                let id = c_id.to_string();
                (
                    note_btn(
                        SharedString::from(format!("edit-cancel-{c_id}")),
                        "Cancel",
                        false,
                        saving_this,
                        backdrop,
                        lh,
                        cx,
                    )
                    .debug_selector(|| "note-cancel".into())
                    .when(!saving_this, |d| {
                        d.on_click(cx.guarded_listener(|this, _, _, cx| this.cancel_edit(cx)))
                    }),
                    note_btn(
                        SharedString::from(format!("edit-save-{c_id}")),
                        if saving_this { "Saving…" } else { "Save" },
                        true,
                        empty || saving_this,
                        backdrop,
                        lh,
                        cx,
                    )
                    .debug_selector(|| "note-save".into())
                    .when(!empty && !saving_this, |d| {
                        d.on_click(
                            cx.guarded_listener(move |this, _, _, cx| this.save_edit(&id, cx)),
                        )
                    }),
                )
            }
        };
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
                            .child(primary),
                    ),
            )
            .into_any_element()
    }

    fn note(
        &mut self,
        c: &PrComment,
        first: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let href = self.permalink(Some(c));
        let time = c.time.clone();
        let editing = self.editing_id == c.id;
        let head_line = 12.5 * 1.45;
        let mut ch = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .mb(px(5.))
            .text_color(hsla(t.muted))
            .child(
                text(c.author.clone())
                    .size(12.5)
                    .weight(600.)
                    .line_height(LineHeight::Px(head_line))
                    .color(t.text)
                    .nowrap(),
            );
        if !time.is_empty() {
            let ct = text(time.clone())
                .mono()
                .size(9.5)
                .line_height(LineHeight::Scale(1.45))
                .color(t.faint)
                .nowrap();
            if href.is_empty() {
                ch = ch.child(ct);
            } else {
                let link = href.clone();
                ch = ch.child(titled(
                    SharedString::from(format!("ct-{}", c.id)),
                    "Open on GitHub",
                    div()
                        .id(SharedString::from(format!("ct-a-{}", c.id)))
                        .cursor_pointer()
                        .text_color(hsla(t.faint))
                        .hover(|s| s.text_color(hsla(t.muted)))
                        .child(ct)
                        .on_click(move |_, _, cx| {
                            recur_content::links::open_external_url(&link, cx)
                        }),
                ));
            }
        }
        if !editing && self.has_more_actions(c) {
            let open = self.more_id == c.id;
            let id = c.id.clone();
            let me = cx.entity().downgrade();
            let probe_id = c.id.clone();
            let probe = canvas(
                move |b, _, cx| {
                    if let Some(me) = me.upgrade() {
                        me.update(cx, |t, _| {
                            t.triggers.insert(probe_id.clone(), b);
                            if t.more_id == probe_id {
                                t.more_trigger = Some(b);
                            }
                        });
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0();
            let btn = div()
                .id(SharedString::from(format!("more-{}", c.id)))
                .debug_selector(|| format!("cmt-more:{id}"))
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .w(px(22.))
                .h(px(22.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_color(hsla(if open { t.text } else { t.faint }))
                .when(open, |d| d.bg(hsla(t.pane_2)))
                .hover(|s| s.text_color(hsla(t.text)).bg(hsla(t.pane_2)))
                .child(icon(IconName::Ellipsis).size(px(13.)))
                .child(probe)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.guarded_listener({
                    let id = c.id.clone();
                    move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_more(&id, cx)
                    }
                }));
            ch = ch.child(div().flex().flex_none().ml_auto().child(titled(
                SharedString::from(format!("more-t-{}", c.id)),
                "More",
                btn,
            )));
        }
        let mut cc = div().flex_1().min_w_0().child(ch);
        if editing {
            cc = cc.child(self.compose_box(Which::Edit, &c.id, window, cx));
        } else if !comment_source(Some(c)).is_empty() {
            let md = self.note_md(c, cx);
            cc = cc.child(div().text_color(hsla(t.text)).child(md));
        }
        if !editing {
            let r = self.reactions_for(c, cx);
            cc = cc.child(r);
        }
        div()
            .debug_selector(|| format!("cmt-kid-note:{}", c.id))
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

    fn more_menu_el(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.more_id.is_empty() {
            return None;
        }
        let trigger = self.more_trigger?;
        let t = Theme::of(cx).tokens;
        let target = self.more_target();
        let can_quote = self.can_comment()
            && target
                .as_ref()
                .is_some_and(|c| !comment_source(Some(c)).is_empty());
        let link = self.permalink(target.as_ref());
        let vp = window.viewport_size();
        let (top, right) = more_menu_fixed_box(
            Some(f32::from(trigger.origin.y)),
            f32::from(trigger.bottom()),
            f32::from(trigger.right()),
            self.more_menu.map_or(0., |b| f32::from(b.size.height)),
            (f32::from(vp.width), f32::from(vp.height)),
        );
        let item = |id: &'static str,
                    name: IconName,
                    label: &'static str,
                    danger: bool,
                    action: MoreAction,
                    cx: &mut Context<Self>| {
            let fg = if danger { t.coral } else { t.text };
            let ic = if danger { t.coral } else { t.faint };
            div()
                .id(SharedString::from(format!("{id}-{}", self.root.id)))
                .debug_selector(move || format!("menuitem:{label}"))
                .flex()
                .items_center()
                .gap(px(8.))
                .w_full()
                .py(px(7.))
                .px(px(10.))
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|s| s.bg(hsla(t.pane_2)))
                .child(icon(name).size(px(13.)).color(hsla(ic)))
                .child(
                    recur_ui::style::button_label(label, 12.5, 500.)
                        .color(fg)
                        .nowrap(),
                )
                .on_click(cx.guarded_listener(move |this, _, window, cx| {
                    this.more_action(action, window, cx)
                }))
                .into_any_element()
        };
        let mut items = Vec::new();
        if !link.is_empty() {
            items.push(item(
                "more-gh",
                IconName::Github,
                "Open in GitHub",
                false,
                MoreAction::OpenGitHub,
                cx,
            ));
            items.push(item(
                "more-copy",
                IconName::Copy,
                "Copy link",
                false,
                MoreAction::CopyLink,
                cx,
            ));
        }
        if can_quote {
            items.push(item(
                "more-quote",
                IconName::Quote,
                "Quote reply",
                false,
                MoreAction::Quote,
                cx,
            ));
        }
        if target
            .as_ref()
            .is_some_and(|c| can_edit_pr_comment(self.pr.as_ref(), Some(c)))
        {
            items.push(item(
                "more-edit",
                IconName::Pencil,
                "Edit",
                false,
                MoreAction::Edit,
                cx,
            ));
        }
        if target
            .as_ref()
            .is_some_and(|c| can_delete_pr_comment(self.pr.as_ref(), Some(c)))
        {
            items.push(item(
                "more-del",
                IconName::Trash,
                "Delete",
                true,
                MoreAction::Delete,
                cx,
            ));
        }
        let me = cx.entity().downgrade();
        let probe = canvas(
            move |b, _, cx| {
                if let Some(me) = me.upgrade() {
                    me.update(cx, |t, cx| {
                        if t.more_menu.map(|m| m.size) != Some(b.size) {
                            t.more_menu = Some(b);
                            cx.notify();
                        }
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let card = recur_ui::style::card(t.rail, t.line, 8.)
            .min_w(px(168.))
            .p(px(4.))
            .flex()
            .flex_col()
            .children(items)
            .child(probe)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        let panel = with_shadow(
            card,
            &[css_box_shadow(0., 6., 18., 0., rgba(0, 0, 0, 0.3))],
            8.,
        );
        let left_of = f32::from(vp.width) - right;
        let w = self.more_menu.map_or(168., |b| f32::from(b.size.width));
        Some(
            deferred(recur_ui::style::anchored_at(left_of - w, top, panel))
                .with_priority(40)
                .into_any_element(),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    Reply,
    Edit,
}

impl Which {
    fn key(self) -> &'static str {
        match self {
            Which::Reply => "reply",
            Which::Edit => "edit",
        }
    }
}

#[derive(Clone, Copy)]
enum MoreAction {
    OpenGitHub,
    CopyLink,
    Quote,
    Edit,
    Delete,
}

impl LineNoteThread {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let root = self.root.clone();
        let replies = self.replies();
        let can_resolve = can_resolve_line_note(self.pr.as_ref(), Some(&root));
        let can_comment = self.can_comment();
        let lh = recur_ui::style::button_label("x", 11., 400.)
            .measure(None, window)
            .2
            .height;
        let backdrop = if root.resolved { t.pane_2 } else { t.pane };
        let mut kid = div()
            .debug_selector(|| format!("cmt-kid:{}", self.root.id))
            .relative()
            .border_1()
            .border_color(hsla(t.line))
            .rounded(px(8.))
            .overflow_hidden()
            .child(recur_ui::style::inset_background(backdrop, 8., 1., t.line));
        if root.resolved {
            let mut bar = div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.))
                .pt(px(8.))
                .px(px(12.))
                .pb(px(10.))
                .text_color(hsla(t.faint))
                .debug_selector({
                    let r = root.id.clone();
                    move || format!("cmt-resolved-bar:{r}")
                })
                .child(text("Resolved").size(12.5).nowrap());
            if can_resolve {
                bar = bar.child(
                    note_btn(
                        SharedString::from(format!("unresolve-{}", root.id)),
                        "Unresolve",
                        false,
                        self.resolving,
                        backdrop,
                        lh,
                        cx,
                    )
                    .debug_selector({
                        let r = root.id.clone();
                        move || format!("cmt-unresolve:{r}")
                    })
                    .when(!self.resolving, |d| {
                        d.on_click(
                            cx.guarded_listener(|this, _, _, cx| this.set_resolved(false, cx)),
                        )
                    }),
                );
            }
            kid = kid.child(bar);
        }
        kid = kid.child(self.note(&root, true, window, cx));
        for r in &replies {
            kid = kid.child(self.note(r, false, window, cx));
        }
        let mut foot = div().pb(px(10.)).px(px(12.));
        if self.reply_open && can_comment {
            foot = foot.child(self.compose_box(Which::Reply, "", window, cx));
        } else if can_comment {
            foot = foot.child(
                div()
                    .id(SharedString::from(format!("reply-ph-{}", root.id)))
                    .debug_selector({
                        let r = root.id.clone();
                        move || format!("cmt-reply-ph:{r}")
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
                    .hover(|s| s.text_color(hsla(t.muted)).border_color(hsla(t.muted)))
                    .child(recur_ui::style::button_label("Reply…", 12.5, 400.))
                    .on_click(
                        cx.guarded_listener(|this, _, window, cx| this.open_reply(window, cx)),
                    ),
            );
        }
        if can_resolve && !root.resolved {
            foot = foot.child(
                div().flex().justify_end().mt(px(8.)).child(
                    note_btn(
                        SharedString::from(format!("resolve-{}", root.id)),
                        "Resolve conversation",
                        false,
                        self.resolving,
                        backdrop,
                        lh,
                        cx,
                    )
                    .debug_selector({
                        let r = root.id.clone();
                        move || format!("cmt-resolve:{r}")
                    })
                    .when(!self.resolving, |d| {
                        d.on_click(
                            cx.guarded_listener(|this, _, _, cx| this.set_resolved(true, cx)),
                        )
                    }),
                ),
            );
        }
        kid = kid.child(foot);
        // `followMoreMenu`: the menu closes once its trigger leaves the clipping pane.
        if !self.more_id.is_empty()
            && let (Some(trigger), Some(clip)) =
                (self.more_trigger, self.clip.as_ref().and_then(|c| c.get()))
        {
            let b = |r: Bounds<Pixels>| {
                (
                    f32::from(r.top()),
                    f32::from(r.right()),
                    f32::from(r.bottom()),
                    f32::from(r.left()),
                )
            };
            if !rects_overlap(b(trigger), b(clip)) {
                self.more_id.clear();
                self.more_trigger = None;
            }
        }
        let menu = self.more_menu_el(window, cx);
        let open = !self.more_id.is_empty();
        let me = cx.entity().downgrade();
        let listeners = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                if !open {
                    return;
                }
                let m1 = me.clone();
                window.on_mouse_event(move |e: &MouseDownEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    if let Some(me) = m1.upgrade() {
                        me.update(cx, |t, cx| {
                            if t.more_menu.is_some_and(|m| m.contains(&e.position)) {
                                return;
                            }
                            if t.triggers
                                .get(&t.more_id)
                                .is_some_and(|b| b.contains(&e.position))
                            {
                                return;
                            }
                            t.close_more(cx);
                        });
                    }
                });
                let m2 = me.clone();
                window.on_key_event(move |e: &KeyDownEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture || e.keystroke.key != "escape" {
                        return;
                    }
                    if let Some(me) = m2.upgrade() {
                        me.update(cx, |t, cx| {
                            if !t.more_id.is_empty() {
                                cx.stop_propagation();
                                t.close_more(cx);
                            }
                        });
                    }
                });
            },
        )
        .absolute()
        .size_0();
        div()
            .debug_selector(|| format!("icmt-thread-inner:{}", self.root.id))
            .relative()
            .child(kid)
            .child(listeners)
            .children(menu)
    }
}

impl Render for LineNoteThread {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// Keeps `Focusable` reachable for hosts that focus the reply box.
impl LineNoteThread {
    pub fn reply_focus(&self, cx: &App) -> Option<gpui_kit::FocusHandle> {
        self.reply
            .as_ref()
            .map(|r| r.area.read(cx).focus_handle(cx))
    }

    pub fn reply_area(&self) -> Option<Entity<TextareaState>> {
        self.reply.as_ref().map(|r| r.area.clone())
    }

    pub fn edit_area(&self) -> Option<Entity<TextareaState>> {
        self.edit.as_ref().map(|r| r.area.clone())
    }

    /// Avatar `<img>` for a note; an empty URL draws nothing.
    pub fn avatar_img(url: &str) -> Option<gpui_kit::Img> {
        (!url.is_empty()).then(|| img(SharedString::from(url.to_string())))
    }
}
