//! Isolated views (`style::isolated_view`): a notify of one renders only it, and the replayed
//! frame around it keeps its content, hit-testing, key dispatch and deferred draws.

use std::cell::RefCell;
use std::collections::BTreeMap;

use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, TestAppContext,
    VisualTestContext, Window, deferred, div, px,
};

use crate::guard::Guarded as _;
use crate::style::{cached_view, isolated_view};

thread_local! {
    static RENDERS: RefCell<BTreeMap<&'static str, u32>> = RefCell::default();
}

fn rendered(name: &'static str) {
    RENDERS.with(|r| *r.borrow_mut().entry(name).or_default() += 1);
}

fn take_renders() -> BTreeMap<&'static str, u32> {
    RENDERS.with(|r| std::mem::take(&mut *r.borrow_mut()))
}

fn sized(h: f32) -> gpui_kit::StyleRefinement {
    let mut s = div().w_full().h(px(h)).flex_none();
    s.style().clone()
}

/// A focusable field: each key counts up.
struct Field {
    name: &'static str,
    keys: u32,
    focus: FocusHandle,
}

impl Render for Field {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        rendered(self.name);
        let label = format!("{}:{}", self.name, self.keys);
        div()
            .id(self.name)
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.guarded_listener(|this, _, _, cx| {
                this.keys += 1;
                cx.notify();
            }))
            .child(div().size_full().debug_selector(move || label.clone()))
    }
}

/// A cached sibling: each click counts up.
struct Sibling {
    clicks: u32,
}

impl Render for Sibling {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        rendered("sib");
        let label = format!("sib:{}", self.clicks);
        div()
            .id("sib")
            .size_full()
            .on_click(cx.guarded_listener(|this, _, _, cx| {
                this.clicks += 1;
                cx.notify();
            }))
            .child(div().size_full().debug_selector(move || label.clone()))
            .child("some text, so the replay carries line layouts")
    }
}

/// A cached group holding an isolated field with a cached sibling after it.
struct Group {
    field: Entity<Field>,
    sib: Entity<Sibling>,
}

impl Render for Group {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        rendered("group");
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().h(px(10.5)).flex_none())
            .child(isolated_view(&self.field, sized(30.)))
            .child(cached_view(&self.sib, sized(30.)))
    }
}

/// The host: uncached, with the cached group, a second isolated field straight under it and a
/// deferred popover after them.
struct Host {
    redraws: u32,
    pops: u32,
    group: Entity<Group>,
    field2: Entity<Field>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        rendered("host");
        let (host, pop) = (
            format!("host:{}", self.redraws),
            format!("pop:{}", self.pops),
        );
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().h(px(20.)).debug_selector(move || host.clone()))
            .child(cached_view(&self.group, sized(80.)))
            .child(div().h(px(7.25)).flex_none())
            .child(isolated_view(&self.field2, sized(30.)))
            .child(
                div().relative().child(deferred(
                    div()
                        .id("pop")
                        .absolute()
                        .top(px(4.))
                        .w(px(50.))
                        .h(px(20.))
                        .on_click(cx.guarded_listener(|this, _, _, cx| {
                            this.pops += 1;
                            cx.notify();
                        }))
                        .child(div().size_full().debug_selector(move || pop.clone())),
                )),
            )
    }
}

struct Root {
    host: Entity<Host>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        rendered("root");
        div().size_full().child(self.host.clone())
    }
}

struct Fixture<'a> {
    cx: &'a mut VisualTestContext,
    host: Entity<Host>,
    field: Entity<Field>,
    field2: Entity<Field>,
}

impl Fixture<'_> {
    fn at(&mut self, selector: &str) -> gpui_kit::Bounds<gpui_kit::Pixels> {
        let selector: &'static str = Box::leak(selector.to_string().into_boxed_str());
        self.cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not drawn"))
    }

    /// Two keys into `field`: what the second rendered (the first after a click refreshes the
    /// whole window, as GPUI does on an input-modality change).
    fn key_into(&mut self, field: &Entity<Field>) -> BTreeMap<&'static str, u32> {
        let focus = field.read_with(self.cx, |f, _| f.focus.clone());
        self.cx.update(|window, cx| window.focus(&focus, cx));
        self.cx.simulate_keystrokes("a");
        self.cx.run_until_parked();
        take_renders();
        self.cx.simulate_keystrokes("a");
        self.cx.run_until_parked();
        take_renders()
    }

    fn click(&mut self, selector: &str) -> BTreeMap<&'static str, u32> {
        let at = self.at(selector).center();
        take_renders();
        self.cx.simulate_click(at, gpui_kit::Modifiers::none());
        self.cx.run_until_parked();
        take_renders()
    }
}

fn fixture(cx: &mut TestAppContext) -> Fixture<'_> {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::init(cx);
    });
    let mut parts = None;
    let (_, vcx) = cx.add_window_view(|_, cx| {
        let field = |name, cx: &mut gpui_kit::App| {
            cx.new(|cx| Field {
                name,
                keys: 0,
                focus: cx.focus_handle(),
            })
        };
        let (f1, f2) = (field("field", cx), field("field2", cx));
        let sib = cx.new(|_| Sibling { clicks: 0 });
        let group = cx.new(|_| Group {
            field: f1.clone(),
            sib,
        });
        let host = cx.new(|_| Host {
            redraws: 0,
            pops: 0,
            group,
            field2: f2.clone(),
        });
        parts = Some((host.clone(), f1, f2));
        Root { host }
    });
    vcx.run_until_parked();
    let (host, field, field2) = parts.unwrap();
    Fixture {
        cx: vcx,
        host,
        field,
        field2,
    }
}

#[gpui_kit::test]
fn a_key_in_an_isolated_view_renders_only_it(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    let places = ["field:0", "sib:0", "field2:0", "pop:0", "host:0"].map(|s| f.at(s));
    let field = f.field.clone();
    let counts = f.key_into(&field);
    assert_eq!(
        counts,
        BTreeMap::from([("field", 1)]),
        "a key under a cached group"
    );
    let field2 = f.field2.clone();
    let counts = f.key_into(&field2);
    assert_eq!(
        counts,
        BTreeMap::from([("field2", 1)]),
        "a key under the host"
    );
    assert_eq!(
        ["field:2", "sib:0", "field2:2", "pop:0", "host:0"].map(|s| f.at(s)),
        places,
        "the replayed frame moved something"
    );
}

/// Replays alternate with full frames that render the host and reuse the group, a click on
/// a cached view after the island and the deferred popover: every frame keeps every view's
/// content, place and handlers.
#[gpui_kit::test]
fn replays_keep_the_frame_around_them_working(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    let place = |f: &mut Fixture, s: &str| f.at(s);
    let (p_field, p_sib, p_field2, p_pop) = (
        place(&mut f, "field:0"),
        place(&mut f, "sib:0"),
        place(&mut f, "field2:0"),
        place(&mut f, "pop:0"),
    );
    let (field, field2) = (f.field.clone(), f.field2.clone());
    for round in 1..=3u32 {
        assert_eq!(f.key_into(&field), BTreeMap::from([("field", 1)]));
        assert_eq!(f.key_into(&field2), BTreeMap::from([("field2", 1)]));

        let host = f.host.clone();
        f.cx.update(|_, cx| {
            host.update(cx, |h, cx| {
                h.redraws += 1;
                cx.notify();
            })
        });
        f.cx.run_until_parked();
        let counts = take_renders();
        assert_eq!(counts.get("group"), None, "the group replays: {counts:?}");
        assert_eq!(counts.get("field"), None, "{counts:?}");
        assert!(counts.contains_key("host"), "{counts:?}");
        assert_eq!(f.at(&format!("host:{round}")).size.height, px(20.));

        let counts = f.click(&format!("sib:{}", round - 1));
        assert!(
            counts.contains_key("sib"),
            "the sibling's click: {counts:?}"
        );
        let counts = f.click(&format!("pop:{}", round - 1));
        assert!(
            counts.contains_key("host"),
            "the popover's click: {counts:?}"
        );

        assert_eq!(f.at(&format!("field:{}", 2 * round)), p_field);
        assert_eq!(f.at(&format!("sib:{round}")), p_sib);
        assert_eq!(f.at(&format!("field2:{}", 2 * round)), p_field2);
        assert_eq!(f.at(&format!("pop:{round}")), p_pop);
    }
}

/// A font change between replays: GPUI does not carry line layouts across one, so a replayed
/// run is shorter than the one it copies, and the views it carries must stay in bounds.
#[gpui_kit::test]
fn a_font_change_between_replays_keeps_the_frame(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    let field = f.field.clone();
    f.key_into(&field);
    f.cx.update(|_, cx| crate::fonts::register(cx).expect("fonts"));
    f.key_into(&field);
    let host = f.host.clone();
    f.cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    f.cx.run_until_parked();
    f.key_into(&field);
    f.cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    f.cx.run_until_parked();
    f.at("field:6");
    f.at("sib:0");
}

/// Typing keeps the frame's books the same size: a replay that drew an isolated view again
/// recorded its state twice, and the duplicate rode every later replay (frames slowed over a
/// long run).
#[gpui_kit::test]
fn typing_does_not_grow_the_frame(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    let field = f.field.clone();
    f.key_into(&field);
    let count = |f: &mut Fixture| f.cx.update(|window, _| window.frame_sizes());
    let before = count(&mut f);
    for _ in 0..10 {
        f.key_into(&field);
    }
    assert_eq!(count(&mut f), before);
}

/// An isolated view that paints nothing (an emptied transcript) beside another.
struct Blank {
    filled: bool,
}

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let d = div().size_full();
        if self.filled {
            d.bg(gpui_kit::red())
                .debug_selector(|| "blank:filled".into())
        } else {
            d
        }
    }
}

struct Painted {
    n: u32,
}

impl Render for Painted {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let n = self.n;
        div()
            .size_full()
            .bg(gpui_kit::blue())
            .debug_selector(move || format!("painted:{n}"))
    }
}

struct Pair {
    blank: Entity<Blank>,
    painted: Entity<Painted>,
}

impl Render for Pair {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(isolated_view(&self.blank, sized(40.)))
            .child(isolated_view(&self.painted, sized(40.)))
    }
}

/// Replays keep their books when an isolated view drew nothing: the runs around it touch, and
/// the view beside it was mapped to the wrong side of the empty one, so a later replay of both
/// read an inverted range and panicked.
#[gpui_kit::test]
fn an_isolated_view_that_paints_nothing_keeps_the_replay_in_order(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::init(cx);
    });
    let mut parts = None;
    let (_, cx) = cx.add_window_view(|_, cx| {
        let blank = cx.new(|_| Blank { filled: false });
        let painted = cx.new(|_| Painted { n: 0 });
        parts = Some((blank.clone(), painted.clone()));
        Pair { blank, painted }
    });
    cx.run_until_parked();
    let (blank, painted) = parts.unwrap();
    for n in 1..4 {
        // The empty view fills while the other replays beside it, then both redraw.
        cx.update(|_, cx| {
            blank.update(cx, |b, cx| {
                b.filled = true;
                cx.notify()
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            blank.update(cx, |_, cx| cx.notify());
            painted.update(cx, |p, cx| {
                p.n = n;
                cx.notify()
            });
        });
        cx.run_until_parked();
        let at: &'static str = Box::leak(format!("painted:{n}").into_boxed_str());
        assert!(cx.debug_bounds(at).is_some(), "{at} is not drawn");
        cx.update(|_, cx| {
            blank.update(cx, |b, cx| {
                b.filled = false;
                cx.notify()
            })
        });
        cx.run_until_parked();
    }
}

/// A draw that replays the root around an island counts the root as replayed, so what a
/// view painted last frame (a native page under a hole, a selection) knows it is still up.
#[gpui_kit::test]
fn a_root_replay_counts_the_root_as_replayed(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    let field2 = f.field2.clone();
    f.key_into(&field2);
    let replayed = std::rc::Rc::new(std::cell::Cell::new(false));
    let seen = replayed.clone();
    f.cx.update(|window, _| {
        let root = window.root::<Root>().flatten().unwrap().entity_id();
        window.after_next_draw(move |window, _| seen.set(window.replayed_view(root)));
    });
    f.cx.simulate_keystrokes("a");
    f.cx.run_until_parked();
    assert_eq!(take_renders(), BTreeMap::from([("field2", 1)]));
    assert!(replayed.get());
}
