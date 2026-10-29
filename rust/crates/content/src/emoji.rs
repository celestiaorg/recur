//! `expandEmojiShortcodes` from `lib/emoji.ts`: GitHub gemoji `:name:` aliases.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::js::Re;

const SHORTCODES_JSON: &str = include_str!("../assets/emoji-shortcodes.json");

fn shortcodes() -> &'static HashMap<String, String> {
    static MAP: OnceLock<HashMap<String, String>> = OnceLock::new();
    MAP.get_or_init(|| shortcode_entries().iter().cloned().collect())
}

struct Ordered(Vec<(String, String)>);

impl<'de> serde::Deserialize<'de> for Ordered {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Ordered;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a shortcode map")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut m: A,
            ) -> Result<Ordered, A::Error> {
                let mut out = Vec::new();
                while let Some(kv) = m.next_entry::<String, String>()? {
                    out.push(kv);
                }
                Ok(Ordered(out))
            }
        }
        d.deserialize_map(V)
    }
}

/// `Object.entries(EMOJI_SHORTCODES)`: integer-like keys ascending first, then file order.
pub fn shortcode_entries() -> &'static [(String, String)] {
    static LIST: OnceLock<Vec<(String, String)>> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut list = serde_json::from_str::<Ordered>(SHORTCODES_JSON)
            .map(|o| o.0)
            .unwrap_or_default();
        let index = |k: &str| {
            (!k.is_empty()
                && k.bytes().all(|b| b.is_ascii_digit())
                && (k == "0" || !k.starts_with('0')))
            .then(|| k.parse::<u64>().ok())
            .flatten()
        };
        list.sort_by_key(|(k, _)| index(k).map_or((1, 0), |n| (0, n)));
        list
    })
}

/// `expandEmojiShortcodesAt`: expand completed shortcodes and shift `caret` (a byte offset)
/// so it stays after the same logical point. `None` when nothing changed.
pub fn expand_emoji_shortcodes_at(src: &str, caret: usize) -> Option<(String, usize)> {
    if !src.contains(':') {
        return None;
    }
    let mut next = caret as isize;
    let text = shortcode_re().replace_with(src, |c| {
        let Some(emoji) = emoji_for_shortcode(c.str(1)) else {
            return c.full().to_string();
        };
        if c.index() < caret {
            next += emoji.len() as isize - c.full().len() as isize;
        }
        emoji.to_string()
    });
    (text != src).then(|| (text, next.max(0) as usize))
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

    #[test]
    fn ordered_entries_and_caret_shift() {
        let e = shortcode_entries();
        assert_eq!(e[0].0, "100");
        assert_eq!(e[1].0, "1234");
        assert_eq!(e[2].0, "grinning");
        let (t, c) = expand_emoji_shortcodes_at("go :rocket: now", 12).unwrap();
        assert_eq!(t, "go 🚀 now");
        assert_eq!(c, 12 + "🚀".len() - ":rocket:".len());
        assert!(expand_emoji_shortcodes_at("no :nope: here", 3).is_none());
    }
}
