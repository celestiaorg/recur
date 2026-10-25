//! Windowing for long PR conversations: past a length only the timeline items near the
//! viewport lay out, and runs of the rest are spacers at their measured (or estimated)
//! height. GPUI lays the whole page out on every frame, where the browser lays out only what
//! changed, so a 200-item timeline cannot mount at once.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, EntityId, InteractiveElement, IntoElement, ParentElement, ScrollHandle,
    SharedString, Styled, canvas, div, point, px,
};
use recur_domain::PrComment;

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
}

/// `None` mounts the item; `Some(h)` reserves `h` px for it.
pub(crate) type Plan = Vec<Option<f32>>;

fn estimate(c: &PrComment) -> f32 {
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

    /// A mounted item, measuring itself; a height other than the reserved one replans.
    pub(crate) fn live(&self, c: &PrComment, el: AnyElement, view: EntityId) -> AnyElement {
        let rec = self.probe.clone();
        let reserved = self.reserved.get(&c.id).copied();
        let key = c.id.clone();
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
            .id(SharedString::from(format!("tli-{}", c.id)))
            .relative()
            .flex()
            .flex_col()
            .child(el)
            .child(probe)
            .into_any_element()
    }

    /// A run of windowed-out items as one spacer.
    pub(crate) fn gap(h: f32) -> AnyElement {
        div().flex_none().h(px(h)).into_any_element()
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
