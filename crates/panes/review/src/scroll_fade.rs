//! Whether a scroller has left each edge, for its `.fade-top` /
//! `.fade-bot` masks. Hosts call these with the scroller's geometry each frame (GPUI has no
//! class list to toggle).

pub const SCROLL_FADE_SLACK: f32 = 1.;

pub fn is_scrolled_from_top(scroll_top: f32, slack: f32) -> bool {
    scroll_top > slack
}

/// More content sits below the viewport (not pinned to the tail).
pub fn is_away_from_bottom(scroll_top: f32, max_scroll: f32, slack: f32) -> bool {
    max_scroll > slack && scroll_top < max_scroll - slack
}

pub fn is_scrolled_from_bottom(
    scroll_top: f32,
    scroll_height: f32,
    client_height: f32,
    slack: f32,
) -> bool {
    is_away_from_bottom(scroll_top, scroll_height - client_height, slack)
}

/// The two edge flags `syncScrollEdgeFades` sets: `(fade_top, fade_bot)`.
pub fn edge_fades(
    scroll_top: f32,
    scroll_height: f32,
    client_height: f32,
    top_slack: f32,
    bot_slack: f32,
) -> (bool, bool) {
    (
        is_scrolled_from_top(scroll_top, top_slack),
        is_scrolled_from_bottom(scroll_top, scroll_height, client_height, bot_slack),
    )
}

/// `fadeScrollEdges`' state: `ready` (`.fade-ready`) once measured, so a stick-to-bottom
/// scroller keeps its default top fade until a restore moved it; parked scrollers keep their
/// classes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EdgeFades {
    pub ready: bool,
    pub top: bool,
    pub bot: bool,
}

impl EdgeFades {
    /// `apply`, with the cached `max_scroll` and whether the scroller is parked.
    pub fn apply(
        &mut self,
        scroll_top: f32,
        max_scroll: f32,
        top_slack: f32,
        bot_slack: f32,
        pinned_to_bottom: bool,
        parked: bool,
    ) {
        if parked {
            return;
        }
        let from_top = is_scrolled_from_top(scroll_top, top_slack);
        if !from_top && pinned_to_bottom && max_scroll > bot_slack {
            return;
        }
        self.ready = true;
        self.top = from_top;
        self.bot = is_away_from_bottom(scroll_top, max_scroll, bot_slack);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: f32 = SCROLL_FADE_SLACK;

    #[test]
    fn scrolled_from_top_is_false_within_1px_slack() {
        assert!(!is_scrolled_from_top(0., S));
        assert!(!is_scrolled_from_top(1., S));
        assert!(is_scrolled_from_top(2., S));
        assert!(is_scrolled_from_top(80., S));
    }

    #[test]
    fn scrolled_from_bottom() {
        assert!(!is_scrolled_from_bottom(0., 80., 80., S));
        assert!(!is_scrolled_from_bottom(0., 81., 80., S));
        assert!(!is_scrolled_from_bottom(100., 200., 100., S));
        assert!(!is_scrolled_from_bottom(99., 200., 100., S));
        assert!(is_scrolled_from_bottom(0., 200., 100., S));
        assert!(is_scrolled_from_bottom(98., 200., 100., S));
        assert!(!is_scrolled_from_bottom(80., 200., 100., 24.));
        assert!(is_scrolled_from_bottom(70., 200., 100., 24.));
    }

    #[test]
    fn away_from_bottom_matches_against_a_cached_max_scroll() {
        assert!(!is_away_from_bottom(80., 100., 24.));
        assert!(is_away_from_bottom(70., 100., 24.));
        assert!(!is_away_from_bottom(0., 1., S));
    }

    #[test]
    fn edge_fades_at_each_position() {
        assert_eq!(edge_fades(0., 80., 80., S, S), (false, false));
        assert_eq!(edge_fades(0., 200., 100., S, S), (false, true));
        assert_eq!(edge_fades(100., 200., 100., S, S), (true, false));
        assert_eq!(edge_fades(40., 200., 100., S, S), (true, true));
        assert_eq!(edge_fades(80., 200., 100., 1., 24.), (true, false));
        assert!(edge_fades(12., 0., 0., S, S).0);
    }

    #[test]
    fn pinned_to_bottom_is_not_ready_at_the_top_while_content_overflows() {
        let mut f = EdgeFades::default();
        f.apply(0., 300., S, 24., true, false);
        assert!(!f.ready);
    }

    #[test]
    fn a_short_transcript_that_fits_is_ready() {
        let mut f = EdgeFades::default();
        f.apply(0., 0., S, 24., true, false);
        assert!(f.ready);
        assert!(!f.top);
    }

    #[test]
    fn a_parked_scroller_keeps_its_fades() {
        let mut f = EdgeFades {
            ready: false,
            top: true,
            bot: false,
        };
        f.apply(0., 300., S, S, false, true);
        assert!(f.top);
        assert!(!f.ready);
    }
}
