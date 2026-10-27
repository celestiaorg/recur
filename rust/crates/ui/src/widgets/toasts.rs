//! `Toasts.svelte` and the toast store in `lib/stores.ts`: transient notices stacked at the
//! bottom right, with optional actions, replace-in-place keys, auto-dismiss TTLs, and the
//! richer ask_user notice with its draining TTL bar.

use crate::fonts::weight;
use crate::guard::Guarded;
use crate::icon::icon;
use crate::icon_name::IconName;
use crate::motion::{self, Easing};
use crate::shadow::{css_box_shadow, with_shadow};
use crate::text::{LineHeight, Text, TextStyled, text};
use crate::theme::Theme;
use gpui_kit::{
    AnyElement, App, AppContext, ClickEvent, Context, CursorStyle, Entity, Global, Hsla,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Task, Window, div, prelude::FluentBuilder, px, relative,
};
use std::rc::Rc;
use std::time::{Duration, Instant};

pub const TOAST_TTL: Duration = Duration::from_millis(3500);
pub const ERROR_TOAST_TTL: Duration = Duration::from_millis(8000);
pub const ASK_TOAST_TTL: Duration = Duration::from_millis(30_000);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Error,
    Info,
}

/// The structured ask_user notice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToastAsk {
    pub project: String,
    pub workspace: String,
    pub question: String,
}

pub type ToastFn = Rc<dyn Fn(&mut Window, &mut App)>;

/// A toast's action button: label and handler.
#[derive(Clone)]
pub struct ToastAction {
    pub label: SharedString,
    pub run: ToastFn,
}

pub fn toast_action(
    label: impl Into<SharedString>,
    run: impl Fn(&mut Window, &mut App) + 'static,
) -> ToastAction {
    ToastAction {
        label: label.into(),
        run: Rc::new(run),
    }
}

#[derive(Clone)]
pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub text: SharedString,
    pub action: Option<ToastAction>,
    pub ask: Option<ToastAsk>,
    /// Auto-dismiss duration; zero is sticky.
    pub ttl: Duration,
    /// A later toast with the same key replaces this one in place.
    pub replace: Option<SharedString>,
    /// Called when dismissed, except after its action runs.
    pub on_dismiss: Option<ToastFn>,
    pub shown_at: Instant,
}

/// `pushToast` options beyond kind and text.
#[derive(Clone, Default)]
pub struct ToastOptions {
    pub action: Option<ToastAction>,
    pub ttl: Option<Duration>,
    pub replace: Option<SharedString>,
    pub on_dismiss: Option<ToastFn>,
}

/// The toast list (one per app). Read it with [`ToastStore::global`].
#[derive(Default)]
pub struct ToastStore {
    toasts: Vec<Toast>,
    seq: u64,
    timers: Vec<(u64, Task<()>)>,
}

struct GlobalToasts(Entity<ToastStore>);

impl Global for GlobalToasts {}

impl ToastStore {
    pub fn global(cx: &mut App) -> Entity<ToastStore> {
        if !cx.has_global::<GlobalToasts>() {
            let store = cx.new(|_| ToastStore::default());
            cx.set_global(GlobalToasts(store));
        }
        cx.global::<GlobalToasts>().0.clone()
    }

    pub fn toasts(&self) -> &[Toast] {
        &self.toasts
    }

    fn arm(&mut self, id: u64, ttl: Duration, cx: &mut Context<Self>) {
        self.timers.retain(|(t, _)| *t != id);
        if ttl.is_zero() {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ttl).await;
            let _ = this.update(cx, |this, cx| this.dismiss_inner(id, true, cx));
        });
        self.timers.push((id, task));
    }

    /// `pushToast`.
    pub fn push(
        &mut self,
        kind: ToastKind,
        text: impl Into<SharedString>,
        opts: ToastOptions,
        cx: &mut Context<Self>,
    ) {
        let text = text.into();
        let ttl = opts.ttl.unwrap_or(if kind == ToastKind::Error {
            ERROR_TOAST_TTL
        } else {
            TOAST_TTL
        });
        if let Some(key) = &opts.replace
            && let Some(existing) = self
                .toasts
                .iter_mut()
                .find(|t| t.replace.as_ref() == Some(key))
        {
            existing.kind = kind;
            existing.text = text;
            existing.action = opts.action;
            existing.ttl = ttl;
            existing.on_dismiss = opts.on_dismiss;
            let id = existing.id;
            self.arm(id, ttl, cx);
            cx.notify();
            return;
        }
        self.seq += 1;
        let id = self.seq;
        self.toasts.push(Toast {
            id,
            kind,
            text,
            action: opts.action,
            ask: None,
            ttl,
            replace: opts.replace,
            on_dismiss: opts.on_dismiss,
            shown_at: crate::motion::now(cx),
        });
        self.arm(id, ttl, cx);
        cx.notify();
    }

    /// `toastEditorTheme`: the theme-cycle notice, replaced while still visible.
    pub fn editor_theme(&mut self, name: &str, cx: &mut Context<Self>) {
        self.push(
            ToastKind::Success,
            format!("Editor theme: {name}"),
            ToastOptions {
                replace: Some("editor-theme".into()),
                ..Default::default()
            },
            cx,
        );
    }

    /// `pushAskToast`: the compact off-tab ask_user notice.
    pub fn push_ask(
        &mut self,
        ask: ToastAsk,
        action: ToastAction,
        ttl: Option<Duration>,
        cx: &mut Context<Self>,
    ) {
        let ttl = ttl.unwrap_or(ASK_TOAST_TTL);
        self.seq += 1;
        let id = self.seq;
        let where_ = [ask.project.as_str(), ask.workspace.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let text = if ask.question.is_empty() {
            format!("Agent has a question · {where_}")
        } else {
            format!("Agent has a question · {where_} — {}", ask.question)
        };
        self.toasts.push(Toast {
            id,
            kind: ToastKind::Info,
            text: text.into(),
            action: Some(action),
            ask: Some(ask),
            ttl,
            replace: None,
            on_dismiss: None,
            shown_at: crate::motion::now(cx),
        });
        self.arm(id, ttl, cx);
        cx.notify();
    }

    fn dismiss_inner(
        &mut self,
        id: u64,
        invoke_on_dismiss: bool,
        cx: &mut Context<Self>,
    ) -> Option<ToastFn> {
        self.timers.retain(|(t, _)| *t != id);
        let gone = self
            .toasts
            .iter()
            .position(|t| t.id == id)
            .map(|i| self.toasts.remove(i));
        cx.notify();
        gone.and_then(|t| t.on_dismiss)
            .filter(|_| invoke_on_dismiss)
    }

    /// `dismissToast`. The toast's `on_dismiss` runs unless `invoke_on_dismiss` is false.
    pub fn dismiss(
        store: &Entity<ToastStore>,
        id: u64,
        invoke_on_dismiss: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let cb = store.update(cx, |s, cx| s.dismiss_inner(id, invoke_on_dismiss, cx));
        if let Some(cb) = cb {
            cb(window, cx);
        }
    }
}

/// Push a toast onto the app's store.
pub fn push_toast(
    kind: ToastKind,
    text: impl Into<SharedString>,
    opts: ToastOptions,
    cx: &mut App,
) {
    let store = ToastStore::global(cx);
    let text = text.into();
    store.update(cx, |s, cx| s.push(kind, text, opts, cx));
}

/// The toast stack. Render one per window, after the main content.
pub struct Toasts {
    store: Entity<ToastStore>,
}

impl Toasts {
    pub fn new(cx: &mut Context<Self>) -> Toasts {
        let store = ToastStore::global(cx);
        cx.guarded_observe(&store, |_, _, cx| cx.notify()).detach();
        Toasts { store }
    }
}

fn activate(store: &Entity<ToastStore>, t: &Toast, window: &mut Window, cx: &mut App) {
    let Some(action) = t.action.clone() else {
        return;
    };
    ToastStore::dismiss(store, t.id, false, window, cx);
    (action.run)(window, cx);
}

impl Toasts {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let vp = window.viewport_size();
        let (vw, vh) = (f32::from(vp.width), f32::from(vp.height));
        let toasts: Vec<Toast> = self.store.read(cx).toasts.clone();
        let items: Vec<AnyElement> = toasts
            .iter()
            .map(|t| {
                if t.ask.is_some() {
                    ask_toast(&self.store, t, vh, window, cx)
                } else {
                    plain_toast(&self.store, t, vh, window, cx)
                }
            })
            .collect();
        // `.toasts`: fixed bottom right, column gap 8, width min(340px, 100vw - 32px).
        div()
            .id("toasts")
            .absolute()
            .right(px(16.))
            .bottom(px(16.))
            .w(px(340f32.min(vw - 32.)))
            .max_h(px(vh - 32.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(items)
    }
}

impl Render for Toasts {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::guard::view(self, window, cx, Self::render_view)
    }
}

/// `toastin` (.16s ease-out, or .2s settle for ask): fade up from 8px below. CSS also scales
/// from .98, which GPUI cannot apply to an element tree (`ui-parity.md`).
fn entrance(
    t: &Toast,
    duration: Duration,
    easing: Easing,
    window: &mut Window,
    cx: &App,
) -> (f32, f32) {
    let p = motion::eased(t.shown_at, duration, easing, cx);
    if p < 1. {
        window.request_animation_frame();
    }
    (p, 8. * (1. - p))
}

fn close_button(
    store: &Entity<ToastStore>,
    id: u64,
    ask: bool,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Theme::of(cx).tokens;
    let store = store.clone();
    // `.tclose:hover` brightens the glyph; the ask toast's also gets a wash.
    let hover_bg = ask.then(|| t.text.fade(0.08));
    let b = div()
        .id(("tclose", id))
        .debug_selector(|| "tclose".into())
        .flex_none()
        .text_color(t.faint)
        .cursor_pointer()
        .hover(move |s| {
            let s = s.text_color(Hsla::from(t.text));
            match hover_bg {
                Some(bg) => s.bg(bg),
                None => s,
            }
        })
        .on_click(move |_: &ClickEvent, window, cx| {
            ToastStore::dismiss(&store, id, true, window, cx)
        })
        .child(text("✕").size(11.).line_height(LineHeight::Normal).nowrap());
    if ask {
        b.absolute()
            .top(px(8.))
            .right(px(8.))
            .py(px(3.))
            .px(px(5.))
            .rounded(px(5.))
    } else {
        b.p(px(2.)).mt(px(1.))
    }
}

/// Tokens with every color's alpha scaled by `p` (an entrance fade).
fn faded_tokens(mut t: crate::theme::Tokens, p: f32) -> crate::theme::Tokens {
    for c in [
        &mut t.text,
        &mut t.muted,
        &mut t.faint,
        &mut t.amber,
        &mut t.pane,
        &mut t.line_bright,
    ] {
        *c = c.with_alpha(c.a * p);
    }
    t
}

fn plain_toast(
    store: &Entity<ToastStore>,
    t: &Toast,
    vh: f32,
    window: &mut Window,
    cx: &App,
) -> AnyElement {
    let k = Theme::of(cx).tokens;
    let (p, dy) = entrance(t, Duration::from_millis(160), Easing::EASE_OUT, window, cx);
    let dot = match t.kind {
        ToastKind::Success => k.emerald,
        ToastKind::Error => k.coral,
        ToastKind::Info => k.cyan,
    };
    let border = if t.kind == ToastKind::Error {
        crate::color::rgba(235, 122, 92, 0.4)
    } else {
        k.line_bright
    };
    let dot_el = div()
        .flex_none()
        .size(px(7.))
        .rounded_full()
        .bg(dot)
        .mt(px(5.));
    let body = div()
        .id(("tx", t.id))
        .flex_1()
        .min_w_0()
        .max_h(px(vh * 0.4))
        .overflow_y_scroll()
        .child(
            text(t.text.clone())
                .color(k.text)
                .opacity(p)
                .leading(1.4)
                .break_anywhere(),
        );
    let actionable = t.action.is_some();
    let hover_border = k.cyan.mix(0.45, k.line_bright);
    let mut toast = div()
        .id(("toast", t.id))
        .flex()
        .items_start()
        .gap(px(9.))
        .min_w_0()
        .max_w_full()
        .relative()
        .child(crate::style::inset_background(
            k.pane.fade(p),
            9.,
            1.,
            border,
        ))
        .border(px(1.))
        .border_color(Hsla::from(border.fade(p)))
        .rounded(px(9.))
        .py(px(10.))
        .px(px(12.))
        .text_size(px(12.5))
        .when(actionable, |d| {
            d.hover(move |s| s.border_color(Hsla::from(hover_border)))
        });
    toast = if let Some(action) = &t.action {
        let store = store.clone();
        let tt = t.clone();
        let label = action.label.clone();
        toast.child(
            div()
                .id(("toast-go", t.id))
                .group(SharedString::from(format!("toast-go-{}", t.id)))
                .flex()
                .items_start()
                .gap(px(9.))
                .flex_1()
                .min_w_0()
                .cursor_pointer()
                .on_click(move |_: &ClickEvent, window, cx| activate(&store, &tt, window, cx))
                .child(dot_el)
                .child(body)
                .child(
                    div().flex_none().mt(px(1.)).child(
                        text(label)
                            .mono()
                            .size(11.)
                            .color(k.amber)
                            .opacity(p)
                            .nowrap(),
                    ),
                ),
        )
    } else {
        toast.child(dot_el).child(body)
    };
    let shadow = css_box_shadow(0., 10., 30., -8., crate::color::rgba(0, 0, 0, 0.6 * p));
    with_shadow(
        toast.child(close_button(store, t.id, false, cx)),
        &[shadow],
        9.,
    )
    .top(px(dy))
    .into_any_element()
}

fn ask_toast(
    store: &Entity<ToastStore>,
    t: &Toast,
    _vh: f32,
    window: &mut Window,
    cx: &App,
) -> AnyElement {
    let Some(ask) = t.ask.clone() else {
        return div().into_any_element();
    };
    let (p, dy) = entrance(t, Duration::from_millis(200), Easing::SETTLE, window, cx);
    // The entrance fades every color of the card together.
    let k = faded_tokens(Theme::of(cx).tokens, p);
    let drain = 1. - motion::progress(t.shown_at, t.ttl, cx);
    if drain > 0. && !motion::is_frozen(cx) {
        window.request_animation_frame();
    }
    let head = div()
        .flex()
        .items_center()
        .gap(px(7.))
        .min_w_0()
        .child(icon(IconName::MessageQuestion).size(px(14.)).color(k.amber))
        .child(
            text("Question for you")
                .mono()
                .size(10.)
                .weight(weight::SEMIBOLD)
                .tracking(0.08)
                .uppercase()
                .color(k.muted)
                .nowrap(),
        )
        .when_some(t.action.as_ref(), |d, action| {
            // `.toast-ask-cta`: an inline-flex of the label and its chevron.
            let cta = |t: Text| t.mono().weight(weight::SEMIBOLD).nowrap();
            d.child(
                div()
                    .ml_auto()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(cta(text(action.label.clone())).size(10.5).color(k.amber))
                    // `.toast-ask-cta-chev`: 13px, line-height 1, .75 opacity, up half a pixel.
                    .child(
                        div().relative().top(px(-0.5)).child(
                            cta(text("›"))
                                .size(13.)
                                .line_height(LineHeight::Scale(1.))
                                .color(k.amber.fade(0.75)),
                        ),
                    ),
            )
        });
    let where_ = div()
        .flex()
        .items_baseline()
        .gap(px(5.))
        .min_w_0()
        .pl(px(1.))
        .font_family(crate::fonts::MONO)
        .text_size(px(11.))
        .text_color(k.muted)
        .child(
            div()
                .flex_initial()
                .min_w_0()
                .max_w(relative(0.45))
                .child(text(ask.project.clone()).color(k.faint).truncate()),
        )
        .child(text("/").color(k.faint.fade(0.7)).nowrap())
        .child(
            div().flex_1().min_w_0().child(
                text(ask.workspace.clone())
                    .color(k.text)
                    .weight(weight::MEDIUM_PLUS)
                    .truncate(),
            ),
        );
    let mut go = div()
        .id(("toast-ask-go", t.id))
        .flex()
        .flex_col()
        .gap(px(6.))
        .flex_1()
        .min_w_0()
        .pt(px(11.))
        .pr(px(30.))
        .pb(px(12.))
        .pl(px(14.))
        .cursor_pointer()
        .child(head)
        .child(where_);
    if !ask.question.is_empty() {
        go = go.child(
            div().pl(px(1.)).child(
                text(ask.question.clone())
                    .size(12.5)
                    .weight(weight::MEDIUM_PLUS)
                    .tracking(-0.01)
                    .leading(1.35)
                    .color(k.text)
                    .line_clamp(2),
            ),
        );
    }
    {
        let store = store.clone();
        let tt = t.clone();
        go = go.on_click(move |_: &ClickEvent, window, cx| activate(&store, &tt, window, cx));
    }
    let border = k.amber.mix(0.32, k.line_bright);
    let hover_border = k.amber.mix(0.48, k.line_bright);
    let tint = k.amber.mix(0.09, k.pane);
    let card = div()
        .id(("toast", t.id))
        .relative()
        .flex()
        .items_start()
        .min_w_0()
        .max_w_full()
        .overflow_hidden()
        .rounded(px(11.))
        .border(px(1.))
        .border_color(Hsla::from(border))
        .hover(move |s| s.border_color(Hsla::from(hover_border)))
        .text_size(px(12.5))
        .child(crate::style::css_gradient(
            165.,
            (tint, 0.),
            (k.pane, 0.42),
            11.,
            1.,
            border,
        ))
        // `::before`, the 3px amber accent rail, and `.toast-ask-ttl`, the bar that drains from
        // the right over the TTL (`scaleX` of a `left: 3px; right: 0` box): both clipped to the
        // card's rounded padding box, as its `overflow: hidden` clips them.
        .child({
            let (top, bottom, ttl) = (k.amber, k.amber.fade(0.3), k.amber.fade(0.55));
            gpui_kit::canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let clip = |rect: gpui_kit::Bounds<gpui_kit::Pixels>| {
                        Some(gpui_kit::ContentMask { bounds: rect })
                    };
                    let rail = gpui_kit::Bounds {
                        origin: bounds.origin,
                        size: gpui_kit::size(px(3.), bounds.size.height),
                    };
                    window.with_content_mask(clip(rail), |w| {
                        crate::style::paint_css_gradient(
                            w,
                            bounds,
                            10.,
                            180.,
                            (top, 0.),
                            (bottom, 1.),
                        )
                    });
                    if drain > 0. {
                        let bar = gpui_kit::Bounds {
                            origin: gpui_kit::point(
                                bounds.origin.x + px(3.),
                                bounds.bottom() - px(2.),
                            ),
                            size: gpui_kit::size((bounds.size.width - px(3.)) * drain, px(2.)),
                        };
                        window.with_content_mask(clip(bar), |w| {
                            w.paint_quad(gpui_kit::quad(
                                bounds,
                                px(10.),
                                Hsla::from(ttl),
                                px(0.),
                                Hsla::transparent_black(),
                                gpui_kit::BorderStyle::Solid,
                            ))
                        });
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        })
        .child(go)
        .child(close_button(store, t.id, true, cx))
        .cursor(CursorStyle::PointingHand);
    let shadows = [
        css_box_shadow(0., 14., 36., -14., crate::color::rgba(0, 0, 0, 0.7 * p)),
        css_box_shadow(0., 0., 0., 1., k.amber.fade(0.12)),
    ];
    with_shadow(card, &shadows, 11.)
        .top(px(dy))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    fn push_replace_and_dismiss(cx: &mut gpui_kit::TestAppContext) {
        let store = cx.new(|_| ToastStore::default());
        store.update(cx, |s, cx| {
            s.push(ToastKind::Success, "one", ToastOptions::default(), cx);
            s.editor_theme("Recur", cx);
            s.editor_theme("Paper", cx);
            s.push(ToastKind::Error, "boom", ToastOptions::default(), cx);
        });
        store.read_with(cx, |s, _| {
            let texts: Vec<&str> = s.toasts().iter().map(|t| t.text.as_ref()).collect();
            assert_eq!(texts, vec!["one", "Editor theme: Paper", "boom"]);
            assert_eq!(s.toasts()[2].ttl, ERROR_TOAST_TTL);
            assert_eq!(s.toasts()[0].ttl, TOAST_TTL);
        });
        // Success toasts go after 3.5s; errors linger to 8s.
        cx.executor().advance_clock(Duration::from_millis(4000));
        store.read_with(cx, |s, _| assert_eq!(s.toasts().len(), 1));
        cx.executor().advance_clock(Duration::from_millis(4500));
        store.read_with(cx, |s, _| assert!(s.toasts().is_empty()));
    }

    #[gpui_kit::test]
    fn ask_toast_text_and_sticky(cx: &mut gpui_kit::TestAppContext) {
        let store = cx.new(|_| ToastStore::default());
        store.update(cx, |s, cx| {
            let ask = ToastAsk {
                project: "recur".into(),
                workspace: "honeyed-cannoli".into(),
                question: "Ship it?".into(),
            };
            s.push_ask(ask, toast_action("Answer", |_, _| {}), None, cx);
            s.push(
                ToastKind::Info,
                "sticky",
                ToastOptions {
                    ttl: Some(Duration::ZERO),
                    ..Default::default()
                },
                cx,
            );
        });
        store.read_with(cx, |s, _| {
            assert_eq!(
                s.toasts()[0].text.as_ref(),
                "Agent has a question · recur · honeyed-cannoli — Ship it?"
            );
        });
        cx.executor().advance_clock(Duration::from_secs(60));
        store.read_with(cx, |s, _| {
            assert_eq!(s.toasts().len(), 1);
            assert_eq!(s.toasts()[0].text.as_ref(), "sticky");
        });
    }
}
