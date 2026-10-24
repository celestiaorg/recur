//! `ArmedButton.svelte` and `lib/armed-confirm.ts`: a two-click confirm button. The first
//! click arms it (red, confirm label) for `timeout`; a second click, or releasing the pointer
//! on it while armed, confirms.

use crate::color::Color;
use crate::fonts::weight;
use crate::style::{Tbtn, TbtnSize, button_label, tbtn};
use crate::tooltip::titled;
use gpui_kit::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement, IntoElement, MouseButton,
    ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Task, Window,
};
use std::rc::Rc;
use std::time::Duration;

/// `ArmedConfirm`: whether the button is armed, and whether the click that follows a
/// pointer-up confirm must be swallowed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArmedConfirm {
    pub armed: bool,
    pub eat_click: bool,
}

/// `resetArmedConfirm`.
pub fn reset_armed_confirm(g: &mut ArmedConfirm) {
    g.armed = false;
    g.eat_click = false;
}

/// `takeClickEcho`.
pub fn take_click_echo(g: &mut ArmedConfirm) -> bool {
    std::mem::take(&mut g.eat_click)
}

/// `armedPointerUp`: true when this release should confirm. Does not arm. `button` is the
/// DOM button index (0 = primary).
pub fn armed_pointer_up(
    g: &mut ArmedConfirm,
    button: u8,
    disabled: bool,
    inspect_ready: bool,
) -> bool {
    if button != 0 || disabled || !g.armed || !inspect_ready {
        return false;
    }
    g.armed = false;
    // `markClickEcho`; the caller clears it on the next task.
    g.eat_click = true;
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArmedAction {
    Arm,
    Confirm,
}

/// `armedClick`.
pub fn armed_click(
    g: &mut ArmedConfirm,
    disabled: bool,
    inspect_ready: bool,
) -> Option<ArmedAction> {
    if take_click_echo(g) || disabled {
        return None;
    }
    if !g.armed {
        g.armed = true;
        return Some(ArmedAction::Arm);
    }
    if !inspect_ready {
        return None;
    }
    g.armed = false;
    Some(ArmedAction::Confirm)
}

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Default)]
struct State {
    gate: ArmedConfirm,
    armed: bool,
    timer: Option<Task<()>>,
}

/// `<ArmedButton label confirmLabel onconfirm onarm cls disabled timeout icon title>`.
#[derive(IntoElement)]
pub struct ArmedButton {
    id: ElementId,
    label: SharedString,
    confirm_label: SharedString,
    on_confirm: Handler,
    on_arm: Option<Handler>,
    size: TbtnSize,
    disabled_over: Option<Color>,
    timeout: Duration,
    icon: Option<AnyElement>,
    title: Option<SharedString>,
}

pub fn armed_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    confirm_label: impl Into<SharedString>,
    on_confirm: impl Fn(&mut Window, &mut App) + 'static,
) -> ArmedButton {
    ArmedButton {
        id: id.into(),
        label: label.into(),
        confirm_label: confirm_label.into(),
        on_confirm: Rc::new(on_confirm),
        on_arm: None,
        size: TbtnSize::BASE,
        disabled_over: None,
        timeout: Duration::from_millis(3000),
        icon: None,
        title: None,
    }
}

impl ArmedButton {
    /// Fired when the button arms (refresh confirm copy, …).
    pub fn on_arm(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_arm = Some(Rc::new(f));
        self
    }
    /// Button metrics (`cls`): the base `.tbtn` or the titlebar's.
    pub fn size(mut self, size: TbtnSize) -> Self {
        self.size = size;
        self
    }
    /// Disabled, faded over `backdrop` (the color it sits on).
    pub fn disabled(mut self, backdrop: Option<Color>) -> Self {
        self.disabled_over = backdrop;
        self
    }
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }
}

impl ArmedButton {
    fn render_view(self, window: &mut Window, cx: &mut App) -> impl IntoElement + use<> {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| State::default());
        let disabled = self.disabled_over.is_some();
        if disabled && (state.read(cx).armed || state.read(cx).gate.armed) {
            state.update(cx, |s, _| {
                s.armed = false;
                reset_armed_confirm(&mut s.gate);
                s.timer = None;
            });
        }
        let armed = state.read(cx).armed;
        let kind = if armed { Tbtn::Armed } else { Tbtn::Danger };
        let label = if armed {
            self.confirm_label
        } else {
            self.label
        };
        let confirm = {
            let state = state.clone();
            let on_confirm = self.on_confirm.clone();
            move |window: &mut Window, cx: &mut App| {
                state.update(cx, |s, _| {
                    s.timer = None;
                    s.armed = false;
                });
                on_confirm(window, cx);
            }
        };
        let mut button = tbtn(self.id.clone(), kind, self.size, self.disabled_over, cx);
        {
            let state = state.clone();
            let confirm = confirm.clone();
            // Capture phase: GPUI fires `on_click` in the bubble phase of the same mouse-up,
            // and the DOM's `pointerup` comes before `click`.
            button = button.capture_any_mouse_up(move |e, window, cx| {
                let b = if e.button == MouseButton::Left { 0 } else { 1 };
                let hit = state.update(cx, |s, _| armed_pointer_up(&mut s.gate, b, disabled, true));
                if !hit {
                    return;
                }
                // `markClickEcho`: forget the echo on the next task even if the button unmounts.
                let st = state.clone();
                cx.defer(move |cx| st.update(cx, |s, _| s.gate.eat_click = false));
                confirm(window, cx);
            });
        }
        {
            let state = state.clone();
            let on_arm = self.on_arm.clone();
            let timeout = self.timeout;
            button = button.on_click(move |_: &ClickEvent, window, cx| {
                let action = state.update(cx, |s, _| armed_click(&mut s.gate, disabled, true));
                match action {
                    Some(ArmedAction::Arm) => {
                        let st = state.clone();
                        let timer = cx.spawn(async move |cx| {
                            cx.background_executor().timer(timeout).await;
                            st.update(cx, |s, cx| {
                                reset_armed_confirm(&mut s.gate);
                                s.armed = false;
                                cx.notify();
                            });
                        });
                        state.update(cx, |s, cx| {
                            s.armed = true;
                            s.timer = Some(timer);
                            cx.notify();
                        });
                        if let Some(f) = &on_arm {
                            f(window, cx);
                        }
                    }
                    Some(ArmedAction::Confirm) => confirm(window, cx),
                    None => {}
                }
            });
        }
        let sz = self.size;
        let button =
            button
                .children(self.icon)
                .child(button_label(label, sz.font_size, weight::MEDIUM));
        match self.title {
            Some(t) => titled((self.id, "title"), t, button).into_any_element(),
            None => button.into_any_element(),
        }
    }
}

impl RenderOnce for ArmedButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        crate::guard::component(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_click_arms_second_confirms() {
        let mut g = ArmedConfirm::default();
        assert_eq!(armed_click(&mut g, false, true), Some(ArmedAction::Arm));
        assert!(g.armed);
        assert_eq!(armed_click(&mut g, false, true), Some(ArmedAction::Confirm));
        assert!(!g.armed);
    }

    #[test]
    fn pointer_up_confirms_and_swallows_the_click() {
        let mut g = ArmedConfirm::default();
        armed_click(&mut g, false, true);
        assert!(armed_pointer_up(&mut g, 0, false, true));
        assert!(!g.armed);
        assert_eq!(armed_click(&mut g, false, true), None);
        assert!(!g.armed);
        // Next task: the echo is gone and the next click arms.
        g.eat_click = false;
        assert_eq!(armed_click(&mut g, false, true), Some(ArmedAction::Arm));
    }

    #[test]
    fn pointer_up_never_arms_and_ignores_other_buttons() {
        let mut g = ArmedConfirm::default();
        assert!(!armed_pointer_up(&mut g, 0, false, true));
        assert_eq!(armed_click(&mut g, false, true), Some(ArmedAction::Arm));
        assert!(!armed_pointer_up(&mut g, 2, false, true));
        assert!(g.armed && !g.eat_click);
    }

    #[test]
    fn disabled_and_not_ready() {
        let mut g = ArmedConfirm::default();
        armed_click(&mut g, false, true);
        assert!(!armed_pointer_up(&mut g, 0, true, true));
        assert_eq!(armed_click(&mut g, true, true), None);
        assert!(g.armed);
        assert!(!armed_pointer_up(&mut g, 0, false, false));
        assert_eq!(armed_click(&mut g, false, false), None);
        assert_eq!(armed_click(&mut g, false, true), Some(ArmedAction::Confirm));
    }
}
