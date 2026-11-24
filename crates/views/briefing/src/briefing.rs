//! The Inbox: one list of workspace cards in Needs you / Working / Pick up
//! buckets with their unread notifications, the Elsewhere notifications that have no card,
//! label-style filters, the Ask Recur dock on the left, and the selected workspace's
//! overview on the right (in place of the list below 1100px). A click selects a card;
//! double-click, ⌘-click, Enter or Open workspace opens it. The dock's visibility
//! (`inboxChatOpen`, `recur.askRecurOpen`) lives here; below 1100px the dock is a drawer.

mod overview;

use recur_ui::guard::Guarded;
use recur_view_core::{ObserveState as _, Topics};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant};

use gpui_kit::Task;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, InteractiveElement,
    IntoElement, KeyDownEvent, KeystrokeEvent, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, point, px,
};
use recur_content::MarkdownView;
use recur_domain::{InboxItem, WorkspaceBrief, WorkspaceChanges, WorkspaceOverview};
use recur_services::AppEvent;
use recur_ui::text::TextStyled;
use recur_ui::tooltip::titled;
use recur_ui::widgets::toasts::ToastKind;
use recur_ui::widgets::{
    Axis, Edge, Select, SelectEvent, SelectOption, TriggerStyle, resize_handle,
};
use recur_ui::{IconName, Theme, icon};
use recur_view_chat::app_chat::{AppChat, AppChatEvent};
use recur_view_chat::cards::permission::PermissionCard;
use recur_view_chat::cards::question::QuestionCard;
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
/// The list width that fits its full rows and bucket chips beside the overview.
const FULL_LIST_W: f32 = 600.;
/// Pick up shows this many until expanded.
pub const PICKUP_PREVIEW: usize = 8;
/// Events about the selected workspace coalesce into one overview refetch.
const OVERVIEW_DEBOUNCE: Duration = Duration::from_millis(200);
/// Live tool steps the overview keeps.
const STEPS_KEPT: usize = 15;

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

/// A service event the open overview reacts to.
enum LiveEvent {
    /// A tool step on a workspace chat, as the overview lists it.
    Step {
        workspace_id: String,
        step: String,
    },
    /// Something on a workspace changed ("" = any); `changes` when commits may have moved.
    Workspace {
        workspace_id: String,
        changes: bool,
    },
    Pr(String),
}

fn live_event(ev: &AppEvent) -> Option<LiveEvent> {
    let ws = |id: &str, changes: bool| {
        Some(LiveEvent::Workspace {
            workspace_id: id.to_owned(),
            changes,
        })
    };
    match ev {
        AppEvent::SessionStep(d) if !d.tag.workspace_id.is_empty() => {
            let input = recur_agent::display::summarize_tool_input(&d.tool, &d.tool_input);
            Some(LiveEvent::Step {
                workspace_id: d.tag.workspace_id.clone(),
                step: if input.is_empty() {
                    d.tool.clone()
                } else {
                    format!("{} {input}", d.tool)
                },
            })
        }
        AppEvent::SessionTurnEnd(d) => ws(&d.tag.workspace_id, true),
        AppEvent::SessionUser(d) => ws(&d.tag.workspace_id, false),
        AppEvent::AgentUpdated(a) => ws(&a.workspace_id, false),
        AppEvent::BriefingUpdated(b) => ws(&b.workspace_id, true),
        AppEvent::FindingsUpdated(r) => ws(&r.workspace_id, false),
        AppEvent::QuestionAsked(q) => ws(&q.workspace_id, false),
        AppEvent::PermissionRequested(p) => ws(&p.workspace_id, false),
        AppEvent::EscalationRaised(e) => ws(&e.workspace_id, false),
        AppEvent::EscalationResolved(e) => ws(&e.workspace_id, false),
        AppEvent::QuestionAnswered(_) | AppEvent::PermissionResolved(_) => ws("", false),
        AppEvent::PrUpdated(p) => Some(LiveEvent::Pr(p.pr_id.clone())),
        _ => None,
    }
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
    ov_scroll: ScrollHandle,
    /// Arrow keys moved the selection: the next render scrolls its row into view.
    reveal_selected: std::cell::Cell<bool>,
    /// The selection is the wide layout's automatic pick, not a click: a narrow window drops it.
    auto_picked: bool,
    /// The selected workspace was removed, or the list was empty on arrival or after a filter
    /// change: no automatic pick until the user selects, filters or comes back to Inbox.
    vacated: bool,
    /// Inbox is on screen with a loaded briefing.
    arrived: bool,
    resize_focus: FocusHandle,
    list_resize_focus: FocusHandle,
    hover_row: Option<String>,
    epoch: Instant,
    /// The bucket to bring to the top once laid out (`briefingFocus`).
    pending_focus: Option<String>,
    /// The workspace whose overview is open.
    selected: Option<String>,
    /// The selected card's last copy: reading a settled workspace's reply drops its card,
    /// and the overview stays open on this.
    held: Option<WorkspaceBrief>,
    /// Overviews and changes by workspace: a revisit paints the last copy at once while
    /// the refetch runs.
    overviews: HashMap<String, WorkspaceOverview>,
    changes: HashMap<String, WorkspaceChanges>,
    overview_gen: u64,
    /// A debounced refetch is pending (and whether it includes the git changes).
    refresh_queued: Option<bool>,
    /// The checks summary each workspace had when Fix checks was sent.
    fix_sent: HashMap<String, String>,
    permission_cards: HashMap<String, Entity<PermissionCard>>,
    /// The selected workspace's unread notifications as of selecting it (selecting marks
    /// them read).
    seen_notes: Vec<InboxItem>,
    /// The latest reply's source and its rendered view.
    reply_md: Option<(String, Entity<MarkdownView>)>,
    plan_md: Option<(String, Entity<MarkdownView>)>,
    question_cards: HashMap<String, Entity<QuestionCard>>,
    ov_open: overview::Open,
    list_focus: FocusHandle,
    _events: Task<()>,
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
        let mut events = vcx.services.subscribe();
        let events_task = cx.spawn(async move |this, cx| {
            while let Some(ev) = events.recv().await {
                let Some(ev) = live_event(&ev) else {
                    continue;
                };
                if this
                    .update(cx, |v, cx| v.on_workspace_event(ev, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let subs = vec![
            cx.observe_state_in(&vcx.state, Topics::ALL, window, |this, _, window, cx| {
                this.sync(window, cx)
            }),
            cx.guarded_subscribe(&chat, |this, _, ev: &AppChatEvent, cx| {
                if matches!(ev, AppChatEvent::Hide) {
                    this.set_chat_open(false, cx);
                }
            }),
            cx.guarded_subscribe(&project_select, |this, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Values(ids) = ev {
                    this.project_filter = ids.iter().map(|s| s.to_string()).collect();
                    this.save_filters(cx);
                    this.drop_hidden_selection(cx);
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
            ov_scroll: ScrollHandle::new(),
            reveal_selected: Default::default(),
            auto_picked: false,
            vacated: false,
            arrived: false,
            resize_focus: cx.focus_handle(),
            list_resize_focus: cx.focus_handle(),
            hover_row: None,
            epoch: Instant::now(),
            pending_focus: None,
            selected: None,
            held: None,
            overviews: HashMap::new(),
            changes: HashMap::new(),
            overview_gen: 0,
            refresh_queued: None,
            fix_sent: HashMap::new(),
            permission_cards: HashMap::new(),
            seen_notes: Vec::new(),
            reply_md: None,
            plan_md: None,
            question_cards: HashMap::new(),
            ov_open: overview::Open::default(),
            list_focus: cx.focus_handle(),
            _events: events_task,
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

    /// The cards, less those of workspaces archived since the last refresh.
    fn briefs_all(&self, cx: &App) -> Vec<WorkspaceBrief> {
        let s = self.vcx.state.read(cx);
        let mut briefs = s.briefing.workspaces.clone().unwrap_or_default();
        briefs.retain(|b| s.workspace(&b.workspace_id).is_none_or(|w| !w.archived));
        briefs
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
        let (on, loaded) = {
            let s = self.vcx.state.read(cx);
            (s.view == ViewName::Briefing, s.briefing.generated_at != 0)
        };
        self.vacated &= on;
        if let Some(id) = self.selected.clone()
            && !self.briefs_all(cx).iter().any(|b| b.workspace_id == id)
        {
            let s = self.vcx.state.read(cx);
            let removed = s.workspace(&id).is_none_or(|w| w.archived);
            if removed || !on {
                self.clear_selection(cx);
                self.vacated = on;
            }
        }
        let arrived = on && loaded;
        if arrived && !self.arrived {
            self.vacated = self.visible_ids(cx).is_empty();
        }
        self.arrived = arrived;
        self.sync_viewing(cx);
        let focus = self.vcx.state.read(cx).briefing_focus.clone();
        if !focus.is_empty() {
            // A footer / palette jump must reach its section: drop the filters that could
            // hide it (the project scope is a deliberate choice).
            self.bucket = Bucket::All;
            if self.unread_only {
                self.unread_only = false;
                self.save_filters(cx);
            }
            self.drop_hidden_selection(cx);
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
            let top = item.origin.y - view.origin.y;
            self.scroll.set_offset(point(self.scroll.offset().x, -top));
            cx.notify();
        }
    }

    /// Where section `ix`'s pinned header sits in content coordinates, once the list scrolled
    /// past its in-flow header: at the viewport top, pushed up by the section's bottom.
    fn sticky_head_top(&self, ix: usize) -> Option<f32> {
        let item = self.scroll.bounds_for_item(ix)?;
        let off = -f32::from(self.scroll.offset().y);
        let top = f32::from(item.top() - self.scroll.bounds().top());
        let bottom = top + f32::from(item.size.height);
        (off > top).then(|| off.min(bottom - crate::style::HEAD_H))
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
        if ev.keystroke.key != "escape"
            || wide
            || self.vcx.state.read(cx).view != ViewName::Briefing
        {
            return;
        }
        if self.chat_open {
            self.set_chat_open(false, cx);
        } else if self.selected.is_some() {
            self.clear_selection(cx);
        }
    }

    fn lists(&self, cx: &App) -> Lists {
        let s = self.vcx.state.read(cx);
        let notes = inbox_by_workspace(&s.inbox);
        let projects: BTreeSet<&String> = self.project_filter.iter().collect();
        let briefs: Vec<WorkspaceBrief> = self
            .briefs_all(cx)
            .into_iter()
            .filter(|w| {
                (projects.is_empty()
                    || (!w.project_id.is_empty() && projects.contains(&w.project_id)))
                    && (!self.unread_only
                        || w.changed_since_viewed
                        || self.selected.as_deref() == Some(w.workspace_id.as_str())
                        || notes.get(&w.workspace_id).is_some_and(|n| !n.is_empty()))
            })
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
        self.drop_hidden_selection(cx);
        cx.notify();
    }

    fn set_bucket(&mut self, b: Bucket, cx: &mut Context<Self>) {
        self.bucket = if b != Bucket::All && self.bucket == b {
            Bucket::All
        } else {
            b
        };
        self.drop_hidden_selection(cx);
        cx.notify();
    }

    /// A list narrowed beside the overview: rows drop their project, plan, diff and age
    /// columns (the overview shows them) and the filters drop the bucket chips (the section
    /// heads name the buckets). Dragging the list wider brings both back.
    fn compact(&self, window: &Window, cx: &App) -> bool {
        self.selected.is_some()
            && (!wide(window)
                || self.vcx.state.read(cx).panels.size(PanelKey::InboxList) < FULL_LIST_W)
    }

    fn filters(&mut self, l: &Lists, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let mut bar = filters_bar(cx);
        if !self.chat_open {
            let unseen = self.vcx.state.read(cx).attention.app_unseen > 0;
            bar = bar.child(titled(
                "ask-reopen-t",
                "Show chat panel",
                recur_view_widgets::chrome::pane_toggle(
                    "ask-reopen",
                    IconName::PanelLeft,
                    false,
                    None,
                    cx,
                )
                .debug_selector(|| "ask-reopen".into())
                // Line up with the chat head's Hide button (8px in; the filters bar pads 12).
                .ml(px(-4.))
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
            let state = self.vcx.state.read(cx);
            let opts = projects
                .into_iter()
                .map(|(v, l)| {
                    let o = SelectOption::new(v.clone(), l);
                    match state.project(&v) {
                        Some(p) => o.pic(p.pic.clone(), p.color.clone()),
                        None => o,
                    }
                })
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
                    v.drop_hidden_selection(cx);
                    cx.notify();
                })),
            ),
        );
        if compact {
            return bar.into_any_element();
        }
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
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let mut section = div()
            .debug_selector(move || format!("briefing-{key}"))
            .flex()
            .flex_col()
            .child(bucket_head(key, label, count, dot, cx));
        for b in list {
            let n = notes.get(&b.workspace_id).cloned().unwrap_or_default();
            let (me, wid) = (cx.entity().downgrade(), b.workspace_id.clone());
            let selected = self.selected.as_deref() == Some(b.workspace_id.as_str());
            let row = recur_view_chat::cards::brief::brief_row(
                SharedString::from(format!("wbrief-row-{}", b.workspace_id)),
                b,
                &n,
                selected,
                self.compact(window, cx),
                move |ev: &ClickEvent, window, cx| {
                    let _ = me.update(cx, |v, cx| v.click_row(&wid, ev, window, cx));
                },
                self.epoch,
                window,
                cx,
            );
            section = if selected && self.reveal_selected.take() {
                section.child(reveal_row(row, self.scroll.clone()))
            } else {
                section.child(row)
            };
        }
        section
    }

    /// A plain click selects; double-click or ⌘-click opens.
    fn click_row(
        &mut self,
        id: &str,
        ev: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select(id, window, cx);
        if ev.click_count() >= 2 || ev.modifiers().secondary(cx) {
            self.open_selected(cx);
        }
    }

    fn select(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.list_focus.focus(window, cx);
        self.auto_picked = false;
        self.select_quiet(id, cx);
    }

    /// Selects without taking focus (the wide layout's automatic first pick).
    fn select_quiet(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.selected.as_deref() == Some(id) {
            return;
        }
        self.selected = Some(id.to_owned());
        self.vacated = false;
        self.seen_notes = inbox_by_workspace(&self.vcx.state.read(cx).inbox)
            .remove(id)
            .unwrap_or_default();
        self.ov_open = overview::Open::default();
        self.ov_scroll.set_offset(point(px(0.), px(0.)));
        self.reply_md = None;
        self.mark_read(id, cx);
        self.load_overview(true, cx);
        self.sync_viewing(cx);
        cx.notify();
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.refresh_queued = None;
        self.sync_viewing(cx);
        self.seen_notes.clear();
        self.reply_md = None;
        self.overview_gen += 1;
        cx.notify();
    }

    /// The row's dot: unread notifications, or activity since it was last viewed.
    pub(crate) fn is_unread(&self, id: &str, cx: &App) -> bool {
        let s = self.vcx.state.read(cx);
        s.inbox.iter().any(|i| i.unread && i.workspace_id == id)
            || s.briefing
                .workspaces
                .iter()
                .flatten()
                .any(|b| b.workspace_id == id && b.changed_since_viewed)
    }

    /// Unread, or still listed: a card that left Inbox cannot be marked unread.
    pub(crate) fn can_toggle_read(&self, id: &str, cx: &App) -> bool {
        self.is_unread(id, cx) || self.briefs_all(cx).iter().any(|b| b.workspace_id == id)
    }

    pub(crate) fn mark_read(&mut self, id: &str, cx: &mut Context<Self>) {
        let activity = self.set_changed(id, false, cx);
        let (svc, wid) = (self.vcx.services.clone(), id.to_owned());
        self.vcx
            .state
            .update(cx, |s, cx| s.mark_workspace_inbox_read(&wid, cx));
        if activity > 0 {
            cx.background_spawn(async move {
                let _ = svc.briefing.mark_workspace_viewed(&wid, activity).await;
            })
            .detach();
        }
    }

    pub(crate) fn mark_unread(&mut self, id: &str, cx: &mut Context<Self>) {
        self.set_changed(id, true, cx);
        let (svc, wid) = (self.vcx.services.clone(), id.to_owned());
        cx.background_spawn(async move {
            let _ = svc.briefing.mark_workspace_unread(&wid).await;
        })
        .detach();
    }

    pub(crate) fn toggle_read(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        if !self.can_toggle_read(&id, cx) {
            return;
        }
        if self.is_unread(&id, cx) {
            self.mark_read(&id, cx);
        } else {
            self.mark_unread(&id, cx);
        }
        cx.notify();
    }

    /// Sets the brief's "changed since viewed" cue ahead of the next snapshot; returns its
    /// last activity.
    fn set_changed(&mut self, id: &str, changed: bool, cx: &mut Context<Self>) -> i64 {
        self.vcx.state.update(cx, |s, cx| {
            let b = s
                .briefing
                .workspaces
                .iter_mut()
                .flatten()
                .find(|b| b.workspace_id == id);
            let activity = b.map_or(0, |b| {
                b.changed_since_viewed = changed;
                b.last_activity_at
            });
            cx.notify();
            activity
        })
    }

    /// The wide layout opens the first row's overview. A row leaving the list on a refresh
    /// never picks the next one: that would mark it read unseen.
    fn ensure_selection(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_none()
            && !self.vacated
            && let Some(first) = self.visible_ids(cx).first()
        {
            self.auto_picked = true;
            self.select_quiet(first, cx)
        }
    }

    /// After a filter change, lets go of a selection the list no longer shows.
    fn drop_hidden_selection(&mut self, cx: &mut Context<Self>) {
        let ids = self.visible_ids(cx);
        self.vacated = ids.is_empty();
        if let Some(s) = &self.selected
            && !ids.contains(s)
        {
            self.clear_selection(cx);
        }
    }

    fn selected_brief(&self, cx: &App) -> Option<WorkspaceBrief> {
        let id = self.selected.as_deref()?;
        self.briefs_all(cx)
            .into_iter()
            .find(|b| b.workspace_id == id)
            .or_else(|| {
                let mut held = self.held.clone().filter(|b| b.workspace_id == id)?;
                // An answered question no longer routes to its chat.
                if held.primary.target.starts_with("question:") {
                    held.primary = Default::default();
                }
                Some(WorkspaceBrief {
                    bucket: String::new(),
                    reasons: None,
                    now: String::new(),
                    ..held
                })
            })
    }

    fn open_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(b) = self.selected_brief(cx) {
            recur_view_chat::cards::brief::open_brief(&self.vcx.state, &b, cx);
        }
    }

    /// Tells Ask Recur which workspace the overview shows.
    fn sync_viewing(&mut self, cx: &mut Context<Self>) {
        self.held = self.selected_brief(cx);
        let viewing = self.held.clone().map(|b| (b.workspace_id, b.title));
        self.chat.update(cx, |c, cx| c.set_viewing(viewing, cx));
    }

    /// Fetches the selected overview and, with `changes`, its git changes (separately: the
    /// overview paints without waiting on git).
    fn load_overview(&mut self, changes: bool, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        self.overview_gen += 1;
        let gen_ = self.overview_gen;
        let services = self.vcx.services.clone();
        let wid = id.clone();
        cx.spawn(async move |this, cx| {
            let res = services.briefing.workspace_overview(&wid).await;
            let _ = this.update(cx, |v, cx| {
                if v.overview_gen == gen_
                    && let Ok(o) = res
                {
                    v.overviews.insert(wid, o);
                    cx.notify();
                }
            });
        })
        .detach();
        if !changes {
            return;
        }
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let res = services.briefing.workspace_changes(&id).await;
            let _ = this.update(cx, |v, cx| {
                if let Ok(c) = res {
                    v.changes.insert(id, c);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// A live event about `ws` ("" = any workspace): a tool step lands in the open overview
    /// at once; anything else queues one debounced refetch.
    fn on_workspace_event(&mut self, ev: LiveEvent, cx: &mut Context<Self>) {
        let Some(sel) = self.selected.clone() else {
            return;
        };
        let (ws, changes) = match ev {
            LiveEvent::Step { workspace_id, step } => {
                if workspace_id != sel {
                    return;
                }
                if let Some(o) = self.overviews.get_mut(&sel) {
                    o.running = true;
                    o.recent_steps.push(step);
                    let over = o.recent_steps.len().saturating_sub(STEPS_KEPT);
                    o.recent_steps.drain(..over);
                    cx.notify();
                }
                return;
            }
            LiveEvent::Pr(pr_id) => {
                let s = self.vcx.state.read(cx);
                let mine = s.prs.iter().any(|p| p.id == pr_id && p.workspace_id == sel);
                (if mine { sel.clone() } else { return }, false)
            }
            LiveEvent::Workspace {
                workspace_id,
                changes,
            } => (workspace_id, changes),
        };
        if !ws.is_empty() && ws != sel {
            return;
        }
        let queued = self.refresh_queued.is_some();
        self.refresh_queued = Some(self.refresh_queued.unwrap_or(false) || changes);
        if queued {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(OVERVIEW_DEBOUNCE).await;
            let _ = this.update(cx, |v, cx| {
                if let Some(changes) = v.refresh_queued.take() {
                    v.load_overview(changes, cx);
                }
            });
        })
        .detach();
    }

    /// Runs a service call for the selected workspace and toasts its error.
    fn act(
        &mut self,
        ok: Option<&'static str>,
        cx: &mut Context<Self>,
        f: impl AsyncFnOnce(recur_services::Services) -> recur_services::Result<()> + 'static,
    ) {
        let services = self.vcx.services.clone();
        cx.spawn(async move |_, cx| {
            let res = f(services).await;
            cx.update(|cx| match res {
                Ok(()) => {
                    if let Some(msg) = ok {
                        recur_view_core::toast(ToastKind::Info, msg, cx)
                    }
                }
                Err(e) => recur_view_core::toast(ToastKind::Error, e.to_string(), cx),
            });
        })
        .detach();
    }

    /// Sends `message` to a workspace's agent and toasts what happened.
    fn send_to_workspace(&mut self, workspace_id: &str, message: &str, cx: &mut Context<Self>) {
        let services = self.vcx.services.clone();
        let (id, message) = (workspace_id.to_owned(), message.to_owned());
        cx.spawn(async move |this, cx| {
            let res = services.briefing.message_workspace(&id, &message).await;
            if res.is_err() {
                let _ = this.update(cx, |v, cx| {
                    v.fix_sent.remove(&id);
                    cx.notify();
                });
            }
            cx.update(|cx| match res {
                Ok(msg) => {
                    recur_view_core::toast(recur_ui::widgets::toasts::ToastKind::Info, msg, cx)
                }
                Err(e) => recur_view_core::toast(
                    recur_ui::widgets::toasts::ToastKind::Error,
                    e.to_string(),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// The card ids in on-screen order.
    fn visible_ids(&self, cx: &App) -> Vec<String> {
        let l = self.lists(cx);
        let expanded = self.pickup_expanded || self.bucket == Bucket::Pickup;
        let pickup = l
            .pickup
            .iter()
            .take(if expanded { usize::MAX } else { PICKUP_PREVIEW });
        l.needs
            .iter()
            .chain(l.working.iter())
            .chain(pickup)
            .map(|b| b.workspace_id.clone())
            .collect()
    }

    /// ↑/↓ move the selection, Enter or ⌘O opens it, Esc closes the overview.
    fn on_list_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let plain = !k.modifiers.modified();
        match k.key.as_str() {
            "down" | "up" if plain => {
                let ids = self.visible_ids(cx);
                let at = self
                    .selected
                    .as_ref()
                    .and_then(|s| ids.iter().position(|i| i == s));
                let next = match (k.key.as_str(), at) {
                    (_, None) => 0,
                    ("down", Some(i)) => (i + 1).min(ids.len().saturating_sub(1)),
                    (_, Some(i)) => i.saturating_sub(1),
                };
                if let Some(id) = ids.get(next) {
                    self.reveal_selected.set(true);
                    self.select(&id.clone(), window, cx);
                }
            }
            "enter" if plain => self.open_selected(cx),
            "u" if k.modifiers.secondary(cx) && k.modifiers.shift => self.toggle_read(cx),
            "o" if k.modifiers.secondary(cx) => self.open_selected(cx),
            "escape" if plain && self.selected.is_some() && !wide(window) => {
                self.clear_selection(cx)
            }
            _ => return,
        }
        cx.stop_propagation();
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
        let wide = wide(window);
        if wide {
            self.ensure_selection(cx);
        } else if self.auto_picked {
            self.auto_picked = false;
            self.clear_selection(cx);
        }
        let l = self.lists(cx);
        let filters = self.filters(&l, self.compact(window, cx), cx);
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
        let heads = [
            ("needs", "Needs you", t.coral, l.needs.len()),
            ("working", "Working", t.amber, l.working.len()),
            ("pickup", "Pick up", t.indigo, l.pickup.len()),
            ("elsewhere", "Elsewhere", t.faint, l.orphans.len()),
        ];
        let first = usize::from(l.is_empty());
        let heads = heads.into_iter().filter(|h| h.3 > 0).enumerate();
        for (i, (key, label, dot, n)) in heads {
            if let Some(at) = self.sticky_head_top(first + i) {
                overview.push(
                    div()
                        .debug_selector(move || format!("briefing-sticky:{key}"))
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(at))
                        .occlude()
                        .child(bucket_head(key, label, n, dot, cx))
                        .into_any_element(),
                );
            }
        }
        let me = cx.entity().downgrade();
        let selected = self.selected_brief(cx);
        let col =
            div()
                .id("briefing-col")
                .track_focus(&self.list_focus)
                .on_key_down(cx.guarded_listener(|v, ev: &KeyDownEvent, window, cx| {
                    v.on_list_key(ev, window, cx)
                }))
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
        let (col, pane) = match &selected {
            None => (Some(col), None),
            Some(b) if !wide => (None, Some(self.overview_pane(b, true, window, cx).flex_1())),
            Some(b) => {
                let pane = self
                    .overview_pane(b, false, window, cx)
                    .flex_1()
                    .min_w_0()
                    .border_l_1()
                    .border_color(hsla(t.line));
                let state = self.vcx.state.clone();
                let width = state.read(cx).panels.size(PanelKey::InboxList);
                let col = col.relative().flex_none().w(px(width)).child(
                    resize_handle(
                        "inbox-list-resize",
                        Axis::X,
                        Edge::End,
                        width,
                        &self.list_resize_focus,
                    )
                    .on_live({
                        let state = state.clone();
                        move |w, _, cx| {
                            state.update(cx, |s, cx| s.set_panel_live(PanelKey::InboxList, w, cx))
                        }
                    })
                    .on_commit({
                        let state = state.clone();
                        move |_, cx| state.update(cx, |s, _| s.persist_panels())
                    })
                    .on_step(move |w, _, cx| {
                        state.update(cx, |s, cx| s.set_panel(PanelKey::InboxList, w, cx))
                    }),
                );
                (Some(col), Some(pane))
            }
        };
        let col = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .when_some(col, |d, c| d.child(c))
            .when_some(pane, |d, p| d.child(p));
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

fn wide(window: &Window) -> bool {
    f32::from(window.viewport_size().width) >= WIDE_MIN
}

/// `scrollIntoView({ block: 'nearest' })`: once laid out, scrolls `row` just into `scroll`'s
/// viewport, below the pinned bucket header.
fn reveal_row(row: AnyElement, scroll: ScrollHandle) -> AnyElement {
    div()
        .relative()
        .child(row)
        .child(
            gpui_kit::canvas(
                move |b, window, _| {
                    let view = scroll.bounds();
                    let below = b.bottom() - view.bottom();
                    let above = view.top() + px(crate::style::HEAD_H) - b.top();
                    let shift = if above > px(0.) {
                        above
                    } else if below > px(0.) {
                        -below
                    } else {
                        return;
                    };
                    let cur = scroll.offset();
                    scroll.set_offset(point(cur.x, cur.y + shift));
                    window.refresh();
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .into_any_element()
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
