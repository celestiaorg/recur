//! Modal plumbing shared by New workspace and New project (`lib/dialog-focus.ts`): window
//! keys in the capture phase, as the Svelte modals listen on `window` with `capture`.

use gpui_kit::{
    Context, DispatchPhase, IntoElement, KeyDownEvent, Styled, WeakEntity, Window, canvas,
};

/// Routes every key-down to `f` before the focused element sees it, while this element is
/// painted (that is, while the modal is open).
pub fn key_capture<V: 'static>(
    view: WeakEntity<V>,
    f: fn(&mut V, &KeyDownEvent, &mut Window, &mut Context<V>),
) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |_, _, window, _| {
            let view = view.clone();
            window.on_key_event(move |ev: &KeyDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                let _ = view.update(cx, |v, cx| f(v, ev, window, cx));
            });
        },
    )
    .absolute()
    .size_0()
}

/// The next stop of `cycleDialogTab`: from `at` (none focused: the first or last), wrapping.
pub fn cycle(len: usize, at: Option<usize>, back: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match (at, back) {
        (None, false) => 0,
        (None, true) => len - 1,
        (Some(0), true) => len - 1,
        (Some(i), true) => i - 1,
        (Some(i), false) if i + 1 >= len => 0,
        (Some(i), false) => i + 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_wraps_both_ways() {
        assert_eq!(cycle(3, None, false), Some(0));
        assert_eq!(cycle(3, None, true), Some(2));
        assert_eq!(cycle(3, Some(2), false), Some(0));
        assert_eq!(cycle(3, Some(0), true), Some(2));
        assert_eq!(cycle(3, Some(1), false), Some(2));
        assert_eq!(cycle(0, None, false), None);
    }
}
