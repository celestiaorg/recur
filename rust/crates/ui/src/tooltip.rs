//! `TitleTooltip.svelte` and `lib/title-tooltip.ts`: the app's themed replacement for native
//! `title` tooltips. Wrap a control in [`titled`]; render one [`TitleTooltip`] overlay as the
//! last child of each window root.
//!
//! Each frame the hovered titled elements report themselves (innermost last), the overlay
//! feeds the same state machine as the web UI (160ms first-show delay, 480ms reshow window,
//! click/Escape dismissal until the pointer leaves) and places the tip below the control,
//! flipping above when it does not fit.

use crate::motion::{self, Easing};
use crate::shadow::{css_box_shadow, with_shadow};
use crate::text::{Text, TextStyled};
use crate::theme::Theme;
use gpui_kit::{
    AnyElement, App, AppContext, Bounds, Context, DispatchPhase, Element, ElementId, Entity,
    FocusHandle, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement,
    KeyDownEvent, LayoutId, MouseDownEvent, MouseExitEvent, MouseMoveEvent, ParentElement, Pixels,
    Render, ScrollWheelEvent, SharedString, Styled, Task, Window, WindowId, canvas, deferred, div,
    px, relative,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// First show: short enough to feel instant, long enough to cross icon rows.
pub const SHOW_DELAY_MS: u64 = 160;
/// After a recent hide, skip the delay so chaining titled controls feels continuous.
pub const RESHOW_WINDOW_MS: u64 = 480;
pub const TIP_GAP: f32 = 6.;
pub const TIP_PAD: f32 = 8.;
/// Prefer the pointer as the X anchor once the target is this wide.
pub const TIP_CURSOR_ANCHOR_PX: f32 = 160.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TipPlacement {
    Above,
    Below,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TipPlace {
    pub x: f32,
    pub y: f32,
    pub placement: TipPlacement,
}

/// Target box in CSS px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TipRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl TipRect {
    fn width(&self) -> f32 {
        self.right - self.left
    }
}

/// `placeTooltip`: centered below the target (or under the cursor on wide targets), clamped
/// to the viewport, flipped above only when below does not fit and above does.
pub fn place_tooltip(
    target: TipRect,
    tip: (f32, f32),
    view: (f32, f32),
    cursor_x: Option<f32>,
) -> TipPlace {
    let (tw, th) = tip;
    let (vw, vh) = view;
    let mut anchor_x = target.left + target.width() / 2.;
    if let Some(cx) = cursor_x
        && cx.is_finite()
        && target.width() >= TIP_CURSOR_ANCHOR_PX
        && cx >= target.left
        && cx <= target.right
    {
        anchor_x = cx;
    }
    let max_x = TIP_PAD.max(vw - tw - TIP_PAD);
    let x = (anchor_x - tw / 2.).clamp(TIP_PAD, max_x);
    let below = target.bottom + TIP_GAP;
    let above = target.top - TIP_GAP - th;
    let below_fits = below + th + TIP_PAD <= vh;
    let above_fits = above >= TIP_PAD;
    let placement = if below_fits || !above_fits {
        TipPlacement::Below
    } else {
        TipPlacement::Above
    };
    let y = match placement {
        TipPlacement::Below => below,
        TipPlacement::Above => above,
    }
    .clamp(TIP_PAD, TIP_PAD.max(vh - th - TIP_PAD));
    TipPlace { x, y, placement }
}

/// `showDelayMs`.
pub fn show_delay_ms(now: u64, last_hide_at: u64, already_visible: bool) -> u64 {
    if already_visible || (last_hide_at > 0 && now.saturating_sub(last_hide_at) < RESHOW_WINDOW_MS)
    {
        0
    } else {
        SHOW_DELAY_MS
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HideReason {
    Leave,
    Dismiss,
    Interrupt,
}

/// `shouldReshowAfterDismiss`: a different title on the same control is a new tip.
pub fn should_reshow_after_dismiss(dismissed: &str, next: Option<&str>) -> bool {
    next.is_some_and(|n| !n.is_empty() && n != dismissed)
}

/// `TipCore`: the fields the overlay drives. `last_hide_at` 0 means never.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TipCore<T> {
    pub target: Option<T>,
    pub pending: Option<T>,
    pub dismissed: Option<T>,
    pub visible: bool,
    pub last_hide_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TipAction<T> {
    Noop,
    Hide(HideReason),
    Reveal(T),
    Schedule(T, u64),
}

/// `applyHide`.
pub fn apply_hide<T: Copy>(s: TipCore<T>, reason: HideReason, now: u64) -> TipCore<T> {
    TipCore {
        target: None,
        pending: None,
        dismissed: if reason == HideReason::Dismiss {
            s.target.or(s.pending).or(s.dismissed)
        } else {
            s.dismissed
        },
        visible: false,
        last_hide_at: if reason == HideReason::Leave
            && (s.visible || s.target.is_some() || s.pending.is_some())
        {
            now
        } else {
            s.last_hide_at
        },
    }
}

/// `applyConsider`: the pointer or focus is now on `el` (or nothing).
pub fn apply_consider<T: Copy + PartialEq>(
    s: TipCore<T>,
    el: Option<T>,
    now: u64,
) -> (TipCore<T>, TipAction<T>) {
    if el.is_some() && el == s.dismissed {
        return (s, TipAction::Noop);
    }
    let dismissed = if s.dismissed.is_some() && el != s.dismissed {
        None
    } else {
        s.dismissed
    };
    let cur = TipCore { dismissed, ..s };
    let Some(el) = el else {
        return (
            apply_hide(cur, HideReason::Leave, now),
            TipAction::Hide(HideReason::Leave),
        );
    };
    if Some(el) == cur.target || Some(el) == cur.pending {
        return (cur, TipAction::Noop);
    }
    let delay = show_delay_ms(now, cur.last_hide_at, cur.visible);
    if delay == 0 {
        (
            TipCore {
                pending: None,
                ..cur
            },
            TipAction::Reveal(el),
        )
    } else {
        (
            TipCore {
                pending: Some(el),
                ..cur
            },
            TipAction::Schedule(el, delay),
        )
    }
}

/// `applyTimer`: the show delay elapsed.
pub fn apply_timer<T: Copy>(s: TipCore<T>) -> (TipCore<T>, TipAction<T>) {
    match s.pending {
        Some(el) => (TipCore { pending: None, ..s }, TipAction::Reveal(el)),
        None => (s, TipAction::Noop),
    }
}

/// A titled control as painted this frame.
#[derive(Clone, Debug, PartialEq)]
struct Candidate {
    key: u64,
    text: SharedString,
    bounds: Bounds<Pixels>,
}

#[derive(Default)]
struct Frame {
    /// Hovered titled elements, in paint order (innermost/topmost last).
    hovered: Vec<Candidate>,
    /// Keyboard-focused titled element.
    focused: Option<Candidate>,
    /// Every titled element painted this frame, for retitle and removal checks.
    present: HashMap<u64, Candidate>,
}

thread_local! {
    static FRAMES: RefCell<HashMap<WindowId, Frame>> = RefCell::default();
}

fn epoch_ms() -> u64 {
    thread_local! {
        static START: Instant = Instant::now();
    }
    START.with(|s| s.elapsed().as_millis() as u64 + 1)
}

/// A control with a themed `title` tooltip. The wrapper adds no box of its own.
pub struct Titled {
    id: ElementId,
    text: SharedString,
    focus: Option<FocusHandle>,
    child: AnyElement,
}

/// `title="…"` on `child`. `id` must be stable across frames.
pub fn titled(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    child: impl IntoElement,
) -> Titled {
    Titled {
        id: id.into(),
        text: text.into(),
        focus: None,
        child: child.into_any_element(),
    }
}

impl Titled {
    /// Also show the tip while `handle` has keyboard focus.
    pub fn focus(mut self, handle: &FocusHandle) -> Self {
        self.focus = Some(handle.clone());
        self
    }
}

impl IntoElement for Titled {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

fn key_of(id: &GlobalElementId) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

impl Element for Titled {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        self.child.prepaint(window, cx);
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
        let Some(id) = id else { return };
        let text: SharedString = self.text.trim().to_string().into();
        let candidate = Candidate {
            key: key_of(id),
            text,
            bounds,
        };
        let hovered = hitbox.is_hovered(window);
        let focused = self
            .focus
            .as_ref()
            .is_some_and(|f| f.is_focused(window) && window.last_input_was_keyboard());
        let window_id = window.window_handle().window_id();
        FRAMES.with(|f| {
            let mut f = f.borrow_mut();
            let frame = f.entry(window_id).or_default();
            frame.present.insert(candidate.key, candidate.clone());
            if !candidate.text.is_empty() {
                if hovered {
                    frame.hovered.push(candidate.clone());
                }
                if focused {
                    frame.focused = Some(candidate);
                }
            }
        });
        // Re-run the overlay when the pointer crosses this control's edge.
        let hitbox = hitbox.clone();
        window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, _| {
            if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) != hovered {
                window.refresh();
            }
        });
    }
}

/// The tooltip overlay for one window. Create it once per window and render it as the last
/// child of the root.
pub struct TitleTooltip {
    core: TipCore<u64>,
    /// The shown (or dismissed) control's last painted state.
    shown: Option<Candidate>,
    dismissed_text: SharedString,
    shown_at: Option<Instant>,
    cursor: Option<Pixels>,
    timer: Option<Task<()>>,
    viewport: Option<gpui_kit::Size<Pixels>>,
}

impl TitleTooltip {
    pub fn new(cx: &mut App) -> Entity<TitleTooltip> {
        cx.new(|_| TitleTooltip {
            core: TipCore::default(),
            shown: None,
            dismissed_text: SharedString::default(),
            shown_at: None,
            cursor: None,
            timer: None,
            viewport: None,
        })
    }

    fn hide(&mut self, reason: HideReason) {
        if reason == HideReason::Dismiss {
            self.dismissed_text = self
                .shown
                .as_ref()
                .map(|c| c.text.clone())
                .unwrap_or_default();
        }
        self.core = apply_hide(self.core, reason, epoch_ms());
        self.timer = None;
        self.shown = None;
        self.shown_at = None;
    }

    fn reveal(&mut self, c: Candidate) {
        self.core.target = Some(c.key);
        self.core.visible = true;
        self.shown = Some(c);
        self.shown_at = Some(Instant::now());
    }

    /// Feed this frame's candidates into the machine.
    fn update(&mut self, frame: Frame, window: &mut Window, cx: &mut Context<Self>) {
        // Retitle, removal and dismissed-retitle checks (`watch` / `watchDismissed`).
        if let Some(key) = self.core.target {
            match frame.present.get(&key) {
                None => self.hide(HideReason::Interrupt),
                Some(c) if c.text.is_empty() => self.hide(HideReason::Interrupt),
                Some(c) => {
                    if self.shown.as_ref() != Some(c) {
                        self.shown = Some(c.clone());
                    }
                }
            }
        }
        if let Some(key) = self.core.dismissed
            && let Some(c) = frame.present.get(&key)
            && should_reshow_after_dismiss(&self.dismissed_text, Some(&c.text))
            && frame.hovered.last().is_some_and(|h| h.key == key)
        {
            self.core.dismissed = None;
            self.reveal(c.clone());
            return;
        }
        let candidate = frame.hovered.last().cloned().or(frame.focused);
        let (next, action) =
            apply_consider(self.core, candidate.as_ref().map(|c| c.key), epoch_ms());
        self.core = next;
        match action {
            TipAction::Noop => {}
            TipAction::Hide(reason) => self.hide(reason),
            TipAction::Reveal(_) => {
                if let Some(c) = candidate {
                    self.reveal(c);
                }
            }
            TipAction::Schedule(_, delay) => {
                let Some(c) = candidate else { return };
                self.timer = Some(cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(delay))
                        .await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        let (next, action) = apply_timer(this.core);
                        this.core = next;
                        if let TipAction::Reveal(key) = action
                            && key == c.key
                        {
                            this.reveal(c.clone());
                        }
                        cx.notify();
                        window.refresh();
                    });
                }));
            }
        }
    }
}

impl Render for TitleTooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        if (self.viewport.is_some_and(|v| v != viewport) || !window.is_window_active())
            && (self.core.visible || self.core.pending.is_some())
        {
            self.hide(HideReason::Interrupt);
        }
        self.viewport = Some(viewport);
        let entity = cx.entity();
        // Collect this frame's titled controls after everything else has painted.
        let collector = deferred(
            canvas(
                |_, _, _| {},
                move |_, _, window, cx| {
                    let window_id = window.window_handle().window_id();
                    let frame = FRAMES
                        .with(|f| f.borrow_mut().remove(&window_id))
                        .unwrap_or_default();
                    let before = entity.read(cx).render_key();
                    entity.update(cx, |this, cx| this.update(frame, window, cx));
                    if entity.read(cx).render_key() != before {
                        window.refresh();
                    }
                    let e = entity.clone();
                    window.on_mouse_event(move |ev: &MouseMoveEvent, _, _, cx| {
                        e.update(cx, |this, _| this.cursor = Some(ev.position.x));
                    });
                    let e = entity.clone();
                    window.on_mouse_event(move |_: &MouseDownEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture {
                            e.update(cx, |this, _| this.hide(HideReason::Dismiss));
                            window.refresh();
                        }
                    });
                    let e = entity.clone();
                    window.on_mouse_event(move |_: &ScrollWheelEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture
                            && e.read(cx).core.target.or(e.read(cx).core.pending).is_some()
                        {
                            e.update(cx, |this, _| this.hide(HideReason::Interrupt));
                            window.refresh();
                        }
                    });
                    let e = entity.clone();
                    window.on_mouse_event(move |_: &MouseExitEvent, _, window, cx| {
                        e.update(cx, |this, _| this.hide(HideReason::Interrupt));
                        window.refresh();
                    });
                    let e = entity.clone();
                    window.on_key_event(move |ev: &KeyDownEvent, phase, window, cx| {
                        let s = e.read(cx).core;
                        if phase == DispatchPhase::Capture
                            && ev.keystroke.key == "escape"
                            && (s.visible || s.target.is_some() || s.pending.is_some())
                        {
                            e.update(cx, |this, _| this.hide(HideReason::Dismiss));
                            window.refresh();
                        }
                    });
                },
            )
            .size_full(),
        )
        .with_priority(usize::MAX - 1);

        let mut root = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(collector);
        if let (Some(c), true) = (self.shown.clone(), self.core.visible) {
            root = root.child(self.tip(c, window, cx));
        }
        root
    }
}

impl TitleTooltip {
    fn render_key(&self) -> (Option<u64>, bool, Option<SharedString>) {
        (
            self.core.target,
            self.core.visible,
            self.shown.as_ref().map(|c| c.text.clone()),
        )
    }

    fn tip(&self, c: Candidate, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx);
        let t = theme.tokens;
        let vp = window.viewport_size();
        let (vw, vh) = (f32::from(vp.width), f32::from(vp.height));
        // `.ui-tip`: max-width min(280px, 100vw - 16px), padding 5px 9px, 1px border.
        let max_w = 280f32.min(vw - 16.);
        let body = || {
            Text::from_spans(vec![crate::text::span(c.text.clone())])
                .ui()
                .size(12.)
                .weight(400.)
                .leading(1.4)
                .tracking(0.01)
                .color(t.text)
                .pre_wrap()
                .break_anywhere()
                .line_clamp(12)
        };
        let inner_max = max_w - 18. - 2.;
        let (w0, _, _) = body().measure(None, window);
        let inner_w = w0.min(inner_max);
        let (_, h, _) = body().measure(Some(inner_w), window);
        let tip = (inner_w + 20., h + 12.);
        let b = c.bounds;
        let target = TipRect {
            left: b.left().into(),
            top: b.top().into(),
            right: b.right().into(),
            bottom: b.bottom().into(),
        };
        let place = place_tooltip(target, tip, (vw, vh), self.cursor.map(f32::from));
        // `ui-tip-in` (.09s ease-out): fade in from 3px below (or above when flipped).
        let p = self.shown_at.map_or(1., |s| {
            motion::eased(s, Duration::from_millis(90), Easing::EASE_OUT, cx)
        });
        if p < 1. {
            window.request_animation_frame();
        }
        let shift = 3.
            * (1. - p)
            * if place.placement == TipPlacement::Above {
                -1.
            } else {
                1.
            };
        let ring = t.amber.fade(if theme.is_light() { 0.16 } else { 0.12 });
        let shadow = if theme.is_light() {
            css_box_shadow(0., 10., 24., 0., crate::color::rgba(0, 0, 0, 0.12))
        } else {
            css_box_shadow(0., 10., 28., 0., crate::color::rgba(0, 0, 0, 0.42))
        };
        let card = crate::style::card(t.pane, t.line_bright, 7.)
            .w(px(tip.0))
            .py(px(5.))
            .px(px(9.))
            .child(div().w(relative(1.)).child(body()));
        let shadows = [shadow, css_box_shadow(0., 0., 0., 1., ring)];
        deferred(crate::style::anchored_at(
            place.x,
            place.y + shift,
            with_shadow(card, &shadows, 7.).opacity(p),
        ))
        .with_priority(usize::MAX)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u8 = 1;
    const B: u8 = 2;

    fn rect(l: f32, t: f32, r: f32, b: f32) -> TipRect {
        TipRect {
            left: l,
            top: t,
            right: r,
            bottom: b,
        }
    }

    #[test]
    fn places_like_place_tooltip() {
        let p = place_tooltip(rect(10., 10., 50., 30.), (80., 24.), (200., 200.), None);
        assert_eq!((p.placement, p.x, p.y), (TipPlacement::Below, 8., 36.));
        let p = place_tooltip(rect(80., 160., 120., 190.), (60., 24.), (200., 200.), None);
        assert_eq!((p.placement, p.y), (TipPlacement::Above, 160. - 6. - 24.));
        let p = place_tooltip(rect(0., 0., 40., 20.), (80., 400.), (200., 200.), None);
        assert_eq!((p.placement, p.y), (TipPlacement::Below, 8.));
        let p = place_tooltip(
            rect(0., 10., 400., 30.),
            (80., 20.),
            (400., 200.),
            Some(200.),
        );
        assert_eq!(p.x, 160.);
        let p = place_tooltip(
            rect(100., 10., 124., 34.),
            (80., 20.),
            (400., 200.),
            Some(200.),
        );
        assert_eq!(p.x, 112. - 40.);
    }

    #[test]
    fn show_delay() {
        assert_eq!(show_delay_ms(1000, 0, false), SHOW_DELAY_MS);
        assert_eq!(show_delay_ms(1000, 0, true), 0);
        assert_eq!(show_delay_ms(1000, 1000 - RESHOW_WINDOW_MS + 1, false), 0);
        assert_eq!(
            show_delay_ms(1000, 1000 - RESHOW_WINDOW_MS, false),
            SHOW_DELAY_MS
        );
        assert_eq!(
            show_delay_ms(1000, 1000 - RESHOW_WINDOW_MS - 1, false),
            SHOW_DELAY_MS
        );
    }

    #[test]
    fn tip_machine() {
        let idle = TipCore::<u8>::default();
        let (first, act) = apply_consider(idle, Some(A), 1000);
        assert_eq!(act, TipAction::Schedule(A, SHOW_DELAY_MS));
        assert_eq!(first.pending, Some(A));
        let (again, act) = apply_consider(first, Some(A), 1016);
        assert_eq!((act, again.pending), (TipAction::Noop, Some(A)));

        let (left, act) = apply_consider(first, None, 1080);
        assert_eq!(act, TipAction::Hide(HideReason::Leave));
        assert_eq!((left.pending, left.last_hide_at), (None, 1080));
        assert_eq!(apply_timer(left).1, TipAction::Noop);

        let (fired, act) = apply_timer(first);
        assert_eq!((act, fired.pending), (TipAction::Reveal(A), None));

        let down = apply_hide(first, HideReason::Dismiss, 1100);
        assert_eq!((down.dismissed, down.pending), (Some(A), None));
        let drift = apply_hide(down, HideReason::Dismiss, 1110);
        assert_eq!(drift.dismissed, Some(A));
        assert_eq!(apply_consider(drift, Some(A), 1120).1, TipAction::Noop);

        let shown = TipCore {
            target: Some(A),
            visible: true,
            ..idle
        };
        let pressed = apply_hide(shown, HideReason::Dismiss, 2000);
        let moved = apply_hide(pressed, HideReason::Dismiss, 2010);
        assert_eq!(moved.dismissed, Some(A));
        assert_eq!(apply_consider(moved, Some(A), 2020).1, TipAction::Noop);

        let scrolled = apply_hide(shown, HideReason::Interrupt, 3000);
        assert_eq!(scrolled.last_hide_at, 0);
        assert_eq!(
            apply_consider(scrolled, Some(A), 3010).1,
            TipAction::Schedule(A, SHOW_DELAY_MS)
        );

        let left = apply_hide(shown, HideReason::Leave, 4000);
        assert_eq!(left.last_hide_at, 4000);
        assert_eq!(apply_consider(left, Some(B), 4100).1, TipAction::Reveal(B));

        let esc = apply_hide(shown, HideReason::Dismiss, 5000);
        let (left, _) = apply_consider(esc, None, 5100);
        assert_eq!(left.dismissed, None);
        assert!(matches!(
            apply_consider(left, Some(A), 5600).1,
            TipAction::Schedule(..)
        ));
    }

    #[test]
    fn reshow_after_dismiss() {
        assert!(should_reshow_after_dismiss(
            "Remove",
            Some("Confirm remove")
        ));
        assert!(should_reshow_after_dismiss(
            "Confirm remove",
            Some("Confirm remove — uncommitted files")
        ));
        assert!(!should_reshow_after_dismiss("Remove", Some("Remove")));
        assert!(!should_reshow_after_dismiss("Remove", None));
        assert!(!should_reshow_after_dismiss("Remove", Some("")));
    }
}
