//! `Briefing.svelte` (the Inbox): one list of workspace cards in Needs you / Working / Pick up
//! buckets with their unread notifications, the Elsewhere notifications that have no card,
//! label-style filters, and the Ask Recur dock on the left. The dock's visibility
//! (`inboxChatOpen`, `recur.askRecurOpen`) lives here; below 1100px the dock is a drawer.

use recur_ui::guard::Guarded;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement,
    IntoElement, KeystrokeEvent, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, point, px,
};
use recur_domain::{InboxItem, WorkspaceBrief};
use recur_ui::text::TextStyled;
use recur_ui::tooltip::titled;
use recur_ui::widgets::{
    Axis, Edge, Select, SelectEvent, SelectOption, TriggerStyle, resize_handle,
};
use recur_ui::{IconName, Theme, icon};
use recur_view_chat::app_chat::{AppChat, AppChatEvent};
use recur_view_chat::html_inline::{InlineStyle, inline_html_text};
use recur_view_core::prefs::PanelKey;
use recur_view_core::{ViewContext, ViewName};

use crate::style::{
    bucket_head, clear_button, empty, empty_desc, filter_seg, filters_bar, hsla, ibrow, ity,
    more_button, seg_count_btn, udot,
};

/// `recur.briefingFilters`: the unread / project scope (the bucket chip is not kept).
pub const FILTERS_KEY: &str = "recur.briefingFilters";
/// `recur.askRecurOpen`.
pub const ASK_OPEN_KEY: &str = "recur.askRecurOpen";
/// Below this the Ask dock is a drawer over the list.
pub const WIDE_MIN: f32 = 1100.;
/// Pick up shows this many until expanded.
pub const PICKUP_PREVIEW: usize = 8;

/// `bucketFilter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bucket {
    All,
    Needs,
    Working,
    Pickup,
}

impl Bucket {
    fn key(self) -> &'static str {
        match self {
            Bucket::All => "",
            Bucket::Needs => "needs",
            Bucket::Working => "working",
            Bucket::Pickup => "pickup",
        }
    }
}

/// The remembered scope: `{ unreadOnly, projects }` (an older `project` string reads as one
/// project).
pub fn parse_saved_filters(raw: Option<&str>) -> (bool, Vec<String>) {
    let Some(v) = raw.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()) else {
        return (false, Vec::new());
    };
    let unread = v
        .get("unreadOnly")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    let projects = match v.get("projects").and_then(|x| x.as_array()) {
        Some(a) => a
            .iter()
            .filter_map(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        None => v
            .get("project")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| vec![s.to_string()])
            .unwrap_or_default(),
    };
    (unread, projects)
}

/// `inboxByWorkspace`: the unread notifications of each workspace.
pub fn inbox_by_workspace(inbox: &[InboxItem]) -> BTreeMap<String, Vec<InboxItem>> {
    let mut m: BTreeMap<String, Vec<InboxItem>> = BTreeMap::new();
    for i in inbox {
        if i.unread && !i.workspace_id.is_empty() {
            m.entry(i.workspace_id.clone()).or_default().push(i.clone());
        }
    }
    m
}

/// `rowIcons`.
fn row_icon(kind: &str) -> Option<IconName> {
    Some(match kind {
        "pause" => IconName::CirclePause,
        "pr" => IconName::GitPullRequest,
        "loop" => IconName::RefreshCw,
        "merge" => IconName::Check,
        "chat" => IconName::MessageCode,
        _ => return None,
    })
}

pub struct BriefingView {
    vcx: ViewContext,
    chat: Entity<AppChat>,
    chat_open: bool,
    bucket: Bucket,
    unread_only: bool,
    project_filter: Vec<String>,
    pickup_expanded: bool,
    project_select: Entity<Select>,
    scroll: ScrollHandle,
    resize_focus: FocusHandle,
    hover_row: Option<String>,
    epoch: Instant,
    /// The bucket to bring to the top once laid out (`briefingFocus`).
    pending_focus: Option<String>,
    _subs: Vec<Subscription>,
}

impl BriefingView {
    pub fn new(vcx: &ViewContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (chat_open, (unread_only, project_filter)) = {
            let s = vcx.state.read(cx);
            (
                s.storage().get(ASK_OPEN_KEY) != Some("0"),
                parse_saved_filters(s.storage().get(FILTERS_KEY)),
            )
        };
        let on = vcx.state.read(cx).view == ViewName::Briefing;
        let chat = cx.new(|cx| AppChat::new(vcx, chat_open && on, window, cx));
        let values: Vec<SharedString> = project_filter.iter().map(|s| s.clone().into()).collect();
        let project_select = cx.new(|cx| {
            Select::new(Vec::new(), window, cx)
                .label("Filter by project")
                .placeholder("All projects")
                .multiple()
                .ghost()
                .trigger_style(TriggerStyle {
                    value_shrink: true,
                    ..Default::default()
                })
                .with_values(values)
        });
        let subs = vec![
            cx.observe_in(&vcx.state, window, |this, _, window, cx| {
                this.sync(window, cx)
            }),
            cx.subscribe(&chat, |this, _, ev: &AppChatEvent, cx| {
                if matches!(ev, AppChatEvent::Hide) {
                    this.set_chat_open(false, cx);
                }
            }),
            cx.subscribe(&project_select, |this, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Values(ids) = ev {
                    this.project_filter = ids.iter().map(|s| s.to_string()).collect();
                    this.save_filters(cx);
                    cx.notify();
                }
            }),
            cx.observe_keystrokes(|this, ev: &KeystrokeEvent, window, cx| {
                this.on_window_key(ev, window, cx)
            }),
        ];
        let mut me = BriefingView {
            vcx: vcx.clone(),
            chat,
            chat_open,
            bucket: Bucket::All,
            unread_only,
            project_filter,
            pickup_expanded: false,
            project_select,
            scroll: ScrollHandle::new(),
            resize_focus: cx.focus_handle(),
            hover_row: None,
            epoch: Instant::now(),
            pending_focus: None,
            _subs: subs,
        };
        me.sync(window, cx);
        me
    }

    pub fn chat(&self) -> &Entity<AppChat> {
        &self.chat
    }

    pub fn chat_open(&self) -> bool {
        self.chat_open
    }

    /// `setInboxChatOpen`.
    pub fn set_chat_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.chat_open = open;
        self.vcx.state.update(cx, |s, _| {
            s.storage_mut()
                .set(ASK_OPEN_KEY, if open { "1" } else { "0" }.to_string())
        });
        self.sync_visible(cx);
        cx.notify();
    }

    fn sync_visible(&mut self, cx: &mut Context<Self>) {
        let visible = self.chat_open && self.vcx.state.read(cx).view == ViewName::Briefing;
        self.chat.update(cx, |c, cx| c.set_visible(visible, cx));
    }

    fn save_filters(&mut self, cx: &mut Context<Self>) {
        let v =
            serde_json::json!({ "unreadOnly": self.unread_only, "projects": self.project_filter });
        self.vcx
            .state
            .update(cx, |s, _| s.storage_mut().set(FILTERS_KEY, v.to_string()));
    }

    fn briefs_all(&self, cx: &App) -> Vec<WorkspaceBrief> {
        self.vcx
            .state
            .read(cx)
            .briefing
            .workspaces
            .clone()
            .unwrap_or_default()
    }

    /// `projectOptions`: the projects with cards, in card order.
    fn project_options(&self, cx: &App) -> Vec<(String, String)> {
        let mut seen: Vec<(String, String)> = Vec::new();
        for w in self.briefs_all(cx) {
            if !w.project_id.is_empty() && !seen.iter().any(|(id, _)| *id == w.project_id) {
                let label = if w.project_name.is_empty() {
                    w.project_id.clone()
                } else {
                    w.project_name.clone()
                };
                seen.push((w.project_id.clone(), label));
            }
        }
        seen
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_visible(cx);
        // A remembered project that no longer has cards would be an invisible filter.
        if !self.project_filter.is_empty() && !self.briefs_all(cx).is_empty() {
            let known: BTreeSet<String> = self
                .project_options(cx)
                .into_iter()
                .map(|(id, _)| id)
                .collect();
            let next: Vec<String> = self
                .project_filter
                .iter()
                .filter(|id| known.contains(*id))
                .cloned()
                .collect();
            if next.len() != self.project_filter.len() {
                self.project_filter = next.clone();
                let values = next.into_iter().map(SharedString::from).collect();
                self.project_select
                    .update(cx, |s, cx| s.set_values(values, cx));
                self.save_filters(cx);
            }
        }
        let focus = self.vcx.state.read(cx).briefing_focus.clone();
        if !focus.is_empty() {
            // A footer / palette jump must reach its section: drop the filters that could
            // hide it (the project scope is a deliberate choice).
            self.bucket = Bucket::All;
            if self.unread_only {
                self.unread_only = false;
                self.save_filters(cx);
            }
            self.pending_focus = Some(focus);
            self.vcx.state.update(cx, |s, _| s.briefing_focus.clear());
            let me = cx.entity().downgrade();
            window.on_next_frame(move |window, cx| {
                let _ = me.update(cx, |v, cx| v.reveal_focus(window, cx));
            });
        }
        cx.notify();
    }

    /// `scrollIntoView({ block: 'start' })` of the focused bucket.
    fn reveal_focus(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(focus) = self.pending_focus.take() else {
            return;
        };
        let order = self.section_order(cx);
        let Some(ix) = order.iter().position(|k| *k == focus) else {
            return;
        };
        if let (Some(item), view) = (self.scroll.bounds_for_item(ix), self.scroll.bounds()) {
            let cur = self.scroll.offset();
            let top = item.origin.y - view.origin.y - cur.y;
            self.scroll.set_offset(point(cur.x, -top));
            cx.notify();
        }
    }

    /// The overview's direct children in order (the scroll handle's items).
    fn section_order(&self, cx: &App) -> Vec<String> {
        let l = self.lists(cx);
        let mut v = Vec::new();
        if l.is_empty() {
            v.push("empty".to_string());
        }
        for (k, n) in [
            ("needs", l.needs.len()),
            ("working", l.working.len()),
            ("pickup", l.pickup.len()),
            ("elsewhere", l.orphans.len()),
        ] {
            if n > 0 {
                v.push(k.to_string());
            }
        }
        v
    }

    /// Escape closes the drawer (narrow window only).
    fn on_window_key(&mut self, ev: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        let wide = f32::from(window.viewport_size().width) >= WIDE_MIN;
        if ev.keystroke.key == "escape"
            && self.chat_open
            && !wide
            && self.vcx.state.read(cx).view == ViewName::Briefing
        {
            self.set_chat_open(false, cx);
        }
    }

    fn lists(&self, cx: &App) -> Lists {
        let s = self.vcx.state.read(cx);
        let notes = inbox_by_workspace(&s.inbox);
        let projects: BTreeSet<&String> = self.project_filter.iter().collect();
        let briefs: Vec<WorkspaceBrief> = s
            .briefing
            .workspaces
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|w| {
                (projects.is_empty()
                    || (!w.project_id.is_empty() && projects.contains(&w.project_id)))
                    && (!self.unread_only
                        || notes.get(&w.workspace_id).is_some_and(|n| !n.is_empty()))
            })
            .cloned()
            .collect();
        let of = |b: &str| -> Vec<WorkspaceBrief> {
            briefs.iter().filter(|w| w.bucket == b).cloned().collect()
        };
        let (needs_all, working_all, pickup_all) = (of("needs"), of("working"), of("pickup"));
        let keep = |b: Bucket, list: &Vec<WorkspaceBrief>| {
            if self.bucket != Bucket::All && self.bucket != b {
                Vec::new()
            } else {
                list.clone()
            }
        };
        let orphans = if self.bucket != Bucket::All || !self.project_filter.is_empty() {
            Vec::new()
        } else {
            let carded: BTreeSet<&String> = s
                .briefing
                .workspaces
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|w| &w.workspace_id)
                .collect();
            s.inbox
                .iter()
                .filter(|i| {
                    (i.workspace_id.is_empty() || !carded.contains(&i.workspace_id))
                        && (!self.unread_only || i.unread)
                })
                .cloned()
                .collect()
        };
        Lists {
            needs: keep(Bucket::Needs, &needs_all),
            working: keep(Bucket::Working, &working_all),
            pickup: keep(Bucket::Pickup, &pickup_all),
            needs_n: needs_all.len(),
            working_n: working_all.len(),
            pickup_n: pickup_all.len(),
            orphans,
            notes,
            unread: s.inbox.iter().filter(|i| i.unread).count(),
        }
    }

    fn filtered(&self) -> bool {
        self.bucket != Bucket::All || self.unread_only || !self.project_filter.is_empty()
    }

    fn clear_filters(&mut self, cx: &mut Context<Self>) {
        self.bucket = Bucket::All;
        self.unread_only = false;
        self.project_filter.clear();
        self.project_select
            .update(cx, |s, cx| s.set_values(Vec::new(), cx));
        self.save_filters(cx);
        cx.notify();
    }

    fn set_bucket(&mut self, b: Bucket, cx: &mut Context<Self>) {
        self.bucket = if b != Bucket::All && self.bucket == b {
            Bucket::All
        } else {
            b
        };
        cx.notify();
    }

    fn filters(&mut self, l: &Lists, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let mut bar = filters_bar(cx);
        if !self.chat_open {
            let unseen = self.vcx.state.read(cx).attention.app_unseen > 0;
            bar = bar.child(titled(
                "ask-reopen-t",
                "Show chat panel",
                recur_view_observatory::style::pane_toggle(
                    "ask-reopen",
                    IconName::PanelLeft,
                    false,
                    None,
                    cx,
                )
                .debug_selector(|| "ask-reopen".into())
                .mr(px(2.))
                .on_click(cx.guarded_listener(|v, _, _, cx| v.set_chat_open(true, cx)))
                .when(unseen, |d| {
                    d.child(titled(
                        "ask-unseen-t",
                        "Unseen reply",
                        div()
                            .debug_selector(|| "ask-unseen-dot".into())
                            .absolute()
                            .top(px(3.))
                            .right(px(3.))
                            .size(px(6.))
                            .rounded_full()
                            .bg(hsla(t.coral)),
                    ))
                }),
            ));
        }
        bar = bar.child(div().flex_1());
        let projects = self.project_options(cx);
        if projects.len() > 1 {
            let opts = projects
                .into_iter()
                .map(|(v, l)| SelectOption::new(v, l))
                .collect();
            self.project_select
                .update(cx, |s, cx| s.set_options(opts, cx));
            bar = bar.child(self.project_select.clone());
        }
        let unread_only = self.unread_only;
        bar = bar.child(
            filter_seg(cx).child(
                seg_count_btn(
                    "br-seg-unread",
                    "Unread",
                    (l.unread > 0).then_some(l.unread),
                    unread_only,
                    cx,
                )
                .on_click(cx.guarded_listener(|v, _, _, cx| {
                    v.unread_only = !v.unread_only;
                    v.save_filters(cx);
                    cx.notify();
                })),
            ),
        );
        let b = self.bucket;
        let mut seg = filter_seg(cx).child(
            seg_count_btn("br-seg-all", "All", None, b == Bucket::All, cx)
                .on_click(cx.guarded_listener(|v, _, _, cx| v.set_bucket(Bucket::All, cx))),
        );
        for (label, which, n) in [
            ("Needs you", Bucket::Needs, l.needs_n),
            ("Working", Bucket::Working, l.working_n),
            ("Pick up", Bucket::Pickup, l.pickup_n),
        ] {
            seg = seg.child(
                seg_count_btn(
                    SharedString::from(format!("br-seg-{}", which.key())),
                    label,
                    (n > 0).then_some(n),
                    b == which,
                    cx,
                )
                .on_click(cx.guarded_listener(move |v, _, _, cx| v.set_bucket(which, cx))),
            );
        }
        bar.child(seg).into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn bucket(
        &self,
        key: &'static str,
        label: &str,
        dot: recur_ui::Color,
        count: usize,
        list: &[WorkspaceBrief],
        notes: &BTreeMap<String, Vec<InboxItem>>,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui_kit::Div {
        let mut section = div()
            .debug_selector(move || format!("briefing-{key}"))
            .flex()
            .flex_col()
            .child(bucket_head(key, label, count, dot, cx));
        for b in list {
            let n = notes.get(&b.workspace_id).cloned().unwrap_or_default();
            section = section.child(recur_view_chat::cards::brief::brief_row(
                SharedString::from(format!("wbrief-row-{}", b.workspace_id)),
                b,
                &n,
                "",
                &self.vcx.state,
                self.epoch,
                window,
                cx,
            ));
        }
        section
    }

    fn orphan_row(&self, x: &InboxItem, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let theme = Theme::of(cx);
        let color = theme.resolve_css_color(&x.color).unwrap_or(t.muted);
        let hovered = self.hover_row.as_deref() == Some(x.id.as_str());
        let (id, target, read) = (x.id.clone(), x.target.clone(), !x.unread);
        let hid = x.id.clone();
        let title = inline_html_text(
            SharedString::from(format!("ibt-{}", x.id)),
            &x.title,
            InlineStyle {
                bold_weight: 600.,
                leading: Some(1.45),
                ..InlineStyle::meta(13., t.text, &t)
            },
        );
        ibrow(SharedString::from(format!("ibrow-{}", x.id)), hovered, cx)
            .debug_selector({
                let title = x.title.clone();
                move || format!("ibrow:{title}:{}", if read { "read" } else { "unread" })
            })
            .cursor_pointer()
            .when(read, |d| d.opacity(0.62))
            .on_hover(cx.guarded_listener(move |v, h: &bool, _, cx| {
                if *h {
                    v.hover_row = Some(hid.clone());
                } else if v.hover_row.as_deref() == Some(hid.as_str()) {
                    v.hover_row = None;
                }
                cx.notify();
            }))
            .on_click(cx.guarded_listener(move |v, _, _, cx| {
                v.vcx.state.update(cx, |s, cx| {
                    s.mark_inbox_item_read(&id, cx);
                    s.route_target(&target, cx);
                })
            }))
            .child(udot(x.unread, cx))
            .child(ity(color).when_some(row_icon(&x.kind), |d, i| d.child(icon(i).size(px(14.)))))
            .child(
                div().flex_1().min_w_0().child(title).child(
                    div().mt(px(2.)).child(
                        recur_ui::text::text(x.sub.clone())
                            .id(SharedString::from(format!("ibs-{}", x.id)))
                            .mono()
                            .size(10.5)
                            .leading(1.45)
                            .color(t.faint),
                    ),
                ),
            )
            .child(
                div().flex_none().child(
                    recur_ui::text::text(x.time.clone())
                        .mono()
                        .size(10.)
                        .leading(1.45)
                        .color(t.faint)
                        .nowrap(),
                ),
            )
            .into_any_element()
    }
}

/// The filtered buckets and counts (`needs`, `needsAll`, …).
struct Lists {
    needs: Vec<WorkspaceBrief>,
    working: Vec<WorkspaceBrief>,
    pickup: Vec<WorkspaceBrief>,
    needs_n: usize,
    working_n: usize,
    pickup_n: usize,
    orphans: Vec<InboxItem>,
    notes: BTreeMap<String, Vec<InboxItem>>,
    unread: usize,
}

impl Lists {
    fn is_empty(&self) -> bool {
        self.needs.is_empty()
            && self.working.is_empty()
            && self.pickup.is_empty()
            && self.orphans.is_empty()
    }
}

impl BriefingView {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let on = self.vcx.state.read(cx).view == ViewName::Briefing;
        let wide = f32::from(window.viewport_size().width) >= WIDE_MIN;
        let l = self.lists(cx);
        let filters = self.filters(&l, cx);
        let mut overview: Vec<AnyElement> = Vec::new();
        if l.is_empty() {
            let e = if self.filtered() {
                empty(
                    Some(IconName::Filter),
                    "Nothing matches these filters",
                    Some(
                        clear_button("br-clear", cx)
                            .on_click(cx.guarded_listener(|v, _, _, cx| v.clear_filters(cx)))
                            .into_any_element(),
                    ),
                    None,
                    cx,
                )
            } else {
                empty(
                    Some(IconName::InboxEmpty),
                    "Nothing in flight",
                    Some(empty_desc(
                        "When a workspace needs you, is working, or is ready to pick up, it shows up here with its notifications.",
                        cx,
                    )),
                    None,
                    cx,
                )
            };
            overview.push(e.into_any_element());
        }
        if !l.needs.is_empty() {
            overview.push(
                self.bucket(
                    "needs",
                    "Needs you",
                    t.coral,
                    l.needs.len(),
                    &l.needs,
                    &l.notes,
                    window,
                    cx,
                )
                .into_any_element(),
            );
        }
        if !l.working.is_empty() {
            overview.push(
                self.bucket(
                    "working",
                    "Working",
                    t.amber,
                    l.working.len(),
                    &l.working,
                    &l.notes,
                    window,
                    cx,
                )
                .into_any_element(),
            );
        }
        if !l.pickup.is_empty() {
            let expanded = self.pickup_expanded || self.bucket == Bucket::Pickup;
            let shown: Vec<WorkspaceBrief> = if expanded {
                l.pickup.clone()
            } else {
                l.pickup.iter().take(PICKUP_PREVIEW).cloned().collect()
            };
            // The head counts every pick-up card, not the preview.
            let mut section = self.bucket(
                "pickup",
                "Pick up",
                t.indigo,
                l.pickup.len(),
                &shown,
                &l.notes,
                window,
                cx,
            );
            if shown.len() < l.pickup.len()
                || (self.pickup_expanded && l.pickup.len() > PICKUP_PREVIEW)
            {
                let label = if self.pickup_expanded {
                    "Show fewer".to_string()
                } else {
                    format!("Show all {}", l.pickup.len())
                };
                section = section.child(more_button("br-more", label, cx).on_click(
                    cx.guarded_listener(|v, _, _, cx| {
                        v.pickup_expanded = !v.pickup_expanded;
                        cx.notify();
                    }),
                ));
            }
            overview.push(section.into_any_element());
        }
        if !l.orphans.is_empty() {
            let mut section = div()
                .debug_selector(|| "briefing-elsewhere".into())
                .flex()
                .flex_col()
                .child(bucket_head(
                    "elsewhere",
                    "Elsewhere",
                    l.orphans.len(),
                    t.faint,
                    cx,
                ));
            for x in &l.orphans {
                section = section.child(self.orphan_row(x, cx));
            }
            overview.push(section.into_any_element());
        }
        let me = cx.entity().downgrade();
        let col = div()
            .id("briefing-col")
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .on_hover(move |h, _, cx| {
                if !*h {
                    let _ = me.update(cx, |v, cx| {
                        v.hover_row = None;
                        cx.notify();
                    });
                }
            })
            .child(filters)
            .child(
                div()
                    .debug_selector(|| "briefing-scroll".into())
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        recur_ui::widgets::scroll_area("briefing-scroll")
                            .track(&self.scroll)
                            .scroller(|d| d.flex().flex_col().pb(px(32.)))
                            .children(overview),
                    ),
            );
        let mut body = div().relative().flex_1().min_h_0().flex();
        if self.chat_open {
            let state = self.vcx.state.clone();
            let width = state.read(cx).panels.size(PanelKey::InboxChat);
            let mut aside = div()
                .debug_selector(|| "briefing-ask".into())
                .relative()
                .flex()
                .flex_col()
                .min_h_0()
                .bg(hsla(t.pane))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .child(self.chat.clone()),
                );
            if wide {
                aside = aside
                    .flex_none()
                    .w(px(width))
                    .border_r_1()
                    .border_color(hsla(t.line))
                    .child(
                        resize_handle(
                            "inbox-chat-resize",
                            Axis::X,
                            Edge::End,
                            width,
                            &self.resize_focus,
                        )
                        .on_live({
                            let state = state.clone();
                            move |w, _, cx| {
                                state.update(cx, |s, cx| {
                                    s.set_panel_live(PanelKey::InboxChat, w, cx)
                                })
                            }
                        })
                        .on_commit({
                            let state = state.clone();
                            move |_, cx| state.update(cx, |s, _| s.persist_panels())
                        })
                        .on_step(move |w, _, cx| {
                            state.update(cx, |s, cx| s.set_panel(PanelKey::InboxChat, w, cx))
                        }),
                    );
            } else {
                aside = aside.absolute().inset_0().occlude();
            }
            if wide {
                body = body.child(aside).child(col);
            } else {
                body = body.child(col).child(aside);
            }
        } else {
            body = body.child(col);
        }
        div()
            .id("v-briefing")
            .debug_selector(move || format!("v-briefing{}", if on { ":on" } else { "" }))
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(hsla(t.bg))
            .child(body)
    }
}

impl Render for BriefingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// Builds the entity (the `briefing` route).
pub fn build(vcx: &ViewContext, window: &mut Window, cx: &mut App) -> Entity<BriefingView> {
    cx.new(|cx| BriefingView::new(vcx, window, cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_filters_read_old_and_new_shapes() {
        assert_eq!(parse_saved_filters(None), (false, vec![]));
        assert_eq!(parse_saved_filters(Some("nope")), (false, vec![]));
        assert_eq!(
            parse_saved_filters(Some(r#"{"unreadOnly":true,"projects":["a","",3,"b"]}"#)),
            (true, vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(
            parse_saved_filters(Some(r#"{"project":"p"}"#)),
            (false, vec!["p".to_string()])
        );
    }

    #[test]
    fn inbox_groups_unread_by_workspace() {
        let item = |id: &str, ws: &str, unread: bool| InboxItem {
            id: id.into(),
            workspace_id: ws.into(),
            unread,
            ..Default::default()
        };
        let m = inbox_by_workspace(&[
            item("1", "a", true),
            item("2", "a", false),
            item("3", "", true),
            item("4", "b", true),
        ]);
        assert_eq!(m.len(), 2);
        assert_eq!(m["a"].len(), 1);
    }
}
