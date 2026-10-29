//! Keycap text in the system UI font,
//! weight 500, tracking .02em, with modifier glyphs split into `.kbd-mod` runs.

use crate::fonts::weight;
use crate::text::{Span, Text, TextStyled, span};
use gpui_kit::SharedString;

/// Modifier and special keys that get optical sizing against Latin letters.
fn is_mod(c: char) -> bool {
    matches!(
        c,
        '⌘' | '⌥' | '⇧' | '⌃' | '↵' | '⏎' | '↩' | '←' | '→' | '↑' | '↓' | '⎋' | '⌫'
    )
}

/// One piece of a shortcut label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutPart {
    pub text: String,
    pub modifier: bool,
}

/// Split a shortcut so modifier glyphs can be styled separately: each modifier is its own
/// part, consecutive other characters merge.
pub fn shortcut_parts(s: &str) -> Vec<ShortcutPart> {
    let mut out: Vec<ShortcutPart> = Vec::new();
    for ch in s.chars() {
        let modifier = is_mod(ch);
        match out.last_mut() {
            Some(last) if !last.modifier && !modifier => last.text.push(ch),
            _ => out.push(ShortcutPart {
                text: ch.to_string(),
                modifier,
            }),
        }
    }
    out
}

/// `<Kbd text>`: a `.kbd` span. Size and color inherit; set them on the result.
pub fn kbd(label: impl Into<SharedString>) -> Text {
    let label: SharedString = label.into();
    let spans: Vec<Span> = shortcut_parts(&label)
        .into_iter()
        // `.kbd-mod { font-size: 1em; line-height: 1; font-weight: inherit }`: same face and
        // size as the cap; its line-height never exceeds the cap's strut.
        .map(|p| span(p.text))
        .collect();
    Text::from_spans(spans)
        .system()
        .weight(weight::MEDIUM)
        .tracking(0.02)
        .nowrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(s: &str) -> Vec<(String, bool)> {
        shortcut_parts(s)
            .into_iter()
            .map(|p| (p.text, p.modifier))
            .collect()
    }

    fn p(t: &str, m: bool) -> (String, bool) {
        (t.to_string(), m)
    }

    #[test]
    fn splits_modifiers_from_letters() {
        assert_eq!(
            parts("⌘⇧P"),
            vec![p("⌘", true), p("⇧", true), p("P", false)]
        );
        assert_eq!(parts("Ctrl+K"), vec![p("Ctrl+K", false)]);
        assert_eq!(parts("⌥←"), vec![p("⌥", true), p("←", true)]);
        assert_eq!(parts("esc"), vec![p("esc", false)]);
        assert!(parts("").is_empty());
    }
}
