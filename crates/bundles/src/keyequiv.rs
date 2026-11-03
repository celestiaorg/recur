//! `keyEquivalent` chord parsing, canonical form, and app-reserved chords.

use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

use crate::{Error, err};

/// Canonical modifier order for chord strings (Apple display order).
const MOD_ORDER: [&str; 5] = ["ctrl", "opt", "shift", "cmd", "fn"];

fn known_mod(tok: &str) -> Option<&'static str> {
    Some(match tok {
        "ctrl" | "control" => "ctrl",
        "opt" | "option" | "alt" => "opt",
        "shift" => "shift",
        "cmd" | "command" | "meta" => "cmd",
        "fn" | "function" => "fn",
        _ => return None,
    })
}

fn key_alias(tok: &str) -> Option<&'static str> {
    Some(match tok {
        "esc" => "escape",
        "return" | "ret" => "enter",
        "bs" => "backspace",
        "del" => "delete",
        "pgup" | "pageup" => "pageup",
        "pgdn" | "pagedown" => "pagedown",
        "arrowup" => "up",
        "arrowdown" => "down",
        "arrowleft" => "left",
        "arrowright" => "right",
        "spacebar" | " " => "space",
        "plus" => "+",
        "minus" => "-",
        "period" => ".",
        "comma" => ",",
        "slash" => "/",
        "backslash" => "\\",
        "backtick" | "grave" => "`",
        "bracketleft" => "[",
        "bracketright" => "]",
        "semicolon" => ";",
        "quote" => "'",
        "equal" => "=",
        _ => return None,
    })
}

static KNOWN_KEYS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut keys: HashSet<&'static str> = [
        "f1",
        "f2",
        "f3",
        "f4",
        "f5",
        "f6",
        "f7",
        "f8",
        "f9",
        "f10",
        "f11",
        "f12",
        "enter",
        "escape",
        "tab",
        "space",
        "up",
        "down",
        "left",
        "right",
        "home",
        "end",
        "pageup",
        "pagedown",
        "delete",
        "backspace",
        "+",
        "-",
        "=",
        ".",
        ",",
        "/",
        "\\",
        "`",
        "[",
        "]",
        ";",
        "'",
        "<",
        ">",
    ]
    .into_iter()
    .collect();
    keys.extend([
        "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r",
        "s", "t", "u", "v", "w", "x", "y", "z", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
    ]);
    keys
});

/// macOS app / workbench bindings bundles may not claim, keyed by canonical
/// chord. Labels match the shortcuts cheatsheet.
pub static RESERVED_APP_CHORDS: LazyLock<BTreeMap<&'static str, &'static str>> =
    LazyLock::new(|| {
        [
            ("shift+cmd+/", "Keyboard shortcuts"),
            ("cmd+k", "Go to"),
            ("shift+cmd+p", "Run command"),
            ("cmd+n", "New workspace"),
            ("cmd+b", "Toggle sidebar"),
            ("cmd+,", "Settings"),
            ("escape", "Back"),
            ("cmd+p", "Quick Open"),
            ("cmd+o", "Open in"),
            ("shift+cmd+o", "Go to symbol"),
            ("cmd+1", "Focus agent panel"),
            ("cmd+2", "Focus editor"),
            ("cmd+3", "Focus terminal"),
            ("cmd+l", "Add file / selection to chat"),
            ("opt+cmd+left", "Previous tab"),
            ("opt+cmd+right", "Next tab"),
            ("opt+cmd+up", "Previous workspace"),
            ("opt+cmd+down", "Next workspace"),
            ("opt+cmd+b", "Toggle left panel"),
            ("opt+shift+cmd+b", "Toggle right panel"),
            ("ctrl+`", "Toggle terminal"),
            ("ctrl+shift+`", "Maximize terminal"),
            ("cmd+t", "New tab"),
            ("cmd+w", "Close tab"),
            ("shift+cmd+t", "Reopen closed editor tab"),
            ("shift+cmd+backspace", "Stop agent"),
            ("cmd+.", "Cycle agent mode"),
            ("cmd+\\", "Split editor"),
            ("cmd+r", "Run…"),
            ("shift+cmd+r", "Stop run"),
            ("f5", "Reload from disk"),
            ("shift+cmd+v", "Cycle editor view"),
            ("ctrl+shift+v", "Cycle editor view"),
            ("opt+cmd+[", "Previous editor theme"),
            ("opt+cmd+]", "Next editor theme"),
            ("cmd+s", "Save"),
            ("cmd+f", "Find"),
            ("cmd+g", "Find next"),
            ("shift+cmd+g", "Find previous"),
            ("opt+cmd+g", "Go to line"),
            ("cmd+d", "Select next occurrence"),
            ("ctrl+shift+up", "Add cursor above"),
            ("ctrl+shift+down", "Add cursor below"),
            ("cmd+/", "Toggle comment"),
            ("f12", "Go to definition"),
            ("cmd+down", "Open file (Explorer)"),
            ("opt+cmd+n", "New file (Explorer)"),
            ("shift+cmd+n", "New folder (Explorer)"),
            ("enter", "Rename (Explorer)"),
            ("cmd+backspace", "Delete (Explorer)"),
            ("cmd+x", "Cut"),
            ("cmd+c", "Copy"),
            ("cmd+v", "Paste"),
            ("opt+cmd+c", "Copy path (Explorer)"),
            ("opt+shift+cmd+c", "Copy relative path (Explorer)"),
            ("opt+cmd+r", "Reveal in Finder"),
            ("opt+cmd+w", "Whole word (find)"),
            ("opt+cmd+z", "Revert diff hunk"),
        ]
        .into_iter()
        .collect()
    });

/// Normalizes an authoring chord like `cmd+shift+u` or `Opt+Cmd+F` to its
/// canonical form (modifiers in Apple order, lowercased key).
pub fn parse_key_equivalent(s: &str) -> Result<String, Error> {
    let s = s.trim();
    if s.is_empty() {
        return Err(err!("empty keyEquivalent"));
    }
    let parts: Vec<&str> = s.split('+').collect();
    let mut mods = HashSet::new();
    let mut key = String::new();
    for (i, p) in parts.iter().enumerate() {
        let mut tok = p.trim().to_lowercase();
        if tok.is_empty() {
            return Err(err!("empty token in {:?}", s));
        }
        if let Some(m) = known_mod(&tok) {
            mods.insert(m);
            continue;
        }
        if i != parts.len() - 1 {
            return Err(err!("{:?}: unknown modifier {:?}", s, p));
        }
        if let Some(alias) = key_alias(&tok) {
            tok = alias.to_string();
        }
        if !KNOWN_KEYS.contains(tok.as_str()) {
            return Err(err!("{:?}: unknown key {:?}", s, p));
        }
        key = tok;
    }
    if key.is_empty() {
        return Err(err!("{:?}: missing key (modifier alone is not a chord)", s));
    }
    let mut out: Vec<&str> = MOD_ORDER
        .iter()
        .copied()
        .filter(|m| mods.contains(m))
        .collect();
    out.push(&key);
    Ok(out.join("+"))
}

/// Whether a canonical chord is owned by the app.
pub fn is_reserved_app_chord(canonical: &str) -> bool {
    RESERVED_APP_CHORDS.contains_key(canonical)
}

/// Turns a canonical chord into macOS glyph form (`⌘⇧U`).
pub fn format_key_equivalent_display(canonical: &str) -> String {
    if canonical.is_empty() {
        return String::new();
    }
    let mut b = String::new();
    for p in canonical.split('+') {
        let glyph = match p {
            "cmd" => "⌘",
            "opt" => "⌥",
            "shift" => "⇧",
            "ctrl" => "⌃",
            "fn" => "fn",
            "escape" => "esc",
            "enter" => "↵",
            "backspace" => "⌫",
            "delete" => "⌦",
            "up" => "↑",
            "down" => "↓",
            "left" => "←",
            "right" => "→",
            "tab" => "⇥",
            "space" => "␣",
            _ => {
                if p.len() == 1 || (p.starts_with('f') && p.len() <= 3) {
                    b.push_str(&p.to_uppercase());
                } else {
                    b.push_str(p);
                }
                continue;
            }
        };
        b.push_str(glyph);
    }
    b
}

/// Reserved chords in sorted order (stable test/docs output).
pub fn sorted_reserved_chords() -> Vec<&'static str> {
    RESERVED_APP_CHORDS.keys().copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_key_equivalent_canonical() {
        for (input, want) in [
            ("cmd+shift+u", "shift+cmd+u"),
            ("Shift+Cmd+U", "shift+cmd+u"),
            ("opt+cmd+f", "opt+cmd+f"),
            ("alt+meta+f", "opt+cmd+f"),
            ("ctrl+shift+`", "ctrl+shift+`"),
            ("f5", "f5"),
            ("escape", "escape"),
            ("esc", "escape"),
            ("ctrl+<", "ctrl+<"),
            ("ctrl+shift+,", "ctrl+shift+,"),
            ("cmd+opt+shift+c", "opt+shift+cmd+c"),
        ] {
            assert_eq!(parse_key_equivalent(input).unwrap(), want, "{input}");
        }
    }

    #[test]
    fn parse_key_equivalent_errors() {
        for input in ["", "mod+u", "cmd+", "cmd", "cmd+foo", "++u"] {
            assert!(parse_key_equivalent(input).is_err(), "{input:?}");
        }
    }

    #[test]
    fn reserved_app_chords_canonical() {
        for chord in RESERVED_APP_CHORDS.keys() {
            assert_eq!(parse_key_equivalent(chord).unwrap(), *chord);
        }
    }

    #[test]
    fn display() {
        assert_eq!(format_key_equivalent_display("shift+cmd+u"), "⇧⌘U");
        assert_eq!(format_key_equivalent_display("opt+cmd+f"), "⌥⌘F");
        assert_eq!(format_key_equivalent_display("opt+cmd+up"), "⌥⌘↑");
        assert_eq!(format_key_equivalent_display("ctrl+opt+up"), "⌃⌥↑");
        assert_eq!(format_key_equivalent_display("ctrl+<"), "⌃<");
    }

    #[test]
    fn sorted_reserved() {
        let got = sorted_reserved_chords();
        let mut want = got.clone();
        want.sort();
        assert_eq!(got, want);
        assert_eq!(got.len(), 58);
    }
}
