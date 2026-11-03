//! Free pointer drag in the project tree: projects (within and across groups) or group headers
//! (group order). The block floats under the pointer over a placeholder, siblings FLIP into
//! their new places, and on release the float settles into the placeholder before the order is
//! persisted.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui_kit::{
    AnyElement, App, Context, CursorStyle, DispatchPhase, IntoElement, MouseButton, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, Styled, canvas,
};
use recur_domain::{Project, ProjectGroup};
use recur_ui::motion::{self, Easing};
use recur_view_core::call;
use recur_view_core::workspace_nav::unique_by_id;

use crate::Sidebar;
use crate::tree::{GROUP_H, GUTTER, Layout, Span};

const DRAG_THRESHOLD: f32 = 4.;
const SETTLE: Duration = Duration::from_millis(200);
/// How near (px) a pointer between groups still snaps into one.
const GROUP_SNAP: f32 = 28.;
const AUTOSCROLL_EDGE: f32 = 36.;
const AUTOSCROLL_STEP: f32 = 10.;

/// The float easing into the placeholder.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Settle {
    at: Instant,
    from: (f32, f32),
    to: (f32, f32),
}

#[derive(Clone, Debug)]
pub(crate) struct Drag {
    pub(crate) group: bool,
    pub(crate) id: String,
    /// Past the threshold: floating and reordering.
    pub(crate) active: bool,
    start: Point<Pixels>,
    offset: (f32, f32),
    /// The block's size at pickup.
    pub(crate) size: (f32, f32),
    /// The float's window position.
    pos: (f32, f32),
    before_projects: Vec<Project>,
    before_groups: Vec<ProjectGroup>,
    /// Collapsed groups opened while targeting them.
    opened: Vec<String>,
    pub(crate) drop_group: Option<String>,
    pub(crate) settle: Option<Settle>,
}

impl Drag {
    /// The float's position and how far it has settled (`0..=1`).
    pub(crate) fn float_pos(&self, cx: &App) -> (f32, f32, f32) {
        match self.settle {
            Some(s) => {
                let p = motion::eased(s.at, SETTLE, Easing::SETTLE, cx);
                (
                    s.from.0 + (s.to.0 - s.from.0) * p,
                    s.from.1 + (s.to.1 - s.from.1) * p,
                    p,
                )
            }
            None => (self.pos.0, self.pos.1, 0.),
        }
    }
}

/// `rebuildOrder`: the global project order with groups first (in group order), then
/// ungrouped. `bucket` replaces `bucket_gid`'s membership (ids may move across groups).
pub fn rebuild_order(
    list: &[Project],
    groups: &[ProjectGroup],
    bucket_gid: &str,
    bucket: &[Project],
) -> Vec<Project> {
    let bucket_ids: HashSet<&str> = bucket.iter().map(|p| p.id.as_str()).collect();
    let normalized: Vec<Project> = unique_by_id(bucket, false)
        .into_iter()
        .map(|p| Project {
            group_id: bucket_gid.to_string(),
            ..p
        })
        .collect();
    let mut by_group: HashMap<String, Vec<Project>> = HashMap::new();
    for p in list {
        if bucket_ids.contains(p.id.as_str()) {
            continue;
        }
        by_group
            .entry(p.group_id.clone())
            .or_default()
            .push(p.clone());
    }
    by_group.insert(bucket_gid.to_string(), normalized);
    let mut next = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut take = |ps: Option<&Vec<Project>>, next: &mut Vec<Project>| {
        for p in ps.into_iter().flatten() {
            if seen.insert(p.id.clone()) {
                next.push(p.clone());
            }
        }
    };
    for g in unique_by_id(groups, false) {
        take(by_group.get(&g.id), &mut next);
    }
    take(by_group.get(""), &mut next);
    let mut seen: HashSet<String> = next.iter().map(|p| p.id.clone()).collect();
    for p in list {
        if seen.contains(&p.id) || bucket_ids.contains(p.id.as_str()) {
            continue;
        }
        seen.insert(p.id.clone());
        next.push(Project {
            group_id: String::new(),
            ..p.clone()
        });
    }
    next
}

/// `resolveDropGroup`: the group whose block contains `y`, else the nearest within
/// [`GROUP_SNAP`], else ungrouped (`""`).
pub fn resolve_drop_group(y: f32, groups: &[(String, f32, f32)]) -> String {
    for (id, top, bottom) in groups {
        if y >= *top && y <= *bottom {
            return id.clone();
        }
    }
    let mut best = (String::new(), f32::INFINITY);
    for (id, top, bottom) in groups {
        let dist = if y < *top {
            top - y
        } else if y > *bottom {
            y - bottom
        } else {
            0.
        };
        if dist < best.1 && dist < GROUP_SNAP {
            best = (id.clone(), dist);
        }
    }
    if best.1 < GROUP_SNAP {
        best.0
    } else {
        String::new()
    }
}

impl Sidebar {
    /// Window y of a content-space y.
    fn window_y(&self, y: f32) -> f32 {
        f32::from(self.scroll.bounds().origin.y) + y - self.scroll_top()
    }

    fn tree_left(&self) -> f32 {
        f32::from(self.scroll.bounds().origin.x)
    }

    fn tree_width(&self) -> f32 {
        f32::from(self.scroll.bounds().size.width)
    }

    pub(crate) fn press_header(
        &mut self,
        group: bool,
        id: &str,
        pos: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.drag.is_some() {
            return;
        }
        if group && self.renaming().is_some_and(|(r, _)| r == id) {
            return;
        }
        let tree = self.tree(cx);
        let layout = self.layout(&tree);
        let (span, x, w) = if group {
            let Some(span) = layout.group(id) else { return };
            (span, self.tree_left(), self.tree_width())
        } else {
            let Some((span, in_group)) = layout.project(id) else {
                return;
            };
            if in_group {
                (span, self.tree_left() + 11., self.tree_width() - 20.)
            } else {
                (span, self.tree_left(), self.tree_width())
            }
        };
        let y = self.window_y(span.top);
        self.suppress_click = false;
        self.drag = Some(Drag {
            group,
            id: id.to_string(),
            active: false,
            start: pos,
            offset: (f32::from(pos.x) - x, f32::from(pos.y) - y),
            size: (w, span.height),
            pos: (x, y),
            before_projects: Vec::new(),
            before_groups: Vec::new(),
            opened: Vec::new(),
            drop_group: None,
            settle: None,
        });
    }

    fn drag_move(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(d) = self.drag.as_mut() else { return };
        if d.settle.is_some() {
            return;
        }
        if !d.active {
            let dx = f32::from(pos.x - d.start.x);
            let dy = f32::from(pos.y - d.start.y);
            if dx.hypot(dy) < DRAG_THRESHOLD {
                return;
            }
            d.active = true;
            let s = self.state.read(cx);
            d.before_projects = s.projects.clone();
            d.before_groups = s.project_groups.clone();
            self.suppress_click = true;
            self.hovered = None;
            self.sync_peek_lock(cx);
        }
        let Some(d) = self.drag.as_mut() else { return };
        d.pos = (f32::from(pos.x) - d.offset.0, f32::from(pos.y) - d.offset.1);
        let y = f32::from(pos.y);
        if d.group {
            self.reorder_groups(y, cx);
        } else {
            self.reorder_projects(y, cx);
        }
        self.autoscroll(y);
        cx.notify();
    }

    fn autoscroll(&self, y: f32) {
        let b = self.scroll.bounds();
        let (top, bottom) = (f32::from(b.top()), f32::from(b.bottom()));
        if y < top + AUTOSCROLL_EDGE {
            self.set_scroll_top((self.scroll_top() - AUTOSCROLL_STEP).max(0.));
        } else if y > bottom - AUTOSCROLL_EDGE {
            let max = f32::from(self.scroll.max_offset().y).abs();
            self.set_scroll_top((self.scroll_top() + AUTOSCROLL_STEP).min(max));
        }
    }

    /// Visual tops (layout plus any running FLIP) for the blocks a drag can move.
    fn flip_tops(&self, group: bool, layout: &Layout, cx: &App) -> HashMap<String, f32> {
        let mut m = HashMap::new();
        if group {
            for (id, span) in &layout.groups {
                let key = format!("g:{id}");
                m.insert(key.clone(), span.top + self.flip_offset(&key, cx));
            }
        } else {
            for (id, span, _) in &layout.projects {
                let key = format!("p:{id}");
                m.insert(key.clone(), span.top + self.flip_offset(&key, cx));
            }
        }
        if let Some(h) = layout.hole {
            m.insert("ph".into(), h.top + self.flip_offset("ph", cx));
        }
        m
    }

    /// Animate blocks from their old visual tops into the new layout.
    fn flip_from(&mut self, group: bool, from: HashMap<String, f32>, cx: &App) {
        if motion::reduced(cx) {
            return;
        }
        let tree = self.tree(cx);
        let layout = self.layout(&tree);
        let now = recur_ui::motion::now(cx);
        let mut to: Vec<(String, f32)> = Vec::new();
        if group {
            to.extend(
                layout
                    .groups
                    .iter()
                    .map(|(id, s)| (format!("g:{id}"), s.top)),
            );
        } else {
            to.extend(
                layout
                    .projects
                    .iter()
                    .map(|(id, s, _)| (format!("p:{id}"), s.top)),
            );
        }
        if let Some(h) = layout.hole {
            to.push(("ph".into(), h.top));
        }
        for (key, top) in to {
            let Some(old) = from.get(&key) else { continue };
            let dy = old - top;
            if dy.abs() >= 0.5 {
                self.flip.insert(key, (dy, now));
            } else {
                self.flip.remove(&key);
            }
        }
    }

    fn reorder_projects(&mut self, y: f32, cx: &mut Context<Self>) {
        let Some(id) = self.drag.as_ref().map(|d| d.id.clone()) else {
            return;
        };
        let tree = self.tree(cx);
        let layout = self.layout(&tree);
        let groups: Vec<(String, f32, f32)> = layout
            .groups
            .iter()
            .map(|(g, s)| (g.clone(), self.window_y(s.top), self.window_y(s.bottom())))
            .collect();
        let target = resolve_drop_group(y, &groups);
        if let Some(d) = self.drag.as_mut() {
            d.drop_group = Some(target.clone()).filter(|g| !g.is_empty());
        }
        if !target.is_empty() && self.collapse.groups.get(&target).copied().unwrap_or(false) {
            self.collapse.groups.insert(target.clone(), false);
            if let Some(d) = self.drag.as_mut()
                && !d.opened.contains(&target)
            {
                d.opened.push(target.clone());
            }
        }
        let (list, all_groups) = {
            let s = self.state().read(cx);
            (s.projects.clone(), s.project_groups.clone())
        };
        let Some(dragged) = list.iter().find(|p| p.id == id).cloned() else {
            return;
        };
        let others: Vec<Project> = list
            .iter()
            .filter(|p| p.id != id && p.group_id == target)
            .cloned()
            .collect();
        // Rects from the layout as it stands (a target group may just have opened).
        let tree = self.tree(cx);
        let layout = self.layout(&tree);
        let mut insert_at = others.len();
        for (i, o) in others.iter().enumerate() {
            let Some((span, _)) = layout.project(&o.id) else {
                continue;
            };
            if y < self.window_y(span.top) + span.height / 2. {
                insert_at = i;
                break;
            }
        }
        if !target.is_empty()
            && !others.is_empty()
            && let Some(span) = layout.group(&target)
        {
            // Dropping on the group's (possibly stuck) header inserts at the top.
            let scroll = self.scroll_top();
            let head = crate::tree::sticky_top(
                span.top,
                span.bottom() - crate::tree::GROUP_PB,
                GROUP_H,
                scroll,
                0.,
            );
            if y < self.window_y(head) + GROUP_H {
                insert_at = 0;
            }
        }
        let moved = Project {
            group_id: target.clone(),
            ..dragged.clone()
        };
        let mut next_bucket = others.clone();
        next_bucket.insert(insert_at, moved);
        let prev_bucket: Vec<&Project> = list
            .iter()
            .filter(|p| p.group_id == target || p.id == id)
            .collect();
        let same = dragged.group_id == target
            && prev_bucket.len() == next_bucket.len()
            && next_bucket
                .iter()
                .zip(&prev_bucket)
                .all(|(a, b)| a.id == b.id);
        if same {
            return;
        }
        let from = self.flip_tops(false, &layout, cx);
        let next = rebuild_order(&list, &all_groups, &target, &next_bucket);
        self.state().update(cx, |s, cx| s.set_projects(next, cx));
        self.flip_from(false, from, cx);
    }

    fn reorder_groups(&mut self, y: f32, cx: &mut Context<Self>) {
        let Some(id) = self.drag.as_ref().map(|d| d.id.clone()) else {
            return;
        };
        let list = self.state().read(cx).project_groups.clone();
        let Some(dragged) = list.iter().find(|g| g.id == id).cloned() else {
            return;
        };
        let tree = self.tree(cx);
        let layout = self.layout(&tree);
        let others: Vec<ProjectGroup> = list.iter().filter(|g| g.id != id).cloned().collect();
        let mut insert_at = others.len();
        for (i, o) in others.iter().enumerate() {
            let Some(span) = layout.group(&o.id) else {
                continue;
            };
            if y < self.window_y(span.top) + span.height / 2. {
                insert_at = i;
                break;
            }
        }
        let mut next = others;
        next.insert(insert_at, dragged);
        if next.iter().zip(&list).all(|(a, b)| a.id == b.id) {
            return;
        }
        let from = self.flip_tops(true, &layout, cx);
        self.state()
            .update(cx, |s, cx| s.set_project_groups(next, cx));
        self.flip_from(true, from, cx);
    }

    fn drag_up(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.drag.as_mut() else { return };
        if d.settle.is_some() {
            return;
        }
        if !d.active {
            self.drag = None;
            return;
        }
        d.drop_group = None;
        // The placeholder's window rect, where the float settles.
        let tree = self.tree(cx);
        let layout = self.layout(&tree);
        let Some(hole) = layout.hole else {
            self.finish(cx);
            return;
        };
        let in_group =
            self.drag.as_ref().is_some_and(|d| !d.group) && self.hole_in_group(&layout, hole, cx);
        let x = self.tree_left() + if in_group { 11. + GUTTER } else { GUTTER };
        let to = (x, self.window_y(hole.top));
        let Some(d) = self.drag.as_mut() else { return };
        if motion::reduced(cx) {
            d.pos = to;
            self.finish(cx);
            return;
        }
        d.settle = Some(Settle {
            at: recur_ui::motion::now(cx),
            from: d.pos,
            to,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            let _ = this.update(cx, |this, cx| this.finish(cx));
        })
        .detach();
    }

    fn hole_in_group(&self, layout: &Layout, hole: Span, _cx: &App) -> bool {
        layout
            .groups
            .iter()
            .any(|(_, g)| hole.top >= g.top + GROUP_H && hole.top < g.bottom())
    }

    /// Drop the float and persist what changed.
    fn finish(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.drag.take() else { return };
        self.sync_peek_lock(cx);
        cx.notify();
        if d.group {
            self.persist_group_order(d.before_groups, cx);
        } else {
            self.persist_project_drop(d.before_projects, d.opened, d.id, cx);
        }
    }

    fn persist_project_drop(
        &mut self,
        snapshot: Vec<Project>,
        opened: Vec<String>,
        dragged: String,
        cx: &mut Context<Self>,
    ) {
        let current = self.state().read(cx).projects.clone();
        let order_same = current.len() == snapshot.len()
            && current.iter().zip(&snapshot).all(|(a, b)| a.id == b.id);
        let moved: Vec<Project> = current
            .iter()
            .filter(|p| {
                snapshot
                    .iter()
                    .find(|x| x.id == p.id)
                    .is_some_and(|prev| prev.group_id != p.group_id)
            })
            .cloned()
            .collect();
        if order_same && moved.is_empty() {
            // Nothing to persist: re-collapse groups only opened for targeting.
            for gid in opened {
                self.collapse.groups.insert(gid, true);
            }
            return;
        }
        let services = self.services.clone();
        let state = self.state().clone();
        cx.spawn(async move |this, cx| {
            let recollapse = |this: &gpui_kit::WeakEntity<Sidebar>,
                              ids: &[String],
                              cx: &mut gpui_kit::AsyncApp| {
                let _ = this.update(cx, |this, cx| {
                    for gid in ids {
                        this.collapse.groups.insert(gid.clone(), true);
                    }
                    cx.notify();
                });
            };
            for p in moved {
                let saved = call(
                    cx,
                    services.projects.set_project_group(&p.id, &p.group_id),
                    "Could not move project",
                )
                .await;
                let Some(saved) = saved else {
                    let _ = state.update(cx, |s, cx| s.load_initial(cx)).await;
                    recollapse(&this, &opened, cx);
                    return;
                };
                state.update(cx, |s, cx| {
                    s.update_project(&p.id, |x| x.group_id = saved.group_id.clone(), cx)
                });
            }
            let landed = state.read_with(cx, |s, _| {
                s.project(&dragged)
                    .map(|p| p.group_id.clone())
                    .unwrap_or_default()
            });
            for gid in opened {
                if gid != landed {
                    recollapse(&this, std::slice::from_ref(&gid), cx);
                    continue;
                }
                let Some(g) = state.read_with(cx, |s, _| {
                    s.project_groups.iter().find(|x| x.id == gid).cloned()
                }) else {
                    continue;
                };
                if g.open {
                    continue;
                }
                if call(
                    cx,
                    services
                        .projects
                        .update_project_group(&ProjectGroup { open: true, ..g }),
                    "Could not save group view",
                )
                .await
                .is_some()
                {
                    state.update(cx, |s, cx| crate::set_group_open(s, &gid, true, cx));
                }
            }
            let ids: Vec<String> =
                state.read_with(cx, |s, _| s.projects.iter().map(|p| p.id.clone()).collect());
            if call(
                cx,
                services.projects.reorder_projects(&ids),
                "Could not reorder projects",
            )
            .await
            .is_none()
            {
                let _ = state.update(cx, |s, cx| s.load_initial(cx)).await;
            }
        })
        .detach();
    }

    fn persist_group_order(&mut self, snapshot: Vec<ProjectGroup>, cx: &mut Context<Self>) {
        let current = self.state().read(cx).project_groups.clone();
        let unchanged = current.len() == snapshot.len()
            && current.iter().zip(&snapshot).all(|(a, b)| a.id == b.id);
        if unchanged {
            return;
        }
        let services = self.services.clone();
        let state = self.state().clone();
        let ids: Vec<String> = current.iter().map(|g| g.id.clone()).collect();
        cx.spawn(async move |_, cx| {
            if call(
                cx,
                services.projects.reorder_project_groups(&ids),
                "Could not reorder groups",
            )
            .await
            .is_none()
            {
                state.update(cx, |s, cx| s.set_project_groups(snapshot, cx));
            }
        })
        .detach();
    }

    /// While a header is pressed, the whole window follows the pointer (pointer capture).
    pub(crate) fn drag_listeners(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let active = self.drag.as_ref()?.active;
        let entity = cx.entity();
        Some(
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    if active {
                        window.set_window_cursor_style(CursorStyle::ClosedHand);
                    }
                    let e = entity.clone();
                    window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture {
                            e.update(cx, |this, cx| this.drag_move(ev.position, cx));
                        }
                    });
                    let e = entity.clone();
                    window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture && ev.button == MouseButton::Left {
                            e.update(cx, |this, cx| this.drag_up(cx));
                        }
                    });
                },
            )
            .absolute()
            .size_full()
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: &str, g: &str) -> Project {
        Project {
            id: id.into(),
            group_id: g.into(),
            ..Project::default()
        }
    }

    fn g(id: &str) -> ProjectGroup {
        ProjectGroup {
            id: id.into(),
            ..ProjectGroup::default()
        }
    }

    fn ids(list: &[Project]) -> Vec<(&str, &str)> {
        list.iter()
            .map(|p| (p.id.as_str(), p.group_id.as_str()))
            .collect()
    }

    #[test]
    fn rebuild_moves_across_groups_and_keeps_group_order() {
        let list = vec![p("a", "g1"), p("b", "g2"), p("c", ""), p("d", "g1")];
        let groups = vec![g("g1"), g("g2")];
        // Move c into g2 above b.
        let bucket = vec![p("c", ""), p("b", "g2")];
        let next = rebuild_order(&list, &groups, "g2", &bucket);
        assert_eq!(
            ids(&next),
            [("a", "g1"), ("d", "g1"), ("c", "g2"), ("b", "g2")]
        );
        // Reorder within ungrouped.
        let list = vec![p("a", ""), p("b", ""), p("c", "g1")];
        let next = rebuild_order(&list, &[g("g1")], "", &[p("b", ""), p("a", "")]);
        assert_eq!(ids(&next), [("c", "g1"), ("b", ""), ("a", "")]);
        // A project of a deleted group falls back to ungrouped.
        let list = vec![p("a", "gone"), p("b", "")];
        let next = rebuild_order(&list, &[], "", &[p("b", "")]);
        assert_eq!(ids(&next), [("b", ""), ("a", "")]);
    }

    #[test]
    fn drop_group_snaps_within_range() {
        let groups = vec![
            ("g1".to_string(), 100., 200.),
            ("g2".to_string(), 230., 300.),
        ];
        assert_eq!(resolve_drop_group(150., &groups), "g1");
        assert_eq!(resolve_drop_group(210., &groups), "g1");
        assert_eq!(resolve_drop_group(222., &groups), "g2");
        assert_eq!(resolve_drop_group(340., &groups), "");
        assert_eq!(resolve_drop_group(80., &groups), "g1");
        assert_eq!(resolve_drop_group(50., &groups), "");
    }
}
