//! Which stacked file owns the scroll position.

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
