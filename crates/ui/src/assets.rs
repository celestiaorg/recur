//! Embedded assets: the generated icon SVGs and the Lucide catalog. `Assets` serves them to
//! GPUI under `recur/…` and falls back to the gpui-kit defaults for everything else.

use gpui_kit::{AssetSource, Result, SharedString};
use recur_base::embed::Dir;
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

static ICONS: Dir = include!(concat!(env!("OUT_DIR"), "/icons.rs"));

/// Asset prefix for app icons.
pub const APP_ICONS: &str = "recur/icons/app";
/// Asset prefix for the Lucide catalog.
pub const LUCIDE_ICONS: &str = "recur/icons/lucide";
/// Asset prefix for agent provider marks.
pub const PROVIDER_ICONS: &str = "recur/icons/provider";

/// The application asset source: Recur's assets first, then gpui-kit's.
pub struct Assets;

fn embedded(path: &str) -> Option<&'static [u8]> {
    let rel = path.strip_prefix("recur/icons/")?;
    ICONS.get_file(rel).map(|f| f.contents)
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = embedded(path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        if let Some(rel) = path.strip_prefix("recur/icons/") {
            let rel = rel.trim_end_matches('/');
            let Some(dir) = ICONS.get_dir(rel) else {
                return Ok(Vec::new());
            };
            return Ok(dir
                .files()
                .map(|f| format!("recur/icons/{}", f.path).into())
                .collect());
        }
        gpui_kit::assets::Assets.list(path)
    }
}

/// SVG bytes for an embedded asset path.
pub fn svg_bytes(path: &str) -> Option<&'static [u8]> {
    embedded(path)
}

/// One Lucide icon in picker order (`LucideIconEntry`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct LucideEntry {
    pub name: String,
    pub aliases: Vec<String>,
}

#[derive(Deserialize)]
struct LucideIndex {
    icons: Vec<LucideEntry>,
    suggested: Vec<String>,
}

struct Lucide {
    icons: Vec<LucideEntry>,
    suggested: Vec<usize>,
    by_name: HashMap<String, usize>,
}

fn lucide() -> &'static Lucide {
    static L: OnceLock<Lucide> = OnceLock::new();
    L.get_or_init(|| {
        let raw = ICONS
            .get_file("lucide.json")
            .map(|f| f.contents)
            .unwrap_or(b"{\"icons\":[],\"suggested\":[]}");
        let index: LucideIndex = serde_json::from_slice(raw).unwrap_or(LucideIndex {
            icons: Vec::new(),
            suggested: Vec::new(),
        });
        let mut by_name = HashMap::new();
        for (i, e) in index.icons.iter().enumerate() {
            by_name.insert(e.name.clone(), i);
            for a in &e.aliases {
                by_name.entry(a.clone()).or_insert(i);
            }
        }
        let suggested = index
            .suggested
            .iter()
            .filter_map(|n| by_name.get(n).copied())
            .collect();
        Lucide {
            icons: index.icons,
            suggested,
            by_name,
        }
    })
}

/// Every Lucide icon, sorted by name (`lucideIcons`).
pub fn lucide_icons() -> &'static [LucideEntry] {
    &lucide().icons
}

/// The IconPicker's default grid (`lucideSuggested`).
pub fn lucide_suggested() -> impl Iterator<Item = &'static LucideEntry> {
    let l = lucide();
    l.suggested.iter().map(|&i| &l.icons[i])
}

/// The catalog entry for a name or alias; trims and lowercases the name first.
pub fn lucide_entry(name: &str) -> Option<&'static LucideEntry> {
    let l = lucide();
    l.by_name
        .get(&name.trim().to_lowercase())
        .map(|&i| &l.icons[i])
}

/// `hasLucideIcon`.
pub fn has_lucide_icon(name: &str) -> bool {
    lucide_entry(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icon_name::IconName;

    #[test]
    fn every_app_icon_is_embedded() {
        for name in IconName::ALL {
            assert!(
                svg_bytes(&format!("{APP_ICONS}/{}.svg", name.key())).is_some(),
                "{name:?}"
            );
        }
    }

    #[test]
    fn lucide_catalog_resolves_names_and_aliases() {
        assert!(lucide_icons().len() > 1500);
        assert!(has_lucide_icon(" Box "));
        let suggested: Vec<&str> = lucide_suggested().map(|e| e.name.as_str()).collect();
        assert_eq!(suggested.first(), Some(&"box"));
        // Lucide lists `house` as an alias of `home`; the picker suggests it by alias.
        assert_eq!(lucide_entry("house").map(|e| e.name.as_str()), Some("home"));
        assert!(suggested.contains(&"home"));
        for e in lucide_icons() {
            assert!(svg_bytes(&format!("{LUCIDE_ICONS}/{}.svg", e.name)).is_some());
        }
    }
}
