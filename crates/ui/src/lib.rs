//! Recur's design system on GPUI: theme tokens, fonts, icons, and shared widgets.
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
#[cfg(test)]
mod backdrop_blur_spec;
#[cfg(target_os = "macos")]
mod cg_raster;
pub mod color;
#[cfg(test)]
mod color_chooser_spec;
#[cfg(test)]
mod contained_panic_spec;
#[cfg(test)]
mod date_time_spec;
#[cfg(test)]
mod deferred_floor_spec;
pub mod field;
pub mod file_icons;
pub mod font_metrics;
pub mod fonts;
#[cfg(feature = "gallery")]
pub mod gallery;
pub mod guard;
pub mod icon;
pub mod icon_name;
pub mod isolated;
#[cfg(test)]
mod isolated_spec;
pub mod kbd;
mod kit;
pub mod layout_root;
pub mod logic;
pub mod motion;
#[cfg(test)]
mod nested_scroll_spec;
pub mod pane;
pub mod resize;
pub mod selection;
#[cfg(test)]
mod selection_spec;
pub mod set;
pub mod shadow;
pub mod style;
pub mod text;
pub mod theme;
pub mod tooltip;
pub mod widgets;

pub use assets::Assets;
pub use color::{Color, hex, rgba};
pub use icon::{Icon, icon, lucide_icon, provider_icon, svg_markup};
pub use icon_name::IconName;
pub use text::{LineHeight, Span, Text, TextStyled, span, text};
pub use theme::{Appearance, Theme, ThemeId, Tokens};

use gpui_kit::App;

/// Register fonts, install the theme and the shared widget state.
pub fn init(cx: &mut App) {
    text::clear_shaped_cache();
    // The faces are compiled in; a failure here means a corrupt bundle, which the font tests
    // catch. The app still runs on system fallbacks.
    let _ = fonts::register(cx);
    theme::init(cx);
    kit::sync_component_theme(cx);
}
