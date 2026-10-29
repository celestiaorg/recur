//! Fitting dropdown menus and popovers to the window.

/// Gap between trigger and menu (`.sel-menu`).
pub const MENU_GAP_PX: f32 = 6.;
/// Gap under a comment ⋯ trigger.
pub const MORE_MENU_GAP_PX: f32 = 4.;
/// Inset from the window edge.
pub const MENU_EDGE_PX: f32 = 8.;
/// Floor for menus whose rows show an inline hint (chat modes).
pub const HINT_MENU_MIN_WIDTH: f32 = 280.;

/// `menuViewportSpace`: px the menu may use without crossing the viewport edge.
pub fn menu_viewport_space(top: f32, bottom: f32, up: bool, viewport_height: f32) -> f32 {
    let space = if up {
        top - MENU_GAP_PX - MENU_EDGE_PX
    } else {
        viewport_height - bottom - MENU_GAP_PX - MENU_EDGE_PX
    };
    space.floor().max(0.)
}

/// `menuPreferUp`: more room above than below.
pub fn menu_prefer_up(top: f32, bottom: f32, viewport_height: f32) -> bool {
    menu_viewport_space(top, bottom, true, viewport_height)
        > menu_viewport_space(top, bottom, false, viewport_height)
}

/// `MenuFixedBox`: either `top` (opens down) or `bottom` (opens up), in viewport px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuBox {
    pub top: Option<f32>,
    pub bottom: Option<f32>,
    pub left: f32,
    pub width: f32,
    pub space: f32,
}

/// `clampMenuLeft`.
pub fn clamp_menu_left(left: f32, width: f32, viewport_width: f32) -> f32 {
    if left + width > viewport_width - MENU_EDGE_PX {
        MENU_EDGE_PX.max(viewport_width - MENU_EDGE_PX - width)
    } else {
        MENU_EDGE_PX.max(left)
    }
}

/// `menuFixedBox` for a trigger rect.
pub fn menu_fixed_box(
    trigger: (f32, f32, f32, f32),
    up: bool,
    min_width: f32,
    viewport: (f32, f32),
) -> MenuBox {
    let (top, bottom, left, width) = trigger;
    let w = width.max(min_width);
    let l = clamp_menu_left(left, w, viewport.0);
    let space = menu_viewport_space(top, bottom, up, viewport.1);
    if up {
        MenuBox {
            top: None,
            bottom: Some(viewport.1 - top + MENU_GAP_PX),
            left: l,
            width: w,
            space,
        }
    } else {
        MenuBox {
            top: Some(bottom + MENU_GAP_PX),
            bottom: None,
            left: l,
            width: w,
            space,
        }
    }
}

/// `popoverTop`: under the trigger, flipped above when it would cross the bottom edge.
pub fn popover_top(
    trigger_top: Option<f32>,
    trigger_bottom: f32,
    menu_height: f32,
    viewport_height: f32,
) -> f32 {
    let below = trigger_bottom + MORE_MENU_GAP_PX;
    match trigger_top {
        Some(top) if menu_height > 0. && below + menu_height > viewport_height - MENU_EDGE_PX => {
            MENU_EDGE_PX.max(top - MORE_MENU_GAP_PX - menu_height)
        }
        _ => below,
    }
}

/// `moreMenuFixedBox`: right-aligned under its trigger. Returns (top, right).
pub fn more_menu_fixed_box(
    trigger_top: Option<f32>,
    trigger_bottom: f32,
    trigger_right: f32,
    menu_height: f32,
    viewport: (f32, f32),
) -> (f32, f32) {
    (
        popover_top(trigger_top, trigger_bottom, menu_height, viewport.1),
        MENU_EDGE_PX.max(viewport.0 - trigger_right),
    )
}

/// `rectsOverlap` for (top, right, bottom, left) boxes.
pub fn rects_overlap(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.2 > b.0 && a.0 < b.2 && a.1 > b.3 && a.3 < b.1
}

/// The menu classes `portalMenuMods` derives from the trigger's context, and their minimum
/// widths and height caps (`max-height: min(<vh>, <px>, var(--sel-space))`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MenuVariant {
    #[default]
    Default,
    /// `.sel-menu-sm` (a `size="sm"` select).
    Small,
    /// `.sel-menu-ghost`.
    Ghost,
    /// `.sel-menu-wide` (crumb base, new-task pickers).
    Wide,
    /// `.sel-menu-compact` (new-task autonomy).
    Compact,
    /// `.sel-menu-font` (editor font picker).
    Font,
    /// `.sel-menu-model` (model pickers).
    Model,
    /// `.sel-menu-issue` (issue picker).
    Issue,
}

impl MenuVariant {
    /// Minimum menu width; a hinted menu (rows with hints) floors at 280.
    pub fn min_width(self, hinted: bool) -> f32 {
        if hinted {
            return HINT_MENU_MIN_WIDTH;
        }
        match self {
            MenuVariant::Model => 240.,
            MenuVariant::Issue => 360.,
            MenuVariant::Compact => 140.,
            MenuVariant::Small | MenuVariant::Ghost | MenuVariant::Wide => 220.,
            MenuVariant::Default | MenuVariant::Font => 0.,
        }
    }

    /// The menu's `max-height` given the viewport height and the measured `space`.
    pub fn max_height(self, viewport_height: f32, space: f32) -> f32 {
        let (vh, cap) = match self {
            MenuVariant::Small | MenuVariant::Ghost => (0.65, 520.),
            MenuVariant::Model => (0.70, 560.),
            MenuVariant::Font => (f32::INFINITY, 320.),
            _ => (0.60, 420.),
        };
        (viewport_height * vh).min(cap).min(space)
    }

    /// `.sel-menu-model { max-width: min(360px, 80vw) }`.
    pub fn max_width(self, viewport_width: f32) -> Option<f32> {
        (self == MenuVariant::Model).then(|| 360f32.min(viewport_width * 0.8))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_space() {
        assert_eq!(menu_viewport_space(220., 244., true, 800.), 220. - 6. - 8.);
        assert_eq!(
            menu_viewport_space(600., 630., false, 800.),
            800. - 630. - 6. - 8.
        );
        assert_eq!(menu_viewport_space(4., 28., true, 800.), 0.);
        assert_eq!(menu_viewport_space(780., 798., false, 800.), 0.);
        assert_eq!(menu_viewport_space(20., 44., true, 800.), 6.);
    }

    #[test]
    fn prefers_the_roomier_side() {
        assert!(!menu_prefer_up(80., 110., 800.));
        assert!(menu_prefer_up(640., 670., 800.));
        assert!(!menu_prefer_up(388., 412., 800.));
    }

    #[test]
    fn fixed_boxes() {
        let b = menu_fixed_box((90., 118., 40., 200.), false, 0., (800., 600.));
        assert_eq!(
            (b.top, b.bottom, b.left, b.width),
            (Some(124.), None, 40., 200.)
        );
        assert_eq!(b.space, 600. - 118. - 6. - 8.);
        let b = menu_fixed_box((400., 430., 20., 160.), true, 0., (800., 720.));
        assert_eq!(
            (b.top, b.bottom, b.left),
            (None, Some(720. - 400. + 6.), 20.)
        );
        let b = menu_fixed_box((100., 130., 700., 100.), false, 240., (800., 720.));
        assert_eq!(b.width, 240.);
        assert!(b.left + b.width <= 800. - 8.);
        assert_eq!(clamp_menu_left(40., 200., 800.), 40.);
        assert_eq!(clamp_menu_left(-12., 200., 800.), 8.);
    }

    #[test]
    fn more_menus_and_overlap() {
        assert_eq!(
            more_menu_fixed_box(None, 120., 700., 0., (800., 720.)),
            (124., 100.)
        );
        assert_eq!(more_menu_fixed_box(None, 40., 796., 0., (800., 720.)).1, 8.);
        assert_eq!(
            more_menu_fixed_box(Some(600.), 620., 700., 150., (800., 720.)).0,
            600. - 4. - 150.
        );
        assert_eq!(
            more_menu_fixed_box(Some(600.), 620., 700., 20., (800., 720.)).0,
            624.
        );
        assert!(rects_overlap(
            (80., 200., 100., 180.),
            (40., 800., 600., 0.)
        ));
        assert!(!rects_overlap(
            (10., 200., 30., 180.),
            (40., 800., 600., 0.)
        ));
    }

    #[test]
    fn variant_floors() {
        assert_eq!(MenuVariant::Small.min_width(false), 220.);
        assert_eq!(MenuVariant::Ghost.min_width(true), HINT_MENU_MIN_WIDTH);
        assert_eq!(MenuVariant::Compact.min_width(false), 140.);
        assert_eq!(MenuVariant::Model.min_width(false), 240.);
        assert_eq!(MenuVariant::Default.max_height(1000., 900.), 420.);
        assert_eq!(MenuVariant::Default.max_height(1000., 100.), 100.);
    }
}
