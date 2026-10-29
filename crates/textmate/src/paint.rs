//! Token metadata -> paint decision (`cm-tm-deco.ts` `classForMeta`).

use crate::tm_theme::{
    composite_css_color, decode_background, decode_font_style, decode_foreground,
    should_paint_token_background, token_foreground_readable,
};

/// What to paint for one token: colour-map indices and font style bits
/// (`tm_font::*`). The web UI expresses the same as `cm-tm-*` classes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TokenPaint {
    pub foreground: Option<u32>,
    pub background: Option<u32>,
    pub font_style: u32,
}

pub fn paint_for_meta(
    meta: u32,
    paint_background: bool,
    color_map: &[String],
    document_bgs: &[String],
    pane_bg: &str,
) -> TokenPaint {
    let fg = decode_foreground(meta);
    let bg = decode_background(meta);
    let color = |i: u32| {
        color_map
            .get(i as usize)
            .map(String::as_str)
            .filter(|c| !c.is_empty())
    };
    let fg_color = if fg > 0 { color(fg) } else { None };
    let bg_color = if bg > 0 { color(bg) } else { None };
    let paint_bg =
        paint_background && bg > 0 && should_paint_token_background(bg_color, document_bgs);
    let surface = match (paint_bg, bg_color) {
        (true, Some(c)) => composite_css_color(c, pane_bg),
        _ => pane_bg.to_string(),
    };
    TokenPaint {
        foreground: (fg > 0 && token_foreground_readable(fg_color, Some(&surface))).then_some(fg),
        background: paint_bg.then_some(bg),
        font_style: decode_font_style(meta),
    }
}

/// The pane fill tokens sit on: the `text` / `source` root fill, else the
/// editor background (`getActivePaneBackground`).
pub fn pane_background(
    scope: &str,
    editor_bg: &str,
    source_bg: Option<&str>,
    text_bg: Option<&str>,
) -> String {
    let text = crate::tm_theme::is_text_mate_text_scope(scope);
    match (text, text_bg, source_bg) {
        (true, Some(t), _) => t.to_string(),
        (false, _, Some(s)) => s.to_string(),
        _ => editor_bg.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    // cm-tm-deco.test.ts
    #[test]
    fn skips_the_editor_background_wash() {
        let map = s(&["", "#cccccc", "#1e1e1e"]);
        let meta = (1 << 15) | (2 << 24);
        let fg_only = TokenPaint {
            foreground: Some(1),
            background: None,
            font_style: 0,
        };
        assert_eq!(
            paint_for_meta(meta, true, &map, &s(&["#1e1e1e"]), "#1e1e1e"),
            fg_only
        );
        assert_eq!(
            paint_for_meta(meta, true, &map, &s(&["#000000"]), "#000000"),
            TokenPaint {
                background: Some(2),
                ..fg_only
            }
        );
        assert_eq!(
            paint_for_meta(meta, false, &map, &s(&["#000000"]), "#000000"),
            fg_only
        );
    }

    #[test]
    fn judges_contrast_against_the_pane_fill() {
        let map = s(&["", "#666666"]);
        let meta = 1 << 15;
        let bgs = s(&["#000000", "#555454"]);
        assert_eq!(
            paint_for_meta(meta, false, &map, &bgs, "#000000").foreground,
            Some(1)
        );
        assert_eq!(
            paint_for_meta(meta, false, &map, &bgs, "#555454").foreground,
            None
        );
    }

    #[test]
    fn pane_background_follows_root_scope() {
        assert_eq!(
            pane_background("text.html", "#1", Some("#s"), Some("#t")),
            "#t"
        );
        assert_eq!(
            pane_background("source.go", "#1", Some("#s"), Some("#t")),
            "#s"
        );
        assert_eq!(pane_background("source.go", "#1", None, Some("#t")), "#1");
    }
}
