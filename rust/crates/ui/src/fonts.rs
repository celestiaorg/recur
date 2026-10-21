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
/// `system-ui` / `-apple-system` (keycaps).
pub const SYSTEM: &str = ".SystemUIFont";

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
            Family::System => SYSTEM.into(),
            Family::Stack(first, _) => first.clone(),
        }
    }

    fn fallbacks(&self) -> Vec<String> {
        match self {
            Family::Ui => vec![SYSTEM.into()],
            Family::Mono => vec!["SF Mono".into(), "Menlo".into()],
            Family::Display => vec![UI.into(), SYSTEM.into()],
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
