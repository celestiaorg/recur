//! `DecodeText.svelte`: a label that scrambles then resolves left to right when its text
//! changes (LLM renames of chat tabs and worktrees). Switching to another entity
//! (`source_key`) snaps without animating.

use crate::text::{Text, text};
use gpui_kit::{App, ElementId, IntoElement, RenderOnce, SharedString, Task, Window};
use std::time::Duration;

/// Glyph pool for the scramble: the mono Latin set UI labels use.
pub const GLYPHS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz0123456789";
pub const TICK: Duration = Duration::from_millis(28);
/// Ticks each column keeps scrambling before it locks.
pub const HOLD_TICKS: usize = 2;

/// The frame shown at `tick` while decoding toward `target`: locked columns show the target,
/// spaces stay spaces, the rest come from `random`. `None` once fully resolved.
pub fn decode_frame(
    target: &str,
    tick: usize,
    mut random: impl FnMut() -> usize,
) -> Option<String> {
    let len = target.chars().count();
    let total = 8.max(len * HOLD_TICKS + 4);
    let locked = len.min(tick / HOLD_TICKS);
    if tick >= total || locked >= len {
        return None;
    }
    Some(
        target
            .chars()
            .enumerate()
            .map(|(i, ch)| {
                if i < locked || ch == ' ' {
                    ch
                } else {
                    GLYPHS[random() % GLYPHS.len()] as char
                }
            })
            .collect(),
    )
}

#[derive(Default)]
struct State {
    display: SharedString,
    decoding: bool,
    seen: SharedString,
    seen_key: SharedString,
    timer: Option<Task<()>>,
}

type Styler = Box<dyn FnOnce(Text) -> Text>;

/// `<DecodeText text animate sourceKey class>`; style the label with [`DecodeText::style`].
#[derive(IntoElement)]
pub struct DecodeText {
    id: ElementId,
    text: SharedString,
    animate: bool,
    source_key: SharedString,
    style: Option<Styler>,
}

pub fn decode_text(id: impl Into<ElementId>, text: impl Into<SharedString>) -> DecodeText {
    DecodeText {
        id: id.into(),
        text: text.into(),
        animate: true,
        source_key: SharedString::default(),
        style: None,
    }
}

impl DecodeText {
    /// `animate={false}` shows the text immediately.
    pub fn animate(mut self, on: bool) -> Self {
        self.animate = on;
        self
    }
    /// Stable identity of the labelled thing; a change of key snaps instead of decoding.
    pub fn source_key(mut self, key: impl Into<SharedString>) -> Self {
        self.source_key = key.into();
        self
    }
    /// Style the label (family, size, truncation, …).
    pub fn style(mut self, f: impl FnOnce(Text) -> Text + 'static) -> Self {
        self.style = Some(Box::new(f));
        self
    }
}

fn rng_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0x9e37_79b9, |d| d.as_nanos() as u64)
        | 1
}

impl RenderOnce for DecodeText {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| State {
            display: self.text.clone(),
            ..State::default()
        });
        let (next, key) = (self.text.clone(), self.source_key.clone());
        let reduced = crate::motion::reduced(cx) || crate::motion::is_frozen(cx);
        state.update(cx, |s, cx| {
            let snap = |s: &mut State| {
                s.timer = None;
                s.decoding = false;
                s.display = next.clone();
                s.seen = next.clone();
                s.seen_key = key.clone();
            };
            // First paint, animation off, or a different entity reused this label.
            if !self.animate || s.seen.is_empty() || (!key.is_empty() && key != s.seen_key) {
                if s.display != next || s.seen != next || s.seen_key != key || s.decoding {
                    snap(s);
                }
                return;
            }
            if s.seen == next {
                return;
            }
            s.seen = next.clone();
            s.timer = None;
            if next.is_empty() || reduced {
                s.decoding = false;
                s.display = next.clone();
                return;
            }
            s.decoding = true;
            let target = next.clone();
            s.timer = Some(cx.spawn(async move |this, cx| {
                let mut seed = rng_seed();
                let mut random = move || {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    seed as usize
                };
                let mut tick = 0;
                loop {
                    cx.background_executor().timer(TICK).await;
                    tick += 1;
                    let frame = decode_frame(&target, tick, &mut random);
                    let done = frame.is_none();
                    let ok = this.update(cx, |s, cx| {
                        s.display = frame.map_or_else(|| target.clone(), Into::into);
                        if done {
                            s.decoding = false;
                            s.timer = None;
                        }
                        cx.notify();
                    });
                    if done || ok.is_err() {
                        break;
                    }
                }
            }));
        });
        let s = state.read(cx);
        let mut label = text(s.display.clone());
        if let Some(f) = self.style {
            label = f(label);
        }
        // `.decoding { opacity: 0.85 }`.
        label.opacity(if s.decoding { 0.85 } else { 1. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_left_to_right_keeping_spaces() {
        let target = "ab cd";
        // Tick 2 locks one column; spaces never scramble.
        let f = decode_frame(target, 2, || 0).unwrap();
        assert!(f.starts_with('a'));
        assert_eq!(f.chars().nth(2), Some(' '));
        assert_eq!(f.chars().nth(1), Some('A'));
        // len 5 → total max(8, 14) ticks; columns all lock by tick 10.
        assert!(decode_frame(target, 10, || 0).is_none());
        assert!(decode_frame("", 1, || 0).is_none());
        // Short labels still scramble for 8 ticks.
        assert!(decode_frame("x", 1, || 0).is_some());
        assert!(decode_frame("x", 2, || 0).is_none());
    }
}
