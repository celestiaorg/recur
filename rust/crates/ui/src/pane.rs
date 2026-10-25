//! Cached sub-views, so a redraw that passes through a view does not render or lay out its
//! regions that did not change (see `rust/docs/ARCHITECTURE.md`, "Frame cost").
//!
//! GPUI renders every view that is not behind a cached boundary on every frame, and a notify
//! marks the notified view's ancestors dirty: a key typed into the chat composer re-renders the
//! shell, the workspace and the Observatory on its way down. A [`Pane`] is one region of such a
//! view (the titlebar, the explorer) as its own cached view: it redraws when its owner notifies,
//! and is reused when the owner merely re-renders as an ancestor of something else. A [`Layer`]
//! does the same for a child view the owner mounts (an overlay, a toast host).
//!
//! Do not wrap a region holding a child that notifies often (an editor, a scrolling review): the
//! child's notify would dirty the pane, which then re-renders with every cache beneath it off.

use gpui_kit::{
    AnyElement, AnyView, AppContext as _, Context, Entity, IntoElement, Render, Subscription,
    WeakEntity, Window,
};

use crate::guard::Guarded;

/// A region `body` of `owner`, rendered as its own cached view.
pub struct Pane<V: 'static> {
    owner: WeakEntity<V>,
    body: fn(&mut V, &mut Window, &mut Context<V>) -> AnyElement,
    _sub: Subscription,
}

impl<V: 'static> Pane<V> {
    pub fn new(
        owner: &Entity<V>,
        body: fn(&mut V, &mut Window, &mut Context<V>) -> AnyElement,
        cx: &mut Context<Self>,
    ) -> Self {
        Pane {
            owner: owner.downgrade(),
            body,
            _sub: cx.guarded_observe(owner, |_, _, cx| cx.notify()),
        }
    }

    /// A pane for `owner`'s region `body`, from `owner`'s own context.
    pub fn of(
        owner: &mut Context<V>,
        body: fn(&mut V, &mut Window, &mut Context<V>) -> AnyElement,
    ) -> Entity<Self> {
        let me = owner.entity();
        owner.new(|cx| Pane::new(&me, body, cx))
    }

    fn render_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let body = self.body;
        match self.owner.upgrade() {
            Some(owner) => crate::style::cached_root(owner.update(cx, |o, cx| body(o, window, cx)))
                .into_any_element(),
            None => gpui_kit::div().into_any_element(),
        }
    }
}

impl<V: 'static> Render for Pane<V> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::guard::view(self, window, cx, Self::render_view)
    }
}

/// A child view behind its own cached boundary.
pub struct Layer {
    view: AnyView,
    center_y: bool,
}

impl Layer {
    pub fn new(view: impl Into<AnyView>, cx: &mut gpui_kit::App) -> Entity<Layer> {
        let view = view.into();
        cx.new(|_| Layer {
            view,
            center_y: false,
        })
    }

    /// The view centered on the layer's height, as `items_center` places it in a row taller
    /// than it.
    pub fn centered(view: impl Into<AnyView>, cx: &mut gpui_kit::App) -> Entity<Layer> {
        let view = view.into();
        cx.new(|_| Layer {
            view,
            center_y: true,
        })
    }

    fn render_view(&mut self, _: &mut Window, _: &mut Context<Self>) -> AnyElement {
        let view = self.view.clone();
        if self.center_y {
            use gpui_kit::{ParentElement as _, Styled as _};
            crate::style::cached_root(
                gpui_kit::div()
                    .size_full()
                    .flex()
                    .items_center()
                    .child(view),
            )
            .into_any_element()
        } else {
            crate::style::cached_root(view).into_any_element()
        }
    }
}

impl Render for Layer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::guard::view(self, window, cx, Self::render_view)
    }
}
