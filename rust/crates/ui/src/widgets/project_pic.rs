//! `ProjectPic.svelte` and `lib/project-pic.ts`: a project avatar that is a letter, a Lucide
//! icon or an image, on the project color.

use crate::assets::lucide_entry;
use crate::color::{Color, hex};
use crate::fonts::weight;
use crate::icon::lucide_icon;
use crate::text::{TextStyled, text};
use crate::theme::Theme;
use gpui_kit::{
    App, Image, ImageFormat, IntoElement, ObjectFit, ParentElement, RenderOnce, Styled,
    StyledImage, Window, div, img, px,
};
use std::sync::Arc;

pub const LUCIDE_PIC_PREFIX: &str = "lucide:";
pub const DEFAULT_LUCIDE_PIC: &str = "lucide:box";

/// `ParsedProjectPic`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedPic {
    Letter(String),
    Icon(String),
    /// A `data:image/…` URL.
    Image(String),
}

/// `parseProjectPic`.
pub fn parse_project_pic(pic: &str, fallback: &str) -> ParsedPic {
    let t = pic.trim();
    if t.starts_with("data:image/") {
        return ParsedPic::Image(t.into());
    }
    if t.to_lowercase().starts_with(LUCIDE_PIC_PREFIX) {
        let name = t[LUCIDE_PIC_PREFIX.len()..].trim().to_lowercase();
        if !name.is_empty() {
            return ParsedPic::Icon(name);
        }
    }
    if !t.is_empty() {
        return ParsedPic::Letter(t.chars().take(2).collect());
    }
    ParsedPic::Letter(fallback.into())
}

/// `formatLucidePic`.
pub fn format_lucide_pic(name: &str) -> String {
    format!("{LUCIDE_PIC_PREFIX}{}", name.trim().to_lowercase())
}

/// `isLetterPic`.
pub fn is_letter_pic(pic: &str) -> bool {
    matches!(parse_project_pic(pic, "?"), ParsedPic::Letter(_))
}

/// `projectPicFromName` (`lib/colors.ts`): the first character, uppercased.
pub fn project_pic_from_name(name: &str) -> String {
    name.trim()
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default()
}

/// `letterFromName`.
pub fn letter_from_name(name: &str, fallback: &str) -> String {
    let p = project_pic_from_name(name);
    if p.is_empty() { fallback.into() } else { p }
}

/// `projectColors` (`lib/colors.ts`; keep in sync with `services.go`).
pub const PROJECT_COLORS: [&str; 5] = [
    "var(--cyan)",
    "var(--amber)",
    "var(--indigo)",
    "var(--emerald)",
    "var(--coral)",
];

/// `isProjectPresetColor`.
pub fn is_project_preset_color(color: &str) -> bool {
    PROJECT_COLORS.contains(&color)
}

const DARK_FG: Color = hex(0x0a0c10);
const LIGHT_FG: Color = hex(0xf4f1ea);

/// `fgOnBackground`: preset swatches always take dark ink; a custom color flips to light ink
/// only when it is truly dark (relative luminance ≤ 0.12).
pub fn fg_on_background(color: &str, theme: &Theme) -> Color {
    if color.is_empty() || is_project_preset_color(color) {
        return DARK_FG;
    }
    match theme.resolve_css_color(color) {
        Some(c) if c.luminance() <= 0.12 => LIGHT_FG,
        _ => DARK_FG,
    }
}

/// Box metrics of the class a view puts on the avatar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PicBox {
    pub size: f32,
    pub radius: f32,
    pub font_size: f32,
}

impl PicBox {
    /// `.nt-pic` (new task, schedule editor).
    pub const NT: PicBox = PicBox {
        size: 18.,
        radius: 5.,
        font_size: 9.,
    };
    /// `.pal-row .pic .proj-pic` (command palette).
    pub const PALETTE: PicBox = PicBox {
        size: 18.,
        radius: 5.,
        font_size: 9.,
    };
}

#[derive(IntoElement)]
pub struct ProjectPic {
    pic: String,
    color: String,
    name: String,
    boxed: PicBox,
}

/// `<ProjectPic pic color name class>`.
pub fn project_pic(
    pic: impl Into<String>,
    color: impl Into<String>,
    name: impl Into<String>,
    boxed: PicBox,
) -> ProjectPic {
    ProjectPic {
        pic: pic.into(),
        color: color.into(),
        name: name.into(),
        boxed,
    }
}

fn data_url_image(url: &str) -> Option<Arc<Image>> {
    let (meta, data) = url.strip_prefix("data:")?.split_once(',')?;
    let format = match meta.split(';').next()? {
        "image/png" => ImageFormat::Png,
        "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        "image/svg+xml" => ImageFormat::Svg,
        "image/bmp" => ImageFormat::Bmp,
        _ => return None,
    };
    let bytes = if meta.ends_with(";base64") {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(data.trim())
            .ok()?
    } else {
        data.as_bytes().to_vec()
    };
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

impl ProjectPic {
    fn render_view(self, _: &mut Window, cx: &mut App) -> impl IntoElement + use<> {
        let theme = Theme::of(cx);
        let parsed = parse_project_pic(&self.pic, &letter_from_name(&self.name, "?"));
        let bg = theme.resolve_css_color(&self.color);
        let fg = fg_on_background(&self.color, theme);
        let b = self.boxed;
        let mut el = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .size(px(b.size))
            .rounded(px(b.radius));
        if let Some(bg) = bg {
            el = el.bg(bg);
        }
        match parsed {
            ParsedPic::Image(src) => match data_url_image(&src) {
                Some(image) => el.child(
                    img(image)
                        .size_full()
                        .rounded(px(b.radius))
                        .object_fit(ObjectFit::Cover),
                ),
                None => el,
            },
            ParsedPic::Icon(name) => {
                // `.proj-pic svg { width: 62%; height: 62% }`; unknown names draw nothing.
                let d = b.size * 0.62;
                if lucide_entry(&name).is_some() {
                    el.child(lucide_icon(&name).size(px(d)).color(fg))
                } else {
                    el
                }
            }
            ParsedPic::Letter(letter) => el.child(
                text(letter)
                    .mono()
                    .weight(weight::BOLD)
                    .size(b.font_size)
                    .leading(1.)
                    .color(fg)
                    .nowrap(),
            ),
        }
    }
}

impl RenderOnce for ProjectPic {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        crate::guard::component(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeId;

    #[test]
    fn parses_like_project_pic_ts() {
        assert_eq!(parse_project_pic("  ", "R"), ParsedPic::Letter("R".into()));
        assert_eq!(
            parse_project_pic("Abc", "?"),
            ParsedPic::Letter("Ab".into())
        );
        assert_eq!(
            parse_project_pic("LUCIDE: Rocket ", "?"),
            ParsedPic::Icon("rocket".into())
        );
        assert_eq!(
            parse_project_pic("lucide:", "?"),
            ParsedPic::Letter("lu".into())
        );
        assert_eq!(
            parse_project_pic("data:image/png;base64,AAAA", "?"),
            ParsedPic::Image("data:image/png;base64,AAAA".into())
        );
        assert_eq!(letter_from_name("émile", "?"), "É");
        assert_eq!(letter_from_name("", "?"), "?");
        assert_eq!(format_lucide_pic(" Box"), "lucide:box");
        assert!(is_letter_pic("x") && !is_letter_pic("lucide:box"));
    }

    #[test]
    fn ink_follows_fg_on_background() {
        let t = Theme::new(ThemeId::Charcoal);
        assert_eq!(fg_on_background("var(--cyan)", &t), DARK_FG);
        assert_eq!(fg_on_background("", &t), DARK_FG);
        assert_eq!(fg_on_background("#1e2a3a", &t), LIGHT_FG);
        assert_eq!(fg_on_background("#f2c94c", &t), DARK_FG);
        assert_eq!(fg_on_background("var(--bg)", &t), LIGHT_FG);
    }

    #[test]
    fn decodes_data_urls() {
        assert!(data_url_image("data:image/png;base64,iVBORw0KGgo=").is_some());
        assert!(data_url_image("data:text/plain,hi").is_none());
    }
}
