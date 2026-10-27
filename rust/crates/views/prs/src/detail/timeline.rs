//! Windowing for long PR conversations: past a length only the timeline items near the
//! viewport lay out, and runs of the rest are spacers at their measured (or estimated)
//! height. GPUI lays the whole page out on every frame, where the browser lays out only what
//! changed, so a 200-item timeline cannot mount at once.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EntityId, InteractiveElement, IntoElement,
    ParentElement, Render, ScrollHandle, SharedString, Styled, Subscription, WeakEntity, Window,
    canvas, div, point, px,
};
use recur_domain::PrComment;
use recur_ui::guard::Guarded;
use recur_ui::layout_root::BlockSlot;

use super::PrDetail;

/// Timelines shorter than this mount whole.
pub(crate) const TIMELINE_WINDOW_FROM: usize = 40;
/// Prefetch band above and below the scrollport, in px.
const PAD_PX: f32 = 600.;

#[derive(Default)]
struct Probe {
    /// Laid-out heights by item id.
    heights: HashMap<String, f32>,
    /// The thread's top in scroll-content coordinates.
    thread_top: f32,
    /// The scrollport height the last layout saw.
    view_h: f32,
}

#[derive(Default)]
pub(crate) struct TimelineWindow {
    pr: String,
    probe: Rc<RefCell<Probe>>,
    /// The height each item reserved in the last plan, for scroll anchoring.
    reserved: HashMap<String, f32>,
    planned_view_h: f32,
    /// Measured items as their own cached views, by id.
    panes: HashMap<String, Entity<ItemPane>>,
    /// A short timeline's items as cached blocks, by id, for `blocks_pr`.
    blocks: HashMap<String, (Entity<ItemPane>, Rc<BlockSlot>)>,
    blocks_pr: String,
}

/// `None` mounts the item; `Some(h)` reserves `h` px for it.
pub(crate) type Plan = Vec<Option<f32>>;

/// A height for an item that has not laid out yet.
pub(crate) fn estimate(c: &PrComment) -> f32 {
    if c.kind == "action" {
        return 36.;
    }
    let n = if c.body_md.is_empty() {
        c.body_html.len() / 2
    } else {
        c.body_md.len()
    };
    // simplify: ~90 columns at 21px; an item is measured the first time it mounts.
    ((n as f32 / 90.).ceil() * 21. + 72.).clamp(88., 900.)
}

impl TimelineWindow {
    /// Which of `roots` mount for the scroller's current offset. Items above the viewport
    /// that laid out at a height other than the one they reserved shift the offset by the
    /// difference, so what is on screen stays put.
    pub(crate) fn plan(&mut self, pr: &str, roots: &[&PrComment], scroll: &ScrollHandle) -> Plan {
        if self.pr != pr {
            self.pr = pr.to_string();
            self.reserved.clear();
            self.panes.clear();
            *self.probe.borrow_mut() = Probe::default();
        }
        let probe = self.probe.borrow();
        let height = |c: &PrComment| {
            probe
                .heights
                .get(&c.id)
                .copied()
                .unwrap_or_else(|| estimate(c))
        };
        let offset = scroll.offset();
        let mut top = -f32::from(offset.y);
        let mut y = probe.thread_top;
        let mut shift = 0.;
        for c in roots {
            let h = height(c);
            if let Some(&was) = self.reserved.get(&c.id)
                && y + was <= top
            {
                shift += h - was;
            }
            y += h;
        }
        if shift != 0. {
            top = (top + shift).max(0.);
            scroll.set_offset(point(offset.x, px(-top)));
        }
        let view_h = probe.view_h.max(f32::from(scroll.bounds().size.height));
        self.planned_view_h = view_h;
        let (lo, hi) = (top - PAD_PX, top + view_h + PAD_PX);
        let mut y = probe.thread_top;
        let mut plan = Vec::with_capacity(roots.len());
        for c in roots {
            let h = height(c);
            self.reserved.insert(c.id.clone(), h);
            plan.push((y >= hi || y + h <= lo).then_some(h));
            y += h;
        }
        plan
    }

    /// Records where the thread sits; a layout that disagrees with the plan (a new viewport
    /// height, or the thread moved) asks `view` for another frame.
    pub(crate) fn thread_probe(&self, scroll: &ScrollHandle, view: EntityId) -> AnyElement {
        let (rec, scroll, planned) = (self.probe.clone(), scroll.clone(), self.planned_view_h);
        canvas(
            move |bounds, _, cx: &mut App| {
                let vp = scroll.bounds();
                let top = f32::from(bounds.origin.y)
                    - f32::from(vp.origin.y)
                    - f32::from(scroll.offset().y);
                let view_h = f32::from(vp.size.height);
                let mut p = rec.borrow_mut();
                if (p.thread_top - top).abs() > 0.5 || (planned - view_h).abs() > 0.5 {
                    cx.notify(view);
                }
                p.thread_top = top;
                p.view_h = view_h;
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
        .into_any_element()
    }

    /// An item mounting for the first time, measuring itself; a height other than the reserved
    /// one replans.
    pub(crate) fn live(&self, c: &PrComment, el: AnyElement, view: EntityId) -> AnyElement {
        let rec = self.probe.clone();
        let key = c.id.clone();
        let id = SharedString::from(format!("tli-{}", c.id));
        let reserved = self.reserved.get(&c.id).copied();
        let probe = canvas(
            move |bounds, _, cx: &mut App| {
                let h = f32::from(bounds.size.height);
                if reserved.is_none_or(|r| (r - h).abs() > 0.5) {
                    cx.notify(view);
                }
                rec.borrow_mut().heights.insert(key.clone(), h);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        div()
            .id(id)
            .relative()
            .flex()
            .flex_col()
            .child(el)
            .child(probe)
            .into_any_element()
    }

    /// The height `id` laid out at, once it has mounted.
    pub(crate) fn measured(&self, id: &str) -> Option<f32> {
        self.probe.borrow().heights.get(id).copied()
    }

    /// The cached view for a measured item.
    pub(crate) fn pane(&mut self, id: &str, cx: &mut Context<PrDetail>) -> Entity<ItemPane> {
        if let Some(p) = self.panes.get(id) {
            return p.clone();
        }
        let detail = cx.entity();
        let probe = self.probe.clone();
        let id = id.to_string();
        let pane = cx.new(|cx| ItemPane {
            detail: detail.downgrade(),
            probe,
            id: id.clone(),
            block: None,
            _sub: cx.guarded_observe(&detail, |_, _, cx| cx.notify()),
        });
        self.panes.insert(id, pane.clone());
        pane
    }

    /// A short timeline's item `id` as its own cached view, laid out in place through
    /// [`recur_ui::layout_root::cached_block`].
    pub(crate) fn block(
        &mut self,
        pr: &str,
        id: &str,
        cx: &mut Context<PrDetail>,
    ) -> (Entity<ItemPane>, Rc<BlockSlot>) {
        if self.blocks_pr != pr {
            self.blocks_pr = pr.to_string();
            self.blocks.clear();
        }
        if let Some(b) = self.blocks.get(id) {
            return b.clone();
        }
        let detail = cx.entity();
        let slot = Rc::new(BlockSlot::default());
        let pane = cx.new(|cx| ItemPane {
            detail: detail.downgrade(),
            probe: Rc::default(),
            id: id.to_string(),
            block: Some(slot.clone()),
            _sub: cx.guarded_observe(&detail, |_, _, cx| cx.notify()),
        });
        self.blocks
            .insert(id.to_string(), (pane.clone(), slot.clone()));
        (pane, slot)
    }

    /// A run of windowed-out items as one spacer.
    pub(crate) fn gap(h: f32) -> AnyElement {
        div().flex_none().h(px(h)).into_any_element()
    }
}

/// A measured timeline item as its own cached view, mounted at its measured height: a redraw
/// of the page that does not notify the PR detail (a key typed in the comment box) reuses its
/// layout. Any notify of the PR detail redraws it; a new natural height replans the window.
pub struct ItemPane {
    detail: WeakEntity<PrDetail>,
    probe: Rc<RefCell<Probe>>,
    id: String,
    /// Set for a short timeline's item, which its block lays out and measures.
    block: Option<Rc<BlockSlot>>,
    _sub: Subscription,
}

impl ItemPane {
    fn render_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(detail) = self.detail.upgrade() else {
            return div().into_any_element();
        };
        let id = self.id.clone();
        let el = detail.update(cx, |d, cx| d.timeline_item_by_id(&id, window, cx));
        // Its text selects, inside the cached view that replays it (`recur_ui::selection`).
        let sel =
            recur_ui::selection::state(SharedString::from(format!("tl-sel-{id}")), window, cx);
        let el = recur_ui::selection::scope(&sel, false, el, cx);
        if let Some(slot) = &self.block {
            return recur_ui::layout_root::block_content(slot, el, window);
        }
        let (rec, view) = (self.probe.clone(), detail.entity_id());
        let measure = canvas(
            move |bounds, _, cx: &mut App| {
                let h = f32::from(bounds.size.height);
                let changed = rec
                    .borrow()
                    .heights
                    .get(&id)
                    .is_none_or(|r| (r - h).abs() > 0.5);
                if changed {
                    rec.borrow_mut().heights.insert(id.clone(), h);
                    cx.notify(view);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        // Sized by its content, not the box, so the probe reads the item's natural height.
        div()
            .w_full()
            .relative()
            .flex()
            .flex_col()
            .child(el)
            .child(measure)
            .into_any_element()
    }
}

impl Render for ItemPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, body: &str) -> PrComment {
        PrComment {
            id: id.into(),
            body_md: body.into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_long_timeline_mounts_only_what_is_near_the_viewport() {
        let items: Vec<PrComment> = (0..200).map(|i| item(&format!("c{i}"), "hi")).collect();
        let roots: Vec<&PrComment> = items.iter().collect();
        let mut w = TimelineWindow::default();
        let scroll = ScrollHandle::new();
        w.plan("pr", &roots, &scroll);
        w.probe.borrow_mut().view_h = 800.;
        let plan = w.plan("pr", &roots, &scroll);
        let live = plan.iter().filter(|p| p.is_none()).count();
        assert!(plan[0].is_none());
        assert!((10..30).contains(&live), "{live} of {} mounted", plan.len());
    }

    #[test]
    fn an_item_above_the_viewport_that_grows_keeps_the_view_still() {
        let items: Vec<PrComment> = (0..100).map(|i| item(&format!("c{i}"), "hi")).collect();
        let roots: Vec<&PrComment> = items.iter().collect();
        let mut w = TimelineWindow::default();
        let scroll = ScrollHandle::new();
        scroll.set_offset(point(px(0.), px(-2000.)));
        w.plan("pr", &roots, &scroll);
        w.probe
            .borrow_mut()
            .heights
            .insert("c0".into(), estimate(&items[0]) + 50.);
        w.plan("pr", &roots, &scroll);
        assert_eq!(f32::from(scroll.offset().y), -2050.);
    }
}
