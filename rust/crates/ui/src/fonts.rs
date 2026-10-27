//! The UI fonts: the exact families and weights `frontend/index.html` loads, bundled as static
//! instances (see `assets/fonts/fetch.sh`), and the CSS font stacks from `_tokens.scss`.

use gpui_kit::{App, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, SharedString, font};
use std::borrow::Cow;

/// `--ui`.
pub const UI: &str = "Inter";
/// `--mono`.
pub const MONO: &str = "JetBrains Mono";
/// `--disp`.
pub const DISPLAY: &str = "Space Grotesk";
/// `system-ui` / `-apple-system` (keycaps): GPUI's name for SF on macOS.
pub const SYSTEM: &str = ".SystemUIFont";

/// The family `system-ui` resolves to. macOS: SF through [`SYSTEM`], as WebKit draws it.
/// Linux: fontconfig's match for `system-ui`, the face WebKitGTK (and so the Go app and the
/// Mermaid labels it measures) uses; GPUI's own `.SystemUIFont` there is IBM Plex Sans.
pub fn system_family() -> SharedString {
    #[cfg(target_os = "linux")]
    {
        static FAMILY: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        if let Some(f) = FAMILY.get_or_init(fontconfig::system_ui) {
            return f.clone().into();
        }
    }
    SYSTEM.into()
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod fontconfig {
    use fontconfig_sys::*;
    use std::ffi::CStr;

    /// `fc-match system-ui`: the first family of the best match after config substitution.
    pub fn system_ui() -> Option<String> {
        // SAFETY: fontconfig's documented match sequence; every object created here is
        // destroyed here, and the family string is copied before its pattern goes.
        unsafe {
            let config = FcInitLoadConfigAndFonts();
            if config.is_null() {
                return None;
            }
            let pattern = FcNameParse(c"system-ui".as_ptr().cast());
            let mut family = None;
            if !pattern.is_null() {
                FcConfigSubstitute(config, pattern, FcMatchPattern);
                FcDefaultSubstitute(pattern);
                let mut result = FcResultMatch;
                let found = FcFontMatch(config, pattern, &mut result);
                if !found.is_null() {
                    let mut name: *mut FcChar8 = std::ptr::null_mut();
                    if FcPatternGetString(found, constants::FC_FAMILY.as_ptr(), 0, &mut name)
                        == FcResultMatch
                        && !name.is_null()
                    {
                        family = Some(CStr::from_ptr(name.cast()).to_string_lossy().into_owned());
                    }
                    FcPatternDestroy(found);
                }
                FcPatternDestroy(pattern);
            }
            FcConfigDestroy(config);
            family
        }
    }
}

/// A CSS `font-family` stack used by the web UI.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    /// `'Inter', system-ui, sans-serif`
    Ui,
    /// `'JetBrains Mono', ui-monospace, 'SF Mono', Menlo, monospace`
    Mono,
    /// `'Space Grotesk', var(--ui)`
    Display,
    /// `system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif`
    System,
    /// Any other stack: first family plus fallbacks, in CSS order.
    Stack(SharedString, Vec<SharedString>),
}

impl Family {
    pub fn primary(&self) -> SharedString {
        match self {
            Family::Ui => UI.into(),
            Family::Mono => MONO.into(),
            Family::Display => DISPLAY.into(),
            Family::System => system_family(),
            Family::Stack(first, _) => first.clone(),
        }
    }

    fn fallbacks(&self) -> Vec<String> {
        match self {
            Family::Ui => vec![system_family().to_string()],
            Family::Mono => vec!["SF Mono".into(), "Menlo".into()],
            Family::Display => vec![UI.into(), system_family().to_string()],
            Family::System => vec![],
            Family::Stack(_, rest) => rest.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// The GPUI font for this stack at a CSS weight.
    pub fn font(&self, weight: f32, italic: bool, features: FontFeatures) -> Font {
        let mut f = font(self.primary());
        f.weight = FontWeight(weight);
        f.style = if italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };
        f.features = features;
        let fallbacks = self.fallbacks();
        if !fallbacks.is_empty() {
            f.fallbacks = Some(FontFallbacks::from_fonts(fallbacks));
        }
        f
    }
}

/// Every bundled face: the families and weights `frontend/index.html` requests from Google
/// Fonts. Inter, JetBrains Mono and Space Grotesk are the chrome; IBM Plex Mono and Source
/// Code Pro are editor/terminal picks (`lib/mono-fonts.ts`).
pub const FACES: &[(&str, f32, &[u8])] = &[
    (
        "Inter",
        400.,
        include_bytes!("../assets/fonts/Inter-400.ttf"),
    ),
    (
        "Inter",
        450.,
        include_bytes!("../assets/fonts/Inter-450.ttf"),
    ),
    (
        "Inter",
        500.,
        include_bytes!("../assets/fonts/Inter-500.ttf"),
    ),
    (
        "Inter",
        600.,
        include_bytes!("../assets/fonts/Inter-600.ttf"),
    ),
    (
        "Space Grotesk",
        500.,
        include_bytes!("../assets/fonts/SpaceGrotesk-500.ttf"),
    ),
    (
        "Space Grotesk",
        600.,
        include_bytes!("../assets/fonts/SpaceGrotesk-600.ttf"),
    ),
    (
        "Space Grotesk",
        700.,
        include_bytes!("../assets/fonts/SpaceGrotesk-700.ttf"),
    ),
    (
        "JetBrains Mono",
        400.,
        include_bytes!("../assets/fonts/JetBrainsMono-400.ttf"),
    ),
    (
        "JetBrains Mono",
        500.,
        include_bytes!("../assets/fonts/JetBrainsMono-500.ttf"),
    ),
    (
        "JetBrains Mono",
        600.,
        include_bytes!("../assets/fonts/JetBrainsMono-600.ttf"),
    ),
    (
        "JetBrains Mono",
        700.,
        include_bytes!("../assets/fonts/JetBrainsMono-700.ttf"),
    ),
    (
        "IBM Plex Mono",
        400.,
        include_bytes!("../assets/fonts/IBMPlexMono-400.ttf"),
    ),
    (
        "IBM Plex Mono",
        500.,
        include_bytes!("../assets/fonts/IBMPlexMono-500.ttf"),
    ),
    (
        "IBM Plex Mono",
        600.,
        include_bytes!("../assets/fonts/IBMPlexMono-600.ttf"),
    ),
    (
        "IBM Plex Mono",
        700.,
        include_bytes!("../assets/fonts/IBMPlexMono-700.ttf"),
    ),
    (
        "Source Code Pro",
        400.,
        include_bytes!("../assets/fonts/SourceCodePro-400.ttf"),
    ),
    (
        "Source Code Pro",
        500.,
        include_bytes!("../assets/fonts/SourceCodePro-500.ttf"),
    ),
    (
        "Source Code Pro",
        600.,
        include_bytes!("../assets/fonts/SourceCodePro-600.ttf"),
    ),
    (
        "Source Code Pro",
        700.,
        include_bytes!("../assets/fonts/SourceCodePro-700.ttf"),
    ),
];

/// Register every bundled face with the GPUI text system.
pub fn register(cx: &mut App) -> gpui_kit::Result<()> {
    cx.text_system()
        .add_fonts(FACES.iter().map(|(_, _, b)| Cow::Borrowed(*b)).collect())
}

/// Font weights the SCSS uses. CSS matching picks the nearest bundled face exactly as the
/// browser does (550 → 600 for Inter; 400 → 500 for Space Grotesk).
pub mod weight {
    pub const REGULAR: f32 = 400.;
    pub const BOOK: f32 = 450.;
    pub const MEDIUM: f32 = 500.;
    pub const MEDIUM_PLUS: f32 = 550.;
    pub const SEMIBOLD: f32 = 600.;
    pub const SEMIBOLD_PLUS: f32 = 650.;
    pub const BOLD: f32 = 700.;
}

/// `font-variant-numeric: tabular-nums`.
pub fn tabular_nums() -> FontFeatures {
    FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]))
}

/// CSS Fonts 4 weight matching over the bundled faces of `family`: the index into [`FACES`]
/// the browser would pick for `weight`, or `None` for a family that is not bundled.
pub fn match_face(family: &str, weight: f32) -> Option<usize> {
    let faces: Vec<(usize, f32)> = FACES
        .iter()
        .enumerate()
        .filter(|(_, f)| f.0 == family)
        .map(|(i, f)| (i, f.1))
        .collect();
    if faces.is_empty() {
        return None;
    }
    let exact = faces.iter().find(|f| f.1 == weight);
    if let Some(f) = exact {
        return Some(f.0);
    }
    let up = |lo: f32, hi: f32| {
        faces
            .iter()
            .filter(|f| f.1 > lo && f.1 <= hi)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|f| f.0)
    };
    let down = |hi: f32| {
        faces
            .iter()
            .filter(|f| f.1 < hi)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|f| f.0)
    };
    if (400. ..=500.).contains(&weight) {
        up(weight, 500.)
            .or_else(|| down(weight))
            .or_else(|| up(500., f32::MAX))
    } else if weight < 400. {
        down(weight).or_else(|| up(weight, f32::MAX))
    } else {
        up(weight, f32::MAX).or_else(|| down(weight))
    }
}

#[cfg(test)]
mod tests {

    /// The face `fc-match system-ui` names (when fontconfig's tools are installed).
    #[cfg(target_os = "linux")]
    #[test]
    fn system_ui_is_fontconfigs_match() {
        let Ok(out) = std::process::Command::new("fc-match")
            .args(["-f", "%{family[0]}", "system-ui"])
            .output()
        else {
            return;
        };
        let expected = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(!expected.is_empty());
        assert_eq!(system_family().as_ref(), expected);
        assert_eq!(Family::System.primary().as_ref(), expected);
    }

    use super::*;

    fn w(family: &str, weight: f32) -> f32 {
        FACES[match_face(family, weight).unwrap()].1
    }

    #[test]
    fn matches_css_weight_rules() {
        assert_eq!(w("Inter", 450.), 450.);
        assert_eq!(w("Inter", 550.), 600.);
        assert_eq!(w("Inter", 700.), 600.);
        assert_eq!(w("Inter", 300.), 400.);
        assert_eq!(w("Space Grotesk", 400.), 500.);
        assert_eq!(w("JetBrains Mono", 650.), 700.);
        assert_eq!(w("IBM Plex Mono", 475.), 500.);
        assert!(match_face("Menlo", 400.).is_none());
    }
}
