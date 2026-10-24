//! Panic containment for GPUI callbacks.
//!
//! On macOS every frame and input event reaches GPUI through an `extern "C"` AppKit callback,
//! and every task through a GCD trampoline. A panic cannot unwind through those frames: the
//! unwinder finds no handler above them and the process aborts with "failed to initiate panic"
//! (or "panic in a function that cannot unwind"). Catching above GPUI would not help either:
//! GPUI takes the window out of the app while it draws or dispatches, and an entity out of its
//! map while it is updated, so an unwind through GPUI frames drops both.
//!
//! So panics are contained inside our own callbacks, before the unwind reaches a GPUI frame:
//!
//! - [`view`] wraps a view's `render` body (every `Render` and `RenderOnce` in the app): a panic
//!   draws [`failed`] in the view's place and the rest of the window keeps working.
//! - [`catch`] wraps the pure layout work of custom elements (text shaping and line breaking,
//!   editor rows, terminal tiles), which then lay out as empty.
//! - [`Guarded`] is `cx.listener`, `cx.subscribe`, `cx.observe` and their window variants with
//!   the callback caught: the event does nothing and an error toast says so.
//!
//! The panic hook (`recur_store::applog`) logs every panic with its backtrace to `recur.log`
//! first, contained or not.

use std::any::type_name;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

use gpui_kit::{
    AnyElement, App, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Styled, Subscription, Window, div, px,
};

use crate::theme::Theme;
use crate::widgets::toasts::{ToastKind, ToastOptions, push_toast};

/// Runs `f`, returning `None` (and logging where) if it panicked.
pub fn catch<R>(site: &str, f: impl FnOnce() -> R) -> Option<R> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => Some(r),
        Err(_) => {
            tracing::error!("recur: recovered from a panic in {site}");
            None
        }
    }
}

/// A view's render body, contained: a panic renders [`failed`] instead.
pub fn view<V: 'static, E: IntoElement>(
    this: &mut V,
    window: &mut Window,
    cx: &mut Context<V>,
    body: impl FnOnce(&mut V, &mut Window, &mut Context<V>) -> E,
) -> AnyElement {
    let name = type_name::<V>();
    count_render(name);
    catch(name, || body(this, window, cx).into_any_element()).unwrap_or_else(|| failed(name, cx))
}

thread_local! {
    static RENDERS: RefCell<Option<BTreeMap<&'static str, u32>>> = const { RefCell::new(None) };
}

fn count_render(name: &'static str) {
    RENDERS.with_borrow_mut(|r| {
        if let Some(r) = r {
            *r.entry(name).or_default() += 1;
        }
    });
}

/// Starts (or stops) counting [`view`] renders by view type on this thread, for frame-cost
/// tests and the real-data smoke.
pub fn count_renders(on: bool) {
    RENDERS.with_borrow_mut(|r| *r = on.then(BTreeMap::new));
}

/// The renders counted since the last call, by view type name.
pub fn take_render_counts() -> BTreeMap<&'static str, u32> {
    RENDERS.with_borrow_mut(|r| r.as_mut().map(std::mem::take).unwrap_or_default())
}

/// A `RenderOnce` body, contained like [`view`].
pub fn component<C, E: IntoElement>(
    this: C,
    window: &mut Window,
    cx: &mut App,
    body: impl FnOnce(C, &mut Window, &mut App) -> E,
) -> AnyElement {
    let name = type_name::<C>();
    catch(name, || body(this, window, cx).into_any_element()).unwrap_or_else(|| failed(name, cx))
}

/// What a view that panicked while rendering shows in its place.
pub fn failed(name: &str, cx: &App) -> AnyElement {
    let color = cx
        .try_global::<Theme>()
        .map(|t| t.tokens.coral)
        .unwrap_or(crate::color::hex(0xf07178));
    let short = name.rsplit("::").next().unwrap_or(name);
    div()
        .debug_selector(|| "guard-failed".into())
        .p(px(12.))
        .text_size(px(12.))
        .text_color(color)
        .child(format!(
            "{short} hit an error and could not be drawn. Details are in recur.log."
        ))
        .into_any_element()
}

/// `cx.listener`, `cx.subscribe`, `cx.observe` and their window variants with the callback
/// contained: a panic drops the event and an error toast says so.
pub trait Guarded<V> {
    fn guarded_listener<E: ?Sized>(
        &self,
        f: impl Fn(&mut V, &E, &mut Window, &mut Context<V>) + 'static,
    ) -> impl Fn(&E, &mut Window, &mut App) + 'static;

    fn guarded_subscribe<T: EventEmitter<Evt>, Evt: 'static>(
        &mut self,
        entity: &Entity<T>,
        f: impl FnMut(&mut V, Entity<T>, &Evt, &mut Context<V>) + 'static,
    ) -> Subscription;

    fn guarded_subscribe_in<T: EventEmitter<Evt>, Evt: 'static>(
        &mut self,
        entity: &Entity<T>,
        window: &Window,
        f: impl FnMut(&mut V, &Entity<T>, &Evt, &mut Window, &mut Context<V>) + 'static,
    ) -> Subscription;

    fn guarded_observe<T: 'static>(
        &mut self,
        entity: &Entity<T>,
        f: impl FnMut(&mut V, Entity<T>, &mut Context<V>) + 'static,
    ) -> Subscription;

    fn guarded_observe_in<T: 'static>(
        &mut self,
        entity: &Entity<T>,
        window: &mut Window,
        f: impl FnMut(&mut V, Entity<T>, &mut Window, &mut Context<V>) + 'static,
    ) -> Subscription;
}

#[allow(clippy::disallowed_methods)]
impl<V: 'static> Guarded<V> for Context<'_, V> {
    fn guarded_listener<E: ?Sized>(
        &self,
        f: impl Fn(&mut V, &E, &mut Window, &mut Context<V>) + 'static,
    ) -> impl Fn(&E, &mut Window, &mut App) + 'static {
        self.listener(move |this: &mut V, e: &E, window, cx| {
            if catch(type_name::<V>(), || f(this, e, window, cx)).is_none() {
                cx.defer(report);
            }
        })
    }

    fn guarded_subscribe<T: EventEmitter<Evt>, Evt: 'static>(
        &mut self,
        entity: &Entity<T>,
        mut f: impl FnMut(&mut V, Entity<T>, &Evt, &mut Context<V>) + 'static,
    ) -> Subscription {
        self.subscribe(entity, move |this, e, ev, cx| {
            if catch(type_name::<V>(), || f(this, e, ev, cx)).is_none() {
                cx.defer(report);
            }
        })
    }

    fn guarded_subscribe_in<T: EventEmitter<Evt>, Evt: 'static>(
        &mut self,
        entity: &Entity<T>,
        window: &Window,
        mut f: impl FnMut(&mut V, &Entity<T>, &Evt, &mut Window, &mut Context<V>) + 'static,
    ) -> Subscription {
        self.subscribe_in(entity, window, move |this, e, ev, window, cx| {
            if catch(type_name::<V>(), || f(this, e, ev, window, cx)).is_none() {
                cx.defer(report);
            }
        })
    }

    fn guarded_observe<T: 'static>(
        &mut self,
        entity: &Entity<T>,
        mut f: impl FnMut(&mut V, Entity<T>, &mut Context<V>) + 'static,
    ) -> Subscription {
        self.observe(entity, move |this, e, cx| {
            if catch(type_name::<V>(), || f(this, e, cx)).is_none() {
                cx.defer(report);
            }
        })
    }

    fn guarded_observe_in<T: 'static>(
        &mut self,
        entity: &Entity<T>,
        window: &mut Window,
        mut f: impl FnMut(&mut V, Entity<T>, &mut Window, &mut Context<V>) + 'static,
    ) -> Subscription {
        self.observe_in(entity, window, move |this, e, window, cx| {
            if catch(type_name::<V>(), || f(this, e, window, cx)).is_none() {
                cx.defer(report);
            }
        })
    }
}

fn report(cx: &mut App) {
    push_toast(
        ToastKind::Error,
        "Something went wrong. Details are in recur.log.",
        ToastOptions {
            replace: Some("recur-guard".into()),
            ..Default::default()
        },
        cx,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{
        AppContext, ClickEvent, InteractiveElement, Render, StatefulInteractiveElement,
        TestAppContext, VisualTestContext,
    };

    struct Boom {
        explode: bool,
        clicks: usize,
    }

    impl Boom {
        fn render_view(
            &mut self,
            _: &mut Window,
            cx: &mut Context<Self>,
        ) -> impl IntoElement + use<> {
            assert!(!self.explode, "render exploded");
            div().size_full().child(
                div()
                    .id("btn")
                    .debug_selector(|| "boom-btn".into())
                    .size(px(40.))
                    .on_click(
                        cx.guarded_listener(|this: &mut Boom, _: &ClickEvent, _, _| {
                            this.clicks += 1;
                            assert!(this.clicks < 2, "second click exploded");
                        }),
                    ),
            )
        }
    }

    impl Render for Boom {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            crate::guard::view(self, window, cx, Self::render_view)
        }
    }

    #[gpui_kit::test]
    fn a_panicking_render_draws_the_fallback_and_recovers(cx: &mut TestAppContext) {
        let (boom, cx) = cx.add_window_view(|_, _| Boom {
            explode: true,
            clicks: 0,
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("guard-failed").is_some());
        assert!(cx.debug_bounds("boom-btn").is_none());

        boom.update(cx, |b, cx| {
            b.explode = false;
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("guard-failed").is_none());
        assert!(cx.debug_bounds("boom-btn").is_some());
    }

    #[gpui_kit::test]
    fn a_panicking_listener_keeps_the_view_and_toasts(cx: &mut TestAppContext) {
        let (boom, cx): (_, &mut VisualTestContext) = cx.add_window_view(|_, _| Boom {
            explode: false,
            clicks: 0,
        });
        cx.run_until_parked();
        let at = cx.debug_bounds("boom-btn").unwrap().center();
        cx.simulate_click(at, Default::default());
        cx.simulate_click(at, Default::default());
        cx.run_until_parked();
        // The entity survived the panic with the state it had reached.
        assert_eq!(boom.read_with(cx, |b, _| b.clicks), 2);
        let toasts = cx.update(|_, cx| {
            let store = crate::widgets::toasts::ToastStore::global(cx);
            store.read(cx).toasts().len()
        });
        assert_eq!(toasts, 1);
        cx.simulate_click(at, Default::default());
        assert_eq!(boom.read_with(cx, |b, _| b.clicks), 3);
    }

    struct Emitter;
    impl EventEmitter<u32> for Emitter {}

    struct Listener {
        seen: Vec<u32>,
        _sub: Option<Subscription>,
    }

    #[gpui_kit::test]
    fn a_panicking_subscription_keeps_the_subscriber_and_toasts(cx: &mut TestAppContext) {
        let emitter = cx.new(|_| Emitter);
        let listener = cx.new(|cx: &mut Context<Listener>| {
            let sub = cx.guarded_subscribe(&emitter, |this: &mut Listener, _, n: &u32, _| {
                this.seen.push(*n);
                assert!(*n != 2, "event 2 exploded");
            });
            Listener {
                seen: Vec::new(),
                _sub: Some(sub),
            }
        });
        for n in 1..=3 {
            emitter.update(cx, |_, cx| cx.emit(n));
        }
        cx.run_until_parked();
        assert_eq!(listener.read_with(cx, |l, _| l.seen.clone()), [1, 2, 3]);
        let toasts = cx.update(|cx| {
            let store = crate::widgets::toasts::ToastStore::global(cx);
            store.read(cx).toasts().len()
        });
        assert_eq!(toasts, 1);
    }

    #[test]
    fn catch_returns_none_on_panic() {
        assert_eq!(catch("t", || 1), Some(1));
        assert_eq!(catch("t", || -> i32 { panic!("x") }), None);
    }
}
