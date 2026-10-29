//! The project tree's elements: group and project blocks in flow, their sticky headers as an
//! absolute layer over the scrolled content (GPUI has no `position: sticky`), the under-fades,
//! the bottom mask and the overlay scrollbar.

use recur_ui::guard::Guarded;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Context, Div, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_domain::{Pr, Project, Workspace};
use recur_ui::fonts::weight;
use recur_ui::motion;
use recur_ui::style::{self, is_focus_visible};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::{PicBox, decode_text, project_pic};
use recur_ui::{Color, IconName, Theme, icon, rgba};
use recur_view_core::format::format_compact_count;
use recur_view_core::modes::{
    parked_needs_you_by_workspace, workspace_can_remove, workspace_is_running,
};
use recur_view_core::pr::{PrLifecycle, pr_for_workspace, pr_head, pr_icon_name, pr_lifecycle};
use recur_view_core::workspace_nav::project_id_from_main_scan;
use recur_view_core::{ScanKind, ScheduleEditorRequest, ViewName};

use crate::tree::{
    FADE, GROUP_H, GROUP_PB, GroupNode, PROJ_H, ProjectNode, ROW_H, Row, SCHED_H, SCHED_MT, Tree,
    empty_h, is_stuck, sticky_top, thumb,
};
use crate::{FADE_IN, Fade, SCROLLBAR_LINGER, Sidebar, fade_gradient};

const PIC: PicBox = PicBox {
    size: 18.,
    radius: 5.,
    font_size: 9.,
};
const FLIP: Duration = Duration::from_millis(180);
const SB_OUT: Duration = Duration::from_millis(450);
const CARET: Duration = Duration::from_millis(120);

/// Overscan above and below the tree's viewport, in px, before rows become spacers.
const ROW_BAND_PAD_PX: f32 = 150.;

/// What every row reads from the stores, gathered once per frame.
pub(crate) struct RowCtx {
    /// Each workspace's chrome PR (`pr_for_workspace`), without its files and comments.
    prs: HashMap<String, Pr>,
    running: HashSet<String>,
    attention: BTreeMap<String, i64>,
    parked: BTreeMap<String, usize>,
    view: ViewName,
    selected: String,
    scan_highlight: Option<String>,
    dragging: bool,
}

impl RowCtx {
    fn live_view(&self) -> bool {
        matches!(self.view, ViewName::Detail | ViewName::Escalation)
    }

    fn task_on(&self, id: &str) -> bool {
        match &self.scan_highlight {
            Some(sid) => sid == id,
            None => self.live_view() && id == self.selected,
        }
    }

    fn main_on(&self, project_id: &str, main: Option<&Workspace>) -> bool {
        match &self.scan_highlight {
            Some(sid) => {
                project_id_from_main_scan(sid) == Some(project_id)
                    || main.is_some_and(|m| &m.id == sid)
            }
            None => self.live_view() && main.is_some_and(|m| m.id == self.selected),
        }
    }

    fn parked(&self, id: &str) -> usize {
        self.parked.get(id).copied().unwrap_or(0)
    }

    fn unseen(&self, id: &str) -> i64 {
        self.attention.get(id).copied().unwrap_or(0)
    }
}

/// Where a sticky header paints this frame.
struct Header {
    top: f32,
    stuck: bool,
}

fn lifecycle_color(life: PrLifecycle, merged_row: bool, cx: &App) -> Color {
    let t = Theme::of(cx).tokens;
    if merged_row {
        return t.indigo;
    }
    match life {
        PrLifecycle::Open => t.emerald,
        PrLifecycle::Draft => t.cyan,
        PrLifecycle::Merged => t.indigo,
        PrLifecycle::Closed => t.coral,
        PrLifecycle::Queued => t.warn,
    }
}

/// `.df`: `+add −del` in mono 9.5px.
fn diff_stat(key: &str, add: i64, del: i64, cx: &App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let el = div()
        .id(SharedString::from(format!("{key}-df")))
        .flex_none()
        .flex()
        .gap(px(5.))
        .child(
            style::button_label(
                format!("+{}", format_compact_count(add as f64)),
                9.5,
                weight::REGULAR,
            )
            .mono()
            .color(t.emerald),
        )
        .child(
            style::button_label(
                format!("−{}", format_compact_count(del as f64)),
                9.5,
                weight::REGULAR,
            )
            .mono()
            .color(t.coral),
        );
    titled(
        SharedString::from(format!("{key}-df-tip")),
        format!("+{add} −{del}"),
        el,
    )
    .into_any_element()
}

/// A tree connector line (`--line`, 1px).
fn vline(cx: &App) -> Div {
    div().absolute().w(px(1.)).bg(Theme::of(cx).tokens.line)
}

impl Sidebar {
    /// What the rows read from the stores (memoized per store change).
    fn row_ctx(&self, cx: &App) -> Rc<RowCtx> {
        let dragging = self.drag.as_ref().is_some_and(|d| d.active);
        if let Some((rev, drag, rc)) = self.row_ctx_memo.borrow().as_ref()
            && *rev == self.state_rev
            && *drag == dragging
        {
            return rc.clone();
        }
        let rc = Rc::new(self.build_row_ctx(cx));
        *self.row_ctx_memo.borrow_mut() = Some((self.state_rev, dragging, rc.clone()));
        rc
    }

    fn build_row_ctx(&self, cx: &App) -> RowCtx {
        let s = self.state().read(cx);
        let ev = &s.scan_event;
        let scan_highlight = ((ev.seq > 0 && ev.kind == ScanKind::Land)
            || (s.scan_pending && project_id_from_main_scan(&ev.workspace_id).is_some()))
        .then(|| ev.workspace_id.clone());
        RowCtx {
            prs: {
                let ids: HashSet<&str> = s.prs.iter().map(|p| p.workspace_id.as_str()).collect();
                ids.into_iter()
                    .filter_map(|id| {
                        let pr = pr_for_workspace(&s.prs, id)?;
                        Some((id.to_string(), pr_head(pr)))
                    })
                    .collect()
            },
            running: s.workspaces_running.clone(),
            attention: s.attention.by_workspace.clone(),
            parked: parked_needs_you_by_workspace(
                &s.workspaces,
                &s.user_questions,
                &s.permission_requests,
            ),
            view: s.view,
            selected: s.selected_workspace_id.clone(),
            scan_highlight,
            dragging: self.drag.as_ref().is_some_and(|d| d.active),
        }
    }

    pub(crate) fn flip_offset(&self, key: &str, cx: &App) -> f32 {
        match self.flip.get(key) {
            Some(&(dy, at)) => dy * (1. - motion::eased(at, FLIP, motion::Easing::SETTLE, cx)),
            None => 0.,
        }
    }

    /// An icon button in a header's hover actions (`.pset`, `.padd`, `.gact`, `.lact`).
    #[allow(clippy::too_many_arguments)]
    fn act_button(
        &mut self,
        key: String,
        label: SharedString,
        name: IconName,
        size: f32,
        icon_size: f32,
        visible: bool,
        color: Color,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<Div> {
        let t = Theme::of(cx).tokens;
        self.control(key, 4., window, cx)
            .aria_label(label)
            .size(px(size))
            .flex_none()
            .rounded(px(4.))
            .flex()
            .items_center()
            .justify_center()
            .text_color(color)
            .opacity(if visible { 1. } else { 0. })
            .hover(move |s| s.bg(t.pane_2).text_color(Hsla::from(t.text)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(icon(name).size(px(icon_size)))
    }

    fn icon_slot(&self, running: bool, key: &str, fallback: AnyElement, cx: &App) -> AnyElement {
        if running {
            titled(
                SharedString::from(format!("{key}-sd")),
                "Agent running",
                self.running_dot(cx)
                    .id(SharedString::from(format!("{key}-sd-dot")))
                    .aria_label("Agent running"),
            )
            .into_any_element()
        } else {
            fallback
        }
    }

    /// `.tloop`: one task workspace row.
    fn task_row(
        &mut self,
        row: &Row,
        rc: &RowCtx,
        live: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let l = &row.ws;
        let id = l.id.clone();
        let key = format!("row:{id}");
        let pr = rc.prs.get(&id).cloned();
        let merged_ws = l.state.as_str() == "merged";
        let life = pr_lifecycle(pr.as_ref()).or(merged_ws.then_some(PrLifecycle::Merged));
        let merged = merged_ws || life == Some(PrLifecycle::Merged);
        let can_rm = workspace_can_remove(Some(l), rc.running.contains(&id));
        let armed = live && self.armed() == Some(id.as_str());
        let on = rc.task_on(&id);
        let hovered = live && self.hover_key(&key);
        let focus_within = live
            && [format!("hit:{id}"), format!("pr:{id}"), format!("rm:{id}")]
                .iter()
                .any(|k| self.focused(k, window));
        let acts = live && !rc.dragging && (hovered || focus_within || armed);

        let br = match life {
            Some(life) => {
                let name = pr
                    .as_ref()
                    .map_or(IconName::GitMerge, |p| pr_icon_name(Some(p)));
                icon(name)
                    .size(px(14.))
                    .color(lifecycle_color(life, merged, cx))
                    .into_any_element()
            }
            None => icon(IconName::GitBranch)
                .size(px(14.))
                .color(t.faint)
                .into_any_element(),
        };
        let slot = self.icon_slot(workspace_is_running(l, &rc.running), &key, br, cx);
        let name_color = if on { t.text } else { t.muted };
        let tn = div().min_w_0().child(
            decode_text(SharedString::from(format!("{key}-tn")), l.short.clone())
                .source_key(format!("{id}:short"))
                .style(move |x| {
                    x.size(12.)
                        .color(name_color)
                        .line_height(LineHeight::Normal)
                        .truncate()
                }),
        );
        let step = (row.nested && !l.parent_step_ids.is_empty()).then(|| {
            let n = l.parent_step_ids.len();
            let title = format!(
                "Plan step{} {} of the parent workspace",
                if n == 1 { "" } else { "s" },
                l.parent_step_ids.join(", ")
            );
            titled(
                SharedString::from(format!("{key}-step")),
                title,
                div()
                    .id(SharedString::from(format!("{key}-step-el")))
                    .flex_none()
                    .py(px(2.))
                    .px(px(5.))
                    .rounded(px(4.))
                    .bg(t.pane_2)
                    .child(
                        text(l.parent_step_ids.join(" "))
                            .mono()
                            .size(10.)
                            .line_height(LineHeight::Scale(1.))
                            .color(t.faint)
                            .nowrap(),
                    ),
            )
        });
        let mut tail = div()
            .ml_auto()
            .flex()
            .items_center()
            .gap(px(5.))
            .flex_none()
            .min_w_0();
        if !acts {
            if l.diff.add != 0 || l.diff.del != 0 {
                tail = tail.child(diff_stat(&key, l.diff.add, l.diff.del, cx));
            }
            tail = tail.children(self.unread_chip(&key, rc.unseen(&id), rc.parked(&id), cx));
        }
        let hit_inner = |d: Div| {
            d.flex()
                .items_center()
                .gap(px(9.))
                .flex_1()
                .min_w_0()
                .h_full()
                .pl(px(14.))
                .rounded(px(6.))
                .relative()
                .when(row.nested, |d| {
                    // The tick from the rail into the child's icon.
                    d.child(
                        div()
                            .absolute()
                            .left(px(7.))
                            .top(gpui_kit::relative(0.5))
                            .w(px(7.))
                            .h(px(1.))
                            .bg(t.line),
                    )
                })
                .child(slot)
                .child(tn)
                .children(step)
                .child(tail)
        };
        let hit: AnyElement = if live {
            let state = self.state().clone();
            let open_id = id.clone();
            let f = self.fh(format!("hit:{id}"), cx);
            let ring = is_focus_visible(&f, window);
            hit_inner(div())
                .id(SharedString::from(format!("hit:{id}")))
                .debug_selector(|| format!("hit:{id}"))
                .track_focus(&f)
                .role(gpui_kit::Role::Button)
                .cursor_pointer()
                .when(ring, |d| d.child(style::focus_ring(4., 0., cx)))
                .on_click(move |_, _, cx| {
                    state.update(cx, |s, cx| s.open_workspace(&open_id, false, cx))
                })
                .into_any_element()
        } else {
            hit_inner(div()).into_any_element()
        };

        let mut el = div()
            .relative()
            .flex()
            .items_center()
            .h(px(ROW_H))
            .w_full()
            .pr(px(8.))
            .when(row.nested, |d| d.pl(px(14.)))
            .rounded(px(6.))
            .when(on, |d| d.bg(t.pane_2))
            .when(!on && hovered, |d| d.bg(t.pane))
            .when(on, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(5.))
                        .bottom(px(5.))
                        .w(px(2.))
                        .rounded(px(2.))
                        .bg(t.amber)
                        .debug_selector(|| format!("on:{id}")),
                )
            })
            .when(row.has_child_below, |d| {
                d.child(vline(cx).left(px(21.)).top(px(ROW_H / 2. + 7.)).bottom_0())
            })
            .when(row.nested, |d| {
                let line = vline(cx).left(px(21.)).top_0();
                d.child(if row.nested_last {
                    line.h(px(ROW_H / 2.))
                } else {
                    line.bottom_0()
                })
            })
            .child(hit);
        if acts {
            let mut box_ = div().flex().items_center().gap(px(1.)).flex_none();
            if let Some(pr) = &pr {
                let label: SharedString = if pr.number != 0 {
                    format!("View PR #{}", pr.number).into()
                } else {
                    "View PR".into()
                };
                let pr_id = pr.id.clone();
                let b = self
                    .act_button(
                        format!("pr:{id}"),
                        label.clone(),
                        pr_icon_name(Some(pr)),
                        18.,
                        12.,
                        true,
                        t.faint,
                        window,
                        cx,
                    )
                    .on_click(
                        cx.guarded_listener(move |this, _, _, cx| this.on_view_pr(&pr_id, cx)),
                    );
                box_ = box_.child(titled(
                    SharedString::from(format!("{key}-pr-tip")),
                    label,
                    b,
                ));
            }
            let title: SharedString = self.remove_title(&id, can_rm).into();
            let (bg, fg) = if armed {
                (Some(t.coral), recur_ui::color::hex(0x0a0c10))
            } else {
                (None, t.faint)
            };
            let (up_id, click_id) = (id.clone(), id.clone());
            let mut rm = self
                .control(format!("rm:{id}"), 4., window, cx)
                .aria_label(title.clone())
                .size(px(18.))
                .flex_none()
                .rounded(px(4.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(fg)
                .when_some(bg, |d, bg| d.bg(bg))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.guarded_listener(move |this, _, _, cx| {
                        this.on_remove_pointer_up(&up_id, can_rm, cx)
                    }),
                )
                .on_click(cx.guarded_listener(move |this, _, _, cx| {
                    this.on_remove_click(&click_id, can_rm, cx)
                }))
                .child(icon(IconName::Trash).size(px(12.)));
            if !can_rm {
                rm = rm.opacity(0.4).cursor_default();
            } else if !armed {
                rm = rm.hover(move |s| s.bg(t.pane_2).text_color(Hsla::from(t.text)));
            }
            box_ = box_.child(titled(
                SharedString::from(format!("{key}-rm-tip")),
                title,
                rm,
            ));
            el = el.child(box_);
        }
        if !live {
            return el.into_any_element();
        }
        let hover_id = id.clone();
        el.debug_selector(|| key.clone())
            .id(SharedString::from(key.clone()))
            .on_hover(cx.guarded_listener(move |this, h: &bool, _, cx| {
                this.set_hover(&format!("row:{hover_id}"), *h, cx);
                if *h {
                    if can_rm {
                        this.prefetch_risk(&hover_id, cx);
                    }
                } else if this.armed() == Some(hover_id.as_str()) {
                    this.clear_armed(cx);
                }
            }))
            .into_any_element()
    }

    /// `.tloop.tmain`: the project's working copy.
    fn main_row(
        &mut self,
        p: &ProjectNode,
        rc: &RowCtx,
        live: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let pid = p.project.id.clone();
        let key = format!("main:{pid}");
        let main = p.main.as_ref();
        let on = rc.main_on(&pid, main);
        let hovered = live && self.hover_key(&key);
        let running = main.is_some_and(|m| workspace_is_running(m, &rc.running));
        let house = icon(IconName::House)
            .size(px(14.))
            .color(t.faint)
            .into_any_element();
        let slot = self.icon_slot(running, &key, house, cx);
        let branch = main.map(|m| m.branch.clone()).unwrap_or_default();
        let (add, del) = main.map_or((0, 0), |m| (m.diff.add, m.diff.del));
        let has_df = add != 0 || del != 0;
        let (unseen, parked) = main.map_or((0, 0), |m| (rc.unseen(&m.id), rc.parked(&m.id)));
        let el = div()
            .relative()
            .flex()
            .items_center()
            .gap(px(9.))
            .h(px(ROW_H))
            .w_full()
            .pl(px(14.))
            .pr(px(8.))
            .rounded(px(6.))
            .when(on, |d| d.bg(t.pane_2))
            .when(!on && hovered, |d| d.bg(t.pane))
            .when(on, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(5.))
                        .bottom(px(5.))
                        .w(px(2.))
                        .rounded(px(2.))
                        .bg(t.amber)
                        .debug_selector(|| format!("on:{key}")),
                )
            })
            .child(slot)
            .child(div().flex_none().child(
                style::button_label("Working copy", 12., weight::REGULAR).color(if on {
                    t.text
                } else {
                    t.muted
                }),
            ))
            .when(!branch.is_empty(), |d| {
                d.child(
                    div()
                        .min_w_0()
                        .py(px(1.))
                        .px(px(6.))
                        .border_l_1()
                        .border_r_1()
                        .border_color(Hsla::from(t.line))
                        .rounded(px(5.))
                        .child(
                            style::button_label(branch.clone(), 10.5, weight::REGULAR)
                                .mono()
                                .color(t.muted)
                                .truncate(),
                        ),
                )
            })
            .when(has_df, |d| {
                d.child(div().ml_auto().child(diff_stat(&key, add, del, cx)))
            })
            .when_some(self.unread_chip(&key, unseen, parked, cx), |d, ct| {
                d.child(div().when(!has_df, |d| d.ml_auto()).child(ct))
            });
        if !live {
            return el.into_any_element();
        }
        let f = self.fh(key.clone(), cx);
        let ring = is_focus_visible(&f, window);
        let title = format!("Working copy — {}'s real checkout", p.project.name);
        let hover_key = key.clone();
        let el =
            el.debug_selector(|| key.clone())
                .id(SharedString::from(key.clone()))
                .track_focus(&f)
                .role(gpui_kit::Role::Button)
                .cursor_pointer()
                .when(ring, |d| d.child(style::focus_ring(4., 0., cx)))
                .on_hover(cx.guarded_listener(move |this, h: &bool, _, cx| {
                    this.set_hover(&hover_key, *h, cx)
                }))
                .on_click(cx.guarded_listener(move |this, _, _, cx| {
                    this.state
                        .update(cx, |s, cx| s.open_main_workspace(&pid, None, cx))
                        .detach();
                }));
        titled(SharedString::from(format!("{key}-tip")), title, el).into_any_element()
    }

    /// `.tsched`: the Scheduled subgroup header and its rows.
    fn sched_section(
        &mut self,
        p: &ProjectNode,
        rc: &RowCtx,
        live: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let pid = p.project.id.clone();
        let key = format!("sched:{pid}");
        let hovered = live && self.hover_key(&key);
        let closed = p.sched_closed;
        let caret = self.caret(&key, closed, cx);
        let head = div()
            .relative()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(SCHED_H))
            .w_full()
            .pl(px(14.))
            .pr(px(8.))
            .rounded(px(6.))
            .text_color(if hovered { t.muted } else { t.faint })
            .when(hovered, |d| d.bg(t.pane))
            .child(
                icon(IconName::ChevronDown)
                    .size(px(14.))
                    .stroke_width(2.4)
                    .rotate(caret),
            )
            .child(
                style::button_label("Scheduled", 10.5, weight::SEMIBOLD)
                    .mono()
                    .tracking(0.04)
                    .uppercase(),
            )
            .when_some(
                self.unread_chip(&key, p.sched_unseen, p.sched_parked, cx),
                |d, ct| d.child(div().ml_auto().child(ct)),
            )
            .when(!closed, |d| {
                d.child(
                    vline(cx)
                        .left(px(21.))
                        .top(px(SCHED_H / 2. + 7.))
                        .bottom_0(),
                )
            });
        let head: AnyElement = if live {
            let f = self.fh(key.clone(), cx);
            let ring = is_focus_visible(&f, window);
            let hover_key = key.clone();
            head.debug_selector(|| key.clone())
                .id(SharedString::from(key.clone()))
                .track_focus(&f)
                .role(gpui_kit::Role::Button)
                .aria_expanded(!closed)
                .cursor_pointer()
                .when(ring, |d| d.child(style::focus_ring(4., 0., cx)))
                .on_hover(cx.guarded_listener(move |this, h: &bool, _, cx| {
                    this.set_hover(&hover_key, *h, cx)
                }))
                .on_click(
                    cx.guarded_listener(move |this, _, _, cx| this.toggle_scheduled(&pid, cx)),
                )
                .into_any_element()
        } else {
            head.into_any_element()
        };
        let mut sec = div().mt(px(SCHED_MT)).child(head);
        if !closed {
            for r in &p.scheduled {
                sec = sec.child(self.task_row(r, rc, live, window, cx));
            }
        }
        sec.into_any_element()
    }

    fn empty_line(label: &'static str, cx: &App) -> Div {
        let t = Theme::of(cx).tokens;
        div()
            .h(px(empty_h()))
            .pt(px(4.))
            .pr(px(8.))
            .pl(px(14.))
            .child(text(label).mono().size(10.5).color(t.faint).nowrap())
    }

    /// `.tloops`: the main row, Scheduled, task rows, or the empty line.
    fn project_loops(
        &mut self,
        p: &ProjectNode,
        rc: &RowCtx,
        live: bool,
        band: Option<(f32, f32, &crate::tree::Layout)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut loops = div()
            .overflow_hidden()
            .child(self.main_row(p, rc, live, window, cx));
        if !p.scheduled.is_empty() {
            loops = loops.child(self.sched_section(p, rc, live, window, cx));
        }
        // Rows outside the visible band (plus overscan) are one spacer per run: a long project
        // lays out the rows on screen, not all of them.
        let mut skipped = 0usize;
        for r in &p.regular {
            if let Some((lo, hi, layout)) = band
                && let Some(&top) = layout.rows.get(&r.ws.id)
                && (top + crate::tree::ROW_H < lo || top > hi)
            {
                skipped += 1;
                continue;
            }
            if skipped > 0 {
                loops = loops.child(div().h(px(skipped as f32 * crate::tree::ROW_H)));
                skipped = 0;
            }
            loops = loops.child(self.task_row(r, rc, live, window, cx));
        }
        if skipped > 0 {
            loops = loops.child(div().h(px(skipped as f32 * crate::tree::ROW_H)));
        }
        if p.regular.is_empty() && p.scheduled.is_empty() && p.project.is_git {
            loops = loops.child(Self::empty_line("no active workspaces", cx));
        }
        loops
    }

    /// `.tproj-h` content: icon slot, name, unread chip, hover actions.
    fn project_header(
        &mut self,
        p: &ProjectNode,
        live: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = Theme::of(cx).tokens;
        let pr = &p.project;
        let key = format!("ph:{}", pr.id);
        let hovered = live && self.hover_key(&key);
        let on = self.project_on(pr, cx);
        let focus_within = live && {
            let keys = [
                format!("ph-unhide:{}", pr.id),
                format!("ph-set:{}", pr.id),
                format!("ph-sched:{}", pr.id),
                format!("ph-new:{}", pr.id),
                key.clone(),
            ];
            keys.iter().any(|k| self.focused(k, window))
        };
        let caret = self.caret(&key, p.collapsed, cx);
        let tico = div()
            .relative()
            .flex_none()
            .size(px(18.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(if hovered {
                icon(IconName::ChevronDown)
                    .size(px(14.))
                    .stroke_width(2.4)
                    .color(t.faint)
                    .rotate(caret)
                    .into_any_element()
            } else {
                project_pic(pr.pic.clone(), pr.color.clone(), pr.name.clone(), PIC)
                    .into_any_element()
            });
        let chip = self.unread_chip(&key, p.unseen, p.parked, cx);
        let has_chip = chip.is_some();
        let on_key = format!("on:{key}");
        let mut el = div()
            .when(on, |d| d.debug_selector(move || on_key))
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(PROJ_H))
            .px(px(8.))
            .size_full()
            .text_color(t.text)
            .child(tico)
            .child(
                div()
                    .min_w_0()
                    .child(style::button_label(pr.name.clone(), 12.5, weight::MEDIUM).truncate()),
            )
            .when_some(chip, |d, ct| d.child(div().ml_auto().child(ct)));
        if live && (hovered || focus_within || on) {
            let (name, id) = (pr.name.clone(), pr.id.clone());
            let mut acts = div()
                .flex()
                .items_center()
                .gap(px(1.))
                .flex_none()
                .when(has_chip, |d| d.ml(px(4.)))
                .when(!has_chip, |d| d.ml_auto());
            let state = self.state().clone();
            if pr.hidden {
                let label: SharedString = format!("Unhide {name}").into();
                let (state, id) = (state.clone(), id.clone());
                let b = self
                    .act_button(
                        format!("ph-unhide:{id}"),
                        label.clone(),
                        IconName::Eye,
                        20.,
                        14.,
                        hovered,
                        t.faint,
                        window,
                        cx,
                    )
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        state
                            .update(cx, |s, cx| s.set_project_hidden(&id, false, cx))
                            .detach();
                    });
                acts = acts.child(titled(
                    SharedString::from(format!("{key}-unhide-tip")),
                    label,
                    b,
                ));
            }
            let set_color = if on { t.text } else { t.faint };
            let b = {
                let (state, id) = (state.clone(), id.clone());
                self.act_button(
                    format!("ph-set:{id}"),
                    format!("Project settings for {name}").into(),
                    IconName::Settings,
                    20.,
                    14.,
                    hovered || on,
                    set_color,
                    window,
                    cx,
                )
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    state.update(cx, |s, cx| s.open_project(&id, cx));
                })
            };
            acts = acts.child(titled(
                SharedString::from(format!("{key}-set-tip")),
                "Project settings",
                b,
            ));
            if pr.is_git {
                let label: SharedString = format!("New schedule in {name}").into();
                let b = {
                    let (state, id) = (state.clone(), id.clone());
                    self.act_button(
                        format!("ph-sched:{id}"),
                        label.clone(),
                        IconName::ClockPlus,
                        20.,
                        14.,
                        hovered,
                        t.faint,
                        window,
                        cx,
                    )
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        state.update(cx, |s, cx| {
                            s.set_schedule_editor(
                                Some(ScheduleEditorRequest {
                                    project_id: Some(id.clone()),
                                    schedule: None,
                                }),
                                cx,
                            )
                        });
                    })
                };
                acts = acts.child(titled(
                    SharedString::from(format!("{key}-sched-tip")),
                    label,
                    b,
                ));
                let label: SharedString = format!("New workspace in {name}").into();
                let b = {
                    let (state, id) = (state.clone(), id.clone());
                    self.act_button(
                        format!("ph-new:{id}"),
                        label.clone(),
                        IconName::Plus,
                        20.,
                        14.,
                        hovered,
                        t.faint,
                        window,
                        cx,
                    )
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        state.update(cx, |s, cx| s.open_new_task(&id, None, cx));
                    })
                };
                acts = acts.child(titled(
                    SharedString::from(format!("{key}-new-tip")),
                    label,
                    b,
                ));
            }
            el = el.child(acts);
        } else if !live && on {
            // The float keeps `.on`'s settings icon showing (`.tproj-h.on .pset`).
            let mut acts = div()
                .flex()
                .items_center()
                .gap(px(1.))
                .flex_none()
                .when(has_chip, |d| d.ml(px(4.)))
                .when(!has_chip, |d| d.ml_auto());
            if pr.hidden {
                acts = acts.child(div().size(px(20.)));
            }
            acts = acts.child(
                div()
                    .size(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(t.text)
                    .child(icon(IconName::Settings).size(px(14.))),
            );
            if pr.is_git {
                acts = acts.child(div().size(px(20.))).child(div().size(px(20.)));
            }
            el = el.child(acts);
        }
        el
    }

    fn project_on(&self, p: &Project, cx: &App) -> bool {
        let s = self.state().read(cx);
        s.view == ViewName::Project && !p.id.is_empty() && p.id == s.project_form_id
    }

    /// `.tgroup-h` content.
    fn group_header(
        &mut self,
        g: &GroupNode,
        live: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = Theme::of(cx).tokens;
        let id = g.group.id.clone();
        let key = format!("gh:{id}");
        let hovered = live && self.hover_key(&key);
        let renaming = live && self.renaming().is_some_and(|(rid, _)| rid == &id);
        let focus_within = live
            && (renaming
                || [
                    format!("gh-rename:{id}"),
                    format!("gh-del:{id}"),
                    key.clone(),
                ]
                .iter()
                .any(|k| self.focused(k, window)));
        let chip = if g.collapsed {
            self.unread_chip(&key, g.unseen, g.parked, cx)
        } else {
            None
        };
        let has_chip = chip.is_some();
        let caret = self.caret(&key, g.collapsed, cx);
        let mut el = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .size_full()
            .text_color(if hovered { t.text } else { t.muted })
            .child(
                icon(IconName::ChevronDown)
                    .size(px(12.))
                    .stroke_width(2.4)
                    .color(t.faint)
                    .rotate(caret),
            );
        if renaming {
            let input = self.renaming().map(|(_, i)| i.clone());
            if let Some(input) = input {
                style::style_text_input(&input, cx);
                let focused =
                    gpui_kit::Focusable::focus_handle(input.read(cx), cx).is_focused(window);
                let field = style::text_field(
                    &input,
                    &SharedString::default(),
                    style::UA_PLACEHOLDER,
                    recur_ui::fonts::Family::Mono,
                    10.5,
                    window,
                    cx,
                );
                el = el.child(
                    div()
                        .id(SharedString::from(format!("gh-input:{id}")))
                        .debug_selector(|| format!("gh-input:{id}"))
                        .flex_1()
                        .min_w_0()
                        .h(px(20.))
                        .px(px(4.))
                        .flex()
                        .items_center()
                        .border_1()
                        .border_color(Hsla::from(if focused { t.muted } else { t.line }))
                        .rounded(px(4.))
                        .bg(t.pane_2)
                        .font_weight(gpui_kit::FontWeight(weight::SEMIBOLD))
                        .aria_label("Group name")
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .capture_key_down(cx.guarded_listener(
                            |this, e: &gpui_kit::KeyDownEvent, _, cx| {
                                if e.keystroke.key == "escape" {
                                    cx.stop_propagation();
                                    this.cancel_rename(cx);
                                }
                            },
                        ))
                        .child(field),
                );
            }
        } else {
            el = el.child(
                div().min_w_0().child(
                    style::button_label(g.group.name.clone(), 10.5, weight::SEMIBOLD)
                        .mono()
                        .tracking(0.04)
                        .uppercase()
                        .truncate(),
                ),
            );
        }
        el = el.when_some(chip, |d, ct| d.child(div().ml_auto().child(ct)));
        if live {
            let show = (hovered || focus_within) && !self.drag.as_ref().is_some_and(|d| d.active);
            let name = g.group.name.clone();
            let gid = id.clone();
            let rename = self
                .act_button(
                    format!("gh-rename:{id}"),
                    format!("Rename {name}").into(),
                    IconName::Pencil,
                    18.,
                    12.,
                    show,
                    t.faint,
                    window,
                    cx,
                )
                .on_click(cx.guarded_listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    let name = this
                        .state()
                        .read(cx)
                        .project_groups
                        .iter()
                        .find(|g| g.id == gid)
                        .map(|g| g.name.clone())
                        .unwrap_or_default();
                    this.start_rename(&gid, &name, window, cx);
                }));
            let gid = id.clone();
            let delete = self
                .act_button(
                    format!("gh-del:{id}"),
                    format!("Delete {name}").into(),
                    IconName::Trash,
                    18.,
                    12.,
                    show,
                    t.faint,
                    window,
                    cx,
                )
                .on_click(cx.guarded_listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.delete_group(&gid, cx);
                }));
            el = el.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(1.))
                    .flex_none()
                    .when(has_chip, |d| d.ml(px(4.)))
                    .when(!has_chip, |d| d.ml_auto())
                    .child(titled(
                        SharedString::from(format!("{key}-rename-tip")),
                        "Rename group",
                        rename,
                    ))
                    .child(titled(
                        SharedString::from(format!("{key}-del-tip")),
                        "Delete group",
                        delete,
                    )),
            );
        }
        el
    }

    /// The header's box with its interactions (hover, click toggle, drag, context menu).
    fn header_box(
        &mut self,
        group: bool,
        id: &str,
        content: Div,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = if group {
            format!("gh:{id}")
        } else {
            format!("ph:{id}")
        };
        let f = self.fh(key.clone(), cx);
        let ring = is_focus_visible(&f, window);
        let (hover_key, click_id, down_id, menu_id) =
            (key.clone(), id.to_string(), id.to_string(), id.to_string());
        let dragging = self.drag.as_ref().is_some_and(|d| d.active);
        let mut el =
            div()
                .debug_selector(|| key.clone())
                .id(SharedString::from(key))
                .track_focus(&f)
                .role(gpui_kit::Role::Button)
                .relative()
                .size_full()
                .rounded(px(6.))
                .when(!dragging, |d| d.cursor_grab())
                .when(ring, |d| d.child(style::focus_ring(4., 0., cx)))
                .on_hover(cx.guarded_listener(move |this, h: &bool, _, cx| {
                    this.set_hover(&hover_key, *h, cx)
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.guarded_listener(move |this, e: &gpui_kit::MouseDownEvent, _, cx| {
                        this.press_header(group, &down_id, e.position, cx)
                    }),
                )
                .on_click(cx.guarded_listener(move |this, _, _, cx| {
                    if std::mem::take(&mut this.suppress_click) {
                        return;
                    }
                    if group {
                        if this.renaming().is_some_and(|(r, _)| r == &click_id) {
                            return;
                        }
                        this.toggle_group(&click_id, cx);
                    } else {
                        this.toggle_project(&click_id, cx);
                    }
                }))
                .child(content);
        if !group {
            el = el.on_mouse_down(
                MouseButton::Right,
                cx.guarded_listener(move |this, e: &gpui_kit::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_move_menu(&menu_id, e.position, cx);
                }),
            );
        }
        el.into_any_element()
    }

    /// A header band: the `::before` full-bleed cover in the header's background, with the
    /// header box inset in it.
    #[allow(clippy::too_many_arguments)]
    fn header_band(
        top: f32,
        height: f32,
        inset_left: f32,
        bleed: bool,
        bg: Color,
        opacity: f32,
        content: AnyElement,
        width: f32,
    ) -> Div {
        let inner = div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(inset_left))
            .right(px(9.))
            .child(content);
        let band = if bleed {
            div()
                .absolute()
                .left_0()
                .right_0()
                .top_0()
                .bottom_0()
                .bg(bg)
        } else {
            div()
                .absolute()
                .left(px(inset_left))
                .right(px(9.))
                .top_0()
                .bottom_0()
                .bg(bg)
        };
        div()
            .absolute()
            .left_0()
            .w(px(width))
            .top(px(top))
            .h(px(height))
            .opacity(opacity)
            .child(band)
            .child(inner)
    }

    /// `::after` under a stuck header: content scrolling under it fades out.
    fn under_fade(
        &mut self,
        key: &str,
        top: f32,
        on: bool,
        inset: Option<(f32, Color)>,
        cx: &App,
    ) -> Option<AnyElement> {
        let t = Theme::of(cx).tokens;
        let fade = self
            .under
            .entry(key.to_string())
            .or_insert_with(|| Fade::new(on));
        fade.set(on, FADE_IN, cx);
        let o = fade.value(cx);
        if o <= 0. {
            return None;
        }
        let (left, right, color) = match inset {
            Some((left, color)) => (left, 9., color),
            None => (0., 0., t.rail),
        };
        Some(
            div()
                .absolute()
                .top(px(top))
                .left(px(left))
                .right(px(right))
                .h(px(FADE))
                .opacity(o)
                .child(fade_gradient(color))
                .into_any_element(),
        )
    }

    pub(crate) fn render_tree(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let tree = self.tree(cx);
        let layout = self.layout(&tree);
        let rc = self.row_ctx(cx);
        let view = self.scroll.bounds();
        let view_h = f32::from(view.size.height);
        let width = f32::from(view.size.width).max(0.);
        let width = if width > 0. {
            width
        } else {
            self.state()
                .read(cx)
                .panels
                .size(recur_view_core::prefs::PanelKey::Rail)
                - 1.
        };
        let max_scroll = (layout.content_h - view_h).max(0.);
        let scroll = self.scroll_top().clamp(0., max_scroll);

        // Scroll activity shows the overlay scrollbar for a while.
        if (scroll - self.seen_scroll).abs() > 0.01 {
            self.seen_scroll = scroll;
            self.scrolling = true;
            self.scroll_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SCROLLBAR_LINGER).await;
                let _ = this.update(cx, |this, cx| {
                    this.scrolling = false;
                    cx.notify();
                });
            }));
        }
        let fade_top = scroll > 1.;
        let fade_bot = max_scroll > 1. && scroll < max_scroll - 1.;
        // Virtualize rows only while nothing moves them (a drag, a flip animation).
        let flipping = self
            .flip
            .keys()
            .any(|k| self.flip_offset(k, cx).abs() > 0.01);
        let band = (view_h > 0. && self.drag.is_none() && !flipping).then_some((
            scroll - ROW_BAND_PAD_PX,
            scroll + view_h + ROW_BAND_PAD_PX,
            &layout,
        ));

        // Sticky headers.
        let mut group_heads: Vec<(usize, Header, f32)> = Vec::new();
        let mut proj_heads: Vec<(Option<usize>, usize, Header, f32)> = Vec::new();
        let mut top_level_stuck = false;
        for (gi, g) in tree.groups.iter().enumerate() {
            let Some(span) = layout.group(&g.group.id) else {
                continue;
            };
            let goff = self.flip_offset(&format!("g:{}", g.group.id), cx);
            let natural = span.top + goff;
            let top = sticky_top(
                natural,
                natural + span.height - GROUP_PB,
                GROUP_H,
                scroll,
                0.,
            );
            let stuck = is_stuck(top, scroll, 0., fade_top);
            top_level_stuck |= stuck;
            group_heads.push((gi, Header { top, stuck }, goff));
            if g.collapsed {
                continue;
            }
            for (pi, p) in g.members.iter().enumerate() {
                let Some((pspan, _)) = layout.project(&p.project.id) else {
                    continue;
                };
                let off = goff + self.flip_offset(&format!("p:{}", p.project.id), cx);
                let natural = pspan.top + off;
                let top = sticky_top(natural, natural + pspan.height, PROJ_H, scroll, GROUP_H);
                let stuck = is_stuck(top, scroll, GROUP_H, fade_top);
                proj_heads.push((Some(gi), pi, Header { top, stuck }, off));
            }
        }
        for (pi, p) in tree.ungrouped.iter().enumerate() {
            let Some((pspan, _)) = layout.project(&p.project.id) else {
                continue;
            };
            let off = self.flip_offset(&format!("p:{}", p.project.id), cx);
            let natural = pspan.top + off;
            let top = sticky_top(natural, natural + pspan.height, PROJ_H, scroll, 0.);
            let stuck = is_stuck(top, scroll, 0., fade_top);
            top_level_stuck |= stuck;
            proj_heads.push((None, pi, Header { top, stuck }, off));
        }
        self.sec_fade.set(fade_top && !top_level_stuck, FADE_IN, cx);

        let drop_group = self
            .drag
            .as_ref()
            .filter(|d| d.active && !d.group)
            .and_then(|d| d.drop_group.clone());
        let float_group = self
            .drag
            .as_ref()
            .filter(|d| d.active && d.group)
            .map(|d| d.id.clone());
        let float_project = self
            .drag
            .as_ref()
            .filter(|d| d.active && !d.group)
            .map(|d| d.id.clone());
        let hole_h = self.drag.as_ref().map_or(0., |d| d.size.1);
        let ph_off = self.flip_offset("ph", cx);

        // Blocks in flow; header slots are spacers the header layer covers.
        let mut content = div()
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .pb(px(crate::tree::TREE_PB));
        let placeholder = |radius: f32, off: f32| {
            div()
                .debug_selector(|| "drag-ph".into())
                .relative()
                .top(px(off))
                .mx(px(9.))
                .h(px(hole_h))
                .rounded(px(radius))
                .bg(t.pane)
                .border_1()
                .border_dashed()
                .border_color(Hsla::from(t.line))
        };
        for g in &tree.groups {
            if float_group.as_deref() == Some(g.group.id.as_str()) {
                content = content.child(placeholder(8., ph_off));
                continue;
            }
            let goff = self.flip_offset(&format!("g:{}", g.group.id), cx);
            let drop = drop_group.as_deref() == Some(g.group.id.as_str());
            let mut block = div()
                .relative()
                .top(px(goff))
                .px(px(9.))
                .pb(px(GROUP_PB))
                .when(drop, |d| {
                    let gid = g.group.id.clone();
                    d.child(
                        div()
                            .debug_selector(move || format!("drop-target:{gid}"))
                            .absolute()
                            .left(px(9.))
                            .right(px(9.))
                            .top_0()
                            .bottom(px(GROUP_PB))
                            .bg(t.pane),
                    )
                })
                .child(div().h(px(GROUP_H)));
            if !g.collapsed {
                let mut body = div().relative().pl(px(2.));
                if g.members.is_empty() {
                    body = body.child(Self::empty_line("no projects", cx));
                }
                for p in &g.members {
                    if float_project.as_deref() == Some(p.project.id.as_str()) {
                        body = body.child(placeholder(6., ph_off));
                        continue;
                    }
                    let off = self.flip_offset(&format!("p:{}", p.project.id), cx);
                    let loops =
                        (!p.collapsed).then(|| self.project_loops(p, &rc, true, band, window, cx));
                    body = body.child(
                        div()
                            .relative()
                            .top(px(off))
                            .pl(px(4.))
                            .child(div().h(px(PROJ_H)))
                            .children(loops),
                    );
                }
                block = block.child(body);
            }
            content = content.child(block);
        }
        for p in &tree.ungrouped {
            if float_project.as_deref() == Some(p.project.id.as_str()) {
                content = content.child(placeholder(6., ph_off));
                continue;
            }
            let off = self.flip_offset(&format!("p:{}", p.project.id), cx);
            let loops = (!p.collapsed).then(|| self.project_loops(p, &rc, true, band, window, cx));
            content = content.child(
                div()
                    .relative()
                    .top(px(off))
                    .px(px(9.))
                    .child(div().h(px(PROJ_H)))
                    .children(loops),
            );
        }

        // Header layer: project headers under group headers, fades under the stuck ones.
        let mut layer: Vec<AnyElement> = Vec::new();
        let mut fades: Vec<AnyElement> = Vec::new();
        let mut stuck_children: HashSet<usize> = HashSet::new();
        for (gi, _, h, _) in &proj_heads {
            if let Some(gi) = gi
                && h.stuck
            {
                stuck_children.insert(*gi);
            }
        }
        let mut seen_under: HashSet<String> = HashSet::new();
        for (gi, pi, h, _) in &proj_heads {
            let p = match gi {
                Some(gi) => &tree.groups[*gi].members[*pi],
                None => &tree.ungrouped[*pi],
            };
            let in_drop = gi
                .is_some_and(|gi| drop_group.as_deref() == Some(tree.groups[gi].group.id.as_str()));
            let key = format!("ph:{}", p.project.id);
            let on = self.project_on(&p.project, cx);
            let bg = if on {
                t.pane_2
            } else if in_drop || self.hover_key(&key) {
                t.pane
            } else {
                t.rail
            };
            let inset = if gi.is_some() { 15. } else { 9. };
            let header = self.project_header(p, true, window, cx);
            let boxed = self.header_box(false, &p.project.id, header, window, cx);
            let opacity = if p.project.hidden { 0.55 } else { 1. };
            layer.push(
                Self::header_band(h.top, PROJ_H, inset, !in_drop, bg, opacity, boxed, width)
                    .into_any_element(),
            );
            let fade_inset = in_drop.then_some((inset, t.pane));
            seen_under.insert(key.clone());
            fades.extend(self.under_fade(&key, h.top + PROJ_H, h.stuck, fade_inset, cx));
        }
        let mut group_layer: Vec<AnyElement> = Vec::new();
        let mut rings: Vec<AnyElement> = Vec::new();
        for (gi, h, goff) in &group_heads {
            let g = &tree.groups[*gi];
            let key = format!("gh:{}", g.group.id);
            let in_drop = drop_group.as_deref() == Some(g.group.id.as_str());
            let bg = if in_drop || self.hover_key(&key) {
                t.pane
            } else {
                t.rail
            };
            let header = self.group_header(g, true, window, cx);
            let boxed = self.header_box(true, &g.group.id, header, window, cx);
            group_layer.push(
                Self::header_band(h.top, GROUP_H, 9., !in_drop, bg, 1., boxed, width)
                    .into_any_element(),
            );
            let under_on = h.stuck && !stuck_children.contains(gi);
            seen_under.insert(key.clone());
            let fade_inset = in_drop.then_some((9., t.pane));
            fades.extend(self.under_fade(&key, h.top + GROUP_H, under_on, fade_inset, cx));
            if in_drop && let Some(span) = layout.group(&g.group.id) {
                rings.push(
                    div()
                        .absolute()
                        .top(px(span.top + goff))
                        .h(px(span.height))
                        .left(px(9.))
                        .right(px(9.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(Hsla::from(t.line))
                        .into_any_element(),
                );
            }
        }
        self.under.retain(|k, _| seen_under.contains(k));
        // `.tproj-h` (z 2) and its fade, then `.tgroup-h` (z 3) and its fade, then the ring.
        let mut proj_fades = Vec::new();
        let mut group_fades = Vec::new();
        let n_proj = proj_heads.len();
        for (i, f) in fades.into_iter().enumerate() {
            if i < n_proj {
                proj_fades.push(f);
            } else {
                group_fades.push(f);
            }
        }
        content = content
            .children(layer)
            .children(proj_fades)
            .children(group_layer)
            .children(group_fades)
            .children(rings);

        let scroller = div()
            .id("tree")
            .debug_selector(|| "rail-tree".into())
            .role(gpui_kit::Role::List)
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(content);

        // `.rail-tree.fade-bot`: the last 20px of the scrollport fade into the rail.
        let mask = fade_bot.then(|| {
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(FADE))
                .child(style::css_gradient(
                    180.,
                    (t.rail.with_alpha(0.), 0.),
                    (t.rail, 1.),
                    0.,
                    0.,
                    rgba(0, 0, 0, 0.),
                ))
        });

        // The overlay scrollbar.
        let sb_on = self.scrolling && (fade_top || fade_bot);
        self.sb.set(sb_on, if sb_on { FADE_IN } else { SB_OUT }, cx);
        let sb_o = self.sb.value(cx);
        let (thumb_h, thumb_top) = thumb(scroll, layout.content_h, view_h);
        let bar = (sb_o > 0.).then(|| {
            div()
                .absolute()
                .top_0()
                .right(px(1.))
                .bottom(px(8.))
                .w(px(6.))
                .opacity(sb_o)
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(thumb_top))
                        .h(px(thumb_h))
                        .rounded(px(99.))
                        .bg(t.scrollbar_thumb),
                )
        });

        let pr_host_running = {
            let s = self.state().read(cx);
            recur_view_core::pr::pr_chat_host_running_count(&s.workspaces, &s.workspaces_running)
                > 0
        };
        let animating = self.sb.animating(cx)
            || self.sec_fade.animating(cx)
            || self.under.values().any(|f| f.animating(cx))
            || self.flip_animating(cx)
            || self.drag.as_ref().is_some_and(|d| d.settle.is_some())
            || self.carets.values().any(|f| f.animating(cx))
            || (!motion::reduced(cx) && (self.has_running_dot(&tree, &rc) || pr_host_running));
        if animating && !motion::is_frozen(cx) {
            window.request_animation_frame();
        }
        let float = self.render_float(&tree, &rc, window, cx);
        let listeners = self.drag_listeners(cx);
        div()
            .relative()
            .size_full()
            .child(scroller)
            .children(mask)
            .children(bar)
            .children(float)
            .children(listeners)
            .into_any_element()
    }

    fn has_running_dot(&self, tree: &Tree, rc: &RowCtx) -> bool {
        let running = |w: &Workspace| workspace_is_running(w, &rc.running);
        let project = |p: &ProjectNode| {
            !p.collapsed
                && (p.main.as_ref().is_some_and(running)
                    || p.regular.iter().any(|r| running(&r.ws))
                    || (!p.sched_closed && p.scheduled.iter().any(|r| running(&r.ws))))
        };
        tree.ungrouped.iter().any(project)
            || tree
                .groups
                .iter()
                .any(|g| !g.collapsed && g.members.iter().any(project))
    }

    /// A caret's rotation, easing over `.12s` when its block opens or closes.
    fn caret(&mut self, key: &str, collapsed: bool, cx: &App) -> f32 {
        let fade = self
            .carets
            .entry(key.to_string())
            .or_insert_with(|| Fade::new(collapsed));
        fade.set(collapsed, CARET, cx);
        -90. * fade.value(cx)
    }

    fn flip_animating(&self, cx: &App) -> bool {
        self.flip
            .values()
            .any(|&(_, at)| motion::progress(at, FLIP, cx) < 1.)
    }

    /// The block being dragged, floating under the pointer (`.tproj.is-float`,
    /// `.tgroup.is-float`): a static copy without pointer events.
    fn render_float(
        &mut self,
        tree: &Tree,
        rc: &RowCtx,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let t = Theme::of(cx).tokens;
        let d = self.drag.as_ref().filter(|d| d.active)?;
        let (group, id, (w, _)) = (d.group, d.id.clone(), d.size);
        let (x, y, settle) = d.float_pos(cx);
        let shadow_a = 0.4 * (1. - settle);
        let border = t.line.fade(1. - settle);
        let body: Div = if group {
            let g = tree.groups.iter().find(|g| g.group.id == id)?.clone();
            let header = self.group_header(&g, false, window, cx);
            let mut el = div()
                .w(px(w))
                .px(px(9.))
                .pb(px(GROUP_PB))
                .rounded(px(8.))
                .bg(t.rail)
                .border_1()
                .border_color(Hsla::from(border))
                .child(div().h(px(GROUP_H)).child(header));
            if !g.collapsed {
                let mut b = div().pl(px(2.));
                if g.members.is_empty() {
                    b = b.child(Self::empty_line("no projects", cx));
                }
                for p in &g.members {
                    let header = self.project_header(p, false, window, cx);
                    let on = self.project_on(&p.project, cx);
                    let loops =
                        (!p.collapsed).then(|| self.project_loops(p, rc, false, None, window, cx));
                    b = b.child(
                        div()
                            .pl(px(4.))
                            .child(
                                div()
                                    .h(px(PROJ_H))
                                    .rounded(px(6.))
                                    .bg(if on { t.pane_2 } else { t.rail })
                                    .when(p.project.hidden, |d| d.opacity(0.55))
                                    .child(header),
                            )
                            .children(loops),
                    );
                }
                el = el.child(b);
            }
            el
        } else {
            let p = tree
                .ungrouped
                .iter()
                .chain(tree.groups.iter().flat_map(|g| g.members.iter()))
                .find(|p| p.project.id == id)?
                .clone();
            let header = self.project_header(&p, false, window, cx);
            let on = self.project_on(&p.project, cx);
            let loops = (!p.collapsed).then(|| self.project_loops(&p, rc, false, None, window, cx));
            div()
                .w(px(w))
                .px(px(9.))
                .rounded(px(6.))
                .bg(t.rail)
                .border_1()
                .border_color(Hsla::from(border))
                .child(
                    div()
                        .relative()
                        .h(px(PROJ_H))
                        .when(p.project.hidden, |d| d.opacity(0.55))
                        // The `::before` band reaches the float's padding edge.
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(px(-9.))
                                .right(px(-9.))
                                .bg(if on { t.pane_2 } else { t.rail }),
                        )
                        .child(div().relative().size_full().child(header)),
                )
                .children(loops)
        };
        let radius = if group { 8. } else { 6. };
        Some(
            gpui_kit::deferred(style::anchored_at(
                x,
                y,
                recur_ui::shadow::with_shadow(
                    body,
                    &[recur_ui::shadow::css_box_shadow(
                        0.,
                        10.,
                        32.,
                        0.,
                        rgba(0, 0, 0, shadow_a),
                    )],
                    radius,
                ),
            ))
            .with_priority(5)
            .into_any_element(),
        )
    }
}
