//! `expandEmojiShortcodes` from `lib/emoji.ts`: GitHub gemoji `:name:` aliases.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::js::Re;

fn shortcodes() -> &'static HashMap<String, String> {
    static MAP: OnceLock<HashMap<String, String>> = OnceLock::new();
    MAP.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../../frontend/src/lib/emoji-shortcodes.json"
        ))
        .unwrap_or_default()
    })
}

fn shortcode_re() -> &'static Re {
    static RE: OnceLock<Re> = OnceLock::new();
    RE.get_or_init(|| Re::new(r":([a-z0-9_+-]+):", "gi"))
}

/// `emojiForShortcode`.
pub fn emoji_for_shortcode(name: &str) -> Option<&'static str> {
    shortcodes().get(&name.to_lowercase()).map(String::as_str)
}

fn replace_shortcodes(src: &str) -> String {
    shortcode_re().replace_with(src, |c| {
        emoji_for_shortcode(c.str(1)).map_or_else(|| c.full().to_string(), str::to_string)
    })
}

/// `expandEmojiShortcodes(src, { skipCode })`: replace completed `:name:` tokens, leaving
/// unknown names (and, with `skip_code`, code spans and fences) alone.
pub fn expand_emoji_shortcodes(src: &str, skip_code: bool) -> String {
    if src.is_empty() || !src.contains(':') {
        return src.to_string();
    }
    if !skip_code {
        return replace_shortcodes(src);
    }
    static CODE: OnceLock<Re> = OnceLock::new();
    let code = CODE.get_or_init(|| Re::new(r"(```[\s\S]*?```|`[^`]+`)", ""));
    code.split(src)
        .into_iter()
        .enumerate()
        .map(|(i, part)| {
            let part = part.unwrap_or("");
            if i % 2 == 1 {
                part.to_string()
            } else {
                replace_shortcodes(part)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_outside_code_only() {
        assert_eq!(
            expand_emoji_shortcodes("Ship it :rocket: and `:smile:` :nope:", true),
            "Ship it 🚀 and `:smile:` :nope:"
        );
        assert_eq!(expand_emoji_shortcodes(":ROCKET:", false), "🚀");
    }
}
