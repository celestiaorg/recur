//! The collapsed rail's left-edge flyout: hovering or tapping the
//! window's left edge overlays the sidebar so navigation stays reachable.

use std::time::Duration;

pub const RAIL_PEEK_EDGE_PX: f32 = 8.;
pub const RAIL_PEEK_COARSE_EDGE_PX: f32 = 16.;
/// Extra px past the edge strip before a pointer move skips the body measure.
pub const RAIL_PEEK_MOVE_SLACK_PX: f32 = 16.;
pub const RAIL_PEEK_SHOW: Duration = Duration::from_millis(90);
pub const RAIL_PEEK_HIDE: Duration = Duration::from_millis(320);

/// A box in window coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PeekRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// True when the pointer is on the body's left edge strip (below the titlebar).
pub fn pointer_in_left_edge(x: f32, y: f32, body: PeekRect, edge_px: f32) -> bool {
    x >= body.left && x <= body.left + edge_px && y >= body.top && y <= body.bottom
}

/// What a move can change while the rail is collapsed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeekFlags {
    pub peeking: bool,
    pub show_pending: bool,
    pub locked: bool,
}

/// True when a collapsed, idle flyout can skip measuring: the pointer is well right of the
/// edge strip, so this move cannot open the overlay.
pub fn pointer_far_from_rail_edge(x: f32, flags: PeekFlags, slack_px: f32, body_left: f32) -> bool {
    if flags.peeking || flags.show_pending || flags.locked {
        return false;
    }
    x > body_left + slack_px
}

/// The default slack: the strip plus [`RAIL_PEEK_MOVE_SLACK_PX`].
pub const DEFAULT_SLACK: f32 = RAIL_PEEK_EDGE_PX + RAIL_PEEK_MOVE_SLACK_PX;

/// A menu or a drag keeps the flyout open without a pointer.
pub fn rail_peek_held(collapsed: bool, menu_open: bool, dragging: bool) -> bool {
    collapsed && (menu_open || dragging)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: PeekRect = PeekRect {
        left: 0.,
        top: 52.,
        right: 1200.,
        bottom: 800.,
    };

    #[test]
    fn left_edge() {
        assert!(pointer_in_left_edge(0., 60., BODY, RAIL_PEEK_EDGE_PX));
        assert!(pointer_in_left_edge(
            RAIL_PEEK_EDGE_PX,
            400.,
            BODY,
            RAIL_PEEK_EDGE_PX
        ));
        assert!(!pointer_in_left_edge(
            RAIL_PEEK_EDGE_PX + 1.,
            400.,
            BODY,
            RAIL_PEEK_EDGE_PX
        ));
        assert!(!pointer_in_left_edge(4., 20., BODY, RAIL_PEEK_EDGE_PX));
        assert!(!pointer_in_left_edge(4., 820., BODY, RAIL_PEEK_EDGE_PX));
        assert!(pointer_in_left_edge(12., 60., BODY, 16.));
        assert!(!pointer_in_left_edge(12., 60., BODY, 8.));
    }

    #[test]
    fn far_from_edge() {
        let idle = PeekFlags::default();
        assert!(pointer_far_from_rail_edge(200., idle, DEFAULT_SLACK, 0.));
        assert!(!pointer_far_from_rail_edge(
            RAIL_PEEK_EDGE_PX + 16.,
            idle,
            DEFAULT_SLACK,
            0.
        ));
        for flags in [
            PeekFlags {
                peeking: true,
                ..idle
            },
            PeekFlags {
                show_pending: true,
                ..idle
            },
            PeekFlags {
                locked: true,
                ..idle
            },
        ] {
            assert!(!pointer_far_from_rail_edge(400., flags, DEFAULT_SLACK, 0.));
        }
        assert!(pointer_far_from_rail_edge(200., idle, DEFAULT_SLACK, 100.));
        assert!(!pointer_far_from_rail_edge(120., idle, DEFAULT_SLACK, 100.));
        assert!(!pointer_far_from_rail_edge(24., idle, DEFAULT_SLACK, 100.));
    }

    #[test]
    fn held() {
        assert!(!rail_peek_held(false, true, true));
        assert!(rail_peek_held(true, true, false));
        assert!(rail_peek_held(true, false, true));
        assert!(!rail_peek_held(true, false, false));
    }
}
