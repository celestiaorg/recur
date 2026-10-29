//! Transcript windowing for long chats (with the review window and spy-pick helpers): past a
//! length only the blocks near the viewport and
//! the live tail mount; the rest are stubs at their measured (or estimated) height.

use std::collections::BTreeSet;

/// How many transcript blocks may mount at once once the list is long.
pub const CHAT_HYDRATE_MAX: usize = 36;
/// Extra blocks around the selected one (usually the live tail).
pub const CHAT_HYDRATE_OVERSCAN: usize = 4;
/// Skip windowing entirely under this length.
pub const CHAT_HYDRATE_ALL_BELOW: usize = 48;
/// Always mount this many trailing blocks so streaming never paints a stub.
pub const CHAT_TAIL_LIVE: usize = 3;
/// Prefetch band above and below the scrollport, in px. Kept small because GPUI lays the
/// whole window out on every frame.
pub const CHAT_VIEW_PAD_PX: f32 = 300.;
/// A collapsed step-group header.
pub const CHAT_BLOCK_HEAD_PX: f32 = 36.;

/// What [`estimate_chat_block_height`] reads.
pub enum BlockShape<'a> {
    Steps,
    Msg {
        me: bool,
        turn_end: bool,
        body: &'a str,
    },
}

/// `estimateChatBlockHeight`.
pub fn estimate_chat_block_height(b: BlockShape<'_>) -> f32 {
    match b {
        BlockShape::Steps => CHAT_BLOCK_HEAD_PX,
        BlockShape::Msg { me: true, .. } => 56.,
        BlockShape::Msg { turn_end: true, .. } => 28.,
        BlockShape::Msg { body, .. } => {
            let n = body.encode_utf16().count();
            if n == 0 {
                return CHAT_BLOCK_HEAD_PX;
            }
            // simplify: ~72 columns; a stub is measured after its first mount.
            ((n as f32 / 72.).ceil() * 20.).clamp(48., 720.)
        }
    }
}

/// `reservedChatHeight`: a live block sizes itself; a stub takes its measured height.
pub fn reserved_chat_height(live: bool, measured: Option<f32>, estimate: f32) -> Option<f32> {
    if live {
        return None;
    }
    Some(measured.filter(|m| *m > 0.).unwrap_or(estimate))
}

/// A block's vertical extent in scroll-content coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockRange {
    pub key: String,
    pub top: f32,
    pub bottom: f32,
}

/// `inViewPaths`: visible blocks first, then the prefetch band, both in document order.
pub fn in_view_keys(ranges: &[BlockRange], scroll_top: f32, view_h: f32, pad: f32) -> Vec<String> {
    let (vis_a, vis_b) = (scroll_top, scroll_top + view_h);
    let (a, b) = (scroll_top - pad, scroll_top + view_h + pad);
    let mut visible = Vec::new();
    let mut prefetch = Vec::new();
    for r in ranges {
        if r.bottom > vis_a && r.top < vis_b {
            visible.push(r.key.clone());
        } else if r.bottom > a && r.top < b {
            prefetch.push(r.key.clone());
        }
    }
    visible.extend(prefetch);
    visible
}

/// `pickHydrated`: the selected block and its neighbors first, then the viewport.
pub fn pick_hydrated(
    keys: &[String],
    selected: &str,
    in_view: &[String],
    max: usize,
    overscan: usize,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if keys.is_empty() || max == 0 {
        return out;
    }
    let index = |k: &str| keys.iter().position(|x| x == k);
    let add = |k: Option<&String>, out: &mut BTreeSet<String>| {
        if let Some(k) = k
            && out.len() < max
            && keys.contains(k)
        {
            out.insert(k.clone());
        }
    };
    let sel = index(selected).unwrap_or(0) as isize;
    for d in 0..=overscan as isize {
        let lo = sel - d;
        if lo >= 0 {
            add(keys.get(lo as usize), &mut out);
        }
        add(keys.get((sel + d) as usize), &mut out);
    }
    for k in in_view {
        add(Some(k), &mut out);
    }
    if out.is_empty() {
        out.insert(keys[0].clone());
    }
    out
}

/// `fileAtScrollTop`: the block containing `scroll_top`, else the last one above it; "" before
/// any block has laid out.
pub fn block_at_scroll_top(ranges: &[BlockRange], scroll_top: f32) -> String {
    if ranges.is_empty() {
        return String::new();
    }
    let (mut lo, mut hi) = (0usize, ranges.len() - 1);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if ranges[mid].top <= scroll_top {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    while lo > 0 && ranges[lo].bottom <= ranges[lo].top {
        lo -= 1;
    }
    if ranges[lo].bottom <= ranges[lo].top {
        return String::new();
    }
    ranges[lo].key.clone()
}

/// Stick to the document end when the viewport is this close to it.
pub const VIEWPORT_END_PX: f32 = 48.;

/// The block at the top of the viewport and its offset, to restore after a layout shift.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewportPin {
    pub at_end: bool,
    pub anchor: Option<(String, f32)>,
}

/// `captureViewportPin`.
pub fn capture_viewport_pin(
    ranges: &[BlockRange],
    scroll_top: f32,
    view_h: f32,
    scroll_height: f32,
) -> ViewportPin {
    let key = block_at_scroll_top(ranges, scroll_top);
    let anchor = (!key.is_empty())
        .then(|| {
            ranges
                .iter()
                .find(|r| r.key == key)
                .map(|r| (key.clone(), r.top - scroll_top))
        })
        .flatten();
    ViewportPin {
        at_end: scroll_top + view_h >= scroll_height - VIEWPORT_END_PX,
        anchor,
    }
}

/// `applyViewportPin`: the scroll offset that puts the pinned block back, `None` to leave it.
pub fn viewport_pin_target(
    ranges: &[BlockRange],
    pin: &ViewportPin,
    scroll_height: f32,
    client_height: f32,
) -> Option<f32> {
    if pin.at_end {
        return Some((scroll_height - client_height).max(0.));
    }
    let (key, rel) = pin.anchor.as_ref()?;
    ranges.iter().find(|r| &r.key == key).map(|r| r.top - rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates() {
        assert_eq!(estimate_chat_block_height(BlockShape::Steps), 36.);
        let me = estimate_chat_block_height(BlockShape::Msg {
            me: true,
            turn_end: false,
            body: "ok",
        });
        let long_body = "x".repeat(400);
        let long = estimate_chat_block_height(BlockShape::Msg {
            me: false,
            turn_end: false,
            body: &long_body,
        });
        assert!(long > me);
        let huge = "x".repeat(20_000);
        assert_eq!(
            estimate_chat_block_height(BlockShape::Msg {
                me: false,
                turn_end: false,
                body: &huge,
            }),
            720.
        );
    }

    #[test]
    fn reserved() {
        assert_eq!(reserved_chat_height(true, Some(400.), 36.), None);
        assert_eq!(reserved_chat_height(false, Some(240.), 36.), Some(240.));
        assert_eq!(reserved_chat_height(false, Some(0.), 56.), Some(56.));
    }

    fn r(key: &str, top: f32, bottom: f32) -> BlockRange {
        BlockRange {
            key: key.into(),
            top,
            bottom,
        }
    }

    // Review window / spy-pick cases the chat relies on
    #[test]
    fn in_view_and_hydrate() {
        let ranges = vec![r("a", 0., 100.), r("b", 100., 900.), r("c", 900., 1000.)];
        assert_eq!(in_view_keys(&ranges, 150., 200., 600.), ["b", "a", "c"]);
        let keys: Vec<String> = ["a", "b", "c", "d"].map(String::from).to_vec();
        let got = pick_hydrated(&keys, "d", &["a".to_string()], 2, 1);
        assert_eq!(got.into_iter().collect::<Vec<_>>(), ["c", "d"]);
        assert_eq!(
            pick_hydrated(&keys, "zz", &[], 3, 0)
                .into_iter()
                .collect::<Vec<_>>(),
            ["a"]
        );
    }

    #[test]
    fn scroll_top_spy_and_pins() {
        let ranges = vec![r("a", 0., 100.), r("b", 100., 300.), r("c", 300., 400.)];
        assert_eq!(block_at_scroll_top(&ranges, 150.), "b");
        assert_eq!(block_at_scroll_top(&ranges, 0.), "a");
        assert_eq!(
            block_at_scroll_top(&[r("a", 0., 0.), r("b", 0., 0.)], 0.),
            ""
        );
        let pin = capture_viewport_pin(&ranges, 150., 100., 400.);
        assert_eq!(pin.anchor, Some(("b".to_string(), -50.)));
        assert!(!pin.at_end);
        let moved = vec![r("a", 0., 140.), r("b", 140., 340.), r("c", 340., 440.)];
        assert_eq!(viewport_pin_target(&moved, &pin, 440., 100.), Some(190.));
        let end = capture_viewport_pin(&ranges, 300., 100., 400.);
        assert!(end.at_end);
        assert_eq!(viewport_pin_target(&moved, &end, 440., 100.), Some(340.));
    }
}
