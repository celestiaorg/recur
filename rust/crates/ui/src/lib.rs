//! Recur's design system on GPUI: theme tokens, fonts, icons, and shared widgets matching the
//! web UI.
//!
//! Call [`init`] once after `gpui_kit::init`, and give the application [`Assets`]:
//!
//! ```no_run
//! gpui_kit::application().with_assets(recur_ui::Assets).run(|cx| {
//!     gpui_kit::init(cx);
//!     recur_ui::init(cx);
//! });
//! ```

pub mod assets;
pub mod color;
pub mod file_icons;
pub mod font_metrics;
pub mod fonts;
pub mod icon;
pub mod icon_name;
mod kit;
pub mod text;
pub mod theme;

pub use assets::Assets;
pub use color::{Color, hex, rgba};
pub use icon::{Icon, icon, lucide_icon, provider_icon, svg_markup};
pub use icon_name::IconName;
pub use text::{LineHeight, Span, Text, TextStyled, span, text};
pub use theme::{Appearance, Theme, ThemeId, Tokens};

use gpui_kit::App;

/// Register fonts, install the theme and the shared widget state.
pub fn init(cx: &mut App) {
    // The faces are compiled in; a failure here means a corrupt bundle, which the font tests
    // catch. The app still runs on system fallbacks.
    let _ = fonts::register(cx);
    theme::init(cx);
    kit::sync_component_theme(cx);
}
