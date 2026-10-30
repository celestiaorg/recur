//! The UI fonts: the families and weights the app uses, bundled as static instances (see
//! `assets/fonts/fetch.sh`), and the font stacks.

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
/// Linux: fontconfig's match for `system-ui`, the face WebKitGTK (and so the
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

/// CSS `ui-monospace`: WebKit's monospaced system font (SF Mono) on macOS, whose hidden family
/// a CoreText cascade entry reaches by name. WebKitGTK has no `ui-monospace`.
#[cfg(target_os = "macos")]
const UI_MONOSPACE: &str = ".SF NS Mono";
#[cfg(not(target_os = "macos"))]
const UI_MONOSPACE: &str = "";

/// A CSS `font-family` stack.
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
            Family::Mono => [UI_MONOSPACE, "SF Mono", "Menlo"]
                .into_iter()
                .filter(|f| !f.is_empty())
                .map(Into::into)
                .collect(),
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

/// Every bundled face. Inter, JetBrains Mono and Space Grotesk are the chrome; IBM Plex Mono
/// and Source Code Pro are editor/terminal picks.
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// The face `fc-match system-ui` names (when fontconfig's tools are installed).
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
}
