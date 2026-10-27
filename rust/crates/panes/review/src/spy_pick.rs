//! `lib/spy-pick.ts`: which stacked file owns the scroll position, and keeping the viewport on
//! it while cards above change height.

use crate::review_window::CardRange;

/// The file whose range contains `scroll_top`, else the last one above it. Empty when no card
/// has laid out yet (all 0×0): guessing would pick the last path and the hydrator would chase
/// it.
pub fn file_at_scroll_top(files: &[CardRange], scroll_top: f32) -> String {
    if files.is_empty() {
        return String::new();
    }
    let (mut lo, mut hi) = (0usize, files.len() - 1);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if files[mid].top <= scroll_top {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    while lo > 0 && files[lo].bottom <= files[lo].top {
        lo -= 1;
    }
    if files[lo].bottom <= files[lo].top {
        return String::new();
    }
    files[lo].path.clone()
}

/// The viewport offset of the file at `scrollTop`.
#[derive(Clone, Debug, PartialEq)]
pub struct ScrollAnchor {
    pub path: String,
    pub rel: f32,
}

/// Stick to the document end when the user is already this close to it.
pub const VIEWPORT_END_PX: f32 = 48.;

#[derive(Clone, Debug, PartialEq)]
pub struct ViewportPin {
    pub at_end: bool,
    pub anchor: Option<ScrollAnchor>,
}

pub fn capture_scroll_anchor(files: &[CardRange], scroll_top: f32) -> Option<ScrollAnchor> {
    let path = file_at_scroll_top(files, scroll_top);
    if path.is_empty() {
        return None;
    }
    files.iter().find(|f| f.path == path).map(|f| ScrollAnchor {
        rel: f.top - scroll_top,
        path,
    })
}

pub fn restore_scroll_top(files: &[CardRange], anchor: &ScrollAnchor) -> Option<f32> {
    files
        .iter()
        .find(|f| f.path == anchor.path)
        .map(|f| f.top - anchor.rel)
}

pub fn capture_viewport_pin(
    files: &[CardRange],
    scroll_top: f32,
    view_h: f32,
    scroll_height: f32,
) -> ViewportPin {
    ViewportPin {
        at_end: scroll_top + view_h >= scroll_height - VIEWPORT_END_PX,
        anchor: capture_scroll_anchor(files, scroll_top),
    }
}

/// `applyViewportPin`: the scrollTop after a hydration swap, `None` to leave it. `at_end` uses
/// the live max scroll so a growing last file does not strand the user; nothing moves when the
/// viewport moved since capture.
pub fn apply_viewport_pin(
    scroll_top: f32,
    max_scroll: f32,
    files: &[CardRange],
    pin: &ViewportPin,
    captured_top: Option<f32>,
) -> Option<f32> {
    if captured_top.is_some_and(|t| t != scroll_top) {
        return None;
    }
    if pin.at_end {
        return Some(max_scroll.max(0.));
    }
    restore_scroll_top(files, pin.anchor.as_ref()?)
}

/// The adjacent path in document order. `None` at the ends or without paths; an unknown
/// `current` steps to the first (+1) or last (-1).
pub fn step_path<'a>(paths: &'a [String], current: &str, dir: i8) -> Option<&'a String> {
    if paths.is_empty() {
        return None;
    }
    let Some(i) = paths.iter().position(|p| p == current) else {
        return if dir == 1 {
            paths.first()
        } else {
            paths.last()
        };
    };
    let next = i as isize + dir as isize;
    if next < 0 {
        return None;
    }
    paths.get(next as usize)
}

/// The top-most intersecting entry (smallest `top`).
pub fn topmost_intersecting(entries: &[(String, f32, bool)]) -> String {
    let mut best = "";
    let mut best_top = f32::INFINITY;
    for (path, top, intersecting) in entries {
        if !*intersecting || path.is_empty() {
            continue;
        }
        if *top < best_top {
            best_top = *top;
            best = path;
        }
    }
    best.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(path: &str, top: f32, bottom: f32) -> CardRange {
        CardRange {
            path: path.into(),
            top,
            bottom,
        }
    }

    fn files() -> Vec<CardRange> {
        vec![
            r("a.ts", 0., 100.),
            r("b.ts", 100., 400.),
            r("c.ts", 400., 480.),
        ]
    }

    #[test]
    fn file_at_scroll_top_picks_the_file_spanning_the_top_edge() {
        assert_eq!(file_at_scroll_top(&[], 0.), "");
        let f = files();
        for (top, want) in [
            (0., "a.ts"),
            (99., "a.ts"),
            (100., "b.ts"),
            (250., "b.ts"),
            (399., "b.ts"),
            (400., "c.ts"),
            (999., "c.ts"),
        ] {
            assert_eq!(file_at_scroll_top(&f, top), want, "{top}");
        }
    }

    #[test]
    fn file_at_scroll_top_waits_for_layout() {
        let unloaded = vec![r("a.ts", 0., 0.), r("b.ts", 0., 0.), r("c.ts", 0., 0.)];
        assert_eq!(file_at_scroll_top(&unloaded, 0.), "");
        let mixed = vec![
            r("a.ts", 0., 200.),
            r("b.ts", 200., 200.),
            r("c.ts", 200., 200.),
        ];
        assert_eq!(file_at_scroll_top(&mixed, 200.), "a.ts");
    }

    fn stack() -> Vec<CardRange> {
        vec![
            r("a.ts", 0., 400.),
            r("b.ts", 400., 900.),
            r("c.ts", 900., 1100.),
        ]
    }

    #[test]
    fn anchor_keeps_the_offset_into_a_file_after_content_above_shrinks() {
        let anchor = capture_scroll_anchor(&stack(), 1000.).expect("anchor");
        assert_eq!(
            anchor,
            ScrollAnchor {
                path: "c.ts".into(),
                rel: -100.
            }
        );
        let shrunk = vec![
            r("a.ts", 0., 50.),
            r("b.ts", 50., 120.),
            r("c.ts", 120., 320.),
        ];
        assert_eq!(restore_scroll_top(&shrunk, &anchor), Some(220.));
        let gone = capture_scroll_anchor(&stack(), 100.).expect("anchor");
        assert_eq!(restore_scroll_top(&[r("z.ts", 0., 10.)], &gone), None);
    }

    #[test]
    fn viewport_pin_marks_the_end_and_keeps_a_file_relative_pin() {
        let pin = capture_viewport_pin(&stack(), 900., 200., 1100.);
        assert!(pin.at_end);
        assert_eq!(pin.anchor.expect("anchor").path, "c.ts");
        let pin = capture_viewport_pin(&stack(), 100., 200., 1100.);
        assert!(!pin.at_end);
        assert_eq!(
            pin.anchor,
            Some(ScrollAnchor {
                path: "a.ts".into(),
                rel: -100.
            })
        );
    }

    #[test]
    fn apply_viewport_pin_snaps_restores_or_skips() {
        let end = ViewportPin {
            at_end: true,
            anchor: None,
        };
        assert_eq!(
            apply_viewport_pin(200., 3500., &stack(), &end, None),
            Some(3500.)
        );
        let after = vec![
            r("a.ts", 0., 50.),
            r("b.ts", 50., 120.),
            r("c.ts", 120., 320.),
        ];
        let pin = ViewportPin {
            at_end: false,
            anchor: Some(ScrollAnchor {
                path: "c.ts".into(),
                rel: -100.,
            }),
        };
        assert_eq!(apply_viewport_pin(0., 0., &after, &pin, None), Some(220.));
        let pin = ViewportPin {
            at_end: false,
            anchor: Some(ScrollAnchor {
                path: "a.ts".into(),
                rel: 0.,
            }),
        };
        assert_eq!(
            apply_viewport_pin(400., 0., &stack(), &pin, Some(100.)),
            None
        );
    }

    #[test]
    fn step_path_steps_and_stops_at_the_ends() {
        let paths: Vec<String> = ["a.ts", "b.ts", "c.ts"].map(String::from).to_vec();
        assert_eq!(step_path(&[], "a.ts", 1), None);
        assert_eq!(
            step_path(&paths, "a.ts", 1).map(String::as_str),
            Some("b.ts")
        );
        assert_eq!(
            step_path(&paths, "b.ts", 1).map(String::as_str),
            Some("c.ts")
        );
        assert_eq!(
            step_path(&paths, "c.ts", -1).map(String::as_str),
            Some("b.ts")
        );
        assert_eq!(
            step_path(&paths, "b.ts", -1).map(String::as_str),
            Some("a.ts")
        );
        assert_eq!(step_path(&paths, "c.ts", 1), None);
        assert_eq!(step_path(&paths, "a.ts", -1), None);
        assert_eq!(step_path(&paths, "", 1).map(String::as_str), Some("a.ts"));
        assert_eq!(
            step_path(&paths, "missing.ts", -1).map(String::as_str),
            Some("c.ts")
        );
    }

    #[test]
    fn topmost_intersecting_takes_the_smallest_top() {
        assert_eq!(topmost_intersecting(&[("b.ts".into(), 40., true)]), "b.ts");
        assert_eq!(
            file_at_scroll_top(&[r("a.ts", 0., 400.), r("b.ts", 400., 700.)], 320.),
            "a.ts"
        );
        assert_eq!(
            topmost_intersecting(&[
                ("b.ts".into(), 40., true),
                ("a.ts".into(), 8., true),
                ("c.ts".into(), 2., false),
            ]),
            "a.ts"
        );
        assert_eq!(
            topmost_intersecting(&[("".into(), 0., true), ("x.ts".into(), 10., false)]),
            ""
        );
    }
}
