//! `lib/mono-fonts.ts`: the monospace fonts the editor and terminal font pickers offer. The
//! union of the native list (`ListMonospaceFonts`) and a probe of common coding faces through
//! the text system (the page probed with a canvas; WKWebView has no Local Font Access), with the
//! bundled faces always first.

use std::sync::OnceLock;

use gpui_kit::{App, AsyncApp, Task, font, px};
use recur_services::Services;
use recur_ui::fonts::Family;

/// `MonoFont`: the stored family name and its label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonoFont {
    pub family: String,
    pub label: String,
}

/// `BUNDLED_MONO_FONTS`: always offered (bundled with the app, or the system stack).
pub const BUNDLED_MONO_FONTS: [&str; 4] = [
    "JetBrains Mono",
    "IBM Plex Mono",
    "Source Code Pro",
    "System Mono",
];

/// `MONO_CANDIDATES`: common coding faces for the permission-free probe.
const MONO_CANDIDATES: [&str; 48] = [
    "JetBrains Mono",
    "JetBrainsMono Nerd Font",
    "JetBrainsMono NF",
    "IBM Plex Mono",
    "Source Code Pro",
    "SauceCodePro Nerd Font",
    "Fira Code",
    "FiraCode Nerd Font",
    "Fira Mono",
    "Cascadia Code",
    "Cascadia Mono",
    "CaskaydiaCove Nerd Font",
    "Hack",
    "Hack Nerd Font",
    "MesloLGS NF",
    "MesloLGL Nerd Font",
    "Meslo LG M",
    "Menlo",
    "Monaco",
    "SF Mono",
    "Consolas",
    "Courier New",
    "Courier",
    "Andale Mono",
    "Roboto Mono",
    "Ubuntu Mono",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Inconsolata",
    "Inconsolata LGC",
    "Anonymous Pro",
    "Victor Mono",
    "Iosevka",
    "Iosevka Term",
    "Iosevka Nerd Font",
    "Input Mono",
    "Operator Mono",
    "Berkeley Mono",
    "Comic Code",
    "Commit Mono",
    "Geist Mono",
    "MD IO",
    "Sudo",
    "PT Mono",
    "Noto Sans Mono",
    "Droid Sans Mono",
    "Lucida Console",
    "OCR A Extended",
];

/// `MONO_EPS`: sub-pixel drift allowed across a four-glyph run at 72px.
const MONO_EPS: f32 = 2.;

/// `looksMonospaceName`.
fn looks_monospace_name(family: &str) -> bool {
    static RE: OnceLock<Option<regex::Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"(?i)mono|consolas|inconsolata|menlo|monaco|iosevka|meslo|jetbrains|hack|fira\s*(code|mono)|cascadia\s*(code|mono)|source\s*code|courier|anonymous\s*pro|nerd\s*font|\bnf\b",
        )
        .ok()
    })
    .as_ref()
    .is_some_and(|re| re.is_match(family))
}

/// `uniqueFamilies`: trimmed, non-empty, first spelling of each case-insensitive name.
fn unique_families(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for n in names {
        let t = n.trim();
        if t.is_empty() || !seen.insert(t.to_lowercase()) {
            continue;
        }
        out.push(t.to_string());
    }
    out
}

/// The picker list: bundled first, then every other discovered family A–Z.
pub fn merge_fonts(native: Vec<String>, probed: Vec<String>) -> Vec<MonoFont> {
    let discovered = unique_families(native.into_iter().chain(probed));
    let bundled: Vec<String> = BUNDLED_MONO_FONTS
        .iter()
        .map(|f| f.to_lowercase())
        .collect();
    let mut extras: Vec<String> = discovered
        .into_iter()
        .filter(|f| !bundled.contains(&f.to_lowercase()))
        .collect();
    extras.sort_by(|a, b| recur_editor::text::locale_compare(a, b));
    BUNDLED_MONO_FONTS
        .iter()
        .map(|f| f.to_string())
        .chain(extras)
        .map(|family| MonoFont {
            label: family.clone(),
            family,
        })
        .collect()
}

/// `isMonospace`: narrow and wide glyph runs advance the same at 72px.
fn is_monospace(family: &str, cx: &App) -> bool {
    let ts = cx.text_system();
    let id = ts.resolve_font(&font(family.to_string()));
    let run = |ch: char| {
        ts.advance(id, px(72.), ch)
            .map(|s| f32::from(s.width) * 4.)
            .ok()
    };
    match (run('i'), run('M')) {
        (Some(i), Some(m)) => (i - m).abs() < MONO_EPS,
        _ => false,
    }
}

/// `familiesFromProbe`: candidates the text system has, that are monospace by name or metrics.
pub fn probe_families(cx: &App) -> Vec<String> {
    let installed = cx.text_system().all_font_names();
    let have = |name: &str| {
        installed.iter().any(|f| f == name)
            || recur_ui::fonts::FACES.iter().any(|(f, _, _)| *f == name)
    };
    MONO_CANDIDATES
        .iter()
        .filter(|name| have(name) && (looks_monospace_name(name) || is_monospace(name, cx)))
        .map(|s| s.to_string())
        .collect()
}

/// `listMonospaceFonts`.
pub fn list_monospace_fonts(services: &Services, cx: &mut App) -> Task<Vec<MonoFont>> {
    let services = services.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let native: Vec<String> = services
            .settings
            .list_monospace_fonts()
            .await
            .map(|l| l.into_iter().filter(|s| !s.trim().is_empty()).collect())
            .unwrap_or_default();
        let probed = cx.update(|cx| probe_families(cx));
        merge_fonts(native, probed)
    })
}

/// `editorFontStack` / `monoFontStack`: the family, then the system monospace stack
/// (`ui-monospace, 'SF Mono', Menlo, Monaco, Consolas, monospace`).
pub fn font_stack(face: &str) -> Family {
    let mut names = recur_editor::prefs::font_stack(face).into_iter();
    let first = names.next().unwrap_or_else(|| "SF Mono".into());
    Family::Stack(first.into(), names.map(Into::into).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_first_then_extras_a_to_z() {
        let got = merge_fonts(
            vec![
                "Menlo".into(),
                " jetbrains mono ".into(),
                "Fira Code".into(),
                "".into(),
            ],
            vec!["Courier".into(), "menlo".into()],
        );
        let names: Vec<&str> = got.iter().map(|f| f.family.as_str()).collect();
        assert_eq!(
            names,
            [
                "JetBrains Mono",
                "IBM Plex Mono",
                "Source Code Pro",
                "System Mono",
                "Courier",
                "Fira Code",
                "Menlo",
            ]
        );
        assert!(got.iter().all(|f| f.label == f.family));
    }

    #[test]
    fn mono_names() {
        for yes in [
            "JetBrainsMono NF",
            "Fira Code",
            "Cascadia Mono",
            "Source Code Pro",
            "Hack Nerd Font",
        ] {
            assert!(looks_monospace_name(yes), "{yes}");
        }
        for no in ["Fira Sans", "Code New Roman", "Helvetica", "Sudo"] {
            assert!(!looks_monospace_name(no), "{no}");
        }
    }

    #[test]
    fn stacks() {
        assert_eq!(font_stack("System Mono").primary(), "SF Mono");
        assert_eq!(font_stack("Fira Code").primary(), "Fira Code");
        assert_eq!(font_stack("plex").primary(), "IBM Plex Mono");
    }
}
