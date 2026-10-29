//! Widget gallery scenes. `crates/ui/gallery/scenes.json` lists them for
//! `examples/gallery.rs`, and each scene name maps to one builder here.

mod scenes;

use crate::theme::ThemeId;
use gpui_kit::{AnyView, App, Window};
use serde::Deserialize;

const SCENES_JSON: &str = include_str!("../../gallery/scenes.json");
pub(crate) const CALIBRATION_JSON: &str = include_str!("../../gallery/calibration.json");

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneSpec {
    pub name: String,
    pub width: f32,
    pub height: f32,
    pub theme: String,
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

/// Scene prefixes built by other crates' galleries (`recur_editor::gallery`).
pub const EXTERNAL: &[&str] = &["editor/"];

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
            .filter(|n| !exists(n) && !EXTERNAL.iter().any(|p| n.starts_with(p)))
            .collect();
        assert!(missing.is_empty(), "no builder for {missing:?}");
    }
}
