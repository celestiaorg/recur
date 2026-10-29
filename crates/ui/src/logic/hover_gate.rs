//! `lib/hoverGate.ts`: ignore hover selection until the pointer actually moves, so a menu
//! opened under a resting cursor keeps the keyboard's default row.

/// Minimum movement before hover may change selection.
pub const HOVER_GATE_THRESHOLD_PX: f32 = 4.;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HoverGate {
    pub armed: bool,
    origin: Option<(f32, f32)>,
}

impl HoverGate {
    /// `resetHoverGate`.
    pub fn reset(&mut self) {
        self.armed = false;
        self.origin = None;
    }

    /// `hoverGateAllows`: true once the pointer has moved past the threshold from the first
    /// position seen.
    pub fn allows(&mut self, x: f32, y: f32) -> bool {
        if self.armed {
            return true;
        }
        let Some((ox, oy)) = self.origin else {
            self.origin = Some((x, y));
            return false;
        };
        let (dx, dy) = (x - ox, y - oy);
        if dx * dx + dy * dy < HOVER_GATE_THRESHOLD_PX * HOVER_GATE_THRESHOLD_PX {
            return false;
        }
        self.armed = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_behaves_like_hover_gate_ts() {
        let mut g = HoverGate::default();
        assert!(!g.allows(100., 200.) && !g.allows(100., 200.) && !g.armed);
        let mut g = HoverGate::default();
        assert!(!g.allows(10., 10.) && !g.allows(12., 11.) && !g.armed);
        let mut g = HoverGate::default();
        assert!(!g.allows(10., 10.));
        assert!(g.allows(15., 10.) && g.armed);
        assert!(g.allows(15., 10.) && g.allows(50., 50.));
        g.reset();
        assert!(!g.armed && !g.allows(10., 0.) && !g.allows(10., 0.));
    }
}
