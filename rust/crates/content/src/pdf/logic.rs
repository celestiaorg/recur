//! `lib/pdf-preview.ts`: paging and zoom arithmetic for the PDF preview.

pub const MIN_SCALE: f32 = 0.1;
pub const MAX_SCALE: f32 = 8.;
pub const ZOOM_STEP: f32 = 1.25;

fn clamp_scale(s: f32) -> f32 {
    if !s.is_finite() {
        return 1.;
    }
    s.clamp(MIN_SCALE, MAX_SCALE)
}

/// `clampPage`: a page number within `1..=pages`.
pub fn clamp_page(n: f64, pages: usize) -> usize {
    if !n.is_finite() || pages < 1 {
        return 1;
    }
    // `Math.round`: halves round up.
    let r = (n + 0.5).floor();
    (r.max(1.) as usize).min(pages)
}

/// `zoomStep`.
pub fn zoom_step(scale: f32, dir: i8) -> f32 {
    clamp_scale(if dir > 0 {
        scale * ZOOM_STEP
    } else {
        scale / ZOOM_STEP
    })
}

/// `fitWidthScale`.
pub fn fit_width_scale(page_width: f32, viewport_width: f32) -> f32 {
    // `!(x > 0)`: NaN is not positive either.
    let positive = |v: f32| v > 0.;
    if !positive(page_width) || !positive(viewport_width) {
        return 1.;
    }
    clamp_scale(viewport_width / page_width)
}

/// `pageAtScroll`: the page whose slot is nearest the viewport top (ties keep the earlier
/// page). Scrolled to the end, a later current page that cannot align to the top stays.
pub fn page_at_scroll(
    scroll_top: f32,
    heights: &[f32],
    gap: f32,
    max_scroll_top: Option<f32>,
    current: Option<usize>,
) -> usize {
    if heights.is_empty() {
        return 1;
    }
    let y = if scroll_top.is_finite() {
        scroll_top
    } else {
        0.
    };
    let mut offset = 0.;
    let mut best = 1;
    let mut best_dist = f32::INFINITY;
    for (i, h) in heights.iter().enumerate() {
        let (top, bottom) = (offset, offset + h);
        let dist = if y < top {
            top - y
        } else if y > bottom {
            y - bottom
        } else {
            0.
        };
        if dist < best_dist {
            best_dist = dist;
            best = i + 1;
        }
        offset = bottom + gap;
    }
    if let (Some(cur), Some(max)) = (current, max_scroll_top)
        && cur > best
        && max > 0.
        && max.is_finite()
        && y >= max - 1.
    {
        return clamp_page(cur as f64, heights.len());
    }
    best
}

/// `pageOriginInScroller`: the first slot's top in scroll coordinates.
pub fn page_origin_in_scroller(
    scroller_top: f32,
    scroll_top: f32,
    first_slot_top: Option<f32>,
    fallback: f32,
) -> f32 {
    match first_slot_top {
        Some(t) => t - scroller_top + scroll_top,
        None => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_pages() {
        assert_eq!(clamp_page(0., 10), 1);
        assert_eq!(clamp_page(11., 10), 10);
        assert_eq!(clamp_page(3.7, 10), 4);
        assert_eq!(clamp_page(3.2, 10), 3);
        assert_eq!(clamp_page(5., 5), 5);
        assert_eq!(clamp_page(f64::NAN, 10), 1);
        assert_eq!(clamp_page(f64::INFINITY, 10), 1);
        assert_eq!(clamp_page(5., 0), 1);
    }

    #[test]
    fn zoom_steps_clamp() {
        assert_eq!(zoom_step(1., 1), 1.25);
        assert_eq!(zoom_step(1., -1), 0.8);
        assert_eq!(zoom_step(8., 1), 8.);
        assert_eq!(zoom_step(0.1, -1), 0.1);
    }

    #[test]
    fn fit_width() {
        assert_eq!(fit_width_scale(100., 200.), 2.);
        assert_eq!(fit_width_scale(100., 1000.), 8.);
        assert_eq!(fit_width_scale(1000., 50.), 0.1);
        assert_eq!(fit_width_scale(0., 100.), 1.);
        assert_eq!(fit_width_scale(100., 0.), 1.);
    }

    #[test]
    fn page_at_scroll_picks_the_nearest_slot() {
        let even = [100., 100., 100.];
        assert_eq!(page_at_scroll(0., &[], 8., None, None), 1);
        assert_eq!(page_at_scroll(0., &even, 8., None, None), 1);
        assert_eq!(page_at_scroll(99., &even, 8., None, None), 1);
        assert_eq!(page_at_scroll(108., &even, 8., None, None), 2);
        assert_eq!(page_at_scroll(216., &even, 8., None, None), 3);
        assert_eq!(page_at_scroll(103., &even, 8., None, None), 1);
        assert_eq!(page_at_scroll(104., &even, 8., None, None), 1);
        assert_eq!(page_at_scroll(105., &even, 8., None, None), 2);
        assert_eq!(page_at_scroll(f32::NAN, &even, 8., None, None), 1);
    }

    #[test]
    fn a_pinned_end_keeps_a_later_page() {
        let h = [400., 400.];
        let (pad, g, vh) = (12., 12., 700.);
        let max = pad + h[0] + g + h[1] + pad - vh;
        let y = max - pad;
        assert_eq!(max, 136.);
        assert_eq!(page_at_scroll(y, &h, g, None, None), 1);
        assert_eq!(page_at_scroll(y, &h, g, Some(y), Some(2)), 2);
        assert_eq!(page_at_scroll(y, &h, g, Some(y), Some(1)), 1);
        assert_eq!(page_at_scroll(0., &h, g, Some(y), Some(2)), 1);
    }

    #[test]
    fn page_origin_uses_the_viewport_not_the_toolbar() {
        let (toolbar, pad, gap) = (40., 12., 12.);
        let h = [800., 800.];
        let next = pad + h[0] + gap;
        assert_eq!(
            page_at_scroll(next - (toolbar + pad), &h, gap, None, None),
            1
        );
        let origin = page_origin_in_scroller(toolbar, next, Some(toolbar + pad - next), pad);
        assert_eq!(origin, pad);
        assert_eq!(page_at_scroll(next - origin, &h, gap, None, None), 2);
        assert_eq!(page_origin_in_scroller(0., 0., None, 12.), 12.);
    }
}
