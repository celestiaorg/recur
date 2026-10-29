//! Change topics: which part of [`AppState`] a change touched, so it reaches only the views
//! that read that part.
//!
//! A plain `cx.notify()` on the store wakes every observer. The setters that run often
//! (backend events while agents stream, PR sweeps, terminal busy/idle, usage polls, the menu
//! state views push, a panel drag) announce their topic with [`AppState::changed`] instead,
//! and a view observes the store with [`ObserveState::observe_state`] naming the topics its
//! render reads. A view observing [`Topics::ALL`] behaves as with a plain observer; an untagged
//! notify still reaches every observer, so a setter that is not tagged stays correct.

use std::cell::RefCell;
use std::ops::{BitOr, BitOrAssign};
use std::rc::Rc;

use gpui_kit::{App, Context, Entity, EventEmitter, Subscription, Window};
use recur_ui::guard::Guarded;

use crate::state::AppState;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Topics(u32);

impl Topics {
    pub const NONE: Topics = Topics(0);
    /// Workspace rows (`workspaces`, their live diff stats).
    pub const WORKSPACES: Topics = Topics(1 << 0);
    /// PR rows (`prs`).
    pub const PRS: Topics = Topics(1 << 1);
    /// `attention`.
    pub const ATTENTION: Topics = Topics(1 << 2);
    /// Live agents (`workspaces_running`, `agents_running`).
    pub const AGENTS: Topics = Topics(1 << 3);
    /// `inbox`.
    pub const INBOX: Topics = Topics(1 << 4);
    /// `briefing`.
    pub const BRIEFING: Topics = Topics(1 << 5);
    /// `provider_usage`.
    pub const USAGE: Topics = Topics(1 << 6);
    /// `resource`.
    pub const RESOURCE: Topics = Topics(1 << 7);
    /// The run store (`run`: sessions, busy terminals).
    pub const RUN: Topics = Topics(1 << 8);
    /// `menu_state`.
    pub const MENU: Topics = Topics(1 << 9);
    /// `panels` while a divider drags.
    pub const PANELS: Topics = Topics(1 << 10);
    pub const ALL: Topics = Topics(u32::MAX);

    pub const fn union(self, other: Topics) -> Topics {
        Topics(self.0 | other.0)
    }

    pub const fn without(self, other: Topics) -> Topics {
        Topics(self.0 & !other.0)
    }

    pub const fn intersects(self, other: Topics) -> bool {
        self.0 & other.0 != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for Topics {
    type Output = Topics;
    fn bitor(self, rhs: Topics) -> Topics {
        self.union(rhs)
    }
}

impl BitOrAssign for Topics {
    fn bitor_assign(&mut self, rhs: Topics) {
        *self = self.union(rhs);
    }
}

/// What [`AppState::changed`] announces: the topics changed since the last announcement.
#[derive(Clone, Copy, Debug)]
pub struct StateChanged(pub Topics);

impl EventEmitter<StateChanged> for AppState {}

impl AppState {
    /// Announce a change to `topics` instead of notifying every observer. Changes in one
    /// effect cycle go out as one [`StateChanged`] (a sweep touching every PR row is one
    /// announcement).
    pub fn changed(&mut self, topics: Topics, cx: &mut Context<Self>) {
        if self.pending_topics.is_empty() {
            let me = cx.entity().downgrade();
            cx.defer(move |cx| {
                let _ = me.update(cx, |s, cx| {
                    let t = std::mem::take(&mut s.pending_topics);
                    if !t.is_empty() {
                        cx.emit(StateChanged(t));
                    }
                });
            });
        }
        self.pending_topics |= topics;
    }
}

/// Observe [`AppState`] for the `topics` a view reads: `f` runs on every untagged notify and
/// on each announcement that names one of `topics`.
pub trait ObserveState<V: 'static> {
    fn observe_state(
        &mut self,
        state: &Entity<AppState>,
        topics: Topics,
        f: impl FnMut(&mut V, Entity<AppState>, &mut Context<V>) + 'static,
    ) -> Subscription;

    fn observe_state_in(
        &mut self,
        state: &Entity<AppState>,
        topics: Topics,
        window: &mut Window,
        f: impl FnMut(&mut V, Entity<AppState>, &mut Window, &mut Context<V>) + 'static,
    ) -> Subscription;

    /// [`ObserveState::observe_state_in`] passing what changed: the announced topics, or
    /// [`Topics::ALL`] for an untagged notify.
    fn observe_state_changes_in(
        &mut self,
        state: &Entity<AppState>,
        topics: Topics,
        window: &mut Window,
        f: impl FnMut(&mut V, Topics, &mut Window, &mut Context<V>) + 'static,
    ) -> Subscription;
}

impl<V: 'static> ObserveState<V> for Context<'_, V> {
    fn observe_state(
        &mut self,
        state: &Entity<AppState>,
        topics: Topics,
        f: impl FnMut(&mut V, Entity<AppState>, &mut Context<V>) + 'static,
    ) -> Subscription {
        let f = Rc::new(RefCell::new(f));
        let g = f.clone();
        Subscription::join(
            self.guarded_observe(state, move |v, s, cx| (f.borrow_mut())(v, s, cx)),
            self.guarded_subscribe(state, move |v, s, ev: &StateChanged, cx| {
                if ev.0.intersects(topics) {
                    (g.borrow_mut())(v, s, cx)
                }
            }),
        )
    }

    fn observe_state_in(
        &mut self,
        state: &Entity<AppState>,
        topics: Topics,
        window: &mut Window,
        f: impl FnMut(&mut V, Entity<AppState>, &mut Window, &mut Context<V>) + 'static,
    ) -> Subscription {
        let f = Rc::new(RefCell::new(f));
        let g = f.clone();
        Subscription::join(
            self.guarded_observe_in(state, window, move |v, s, w, cx| {
                (f.borrow_mut())(v, s, w, cx)
            }),
            self.guarded_subscribe_in(state, window, move |v, s, ev: &StateChanged, w, cx| {
                if ev.0.intersects(topics) {
                    (g.borrow_mut())(v, s.clone(), w, cx)
                }
            }),
        )
    }

    fn observe_state_changes_in(
        &mut self,
        state: &Entity<AppState>,
        topics: Topics,
        window: &mut Window,
        f: impl FnMut(&mut V, Topics, &mut Window, &mut Context<V>) + 'static,
    ) -> Subscription {
        let f = Rc::new(RefCell::new(f));
        let g = f.clone();
        Subscription::join(
            self.guarded_observe_in(state, window, move |v, _, w, cx| {
                (f.borrow_mut())(v, Topics::ALL, w, cx)
            }),
            self.guarded_subscribe_in(state, window, move |v, _, ev: &StateChanged, w, cx| {
                if ev.0.intersects(topics) {
                    (g.borrow_mut())(v, ev.0, w, cx)
                }
            }),
        )
    }
}

/// [`ObserveState::observe_state`] for code outside a view (the desktop, smoke drivers).
pub fn observe_state_app(
    state: &Entity<AppState>,
    topics: Topics,
    f: impl FnMut(Entity<AppState>, &mut App) + 'static,
    cx: &mut App,
) -> Subscription {
    let f = Rc::new(RefCell::new(f));
    let g = f.clone();
    Subscription::join(
        cx.observe(state, move |s, cx| (f.borrow_mut())(s, cx)),
        cx.subscribe(state, move |s, ev: &StateChanged, cx| {
            if ev.0.intersects(topics) {
                (g.borrow_mut())(s, cx)
            }
        }),
    )
}
