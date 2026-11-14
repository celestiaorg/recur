//! Bundle `keyEquivalent` parsing and display for the editor (the bundle
//! side is `recur_bundles::keyequiv`).

use gpui_kit::Keystroke;

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

fn key_alias(tok: &str) -> &str {
    match tok {
        "esc" => "escape",
        "return" | "ret" => "enter",
        "bs" => "backspace",
        "del" => "delete",
        "pgup" => "pageup",
        "pgdn" => "pagedown",
        "arrowup" => "up",
        "arrowdown" => "down",
        "arrowleft" => "left",
        "arrowright" => "right",
        "spacebar" => "space",
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
        other => other,
    }
}

fn is_function_key(k: &str) -> bool {
    k.strip_prefix('f')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=12).contains(&n) && !k[1..].starts_with('0'))
}

/// `/^f\d{1,2}$/`.
fn fn_key_shape(k: &str) -> bool {
    k.strip_prefix('f')
        .is_some_and(|d| (1..=2).contains(&d.len()) && d.bytes().all(|b| b.is_ascii_digit()))
}

fn known_key(k: &str) -> bool {
    let single = k.chars().count() == 1
        && k.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    single
        || is_function_key(k)
        || matches!(
            k,
            "enter"
                | "escape"
                | "tab"
                | "space"
                | "up"
                | "down"
                | "left"
                | "right"
                | "home"
                | "end"
                | "pageup"
                | "pagedown"
                | "delete"
                | "backspace"
                | "+"
                | "-"
                | "="
                | "."
                | ","
                | "/"
                | "\\"
                | "`"
                | "["
                | "]"
                | ";"
                | "'"
                | "<"
                | ">"
        )
}

/// Reserved app chords (canonical). Keep in sync with
/// `recur_bundles::keyequiv::RESERVED_APP_CHORDS`.
pub const RESERVED_APP_CHORDS: &[&str] = &[
    "shift+cmd+/",
    "cmd+k",
    "shift+cmd+p",
    "cmd+n",
    "cmd+b",
    "cmd+,",
    "escape",
    "cmd+p",
    "cmd+o",
    "shift+cmd+o",
    "cmd+1",
    "cmd+2",
    "cmd+3",
    "cmd+9",
    "cmd+l",
    "opt+cmd+left",
    "opt+cmd+right",
    "opt+cmd+up",
    "opt+cmd+down",
    "opt+cmd+b",
    "opt+shift+cmd+b",
    "ctrl+`",
    "ctrl+shift+`",
    "cmd+t",
    "cmd+w",
    "shift+cmd+t",
    "shift+cmd+backspace",
    "cmd+.",
    "cmd+\\",
    "cmd+r",
    "shift+cmd+r",
    "f5",
    "shift+cmd+v",
    "ctrl+shift+v",
    "opt+cmd+[",
    "opt+cmd+]",
    "cmd+s",
    "cmd+f",
    "cmd+g",
    "shift+cmd+g",
    "opt+cmd+g",
    "cmd+d",
    "ctrl+shift+up",
    "ctrl+shift+down",
    "cmd+/",
    "f12",
    "cmd+down",
    "opt+cmd+n",
    "shift+cmd+n",
    "enter",
    "cmd+backspace",
    "cmd+x",
    "cmd+c",
    "cmd+v",
    "opt+cmd+c",
    "opt+shift+cmd+c",
    "opt+cmd+r",
    "opt+cmd+w",
    "opt+cmd+z",
];

fn canonical(mods: &[&str], key: &str) -> String {
    let mut out: Vec<&str> = MOD_ORDER
        .iter()
        .copied()
        .filter(|m| mods.contains(m))
        .collect();
    out.push(key);
    out.join("+")
}

pub fn parse_key_equivalent(s: &str) -> Option<String> {
    let raw = s.trim();
    if raw.is_empty() {
        return None;
    }
    let parts: Vec<&str> = raw.split('+').collect();
    let mut mods: Vec<&str> = Vec::new();
    let mut key = String::new();
    for (i, part) in parts.iter().enumerate() {
        let tok = part.trim().to_lowercase();
        if tok.is_empty() {
            return None;
        }
        if let Some(m) = known_mod(&tok) {
            if !mods.contains(&m) {
                mods.push(m);
            }
            continue;
        }
        if i != parts.len() - 1 {
            return None;
        }
        let tok = key_alias(&tok).to_string();
        if !known_key(&tok) {
            return None;
        }
        key = tok;
    }
    if key.is_empty() {
        return None;
    }
    Some(canonical(&mods, &key))
}

pub fn is_reserved_app_chord(canonical: &str) -> bool {
    RESERVED_APP_CHORDS.contains(&canonical)
}

fn key_from_keystroke(k: &Keystroke) -> Option<String> {
    let m = &k.modifiers;
    let with_mod = m.control || m.platform || m.alt;
    let typed = k.key_char.as_deref();
    if typed == Some("<") || (k.key == "," && m.shift && with_mod) {
        return Some("<".into());
    }
    if typed == Some(">") || (k.key == "." && m.shift && with_mod) {
        return Some(">".into());
    }
    let key = k.key.as_str();
    let named = match key {
        "enter" | "escape" | "tab" | "space" | "up" | "down" | "left" | "right" | "home"
        | "end" | "pageup" | "pagedown" | "delete" | "backspace" => Some(key),
        _ => None,
    };
    if let Some(n) = named {
        return Some(n.into());
    }
    if fn_key_shape(key) {
        return Some(key.into());
    }
    let lower = key.to_lowercase();
    if lower.chars().count() == 1 && known_key(&lower) {
        return Some(lower);
    }
    None
}

/// A GPUI keystroke as a canonical chord, or `None` when it is typing
/// rather than a chord (`chordFromKeyboardEvent`).
pub fn chord_from_keystroke(k: &Keystroke) -> Option<String> {
    let mut mods: Vec<&str> = Vec::new();
    if k.modifiers.control {
        mods.push("ctrl");
    }
    if k.modifiers.alt {
        mods.push("opt");
    }
    if k.modifiers.shift {
        mods.push("shift");
    }
    if k.modifiers.platform {
        mods.push("cmd");
    }
    let key = key_from_keystroke(k)?;
    if (key == "<" || key == ">") && mods.contains(&"shift") {
        mods.retain(|m| *m != "shift");
        if mods.is_empty() {
            return None;
        }
    }
    if mods == ["shift"]
        && key.chars().count() == 1
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return None;
    }
    Some(canonical(&mods, &key))
}

/// Glyph form for palette and menus (`⇧⌘U`).
pub fn format_key_equivalent_display(canonical: &str) -> String {
    if canonical.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for p in canonical.split('+') {
        match p {
            "cmd" => out.push('⌘'),
            "opt" => out.push('⌥'),
            "shift" => out.push('⇧'),
            "ctrl" => out.push('⌃'),
            "fn" => out.push_str("fn"),
            "escape" => out.push_str("esc"),
            "enter" => out.push('↵'),
            "backspace" => out.push('⌫'),
            "delete" => out.push('⌦'),
            "up" => out.push('↑'),
            "down" => out.push('↓'),
            "left" => out.push('←'),
            "right" => out.push('→'),
            "tab" => out.push('⇥'),
            "space" => out.push('␣'),
            _ if p.chars().count() == 1 || fn_key_shape(p) => out.push_str(&p.to_uppercase()),
            _ => out.push_str(p),
        }
    }
    out
}

/// The binding a command gets: parsed and not reserved, else empty.
pub fn effective_key_equivalent(raw: Option<&str>) -> String {
    let Some(raw) = raw.filter(|r| !r.trim().is_empty()) else {
        return String::new();
    };
    match parse_key_equivalent(raw) {
        Some(c) if !is_reserved_app_chord(&c) => c,
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ks(key: &'static str, key_char: Option<&str>, ctrl: bool, shift: bool) -> Keystroke {
        let mut k = Keystroke::parse(key).unwrap();
        k.key_char = key_char.map(Into::into);
        k.modifiers.control = ctrl;
        k.modifiers.shift = shift;
        k
    }

    #[test]
    fn reserved_chords() {
        for c in [
            "cmd+l",
            "cmd+r",
            "cmd+o",
            "ctrl+shift+up",
            "opt+cmd+down",
            "shift+cmd+v",
            "opt+cmd+[",
            "opt+cmd+z",
        ] {
            assert!(is_reserved_app_chord(c), "{c}");
        }
        assert!(!is_reserved_app_chord("ctrl+opt+up"));
    }

    #[test]
    fn parses_and_formats() {
        assert_eq!(parse_key_equivalent("cmd+l").as_deref(), Some("cmd+l"));
        assert_eq!(parse_key_equivalent("command+L").as_deref(), Some("cmd+l"));
        assert_eq!(
            parse_key_equivalent("shift+cmd+u").as_deref(),
            Some("shift+cmd+u")
        );
        assert_eq!(
            parse_key_equivalent("cmd+shift+u").as_deref(),
            Some("shift+cmd+u")
        );
        assert_eq!(parse_key_equivalent("ctrl+<").as_deref(), Some("ctrl+<"));
        assert_eq!(parse_key_equivalent("cmd+f13"), None);
        assert_eq!(parse_key_equivalent("cmd+x+y"), None);
        assert_eq!(format_key_equivalent_display("ctrl+<"), "⌃<");
        assert_eq!(format_key_equivalent_display("shift+cmd+u"), "⇧⌘U");
        assert_eq!(format_key_equivalent_display("f12"), "F12");
        assert_eq!(effective_key_equivalent(Some("cmd+s")), "");
        assert_eq!(effective_key_equivalent(Some("ctrl+opt+u")), "ctrl+opt+u");
    }

    #[test]
    fn keystroke_chords() {
        assert_eq!(
            chord_from_keystroke(&ks(",", Some("<"), true, true)).as_deref(),
            Some("ctrl+<")
        );
        assert_eq!(chord_from_keystroke(&ks(",", Some("<"), false, true)), None);
        assert_eq!(chord_from_keystroke(&ks("a", Some("A"), false, true)), None);
        assert_eq!(
            chord_from_keystroke(&ks("enter", None, true, false)).as_deref(),
            Some("ctrl+enter")
        );
    }
}
