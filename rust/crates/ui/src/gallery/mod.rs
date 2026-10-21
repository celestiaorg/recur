//! Widget gallery scenes. `rust/tools/ui-parity/scenes.json` is the single scene list: the
//! Svelte reference harness, the headless parity renderer and `examples/gallery.rs` all read
//! it, and each scene name maps to one builder here.

mod scenes;

use crate::theme::ThemeId;
use gpui_kit::{AnyView, App, Window};
use serde::Deserialize;

const SCENES_JSON: &str = include_str!("../../../../tools/ui-parity/scenes.json");
pub(crate) const CALIBRATION_JSON: &str =
    include_str!("../../../../tools/ui-parity/calibration.json");

/// One input step, replayed identically by Playwright and by the Rust renderer.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Step {
    Move([f32; 2]),
    Down([f32; 2]),
    Up([f32; 2]),
    Click([f32; 2]),
    /// A Playwright key name (`Tab`, `ArrowDown`, `Escape`, `Enter`, `a`).
    Key(String),
    Type(String),
    Wait(u64),
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneSpec {
    pub name: String,
    pub width: f32,
    pub height: f32,
    pub theme: String,
    /// Differing text and anti-aliased pixels allowed. Documented in `ui-parity.md`.
    #[serde(default)]
    pub tolerance: u64,
    /// Hard differences allowed, each one explained in `ui-parity.md` (curved-edge coverage
    /// where GPUI's shader and CoreGraphics disagree, gradient dither beside a glyph).
    #[serde(default)]
    pub hard_tolerance: u64,
    #[serde(default)]
    pub steps: Vec<Step>,
    /// Where infinite animations are frozen, in ms (finite ones settle at their end).
    pub animation_time: Option<u64>,
}

impl SceneSpec {
    pub fn theme_id(&self) -> ThemeId {
        ThemeId::from_key(&self.theme)
    }
}

#[derive(Deserialize)]
struct Specs {
    scenes: Vec<SceneSpec>,
}

/// Every scene in `scenes.json`, in file order.
pub fn specs() -> Vec<SceneSpec> {
    serde_json::from_str::<Specs>(SCENES_JSON)
        .map(|s| s.scenes)
        .unwrap_or_default()
}

/// Placeholder root for a name without a builder.
pub struct Missing;

impl gpui_kit::Render for Missing {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::Styled;
        crate::style::body(cx).size_full()
    }
}

/// Whether a builder exists for this scene name.
pub fn exists(name: &str) -> bool {
    scenes::NAMES.iter().any(|p| scenes::matches(p, name))
}

/// Build the root view for a scene, or `None` for a name without a builder.
pub fn build(name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    scenes::build(name, window, cx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_scene_has_a_builder() {
        let specs = specs();
        assert!(!specs.is_empty());
        let missing: Vec<&str> = specs
            .iter()
            .map(|s| s.name.as_str())
            .filter(|n| !exists(n))
            .collect();
        assert!(missing.is_empty(), "no builder for {missing:?}");
    }
}
